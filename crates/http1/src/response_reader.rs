use std::error::Error as StdError;
use std::fmt;
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use hyper::ext::{BodyProgress, ResponseFraming, TransportRead};

use crate::http_util;
use crate::net_error::NetError;
use crate::response_headers::{HttpResponseHeaders, HttpVersion};
use crate::stream_parser::{Socket, SocketLiveness, StreamParser, IO_PENDING, OK};
use crate::request_url::RequestUrl;

const BODY_READ_SIZE: usize = 32 * 1024;
const DRAIN_BODY_BUFFER_SIZE: usize = 16384;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResponseError {
    Net(NetError),
    Misdirected,
}

impl fmt::Display for ResponseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResponseError::Net(error) => error.fmt(f),
            ResponseError::Misdirected => f.write_str("421 response to be retried on a new connection"),
        }
    }
}

impl StdError for ResponseError {}

#[derive(Clone, Debug)]
pub struct ChromiumResponse(pub HttpResponseHeaders);

#[derive(Clone, Copy, Debug)]
pub struct RejectedResponse(pub NetError);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Misdirection {
    Retry,
    Accept,
}

#[derive(Debug, Default)]
pub struct ConnectionVerdict(Mutex<Option<bool>>);

impl ConnectionVerdict {
    pub fn reusable(&self) -> Option<bool> {
        *self.0.lock().expect("verdict lock")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    StartHeaders,
    Headers,
    Body,
    BodyPending,
    Drain { total_read: usize, pending: bool },
    Finished,
}

enum HeadersOutcome {
    ReadAgain,
    Final,
    Failed(ResponseError),
}

enum Disposal {
    Verdict(bool),
    Drain,
}

pub struct ResponseReader {
    parser: StreamParser,
    method_is_put: bool,
    accept_encoding: Option<Vec<u8>>,
    misdirection: Misdirection,
    phase: Phase,
    verdict: Arc<ConnectionVerdict>,
}

impl ResponseReader {
    pub fn new(
        url: &RequestUrl,
        method: &[u8],
        accept_encoding: Option<Vec<u8>>,
        misdirection: Misdirection,
        liveness: Arc<dyn SocketLiveness>,
        verdict: Arc<ConnectionVerdict>,
    ) -> Self {
        ResponseReader {
            parser: StreamParser::new(url, method, Socket::new(liveness)),
            method_is_put: method == b"PUT",
            accept_encoding,
            misdirection,
            phase: Phase::StartHeaders,
            verdict,
        }
    }

    fn content_encodings_valid(&self, headers: &HttpResponseHeaders) -> bool {
        let Some(allowed) = http_util::parse_accept_encoding(self.accept_encoding.as_deref().unwrap_or_default()) else { return false };
        let content_encoding = headers.normalized_header(b"Content-Encoding").unwrap_or_default();
        let Some(used) = http_util::parse_content_encoding(&content_encoding) else { return false };
        if allowed.contains(&b"*"[..]) {
            return true;
        }
        let known = |encoding: &[u8]| matches!(encoding, b"" | b"br" | b"deflate" | b"gzip" | b"x-gzip" | b"zstd");
        let valid = used.iter().all(|encoding| !known(encoding) || allowed.contains(encoding));
        valid || headers.is_redirect()
    }

    fn headers_complete(&mut self, result: i64) -> HeadersOutcome {
        let mut result = result;
        if result == NetError::ConnectionClosed.code() && self.parser.headers.is_some() {
            result = OK;
        }
        if result < 0 {
            return HeadersOutcome::Failed(ResponseError::Net(NetError::from_code(result).expect("header reads fail with net errors")));
        }
        let headers = self.parser.headers.as_ref().expect("a successful header read has headers");
        let code = headers.response_code();
        if code == 103 {
            self.parser.headers = Some(HttpResponseHeaders::from_status_line(b""));
            return HeadersOutcome::ReadAgain;
        }
        if !self.content_encodings_valid(headers) {
            return HeadersOutcome::Failed(ResponseError::Net(NetError::ContentDecodingFailed));
        }
        if headers.version() < HttpVersion::new(1, 0) && self.method_is_put {
            return HeadersOutcome::Failed(ResponseError::Net(NetError::MethodNotSupported));
        }
        if code / 100 == 1 {
            self.parser.headers = Some(HttpResponseHeaders::from_status_line(b""));
            return HeadersOutcome::ReadAgain;
        }
        if code == 421 && self.misdirection == Misdirection::Retry {
            return HeadersOutcome::Failed(ResponseError::Misdirected);
        }
        if code == 407 {
            return HeadersOutcome::Failed(ResponseError::Net(NetError::UnexpectedProxyAuth));
        }
        HeadersOutcome::Final
    }

    fn disposal(&self) -> Disposal {
        if !self.parser.can_reuse_connection() {
            Disposal::Verdict(false)
        } else if self.parser.is_response_body_complete() {
            Disposal::Verdict(self.parser.socket.is_connected_and_idle())
        } else {
            Disposal::Drain
        }
    }

    fn poll_drain(&mut self, mut total_read: usize, mut pending: bool) -> BodyProgress {
        loop {
            let result = if pending {
                match self.parser.on_io_complete() {
                    Some(result) => result,
                    None => {
                        self.phase = Phase::Drain { total_read, pending };
                        return BodyProgress::NeedRead;
                    }
                }
            } else {
                self.parser.read_response_body(DRAIN_BODY_BUFFER_SIZE - total_read)
            };
            if result == IO_PENDING {
                pending = true;
                continue;
            }
            pending = false;
            let reusable = if result < 0 {
                Some(false)
            } else {
                total_read += result as usize;
                if self.parser.is_response_body_complete() {
                    Some(self.parser.can_reuse_connection() && self.parser.socket.is_connected_and_idle())
                } else if total_read >= DRAIN_BODY_BUFFER_SIZE || result == 0 {
                    Some(false)
                } else {
                    None
                }
            };
            if let Some(reusable) = reusable {
                self.finish(reusable);
                return BodyProgress::End { reusable };
            }
        }
    }

    fn head(&self) -> http::Response<()> {
        let headers = self.parser.headers.clone().expect("a complete head has headers");
        let mut response = http::Response::new(());
        *response.version_mut() = match headers.version() {
            HttpVersion { major: 0, minor: 9 } => http::Version::HTTP_09,
            HttpVersion { major: 1, minor: 0 } => http::Version::HTTP_10,
            _ => http::Version::HTTP_11,
        };
        response.extensions_mut().insert(ChromiumResponse(headers));
        response
    }

    fn finish(&mut self, reusable: bool) {
        self.phase = Phase::Finished;
        *self.verdict.0.lock().expect("verdict lock") = Some(reusable);
    }
}

impl ResponseFraming for ResponseReader {
    fn receive(&mut self, read: TransportRead<'_>) {
        match read {
            TransportRead::Data(data) => self.parser.socket.receive_data(data),
            TransportRead::Closed => self.parser.socket.receive_close(),
            TransportRead::Failed(error) => self.parser.socket.receive_failure(NetError::from_io(error)),
        }
    }

    fn poll_head(&mut self) -> Result<Option<http::Response<()>>, Box<dyn StdError + Send + Sync>> {
        loop {
            let result = match self.phase {
                Phase::StartHeaders => {
                    self.phase = Phase::Headers;
                    self.parser.read_response_headers()
                }
                Phase::Headers if self.parser.has_pending_read() => match self.parser.on_io_complete() {
                    Some(result) => result,
                    None => return Ok(None),
                },
                Phase::Headers | Phase::Body | Phase::BodyPending | Phase::Drain { .. } | Phase::Finished => unreachable!("the head is read once"),
            };
            if result == IO_PENDING {
                return Ok(None);
            }
            match self.headers_complete(result) {
                HeadersOutcome::Final => {
                    self.phase = Phase::Body;
                    return Ok(Some(self.head()));
                }
                HeadersOutcome::ReadAgain => self.phase = Phase::StartHeaders,
                HeadersOutcome::Failed(ResponseError::Misdirected) => {
                    self.finish(false);
                    return Err(Box::new(ResponseError::Misdirected));
                }
                HeadersOutcome::Failed(ResponseError::Net(error)) => match self.disposal() {
                    Disposal::Verdict(reusable) => {
                        self.finish(reusable);
                        return Err(Box::new(ResponseError::Net(error)));
                    }
                    Disposal::Drain => {
                        self.phase = Phase::Drain { total_read: 0, pending: false };
                        let mut head = self.head();
                        head.extensions_mut().insert(RejectedResponse(error));
                        return Ok(Some(head));
                    }
                },
            }
        }
    }

    fn poll_body(&mut self) -> Result<BodyProgress, Box<dyn StdError + Send + Sync>> {
        let result = match self.phase {
            Phase::Drain { total_read, pending } => return Ok(self.poll_drain(total_read, pending)),
            Phase::Body => self.parser.read_response_body(BODY_READ_SIZE),
            Phase::BodyPending => match self.parser.on_io_complete() {
                Some(result) => result,
                None => return Ok(BodyProgress::NeedRead),
            },
            Phase::StartHeaders | Phase::Headers | Phase::Finished => unreachable!("the body follows its head"),
        };
        if result == IO_PENDING {
            self.phase = Phase::BodyPending;
            return Ok(BodyProgress::NeedRead);
        }
        self.phase = Phase::Body;
        if result > 0 {
            return Ok(BodyProgress::Data(Bytes::copy_from_slice(self.parser.body_bytes(result as usize))));
        }
        if result < 0 {
            self.finish(false);
            return Err(Box::new(ResponseError::Net(NetError::from_code(result).expect("body reads fail with net errors"))));
        }
        let keep_alive = self.parser.is_response_body_complete() && self.parser.can_reuse_connection();
        let reusable = keep_alive && self.parser.socket.is_connected_and_idle();
        self.finish(reusable);
        Ok(BodyProgress::End { reusable })
    }
}

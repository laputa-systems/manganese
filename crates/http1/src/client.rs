use std::collections::VecDeque;
use std::convert::Infallible;
use std::error::Error as StdError;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::Bytes;
use h12tiny_core::io::FuturesIo;
use http_body::{Body, Frame, SizeHint};
use hyper::ext::{Http1RequestBody, Http1RequestHead};

use crate::net_error::NetError;
use crate::request::{self, BodyFraming, InvalidRequest, RequestInfo, WireRequest};
use crate::request_url::RequestUrl;
use crate::response_headers::HttpResponseHeaders;
use crate::response_reader::{ChromiumResponse, ConnectionVerdict, Misdirection, RejectedResponse, ResponseError, ResponseReader};
use crate::stream_parser::SocketLiveness;

const TRANSPORT_READ_SIZE: usize = 64 * 1024;

pub struct RequestBody {
    frames: VecDeque<Bytes>,
}

impl Body for RequestBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
        Poll::Ready(self.frames.pop_front().map(|frame| Ok(Frame::data(frame))))
    }

    fn is_end_stream(&self) -> bool {
        self.frames.is_empty()
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::with_exact(self.frames.iter().map(|frame| frame.len() as u64).sum())
    }
}

#[derive(Clone, Debug)]
pub struct PreparedRequest {
    url: RequestUrl,
    method: Vec<u8>,
    wire: WireRequest,
}

impl PreparedRequest {
    pub fn new(info: &RequestInfo) -> Result<Self, InvalidRequest> {
        Ok(PreparedRequest { url: info.url.clone(), method: info.method.clone(), wire: request::build(info)? })
    }

    pub fn wire_bytes(&self) -> Vec<u8> {
        self.wire.wire_bytes()
    }

    fn hyper_request(&self, misdirection: Misdirection, liveness: Arc<dyn SocketLiveness>, verdict: Arc<ConnectionVerdict>) -> http::Request<RequestBody> {
        let (framing, frames) = match &self.wire.body {
            BodyFraming::Empty => (Http1RequestBody::Empty, VecDeque::new()),
            BodyFraming::Length(body) => (Http1RequestBody::Length(body.len() as u64), VecDeque::from([Bytes::copy_from_slice(body)])),
            BodyFraming::Chunked(chunks) => (Http1RequestBody::Chunked, chunks.iter().map(|chunk| Bytes::copy_from_slice(chunk)).collect()),
        };
        let mut request = http::Request::new(RequestBody { frames });
        request.extensions_mut().insert(Http1RequestHead::new(Bytes::copy_from_slice(&self.wire.head), framing));
        let url = self.url.clone();
        let method = self.method.clone();
        let accept_encoding = self.wire.headers.get(b"Accept-Encoding").map(<[u8]>::to_vec);
        hyper::ext::frame_response(&mut request, move || {
            Box::new(ResponseReader::new(&url, &method, accept_encoding.clone(), misdirection, liveness.clone(), verdict.clone()))
        });
        request
    }
}

pub struct Connected<S> {
    pub stream: S,
    pub liveness: Arc<dyn SocketLiveness>,
    pub verdict: Arc<ConnectionVerdict>,
}

#[derive(Debug)]
pub enum SendError {
    Net(NetError),
    Transport(hyper::Error),
}

fn response_error(error: &(dyn StdError + 'static)) -> Option<ResponseError> {
    let mut source = Some(error);
    while let Some(error) = source {
        if let Some(response) = error.downcast_ref::<ResponseError>() {
            return Some(*response);
        }
        if let Some(io) = error.downcast_ref::<std::io::Error>()
            && let Some(response) = io.get_ref().and_then(|inner| inner.downcast_ref::<ResponseError>())
        {
            return Some(*response);
        }
        source = error.source();
    }
    None
}

pub struct Response {
    pub headers: HttpResponseHeaders,
    body: hyper::body::Incoming,
}

impl Response {
    pub async fn next_chunk(&mut self) -> Option<Result<Bytes, SendError>> {
        loop {
            let frame = std::future::poll_fn(|cx| Pin::new(&mut self.body).poll_frame(cx)).await?;
            match frame {
                Ok(frame) => match frame.into_data() {
                    Ok(data) => return Some(Ok(data)),
                    Err(_) => continue,
                },
                Err(error) => {
                    return Some(Err(match response_error(&error) {
                        Some(ResponseError::Net(net)) => SendError::Net(net),
                        _ => SendError::Transport(error),
                    }));
                }
            }
        }
    }
}

pub type Spawn<'a> = &'a dyn Fn(Pin<Box<dyn Future<Output = ()>>>);

pub async fn send<S, D>(request: &PreparedRequest, mut dial: D, spawn: Spawn<'_>) -> Result<Response, SendError>
where
    S: futures_io::AsyncRead + futures_io::AsyncWrite + Unpin + 'static,
    D: FnMut() -> Connected<S>,
{
    let mut misdirection = Misdirection::Retry;
    loop {
        let connected = dial();
        let (mut sender, connection) = hyper::client::conn::http1::Builder::new()
            .read_buf_exact_size(Some(TRANSPORT_READ_SIZE))
            .handshake::<_, RequestBody>(FuturesIo::new(connected.stream))
            .await
            .map_err(SendError::Transport)?;
        spawn(Box::pin(async move {
            let _ = connection.await;
        }));
        match sender.send_request(request.hyper_request(misdirection, connected.liveness, connected.verdict)).await {
            Ok(response) => {
                let (parts, mut body) = response.into_parts();
                if let Some(RejectedResponse(error)) = parts.extensions.get::<RejectedResponse>().copied() {
                    spawn(Box::pin(async move { while let Some(Ok(_)) = std::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx)).await {} }));
                    return Err(SendError::Net(error));
                }
                let ChromiumResponse(headers) = parts.extensions.get::<ChromiumResponse>().cloned().expect("framed responses carry their Chromium headers");
                return Ok(Response { headers, body });
            }
            Err(error) => match response_error(&error) {
                Some(ResponseError::Misdirected) => misdirection = Misdirection::Accept,
                Some(ResponseError::Net(net)) => return Err(SendError::Net(net)),
                None => return Err(SendError::Transport(error)),
            },
        }
    }
}

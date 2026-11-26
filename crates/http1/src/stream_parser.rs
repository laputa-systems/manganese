use std::collections::VecDeque;
use std::sync::Arc;

use crate::chunked_decoder::HttpChunkedDecoder;
use crate::http_util;
use crate::net_error::NetError;
use crate::response_headers::HttpResponseHeaders;
use crate::request_url::{RequestUrl, Scheme};

const HEADER_BUF_INITIAL_SIZE: usize = 4 * 1024;
const MAX_HEADER_BUF_SIZE: usize = HEADER_BUF_INITIAL_SIZE * 64;

pub(crate) const OK: i64 = 0;
pub(crate) const IO_PENDING: i64 = -1;

pub trait SocketLiveness: Send + Sync {
    fn has_undelivered_data(&self) -> bool;
    fn is_peer_open(&self) -> bool;
}

enum SocketEnd {
    Closed,
    Failed(NetError),
}

pub(crate) struct Socket {
    arrivals: VecDeque<Vec<u8>>,
    offset: usize,
    end: Option<SocketEnd>,
    liveness: Arc<dyn SocketLiveness>,
}

impl Socket {
    pub(crate) fn new(liveness: Arc<dyn SocketLiveness>) -> Self {
        Socket { arrivals: VecDeque::new(), offset: 0, end: None, liveness }
    }

    pub(crate) fn receive_data(&mut self, data: &[u8]) {
        if !data.is_empty() {
            self.arrivals.push_back(data.to_vec());
        }
    }

    pub(crate) fn receive_close(&mut self) {
        self.end.get_or_insert(SocketEnd::Closed);
    }

    pub(crate) fn receive_failure(&mut self, error: NetError) {
        self.end.get_or_insert(SocketEnd::Failed(error));
    }

    fn read(&mut self, into: &mut [u8]) -> i64 {
        if let Some(front) = self.arrivals.front() {
            let size = into.len().min(front.len() - self.offset);
            into[..size].copy_from_slice(&front[self.offset..self.offset + size]);
            self.offset += size;
            if self.offset == front.len() {
                self.arrivals.pop_front();
                self.offset = 0;
            }
            return size as i64;
        }
        match self.end {
            Some(SocketEnd::Closed) => 0,
            Some(SocketEnd::Failed(error)) => error.code(),
            None => IO_PENDING,
        }
    }

    fn has_unread_data(&self) -> bool {
        !self.arrivals.is_empty() || self.liveness.has_undelivered_data()
    }

    pub(crate) fn is_connected(&self) -> bool {
        self.has_unread_data() || (self.end.is_none() && self.liveness.is_peer_open())
    }

    pub(crate) fn is_connected_and_idle(&self) -> bool {
        self.is_connected() && !self.has_unread_data()
    }
}

struct GrowableBuffer {
    data: Vec<u8>,
    offset: usize,
}

impl GrowableBuffer {
    fn capacity(&self) -> usize {
        self.data.len()
    }

    fn remaining_capacity(&self) -> usize {
        self.data.len() - self.offset
    }

    fn set_capacity(&mut self, capacity: usize) {
        self.data.resize(capacity, 0);
        self.offset = self.offset.min(capacity);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IoState {
    None,
    ReadHeaders,
    ReadHeadersComplete,
    ReadBody,
    ReadBodyComplete,
    Done,
}

pub(crate) struct StreamParser {
    io_state: IoState,
    read_buf: GrowableBuffer,
    read_buf_unused_offset: usize,
    response_header_start_offset: Option<usize>,
    response_body_length: i64,
    response_body_read: i64,
    chunked_decoder: Option<HttpChunkedDecoder>,
    user_read_buf: Vec<u8>,
    has_seen_status_line: bool,
    response_is_keep_alive: bool,
    discarded_extra_data: bool,
    cryptographic: bool,
    scheme: Scheme,
    default_port: bool,
    method_is_head: bool,
    pub(crate) socket: Socket,
    pub(crate) headers: Option<HttpResponseHeaders>,
}

impl StreamParser {
    pub(crate) fn new(url: &RequestUrl, method: &[u8], socket: Socket) -> Self {
        StreamParser {
            io_state: IoState::None,
            read_buf: GrowableBuffer { data: Vec::new(), offset: 0 },
            read_buf_unused_offset: 0,
            response_header_start_offset: None,
            response_body_length: -1,
            response_body_read: 0,
            chunked_decoder: None,
            user_read_buf: Vec::new(),
            has_seen_status_line: false,
            response_is_keep_alive: false,
            discarded_extra_data: false,
            cryptographic: url.is_cryptographic(),
            scheme: url.scheme(),
            default_port: url.uses_default_port(),
            method_is_head: method == b"HEAD",
            socket,
            headers: None,
        }
    }

    pub(crate) fn read_response_headers(&mut self) -> i64 {
        if self.io_state == IoState::Done {
            return NetError::ConnectionClosed.code();
        }
        let mut result = OK;
        self.io_state = IoState::ReadHeaders;
        if self.read_buf.offset > 0 {
            result = self.read_buf.offset as i64;
            self.read_buf.offset = 0;
        }
        if result > 0 {
            self.io_state = IoState::ReadHeadersComplete;
        }
        let result = self.do_loop(result);
        if result > 0 { OK } else { result }
    }

    pub(crate) fn read_response_body(&mut self, len: usize) -> i64 {
        if self.io_state == IoState::Done {
            return OK;
        }
        self.user_read_buf = vec![0; len];
        self.io_state = IoState::ReadBody;
        self.do_loop(OK)
    }

    pub(crate) fn has_pending_read(&self) -> bool {
        matches!(self.io_state, IoState::ReadHeadersComplete | IoState::ReadBodyComplete)
    }

    pub(crate) fn on_io_complete(&mut self) -> Option<i64> {
        let result = match self.io_state {
            IoState::ReadHeadersComplete => {
                let offset = self.read_buf.offset;
                self.socket.read(&mut self.read_buf.data[offset..])
            }
            IoState::ReadBodyComplete => self.socket.read(&mut self.user_read_buf),
            _ => return None,
        };
        if result == IO_PENDING {
            return None;
        }
        let result = self.do_loop(result);
        (result != IO_PENDING).then_some(result)
    }

    pub(crate) fn body_bytes(&self, len: usize) -> &[u8] {
        &self.user_read_buf[..len]
    }

    fn do_loop(&mut self, mut result: i64) -> i64 {
        loop {
            let state = self.io_state;
            self.io_state = IoState::None;
            result = match state {
                IoState::ReadHeaders => self.do_read_headers(),
                IoState::ReadHeadersComplete => self.do_read_headers_complete(result),
                IoState::ReadBody => self.do_read_body(),
                IoState::ReadBodyComplete => self.do_read_body_complete(result),
                IoState::None | IoState::Done => unreachable!("DoLoop runs only from a reading state"),
            };
            if result == IO_PENDING || matches!(self.io_state, IoState::Done | IoState::None) {
                return result;
            }
        }
    }

    fn do_read_headers(&mut self) -> i64 {
        self.io_state = IoState::ReadHeadersComplete;
        if self.read_buf.remaining_capacity() == 0 {
            let capacity = self.read_buf.capacity() + HEADER_BUF_INITIAL_SIZE;
            self.read_buf.set_capacity(capacity);
        }
        let offset = self.read_buf.offset;
        self.socket.read(&mut self.read_buf.data[offset..])
    }

    fn do_read_headers_complete(&mut self, result: i64) -> i64 {
        self.handle_read_header_result(result)
    }

    fn handle_read_header_result(&mut self, mut result: i64) -> i64 {
        if result == 0 {
            result = NetError::ConnectionClosed.code();
        }
        if result == NetError::ConnectionClosed.code() {
            if self.read_buf.offset == 0 {
                self.io_state = IoState::Done;
                return NetError::EmptyResponse.code();
            }
            if self.cryptographic {
                self.io_state = IoState::Done;
                return NetError::ResponseHeadersTruncated.code();
            }
            let end_offset = if self.response_header_start_offset.is_some() {
                self.io_state = IoState::ReadBodyComplete;
                self.read_buf.offset
            } else {
                0
            };
            let parsed = self.parse_response_headers(end_offset);
            if parsed < 0 {
                return parsed;
            }
            return result;
        }
        if result < 0 {
            self.io_state = IoState::Done;
            return result;
        }
        self.read_buf.offset += result as usize;
        let end_of_header_offset = self.find_and_parse_response_headers(result as usize);
        if end_of_header_offset < -1 {
            return end_of_header_offset;
        }
        if end_of_header_offset == -1 {
            self.io_state = IoState::ReadHeaders;
            if self.read_buf.offset >= MAX_HEADER_BUF_SIZE {
                self.io_state = IoState::Done;
                return NetError::ResponseHeadersTooBig.code();
            }
            return OK;
        }
        self.calculate_response_body_size();
        let end_of_header_offset = end_of_header_offset as usize;
        let headers = self.headers.as_ref().expect("parsed headers are set");
        let informational = headers.response_code() / 100 == 1;
        if self.response_body_length == 0 {
            let extra = self.read_buf.data[end_of_header_offset..self.read_buf.offset].to_vec();
            self.read_buf.data[..extra.len()].copy_from_slice(&extra);
            self.read_buf.set_capacity(extra.len());
            if informational {
                self.response_header_start_offset = None;
                self.response_body_length = -1;
            } else {
                self.response_is_keep_alive = self.headers.as_ref().is_some_and(HttpResponseHeaders::is_keep_alive);
                self.io_state = IoState::Done;
            }
            return OK;
        }
        self.response_is_keep_alive = headers.is_keep_alive();
        self.read_buf_unused_offset = end_of_header_offset;
        OK
    }

    fn find_and_parse_response_headers(&mut self, new_bytes: usize) -> i64 {
        if self.response_header_start_offset.is_none() {
            self.response_header_start_offset = http_util::locate_start_of_status_line(&self.read_buf.data[..self.read_buf.offset]);
        }
        let end_offset = if let Some(start) = self.response_header_start_offset {
            let lower_bound = self.read_buf.offset.saturating_sub(new_bytes).saturating_sub(3);
            http_util::locate_end_of_headers(&self.read_buf.data[..self.read_buf.offset], start.max(lower_bound))
        } else if self.read_buf.offset >= 8 {
            Some(0)
        } else {
            None
        };
        let Some(end_offset) = end_offset else { return -1 };
        let parsed = self.parse_response_headers(end_offset);
        if parsed < 0 {
            return parsed;
        }
        end_offset as i64
    }

    fn parse_response_headers(&mut self, end_offset: usize) -> i64 {
        let headers = if self.response_header_start_offset.is_some() {
            let Some(headers) = HttpResponseHeaders::try_to_create(&self.read_buf.data[..end_offset]) else {
                return NetError::InvalidHttpResponse.code();
            };
            self.has_seen_status_line = true;
            headers
        } else {
            if self.has_seen_status_line {
                return NetError::InvalidHttpResponse.code();
            }
            if !self.default_port {
                let shoutcast = self.read_buf.offset >= 3 && self.scheme == Scheme::Http && self.read_buf.data[..3].eq_ignore_ascii_case(b"icy");
                if !shoutcast {
                    return NetError::InvalidHttpResponse.code();
                }
            }
            HttpResponseHeaders::from_status_line(b"HTTP/0.9 200 OK")
        };
        if !headers.is_chunk_encoded() && headers.contains_multiple_copies_of_field(b"Content-Length") {
            return NetError::ResponseHeadersMultipleContentLength.code();
        }
        if headers.contains_multiple_copies_of_field(b"Content-Disposition") {
            return NetError::ResponseHeadersMultipleContentDisposition.code();
        }
        if headers.contains_multiple_copies_of_field(b"Location") {
            return NetError::ResponseHeadersMultipleLocation.code();
        }
        self.headers = Some(headers);
        OK
    }

    fn calculate_response_body_size(&mut self) {
        let headers = self.headers.as_ref().expect("body size follows parsed headers");
        if headers.response_code() / 100 == 1 || matches!(headers.response_code(), 204 | 205 | 304) {
            self.response_body_length = 0;
        }
        if self.method_is_head {
            self.response_body_length = 0;
        }
        if self.response_body_length == -1 {
            if headers.is_chunk_encoded() {
                self.chunked_decoder = Some(HttpChunkedDecoder::default());
            } else {
                self.response_body_length = headers.content_length().unwrap_or(-1);
            }
        }
    }

    fn do_read_body(&mut self) -> i64 {
        self.io_state = IoState::ReadBodyComplete;
        let mut remaining_read_len = self.user_read_buf.len() as u64;
        let mut remaining_body = 0u64;
        if self.chunked_decoder.is_none() && self.response_body_length >= 0 {
            remaining_body = (self.response_body_length - self.response_body_read) as u64;
            remaining_read_len = remaining_read_len.min(remaining_body);
        }
        if self.read_buf.offset > 0 {
            let available = self.read_buf.offset - self.read_buf_unused_offset;
            if available > 0 {
                let bytes_from_buffer = (available as u64).min(remaining_read_len) as usize;
                let unused = self.read_buf_unused_offset;
                self.user_read_buf[..bytes_from_buffer].copy_from_slice(&self.read_buf.data[unused..unused + bytes_from_buffer]);
                self.read_buf_unused_offset += bytes_from_buffer;
                if remaining_body == bytes_from_buffer as u64 && available > bytes_from_buffer {
                    self.read_buf.set_capacity(0);
                    self.read_buf_unused_offset = 0;
                    self.discarded_extra_data = true;
                } else if bytes_from_buffer == available {
                    self.read_buf.set_capacity(0);
                    self.read_buf_unused_offset = 0;
                }
                return bytes_from_buffer as i64;
            }
            self.read_buf.set_capacity(0);
            self.read_buf_unused_offset = 0;
        }
        if self.is_response_body_complete() {
            return 0;
        }
        self.socket.read(&mut self.user_read_buf)
    }

    fn do_read_body_complete(&mut self, mut result: i64) -> i64 {
        if self.chunked_decoder.is_none() && self.response_body_length >= 0 {
            let remaining_body = (self.response_body_length - self.response_body_read) as u64;
            let remaining_read_len = (self.user_read_buf.len() as u64).min(remaining_body);
            if result > 0 && result as u64 > remaining_read_len {
                result = remaining_read_len as i64;
                self.discarded_extra_data = true;
            }
        }
        if result == 0 && !self.is_response_body_complete() && self.can_find_end_of_response() {
            result = if self.chunked_decoder.is_some() { NetError::IncompleteChunkedEncoding.code() } else { NetError::ContentLengthMismatch.code() };
        }
        if result > 0
            && let Some(decoder) = &mut self.chunked_decoder
        {
            result = match decoder.filter_buf(&mut self.user_read_buf[..result as usize]) {
                Ok(decoded) => decoded as i64,
                Err(error) => error.code(),
            };
            if result == 0 && !decoder.reached_eof() {
                self.io_state = IoState::ReadBody;
                return OK;
            }
        }
        if result > 0 {
            self.response_body_read += result;
        }
        if result <= 0 || self.is_response_body_complete() {
            self.io_state = IoState::Done;
            let additional_save_amount = self.read_buf.offset - self.read_buf_unused_offset;
            let mut save_amount = 0usize;
            if let Some(decoder) = &self.chunked_decoder {
                save_amount = decoder.bytes_after_eof();
            } else if self.response_body_length >= 0 {
                let extra_data_read = self.response_body_read - self.response_body_length;
                if extra_data_read > 0 {
                    save_amount = extra_data_read as usize;
                    if result > 0 {
                        result -= save_amount as i64;
                    }
                }
            }
            let new_capacity = save_amount + additional_save_amount;
            if self.read_buf.capacity() < new_capacity {
                self.read_buf.set_capacity(new_capacity);
            }
            if save_amount > 0 {
                let from = result.max(0) as usize;
                let saved = self.user_read_buf[from..from + save_amount].to_vec();
                self.read_buf.data[..save_amount].copy_from_slice(&saved);
            }
            self.read_buf.offset = save_amount;
            if additional_save_amount > 0 {
                let unused = self.read_buf_unused_offset;
                self.read_buf.data.copy_within(unused..unused + additional_save_amount, save_amount);
                self.read_buf.offset = new_capacity;
            }
            self.read_buf_unused_offset = 0;
        }
        result
    }

    pub(crate) fn is_response_body_complete(&self) -> bool {
        if let Some(decoder) = &self.chunked_decoder {
            return decoder.reached_eof();
        }
        self.response_body_length != -1 && self.response_body_read >= self.response_body_length
    }

    fn can_find_end_of_response(&self) -> bool {
        self.chunked_decoder.is_some() || self.response_body_length >= 0
    }

    fn is_more_data_buffered(&self) -> bool {
        self.read_buf.offset > 0 && self.read_buf.offset > self.read_buf_unused_offset
    }

    pub(crate) fn can_reuse_connection(&self) -> bool {
        if !self.can_find_end_of_response() || !self.response_is_keep_alive {
            return false;
        }
        if (self.is_response_body_complete() && self.is_more_data_buffered()) || self.discarded_extra_data {
            return false;
        }
        self.socket.is_connected()
    }
}

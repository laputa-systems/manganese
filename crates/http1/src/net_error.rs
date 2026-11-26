use std::fmt;
use std::io;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetError {
    Failed = -2,
    ConnectionClosed = -100,
    ConnectionReset = -101,
    ConnectionAborted = -103,
    InvalidChunkedEncoding = -321,
    MethodNotSupported = -322,
    UnexpectedProxyAuth = -323,
    EmptyResponse = -324,
    ResponseHeadersTooBig = -325,
    ContentDecodingFailed = -330,
    ResponseHeadersMultipleContentLength = -346,
    ResponseHeadersMultipleContentDisposition = -349,
    ResponseHeadersMultipleLocation = -350,
    ContentLengthMismatch = -354,
    IncompleteChunkedEncoding = -355,
    ResponseHeadersTruncated = -357,
    InvalidHttpResponse = -370,
}

impl NetError {
    pub const ALL: [NetError; 17] = [NetError::Failed, NetError::ConnectionClosed, NetError::ConnectionReset, NetError::ConnectionAborted, NetError::InvalidChunkedEncoding, NetError::MethodNotSupported, NetError::UnexpectedProxyAuth, NetError::EmptyResponse, NetError::ResponseHeadersTooBig, NetError::ContentDecodingFailed, NetError::ResponseHeadersMultipleContentLength, NetError::ResponseHeadersMultipleContentDisposition, NetError::ResponseHeadersMultipleLocation, NetError::ContentLengthMismatch, NetError::IncompleteChunkedEncoding, NetError::ResponseHeadersTruncated, NetError::InvalidHttpResponse];

    pub fn code(self) -> i64 {
        self as i64
    }

    pub fn from_code(code: i64) -> Option<Self> {
        Self::ALL.into_iter().find(|error| error.code() == code)
    }

    pub fn name(self) -> &'static str {
        match self {
            NetError::Failed => "ERR_FAILED",
            NetError::ConnectionClosed => "ERR_CONNECTION_CLOSED",
            NetError::ConnectionReset => "ERR_CONNECTION_RESET",
            NetError::ConnectionAborted => "ERR_CONNECTION_ABORTED",
            NetError::InvalidChunkedEncoding => "ERR_INVALID_CHUNKED_ENCODING",
            NetError::MethodNotSupported => "ERR_METHOD_NOT_SUPPORTED",
            NetError::UnexpectedProxyAuth => "ERR_UNEXPECTED_PROXY_AUTH",
            NetError::EmptyResponse => "ERR_EMPTY_RESPONSE",
            NetError::ResponseHeadersTooBig => "ERR_RESPONSE_HEADERS_TOO_BIG",
            NetError::ContentDecodingFailed => "ERR_CONTENT_DECODING_FAILED",
            NetError::ResponseHeadersMultipleContentLength => "ERR_RESPONSE_HEADERS_MULTIPLE_CONTENT_LENGTH",
            NetError::ResponseHeadersMultipleContentDisposition => "ERR_RESPONSE_HEADERS_MULTIPLE_CONTENT_DISPOSITION",
            NetError::ResponseHeadersMultipleLocation => "ERR_RESPONSE_HEADERS_MULTIPLE_LOCATION",
            NetError::ContentLengthMismatch => "ERR_CONTENT_LENGTH_MISMATCH",
            NetError::IncompleteChunkedEncoding => "ERR_INCOMPLETE_CHUNKED_ENCODING",
            NetError::ResponseHeadersTruncated => "ERR_RESPONSE_HEADERS_TRUNCATED",
            NetError::InvalidHttpResponse => "ERR_INVALID_HTTP_RESPONSE",
        }
    }

    pub fn from_io(error: &io::Error) -> Self {
        match error.kind() {
            io::ErrorKind::ConnectionReset => NetError::ConnectionReset,
            io::ErrorKind::ConnectionAborted => NetError::ConnectionAborted,
            _ => NetError::Failed,
        }
    }
}

impl fmt::Display for NetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl std::error::Error for NetError {}

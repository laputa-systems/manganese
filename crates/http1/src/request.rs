use crate::http_util;
use crate::request_headers::{HttpRequestHeaders, InvalidHeader};
use crate::request_url::RequestUrl;

const CHUNKED_UPLOAD_READ_SIZE: usize = (1 << 14) - 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CacheLoad {
    Normal,
    BypassCache,
    ValidateCache,
}

#[derive(Clone, Debug)]
pub enum UploadBody {
    None,
    Bytes(Vec<u8>),
    Chunked(Vec<Vec<u8>>),
}

#[derive(Clone, Debug)]
pub struct RequestInfo {
    pub url: RequestUrl,
    pub method: Vec<u8>,
    pub cache_load: CacheLoad,
    pub upload: UploadBody,
    pub caller_headers: Vec<(Vec<u8>, Vec<u8>)>,
    pub referrer: Option<String>,
    pub user_agent: Vec<u8>,
    pub accept_language: Vec<u8>,
    pub cookie_line: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BodyFraming {
    Empty,
    Length(Vec<u8>),
    Chunked(Vec<Vec<u8>>),
}

#[derive(Clone, Debug)]
pub struct WireRequest {
    pub head: Vec<u8>,
    pub body: BodyFraming,
    pub headers: HttpRequestHeaders,
}

impl WireRequest {
    pub fn wire_bytes(&self) -> Vec<u8> {
        let mut bytes = self.head.clone();
        match &self.body {
            BodyFraming::Empty => {}
            BodyFraming::Length(body) => bytes.extend_from_slice(body),
            BodyFraming::Chunked(chunks) => {
                for chunk in chunks {
                    bytes.extend_from_slice(format!("{:X}\r\n", chunk.len()).as_bytes());
                    bytes.extend_from_slice(chunk);
                    bytes.extend_from_slice(b"\r\n");
                }
                bytes.extend_from_slice(b"0\r\n\r\n");
            }
        }
        bytes
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum InvalidRequest {
    Method,
    Header,
}

impl From<InvalidHeader> for InvalidRequest {
    fn from(_: InvalidHeader) -> Self {
        InvalidRequest::Header
    }
}

fn caller_extra_headers(info: &RequestInfo) -> Result<HttpRequestHeaders, InvalidRequest> {
    let mut extra = HttpRequestHeaders::default();
    for (name, value) in &info.caller_headers {
        extra.set(name, value)?;
    }
    extra.remove(b"Referer");
    if let Some(referrer) = &info.referrer {
        extra.set(b"Referer", referrer.as_bytes())?;
    }
    extra.set_if_missing(b"User-Agent", &info.user_agent)?;
    set_accept_encoding_if_missing(&mut extra, &info.url)?;
    if !info.accept_language.is_empty() {
        extra.set_if_missing(b"Accept-Language", &info.accept_language)?;
    }
    if !info.cookie_line.is_empty() {
        extra.set(b"Cookie", &info.cookie_line)?;
    }
    Ok(extra)
}

fn set_accept_encoding_if_missing(headers: &mut HttpRequestHeaders, url: &RequestUrl) -> Result<(), InvalidHeader> {
    if headers.has(b"Accept-Encoding") {
        return Ok(());
    }
    if headers.has(b"Range") {
        return headers.set(b"Accept-Encoding", b"identity");
    }
    let value: &[u8] = if url.is_cryptographic() || url.is_localhost() { b"gzip, deflate, br, zstd" } else { b"gzip, deflate" };
    headers.set(b"Accept-Encoding", value)
}

fn chunks(pieces: &[Vec<u8>]) -> Vec<Vec<u8>> {
    pieces.concat().chunks(CHUNKED_UPLOAD_READ_SIZE).map(<[u8]>::to_vec).collect()
}

pub fn build(info: &RequestInfo) -> Result<WireRequest, InvalidRequest> {
    if !http_util::is_token(&info.method) {
        return Err(InvalidRequest::Method);
    }
    let extra = caller_extra_headers(info)?;
    let mut headers = HttpRequestHeaders::default();
    headers.set(b"Host", info.url.host_and_optional_port().as_bytes())?;
    headers.set(b"Connection", b"keep-alive")?;
    let body = match &info.upload {
        UploadBody::Chunked(pieces) => {
            headers.set(b"Transfer-Encoding", b"chunked")?;
            BodyFraming::Chunked(chunks(pieces))
        }
        UploadBody::Bytes(bytes) => {
            headers.set(b"Content-Length", bytes.len().to_string().as_bytes())?;
            if bytes.is_empty() { BodyFraming::Empty } else { BodyFraming::Length(bytes.clone()) }
        }
        UploadBody::None => {
            if info.method == b"POST" || info.method == b"PUT" {
                headers.set(b"Content-Length", b"0")?;
            }
            BodyFraming::Empty
        }
    };
    match info.cache_load {
        CacheLoad::Normal => {}
        CacheLoad::BypassCache => {
            headers.set(b"Pragma", b"no-cache")?;
            headers.set(b"Cache-Control", b"no-cache")?;
        }
        CacheLoad::ValidateCache => headers.set(b"Cache-Control", b"max-age=0")?,
    }
    headers.merge_from(&extra);
    let mut head = Vec::new();
    head.extend_from_slice(&info.method);
    head.push(b' ');
    head.extend_from_slice(info.url.path_for_request().as_bytes());
    head.extend_from_slice(b" HTTP/1.1\r\n");
    head.extend_from_slice(&headers.to_wire());
    Ok(WireRequest { head, body, headers })
}

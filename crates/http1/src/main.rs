use std::cell::RefCell;
use std::io::{self, Read, Write};
use std::pin::Pin;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use http1::client::{self, Connected, PreparedRequest, SendError};
use http1::http_util;
use http1::request::{CacheLoad, RequestInfo, UploadBody};
use http1::request_url::{RequestUrl, Scheme};
use http1::response_reader::ConnectionVerdict;
use http1::stream_parser::SocketLiveness;

const MAX_SEGMENT: usize = 16384;

#[derive(Clone, Copy, PartialEq, Eq)]
enum End {
    Close,
    Reset,
    Hang,
}

struct Record {
    url: Vec<u8>,
    method: Vec<u8>,
    cache_load: CacheLoad,
    upload: UploadBody,
    headers: Vec<(Vec<u8>, Vec<u8>)>,
    referrer: Vec<u8>,
    user_agent: Vec<u8>,
    accept_language: Vec<u8>,
    cookie_line: Vec<u8>,
    segments: Vec<Vec<u8>>,
    end: End,
}

struct Cursor<'a>(&'a [u8]);

impl<'a> Cursor<'a> {
    fn take(&mut self, size: usize) -> Option<&'a [u8]> {
        if self.0.len() < size {
            return None;
        }
        let (head, rest) = self.0.split_at(size);
        self.0 = rest;
        Some(head)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|bytes| bytes[0])
    }
    fn u16(&mut self) -> Option<usize> {
        self.take(2).map(|bytes| usize::from(u16::from_be_bytes([bytes[0], bytes[1]])))
    }
    fn u32(&mut self) -> Option<usize> {
        self.take(4).map(|bytes| u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize)
    }
    fn short(&mut self) -> Option<Vec<u8>> {
        let size = self.u16()?;
        self.take(size).map(<[u8]>::to_vec)
    }
    fn long(&mut self) -> Option<Vec<u8>> {
        let size = self.u32()?;
        self.take(size).map(<[u8]>::to_vec)
    }
}

fn decode(input: &[u8]) -> Option<Record> {
    let mut cursor = Cursor(input);
    if cursor.u8()? != 1 {
        return None;
    }
    let url = cursor.short()?;
    let method = cursor.short()?;
    let cache_load = match cursor.u8()? {
        0 => CacheLoad::Normal,
        1 => CacheLoad::BypassCache,
        2 => CacheLoad::ValidateCache,
        _ => return None,
    };
    let upload = match cursor.u8()? {
        0 => UploadBody::None,
        1 => UploadBody::Bytes(cursor.long()?),
        2 => {
            let count = cursor.u16()?;
            let mut pieces = Vec::with_capacity(count);
            for _ in 0..count {
                let piece = cursor.long()?;
                if piece.is_empty() {
                    return None;
                }
                pieces.push(piece);
            }
            UploadBody::Chunked(pieces)
        }
        _ => return None,
    };
    let count = cursor.u16()?;
    let mut headers = Vec::with_capacity(count);
    for _ in 0..count {
        headers.push((cursor.short()?, cursor.short()?));
    }
    let referrer = cursor.short()?;
    let user_agent = cursor.short()?;
    let accept_language = cursor.short()?;
    let cookie_line = cursor.short()?;
    let count = cursor.u16()?;
    let mut segments = Vec::with_capacity(count);
    for _ in 0..count {
        let segment = cursor.long()?;
        if segment.is_empty() || segment.len() > MAX_SEGMENT {
            return None;
        }
        segments.push(segment);
    }
    let end = match cursor.u8()? {
        0 => End::Close,
        1 => End::Reset,
        2 => End::Hang,
        _ => return None,
    };
    if !cursor.0.is_empty() {
        return None;
    }
    Some(Record { url, method, cache_load, upload, headers, referrer, user_agent, accept_language, cookie_line, segments, end })
}

struct Script {
    segments: Vec<Vec<u8>>,
    end: End,
    request_len: usize,
}

struct StreamState {
    script: Arc<Script>,
    sent: Vec<u8>,
    next_segment: usize,
    reader: Option<Waker>,
}

struct ScriptedStream(Arc<Mutex<StreamState>>);

struct Dialed {
    stream: Arc<Mutex<StreamState>>,
    verdict: Arc<ConnectionVerdict>,
}

struct ScriptLiveness(Arc<Mutex<StreamState>>);

impl SocketLiveness for ScriptLiveness {
    fn has_undelivered_data(&self) -> bool {
        let state = self.0.lock().expect("stream lock");
        state.next_segment < state.script.segments.len()
    }

    fn is_peer_open(&self) -> bool {
        self.0.lock().expect("stream lock").script.end == End::Hang
    }
}

impl futures_io::AsyncRead for ScriptedStream {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        let mut state = self.0.lock().expect("stream lock");
        if state.sent.len() < state.script.request_len {
            state.reader = Some(cx.waker().clone());
            return Poll::Pending;
        }
        let script = state.script.clone();
        if let Some(segment) = script.segments.get(state.next_segment) {
            if buf.len() < segment.len() {
                return Poll::Ready(Err(io::Error::other("transport read buffer is smaller than a segment")));
            }
            buf[..segment.len()].copy_from_slice(segment);
            state.next_segment += 1;
            return Poll::Ready(Ok(segment.len()));
        }
        match script.end {
            End::Close => Poll::Ready(Ok(0)),
            End::Reset => Poll::Ready(Err(io::ErrorKind::ConnectionReset.into())),
            End::Hang => Poll::Pending,
        }
    }
}

impl futures_io::AsyncWrite for ScriptedStream {
    fn poll_write(self: Pin<&mut Self>, _cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        let mut state = self.0.lock().expect("stream lock");
        state.sent.extend_from_slice(buf);
        if state.sent.len() >= state.script.request_len
            && let Some(reader) = state.reader.take()
        {
            reader.wake();
        }
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

fn canonical_url(bytes: &[u8]) -> Option<RequestUrl> {
    RequestUrl::parse(std::str::from_utf8(bytes).ok()?).ok()
}

fn validate(record: &Record) -> Result<RequestInfo, &'static str> {
    let url = canonical_url(&record.url).filter(|url| url.scheme() == Scheme::Http).ok_or("invalid url\n")?;
    if !http_util::is_token(&record.method) {
        return Err("invalid method\n");
    }
    if !record.headers.iter().all(|(name, value)| http_util::is_valid_header_name(name) && http_util::is_valid_header_value(value)) {
        return Err("invalid header\n");
    }
    if !record.referrer.is_empty() && canonical_url(&record.referrer).is_none() {
        return Err("invalid referrer\n");
    }
    if ![&record.user_agent, &record.accept_language, &record.cookie_line].iter().all(|value| http_util::is_valid_header_value(value)) {
        return Err("invalid header\n");
    }
    Ok(RequestInfo {
        url,
        method: record.method.clone(),
        cache_load: record.cache_load,
        upload: record.upload.clone(),
        caller_headers: record.headers.clone(),
        referrer: (!record.referrer.is_empty()).then(|| String::from_utf8(record.referrer.clone()).expect("a canonical referrer is UTF-8")),
        user_agent: record.user_agent.clone(),
        accept_language: record.accept_language.clone(),
        cookie_line: record.cookie_line.clone(),
    })
}

#[derive(Default)]
struct Outcome {
    start: Option<Result<(), &'static str>>,
    response: Vec<u8>,
    body: Vec<u8>,
    end: Option<&'static str>,
}

fn net_error_name(error: SendError, transport: &RefCell<Option<hyper::Error>>) -> &'static str {
    match error {
        SendError::Net(net) => net.name(),
        SendError::Transport(error) => {
            transport.borrow_mut().get_or_insert(error);
            "transport"
        }
    }
}

fn line(out: &mut Vec<u8>, parts: &[&[u8]]) {
    for part in parts {
        out.extend_from_slice(part);
    }
    out.push(b'\n');
}

fn execute(input: &[u8]) -> Result<Vec<u8>, hyper::Error> {
    let Some(record) = decode(input) else { return Ok(b"invalid record\n".to_vec()) };
    let info = match validate(&record) {
        Ok(info) => info,
        Err(message) => return Ok(message.as_bytes().to_vec()),
    };
    let request = PreparedRequest::new(&info).expect("validated requests build");
    let script = Arc::new(Script { segments: record.segments, end: record.end, request_len: request.wire_bytes().len() });
    let connections: Rc<RefCell<Vec<Dialed>>> = Rc::default();
    let outcome: Rc<RefCell<Outcome>> = Rc::default();
    let transport: Rc<RefCell<Option<hyper::Error>>> = Rc::default();
    let executor = Rc::new(smol::LocalExecutor::new());
    {
        let connections = connections.clone();
        let outcome = outcome.clone();
        let transport = transport.clone();
        let spawner = executor.clone();
        executor
            .spawn(async move {
                let dial = || {
                    let state = Arc::new(Mutex::new(StreamState { script: script.clone(), sent: Vec::new(), next_segment: 0, reader: None }));
                    let verdict = Arc::new(ConnectionVerdict::default());
                    connections.borrow_mut().push(Dialed { stream: state.clone(), verdict: verdict.clone() });
                    Connected { stream: ScriptedStream(state.clone()), liveness: Arc::new(ScriptLiveness(state)), verdict }
                };
                let spawn = |future| spawner.spawn(future).detach();
                let mut response = match client::send(&request, dial, &spawn).await {
                    Ok(response) => response,
                    Err(error) => {
                        outcome.borrow_mut().start = Some(Err(net_error_name(error, &transport)));
                        return;
                    }
                };
                {
                    let mut outcome = outcome.borrow_mut();
                    outcome.start = Some(Ok(()));
                    let headers = &response.headers;
                    let version = headers.version();
                    line(&mut outcome.response, &[format!("version {}.{}", version.major, version.minor).as_bytes()]);
                    line(&mut outcome.response, &[format!("code {}", headers.response_code()).as_bytes()]);
                    line(&mut outcome.response, &[b"line ", headers.status_line()]);
                    for (name, value) in headers.header_lines() {
                        line(&mut outcome.response, &[b"header ", name, b": ", value]);
                    }
                }
                loop {
                    match response.next_chunk().await {
                        Some(Ok(data)) => outcome.borrow_mut().body.extend_from_slice(&data),
                        Some(Err(error)) => {
                            outcome.borrow_mut().end = Some(net_error_name(error, &transport));
                            return;
                        }
                        None => break,
                    }
                }
                outcome.borrow_mut().end = Some("OK");
            })
            .detach();
    }
    while executor.try_tick() {}
    if let Some(error) = transport.borrow_mut().take() {
        return Err(error);
    }
    let outcome = outcome.borrow();
    let mut out = Vec::new();
    for connection in connections.borrow().iter() {
        let state = connection.stream.lock().expect("stream lock");
        line(&mut out, &[format!("sent {}", state.sent.len()).as_bytes()]);
        line(&mut out, &[&state.sent]);
    }
    let start = match outcome.start {
        None => "ERR_IO_PENDING",
        Some(Ok(())) => "OK",
        Some(Err(name)) => name,
    };
    line(&mut out, &[b"start ", start.as_bytes()]);
    out.extend_from_slice(&outcome.response);
    line(&mut out, &[format!("body {}", outcome.body.len()).as_bytes()]);
    line(&mut out, &[&outcome.body]);
    let end = match (outcome.start, outcome.end) {
        (Some(Ok(())), None) => "ERR_IO_PENDING",
        (Some(Ok(())), Some(end)) => end,
        _ => "-",
    };
    line(&mut out, &[b"end ", end.as_bytes()]);
    let reuse = connections.borrow().last().is_some_and(|connection| connection.verdict.reusable() == Some(true));
    line(&mut out, &[b"reuse ", if reuse { b"yes" } else { b"no" }]);
    Ok(out)
}

fn read_exact(input: &mut impl Read, size: usize) -> io::Result<Option<Vec<u8>>> {
    let mut buf = vec![0; size];
    let mut filled = 0;
    while filled < size {
        match input.read(&mut buf[filled..])? {
            0 if filled == 0 => return Ok(None),
            0 => return Err(io::ErrorKind::UnexpectedEof.into()),
            n => filled += n,
        }
    }
    Ok(Some(buf))
}

fn main() -> io::Result<()> {
    let mut stdin = io::stdin().lock();
    let mut stdout = io::stdout().lock();
    if read_exact(&mut stdin, 4)?.as_deref() != Some(b"MGB1") {
        return Err(io::Error::other("missing MGB1 preamble"));
    }
    stdout.write_all(b"MGB1")?;
    stdout.flush()?;
    while let Some(length) = read_exact(&mut stdin, 4)? {
        let length = u32::from_be_bytes([length[0], length[1], length[2], length[3]]) as usize;
        let input = read_exact(&mut stdin, length)?.ok_or_else(|| io::Error::from(io::ErrorKind::UnexpectedEof))?;
        let output = execute(&input).map_err(|error| io::Error::other(format!("transport error outside Chromium's error space: {error}")))?;
        stdout.write_all(&(output.len() as u32).to_be_bytes())?;
        stdout.write_all(&output)?;
        stdout.flush()?;
    }
    Ok(())
}

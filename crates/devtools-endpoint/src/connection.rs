use crate::compression::Deflater;
use crate::compression::Inflater;
use crate::handler::Handler;
use crate::handler::Route;
use crate::handler::Session;
use crate::read_buffer::ReadBuffer;
use crate::read_buffer::WEBSOCKET_MAX_CAPACITY;
use crate::request::Parsed;
use crate::request::Request;
use crate::response::Response;
use crate::websocket::Decoded;
use crate::websocket::Negotiation;
use crate::websocket::accept_key;
use crate::websocket::close_frame;
use crate::websocket::compressed_text_frame;
use crate::websocket::decode_frame;
use crate::websocket::handshake_response;
use crate::websocket::negotiate_extensions;
use crate::websocket::pong_frame;
use crate::websocket::text_frame;

const MAX_BODY: usize = 100 << 20;

/// base::StringToSizeT as net::HttpServer applies it: an optional plus sign and digits only.
fn parse_content_length(text: &[u8]) -> Option<usize> {
    let digits = text.strip_prefix(b"+").unwrap_or(text);
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(digits).ok()?.parse::<usize>().ok().filter(|length| *length <= MAX_BODY)
}

/// Work Chromium hands from its IO thread to the UI thread: the connection has moved on by the time it runs,
/// and a connection closed in the meantime never sees its answer.
enum Task {
    Http(Request),
    WebSocketRequest(Request),
    WebSocketMessage(Vec<u8>),
}

#[derive(Default)]
struct WebSocket {
    accepted: bool,
    closed: bool,
    session: Option<Session>,
    message: Vec<u8>,
    message_compressed: bool,
    message_started: bool,
    inflater: Option<Inflater>,
    deflater: Option<Deflater>,
}

enum Phase {
    Http,
    WebSocket(WebSocket),
}

/// What a read produced: bytes to send, in order, and whether to close the connection afterwards.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub send: Vec<u8>,
    pub close: bool,
}

/// One client connection of net::HttpServer, as a state machine fed by reads.
pub struct Connection {
    buffer: ReadBuffer,
    phase: Phase,
    tasks: Vec<Task>,
    out: Vec<u8>,
    closed: bool,
}

impl Default for Connection {
    fn default() -> Self {
        Self::new()
    }
}

impl Connection {
    pub fn new() -> Self {
        Self { buffer: ReadBuffer::default(), phase: Phase::Http, tasks: Vec::new(), out: Vec::new(), closed: false }
    }

    /// How many bytes the next read may take, or `None` when the connection must be closed because its buffer is full.
    pub fn prepare_read(&mut self) -> Option<usize> {
        self.buffer.prepare_read()
    }

    pub fn receive(&mut self, handler: &Handler, data: &[u8]) -> Outcome {
        self.buffer.did_read(data);
        self.drain(handler);
        if !self.closed {
            for task in std::mem::take(&mut self.tasks) {
                self.run(handler, task);
            }
        }
        Outcome { send: std::mem::take(&mut self.out), close: self.closed }
    }

    fn send(&mut self, response: &Response) {
        self.out.extend_from_slice(&response.serialize());
    }

    fn close(&mut self) {
        self.closed = true;
        self.tasks.clear();
    }

    fn drain(&mut self, handler: &Handler) {
        while !self.closed && !self.buffer.readable().is_empty() {
            let more = match self.phase {
                Phase::Http => self.http_step(handler),
                Phase::WebSocket(_) => self.websocket_step(),
            };
            if !more {
                break;
            }
        }
    }

    fn http_step(&mut self, handler: &Handler) -> bool {
        let (parsed, mut request) = Request::parse(self.buffer.readable());
        let mut consumed = match parsed {
            Parsed::Invalid => {
                self.close();
                return false;
            }
            Parsed::Incomplete => return false,
            Parsed::Complete { consumed } => consumed,
        };
        if request.has_header_token("connection", "upgrade") && request.has_header_token("upgrade", "websocket") {
            self.phase = Phase::WebSocket(WebSocket::default());
            self.buffer.consume(consumed);
            match handler.route_websocket(&request) {
                Route::Now(response) => self.send(&response),
                Route::Later => self.tasks.push(Task::WebSocketRequest(request)),
            }
            return true;
        }
        if request.has_header("content-length") {
            let Some(length) = parse_content_length(request.header("content-length")) else {
                self.send(&Response::internal_error(b"request content-length too big or unknown."));
                self.close();
                return false;
            };
            if self.buffer.readable().len() - consumed < length {
                return false;
            }
            request.set_body(self.buffer.readable()[consumed..consumed + length].to_vec());
            consumed += length;
        }
        self.buffer.consume(consumed);
        let devtools_path = request.path().starts_with(b"/devtools/");
        if devtools_path && matches!(Request::parse(self.buffer.readable()).0, Parsed::Invalid) {
            self.close();
            return false;
        }
        match handler.route_http(&request) {
            Route::Now(response) => self.send(&response),
            Route::Later => self.tasks.push(Task::Http(request)),
        }
        true
    }

    fn websocket_step(&mut self) -> bool {
        let Phase::WebSocket(websocket) = &self.phase else {
            return false;
        };
        if websocket.closed || !websocket.accepted {
            self.close();
            return false;
        }
        let (decoded, consumed) = decode_frame(self.buffer.readable());
        match decoded {
            Decoded::Incomplete => return false,
            Decoded::Error => {
                self.close();
                return false;
            }
            Decoded::Close => {
                self.out.extend_from_slice(&close_frame(&[0x03, 0xe8]));
                self.close();
                return false;
            }
            Decoded::Pong => {}
            Decoded::Ping(payload) => self.out.extend_from_slice(&pong_frame(&payload)),
            Decoded::Message { payload, fin, compressed } => {
                let Phase::WebSocket(websocket) = &mut self.phase else {
                    return false;
                };
                if !websocket.message_started {
                    websocket.message.clear();
                    websocket.message_compressed = compressed;
                    websocket.message_started = true;
                }
                websocket.message.extend_from_slice(&payload);
                if fin {
                    websocket.message_started = false;
                    let mut message = std::mem::take(&mut websocket.message);
                    if std::mem::take(&mut websocket.message_compressed) {
                        let Some(inflater) = websocket.inflater.as_mut() else {
                            self.close();
                            return false;
                        };
                        let Some(inflated) = inflater.inflate(&message) else {
                            self.close();
                            return false;
                        };
                        message = inflated;
                    }
                    self.tasks.push(Task::WebSocketMessage(message));
                }
            }
        }
        self.buffer.consume(consumed);
        true
    }

    fn run(&mut self, handler: &Handler, task: Task) {
        match task {
            Task::Http(request) => {
                let response = handler.serve_http(&request);
                self.send(&response);
            }
            Task::WebSocketRequest(request) => {
                self.buffer.set_max_capacity(WEBSOCKET_MAX_CAPACITY);
                match handler.open_websocket(&request) {
                    Ok(session) => self.accept(&request, session),
                    Err(response) => self.send(&response),
                }
            }
            Task::WebSocketMessage(message) => {
                let Phase::WebSocket(websocket) = &mut self.phase else {
                    return;
                };
                if websocket.closed {
                    return;
                }
                if let Some(session) = &websocket.session {
                    let reply = handler.handle_message(session, &message);
                    if !reply.is_empty() {
                        let frame = match websocket.deflater.as_mut().and_then(|deflater| deflater.compress(&reply)) {
                            Some(payload) => compressed_text_frame(&payload),
                            None => text_frame(&reply),
                        };
                        self.out.extend_from_slice(&frame);
                    }
                }
            }
        }
    }

    fn accept(&mut self, request: &Request, session: Session) {
        let version = request.header("sec-websocket-version");
        let key = request.header("sec-websocket-key");
        let failure: Option<&[u8]> = if version != b"8" && version != b"13" {
            Some(b"Invalid request format. The version is not valid.")
        } else if key.is_empty() {
            Some(b"Invalid request format. Sec-WebSocket-Key is empty or isn't specified.")
        } else {
            None
        };
        let Phase::WebSocket(websocket) = &mut self.phase else {
            return;
        };
        websocket.session = Some(session);
        if let Some(message) = failure {
            websocket.closed = true;
            self.send(&Response::internal_error(message));
            return;
        }
        let mut extension = None;
        if request.has_header("sec-websocket-extensions") {
            match negotiate_extensions(request.header("sec-websocket-extensions")) {
                Negotiation::Rejected => {
                    websocket.closed = true;
                    self.close();
                    return;
                }
                Negotiation::Declined => {}
                Negotiation::Deflate(agreement) => {
                    websocket.deflater = Some(Deflater::new(agreement.server_max_window_bits, !agreement.server_no_context_takeover));
                    websocket.inflater = Some(Inflater::new(agreement.client_max_window_bits));
                    extension = Some(agreement.response);
                }
            }
        }
        websocket.accepted = true;
        self.out.extend_from_slice(&handshake_response(&accept_key(key), extension.as_deref()));
    }
}

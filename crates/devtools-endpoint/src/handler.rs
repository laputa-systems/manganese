use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::MutexGuard;

use crate::cdp::Identity;
use crate::cdp::handle_message;
use crate::json::Json;
use crate::json::escape_html;
use crate::json::pretty;
use crate::registry::Registry;
use crate::registry::Target;
use crate::request::Request;
use crate::response::Response;
use crate::url::host_is_safe;
use crate::url::target_url;

const PAGE_PREFIX: &[u8] = b"/devtools/page/";
const BROWSER_PREFIX: &str = "/devtools/browser";
const JSON_TYPE: &str = "application/json; charset=UTF-8";
const UNSAFE_HOST: &[u8] = b"Host header is specified and is not an IP address or localhost.";

/// Where `/json/protocol` gets Chromium's protocol description.
#[derive(Clone, Debug)]
pub enum ProtocolSource {
    Bytes(Vec<u8>),
    File(PathBuf),
}

#[derive(Clone, Debug)]
pub struct Config {
    pub identity: Identity,
    pub frontend_host: String,
    pub discovery_page: String,
    pub allowed_origins: Vec<String>,
    pub protocol: ProtocolSource,
}

/// Which target a WebSocket session talks to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Session {
    Browser,
    Page(String),
}

pub enum Route {
    Now(Response),
    Later,
}

pub struct Handler {
    config: Config,
    registry: Mutex<Registry>,
    address: SocketAddr,
}

fn starts_with(haystack: &[u8], prefix: &[u8]) -> bool {
    haystack.starts_with(prefix)
}

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn json_response(status: u16, value: Option<&Json>, message: &[u8]) -> Response {
    let mut body = value.map(|value| pretty(value).into_bytes()).unwrap_or_default();
    body.extend_from_slice(message);
    Response::new(status).header("Content-Security-Policy", "frame-ancestors 'none'").body(body, JSON_TYPE)
}

fn concat(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

impl Handler {
    pub fn new(config: Config, registry: Registry, address: SocketAddr) -> Self {
        Self { config, registry: Mutex::new(registry), address }
    }

    fn registry(&self) -> MutexGuard<'_, Registry> {
        self.registry.lock().expect("no code panics while holding the registry")
    }

    pub fn is_safe_to_serve(&self, request: &Request) -> bool {
        host_is_safe(&lossy(request.header("host")))
    }

    fn descriptor(&self, target: &Target, host: &str) -> Json {
        let ws = format!("{host}/devtools/page/{}", target.id);
        Json::dict([
            ("description", Json::str(target.description.clone())),
            ("devtoolsFrontendUrl", Json::str(format!("https://{}/serve_rev/{}/inspector.html?ws={ws}", self.config.frontend_host, self.config.identity.revision))),
            ("id", Json::str(target.id.clone())),
            ("title", Json::str(escape_html(&target.title))),
            ("type", Json::str(target.kind.as_str())),
            ("url", Json::str(target.url.clone())),
            ("webSocketDebuggerUrl", Json::str(format!("ws://{ws}"))),
        ])
    }

    /// The part of net::HttpServer's delegate that runs on the IO thread: requests it can answer itself
    /// are answered at once, everything else is handed to the UI thread and answered later.
    pub fn route_http(&self, request: &Request) -> Route {
        if !self.is_safe_to_serve(request) {
            return Route::Now(Response::internal_error(UNSAFE_HOST));
        }
        let path = request.path();
        if starts_with(path, b"/json") || path.is_empty() || path == b"/" {
            return Route::Later;
        }
        Route::Now(Response::not_found())
    }

    pub fn serve_http(&self, request: &Request) -> Response {
        if starts_with(request.path(), b"/json") {
            return self.serve_json(request);
        }
        Response::new(200).header("X-Frame-Options", "DENY").body(self.config.discovery_page.clone().into_bytes(), "text/html; charset=UTF-8")
    }

    fn serve_json(&self, request: &Request) -> Response {
        let mut path = &request.path()[5..];
        let mut query: &[u8] = &[];
        if let Some(position) = path.iter().position(|byte| *byte == b'?') {
            query = &path[position + 1..];
            path = &path[..position];
        }
        if let Some(position) = path.iter().position(|byte| *byte == b'#') {
            path = &path[..position];
        }
        let (command, target_id): (&[u8], &[u8]) = if path.is_empty() {
            (b"list", b"")
        } else if path[0] != b'/' {
            let message = concat(&[b"Malformed query: ", request.path()]);
            return json_response(404, None, &message);
        } else {
            let view = &path[1..];
            match view.iter().position(|byte| *byte == b'/') {
                Some(position) => (&view[..position], &view[position + 1..]),
                None => (view, b""),
            }
        };
        let host = lossy(request.header("host"));
        match command {
            b"version" => self.version(&host),
            b"protocol" => self.protocol(),
            b"list" => {
                let registry = self.registry();
                let list = registry.by_activity().into_iter().map(|target| self.descriptor(target, &host)).collect();
                json_response(200, Some(&Json::List(list)), b"")
            }
            b"new" => self.new_target(request, query, &host),
            b"activate" | b"close" => self.activate_or_close(command == b"activate", target_id),
            _ => json_response(404, None, &concat(&[b"Unknown command: ", command])),
        }
    }

    fn version(&self, host: &str) -> Response {
        let identity = &self.config.identity;
        let browser_id = self.registry().browser_id().to_owned();
        let value = Json::dict([
            ("Browser", Json::str(identity.product.clone())),
            ("File-Path-Style", Json::str("posix")),
            ("Protocol-Version", Json::str(identity.protocol_version.clone())),
            ("User-Agent", Json::str(identity.user_agent.clone())),
            ("V8-Version", Json::str(identity.js_version.clone())),
            ("WebKit-Version", Json::str(format!("537.36 ({})", identity.revision))),
            ("webSocketDebuggerUrl", Json::str(format!("ws://{host}{BROWSER_PREFIX}/{browser_id}"))),
        ]);
        json_response(200, Some(&value), b"")
    }

    fn protocol(&self) -> Response {
        let bytes = match &self.config.protocol {
            ProtocolSource::Bytes(bytes) => Ok(bytes.clone()),
            ProtocolSource::File(path) => std::fs::read(path).map_err(|error| format!("Could not read the protocol description {}: {error}", path.display())),
        };
        match bytes {
            Ok(bytes) => Response::new(200).body(bytes, JSON_TYPE),
            Err(message) => json_response(500, None, message.as_bytes()),
        }
    }

    fn new_target(&self, request: &Request, query: &[u8], host: &str) -> Response {
        if !request.method().eq_ignore_ascii_case(b"PUT") {
            let message = concat(&[b"Using unsafe HTTP verb ", request.method(), b" to invoke /json/new. This action supports only PUT verb."]);
            return json_response(405, None, &message);
        }
        let first = query.split(|byte| *byte == b'&').find(|component| !component.is_empty()).unwrap_or_default();
        let url = target_url(&lossy(first));
        let target = self.registry().create(&url);
        json_response(200, Some(&self.descriptor(&target, host)), b"")
    }

    fn activate_or_close(&self, activate: bool, target_id: &[u8]) -> Response {
        let id = lossy(target_id);
        let mut registry = self.registry();
        let known = if activate { registry.activate(&id) } else { registry.close(&id) };
        if !known {
            return json_response(404, None, &concat(&[b"No such target id: ", target_id]));
        }
        json_response(200, None, if activate { b"Target activated" } else { b"Target is closing" })
    }

    pub fn route_websocket(&self, request: &Request) -> Route {
        if self.is_safe_to_serve(request) {
            Route::Later
        } else {
            Route::Now(Response::internal_error(UNSAFE_HOST))
        }
    }

    fn same_origin(&self, origin: &str) -> bool {
        let Some(rest) = origin.strip_prefix("http://") else {
            return false;
        };
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default().to_ascii_lowercase();
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) => (host.to_owned(), port.parse::<u16>().ok()),
            None => (authority.clone(), Some(80)),
        };
        host == self.address.ip().to_string() && port == Some(self.address.port())
    }

    /// DevToolsHttpHandler::OnWebSocketRequest: pick the agent host the socket will talk to.
    pub fn open_websocket(&self, request: &Request) -> Result<Session, Response> {
        if request.has_header("origin") {
            let origin = lossy(request.header("origin"));
            let allowed = self.config.allowed_origins.iter().any(|allowed| allowed == "*" || allowed.eq_ignore_ascii_case(&origin));
            if !self.same_origin(&origin) && !allowed {
                let message = format!(
                    "Rejected an incoming WebSocket connection from the {origin} origin. Use the command line flag --remote-allow-origins={origin} to allow connections from this origin or --remote-allow-origins=* to allow all origins."
                );
                return Err(Response::new(403).body(message.into_bytes(), "text/html"));
            }
        }
        let path = request.path();
        let browser_path = format!("{BROWSER_PREFIX}/{}", self.registry().browser_id());
        if starts_with(path, browser_path.as_bytes()) {
            return Ok(Session::Browser);
        }
        if !starts_with(path, PAGE_PREFIX) {
            return Err(Response::not_found());
        }
        let id = &path[PAGE_PREFIX.len()..];
        match std::str::from_utf8(id).ok().filter(|id| self.registry().get(id).is_some()) {
            Some(id) => Ok(Session::Page(id.to_owned())),
            None => Err(Response::internal_error(&concat(&[b"No such target id: ", id]))),
        }
    }

    pub fn handle_message(&self, session: &Session, message: &[u8]) -> Vec<u8> {
        let mut registry = self.registry();
        if let Session::Page(id) = session
            && registry.get(id).is_none()
        {
            return Vec::new();
        }
        handle_message(message, &mut registry, &self.config.identity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::SequenceIds;
    use crate::registry::TargetKind;

    fn handler() -> Handler {
        let config = Config {
            identity: Identity { protocol_version: "1.3".into(), product: "Chrome/1".into(), revision: "@r".into(), user_agent: "UA".into(), js_version: "1".into() },
            frontend_host: "frontend.example".into(),
            discovery_page: String::new(),
            allowed_origins: Vec::new(),
            protocol: ProtocolSource::Bytes(b"{\"version\":{}}".to_vec()),
        };
        let registry = Registry::new(Box::new(SequenceIds::new()), Some("uuid".into()), vec![Target::new("T1", TargetKind::Page, "a<b", "about:blank")]);
        Handler::new(config, registry, "127.0.0.1:9221".parse().expect("an address"))
    }

    fn body(response: &Response) -> String {
        let bytes = response.serialize();
        let split = bytes.windows(4).position(|window| window == b"\r\n\r\n").expect("a header block") + 4;
        String::from_utf8(bytes[split..].to_vec()).expect("text")
    }

    #[test]
    fn json_endpoints_use_the_request_host_and_chromiums_layout() {
        let handler = handler();
        let version = handler.serve_http(&Request::new("GET", "/json/version").with_header("host", "relay:1"));
        assert!(body(&version).contains("\"webSocketDebuggerUrl\": \"ws://relay:1/devtools/browser/uuid\""));
        let list = body(&handler.serve_http(&Request::new("GET", "/json").with_header("host", "")));
        assert!(list.starts_with("[ {\n   \"description\": \"\","));
        assert!(list.contains("\"title\": \"a&lt;b\""));
        assert!(list.contains("ws=/devtools/page/T1"));
    }

    #[test]
    fn json_commands_answer_with_chromiums_messages() {
        let handler = handler();
        let ask = |method: &str, path: &str| body(&handler.serve_http(&Request::new(method, path)));
        assert_eq!(ask("GET", "/json/new"), "Using unsafe HTTP verb GET to invoke /json/new. This action supports only PUT verb.");
        assert_eq!(ask("GET", "/json/activate/nope"), "No such target id: nope");
        assert_eq!(ask("GET", "/json/activate/T1"), "Target activated");
        assert_eq!(ask("GET", "/json/close/T1"), "Target is closing");
        assert_eq!(ask("GET", "/json/close/T1"), "No such target id: T1");
        assert_eq!(ask("GET", "/json/x"), "Unknown command: x");
        assert_eq!(ask("GET", "/jsonx"), "Malformed query: /jsonx");
    }

    #[test]
    fn hundred_consecutive_protocol_requests_all_answer_and_the_endpoint_keeps_serving() {
        let handler = handler();
        for _ in 0..100 {
            let response = handler.serve_http(&Request::new("GET", "/json/protocol"));
            assert_eq!(response.status(), 200);
            assert_eq!(body(&response), "{\"version\":{}}");
        }
        assert_eq!(handler.serve_http(&Request::new("GET", "/json/version")).status(), 200);
    }
}

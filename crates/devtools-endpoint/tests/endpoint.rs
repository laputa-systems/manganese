use std::io::Read;
use std::io::Write;
use std::net::SocketAddr;
use std::net::TcpStream;
use std::time::Duration;

use devtools_endpoint::endpoint::Endpoint;
use devtools_endpoint::handler::ProtocolSource;
use devtools_endpoint::launch;
use devtools_endpoint::registry::SequenceIds;

const FIXTURE: &[u8] = b"{\"version\":{\"major\":\"1\",\"minor\":\"3\"},\"domains\":[]}";

fn endpoint() -> Endpoint {
    let mut config = launch::config();
    config.protocol = ProtocolSource::Bytes(FIXTURE.to_vec());
    Endpoint::start(SocketAddr::from(([127, 0, 0, 1], 0)), config, launch::registry(Box::new(SequenceIds::new()))).expect("the endpoint starts")
}

fn connect(endpoint: &Endpoint) -> TcpStream {
    let stream = TcpStream::connect(endpoint.local_addr()).expect("the endpoint accepts");
    stream.set_read_timeout(Some(Duration::from_millis(500))).expect("a timeout");
    stream
}

fn read_all(stream: &mut TcpStream) -> Vec<u8> {
    let mut out = Vec::new();
    let mut chunk = [0u8; 4096];
    while let Ok(count) = stream.read(&mut chunk) {
        if count == 0 {
            break;
        }
        out.extend_from_slice(&chunk[..count]);
    }
    out
}

fn get(endpoint: &Endpoint, path: &str) -> Vec<u8> {
    let mut stream = connect(endpoint);
    write!(stream, "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:9221\r\n\r\n").expect("the request is sent");
    let mut out = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let count = stream.read(&mut chunk).expect("the endpoint answers");
        assert_ne!(count, 0, "the endpoint closed before answering");
        out.extend_from_slice(&chunk[..count]);
        if let Some(split) = out.windows(4).position(|window| window == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&out[..split]).to_ascii_lowercase();
            let length: usize = head.split("content-length:").nth(1).and_then(|rest| rest.split("\r\n").next()).and_then(|value| value.trim().parse().ok()).expect("a content length");
            if out.len() >= split + 4 + length {
                return out;
            }
        }
    }
}

fn body(response: &[u8]) -> &[u8] {
    let split = response.windows(4).position(|window| window == b"\r\n\r\n").expect("a header block") + 4;
    &response[split..]
}

fn masked_text(payload: &[u8]) -> Vec<u8> {
    let key = [1u8, 2, 3, 4];
    let mut frame = vec![0x81, 0x80 | payload.len() as u8];
    frame.extend_from_slice(&key);
    frame.extend(payload.iter().enumerate().map(|(index, byte)| byte ^ key[index % 4]));
    frame
}

#[test]
fn a_hundred_protocol_requests_each_return_the_protocol_and_the_endpoint_keeps_serving() {
    let endpoint = Endpoint::start(SocketAddr::from(([127, 0, 0, 1], 0)), launch::config(), launch::registry(Box::new(SequenceIds::new()))).expect("the endpoint starts");
    for round in 0..100 {
        let response = get(&endpoint, "/json/protocol");
        assert!(response.starts_with(b"HTTP/1.1 200 OK\r\n"), "round {round}");
        assert_eq!(body(&response), include_bytes!("../protocol.json"), "round {round}");
    }
    let version = get(&endpoint, "/json/version");
    assert!(version.starts_with(b"HTTP/1.1 200 OK\r\n"));
    assert!(String::from_utf8_lossy(body(&version)).contains("\"Browser\": \"Chrome/156.0.8078.79\""));
}

#[test]
fn protocol_requests_on_one_connection_are_answered_in_order() {
    let endpoint = endpoint();
    let mut stream = connect(&endpoint);
    for _ in 0..100 {
        write!(stream, "GET /json/protocol HTTP/1.1\r\nHost: localhost\r\n\r\n").expect("the request is sent");
    }
    let all = read_all(&mut stream);
    assert_eq!(all.windows(FIXTURE.len()).filter(|window| *window == FIXTURE).count(), 100);
}

#[test]
fn http_one_point_zero_requests_are_closed_without_a_response() {
    let endpoint = endpoint();
    let mut stream = connect(&endpoint);
    write!(stream, "GET /json/version HTTP/1.0\r\n\r\n").expect("the request is sent");
    assert!(read_all(&mut stream).is_empty());
}

#[test]
fn a_websocket_session_answers_browser_get_version() {
    let endpoint = endpoint();
    let mut stream = connect(&endpoint);
    write!(
        stream,
        "GET /devtools/browser/{} HTTP/1.1\r\nHost: 127.0.0.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n",
        launch::BROWSER_ID
    )
    .expect("the handshake is sent");
    std::thread::sleep(Duration::from_millis(100));
    stream.write_all(&masked_text(br#"{"id":1,"method":"Browser.getVersion"}"#)).expect("the frame is sent");
    let reply = read_all(&mut stream);
    let text = String::from_utf8_lossy(&reply);
    assert!(text.starts_with("HTTP/1.1 101 WebSocket Protocol Handshake\r\nUpgrade: WebSocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n\r\n"));
    assert!(text.contains(r#"{"id":1,"result":{"protocolVersion":"1.3","product":"Chrome/156.0.8078.79""#));
}

#[test]
fn client_disconnect_mid_request_does_not_interrupt_other_connections() {
    let endpoint = endpoint();
    let mut abandoned = connect(&endpoint);
    abandoned.write_all(b"GET /json/version HTTP/1.1\r\nHost:").expect("partial request");
    drop(abandoned);
    assert!(get(&endpoint, "/json/version").starts_with(b"HTTP/1.1 200 OK\r\n"));
}

#[test]
fn upgraded_connection_closes_after_a_close_frame() {
    let endpoint = endpoint();
    let mut stream = connect(&endpoint);
    write!(stream, "GET /devtools/browser/{} HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n", launch::BROWSER_ID).expect("handshake");
    let mut handshake = Vec::new();
    while !handshake.ends_with(b"\r\n\r\n") {
        let mut byte = [0u8; 1];
        stream.read_exact(&mut byte).expect("handshake response");
        handshake.push(byte[0]);
    }
    assert!(handshake.starts_with(b"HTTP/1.1 101"));
    stream.write_all(&[0x88, 0x80, 1, 2, 3, 4]).expect("close frame");
    let mut reply = Vec::new();
    stream.read_to_end(&mut reply).expect("server closes the connection");
    assert_eq!(reply, [0x88, 0x02, 0x03, 0xe8]);
    assert!(get(&endpoint, "/json/version").starts_with(b"HTTP/1.1 200 OK\r\n"));
}

#[test]
fn dropping_endpoint_closes_an_idle_keep_alive_connection() {
    let endpoint = endpoint();
    let mut stream = connect(&endpoint);
    stream.write_all(b"GET /json/version HTTP/1.1\r\nHost: localhost\r\n\r\n").expect("request");
    assert!(read_all(&mut stream).starts_with(b"HTTP/1.1 200 OK\r\n"));
    drop(endpoint);
    let mut byte = [0u8; 1];
    assert_eq!(stream.read(&mut byte).expect("connection closes"), 0);
}

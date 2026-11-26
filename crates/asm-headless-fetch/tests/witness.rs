use std::net::TcpStream;
use std::sync::atomic::Ordering;
use std::time::Duration;

use asm_headless_fetch::{
    CancellationWiring, Relay, RelayMode, endpoint, fetch, http_get, run_cancellation, spawn_relay,
};

#[test]
fn endpoint_starts_and_stays_reachable() {
    let service = endpoint();
    let address = service.local_addr();
    for round in 0..3 {
        let response = http_get(address, "/json/version");
        assert!(response.starts_with(b"HTTP/1.1 200 OK\r\n"), "round {round}");
        let text = String::from_utf8_lossy(&response);
        assert!(text.contains("\"Browser\": \"Chrome/156.0.8078.79\""), "round {round}");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(http_get(address, "/json/list").starts_with(b"HTTP/1.1 200 OK\r\n"));
}

#[test]
fn http_fetch_traverses_the_local_socks5_relay() {
    let body = b"delivered-through-socks5".to_vec();
    let mut relay = spawn_relay(RelayMode::Complete(body.clone()));
    let observed = fetch(&relay);
    assert_eq!(observed, body);
    let details = relay.observations();
    assert!(details.handshake.load(Ordering::SeqCst), "the SOCKS5 handshake completed");
    assert_eq!(*details.connect_target.lock().expect("target lock"), "manganese.invalid:8123");
    let request = details.request.lock().expect("request lock").clone();
    assert!(request.starts_with("GET /witness HTTP/1.1\r\n"), "the accepted client wrote the request");
    assert!(request.to_ascii_lowercase().contains("host: manganese.invalid:8123\r\n"));
    relay.stop_and_join();
}

#[test]
fn controlled_deadline_cancellation_reaches_the_fetch_and_cleans_up() {
    let observation = run_cancellation(CancellationWiring { propagate: true, cleanup: true });
    assert_eq!(observation.outcome, "canceled");
    assert!(observation.request_seen, "the fetch was in flight at the relay");
    assert!(observation.body_len > 0 && observation.body_len < 64, "only a partial body arrived");
    assert_eq!(observation.retained_connections, 0, "no connection handle is retained");
    assert!(observation.peer_closed, "the relay saw the client close the connection");
    assert!(!observation.relay_timed_out, "the relay did not wait out a leaked socket");
}

#[test]
fn endpoint_and_relay_shut_down_cleanly() {
    let body = b"clean-shutdown".to_vec();
    let mut relay: Relay = spawn_relay(RelayMode::Complete(body.clone()));
    let service = endpoint();
    let address = service.local_addr();
    assert_eq!(fetch(&relay), body);
    assert!(relay.observations().server_closed.load(Ordering::SeqCst));

    drop(service);
    let refused = TcpStream::connect_timeout(&address, Duration::from_millis(200)).is_err();
    assert!(refused, "the endpoint listener is gone after shutdown");

    relay.stop_and_join();
    assert!(!relay.has_live_child(), "the relay stub left no child thread");
    assert_eq!(relay.counter().active(), 0, "no fetch socket is left open");
    assert!(
        std::net::TcpStream::connect(relay.address()).is_err(),
        "the relay listener is gone after shutdown"
    );
}

#[test]
fn negative_control_missing_cancellation_propagation_is_detected() {
    let observation = run_cancellation(CancellationWiring { propagate: false, cleanup: true });
    assert_eq!(observation.outcome, "watchdog", "without the deadline race the fetch never cancels");
    assert!(observation.request_seen, "the broken wiring still reached the relay");
}

#[test]
fn negative_control_missing_cleanup_leaves_the_peer_open() {
    let observation = run_cancellation(CancellationWiring { propagate: true, cleanup: false });
    assert_eq!(observation.outcome, "canceled", "the deadline still reaches the fetch");
    assert!(observation.retained_connections > 0, "an unreleased connection handle is a leaked socket");
}

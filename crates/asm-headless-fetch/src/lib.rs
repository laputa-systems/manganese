use std::cell::RefCell;
use std::future::Future;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener as StdTcpListener, TcpStream as StdTcpStream};
use std::pin::Pin;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self as sync_channel, Receiver as SyncReceiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::thread::JoinHandle;
use std::time::Duration;

use devtools_endpoint::endpoint::Endpoint;
use devtools_endpoint::handler::ProtocolSource;
use devtools_endpoint::launch;
use devtools_endpoint::registry::SequenceIds;
use futures_io::{AsyncRead, AsyncWrite};
use http1::client::{self, Connected, PreparedRequest};
use http1::request::{CacheLoad, RequestInfo, UploadBody};
use http1::request_url::RequestUrl;
use http1::response_reader::ConnectionVerdict;
use http1::stream_parser::SocketLiveness;
use runtime::Deadline;
use smol::channel::{self, Sender as AsyncSender};
use smol::net::TcpStream as AsyncTcpStream;
use smol::Task;

const TARGET_HOST: &str = "manganese.invalid";
const TARGET_PORT: u16 = 8123;
const FULL_BODY_LEN: usize = 64;
const PARTIAL_BODY: &[u8] = b"partial-body";
const MAX_RETAINED_CONNECTIONS: usize = 8;
const RELAY_POLL_INTERVAL: Duration = Duration::from_millis(50);
const OBSERVATION_TIMEOUT: Duration = Duration::from_millis(2000);
const TRIGGER_ELAPSED: Duration = Duration::from_millis(9);

#[derive(Debug, Default)]
pub struct ConnectionCounter {
    active: AtomicUsize,
}

impl ConnectionCounter {
    pub fn active(&self) -> usize {
        self.active.load(Ordering::SeqCst)
    }
}

struct ConnectionGuard(Arc<ConnectionCounter>);

impl ConnectionGuard {
    fn new(counter: Arc<ConnectionCounter>) -> Self {
        counter.active.fetch_add(1, Ordering::SeqCst);
        Self(counter)
    }
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
    }
}

#[derive(Clone, Debug, Default)]
pub struct RetainedConnections(Rc<RefCell<Vec<StdTcpStream>>>);

impl RetainedConnections {
    fn retain(&self, stream: &StdTcpStream) {
        let mut held = self.0.borrow_mut();
        if held.len() >= MAX_RETAINED_CONNECTIONS {
            held.remove(0);
        }
        held.push(stream.try_clone().expect("a retained connection handle"));
    }

    fn release(&self) {
        self.0.borrow_mut().clear();
    }

    fn len(&self) -> usize {
        self.0.borrow().len()
    }
}

struct TrackedStream {
    inner: AsyncTcpStream,
    _guard: ConnectionGuard,
}

impl AsyncRead for TrackedStream {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for TrackedStream {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_close(cx)
    }
}

struct TestLiveness;

impl SocketLiveness for TestLiveness {
    fn has_undelivered_data(&self) -> bool {
        false
    }

    fn is_peer_open(&self) -> bool {
        false
    }
}

#[derive(Debug, Default)]
pub struct RelayObservations {
    pub handshake: AtomicBool,
    pub request_seen: AtomicBool,
    pub peer_closed: AtomicBool,
    pub server_closed: AtomicBool,
    pub timed_out: AtomicBool,
    pub connect_target: Mutex<String>,
    pub request: Mutex<String>,
}

#[derive(Debug)]
pub enum RelayMode {
    Complete(Vec<u8>),
    Stall {
        request_seen: AsyncSender<()>,
        release: SyncReceiver<()>,
        observed: AsyncSender<()>,
    },
}

#[derive(Debug)]
pub struct Relay {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    stop_signal: SyncSender<()>,
    thread: Option<JoinHandle<()>>,
    observations: Arc<RelayObservations>,
    counter: Arc<ConnectionCounter>,
}

impl Relay {
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    pub fn observations(&self) -> &Arc<RelayObservations> {
        &self.observations
    }

    pub fn counter(&self) -> &Arc<ConnectionCounter> {
        &self.counter
    }

    pub fn has_live_child(&self) -> bool {
        self.thread.is_some()
    }

    pub fn stop_and_join(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = self.stop_signal.try_send(());
        if let Some(thread) = self.thread.take() {
            if thread.join().is_err() {
                eprintln!("relay stub panicked during cleanup");
            }
        }
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.stop_and_join();
    }
}

pub fn spawn_relay(mode: RelayMode) -> Relay {
    let listener = StdTcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).expect("the relay binds loopback");
    let address = listener.local_addr().expect("the relay has an address");
    let stop = Arc::new(AtomicBool::new(false));
    let (stop_signal, stop_rx) = sync_channel::sync_channel(1);
    let observations = Arc::new(RelayObservations::default());
    let counter = Arc::new(ConnectionCounter::default());
    let thread_stop = stop.clone();
    let thread_observations = observations.clone();
    let thread = std::thread::Builder::new()
        .name("socks5-relay-stub".into())
        .spawn(move || relay_loop(listener, thread_stop, stop_rx, mode, thread_observations))
        .expect("the relay thread starts");
    Relay { address, stop, stop_signal, thread: Some(thread), observations, counter }
}

fn relay_loop(
    listener: StdTcpListener,
    stop: Arc<AtomicBool>,
    stop_rx: SyncReceiver<()>,
    mode: RelayMode,
    observations: Arc<RelayObservations>,
) {
    listener.set_nonblocking(true).expect("the relay accept loop is nonblocking");
    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((mut stream, _)) => handle_connection(&mut stream, &mode, &observations, &stop),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                match stop_rx.recv_timeout(RELAY_POLL_INTERVAL) {
                    Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
                    Err(RecvTimeoutError::Timeout) => {}
                }
            }
            Err(error) => panic!("relay accept failed: {error}"),
        }
    }
}

fn handle_connection(stream: &mut StdTcpStream, mode: &RelayMode, observations: &RelayObservations, stop: &AtomicBool) {
    stream.set_read_timeout(Some(Duration::from_millis(1000))).expect("a bounded relay read");
    let mut greeting = [0u8; 2];
    stream.read_exact(&mut greeting).expect("a SOCKS greeting");
    assert_eq!(greeting[0], 5, "SOCKS5 greeting");
    let mut methods = vec![0u8; greeting[1] as usize];
    stream.read_exact(&mut methods).expect("SOCKS methods");
    stream.write_all(&[5, 0]).expect("no-authentication method");

    let mut request_head = [0u8; 4];
    stream.read_exact(&mut request_head).expect("a SOCKS CONNECT request");
    assert_eq!(&request_head[..3], &[5, 1, 0], "SOCKS5 CONNECT");
    let (host, port) = match request_head[3] {
        1 => {
            let mut octets = [0u8; 4];
            stream.read_exact(&mut octets).expect("an IPv4 target");
            (format!("{}.{}.{}.{}", octets[0], octets[1], octets[2], octets[3]), read_port(stream))
        }
        3 => {
            let mut length = [0u8; 1];
            stream.read_exact(&mut length).expect("a domain length");
            let mut name = vec![0u8; length[0] as usize];
            stream.read_exact(&mut name).expect("a domain target");
            (String::from_utf8(name).expect("an ASCII target"), read_port(stream))
        }
        other => panic!("unsupported SOCKS address type {other}"),
    };
    stream.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).expect("a SOCKS success reply");
    observations.handshake.store(true, Ordering::SeqCst);
    *observations.connect_target.lock().expect("target lock") = format!("{host}:{port}");

    let mut buffer = [0u8; 4096];
    let read = stream.read(&mut buffer).expect("an HTTP request through the relay");
    assert_ne!(read, 0, "the client sends a request");
    *observations.request.lock().expect("request lock") = String::from_utf8_lossy(&buffer[..read]).into_owned();
    observations.request_seen.store(true, Ordering::SeqCst);

    match mode {
        RelayMode::Complete(body) => {
            let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            stream.write_all(head.as_bytes()).expect("response headers");
            stream.write_all(body).expect("response body");
            stream.flush().expect("a flushed response");
            observations.server_closed.store(true, Ordering::SeqCst);
        }
        RelayMode::Stall { request_seen, release, observed } => {
            let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {FULL_BODY_LEN}\r\nConnection: close\r\n\r\n");
            stream.write_all(head.as_bytes()).expect("response headers");
            stream.write_all(PARTIAL_BODY).expect("a partial response body");
            stream.flush().expect("a flushed partial response");
            let _ = request_seen.try_send(());
            while !stop.load(Ordering::SeqCst) {
                match release.recv_timeout(RELAY_POLL_INTERVAL) {
                    Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
                    Err(RecvTimeoutError::Timeout) => {}
                }
            }
            let mut byte = [0u8; 1];
            match stream.read(&mut byte) {
                Ok(_) => observations.peer_closed.store(true, Ordering::SeqCst),
                Err(error) if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {
                    observations.timed_out.store(true, Ordering::SeqCst)
                }
                Err(_) => observations.peer_closed.store(true, Ordering::SeqCst),
            }
            let _ = observed.try_send(());
        }
    }
}

fn read_port(stream: &mut StdTcpStream) -> u16 {
    let mut port = [0u8; 2];
    stream.read_exact(&mut port).expect("a target port");
    u16::from_be_bytes(port)
}

fn dial_relay(address: SocketAddr, counter: Arc<ConnectionCounter>, retained: &RetainedConnections) -> TrackedStream {
    let mut stream = StdTcpStream::connect(address).expect("the relay accepts a connection");
    stream.set_read_timeout(Some(Duration::from_millis(1000))).expect("a bounded SOCKS reply");
    stream.set_write_timeout(Some(Duration::from_millis(1000))).expect("a bounded SOCKS request");
    stream.write_all(&[5, 1, 0]).expect("the SOCKS greeting");
    let mut method = [0u8; 2];
    stream.read_exact(&mut method).expect("the SOCKS method reply");
    assert_eq!(method, [5, 0], "the relay selects no authentication");
    let host = TARGET_HOST.as_bytes();
    let mut connect = vec![5, 1, 0, 3, host.len() as u8];
    connect.extend_from_slice(host);
    connect.extend_from_slice(&TARGET_PORT.to_be_bytes());
    stream.write_all(&connect).expect("the SOCKS CONNECT request");
    let mut reply = [0u8; 10];
    stream.read_exact(&mut reply).expect("the SOCKS CONNECT reply");
    assert_eq!(&reply[..2], &[5, 0], "the relay accepts the CONNECT");
    retained.retain(&stream);
    stream.set_nonblocking(true).expect("a nonblocking relay socket");
    let inner = AsyncTcpStream::try_from(stream).expect("an async relay socket");
    TrackedStream { inner, _guard: ConnectionGuard::new(counter) }
}

fn make_request() -> PreparedRequest {
    let info = RequestInfo {
        url: RequestUrl::parse("http://manganese.invalid:8123/witness").expect("a canonical request URL"),
        method: b"GET".to_vec(),
        cache_load: CacheLoad::Normal,
        upload: UploadBody::None,
        caller_headers: Vec::new(),
        referrer: None,
        user_agent: b"manganese-assembly-witness/1".to_vec(),
        accept_language: Vec::new(),
        cookie_line: Vec::new(),
    };
    PreparedRequest::new(&info).expect("the request builds")
}

pub fn fetch(relay: &Relay) -> Vec<u8> {
    let executor = smol::LocalExecutor::new();
    let request = make_request();
    let address = relay.address();
    let counter = relay.counter().clone();
    let retained = RetainedConnections::default();
    smol::block_on(executor.run(async {
        let tasks: Rc<RefCell<Vec<Task<()>>>> = Rc::new(RefCell::new(Vec::new()));
        let handles = tasks.clone();
        let exec = &executor;
        let spawn = move |future: Pin<Box<dyn Future<Output = ()>>>| {
            let task = exec.spawn(async move { future.await });
            handles.borrow_mut().push(task);
        };
        let mut dial = || {
            let stream = dial_relay(address, counter.clone(), &retained);
            Connected { stream, liveness: Arc::new(TestLiveness), verdict: Arc::new(ConnectionVerdict::default()) }
        };
        let mut response = client::send(&request, &mut dial, &spawn).await.expect("the accepted client completes the fetch");
        let mut body = Vec::new();
        while let Some(chunk) = response.next_chunk().await {
            body.extend_from_slice(&chunk.expect("a body chunk"));
        }
        retained.release();
        tasks.borrow_mut().clear();
        body
    }))
}

pub fn endpoint() -> Endpoint {
    let mut config = launch::config();
    config.protocol = ProtocolSource::Bytes(Vec::new());
    Endpoint::start(
        SocketAddr::from(([127, 0, 0, 1], 0)),
        config,
        launch::registry(Box::new(SequenceIds::new())),
    )
    .expect("the DevTools endpoint starts")
}

#[derive(Debug)]
pub struct CancellationWiring {
    pub propagate: bool,
    pub cleanup: bool,
}

#[derive(Debug)]
pub struct CancellationObservation {
    pub outcome: &'static str,
    pub body_len: usize,
    pub active_connections: usize,
    pub retained_connections: usize,
    pub peer_closed: bool,
    pub relay_timed_out: bool,
    pub request_seen: bool,
}

pub fn run_cancellation(wiring: CancellationWiring) -> CancellationObservation {
    let progress = Arc::new(AtomicUsize::new(0));
    let (request_seen_tx, request_seen_rx) = channel::bounded(1);
    let (release_tx, release_rx) = sync_channel::sync_channel(1);
    let (observed_tx, observed_rx) = channel::bounded(1);
    let (first_chunk_tx, first_chunk_rx) = channel::bounded(1);
    let (cancel_trigger, deadline) = Deadline::controlled();
    let (watchdog_trigger, watchdog_deadline) = Deadline::controlled();
    let mut relay = spawn_relay(RelayMode::Stall {
        request_seen: request_seen_tx,
        release: release_rx,
        observed: observed_tx,
    });
    let address = relay.address();
    let counter = relay.counter().clone();
    let retained = RetainedConnections::default();
    let executor = smol::LocalExecutor::new();
    let request = make_request();
    let (outcome, active, body_len, retained_count) = smol::block_on(executor.run(async {
        let tasks: Rc<RefCell<Vec<Task<()>>>> = Rc::new(RefCell::new(Vec::new()));
        let handles = tasks.clone();
        let exec = &executor;
        let spawn = move |future: Pin<Box<dyn Future<Output = ()>>>| {
            let task = exec.spawn(async move { future.await });
            handles.borrow_mut().push(task);
        };
        let mut dial = || {
            let stream = dial_relay(address, counter.clone(), &retained);
            Connected { stream, liveness: Arc::new(TestLiveness), verdict: Arc::new(ConnectionVerdict::default()) }
        };
        let observed_progress = progress.clone();
        let chunk_signal = first_chunk_tx.clone();
        let fetch = async {
            let mut response = match client::send(&request, &mut dial, &spawn).await {
                Ok(response) => response,
                Err(_) => return "failed",
            };
            let mut first = true;
            while let Some(chunk) = response.next_chunk().await {
                match chunk {
                    Ok(chunk) => {
                        observed_progress.fetch_add(chunk.len(), Ordering::SeqCst);
                        if first {
                            first = false;
                            let _ = chunk_signal.try_send(());
                        }
                    }
                    Err(_) => return "failed",
                }
            }
            "complete"
        };
        let cancel = async {
            smol::future::or(
                async {
                    let _ = deadline.wait().await;
                    "canceled"
                },
                async {
                    let _ = watchdog_deadline.wait().await;
                    "watchdog"
                },
            )
            .await
        };
        let mut cancel_trigger = Some(cancel_trigger);
        let mut watchdog_trigger = Some(watchdog_trigger);
        let arm = async {
            let _ = request_seen_rx.recv().await;
            let _ = first_chunk_rx.recv().await;
            let fired = if wiring.propagate {
                cancel_trigger.take().expect("a live cancellation trigger").fire(TRIGGER_ELAPSED)
            } else {
                watchdog_trigger.take().expect("a live watchdog trigger").fire(TRIGGER_ELAPSED)
            };
            fired.expect("the controlled trigger is live");
            release_tx.try_send(()).expect("the relay release signal is delivered");
        };
        let (outcome, ()) = smol::future::zip(smol::future::or(fetch, cancel), arm).await;
        if outcome == "canceled" {
            if wiring.cleanup {
                retained.release();
                tasks.borrow_mut().clear();
            } else {
                for task in tasks.borrow_mut().drain(..) {
                    task.detach();
                }
            }
        }
        let observed = smol::future::or(
            async { observed_rx.recv().await.is_ok() },
            async {
                smol::Timer::after(OBSERVATION_TIMEOUT).await;
                false
            },
        )
        .await;
        assert!(observed, "the relay completed its final read within the bound");
        (outcome, counter.active(), observed_progress.load(Ordering::SeqCst), retained.len())
    }));
    let request_seen = relay.observations().request_seen.load(Ordering::SeqCst);
    let peer_closed = relay.observations().peer_closed.load(Ordering::SeqCst);
    let relay_timed_out = relay.observations().timed_out.load(Ordering::SeqCst);
    relay.stop_and_join();
    CancellationObservation { outcome, body_len, active_connections: active, retained_connections: retained_count, peer_closed, relay_timed_out, request_seen }
}

pub fn http_get(address: SocketAddr, path: &str) -> Vec<u8> {
    let mut stream = StdTcpStream::connect(address).expect("the endpoint accepts a connection");
    stream.set_read_timeout(Some(Duration::from_millis(500))).expect("a bounded endpoint read");
    write!(stream, "GET {path} HTTP/1.1\r\nHost: localhost\r\n\r\n").expect("the request is sent");
    let mut output = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let read = stream.read(&mut chunk).expect("the endpoint answers");
        assert_ne!(read, 0, "the endpoint closed before answering");
        output.extend_from_slice(&chunk[..read]);
        if let Some(split) = output.windows(4).position(|window| window == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&output[..split]).to_ascii_lowercase();
            let length: usize = head
                .split("content-length:")
                .nth(1)
                .and_then(|rest| rest.split("\r\n").next())
                .and_then(|value| value.trim().parse().ok())
                .expect("a content length");
            if output.len() >= split + 4 + length {
                return output;
            }
        }
    }
}

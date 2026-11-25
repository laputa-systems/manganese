use std::io;
use std::net::SocketAddr;
use std::net::TcpListener as StdTcpListener;
use std::sync::Arc;
use std::sync::Weak;
use std::thread::JoinHandle;

use smol::Executor;
use smol::channel::Sender;
use smol::net::TcpListener;
use smol::net::TcpStream;
use smol::prelude::*;

use crate::connection::Connection;
use crate::handler::Config;
use crate::handler::Handler;
use crate::registry::Registry;

const READ_LIMIT: usize = 256 * 1024;

/// A running DevTools endpoint: an HTTP and WebSocket listener answering from a target registry.
/// Dropping it stops the listener and drops every open connection.
pub struct Endpoint {
    address: SocketAddr,
    executor: Arc<Executor<'static>>,
    stop: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for Endpoint {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Endpoint").field("address", &self.address).finish_non_exhaustive()
    }
}

async fn serve_connection(mut stream: TcpStream, handler: Arc<Handler>) {
    let mut connection = Connection::new();
    let mut buffer = vec![0u8; 1024];
    loop {
        let Some(room) = connection.prepare_read() else {
            return;
        };
        let size = room.min(READ_LIMIT);
        if buffer.len() < size {
            buffer.resize(size, 0);
        }
        let read = match stream.read(&mut buffer[..size]).await {
            Ok(0) | Err(_) => return,
            Ok(read) => read,
        };
        let outcome = connection.receive(&handler, &buffer[..read]);
        if !outcome.send.is_empty() && stream.write_all(&outcome.send).await.is_err() {
            return;
        }
        if outcome.close {
            return;
        }
    }
}

async fn accept_loop(listener: TcpListener, handler: Arc<Handler>, executor: Weak<Executor<'static>>) {
    while let Ok((stream, _)) = listener.accept().await {
        let Some(executor) = executor.upgrade() else {
            return;
        };
        executor.spawn(serve_connection(stream, Arc::clone(&handler))).detach();
    }
}

impl Endpoint {
    /// Binds `address` and starts answering from `registry`.
    pub fn start(address: SocketAddr, config: Config, registry: Registry) -> io::Result<Self> {
        let std_listener = StdTcpListener::bind(address)?;
        std_listener.set_nonblocking(true)?;
        let address = std_listener.local_addr()?;
        let listener = TcpListener::try_from(std_listener)?;
        let handler = Arc::new(Handler::new(config, registry, address));
        let executor = Arc::new(Executor::new());
        executor.spawn(accept_loop(listener, handler, Arc::downgrade(&executor))).detach();
        let (stop, stopped) = smol::channel::bounded::<()>(1);
        let runner = Arc::clone(&executor);
        let thread = std::thread::Builder::new().name("devtools-endpoint".into()).spawn(move || {
            smol::block_on(runner.run(async {
                let _ = stopped.recv().await;
            }));
        })?;
        Ok(Self { address, executor, stop: Some(stop), thread: Some(thread) })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.address
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = &self.executor;
    }
}

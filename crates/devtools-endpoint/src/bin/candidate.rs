//! The devtools-endpoint box's candidate: replays each connection script against a fresh endpoint in the launch
//! state and returns the canonical form of everything the endpoint sent.

use std::io::ErrorKind;
use std::io::Read;
use std::io::Write;
use std::net::SocketAddr;
use std::net::TcpStream;
use std::process::ExitCode;
use std::time::Duration;

use devtools_endpoint::canonical::INVALID_SCRIPT;
use devtools_endpoint::canonical::canonical;
use devtools_endpoint::endpoint::Endpoint;
use devtools_endpoint::launch;
use devtools_endpoint::registry::RandomIds;
use devtools_endpoint::rename::rename_generated_ids;
use devtools_endpoint::wire::Script;
use devtools_endpoint::wire::serve_box;

fn replay(script: &Script) -> std::io::Result<(bool, Vec<u8>)> {
    let endpoint = Endpoint::start(SocketAddr::from(([127, 0, 0, 1], 0)), launch::config(), launch::registry(Box::new(RandomIds)))?;
    let mut client = TcpStream::connect(endpoint.local_addr())?;
    client.set_nodelay(true)?;
    let mut sent = Vec::new();
    for segment in &script.segments {
        if client.write_all(&segment.data).is_err() {
            break;
        }
        sent.extend_from_slice(&segment.data);
        if segment.pause_ms > 0 {
            std::thread::sleep(Duration::from_millis(u64::from(segment.pause_ms)));
        }
    }
    client.set_read_timeout(Some(Duration::from_millis(u64::from(script.idle_ms.max(1)))))?;
    let mut received = Vec::new();
    let mut closed = false;
    let mut chunk = vec![0u8; 1 << 20];
    loop {
        match client.read(&mut chunk) {
            Ok(0) => {
                closed = true;
                break;
            }
            Ok(count) => received.extend_from_slice(&chunk[..count]),
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => break,
            Err(error) if matches!(error.kind(), ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted | ErrorKind::BrokenPipe) => {
                closed = true;
                break;
            }
            Err(error) => return Err(error),
        }
    }
    let fixed = [launch::BROWSER_ID, launch::PAGE_ID, launch::OMNIBOX_ID, launch::OMNIBOX_AIM_ID];
    Ok((closed, rename_generated_ids(&received, &sent, &fixed)))
}

fn answer(request: &[u8]) -> std::io::Result<Vec<u8>> {
    let Ok(script) = Script::decode(request) else {
        return Ok(INVALID_SCRIPT.to_vec());
    };
    let (closed, raw) = replay(&script)?;
    Ok(canonical(closed, &raw))
}

fn main() -> ExitCode {
    let mut failure = None;
    let result = serve_box(|request| match answer(request) {
        Ok(reply) => reply,
        Err(error) => {
            failure.get_or_insert(error);
            Vec::new()
        }
    });
    if let Some(error) = failure.or(result.err()) {
        let _ = writeln!(std::io::stderr(), "devtools-endpoint: {error}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

//! The devtools-endpoint box's oracle adapter: relays each request to the oracle daemon
//! (oracle/harness/devtools-endpoint/daemon.py) on 127.0.0.1:47221 and canonicalizes its reply.

use std::io::{self, Write};
use std::net::TcpStream;
use std::process::ExitCode;

use devtools_endpoint::{canonical::{canonical, INVALID_SCRIPT}, wire};

const DAEMON: &str = "127.0.0.1:47221";

fn relay(request: &[u8]) -> io::Result<Vec<u8>> {
    let Ok(script) = wire::Script::decode(request) else {
        return Ok(INVALID_SCRIPT.to_vec());
    };
    let mut connection = TcpStream::connect(DAEMON)?;
    wire::write_frame(&mut connection, &script.encode())?;
    let reply = wire::read_frame(&mut connection)?.ok_or_else(|| io::Error::other("oracle daemon closed without a reply"))?;
    let (closed, raw) = reply.split_first().ok_or_else(|| io::Error::other("empty oracle reply"))?;
    Ok(canonical(*closed != 0, raw))
}

fn main() -> ExitCode {
    let mut failure = None;
    let result = wire::serve_box(|request| match relay(request) {
        Ok(reply) => reply,
        Err(error) => {
            failure.get_or_insert(error);
            Vec::new()
        }
    });
    if let Some(error) = failure.or(result.err()) {
        let _ = writeln!(io::stderr(), "devtools-endpoint-oracle: {error}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

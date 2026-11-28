mod chromium_config;
mod database;
mod engine;
mod meta_table;
mod session;
mod sql_value;
mod wire;

use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;

use session::Session;

const REQUEST_FLAG: &str = "--request";
const REQUEST_DIRECTORY: &str = "r0";

fn main() {
    let Some(scratch) = std::env::var_os("GOLDENS_SCRATCH").filter(|value| !value.is_empty()) else {
        wire::fail("GOLDENS_SCRATCH is not set");
    };
    let scratch = PathBuf::from(scratch);
    // SAFETY: main runs before any other thread exists, so no thread reads the environment concurrently.
    unsafe { std::env::set_var("TURSO_TMPDIR", &scratch) };
    let mut arguments = std::env::args_os().skip(1);
    match (arguments.next(), arguments.next()) {
        (None, _) => serve(),
        (Some(flag), None) if flag == REQUEST_FLAG => serve_one(&scratch.join(REQUEST_DIRECTORY)),
        _ => wire::fail("usage: sqlite [--request]"),
    }
}

fn serve() {
    let Ok(program) = std::env::current_exe() else {
        wire::fail("cannot locate the candidate executable");
    };
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    let mut magic = [0u8; 4];
    if input.read_exact(&mut magic).is_err() || &magic != b"MGB1" {
        wire::fail("missing MGB1 header");
    }
    if output.write_all(b"MGB1").is_err() || output.flush().is_err() {
        wire::fail("cannot write header");
    }
    while let Some(request) = wire::read_frame(&mut input) {
        let response = isolated(&program, &request);
        wire::write_frame(&mut output, &response);
    }
}

fn isolated(program: &Path, request: &[u8]) -> Vec<u8> {
    let Ok(mut child) = Command::new(program).arg(REQUEST_FLAG).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn()
    else {
        wire::fail("cannot start a request process");
    };
    let mut stdin = child.stdin.take().expect("the request process has a piped stdin");
    if stdin.write_all(request).is_err() {
        wire::fail("cannot send a request to its process");
    }
    drop(stdin);
    let Ok(finished) = child.wait_with_output() else {
        wire::fail("cannot wait for a request process");
    };
    if !finished.status.success() {
        eprintln!("sqlite candidate: request process {}", finished.status);
        std::process::exit(finished.status.code().filter(|code| *code != 0).unwrap_or(2));
    }
    finished.stdout
}

fn serve_one(root: &Path) {
    let mut request = Vec::new();
    if std::io::stdin().lock().read_to_end(&mut request).is_err() {
        wire::fail("cannot read request");
    }
    let response = Session::new(root.to_path_buf()).run(&request);
    let mut output = std::io::stdout().lock();
    if output.write_all(&response).is_err() || output.flush().is_err() {
        wire::fail("cannot write response");
    }
}

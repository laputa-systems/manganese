mod chromium_config;
mod database;
mod engine;
mod meta_table;
mod session;
mod sql_value;
mod wire;

use std::io::Read;
use std::io::Write;
use std::path::PathBuf;

use session::Session;

fn main() {
    let Some(scratch) = std::env::var_os("GOLDENS_SCRATCH").filter(|value| !value.is_empty()) else {
        wire::fail("GOLDENS_SCRATCH is not set");
    };
    let scratch = PathBuf::from(scratch);
    // SAFETY: main runs before any other thread exists, so no thread reads the environment concurrently.
    unsafe { std::env::set_var("TURSO_TMPDIR", &scratch) };
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    let mut magic = [0u8; 4];
    if input.read_exact(&mut magic).is_err() || &magic != b"MGB1" {
        wire::fail("missing MGB1 header");
    }
    if output.write_all(b"MGB1").is_err() || output.flush().is_err() {
        wire::fail("cannot write header");
    }
    let mut counter: u64 = 0;
    while let Some(request) = wire::read_frame(&mut input) {
        let root = scratch.join(format!("r{counter}"));
        counter += 1;
        let response = Session::new(root).run(&request);
        wire::write_frame(&mut output, &response);
    }
}

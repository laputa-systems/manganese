mod avio;
mod bits;
mod codec;
mod demuxer;
mod flac;
mod flac_decoder;
mod flac_demux;
mod flac_parser;
mod dict;
mod format;
mod id3v2;
mod mp3;
mod mpegaudio;
mod ogg;
mod parser;
mod probe;
mod rational;
mod wav;

use std::io::Read;
use std::io::Write;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    let mut magic = [0u8; 4];
    if input.read_exact(&mut magic).is_err() || &magic != b"MGB1" {
        eprintln!("media-demux: missing MGB1 header");
        return ExitCode::FAILURE;
    }
    if output.write_all(b"MGB1").and_then(|()| output.flush()).is_err() {
        return ExitCode::FAILURE;
    }
    loop {
        let mut length = [0u8; 4];
        match input.read(&mut length[..1]) {
            Ok(0) => return ExitCode::SUCCESS,
            Ok(_) => {}
            Err(error) => {
                eprintln!("media-demux: {error}");
                return ExitCode::FAILURE;
            }
        }
        if input.read_exact(&mut length[1..]).is_err() {
            eprintln!("media-demux: truncated request length");
            return ExitCode::FAILURE;
        }
        let mut request = vec![0u8; u32::from_be_bytes(length) as usize];
        if input.read_exact(&mut request).is_err() {
            eprintln!("media-demux: truncated request");
            return ExitCode::FAILURE;
        }
        let response = match demuxer::demux(&request) {
            Ok(response) => response,
            Err(format::Unemulated(path)) => {
                eprintln!("media-demux: input reaches an unported FFmpeg path: {path}");
                return ExitCode::from(3);
            }
        };
        let bytes = response.as_bytes();
        let written = output
            .write_all(&(bytes.len() as u32).to_be_bytes())
            .and_then(|()| output.write_all(bytes))
            .and_then(|()| output.flush());
        if written.is_err() {
            return ExitCode::FAILURE;
        }
    }
}

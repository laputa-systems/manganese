use std::io::{self, Read, Write};

mod chromium;

use snap::raw::{Decoder, decompress_len};

const MAX_REQUEST: usize = 4 << 20;

fn get32(bytes: &[u8]) -> u32 {
    u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn put32(value: usize) -> [u8; 4] {
    (value as u32).to_be_bytes()
}

fn fail() -> Vec<u8> {
    let mut response = Vec::with_capacity(5);
    response.push(1);
    response.extend_from_slice(&0u32.to_be_bytes());
    response
}

fn answer(status: u8, bytes: &[u8]) -> Vec<u8> {
    let mut response = Vec::with_capacity(5 + bytes.len());
    response.push(status);
    response.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    response.extend_from_slice(bytes);
    response
}

fn record(input: &[u8], decoder: &mut Decoder) -> Vec<u8> {
    let Some(&op) = input.first() else {
        return fail();
    };
    if op == 0 {
        if input.len() < 5 {
            return fail();
        }
        let size = get32(&input[1..5]) as usize;
        if size > MAX_REQUEST || input.len() != size + 5 {
            return fail();
        }
        answer(0, &chromium::compress(&input[5..]))
    } else if op == 1 {
        if input.len() < 9 {
            return fail();
        }
        let limit = get32(&input[1..5]) as usize;
        let size = get32(&input[5..9]) as usize;
        if size > MAX_REQUEST || input.len() != size + 9 {
            return fail();
        }
        let compressed = &input[9..];
        let Ok(expected) = decompress_len(compressed) else {
            return fail();
        };
        if expected > limit {
            return fail();
        }
        let mut output = vec![0u8; expected];
        match decoder.decompress(compressed, &mut output) {
            Ok(_) => answer(0, &output),
            Err(_) => fail(),
        }
    } else {
        fail()
    }
}

fn main() {
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let stdout = io::stdout();
    let mut output = io::BufWriter::new(stdout.lock());
    let mut header = [0u8; 4];
    if input.read_exact(&mut header).is_err() || &header != b"MGB1" {
        std::process::exit(1);
    }
    if output.write_all(b"MGB1").is_err() || output.flush().is_err() {
        std::process::exit(1);
    }
    let mut decoder = Decoder::new();
    loop {
        match input.read(&mut header[..1]) {
            Ok(0) => break,
            Ok(1) => {}
            _ => std::process::exit(1),
        }
        if input.read_exact(&mut header[1..]).is_err() {
            std::process::exit(1);
        }
        let size = get32(&header) as usize;
        if size > MAX_REQUEST {
            std::process::exit(1);
        }
        let mut request = vec![0u8; size];
        if input.read_exact(&mut request).is_err() {
            std::process::exit(1);
        }
        let response = record(&request, &mut decoder);
        if output.write_all(&put32(response.len())).is_err()
            || output.write_all(&response).is_err()
            || output.flush().is_err()
        {
            std::process::exit(1);
        }
    }
}

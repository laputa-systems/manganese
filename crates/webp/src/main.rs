use img_webp::decode_bgra;
use std::io::{self, Read, Write};

fn put32(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(&value.to_be_bytes());
}

fn decode(input: &[u8]) -> Option<Vec<u8>> {
    if input.first() != Some(&0) {
        return None;
    }
    let mut output = Vec::new();
    match decode_bgra(&input[1..]) {
        None => put32(&mut output, 0),
        Some(decoded) => {
            put32(&mut output, 1);
            put32(&mut output, decoded.features.width);
            put32(&mut output, decoded.features.height);
            put32(&mut output, decoded.features.has_alpha as u32);
            put32(&mut output, decoded.features.frame_count);
            put32(&mut output, decoded.features.width);
            put32(&mut output, decoded.features.height);
            put32(&mut output, decoded.bgra.len() as u32);
            output.extend_from_slice(&decoded.bgra);
        }
    }
    Some(output)
}

fn main() -> io::Result<()> {
    let mut stdin = io::stdin().lock();
    let mut stdout = io::stdout().lock();
    let mut header = [0u8; 4];
    stdin.read_exact(&mut header)?;
    if &header != b"MGB1" {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "protocol header"));
    }
    stdout.write_all(b"MGB1")?;
    stdout.flush()?;
    loop {
        match stdin.read_exact(&mut header) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(error) => return Err(error),
        }
        let mut input = vec![0u8; u32::from_be_bytes(header) as usize];
        stdin.read_exact(&mut input)?;
        let output = decode(&input).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "malformed request"))?;
        stdout.write_all(&(output.len() as u32).to_be_bytes())?;
        stdout.write_all(&output)?;
        stdout.flush()?;
    }
}

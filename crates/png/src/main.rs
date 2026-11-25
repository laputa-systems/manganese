mod codec;
mod decoder;
mod encode;
mod stream;
mod swizzle;

use std::io::{self, Read, Write};

fn take_u32(input: &[u8], offset: &mut usize) -> Result<u32, &'static str> {
    let bytes = input.get(*offset..*offset + 4).ok_or("short record")?;
    *offset += 4;
    Ok(u32::from_be_bytes(bytes.try_into().map_err(|_| "length")?))
}

fn decode_request(input: &[u8]) -> Result<Vec<u8>, &'static str> {
    let mut offset = 1;
    let count = take_u32(input, &mut offset)?;
    let mut data = Vec::new();
    let mut output = Vec::new();
    for index in 0..count {
        let size = take_u32(input, &mut offset)? as usize;
        let chunk = input.get(offset..offset.checked_add(size).ok_or("length")?).ok_or("short chunk")?;
        offset += size;
        data.extend_from_slice(chunk);
        let reception = if index + 1 == count { decoder::Reception::Complete } else { decoder::Reception::Partial };
        decoder::step(&data, reception, &mut output);
    }
    if offset != input.len() {
        return Err("trailing input");
    }
    Ok(output)
}

fn encode_request(input: &[u8]) -> Result<Vec<u8>, &'static str> {
    let mut offset = 1;
    let width = take_u32(input, &mut offset)?;
    let height = take_u32(input, &mut offset)?;
    let mode = take_u32(input, &mut offset)?;
    let pixels = &input[offset..];
    if mode >= 15 || width == 0 || height == 0 || pixels.len() % 4 != 0 || u64::from(width) * u64::from(height) != (pixels.len() / 4) as u64 {
        return Err("invalid encode request");
    }
    Ok(encode::encode(width, height, mode, pixels).unwrap_or_default())
}

fn record(input: &[u8]) -> Result<Vec<u8>, &'static str> {
    match input.first() {
        Some(0) => decode_request(input),
        Some(1) => encode_request(input),
        _ => Err("invalid operation"),
    }
}

fn main() -> io::Result<()> {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let mut header = [0; 4];
    input.read_exact(&mut header)?;
    if &header != b"MGB1" {
        return Err(io::Error::other("invalid protocol"));
    }
    output.write_all(b"MGB1")?;
    output.flush()?;
    loop {
        if input.read(&mut header[..1])? == 0 {
            break;
        }
        input.read_exact(&mut header[1..])?;
        let mut bytes = vec![0; u32::from_be_bytes(header) as usize];
        input.read_exact(&mut bytes)?;
        let result = record(&bytes).map_err(io::Error::other)?;
        output.write_all(&(result.len() as u32).to_be_bytes())?;
        output.write_all(&result)?;
        output.flush()?;
    }
    Ok(())
}

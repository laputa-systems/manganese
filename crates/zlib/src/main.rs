use std::io::{self, Read, Write};
use zlib_rs::{Deflate, DeflateConfig, DeflateFlush, Inflate, InflateFlush, Status, Strategy};

fn status(value: Result<Status, i32>) -> i32 {
    match value { Ok(Status::Ok) => 0, Ok(Status::StreamEnd) => 1, Ok(Status::BufError) => -5, Err(n) => n }
}

fn record(input: &[u8]) -> Result<Vec<u8>, &'static str> {
    if input.len() < 7 { return Err("short record"); }
    let op = input[0];
    let window = i32::from(input[2] as i8);
    let strategy = match input[4] { 0 => Strategy::Default, 1 => Strategy::Filtered, 2 => Strategy::HuffmanOnly, 3 => Strategy::Rle, 4 => Strategy::Fixed, _ => return Err("invalid strategy") };
    let mut deflater = if op == 0 {
        if window == -8 || window == 24 { return Ok([(-2i32).to_be_bytes(), 0u32.to_be_bytes()].concat()); }
        Some(Deflate::new_with_config(DeflateConfig { level: i32::from(input[1] as i8), window_bits: window, mem_level: i32::from(input[3]), strategy, ..DeflateConfig::default() }))
    } else { None };
    let mut inflater = if op == 1 { Some(Inflate::new(window > 0, window.unsigned_abs() as u8)) } else { None };
    let chunks = u16::from_be_bytes([input[5], input[6]]);
    let mut pos = 7usize;
    let mut output = vec![0; 4];
    let mut code = 0i32;
    let mut checksum = if op == 3 || op == 5 { 1 } else { 0 };
    for _ in 0..chunks {
        if input.len().saturating_sub(pos) < 5 { return Err("short chunk"); }
        let n = u32::from_be_bytes(input[pos..pos+4].try_into().map_err(|_| "length")?) as usize;
        let flush = input[pos+4]; pos += 5;
        let end = pos.checked_add(n).filter(|end| *end <= input.len()).ok_or("short data")?;
        let data = &input[pos..end]; pos = end;
        if op == 2 { checksum = zlib_rs::crc32::crc32(checksum, data); continue; }
        if op == 3 { checksum = zlib_rs::adler32::adler32(checksum, data); continue; }
        if op == 4 { checksum = zlib_rs::crc32::crc32_combine(checksum, zlib_rs::crc32::crc32(0, data), data.len() as u64); continue; }
        if op == 5 { checksum = zlib_rs::adler32::adler32_combine(checksum, zlib_rs::adler32::adler32(1, data), data.len() as u64); continue; }
        if code != 0 { continue; }
        let mut offset = 0;
        loop {
            let mut buf = [0; 65536];
            let (consumed, produced, result) = if let Some(stream) = &mut deflater {
                let before_in = stream.total_in(); let before_out = stream.total_out();
                let mode = match flush { 0 => DeflateFlush::NoFlush, 2 => DeflateFlush::SyncFlush, 3 => DeflateFlush::FullFlush, 4 => DeflateFlush::Finish, _ => return Err("invalid flush") };
                let result = stream.compress(&data[offset..], &mut buf, mode).map_err(|e| e as i32);
                ((stream.total_in()-before_in) as usize, (stream.total_out()-before_out) as usize, result)
            } else if let Some(stream) = &mut inflater {
                let before_in = stream.total_in(); let before_out = stream.total_out();
                let mode = match flush { 0 => InflateFlush::NoFlush, 2 => InflateFlush::SyncFlush, 4 => InflateFlush::Finish, _ => return Err("invalid inflate flush") };
                let result = stream.decompress(&data[offset..], &mut buf, mode).map_err(|e| match e { zlib_rs::InflateError::NeedDict { .. } => 2, zlib_rs::InflateError::StreamError => -2, zlib_rs::InflateError::DataError => -3, zlib_rs::InflateError::MemError => -4 });
                ((stream.total_in()-before_in) as usize, (stream.total_out()-before_out) as usize, result)
            } else { return Err("invalid operation"); };
            offset += consumed; output.extend_from_slice(&buf[..produced]); code = status(result);
            if code != 0 || (offset == data.len() && produced < buf.len()) { break; }
            if consumed == 0 && produced == 0 { break; }
        }
    }
    if pos != input.len() { return Err("trailing input"); }
    output[..4].copy_from_slice(&code.to_be_bytes());
    let message = inflater.as_ref().and_then(Inflate::error_message).or_else(|| deflater.as_ref().and_then(Deflate::error_message)).unwrap_or("");
    output.splice(4..4, (message.len() as u32).to_be_bytes().into_iter().chain(message.bytes()));
    if op >= 2 { output.extend_from_slice(&checksum.to_be_bytes()); }
    Ok(output)
}

fn main() -> io::Result<()> {
    let mut input = io::stdin().lock(); let mut output = io::stdout().lock(); let mut header = [0; 4];
    input.read_exact(&mut header)?;
    if &header != b"MGB1" { return Err(io::Error::other("invalid protocol")); }
    output.write_all(b"MGB1")?; output.flush()?;
    loop {
        if input.read(&mut header[..1])? == 0 { break; }
        input.read_exact(&mut header[1..])?;
        let mut bytes = vec![0; u32::from_be_bytes(header) as usize]; input.read_exact(&mut bytes)?;
        let result = record(&bytes).map_err(io::Error::other)?;
        output.write_all(&(result.len() as u32).to_be_bytes())?; output.write_all(&result)?; output.flush()?;
    }
    Ok(())
}

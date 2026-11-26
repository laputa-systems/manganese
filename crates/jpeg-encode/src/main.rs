use std::io::{self, Read, Write};

use img_jpeg::{Downsample, EncodeColorSpace, EncodeOptions, encode_stream};

const SRGB_ICC: &[u8] = include_bytes!("../data/srgb-icc.bin");

fn take_u32(input: &[u8], offset: &mut usize) -> Result<u32, &'static str> {
    let bytes = input.get(*offset..*offset + 4).ok_or("short record")?;
    *offset += 4;
    Ok(u32::from_be_bytes(bytes.try_into().map_err(|_| "length")?))
}

fn encode_request(input: &[u8]) -> Result<Vec<u8>, &'static str> {
    let mut offset = 1;
    let width = take_u32(input, &mut offset)? as usize;
    let height = take_u32(input, &mut offset)? as usize;
    let requested_quality = take_u32(input, &mut offset)?;
    let quality = requested_quality.min(100) as u8;
    let pixels = &input[offset..];
    if width == 0 || height == 0 || pixels.len() != width * height * 4 {
        return Ok(Vec::new());
    }
    let options = EncodeOptions {
        quality,
        downsample: if requested_quality == 100 { Downsample::S444 } else { Downsample::S420 },
        blend_on_black: true,
        icc: Some(SRGB_ICC),
    };
    Ok(encode_stream(width, height, pixels, EncodeColorSpace::Bgra, &options).unwrap_or_default())
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
        let result = match bytes.first() {
            Some(1) => encode_request(&bytes).map_err(io::Error::other)?,
            _ => return Err(io::Error::other("invalid operation")),
        };
        output.write_all(&(result.len() as u32).to_be_bytes())?;
        output.write_all(&result)?;
        output.flush()?;
    }
    Ok(())
}

use std::io::{self, Read, Write};
use rust_brotli::{Allocator, BrotliDecompressStream, BrotliResult, BrotliState, HeapAlloc, SliceWrapperMut};

fn read_u32(input: &[u8], pos: &mut usize) -> Result<usize, &'static str> {
    let end = pos.checked_add(4).filter(|end| *end <= input.len()).ok_or("short length")?;
    let value = u32::from_be_bytes(input[*pos..end].try_into().map_err(|_| "length")?) as usize;
    *pos = end;
    Ok(value)
}

fn read_bytes<'a>(input: &'a [u8], pos: &mut usize, n: usize) -> Result<&'a [u8], &'static str> {
    let end = pos.checked_add(n).filter(|end| *end <= input.len()).ok_or("short data")?;
    let result = &input[*pos..end];
    *pos = end;
    Ok(result)
}

fn record(input: &[u8]) -> Result<Vec<u8>, &'static str> {
    if input.len() < 3 { return Err("short record"); }
    let mode = input[0];
    if mode > 3 { return Err("invalid mode"); }
    let count = u16::from_be_bytes([input[1], input[2]]);
    let mut pos = 3;
    let output_size = read_u32(input, &mut pos)?;
    if (mode == 0 && output_size == 0) || output_size > 16 * 1024 * 1024 { return Err("invalid output size"); }
    let dict_size = read_u32(input, &mut pos)?;
    if mode != 0 && (dict_size != 0 || count != 1) { return Err("invalid one-shot request"); }
    if mode == 3 && output_size != 32768 { return Err("invalid patch output size"); }
    let dictionary = read_bytes(input, &mut pos, dict_size)?;
    let mut chunks = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let size = read_u32(input, &mut pos)?;
        if mode == 0 && size > 32768 { return Err("oversized input chunk"); }
        chunks.push(read_bytes(input, &mut pos, size)?);
    }
    if pos != input.len() { return Err("trailing record data"); }
    let mut state = BrotliState::new_strict(HeapAlloc::default(), HeapAlloc::default(), HeapAlloc::default());
    if !dictionary.is_empty() {
        let mut memory = state.alloc_u8.alloc_cell(dictionary.len());
        memory.slice_mut().copy_from_slice(dictionary);
        if !state.attach_dictionary(memory) { return Err("invalid dictionary"); }
    }
    let mut response = vec![mode];
    let mut total_out = 0;
    if mode == 1 || mode == 2 {
        let data = chunks[0];
        let mut available_in = data.len();
        let mut input_offset = 0;
        let mut output = vec![0; output_size];
        let mut available_out = output_size;
        let mut output_offset = 0;
        let result = BrotliDecompressStream(&mut available_in, &mut input_offset, data, &mut available_out, &mut output_offset, &mut output, &mut total_out, &mut state);
        let success = mode == 2 || (matches!(result, BrotliResult::ResultSuccess) && total_out == output_size);
        response.extend_from_slice(&(i32::from(success)).to_be_bytes());
        if success { response.extend_from_slice(&output); }
        return Ok(response);
    }
    if mode == 3 {
        let data = chunks[0];
        let mut available_in = data.len();
        let mut input_offset = 0;
        response.extend_from_slice(&1i32.to_be_bytes());
        loop {
            let mut output = vec![0; output_size];
            let mut available_out = output_size;
            let mut output_offset = 0;
            let result = BrotliDecompressStream(&mut available_in, &mut input_offset, data, &mut available_out, &mut output_offset, &mut output, &mut total_out, &mut state);
            match result {
                BrotliResult::ResultFailure | BrotliResult::NeedsMoreInput => {
                    response.truncate(1); response.extend_from_slice(&0i32.to_be_bytes()); break;
                }
                BrotliResult::NeedsMoreOutput => response.extend_from_slice(&output[..output_offset]),
                BrotliResult::ResultSuccess => {
                    response.extend_from_slice(&output[..output_offset]);
                    if available_in != 0 { response.truncate(1); response.extend_from_slice(&(-1i32).to_be_bytes()); }
                    break;
                }
            }
        }
        return Ok(response);
    }
    let mut finished = false;
    let mut failed = false;
    chunks.push(&[]);
    for data in chunks {
        if finished { continue; }
        let mut available_in = data.len();
        let mut input_offset = 0;
        loop {
            let mut output = vec![0; output_size];
            let mut available_out = output_size;
            let mut output_offset = 0;
            let result = BrotliDecompressStream(&mut available_in, &mut input_offset, data, &mut available_out, &mut output_offset, &mut output, &mut total_out, &mut state);
            if matches!(result, BrotliResult::ResultFailure) {
                response.extend_from_slice(&(-330i32).to_be_bytes());
                failed = true; finished = true; break;
            }
            if output_offset != 0 {
                response.extend_from_slice(&(output_offset as i32).to_be_bytes());
                response.extend_from_slice(&output[..output_offset]);
            }
            match result {
                BrotliResult::ResultSuccess => { finished = true; break; }
                BrotliResult::NeedsMoreInput => break,
                BrotliResult::NeedsMoreOutput => continue,
                BrotliResult::ResultFailure => unreachable!(),
            }
        }
    }
    if !failed { response.extend_from_slice(&0i32.to_be_bytes()); }
    Ok(response)
}

fn main() -> io::Result<()> {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let mut header = [0; 4];
    input.read_exact(&mut header)?;
    if &header != b"MGB1" { return Err(io::Error::other("invalid protocol")); }
    output.write_all(b"MGB1")?;
    output.flush()?;
    loop {
        if input.read(&mut header[..1])? == 0 { break; }
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

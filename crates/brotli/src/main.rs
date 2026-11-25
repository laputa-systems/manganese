use rust_brotli::{
    Allocator, BrotliDecoderIsFinished, BrotliDecompressStream, BrotliResult, BrotliState, HeapAlloc, SliceWrapperMut,
};
use std::io::{self, Read, Write};

const STREAM: u8 = 0;
const ONESHOT_EXACT: u8 = 1;
const ONESHOT_BUFFER: u8 = 2;
const PATCH: u8 = 3;
const SHARED: u8 = 4;
const SHARED_DICT: u8 = 5;
const MAX_OUTPUT: usize = 16 * 1024 * 1024;
const MAX_INPUT_CHUNK: usize = 32768;
const DECODING_FAILED: i32 = -330;

type State = BrotliState<HeapAlloc<u8>, HeapAlloc<u32>, HeapAlloc<rust_brotli::HuffmanCode>>;

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], &'static str> {
        let end = self.pos.checked_add(n).filter(|end| *end <= self.bytes.len()).ok_or("short record")?;
        let slice = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn u32(&mut self) -> Result<usize, &'static str> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().map_err(|_| "length")?) as usize)
    }
}

struct Pass {
    result: BrotliResult,
    consumed: usize,
    produced: usize,
}

struct Decoder {
    state: State,
    total_out: usize,
}

impl Decoder {
    fn new() -> Self {
        Decoder {
            state: BrotliState::new_strict(HeapAlloc::default(), HeapAlloc::default(), HeapAlloc::default()),
            total_out: 0,
        }
    }

    fn attach(&mut self, dictionary: &[u8]) -> bool {
        let mut memory = self.state.alloc_u8.alloc_cell(dictionary.len());
        memory.slice_mut().copy_from_slice(dictionary);
        self.state.attach_dictionary(memory)
    }

    fn pass(&mut self, input: &[u8], input_offset: &mut usize, output: &mut [u8]) -> Pass {
        let start = *input_offset;
        let mut available_in = input.len() - start;
        let mut available_out = output.len();
        let mut output_offset = 0;
        let result = BrotliDecompressStream(
            &mut available_in,
            input_offset,
            input,
            &mut available_out,
            &mut output_offset,
            output,
            &mut self.total_out,
            &mut self.state,
        );
        Pass { result, consumed: *input_offset - start, produced: output_offset }
    }

    fn finished(&self) -> bool {
        BrotliDecoderIsFinished(&self.state)
    }
}

fn stream(decoder: &mut Decoder, chunks: &[&[u8]], output_size: usize, response: &mut Vec<u8>) {
    let mut output = vec![0; output_size];
    let mut done = false;
    for index in 0..=chunks.len() {
        let input = chunks.get(index).copied().unwrap_or(&[]);
        let mut offset = 0;
        while !done {
            let pass = decoder.pass(input, &mut offset, &mut output);
            if let BrotliResult::ResultFailure = pass.result {
                response.extend_from_slice(&DECODING_FAILED.to_be_bytes());
                return;
            }
            if pass.produced != 0 {
                response.extend_from_slice(&(pass.produced as i32).to_be_bytes());
                response.extend_from_slice(&output[..pass.produced]);
            }
            if let BrotliResult::ResultSuccess = pass.result {
                done = true;
                break;
            }
            if pass.produced == 0 {
                break;
            }
        }
    }
    response.extend_from_slice(&0i32.to_be_bytes());
}

fn oneshot(mode: u8, input: &[u8], output_size: usize, response: &mut Vec<u8>) {
    let mut decoder = Decoder::new();
    let mut output = vec![0; output_size];
    let mut offset = 0;
    let pass = decoder.pass(input, &mut offset, &mut output);
    let ok = matches!(pass.result, BrotliResult::ResultSuccess) && (mode == ONESHOT_BUFFER || decoder.total_out == output_size);
    response.extend_from_slice(&i32::from(ok).to_be_bytes());
    if ok {
        response.extend_from_slice(&output);
    }
}

fn patch(input: &[u8], output_size: usize, response: &mut Vec<u8>) {
    let mut decoder = Decoder::new();
    let mut output = vec![0; output_size];
    let mut body = Vec::new();
    let mut offset = 0;
    let mut code = 1i32;
    while offset != input.len() || !decoder.finished() {
        let pass = decoder.pass(input, &mut offset, &mut output);
        if matches!(pass.result, BrotliResult::ResultFailure | BrotliResult::NeedsMoreInput) {
            code = 0;
            break;
        }
        if matches!(pass.result, BrotliResult::ResultSuccess) && pass.consumed == 0 && pass.produced == 0 {
            code = -1;
            break;
        }
        body.extend_from_slice(&output[..pass.produced]);
    }
    response.extend_from_slice(&code.to_be_bytes());
    if code == 1 {
        response.extend_from_slice(&body);
    }
}

fn shared(mode: u8, dictionary: &[u8], input: &[u8], output_size: usize, response: &mut Vec<u8>) {
    let mut decoder = Decoder::new();
    if mode == SHARED_DICT && !decoder.attach(dictionary) {
        response.extend_from_slice(&1i32.to_be_bytes());
        return;
    }
    let mut output = vec![0; output_size];
    let mut offset = 0;
    let mut available_out = output_size;
    let mut output_offset = 0;
    loop {
        let mut available_in = input.len() - offset;
        let result = BrotliDecompressStream(
            &mut available_in,
            &mut offset,
            input,
            &mut available_out,
            &mut output_offset,
            &mut output,
            &mut decoder.total_out,
            &mut decoder.state,
        );
        let code = match result {
            BrotliResult::ResultSuccess => break,
            BrotliResult::ResultFailure => 2,
            BrotliResult::NeedsMoreInput if available_in == 0 => 2,
            BrotliResult::NeedsMoreOutput if available_out == 0 => 3,
            _ => continue,
        };
        response.extend_from_slice(&(code as i32).to_be_bytes());
        return;
    }
    if offset != input.len() {
        response.extend_from_slice(&4i32.to_be_bytes());
    } else if decoder.total_out > output_size {
        response.extend_from_slice(&3i32.to_be_bytes());
    } else {
        response.extend_from_slice(&0i32.to_be_bytes());
        response.extend_from_slice(&output[..decoder.total_out]);
    }
}

fn record(input: &[u8]) -> Result<Vec<u8>, &'static str> {
    let mut reader = Reader { bytes: input, pos: 0 };
    let header = reader.take(3)?;
    let mode = header[0];
    let count = usize::from(u16::from_be_bytes([header[1], header[2]]));
    let output_size = reader.u32()?;
    let dict_size = reader.u32()?;
    if mode > SHARED_DICT || output_size > MAX_OUTPUT {
        return Err("invalid request");
    }
    if mode == STREAM {
        if output_size == 0 {
            return Err("invalid output size");
        }
    } else if count != 1 || (mode != SHARED_DICT && dict_size != 0) {
        return Err("invalid one-shot request");
    }
    if mode == PATCH && output_size != 32768 {
        return Err("invalid patch output size");
    }
    let dictionary = reader.take(dict_size)?;
    let mut chunks = Vec::with_capacity(count);
    for _ in 0..count {
        let size = reader.u32()?;
        if mode == STREAM && (size == 0 || size > MAX_INPUT_CHUNK) {
            return Err("invalid input chunk");
        }
        chunks.push(reader.take(size)?);
    }
    if reader.pos != input.len() {
        return Err("trailing record data");
    }
    let mut response = vec![mode];
    match mode {
        STREAM => {
            let mut decoder = Decoder::new();
            if !dictionary.is_empty() && !decoder.attach(dictionary) {
                return Err("invalid dictionary");
            }
            stream(&mut decoder, &chunks, output_size, &mut response);
        }
        ONESHOT_EXACT | ONESHOT_BUFFER => oneshot(mode, chunks[0], output_size, &mut response),
        PATCH => patch(chunks[0], output_size, &mut response),
        SHARED | SHARED_DICT => shared(mode, dictionary, chunks[0], output_size, &mut response),
        _ => return Err("invalid mode"),
    }
    Ok(response)
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

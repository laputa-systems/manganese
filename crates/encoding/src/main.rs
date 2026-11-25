use encoding_rs::{CoderResult, DecoderResult, Encoding};
use std::io::{self, Read, Write};

struct Reader<'a> { data: &'a [u8], offset: usize }
impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8], &'static str> {
        let end = self.offset.checked_add(len).filter(|&n| n <= self.data.len()).ok_or("short request")?;
        let result = &self.data[self.offset..end];
        self.offset = end;
        Ok(result)
    }
    fn byte(&mut self) -> Result<u8, &'static str> { Ok(self.take(1)?[0]) }
    fn length(&mut self) -> Result<usize, &'static str> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().map_err(|_| "length")?) as usize)
    }
    fn blob(&mut self) -> Result<&'a [u8], &'static str> { let len = self.length()?; self.take(len) }
}

fn decode(enc: &'static Encoding, mode: u8, fatal: bool, chunks: &[&[u8]]) -> Result<Vec<u8>, &'static str> {
    let mut decoder = match mode {
        0 => enc.new_decoder_without_bom_handling(),
        1 => enc.new_decoder_with_bom_removal(),
        2 => enc.new_decoder(),
        _ => return Err("invalid BOM mode"),
    };
    let mut output = Vec::new();
    let mut saw_error = false;
    for (i, chunk) in chunks.iter().enumerate() {
        let last = i + 1 == chunks.len();
        let capacity = decoder.max_utf16_buffer_length(chunk.len()).ok_or("input too large")?;
        let mut units = vec![0u16; capacity.max(2)];
        let (written, error) = if fatal {
            let (result, _, written) = decoder.decode_to_utf16_without_replacement(chunk, &mut units, last);
            match result {
                DecoderResult::InputEmpty => (written, false),
                DecoderResult::Malformed(_, _) => (0, true),
                DecoderResult::OutputFull => return Err("decode buffer too small"),
            }
        } else {
            let (result, _, written, error) = decoder.decode_to_utf16(chunk, &mut units, last);
            if result != CoderResult::InputEmpty { return Err("decode buffer too small"); }
            (written, error)
        };
        saw_error |= error;
        output.extend_from_slice(&(written as u32).to_be_bytes());
        for unit in &units[..written] { output.extend_from_slice(&unit.to_be_bytes()); }
        output.push(u8::from(error));
        if fatal && error { break; }
    }
    let mut result = vec![u8::from(saw_error)];
    result.extend(output);
    Ok(result)
}

fn encode(enc: &'static Encoding, input: &[u8]) -> Result<Vec<u8>, &'static str> {
    if input.len() % 2 != 0 { return Err("odd UTF-16 length"); }
    let input: Vec<u16> = input.chunks_exact(2).map(|b| u16::from_be_bytes([b[0], b[1]])).collect();
    let mut encoder = enc.new_encoder();
    let capacity = input.len().checked_mul(16).and_then(|n| n.checked_add(64)).ok_or("input too large")?;
    let mut output = vec![0; capacity];
    let (result, _, written, _) = encoder.encode_from_utf16(&input, &mut output, true);
    if result != CoderResult::InputEmpty { return Err("encode buffer too small"); }
    output.truncate(written);
    Ok(output)
}

fn record(input: &[u8]) -> Result<Vec<u8>, &'static str> {
    let mut reader = Reader { data: input, offset: 0 };
    let op = reader.byte()?;
    let label = reader.blob()?;
    let enc = Encoding::for_label(label);
    if op == 0 {
        if reader.offset != input.len() { return Err("trailing request"); }
        return Ok(enc.map_or_else(Vec::new, |encoding| encoding.name().to_ascii_lowercase().into_bytes()));
    }
    let enc = enc.ok_or("unknown label")?;
    let response = match op {
        1 => {
            let mode = reader.byte()?;
            let fatal = match reader.byte()? { 0 => false, 1 => true, _ => return Err("invalid fatal flag") };
            let count = reader.length()?;
            if count == 0 || count > 65536 { return Err("invalid chunk count"); }
            let mut chunks = Vec::with_capacity(count);
            for _ in 0..count { chunks.push(reader.blob()?); }
            decode(enc, mode, fatal, &chunks)?
        }
        2 => encode(enc, reader.blob()?)?,
        _ => return Err("unknown operation"),
    };
    if reader.offset != input.len() { return Err("trailing request"); }
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
        let len = u32::from_be_bytes(header) as usize;
        if len > 16 * 1024 * 1024 { return Err(io::Error::other("request too large")); }
        let mut bytes = vec![0; len];
        input.read_exact(&mut bytes)?;
        let response = record(&bytes).map_err(io::Error::other)?;
        output.write_all(&(response.len() as u32).to_be_bytes())?;
        output.write_all(&response)?;
        output.flush()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn streaming_utf8_and_fatal_error() {
        let enc = Encoding::for_label(b"utf-8").unwrap();
        assert_eq!(decode(enc, 1, false, &[b"\xef", b"\xbb\xbf\xe2", b"\x82\xac"]).unwrap(), [0, 0,0,0,0,0, 0,0,0,0,0, 0,0,0,1,0x20,0xac,0]);
        assert_eq!(decode(enc, 0, true, &[b"a\xff"]).unwrap(), [1, 0,0,0,0,1]);
    }
    #[test]
    fn ncr_for_unencodable_input() {
        let enc = Encoding::for_label(b"windows-1252").unwrap();
        assert_eq!(encode(enc, &[0x20, 0xac, 0x4e, 0x2d]).unwrap(), b"\x80&#20013;");
    }
    #[test]
    fn label_lookup_canonicalizes_and_rejects_unknown_names() {
        assert_eq!(record(b"\x00\x00\x00\x00\x05UtF-8").unwrap(), b"utf-8");
        assert!(record(b"\x00\x00\x00\x00\x03bad").unwrap().is_empty());
    }
}

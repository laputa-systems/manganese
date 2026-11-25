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

#[derive(Default)]
struct EucJpCompat { lead: u8, jis0212: bool }

impl EucJpCompat {
    fn transform(&mut self, bytes: &[u8]) -> Vec<u8> {
        let mut output = Vec::with_capacity(bytes.len());
        for &byte in bytes {
            let lead = std::mem::take(&mut self.lead);
            if lead != 0 {
                if lead == 0x8f && (0xa1..=0xfe).contains(&byte) {
                    self.jis0212 = true;
                    self.lead = byte;
                } else if (0xa1..=0xfe).contains(&lead) && (0xa1..=0xfe).contains(&byte) {
                    self.jis0212 = false;
                }
            } else if byte == 0x8e || byte == 0x8f || (0xa1..=0xfe).contains(&byte) {
                if self.jis0212 && (0xa1..=0xfe).contains(&byte) { output.push(0x8f); }
                self.lead = byte;
            }
            output.push(byte);
        }
        output
    }
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
    let prefix: Vec<u8> = chunks.iter().flat_map(|chunk| chunk.iter().copied()).take(3).collect();
    let sniffed = mode == 2 && (prefix.starts_with(&[0xef, 0xbb, 0xbf]) || prefix.starts_with(&[0xff, 0xfe]) || prefix.starts_with(&[0xfe, 0xff]));
    let mut euc_jp = (enc == encoding_rs::EUC_JP && !fatal && !sniffed).then(EucJpCompat::default);
    for (i, chunk) in chunks.iter().enumerate() {
        let last = i + 1 == chunks.len();
        let transformed = euc_jp.as_mut().map(|state| state.transform(chunk));
        let chunk = transformed.as_deref().unwrap_or(chunk);
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
            let (result, _, written, _) = decoder.decode_to_utf16(chunk, &mut units, last);
            if result != CoderResult::InputEmpty { return Err("decode buffer too small"); }
            (written, false)
        };
        saw_error |= error;
        if enc == encoding_rs::BIG5 && !sniffed {
            for index in 0..written.saturating_sub(1) {
                if matches!(units[index + 1], 0x0304 | 0x030c) {
                    let first = match units[index] { 0x00ca => 0x0093, 0x00ea => 0x00b3, _ => continue };
                    units[index] = first;
                    units[index + 1] = if units[index + 1] == 0x0304 { 0xdf04 } else { 0xdf0c };
                }
            }
        }
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
    let mut output = Vec::with_capacity(capacity);
    let mut buffer = vec![0; capacity];
    let mut start = 0;
    let mut i = 0;
    while i < input.len() {
        let unit = input[i];
        if (0xd800..=0xdbff).contains(&unit) && i + 1 < input.len() && (0xdc00..=0xdfff).contains(&input[i + 1]) {
            i += 2;
            continue;
        }
        if (0xd800..=0xdfff).contains(&unit) && enc.output_encoding() != encoding_rs::UTF_8 {
            let (result, _, written, _) = encoder.encode_from_utf16(&input[start..i], &mut buffer, false);
            if result != CoderResult::InputEmpty { return Err("encode buffer too small"); }
            output.extend_from_slice(&buffer[..written]);
            if enc == encoding_rs::GB18030 {
                let pointer = 19043 + u32::from(unit) - 0x9fa6;
                output.extend_from_slice(&[(pointer / 12600 + 0x81) as u8, ((pointer % 12600) / 1260 + 0x30) as u8, ((pointer % 1260) / 10 + 0x81) as u8, (pointer % 10 + 0x30) as u8]);
            } else {
                let numeric = matches!(enc.name(), "windows-1252" | "Big5" | "EUC-JP" | "EUC-KR" | "GBK" | "ISO-2022-JP" | "Shift_JIS" | "x-user-defined");
                let replacement = if numeric { format!("&#{};", unit) } else { format!("%U{unit:04X}") };
                let units: Vec<u16> = replacement.encode_utf16().collect();
                let (result, _, written, _) = encoder.encode_from_utf16(&units, &mut buffer, false);
                if result != CoderResult::InputEmpty { return Err("encode buffer too small"); }
                output.extend_from_slice(&buffer[..written]);
            }
            start = i + 1;
        }
        i += 1;
    }
    let (result, _, written, _) = encoder.encode_from_utf16(&input[start..], &mut buffer, true);
    if result != CoderResult::InputEmpty { return Err("encode buffer too small"); }
    output.extend_from_slice(&buffer[..written]);
    Ok(output)
}

fn record(input: &[u8]) -> Result<Vec<u8>, &'static str> {
    let mut reader = Reader { data: input, offset: 0 };
    let op = reader.byte()?;
    if op == 4 {
        let count = reader.length()?;
        if count == 0 || count > 1024 { return Err("invalid batch size"); }
        let mut response = Vec::new();
        for _ in 0..count {
            let item = reader.blob()?;
            if item.first() == Some(&4) { return Err("nested batch"); }
            let result = record(item)?;
            response.extend_from_slice(&(result.len() as u32).to_be_bytes());
            response.extend_from_slice(&result);
        }
        if reader.offset != input.len() { return Err("trailing request"); }
        return Ok(response);
    }
    let label = reader.blob()?;
    let enc = Encoding::for_label(label);
    if op == 0 || op == 5 {
        if reader.offset != input.len() { return Err("trailing request"); }
        if op == 5 && enc == Some(encoding_rs::REPLACEMENT) { return Ok(Vec::new()); }
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
    fn malformed_euc_jp_keeps_blink_jis0212_state() {
        assert_eq!(decode(encoding_rs::EUC_JP, 0, false, &[b"\x8f\xc6", b"\x7d\xcb", b"\xbc"]).unwrap(), [0, 0,0,0,0,0, 0,0,0,2,0xff,0xfd,0,0x7d,0, 0,0,0,1,0x73,0x56,0]);
    }
    #[test]
    fn big5_composite_mappings_match_blink_units() {
        assert_eq!(decode(encoding_rs::BIG5, 0, false, &[b"\x88", b"\xa5"]).unwrap(), [0, 0,0,0,0,0, 0,0,0,2,0,0xb3,0xdf,0x0c,0]);
    }
    #[test]
    fn ncr_for_unencodable_input() {
        let enc = Encoding::for_label(b"windows-1252").unwrap();
        assert_eq!(encode(enc, &[0x20, 0xac, 0x4e, 0x2d]).unwrap(), b"\x80&#20013;");
    }
    #[test]
    fn lone_surrogates_follow_each_codec() {
        let input = [0xd8, 0x00, 0x00, 0x41, 0xdc, 0x00];
        assert_eq!(encode(encoding_rs::WINDOWS_1252, &input).unwrap(), b"&#55296;A&#56320;");
        assert_eq!(encode(encoding_rs::WINDOWS_1251, &input).unwrap(), b"%UD800A%UDC00");
        assert_eq!(encode(encoding_rs::GB18030, &input).unwrap(), b"\x83\x36\xc7\x39A\x83\x37\xb0\x33");
    }
    #[test]
    fn label_lookup_canonicalizes_and_rejects_unknown_names() {
        assert_eq!(record(b"\x00\x00\x00\x00\x05UtF-8").unwrap(), b"utf-8");
        assert!(record(b"\x00\x00\x00\x00\x03bad").unwrap().is_empty());
        assert!(record(b"\x05\x00\x00\x00\x0biso-2022-kr").unwrap().is_empty());
    }
    #[test]
    fn batch_keeps_each_response_separate() {
        let input = b"\x04\x00\x00\x00\x02\x00\x00\x00\x0a\x00\x00\x00\x00\x05UtF-8\x00\x00\x00\x08\x00\x00\x00\x00\x03bad";
        assert_eq!(record(input).unwrap(), b"\x00\x00\x00\x05utf-8\x00\x00\x00\x00");
    }
}

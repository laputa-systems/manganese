use std::io::{self, Read, Write};
use ruzstd::decoding::{BlockDecodingStrategy, FrameDecoder};
use ruzstd::decoding::errors::{FrameDecoderError, FrameHeaderError};

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn complete_frame(bytes: &[u8], mut pos: usize, checksum: bool) -> bool {
    loop {
        let Some(header) = bytes.get(pos..pos + 3) else { return false; };
        let block = u32::from_le_bytes([header[0], header[1], header[2], 0]);
        let kind = (block >> 1) & 3;
        if kind == 3 { return false; }
        let len = if kind == 1 { 1 } else { (block >> 3) as usize };
        pos += 3 + len;
        if pos > bytes.len() { return false; }
        if block & 1 != 0 { return pos + usize::from(checksum) * 4 <= bytes.len(); }
    }
}

fn decode(request: &[u8]) -> Option<Vec<u8>> {
    let output_size = u32_at(request, 0)? as usize;
    let dict_size = u32_at(request, 4)? as usize;
    let chunks = u32_at(request, 8)? as usize;
    if output_size == 0 || output_size > 1048576 { return None; }
    let dictionary = request.get(12..12 + dict_size)?;
    let mut pos = 12 + dict_size;
    let mut pending = Vec::new();
    let mut decoder = FrameDecoder::new();
    let mut window_log = 23;
    while window_log < 27 && (1usize << window_log) < dict_size.saturating_mul(5) / 4 { window_log += 1; }
    decoder.set_max_window_size(1 << window_log);
    let mut active = false;
    let mut checksum = false;
    let mut expected_size = None;
    let mut frame_size = 0u64;
    let mut validate_frame_size = false;
    let mut last_nonempty = false;
    let mut block_limit = 131072usize;
    let mut raw_block: Option<(usize, bool)> = None;
    let mut awaiting_checksum = false;
    let mut skip = 0usize;
    let mut status = 0u32;
    let mut response = vec![0; 4];
    for _ in 0..chunks {
        let len = u32_at(request, pos)? as usize;
        pos += 4;
        let mut fresh = pending.is_empty();
        pending.extend_from_slice(request.get(pos..pos + len)?);
        pos += len;
        let mut produced = Vec::new();
        let mut delivered_frames = 0;
        while status == 0 {
            if awaiting_checksum {
                if pending.len() < 4 { break; }
                let expected = u32::from_le_bytes(pending[..4].try_into().ok()?);
                pending.drain(..4);
                if decoder.get_calculated_checksum() != Some(expected) || ((validate_frame_size || last_nonempty) && expected_size.is_some_and(|size| size != frame_size)) {
                    status = 1;
                    break;
                }
                awaiting_checksum = false;
                active = false;
                delivered_frames = produced.len();
                fresh = true;
                continue;
            }
            if let Some((remaining, last)) = raw_block {
                let count = remaining.min(pending.len());
                decoder.append_raw_content(&pending[..count]).ok()?;
                pending.drain(..count);
                let mut buffer = vec![0; output_size];
                while decoder.can_collect() != 0 {
                    let count = decoder.read(&mut buffer).ok()?;
                    if count == 0 { return None; }
                    produced.extend_from_slice(&buffer[..count]);
                    frame_size += count as u64;
                }
                if remaining > count {
                    raw_block = Some((remaining - count, last));
                    break;
                }
                raw_block = None;
                if last {
                    if (validate_frame_size || last_nonempty) && expected_size.is_some_and(|size| size != frame_size) {
                        status = 1;
                        break;
                    }
                    if checksum { awaiting_checksum = true; } else { active = false; delivered_frames = produced.len(); fresh = true; }
                }
                continue;
            }
            if skip != 0 {
                let take = skip.min(pending.len());
                pending.drain(..take);
                skip -= take;
                if skip != 0 { break; }
            }
            if !active {
                if pending.len() < 4 {
                    let n = pending.len();
                    let normal = b"\x28\xb5\x2f\xfd".starts_with(&pending);
                    let skippable = n == 0 || ((0x50..=0x5f).contains(&pending[0]) && b"\x2a\x4d\x18".starts_with(&pending[1..]));
                    if !normal && !skippable { status = 1; }
                    break;
                }
                let magic = u32::from_le_bytes(pending[..4].try_into().ok()?);
                if (0x184d2a50..=0x184d2a5f).contains(&magic) {
                    if pending.len() < 8 { break; }
                    skip = u32::from_le_bytes(pending[4..8].try_into().ok()?) as usize;
                    pending.drain(..8);
                    continue;
                }
                if magic != 0xfd2fb528 { status = 1; break; }
                if pending.len() < 5 { break; }
                let desc = pending[4];
                let single = desc & 0x20 != 0;
                let dict_len = [0, 1, 2, 4][(desc & 3) as usize];
                let content_len = match desc >> 6 { 0 => usize::from(single), 1 => 2, 2 => 4, _ => 8 };
                let header_len = 5 + usize::from(!single) + dict_len + content_len;
                if pending.len() < header_len { break; }
                let dict_offset = 5 + usize::from(!single);
                if desc & 8 != 0 || pending[dict_offset..dict_offset + dict_len].iter().any(|&b| b != 0) {
                    status = 1;
                    break;
                }
                let mut header = &pending[..header_len];
                match decoder.init(&mut header) {
                    Ok(()) => {}
                    Err(FrameDecoderError::WindowSizeTooBig { .. } | FrameDecoderError::FrameHeaderError(FrameHeaderError::WindowTooBig { .. })) => { status = 2; break; }
                    Err(_) => { status = 1; break; }
                }
                if !dictionary.is_empty() { decoder.force_raw_dictionary(dictionary).ok()?; }
                expected_size = if content_len == 0 { None } else { Some(decoder.content_size()) };
                validate_frame_size = fresh && expected_size.is_some_and(|size| size <= output_size as u64)
                    && complete_frame(&pending, header_len, desc & 4 != 0);
                frame_size = 0;
                let window = if single {
                    decoder.content_size()
                } else {
                    let descriptor = pending[5];
                    let base = 1u64 << (10 + (descriptor >> 3));
                    base + base / 8 * u64::from(descriptor & 7)
                };
                block_limit = window.min(131072) as usize;
                checksum = desc & 4 != 0;
                pending.drain(..header_len);
                active = true;
            }
            if pending.len() < 3 { break; }
            let block = u32::from_le_bytes([pending[0], pending[1], pending[2], 0]);
            let kind = (block >> 1) & 3;
            if kind == 3 { status = 1; break; }
            if kind == 1 && block_limit == 0 { status = 1; break; }
            if kind == 1 && pending.len() < 4 { break; }
            if (block >> 3) as usize > block_limit { status = 1; break; }
            let block_size = if kind == 1 { 1 } else { (block >> 3) as usize };
            let last = block & 1 != 0;
            if last { last_nonempty = block_size != 0; }
            if kind == 0 {
                pending.drain(..3);
                raw_block = Some((block_size, last));
                continue;
            }
            let total = 3 + block_size;
            if pending.len() < total { break; }
            let mut block_bytes = pending[..total].to_vec();
            if last && checksum { block_bytes.extend_from_slice(&[0; 4]); }
            let mut source = block_bytes.as_slice();
            if decoder.decode_blocks(&mut source, BlockDecodingStrategy::UptoBlocks(1)).is_err() {
                status = 1;
                break;
            }
            let mut buffer = vec![0; output_size];
            while decoder.can_collect() != 0 {
                let count = decoder.read(&mut buffer).ok()?;
                if count == 0 { return None; }
                produced.extend_from_slice(&buffer[..count]);
                frame_size += count as u64;
            }
            pending.drain(..total);
            if last {
                if (validate_frame_size || last_nonempty) && expected_size.is_some_and(|size| size != frame_size) {
                    status = 1;
                    break;
                }
                if checksum { awaiting_checksum = true; } else { active = false; delivered_frames = produced.len(); fresh = true; }
            }
        }
        if status != 0 {
            let delivered = delivered_frames + (produced.len() - delivered_frames).saturating_sub(1) / output_size * output_size;
            produced.truncate(delivered);
        }
        response.extend_from_slice(&(produced.len() as u32).to_be_bytes());
        response.extend_from_slice(&produced);
    }
    if pos != request.len() { return None; }
    response[..4].copy_from_slice(&status.to_be_bytes());
    Some(response)
}

fn main() -> io::Result<()> {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let mut magic = [0; 4];
    input.read_exact(&mut magic)?;
    if &magic != b"MGB1" {
        return Err(io::ErrorKind::InvalidData.into());
    }
    output.write_all(b"MGB1")?;
    output.flush()?;
    loop {
        let mut size = [0; 4];
        match input.read_exact(&mut size) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(e),
        }
        let mut request = vec![0; u32::from_be_bytes(size) as usize];
        input.read_exact(&mut request)?;
        let result = decode(&request).ok_or(io::ErrorKind::InvalidData)?;
        output.write_all(&(result.len() as u32).to_be_bytes())?;
        output.write_all(&result)?;
        output.flush()?;
    }
}

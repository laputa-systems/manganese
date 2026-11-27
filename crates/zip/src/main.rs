use std::io::{self, Read, Write};

use rawzip::extra_fields::ExtraFieldId;
use rawzip::{CompressionMethod, ZipArchive, ZipFileHeaderRecord};
use sha2::{Digest, Sha256};
use zlib_rs::{Inflate, InflateFlush, Status};

const MAX_PAYLOAD: usize = 1 << 20;
const UNIX_EPOCH_INTERNAL: i64 = 11_644_473_600_000_000;

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        result.push(DIGITS[(byte >> 4) as usize] as char);
        result.push(DIGITS[(byte & 15) as usize] as char);
    }
    result
}

fn field(out: &mut String, name: &str, value: u64) {
    out.push_str(name);
    out.push(' ');
    out.push_str(&value.to_string());
    out.push('\n');
}

fn utf8_to_utf16(bytes: &[u8]) -> Vec<u16> {
    let mut result = Vec::with_capacity(bytes.len());
    let mut rest = bytes;
    while !rest.is_empty() {
        match std::str::from_utf8(rest) {
            Ok(valid) => {
                result.extend(valid.encode_utf16());
                break;
            }
            Err(error) => {
                let valid = error.valid_up_to();
                if valid > 0 {
                    result.extend(std::str::from_utf8(&rest[..valid]).unwrap_or("").encode_utf16());
                }
                result.push(0xfffd);
                match error.error_len() {
                    Some(length) => rest = &rest[valid + length..],
                    None => break,
                }
            }
        }
    }
    result
}

fn valid_windows_name(c: u16) -> bool {
    if c < 32 {
        return false;
    }
    !matches!(c, 60 | 62 | 58 | 34 | 124 | 63 | 42 | 47 | 92)
}

fn normalize(path: &[u16]) -> (String, bool, bool) {
    let is_dir = path.last() == Some(&(b'/' as u16));
    let mut result: Vec<u16> = Vec::new();
    let mut unsafe_path = true;
    if path.first() == Some(&(b'/' as u16)) {
        result.extend("ROOT".encode_utf16());
        unsafe_path = false;
    }
    let mut rest = path;
    while !rest.is_empty() {
        let start = match rest.iter().position(|c| *c != b'/' as u16) {
            Some(index) => index,
            None => break,
        };
        rest = &rest[start..];
        let end = rest.iter().position(|c| *c == b'/' as u16).unwrap_or(rest.len());
        let part = &rest[..end];
        rest = &rest[end..];
        if !result.is_empty() {
            result.push(b'/' as u16);
        }
        if part == [b'.' as u16] {
            result.extend("DOT".encode_utf16());
            unsafe_path = true;
            continue;
        }
        if part == [b'.' as u16, b'.' as u16] {
            result.extend("UP".encode_utf16());
            unsafe_path = true;
            continue;
        }
        for c in part {
            result.push(if valid_windows_name(*c) { *c } else { 0xfffd });
        }
        unsafe_path = false;
    }
    if is_dir && !result.is_empty() {
        result.push(b'/' as u16);
        unsafe_path = false;
    }
    let text = String::from_utf16_lossy(&result);
    (text, is_dir, unsafe_path)
}

fn days_from_civil(year: i32, month: i32, day: i32) -> i64 {
    let year = year - if month <= 2 { 1 } else { 0 };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let mp = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    i64::from(era * 146097 + doe - 719468)
}

fn chromium_time(year: u16, month: u8, day: u8, hour: u8, minute: u8, second: u8) -> i64 {
    let valid_month = (1..=12).contains(&month);
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let valid_day = valid_month && day >= 1 && day <= days[(month - 1) as usize];
    if !valid_month || !valid_day || hour > 23 || minute > 59 || second > 59 || year < 1601 || year > 30827 {
        return UNIX_EPOCH_INTERNAL;
    }
    let days = days_from_civil(year as i32, month as i32, day as i32) - days_from_civil(1601, 1, 1);
    days * 86_400_000_000
        + i64::from(hour) * 3_600_000_000
        + i64::from(minute) * 60_000_000
        + i64::from(second) * 1_000_000
}

fn unicode_path_override(entry: &ZipFileHeaderRecord<'_>, physical: &[u8]) -> Option<Vec<u8>> {
    for (kind, body) in entry.extra_fields() {
        if kind != ExtraFieldId::INFO_ZIP_UNICODE_PATH || body.len() < 5 {
            continue;
        }
        let expected = u32::from_le_bytes(body[1..5].try_into().ok()?);
        let name = &body[5..];
        if body[0] == 1 && expected == zlib_rs::crc32::crc32(0, physical) {
            return Some(name.to_vec());
        }
    }
    None
}

fn inflate(compressed: &[u8], declared: u64) -> (Vec<u8>, bool) {
    let mut output = Vec::new();
    let limit = (declared as usize).min(compressed.len());
    let mut inflater = Inflate::new(false, 15);
    let mut buffer = [0u8; 32768];
    let mut position = 0;
    let mut ended = false;
    while position < limit && !ended {
        let before_in = inflater.total_in() as usize;
        let before_out = inflater.total_out() as usize;
        let result = inflater.decompress(&compressed[position..limit], &mut buffer, InflateFlush::NoFlush);
        let consumed = inflater.total_in() as usize - before_in;
        let produced = inflater.total_out() as usize - before_out;
        output.extend_from_slice(&buffer[..produced]);
        position += consumed;
        match result {
            Ok(Status::StreamEnd) => {
                ended = true;
                break;
            }
            Ok(Status::Ok) | Ok(Status::BufError) => {}
            Err(_) => return (output, false),
        }
        if consumed == 0 && produced == 0 {
            break;
        }
    }
    (output, ended || position >= limit)
}

fn payload(entry: &ZipFileHeaderRecord<'_>, archive: &rawzip::ZipSliceArchive<&[u8]>,
           physical: &[u8]) -> (Vec<u8>, bool) {
    let local = match archive.get_entry(entry.wayfinder()) {
        Ok(local) => local,
        Err(_) => return (Vec::new(), false),
    };
    let local_path = local.local_header().file_path();
    let local_name: &[u8] = local_path.as_ref();
    if local_name != physical {
        return (Vec::new(), false);
    }
    let compressed = local.data();
    let method = entry.compression_method();
    let (data, complete) = if method == CompressionMethod::DEFLATE {
        inflate(compressed, entry.compressed_size_hint())
    } else if method == CompressionMethod::STORE || method == CompressionMethod::from(12) {
        (compressed.to_vec(), true)
    } else {
        (Vec::new(), false)
    };
    let crc = zlib_rs::crc32::crc32(0, &data);
    let length = data.len();
    (data, complete && crc == entry.crc32() && length <= MAX_PAYLOAD)
}

fn read_archive(input: &[u8]) -> String {
    let data = &input[1..];
    let archive = match ZipArchive::from_slice(data) {
        Ok(archive) => archive,
        Err(_) => return "open 0\n".to_string(),
    };
    let mut out = "open 1\n".to_string();
    let mut entries = archive.entries();
    let mut index = 0;
    let ok = loop {
        let entry = match entries.next_entry() {
            Ok(Some(entry)) => entry,
            Ok(None) => break true,
            Err(_) => break false,
        };
        field(&mut out, "entry", index);
        index += 1;
        let raw_path = entry.file_path();
        let physical_raw: &[u8] = raw_path.as_ref();
        let effective_raw = unicode_path_override(&entry, physical_raw).unwrap_or_else(|| physical_raw.to_vec());
        let (physical, physical_dir, physical_unsafe) = normalize(&utf8_to_utf16(physical_raw));
        let (path, path_dir, path_unsafe) = normalize(&utf8_to_utf16(&effective_raw));
        out.push_str("path ");
        out.push_str(&hex(path.as_bytes()));
        out.push('\n');
        out.push_str("raw ");
        out.push_str(&hex(&effective_raw));
        out.push('\n');
        out.push_str("physical ");
        out.push_str(&hex(physical.as_bytes()));
        out.push('\n');
        field(&mut out, "size", entry.uncompressed_size_hint());
        let dos = entry.last_modified_dos();
        let packed_time = dos.packed_time();
        let packed_date = dos.packed_date();
        field(&mut out, "time", chromium_time(
            ((packed_date >> 9) & 0x7f) as u16 + 1980,
            ((packed_date >> 5) & 0x0f) as u8,
            (packed_date & 0x1f) as u8,
            ((packed_time >> 11) & 0x1f) as u8,
            ((packed_time >> 5) & 0x3f) as u8,
            ((packed_time & 0x1f) * 2) as u8,
        ) as u64);
        field(&mut out, "dir", u64::from(physical_dir && path_dir));
        field(&mut out, "unsafe", u64::from(physical_unsafe || path_unsafe));
        let flags = entry.flags().bits();
        let encrypted = flags & 1 != 0;
        field(&mut out, "encrypted", u64::from(encrypted));
        field(&mut out, "aes", u64::from(encrypted && entry.compression_method() == CompressionMethod::from(99)));
        field(&mut out, "mode", u64::from((entry.external_attributes() >> 16) & 0o777));
        field(&mut out, "symlink", u64::from((entry.external_attributes() >> 16) & 0o170000 == 0o120000));
        if physical_dir && path_dir || encrypted {
            out.push_str("payload none 0 ");
            out.push_str(&hex(&[0u8; 32]));
            out.push('\n');
        } else {
            let (data, complete) = payload(&entry, &archive, physical_raw);
            let digest = Sha256::digest(&data);
            out.push_str(if complete { "payload ok " } else { "payload fail " });
            out.push_str(&data.len().to_string());
            out.push(' ');
            out.push_str(&hex(&digest));
            out.push('\n');
        }
    };
    field(&mut out, "ok", u64::from(ok && index == archive.entries_hint()));
    out
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
            Some(1) => read_archive(&bytes),
            _ => return Err(io::Error::other("invalid operation")),
        };
        output.write_all(&(result.len() as u32).to_be_bytes())?;
        output.write_all(result.as_bytes())?;
        output.flush()?;
    }
    Ok(())
}

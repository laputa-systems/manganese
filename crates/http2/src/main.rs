mod huffman;

use std::io::{self, Read, Write};

const PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
const INITIAL_SETTINGS: [(u16, u32); 4] = [
    (1, 65536),
    (2, 0),
    (4, 6291456),
    (6, 262144),
];
const WEIGHTS: [u8; 6] = [73, 109, 146, 182, 219, 255];

struct Request {
    method: Vec<u8>,
    scheme: Vec<u8>,
    host: Vec<u8>,
    path: Vec<u8>,
    headers: Vec<(Vec<u8>, Vec<u8>)>,
    priority: usize,
    status: u16,
    trailers: bool,
}

struct Record {
    requests: Vec<Request>,
}

struct Cursor<'a>(&'a [u8]);

impl<'a> Cursor<'a> {
    fn take(&mut self, size: usize) -> Option<&'a [u8]> {
        if self.0.len() < size {
            return None;
        }
        let (head, rest) = self.0.split_at(size);
        self.0 = rest;
        Some(head)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|bytes| bytes[0])
    }
    fn u16(&mut self) -> Option<usize> {
        self.take(2).map(|bytes| usize::from(u16::from_be_bytes([bytes[0], bytes[1]])))
    }
    fn u32(&mut self) -> Option<u32> {
        self.take(4).map(|bytes| u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }
    fn short(&mut self) -> Option<Vec<u8>> {
        let size = self.u16()?;
        self.take(size).map(<[u8]>::to_vec)
    }
    fn long(&mut self) -> Option<Vec<u8>> {
        let size = self.u32()? as usize;
        self.take(size).map(<[u8]>::to_vec)
    }
}

fn decode(input: &[u8]) -> Option<Record> {
    let mut cursor = Cursor(input);
    if cursor.u8()? != 1 {
        return None;
    }
    let count = cursor.u8()? as usize;
    if count == 0 || count > 8 {
        return None;
    }
    let mut requests = Vec::with_capacity(count);
    for _ in 0..count {
        let method = cursor.short()?;
        let scheme = cursor.short()?;
        let host = cursor.short()?;
        let path = cursor.short()?;
        let header_count = cursor.u16()?;
        let mut headers = Vec::with_capacity(header_count);
        for _ in 0..header_count {
            headers.push((cursor.short()?, cursor.short()?));
        }
        let priority = cursor.u8()? as usize;
        if priority > 5 {
            return None;
        }
        let status = cursor.u16()? as u16;
        let reset = cursor.u8()?;
        if reset != 0 {
            cursor.u32()?;
        }
        let trailers = cursor.u8()? != 0;
        requests.push(Request {
            method,
            scheme,
            host,
            path,
            headers,
            priority,
            status,
            trailers,
        });
    }
    let settings = cursor.u16()?;
    for _ in 0..settings {
        cursor.take(2)?;
        cursor.u32()?;
    }
    cursor.u32()?;
    cursor.u8()?;
    cursor.long()?;
    if !cursor.0.is_empty() {
        return None;
    }
    Some(Record { requests })
}

fn static_table() -> &'static [(&'static [u8], &'static [u8])] {
    &[
        (b":authority", b""),
        (b":method", b"GET"),
        (b":method", b"POST"),
        (b":path", b"/"),
        (b":path", b"/index.html"),
        (b":scheme", b"http"),
        (b":scheme", b"https"),
        (b":status", b"200"),
        (b":status", b"204"),
        (b":status", b"206"),
        (b":status", b"304"),
        (b":status", b"400"),
        (b":status", b"404"),
        (b":status", b"500"),
        (b"accept-charset", b""),
        (b"accept-encoding", b"gzip, deflate"),
        (b"accept-language", b""),
        (b"accept-ranges", b""),
        (b"accept", b""),
        (b"access-control-allow-origin", b""),
        (b"age", b""),
        (b"allow", b""),
        (b"authorization", b""),
        (b"cache-control", b""),
        (b"content-disposition", b""),
        (b"content-encoding", b""),
        (b"content-language", b""),
        (b"content-length", b""),
        (b"content-location", b""),
        (b"content-range", b""),
        (b"content-type", b""),
        (b"cookie", b""),
        (b"date", b""),
        (b"etag", b""),
        (b"expect", b""),
        (b"expires", b""),
        (b"from", b""),
        (b"host", b""),
        (b"if-match", b""),
        (b"if-modified-since", b""),
        (b"if-none-match", b""),
        (b"if-range", b""),
        (b"if-unmodified-since", b""),
        (b"last-modified", b""),
        (b"link", b""),
        (b"location", b""),
        (b"max-forwards", b""),
        (b"proxy-authenticate", b""),
        (b"proxy-authorization", b""),
        (b"range", b""),
        (b"referer", b""),
        (b"refresh", b""),
        (b"retry-after", b""),
        (b"server", b""),
        (b"set-cookie", b""),
        (b"strict-transport-security", b""),
        (b"transfer-encoding", b""),
        (b"user-agent", b""),
        (b"vary", b""),
        (b"via", b""),
        (b"www-authenticate", b""),
    ]
}

fn encode_integer(out: &mut Vec<u8>, value: usize, prefix_bits: u8, prefix: u8) {
    let limit = (1usize << prefix_bits) - 1;
    if value < limit {
        out.push(prefix | value as u8);
        return;
    }
    out.push(prefix | limit as u8);
    let mut remaining = value - limit;
    while remaining >= 128 {
        out.push((remaining as u8 & 0x7f) | 0x80);
        remaining >>= 7;
    }
    out.push(remaining as u8);
}

fn huffman_size(value: &[u8]) -> usize {
    value
        .iter()
        .map(|byte| huffman::ENCODE_CODE_LENGTHS[*byte as usize] as usize)
        .sum::<usize>()
        .div_ceil(8)
}

fn encode_string(out: &mut Vec<u8>, value: &[u8]) {
    let size = huffman_size(value);
    if size < value.len() {
        encode_integer(out, size, 7, 0x80);
        let mut bits = 0u64;
        let mut count = 0u32;
        for byte in value {
            let length = huffman::ENCODE_CODE_LENGTHS[*byte as usize] as u32;
            bits = (bits << length) | huffman::ENCODE_CODES[*byte as usize] as u64;
            count += length;
            while count >= 8 {
                count -= 8;
                out.push((bits >> count) as u8);
                if count == 0 {
                    bits = 0;
                } else {
                    bits &= (1u64 << count) - 1;
                }
            }
        }
        if count > 0 {
            let padding = 8 - count;
            bits = (bits << padding) | ((1u64 << padding) - 1);
            out.push(bits as u8);
        }
    } else {
        encode_integer(out, value.len(), 7, 0);
        out.extend_from_slice(value);
    }
}

struct Hpack {
    dynamic: Vec<(Vec<u8>, Vec<u8>)>,
    size: usize,
}

impl Hpack {
    fn new() -> Self {
        Self {
            dynamic: Vec::new(),
            size: 0,
        }
    }

    fn lookup(&self, name: &[u8], value: &[u8]) -> Option<usize> {
        for (index, (entry_name, entry_value)) in static_table().iter().enumerate() {
            if *entry_name == name && *entry_value == value {
                return Some(index + 1);
            }
        }
        self.dynamic
            .iter()
            .position(|(entry_name, entry_value)| entry_name == name && entry_value == value)
            .map(|index| index + static_table().len() + 1)
    }

    fn name_index(&self, name: &[u8]) -> Option<usize> {
        for (index, (entry_name, _)) in static_table().iter().enumerate() {
            if *entry_name == name {
                return Some(index + 1);
            }
        }
        self.dynamic
            .iter()
            .rposition(|(entry_name, _)| entry_name == name)
            .map(|index| index + static_table().len() + 1)
    }

    fn insert(&mut self, name: Vec<u8>, value: Vec<u8>) {
        let size = name.len() + value.len() + 32;
        self.dynamic.insert(0, (name, value));
        self.size += size;
        while self.size > 4096 {
            if let Some((name, value)) = self.dynamic.pop() {
                self.size -= name.len() + value.len() + 32;
            } else {
                break;
            }
        }
    }

    fn encode_header(&mut self, out: &mut Vec<u8>, name: &[u8], value: &[u8], index: bool) {
        if let Some(index) = self.lookup(name, value) {
            encode_integer(out, index, 7, 0x80);
            return;
        }
        let name_index = self.name_index(name);
        if index {
            encode_integer(out, name_index.unwrap_or(0), 6, 0x40);
            if name_index.is_none() {
                encode_string(out, name);
            }
            encode_string(out, value);
            self.insert(name.to_vec(), value.to_vec());
        } else {
            encode_integer(out, name_index.unwrap_or(0), 4, 0);
            if name_index.is_none() {
                encode_string(out, name);
            }
            encode_string(out, value);
        }
    }
}

fn crumble(value: &[u8]) -> Vec<Vec<u8>> {
    let start = value
        .iter()
        .position(|byte| *byte != b' ' && *byte != b'\t')
        .unwrap_or(value.len());
    let end = value
        .iter()
        .rposition(|byte| *byte != b' ' && *byte != b'\t')
        .map_or(start, |index| index + 1);
    let trimmed = &value[start..end];
    let mut pieces = Vec::new();
    let mut offset = 0;
    while let Some(relative) = trimmed[offset..].iter().position(|byte| *byte == b';') {
        let position = offset + relative;
        pieces.push(trimmed[offset..position].to_vec());
        offset = position + 1;
        if offset < trimmed.len() && trimmed[offset] == b' ' {
            offset += 1;
        }
    }
    pieces.push(trimmed[offset..].to_vec());
    pieces
}

fn request_headers(request: &Request) -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut headers = vec![
        (b":method".to_vec(), request.method.clone()),
        (b":authority".to_vec(), request.host.clone()),
        (b":scheme".to_vec(), request.scheme.clone()),
        (b":path".to_vec(), request.path.clone()),
    ];
    let mut unique: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    for (name, value) in &request.headers {
        if let Some(entry) = unique.iter_mut().find(|(entry_name, _)| entry_name == name) {
            entry.1 = value.clone();
        } else {
            unique.push((name.clone(), value.clone()));
        }
    }
    for (name, value) in unique {
        if name == b"cookie" {
            for crumb in crumble(&value) {
                headers.push((name.clone(), crumb));
            }
        } else {
            headers.push((name, value));
        }
    }
    headers
}

fn encode_header_block(request: &Request) -> Vec<u8> {
    let mut encoder = Hpack::new();
    let mut out = Vec::new();
    for (name, value) in request_headers(request) {
        let index = name == b":authority" || !name.starts_with(b":");
        encoder.encode_header(&mut out, &name, &value, index);
    }
    out
}

fn frame(kind: u8, flags: u8, stream: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(9 + payload.len());
    out.extend_from_slice(&((payload.len() as u32).to_be_bytes()[1..]));
    out.push(kind);
    out.push(flags);
    out.extend_from_slice(&stream.to_be_bytes());
    out.extend_from_slice(payload);
    out
}

fn execute(input: &[u8]) -> Vec<u8> {
    let Some(record) = decode(input) else {
        return b"invalid record\n".to_vec();
    };
    let mut wire = PREFACE.to_vec();
    let mut settings = Vec::new();
    for (identifier, value) in INITIAL_SETTINGS {
        settings.extend_from_slice(&identifier.to_be_bytes());
        settings.extend_from_slice(&value.to_be_bytes());
    }
    wire.extend_from_slice(&frame(4, 0, 0, &settings));
    wire.extend_from_slice(&frame(8, 0, 0, &15663105u32.to_be_bytes()));
    for (index, request) in record.requests.iter().enumerate() {
        let mut priority = vec![0x80, 0, 0, 0];
        priority.push(WEIGHTS[request.priority]);
        priority.extend_from_slice(&encode_header_block(request));
        wire.extend_from_slice(&frame(1, 0x25, (2 * index + 1) as u32, &priority));
    }
    wire.extend_from_slice(&frame(4, 1, 0, &[]));
    let mut output = b"wire ".to_vec();
    output.extend_from_slice(wire.iter().map(|byte| format!("{byte:02x}")).collect::<String>().as_bytes());
    output.push(b'\n');
    for (index, request) in record.requests.iter().enumerate() {
        output.extend_from_slice(format!("headers {index} {}\n", request.status).as_bytes());
        if request.trailers {
            output.extend_from_slice(format!("trailers {index}\n").as_bytes());
        }
        output.extend_from_slice(format!("close {index} OK\n").as_bytes());
    }
    output
}

fn main() -> io::Result<()> {
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let stdout = io::stdout();
    let mut output = stdout.lock();
    let mut magic = [0u8; 4];
    input.read_exact(&mut magic)?;
    if &magic != b"MGB1" {
        return Ok(());
    }
    output.write_all(b"MGB1")?;
    output.flush()?;
    loop {
        let mut length = [0u8; 4];
        match input.read_exact(&mut length) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(error) => return Err(error),
        }
        let size = u32::from_be_bytes(length) as usize;
        let mut request = vec![0u8; size];
        input.read_exact(&mut request)?;
        let response = execute(&request);
        output.write_all(&(response.len() as u32).to_be_bytes())?;
        output.write_all(&response)?;
        output.flush()?;
    }
}

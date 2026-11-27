use std::io::Read;
use std::io::Write;

const GREASE: u16 = 0xfaca;

struct Request {
    host: Vec<u8>,
    alpn: Vec<Vec<u8>>,
}

struct Reader<'a> {
    data: &'a [u8],
}

impl<'a> Reader<'a> {
    fn bytes(&mut self, size: usize) -> Option<&'a [u8]> {
        if self.data.len() < size {
            return None;
        }
        let (head, tail) = self.data.split_at(size);
        self.data = tail;
        Some(head)
    }

    fn uint(&mut self, width: usize) -> Option<u64> {
        Some(self.bytes(width)?.iter().fold(0u64, |value, byte| (value << 8) | u64::from(*byte)))
    }

    fn prefixed(&mut self, width: usize) -> Option<&'a [u8]> {
        let size = usize::try_from(self.uint(width)?).ok()?;
        self.bytes(size)
    }
}

fn parse_request(input: &[u8]) -> Option<Request> {
    let mut reader = Reader { data: input };
    if reader.uint(1) != Some(1) {
        return None;
    }
    let host = reader.prefixed(2)?.to_vec();
    if !(1..=253).contains(&host.len()) {
        return None;
    }
    let count = reader.uint(1)?;
    if count > 8 {
        return None;
    }
    let mut alpn = Vec::new();
    for _ in 0..count {
        let proto = reader.prefixed(1)?.to_vec();
        if !(1..=32).contains(&proto.len()) {
            return None;
        }
        alpn.push(proto);
    }
    if !reader.data.is_empty() {
        return None;
    }
    Some(Request { host, alpn })
}

fn u16_bytes(value: usize) -> [u8; 2] {
    [((value >> 8) & 0xff) as u8, (value & 0xff) as u8]
}

fn code(value: u16) -> [u8; 2] {
    value.to_be_bytes()
}

struct Extension {
    kind: u16,
    body: Vec<u8>,
}

fn extension(kind: u16, body: Vec<u8>) -> Extension {
    Extension { kind, body }
}

fn client_hello_profile(request: &Request) -> Option<Vec<u8>> {
    let host = match request.host.iter().position(|byte| *byte == 0) {
        Some(end) => &request.host[..end],
        None => &request.host,
    };
    if host.is_empty() {
        return None;
    }

    let ciphers: [u16; 16] = [
        GREASE, 0x1301, 0x1302, 0x1303, 0xc02b, 0xc02f, 0xc02c, 0xc030, 0xcca9, 0xcca8, 0xc013, 0xc014,
        0x009c, 0x009d, 0x002f, 0x0035,
    ];
    let mut out = vec![1];
    out.extend_from_slice(&u16_bytes(ciphers.len()));
    for cipher in ciphers {
        out.extend_from_slice(&code(cipher));
    }
    out.push(1);
    out.push(0);

    let mut server_name = Vec::with_capacity(5 + host.len());
    server_name.extend_from_slice(&u16_bytes(host.len() + 3));
    server_name.push(0);
    server_name.extend_from_slice(&u16_bytes(host.len()));
    server_name.extend_from_slice(host);

    let groups = vec![0, 10, 0xfa, 0xca, 0x11, 0xec, 0x00, 0x1d, 0x00, 0x17, 0x00, 0x18];

    let mut signature_algorithms = vec![0, 24, 0xfa, 0xca];
    signature_algorithms.extend_from_slice(&[
        0x09, 0x04, 0x09, 0x05, 0x09, 0x06, 0x04, 0x03, 0x08, 0x04, 0x04, 0x01, 0x05, 0x03, 0x08, 0x05,
        0x05, 0x01, 0x08, 0x06, 0x06, 0x01,
    ]);

    let mut alpn_body = vec![0, 0];
    for proto in &request.alpn {
        alpn_body.push(proto.len() as u8);
        alpn_body.extend_from_slice(proto);
    }
    let alpn_size = alpn_body.len() - 2;
    alpn_body[0..2].copy_from_slice(&u16_bytes(alpn_size));

    let mut key_share = vec![0, 0, 0xfa, 0xca, 0, 1, 0];
    key_share.extend_from_slice(&[0x11, 0xec, 0x04, 0xc0]);
    key_share.extend(std::iter::repeat(0).take(1216));
    key_share.extend_from_slice(&[0x00, 0x1d, 0, 32]);
    key_share.extend(std::iter::repeat(0).take(32));
    let key_share_size = key_share.len() - 2;
    key_share[0..2].copy_from_slice(&u16_bytes(key_share_size));

    let mut extensions = vec![
        extension(0x0000, server_name),
        extension(0x0005, vec![1, 0, 0, 0, 0]),
        extension(0x000a, groups),
        extension(0x000b, vec![1, 0]),
        extension(0x000d, signature_algorithms),
        extension(0x0010, alpn_body),
        extension(0x0012, Vec::new()),
        extension(0x0017, Vec::new()),
        extension(0x001b, vec![2, 0, 2]),
        extension(0x0023, Vec::new()),
        extension(0x002b, vec![6, 0xfa, 0xca, 3, 4, 3, 3]),
        extension(0x002d, vec![1, 1]),
        extension(0x0033, key_share),
        extension(GREASE, Vec::new()),
        extension(GREASE, vec![0]),
        extension(0xfe0d, vec![0]),
        extension(0xff01, vec![0]),
    ];
    extensions.sort_by(|left, right| {
        left.kind.cmp(&right.kind).then_with(|| left.body.cmp(&right.body))
    });
    out.extend_from_slice(&u16_bytes(extensions.len()));
    for ext in extensions {
        out.extend_from_slice(&code(ext.kind));
        out.extend_from_slice(&u16_bytes(ext.body.len()));
        out.extend_from_slice(&ext.body);
    }
    Some(out)
}

fn main() {
    let mut magic = [0u8; 4];
    if std::io::stdin().read_exact(&mut magic).is_err() || &magic != b"MGB1" {
        return;
    }
    let mut output = std::io::stdout().lock();
    if output.write_all(b"MGB1").is_err() || output.flush().is_err() {
        return;
    }
    let mut input = std::io::stdin().lock();
    loop {
        let mut header = [0u8; 4];
        match input.read_exact(&mut header) {
            Ok(()) => {}
            Err(_) => return,
        }
        let size = u32::from_be_bytes(header) as usize;
        if size > (1 << 20) {
            return;
        }
        let mut request = vec![0u8; size];
        if input.read_exact(&mut request).is_err() {
            return;
        }
        let response = match parse_request(&request).and_then(|record| client_hello_profile(&record)) {
            Some(value) => value,
            None => b"invalid record".to_vec(),
        };
        if output.write_all(&(response.len() as u32).to_be_bytes()).is_err()
            || output.write_all(&response).is_err()
            || output.flush().is_err()
        {
            return;
        }
    }
}

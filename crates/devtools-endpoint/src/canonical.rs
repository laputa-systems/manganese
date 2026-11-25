//! The comparable form of what a DevTools endpoint sent on one connection.
//!
//! HTTP framing whitespace and header-name case are not observable to CDP clients, so a response
//! becomes its status code, its headers in order with lowercase names and trimmed values, and its
//! body. After a 101 response the rest is RFC 6455 server frames. Bytes that do not parse are kept.

pub const INVALID_SCRIPT: &[u8] = b"invalid script\n";

fn header_value<'a>(headers: &[(String, &'a [u8])], name: &str) -> Option<&'a [u8]> {
    headers.iter().rev().find(|(key, _)| key == name).map(|(_, value)| *value)
}

fn trim(bytes: &[u8]) -> &[u8] {
    let start = bytes.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(bytes.len());
    let end = bytes.iter().rposition(|b| !b.is_ascii_whitespace()).map_or(start, |end| end + 1);
    &bytes[start..end]
}

fn bytes_block(out: &mut Vec<u8>, label: &str, data: &[u8]) {
    out.extend_from_slice(format!("{label} {}\n", data.len()).as_bytes());
    out.extend_from_slice(data);
    out.push(b'\n');
}

fn websocket(out: &mut Vec<u8>, mut rest: &[u8]) {
    out.extend_from_slice(b"websocket\n");
    while !rest.is_empty() {
        if rest.len() < 2 {
            return bytes_block(out, "partial", rest);
        }
        let (first, second) = (rest[0], rest[1]);
        let (length, header) = match second & 0x7f {
            126 if rest.len() >= 4 => (u64::from(u16::from_be_bytes([rest[2], rest[3]])), 4),
            127 if rest.len() >= 10 => (u64::from_be_bytes(rest[2..10].try_into().unwrap_or_default()), 10),
            126 | 127 => return bytes_block(out, "partial", rest),
            short => (u64::from(short), 2),
        };
        let mask = if second & 0x80 != 0 { 4 } else { 0 };
        let total = header + mask + length as usize;
        if second & 0x80 != 0 || rest.len() < total {
            return bytes_block(out, "partial", rest);
        }
        let label = format!("frame fin={} rsv={} opcode={}", first >> 7, (first >> 4) & 7, first & 15);
        bytes_block(out, &label, &rest[header..total]);
        rest = &rest[total..];
    }
}

pub fn canonical(closed: bool, raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut rest = raw;
    while !rest.is_empty() {
        let Some(end) = rest.windows(4).position(|window| window == b"\r\n\r\n") else {
            bytes_block(&mut out, "partial", rest);
            break;
        };
        let head = &rest[..end];
        let mut lines = head.split(|b| *b == b'\n').map(|line| line.strip_suffix(b"\r").unwrap_or(line));
        let status_line = lines.next().unwrap_or_default();
        let mut words = status_line.splitn(3, |b| *b == b' ');
        let (version, code) = (words.next().unwrap_or_default(), words.next().unwrap_or_default());
        if !version.starts_with(b"HTTP/") || code.len() != 3 || !code.iter().all(u8::is_ascii_digit) {
            bytes_block(&mut out, "partial", rest);
            break;
        }
        out.extend_from_slice(b"response\nversion ");
        out.extend_from_slice(version);
        out.extend_from_slice(b"\nstatus ");
        out.extend_from_slice(code);
        out.push(b'\n');
        let mut headers = Vec::new();
        for line in lines {
            let split = line.iter().position(|b| *b == b':').unwrap_or(line.len());
            let name = String::from_utf8_lossy(trim(&line[..split])).to_ascii_lowercase();
            let value = trim(line.get(split + 1..).unwrap_or_default());
            out.extend_from_slice(format!("header {name}: ").as_bytes());
            out.extend_from_slice(value);
            out.push(b'\n');
            headers.push((name, value));
        }
        rest = &rest[end + 4..];
        if code == b"101" {
            websocket(&mut out, rest);
            break;
        }
        let length = header_value(&headers, "content-length")
            .and_then(|value| std::str::from_utf8(value).ok()?.parse::<usize>().ok());
        let take = match length {
            Some(length) => length.min(rest.len()),
            None if code == b"204" || code == b"304" => 0,
            None => rest.len(),
        };
        bytes_block(&mut out, "body", &rest[..take]);
        rest = &rest[take..];
    }
    out.extend_from_slice(if closed { b"closed yes\n" } else { b"closed no\n" });
    out
}

#[cfg(test)]
mod tests {
    use super::canonical;

    #[test]
    fn framing_whitespace_and_name_case_do_not_matter() {
        let chromium = b"HTTP/1.1 200 OK\r\nContent-Length:2\r\nContent-Type:text/html\r\n\r\nhi";
        let hyper = b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\ncontent-type: text/html\r\n\r\nhi";
        assert_eq!(canonical(false, chromium), canonical(false, hyper));
    }

    #[test]
    fn status_headers_order_body_and_close_do_matter() {
        let base = canonical(false, b"HTTP/1.1 200 OK\r\nA:1\r\nB:2\r\nContent-Length:2\r\n\r\nhi");
        for other in [
            canonical(false, b"HTTP/1.1 404 OK\r\nA:1\r\nB:2\r\nContent-Length:2\r\n\r\nhi"),
            canonical(false, b"HTTP/1.1 200 OK\r\nB:2\r\nA:1\r\nContent-Length:2\r\n\r\nhi"),
            canonical(false, b"HTTP/1.1 200 OK\r\nA:1\r\nB:2\r\nContent-Length:2\r\n\r\nho"),
            canonical(true, b"HTTP/1.1 200 OK\r\nA:1\r\nB:2\r\nContent-Length:2\r\n\r\nhi"),
            canonical(false, b"HTTP/1.0 200 OK\r\nA:1\r\nB:2\r\nContent-Length:2\r\n\r\nhi"),
        ] {
            assert_ne!(base, other);
        }
    }

    #[test]
    fn pipelined_responses_and_websocket_frames_are_separated() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length:1\r\n\r\naHTTP/1.1 101 Switching\r\nUpgrade:websocket\r\n\r\n\x81\x02hi\x88\x00";
        let text = String::from_utf8(canonical(true, raw)).unwrap();
        assert_eq!(text.matches("response\n").count(), 2);
        assert!(text.contains("frame fin=1 rsv=0 opcode=1 2\nhi\n"));
        assert!(text.contains("frame fin=1 rsv=0 opcode=8 0\n\n"));
    }

    #[test]
    fn unparsed_bytes_are_kept() {
        assert!(canonical(true, b"garbage").windows(7).any(|w| w == b"garbage"));
        assert!(canonical(false, b"HTTP/1.1 200 OK\r\nContent-Le").starts_with(b"partial "));
    }
}

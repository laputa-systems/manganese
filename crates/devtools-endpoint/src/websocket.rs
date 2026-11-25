use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use fastwebsockets::Frame;
use fastwebsockets::Payload;
use sha1::Digest;
use sha1::Sha1;

const HANDSHAKE_GUID: &[u8] = b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
const MAX_PAYLOAD: u64 = 0x7FFF_FFFF_FFFF_FFFF;

pub fn accept_key(key: &[u8]) -> String {
    let mut digest = Sha1::new();
    digest.update(key);
    digest.update(HANDSHAKE_GUID);
    STANDARD.encode(digest.finalize())
}

pub fn handshake_response(accept: &str, extension: Option<&str>) -> Vec<u8> {
    let extension = extension.map_or_else(String::new, |value| format!("Sec-WebSocket-Extensions: {value}\r\n"));
    format!("HTTP/1.1 101 WebSocket Protocol Handshake\r\nUpgrade: WebSocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n{extension}\r\n").into_bytes()
}

#[derive(Debug, PartialEq, Eq)]
pub enum Negotiation {
    Rejected,
    Declined,
    Deflate(String),
}

struct Parameter {
    name: String,
    value: Option<String>,
}

struct Extension {
    name: String,
    parameters: Vec<Parameter>,
}

fn is_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
}

struct Cursor<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl Cursor<'_> {
    fn skip_blanks(&mut self) {
        while self.bytes.get(self.position).is_some_and(|byte| matches!(byte, b' ' | b'\t')) {
            self.position += 1;
        }
    }

    fn token(&mut self) -> Option<String> {
        let start = self.position;
        while self.bytes.get(self.position).is_some_and(|byte| is_token_byte(*byte)) {
            self.position += 1;
        }
        (self.position > start).then(|| String::from_utf8_lossy(&self.bytes[start..self.position]).into_owned())
    }

    fn quoted(&mut self) -> Option<String> {
        self.position += 1;
        let mut out = Vec::new();
        loop {
            match *self.bytes.get(self.position)? {
                b'"' => {
                    self.position += 1;
                    return Some(String::from_utf8_lossy(&out).into_owned());
                }
                b'\\' => {
                    out.push(*self.bytes.get(self.position + 1)?);
                    self.position += 2;
                }
                byte => {
                    out.push(byte);
                    self.position += 1;
                }
            }
        }
    }
}

fn parse_extensions(header: &[u8]) -> Option<Vec<Extension>> {
    let mut cursor = Cursor { bytes: header, position: 0 };
    let mut extensions = Vec::new();
    loop {
        cursor.skip_blanks();
        let name = cursor.token()?;
        let mut parameters = Vec::new();
        loop {
            cursor.skip_blanks();
            if cursor.bytes.get(cursor.position) != Some(&b';') {
                break;
            }
            cursor.position += 1;
            cursor.skip_blanks();
            let parameter = cursor.token()?;
            cursor.skip_blanks();
            let mut value = None;
            if cursor.bytes.get(cursor.position) == Some(&b'=') {
                cursor.position += 1;
                cursor.skip_blanks();
                value = Some(if cursor.bytes.get(cursor.position) == Some(&b'"') { cursor.quoted()? } else { cursor.token()? });
            }
            parameters.push(Parameter { name: parameter, value });
        }
        extensions.push(Extension { name, parameters });
        cursor.skip_blanks();
        match cursor.bytes.get(cursor.position) {
            None => return Some(extensions),
            Some(b',') => cursor.position += 1,
            Some(_) => return None,
        }
    }
}

fn window_bits(value: &str) -> Option<u8> {
    if value.is_empty() || value.starts_with('0') || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse::<u8>().ok().filter(|bits| (8..=15).contains(bits))
}

#[derive(Default)]
struct DeflateOffer {
    server_no_context_takeover: bool,
    client_no_context_takeover: bool,
    server_max_window_bits: Option<u8>,
    client_max_window_bits: Option<Option<u8>>,
}

fn deflate_offer(extension: &Extension) -> Option<DeflateOffer> {
    if extension.name != "permessage-deflate" {
        return None;
    }
    let mut offer = DeflateOffer::default();
    for parameter in &extension.parameters {
        match (parameter.name.as_str(), &parameter.value) {
            ("server_no_context_takeover", None) if !offer.server_no_context_takeover => offer.server_no_context_takeover = true,
            ("client_no_context_takeover", None) if !offer.client_no_context_takeover => offer.client_no_context_takeover = true,
            ("server_max_window_bits", Some(value)) if offer.server_max_window_bits.is_none() => offer.server_max_window_bits = Some(window_bits(value)?),
            ("client_max_window_bits", None) if offer.client_max_window_bits.is_none() => offer.client_max_window_bits = Some(None),
            ("client_max_window_bits", Some(value)) if offer.client_max_window_bits.is_none() => {
                offer.client_max_window_bits = Some(if value.is_empty() { None } else { Some(window_bits(value)?) });
            }
            _ => return None,
        }
    }
    Some(offer)
}

/// net::WebSocketEncoder::CreateServer: an unparsable header fails the connection, an unusable offer is
/// declined, and the first acceptable permessage-deflate offer is echoed back.
pub fn negotiate_extensions(header: &[u8]) -> Negotiation {
    let Some(extensions) = parse_extensions(header) else {
        return Negotiation::Rejected;
    };
    for extension in &extensions {
        let Some(offer) = deflate_offer(extension) else {
            continue;
        };
        let mut response = String::from("permessage-deflate");
        if offer.server_no_context_takeover {
            response.push_str("; server_no_context_takeover");
        }
        if offer.client_no_context_takeover {
            response.push_str("; client_no_context_takeover");
        }
        if let Some(bits) = offer.server_max_window_bits {
            response.push_str(&format!("; server_max_window_bits={bits}"));
        }
        if let Some(bits) = offer.client_max_window_bits {
            response.push_str(&format!("; client_max_window_bits={}", bits.unwrap_or(15)));
        }
        return Negotiation::Deflate(response);
    }
    Negotiation::Declined
}

#[derive(Debug, PartialEq, Eq)]
pub enum Decoded {
    Incomplete,
    Error,
    Ping(Vec<u8>),
    Pong,
    Close,
    Message { payload: Vec<u8>, fin: bool, compressed: bool },
}

/// Port of net::DecodeFrameHybi17 for server-side frames; returns the result and the bytes consumed.
pub fn decode_frame(frame: &[u8]) -> (Decoded, usize) {
    if frame.len() < 2 {
        return (Decoded::Incomplete, 0);
    }
    let (first, second) = (frame[0], frame[1]);
    let fin = first & 0x80 != 0;
    let compressed = first & 0x40 != 0;
    let opcode = first & 0x0f;
    let masked = second & 0x80 != 0;
    if first & 0x30 != 0 {
        return (Decoded::Error, 0);
    }
    if !matches!(opcode, 0 | 1 | 8 | 9 | 10) || !masked {
        return (Decoded::Error, 0);
    }
    let mut remaining = &frame[2..];
    let mut length = u64::from(second & 0x7f);
    if length > 125 {
        let width = if length == 126 { 2 } else { 8 };
        if remaining.len() < width {
            return (Decoded::Incomplete, 0);
        }
        length = remaining[..width].iter().fold(0, |value, byte| value << 8 | u64::from(*byte));
        remaining = &remaining[width..];
    }
    if length > MAX_PAYLOAD || length.checked_add(4).is_none() {
        return (Decoded::Error, 0);
    }
    let Some(total) = usize::try_from(length).ok().and_then(|length| length.checked_add(4)) else {
        return (Decoded::Error, 0);
    };
    if remaining.len() < total {
        return (Decoded::Incomplete, 0);
    }
    let (key, payload) = remaining[..total].split_at(4);
    let payload: Vec<u8> = payload.iter().enumerate().map(|(index, byte)| byte ^ key[index % 4]).collect();
    let consumed = frame.len() - remaining.len() + total;
    let decoded = match opcode {
        9 => Decoded::Ping(payload),
        10 => Decoded::Pong,
        8 => Decoded::Close,
        _ => Decoded::Message { payload, fin, compressed },
    };
    (decoded, consumed)
}

fn encode(frame: Frame<'_>) -> Vec<u8> {
    let mut frame = frame;
    let mut buffer = Vec::new();
    frame.write(&mut buffer).to_vec()
}

pub fn text_frame(payload: &[u8]) -> Vec<u8> {
    encode(Frame::text(Payload::Borrowed(payload)))
}

pub fn pong_frame(payload: &[u8]) -> Vec<u8> {
    encode(Frame::pong(Payload::Borrowed(payload)))
}

pub fn close_frame(payload: &[u8]) -> Vec<u8> {
    encode(Frame::close_raw(Payload::Borrowed(payload)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn masked(opcode: u8, fin: bool, payload: &[u8]) -> Vec<u8> {
        let key = [0x11, 0x22, 0x33, 0x44];
        let mut out = vec![if fin { 0x80 } else { 0 } | opcode];
        match payload.len() {
            0..=125 => out.push(0x80 | payload.len() as u8),
            126..=65535 => {
                out.push(0x80 | 126);
                out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
            }
            _ => {
                out.push(0x80 | 127);
                out.extend_from_slice(&(payload.len() as u64).to_be_bytes());
            }
        }
        out.extend_from_slice(&key);
        out.extend(payload.iter().enumerate().map(|(index, byte)| byte ^ key[index % 4]));
        out
    }

    #[test]
    fn the_handshake_accept_matches_rfc_6455() {
        assert_eq!(accept_key(b"dGhlIHNhbXBsZSBub25jZQ=="), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }

    #[test]
    fn frames_decode_with_chromiums_leniency_and_strictness() {
        for length in [0, 5, 125, 126, 70000] {
            let payload = vec![b'a'; length];
            let frame = masked(1, true, &payload);
            assert_eq!(decode_frame(&frame), (Decoded::Message { payload, fin: true, compressed: false }, frame.len()));
        }
        assert_eq!(decode_frame(&masked(2, true, b"x")).0, Decoded::Error);
        assert_eq!(decode_frame(&[0x81, 0x01, b'x']).0, Decoded::Error);
        assert_eq!(decode_frame(&masked(1, true, b"abc")[..5]).0, Decoded::Incomplete);
        assert_eq!(decode_frame(&masked(9, true, &[b'p'; 200])).0, Decoded::Ping(vec![b'p'; 200]));
        assert_eq!(decode_frame(&masked(8, true, b"\x03\xe8")).0, Decoded::Close);
    }

    #[test]
    fn server_frames_are_unmasked_and_sized_like_chromiums() {
        assert_eq!(text_frame(b"hi"), b"\x81\x02hi");
        assert_eq!(pong_frame(b"p"), b"\x8a\x01p");
        assert_eq!(close_frame(b"\x03\xe8"), b"\x88\x02\x03\xe8");
        assert_eq!(&text_frame(&[b'a'; 300])[..4], &[0x81, 126, 1, 44]);
    }

    #[test]
    fn extension_headers_are_negotiated_like_net_websocket_encoder() {
        assert_eq!(negotiate_extensions(b"permessage-deflate"), Negotiation::Deflate("permessage-deflate".into()));
        assert_eq!(negotiate_extensions(b"permessage-deflate; client_max_window_bits"), Negotiation::Deflate("permessage-deflate; client_max_window_bits=15".into()));
        assert_eq!(negotiate_extensions(b"x-other"), Negotiation::Declined);
        assert_eq!(negotiate_extensions(b""), Negotiation::Rejected);
        assert_eq!(negotiate_extensions(b"a;"), Negotiation::Rejected);
    }
}

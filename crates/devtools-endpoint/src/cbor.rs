use crate::status::Error;
use crate::status::Status;

const ENVELOPE_HEADER: usize = 7;
const MAP_START: u8 = 0xbf;
const ARRAY_START: u8 = 0x9f;
const STOP: u8 = 0xff;

fn write_token_start(major: u8, value: u64, out: &mut Vec<u8>) {
    let major = major << 5;
    if value < 24 {
        out.push(major | value as u8);
    } else if value <= 0xff {
        out.push(major | 24);
        out.push(value as u8);
    } else if value <= 0xffff {
        out.push(major | 25);
        out.extend_from_slice(&(value as u16).to_be_bytes());
    } else if value <= 0xffff_ffff {
        out.push(major | 26);
        out.extend_from_slice(&(value as u32).to_be_bytes());
    } else {
        out.push(major | 27);
        out.extend_from_slice(&value.to_be_bytes());
    }
}

/// Mirror of crdtp's CBOR encoder, which wraps every map and array in a length-prefixed envelope.
#[derive(Default)]
pub struct Encoder {
    out: Vec<u8>,
    envelopes: Vec<usize>,
}

impl Encoder {
    fn open(&mut self, tag: u8) {
        self.out.extend_from_slice(&[0xd8, 0x18, 0x5a]);
        self.envelopes.push(self.out.len());
        self.out.extend_from_slice(&[0; 4]);
        self.out.push(tag);
    }

    fn close(&mut self) {
        self.out.push(STOP);
        let start = self.envelopes.pop().expect("an open envelope");
        let size = (self.out.len() - start - 4) as u32;
        self.out[start..start + 4].copy_from_slice(&size.to_be_bytes());
    }

    pub fn map_begin(&mut self) {
        self.open(MAP_START);
    }

    pub fn map_end(&mut self) {
        self.close();
    }

    pub fn array_begin(&mut self) {
        self.open(ARRAY_START);
    }

    pub fn array_end(&mut self) {
        self.close();
    }

    pub fn string16(&mut self, units: &[u16]) {
        if units.iter().all(|unit| *unit <= 127) {
            write_token_start(3, units.len() as u64, &mut self.out);
            self.out.extend(units.iter().map(|unit| *unit as u8));
        } else {
            write_token_start(2, units.len() as u64 * 2, &mut self.out);
            for unit in units {
                self.out.extend_from_slice(&unit.to_le_bytes());
            }
        }
    }

    pub fn int32(&mut self, value: i32) {
        if value >= 0 {
            write_token_start(0, value as u64, &mut self.out);
        } else {
            write_token_start(1, (-(i64::from(value) + 1)) as u64, &mut self.out);
        }
    }

    pub fn double(&mut self, value: f64) {
        self.out.push(0xfb);
        self.out.extend_from_slice(&value.to_bits().to_be_bytes());
    }

    pub fn boolean(&mut self, value: bool) {
        self.out.push(if value { 0xf5 } else { 0xf4 });
    }

    pub fn null(&mut self) {
        self.out.push(0xf6);
    }

    pub fn finish(self) -> Vec<u8> {
        self.out
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Token {
    Done,
    True,
    False,
    Null,
    Int32(i32),
    Double(f64),
    String8,
    String16,
    MapStart,
    ArrayStart,
    Stop,
    Envelope,
}

/// Reads the CBOR this module's [`Encoder`] writes, reporting positions as crdtp's tokenizer does.
pub struct Tokenizer<'a> {
    bytes: &'a [u8],
    position: usize,
    token: Token,
    size: usize,
}

fn read_token_start(bytes: &[u8]) -> Option<(u8, u64, usize)> {
    let first = *bytes.first()?;
    let major = first >> 5;
    match first & 0x1f {
        value @ 0..=23 => Some((major, u64::from(value), 1)),
        24 => Some((major, u64::from(*bytes.get(1)?), 2)),
        25 => Some((major, u64::from(u16::from_be_bytes(bytes.get(1..3)?.try_into().ok()?)), 3)),
        26 => Some((major, u64::from(u32::from_be_bytes(bytes.get(1..5)?.try_into().ok()?)), 5)),
        27 => Some((major, u64::from_be_bytes(bytes.get(1..9)?.try_into().ok()?), 9)),
        _ => None,
    }
}

impl<'a> Tokenizer<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        let mut tokenizer = Self { bytes, position: 0, token: Token::Done, size: 0 };
        tokenizer.read();
        tokenizer
    }

    pub fn token(&self) -> Token {
        self.token
    }

    pub fn position(&self) -> usize {
        self.position
    }

    pub fn status(&self, error: Error) -> Status {
        Status { error, position: self.position }
    }

    fn read(&mut self) {
        let rest = &self.bytes[self.position.min(self.bytes.len())..];
        let Some(first) = rest.first().copied() else {
            self.token = Token::Done;
            self.size = 0;
            return;
        };
        (self.token, self.size) = match first {
            0xf5 => (Token::True, 1),
            0xf4 => (Token::False, 1),
            0xf6 => (Token::Null, 1),
            MAP_START => (Token::MapStart, 1),
            ARRAY_START => (Token::ArrayStart, 1),
            STOP => (Token::Stop, 1),
            0xfb => (Token::Double(f64::from_bits(u64::from_be_bytes(rest[1..9].try_into().expect("an encoded double")))), 9),
            0xd8 => {
                let length = u32::from_be_bytes(rest[3..7].try_into().expect("an envelope header")) as usize;
                (Token::Envelope, ENVELOPE_HEADER + length)
            }
            _ => {
                let (major, value, header) = read_token_start(rest).expect("a well-formed token");
                match major {
                    0 => (Token::Int32(value as i32), header),
                    1 => (Token::Int32((-(value as i64) - 1) as i32), header),
                    2 => (Token::String16, header + value as usize),
                    _ => (Token::String8, header + value as usize),
                }
            }
        };
    }

    pub fn next(&mut self) {
        self.position += self.size;
        self.read();
    }

    pub fn enter_envelope(&mut self) {
        self.position += ENVELOPE_HEADER;
        self.read();
    }

    pub fn envelope(&self) -> &'a [u8] {
        &self.bytes[self.position..self.position + self.size]
    }

    pub fn string8(&self) -> &'a [u8] {
        let (_, _, header) = read_token_start(&self.bytes[self.position..]).expect("a string token");
        &self.bytes[self.position + header..self.position + self.size]
    }

    pub fn string16(&self) -> String {
        let (_, _, header) = read_token_start(&self.bytes[self.position..]).expect("a string token");
        let units: Vec<u16> = self.bytes[self.position + header..self.position + self.size].chunks_exact(2).map(|pair| u16::from_le_bytes([pair[0], pair[1]])).collect();
        String::from_utf16_lossy(&units)
    }
}

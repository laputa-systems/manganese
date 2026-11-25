use prost::Message;
use prost::encoding::{DecodeContext, WireType, decode_key, decode_varint, encode_key, encode_varint};
use std::io::{self, Read, Write};

#[derive(Clone, PartialEq, Message)]
struct Metadata {
    #[prost(uint64, optional, tag = "1")]
    corruptions: Option<u64>,
    #[prost(int32, optional, tag = "3")]
    migration_status: Option<i32>,
    #[prost(uint32, optional, tag = "4")]
    failure_count: Option<u32>,
}

#[derive(Clone, PartialEq, Message)]
struct Origin {
    #[prost(bytes = "vec", optional, tag = "1")]
    scheme: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "2")]
    host: Option<Vec<u8>>,
    #[prost(uint32, optional, tag = "3")]
    port: Option<u32>,
}

#[derive(Clone, PartialEq, Message)]
struct Token {
    #[prost(bytes = "vec", optional, tag = "1")]
    trial_name: Option<Vec<u8>>,
    #[prost(uint64, optional, tag = "2")]
    token_expiry: Option<u64>,
    #[prost(uint32, optional, tag = "3")]
    usage_restriction: Option<u32>,
    #[prost(bytes = "vec", optional, tag = "4")]
    token_signature: Option<Vec<u8>>,
    #[prost(bytes = "vec", repeated, tag = "5")]
    partition_sites: Vec<Vec<u8>>,
    #[prost(bool, optional, tag = "6")]
    match_subdomains: Option<bool>,
}

#[derive(Default)]
struct Retained<T> {
    known: T,
    unknown: Vec<u8>,
}

struct Field {
    tag: u32,
    wire: WireType,
    value: Vec<u8>,
    raw: Vec<u8>,
}

fn fields(mut input: &[u8], depth: usize, end: Option<u32>) -> Result<(Vec<Field>, usize), ()> {
    if depth > 100 { return Err(()); }
    let start = input.len();
    let mut result = Vec::new();
    while !input.is_empty() {
        let (tag, wire) = decode_key(&mut input).map_err(|_| ())?;
        if wire == WireType::EndGroup {
            return if end == Some(tag) { Ok((result, start - input.len())) } else { Err(()) };
        }
        let mut raw = Vec::new();
        encode_key(tag, wire, &mut raw);
        let mut value = Vec::new();
        match wire {
            WireType::Varint => {
                let n = decode_varint(&mut input).map_err(|_| ())?;
                encode_varint(n, &mut value);
                raw.extend_from_slice(&value);
            }
            WireType::SixtyFourBit | WireType::ThirtyTwoBit => {
                let n = if wire == WireType::SixtyFourBit { 8 } else { 4 };
                if input.len() < n { return Err(()); }
                value.extend_from_slice(&input[..n]);
                raw.extend_from_slice(&value);
                input = &input[n..];
            }
            WireType::LengthDelimited => {
                let n = decode_varint(&mut input).map_err(|_| ())?;
                if n > i32::MAX as u64 || n > input.len() as u64 { return Err(()); }
                value.extend_from_slice(&input[..n as usize]);
                encode_varint(n, &mut raw);
                raw.extend_from_slice(&value);
                input = &input[n as usize..];
            }
            WireType::StartGroup => {
                let (nested, consumed) = fields(input, depth + 1, Some(tag))?;
                for field in nested { raw.extend_from_slice(&field.raw); }
                encode_key(tag, WireType::EndGroup, &mut raw);
                input = &input[consumed..];
            }
            WireType::EndGroup => unreachable!(),
        }
        result.push(Field { tag, wire, value, raw });
    }
    if end.is_some() { Err(()) } else { Ok((result, start)) }
}

fn merge_known<T: Message>(message: &mut T, field: &Field) -> Result<(), ()> {
    let mut encoded = Vec::new();
    if field.wire == WireType::LengthDelimited { encode_varint(field.value.len() as u64, &mut encoded); }
    encoded.extend_from_slice(&field.value);
    message.merge_field(field.tag, field.wire, &mut encoded.as_slice(), DecodeContext::default()).map_err(|_| ())
}

impl<T: Message + Default> Retained<T> {
    fn encode(&self) -> Vec<u8> {
        let mut bytes = self.known.encode_to_vec();
        bytes.extend_from_slice(&self.unknown);
        bytes
    }
}

fn metadata(input: &[u8]) -> Result<Vec<u8>, ()> {
    let mut result = Retained::<Metadata>::default();
    for field in fields(input, 0, None)?.0 {
        let valid = field.wire == WireType::Varint && match field.tag {
            1 | 4 => true,
            3 => (0..=4).contains(&(decode_varint(&mut field.value.as_slice()).map_err(|_| ())? as i32)),
            _ => false,
        };
        if valid {
            merge_known(&mut result.known, &field)?;
        } else if field.tag == 3 && field.wire == WireType::Varint {
            let value = decode_varint(&mut field.value.as_slice()).map_err(|_| ())? as i32;
            encode_key(3, WireType::Varint, &mut result.unknown);
            encode_varint(value as i64 as u64, &mut result.unknown);
        } else {
            result.unknown.extend_from_slice(&field.raw);
        }
    }
    Ok(result.encode())
}

fn origin(input: &[u8], message: &mut Retained<Origin>) -> Result<(), ()> {
    for field in fields(input, 1, None)?.0 {
        let valid = matches!((field.tag, field.wire), (1 | 2, WireType::LengthDelimited) | (3, WireType::Varint));
        if valid { merge_known(&mut message.known, &field)?; } else { message.unknown.extend_from_slice(&field.raw); }
    }
    Ok(())
}

fn token(input: &[u8]) -> Result<Vec<u8>, ()> {
    let mut result = Retained::<Token>::default();
    for field in fields(input, 1, None)?.0 {
        let valid = matches!((field.tag, field.wire), (1 | 4 | 5, WireType::LengthDelimited) | (2 | 3 | 6, WireType::Varint));
        if valid { merge_known(&mut result.known, &field)?; } else { result.unknown.extend_from_slice(&field.raw); }
    }
    Ok(result.encode())
}

fn entries(input: &[u8]) -> Result<Vec<u8>, ()> {
    let mut origin_value = None;
    let mut tokens = Vec::new();
    let mut unknown = Vec::new();
    for field in fields(input, 0, None)?.0 {
        match (field.tag, field.wire) {
            (1, WireType::LengthDelimited) => origin(&field.value, origin_value.get_or_insert_with(Retained::<Origin>::default))?,
            (2, WireType::LengthDelimited) => tokens.push(token(&field.value)?),
            _ => unknown.extend_from_slice(&field.raw),
        }
    }
    let mut output = Vec::new();
    if let Some(origin) = origin_value {
        let bytes = origin.encode();
        encode_key(1, WireType::LengthDelimited, &mut output);
        encode_varint(bytes.len() as u64, &mut output);
        output.extend_from_slice(&bytes);
    }
    for token in tokens {
        encode_key(2, WireType::LengthDelimited, &mut output);
        encode_varint(token.len() as u64, &mut output);
        output.extend_from_slice(&token);
    }
    output.extend_from_slice(&unknown);
    Ok(output)
}

fn process(input: &[u8]) -> Vec<u8> {
    let response = match input.split_first() {
        Some((0, data)) => metadata(data),
        Some((1, data)) => entries(data),
        _ => Err(()),
    };
    match response { Ok(bytes) => [vec![1], bytes].concat(), Err(()) => vec![0] }
}

fn main() -> io::Result<()> {
    let mut stdin = io::stdin().lock();
    let mut stdout = io::stdout().lock();
    let mut header = [0; 4];
    stdin.read_exact(&mut header)?;
    if &header != b"MGB1" { return Err(io::ErrorKind::InvalidData.into()); }
    stdout.write_all(b"MGB1")?;
    stdout.flush()?;
    loop {
        match stdin.read(&mut header[..1])? { 0 => break, 1 => (), _ => unreachable!() }
        stdin.read_exact(&mut header[1..])?;
        let len = u32::from_be_bytes(header) as usize;
        let mut input = vec![0; len];
        stdin.read_exact(&mut input)?;
        let output = process(&input);
        stdout.write_all(&(output.len() as u32).to_be_bytes())?;
        stdout.write_all(&output)?;
        stdout.flush()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_keeps_unknowns_after_known_fields() {
        assert_eq!(metadata(&[0x50, 0x81, 0, 0x08, 0, 0x18, 9, 0x18, 1]).unwrap(), [0x08, 0, 0x18, 1, 0x50, 1, 0x18, 9]);
    }

    #[test]
    fn origin_fields_merge_without_losing_nested_unknowns() {
        let input = [0x0a, 4, 0x0a, 2, b'h', b't', 0x0a, 5, 0x12, 1, b'x', 0x20, 1];
        assert_eq!(entries(&input).unwrap(), [0x0a, 9, 0x0a, 2, b'h', b't', 0x12, 1, b'x', 0x20, 1]);
    }

    #[test]
    fn malformed_groups_and_lengths_fail() {
        assert!(metadata(&[0x53, 0x5c]).is_err());
        assert!(entries(&[0x0a, 2, 0x08]).is_err());
        assert!(metadata(&[0x80]).is_err());
    }
}

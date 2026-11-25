use prost::Message;
use prost::encoding::{DecodeContext, WireType, decode_varint, encode_key, encode_varint};
use std::io::{self, Read, Write};

#[path = "generated/leveldb_proto.rs"]
mod leveldb_proto;
#[path = "generated/origin_trials_pb.rs"]
mod origin_trials_pb;
#[path = "generated/content.rs"]
mod content;

use leveldb_proto::SharedDbMetadataProto as Metadata;
use origin_trials_pb::{OriginMessage as Origin, TrialTokenDbEntry as Token};

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

fn read_varint(input: &mut &[u8], limit: usize) -> Result<u64, ()> {
    let mut value = 0u64;
    for index in 0..limit {
        let (&byte, rest) = input.split_first().ok_or(())?;
        *input = rest;
        value |= u64::from(byte & 0x7f) << (index * 7);
        if byte < 0x80 { return Ok(value); }
    }
    Err(())
}

fn fields(mut input: &[u8], depth: usize, end: Option<u32>) -> Result<(Vec<Field>, usize), ()> {
    if depth > 100 { return Err(()); }
    let start = input.len();
    let mut result = Vec::new();
    while !input.is_empty() {
        let key = read_varint(&mut input, 5)? as u32;
        let tag = key >> 3;
        if tag == 0 { return Err(()); }
        let wire = WireType::try_from(u64::from(key & 7)).map_err(|_| ())?;
        if wire == WireType::EndGroup {
            return if end == Some(tag) { Ok((result, start - input.len())) } else { Err(()) };
        }
        let mut raw = Vec::new();
        encode_key(tag, wire, &mut raw);
        let mut value = Vec::new();
        match wire {
            WireType::Varint => {
                let n = read_varint(&mut input, 10)?;
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
                let n = read_varint(&mut input, 5)?;
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

fn encode_nested<T: Message>(message: &Retained<T>, tag: u32, nested: &[Vec<u8>]) -> Result<Vec<u8>, ()> {
    fn append(output: &mut Vec<u8>, tag: u32, nested: &[Vec<u8>]) {
        for value in nested {
            encode_key(tag, WireType::LengthDelimited, output);
            encode_varint(value.len() as u64, output);
            output.extend_from_slice(value);
        }
    }
    let encoded = message.known.encode_to_vec();
    let mut output = Vec::new();
    let mut inserted = false;
    for field in fields(&encoded, 0, None)?.0 {
        if field.tag == tag { return Err(()); }
        if !inserted && field.tag > tag {
            append(&mut output, tag, nested);
            inserted = true;
        }
        output.extend_from_slice(&field.raw);
    }
    if !inserted { append(&mut output, tag, nested); }
    output.extend_from_slice(&message.unknown);
    Ok(output)
}

fn periodic(input: &[u8], result: &mut Retained<content::PeriodicSyncOptions>) -> Result<(), ()> {
    for field in fields(input, 2, None)?.0 {
        if matches!((field.tag, field.wire), (1, WireType::Varint)) {
            merge_known(&mut result.known, &field)?;
        } else {
            result.unknown.extend_from_slice(&field.raw);
        }
    }
    Ok(())
}

fn registration(input: &[u8]) -> Result<Vec<u8>, ()> {
    let mut result = Retained::<content::BackgroundSyncRegistrationProto>::default();
    let mut options = None;
    let mut required = 0u8;
    for field in fields(input, 1, None)?.0 {
        match (field.tag, field.wire) {
            (2, WireType::LengthDelimited) => { required |= 1; merge_known(&mut result.known, &field)?; }
            (7, WireType::Varint) => { required |= 2; merge_known(&mut result.known, &field)?; }
            (8, WireType::Varint) => { required |= 4; merge_known(&mut result.known, &field)?; }
            (9, WireType::LengthDelimited) => periodic(&field.value, options.get_or_insert_with(Retained::<content::PeriodicSyncOptions>::default))?,
            (10, WireType::Varint) => merge_known(&mut result.known, &field)?,
            _ => result.unknown.extend_from_slice(&field.raw),
        }
    }
    if required != 7 { return Err(()); }
    let nested = options.map(|value| vec![value.encode()]).unwrap_or_default();
    encode_nested(&result, 9, &nested)
}

fn registrations(input: &[u8]) -> Result<Vec<u8>, ()> {
    let mut result = Retained::<content::BackgroundSyncRegistrationsProto>::default();
    let mut nested = Vec::new();
    let mut origin_set = false;
    for field in fields(input, 0, None)?.0 {
        match (field.tag, field.wire) {
            (1, WireType::LengthDelimited) => nested.push(registration(&field.value)?),
            (3, WireType::LengthDelimited) => { origin_set = true; merge_known(&mut result.known, &field)?; }
            _ => result.unknown.extend_from_slice(&field.raw),
        }
    }
    if !origin_set { return Err(()); }
    encode_nested(&result, 1, &nested)
}

fn image_size(input: &[u8]) -> Result<Vec<u8>, ()> {
    let mut result = Retained::<content::ImageSizeProto>::default();
    for field in fields(input, 2, None)?.0 {
        if matches!((field.tag, field.wire), (1 | 2, WireType::Varint)) {
            merge_known(&mut result.known, &field)?;
        } else {
            result.unknown.extend_from_slice(&field.raw);
        }
    }
    Ok(result.encode())
}

fn payment_image(input: &[u8]) -> Result<Vec<u8>, ()> {
    let mut result = Retained::<content::StoredPaymentInstrumentImageObject>::default();
    let mut sizes = Vec::new();
    for field in fields(input, 1, None)?.0 {
        match (field.tag, field.wire) {
            (1 | 2, WireType::LengthDelimited) => merge_known(&mut result.known, &field)?,
            (3, WireType::LengthDelimited) => sizes.push(image_size(&field.value)?),
            _ => result.unknown.extend_from_slice(&field.raw),
        }
    }
    encode_nested(&result, 3, &sizes)
}

fn payment(input: &[u8]) -> Result<Vec<u8>, ()> {
    let mut result = Retained::<content::StoredPaymentInstrumentProto>::default();
    let mut icons = Vec::new();
    for field in fields(input, 0, None)?.0 {
        match (field.tag, field.wire) {
            (1, WireType::Varint) | (2..=5 | 7, WireType::LengthDelimited) | (8, WireType::Varint) => merge_known(&mut result.known, &field)?,
            (8, WireType::LengthDelimited) => {
                let mut input = field.value.as_slice();
                let mut value = Vec::new();
                while !input.is_empty() { encode_varint(read_varint(&mut input, 10)?, &mut value); }
                merge_known(&mut result.known, &Field { value, ..field })?;
            }
            (6, WireType::LengthDelimited) => icons.push(payment_image(&field.value)?),
            _ => result.unknown.extend_from_slice(&field.raw),
        }
    }
    encode_nested(&result, 6, &icons)
}

fn process(input: &[u8]) -> Vec<u8> {
    let response = match input.split_first() {
        Some((0, data)) => metadata(data),
        Some((1, data)) => entries(data),
        Some((2, data)) => registrations(data),
        Some((3, data)) => payment(data),
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

    #[test]
    fn sync_checks_required_fields_in_each_message() {
        assert!(registrations(&[]).is_err());
        assert!(registrations(&[0x0a, 4, 0x12, 0, 0x38, 0, 0x1a, 0]).is_err());
        let valid = [0x0a, 6, 0x12, 0, 0x38, 0, 0x40, 0, 0x1a, 0];
        assert_eq!(registrations(&valid).unwrap(), valid);
    }

    #[test]
    fn payment_reads_packed_and_unpacked_networks() {
        let input = [0x42, 3, 1, 0x96, 1, 0x40, 2];
        assert_eq!(payment(&input).unwrap(), [0x40, 1, 0x40, 0x96, 1, 0x40, 2]);
        assert!(payment(&[0x42, 1, 0x80]).is_err());
    }

    #[test]
    fn chromium_truncates_oversized_varint_values_and_keys() {
        assert_eq!(metadata(&[0x08, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 2]).unwrap(),
            [0x08, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f]);
        assert_eq!(metadata(&[0xa8, 0x80, 0x80, 0x80, 0x10, 1]).unwrap(), [0x28, 1]);
        assert!(metadata(&[0x88, 0x80, 0x80, 0x80, 0x80, 0, 1]).is_err());
        assert!(metadata(&[0x0a, 0x80, 0x80, 0x80, 0x80, 0x80, 0]).is_err());
    }

    #[test]
    fn payment_accepts_oversized_tenth_packed_varint_byte() {
        let input = [0x42, 10, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 2];
        assert_eq!(payment(&input).unwrap(), [0x40, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 1]);
    }
}

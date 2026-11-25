use std::io::{self, Read, Write};
use numerics::{blink_fixed, blink_precision, fixed, parse_double, parse_integer, parse_json_number, precision, shortest, shortest_exponential, utf16_to_utf8, validate_utf8, IntegerKind, IntegerRadix, JsonNumber, PrecisionStyle, ShortestStyle, Utf16Conversion};

fn main() -> io::Result<()> {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let mut magic = [0; 4];
    input.read_exact(&mut magic)?;
    if &magic != b"MGB1" { return Err(io::ErrorKind::InvalidData.into()); }
    output.write_all(b"MGB1")?;
    output.flush()?;
    loop {
        let mut length = [0; 4];
        match input.read(&mut length[..1])? {
            0 => return Ok(()),
            1 => input.read_exact(&mut length[1..])?,
            _ => unreachable!(),
        }
        let length = u32::from_be_bytes(length) as usize;
        if length == 0 || length > 65536 { return Err(io::ErrorKind::InvalidData.into()); }
        let mut request = vec![0; length];
        input.read_exact(&mut request)?;
        if request[0] == 12 {
            let response = match parse_json_number(&request[1..]) {
                JsonNumber::Invalid => vec![0],
                JsonNumber::Integer(value) => { let mut out = vec![1]; out.extend(value.to_be_bytes()); out }
                JsonNumber::Double(value) => { let mut out = vec![2]; out.extend(value.to_bits().to_be_bytes()); out }
            };
            output.write_all(&(response.len() as u32).to_be_bytes())?;
            output.write_all(&response)?;
            output.flush()?;
            continue;
        }
        if matches!(request[0], 10 | 11) && length == 11 {
            let bits = u64::from_be_bytes(request[1..9].try_into().unwrap());
            let number = f64::from_bits(bits);
            let digits = u16::from_be_bytes(request[9..11].try_into().unwrap());
            if digits > 20 || (request[0] == 10 && digits == 0) { return Err(io::ErrorKind::InvalidData.into()); }
            let text = if request[0] == 10 { blink_precision(number, digits.into()) } else { blink_fixed(number, digits.into()) };
            output.write_all(&(text.len() as u32).to_be_bytes())?;
            output.write_all(text.as_bytes())?;
            output.flush()?;
            continue;
        }
        if request[0] == 9 {
            let (all_characters, valid_codepoints) = validate_utf8(&request[1..]);
            output.write_all(&2u32.to_be_bytes())?;
            output.write_all(&[u8::from(all_characters), u8::from(valid_codepoints)])?;
            output.flush()?;
            continue;
        }
        if request[0] == 7 && length >= 2 {
            let kind = match request[1] % 4 {
                0 => IntegerKind::I32, 1 => IntegerKind::U32,
                2 => IntegerKind::I64, _ => IntegerKind::U64,
            };
            if request[1] > 7 { return Err(io::ErrorKind::InvalidData.into()); }
            let radix = if request[1] < 4 { IntegerRadix::Decimal } else { IntegerRadix::Hexadecimal };
            let (value, valid) = parse_integer(&request[2..], kind, radix);
            output.write_all(&9u32.to_be_bytes())?;
            output.write_all(&[u8::from(valid)])?;
            output.write_all(&value.to_be_bytes())?;
            output.flush()?;
            continue;
        }
        if request[0] == 8 {
            let (value, consumed) = parse_double(&request[1..]);
            let first_is_space = request.get(1).is_some_and(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c));
            let valid = length > 1 && value.is_finite() && consumed == length - 1 && !first_is_space;
            output.write_all(&9u32.to_be_bytes())?;
            output.write_all(&[u8::from(valid)])?;
            output.write_all(&value.to_bits().to_be_bytes())?;
            output.flush()?;
            continue;
        }
        if request[0] == 6 && length % 2 == 1 {
            let units: Vec<u16> = request[1..].chunks_exact(2).map(|pair| u16::from_be_bytes([pair[0], pair[1]])).collect();
            let (error, count, text) = match utf16_to_utf8(&units) {
                Utf16Conversion::Complete(text) => (0u32, text.len(), text),
                Utf16Conversion::Invalid { valid_units, prefix } => (6u32, valid_units, prefix),
            };
            output.write_all(&((8 + text.len()) as u32).to_be_bytes())?;
            output.write_all(&error.to_be_bytes())?;
            output.write_all(&(count as u32).to_be_bytes())?;
            output.write_all(text.as_bytes())?;
            output.flush()?;
            continue;
        }
        if request[0] == 5 {
            let (value, consumed) = parse_double(&request[1..]);
            output.write_all(&12_u32.to_be_bytes())?;
            output.write_all(&(consumed as u32).to_be_bytes())?;
            output.write_all(&value.to_bits().to_be_bytes())?;
            output.flush()?;
            continue;
        }
        if length != 11 { return Err(io::ErrorKind::InvalidData.into()); }
        let bits = u64::from_be_bytes(request[1..9].try_into().unwrap());
        let number = f64::from_bits(bits);
        let places = u16::from_be_bytes(request[9..11].try_into().unwrap());
        let value = match request[0] {
            0 => shortest(number, ShortestStyle::Base),
            1 => places.checked_sub(1).and_then(|places| fixed(number, places.into())),
            2 => shortest(number, ShortestStyle::EcmaScript),
            3 => precision(number, places.saturating_sub(1).into(), PrecisionStyle::Precision),
            4 if places == 0 => Some(shortest_exponential(number)),
            4 => precision(number, places.into(), PrecisionStyle::Exponential),
            _ => return Err(io::ErrorKind::InvalidData.into()),
        };
        let mut response = vec![u8::from(value.is_some())];
        if let Some(value) = value { response.extend(value.as_bytes()); }
        output.write_all(&(response.len() as u32).to_be_bytes())?;
        output.write_all(&response)?;
        output.flush()?;
    }
}

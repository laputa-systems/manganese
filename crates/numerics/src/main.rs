use std::io::{self, Read, Write};
use numerics::{base_utf8_to_utf16, base_utf16_to_utf8, blink_fixed, blink_format, blink_precision, css_number, fixed, parse_double, parse_integer, parse_json_number, parse_wide_double, precision, shortest, shortest_exponential, utf16_to_utf8, utf16_to_wide, utf8_to_wide, validate_utf8, wide_to_string, write_json_double, BlinkFormat, IntegerKind, IntegerRadix, JsonDoubleStyle, JsonNumber, PrecisionStyle, ShortestStyle, Utf16Conversion};

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
        if request[0] == 26 && length % 2 == 1 {
            let units: Vec<u16> = request[1..].chunks_exact(2).map(|part| u16::from_be_bytes([part[0], part[1]])).collect();
            let (value, consumed, valid) = parse_wide_double(&units);
            output.write_all(&13u32.to_be_bytes())?;
            output.write_all(&[u8::from(valid)])?;
            output.write_all(&(consumed as u32).to_be_bytes())?;
            output.write_all(&value.to_bits().to_be_bytes())?;
            output.flush()?;
            continue;
        }
        if request[0] == 27 && length >= 2 && length % 2 == 0 {
            let kind = match request[1] % 4 {
                0 => IntegerKind::I32, 1 => IntegerKind::U32,
                2 => IntegerKind::I64, _ => IntegerKind::U64,
            };
            if request[1] > 7 { return Err(io::ErrorKind::InvalidData.into()); }
            let radix = if request[1] < 4 { IntegerRadix::Decimal } else { IntegerRadix::Hexadecimal };
            let chars: Vec<u8> = request[2..].chunks_exact(2).map(|pair| if pair[0] == 0 { pair[1] } else { 0 }).collect();
            let (value, valid) = parse_integer(&chars, kind, radix);
            output.write_all(&9u32.to_be_bytes())?;
            output.write_all(&[u8::from(valid)])?;
            output.write_all(&value.to_be_bytes())?;
            output.flush()?;
            continue;
        }
        if (23..=25).contains(&request[0]) && length == 10 {
            let bits = u64::from_be_bytes(request[2..10].try_into().unwrap());
            let text = match request[1] {
                0 => (bits as i32).to_string(), 1 => (bits as u32).to_string(),
                2 => (bits as i64).to_string(), 3 => bits.to_string(),
                _ => return Err(io::ErrorKind::InvalidData.into()),
            };
            let bytes: Vec<u8> = if request[0] == 24 { text.encode_utf16().flat_map(u16::to_be_bytes).collect() } else { text.into_bytes() };
            output.write_all(&(bytes.len() as u32).to_be_bytes())?;
            output.write_all(&bytes)?;
            output.flush()?;
            continue;
        }
        if request[0] == 19 {
            let (units, valid) = utf8_to_wide(&request[1..]);
            output.write_all(&((1 + units.len() * 4) as u32).to_be_bytes())?;
            output.write_all(&[u8::from(valid)])?;
            for unit in units { output.write_all(&unit.to_be_bytes())?; }
            output.flush()?;
            continue;
        }
        if matches!(request[0], 20 | 21) && length % 4 == 1 {
            let units: Vec<u32> = request[1..].chunks_exact(4).map(|part| u32::from_be_bytes(part.try_into().unwrap())).collect();
            let (text, valid) = wide_to_string(&units);
            let bytes = if request[0] == 20 { text.into_bytes() } else { text.encode_utf16().flat_map(u16::to_be_bytes).collect() };
            output.write_all(&((1 + bytes.len()) as u32).to_be_bytes())?;
            output.write_all(&[u8::from(valid)])?;
            output.write_all(&bytes)?;
            output.flush()?;
            continue;
        }
        if request[0] == 22 && length % 2 == 1 {
            let units: Vec<u16> = request[1..].chunks_exact(2).map(|part| u16::from_be_bytes([part[0], part[1]])).collect();
            let (wide, valid) = utf16_to_wide(&units);
            output.write_all(&((1 + wide.len() * 4) as u32).to_be_bytes())?;
            output.write_all(&[u8::from(valid)])?;
            for unit in wide { output.write_all(&unit.to_be_bytes())?; }
            output.flush()?;
            continue;
        }
        if request[0] == 17 {
            let (text, valid) = base_utf8_to_utf16(&request[1..]);
            output.write_all(&((1 + text.len() * 2) as u32).to_be_bytes())?;
            output.write_all(&[u8::from(valid)])?;
            for unit in text { output.write_all(&unit.to_be_bytes())?; }
            output.flush()?;
            continue;
        }
        if request[0] == 18 && length % 2 == 1 {
            let units: Vec<u16> = request[1..].chunks_exact(2).map(|pair| u16::from_be_bytes([pair[0], pair[1]])).collect();
            let (text, valid) = base_utf16_to_utf8(&units);
            output.write_all(&((1 + text.len()) as u32).to_be_bytes())?;
            output.write_all(&[u8::from(valid)])?;
            output.write_all(text.as_bytes())?;
            output.flush()?;
            continue;
        }
        if matches!(request[0], 15 | 16) && length == 11 {
            let bits = u64::from_be_bytes(request[1..9].try_into().unwrap());
            let value = f64::from_bits(bits);
            let text = if request[0] == 15 {
                if request[9] != 0 || request[10] != 0 { return Err(io::ErrorKind::InvalidData.into()); }
                css_number(value)
            } else {
                let digits = usize::from(request[10]);
                if digits > 20 { return Err(io::ErrorKind::InvalidData.into()); }
                let mode = match request[9] {
                    0 => BlinkFormat::Shortest,
                    1 => BlinkFormat::General(digits),
                    2 => BlinkFormat::Exponential(digits),
                    3 => BlinkFormat::Fixed(digits),
                    _ => return Err(io::ErrorKind::InvalidData.into()),
                };
                blink_format(value, mode)
            };
            output.write_all(&(text.len() as u32).to_be_bytes())?;
            output.write_all(text.as_bytes())?;
            output.flush()?;
            continue;
        }
        if request[0] == 14 && length % 2 == 1 {
            let chars: Vec<u8> = request[1..].chunks_exact(2).map(|pair| {
                let value = u16::from_be_bytes([pair[0], pair[1]]);
                if value < 128 { value as u8 } else { 0 }
            }).collect();
            let (value, consumed) = parse_double(&chars);
            output.write_all(&12u32.to_be_bytes())?;
            output.write_all(&(consumed as u32).to_be_bytes())?;
            output.write_all(&value.to_bits().to_be_bytes())?;
            output.flush()?;
            continue;
        }
        if request[0] == 13 && length == 11 {
            let bits = u64::from_be_bytes(request[1..9].try_into().unwrap());
            let style = match request[9] {
                0 => JsonDoubleStyle::PreserveType,
                1 => JsonDoubleStyle::OmitType,
                _ => return Err(io::ErrorKind::InvalidData.into()),
            };
            if request[10] != 0 { return Err(io::ErrorKind::InvalidData.into()); }
            let text = write_json_double(f64::from_bits(bits), style);
            output.write_all(&((text.len() + 1) as u32).to_be_bytes())?;
            output.write_all(&[1])?;
            output.write_all(text.as_bytes())?;
            output.flush()?;
            continue;
        }
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

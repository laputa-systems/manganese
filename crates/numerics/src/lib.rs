#[derive(Clone, Copy, Debug)]
pub enum ShortestStyle {
    Base,
    EcmaScript,
}

pub fn shortest(value: f64, style: ShortestStyle) -> Option<String> {
    if !value.is_finite() {
        return match style {
            ShortestStyle::Base => None,
            ShortestStyle::EcmaScript if value.is_nan() => Some("NaN".into()),
            ShortestStyle::EcmaScript if value.is_sign_negative() => Some("-Infinity".into()),
            ShortestStyle::EcmaScript => Some("Infinity".into()),
        };
    }
    if value == 0.0 {
        return Some(match style {
            ShortestStyle::Base if value.is_sign_negative() => "-0",
            _ => "0",
        }.into());
    }
    let representation = value.abs().to_string();
    let point = representation.find('.').unwrap_or(representation.len());
    let digits: String = representation.chars().filter(|&c| c != '.').collect();
    let first = digits.bytes().position(|b| b != b'0').unwrap();
    let last = digits.bytes().rposition(|b| b != b'0').unwrap() + 1;
    let mut digits = digits[first..last].to_owned();
    let exponent = point as i32 - first as i32 - 1;
    let decimal_exponent = exponent - digits.len() as i32 + 1;
    if digits.as_bytes().last().unwrap() % 2 == 1 && is_decimal_halfway(value.abs(), decimal_exponent) {
        let last = digits.pop().unwrap();
        digits.push((last as u8 - 1) as char);
    }
    let digits = digits.as_str();
    let high = match style { ShortestStyle::Base => 12, ShortestStyle::EcmaScript => 21 };
    let mut output = String::new();
    if value.is_sign_negative() { output.push('-'); }
    if exponent < -6 || exponent >= high {
        output.push(digits.as_bytes()[0] as char);
        if digits.len() > 1 { output.push('.'); output.push_str(&digits[1..]); }
        output.push('e');
        if exponent >= 0 { output.push('+'); }
        output.push_str(&exponent.to_string());
    } else {
        let decimal = exponent + 1;
        if decimal <= 0 {
            output.push_str("0.");
            output.extend(std::iter::repeat_n('0', (-decimal) as usize));
            output.push_str(digits);
        } else if decimal as usize >= digits.len() {
            output.push_str(digits);
            output.extend(std::iter::repeat_n('0', decimal as usize - digits.len()));
        } else {
            output.push_str(&digits[..decimal as usize]);
            output.push('.');
            output.push_str(&digits[decimal as usize..]);
        }
    }
    Some(output)
}

fn is_decimal_halfway(value: f64, decimal_exponent: i32) -> bool {
    let bits = value.to_bits();
    let encoded = ((bits >> 52) & 0x7ff) as i32;
    let mut significand = bits & ((1 << 52) - 1);
    let mut exponent = if encoded == 0 { -1074 } else {
        significand |= 1 << 52;
        encoded - 1075
    };
    if decimal_exponent >= 0 {
        for _ in 0..decimal_exponent {
            if significand % 5 != 0 { return false; }
            significand /= 5;
        }
        exponent -= decimal_exponent;
    } else {
        exponent -= decimal_exponent;
    }
    exponent + significand.trailing_zeros() as i32 == -1
}

#[cfg(test)]
mod tests {
    use super::{blink_fixed, blink_precision, fixed, parse_double, parse_integer, precision, shortest, shortest_exponential, utf16_to_utf8, validate_utf8, IntegerKind, IntegerRadix, PrecisionStyle, ShortestStyle, Utf16Conversion};

    #[test]
    fn blink_formats_strip_only_nonexponential_trailing_zeroes() {
        assert_eq!(blink_precision(1.5, 4), "1.5");
        assert_eq!(blink_precision(1.5e20, 4), "1.500e+20");
        assert_eq!(blink_fixed(-0.0, 2), "0.00");
        assert_eq!(blink_fixed(f64::INFINITY, 2), "Infinity");
    }

    #[test]
    fn utf8_distinguishes_noncharacters_from_invalid_sequences() {
        assert_eq!(validate_utf8(b"valid"), (true, true));
        assert_eq!(validate_utf8("\u{fdd0}".as_bytes()), (false, true));
        assert_eq!(validate_utf8(&[0xed, 0xa0, 0x80]), (false, false));
        assert_eq!(validate_utf8(&[0xc0, 0x80]), (false, false));
    }

    #[test]
    fn integer_parsing_preserves_partial_value_and_saturation() {
        assert_eq!(parse_integer(b" 123", IntegerKind::I32, IntegerRadix::Decimal), (123, false));
        assert_eq!(parse_integer(b"-12x", IntegerKind::I32, IntegerRadix::Decimal), ((-12i64) as u64, false));
        assert_eq!(parse_integer(b"2147483648", IntegerKind::I32, IntegerRadix::Decimal), (2147483647, false));
        assert_eq!(parse_integer(b"-1", IntegerKind::U32, IntegerRadix::Decimal), (0, false));
        assert_eq!(parse_integer(b"0xff", IntegerKind::U32, IntegerRadix::Hexadecimal), (255, true));
    }

    #[test]
    fn utf16_reports_the_first_unpaired_surrogate() {
        assert_eq!(utf16_to_utf8(&[0x41, 0xd83d, 0xde00]), Utf16Conversion::Complete("A😀".into()));
        assert_eq!(utf16_to_utf8(&[0x41, 0xd800, 0x42]), Utf16Conversion::Invalid { valid_units: 1, prefix: "A".into() });
        assert_eq!(utf16_to_utf8(&[0xdc00]), Utf16Conversion::Invalid { valid_units: 0, prefix: String::new() });
    }

    #[test]
    fn precision_preserves_significant_digits_and_exponent() {
        assert_eq!(precision(1.25, 2, PrecisionStyle::Precision).as_deref(), Some("1.3"));
        assert_eq!(precision(12345.0, 3, PrecisionStyle::Precision).as_deref(), Some("1.23e+4"));
        assert_eq!(precision(0.000001, 3, PrecisionStyle::Precision).as_deref(), Some("0.00000100"));
        assert_eq!(precision(-0.0, 3, PrecisionStyle::Exponential).as_deref(), Some("0.00e+0"));
        assert_eq!(shortest_exponential(100.0), "1e+2");
    }

    #[test]
    fn parse_reports_prefix_and_saturation() {
        assert_eq!(parse_double(b"  -1.25e+2junk"), (-125.0, 10));
        assert_eq!(parse_double(b"1e+"), (1.0, 1));
        assert_eq!(parse_double(b"- 1"), (0.0, 0));
        assert_eq!(parse_double(b"  "), (0.0, 2));
        assert_eq!(parse_double(b"1e999"), (f64::INFINITY, 5));
        assert_eq!(parse_double(b"-0").0.to_bits(), (-0.0f64).to_bits());
    }

    #[test]
    fn fixed_rounds_exact_values_away_from_zero() {
        assert_eq!(fixed(1.25, 1).as_deref(), Some("1.3"));
        assert_eq!(fixed(-1.25, 1).as_deref(), Some("-1.3"));
        assert_eq!(fixed(-0.0, 2).as_deref(), Some("-0.00"));
        assert_eq!(fixed(0.1, 17).as_deref(), Some("0.10000000000000001"));
        assert_eq!(fixed(1e60, 2), None);
    }

    #[test]
    fn shortest_rounds_decimal_halfway_to_even() {
        let value = f64::from_bits(0xc3093b6e37e9d2e2);
        assert_eq!(shortest(value, ShortestStyle::EcmaScript).as_deref(), Some("-887777373534812.2"));
    }

    #[test]
    fn shortest_uses_each_callers_notation_and_zero_rules() {
        assert_eq!(shortest(-0.0, ShortestStyle::Base).as_deref(), Some("-0"));
        assert_eq!(shortest(-0.0, ShortestStyle::EcmaScript).as_deref(), Some("0"));
        assert_eq!(shortest(1e12, ShortestStyle::Base).as_deref(), Some("1e+12"));
        assert_eq!(shortest(1e12, ShortestStyle::EcmaScript).as_deref(), Some("1000000000000"));
        assert_eq!(shortest(1e-7, ShortestStyle::EcmaScript).as_deref(), Some("1e-7"));
        assert_eq!(shortest(f64::INFINITY, ShortestStyle::Base), None);
        assert_eq!(shortest(f64::INFINITY, ShortestStyle::EcmaScript).as_deref(), Some("Infinity"));
    }
}

fn exact_decimal(value: f64) -> (Vec<u8>, usize) {
    let bits = value.to_bits();
    let encoded = ((bits >> 52) & 0x7ff) as i32;
    let mut significand = bits & ((1 << 52) - 1);
    let exponent = if encoded == 0 { -1074 } else {
        significand |= 1 << 52;
        encoded - 1075
    };
    let mut digits: Vec<u8> = significand.to_string().bytes().rev().map(|b| b - b'0').collect();
    let (factor, count, scale) = if exponent < 0 { (5, -exponent, -exponent as usize) } else { (2, exponent, 0) };
    for _ in 0..count {
        let mut carry = 0;
        for digit in &mut digits {
            let next = *digit * factor + carry;
            *digit = next % 10;
            carry = next / 10;
        }
        if carry > 0 { digits.push(carry); }
    }
    (digits, scale)
}

pub fn fixed(value: f64, places: usize) -> Option<String> {
    if !value.is_finite() || value.abs() >= 1e60 || places > 100 { return None; }
    let (mut digits, scale) = exact_decimal(value);
    if scale > places {
        let drop = scale - places;
        let round_up = digits.get(drop - 1).is_some_and(|&digit| digit >= 5);
        if drop >= digits.len() { digits.clear(); } else { digits.drain(..drop); }
        if round_up {
            let mut carry = 1;
            for digit in &mut digits {
                let next = *digit + carry;
                *digit = next % 10;
                carry = next / 10;
                if carry == 0 { break; }
            }
            if carry != 0 { digits.push(carry); }
        }
    } else {
        digits.splice(0..0, std::iter::repeat_n(0, places - scale));
    }
    digits.resize(digits.len().max(places + 1), 0);
    let mut result = String::new();
    if value.is_sign_negative() { result.push('-'); }
    for (index, digit) in digits.iter().enumerate().rev() {
        result.push(char::from(b'0' + *digit));
        if index == places && places > 0 { result.push('.'); }
    }
    Some(result)
}

pub fn parse_double(input: &[u8]) -> (f64, usize) {
    let mut cursor = 0;
    while cursor < input.len() && matches!(input[cursor], b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c) { cursor += 1; }
    if cursor == input.len() { return (0.0, cursor); }
    let number_start = cursor;
    if matches!(input[cursor], b'+' | b'-') { cursor += 1; }
    let mut digits = 0;
    while cursor < input.len() && input[cursor].is_ascii_digit() { cursor += 1; digits += 1; }
    if cursor < input.len() && input[cursor] == b'.' {
        cursor += 1;
        while cursor < input.len() && input[cursor].is_ascii_digit() { cursor += 1; digits += 1; }
    }
    if digits == 0 { return (0.0, 0); }
    if cursor < input.len() && matches!(input[cursor], b'e' | b'E') {
        let exponent_start = cursor;
        cursor += 1;
        if cursor < input.len() && matches!(input[cursor], b'+' | b'-') { cursor += 1; }
        let first_digit = cursor;
        while cursor < input.len() && input[cursor].is_ascii_digit() { cursor += 1; }
        if cursor == first_digit { cursor = exponent_start; }
    }
    let decimal = std::str::from_utf8(&input[number_start..cursor]).expect("decimal grammar is ASCII");
    let value = decimal.parse::<f64>().expect("decimal grammar is valid");
    (value, cursor)
}

#[derive(Clone, Copy, Debug)]
pub enum PrecisionStyle {
    Precision,
    Exponential,
}

pub fn precision(value: f64, count: usize, style: PrecisionStyle) -> Option<String> {
    let exponential = matches!(style, PrecisionStyle::Exponential);
    if !value.is_finite() { return shortest(value, ShortestStyle::EcmaScript); }
    if count == 0 || count > 121 || (!exponential && count > 120) { return None; }
    let (mut digits, scale) = if value == 0.0 { (vec![0], 0) } else { exact_decimal(value) };
    let mut point = digits.len() as i32 - scale as i32;
    if digits.len() > count {
        let drop = digits.len() - count;
        let round = digits[drop - 1] >= 5;
        digits.drain(..drop);
        if round {
            let mut carry = 1;
            for digit in &mut digits {
                let next = *digit + carry;
                *digit = next % 10;
                carry = next / 10;
                if carry == 0 { break; }
            }
            if carry != 0 { digits.push(carry); }
            if digits.len() > count { digits.remove(0); point += 1; }
        }
    } else {
        digits.splice(0..0, std::iter::repeat_n(0, count - digits.len()));
    }
    let text: String = digits.iter().rev().map(|digit| char::from(b'0' + *digit)).collect();
    let mut result = String::new();
    if value.is_sign_negative() && value != 0.0 { result.push('-'); }
    if exponential || 1 - point > 6 || point > count as i32 {
        result.push(text.as_bytes()[0] as char);
        if count > 1 { result.push('.'); result.push_str(&text[1..]); }
        result.push('e');
        if point - 1 >= 0 { result.push('+'); }
        result.push_str(&(point - 1).to_string());
    } else if point <= 0 {
        result.push_str("0.");
        result.extend(std::iter::repeat_n('0', (-point) as usize));
        result.push_str(&text);
    } else {
        result.push_str(&text[..point as usize]);
        if (point as usize) < count { result.push('.'); result.push_str(&text[point as usize..]); }
    }
    Some(result)
}

pub fn shortest_exponential(value: f64) -> String {
    let shortest = shortest(value, ShortestStyle::EcmaScript).expect("ECMAScript formats all values");
    if !value.is_finite() { return shortest; }
    if value == 0.0 { return "0e+0".into(); }
    let (sign, magnitude) = if let Some(rest) = shortest.strip_prefix('-') { ("-", rest) } else { ("", shortest.as_str()) };
    if magnitude.contains('e') { return shortest; }
    let point = magnitude.find('.').unwrap_or(magnitude.len());
    let raw: String = magnitude.chars().filter(|&c| c != '.').collect();
    let first = raw.bytes().position(|b| b != b'0').expect("nonzero value");
    let last = raw.bytes().rposition(|b| b != b'0').expect("nonzero value") + 1;
    let digits = &raw[first..last];
    let exponent = point as i32 - first as i32 - 1;
    let mut result = String::from(sign);
    result.push(digits.as_bytes()[0] as char);
    if digits.len() > 1 { result.push('.'); result.push_str(&digits[1..]); }
    result.push('e');
    if exponent >= 0 { result.push('+'); }
    result.push_str(&exponent.to_string());
    result
}

#[derive(Debug, PartialEq, Eq)]
pub enum Utf16Conversion {
    Complete(String),
    Invalid { valid_units: usize, prefix: String },
}

pub fn utf16_to_utf8(input: &[u16]) -> Utf16Conversion {
    let mut prefix = String::new();
    let mut cursor = 0;
    while cursor < input.len() {
        let first = input[cursor];
        let scalar = if (0xd800..=0xdbff).contains(&first) {
            if cursor + 1 == input.len() || !(0xdc00..=0xdfff).contains(&input[cursor + 1]) {
                return Utf16Conversion::Invalid { valid_units: cursor, prefix };
            }
            cursor += 2;
            0x10000 + ((u32::from(first) - 0xd800) << 10) + u32::from(input[cursor - 1]) - 0xdc00
        } else if (0xdc00..=0xdfff).contains(&first) {
            return Utf16Conversion::Invalid { valid_units: cursor, prefix };
        } else {
            cursor += 1;
            u32::from(first)
        };
        prefix.push(char::from_u32(scalar).expect("valid UTF-16 scalar"));
    }
    Utf16Conversion::Complete(prefix)
}

#[derive(Clone, Copy, Debug)]
pub enum IntegerKind { I32, U32, I64, U64 }

#[derive(Clone, Copy, Debug)]
pub enum IntegerRadix { Decimal, Hexadecimal }

pub fn parse_integer(input: &[u8], kind: IntegerKind, radix: IntegerRadix) -> (u64, bool) {
    let mut cursor = 0;
    while cursor < input.len() && matches!(input[cursor], b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c) { cursor += 1; }
    let leading_space = cursor != 0;
    let signed = matches!(kind, IntegerKind::I32 | IntegerKind::I64);
    let negative = input.get(cursor) == Some(&b'-');
    if negative && !signed { return (0, false); }
    if matches!(input.get(cursor), Some(b'+' | b'-')) { cursor += 1; }
    let base = match radix { IntegerRadix::Decimal => 10u128, IntegerRadix::Hexadecimal => 16u128 };
    if base == 16 && input.len().saturating_sub(cursor) > 2 && input[cursor] == b'0' && matches!(input[cursor + 1], b'x' | b'X') { cursor += 2; }
    if cursor == input.len() { return (0, false); }
    let limit = match (kind, negative) {
        (IntegerKind::I32, false) => i32::MAX as u128,
        (IntegerKind::I32, true) => 1u128 << 31,
        (IntegerKind::U32, _) => u32::MAX as u128,
        (IntegerKind::I64, false) => i64::MAX as u128,
        (IntegerKind::I64, true) => 1u128 << 63,
        (IntegerKind::U64, _) => u64::MAX as u128,
    };
    let mut value = 0u128;
    let mut valid = !leading_space;
    for &byte in &input[cursor..] {
        let digit = match byte {
            b'0'..=b'9' => u128::from(byte - b'0'),
            b'a'..=b'f' if base == 16 => u128::from(byte - b'a' + 10),
            b'A'..=b'F' if base == 16 => u128::from(byte - b'A' + 10),
            _ => { valid = false; break; }
        };
        if value > (limit - digit) / base { value = limit; valid = false; break; }
        value = value * base + digit;
    }
    let value = value as u64;
    (if negative { value.wrapping_neg() } else { value }, valid)
}

pub fn validate_utf8(input: &[u8]) -> (bool, bool) {
    match std::str::from_utf8(input) {
        Ok(text) => (text.chars().all(|c| {
            let n = u32::from(c);
            !(0xfdd0..=0xfdef).contains(&n) && n & 0xfffe != 0xfffe
        }), true),
        Err(_) => (false, false),
    }
}

pub fn blink_precision(value: f64, digits: usize) -> String {
    let mut text = precision(value, digits, PrecisionStyle::Precision).unwrap_or_default();
    if !text.contains('e') && text.contains('.') {
        while text.ends_with('0') { text.pop(); }
        if text.ends_with('.') { text.pop(); }
    }
    text
}

pub fn blink_fixed(value: f64, digits: usize) -> String {
    if !value.is_finite() { return shortest(value, ShortestStyle::EcmaScript).expect("ECMAScript formats special values"); }
    let mut text = fixed(value, digits).unwrap_or_default();
    if value == 0.0 && text.starts_with('-') { text.remove(0); }
    text
}

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
    use super::{shortest, ShortestStyle};

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

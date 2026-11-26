use turso_core::Numeric;
use turso_core::Value;
use turso_core::ValueRef;

#[derive(Clone, Debug, PartialEq)]
pub enum SqlValue {
    Null,
    Integer(i64),
    Float(f64),
    Text(Vec<u8>),
    Blob(Vec<u8>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColumnType {
    Integer = 1,
    Float = 2,
    Text = 3,
    Blob = 4,
    Null = 5,
}

impl SqlValue {
    pub fn from_turso(value: ValueRef<'_>) -> Self {
        match value {
            ValueRef::Null => Self::Null,
            ValueRef::Numeric(Numeric::Integer(value)) => Self::Integer(value),
            ValueRef::Numeric(Numeric::Float(value)) => Self::Float(f64::from(value)),
            ValueRef::Text(text) => Self::Text(text.as_str().as_bytes().to_vec()),
            ValueRef::Blob(blob) => Self::Blob(blob.to_vec()),
        }
    }

    pub fn to_turso(&self) -> Value {
        match self {
            Self::Null => Value::Null,
            Self::Integer(value) => Value::from_i64(*value),
            Self::Float(value) => Value::from_f64(*value),
            Self::Text(bytes) => Value::build_text(String::from_utf8_lossy(bytes).into_owned()),
            Self::Blob(bytes) => Value::from_slice(bytes).expect("blob allocation"),
        }
    }

    pub fn column_type(&self) -> ColumnType {
        match self {
            Self::Null => ColumnType::Null,
            Self::Integer(_) => ColumnType::Integer,
            Self::Float(_) => ColumnType::Float,
            Self::Text(_) => ColumnType::Text,
            Self::Blob(_) => ColumnType::Blob,
        }
    }

    pub fn int64(&self) -> i64 {
        match self {
            Self::Null => 0,
            Self::Integer(value) => *value,
            Self::Float(value) => real_to_i64(*value),
            Self::Text(bytes) | Self::Blob(bytes) => atoi64(bytes),
        }
    }

    pub fn double(&self) -> f64 {
        match self {
            Self::Null => 0.0,
            Self::Integer(value) => *value as f64,
            Self::Float(value) => *value,
            Self::Text(bytes) | Self::Blob(bytes) => atof(bytes),
        }
    }

    pub fn text(&self) -> Vec<u8> {
        match self {
            Self::Null => Vec::new(),
            Self::Integer(value) => value.to_string().into_bytes(),
            Self::Float(value) => render_real(*value).into_bytes(),
            Self::Text(bytes) | Self::Blob(bytes) => bytes.clone(),
        }
    }
}

pub fn render_real(value: f64) -> String {
    Value::from_f64(value).to_string()
}

fn real_to_i64(value: f64) -> i64 {
    if value <= i64::MIN as f64 {
        i64::MIN
    } else if value >= i64::MAX as f64 {
        i64::MAX
    } else {
        value as i64
    }
}

fn is_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

pub fn atoi64(bytes: &[u8]) -> i64 {
    let mut pos = 0;
    while pos < bytes.len() && is_space(bytes[pos]) {
        pos += 1;
    }
    let mut negative = false;
    if pos < bytes.len() {
        if bytes[pos] == b'-' {
            negative = true;
            pos += 1;
        } else if bytes[pos] == b'+' {
            pos += 1;
        }
    }
    while pos < bytes.len() && bytes[pos] == b'0' {
        pos += 1;
    }
    let mut magnitude: u64 = 0;
    let mut digits = 0;
    while pos < bytes.len() && bytes[pos].is_ascii_digit() {
        magnitude = magnitude.wrapping_mul(10).wrapping_add(u64::from(bytes[pos] - b'0'));
        pos += 1;
        digits += 1;
    }
    if digits > 19 || magnitude > i64::MAX as u64 {
        if negative { i64::MIN } else { i64::MAX }
    } else if negative {
        -(magnitude as i64)
    } else {
        magnitude as i64
    }
}

fn dekker_mul2(x: &mut [f64; 2], y: f64, yy: f64) {
    let split = |value: f64| f64::from_bits(value.to_bits() & 0xffff_ffff_fc00_0000);
    let hx = split(x[0]);
    let tx = x[0] - hx;
    let hy = split(y);
    let ty = y - hy;
    let p = hx * hy;
    let q = hx * ty + tx * hy;
    let c = p + q;
    let cc = p - c + q + tx * ty;
    let cc = x[0] * yy + x[1] * y + cc;
    x[0] = c + cc;
    x[1] = c - x[0];
    x[1] += cc;
}

pub fn atof(bytes: &[u8]) -> f64 {
    let end = bytes.len();
    let mut pos = 0;
    while pos < end && is_space(bytes[pos]) {
        pos += 1;
    }
    if pos >= end {
        return 0.0;
    }
    let mut sign = 1;
    if bytes[pos] == b'-' {
        sign = -1;
        pos += 1;
    } else if bytes[pos] == b'+' {
        pos += 1;
    }
    let limit = (u64::MAX - 9) / 10;
    let mut significand: u64 = 0;
    let mut shift: i32 = 0;
    let mut exponent: i32 = 0;
    let mut exponent_sign = 1;
    'parse: {
        while pos < end && bytes[pos].is_ascii_digit() {
            significand = significand * 10 + u64::from(bytes[pos] - b'0');
            pos += 1;
            if significand >= limit {
                while pos < end && bytes[pos].is_ascii_digit() {
                    pos += 1;
                    shift += 1;
                }
            }
        }
        if pos >= end {
            break 'parse;
        }
        if bytes[pos] == b'.' {
            pos += 1;
            while pos < end && bytes[pos].is_ascii_digit() {
                if significand < limit {
                    significand = significand * 10 + u64::from(bytes[pos] - b'0');
                    shift -= 1;
                }
                pos += 1;
            }
        }
        if pos >= end {
            break 'parse;
        }
        if bytes[pos] == b'e' || bytes[pos] == b'E' {
            pos += 1;
            if pos >= end {
                break 'parse;
            }
            if bytes[pos] == b'-' {
                exponent_sign = -1;
                pos += 1;
            } else if bytes[pos] == b'+' {
                pos += 1;
            }
            while pos < end && bytes[pos].is_ascii_digit() {
                exponent = if exponent < 10000 { exponent * 10 + i32::from(bytes[pos] - b'0') } else { 10000 };
                pos += 1;
            }
        }
    }
    if significand == 0 {
        return if sign < 0 { -0.0 } else { 0.0 };
    }
    let mut e = exponent * exponent_sign + shift;
    while e > 0 && significand < (u64::MAX - 0x7ff) / 10 {
        significand *= 10;
        e -= 1;
    }
    while e < 0 && significand % 10 == 0 {
        significand /= 10;
        e += 1;
    }
    let mut rr = [significand as f64, 0.0];
    if rr[0] <= 18446744073709549568.0 {
        let rounded = rr[0] as u64;
        rr[1] = if significand >= rounded { (significand - rounded) as f64 } else { -((rounded - significand) as f64) };
    }
    if e > 0 {
        while e >= 100 {
            e -= 100;
            dekker_mul2(&mut rr, 1.0e+100, -1.5902891109759918046e+83);
        }
        while e >= 10 {
            e -= 10;
            dekker_mul2(&mut rr, 1.0e+10, 0.0);
        }
        while e >= 1 {
            e -= 1;
            dekker_mul2(&mut rr, 1.0e+01, 0.0);
        }
    } else {
        while e <= -100 {
            e += 100;
            dekker_mul2(&mut rr, 1.0e-100, -1.99918998026028836196e-117);
        }
        while e <= -10 {
            e += 10;
            dekker_mul2(&mut rr, 1.0e-10, -3.6432197315497741579e-27);
        }
        while e <= -1 {
            e += 1;
            dekker_mul2(&mut rr, 1.0e-01, -5.5511151231257827021e-18);
        }
    }
    let mut result = rr[0] + rr[1];
    if result.is_nan() {
        result = f64::INFINITY;
    }
    if sign < 0 { -result } else { result }
}

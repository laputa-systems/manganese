pub type Tag = u32;

pub const CONSTRUCTED: Tag = 0x20 << 24;
pub const CONTEXT_SPECIFIC: Tag = 0x80 << 24;
pub const BOOLEAN: Tag = 1;
pub const INTEGER: Tag = 2;
pub const BIT_STRING: Tag = 3;
pub const OCTET_STRING: Tag = 4;
pub const NULL: Tag = 5;
pub const OBJECT: Tag = 6;
pub const UTF8_STRING: Tag = 12;
pub const SEQUENCE: Tag = 16 | CONSTRUCTED;
pub const SET: Tag = 17 | CONSTRUCTED;
pub const PRINTABLE_STRING: Tag = 19;
pub const T61_STRING: Tag = 20;
pub const IA5_STRING: Tag = 22;
pub const UTC_TIME: Tag = 23;
pub const GENERALIZED_TIME: Tag = 24;
pub const UNIVERSAL_STRING: Tag = 28;
pub const BMP_STRING: Tag = 30;

pub fn context(number: u32) -> Tag {
    CONTEXT_SPECIFIC | number
}

pub fn context_constructed(number: u32) -> Tag {
    CONTEXT_SPECIFIC | CONSTRUCTED | number
}

struct Element<'a> {
    tag: Tag,
    header: usize,
    total: usize,
    value: &'a [u8],
}

fn oid_component(data: &[u8], offset: &mut usize) -> Option<u64> {
    let mut value: u64 = 0;
    loop {
        let byte = *data.get(*offset)?;
        *offset += 1;
        if value >> (64 - 7) != 0 {
            return None;
        }
        if value == 0 && byte == 0x80 {
            return None;
        }
        value = (value << 7) | u64::from(byte & 0x7f);
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
}

fn element(data: &[u8]) -> Option<Element<'_>> {
    let first = *data.first()?;
    let mut offset = 1;
    let mut tag = (Tag::from(first) & 0xe0) << 24;
    let mut number = Tag::from(first) & 0x1f;
    if number == 0x1f {
        let value = oid_component(data, &mut offset)?;
        if value > 0x1fff_ffff || value < 0x1f {
            return None;
        }
        number = value as Tag;
    }
    tag |= number;
    if tag & !CONSTRUCTED == 0 {
        return None;
    }
    let length_byte = *data.get(offset)?;
    offset += 1;
    let (length, header) = if length_byte & 0x80 == 0 {
        (usize::from(length_byte), offset)
    } else {
        let count = usize::from(length_byte & 0x7f);
        if count == 0 || count > 4 {
            return None;
        }
        let bytes = data.get(offset..offset + count)?;
        let mut length: u64 = 0;
        for byte in bytes {
            length = (length << 8) | u64::from(*byte);
        }
        if length < 128 || length >> ((count - 1) * 8) == 0 {
            return None;
        }
        (usize::try_from(length).ok()?, offset + count)
    };
    let total = header.checked_add(length)?;
    let value = data.get(header..total)?;
    Some(Element { tag, header, total, value })
}

#[derive(Clone, Copy)]
pub struct Parser<'a> {
    data: &'a [u8],
}

impl<'a> Parser<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data }
    }

    pub fn has_more(&self) -> bool {
        !self.data.is_empty()
    }

    pub fn remaining(&self) -> &'a [u8] {
        self.data
    }

    pub fn read_raw_tlv(&mut self) -> Option<&'a [u8]> {
        let e = element(self.data)?;
        let tlv = &self.data[..e.total];
        self.data = &self.data[e.total..];
        Some(tlv)
    }

    pub fn read_tag_and_value(&mut self) -> Option<(Tag, &'a [u8])> {
        let e = element(self.data)?;
        let _ = e.header;
        self.data = &self.data[e.total..];
        Some((e.tag, e.value))
    }

    pub fn read_optional_tag(&mut self, tag: Tag) -> Option<Option<&'a [u8]>> {
        if !self.has_more() {
            return Some(None);
        }
        let e = element(self.data)?;
        if e.tag == tag {
            self.data = &self.data[e.total..];
            Some(Some(e.value))
        } else {
            Some(None)
        }
    }

    pub fn read_tag(&mut self, tag: Tag) -> Option<&'a [u8]> {
        let e = element(self.data)?;
        if e.tag != tag {
            return None;
        }
        self.data = &self.data[e.total..];
        Some(e.value)
    }

    pub fn skip_tag(&mut self, tag: Tag) -> bool {
        self.read_tag(tag).is_some()
    }

    pub fn read_constructed(&mut self, tag: Tag) -> Option<Parser<'a>> {
        if tag & CONSTRUCTED == 0 {
            return None;
        }
        self.read_tag(tag).map(Parser::new)
    }

    pub fn read_sequence(&mut self) -> Option<Parser<'a>> {
        self.read_constructed(SEQUENCE)
    }

    pub fn read_uint8(&mut self) -> Option<u8> {
        parse_uint8(self.read_tag(INTEGER)?)
    }

    pub fn read_uint64(&mut self) -> Option<u64> {
        parse_uint64(self.read_tag(INTEGER)?)
    }

    pub fn read_bit_string(&mut self) -> Option<BitString<'a>> {
        parse_bit_string(self.read_tag(BIT_STRING)?)
    }
}

pub fn is_valid_integer(data: &[u8]) -> Option<bool> {
    let first = *data.first()?;
    let negative = first & 0x80 != 0;
    if let Some(second) = data.get(1) {
        if (first == 0 && second & 0x80 == 0) || (first == 0xff && second & 0x80 != 0) {
            return None;
        }
    }
    Some(negative)
}

pub fn parse_uint64(data: &[u8]) -> Option<u64> {
    if is_valid_integer(data)? {
        return None;
    }
    let unsigned_length = if data[0] == 0 && data.len() > 1 { data.len() - 1 } else { data.len() };
    if unsigned_length > 8 {
        return None;
    }
    Some(data.iter().fold(0u64, |value, byte| (value << 8) | u64::from(*byte)))
}

pub fn parse_uint8(data: &[u8]) -> Option<u8> {
    u8::try_from(parse_uint64(data)?).ok()
}

pub fn parse_bool(data: &[u8]) -> Option<bool> {
    match data {
        [0] => Some(false),
        [0xff] => Some(true),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BitString<'a> {
    pub bytes: &'a [u8],
    pub unused_bits: u8,
}

impl BitString<'_> {
    pub fn asserts_bit(&self, bit: usize) -> bool {
        let index = bit / 8;
        match self.bytes.get(index) {
            Some(byte) => byte & (1 << (7 - (bit - index * 8))) != 0,
            None => false,
        }
    }
}

pub fn parse_bit_string(data: &[u8]) -> Option<BitString<'_>> {
    let (&unused_bits, bytes) = data.split_first()?;
    if unused_bits > 7 {
        return None;
    }
    if unused_bits > 0 {
        let last = *bytes.last()?;
        if last & (0xff >> (8 - unused_bits)) != 0 {
            return None;
        }
    }
    Some(BitString { bytes, unused_bits })
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct GeneralizedTime {
    pub year: u32,
    pub month: u32,
    pub day: u32,
    pub hours: u32,
    pub minutes: u32,
    pub seconds: u32,
}

fn digits(data: &[u8], offset: &mut usize, count: usize) -> Option<u32> {
    let mut value = 0u32;
    for _ in 0..count {
        let digit = *data.get(*offset)?;
        *offset += 1;
        if !digit.is_ascii_digit() {
            return None;
        }
        value = value * 10 + u32::from(digit - b'0');
    }
    Some(value)
}

fn validate(time: &GeneralizedTime) -> bool {
    if time.month < 1 || time.month > 12 || time.day < 1 || time.hours > 23 || time.minutes > 59 || time.seconds > 60 {
        return false;
    }
    let max_day = match time.month {
        4 | 6 | 9 | 11 => 30,
        2 if time.year % 4 == 0 && (time.year % 100 != 0 || time.year % 400 == 0) => 29,
        2 => 28,
        _ => 31,
    };
    time.day <= max_day
}

fn parse_time(data: &[u8], year_digits: usize) -> Option<GeneralizedTime> {
    let mut offset = 0;
    let mut time = GeneralizedTime {
        year: digits(data, &mut offset, year_digits)?,
        month: digits(data, &mut offset, 2)?,
        day: digits(data, &mut offset, 2)?,
        hours: digits(data, &mut offset, 2)?,
        minutes: digits(data, &mut offset, 2)?,
        seconds: digits(data, &mut offset, 2)?,
    };
    if data.get(offset) != Some(&b'Z') || offset + 1 != data.len() {
        return None;
    }
    if year_digits == 2 {
        time.year += if time.year < 50 { 2000 } else { 1900 };
    }
    validate(&time).then_some(time)
}

pub fn parse_utc_time(data: &[u8]) -> Option<GeneralizedTime> {
    parse_time(data, 2)
}

pub fn parse_generalized_time(data: &[u8]) -> Option<GeneralizedTime> {
    parse_time(data, 4)
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146097 + day_of_era - 719468
}

impl GeneralizedTime {
    pub fn to_posix(&self) -> i64 {
        let seconds = if self.seconds == 60 { 59 } else { self.seconds };
        days_from_civil(i64::from(self.year), i64::from(self.month), i64::from(self.day)) * 86400
            + i64::from(self.hours) * 3600
            + i64::from(self.minutes) * 60
            + i64::from(seconds)
    }

    pub fn from_posix(posix: i64) -> Option<Self> {
        let days = posix.div_euclid(86400);
        let rem = posix.rem_euclid(86400);
        let z = days + 719468;
        let era = if z >= 0 { z } else { z - 146096 } / 146097;
        let day_of_era = z - era * 146097;
        let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146096) / 365;
        let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
        let mp = (5 * day_of_year + 2) / 153;
        let day = day_of_year - (153 * mp + 2) / 5 + 1;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = year_of_era + era * 400 + i64::from(month <= 2);
        if !(0..=9999).contains(&year) {
            return None;
        }
        Some(Self {
            year: year as u32,
            month: month as u32,
            day: day as u32,
            hours: (rem / 3600) as u32,
            minutes: (rem % 3600 / 60) as u32,
            seconds: (rem % 60) as u32,
        })
    }
}

fn is_valid_code_point(value: u32) -> bool {
    !(value > 0x10ffff || value & 0xfffe == 0xfffe || (0xfdd0..=0xfdef).contains(&value) || (0xd800..=0xdfff).contains(&value))
}

fn push_utf8(out: &mut Vec<u8>, value: u32) -> bool {
    if !is_valid_code_point(value) {
        return false;
    }
    match char::from_u32(value) {
        Some(c) => {
            let mut buffer = [0u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut buffer).as_bytes());
            true
        }
        None => false,
    }
}

pub fn parse_universal_string(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() % 4 != 0 {
        return None;
    }
    let mut out = Vec::new();
    for chunk in data.chunks(4) {
        if !push_utf8(&mut out, u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]])) {
            return None;
        }
    }
    Some(out)
}

pub fn parse_bmp_string(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::new();
    for chunk in data.chunks(2) {
        if !push_utf8(&mut out, u32::from(u16::from_be_bytes([chunk[0], chunk[1]]))) {
            return None;
        }
    }
    Some(out)
}

pub fn parse_teletex_as_latin1(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for &byte in data {
        if byte <= 0x7f {
            out.push(byte);
        } else {
            out.push(0xc0 | (byte >> 6));
            out.push(0x80 | (byte & 0x3f));
        }
    }
    out
}

pub fn is_printable_string(data: &[u8]) -> bool {
    data.iter().all(|&c| c.is_ascii_alphabetic() || c == b' ' || (b'\''..=b':').contains(&c) || c == b'=' || c == b'?')
}

pub fn encode(tag: Tag, content: &[u8]) -> Vec<u8> {
    let class_and_constructed = ((tag >> 24) & 0xe0) as u8;
    let number = tag & 0x1fff_ffff;
    let mut out = Vec::with_capacity(content.len() + 6);
    if number < 0x1f {
        out.push(class_and_constructed | number as u8);
    } else {
        out.push(class_and_constructed | 0x1f);
        let mut groups = Vec::new();
        let mut value = number;
        groups.push((value & 0x7f) as u8);
        value >>= 7;
        while value != 0 {
            groups.push(0x80 | (value & 0x7f) as u8);
            value >>= 7;
        }
        out.extend(groups.iter().rev());
    }
    let length = content.len();
    if length < 0x80 {
        out.push(length as u8);
    } else {
        let bytes = length.to_be_bytes();
        let skip = bytes.iter().take_while(|b| **b == 0).count();
        out.push(0x80 | (bytes.len() - skip) as u8);
        out.extend_from_slice(&bytes[skip..]);
    }
    out.extend_from_slice(content);
    out
}

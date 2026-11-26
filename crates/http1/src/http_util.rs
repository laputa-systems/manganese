use std::collections::BTreeSet;

pub fn is_lws(byte: u8) -> bool {
    byte == b' ' || byte == b'\t'
}

pub fn trim_lws(bytes: &[u8]) -> &[u8] {
    let (begin, end) = trim_lws_range(bytes, 0, bytes.len());
    &bytes[begin..end]
}

pub fn trim_lws_range(bytes: &[u8], mut begin: usize, mut end: usize) -> (usize, usize) {
    while begin < end && is_lws(bytes[begin]) {
        begin += 1;
    }
    while begin < end && is_lws(bytes[end - 1]) {
        end -= 1;
    }
    (begin, end)
}

pub fn is_token_char(byte: u8) -> bool {
    !(byte >= 0x7f || byte <= 0x20 || b"()<>@,;:\\\"/[]?={}".contains(&byte))
}

pub fn is_token(bytes: &[u8]) -> bool {
    !bytes.is_empty() && bytes.iter().all(|&byte| is_token_char(byte))
}

pub fn is_valid_header_name(name: &[u8]) -> bool {
    is_token(name)
}

pub fn is_valid_header_value(value: &[u8]) -> bool {
    !value.iter().any(|&byte| byte == 0 || byte == b'\r' || byte == b'\n')
}

pub fn is_non_coalescing_header(name: &[u8]) -> bool {
    const NON_COALESCING: [&[u8]; 9] = [
        b"date", b"expires", b"last-modified", b"location", b"retry-after", b"set-cookie", b"www-authenticate",
        b"proxy-authenticate", b"strict-transport-security",
    ];
    NON_COALESCING.iter().any(|header| name.eq_ignore_ascii_case(header))
}

pub fn locate_start_of_status_line(buf: &[u8]) -> Option<usize> {
    const SLOP: usize = 4;
    const HTTP_LEN: usize = 4;
    if buf.len() < HTTP_LEN {
        return None;
    }
    let last = (buf.len() - HTTP_LEN).min(SLOP);
    (0..=last).find(|&i| buf[i..i + HTTP_LEN].eq_ignore_ascii_case(b"http"))
}

pub fn locate_end_of_headers(buf: &[u8], start: usize) -> Option<usize> {
    let mut last = 0u8;
    let mut was_lf = false;
    for (i, &byte) in buf.iter().enumerate().skip(start) {
        if byte == b'\n' {
            if was_lf {
                return Some(i + 1);
            }
            was_lf = true;
        } else if byte != b'\r' || last != b'\n' {
            was_lf = false;
        }
        last = byte;
    }
    None
}

fn is_line_segment_continuable(line: &[u8]) -> bool {
    let Some(colon) = line.iter().position(|&byte| byte == b':') else { return false };
    let name = &line[..colon];
    !name.is_empty() && !is_lws(name[0])
}

pub fn assemble_raw_headers(input: &[u8]) -> Vec<u8> {
    let mut input = input;
    if let Some(start) = locate_start_of_status_line(input) {
        input = &input[start..];
    }
    let status_end = input.iter().position(|&byte| byte == b'\r' || byte == b'\n').unwrap_or(input.len());
    let mut raw = input[..status_end].to_vec();
    let mut previous_continuable = false;
    for line in tokens(&input[status_end..], b"\r\n") {
        if previous_continuable && is_lws(line[0]) {
            raw.push(b' ');
            let first = line.iter().position(|&byte| !is_lws(byte)).unwrap_or(line.len());
            raw.extend_from_slice(&line[first..]);
        } else {
            raw.push(b'\n');
            raw.extend_from_slice(line);
            previous_continuable = is_line_segment_continuable(line);
        }
    }
    raw.extend_from_slice(b"\n\n");
    raw.retain(|&byte| byte != 0);
    for byte in &mut raw {
        if *byte == b'\n' {
            *byte = 0;
        }
    }
    raw
}

pub fn tokens<'a>(input: &'a [u8], delimiters: &'a [u8]) -> impl Iterator<Item = &'a [u8]> + 'a {
    input.split(move |byte| delimiters.contains(byte)).filter(|token| !token.is_empty())
}

pub struct HeaderLine {
    pub name: (usize, usize),
    pub values: (usize, usize),
}

pub fn header_lines(headers: &[u8]) -> Vec<HeaderLine> {
    let mut lines = Vec::new();
    let mut begin = 0;
    while begin < headers.len() {
        if headers[begin] == 0 {
            begin += 1;
            continue;
        }
        let end = headers[begin..].iter().position(|&byte| byte == 0).map_or(headers.len(), |offset| begin + offset);
        let line = &headers[begin..end];
        if let Some(colon) = line.iter().position(|&byte| byte == b':') {
            let colon = begin + colon;
            if colon != begin && !is_lws(headers[begin]) {
                let name = trim_lws_range(headers, begin, colon);
                if is_token(&headers[name.0..name.1]) {
                    lines.push(HeaderLine { name, values: trim_lws_range(headers, colon + 1, end) });
                }
            }
        }
        begin = end;
    }
    lines
}

pub fn quoted_values(values: &[u8], delimiter: u8) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut begin = 0;
    loop {
        let mut end = begin;
        let mut in_quote = false;
        let mut in_escape = false;
        let mut quote = 0u8;
        while end < values.len() {
            let byte = values[end];
            if in_quote {
                if in_escape {
                    in_escape = false;
                } else if byte == b'\\' {
                    in_escape = true;
                } else if byte == quote {
                    in_quote = false;
                }
            } else {
                if byte == delimiter {
                    break;
                }
                quote = byte;
                in_quote = byte == b'"';
            }
            end += 1;
        }
        out.push(trim_lws_range(values, begin, end));
        if end == values.len() {
            return out;
        }
        begin = end + 1;
    }
}

fn digit(byte: u8, base: u32) -> Option<u32> {
    char::from(byte).to_digit(base)
}

pub struct Parsed<T> {
    pub value: T,
    pub valid: bool,
}

pub fn string_to_number(input: &[u8], base: u32, max: i64, min: i64) -> Parsed<i64> {
    let mut begin = 0;
    let mut leading_whitespace = false;
    while begin < input.len() && is_ascii_whitespace(input[begin]) {
        leading_whitespace = true;
        begin += 1;
    }
    let mut negative = false;
    if begin < input.len() && input[begin] == b'-' {
        negative = true;
        begin += 1;
    } else if begin < input.len() && input[begin] == b'+' {
        begin += 1;
    }
    let mut digits = &input[begin..];
    if digits.is_empty() {
        return Parsed { value: 0, valid: false };
    }
    if base == 16 && digits.len() > 2 && digits[0] == b'0' && (digits[1] == b'x' || digits[1] == b'X') {
        digits = &digits[2..];
    }
    let base_value = i64::from(base);
    let mut value: i64 = 0;
    for (index, &byte) in digits.iter().enumerate() {
        let Some(new_digit) = digit(byte, base) else { return Parsed { value, valid: false } };
        let new_digit = i64::from(new_digit);
        if index != 0 {
            if negative {
                if value < min / base_value || (value == min / base_value && new_digit > -(min % base_value)) {
                    return Parsed { value: min, valid: false };
                }
            } else if value > max / base_value || (value == max / base_value && new_digit > max % base_value) {
                return Parsed { value: max, valid: false };
            }
            value *= base_value;
        }
        value = if negative { value - new_digit } else { value + new_digit };
    }
    Parsed { value, valid: !leading_whitespace }
}

fn is_ascii_whitespace(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

fn lws_inside(bytes: &[u8]) -> bool {
    bytes.iter().any(|&byte| is_lws(byte))
}

pub fn parse_accept_encoding(accept_encoding: &[u8]) -> Option<BTreeSet<Vec<u8>>> {
    if accept_encoding.contains(&b'"') {
        return None;
    }
    let mut allowed = BTreeSet::new();
    for entry in tokens(accept_encoding, b",") {
        let entry = trim_lws(entry);
        let Some(semicolon) = entry.iter().position(|&byte| byte == b';') else {
            if lws_inside(entry) {
                return None;
            }
            allowed.insert(entry.to_ascii_lowercase());
            continue;
        };
        let encoding = trim_lws(&entry[..semicolon]);
        if lws_inside(encoding) {
            return None;
        }
        let params = trim_lws(&entry[semicolon + 1..]);
        let equals = params.iter().position(|&byte| byte == b'=')?;
        if !trim_lws(&params[..equals]).eq_ignore_ascii_case(b"q") {
            return None;
        }
        let qvalue = trim_lws(&params[equals + 1..]);
        match qvalue.first() {
            None => return None,
            Some(b'1') => {
                if b"1.000".starts_with(qvalue) {
                    allowed.insert(encoding.to_ascii_lowercase());
                    continue;
                }
                return None;
            }
            Some(b'0') => {}
            Some(_) => return None,
        }
        if qvalue.len() == 1 {
            continue;
        }
        if qvalue.len() <= 2 || qvalue.len() > 5 || qvalue[1] != b'.' {
            return None;
        }
        let mut nonzero = false;
        for &byte in &qvalue[2..] {
            if !byte.is_ascii_digit() {
                return None;
            }
            nonzero |= byte != b'0';
        }
        if nonzero {
            allowed.insert(encoding.to_ascii_lowercase());
        }
    }
    if allowed.is_empty() {
        allowed.insert(b"*".to_vec());
        return Some(allowed);
    }
    allowed.insert(b"identity".to_vec());
    for (a, b) in [(&b"gzip"[..], &b"x-gzip"[..]), (b"x-gzip", b"gzip"), (b"compress", b"x-compress"), (b"x-compress", b"compress")] {
        if allowed.contains(a) {
            allowed.insert(b.to_vec());
        }
    }
    Some(allowed)
}

pub fn parse_content_encoding(content_encoding: &[u8]) -> Option<BTreeSet<Vec<u8>>> {
    if content_encoding.iter().any(|byte| b"\"=;*".contains(byte)) {
        return None;
    }
    let mut used = BTreeSet::new();
    for token in tokens(content_encoding, b",") {
        let encoding = trim_lws(token);
        if lws_inside(encoding) {
            return None;
        }
        used.insert(encoding.to_ascii_lowercase());
    }
    Some(used)
}

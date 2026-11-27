use std::io::{self, Read, Write};

use regex_ecma::RegexBuilder;

const JS_SPACE_INNER: &str = "\\t\\n\\x0b\\x0c\\r \\u{a0}\\u{1680}\\u{2000}-\\u{200a}\\u{2028}\\u{2029}\\u{202f}\\u{205f}\\u{3000}\\u{feff}";

#[derive(Default, Clone, Copy)]
struct Flags {
    global: bool,
    ignore_case: bool,
    multiline: bool,
    dot_all: bool,
    unicode: bool,
}

impl Flags {
    fn parse(text: &str) -> Result<Self, ()> {
        let mut result = Self::default();
        let mut seen = [false; 128];
        for byte in text.bytes() {
            if byte >= 128 || seen[byte as usize] {
                return Err(());
            }
            seen[byte as usize] = true;
            match byte {
                b'd' => {}
                b'g' => result.global = true,
                b'i' => result.ignore_case = true,
                b'm' => result.multiline = true,
                b's' => result.dot_all = true,
                b'u' => result.unicode = true,
                _ => return Err(()),
            }
        }
        Ok(result)
    }

    fn normalized(self) -> String {
        let mut result = String::from("d");
        for (flag, present) in [
            ('g', self.global),
            ('i', self.ignore_case),
            ('m', self.multiline),
            ('s', self.dot_all),
            ('u', self.unicode),
        ] {
            if present {
                result.push(flag);
            }
        }
        result
    }

    fn inline(self) -> String {
        let mut result = String::new();
        if self.ignore_case {
            result.push_str("(?i)");
        }
        if self.multiline {
            result.push_str("(?m)");
        }
        if self.dot_all {
            result.push_str("(?s)");
        }
        result
    }
}

fn hex_utf16(text: &str) -> String {
    let mut result = String::with_capacity(text.len() * 2);
    for unit in text.encode_utf16() {
        use std::fmt::Write;
        let _ = write!(result, "{unit:04x}");
    }
    result
}

fn translate_class(chars: &[char], index: &mut usize) -> Result<String, ()> {
    let mut result = String::from("[");
    *index += 1;
    if *index < chars.len() && chars[*index] == '^' {
        result.push('^');
        *index += 1;
    }
    if *index < chars.len() && chars[*index] == ']' {
        result.push_str("^\\s\\S");
        *index += 1;
    }
    while *index < chars.len() && chars[*index] != ']' {
        if chars[*index] == '\\' {
            *index += 1;
            if *index >= chars.len() {
                return Err(());
            }
            let escape = chars[*index];
            match escape {
                'd' => result.push_str("0-9"),
                'w' => result.push_str("A-Za-z0-9_"),
                's' => result.push_str(JS_SPACE_INNER),
                'D' | 'W' | 'S' => return Err(()),
                'n' => result.push_str("\\n"),
                'r' => result.push_str("\\r"),
                't' => result.push_str("\\t"),
                'f' => result.push_str("\\f"),
                'v' => result.push_str("\\x0b"),
                '0' => result.push_str("\\x00"),
                '1'..='9' => return Err(()),
                'x' => {
                    if *index + 2 >= chars.len() {
                        return Err(());
                    }
                    let value: String = chars[*index + 1..*index + 3].iter().collect();
                    if !value.chars().all(|c| c.is_ascii_hexdigit()) {
                        return Err(());
                    }
                    result.push_str("\\x");
                    result.push_str(&value);
                    *index += 2;
                }
                'u' => {
                    if *index + 4 >= chars.len() {
                        return Err(());
                    }
                    let value: String = chars[*index + 1..*index + 5].iter().collect();
                    if !value.chars().all(|c| c.is_ascii_hexdigit()) {
                        return Err(());
                    }
                    let code = u32::from_str_radix(&value, 16).map_err(|_| ())?;
                    result.push_str(&format!("\\u{{{code:x}}}"));
                    *index += 4;
                }
                '\\' | '^' | '-' | ']' | '[' => {
                    result.push('\\');
                    result.push(escape);
                }
                other if other.is_ascii_alphanumeric() => return Err(()),
                other => result.push(other),
            }
            *index += 1;
        } else {
            result.push(chars[*index]);
            *index += 1;
        }
    }
    if *index >= chars.len() {
        return Err(());
    }
    result.push(']');
    *index += 1;
    Ok(result)
}

fn translate(pattern: &str, flags: Flags) -> Result<String, ()> {
    let mut result = flags.inline();
    let chars: Vec<char> = pattern.chars().collect();
    let mut index = 0;
    while index < chars.len() {
        match chars[index] {
            '\\' => {
                index += 1;
                if index >= chars.len() {
                    return Err(());
                }
                let escape = chars[index];
                match escape {
                    'd' => result.push_str("[0-9]"),
                    'D' => result.push_str("[^0-9]"),
                    'w' => result.push_str("[A-Za-z0-9_]"),
                    'W' => result.push_str("[^A-Za-z0-9_]"),
                    's' => {
                        result.push('[');
                        result.push_str(JS_SPACE_INNER);
                        result.push(']');
                    }
                    'S' => {
                        result.push_str("[^");
                        result.push_str(JS_SPACE_INNER);
                        result.push(']');
                    }
                    'n' => result.push_str("\\n"),
                    'r' => result.push_str("\\r"),
                    't' => result.push_str("\\t"),
                    'f' => result.push_str("\\f"),
                    'v' => result.push_str("\\x0b"),
                    '0' => {
                        if index + 1 < chars.len() && chars[index + 1].is_ascii_digit() {
                            return Err(());
                        }
                        result.push_str("\\x00");
                    }
                    '1'..='9' | 'b' | 'B' | 'k' | 'p' | 'P' => return Err(()),
                    'x' => {
                        if index + 2 >= chars.len() {
                            return Err(());
                        }
                        let value: String = chars[index + 1..index + 3].iter().collect();
                        if !value.chars().all(|c| c.is_ascii_hexdigit()) {
                            return Err(());
                        }
                        result.push_str("\\x");
                        result.push_str(&value);
                        index += 2;
                    }
                    'u' => {
                        if flags.unicode && index + 1 < chars.len() && chars[index + 1] == '{' {
                            return Err(());
                        }
                        if index + 4 >= chars.len() {
                            return Err(());
                        }
                        let value: String = chars[index + 1..index + 5].iter().collect();
                        if !value.chars().all(|c| c.is_ascii_hexdigit()) {
                            return Err(());
                        }
                        let code = u32::from_str_radix(&value, 16).map_err(|_| ())?;
                        result.push_str(&format!("\\u{{{code:x}}}"));
                        index += 4;
                    }
                    '/' => result.push('/'),
                    '\\' | '.' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' | '|' | '-' => {
                        result.push('\\');
                        result.push(escape);
                    }
                    other if other.is_ascii_alphanumeric() => return Err(()),
                    other => result.push(other),
                }
                index += 1;
            }
            '[' => result.push_str(&translate_class(&chars, &mut index)?),
            '(' if index + 2 < chars.len() && chars[index + 1] == '?' => {
                if chars[index + 2] == '=' || chars[index + 2] == '!'
                    || (chars[index + 2] == '<'
                        && index + 3 < chars.len()
                        && (chars[index + 3] == '=' || chars[index + 3] == '!'))
                {
                    return Err(());
                }
                result.push('(');
                index += 1;
            }
            other => {
                result.push(other);
                index += 1;
            }
        }
    }
    Ok(result)
}

fn utf16_offset(text: &str, byte_offset: usize) -> usize {
    let mut bytes = 0;
    let mut units = 0;
    for character in text.chars() {
        if bytes >= byte_offset {
            break;
        }
        bytes += character.len_utf8();
        units += character.len_utf16();
    }
    units
}

fn byte_offset(text: &str, target_units: usize) -> usize {
    let mut bytes = 0;
    let mut units = 0;
    for character in text.chars() {
        if units >= target_units {
            break;
        }
        bytes += character.len_utf8();
        units += character.len_utf16();
    }
    bytes
}

fn invalid_quantifiers(pattern: &str) -> bool {
    let chars: Vec<char> = pattern.chars().collect();
    let mut index = 0;
    let mut in_class = false;
    let mut previous_quantifier = false;
    let mut lazy = false;
    while index < chars.len() {
        match chars[index] {
            '\\' => {
                index += 2;
                previous_quantifier = false;
                lazy = false;
                continue;
            }
            '[' => {
                in_class = true;
                previous_quantifier = false;
                lazy = false;
            }
            ']' => {
                in_class = false;
                previous_quantifier = false;
                lazy = false;
            }
            '*' | '+' | '{' if !in_class => {
                if previous_quantifier {
                    return true;
                }
                previous_quantifier = true;
                lazy = false;
            }
            '?' if !in_class => {
                if previous_quantifier {
                    if lazy {
                        return true;
                    }
                    lazy = true;
                } else {
                    previous_quantifier = true;
                    lazy = false;
                }
            }
            _ if !in_class => {
                previous_quantifier = false;
                lazy = false;
            }
            _ => {}
        }
        index += 1;
    }
    false
}

fn quantified_assertion(pattern: &str) -> bool {
    let chars: Vec<char> = pattern.chars().collect();
    let mut index = 0;
    let mut previous_assertion = false;
    while index < chars.len() {
        match chars[index] {
            '\\' => {
                index += 2;
                previous_assertion = false;
                continue;
            }
            '[' => {
                while index < chars.len() && chars[index] != ']' {
                    index += if chars[index] == '\\' { 2 } else { 1 };
                }
                previous_assertion = false;
            }
            '^' | '$' => previous_assertion = true,
            '*' | '+' | '?' | '{' if previous_assertion => return true,
            _ => previous_assertion = false,
        }
        index += 1;
    }
    false
}

fn answer(pattern: &str, flags_text: &str, subject: &str, iterations: u16, start: u16) -> String {
    let flags = match Flags::parse(flags_text) {
        Ok(flags) => flags,
        Err(()) => return String::from("E"),
    };
    if invalid_quantifiers(pattern) || (flags.unicode && quantified_assertion(pattern)) {
        return String::from("E");
    }
    let translated = match translate(pattern, flags) {
        Ok(value) => value,
        Err(()) => return String::from("E"),
    };
    let regex = match RegexBuilder::new(&translated).build() {
        Ok(regex) => regex,
        Err(_) => return String::from("E"),
    };
    let mut records = Vec::with_capacity(iterations as usize);
    let mut last_index = start as usize;
    for _ in 0..iterations {
        let from = if flags.global {
            byte_offset(subject, last_index)
        } else {
            0
        };
        match regex.find_at(subject, from) {
            None => {
                if flags.global {
                    last_index = 0;
                }
                records.push(format!("N,{last_index}"));
            }
            Some(found) => {
                let start_units = utf16_offset(subject, found.start());
                let end_units = utf16_offset(subject, found.end());
                if flags.global {
                    last_index = end_units;
                }
                let captures = match regex.captures_at(subject, from) {
                    Some(captures) => captures,
                    None => return String::from("X"),
                };
                let mut groups = String::new();
                for group in 1..captures.len() {
                    if group > 1 {
                        groups.push(';');
                    }
                    match captures.get(group) {
                        None => groups.push('U'),
                        Some(value) => {
                            let group_start = utf16_offset(subject, value.start());
                            let group_end = utf16_offset(subject, value.end());
                            groups.push_str(&format!(
                                "{group_start},{group_end},{}",
                                hex_utf16(&subject[value.start()..value.end()])
                            ));
                        }
                    }
                }
                records.push(format!(
                    "M,{start_units},{end_units},{},{groups},{last_index}",
                    hex_utf16(&subject[found.start()..found.end()])
                ));
            }
        }
    }
    format!("O{}|{}", flags.normalized(), records.join("|"))
}

fn field<'a>(input: &'a [u8], offset: &mut usize) -> Result<&'a str, ()> {
    if *offset + 2 > input.len() {
        return Err(());
    }
    let length = (input[*offset] as usize) << 8 | input[*offset + 1] as usize;
    *offset += 2;
    if *offset + length > input.len() {
        return Err(());
    }
    let value = std::str::from_utf8(&input[*offset..*offset + length]).map_err(|_| ())?;
    *offset += length;
    Ok(value)
}

fn unit(input: &[u8], offset: &mut usize) -> Result<u16, ()> {
    if *offset + 2 > input.len() {
        return Err(());
    }
    let value = (input[*offset] as u16) << 8 | input[*offset + 1] as u16;
    *offset += 2;
    Ok(value)
}

fn main() -> io::Result<()> {
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut stdout = io::stdout().lock();
    let mut magic = [0u8; 4];
    input.read_exact(&mut magic)?;
    if &magic != b"MGB1" {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "bad protocol header"));
    }
    stdout.write_all(b"MGB1")?;
    stdout.flush()?;
    loop {
        let mut header = [0u8; 4];
        if input.read_exact(&mut header).is_err() {
            return Ok(());
        }
        let length = u32::from_be_bytes(header) as usize;
        if length == 0 || length > 65536 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "bad record length"));
        }
        let mut record = vec![0u8; length];
        input.read_exact(&mut record)?;
        let reply = if record[0] != 1 {
            String::from("E")
        } else {
            let mut offset = 1;
            let pattern = field(&record, &mut offset).unwrap_or("");
            let flags = field(&record, &mut offset).unwrap_or("");
            let subject = field(&record, &mut offset).unwrap_or("");
            let iterations = unit(&record, &mut offset).unwrap_or(0);
            let start = unit(&record, &mut offset).unwrap_or(0);
            if offset != record.len() || iterations < 1 || iterations > 8 {
                String::from("E")
            } else {
                answer(pattern, flags, subject, iterations, start)
            }
        };
        stdout.write_all(&(reply.len() as u32).to_be_bytes())?;
        stdout.write_all(reply.as_bytes())?;
        stdout.flush()?;
    }
}

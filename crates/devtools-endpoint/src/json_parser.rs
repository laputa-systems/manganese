use crate::cbor::Encoder;
use crate::status::Error;
use crate::status::Status;

const STACK_LIMIT: usize = 300;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Token {
    ObjectBegin,
    ObjectEnd,
    ArrayBegin,
    ArrayEnd,
    StringLiteral,
    Number,
    True,
    False,
    Null,
    ListSeparator,
    ObjectPairSeparator,
    Invalid,
    NoInput,
}

struct Scanned {
    token: Token,
    start: usize,
    end: usize,
}

/// Port of crdtp's JSON parser. Its leniencies and error positions are observable in DevTools error replies.
struct Parser<'a> {
    input: &'a [u8],
    encoder: Encoder,
    error: Option<Status>,
}

fn is_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\n' | 0x0b | 0x0c | b'\r' | b'\t')
}

impl Parser<'_> {
    fn skip_whitespace(&self, mut start: usize) -> usize {
        while start < self.input.len() && is_space(self.input[start]) {
            start += 1;
        }
        start
    }

    fn const_token(&self, start: usize, token: &[u8]) -> Option<usize> {
        let (mut position, mut index) = (start, 0);
        while position < self.input.len() && index < token.len() {
            let equal = self.input[position] == token[index];
            position += 1;
            index += 1;
            if !equal {
                break;
            }
        }
        (index == token.len()).then_some(position)
    }

    fn read_int(&self, start: usize, allow_leading_zeros: bool) -> Option<usize> {
        if start == self.input.len() {
            return None;
        }
        let leading_zero = self.input[start] == b'0';
        let mut position = start;
        while position < self.input.len() && self.input[position].is_ascii_digit() {
            position += 1;
        }
        let length = position - start;
        if length == 0 || (!allow_leading_zeros && length > 1 && leading_zero) {
            return None;
        }
        Some(position)
    }

    fn number_token(&self, mut start: usize) -> Option<usize> {
        if start == self.input.len() {
            return None;
        }
        if self.input[start] == b'-' {
            start += 1;
        }
        start = self.read_int(start, false)?;
        if start == self.input.len() {
            return Some(start);
        }
        let mut byte = self.input[start];
        if byte == b'.' {
            start = self.read_int(start + 1, true)?;
            if start == self.input.len() {
                return Some(start);
            }
            byte = self.input[start];
        }
        if byte == b'e' || byte == b'E' {
            start += 1;
            if start == self.input.len() {
                return None;
            }
            if matches!(self.input[start], b'-' | b'+') {
                start += 1;
                if start == self.input.len() {
                    return None;
                }
            }
            start = self.read_int(start, true)?;
        }
        Some(start)
    }

    fn hex_digits(&self, start: usize, count: usize) -> Option<usize> {
        if self.input.len() - start.min(self.input.len()) < count {
            return None;
        }
        self.input[start..start + count].iter().all(u8::is_ascii_hexdigit).then_some(start + count)
    }

    fn string_token(&self, mut start: usize) -> Option<usize> {
        while start < self.input.len() {
            let byte = self.input[start];
            start += 1;
            if byte == b'\\' {
                let escaped = *self.input.get(start)?;
                start += 1;
                match escaped {
                    b'x' => start = self.hex_digits(start, 2)?,
                    b'u' => start = self.hex_digits(start, 4)?,
                    b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' | b'v' | b'"' => {}
                    _ => return None,
                }
            } else if byte == b'"' {
                return Some(start);
            }
        }
        None
    }

    fn scan(&self, from: usize) -> Scanned {
        let start = self.skip_whitespace(from);
        let single = |token| Scanned { token, start, end: start + 1 };
        let Some(&byte) = self.input.get(start) else {
            return Scanned { token: Token::NoInput, start, end: start };
        };
        let found = |token, end: Option<usize>| match end {
            Some(end) => Scanned { token, start, end },
            None => Scanned { token: Token::Invalid, start, end: start },
        };
        match byte {
            b'n' => found(Token::Null, self.const_token(start, b"null")),
            b't' => found(Token::True, self.const_token(start, b"true")),
            b'f' => found(Token::False, self.const_token(start, b"false")),
            b'[' => single(Token::ArrayBegin),
            b']' => single(Token::ArrayEnd),
            b',' => single(Token::ListSeparator),
            b'{' => single(Token::ObjectBegin),
            b'}' => single(Token::ObjectEnd),
            b':' => single(Token::ObjectPairSeparator),
            b'0'..=b'9' | b'-' => found(Token::Number, self.number_token(start)),
            b'"' => found(Token::StringLiteral, self.string_token(start + 1)),
            _ => Scanned { token: Token::Invalid, start, end: start },
        }
    }

    fn decode_string(&self, mut start: usize, end: usize) -> Option<Vec<u16>> {
        let mut out = Vec::new();
        while start < end {
            let mut unit = u32::from(self.input[start]);
            start += 1;
            if unit > 0x7f {
                let (mut left, mut codepoint) = if unit & 0xe0 == 0xc0 {
                    (1, unit & 0x1f)
                } else if unit & 0xf0 == 0xe0 {
                    (2, unit & 0x0f)
                } else if unit & 0xf8 == 0xf0 {
                    (3, unit & 0x07)
                } else {
                    return None;
                };
                if start + left > end {
                    return None;
                }
                while left > 0 {
                    let next = u32::from(self.input[start]);
                    start += 1;
                    left -= 1;
                    if next & 0xc0 != 0x80 {
                        return None;
                    }
                    codepoint = codepoint << 6 | (next & 0x3f);
                }
                if codepoint <= 0x7f || codepoint > 0x10ffff {
                    return None;
                }
                if codepoint <= 0xffff {
                    out.push(codepoint as u16);
                } else {
                    codepoint -= 0x10000;
                    out.push((codepoint >> 10) as u16 + 0xd800);
                    out.push((codepoint & 0x3ff) as u16 + 0xdc00);
                }
                continue;
            }
            if unit != u32::from(b'\\') {
                out.push(unit as u16);
                continue;
            }
            if start == end {
                return None;
            }
            unit = u32::from(self.input[start]);
            start += 1;
            let decoded = match unit as u8 {
                b'x' => return None,
                b'"' | b'/' | b'\\' => unit,
                b'b' => 8,
                b'f' => 12,
                b'n' => 10,
                b'r' => 13,
                b't' => 9,
                b'v' => 11,
                b'u' => {
                    let digits = std::str::from_utf8(&self.input[start..start + 4]).ok()?;
                    start += 4;
                    u32::from_str_radix(digits, 16).ok()?
                }
                _ => return None,
            };
            out.push(decoded as u16);
        }
        Some(out)
    }

    fn fail(&mut self, error: Error, position: usize) {
        if self.error.is_none() {
            self.error = Some(Status { error, position });
        }
    }

    fn parse_value(&mut self, start: usize, depth: usize) -> usize {
        if depth > STACK_LIMIT {
            self.fail(Error::JsonStackLimitExceeded, start);
            return start;
        }
        let Scanned { token, start: token_start, end: mut token_end } = self.scan(start);
        match token {
            Token::NoInput => {
                self.fail(Error::JsonNoInput, token_start);
                return token_start;
            }
            Token::Invalid => {
                self.fail(Error::JsonInvalidToken, token_start);
                return token_start;
            }
            Token::Null => self.encoder.null(),
            Token::True => self.encoder.boolean(true),
            Token::False => self.encoder.boolean(false),
            Token::Number => {
                let text = std::str::from_utf8(&self.input[token_start..token_end]).expect("number tokens are ASCII");
                match text.parse::<f64>() {
                    Ok(value) if value.is_finite() => {
                        if value >= f64::from(i32::MIN) && value <= f64::from(i32::MAX) && f64::from(value as i32) == value {
                            self.encoder.int32(value as i32);
                        } else {
                            self.encoder.double(value);
                        }
                    }
                    _ => {
                        self.fail(Error::JsonInvalidNumber, token_start);
                        return token_start;
                    }
                }
            }
            Token::StringLiteral => match self.decode_string(token_start + 1, token_end - 1) {
                Some(units) => self.encoder.string16(&units),
                None => {
                    self.fail(Error::JsonInvalidString, token_start);
                    return token_start;
                }
            },
            Token::ArrayBegin => {
                self.encoder.array_begin();
                let mut position = token_end;
                let mut next = self.scan(position);
                while next.token != Token::ArrayEnd {
                    let value_end = self.parse_value(position, depth + 1);
                    if self.error.is_some() {
                        return value_end;
                    }
                    position = value_end;
                    next = self.scan(position);
                    if next.token == Token::ListSeparator {
                        position = next.end;
                        next = self.scan(position);
                        if next.token == Token::ArrayEnd {
                            self.fail(Error::JsonUnexpectedArrayEnd, next.start);
                            return next.start;
                        }
                    } else if next.token != Token::ArrayEnd {
                        self.fail(Error::JsonCommaOrArrayEndExpected, next.start);
                        return next.start;
                    }
                }
                self.encoder.array_end();
                token_end = next.end;
            }
            Token::ObjectBegin => {
                self.encoder.map_begin();
                let mut position = token_end;
                let mut next = self.scan(position);
                while next.token != Token::ObjectEnd {
                    if next.token != Token::StringLiteral {
                        self.fail(Error::JsonStringLiteralExpected, next.start);
                        return next.start;
                    }
                    let Some(key) = self.decode_string(next.start + 1, next.end - 1) else {
                        self.fail(Error::JsonInvalidString, next.start);
                        return next.start;
                    };
                    self.encoder.string16(&key);
                    position = next.end;
                    next = self.scan(position);
                    if next.token != Token::ObjectPairSeparator {
                        self.fail(Error::JsonColonExpected, next.start);
                        return next.start;
                    }
                    position = next.end;
                    let value_end = self.parse_value(position, depth + 1);
                    if self.error.is_some() {
                        return value_end;
                    }
                    position = value_end;
                    next = self.scan(position);
                    if next.token == Token::ListSeparator {
                        position = next.end;
                        next = self.scan(position);
                        if next.token == Token::ObjectEnd {
                            self.fail(Error::JsonUnexpectedMapEnd, next.start);
                            return next.start;
                        }
                    } else if next.token != Token::ObjectEnd {
                        self.fail(Error::JsonCommaOrMapEndExpected, next.start);
                        return next.start;
                    }
                }
                self.encoder.map_end();
                token_end = next.end;
            }
            _ => {
                self.fail(Error::JsonValueExpected, token_start);
                return token_start;
            }
        }
        self.skip_whitespace(token_end)
    }
}

/// Converts a JSON message to the CBOR crdtp dispatches on, or reports where the JSON went wrong.
pub fn to_cbor(input: &[u8]) -> Result<Vec<u8>, Status> {
    let mut parser = Parser { input, encoder: Encoder::default(), error: None };
    let end = parser.parse_value(0, 0);
    if let Some(status) = parser.error {
        return Err(status);
    }
    if end != input.len() {
        return Err(Status { error: Error::JsonUnprocessedInputRemains, position: end });
    }
    Ok(parser.encoder.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error(input: &[u8]) -> String {
        to_cbor(input).expect_err("invalid JSON").describe()
    }

    #[test]
    fn errors_name_the_token_and_the_byte_offset() {
        assert_eq!(error(b""), "JSON: no input at position 0");
        assert_eq!(error(b"{not json"), "JSON: string literal expected at position 1");
        assert_eq!(error(b"{\"a\" 1}"), "JSON: colon expected at position 5");
        assert_eq!(error(b"[1,]"), "JSON: unexpected array end at position 3");
        assert_eq!(error(b"{\"a\":1,}"), "JSON: unexpected map end at position 7");
        assert_eq!(error(b"[1 2]"), "JSON: comma or array end expected at position 3");
        assert_eq!(error(b"{} x"), "JSON: unprocessed input remains at position 3");
        assert_eq!(error(b"\"\\x41\""), "JSON: invalid string at position 0");
        assert_eq!(error(b"\"\xff\""), "JSON: invalid string at position 0");
        assert_eq!(to_cbor(b"nulx").expect("crdtp accepts a one-byte typo in a literal"), [0xf6]);
    }

    #[test]
    fn objects_are_wrapped_in_envelopes() {
        assert_eq!(to_cbor(b"{}").expect("valid JSON"), [0xd8, 0x18, 0x5a, 0, 0, 0, 2, 0xbf, 0xff]);
    }
}

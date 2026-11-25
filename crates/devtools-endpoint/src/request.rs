use std::collections::HashMap;

#[derive(Clone, Debug, Default)]
pub struct Request {
    method: Vec<u8>,
    path: Vec<u8>,
    headers: HashMap<Vec<u8>, Vec<u8>>,
    body: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Parsed {
    Complete { consumed: usize },
    Incomplete,
    Invalid,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Method,
    Url,
    Proto,
    Header,
    Name,
    Separator,
    Value,
    Done,
}

#[derive(Clone, Copy)]
enum Input {
    Lws,
    Cr,
    Lf,
    Colon,
    Other,
}

fn input_of(byte: u8) -> Input {
    match byte {
        b' ' | b'\t' => Input::Lws,
        b'\r' => Input::Cr,
        b'\n' => Input::Lf,
        b':' => Input::Colon,
        _ => Input::Other,
    }
}

fn next_state(state: State, input: Input) -> Option<State> {
    use Input::{Colon, Cr, Lf, Lws, Other};
    use State::{Done, Header, Method, Name, Proto, Separator, Url, Value};
    match (state, input) {
        (Method, Lws) => Some(Url),
        (Method, Other) => Some(Method),
        (Url, Lws) => Some(Proto),
        (Url, Colon | Other) => Some(Url),
        (Proto, Cr) => Some(Header),
        (Proto, Lf) => Some(Name),
        (Proto, Other) => Some(Proto),
        (Header, Lf) => Some(Name),
        (Name, Lws) => Some(Separator),
        (Name, Cr) => Some(Done),
        (Name, Colon) => Some(Value),
        (Name, Other) => Some(Name),
        (Separator, Lws) => Some(Separator),
        (Separator, Colon) => Some(Value),
        (Value, Lws | Colon | Other) => Some(Value),
        (Value, Cr) => Some(Header),
        (Value, Lf) => Some(Name),
        (Done, Lf) => Some(Done),
        _ => None,
    }
}

fn trim_leading_whitespace(token: &[u8]) -> &[u8] {
    let start = token.iter().position(|byte| !matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)).unwrap_or(token.len());
    &token[start..]
}

impl Request {
    pub fn new(method: &str, path: &str) -> Self {
        Self { method: method.as_bytes().to_vec(), path: path.as_bytes().to_vec(), ..Self::default() }
    }

    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.insert(name.as_bytes().to_vec(), value.as_bytes().to_vec());
        self
    }

    pub fn method(&self) -> &[u8] {
        &self.method
    }

    pub fn path(&self) -> &[u8] {
        &self.path
    }

    pub fn body(&self) -> &[u8] {
        &self.body
    }

    pub fn header(&self, name: &str) -> &[u8] {
        self.headers.get(name.as_bytes()).map_or(&[], Vec::as_slice)
    }

    pub fn has_header(&self, name: &str) -> bool {
        self.headers.contains_key(name.as_bytes())
    }

    pub fn has_header_token(&self, name: &str, token: &str) -> bool {
        let value = self.header(name).to_ascii_lowercase();
        value
            .split(|byte| *byte == b',')
            .filter(|part| !part.is_empty())
            .any(|part| trim_blanks(part) == token.as_bytes())
    }

    pub fn set_body(&mut self, body: Vec<u8>) {
        self.body = body;
    }

    /// Port of net::HttpServer::ParseHeaders, whose request grammar is looser than HTTP/1.1's in ways clients observe.
    pub fn parse(data: &[u8]) -> (Parsed, Request) {
        let mut request = Request::default();
        let mut state = State::Method;
        let mut token_start = 0;
        let mut header_name = Vec::new();
        let mut position = 0;
        while position < data.len() {
            let byte = data[position];
            if byte == 0 {
                return (Parsed::Invalid, request);
            }
            let Some(next) = next_state(state, input_of(byte)) else {
                return (Parsed::Invalid, request);
            };
            if next != state {
                let token = &data[token_start..position];
                token_start = position + 1;
                match state {
                    State::Method => request.method = token.to_vec(),
                    State::Url => request.path = token.to_vec(),
                    State::Proto => {
                        if token != b"HTTP/1.1" {
                            return (Parsed::Invalid, request);
                        }
                    }
                    State::Name => header_name = token.to_ascii_lowercase(),
                    State::Value => {
                        let value = trim_leading_whitespace(token);
                        match request.headers.get_mut(&header_name) {
                            Some(existing) => {
                                existing.push(b',');
                                existing.extend_from_slice(value);
                            }
                            None => {
                                request.headers.insert(std::mem::take(&mut header_name), value.to_vec());
                            }
                        }
                        header_name.clear();
                    }
                    State::Header | State::Separator | State::Done => {}
                }
                state = next;
            } else if state == State::Done {
                return (Parsed::Complete { consumed: position + 1 }, request);
            }
            position += 1;
        }
        (Parsed::Incomplete, request)
    }
}

fn trim_blanks(bytes: &[u8]) -> &[u8] {
    let start = bytes.iter().position(|byte| !matches!(byte, b' ' | b'\t')).unwrap_or(bytes.len());
    let end = bytes.iter().rposition(|byte| !matches!(byte, b' ' | b'\t')).map_or(start, |end| end + 1);
    &bytes[start..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &[u8]) -> (Parsed, Request) {
        Request::parse(text)
    }

    #[test]
    fn a_well_formed_request_is_consumed_to_its_blank_line() {
        let (parsed, request) = parse(b"GET /json/version HTTP/1.1\r\nHost: 127.0.0.1:9221\r\nX-A:b\r\n\r\nrest");
        assert_eq!(parsed, Parsed::Complete { consumed: 59 });
        assert_eq!(request.method(), b"GET");
        assert_eq!(request.path(), b"/json/version");
        assert_eq!(request.header("host"), b"127.0.0.1:9221");
        assert_eq!(request.header("x-a"), b"b");
    }

    #[test]
    fn duplicate_headers_are_joined_with_commas() {
        let (_, request) = parse(b"GET / HTTP/1.1\r\nHost: a\r\nHost: b\r\n\r\n");
        assert_eq!(request.header("host"), b"a,b");
    }

    #[test]
    fn partial_and_invalid_requests_are_told_apart() {
        assert_eq!(parse(b"GET /json/version HTTP/1.1\r\nHost: x\r\n").0, Parsed::Incomplete);
        for bad in [&b"garbage\r\n\r\n"[..], b"GET /x\r\n\r\n", b"\r\n\r\n", b"GET / HTTP/1.1\n\n", b"GET / HTTP/1.0\r\n\r\n", b"GET / HTTP/1.1\r\nHost 1\r\n\r\n", b"GET\0 / HTTP/1.1\r\n\r\n"] {
            assert_eq!(parse(bad).0, Parsed::Invalid, "{bad:?}");
        }
    }
}

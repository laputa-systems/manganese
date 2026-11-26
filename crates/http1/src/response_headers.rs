use crate::http_util;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct HttpVersion {
    pub major: u16,
    pub minor: u16,
}

impl HttpVersion {
    pub const fn new(major: u16, minor: u16) -> Self {
        HttpVersion { major, minor }
    }
}

#[derive(Clone, Copy, Debug)]
struct ParsedHeader {
    name_begin: usize,
    name_end: usize,
    value_begin: usize,
    value_end: usize,
}

impl ParsedHeader {
    fn is_continuation(&self) -> bool {
        self.name_begin == self.name_end
    }
}

#[derive(Clone, Debug)]
pub struct HttpResponseHeaders {
    raw: Vec<u8>,
    parsed: Vec<ParsedHeader>,
    response_code: i32,
    version: HttpVersion,
}

impl HttpResponseHeaders {
    pub fn try_to_create(headers: &[u8]) -> Option<Self> {
        if headers.contains(&0) || headers.len() > i32::MAX as usize {
            return None;
        }
        Some(Self::parse(&http_util::assemble_raw_headers(headers)))
    }

    pub fn from_status_line(line: &[u8]) -> Self {
        Self::parse(line)
    }

    fn parse(input: &[u8]) -> Self {
        let mut headers = HttpResponseHeaders { raw: Vec::with_capacity(input.len()), parsed: Vec::new(), response_code: 0, version: HttpVersion::new(1, 0) };
        let line_end = input.iter().position(|&byte| byte == 0);
        let has_headers = matches!(line_end, Some(end) if end + 1 != input.len() && input[end + 1] != 0);
        headers.parse_status_line(&input[..line_end.unwrap_or(input.len())], has_headers);
        headers.raw.push(0);
        let Some(line_end) = line_end else {
            headers.raw.push(0);
            return headers;
        };
        let status_line_len = headers.raw.len();
        headers.raw.extend_from_slice(&input[line_end + 1..]);
        while headers.raw.len() < 2 || headers.raw[headers.raw.len() - 2] != 0 || headers.raw[headers.raw.len() - 1] != 0 {
            headers.raw.push(0);
        }
        for line in http_util::header_lines(&headers.raw[status_line_len..]) {
            headers.add_header(
                status_line_len + line.name.0,
                status_line_len + line.name.1,
                status_line_len + line.values.0,
                status_line_len + line.values.1,
            );
        }
        headers
    }

    fn add_header(&mut self, name_begin: usize, name_end: usize, values_begin: usize, values_end: usize) {
        if values_begin == values_end || http_util::is_non_coalescing_header(&self.raw[name_begin..name_end]) {
            self.parsed.push(ParsedHeader { name_begin, name_end, value_begin: values_begin, value_end: values_end });
            return;
        }
        let (mut name_begin, mut name_end) = (name_begin, name_end);
        for (begin, end) in http_util::quoted_values(&self.raw[values_begin..values_end], b',') {
            self.parsed.push(ParsedHeader { name_begin, name_end, value_begin: values_begin + begin, value_end: values_begin + end });
            name_begin = values_end;
            name_end = values_end;
        }
    }

    fn parse_version(line: &[u8]) -> HttpVersion {
        if line.len() < 4 || !line[..4].eq_ignore_ascii_case(b"http") {
            return HttpVersion::new(0, 0);
        }
        let p = 4;
        if p >= line.len() || line[p] != b'/' {
            return HttpVersion::new(0, 0);
        }
        let Some(dot) = line[p..].iter().position(|&byte| byte == b'.').map(|offset| p + offset) else {
            return HttpVersion::new(0, 0);
        };
        if dot + 1 == line.len() {
            return HttpVersion::new(0, 0);
        }
        let (major, minor) = (line[p + 1], line[dot + 1]);
        if !major.is_ascii_digit() || !minor.is_ascii_digit() {
            return HttpVersion::new(0, 0);
        }
        HttpVersion::new(u16::from(major - b'0'), u16::from(minor - b'0'))
    }

    fn parse_status_line(&mut self, line: &[u8], has_headers: bool) {
        let parsed = Self::parse_version(line);
        if parsed == HttpVersion::new(0, 9) && !has_headers {
            self.version = HttpVersion::new(0, 9);
            self.raw = b"HTTP/0.9".to_vec();
        } else if parsed == HttpVersion::new(2, 0) {
            self.version = HttpVersion::new(2, 0);
            self.raw = b"HTTP/2.0".to_vec();
        } else if parsed >= HttpVersion::new(1, 1) {
            self.version = HttpVersion::new(1, 1);
            self.raw = b"HTTP/1.1".to_vec();
        } else {
            self.version = HttpVersion::new(1, 0);
            self.raw = b"HTTP/1.0".to_vec();
        }
        let Some(space) = line.iter().position(|&byte| byte == b' ') else {
            self.raw.extend_from_slice(b" 200 OK");
            self.response_code = 200;
            return;
        };
        self.response_code = Self::parse_status(&line[space + 1..], &mut self.raw);
    }

    fn parse_status(status: &[u8], append_to: &mut Vec<u8>) -> i32 {
        let skip = status.iter().position(|&byte| byte != b' ').unwrap_or(status.len());
        let status = &status[skip..];
        let digits = status.iter().position(|byte| !byte.is_ascii_digit()).unwrap_or(status.len());
        if digits == 0 {
            append_to.extend_from_slice(b" 200");
            return 200;
        }
        append_to.push(b' ');
        append_to.extend_from_slice(&status[..digits]);
        let code = http_util::string_to_number(&status[..digits], 10, i64::from(i32::MAX), i64::from(i32::MIN)).value;
        let rest = &status[digits..];
        let skip = rest.iter().position(|&byte| byte != b' ').unwrap_or(rest.len());
        let rest = &rest[skip..];
        let end = rest.iter().rposition(|&byte| byte != b' ').map_or(0, |last| last + 1);
        let text = &rest[..end];
        if !text.is_empty() {
            append_to.push(b' ');
            append_to.extend_from_slice(text);
        }
        i32::try_from(code).expect("status digits parse within i32")
    }

    pub fn response_code(&self) -> i32 {
        self.response_code
    }

    pub fn version(&self) -> HttpVersion {
        self.version
    }

    pub fn status_line(&self) -> &[u8] {
        let end = self.raw.iter().position(|&byte| byte == 0).unwrap_or(self.raw.len());
        &self.raw[..end]
    }

    fn name(&self, header: &ParsedHeader) -> &[u8] {
        &self.raw[header.name_begin..header.name_end]
    }

    fn value(&self, header: &ParsedHeader) -> &[u8] {
        &self.raw[header.value_begin..header.value_end]
    }

    fn find_header(&self, from: usize, name: &[u8]) -> Option<usize> {
        (from..self.parsed.len()).find(|&i| !self.parsed[i].is_continuation() && self.name(&self.parsed[i]).eq_ignore_ascii_case(name))
    }

    pub fn header_lines(&self) -> Vec<(&[u8], &[u8])> {
        let mut lines = Vec::new();
        let mut i = 0;
        while i < self.parsed.len() {
            let first = self.parsed[i];
            let mut value_end = first.value_end;
            i += 1;
            while i < self.parsed.len() && self.parsed[i].is_continuation() {
                value_end = self.parsed[i].value_end;
                i += 1;
            }
            lines.push((self.name(&first), &self.raw[first.value_begin..value_end]));
        }
        lines
    }

    pub fn enumerate_header<'a>(&'a self, name: &'a [u8]) -> impl Iterator<Item = &'a [u8]> + 'a {
        let mut next = self.find_header(0, name);
        std::iter::from_fn(move || {
            let i = next?;
            next = if i + 1 >= self.parsed.len() {
                None
            } else if self.parsed[i + 1].is_continuation() {
                Some(i + 1)
            } else {
                self.find_header(i + 1, name)
            };
            Some(self.value(&self.parsed[i]))
        })
    }

    pub fn normalized_header(&self, name: &[u8]) -> Option<Vec<u8>> {
        let mut value: Option<Vec<u8>> = None;
        let mut i = 0;
        while i < self.parsed.len() {
            let Some(found) = self.find_header(i, name) else { break };
            let joined = match &mut value {
                Some(joined) => {
                    joined.extend_from_slice(b", ");
                    joined
                }
                None => value.insert(Vec::new()),
            };
            let begin = self.parsed[found].value_begin;
            let mut end = self.parsed[found].value_end;
            i = found + 1;
            while i < self.parsed.len() && self.parsed[i].is_continuation() {
                end = self.parsed[i].value_end;
                i += 1;
            }
            joined.extend_from_slice(&self.raw[begin..end]);
        }
        value
    }

    pub fn has_header(&self, name: &[u8]) -> bool {
        self.find_header(0, name).is_some()
    }

    pub fn has_header_value(&self, name: &[u8], value: &[u8]) -> bool {
        self.enumerate_header(name).any(|candidate| candidate.eq_ignore_ascii_case(value))
    }

    pub fn is_keep_alive(&self) -> bool {
        if self.version < HttpVersion::new(1, 0) {
            return false;
        }
        for header in [&b"connection"[..], b"proxy-connection"] {
            for token in self.enumerate_header(header) {
                if token.eq_ignore_ascii_case(b"keep-alive") {
                    return true;
                }
                if token.eq_ignore_ascii_case(b"close") {
                    return false;
                }
            }
        }
        self.version != HttpVersion::new(1, 0)
    }

    pub fn content_length(&self) -> Option<i64> {
        let value = self.enumerate_header(b"content-length").next()?;
        if value.is_empty() || value[0] == b'+' {
            return None;
        }
        let parsed = http_util::string_to_number(value, 10, i64::MAX, i64::MIN);
        (parsed.valid && parsed.value >= 0).then_some(parsed.value)
    }

    pub fn is_chunk_encoded(&self) -> bool {
        self.version >= HttpVersion::new(1, 1) && self.has_header_value(b"Transfer-Encoding", b"chunked")
    }

    pub fn contains_multiple_copies_of_field(&self, name: &[u8]) -> bool {
        let mut values = self.enumerate_header(name);
        let Some(first) = values.next() else { return false };
        values.any(|value| value != first)
    }

    pub fn is_redirect(&self) -> bool {
        if !matches!(self.response_code, 301 | 302 | 303 | 307 | 308) {
            return false;
        }
        let mut from = 0;
        while let Some(i) = self.find_header(from, b"location") {
            if self.parsed[i].value_begin != self.parsed[i].value_end {
                return true;
            }
            from = i + 1;
        }
        false
    }
}

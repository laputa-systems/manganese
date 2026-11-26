use crate::http_util;
use crate::net_error::NetError;

const MAX_LINE_BUF_LEN: usize = 16384;

#[derive(Debug, Default)]
pub struct HttpChunkedDecoder {
    chunk_remaining: u64,
    line_buf: Vec<u8>,
    chunk_terminator_remaining: bool,
    reached_last_chunk: bool,
    reached_eof: bool,
    bytes_after_eof: usize,
}

impl HttpChunkedDecoder {
    pub fn reached_eof(&self) -> bool {
        self.reached_eof
    }

    pub fn bytes_after_eof(&self) -> usize {
        self.bytes_after_eof
    }

    pub fn filter_buf(&mut self, buf: &mut [u8]) -> Result<usize, NetError> {
        let mut result = 0;
        let mut start = 0;
        let mut len = buf.len();
        while len > 0 {
            if self.chunk_remaining > 0 {
                let num = usize::try_from(self.chunk_remaining).unwrap_or(usize::MAX).min(len);
                self.chunk_remaining -= num as u64;
                result += num;
                start += num;
                len -= num;
                if self.chunk_remaining == 0 {
                    self.chunk_terminator_remaining = true;
                }
                continue;
            } else if self.reached_eof {
                self.bytes_after_eof += len;
                break;
            }
            let consumed = self.scan_for_chunk_remaining(&buf[start..start + len])?;
            buf.copy_within(start + consumed..start + len, start);
            len -= consumed;
        }
        Ok(result)
    }

    fn scan_for_chunk_remaining(&mut self, buf: &[u8]) -> Result<usize, NetError> {
        let Some(index_of_lf) = buf.iter().position(|&byte| byte == b'\n') else {
            let mut partial = buf;
            if partial.last() == Some(&b'\r') {
                partial = &partial[..partial.len() - 1];
            }
            if self.line_buf.len() + partial.len() > MAX_LINE_BUF_LEN {
                return Err(NetError::InvalidChunkedEncoding);
            }
            self.line_buf.extend_from_slice(partial);
            return Ok(buf.len());
        };
        let mut line = &buf[..index_of_lf];
        if line.last() == Some(&b'\r') {
            line = &line[..line.len() - 1];
        }
        let joined;
        if !self.line_buf.is_empty() {
            if self.line_buf.len() + line.len() > MAX_LINE_BUF_LEN {
                return Err(NetError::InvalidChunkedEncoding);
            }
            self.line_buf.extend_from_slice(line);
            joined = std::mem::take(&mut self.line_buf);
            line = &joined;
        }
        if self.reached_last_chunk {
            if line.is_empty() {
                self.reached_eof = true;
            }
        } else if self.chunk_terminator_remaining {
            if !line.is_empty() {
                return Err(NetError::InvalidChunkedEncoding);
            }
            self.chunk_terminator_remaining = false;
        } else if !line.is_empty() {
            let size = match line.iter().position(|&byte| byte == b';') {
                Some(semicolon) => &line[..semicolon],
                None => line,
            };
            self.chunk_remaining = parse_chunk_size(size).ok_or(NetError::InvalidChunkedEncoding)?;
            if self.chunk_remaining == 0 {
                self.reached_last_chunk = true;
            }
        } else {
            return Err(NetError::InvalidChunkedEncoding);
        }
        self.line_buf.clear();
        Ok(index_of_lf + 1)
    }
}

fn parse_chunk_size(buf: &[u8]) -> Option<u64> {
    let end = buf.iter().rposition(|&byte| byte != b' ').map_or(0, |last| last + 1);
    let size = &buf[..end];
    if !size.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    let parsed = http_util::string_to_number(size, 16, i64::MAX, i64::MIN);
    (parsed.valid && parsed.value >= 0).then_some(parsed.value as u64)
}

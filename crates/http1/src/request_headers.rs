use crate::http_util;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HttpRequestHeaders {
    headers: Vec<(Vec<u8>, Vec<u8>)>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct InvalidHeader;

impl HttpRequestHeaders {
    fn find(&self, key: &[u8]) -> Option<usize> {
        self.headers.iter().position(|(name, _)| name.eq_ignore_ascii_case(key))
    }

    fn check(key: &[u8], value: &[u8]) -> Result<(), InvalidHeader> {
        if http_util::is_valid_header_name(key) && http_util::is_valid_header_value(value) { Ok(()) } else { Err(InvalidHeader) }
    }

    pub fn get(&self, key: &[u8]) -> Option<&[u8]> {
        self.find(key).map(|i| self.headers[i].1.as_slice())
    }

    pub fn has(&self, key: &[u8]) -> bool {
        self.find(key).is_some()
    }

    pub fn set(&mut self, key: &[u8], value: &[u8]) -> Result<(), InvalidHeader> {
        Self::check(key, value)?;
        match self.find(key) {
            Some(i) => self.headers[i].1 = value.to_vec(),
            None => self.headers.push((key.to_vec(), value.to_vec())),
        }
        Ok(())
    }

    pub fn set_if_missing(&mut self, key: &[u8], value: &[u8]) -> Result<(), InvalidHeader> {
        Self::check(key, value)?;
        if self.find(key).is_none() {
            self.headers.push((key.to_vec(), value.to_vec()));
        }
        Ok(())
    }

    pub fn remove(&mut self, key: &[u8]) {
        if let Some(i) = self.find(key) {
            self.headers.remove(i);
        }
    }

    pub fn merge_from(&mut self, other: &HttpRequestHeaders) {
        for (key, value) in &other.headers {
            self.set(key, value).expect("merged headers were validated when set");
        }
    }

    pub fn to_wire(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for (key, value) in &self.headers {
            out.extend_from_slice(key);
            out.extend_from_slice(b": ");
            out.extend_from_slice(value);
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(b"\r\n");
        out
    }
}

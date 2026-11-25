#[derive(Clone, Debug)]
pub struct Response {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

fn reason(status: u16) -> &'static str {
    match status {
        101 => "WebSocket Protocol Handshake",
        200 => "OK",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        500 => "Internal Server Error",
        _ => "",
    }
}

impl Response {
    pub fn new(status: u16) -> Self {
        Self { status, headers: Vec::new(), body: Vec::new() }
    }

    pub fn not_found() -> Self {
        Self::new(404).body(Vec::new(), "text/html")
    }

    pub fn internal_error(message: &[u8]) -> Self {
        Self::new(500).body(message.to_vec(), "text/html")
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }

    pub fn body(mut self, body: Vec<u8>, content_type: &str) -> Self {
        self.headers.push(("Content-Length".to_owned(), body.len().to_string()));
        self.headers.push(("Content-Type".to_owned(), content_type.to_owned()));
        self.body = body;
        self
    }

    pub fn status(&self) -> u16 {
        self.status
    }

    pub fn serialize(&self) -> Vec<u8> {
        let mut out = format!("HTTP/1.1 {} {}\r\n", self.status, reason(self.status)).into_bytes();
        for (name, value) in &self.headers {
            out.extend_from_slice(name.as_bytes());
            out.push(b':');
            out.extend_from_slice(value.as_bytes());
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(b"\r\n");
        out.extend_from_slice(&self.body);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::Response;

    #[test]
    fn headers_are_written_without_a_space_after_the_colon() {
        let response = Response::new(200).header("X-Frame-Options", "DENY").body(b"hi".to_vec(), "text/html");
        assert_eq!(response.serialize(), b"HTTP/1.1 200 OK\r\nX-Frame-Options:DENY\r\nContent-Length:2\r\nContent-Type:text/html\r\n\r\nhi");
    }
}

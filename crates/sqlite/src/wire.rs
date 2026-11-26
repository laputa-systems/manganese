use std::io::{Read, Write};

pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn done(&self) -> bool {
        self.pos == self.data.len()
    }

    fn need(&self, count: usize) {
        if self.data.len() - self.pos < count {
            fail("truncated request");
        }
    }

    pub fn u8(&mut self) -> u8 {
        self.need(1);
        let value = self.data[self.pos];
        self.pos += 1;
        value
    }

    pub fn u16(&mut self) -> u16 {
        u16::from(self.u8()) << 8 | u16::from(self.u8())
    }

    pub fn u32(&mut self) -> u32 {
        u32::from(self.u16()) << 16 | u32::from(self.u16())
    }

    pub fn i32(&mut self) -> i32 {
        self.u32() as i32
    }

    pub fn u64(&mut self) -> u64 {
        u64::from(self.u32()) << 32 | u64::from(self.u32())
    }

    pub fn bytes(&mut self) -> Vec<u8> {
        let length = self.u32() as usize;
        self.need(length);
        let value = self.data[self.pos..self.pos + length].to_vec();
        self.pos += length;
        value
    }
}

#[derive(Default)]
pub struct Writer {
    out: Vec<u8>,
}

impl Writer {
    pub fn u8(&mut self, value: u8) {
        self.out.push(value);
    }

    pub fn u16(&mut self, value: u16) {
        self.out.extend_from_slice(&value.to_be_bytes());
    }

    pub fn u32(&mut self, value: u32) {
        self.out.extend_from_slice(&value.to_be_bytes());
    }

    pub fn u64(&mut self, value: u64) {
        self.out.extend_from_slice(&value.to_be_bytes());
    }

    pub fn i32(&mut self, value: i32) {
        self.u32(value as u32);
    }

    pub fn i64(&mut self, value: i64) {
        self.u64(value as u64);
    }

    pub fn bool(&mut self, value: bool) {
        self.u8(u8::from(value));
    }

    pub fn bytes(&mut self, value: &[u8]) {
        self.u32(value.len() as u32);
        self.out.extend_from_slice(value);
    }

    pub fn into_inner(self) -> Vec<u8> {
        self.out
    }
}

pub fn fail(message: &str) -> ! {
    eprintln!("sqlite candidate: {message}");
    std::process::exit(2);
}

pub fn read_frame(input: &mut impl Read) -> Option<Vec<u8>> {
    let mut header = [0u8; 4];
    let mut got = 0;
    while got < 4 {
        match input.read(&mut header[got..]) {
            Ok(0) if got == 0 => return None,
            Ok(0) => fail("truncated frame header"),
            Ok(count) => got += count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => fail("cannot read request"),
        }
    }
    let mut body = vec![0u8; u32::from_be_bytes(header) as usize];
    if input.read_exact(&mut body).is_err() {
        fail("truncated frame body");
    }
    Some(body)
}

pub fn write_frame(output: &mut impl Write, body: &[u8]) {
    let ok = output.write_all(&(body.len() as u32).to_be_bytes()).is_ok()
        && output.write_all(body).is_ok()
        && output.flush().is_ok();
    if !ok {
        fail("cannot write response");
    }
}

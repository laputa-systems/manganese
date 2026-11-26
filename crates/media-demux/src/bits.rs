#[derive(Clone)]
pub struct BitReader<'a> {
    buf: &'a [u8],
    index: usize,
    size_in_bits_plus8: usize,
}

impl<'a> BitReader<'a> {
    pub fn new(buf: &'a [u8], bit_size: usize) -> Self {
        BitReader { buf, index: 0, size_in_bits_plus8: bit_size + 8 }
    }

    pub fn bytes(buf: &'a [u8]) -> Self {
        Self::new(buf, buf.len() * 8)
    }

    fn byte(&self, at: usize) -> u64 {
        self.buf.get(at).copied().unwrap_or(0) as u64
    }

    fn show(&self, n: u32) -> u64 {
        if n == 0 {
            return 0;
        }
        let first = self.index >> 3;
        let mut window: u128 = 0;
        for k in 0..10 {
            window = window << 8 | self.byte(first + k) as u128;
        }
        let shifted = window << (self.index & 7);
        (shifted >> (80 - n)) as u64 & ((1u128 << n) - 1) as u64
    }

    fn skip(&mut self, n: usize) {
        self.index = (self.index + n).min(self.size_in_bits_plus8);
    }

    pub fn get(&mut self, n: u32) -> u32 {
        let value = self.show(n) as u32;
        self.skip(n as usize);
        value
    }

    pub fn get1(&mut self) -> bool {
        self.get(1) != 0
    }

    pub fn get64(&mut self, n: u32) -> u64 {
        if n <= 32 {
            return self.get(n) as u64;
        }
        let high = (self.get(n - 32) as u64) << 32;
        high | self.get(32) as u64
    }

    pub fn skip_bits(&mut self, n: usize) {
        self.skip(n);
    }

    pub fn count(&self) -> usize {
        self.index
    }

    pub fn left(&self) -> i64 {
        self.size_in_bits_plus8 as i64 - 8 - self.index as i64
    }

    pub fn buffer(&self) -> &'a [u8] {
        self.buf
    }
}

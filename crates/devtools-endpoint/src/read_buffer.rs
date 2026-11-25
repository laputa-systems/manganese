const INITIAL_CAPACITY: usize = 1024;
const MINIMUM_CAPACITY: usize = 128;
const GROWTH_FACTOR: usize = 2;
const DEFAULT_MAX_CAPACITY: usize = 1024 * 1024;
pub const WEBSOCKET_MAX_CAPACITY: usize = 100 * 1024 * 1024;

/// net::HttpConnection::ReadIOBuffer: how much one socket read may return depends on this capacity,
/// and where reads split decides which pipelined requests a single parse pass sees.
#[derive(Debug)]
pub struct ReadBuffer {
    data: Vec<u8>,
    capacity: usize,
    max_capacity: usize,
}

impl Default for ReadBuffer {
    fn default() -> Self {
        Self { data: Vec::new(), capacity: INITIAL_CAPACITY, max_capacity: DEFAULT_MAX_CAPACITY }
    }
}

impl ReadBuffer {
    pub fn readable(&self) -> &[u8] {
        &self.data
    }

    pub fn set_max_capacity(&mut self, max_capacity: usize) {
        self.max_capacity = max_capacity;
    }

    /// Room for the next read, growing the buffer when it is full; `None` when it cannot grow.
    pub fn prepare_read(&mut self) -> Option<usize> {
        if self.capacity == self.data.len() {
            if self.capacity >= self.max_capacity {
                return None;
            }
            self.capacity = (self.capacity * GROWTH_FACTOR).min(self.max_capacity);
        }
        Some(self.capacity - self.data.len())
    }

    pub fn did_read(&mut self, bytes: &[u8]) {
        self.data.extend_from_slice(bytes);
    }

    pub fn consume(&mut self, count: usize) {
        let previous = self.data.len();
        self.data.drain(..count);
        if self.capacity > MINIMUM_CAPACITY && self.capacity > previous * GROWTH_FACTOR {
            self.capacity = (self.capacity / GROWTH_FACTOR).max(MINIMUM_CAPACITY);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_grows_when_full_and_shrinks_after_small_consumptions() {
        let mut buffer = ReadBuffer::default();
        assert_eq!(buffer.prepare_read(), Some(1024));
        buffer.did_read(&[0; 1024]);
        assert_eq!(buffer.prepare_read(), Some(1024));
        buffer.consume(1024);
        assert_eq!(buffer.prepare_read(), Some(2048));
        buffer.did_read(&[0; 90]);
        buffer.consume(90);
        assert_eq!(buffer.prepare_read(), Some(1024));
    }

    #[test]
    fn a_full_buffer_at_the_maximum_cannot_grow() {
        let mut buffer = ReadBuffer::default();
        buffer.set_max_capacity(1024);
        buffer.did_read(&[0; 1024]);
        assert_eq!(buffer.prepare_read(), None);
    }
}

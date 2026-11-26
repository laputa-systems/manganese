pub const fn fferrtag(tag: &[u8; 4]) -> i32 {
    -((tag[0] as i32) | (tag[1] as i32) << 8 | (tag[2] as i32) << 16 | (tag[3] as i32) << 24)
}

pub const AVERROR_EOF: i32 = fferrtag(b"EOF ");
pub const AVERROR_INVALIDDATA: i32 = fferrtag(b"INDA");
pub const AVERROR_PATCHWELCOME: i32 = fferrtag(b"PAWE");
pub const EIO: i32 = -5;
pub const EINVAL: i32 = -22;

const IO_BUFFER_SIZE: usize = 32768;
const SHORT_SEEK_THRESHOLD: i64 = 32768;

pub struct RawRead {
    pub data: Vec<u8>,
    pub position: i64,
    pub corrupt: bool,
    pub result: i32,
}

pub enum Whence {
    Set,
    Cur,
}

pub struct Avio<'a> {
    data: &'a [u8],
    read_position: usize,
    buffer: Vec<u8>,
    orig_buffer_size: usize,
    buf_ptr: usize,
    buf_end: usize,
    buf_ptr_max: usize,
    pos: i64,
    eof_reached: bool,
    pub error: i32,
}

impl<'a> Avio<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Avio {
            data,
            read_position: 0,
            buffer: vec![0; IO_BUFFER_SIZE],
            orig_buffer_size: IO_BUFFER_SIZE,
            buf_ptr: 0,
            buf_end: 0,
            buf_ptr_max: 0,
            pos: 0,
            eof_reached: false,
            error: 0,
        }
    }

    fn protocol_read(&mut self, into: Option<&mut [u8]>, dst: usize, len: usize) -> i32 {
        if len == 0 {
            return 0;
        }
        if self.read_position >= self.data.len() {
            return AVERROR_EOF;
        }
        let count = len.min(self.data.len() - self.read_position);
        let source = &self.data[self.read_position..self.read_position + count];
        match into {
            Some(out) => out[..count].copy_from_slice(source),
            None => self.buffer[dst..dst + count].copy_from_slice(source),
        }
        self.read_position += count;
        count as i32
    }

    fn protocol_seek(&mut self, offset: i64) -> i64 {
        if offset < 0 || offset > self.data.len() as i64 {
            return EIO as i64;
        }
        self.read_position = offset as usize;
        offset
    }

    fn fill_buffer(&mut self) {
        let mut dst = if self.buf_end + IO_BUFFER_SIZE <= self.buffer.len() { self.buf_end } else { 0 };
        let mut len = self.buffer.len() - dst;
        if self.eof_reached {
            return;
        }
        if self.orig_buffer_size != 0 && self.buffer.len() > self.orig_buffer_size && len >= self.orig_buffer_size {
            if dst == 0 && self.buf_ptr != dst {
                self.buffer = vec![0; self.orig_buffer_size];
                self.buf_ptr = 0;
                self.buf_ptr_max = 0;
                self.buf_end = 0;
                dst = 0;
            }
            len = self.orig_buffer_size;
        }
        let read = self.protocol_read(None, dst, len);
        if read == AVERROR_EOF {
            self.eof_reached = true;
        } else if read < 0 {
            self.eof_reached = true;
            self.error = read;
        } else {
            self.pos += read as i64;
            self.buf_ptr = dst;
            self.buf_end = dst + read as usize;
        }
    }

    pub fn r8(&mut self) -> u32 {
        if self.buf_ptr >= self.buf_end {
            self.fill_buffer();
        }
        if self.buf_ptr < self.buf_end {
            let value = self.buffer[self.buf_ptr];
            self.buf_ptr += 1;
            return value as u32;
        }
        0
    }

    pub fn rl16(&mut self) -> u32 {
        let low = self.r8();
        low | self.r8() << 8
    }

    pub fn rl32(&mut self) -> u32 {
        let low = self.rl16();
        low | self.rl16() << 16
    }

    pub fn rl64(&mut self) -> u64 {
        let low = self.rl32() as u64;
        low | (self.rl32() as u64) << 32
    }

    pub fn rb16(&mut self) -> u32 {
        let high = self.r8() << 8;
        high | self.r8()
    }

    pub fn rb32(&mut self) -> u32 {
        let high = self.rb16() << 16;
        high | self.rb16()
    }

    pub fn rb64(&mut self) -> u64 {
        let high = (self.rb32() as u64) << 32;
        high | self.rb32() as u64
    }

    pub fn read(&mut self, out: &mut [u8]) -> i32 {
        let requested = out.len();
        let mut size = requested;
        let mut written = 0;
        while size > 0 {
            let available = (self.buf_end - self.buf_ptr).min(size);
            if available == 0 {
                if size > self.buffer.len() {
                    let read = self.protocol_read(Some(&mut out[written..]), 0, size);
                    if read == AVERROR_EOF {
                        self.eof_reached = true;
                        break;
                    } else if read < 0 {
                        self.eof_reached = true;
                        self.error = read;
                        break;
                    }
                    self.pos += read as i64;
                    size -= read as usize;
                    written += read as usize;
                    self.buf_ptr = 0;
                    self.buf_end = 0;
                } else {
                    self.fill_buffer();
                    if self.buf_end - self.buf_ptr == 0 {
                        break;
                    }
                }
            } else {
                out[written..written + available].copy_from_slice(&self.buffer[self.buf_ptr..self.buf_ptr + available]);
                written += available;
                self.buf_ptr += available;
                size -= available;
            }
        }
        if size == requested {
            if self.error != 0 {
                return self.error;
            }
            if self.feof() {
                return AVERROR_EOF;
            }
        }
        (requested - size) as i32
    }

    pub fn read_size(&mut self, out: &mut [u8]) -> i32 {
        let read = self.read(out);
        if read as usize == out.len() && read >= 0 {
            return read;
        }
        if read < 0 && read != AVERROR_EOF {
            return read;
        }
        AVERROR_INVALIDDATA
    }

    pub fn seek(&mut self, offset: i64, whence: Whence) -> i64 {
        let buffer_size = self.buf_end as i64;
        let pos = self.pos - buffer_size;
        let mut offset = offset;
        if let Whence::Cur = whence {
            let offset1 = pos + self.buf_ptr as i64;
            if offset == 0 {
                return offset1;
            }
            if offset > i64::MAX - offset1 {
                return EINVAL as i64;
            }
            offset += offset1;
        }
        if offset < 0 {
            return EINVAL as i64;
        }
        let offset1 = offset - pos;
        self.buf_ptr_max = self.buf_ptr_max.max(self.buf_ptr);
        if offset1 >= 0 && offset1 <= buffer_size {
            self.buf_ptr = offset1 as usize;
        } else if offset1 <= buffer_size + SHORT_SEEK_THRESHOLD && offset1 >= 0 {
            while self.pos < offset && !self.eof_reached {
                self.fill_buffer();
            }
            if self.eof_reached {
                return AVERROR_EOF as i64;
            }
            self.buf_ptr = (self.buf_end as i64 - (self.pos - offset)) as usize;
        } else if offset1 < 0 && -offset1 < buffer_size >> 1 && offset > 0 {
            let back = pos - (buffer_size >> 1).min(pos);
            let result = self.protocol_seek(back);
            if result < 0 {
                return result;
            }
            self.buf_end = 0;
            self.buf_ptr = 0;
            self.pos = back;
            self.eof_reached = false;
            self.fill_buffer();
            return self.seek(offset, Whence::Set);
        } else {
            let result = self.protocol_seek(offset);
            if result < 0 {
                return result;
            }
            self.buf_end = 0;
            self.buf_ptr = 0;
            self.buf_ptr_max = 0;
            self.pos = offset;
        }
        self.eof_reached = false;
        offset
    }

    pub fn skip(&mut self, offset: i64) -> i64 {
        self.seek(offset, Whence::Cur)
    }

    pub fn tell(&mut self) -> i64 {
        self.seek(0, Whence::Cur)
    }

    pub fn size(&self) -> i64 {
        self.data.len() as i64
    }

    pub fn feof(&mut self) -> bool {
        if self.eof_reached {
            self.eof_reached = false;
            self.fill_buffer();
        }
        self.eof_reached
    }

    pub fn rewind_with_probe_data(&mut self, mut probe: Vec<u8>, probe_size: usize) -> i32 {
        let buffer_size = self.buf_end as i64;
        let buffer_start = self.pos - buffer_size;
        let mut size = probe_size as i64;
        if buffer_start > size {
            return EINVAL;
        }
        let overlap = size - buffer_start;
        let new_size = size + buffer_size - overlap;
        let alloc_size = (self.buffer.len() as i64).max(new_size);
        probe.resize(alloc_size.max(size) as usize, 0);
        if new_size > size {
            let from = overlap as usize;
            let to = self.buf_end;
            probe[size as usize..(size as usize + to - from)].copy_from_slice(&self.buffer[from..to]);
            size = new_size;
        }
        probe.truncate(alloc_size as usize);
        self.buffer = probe;
        self.buf_ptr = 0;
        self.pos = size;
        self.buf_end = size as usize;
        self.eof_reached = false;
        0
    }

    pub fn get_packet(&mut self, size: i32) -> RawRead {
        let position = self.tell();
        let mut data = vec![0; size.max(0) as usize];
        let read = self.read(&mut data);
        if read != size {
            data.truncate(read.max(0) as usize);
        }
        let corrupt = (data.len() as i32) < size;
        let result = if !data.is_empty() { data.len() as i32 } else { read };
        RawRead { data, position, corrupt, result }
    }
}

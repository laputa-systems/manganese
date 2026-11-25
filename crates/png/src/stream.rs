use std::cell::Cell;
use std::io::{self, ErrorKind, Read, Seek, SeekFrom};
use std::rc::Rc;

#[derive(Clone)]
pub struct Stream {
    data: Rc<[u8]>,
    position: Rc<Cell<usize>>,
}

impl Stream {
    pub fn new(data: &[u8]) -> Self {
        Self { data: data.into(), position: Rc::new(Cell::new(0)) }
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn rewind(&self) {
        self.position.set(0);
    }

    fn seek_from_start(&self, position: u64) -> io::Result<u64> {
        let position = usize::try_from(position).map_err(|_| io::Error::from(ErrorKind::Other))?;
        self.position.set(position);
        Ok(position as u64)
    }
}

impl Read for Stream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let position = self.position.get();
        if position > self.data.len() {
            return Ok(0);
        }
        let count = buffer.len().min(self.data.len() - position);
        buffer[..count].copy_from_slice(&self.data[position..position + count]);
        self.position.set(position + count);
        Ok(count)
    }
}

impl Seek for Stream {
    fn seek(&mut self, target: SeekFrom) -> io::Result<u64> {
        match target {
            SeekFrom::Start(position) => self.seek_from_start(position),
            SeekFrom::End(offset) => {
                if offset > 0 || offset == i64::MIN {
                    return Err(io::Error::from(ErrorKind::Other));
                }
                let back = offset.unsigned_abs();
                let length = self.data.len() as u64;
                if back > length {
                    return Err(io::Error::from(ErrorKind::Other));
                }
                self.seek_from_start(length - back)
            }
            SeekFrom::Current(offset) => {
                if offset != 0 {
                    self.position.set(self.position.get().wrapping_add(offset as isize as usize));
                }
                Ok(self.position.get() as u64)
            }
        }
    }
}

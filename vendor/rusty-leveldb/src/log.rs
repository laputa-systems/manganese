//! A log consists of a number of blocks.
//! A block consists of a number of records and an optional trailer (filler).
//! A record is a bytestring: [checksum: uint32, length: uint16, type: uint8, data: [u8]]
//! checksum is the crc32 sum of type and data; type is one of RecordType::{Full/First/Middle/Last}

use crate::crc;
use crate::env::WritableFile;
use crate::error::{err, Result, StatusCode};

use std::io::{Read, Write};

use integer_encoding::FixedInt;

const BLOCK_SIZE: usize = 32 * 1024;
const HEADER_SIZE: usize = 4 + 2 + 1;

#[derive(Clone, Copy)]
pub enum RecordType {
    Full = 1,
    First = 2,
    Middle = 3,
    Last = 4,
}

pub struct LogWriter<W: Write> {
    dst: W,
    current_block_offset: usize,
    block_size: usize,
}

impl<W: Write> LogWriter<W> {
    pub fn new(writer: W) -> LogWriter<W> {
        LogWriter {
            dst: writer,
            current_block_offset: 0,
            block_size: BLOCK_SIZE,
        }
    }

    /// new_with_off opens a writer starting at some offset of an existing log file. The file must
    /// have the default block size.
    pub fn new_with_off(writer: W, off: usize) -> LogWriter<W> {
        let mut w = LogWriter::new(writer);
        w.current_block_offset = off % BLOCK_SIZE;
        w
    }

    pub fn add_record(&mut self, mut record: &[u8]) -> Result<usize> {
        let mut first_frag = true;
        let mut result = Ok(0);
        while result.is_ok() && !record.is_empty() {
            assert!(self.block_size > HEADER_SIZE);

            let space_left = self.block_size - self.current_block_offset;

            // Fill up block; go to next block.
            if space_left < HEADER_SIZE {
                self.dst.write_all(&vec![0, 0, 0, 0, 0, 0][0..space_left])?;
                self.current_block_offset = 0;
            }

            let avail_for_data = self.block_size - self.current_block_offset - HEADER_SIZE;

            let data_frag_len = if record.len() < avail_for_data {
                record.len()
            } else {
                avail_for_data
            };

            let recordtype = if first_frag && data_frag_len == record.len() {
                RecordType::Full
            } else if first_frag {
                RecordType::First
            } else if data_frag_len == record.len() {
                RecordType::Last
            } else {
                RecordType::Middle
            };

            result = self.emit_record(recordtype, record, data_frag_len);
            record = &record[data_frag_len..];
            first_frag = false;
        }
        result
    }

    fn emit_record(&mut self, t: RecordType, data: &[u8], len: usize) -> Result<usize> {
        assert!(len < 256 * 256);

        let mut digest = crc::digest();
        digest.update(&[t as u8]);
        digest.update(&data[0..len]);

        let chksum = mask_crc(digest.finalize());

        self.dst.write_all(&chksum.encode_fixed_vec())?;
        self.dst.write_all(&(len as u16).encode_fixed_vec())?;
        self.dst.write_all(&[t as u8])?;
        self.dst.write_all(&data[0..len])?;

        let s = HEADER_SIZE + len;
        self.current_block_offset += s;
        Ok(s)
    }

    pub fn flush(&mut self) -> Result<()> {
        self.dst.flush()?;
        Ok(())
    }
}

impl<W: WritableFile> LogWriter<W> {
    pub fn sync(&mut self) -> Result<()> {
        self.dst.sync()
    }
}

pub struct LogReader<R: Read> {
    src: R,
    blocksize: usize,
    block: Vec<u8>,
    offset: usize,
    eof: bool,
    checksums: bool,
    pending: Option<(u8, Vec<u8>)>,
}

impl<R: Read> LogReader<R> {
    pub fn new(src: R, chksum: bool) -> LogReader<R> {
        LogReader {
            src,
            blocksize: BLOCK_SIZE,
            block: Vec::new(),
            offset: 0,
            eof: false,
            checksums: chksum,
            pending: None,
        }
    }

    /// EOF is signalled by Ok(0)
    pub fn read(&mut self, dst: &mut Vec<u8>) -> Result<usize> {
        Ok(self.read_record(dst)?.unwrap_or(0))
    }

    pub fn read_record(&mut self, dst: &mut Vec<u8>) -> Result<Option<usize>> {
        dst.clear();
        let mut fragmented = false;

        loop {
            let fragment = match self.pending.take() {
                Some(fragment) => Some(fragment),
                None => self.read_fragment()?,
            };
            let Some((typ, data)) = fragment else {
                dst.clear();
                return Ok(None);
            };
            match typ {
                1 | 2 => {
                    if fragmented && !dst.is_empty() {
                        self.pending = Some((typ, data));
                        return err(StatusCode::Corruption, "partial record without end");
                    }
                    dst.clear();
                    dst.extend_from_slice(&data);
                    if typ == RecordType::Full as u8 {
                        return Ok(Some(dst.len()));
                    }
                    fragmented = true;
                }
                3 | 4 => {
                    if !fragmented {
                        return err(StatusCode::Corruption, "missing start of fragmented record");
                    }
                    dst.extend_from_slice(&data);
                    if typ == RecordType::Last as u8 {
                        return Ok(Some(dst.len()));
                    }
                }
                0 if data.is_empty() => {
                    if fragmented {
                        return err(StatusCode::Corruption, "error in middle of record");
                    }
                }
                _ => return err(StatusCode::Corruption, "unknown record type"),
            }
        }
    }

    fn read_fragment(&mut self) -> Result<Option<(u8, Vec<u8>)>> {
        loop {
            if self.block.len() - self.offset < HEADER_SIZE {
                if self.eof {
                    return Ok(None);
                }
                self.block.resize(self.blocksize, 0);
                let mut filled = 0;
                while filled < self.blocksize {
                    match self.src.read(&mut self.block[filled..]) {
                        Ok(0) => { self.eof = true; break; }
                        Ok(n) => filled += n,
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(e) => {
                            self.eof = true;
                            self.block.clear();
                            self.offset = 0;
                            return Err(e.into());
                        }
                    }
                }
                self.block.truncate(filled);
                self.offset = 0;
                continue;
            }

            let header = &self.block[self.offset..self.offset + HEADER_SIZE];
            let checksum = u32::decode_fixed(&header[..4]);
            let length = u16::decode_fixed(&header[4..6]) as usize;
            let typ = header[6];
            if HEADER_SIZE + length > self.block.len() - self.offset {
                self.offset = self.block.len();
                if self.eof {
                    return Ok(None);
                }
                return err(StatusCode::Corruption, "bad record length");
            }
            if typ == 0 && length == 0 {
                self.offset = self.block.len();
                return Ok(Some((0, Vec::new())));
            }
            let start = self.offset + HEADER_SIZE;
            let data = self.block[start..start + length].to_vec();
            self.offset = start + length;
            if self.checksums {
                let mut digest = crc::digest();
                digest.update(&[typ]);
                digest.update(&data);
                if unmask_crc(checksum) != digest.finalize() {
                    self.offset = self.block.len();
                    return err(StatusCode::Corruption, "Invalid Checksum");
                }
            }
            return Ok(Some((typ, data)));
        }
    }
}

const MASK_DELTA: u32 = 0xa282ead8;

pub fn mask_crc(c: u32) -> u32 {
    (c.wrapping_shr(15) | c.wrapping_shl(17)).wrapping_add(MASK_DELTA)
}

pub fn unmask_crc(mc: u32) -> u32 {
    let rot = mc.wrapping_sub(MASK_DELTA);
    rot.wrapping_shr(17) | rot.wrapping_shl(15)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_crc_mask_crc() {
        let mut digest = crc::digest();
        digest.update("abcde".as_bytes());
        let sum = digest.finalize();
        assert_eq!(sum, unmask_crc(mask_crc(sum)));
        assert!(sum != mask_crc(sum));
    }

    #[test]
    fn test_crc_sanity() {
        assert_eq!(0x8a9136aa, crc::crc32([0_u8; 32]));
        assert_eq!(0x62a8ab43, crc::crc32([0xff_u8; 32]));
    }

    #[test]
    fn test_writer() {
        let data = &[
            "hello world. My first log entry.",
            "and my second",
            "and my third",
        ];
        let mut lw = LogWriter::new(Vec::new());
        let total_len = data.iter().fold(0, |l, d| l + d.len());

        for d in data {
            let _ = lw.add_record(d.as_bytes());
        }

        assert_eq!(lw.current_block_offset, total_len + 3 * super::HEADER_SIZE);
    }

    #[test]
    fn writer_retries_short_writes() {
        struct ShortWriter(Vec<u8>);

        impl Write for ShortWriter {
            fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
                let len = data.len().min(1);
                self.0.extend_from_slice(&data[..len]);
                Ok(len)
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let mut writer = LogWriter::new(ShortWriter(Vec::new()));
        writer.add_record(b"key and value").unwrap();
        let mut reader = LogReader::new(writer.dst.0.as_slice(), true);
        let mut record = Vec::new();
        assert_eq!(reader.read(&mut record).unwrap(), 13);
        assert_eq!(record, b"key and value");
    }

    #[test]
    fn test_writer_append() {
        let data = &[
            "hello world. My first log entry.",
            "and my second",
            "and my third",
        ];

        let mut dst = vec![0_u8; 1024];

        {
            let mut lw = LogWriter::new(Cursor::new(dst.as_mut_slice()));
            for d in data {
                let _ = lw.add_record(d.as_bytes());
            }
        }

        let old = dst.clone();

        // Ensure that new_with_off positions the writer correctly. Some ugly mucking about with
        // cursors and stuff is required.
        {
            let offset = data[0].len() + super::HEADER_SIZE;
            let mut lw =
                LogWriter::new_with_off(Cursor::new(&mut dst.as_mut_slice()[offset..]), offset);
            for d in &data[1..] {
                let _ = lw.add_record(d.as_bytes());
            }
        }
        assert_eq!(old, dst);
    }

    #[test]
    fn test_reader() {
        let data = [
            "abcdefghi".as_bytes().to_vec(),    // fits one block of 17
            "123456789012".as_bytes().to_vec(), // spans two blocks of 17
            "0101010101010101010101".as_bytes().to_vec(),
        ]; // spans three blocks of 17
        let mut lw = LogWriter::new(Vec::new());
        lw.block_size = super::HEADER_SIZE + 10;

        for e in data.iter() {
            assert!(lw.add_record(e).is_ok());
        }

        assert_eq!(lw.dst.len(), 93);
        // Corrupt first record.
        lw.dst[2] += 1;

        let mut lr = LogReader::new(lw.dst.as_slice(), true);
        lr.blocksize = super::HEADER_SIZE + 10;
        let mut dst = Vec::with_capacity(128);

        // First record is corrupted.
        assert_eq!(
            err(StatusCode::Corruption, "Invalid Checksum"),
            lr.read(&mut dst)
        );

        let mut i = 1;
        loop {
            match lr.read(&mut dst) {
                Err(e) => panic!("{}", e),
                Ok(0) => break,
                _ => {}
            }

            assert_eq!(dst, data[i]);
            i += 1;
        }
        assert_eq!(i, data.len());
    }
}

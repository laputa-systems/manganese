#![allow(missing_docs)]

pub mod chromium;

use std::cell::RefCell;
use std::io::{self, BufRead, Read, Seek, SeekFrom, Write};
use std::rc::Rc;

pub use png::{BitDepth, ColorType};

pub enum Compression {
    Low,
    Medium,
}

pub fn encode(width: u32, height: u32, color: ColorType, depth: BitDepth, pixels: &[u8], compression: Compression) -> Result<Vec<u8>, png::EncodingError> {
    let mut output = Vec::new();
    let mut encoder = png::Encoder::new(&mut output, width, height);
    encoder.set_color(color);
    encoder.set_depth(depth);
    match compression {
        Compression::Low => {
            encoder.set_deflate_compression(png::DeflateCompression::Level(1));
            encoder.set_filter(png::Filter::Up);
        }
        Compression::Medium => encoder.set_compression(png::Compression::Balanced),
    }
    {
        let mut writer = encoder.write_header()?;
        {
            let mut stream = writer.stream_writer()?;
            stream.write_all(pixels)?;
            stream.finish()?;
        }
        writer.finish()?;
    }
    Ok(output)
}

type Input = Rc<RefCell<Vec<u8>>>;

struct Feed {
    input: Input,
    bytes: Vec<u8>,
    position: usize,
}

impl Feed {
    fn new(input: Input) -> Self {
        Self { input, bytes: Vec::new(), position: 0 }
    }

    fn receive(&mut self) {
        self.bytes.append(&mut self.input.borrow_mut());
    }
}

impl Read for Feed {
    fn read(&mut self, destination: &mut [u8]) -> io::Result<usize> {
        let data = self.fill_buf()?;
        let len = data.len().min(destination.len());
        destination[..len].copy_from_slice(&data[..len]);
        self.consume(len);
        Ok(len)
    }
}

impl BufRead for Feed {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        self.receive();
        Ok(&self.bytes[self.position..])
    }

    fn consume(&mut self, amount: usize) {
        self.position = self.position.saturating_add(amount).min(self.bytes.len());
    }
}

impl Seek for Feed {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.receive();
        let next = match position {
            SeekFrom::Start(value) => i128::from(value),
            SeekFrom::End(value) => self.bytes.len() as i128 + i128::from(value),
            SeekFrom::Current(value) => self.position as i128 + i128::from(value),
        };
        self.position = usize::try_from(next).ok().filter(|&value| value <= self.bytes.len())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid PNG seek"))?;
        Ok(self.position as u64)
    }
}

pub enum Event {
    Header(Box<png::Info<'static>>),
    Frame(png::FrameControl),
    Row { data: Vec<u8>, interlace: png::InterlaceInfo },
    FrameEnd,
    Complete,
    NeedMore,
}

enum Phase {
    Header,
    Rows,
    NextFrame,
    Trailer,
    Complete,
}

pub struct Decoder {
    input: Input,
    header: Vec<u8>,
    reader: Option<png::Reader<Feed>>,
    phase: Phase,
    frames_remaining: u64,
    finished: bool,
    limit: usize,
    received: usize,
}

impl Decoder {
    pub fn new(limit: usize) -> Self {
        Self { input: Rc::default(), header: Vec::new(), reader: None, phase: Phase::Header,
            frames_remaining: 0, finished: false, limit, received: 0 }
    }

    pub fn push(&mut self, bytes: &[u8]) -> Result<(), png::DecodingError> {
        if self.finished || self.received.checked_add(bytes.len()).is_none_or(|n| n > self.limit) {
            return Err(png::DecodingError::LimitsExceeded);
        }
        self.received += bytes.len();
        if matches!(self.phase, Phase::Header) {
            self.header.extend_from_slice(bytes);
        } else {
            self.input.borrow_mut().extend_from_slice(bytes);
        }
        Ok(())
    }

    pub fn finish(&mut self) {
        self.finished = true;
    }

    pub fn next_event(&mut self) -> Result<Event, png::DecodingError> {
        let result = self.decode();
        match result {
            Err(png::DecodingError::IoError(error)) if error.kind() == io::ErrorKind::UnexpectedEof && !self.finished => Ok(Event::NeedMore),
            other => other,
        }
    }

    fn decode(&mut self) -> Result<Event, png::DecodingError> {
        match self.phase {
            Phase::Header => {
                let trial_input = Rc::new(RefCell::new(self.header.clone()));
                let mut decoder = png::Decoder::new(Feed::new(trial_input.clone()));
                decoder.set_limits(png::Limits { bytes: self.limit });
                decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
                let reader = decoder.read_info()?;
                let info = reader.info().clone();
                self.frames_remaining = info.animation_control.map_or(1, |animation| {
                    u64::from(animation.num_frames) + u64::from(info.frame_control.is_none())
                });
                self.header.clear();
                self.input = trial_input;
                self.reader = Some(reader);
                self.phase = Phase::Rows;
                Ok(Event::Header(Box::new(info)))
            }
            Phase::Rows => {
                let reader = self.reader.as_mut().expect("reader is initialized");
                match reader.next_interlaced_row()? {
                    Some(row) => Ok(Event::Row { data: row.data().to_vec(), interlace: *row.interlace() }),
                    None => {
                        self.frames_remaining -= 1;
                        self.phase = if self.frames_remaining == 0 { Phase::Trailer } else { Phase::NextFrame };
                        Ok(Event::FrameEnd)
                    }
                }
            }
            Phase::NextFrame => {
                let reader = self.reader.as_mut().expect("reader is initialized");
                let info = *reader.next_frame_info()?;
                self.phase = Phase::Rows;
                Ok(Event::Frame(info))
            }
            Phase::Trailer => {
                self.reader.as_mut().expect("reader is initialized").finish()?;
                self.phase = Phase::Complete;
                Ok(Event::Complete)
            }
            Phase::Complete => Ok(Event::Complete),
        }
    }
}

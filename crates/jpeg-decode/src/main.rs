mod buffers;
mod decoder;
mod metadata;

use buffers::{RwBuffer, SharedBuffer};
use decoder::JpegDecoder;
use std::io::{self, Read, Write};

const SNIFF_LENGTH: usize = 14;
const NONE: u32 = u32::MAX;

fn put32(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(&value.to_be_bytes());
}

#[derive(Default)]
struct Image {
    shared: SharedBuffer,
    header: Option<JpegDecoder>,
    parked: Option<RwBuffer>,
    frames: Option<JpegDecoder>,
}

impl Image {
    fn park(&mut self) {
        let parked = self.parked.get_or_insert_with(|| RwBuffer::new(self.shared.size()));
        for (start, end) in self.shared.segments_from(parked.size()) {
            let remaining = self.shared.size() - parked.size() - (end - start);
            parked.append(end - start, remaining);
        }
    }

    fn feed_frames(&mut self, all_received: bool) {
        self.park();
        let snapshot = self.parked.as_ref().expect("parked image").snapshot(&self.shared.snapshot().bytes);
        let size = self.header.as_ref().expect("header decoder").size;
        let frames = self.frames.get_or_insert_with(|| JpegDecoder::new(Some(size)));
        frames.set_data(Some(snapshot), all_received);
    }

    fn step(&mut self, prefix: &[u8], chunk: &[u8], all_received: bool, output: &mut Vec<u8>) {
        self.shared.append(chunk);
        if self.header.is_none() {
            if !all_received && prefix.len() < SNIFF_LENGTH {
                put32(output, 0);
                return;
            }
            if prefix.len() < 3 || prefix[..3] != [0xFF, 0xD8, 0xFF] {
                put32(output, 1);
                return;
            }
            self.header = Some(JpegDecoder::new(None));
        }
        put32(output, 2);
        let header = self.header.as_mut().expect("header decoder");
        header.set_data(Some(self.shared.snapshot()), all_received);
        let available = header.is_size_available();
        put32(output, available as u32);
        put32(output, header.failed as u32);
        if available {
            put32(output, header.size.0);
            put32(output, header.size.1);
            put32(output, header.orientation as u32);
            put32(output, header.density_corrected_size.0);
            put32(output, header.density_corrected_size.1);
            match metadata::icc_profile(&header.markers) {
                Some(profile) => {
                    put32(output, profile.len() as u32);
                    output.extend_from_slice(&profile);
                }
                None => put32(output, NONE),
            }
            self.feed_frames(all_received);
        } else if self.frames.is_some() {
            self.feed_frames(all_received);
        }
        put32(output, self.frames.is_some() as u32);
        let Some(frames) = self.frames.as_mut() else { return };
        if all_received {
            frames.frame_count();
        }
        frames.set_memory_allocator(true);
        frames.decode_frame_buffer_at_index();
        frames.set_memory_allocator(false);
        frames.set_data(None, false);
        put32(output, frames.failed as u32);
        let frame = &frames.frames[0];
        put32(output, frame.status as u32);
        put32(output, frame.has_alpha as u32);
        match &frame.pixels {
            Some(pixels) => {
                put32(output, frame.width);
                put32(output, frame.height);
                put32(output, pixels.len() as u32);
                output.extend_from_slice(pixels);
            }
            None => {
                put32(output, 0);
                put32(output, 0);
                put32(output, 0);
            }
        }
    }
}

fn take32(data: &[u8], offset: &mut usize) -> Option<u32> {
    let bytes = data.get(*offset..*offset + 4)?;
    *offset += 4;
    Some(u32::from_be_bytes(bytes.try_into().expect("four bytes")))
}

fn decode(input: &[u8]) -> Option<Vec<u8>> {
    let mut offset = 1;
    let count = take32(input, &mut offset)?;
    if count == 0 {
        return None;
    }
    let mut image = Image::default();
    let mut prefix = Vec::new();
    let mut output = Vec::new();
    for index in 0..count {
        let size = take32(input, &mut offset)? as usize;
        let chunk = input.get(offset..offset + size)?;
        prefix.extend_from_slice(chunk);
        offset += size;
        image.step(&prefix, chunk, index + 1 == count, &mut output);
    }
    (offset == input.len()).then_some(output)
}

fn main() -> io::Result<()> {
    let mut stdin = io::stdin().lock();
    let mut stdout = io::stdout().lock();
    let mut header = [0u8; 4];
    stdin.read_exact(&mut header)?;
    if &header != b"MGB1" {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "protocol header"));
    }
    stdout.write_all(b"MGB1")?;
    stdout.flush()?;
    loop {
        match stdin.read_exact(&mut header) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(error) => return Err(error),
        }
        let mut input = vec![0u8; u32::from_be_bytes(header) as usize];
        stdin.read_exact(&mut input)?;
        if input.first() != Some(&0) {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "unknown request"));
        }
        let output = decode(&input).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "malformed request"))?;
        stdout.write_all(&(output.len() as u32).to_be_bytes())?;
        stdout.write_all(&output)?;
        stdout.flush()?;
    }
}

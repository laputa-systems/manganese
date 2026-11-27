//! Pure-Rust GIF decoding mechanisms matching Chromium 156's Skia/Wuffs GIF
//! path. The decoder exposes the stream, palette, interlace and frame state a
//! caller needs; animation policy belongs in the caller.
#![expect(missing_docs, reason = "this snapshot stands alone as a decoder; its fields are read by adapters")]

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blend {
    Src,
    SrcOver,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disposal {
    Keep,
    Background,
    Previous,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    BadHeader,
    BadGraphicControl,
    BadLiteralWidth,
    BadCode,
    NotEnoughData,
    BadPalette,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::BadHeader => "bad header",
            Self::BadGraphicControl => "bad graphic control",
            Self::BadLiteralWidth => "bad literal width",
            Self::BadCode => "bad code",
            Self::NotEnoughData => "not enough data",
            Self::BadPalette => "bad palette",
        };
        formatter.write_str(name)
    }
}

impl std::error::Error for Error {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Complete,
    ShortRead,
    Error(Error),
}

#[derive(Clone, Debug)]
pub struct Frame {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub duration_ms: u32,
    pub disposal: Disposal,
    pub blend: Blend,
    pub transparent_index: Option<u8>,
    pub opaque_within_bounds: bool,
    pub interlace: bool,
    pub minimum_code_size: u8,
    pub data_start: usize,
    stream_width: u32,
    stream_height: u32,
    bad_literal_width: bool,
    palette: [[u8; 4]; 256],
}

impl Frame {
    pub fn right(&self) -> u32 {
        self.x.wrapping_add(self.width)
    }

    pub fn bottom(&self) -> u32 {
        self.y.wrapping_add(self.height)
    }
}

#[derive(Clone, Debug)]
pub struct Decoder {
    pub width: u32,
    pub height: u32,
    frame_width: u32,
    frame_height: u32,
    pub frames: Vec<Frame>,
    loop_count: Option<u16>,
}

const INTERLACE_START: [u32; 5] = [u32::MAX, 1, 2, 4, 0];
const INTERLACE_DELTA: [u32; 5] = [1, 2, 4, 8, 8];
const INTERLACE_COUNT: [u32; 5] = [0, 1, 2, 4, 8];

fn le16(data: &[u8], position: usize) -> Option<u16> {
    let bytes = data.get(position..position + 2)?;
    Some(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn skip_blocks(data: &[u8], mut position: usize) -> Option<usize> {
    loop {
        let size = *data.get(position)? as usize;
        position += 1;
        if size == 0 {
            return Some(position);
        }
        position = position.checked_add(size)?;
        if position > data.len() {
            return None;
        }
    }
}

fn read_blocks(data: &[u8], mut position: usize, output: &mut Vec<u8>) -> Option<usize> {
    loop {
        let size = *data.get(position)? as usize;
        position += 1;
        if size == 0 {
            return Some(position);
        }
        let end = position.checked_add(size)?;
        output.extend_from_slice(data.get(position..end)?);
        position = end;
    }
}

fn make_palette(bytes: &[u8]) -> [[u8; 4]; 256] {
    let mut palette = [[0, 0, 0, 255]; 256];
    for (index, entry) in palette.iter_mut().enumerate() {
        let offset = index * 3;
        if let Some(rgb) = bytes.get(offset..offset + 3) {
            *entry = [rgb[2], rgb[1], rgb[0], 255];
        }
    }
    palette
}

struct GraphicControl {
    disposal: Disposal,
    delay: u16,
    transparent: Option<u8>,
}

impl Decoder {
    /// Parse the image header and every frame configuration the input reveals.
    pub fn new(data: &[u8]) -> Result<Self, Status> {
        if data.len() < 6 {
            return Err(Status::ShortRead);
        }
        if &data[..3] != b"GIF" || (data[3] != b'8') || (data[4] != b'7' && data[4] != b'9') || data[5] != b'a' {
            return Err(Status::Error(Error::BadHeader));
        }
        let width = le16(data, 6).ok_or(Status::ShortRead)? as u32;
        let height = le16(data, 8).ok_or(Status::ShortRead)? as u32;
        let flags = *data.get(10).ok_or(Status::ShortRead)?;
        let _background = *data.get(11).ok_or(Status::ShortRead)?;
        let mut position = 13;
        let global = if flags & 0x80 != 0 {
            let count = 1usize << ((flags & 7) + 1);
            let bytes = data.get(position..position + 3 * count).ok_or(Status::ShortRead)?;
            position += 3 * count;
            make_palette(bytes)
        } else {
            [[0, 0, 0, 255]; 256]
        };
        let mut decoder = Self {
            width,
            height,
            frame_width: width,
            frame_height: height,
            frames: Vec::new(),
            loop_count: None,
        };
        let mut control: Option<GraphicControl> = None;
        let mut seen_frame = false;
        let mut seen_trailer = false;
        'scan: while let Some(&marker) = data.get(position) {
            position += 1;
            match marker {
                0x2c => {
                    let Some(x) = le16(data, position).map(u32::from) else { break 'scan };
                    let Some(y) = le16(data, position + 2).map(u32::from) else { break 'scan };
                    let Some(frame_width) = le16(data, position + 4).map(u32::from) else { break 'scan };
                    let Some(frame_height) = le16(data, position + 6).map(u32::from) else { break 'scan };
                    let Some(local_flags) = data.get(position + 8).copied() else { break 'scan };
                    position += 9;
                    let mut palette = global;
                    if local_flags & 0x80 != 0 {
                        let count = 1usize << ((local_flags & 7) + 1);
                        let Some(bytes) = data.get(position..position + 3 * count) else { break 'scan };
                        position += 3 * count;
                        palette = make_palette(bytes);
                    }
                    let right = x.wrapping_add(frame_width);
                    let bottom = y.wrapping_add(frame_height);
                    if decoder.frames.is_empty() {
                        decoder.frame_width = decoder.frame_width.max(right);
                        decoder.frame_height = decoder.frame_height.max(bottom);
                        decoder.width = decoder.frame_width;
                        decoder.height = decoder.frame_height;
                    }
                    let control = control.take().unwrap_or(GraphicControl {
                        disposal: Disposal::Keep,
                        delay: 0,
                        transparent: None,
                    });
                    if let Some(index) = control.transparent {
                        palette[index as usize] = [0, 0, 0, 0];
                    }
                    let Some(minimum) = data.get(position).copied() else { break 'scan };
                    let bad_literal_width = minimum > 8;
                    if bad_literal_width && !seen_frame {
                        return Err(Status::Error(Error::BadLiteralWidth));
                    }
                    position += 1;
                    seen_frame = true;
                    decoder.frames.push(Frame {
                        x: x.min(decoder.frame_width),
                        y: y.min(decoder.frame_height),
                        width: right.min(decoder.frame_width).saturating_sub(x.min(decoder.frame_width)),
                        height: bottom.min(decoder.frame_height).saturating_sub(y.min(decoder.frame_height)),
                        duration_ms: control.delay as u32 * 10,
                        disposal: control.disposal,
                        blend: Blend::SrcOver,
                        transparent_index: control.transparent,
                        opaque_within_bounds: control.transparent.is_none(),
                        interlace: local_flags & 0x40 != 0,
                        minimum_code_size: minimum.min(8),
                        data_start: position,
                        stream_width: frame_width,
                        stream_height: frame_height,
                        bad_literal_width,
                        palette,
                    });
                    let Some(next) = skip_blocks(data, position) else { break 'scan };
                    position = next;
                }
                0x21 => {
                    let Some(label) = data.get(position).copied() else { break 'scan };
                    position += 1;
                    if label == 0xf9 {
                        if data.get(position) != Some(&4) {
                            if !seen_frame { return Err(Status::Error(Error::BadGraphicControl)); }
                            break 'scan;
                        }
                        let Some(packed) = data.get(position + 1).copied() else { break 'scan };
                        let Some(delay) = le16(data, position + 2) else { break 'scan };
                        let Some(index) = data.get(position + 4).copied() else { break 'scan };
                        if data.get(position + 5) != Some(&0) {
                            if !seen_frame { return Err(Status::Error(Error::BadGraphicControl)); }
                            break 'scan;
                        }
                        position += 6;
                        let raw = (packed >> 2) & 7;
                        control = Some(GraphicControl {
                            disposal: if raw == 2 {
                                Disposal::Background
                            } else if raw == 3 || raw == 4 {
                                Disposal::Previous
                            } else {
                                Disposal::Keep
                            },
                            delay,
                            transparent: if packed & 1 != 0 { Some(index) } else { None },
                        });
                    } else if label == 0xff {
                        let Some(size) = data.get(position).copied() else { break 'scan };
                        let size = size as usize;
                        position += 1;
                        let Some(identifier) = data.get(position..position + size) else { break 'scan };
                        let netscape = identifier == b"NETSCAPE2.0" || identifier == b"ANIMEXTS1.0";
                        position += size;
                        if netscape && size == 11 {
                            let Some(sub) = data.get(position).copied() else { break 'scan };
                            let sub = sub as usize;
                            position += 1;
                            if sub == 3 && data.get(position) == Some(&1) {
                                decoder.loop_count = le16(data, position + 1);
                                position += 3;
                            } else {
                                position = position.saturating_add(sub);
                            }
                        }
                        let Some(next) = skip_blocks(data, position) else { break 'scan };
                        position = next;
                    } else {
                        let Some(next) = skip_blocks(data, position) else { break 'scan };
                        position = next;
                    }
                }
                0x3b => { seen_trailer = true; break },
                _ => {
                    if !seen_frame && !seen_trailer { return Err(Status::ShortRead); }
                    break;
                }
            }
        }
        if !seen_frame && !seen_trailer {
            return Err(Status::ShortRead);
        }
        if decoder.width == 0 || decoder.height == 0 {
            return Err(Status::Error(Error::BadHeader));
        }
        Ok(decoder)
    }

    /// Wuffs' number of animation loops before Skia's repetition conversion.
    pub fn num_animation_loops(&self) -> u32 {
        match self.loop_count {
            Some(0) => 0,
            Some(value) => u32::from(value) + 1,
            None if self.frames.len() > 1 => 1,
            None => 0,
        }
    }

    /// Decode one frame's LZW stream and paint it into a BGRA image buffer.
    pub fn decode_frame(
        &self,
        data: &[u8],
        index: usize,
        destination: &mut [u8],
        row_bytes: usize,
        blend: Blend,
    ) -> Status {
        let Some(frame) = self.frames.get(index) else {
            return Status::Error(Error::BadCode);
        };
        let right = frame.right();
        let bottom = frame.bottom();
        if frame.stream_width == 0 || frame.stream_height == 0 || bottom <= frame.y {
            return Status::Complete;
        }
        if frame.bad_literal_width {
            return Status::Error(Error::BadLiteralWidth);
        }
        let mut compressed = Vec::new();
        let mut position = frame.data_start;
        let truncated = read_blocks(data, position, &mut compressed).is_none();
        position = frame.data_start;
        let _ = position;
        let lzw = lzw(&compressed, frame.minimum_code_size);
        let bad_code = lzw.bad_code;
        let indices = lzw.indices;
        let mut x = frame.x;
        let mut y = frame.y;
        let mut interlace = if frame.interlace { 4usize } else { 0usize };
        let wrap_right = frame.x.saturating_add(frame.stream_width);
        let mut consumed = 0usize;
        for &index_value in &indices {
            consumed += 1;
            if y >= bottom {
                if consumed >= required_pixels(frame, interlace) {
                    return Status::Complete;
                }
                break;
            }
            let row = y as usize * row_bytes;
            let start = row + (x as usize * 4);
            let end = row + (right.min(self.width) as usize * 4);
            if frame.transparent_index != Some(index_value) && start + 4 <= destination.len() && start < end {
                let color = frame.palette[index_value as usize];
                if color[3] != 0 || blend == Blend::Src {
                    destination[start..start + 4].copy_from_slice(&color);
                }
            }
            x = x.wrapping_add(1);
            if wrap_right <= x {
                x = frame.x;
                if interlace == 0 {
                    y = y.wrapping_add(1);
                    continue;
                }
                if index == 0 && frame.transparent_index.is_none() && interlace > 1 {
                    let source_row = y as usize * row_bytes;
                    let last = (y + INTERLACE_COUNT[interlace]).min(bottom).min(self.height);
                    let mut replicate = y.wrapping_add(1);
                    while replicate < last {
                        let from = source_row..source_row + (self.width as usize * 4);
                        let to = replicate as usize * row_bytes;
                        if from.end <= destination.len() && to + (self.width as usize * 4) <= destination.len() {
                            destination.copy_within(from, to);
                        }
                        replicate = replicate.wrapping_add(1);
                    }
                }
                y = y.wrapping_add(INTERLACE_DELTA[interlace]);
                while interlace > 0 && y >= bottom {
                    interlace -= 1;
                    y = frame.y.wrapping_add(INTERLACE_START[interlace]);
                }
            }
        }
        let complete = bottom <= frame.y || consumed >= required_pixels(frame, interlace);
        if bad_code {
            Status::Error(Error::BadCode)
        } else if truncated || lzw.truncated {
            Status::ShortRead
        } else if complete {
            Status::Complete
        } else {
            Status::Error(Error::NotEnoughData)
        }
    }
}

fn required_pixels(frame: &Frame, interlace: usize) -> usize {
    if frame.stream_width == 0 || frame.stream_height == 0 {
        return 0;
    }
    let bottom = frame.y.wrapping_add(frame.stream_height);
    if interlace == 0 && !frame.interlace {
        return frame.stream_width as usize * frame.stream_height as usize;
    }
    let mut rows = 0usize;
    for start in [0u32, 4, 2, 1] {
        let mut y = frame.y.wrapping_add(start);
        while y < bottom {
            rows += 1;
            y = y.wrapping_add(if start == 0 || start == 4 { 8 } else if start == 2 { 4 } else { 2 });
        }
    }
    rows * frame.stream_width as usize
}

struct LzwOutput {
    indices: Vec<u8>,
    truncated: bool,
    bad_code: bool,
}

fn lzw(data: &[u8], minimum: u8) -> LzwOutput {
    let clear = 1usize << minimum;
    let end = clear + 1;
    let mut entries: Vec<Vec<u8>> = (0..clear + 2).map(|value| vec![(value & 255) as u8]).collect();
    let mut width = (minimum + 1) as u32;
    let mut bits = 0u32;
    let mut available = 0u32;
    let mut byte = 0usize;
    let mut save = clear + 1;
    let mut previous: Option<Vec<u8>> = None;
    let mut output = Vec::new();
    loop {
        while available < width {
            let Some(&value) = data.get(byte) else {
                return LzwOutput { indices: output, truncated: true, bad_code: false };
            };
            byte += 1;
            bits |= (value as u32) << available;
            available += 8;
        }
        let mask = (1u32 << width) - 1;
        let code = (bits & mask) as usize;
        bits >>= width;
        available -= width;
        if code == clear {
            entries.truncate(clear + 2);
            save = clear + 1;
            width = (minimum + 1) as u32;
            previous = None;
            continue;
        }
        if code == end {
            return LzwOutput { indices: output, truncated: false, bad_code: false };
        }
        let value = if code < entries.len() {
            entries[code].clone()
        } else if code == save && previous.is_some() {
            let first = previous.as_ref().expect("previous entry")[0];
            let mut entry = previous.clone().expect("previous entry");
            entry.push(first);
            entry
        } else {
            return LzwOutput { indices: output, truncated: false, bad_code: true };
        };
        output.extend_from_slice(&value);
        if save < 4096 {
            let mut entry = previous.clone().unwrap_or_default();
            entry.push(value[0]);
            if save < entries.len() {
                entries[save] = entry;
            } else if entries.len() < 4096 {
                entries.push(entry);
            }
        }
        save += 1;
        if width < 12 && (save >> width) & 1 != 0 {
            width += 1;
        }
        previous = Some(value);
    }
}

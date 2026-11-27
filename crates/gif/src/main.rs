use img_gif::{Blend, Decoder as GifDecoder, Disposal, Frame as GifFrame, Status};
use std::collections::HashSet;
use std::io::{self, Read, Write};

const EMPTY: u32 = 0;
const INITIALIZED: u32 = 1;
const PARTIAL: u32 = 2;
const COMPLETE: u32 = 3;
const MAX_RECORD: usize = 64 * 1024 * 1024;

fn put32(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(&value.to_be_bytes());
}

fn take32(input: &[u8], offset: &mut usize) -> Option<u32> {
    let bytes = input.get(*offset..*offset + 4)?;
    *offset += 4;
    Some(u32::from_be_bytes(bytes.try_into().expect("four bytes")))
}

#[derive(Clone, Default, PartialEq, Eq)]
struct Rect {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

impl Rect {
    fn right(&self) -> u32 {
        self.x.saturating_add(self.width)
    }

    fn bottom(&self) -> u32 {
        self.y.saturating_add(self.height)
    }

    fn screen(width: u32, height: u32) -> Self {
        Self { x: 0, y: 0, width, height }
    }

    fn intersect(mut self, screen: &Self) -> Self {
        let right = self.right().min(screen.right());
        let bottom = self.bottom().min(screen.bottom());
        self.x = self.x.max(screen.x).min(right);
        self.y = self.y.max(screen.y).min(bottom);
        self.width = right - self.x;
        self.height = bottom - self.y;
        self
    }

    fn contains(&self, other: &Self) -> bool {
        !self.is_empty()
            && !other.is_empty()
            && other.x >= self.x
            && other.y >= self.y
            && other.right() <= self.right()
            && other.bottom() <= self.bottom()
    }

    fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }
}

#[derive(Default)]
struct FrameState {
    status: u32,
    has_alpha: bool,
    expected_alpha: bool,
    duration: i32,
    required: i32,
    disposal: u32,
    blend: u32,
    rect: Rect,
    pixels: Vec<u8>,
    allocated: bool,
}

#[derive(Default)]
struct Decoder {
    failed: bool,
    size_available: bool,
    all_received: bool,
    width: u32,
    height: u32,
    frames: Vec<FrameState>,
    failed_frames: HashSet<usize>,
    already_started: Option<usize>,
    prior_frame: i32,
    repetition_count: i32,
}

fn frame_rect(frame: &GifFrame) -> Rect {
    Rect { x: frame.x, y: frame.y, width: frame.width, height: frame.height }
}

fn disposal_code(disposal: Disposal) -> u32 {
    match disposal {
        Disposal::Keep => 0,
        Disposal::Background => 1,
        Disposal::Previous => 2,
    }
}

fn initialize_frame(index: usize, decoder: &GifDecoder, frames: &[FrameState], width: u32, height: u32) -> FrameState {
    let metadata = &decoder.frames[index];
    let screen = Rect::screen(width, height);
    let rect = frame_rect(metadata).intersect(&screen);
    let reports_alpha = !metadata.opaque_within_bounds;
    let blend_source = metadata.blend == Blend::Src;
    let mut state = FrameState {
        duration: metadata.duration_ms as i32,
        disposal: disposal_code(metadata.disposal),
        blend: u32::from(blend_source),
        rect: frame_rect(metadata),
        has_alpha: true,
        expected_alpha: reports_alpha,
        required: -1,
        ..FrameState::default()
    };
    if index == 0 {
        state.expected_alpha = reports_alpha || rect != screen;
        return state;
    }
    if (!reports_alpha || blend_source) && rect == screen {
        state.expected_alpha = reports_alpha;
        return state;
    }
    let mut previous = index - 1;
    loop {
        if decoder.frames[previous].disposal != Disposal::Previous {
            break;
        }
        if previous == 0 {
            state.expected_alpha = true;
            return state;
        }
        previous -= 1;
    }
    let clear_previous = decoder.frames[previous].disposal == Disposal::Background;
    let previous_rect = frame_rect(&decoder.frames[previous]).intersect(&screen);
    if clear_previous && (previous_rect == screen || frames[previous].required == -1) {
        state.has_alpha = true;
        return state;
    }
    if reports_alpha && !blend_source {
        state.required = previous as i32;
        state.expected_alpha = frames[previous].expected_alpha || clear_previous;
        return state;
    }
    let mut candidate_rect = previous_rect;
    loop {
        if !rect.contains(&candidate_rect) {
            break;
        }
        if frames[previous].required == -1 {
            state.required = -1;
            state.expected_alpha = true;
            return state;
        }
        previous = frames[previous].required as usize;
        candidate_rect = frame_rect(&decoder.frames[previous]).intersect(&screen);
    }
    state.required = previous as i32;
    state.expected_alpha = if clear_previous {
        true
    } else {
        frames[previous].expected_alpha || (reports_alpha && blend_source)
    };
    state
}

impl Decoder {
    fn set_failed(&mut self) {
        self.failed = true;
    }

    fn decode_frame_count(&self) -> u32 {
        self.frames.len() as u32
    }

    fn set_failed_frame(&mut self, index: usize) {
        self.failed_frames.insert(index);
        if self.failed_frames.len() as u32 == self.decode_frame_count() {
            self.set_failed();
        }
    }

    fn viable_reference(&self, dependent: usize) -> Option<usize> {
        let required = self.frames[dependent].required as usize;
        if required >= dependent {
            return None;
        }
        let mut index = dependent;
        while index != required {
            index -= 1;
            let frame = &self.frames[index];
            if frame.disposal == 2 {
                continue;
            }
            if frame.status == COMPLETE {
                return Some(index);
            }
        }
        None
    }

    fn decode(&mut self, decoder: &GifDecoder, data: &[u8], index: usize) {
        if self.failed || self.failed_frames.contains(&index) || index >= self.frames.len() {
            return;
        }
        if self.frames[index].status == COMPLETE {
            return;
        }
        if self.frames[index].status == EMPTY {
            self.already_started = None;
            if self.frames[index].required < 0 {
                let bytes = u64::from(self.width) * u64::from(self.height) * 4;
                if bytes > 64 << 20 {
                    self.set_failed_frame(index);
                    return;
                }
                self.frames[index].pixels = vec![0; bytes as usize];
                self.frames[index].allocated = true;
                self.frames[index].status = INITIALIZED;
                self.prior_frame = -1;
            } else {
                let Some(previous) = self.viable_reference(index) else {
                    self.set_failed_frame(index);
                    return;
                };
                if self.failed_frames.contains(&previous) {
                    self.set_failed_frame(index);
                    return;
                }
                self.frames[index].pixels = self.frames[previous].pixels.clone();
                if self.frames[previous].disposal == 1 {
                    let rect = self.frames[previous].rect.clone();
                    for y in rect.y..rect.y.saturating_add(rect.height).min(self.height) {
                        let start = (y as usize * self.width as usize + rect.x as usize) * 4;
                        let end = (y as usize * self.width as usize + rect.right().min(self.width) as usize) * 4;
                        if end <= self.frames[index].pixels.len() {
                            self.frames[index].pixels[start..end].fill(0);
                        }
                    }
                }
                self.frames[index].allocated = true;
                self.frames[index].status = INITIALIZED;
                self.prior_frame = previous as i32;
            }
        }
        let started = self.already_started == Some(index);
        if !started {
            let blend = if index == 0 || self.frames[index].required < 0 {
                Blend::Src
            } else {
                Blend::SrcOver
            };
            let mut pixels = std::mem::take(&mut self.frames[index].pixels);
            let result = decoder.decode_frame(data, index, &mut pixels, self.width as usize * 4, blend);
            self.frames[index].pixels = pixels;
            match result {
                Status::Complete => {
                    self.frames[index].status = COMPLETE;
                    self.frames[index].has_alpha = self.frames[index].expected_alpha;
                    self.already_started = None;
                }
                Status::ShortRead => {
                    self.frames[index].status = PARTIAL;
                    self.already_started = None;
                    if self.all_received {
                        self.set_failed_frame(index);
                    }
                }
                Status::Error(_) => {
                    self.frames[index].status = PARTIAL;
                    self.already_started = None;
                    self.set_failed_frame(index);
                }
            }
        }
    }

    fn step(&mut self, prefix: &[u8], all_received: bool, output: &mut Vec<u8>) {
        self.all_received = all_received;
        let decoder = match GifDecoder::new(prefix) {
            Ok(decoder) => decoder,
            Err(Status::ShortRead) | Err(Status::Complete) => {
                if all_received {
                    self.set_failed();
                }
                self.emit(output);
                return;
            }
            Err(Status::Error(_)) => {
                self.set_failed();
                self.emit(output);
                return;
            }
        };
        let width = decoder.width;
        let height = decoder.height;
        let total = u64::from(width) * u64::from(height) * 4;
        if width > 1_000_000 || height > 1_000_000 || total > 0x7fff_ffff {
            self.set_failed();
            self.emit(output);
            return;
        }
        self.width = width;
        self.height = height;
        self.size_available = true;
        let count = decoder.frames.len();
        if self.frames.len() != count {
            self.frames.clear();
            for index in 0..count {
                let state = initialize_frame(index, &decoder, &self.frames, width, height);
                self.frames.push(state);
            }
        }
        let loops = decoder.num_animation_loops();
        let skia = if loops == 0 {
            -1
        } else {
            ((loops - 1).min(i32::MAX as u32 - 1)) as i32
        };
        if skia == 0 {
            self.repetition_count = if count > 1 { 0 } else if all_received && count == 1 { -2 } else { 0 };
        } else {
            self.repetition_count = skia;
        }
        if !self.failed {
            for index in 0..count {
                self.decode(&decoder, prefix, index);
            }
        }
        self.emit(output);
    }

    fn emit(&self, output: &mut Vec<u8>) {
        let available = self.size_available && !self.failed;
        put32(output, available as u32);
        put32(output, self.failed as u32);
        put32(output, if available { self.width } else { 0 });
        put32(output, if available { self.height } else { 0 });
        put32(output, self.frames.len() as u32);
        put32(output, if self.failed { 0 } else { self.repetition_count as u32 });
        for frame in &self.frames {
            put32(output, frame.status);
            put32(output, frame.has_alpha as u32);
            put32(output, frame.duration as u32);
            put32(output, frame.required as u32);
            put32(output, frame.disposal);
            put32(output, frame.blend);
            put32(output, frame.rect.x);
            put32(output, frame.rect.y);
            put32(output, frame.rect.width);
            put32(output, frame.rect.height);
            put32(output, if frame.allocated { frame.pixels.len() as u32 } else { 0 });
            if frame.allocated {
                output.extend_from_slice(&frame.pixels);
            }
        }
    }
}

fn decode_request(input: &[u8]) -> Result<Vec<u8>, &'static str> {
    let mut offset = 1;
    let count = take32(input, &mut offset).ok_or("short record")?;
    let mut data = Vec::new();
    let mut output = Vec::new();
    for index in 0..count {
        let size = take32(input, &mut offset).ok_or("short chunk")? as usize;
        let chunk = input.get(offset..offset.checked_add(size).ok_or("length")?).ok_or("short chunk")?;
        offset += size;
        data.extend_from_slice(chunk);
        let mut decoder = Decoder::default();
        decoder.step(&data, index + 1 == count, &mut output);
    }
    if offset != input.len() {
        return Err("trailing input");
    }
    Ok(output)
}

fn main() -> io::Result<()> {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let mut header = [0; 4];
    input.read_exact(&mut header)?;
    if &header != b"MGB1" {
        return Err(io::Error::other("invalid protocol"));
    }
    output.write_all(b"MGB1")?;
    output.flush()?;
    loop {
        if input.read(&mut header[..1])? == 0 {
            break;
        }
        input.read_exact(&mut header[1..])?;
        let length = u32::from_be_bytes(header) as usize;
        if length == 0 || length > MAX_RECORD {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "bad record length"));
        }
        let mut bytes = vec![0; length];
        input.read_exact(&mut bytes)?;
        if bytes.first() != Some(&0) {
            return Err(io::Error::other("invalid operation"));
        }
        let result = decode_request(&bytes).map_err(io::Error::other)?;
        output.write_all(&(result.len() as u32).to_be_bytes())?;
        output.write_all(&result)?;
        output.flush()?;
    }
    Ok(())
}

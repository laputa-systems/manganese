use crate::codec::{Blend, Code, Codec, Disposal, Target, NO_FRAME};
use crate::swizzle::DstAlpha;
use std::collections::BTreeSet;

const LOOP_ONCE: i32 = 0;
const LOOP_INFINITE: i32 = -1;
const NONE: i32 = -2;
const MAX_FRAME_BYTES: u64 = 64 << 20;
const MAX_SIDE: u32 = 1_000_000;

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum Reception {
    Partial,
    Complete,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Status {
    Empty = 0,
    Initialized = 1,
    Partial = 2,
    Complete = 3,
}

struct Slot {
    status: Status,
    has_alpha: bool,
    duration: i32,
    required: i32,
    disposal: u32,
    blend: u32,
    rect: [i32; 4],
    pixels: Option<Vec<u8>>,
}

pub struct Decoder {
    codec: Option<Codec>,
    failed: bool,
    size_available: bool,
    all_received: bool,
    width: i32,
    height: i32,
    frames: Vec<Slot>,
    failed_frames: BTreeSet<usize>,
    already_started: Option<usize>,
    prior_frame: i32,
    repetition_count: i32,
}

fn put(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(&value.to_be_bytes());
}

impl Decoder {
    fn new(all_received: bool) -> Self {
        Self {
            codec: None,
            failed: false,
            size_available: false,
            all_received,
            width: 0,
            height: 0,
            frames: Vec::new(),
            failed_frames: BTreeSet::new(),
            already_started: None,
            prior_frame: NO_FRAME,
            repetition_count: LOOP_ONCE,
        }
    }

    fn set_failed(&mut self) {
        self.codec = None;
        self.failed = true;
    }

    fn decode_frame_count(&mut self) -> usize {
        match self.codec.as_mut() {
            Some(codec) => codec.frame_count() as usize,
            None => self.frames.len(),
        }
    }

    fn set_failed_frame(&mut self, index: usize) {
        self.failed_frames.insert(index);
        if self.failed_frames.len() == self.decode_frame_count() {
            self.set_failed();
        }
    }

    fn frame_is_received(&self, index: usize) -> bool {
        self.all_received || self.codec.as_ref().and_then(|codec| codec.frame_info(index as i32)).is_some_and(|info| info.fully_received)
    }

    fn frame_count(&mut self) -> usize {
        let old = self.frames.len();
        let new = self.decode_frame_count();
        if old != new {
            self.frames.truncate(new);
            for index in old..new {
                let info = self.codec.as_ref().and_then(|codec| codec.frame_info(index as i32)).expect("frame info");
                self.frames.push(Slot {
                    status: Status::Empty,
                    has_alpha: true,
                    duration: info.duration,
                    required: info.required,
                    disposal: match info.disposal {
                        Disposal::Keep => 0,
                        Disposal::RestoreBackground => 1,
                        Disposal::RestorePrevious => 2,
                    },
                    blend: u32::from(info.blend == Blend::Src),
                    rect: [info.rect.left, info.rect.top, info.rect.width(), info.rect.height()],
                    pixels: None,
                });
            }
        }
        new
    }

    fn viable_reference(&self, dependent: usize) -> Option<usize> {
        let required = self.frames[dependent].required as usize;
        let mut index = dependent - 1;
        while index != required {
            let frame = &self.frames[index];
            if frame.disposal != 2 && frame.status == Status::Complete {
                return Some(index);
            }
            index -= 1;
        }
        None
    }

    fn decode(&mut self, index: usize) {
        let mut stack = vec![(index, None::<usize>)];
        while let Some((current, mut previous)) = stack.pop() {
            if self.codec.is_none() || self.failed_frames.contains(&current) {
                continue;
            }
            if self.frames[current].status == Status::Complete {
                continue;
            }
            if self.frames[current].status == Status::Empty {
                self.already_started = None;
                let required = self.frames[current].required;
                if required < 0 {
                    let bytes = u64::from(self.width as u32) * u64::from(self.height as u32) * 4;
                    if bytes > MAX_FRAME_BYTES {
                        self.set_failed_frame(current);
                        continue;
                    }
                    let frame = &mut self.frames[current];
                    frame.pixels = Some(vec![0; bytes as usize]);
                    frame.status = Status::Initialized;
                    self.prior_frame = NO_FRAME;
                } else {
                    if previous.is_none() {
                        previous = self.viable_reference(current);
                        if previous.is_none() {
                            stack.push((current, Some(required as usize)));
                            stack.push((required as usize, None));
                            continue;
                        }
                    }
                    let previous = previous.expect("previous frame");
                    if self.failed_frames.contains(&previous) {
                        self.set_failed_frame(current);
                        continue;
                    }
                    let pixels = self.frames[previous].pixels.clone();
                    let frame = &mut self.frames[current];
                    frame.pixels = pixels;
                    frame.status = Status::Initialized;
                    self.prior_frame = previous as i32;
                }
            }
            let started = self.already_started == Some(current);
            let (width, height) = (self.width, self.height);
            let codec = self.codec.as_mut().expect("codec");
            let opaque = codec.frame_info(current as i32).expect("frame info").opaque;
            let target = Target { alpha: if opaque { DstAlpha::Opaque } else { DstAlpha::Premul }, width, height };
            if !started {
                self.already_started = None;
                let result = match self.frames[current].pixels.as_mut() {
                    Some(pixels) => codec.start_incremental_decode(target, pixels, current as i32, self.prior_frame),
                    None => Code::InvalidParameters,
                };
                match result {
                    Code::Success => {}
                    Code::IncompleteInput => continue,
                    _ => {
                        self.set_failed_frame(current);
                        continue;
                    }
                }
                self.frames[current].status = Status::Partial;
                self.already_started = Some(current);
            }
            let pixels = self.frames[current].pixels.as_mut().expect("pixels");
            let result = codec.incremental_decode(pixels, target);
            match result {
                Code::Success => {
                    self.already_started = None;
                    let info = self.codec.as_ref().expect("codec").frame_info(current as i32).expect("frame info");
                    let frame = &mut self.frames[current];
                    frame.has_alpha = !info.opaque;
                    frame.status = Status::Complete;
                }
                Code::IncompleteInput => {
                    if self.frame_is_received(current) {
                        self.set_failed_frame(current);
                    }
                }
                _ => {
                    self.already_started = None;
                    self.set_failed_frame(current);
                }
            }
        }
    }

    fn decode_frame_at(&mut self, index: usize) {
        if index >= self.frame_count() {
            return;
        }
        if self.frames[index].status != Status::Complete {
            self.decode(index);
        }
    }

    fn repetition(&mut self) -> i32 {
        let Some(codec) = self.codec.as_mut() else { return self.repetition_count };
        let count = codec.repetition_count();
        self.repetition_count = match count {
            0 => {
                if codec.is_animated() {
                    LOOP_ONCE
                } else {
                    NONE
                }
            }
            -1 => LOOP_INFINITE,
            count => count,
        };
        self.repetition_count
    }
}

pub fn step(prefix: &[u8], reception: Reception, output: &mut Vec<u8>) {
    let all_received = reception == Reception::Complete;
    let mut decoder = Decoder::new(all_received);
    match Codec::make(prefix) {
        Ok(codec) => {
            let info = *codec.encoded();
            let (width, height) = (info.width as u32, info.height as u32);
            if width > MAX_SIDE || height > MAX_SIDE || u64::from(width) * u64::from(height) * 4 > 0x7fff_ffff {
                decoder.set_failed();
            } else {
                decoder.width = info.width;
                decoder.height = info.height;
                decoder.size_available = true;
                decoder.codec = Some(codec);
            }
        }
        Err(Code::IncompleteInput) => {
            if all_received {
                decoder.set_failed();
            }
        }
        Err(_) => decoder.set_failed(),
    }
    if decoder.size_available && !decoder.failed {
        let mut index = 0;
        while index < decoder.frame_count() {
            decoder.decode_frame_at(index);
            index += 1;
        }
    }
    let repetition = if decoder.size_available { decoder.repetition() } else { decoder.repetition_count };
    let available = decoder.size_available && !decoder.failed;
    put(output, u32::from(available));
    put(output, u32::from(decoder.failed));
    put(output, if available { decoder.width as u32 } else { 0 });
    put(output, if available { decoder.height as u32 } else { 0 });
    put(output, decoder.frames.len() as u32);
    put(output, repetition as u32);
    for frame in &decoder.frames {
        put(output, frame.status as u32);
        put(output, u32::from(frame.has_alpha));
        put(output, frame.duration as u32);
        put(output, frame.required as u32);
        put(output, frame.disposal);
        put(output, frame.blend);
        for value in frame.rect {
            put(output, value as u32);
        }
        match &frame.pixels {
            Some(pixels) => {
                put(output, pixels.len() as u32);
                output.extend_from_slice(pixels);
            }
            None => put(output, 0),
        }
    }
}

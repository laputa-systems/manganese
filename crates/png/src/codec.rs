use crate::stream::Stream;
use crate::swizzle::{self, Alpha, Color, DstAlpha, EncodedInfo, Swizzler};
use img_png::chromium::{Outcome, Reader};

pub const NO_FRAME: i32 = -1;
const UNINITIALIZED: i32 = -2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Code {
    Success,
    IncompleteInput,
    ErrorInInput,
    InvalidConversion,
    InvalidParameters,
    InvalidInput,
    InternalError,
    Unimplemented,
}

fn code_of(outcome: Outcome) -> Code {
    match outcome {
        Outcome::Success => Code::Success,
        Outcome::FormatError => Code::ErrorInInput,
        Outcome::ParameterError => Code::InvalidParameters,
        Outcome::OtherIoError | Outcome::LimitsExceeded => Code::InternalError,
        Outcome::IncompleteInput => Code::IncompleteInput,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Disposal {
    Keep,
    RestoreBackground,
    RestorePrevious,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Blend {
    Src,
    SrcOver,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub fn xywh(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { left: x, top: y, right: x.wrapping_add(w), bottom: y.wrapping_add(h) }
    }

    pub fn width(&self) -> i32 {
        self.right.wrapping_sub(self.left)
    }

    pub fn height(&self) -> i32 {
        self.bottom.wrapping_sub(self.top)
    }

    fn is_empty(&self) -> bool {
        let w = i64::from(self.right) - i64::from(self.left);
        let h = i64::from(self.bottom) - i64::from(self.top);
        w <= 0 || h <= 0 || i32::try_from(w | h).is_err()
    }

    fn intersect(&self, other: &Rect) -> Option<Rect> {
        let result = Rect {
            left: self.left.max(other.left),
            top: self.top.max(other.top),
            right: self.right.min(other.right),
            bottom: self.bottom.min(other.bottom),
        };
        (!result.is_empty()).then_some(result)
    }

    fn contains(&self, other: &Rect) -> bool {
        !other.is_empty() && !self.is_empty() && self.left <= other.left && self.top <= other.top && self.right >= other.right && self.bottom >= other.bottom
    }
}

fn on_screen(rect: Rect, screen: Rect) -> Rect {
    rect.intersect(&screen).unwrap_or(Rect { left: 0, top: 0, right: 0, bottom: 0 })
}

#[derive(Clone, Debug)]
pub struct Frame {
    id: i32,
    has_alpha: bool,
    required: i32,
    rect: Rect,
    disposal: Disposal,
    duration: i32,
    blend: Blend,
    reported: Alpha,
    fully_received: bool,
}

impl Frame {
    fn new(id: i32, reported: Alpha) -> Self {
        Self {
            id,
            has_alpha: false,
            required: UNINITIALIZED,
            rect: Rect { left: 0, top: 0, right: 0, bottom: 0 },
            disposal: Disposal::Keep,
            duration: 0,
            blend: Blend::SrcOver,
            reported,
            fully_received: false,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct FrameInfo {
    pub required: i32,
    pub duration: i32,
    pub fully_received: bool,
    pub opaque: bool,
    pub disposal: Disposal,
    pub blend: Blend,
    pub rect: Rect,
}

struct Holder {
    frames: Vec<Frame>,
    screen: Rect,
}

impl Holder {
    fn frame(&self, index: i32) -> Option<&Frame> {
        usize::try_from(index).ok().and_then(|index| self.frames.get(index))
    }

    fn set_alpha_and_required(&self, frame: &mut Frame) {
        let reports_alpha = frame.reported != Alpha::Opaque;
        let frame_rect = on_screen(frame.rect, self.screen);
        let i = frame.id;
        if i == 0 {
            frame.has_alpha = reports_alpha || frame_rect != self.screen;
            frame.required = NO_FRAME;
            return;
        }
        let blend_with_previous = frame.blend == Blend::SrcOver;
        if (!reports_alpha || !blend_with_previous) && frame_rect == self.screen {
            frame.has_alpha = reports_alpha;
            frame.required = NO_FRAME;
            return;
        }
        let mut previous = self.frame(i - 1).expect("previous frame");
        while previous.disposal == Disposal::RestorePrevious {
            let id = previous.id;
            if id == 0 {
                frame.has_alpha = true;
                frame.required = NO_FRAME;
                return;
            }
            previous = self.frame(id - 1).expect("earlier frame");
        }
        let clear_previous = previous.disposal == Disposal::RestoreBackground;
        let mut previous_rect = on_screen(previous.rect, self.screen);
        if clear_previous && (previous_rect == self.screen || previous.required == NO_FRAME) {
            frame.has_alpha = true;
            frame.required = NO_FRAME;
            return;
        }
        if reports_alpha && blend_with_previous {
            frame.required = previous.id;
            frame.has_alpha = previous.has_alpha || clear_previous;
            return;
        }
        while frame_rect.contains(&previous_rect) {
            let previous_required = previous.required;
            if previous_required == NO_FRAME {
                frame.required = NO_FRAME;
                frame.has_alpha = true;
                return;
            }
            previous = self.frame(previous_required).expect("required frame");
            previous_rect = on_screen(previous.rect, self.screen);
        }
        frame.required = previous.id;
        if previous.disposal == Disposal::RestoreBackground {
            frame.has_alpha = true;
            return;
        }
        frame.has_alpha = previous.has_alpha || (reports_alpha && !blend_with_previous);
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Target {
    pub alpha: DstAlpha,
    pub width: i32,
    pub height: i32,
}

impl Target {
    fn row_bytes(&self) -> usize {
        self.width as usize * 4
    }
}

struct DstSpan {
    stride: usize,
    start: usize,
    end: usize,
    row_size: usize,
    row_count: usize,
}

struct State {
    frame_index: i32,
    stride: usize,
    swizzler: Swizzler,
    encoded_row_bytes: usize,
    dst: Option<DstSpan>,
    preblend: Vec<u8>,
    rows_written: usize,
}

pub struct Codec {
    info: EncodedInfo,
    stream: Stream,
    reader: Reader<Stream>,
    frame_at_current: i32,
    at_start_of_data: bool,
    max_length_seen: Option<usize>,
    incremental: Option<State>,
    holder: Holder,
    can_parse_more: bool,
    started: bool,
}

fn to_info(reader: &Reader<Stream>) -> Option<EncodedInfo> {
    let info = reader.info();
    let (color_type, depth) = reader.output_color_type();
    let bits = depth as u8;
    let sbit = info.sbit.as_deref();
    let color = match color_type {
        png_codec::ColorType::Grayscale => Color::Gray,
        png_codec::ColorType::Rgb => match sbit {
            Some([5, 6, 5]) if bits == 8 => Color::Rgb565,
            _ => Color::Rgb,
        },
        png_codec::ColorType::GrayscaleAlpha => match sbit {
            Some([1, 8]) if bits == 8 => Color::XAlpha,
            _ => Color::GrayAlpha,
        },
        png_codec::ColorType::Rgba => Color::Rgba,
        png_codec::ColorType::Indexed => Color::Palette,
    };
    let alpha = match color_type {
        png_codec::ColorType::Grayscale | png_codec::ColorType::Rgb => Alpha::Opaque,
        png_codec::ColorType::GrayscaleAlpha | png_codec::ColorType::Rgba => Alpha::Unpremul,
        png_codec::ColorType::Indexed => if info.trns.is_some() { Alpha::Unpremul } else { Alpha::Opaque },
    };
    const MAX_SIZE: u32 = 1_000_000;
    if info.width > MAX_SIZE || info.height > MAX_SIZE {
        return None;
    }
    Some(EncodedInfo { width: info.width as i32, height: info.height as i32, color, alpha, bits_per_component: bits })
}

fn valid_fctl(info: &EncodedInfo, reader: &Reader<Stream>) -> bool {
    match reader.info().frame_control {
        None => true,
        Some(control) => {
            let (Ok(width), Ok(height)) = (i32::try_from(control.width), i32::try_from(control.height)) else { return false };
            !(control.x_offset != 0 || width != info.width || control.y_offset != 0 || height != info.height)
        }
    }
}

impl Codec {
    pub fn make(data: &[u8]) -> Result<Codec, Code> {
        let stream = Stream::new(data);
        let reader = Reader::new(stream.clone()).map_err(code_of)?;
        let info = to_info(&reader).ok_or(Code::ErrorInInput)?;
        if !valid_fctl(&info, &reader) {
            return Err(Code::ErrorInInput);
        }
        let screen = Rect::xywh(0, 0, info.width, info.height);
        let mut codec = Codec {
            info,
            stream,
            reader,
            frame_at_current: 0,
            at_start_of_data: true,
            max_length_seen: None,
            incremental: None,
            holder: Holder { frames: Vec::new(), screen },
            can_parse_more: true,
            started: false,
        };
        let idat_is_not_part_of_animation = codec.reader.info().animation_control.is_some() && codec.reader.info().frame_control.is_none();
        codec.frame_at_current = if idat_is_not_part_of_animation { -1 } else { 0 };
        codec.at_start_of_data = true;
        if !idat_is_not_part_of_animation {
            let result = codec.append_new_frame();
            assert_eq!(result, Code::Success);
        }
        Ok(codec)
    }

    pub fn encoded(&self) -> &EncodedInfo {
        &self.info
    }

    fn raw_frame_count(&self) -> i32 {
        match self.reader.info().animation_control {
            None => 1,
            Some(control) => i32::try_from(control.num_frames).unwrap_or(i32::MAX),
        }
    }

    fn append_new_frame(&mut self) -> Code {
        let id = self.holder.frames.len();
        if i32::try_from(id + 1).is_err() {
            return Code::ErrorInInput;
        }
        let id = id as i32;
        let info = self.reader.info();
        if let (Some(_), Some(control)) = (info.animation_control, info.frame_control) {
            if let Some(last) = self.holder.frames.last_mut() {
                last.fully_received = true;
            }
            let mut frame = Frame::new(id, self.info.alpha);
            let (Ok(x), Ok(y), Ok(w), Ok(h)) = (
                i32::try_from(control.x_offset),
                i32::try_from(control.y_offset),
                i32::try_from(control.width),
                i32::try_from(control.height),
            ) else {
                return Code::ErrorInInput;
            };
            let duration_ms = if control.delay_den == 0 {
                10 * u32::from(control.delay_num)
            } else {
                1000 * u32::from(control.delay_num) / u32::from(control.delay_den)
            };
            let Ok(duration) = i32::try_from(duration_ms) else { return Code::ErrorInInput };
            frame.rect = Rect::xywh(x, y, w, h);
            frame.duration = duration;
            frame.disposal = match control.dispose_op {
                png_codec::DisposeOp::None => Disposal::Keep,
                png_codec::DisposeOp::Background => Disposal::RestoreBackground,
                png_codec::DisposeOp::Previous => Disposal::RestorePrevious,
            };
            let blend = if id == 0 { png_codec::BlendOp::Source } else { control.blend_op };
            frame.blend = match blend {
                png_codec::BlendOp::Source => Blend::Src,
                png_codec::BlendOp::Over => Blend::SrcOver,
            };
            self.holder.set_alpha_and_required(&mut frame);
            self.holder.frames.push(frame);
            return Code::Success;
        }
        assert!(id == 0);
        let mut frame = Frame::new(0, self.info.alpha);
        frame.rect = Rect::xywh(0, 0, self.info.width, self.info.height);
        frame.blend = Blend::Src;
        self.holder.set_alpha_and_required(&mut frame);
        self.holder.frames.push(frame);
        Code::Success
    }

    fn read_to_start_of_next_frame(&mut self) -> Code {
        assert!(self.frame_at_current < self.raw_frame_count());
        let result = code_of(self.reader.next_frame_info());
        if result != Code::Success {
            self.at_start_of_data = false;
            return result;
        }
        self.at_start_of_data = true;
        self.frame_at_current += 1;
        if self.frame_at_current as usize == self.holder.frames.len() {
            return self.append_new_frame();
        }
        Code::Success
    }

    fn seek_to_start_of_frame(&mut self, index: i32) -> Code {
        assert!(0 <= index && index as usize <= self.holder.frames.len());
        if index < self.frame_at_current || (index == self.frame_at_current && !self.at_start_of_data) {
            self.stream.rewind();
            self.reader = Reader::new(self.stream.clone()).map_err(code_of).expect("reader recreated");
            let idat_is_not_part_of_animation = self.reader.info().animation_control.is_some() && self.reader.info().frame_control.is_none();
            self.frame_at_current = if idat_is_not_part_of_animation { -1 } else { 0 };
            self.at_start_of_data = true;
        }
        while self.frame_at_current < index {
            let result = self.read_to_start_of_next_frame();
            if result != Code::Success {
                return result;
            }
        }
        Code::Success
    }

    fn parse_additional_frame_infos(&mut self) -> Code {
        while (self.holder.frames.len() as i32) < self.raw_frame_count() {
            let old = self.holder.frames.len();
            let result = self.seek_to_start_of_frame(old as i32);
            if result != Code::Success {
                return result;
            }
            assert_eq!(self.holder.frames.len(), old + 1);
        }
        Code::Success
    }

    pub fn frame_count(&mut self) -> i32 {
        'parse: {
            if !self.can_parse_more || self.incremental.is_some() {
                break 'parse;
            }
            let current = self.stream.len();
            if self.max_length_seen.is_some_and(|old| old >= current) {
                break 'parse;
            }
            self.max_length_seen = Some(current);
            match self.parse_additional_frame_infos() {
                Code::IncompleteInput => self.can_parse_more = true,
                Code::Success => {
                    assert_eq!(self.holder.frames.len() as i32, self.raw_frame_count());
                    self.can_parse_more = false;
                }
                _ => self.can_parse_more = false,
            }
        }
        self.holder.frames.len() as i32
    }

    pub fn frame_info(&self, index: i32) -> Option<FrameInfo> {
        let frame = self.holder.frame(index)?;
        Some(FrameInfo {
            required: frame.required,
            duration: frame.duration,
            fully_received: frame.fully_received,
            opaque: !frame.has_alpha,
            disposal: frame.disposal,
            blend: frame.blend,
            rect: frame.rect,
        })
    }

    pub fn repetition_count(&self) -> i32 {
        let Some(control) = self.reader.info().animation_control else { return 0 };
        if control.num_frames <= 1 {
            return 0;
        }
        match i32::try_from(control.num_plays) {
            Ok(0) | Err(_) => -1,
            Ok(plays) => plays - 1,
        }
    }

    pub fn is_animated(&self) -> bool {
        self.reader.info().animation_control.is_some_and(|control| control.num_frames > 1)
    }

    fn handle_frame_index(&mut self, target: Target, pixels: &mut [u8], frame_index: i32, prior: i32) -> Code {
        let valid_alpha = |source_opaque: bool| source_opaque || target.alpha != DstAlpha::Opaque;
        if frame_index == 0 {
            return if valid_alpha(self.info.alpha == Alpha::Opaque) { Code::Success } else { Code::InvalidConversion };
        }
        if frame_index < 0 {
            return Code::InvalidParameters;
        }
        if frame_index >= self.frame_count() {
            return Code::IncompleteInput;
        }
        let frame = self.holder.frame(frame_index).expect("frame").clone();
        let required = frame.required;
        if required != NO_FRAME {
            if prior == NO_FRAME {
                return Code::InternalError;
            }
            if prior < required || prior >= frame_index {
                return Code::InvalidParameters;
            }
            let prepped = self.holder.frame(prior).expect("prior frame");
            match prepped.disposal {
                Disposal::RestorePrevious => return Code::InvalidParameters,
                Disposal::RestoreBackground => {
                    if prepped.id == required {
                        let rect = prepped.rect;
                        zero_rect(pixels, target, rect);
                    }
                }
                Disposal::Keep => {}
            }
        }
        if valid_alpha(!frame.has_alpha) { Code::Success } else { Code::InvalidConversion }
    }

    pub fn start_incremental_decode(&mut self, target: Target, pixels: &mut [u8], frame_index: i32, prior: i32) -> Code {
        self.started = false;
        let result = self.handle_frame_index(target, pixels, frame_index, prior);
        if result != Code::Success {
            return result;
        }
        self.incremental = None;
        let result = self.start_decoding(target, frame_index);
        if result == Code::Success {
            self.started = true;
        }
        result
    }

    fn start_decoding(&mut self, target: Target, frame_index: i32) -> Code {
        if frame_index == 0 && self.holder.frames.is_empty() {
            let result = self.seek_to_start_of_frame(0);
            if result != Code::Success {
                return result;
            }
        }
        if frame_index < 0 || frame_index as usize >= self.holder.frames.len() {
            return Code::InvalidParameters;
        }
        let frame_width = self.holder.frame(frame_index).expect("frame").rect.width();
        let result = self.seek_to_start_of_frame(frame_index);
        if result != Code::Success {
            return result;
        }
        if self.info.color == Color::Palette && self.reader.info().palette.is_none() {
            return Code::InvalidInput;
        }
        let table = if self.info.color == Color::Palette {
            let palette = self.reader.info().palette.as_deref().expect("palette");
            let trns = self.reader.info().trns.as_deref();
            Some(swizzle::color_table(palette, trns, target.alpha, &self.info))
        } else {
            None
        };
        let Some(swizzler) = Swizzler::make(&self.info, table, target.alpha, frame_width as usize) else { return Code::Unimplemented };
        self.incremental = Some(State {
            frame_index,
            stride: target.row_bytes(),
            swizzler,
            encoded_row_bytes: self.info.row_bytes(frame_width),
            dst: None,
            preblend: Vec::new(),
            rows_written: 0,
        });
        Code::Success
    }

    fn initialize_dst(&mut self, target: Target) -> Code {
        let state = self.incremental.as_mut().expect("state");
        let frame = self.holder.frames[state.frame_index as usize].clone();
        let interlaced = self.reader.info().interlaced;
        let row_size = state.swizzler.width() * 4;
        if state.stride < target.row_bytes() {
            return Code::InvalidParameters;
        }
        let image_size = state.stride * target.height as usize;
        let x_offset = 4 * frame.rect.left as usize;
        let y_offset = state.stride * frame.rect.top as usize;
        let frame_bytes = frame.rect.height() as usize * state.stride;
        let start = (x_offset + y_offset).min(image_size);
        assert!(x_offset <= image_size && x_offset + y_offset <= image_size);
        let mut end = image_size;
        if frame_bytes < end - start {
            end = start + frame_bytes;
        }
        let span = end - start;
        let preblend_size = if interlaced {
            if frame.blend == Blend::SrcOver { span } else { 0 }
        } else if frame.blend == Blend::SrcOver {
            row_size
        } else {
            0
        };
        state.preblend = vec![0; preblend_size];
        state.dst = Some(DstSpan { stride: state.stride, start, end, row_size, row_count: frame.rect.height() as usize });
        Code::Success
    }

    pub fn incremental_decode(&mut self, pixels: &mut [u8], target: Target) -> Code {
        if !self.started {
            return Code::InvalidParameters;
        }
        if self.incremental.is_none() {
            return Code::InvalidParameters;
        }
        if self.incremental.as_ref().is_some_and(|state| state.dst.is_none()) {
            let result = self.initialize_dst(target);
            if result != Code::Success {
                self.incremental = None;
                return result;
            }
        }
        let result = self.decode_rows(pixels);
        if result != Code::IncompleteInput {
            self.incremental = None;
        }
        result
    }

    fn apply_xform_row(state: &State, dst: &mut [u8], src: &[u8]) {
        assert!(dst.len() >= state.swizzler.width() * 4);
        assert!(src.len() >= state.encoded_row_bytes);
        state.swizzler.swizzle(dst, src);
    }

    fn expand_interlaced(reader: &Reader<Stream>, state: &State, target: &mut [u8], row: &[u8], info: &DstSpan) {
        let mut padded = vec![0u8; state.encoded_row_bytes];
        padded[..row.len()].copy_from_slice(row);
        let mut transformed = vec![0u8; info.row_size];
        Self::apply_xform_row(state, &mut transformed, &padded);
        let whole = info.stride * info.row_count;
        if target.len() >= whole {
            reader.splat_last_interlaced_row(&mut target[..whole], info.stride, &transformed, 32);
        } else {
            reader.expand_last_interlaced_row(target, info.stride, &transformed, 32);
        }
    }

    fn decode_rows(&mut self, pixels: &mut [u8]) -> Code {
        let interlaced = self.reader.info().interlaced;
        loop {
            self.at_start_of_data = false;
            let row = match self.reader.next_interlaced_row() {
                Ok(row) => row.map(<[u8]>::to_vec),
                Err(outcome) => return code_of(outcome),
            };
            let state = self.incremental.as_mut().expect("state");
            let (start, end, stride, row_size, row_count) = {
                let dst = state.dst.as_ref().expect("dst");
                (dst.start, dst.end, dst.stride, dst.row_size, dst.row_count)
            };
            let Some(row) = row else {
                if interlaced && !state.preblend.is_empty() {
                    blend_all_rows(&mut pixels[start..end], &state.preblend, row_size, stride);
                }
                let index = state.frame_index as usize;
                self.holder.frames[index].fully_received = true;
                return Code::Success;
            };
            if interlaced {
                let info = DstSpan { stride, start, end, row_size, row_count };
                if state.preblend.is_empty() {
                    Self::expand_interlaced(&self.reader, state, &mut pixels[start..end], &row, &info);
                } else {
                    let mut preblend = std::mem::take(&mut state.preblend);
                    Self::expand_interlaced(&self.reader, state, &mut preblend, &row, &info);
                    state.preblend = preblend;
                }
            } else {
                if state.preblend.is_empty() {
                    Self::apply_xform_row(state, &mut pixels[start..end], &row);
                } else {
                    let mut preblend = std::mem::take(&mut state.preblend);
                    Self::apply_xform_row(state, &mut preblend, &row);
                    blend_row(&mut pixels[start..end], &preblend);
                    state.preblend = preblend;
                }
                let advance = stride.min(end - start);
                state.dst.as_mut().expect("dst").start += advance;
                state.rows_written += 1;
            }
        }
    }
}

fn zero_rect(pixels: &mut [u8], target: Target, rect: Rect) {
    let Some(rect) = rect.intersect(&Rect::xywh(0, 0, target.width, target.height)) else { return };
    for row in rect.top..rect.bottom {
        let start = row as usize * target.row_bytes() + rect.left as usize * 4;
        pixels[start..start + rect.width() as usize * 4].fill(0);
    }
}

fn blend_row(dst: &mut [u8], src: &[u8]) {
    assert!(dst.len() >= src.len());
    for x in 0..src.len() / 4 {
        let a = u32::from(src[4 * x + 3]);
        let inverse = 255 - a;
        for channel in 0..4 {
            let d = u32::from(dst[4 * x + channel]);
            let s = u32::from(src[4 * x + channel]);
            let mut v = d * inverse + 128;
            v = (v + v / 256) / 256;
            dst[4 * x + channel] = (s + v) as u8;
        }
    }
}

fn blend_all_rows(mut dst: &mut [u8], mut src: &[u8], row_size: usize, stride: usize) {
    while src.len() >= row_size {
        blend_row(dst, &src[..row_size]);
        let step = stride.min(src.len());
        src = &src[step..];
        let step = stride.min(dst.len());
        dst = &mut dst[step..];
    }
}

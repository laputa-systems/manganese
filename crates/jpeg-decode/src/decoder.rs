use crate::buffers::Snapshot;
use crate::metadata;
use img_jpeg::{ColorSpace, ConsumeStatus, Decompress, Error, SavedMarker, Source, SourceState};
use std::rc::Rc;

pub const NULL: usize = usize::MAX;
const SCALE_DENOMINATOR: u32 = 8;
const MAX_FRAME_BYTES: u64 = 64 << 20;
const NO_DECODED_IMAGE_BYTE_LIMIT: u32 = u32::MAX;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Empty = 0,
    Partial = 2,
    Complete = 3,
}

pub struct Frame {
    pub status: Status,
    pub has_alpha: bool,
    pub width: u32,
    pub height: u32,
    pub pixels: Option<Vec<u8>>,
}

impl Frame {
    fn new() -> Self {
        Frame { status: Status::Empty, has_alpha: true, width: 0, height: 0, pixels: None }
    }
}

struct ReaderSource {
    bytes: Rc<Vec<u8>>,
    data: Option<Snapshot>,
    needs_restart: bool,
    restart_position: usize,
    next_read_position: usize,
    last_set_byte: usize,
    completed: bool,
}

impl ReaderSource {
    fn update_restart_position(&mut self, state: &SourceState) {
        if self.last_set_byte != state.next_input_byte {
            self.restart_position = self.next_read_position - state.bytes_in_buffer;
        }
    }

    fn clear_buffer(&mut self, state: &mut SourceState) {
        state.bytes_in_buffer = 0;
        state.next_input_byte = NULL;
        self.last_set_byte = NULL;
    }

    fn set_data(&mut self, data: Option<Snapshot>, state: &mut SourceState) {
        if let Some(snapshot) = &data {
            self.bytes = snapshot.bytes.clone();
        }
        self.data = data;
        if self.needs_restart {
            return;
        }
        self.next_read_position -= state.bytes_in_buffer;
        self.clear_buffer(state);
    }
}

impl Source for ReaderSource {
    fn data(&self) -> &[u8] {
        &self.bytes
    }

    fn fill_input_buffer(&mut self, state: &mut SourceState) -> bool {
        if self.needs_restart {
            self.needs_restart = false;
            self.next_read_position = self.restart_position;
        } else {
            self.update_restart_position(state);
        }
        let segment = self.data.as_ref().and_then(|data| data.some_data(self.next_read_position));
        let Some((start, end)) = segment else {
            self.needs_restart = true;
            self.clear_buffer(state);
            return false;
        };
        self.next_read_position += end - start;
        state.bytes_in_buffer = end - start;
        state.next_input_byte = start;
        self.last_set_byte = start;
        true
    }

    fn skip_input_data(&mut self, state: &mut SourceState, num_bytes: i64) {
        if num_bytes <= 0 {
            return;
        }
        let bytes_to_skip = num_bytes as usize;
        if bytes_to_skip < state.bytes_in_buffer {
            state.bytes_in_buffer -= bytes_to_skip;
            state.next_input_byte += bytes_to_skip;
        } else {
            self.next_read_position = self.next_read_position + bytes_to_skip - state.bytes_in_buffer;
            state.bytes_in_buffer = 0;
            state.next_input_byte = NULL;
        }
        self.restart_position = self.next_read_position - state.bytes_in_buffer;
        self.last_set_byte = state.next_input_byte;
    }

    fn term_source(&mut self) {
        self.completed = true;
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ReaderState {
    Header,
    StartDecompress,
    BufferedImage,
    Sequential,
    Done,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Header,
    Bitmap,
}

struct Reader {
    info: Decompress<ReaderSource>,
    state: ReaderState,
}

pub struct JpegDecoder {
    max_decoded_bytes: u32,
    desired_size: Option<(u32, u32)>,
    data: Option<Snapshot>,
    all_received: bool,
    pub failed: bool,
    size_available: bool,
    pub size: (u32, u32),
    decoded_size: (u32, u32),
    pub orientation: u16,
    pub density_corrected_size: (u32, u32),
    supported_sizes: Option<Vec<(u32, u32)>>,
    pub frames: Vec<Frame>,
    cap_allocations: bool,
    reader: Option<Box<Reader>>,
    pub markers: Vec<SavedMarker>,
}

impl JpegDecoder {
    pub fn new(desired_size: Option<(u32, u32)>) -> Self {
        JpegDecoder {
            max_decoded_bytes: NO_DECODED_IMAGE_BYTE_LIMIT,
            desired_size,
            data: None,
            all_received: false,
            failed: false,
            size_available: false,
            size: (0, 0),
            decoded_size: (0, 0),
            orientation: 1,
            density_corrected_size: (0, 0),
            supported_sizes: None,
            frames: Vec::new(),
            cap_allocations: false,
            reader: None,
            markers: Vec::new(),
        }
    }

    pub fn set_failed(&mut self) -> bool {
        self.failed = true;
        false
    }

    pub fn set_data(&mut self, data: Option<Snapshot>, all_received: bool) {
        if self.failed {
            return;
        }
        self.data = data.clone();
        self.all_received = all_received;
        if let Some(reader) = self.reader.as_mut() {
            let info = &mut reader.info;
            info.src.set_data(data, &mut info.src_state);
            if reader.state > ReaderState::StartDecompress {
                return;
            }
        }
        if self.all_received {
            self.is_size_available();
        }
    }

    pub fn is_size_available(&mut self) -> bool {
        if self.failed {
            return false;
        }
        if !self.size_available {
            self.decode(Mode::Header);
        }
        !self.failed && self.size_available
    }

    pub fn frame_count(&mut self) -> usize {
        if self.frames.is_empty() {
            self.frames.push(Frame::new());
        }
        1
    }

    pub fn set_memory_allocator(&mut self, capped: bool) {
        self.frame_count();
        self.cap_allocations = capped;
    }

    pub fn decode_frame_buffer_at_index(&mut self) -> &Frame {
        self.frame_count();
        if self.frames[0].status != Status::Complete {
            self.decode(Mode::Bitmap);
        }
        &self.frames[0]
    }

    fn set_size(&mut self, width: u32, height: u32) -> bool {
        let total = (width as i64) * (height as i64) * 4;
        if total > i32::MAX as i64 {
            return self.set_failed();
        }
        self.size = (width, height);
        self.size_available = true;
        if self.desired_scale_numerator() == 0 {
            return self.set_failed();
        }
        self.decoded_size = (width, height);
        true
    }

    fn desired_scale_numerator(&self) -> u32 {
        let original_bytes = self.size.0.wrapping_mul(self.size.1).wrapping_mul(4);
        let mut target_bytes = self.max_decoded_bytes;
        if let Some((width, height)) = self.desired_size
            && width != 0
            && height != 0
        {
            target_bytes = target_bytes.min(width.wrapping_mul(height).wrapping_mul(4));
        }
        if original_bytes <= target_bytes {
            return SCALE_DENOMINATOR;
        }
        (((target_bytes as f64 / original_bytes as f64).sqrt() * SCALE_DENOMINATOR as f64).floor() as u32).max(1)
    }

    fn decode(&mut self, mode: Mode) {
        if self.failed {
            return;
        }
        let mut reader = match self.reader.take() {
            Some(reader) => reader,
            None => {
                let mut reader = Box::new(Reader::new());
                let info = &mut reader.info;
                info.src.set_data(self.data.clone(), &mut info.src_state);
                reader
            }
        };
        let decoded = reader.decode(self, mode);
        if !decoded && self.all_received {
            self.set_failed();
        }
        let complete = !self.frames.is_empty() && self.frames[0].status == Status::Complete;
        if !(complete || self.failed) {
            self.reader = Some(reader);
        }
    }

    fn output_scanlines(&mut self, info: &mut Decompress<ReaderSource>) -> Result<bool, Error> {
        if self.frames.is_empty() {
            return Ok(false);
        }
        let width = info.output_width;
        let height = info.output_height;
        if self.frames[0].status == Status::Empty {
            let bytes = width as u64 * height as u64 * 4;
            if self.cap_allocations && bytes > MAX_FRAME_BYTES {
                return Ok(self.set_failed());
            }
            let frame = &mut self.frames[0];
            frame.pixels = Some(vec![0; bytes as usize]);
            frame.width = width;
            frame.height = height;
            frame.status = Status::Partial;
            frame.has_alpha = true;
        }
        let stride = width as usize * 4;
        if info.out_color_space == ColorSpace::ExtBgra {
            while info.output_scanline < info.output_height {
                let y = info.output_scanline as usize;
                let pixels = self.frames[0].pixels.as_mut().expect("frame pixels");
                if info.read_scanlines(&mut pixels[y * stride..(y + 1) * stride])? != 1 {
                    return Ok(false);
                }
            }
            return Ok(true);
        }
        let mut samples = vec![0u8; stride];
        while info.output_scanline < info.output_height {
            let y = info.output_scanline as usize;
            if info.read_scanlines(&mut samples)? != 1 {
                return Ok(false);
            }
            let pixels = self.frames[0].pixels.as_mut().expect("frame pixels");
            let row = &mut pixels[y * stride..(y + 1) * stride];
            for x in 0..width as usize {
                let k = samples[x * 4 + 3] as u32;
                let r = (samples[x * 4] as u32 * k / 255) as u8;
                let g = (samples[x * 4 + 1] as u32 * k / 255) as u8;
                let b = (samples[x * 4 + 2] as u32 * k / 255) as u8;
                row[x * 4..x * 4 + 4].copy_from_slice(&[b, g, r, 255]);
            }
        }
        Ok(true)
    }

    fn complete(&mut self) {
        if self.frames.is_empty() {
            return;
        }
        self.frames[0].has_alpha = false;
        self.frames[0].status = Status::Complete;
    }
}

fn progress_monitor(scan: i32) -> Result<(), Error> {
    if scan >= 100 { Err(Error::Aborted) } else { Ok(()) }
}

impl Reader {
    fn new() -> Self {
        let source = ReaderSource { bytes: Rc::new(Vec::new()), data: None, needs_restart: false, restart_position: 0, next_read_position: 0, last_set_byte: NULL, completed: false };
        let mut info = Decompress::new(source);
        info.progress_monitor = Some(Box::new(progress_monitor));
        info.save_markers(0xE1, 0xFFFF);
        info.save_markers(0xE2, 0xFFFF);
        info.save_markers(0xEB, 0xFFFF);
        Reader { info, state: ReaderState::Header }
    }

    fn decode(&mut self, decoder: &mut JpegDecoder, mode: Mode) -> bool {
        match self.run(decoder, mode) {
            Ok(value) => value,
            Err(_) => decoder.set_failed(),
        }
    }

    fn all_components_seen(&self) -> bool {
        match &self.info.coef_bits {
            Some(bits) => (0..self.info.num_components as usize).all(|c| bits[c][0] != -1),
            None => self.info.input_complete(),
        }
    }

    fn run(&mut self, decoder: &mut JpegDecoder, mode: Mode) -> Result<bool, Error> {
        if self.state == ReaderState::Header {
            if self.info.read_header(true)? == img_jpeg::ReadHeader::Suspended {
                return Ok(false);
            }
            self.info.out_color_space = match self.info.jpeg_color_space {
                ColorSpace::YCbCr | ColorSpace::Grayscale | ColorSpace::Rgb => ColorSpace::ExtBgra,
                ColorSpace::Cmyk | ColorSpace::Ycck => ColorSpace::Cmyk,
                _ => return Ok(decoder.set_failed()),
            };
            self.state = ReaderState::StartDecompress;
            decoder.markers = self.info.marker_list.clone();
            if !decoder.set_size(self.info.image_width, self.info.image_height) {
                return Ok(false);
            }
            let max_numerator = decoder.desired_scale_numerator();
            self.info.scale_denom = SCALE_DENOMINATOR;
            if decoder.supported_sizes.is_none() {
                let mut sizes = Vec::new();
                if max_numerator == SCALE_DENOMINATOR && self.should_decode_to_original_size() {
                    sizes.push((self.info.image_width, self.info.image_height));
                } else {
                    for numerator in 1..=max_numerator {
                        self.info.scale_num = numerator;
                        self.info.calc_output_dimensions()?;
                        sizes.push((self.info.output_width, self.info.output_height));
                    }
                }
                decoder.supported_sizes = Some(sizes);
            }
            self.info.scale_num = max_numerator;
            self.info.calc_output_dimensions()?;
            decoder.decoded_size = (self.info.output_width, self.info.output_height);
            let exif = metadata::exif(&self.info.marker_list);
            let parsed = metadata::parse_exif(exif.as_deref());
            decoder.orientation = parsed.origin.unwrap_or(1);
            decoder.density_corrected_size = metadata::density_corrected_size(&parsed, (self.info.output_width, self.info.output_height));
            self.info.buffered_image = self.info.has_multiple_scans()?;
            if self.info.buffered_image {
                self.info.count_corrupt_warnings = true;
                self.info.corrupt_warnings = 0;
            }
            if mode == Mode::Header {
                let info = &mut self.info;
                info.src.needs_restart = true;
                info.src.update_restart_position(&info.src_state);
                info.src.clear_buffer(&mut info.src_state);
                return Ok(true);
            }
        }
        if self.state == ReaderState::StartDecompress {
            self.info.do_fancy_upsampling = true;
            self.info.do_block_smoothing = true;
            if !self.info.start_decompress()? {
                return Ok(false);
            }
            self.state = if self.info.buffered_image { ReaderState::BufferedImage } else { ReaderState::Sequential };
        }
        if self.state == ReaderState::Sequential {
            if !decoder.output_scanlines(&mut self.info)? {
                return Ok(false);
            }
            self.state = ReaderState::Done;
        }
        if self.state == ReaderState::BufferedImage {
            let mut status = ConsumeStatus::Suspended;
            let mut first_scan_to_display = if self.all_components_seen() { self.info.input_scan_number } else { 0 };
            loop {
                if self.info.corrupt_warnings != 0 {
                    break;
                }
                status = self.info.consume_input()?;
                if matches!(status, ConsumeStatus::ReachedSos | ConsumeStatus::ReachedEoi | ConsumeStatus::Suspended) && first_scan_to_display == 0 && self.all_components_seen() {
                    first_scan_to_display = self.info.input_scan_number;
                }
                if status == ConsumeStatus::Suspended || status == ConsumeStatus::ReachedEoi {
                    break;
                }
            }
            if first_scan_to_display == 0 {
                return Ok(false);
            }
            loop {
                if self.info.output_scanline == 0 {
                    let mut scan = self.info.input_scan_number;
                    if self.info.output_scan_number == 0 && scan > first_scan_to_display && status != ConsumeStatus::ReachedEoi {
                        scan -= 1;
                    }
                    if !self.info.start_output(scan)? {
                        return Ok(false);
                    }
                }
                if self.info.output_scanline == 0xffffff {
                    self.info.output_scanline = 0;
                }
                if !decoder.output_scanlines(&mut self.info)? {
                    if decoder.failed {
                        return Ok(false);
                    }
                    if self.info.output_scanline == 0 {
                        self.info.output_scanline = 0xffffff;
                    }
                    return Ok(false);
                }
                if self.info.output_scanline == self.info.output_height {
                    if !self.info.finish_output()? {
                        return Ok(false);
                    }
                    if self.info.input_complete() && self.info.input_scan_number == self.info.output_scan_number {
                        break;
                    }
                    self.info.output_scanline = 0;
                }
            }
            self.state = ReaderState::Done;
        }
        let finished = self.info.finish_decompress()?;
        if std::mem::take(&mut self.info.src.completed) {
            decoder.complete();
        }
        Ok(finished)
    }

    fn should_decode_to_original_size(&self) -> bool {
        let mcu_width = self.info.max_h_samp_factor as u32 * 8;
        let mcu_height = self.info.max_v_samp_factor as u32 * 8;
        self.info.image_width % mcu_width != 0 || self.info.image_height % mcu_height != 0
    }
}

use crate::codec::Codec;
use crate::flac_parser::FlacParser;
use crate::flac_parser::Parsed;
use crate::format::ChannelLayout;
use crate::format::CodecParams;
use crate::mpegaudio;
use crate::rational::NOPTS;

const PTS_NB: usize = 4;
pub const PICTURE_TYPE_NONE: i32 = 0;
pub const PICTURE_TYPE_I: i32 = 1;

pub struct ParserCommon {
    pub use_codec_ts: bool,
    pub complete_frames: bool,
    fetched_offset: bool,
    pub pts: i64,
    pub dts: i64,
    pub pos: i64,
    pub frame_offset: i64,
    cur_offset: i64,
    next_frame_offset: i64,
    fetch_timestamp: bool,
    cur_frame_start_index: usize,
    cur_frame_offset: [i64; PTS_NB],
    cur_frame_end: [i64; PTS_NB],
    cur_frame_pts: [i64; PTS_NB],
    cur_frame_dts: [i64; PTS_NB],
    cur_frame_pos: [i64; PTS_NB],
    pub duration: i32,
    pub key_frame: i32,
    pub pict_type: i32,
}

pub const END_NOT_FOUND: i32 = -100;

#[derive(Default)]
pub struct ParseContext {
    buffer: Vec<u8>,
    index: usize,
    pub state: u32,
}

impl ParseContext {
    pub fn combine_frame(&mut self, next: i32, buf: &[u8]) -> Option<Vec<u8>> {
        let mut next = next;
        if buf.is_empty() && next == END_NOT_FOUND {
            next = 0;
        }
        if next == END_NOT_FOUND {
            self.buffer.truncate(self.index);
            self.buffer.extend_from_slice(buf);
            self.index += buf.len();
            return None;
        }
        let next = next as usize;
        if self.index == 0 {
            return Some(buf[..next].to_vec());
        }
        self.buffer.truncate(self.index);
        self.buffer.extend_from_slice(&buf[..next]);
        self.index = 0;
        Some(std::mem::take(&mut self.buffer))
    }
}

const SAME_HEADER_MASK: u32 = 0xffe00000 | (3 << 17) | (3 << 10) | (3 << 19);

#[derive(Default)]
pub struct MpegAudioParser {
    pc: ParseContext,
    frame_size: i32,
    header: u32,
    header_count: i32,
    no_bitrate: bool,
}

impl MpegAudioParser {
    fn parse(&mut self, common: &mut ParserCommon, avctx: &mut CodecParams, buf: &[u8]) -> Parsed {
        let buf_size = buf.len();
        let mut state = self.pc.state;
        let mut i = 0usize;
        let mut next = END_NOT_FOUND;
        let flush = buf_size == 0;
        while i < buf_size {
            if self.frame_size != 0 {
                let inc = (buf_size - i).min(self.frame_size as usize);
                i += inc;
                self.frame_size -= inc as i32;
                state = 0;
                if self.frame_size == 0 {
                    next = i as i32;
                    break;
                }
            } else {
                while i < buf_size {
                    state = (state << 8).wrapping_add(buf[i] as u32);
                    i += 1;
                    let info = mpegaudio::frame_info(state, avctx.codec_id).filter(|info| info.frame_size >= 4);
                    let Some(info) = info else {
                        if i > 4 {
                            self.header_count = -2;
                        }
                        continue;
                    };
                    let header_threshold = (!avctx.codec_id.is_none() && avctx.codec_id != info.codec) as i32;
                    if state & SAME_HEADER_MASK != self.header & SAME_HEADER_MASK && self.header != 0 {
                        self.header_count = -3;
                    }
                    self.header = state;
                    self.header_count += 1;
                    self.frame_size = info.frame_size - 4;
                    if self.header_count > header_threshold {
                        avctx.sample_rate = info.sample_rate;
                        avctx.ch_layout = ChannelLayout::default_for(info.channels);
                        common.duration = info.samples;
                        avctx.codec_id = info.codec;
                        if self.no_bitrate || avctx.bit_rate == 0 {
                            self.no_bitrate = true;
                            avctx.bit_rate += (info.bit_rate as i64 - avctx.bit_rate) / (self.header_count - header_threshold) as i64;
                        }
                    }
                    if common.complete_frames {
                        self.frame_size = 0;
                        next = buf_size as i32;
                    }
                    break;
                }
            }
        }
        self.pc.state = state;
        let Some(frame) = self.pc.combine_frame(next, buf) else {
            return Parsed { index: buf_size as i32, frame: None };
        };
        if flush && !frame.is_empty() && self.frame_size == 0 {
            return Parsed { index: next, frame: None };
        }
        Parsed { index: next, frame: if frame.is_empty() { None } else { Some(frame) } }
    }
}

enum Kind {
    Flac(Box<FlacParser>),
    MpegAudio(Box<MpegAudioParser>),
}

pub struct Parser {
    pub common: ParserCommon,
    kind: Kind,
}

pub fn init(codec: Codec) -> Option<Parser> {
    let kind = match codec.0 {
        "flac" => Kind::Flac(Box::new(FlacParser::new())),
        "mp1" | "mp2" | "mp3" | "mp3adu" => Kind::MpegAudio(Box::default()),
        _ => return None,
    };
    Some(Parser {
        common: ParserCommon {
            use_codec_ts: false,
            complete_frames: false,
            fetched_offset: false,
            pts: 0,
            dts: 0,
            pos: 0,
            frame_offset: 0,
            cur_offset: 0,
            next_frame_offset: 0,
            fetch_timestamp: true,
            cur_frame_start_index: 0,
            cur_frame_offset: [0; PTS_NB],
            cur_frame_end: [0; PTS_NB],
            cur_frame_pts: [0; PTS_NB],
            cur_frame_dts: [0; PTS_NB],
            cur_frame_pos: [0; PTS_NB],
            duration: 0,
            key_frame: -1,
            pict_type: PICTURE_TYPE_I,
        },
        kind,
    })
}

impl ParserCommon {
    fn fetch_timestamp(&mut self) {
        self.dts = NOPTS;
        self.pts = NOPTS;
        self.pos = -1;
        for i in 0..PTS_NB {
            if self.cur_offset >= self.cur_frame_offset[i]
                && (self.frame_offset < self.cur_frame_offset[i] || (self.frame_offset == 0 && self.next_frame_offset == 0))
                && self.cur_frame_end[i] != 0
            {
                self.dts = self.cur_frame_dts[i];
                self.pts = self.cur_frame_pts[i];
                self.pos = self.cur_frame_pos[i];
                if self.cur_offset < self.cur_frame_end[i] {
                    break;
                }
            }
        }
    }
}

impl Parser {
    pub fn parse(&mut self, avctx: &mut CodecParams, buf: &[u8], pts: i64, dts: i64, pos: i64) -> (usize, Option<Vec<u8>>) {
        let common = &mut self.common;
        if !common.fetched_offset {
            common.next_frame_offset = pos;
            common.cur_offset = pos;
            common.fetched_offset = true;
        }
        let size = buf.len() as i64;
        if size != 0 && common.cur_offset + size != common.cur_frame_end[common.cur_frame_start_index] {
            let i = (common.cur_frame_start_index + 1) & (PTS_NB - 1);
            common.cur_frame_start_index = i;
            common.cur_frame_offset[i] = common.cur_offset;
            common.cur_frame_end[i] = common.cur_offset + size;
            common.cur_frame_pts[i] = pts;
            common.cur_frame_dts[i] = dts;
            common.cur_frame_pos[i] = pos;
        }
        if common.fetch_timestamp {
            common.fetch_timestamp = false;
            common.fetch_timestamp();
        }
        let parsed = match &mut self.kind {
            Kind::Flac(parser) => parser.parse(common, avctx, buf),
            Kind::MpegAudio(parser) => parser.parse(common, avctx, buf),
        };
        let mut index = parsed.index as i64;
        if parsed.frame.is_some() {
            common.frame_offset = common.next_frame_offset;
            common.next_frame_offset = common.cur_offset + index;
            common.fetch_timestamp = true;
        }
        if index < 0 {
            index = 0;
        }
        common.cur_offset += index;
        (index as usize, parsed.frame)
    }
}

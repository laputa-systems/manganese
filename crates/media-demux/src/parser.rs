use crate::codec::Codec;
use crate::flac_parser::FlacParser;
use crate::format::CodecParams;
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

enum Kind {
    Flac(Box<FlacParser>),
}

pub struct Parser {
    pub common: ParserCommon,
    kind: Kind,
}

pub fn init(codec: Codec) -> Option<Parser> {
    let kind = match codec.0 {
        "flac" => Kind::Flac(Box::new(FlacParser::new())),
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

use symphonia_core::checksum::Crc8Ccitt;
use symphonia_core::checksum::Crc16Ansi;
use symphonia_core::io::Monitor;

use crate::bits::BitReader;
use crate::format::ChannelLayout;
use crate::format::CodecParams;

pub const STREAMINFO_SIZE: usize = 34;
pub const MIN_BLOCKSIZE: i32 = 16;
pub const MAX_BLOCKSIZE: i32 = 65535;
pub const MIN_FRAME_SIZE: usize = 10;
const CHMODE_INDEPENDENT: i32 = 0;
const CHMODE_MID_SIDE: i32 = 3;

pub mod block {
    pub const STREAMINFO: u8 = 0;
    pub const SEEKTABLE: u8 = 3;
    pub const VORBIS_COMMENT: u8 = 4;
    pub const CUESHEET: u8 = 5;
    pub const PICTURE: u8 = 6;
}

const SAMPLE_SIZE_TABLE: [i32; 8] = [0, 8, 12, 0, 16, 20, 24, 32];
const SAMPLE_RATE_TABLE: [i32; 16] = [0, 88200, 176400, 192000, 8000, 16000, 22050, 24000, 32000, 44100, 48000, 96000, 0, 0, 0, 0];
const BLOCKSIZE_TABLE: [i32; 16] = [0, 192, 576, 1152, 2304, 4608, 0, 0, 256, 512, 1024, 2048, 4096, 8192, 16384, 32768];
const LAYOUT_MASKS: [u64; 8] = [0x4, 0x3, 0x7, 0x33, 0x607, 0x60F, 0x70F, 0x63F];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameInfo {
    pub samplerate: i32,
    pub channels: i32,
    pub bps: i32,
    pub blocksize: i32,
    pub ch_mode: i32,
    pub frame_or_sample_num: i64,
    pub is_var_size: bool,
}

pub fn crc8(data: &[u8]) -> u8 {
    let mut crc = Crc8Ccitt::new(0);
    crc.process_buf_bytes(data);
    crc.crc()
}

pub struct Crc16(Crc16Ansi);

impl Crc16 {
    pub fn new() -> Self {
        Crc16(Crc16Ansi::new(0))
    }

    pub fn update(&mut self, data: &[u8]) {
        self.0.process_buf_bytes(data);
    }

    pub fn value(&self) -> u16 {
        self.0.crc()
    }
}

fn utf8(gb: &mut BitReader) -> Option<i64> {
    let mut val = gb.get(8) as i64;
    let mut top: u32 = ((val & 128) >> 1) as u32;
    if (val & 0xc0) == 0x80 || val >= 0xFE {
        return None;
    }
    while val & top as i64 != 0 {
        let tmp = (gb.get(8) as u8 as u32).wrapping_sub(128);
        if tmp >> 6 != 0 {
            return None;
        }
        val = (val << 6) + tmp as i64;
        top <<= 5;
    }
    val &= (top.wrapping_shl(1).wrapping_sub(1)) as i64;
    Some(val)
}

pub fn decode_frame_header(gb: &mut BitReader) -> Option<FrameInfo> {
    if gb.get(15) & 0x7FFF != 0x7FFC {
        return None;
    }
    let is_var_size = gb.get1();
    let bs_code = gb.get(4);
    let sr_code = gb.get(4);
    let mut ch_mode = gb.get(4) as i32;
    let channels;
    if ch_mode < 8 {
        channels = ch_mode + 1;
        ch_mode = CHMODE_INDEPENDENT;
    } else if ch_mode < 8 + CHMODE_MID_SIDE {
        channels = 2;
        ch_mode -= 8 - 1;
    } else {
        return None;
    }
    let bps_code = gb.get(3);
    if bps_code == 3 {
        return None;
    }
    let bps = SAMPLE_SIZE_TABLE[bps_code as usize];
    if gb.get1() {
        return None;
    }
    let frame_or_sample_num = utf8(gb)?;
    if frame_or_sample_num < 0 {
        return None;
    }
    let blocksize = match bs_code {
        0 => return None,
        6 => gb.get(8) as i32 + 1,
        7 => gb.get(16) as i32 + 1,
        code => BLOCKSIZE_TABLE[code as usize],
    };
    let samplerate = match sr_code {
        0..=11 => SAMPLE_RATE_TABLE[sr_code as usize],
        12 => gb.get(8) as i32 * 1000,
        13 => gb.get(16) as i32,
        14 => gb.get(16) as i32 * 10,
        _ => return None,
    };
    gb.skip_bits(8);
    let crc_len = gb.count() / 8;
    let mut header = gb.buffer()[..crc_len.min(gb.buffer().len())].to_vec();
    header.resize(crc_len, 0);
    if crc8(&header) != 0 {
        return None;
    }
    Some(FrameInfo { samplerate, channels, bps, blocksize, ch_mode, frame_or_sample_num, is_var_size })
}

pub fn set_channel_layout(avctx: &mut CodecParams, channels: i32) {
    if channels == avctx.ch_layout.nb_channels && avctx.ch_layout.order != crate::format::LayoutOrder::Unspec {
        return;
    }
    avctx.ch_layout = if channels as usize <= LAYOUT_MASKS.len() {
        ChannelLayout::from_mask(LAYOUT_MASKS[channels as usize - 1]).expect("FLAC layout masks are nonzero")
    } else {
        ChannelLayout::unspec(channels)
    };
}

#[derive(Clone, Copy, Debug, Default)]
pub struct StreamInfo {
    pub samplerate: i32,
    pub channels: i32,
    pub bps: i32,
    pub max_blocksize: i32,
    pub max_framesize: i32,
    pub samples: i64,
}

pub fn parse_streaminfo(avctx: &mut CodecParams, info: &mut StreamInfo, buffer: &[u8]) -> bool {
    let mut gb = BitReader::new(buffer, STREAMINFO_SIZE * 8);
    gb.skip_bits(16);
    info.max_blocksize = gb.get(16) as i32;
    if info.max_blocksize < MIN_BLOCKSIZE {
        info.max_blocksize = 16;
        return false;
    }
    gb.skip_bits(24);
    info.max_framesize = gb.get(24) as i32;
    info.samplerate = gb.get(20) as i32;
    info.channels = gb.get(3) as i32 + 1;
    info.bps = gb.get(5) as i32 + 1;
    if info.bps < 4 {
        info.bps = 16;
        return false;
    }
    avctx.sample_rate = info.samplerate;
    avctx.bits_per_raw_sample = info.bps;
    set_channel_layout(avctx, info.channels);
    info.samples = gb.get64(36) as i64;
    true
}

pub struct BlockHeader {
    pub last: bool,
    pub kind: u8,
    pub size: usize,
}

pub fn block_header(bytes: &[u8]) -> BlockHeader {
    BlockHeader {
        last: bytes[0] & 0x80 != 0,
        kind: bytes[0] & 0x7F,
        size: (bytes[1] as usize) << 16 | (bytes[2] as usize) << 8 | bytes[3] as usize,
    }
}

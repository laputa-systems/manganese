use crate::codec::Codec;

pub const MP3_MASK: u32 = 0xFFFE0CCF;
pub const HEADER_SIZE: usize = 4;
pub const MODE_MONO: u32 = 3;

const BITRATE: [[[u16; 15]; 3]; 2] = [
    [
        [0, 32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448],
        [0, 32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384],
        [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320],
    ],
    [
        [0, 32, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256],
        [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160],
        [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160],
    ],
];
const FREQ: [i32; 3] = [44100, 48000, 32000];

#[derive(Clone, Copy, Debug)]
pub struct Header {
    pub lsf: i32,
    pub layer: i32,
    pub sample_rate: i32,
    pub error_protection: bool,
    pub bit_rate: i32,
    pub nb_channels: i32,
}

pub enum Decoded {
    Invalid,
    FreeFormat(Header),
    Frame(Header, i32),
}

pub fn check_header(header: u32) -> bool {
    header & 0xffe00000 == 0xffe00000
        && header & (3 << 19) != 1 << 19
        && header & (3 << 17) != 0
        && header & (0xf << 12) != 0xf << 12
        && header & (3 << 10) != 3 << 10
}

pub fn decode_header(header: u32) -> Decoded {
    if !check_header(header) {
        return Decoded::Invalid;
    }
    let (lsf, mpeg25) = if header & (1 << 20) != 0 { (if header & (1 << 19) != 0 { 0 } else { 1 }, 0) } else { (1, 1) };
    let layer = 4 - ((header >> 17) & 3) as i32;
    let sample_rate = FREQ[((header >> 10) & 3) as usize] >> (lsf + mpeg25);
    let bitrate_index = ((header >> 12) & 0xf) as usize;
    let padding = ((header >> 9) & 1) as i32;
    let mode = (header >> 6) & 3;
    let mut decoded = Header {
        lsf,
        layer,
        sample_rate,
        error_protection: (header >> 16) & 1 == 0,
        bit_rate: 0,
        nb_channels: if mode == MODE_MONO { 1 } else { 2 },
    };
    if bitrate_index == 0 {
        return Decoded::FreeFormat(decoded);
    }
    let kbps = BITRATE[lsf as usize][(layer - 1) as usize][bitrate_index] as i32;
    decoded.bit_rate = kbps * 1000;
    let frame_size = match layer {
        1 => ((kbps * 12000) / sample_rate + padding) * 4,
        2 => (kbps * 144000) / sample_rate + padding,
        _ => (kbps * 144000) / (sample_rate << lsf) + padding,
    };
    Decoded::Frame(decoded, frame_size)
}

pub struct FrameInfo {
    pub frame_size: i32,
    pub sample_rate: i32,
    pub channels: i32,
    pub samples: i32,
    pub bit_rate: i32,
    pub codec: Codec,
}

pub fn frame_info(head: u32, codec: Codec) -> Option<FrameInfo> {
    let Decoded::Frame(header, frame_size) = decode_header(head) else {
        return None;
    };
    let (codec, samples) = match header.layer {
        1 => (Codec("mp1"), 384),
        2 => (Codec("mp2"), 1152),
        _ => (if codec == Codec("mp3adu") { codec } else { Codec("mp3") }, if header.lsf != 0 { 576 } else { 1152 }),
    };
    Some(FrameInfo { frame_size, sample_rate: header.sample_rate, channels: header.nb_channels, samples, bit_rate: header.bit_rate, codec })
}

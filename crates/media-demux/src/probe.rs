use crate::avio::AVERROR_EOF;
use crate::avio::AVERROR_INVALIDDATA;
use crate::avio::Avio;

pub const SCORE_MAX: i32 = 100;
pub const SCORE_EXTENSION: i32 = 50;
pub const SCORE_RETRY: i32 = SCORE_MAX / 4;
pub const SCORE_STREAM_RETRY: i32 = SCORE_MAX / 4 - 1;
pub const PADDING: usize = 32;
const PROBE_BUF_MIN: usize = 2048;
const PROBE_BUF_MAX: usize = 1 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Aac,
    Flac,
    Matroska,
    Mov,
    Mp3,
    Ogg,
    Wav,
}

const FORMATS: [Format; 7] = [Format::Aac, Format::Flac, Format::Matroska, Format::Mov, Format::Mp3, Format::Ogg, Format::Wav];

impl Format {
    pub fn name(self) -> &'static str {
        match self {
            Format::Aac => "aac",
            Format::Flac => "flac",
            Format::Matroska => "matroska,webm",
            Format::Mov => "mov,mp4,m4a,3gp,3g2,mj2",
            Format::Mp3 => "mp3",
            Format::Ogg => "ogg",
            Format::Wav => "wav",
        }
    }

    pub fn probe(self, buf: &[u8], size: usize) -> i32 {
        match self {
            Format::Aac => adts_aac_probe(buf, size),
            Format::Flac => flac_probe(buf, size),
            Format::Matroska => matroska_probe(buf, size),
            Format::Mov => mov_probe(buf, size),
            Format::Mp3 => mp3_read_probe(buf, size),
            Format::Ogg => ogg_probe(buf),
            Format::Wav => wav_probe(buf, size),
        }
    }
}

fn rb16(buf: &[u8], at: usize) -> u32 {
    (buf[at] as u32) << 8 | buf[at + 1] as u32
}

fn rb24(buf: &[u8], at: usize) -> u32 {
    rb16(buf, at) << 8 | buf[at + 2] as u32
}

fn rb32(buf: &[u8], at: usize) -> u32 {
    rb16(buf, at) << 16 | rb16(buf, at + 2)
}

fn rl32(buf: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]])
}

fn tag(name: &[u8; 4]) -> u32 {
    u32::from_le_bytes(*name)
}

pub fn id3v2_match(buf: &[u8]) -> bool {
    &buf[0..3] == b"ID3"
        && buf[3] != 0xff
        && buf[4] != 0xff
        && buf[6] & 0x80 == 0
        && buf[7] & 0x80 == 0
        && buf[8] & 0x80 == 0
        && buf[9] & 0x80 == 0
}

pub fn id3v2_tag_len(buf: &[u8]) -> i32 {
    let mut len = ((buf[6] & 0x7f) as i32) << 21
        | ((buf[7] & 0x7f) as i32) << 14
        | ((buf[8] & 0x7f) as i32) << 7
        | (buf[9] & 0x7f) as i32;
    len += 10;
    if buf[5] & 0x10 != 0 {
        len += 10;
    }
    len
}

pub struct MpegAudioHeader {
    pub frame_size: i32,
}

pub fn mpa_check_header(header: u32) -> bool {
    header & 0xffe00000 == 0xffe00000
        && header & (3 << 19) != 1 << 19
        && header & (3 << 17) != 0
        && header & (0xf << 12) != 0xf << 12
        && header & (3 << 10) != 3 << 10
}

const MPA_BITRATE: [[[u16; 15]; 3]; 2] = [
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
const MPA_FREQ: [i32; 3] = [44100, 48000, 32000];

pub enum MpegAudioDecode {
    Invalid,
    FreeFormat,
    Frame(MpegAudioHeader),
}

pub fn mpegaudio_decode_header(header: u32) -> MpegAudioDecode {
    if !mpa_check_header(header) {
        return MpegAudioDecode::Invalid;
    }
    let (lsf, mpeg25) = if header & (1 << 20) != 0 { (if header & (1 << 19) != 0 { 0 } else { 1 }, 0) } else { (1, 1) };
    let layer = 4 - ((header >> 17) & 3) as i32;
    let sample_rate_index = ((header >> 10) & 3) as usize;
    let sample_rate = MPA_FREQ[sample_rate_index] >> (lsf + mpeg25);
    let bitrate_index = ((header >> 12) & 0xf) as usize;
    let padding = ((header >> 9) & 1) as i32;
    if bitrate_index == 0 {
        return MpegAudioDecode::FreeFormat;
    }
    let kbps = MPA_BITRATE[lsf as usize][(layer - 1) as usize][bitrate_index] as i32;
    let frame_size = match layer {
        1 => ((kbps * 12000) / sample_rate + padding) * 4,
        2 => (kbps * 144000) / sample_rate + padding,
        _ => (kbps * 144000) / (sample_rate << lsf) + padding,
    };
    MpegAudioDecode::Frame(MpegAudioHeader { frame_size })
}

const MP3_MASK: u32 = 0xFFFE0CCF;

fn mp3_read_probe(buf: &[u8], size: usize) -> i32 {
    let end = size as isize - 4;
    let mut buf0: isize = 0;
    while buf0 < end && buf[buf0 as usize] == 0 {
        buf0 += 1;
    }
    let mut max_frames = 0;
    let mut max_framesizes = 0;
    let mut first_frames = 0;
    let mut whole_used = false;
    let mut start = buf0;
    while start < end {
        let mut buf2 = start;
        let mut frames = 0;
        let mut framesizes = 0;
        while buf2 < end {
            let header = rb32(buf, buf2 as usize);
            let h = match mpegaudio_decode_header(header) {
                MpegAudioDecode::Frame(h) => h,
                MpegAudioDecode::Invalid | MpegAudioDecode::FreeFormat => break,
            };
            let available = (h.frame_size as isize).min(end - buf2);
            let mut header_emu = 0;
            let mut buf3 = buf2 + 4;
            while buf3 < buf2 + available {
                if rb32(buf, buf3 as usize) & MP3_MASK == header & MP3_MASK {
                    header_emu += 1;
                }
                buf3 += 1;
            }
            if header_emu > 2 {
                break;
            }
            framesizes += h.frame_size;
            if available < h.frame_size as isize {
                frames += 1;
                break;
            }
            buf2 += h.frame_size as isize;
            frames += 1;
        }
        max_frames = max_frames.max(frames);
        max_framesizes = max_framesizes.max(framesizes);
        if start == buf0 {
            first_frames = frames;
            if buf2 == end + 4 {
                whole_used = true;
            }
        }
        start = buf2 + 1;
    }
    let size = size as i64;
    let b0 = buf0 as usize;
    if first_frames >= 7 {
        SCORE_EXTENSION + 2
    } else if max_frames > 200 && size < 2 * max_framesizes as i64 {
        SCORE_EXTENSION
    } else if max_frames >= 4 && size < 2 * max_framesizes as i64 {
        SCORE_EXTENSION / 2
    } else if id3v2_match(&buf[b0..]) && 2 * id3v2_tag_len(&buf[b0..]) as i64 >= size {
        if size < PROBE_BUF_MAX as i64 { SCORE_EXTENSION / 4 } else { SCORE_EXTENSION - 2 }
    } else if first_frames > 1 && whole_used {
        5
    } else if max_frames >= 1 && size < 10 * max_framesizes as i64 {
        1
    } else {
        0
    }
}

fn adts_aac_probe(buf: &[u8], size: usize) -> i32 {
    let end = size as isize - 7;
    let mut max_frames = 0;
    let mut first_frames = 0;
    let mut start: isize = 0;
    while start < end {
        let mut buf2 = start;
        let mut frames = 0;
        while buf2 < end {
            let header = rb16(buf, buf2 as usize);
            if header & 0xFFF6 != 0xFFF0 {
                if start != 0 {
                    frames = 0;
                }
                break;
            }
            let fsize = ((rb32(buf, buf2 as usize + 3) >> 13) & 0x1FFF) as isize;
            if fsize < 7 {
                break;
            }
            buf2 += fsize.min(end - buf2);
            frames += 1;
        }
        max_frames = max_frames.max(frames);
        if start == 0 {
            first_frames = frames;
        }
        start = buf2 + 1;
    }
    if first_frames >= 3 {
        SCORE_EXTENSION + 1
    } else if max_frames > 100 {
        SCORE_EXTENSION
    } else if max_frames >= 3 {
        SCORE_EXTENSION / 2
    } else if first_frames >= 1 {
        1
    } else {
        0
    }
}

fn raw_flac_probe(buf: &[u8]) -> i32 {
    if buf[2] & 0xF0 == 0 || buf[2] & 0x0F == 0x0F || buf[3] & 0xF0 >= 0xB0 || buf[3] & 0x06 == 0x06 || buf[3] & 0x01 == 0x01 {
        return 0;
    }
    SCORE_EXTENSION / 4 + 1
}

fn flac_probe(buf: &[u8], size: usize) -> i32 {
    if rb16(buf, 0) & 0xFFFE == 0xFFF8 {
        return raw_flac_probe(buf);
    }
    if size >= 4 + 4 + 13 {
        let block_type = buf[4] & 0x7f;
        let length = rb24(buf, 5);
        let min_block_size = rb16(buf, 8);
        let max_block_size = rb16(buf, 10);
        let sample_rate = rb24(buf, 18) >> 4;
        if &buf[0..4] != b"fLaC" {
            return 0;
        }
        if block_type == 0
            && length == 34
            && min_block_size >= 16
            && max_block_size >= min_block_size
            && sample_rate != 0
            && sample_rate <= 655350
        {
            return SCORE_MAX;
        }
        return SCORE_EXTENSION;
    }
    0
}

fn matroska_probe(buf: &[u8], size: usize) -> i32 {
    if rb32(buf, 0) != 0x1A45DFA3 {
        return 0;
    }
    let mut total = buf[4] as u64;
    let mut len_mask: u64 = 0x80;
    let mut length = 1usize;
    while length <= 8 && total & len_mask == 0 {
        length += 1;
        len_mask >>= 1;
    }
    if length > 8 {
        return 0;
    }
    total &= len_mask - 1;
    for n in 1..length {
        total = total << 8 | buf[4 + n] as u64;
    }
    if total.wrapping_add(1) == 1u64 << (7 * length) {
        total = (size as u64).wrapping_sub(4 + length as u64);
    } else if (size as u64) < 4 + length as u64 + total {
        return 0;
    }
    for doctype in [&b"matroska"[..], &b"webm"[..]] {
        let probelen = doctype.len() as u64;
        if total < probelen {
            continue;
        }
        let first = 4 + length as u64;
        let last = first + total - probelen;
        let mut n = first;
        while n <= last && n + probelen <= (buf.len() as u64) {
            if &buf[n as usize..(n + probelen) as usize] == doctype {
                return SCORE_MAX;
            }
            n += 1;
        }
    }
    SCORE_EXTENSION
}

fn mov_probe(buf: &[u8], size: usize) -> i32 {
    let mut offset: i64 = 0;
    let mut score = 0;
    let mut moov_offset: i64 = -1;
    loop {
        if (offset as u64).wrapping_add(8) > size as u64 {
            break;
        }
        let at = offset as usize;
        let mut atom_size = rb32(buf, at) as i64;
        let mut minsize = 8;
        if atom_size == 1 && offset + 16 <= size as i64 {
            atom_size = ((rb32(buf, at + 8) as u64) << 32 | rb32(buf, at + 12) as u64) as i64;
            minsize = 16;
        } else if atom_size == 0 {
            atom_size = size as i64 - offset;
        }
        if atom_size < minsize {
            offset += 4;
            continue;
        }
        let atom = rl32(buf, at + 4);
        if atom == tag(b"moov") || atom == tag(b"mdat") || atom == tag(b"pnot") || atom == tag(b"udta") || atom == tag(b"ftyp") {
            if atom == tag(b"moov") {
                moov_offset = offset + 4;
            }
            let brand = rl32(buf, at + 8);
            if atom == tag(b"ftyp") && (brand == tag(b"jp2 ") || brand == tag(b"jpx ") || brand == tag(b"jxl ")) {
                score = score.max(5);
            } else {
                score = SCORE_MAX;
            }
        } else if atom == tag(b"ediw") || atom == tag(b"wide") || atom == tag(b"free") || atom == tag(b"junk") || atom == tag(b"pict") {
            score = score.max(SCORE_MAX - 5);
        } else if atom == u32::from_le_bytes([0x82, 0x82, 0x7f, 0x7d]) {
            score = score.max(SCORE_EXTENSION - 5);
        } else if atom == tag(b"skip") || atom == tag(b"uuid") || atom == tag(b"prfl") {
            score = score.max(SCORE_EXTENSION);
        }
        if atom_size > i64::MAX - offset {
            break;
        }
        offset += atom_size;
    }
    if score > SCORE_MAX - 50 && moov_offset != -1 {
        let mut offset = moov_offset;
        while offset < size as i64 - 16 {
            let at = offset as usize;
            if rl32(buf, at) == tag(b"hdlr") && rl32(buf, at + 8) == tag(b"mhlr") && rl32(buf, at + 12) == tag(b"MPEG") {
                return 5;
            }
            offset += 2;
        }
    }
    score
}

fn ogg_probe(buf: &[u8]) -> i32 {
    if &buf[0..5] == b"OggS\0" && buf[5] <= 7 { SCORE_MAX } else { 0 }
}

fn wav_probe(buf: &[u8], size: usize) -> i32 {
    if size <= 32 {
        return 0;
    }
    if &buf[8..12] == b"WAVE" {
        if &buf[0..4] == b"RIFF" || &buf[0..4] == b"RIFX" {
            return SCORE_MAX - 1;
        } else if (&buf[0..4] == b"RF64" || &buf[0..4] == b"BW64") && &buf[12..16] == b"ds64" {
            return SCORE_MAX;
        }
    }
    0
}

pub struct ProbeResult {
    pub format: Option<Format>,
    pub score: i32,
}

pub fn probe_input_format3(padded: &[u8], size: usize) -> ProbeResult {
    let mut offset = 0usize;
    let mut remaining = size as i64;
    let mut tag_beyond_probe = false;
    while remaining > 10 && id3v2_match(&padded[offset..]) {
        let id3len = id3v2_tag_len(&padded[offset..]) as i64;
        if remaining > id3len + 16 {
            offset += id3len as usize;
            remaining -= id3len;
        } else {
            tag_beyond_probe = id3len < PROBE_BUF_MAX as i64;
            break;
        }
    }
    let view = &padded[offset..];
    let mut score_max = 0;
    let mut format = None;
    for candidate in FORMATS {
        let score = candidate.probe(view, remaining as usize);
        if score > score_max {
            score_max = score;
            format = Some(candidate);
        } else if score == score_max {
            format = None;
        }
    }
    if tag_beyond_probe {
        score_max = score_max.min(SCORE_EXTENSION / 2 - 1);
    }
    ProbeResult { format, score: score_max }
}

pub struct BufferProbe {
    pub format: Option<Format>,
    pub result: i32,
}

pub fn probe_input_buffer(pb: &mut Avio) -> BufferProbe {
    let mut buf: Vec<u8> = Vec::new();
    let mut probe_size = PROBE_BUF_MIN;
    let mut buf_offset = 0usize;
    let mut score = 0;
    let mut eof = false;
    let mut format = None;
    let mut ret = 0;
    while probe_size <= PROBE_BUF_MAX && format.is_none() && !eof {
        score = if probe_size < PROBE_BUF_MAX { SCORE_RETRY } else { 0 };
        buf.resize(probe_size + PADDING, 0);
        let read = pb.read(&mut buf[buf_offset..probe_size]);
        let read = if read < 0 {
            if read != AVERROR_EOF {
                ret = read;
                break;
            }
            score = 0;
            eof = true;
            0
        } else {
            read
        };
        buf_offset += read as usize;
        for byte in &mut buf[buf_offset..buf_offset + PADDING] {
            *byte = 0;
        }
        let probed = probe_input_format3(&buf, buf_offset);
        if probed.score > score {
            score = probed.score;
            format = probed.format;
        }
        probe_size = (probe_size << 1).min(PROBE_BUF_MAX.max(probe_size + 1));
    }
    if ret >= 0 && format.is_none() {
        ret = AVERROR_INVALIDDATA;
    }
    let rewind = pb.rewind_with_probe_data(buf, buf_offset);
    if ret >= 0 {
        ret = rewind;
    }
    BufferProbe { format, result: if ret < 0 { ret } else { score } }
}

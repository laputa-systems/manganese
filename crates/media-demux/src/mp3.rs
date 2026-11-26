use crate::avio::AVERROR_EOF;
use crate::avio::AVERROR_INVALIDDATA;
use crate::avio::Avio;
use crate::avio::Whence;
use crate::bits::BitReader;
use crate::codec::Codec;
use crate::codec::MediaType;
use crate::dict;
use crate::dict::Dict;
use crate::format::ChannelLayout;
use crate::format::CodecParams;
use crate::format::FormatContext;
use crate::format::Packet;
use crate::format::Parsing;
use crate::format::Step;
use crate::mpegaudio;
use crate::mpegaudio::Decoded;
use crate::rational;
use crate::rational::NOPTS;
use crate::rational::Rational;

const XING_FLAG_FRAMES: u32 = 0x01;
const XING_FLAG_SIZE: u32 = 0x02;
const XING_FLAG_TOC: u32 = 0x04;
const XING_FLAG_QSCALE: u32 = 0x08;
const XING_TOC_COUNT: usize = 100;
const ID3V1_TAG_SIZE: i64 = 128;
const PACKET_SIZE: i64 = 1024;
const RESYNC_WINDOW: i64 = 64 * 1024;
const SMPB_MAX_SAMPLES: u64 = 1 << 40;
const PRIMING_MAX: i64 = 16384;
const LAME_DECODER_DELAY: i64 = 528 + 1;
const TIME_BASE_DEN: u32 = 14112000;

#[derive(Default)]
pub struct Mp3State {
    filesize: i64,
    start_pad: i64,
    end_pad: i64,
    frames: u32,
    header_filesize: u32,
    is_cbr: bool,
}

enum Check {
    Frame(i32, u32),
    WrongHeader,
    SeekFailed,
}

fn check(pb: &mut Avio, pos: i64) -> Check {
    if pb.seek(pos, Whence::Set) < 0 {
        return Check::SeekFailed;
    }
    let mut header = [0u8; 4];
    if pb.read(&mut header) < 4 {
        return Check::SeekFailed;
    }
    let header = u32::from_be_bytes(header);
    if !mpegaudio::check_header(header) {
        return Check::WrongHeader;
    }
    match mpegaudio::decode_header(header) {
        Decoded::Frame(_, frame_size) => Check::Frame(frame_size, header),
        Decoded::FreeFormat(_) | Decoded::Invalid => Check::WrongHeader,
    }
}

fn mktag_be(tag: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*tag)
}

fn is_space(byte: u8) -> bool {
    byte == b' ' || (9..=13).contains(&byte)
}

fn hex_value(byte: u8) -> Option<u64> {
    (byte as char).to_digit(16).map(u64::from)
}

fn scan_hex(text: &[u8], at: &mut usize) -> Option<u64> {
    let start = *at;
    let limit = start + 16;
    let next = |at: &mut usize| -> Option<u8> {
        if *at >= limit || *at >= text.len() {
            return None;
        }
        let byte = text[*at];
        *at += 1;
        Some(byte)
    };
    let mut c = next(at);
    let mut negative = false;
    if let Some(sign @ (b'+' | b'-')) = c {
        negative = sign == b'-';
        c = next(at);
    }
    if c == Some(b'0') {
        c = next(at);
        if c.is_some_and(|byte| byte | 32 == b'x') {
            c = next(at);
            if c.and_then(hex_value).is_none() {
                return None;
            }
        }
    } else if c.and_then(hex_value).is_none() {
        return None;
    }
    let mut value: u64 = 0;
    while let Some(digit) = c.and_then(hex_value) {
        value = value << 4 | digit;
        c = next(at);
    }
    if c.is_some() {
        *at -= 1;
    }
    Some(if negative { value.wrapping_neg() } else { value })
}

fn parse_itunes_smpb(value: &[u8]) -> Option<(i64, i64, i64)> {
    let mut at = 0;
    let mut fields = [0u64; 4];
    let mut ends = [0usize; 4];
    for index in 0..4 {
        while at < value.len() && is_space(value[at]) {
            at += 1;
        }
        if at >= value.len() {
            return None;
        }
        fields[index] = scan_hex(value, &mut at)?;
        ends[index] = at;
    }
    if ends.iter().any(|&end| value.get(end).is_some_and(u8::is_ascii_hexdigit)) {
        return None;
    }
    let [_, priming, remainder, samples] = fields;
    if priming > SMPB_MAX_SAMPLES || remainder > SMPB_MAX_SAMPLES || samples > SMPB_MAX_SAMPLES {
        return None;
    }
    Some((priming as i64, remainder as i64, samples as i64))
}

fn find_itunes_smpb(metadata: &Dict) -> Option<usize> {
    const COMMENT: &[u8] = b"comment-iTunSMPB";
    let mut entry = None;
    while let Some(index) = metadata.get(COMMENT, entry, dict::IGNORE_SUFFIX) {
        let lang = &metadata.key(index)[COMMENT.len()..];
        if lang.is_empty() || (lang[0] == b'-' && lang.len() - 1 == 3) {
            return Some(index);
        }
        entry = Some(index);
    }
    metadata.get(b"iTunSMPB", None, 0)
}

fn parse_info_tag(ctx: &mut FormatContext, index: usize, header: &mpegaudio::Header, spf: u32) {
    const XING_OFFSETS: [[i64; 2]; 2] = [[32, 17], [17, 9]];
    let size = ctx.pb.size() as u64;
    let pos = ctx.pb.tell();
    let fsize = if size >= pos as u64 { size - pos as u64 } else { 0 };
    ctx.pb.skip(XING_OFFSETS[(header.lsf == 1) as usize][(header.nb_channels == 1) as usize]);
    let tag = ctx.pb.rb32();
    let mp3 = &mut ctx.mp3;
    mp3.is_cbr = tag == mktag_be(b"Info");
    if tag != mktag_be(b"Xing") && !mp3.is_cbr {
        return;
    }
    let flags = ctx.pb.rb32();
    if flags & XING_FLAG_FRAMES != 0 {
        ctx.mp3.frames = ctx.pb.rb32();
    }
    if flags & XING_FLAG_SIZE != 0 {
        ctx.mp3.header_filesize = ctx.pb.rb32();
    }
    let header_filesize = ctx.mp3.header_filesize as u64;
    if fsize != 0 && header_filesize != 0 {
        let min = fsize.min(header_filesize);
        let delta = fsize.max(header_filesize) - min;
        if fsize > header_filesize && delta > min >> 4 {
            ctx.mp3.frames = 0;
        }
    }
    if flags & XING_FLAG_TOC != 0 {
        for _ in 0..XING_TOC_COUNT {
            ctx.pb.r8();
        }
    }
    if flags & XING_FLAG_QSCALE != 0 {
        ctx.pb.rb32();
    }
    let mut version = [0u8; 10];
    ctx.pb.read(&mut version[..9]);
    ctx.pb.r8();
    ctx.pb.r8();
    ctx.pb.rb32();
    ctx.pb.rb16();
    ctx.pb.rb16();
    ctx.pb.r8();
    ctx.pb.r8();
    let delays = ctx.pb.rb24();
    let encoder = u32::from_be_bytes([version[0], version[1], version[2], version[3]]);
    if encoder == mktag_be(b"LAME") || encoder == mktag_be(b"Lavf") || encoder == mktag_be(b"Lavc") {
        let mp3 = &mut ctx.mp3;
        mp3.start_pad = (delays >> 12) as i64;
        mp3.end_pad = (delays & 4095) as i64;
        let st = &mut ctx.streams[index];
        st.start_skip_samples = mp3.start_pad + LAME_DECODER_DELAY;
        if mp3.frames != 0 {
            st.first_discard_sample = -mp3.end_pad + LAME_DECODER_DELAY + mp3.frames as i64 * spf as i64;
            st.last_discard_sample = mp3.frames as i64 * spf as i64;
        }
    }
    ctx.pb.r8();
    ctx.pb.r8();
    ctx.pb.rb16();
    ctx.pb.rb32();
    ctx.pb.rb16();
    ctx.pb.rb16();
}

fn parse_vbri_tag(ctx: &mut FormatContext, base: i64) {
    ctx.pb.seek(base + 4 + 32, Whence::Set);
    if ctx.pb.rb32() == mktag_be(b"VBRI") && ctx.pb.rb16() == 1 {
        ctx.pb.skip(4);
        ctx.mp3.header_filesize = ctx.pb.rb32();
        ctx.mp3.frames = ctx.pb.rb32();
    }
}

fn parse_itunes(ctx: &mut FormatContext, index: usize, header: &mpegaudio::Header, spf: u32) {
    if ctx.streams[index].start_skip_samples != 0 {
        return;
    }
    let Some(entry) = find_itunes_smpb(&ctx.metadata) else {
        return;
    };
    let Some((priming, remainder, samples)) = parse_itunes_smpb(ctx.metadata.value(entry)) else {
        return;
    };
    if priming > PRIMING_MAX || samples == 0 {
        return;
    }
    let frames = ctx.mp3.frames as i64;
    if frames != 0 && priming + samples + remainder != frames * spf as i64 {
        return;
    }
    let st = &mut ctx.streams[index];
    st.start_skip_samples = priming;
    st.first_discard_sample = priming + samples;
    st.last_discard_sample = priming + samples + remainder;
    st.duration = rational::rescale_q(samples, Rational::new(1, header.sample_rate), st.time_base);
}

fn parse_vbr_tags(ctx: &mut FormatContext, index: usize, base: i64) -> i32 {
    let head = ctx.pb.rb32();
    let (header, vbrtag_size) = match mpegaudio::decode_header(head) {
        Decoded::Invalid => return -1,
        Decoded::FreeFormat(header) => (header, 0),
        Decoded::Frame(header, frame_size) => (header, frame_size),
    };
    if header.layer != 3 {
        return -1;
    }
    let spf: u32 = if header.lsf != 0 { 576 } else { 1152 };
    ctx.mp3.frames = 0;
    ctx.mp3.header_filesize = 0;
    parse_info_tag(ctx, index, &header, spf);
    parse_vbri_tag(ctx, base);
    parse_itunes(ctx, index, &header, spf);
    let sample_base = Rational::new(1, header.sample_rate);
    let st = &mut ctx.streams[index];
    if st.start_skip_samples != 0 {
        st.start_time = rational::rescale_q(st.start_skip_samples, sample_base, st.time_base);
    }
    let mp3 = &ctx.mp3;
    if mp3.frames == 0 && mp3.header_filesize == 0 {
        return -1;
    }
    ctx.pb.seek(base + vbrtag_size as i64, Whence::Set);
    let mp3 = &ctx.mp3;
    if mp3.frames != 0 {
        let full = mp3.frames as i64 * spf as i64;
        let st = &mut ctx.streams[index];
        if st.duration == NOPTS {
            st.duration = rational::rescale_q(full - mp3.start_pad - mp3.end_pad, sample_base, st.time_base);
        }
        if mp3.header_filesize != 0 && !mp3.is_cbr {
            st.codecpar.bit_rate = rational::rescale(mp3.header_filesize as i64, 8 * header.sample_rate as i64, full);
        }
    }
    0
}

pub fn read_header(ctx: &mut FormatContext) -> Step<i32> {
    ctx.metadata = std::mem::take(&mut ctx.id3v2_meta);
    let index = ctx.new_stream();
    let st = &mut ctx.streams[index];
    st.codecpar.codec_type = MediaType::Audio;
    st.codecpar.codec_id = Codec("mp3");
    st.need_parsing = Parsing::FullRaw;
    st.start_time = 0;
    st.set_pts_info(64, 1, TIME_BASE_DEN);
    let off = ctx.pb.tell();
    ctx.mp3.filesize = ctx.pb.size();
    if parse_vbr_tags(ctx, index, off) < 0 {
        ctx.pb.seek(off, Whence::Set);
    }
    let off = ctx.pb.tell();
    let mut found = None;
    for i in 0..RESYNC_WINDOW {
        match check(&mut ctx.pb, off + i) {
            Check::Frame(frame_size, header) => {
                if let Check::Frame(_, header2) = check(&mut ctx.pb, off + i + frame_size as i64)
                    && header & mpegaudio::MP3_MASK == header2 & mpegaudio::MP3_MASK
                {
                    found = Some(i);
                    break;
                }
            }
            Check::SeekFailed => return Ok(AVERROR_INVALIDDATA),
            Check::WrongHeader => {}
        }
    }
    let ret = ctx.pb.seek(off + found.unwrap_or(0), Whence::Set);
    if ret < 0 {
        return Ok(ret as i32);
    }
    Ok(0)
}

pub fn read_packet(ctx: &mut FormatContext) -> Step<Result<Packet, i32>> {
    let mut size = PACKET_SIZE;
    let pos = ctx.pb.tell();
    let filesize = ctx.mp3.filesize;
    if filesize > ID3V1_TAG_SIZE && pos < filesize {
        size = size.min(filesize - pos);
    }
    let read = ctx.pb.get_packet(size as i32);
    if read.result <= 0 {
        return Ok(Err(if read.result < 0 { read.result } else { AVERROR_EOF }));
    }
    Ok(Ok(Packet::new(read.data, read.position, 0)))
}

pub enum Mp3Decoded {
    Failed(i32),
    Consumed { consumed: usize, frame: bool },
}

fn layer3_side_info_valid(header: &mpegaudio::Header, frame: &[u8]) -> bool {
    let mut gb = BitReader::bytes(&frame[mpegaudio::HEADER_SIZE..]);
    if header.error_protection {
        gb.get(16);
    }
    let channels = header.nb_channels as usize;
    let granules = if header.lsf != 0 {
        gb.get(8);
        gb.skip_bits(channels);
        1
    } else {
        gb.get(9);
        gb.skip_bits(if channels == 2 { 3 } else { 5 });
        for _ in 0..channels {
            gb.get(4);
        }
        2
    };
    for _ in 0..granules {
        for _ in 0..channels {
            gb.get(12);
            if gb.get(9) > 288 {
                return false;
            }
            gb.get(8);
            gb.get(if header.lsf != 0 { 9 } else { 4 });
            if gb.get1() {
                if gb.get(2) == 0 {
                    return false;
                }
                gb.get(1);
                gb.get(10);
                gb.get(9);
            } else {
                gb.get(15);
                gb.get(4);
                gb.get(3);
            }
            if header.lsf == 0 {
                gb.get(1);
            }
            gb.get(1);
            gb.get(1);
        }
    }
    true
}

pub fn decode(params: &mut CodecParams, packet: &[u8]) -> Mp3Decoded {
    let skipped = packet.iter().take_while(|&&byte| byte == 0).count();
    let buf = &packet[skipped..];
    if buf.len() < mpegaudio::HEADER_SIZE {
        return Mp3Decoded::Failed(AVERROR_INVALIDDATA);
    }
    let head = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);
    if head >> 8 == u32::from_be_bytes(*b"\0TAG") {
        return Mp3Decoded::Consumed { consumed: buf.len() + skipped, frame: false };
    }
    let (header, frame_size) = match mpegaudio::decode_header(head) {
        Decoded::Frame(header, frame_size) => (header, frame_size as usize),
        Decoded::Invalid | Decoded::FreeFormat(_) => return Mp3Decoded::Failed(AVERROR_INVALIDDATA),
    };
    params.ch_layout = ChannelLayout::from_mask(if header.nb_channels == 1 { 0x4 } else { 0x3 }).expect("mono and stereo masks are nonzero");
    if params.bit_rate == 0 {
        params.bit_rate = header.bit_rate as i64;
    }
    let size = buf.len().min(frame_size);
    params.frame_size = match header.layer {
        1 => 384,
        2 => 1152,
        _ if header.lsf != 0 => 576,
        _ => 1152,
    };
    let decoded = header.layer != 3 || layer3_side_info_valid(&header, &buf[..size]);
    if decoded {
        params.sample_rate = header.sample_rate;
    } else if size == packet.len() {
        return Mp3Decoded::Failed(AVERROR_INVALIDDATA);
    }
    Mp3Decoded::Consumed { consumed: size + skipped, frame: decoded }
}

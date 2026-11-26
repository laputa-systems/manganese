use crate::avio::AVERROR_EOF;
use crate::avio::AVERROR_INVALIDDATA;
use crate::avio::Whence;
use crate::codec::Codec;
use crate::codec::MediaType;
use crate::flac;
use crate::flac::block;
use crate::format::ChannelLayout;
use crate::format::FormatContext;
use crate::format::Packet;
use crate::format::Parsing;
use crate::format::Step;
use crate::format::Unemulated;

const RAW_PACKET_SIZE: i32 = 1024;
const MAX_TRUNC_PICTURE_SIZE: u32 = 500 * 1024 * 1024;
const PNG_SIGNATURE: u64 = 0x89504e470d0a1a0a;
const PICTURE_TYPES: u32 = 21;
const CHANNEL_MASK_KEY: &[u8] = b"WAVEFORMATEXTENSIBLE_CHANNEL_MASK";
const SR_TABLE: [i32; 16] = [0, 1764, 3528, 3840, 160, 320, 441, 480, 640, 882, 960, 1920, 0, 0, 0, 0];
const PICTURE_MIMES: &[(&[u8], &str)] = &[
    (b"image/gif", "gif"),
    (b"image/jpeg", "mjpeg"),
    (b"image/jpg", "mjpeg"),
    (b"image/jxl", "jpegxl"),
    (b"image/png", "png"),
    (b"image/tiff", "tiff"),
    (b"image/bmp", "bmp"),
    (b"image/webp", "webp"),
    (b"JPG", "mjpeg"),
    (b"PNG", "png"),
];

fn be32(buf: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]])
}

fn c_string(bytes: &[u8]) -> &[u8] {
    match bytes.iter().position(|&byte| byte == 0) {
        Some(end) => &bytes[..end],
        None => bytes,
    }
}

fn equal_ignore_case(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.to_ascii_uppercase() == y.to_ascii_uppercase())
}

pub fn parse_picture(ctx: &mut FormatContext, buf: &[u8], truncate_workaround: bool) -> Step<i32> {
    let buf_size = buf.len();
    if buf_size < 34 {
        return Ok(AVERROR_INVALIDDATA);
    }
    let mut at = 0usize;
    let left = |at: usize| (buf_size - at) as u32;
    let picture_type = be32(buf, at);
    at += 4;
    if picture_type >= PICTURE_TYPES {
        return Ok(AVERROR_INVALIDDATA);
    }
    let len = be32(buf, at);
    at += 4;
    if len == 0 || len >= 64 {
        return Ok(AVERROR_INVALIDDATA);
    }
    if len as u64 + 24 > left(at) as u64 {
        return Ok(AVERROR_INVALIDDATA);
    }
    let mimetype = c_string(&buf[at..at + len as usize]);
    at += len as usize;
    let Some(&(_, codec)) = PICTURE_MIMES.iter().find(|(name, _)| *name == mimetype) else {
        return Ok(0);
    };
    let mut codec = Codec(codec);
    let len = be32(buf, at);
    at += 4;
    if len > left(at) - 20 {
        return Ok(AVERROR_INVALIDDATA);
    }
    at += len as usize;
    let width = be32(buf, at) as i32;
    let height = be32(buf, at + 4) as i32;
    at += 16;
    let len = be32(buf, at);
    at += 4;
    let left = left(at);
    let mut trunclen = 0u32;
    if len == 0 || len > left {
        if len > MAX_TRUNC_PICTURE_SIZE || len >= i32::MAX as u32 - 64 {
            return Ok(AVERROR_INVALIDDATA);
        }
        if truncate_workaround && len > left && (len & 0xffffff) == left {
            trunclen = len - left;
        } else {
            return Ok(AVERROR_INVALIDDATA);
        }
    }
    let mut data = if trunclen == 0 {
        buf[at..at + len as usize].to_vec()
    } else {
        let mut data = buf[at..].to_vec();
        let mut rest = vec![0u8; trunclen as usize];
        let ret = ctx.pb.read_size(&mut rest);
        if ret < 0 {
            return Ok(ret);
        }
        data.extend_from_slice(&rest);
        data
    };
    let mut signature = [0u8; 8];
    for (slot, byte) in signature.iter_mut().zip(data.iter()) {
        *slot = *byte;
    }
    if u64::from_be_bytes(signature) == PNG_SIGNATURE {
        codec = Codec("png");
    }
    data.shrink_to_fit();
    let index = ctx.new_stream();
    let st = &mut ctx.streams[index];
    st.attached_pic = Some(data);
    st.codecpar.codec_type = MediaType::Video;
    st.codecpar.codec_id = codec;
    st.codecpar.width = width;
    st.codecpar.height = height;
    Ok(0)
}

fn channel_mask_value(value: &[u8]) -> Option<u64> {
    let mut at = 0;
    while at < value.len() && (value[at] == b' ' || (9..=13).contains(&value[at])) {
        at += 1;
    }
    let mut negative = false;
    if at < value.len() && (value[at] == b'+' || value[at] == b'-') {
        negative = value[at] == b'-';
        at += 1;
    }
    let mut base = 10u64;
    if at < value.len() && value[at] == b'0' {
        at += 1;
        if at < value.len() && value[at] | 32 == b'x' {
            at += 1;
            if at >= value.len() || !value[at].is_ascii_hexdigit() {
                return None;
            }
            base = 16;
        } else {
            base = 8;
        }
    }
    let mut number: u64 = 0;
    while at < value.len() {
        let digit = match value[at] {
            digit @ b'0'..=b'9' => (digit - b'0') as u64,
            digit @ b'a'..=b'z' => (digit - b'a') as u64 + 10,
            digit @ b'A'..=b'Z' => (digit - b'A') as u64 + 10,
            _ => break,
        };
        if digit >= base {
            break;
        }
        number = number.saturating_mul(base).saturating_add(digit);
        at += 1;
    }
    if negative || number == 0 || number & !0x3ffff != 0 {
        return None;
    }
    Some(number)
}

fn vorbis_comment(ctx: &mut FormatContext, buf: &[u8]) -> Step<()> {
    let size = buf.len();
    if size < 8 {
        return Ok(());
    }
    let le32 = |at: usize| u32::from_le_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]]);
    let vendor = le32(0) as i32;
    let mut at = 4usize;
    if ((size - at) as i64 - 4) < vendor as i64 || vendor < 0 {
        return Ok(());
    }
    at += vendor as usize;
    let mut remaining = le32(at);
    at += 4;
    while size - at >= 4 && remaining > 0 {
        let length = le32(at) as i32;
        at += 4;
        if ((size - at) as i64) < length as i64 || length < 0 {
            break;
        }
        let comment = &buf[at..at + length as usize];
        if let Some(equals) = comment.iter().position(|&byte| byte == b'=') {
            let key = c_string(&comment[..equals]);
            let value = c_string(&comment[equals + 1..]);
            if equals != 0 && equals + 1 != comment.len() {
                if equal_ignore_case(key, b"METADATA_BLOCK_PICTURE") {
                    return Err(Unemulated("METADATA_BLOCK_PICTURE cover art"));
                }
                if equal_ignore_case(key, CHANNEL_MASK_KEY) {
                    match &mut ctx.channel_mask {
                        Some(existing) => {
                            existing.push(b';');
                            existing.extend_from_slice(value);
                        }
                        None => ctx.channel_mask = Some(value.to_vec()),
                    }
                }
            }
        }
        at += length as usize;
        remaining -= 1;
    }
    Ok(())
}

fn cuesheet_valid(buffer: &[u8]) -> bool {
    let size = buffer.len();
    if size < 431 {
        return false;
    }
    let chapters = buffer[395] as i32 - 1;
    if chapters <= 0 {
        return false;
    }
    let mut at = 396usize;
    for _ in 0..chapters {
        if at + 36 > size {
            return false;
        }
        let ti = buffer[at + 35] as i32;
        if ti <= 0 {
            return false;
        }
        at += 36 + ti as usize * 12;
    }
    true
}

pub fn read_header(ctx: &mut FormatContext) -> Step<i32> {
    let index = ctx.new_stream();
    {
        let st = &mut ctx.streams[index];
        st.codecpar.codec_type = MediaType::Audio;
        st.codecpar.codec_id = Codec("flac");
        st.need_parsing = Parsing::FullRaw;
    }
    let marker = ctx.pb.rl32();
    if marker != u32::from_le_bytes(*b"fLaC") {
        let sample_rate = 50 * SR_TABLE[((marker >> 16) & 0xF) as usize];
        if sample_rate != 0 {
            ctx.streams[index].set_pts_info(64, 1, sample_rate as u32);
        }
        ctx.pb.seek(-4, Whence::Cur);
        return Ok(0);
    }
    let mut found_streaminfo = false;
    let mut last = false;
    while !ctx.pb.feof() && !last {
        let mut header = [0u8; 4];
        let ret = ctx.pb.read(&mut header);
        if ret < 0 {
            return Ok(ret);
        } else if ret != 4 {
            return Ok(AVERROR_EOF);
        }
        let block = flac::block_header(&header);
        last = block.last;
        let buffer = match block.kind {
            block::STREAMINFO | block::CUESHEET | block::PICTURE | block::VORBIS_COMMENT | block::SEEKTABLE => {
                let mut buffer = vec![0u8; block.size];
                let ret = ctx.pb.read_size(&mut buffer);
                if ret < 0 {
                    return Ok(ret);
                }
                buffer
            }
            _ => {
                let ret = ctx.pb.skip(block.size as i64);
                if ret < 0 {
                    return Ok(ret as i32);
                }
                Vec::new()
            }
        };
        match block.kind {
            block::STREAMINFO => {
                if found_streaminfo || block.size != flac::STREAMINFO_SIZE {
                    return Ok(AVERROR_INVALIDDATA);
                }
                found_streaminfo = true;
                let samplerate = ((buffer[10] as u32) << 16 | (buffer[11] as u32) << 8 | buffer[12] as u32) >> 4;
                let mut tail = [0u8; 8];
                tail.copy_from_slice(&buffer[13..21]);
                let samples = (u64::from_be_bytes(tail) >> 24) & ((1u64 << 36) - 1);
                let st = &mut ctx.streams[index];
                st.codecpar.extradata = Some(buffer);
                if samplerate > 0 {
                    st.set_pts_info(64, 1, samplerate);
                    if samples > 0 {
                        st.duration = samples as i64;
                    }
                }
            }
            block::CUESHEET => {
                if !cuesheet_valid(&buffer) {
                    return Ok(AVERROR_INVALIDDATA);
                }
            }
            block::PICTURE => {
                let ret = parse_picture(ctx, &buffer, true)?;
                if ret < 0 {
                    return Ok(ret);
                }
            }
            block::SEEKTABLE => {}
            kind => {
                if !found_streaminfo {
                    return Ok(AVERROR_INVALIDDATA);
                }
                if kind == block::VORBIS_COMMENT {
                    vorbis_comment(ctx, &buffer)?;
                    if let Some(value) = &ctx.channel_mask
                        && let Some(mask) = channel_mask_value(value)
                    {
                        ctx.streams[index].codecpar.ch_layout = ChannelLayout::from_mask(mask).expect("a valid mask is nonzero");
                        ctx.channel_mask = None;
                    }
                }
            }
        }
    }
    Ok(0)
}

pub fn read_packet(ctx: &mut FormatContext) -> Step<Result<Packet, i32>> {
    let pos = ctx.pb.tell();
    let mut data = vec![0u8; RAW_PACKET_SIZE as usize];
    let ret = ctx.pb.read_partial(&mut data);
    if ret < 0 {
        return Ok(Err(ret));
    }
    data.truncate(ret as usize);
    Ok(Ok(Packet::new(data, pos, 0)))
}

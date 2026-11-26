use std::collections::VecDeque;
use std::sync::OnceLock;

use crate::avio::AVERROR_EOF;
use crate::avio::AVERROR_INVALIDDATA;
use crate::bits::BitReader;
use crate::codec::Codec;
use crate::codec::MediaType;
use crate::codec::sample_fmt;
use crate::format::ChannelLayout;
use crate::format::CodecParams;
use crate::format::FormatContext;
use crate::format::Packet;
use crate::format::Step;
use crate::rational::NOPTS;

const MAX_PAGE_SIZE: usize = 65307;
const NOGRANULE: u64 = u64::MAX;
const OPUS_HEAD_SIZE: usize = 19;
const OPUS_PREROLL: i64 = 3840;

fn crc_table() -> &'static [u32; 256] {
    static TABLE: OnceLock<[u32; 256]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = [0u32; 256];
        for (index, slot) in table.iter_mut().enumerate() {
            let mut value = (index as u32) << 24;
            for _ in 0..8 {
                value = if value & 0x8000_0000 != 0 { (value << 1) ^ 0x04c1_1db7 } else { value << 1 };
            }
            *slot = value.swap_bytes();
        }
        table
    })
}

fn page_crc(page: &[u8]) -> u32 {
    let table = crc_table();
    let mut crc = 0x4fa9_b05fu32;
    for &byte in &page[4..] {
        crc = table[((crc & 0xff) as usize) ^ byte as usize] ^ (crc >> 8);
    }
    crc
}

struct Page {
    serial: u32,
    granule: u64,
    flags: u8,
    packets: Vec<Vec<u8>>,
    positions: Vec<usize>,
    seg_starts: Vec<usize>,
    page_ends: Vec<bool>,
    pos: i64,
}

fn read_page(data: &[u8], cursor: &mut usize) -> Option<Page> {
    let mut sync = [0u8; 4];
    let mut first = true;
    loop {
        let read = data.get(*cursor..*cursor + 4)?;
        sync.copy_from_slice(read);
        if sync == *b"OggS" {
            break;
        }
        if first {
            *cursor += 1;
            first = false;
            continue;
        }
        *cursor += 1;
        if *cursor + 27 > data.len() || *cursor > MAX_PAGE_SIZE {
            return None;
        }
    }
    let start = *cursor;
    if start + 27 > data.len() {
        *cursor = start + 4;
        return None;
    }
    let flags = data[start + 5];
    let granule = u64::from_le_bytes(data[start + 6..start + 14].try_into().ok()?);
    let serial = u32::from_le_bytes(data[start + 14..start + 18].try_into().ok()?);
    let stored = u32::from_be_bytes(data[start + 22..start + 26].try_into().ok()?);
    let segments_len = data[start + 26] as usize;
    let segments_end = start + 27 + segments_len;
    if segments_end > data.len() {
        *cursor = data.len();
        return None;
    }
    let segments = data[start + 27..segments_end].to_vec();
    let total: usize = segments.iter().map(|&size| size as usize).sum();
    let body_end = segments_end.checked_add(total)?;
    if body_end > data.len() {
        *cursor = data.len();
        return None;
    }
    let mut check = Vec::with_capacity(body_end - start);
    check.extend_from_slice(&data[start..start + 22]);
    check.extend_from_slice(&[0; 4]);
    check.extend_from_slice(&data[start + 26..body_end]);
    if page_crc(&check) != stored {
        *cursor = start + 1;
        return None;
    }
    if data[start + 4] != 0 {
        *cursor = start + 4;
        return None;
    }
    if granule > i64::MAX as u64 {
        *cursor = data.len();
        return None;
    }
    let mut packets = Vec::new();
    let mut positions = Vec::new();
    let mut seg_starts = Vec::new();
    let mut page_ends = Vec::new();
    let mut at = segments_end;
    let mut size = 0usize;
    let mut packet_start = 0usize;
    for (index, &segment) in segments.iter().enumerate() {
        size += segment as usize;
        if segment < 255 {
            positions.push(at);
            packets.push(data[at..at + size].to_vec());
            seg_starts.push(packet_start);
            packet_start = index + 1;
            at += size;
            size = 0;
        }
    }
    for index in 0..packets.len() {
        let end = seg_starts.get(index + 1).copied().unwrap_or(segments.len());
        page_ends.push(!segments[end..].iter().any(|&size| size < 255));
    }
    *cursor = body_end;
    Some(Page { serial, granule, flags, packets, positions, seg_starts, page_ends, pos: start as i64 })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Vorbis,
    Opus,
    Unknown,
}

struct Vorbis {
    blocksize: [i32; 2],
    mode_count: i32,
    mode_mask: i32,
    prev_mask: i32,
    mode_blocksize: [bool; 64],
    previous_blocksize: i32,
    headers: [Option<Vec<u8>>; 3],
    lens: [usize; 3],
    bos_page_pos: i64,
    ts_offset: i32,
    last_page_granule: u64,
    eos_page_duration: i64,
}

impl Vorbis {
    fn new() -> Self {
        Vorbis {
            blocksize: [0, 0],
            mode_count: 0,
            mode_mask: 0,
            prev_mask: 0,
            mode_blocksize: [false; 64],
            previous_blocksize: 0,
            headers: [None, None, None],
            lens: [0, 0, 0],
            bos_page_pos: -1,
            ts_offset: 0,
            last_page_granule: 0,
            eos_page_duration: 0,
        }
    }

    fn init(&mut self, params: &mut CodecParams, header: &[u8], setup: &[u8]) -> Result<(), i32> {
        if header.len() < 30 || header[0] != 1 || &header[1..7] != b"vorbis" || header[29] & 1 == 0 {
            return Err(AVERROR_INVALIDDATA);
        }
        self.blocksize = [1 << (header[28] & 0xf), 1 << (header[28] >> 4)];
        if setup.len() < 7 || setup[0] != 5 || &setup[1..7] != b"vorbis" {
            return Err(AVERROR_INVALIDDATA);
        }
        let reversed: Vec<u8> = setup.iter().rev().copied().collect();
        let mut gb = BitReader::bytes(&reversed);
        let mut framing = 0usize;
        while gb.left() > 97 {
            if gb.get1() {
                framing = gb.count();
                break;
            }
        }
        if framing == 0 {
            return Err(AVERROR_INVALIDDATA);
        }
        let mut count = 0i32;
        let mut last = 0i32;
        let mut found = false;
        while gb.left() >= 97 {
            if gb.get(8) > 63 || gb.get(16) != 0 || gb.get(16) != 0 {
                break;
            }
            gb.skip_bits(1);
            count += 1;
            if count > 64 {
                break;
            }
            let mut probe = gb.clone();
            if probe.get(6) + 1 == count as u32 {
                found = true;
                last = count;
            }
        }
        if !found || last > 63 {
            return Err(AVERROR_INVALIDDATA);
        }
        self.mode_count = last;
        self.mode_mask = ((1i32 << (32 - (last - 1).leading_zeros())) - 1) << 1;
        self.prev_mask = (self.mode_mask | 1) + 1;
        let mut modes = BitReader::bytes(&reversed);
        modes.skip_bits(framing);
        for index in (0..last as usize).rev() {
            modes.skip_bits(40);
            self.mode_blocksize[index] = modes.get1();
        }
        self.previous_blocksize = self.blocksize[self.mode_blocksize[0] as usize];
        params.sample_rate = i32::from_le_bytes(header[12..16].try_into().unwrap());
        params.ch_layout = ChannelLayout::default_for(header[11] as i32);
        params.bit_rate = i32::from_le_bytes(header[20..24].try_into().unwrap()) as i64;
        Ok(())
    }

    fn duration(&mut self, packet: &[u8], flags: &mut u32) -> i32 {
        if packet.is_empty() {
            return 0;
        }
        if packet[0] & 1 != 0 {
            if packet[0] == 1 {
                *flags |= VORBIS_FLAG_HEADER;
            } else if packet[0] == 3 {
                *flags |= VORBIS_FLAG_COMMENT;
            } else if packet[0] == 5 {
                *flags |= VORBIS_FLAG_SETUP;
            }
            return 0;
        }
        let mode = if self.mode_count == 1 {
            0usize
        } else {
            ((packet[0] as i32 & self.mode_mask) >> 1) as usize
        };
        if mode >= self.mode_count as usize {
            return AVERROR_INVALIDDATA;
        }
        let mut previous = self.previous_blocksize;
        if self.mode_blocksize[mode] {
            previous = self.blocksize[(packet[0] as i32 & self.prev_mask != 0) as usize];
        }
        let current = self.blocksize[self.mode_blocksize[mode] as usize];
        self.previous_blocksize = current;
        (previous + current) >> 2
    }
}

const VORBIS_FLAG_HEADER: u32 = 1;
const VORBIS_FLAG_COMMENT: u32 = 2;
const VORBIS_FLAG_SETUP: u32 = 4;

struct Opus {
    pre_skip: i64,
    need_comments: bool,
    cur_dts: i64,
    trim_pending: bool,
}

impl Opus {
    fn new() -> Self {
        Opus { pre_skip: 0, need_comments: false, cur_dts: 0, trim_pending: false }
    }

    fn rearm(&mut self, packet: &[u8]) -> Result<(), i32> {
        if packet.len() < OPUS_HEAD_SIZE || packet[8] & 0xf0 != 0 {
            return Err(AVERROR_INVALIDDATA);
        }
        self.pre_skip = u16::from_le_bytes(packet[10..12].try_into().unwrap()) as i64;
        self.need_comments = true;
        self.trim_pending = true;
        Ok(())
    }
}

struct Stream {
    serial: u32,
    replace: bool,
    codec: Kind,
    codec_data: CodecData,
    header: i32,
    nb_header: i32,
    lastpts: i64,
    lastdts: i64,
    granule: u64,
    got_data: bool,
}

enum CodecData {
    Vorbis(Vorbis),
    Opus(Opus),
    None,
}

pub struct OggState {
    packets: VecDeque<Packet>,
}

impl Default for OggState {
    fn default() -> Self {
        OggState { packets: VecDeque::new() }
    }
}

fn xiph_lacing(into: &mut Vec<u8>, size: usize) {
    let mut left = size;
    while left >= 255 {
        into.push(255);
        left -= 255;
    }
    into.push(left as u8);
}

fn gptopts(stream: &Stream, granule: u64) -> i64 {
    let value = match stream.codec {
        Kind::Vorbis | Kind::Opus => granule,
        Kind::Unknown => granule,
    };
    if value > i64::MAX as u64 {
        NOPTS
    } else {
        value as i64
    }
}

fn opus_duration(packet: &[u8]) -> Result<i32, i32> {
    if packet.is_empty() {
        return Err(AVERROR_INVALIDDATA);
    }
    let toc = packet[0] as u32;
    let config = toc >> 3;
    let count = toc & 3;
    let frame = if config < 12 {
        (480).max(960 * (config & 3))
    } else if config < 16 {
        480 << (config & 1)
    } else {
        120 << (config & 3)
    };
    let frames = if count == 3 {
        if packet.len() < 2 {
            return Err(AVERROR_INVALIDDATA);
        }
        (packet[1] & 0x3f) as u32
    } else if count != 0 {
        2
    } else {
        1
    };
    Ok((frame * frames) as i32)
}

fn find_codec(data: &[u8]) -> Kind {
    if data.starts_with(b"\x01vorbis") {
        Kind::Vorbis
    } else if data.starts_with(b"OpusHead") {
        Kind::Opus
    } else {
        Kind::Unknown
    }
}

fn set_params(params: &mut CodecParams, codec: Codec, format: i32, rate: i32, channels: i32) {
    params.codec_type = MediaType::Audio;
    params.codec_id = codec;
    params.format = format;
    params.sample_rate = rate;
    params.ch_layout = ChannelLayout::default_for(channels);
}

fn vorbis_extradata(private: &Vorbis) -> Vec<u8> {
    let mut out = vec![2u8];
    xiph_lacing(&mut out, private.lens[0]);
    xiph_lacing(&mut out, private.lens[1]);
    for header in &private.headers {
        if let Some(header) = header {
            out.extend_from_slice(header);
        }
    }
    out
}

fn comment_without_fields(header: &[u8]) -> Vec<u8> {
    let mut out = header.to_vec();
    if out.len() < 12 {
        return out;
    }
    let vendor = u32::from_le_bytes(out[7..11].try_into().unwrap()) as usize;
    let at = 7 + 4 + vendor + 4;
    if at + 1 <= out.len() {
        out[at - 4..at].copy_from_slice(&0u32.to_le_bytes());
        out[at] = 1;
        out.truncate(at + 1);
    }
    out
}

fn process_header(
    ctx: &mut FormatContext<'_>,
    streams: &mut [Stream],
    index: usize,
    packet: &[u8],
    flags: u8,
) -> Step<i32> {
    let stream = &mut streams[index];
    let packet_type = packet.first().copied().unwrap_or(0);
    match stream.codec {
        Kind::Vorbis => {
            let CodecData::Vorbis(private) = &mut stream.codec_data else { unreachable!() };
            if packet_type & 1 == 0 {
                return Ok(if private.mode_count != 0 { 0 } else { AVERROR_INVALIDDATA });
            }
            if packet_type > 5 {
                return Ok(1);
            }
            let slot = (packet_type >> 1) as usize;
            if private.headers[slot].is_some() {
                return Ok(AVERROR_INVALIDDATA);
            }
            if slot > 0 && private.headers[0].is_none() || slot > 1 && private.headers[1].is_none() {
                return Ok(if private.mode_count != 0 { 0 } else { AVERROR_INVALIDDATA });
            }
            private.lens[slot] = packet.len();
            private.headers[slot] = Some(if slot == 1 { comment_without_fields(packet) } else { packet.to_vec() });
            if slot == 0 {
                if packet.len() != 30 {
                    return Ok(AVERROR_INVALIDDATA);
                }
                if u32::from_le_bytes(packet[7..11].try_into().unwrap()) != 0 {
                    return Ok(AVERROR_INVALIDDATA);
                }
                let bs0 = packet[28] & 0xf;
                let bs1 = packet[28] >> 4;
                if bs0 > bs1 || bs0 < 6 || bs1 > 13 || packet[29] != 1 {
                    return Ok(AVERROR_INVALIDDATA);
                }
                let st = &mut ctx.streams[index];
                st.codecpar.codec_type = MediaType::Audio;
                st.codecpar.codec_id = Codec("vorbis");
                st.codecpar.sample_rate = i32::from_le_bytes(packet[12..16].try_into().unwrap());
                st.codecpar.ch_layout = ChannelLayout::default_for(packet[11] as i32);
                st.codecpar.bit_rate = i32::from_le_bytes(packet[20..24].try_into().unwrap()) as i64;
                if st.codecpar.sample_rate > 0 {
                    st.set_pts_info(64, 1, st.codecpar.sample_rate as u32);
                }
            } else if slot == 2 {
                let setup = private.headers[2].clone().unwrap();
                let id = private.headers[0].clone().unwrap();
                let st = &mut ctx.streams[index];
                if private.init(&mut st.codecpar, &id, &setup) != Ok(()) {
                    return Ok(AVERROR_INVALIDDATA);
                }
                let extra = vorbis_extradata(private);
                st.codecpar.extradata = Some(extra);
            }
            Ok(1)
        }
        Kind::Opus => {
            let CodecData::Opus(private) = &mut stream.codec_data else { unreachable!() };
            if flags & OGG_FLAG_BOS != 0 {
                if packet.len() < OPUS_HEAD_SIZE || packet[8] & 0xf0 != 0 {
                    return Ok(AVERROR_INVALIDDATA);
                }
                let channels = packet[9] as i32;
                if channels == 0 {
                    return Ok(AVERROR_INVALIDDATA);
                }
                private.pre_skip = u16::from_le_bytes(packet[10..12].try_into().unwrap()) as i64;
                private.need_comments = true;
                private.trim_pending = true;
                let st = &mut ctx.streams[index];
                set_params(&mut st.codecpar, Codec("opus"), sample_fmt::S16, 48000, channels);
                st.codecpar.extradata = Some(packet.to_vec());
                st.codecpar.seek_preroll = OPUS_PREROLL as i32;
                st.codecpar.initial_padding = private.pre_skip as i32;
                st.set_pts_info(64, 1, 48000);
                Ok(1)
            } else if private.need_comments {
                if packet.len() < 8 || &packet[..8] != b"OpusTags" {
                    return Ok(AVERROR_INVALIDDATA);
                }
                private.need_comments = false;
                Ok(1)
            } else {
                Ok(0)
            }
        }
        Kind::Unknown => Ok(0),
    }
}

const OGG_FLAG_BOS: u8 = 2;
const OGG_FLAG_EOS: u8 = 4;

struct Delivery {
    packet: Packet,
}

fn deliver(
    ctx: &mut FormatContext<'_>,
    streams: &mut Vec<Stream>,
    index: usize,
    packet: &[u8],
    pos: i64,
    page: &Page,
    packet_at: usize,
) -> Step<Result<Delivery, i32>> {
    if streams[index].codec == Kind::Opus && packet.starts_with(b"OpusHead") {
        let CodecData::Opus(private) = &mut streams[index].codec_data else { unreachable!() };
        if let Err(err) = private.rearm(packet) {
            return Ok(Err(err));
        }
        return Ok(Ok(Delivery { packet: Packet::new(Vec::new(), pos, index) }));
    }
    let stream = &mut streams[index];
    if packet.is_empty() {
        return Ok(Err(AVERROR_INVALIDDATA));
    }
    let segp = page.seg_starts[packet_at];
    let page_end = page.page_ends[packet_at];
    let granule = stream.granule;
    let mut duration = 0i32;
    let mut flags = 0u32;
    let mut start_trim = 0i32;
    let mut end_trim = 0i32;
    let mut skip = false;
    match stream.codec {
        Kind::Vorbis => {
            let CodecData::Vorbis(private) = &mut stream.codec_data else { unreachable!() };
            if private.bos_page_pos < 0 {
                private.bos_page_pos = page.pos;
            }
            let bos = segp == 0 && page.pos == private.bos_page_pos;
            if bos {
                private.previous_blocksize = private.blocksize[0];
                private.last_page_granule = 0;
            }
            duration = private.duration(packet, &mut flags);
            if duration < 0 {
                return Ok(Ok(Delivery { packet: Packet::new(Vec::new(), pos, index) }));
            }
            if private.ts_offset == 0 {
                private.ts_offset = duration;
            }
            if bos && stream.lastpts != NOPTS {
                stream.lastpts -= private.ts_offset as i64;
            }
            if granule == 0 && duration != 0 {
                stream.lastpts = NOPTS;
                stream.lastdts = NOPTS;
            }
            if page.flags & OGG_FLAG_EOS != 0 {
                if segp == 0 {
                    private.eos_page_duration = 0;
                }
                if page_end {
                    let skip_amount = private.eos_page_duration as i64 + duration as i64
                        - (granule.wrapping_sub(private.last_page_granule) as i64);
                    if skip_amount > 0 {
                        end_trim = skip_amount as i32;
                    }
                } else {
                    private.eos_page_duration += duration as i64;
                }
            } else {
                private.last_page_granule = granule;
            }
            if flags & VORBIS_FLAG_HEADER != 0 {
                let id = packet.to_vec();
                private.headers[0] = Some(id.clone());
                let setup = private.headers[2].clone().unwrap_or_default();
                let mut params = ctx.streams[index].codecpar.clone();
                if private.init(&mut params, &id, &setup).is_ok() {
                    ctx.streams[index].codecpar = params;
                }
                skip = true;
            }
            if flags & VORBIS_FLAG_COMMENT != 0 {
                skip = true;
            }
            if flags & VORBIS_FLAG_SETUP != 0 {
                skip = true;
            }
        }
        Kind::Opus => {
            let CodecData::Opus(private) = &mut stream.codec_data else { unreachable!() };
            if packet.starts_with(b"OpusHead") || packet.starts_with(b"OpusTags") {
                return Ok(Ok(Delivery { packet: Packet::new(Vec::new(), pos, index) }));
            }
            if (stream.lastpts == 0 || stream.lastpts == NOPTS) && page.flags & OGG_FLAG_EOS == 0 {
                let mut total = 0i64;
                for data in &page.packets[packet_at..] {
                    if let Ok(value) = opus_duration(data) {
                        total += value as i64;
                    }
                }
                stream.lastpts = granule as i64 - total;
                stream.lastdts = stream.lastpts;
            }
            let Ok(value) = opus_duration(packet) else {
                return Ok(Err(AVERROR_INVALIDDATA));
            };
            duration = value;
            if stream.lastpts != NOPTS {
                if ctx.streams[index].start_time == NOPTS {
                    ctx.streams[index].start_time = stream.lastpts;
                }
                private.cur_dts = stream.lastdts;
                stream.lastpts -= private.pre_skip;
                private.cur_dts = stream.lastpts;
            }
            private.cur_dts += duration as i64;
            if page.flags & OGG_FLAG_EOS != 0 {
                let mut amount = private.cur_dts - granule as i64 + private.pre_skip;
                if amount > duration as i64 {
                    amount = duration as i64;
                }
                if amount > 0 {
                    end_trim = amount as i32;
                    duration = if amount < duration as i64 { duration - amount as i32 } else { 1 };
                }
            }
            if private.trim_pending {
                start_trim = private.pre_skip as i32;
                private.trim_pending = false;
            }
        }
        Kind::Unknown => {}
    }
    let mut pts = if stream.lastpts != NOPTS {
        let value = stream.lastpts;
        stream.lastpts = NOPTS;
        value
    } else {
        NOPTS
    };
    if stream.lastdts != NOPTS {
        stream.lastdts = NOPTS;
    }
    if stream.replace {
        pts = NOPTS;
        stream.replace = false;
    }
    if stream.granule != NOGRANULE && page_end {
        let value = gptopts(stream, stream.granule);
        if stream.codec == Kind::Unknown {
            pts = value;
        } else {
            stream.lastpts = value;
            stream.lastdts = value;
        }
        stream.granule = NOGRANULE;
    }
    if skip {
        return Ok(Ok(Delivery { packet: Packet::new(Vec::new(), pos, index) }));
    }
    let st = &ctx.streams[index];
    let mut out = Packet::new(packet.to_vec(), pos, index);
    out.pts = pts;
    out.dts = NOPTS;
    out.duration = (duration as i64).max(0);
    out.flags = flags;
    if start_trim != 0 || end_trim != 0 {
        out.skip_samples = Some((start_trim.max(0) as u32, end_trim.max(0) as u32));
    }
    let _ = st;
    Ok(Ok(Delivery { packet: out }))
}

pub fn read_header(ctx: &mut FormatContext<'_>) -> Step<i32> {
    let data = ctx.data;
    let mut cursor = 0usize;
    let mut streams: Vec<Stream> = Vec::new();
    let mut packets = VecDeque::new();
    let mut headers = false;
    let mut last_granules: Vec<u64> = Vec::new();
    let mut duration_scan_done = false;
    loop {
        let Some(page) = read_page(data, &mut cursor) else {
            if cursor >= data.len().saturating_sub(3) {
                break;
            }
            continue;
        };
        let index = match streams.iter().position(|stream| stream.serial == page.serial) {
            Some(index) => index,
            None if streams.len() == 1 && streams[0].got_data => {
                let first = page.packets.first().map(Vec::as_slice).unwrap_or_default();
                if find_codec(first) != streams[0].codec {
                    break;
                }
                let stream = &mut streams[0];
                stream.serial = page.serial;
                stream.replace = true;
                stream.lastpts = 0;
                stream.lastdts = NOPTS;
                stream.granule = NOGRANULE;
                stream.header = 0;
                stream.nb_header = 0;
                0
            }
            None => {
                let index = streams.len();
                duration_scan_done |= index > 0;
                let _ = ctx.new_stream();
                ctx.streams[index].set_pts_info(64, 1, 1_000_000);
                streams.push(Stream {
                    serial: page.serial,
                    replace: false,
                    codec: Kind::Unknown,
                    codec_data: CodecData::None,
                    header: -1,
                    nb_header: 0,
                    lastpts: 0,
                    lastdts: NOPTS,
                    granule: NOGRANULE,
                    got_data: page.flags & OGG_FLAG_BOS == 0,
                });
                index
            }
        };
        let stream = &mut streams[index];
        stream.granule = page.granule;
        stream.got_data = page.flags & OGG_FLAG_BOS == 0;
        if stream.header < 0 {
            let first = page.packets.first().map(Vec::as_slice).unwrap_or_default();
            let codec = find_codec(first);
            stream.codec = codec;
            stream.codec_data = match codec {
                Kind::Vorbis => CodecData::Vorbis(Vorbis::new()),
                Kind::Opus => CodecData::Opus(Opus::new()),
                Kind::Unknown => CodecData::None,
            };
            stream.header = if codec == Kind::Unknown { 0 } else { 1 };
        }
        for (at, packet) in page.packets.iter().enumerate() {
            if streams[index].header > 0 {
                let ret = process_header(ctx, &mut streams, index, packet, page.flags)?;
                if ret < 0 {
                    return Ok(ret);
                }
                streams[index].header = ret;
                if ret == 0 {
                    headers = true;
                } else {
                    streams[index].nb_header += 1;
                    continue;
                }
            }
            if streams[index].codec == Kind::Unknown {
                continue;
            }
            let result = deliver(ctx, &mut streams, index, packet, page.positions[at] as i64, &page, at)?;
            if let Ok(delivery) = result {
                if delivery.packet.data.is_empty() {
                    continue;
                }
                if delivery.packet.stream_index != usize::MAX {
                    packets.push_back(delivery.packet);
                }
            }
        }
        if !duration_scan_done && page.granule != NOGRANULE && page.granule != 0 {
            last_granules.resize(streams.len(), NOGRANULE);
            last_granules[index] = page.granule;
        }
    }
    if !headers {
        return Ok(AVERROR_INVALIDDATA);
    }
    if last_granules.len() < streams.len() {
        last_granules.resize(streams.len(), NOGRANULE);
    }
    for (index, granule) in last_granules.iter().enumerate() {
        if *granule != NOGRANULE && *granule != 0 && streams[index].codec != Kind::Unknown {
            let mut end = gptopts(&streams[index], *granule);
            if streams[index].codec == Kind::Vorbis
                && let CodecData::Vorbis(private) = &streams[index].codec_data
            {
                end += private.ts_offset as i64;
            }
            ctx.streams[index].duration = end;
        }
    }
    ctx.ogg = OggState { packets };
    Ok(0)
}

pub fn read_packet(ctx: &mut FormatContext<'_>) -> Step<Result<Packet, i32>> {
    match ctx.ogg.packets.pop_front() {
        Some(packet) => Ok(Ok(packet)),
        None => Ok(Err(AVERROR_EOF)),
    }
}

use crate::avio::AVERROR_EOF;
use crate::avio::AVERROR_INVALIDDATA;
use crate::avio::AVERROR_PATCHWELCOME;
use crate::avio::Avio;
use crate::avio::EINVAL;
use crate::avio::Whence;
use crate::codec;
use crate::codec::Codec;
use crate::codec::MediaType;
use crate::format::ChannelLayout;
use crate::format::CodecParams;
use crate::format::FormatContext;
use crate::format::PKT_FLAG_CORRUPT;
use crate::format::Packet;
use crate::format::Parsing;
use crate::format::Step;
use crate::format::Unemulated;
use crate::probe;

#[derive(Clone, Copy, Default)]
pub struct WavState {
    data_end: i64,
    unaligned: bool,
    rifx: bool,
}

const MAX_SIZE: i32 = 4096;

fn mktag(name: &[u8; 4]) -> u32 {
    u32::from_le_bytes(*name)
}

fn next_tag(pb: &mut Avio, rifx: bool) -> (u32, i64) {
    let tag = pb.rl32();
    let size = if rifx { pb.rb32() } else { pb.rl32() };
    (tag, size as i64)
}

fn seek_tag(wav: &WavState, pb: &mut Avio, offset: i64) -> i64 {
    let adjust = offset < i64::MAX && (offset + wav.unaligned as i64) & 1 != 0;
    pb.seek(offset + adjust as i64, Whence::Set)
}

fn find_tag(wav: &WavState, pb: &mut Avio, wanted: u32) -> i64 {
    if (pb.tell() + wav.unaligned as i64) & 1 != 0 {
        pb.skip(1);
    }
    loop {
        if pb.feof() {
            return AVERROR_EOF as i64;
        }
        let (tag, size) = next_tag(pb, wav.rifx);
        if tag == wanted {
            return size;
        }
        pb.skip(size + (size & 1));
    }
}

fn get_extradata(par: &mut CodecParams, pb: &mut Avio, size: i32) -> i32 {
    par.extradata = None;
    if !(0..i32::MAX - 64).contains(&size) {
        return EINVAL;
    }
    let mut data = vec![0; size as usize];
    let ret = pb.read_size(&mut data);
    if ret < 0 {
        return ret;
    }
    par.extradata = Some(data);
    ret
}

fn compute_bitrate(par: &CodecParams) -> i64 {
    if par.sample_rate <= 0 || par.block_align <= 0 || par.ch_layout.nb_channels <= 0 {
        return 0;
    }
    match par.codec_id.0 {
        "pcm_s8" | "pcm_u8" | "pcm_s16le" | "pcm_s16be" | "pcm_u16le" | "pcm_u16be" | "pcm_s24le" | "pcm_s24be" | "pcm_s32le"
        | "pcm_s32be" | "pcm_s64le" | "pcm_f32le" | "pcm_f64le" | "pcm_alaw" | "pcm_mulaw" => {
            let block_align = ((par.bits_per_coded_sample + 7) / 8) * par.ch_layout.nb_channels;
            if par.block_align != block_align {
                return 0;
            }
            par.sample_rate as i64 * block_align as i64 * 8
        }
        _ => 0,
    }
}

fn parse_waveformatex(pb: &mut Avio, par: &mut CodecParams) {
    let bps = pb.rl16() as i32;
    if bps != 0 {
        par.bits_per_coded_sample = bps;
    }
    let mask = pb.rl32() as u64;
    if let Some(layout) = ChannelLayout::from_mask(mask) {
        par.ch_layout = layout;
    }
    let mut guid = [0u8; 16];
    if pb.read_size(&mut guid) < 0 {
        guid = [0; 16];
    }
    const AMBISONIC: [u8; 12] = [0x21, 0x07, 0xD3, 0x11, 0x86, 0x44, 0xC8, 0xC1, 0xCA, 0x00, 0x00, 0x00];
    const BROKEN: [u8; 12] = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA];
    const MEDIASUBTYPE: [u8; 12] = [0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71];
    if guid[4..] == AMBISONIC || guid[4..] == BROKEN || guid[4..] == MEDIASUBTYPE {
        par.codec_tag = u32::from_le_bytes([guid[0], guid[1], guid[2], guid[3]]);
        par.codec_id = codec::wav_codec_get_id(par.codec_tag, par.bits_per_coded_sample);
    } else {
        par.codec_id = codec::WAV_GUIDS.iter().find(|(known, _)| *known == guid).map(|(_, name)| Codec(name)).unwrap_or(Codec::NONE);
    }
}

fn get_wav_header(pb: &mut Avio, par: &mut CodecParams, size: i32, big_endian: bool) -> Step<i32> {
    if size < 14 {
        return Ok(AVERROR_INVALIDDATA);
    }
    par.ch_layout = ChannelLayout::EMPTY;
    par.codec_type = MediaType::Audio;
    let mut channels: i32 = 0;
    let mut bitrate: u64 = 0;
    let id;
    if !big_endian {
        id = pb.rl16();
        if id != 0x0165 {
            channels = pb.rl16() as i32;
            par.sample_rate = pb.rl32() as i32;
            bitrate = pb.rl32() as u64 * 8;
            par.block_align = pb.rl16() as i32;
        }
    } else {
        id = pb.rb16();
        channels = pb.rb16() as i32;
        par.sample_rate = pb.rb32() as i32;
        bitrate = pb.rb32() as u64 * 8;
        par.block_align = pb.rb16() as i32;
    }
    par.bits_per_coded_sample = if size == 14 {
        8
    } else if !big_endian {
        pb.rl16() as i32
    } else {
        pb.rb16() as i32
    };
    if id == 0xFFFE {
        par.codec_tag = 0;
    } else {
        par.codec_tag = id;
        par.codec_id = codec::wav_codec_get_id(id, par.bits_per_coded_sample);
    }
    let mut size = size;
    if size >= 18 && id != 0x0165 {
        let cb_size = pb.rl16() as i32;
        if big_endian {
            return Ok(AVERROR_PATCHWELCOME);
        }
        size -= 18;
        let mut cb_size = size.min(cb_size);
        if cb_size >= 22 && id == 0xfffe {
            parse_waveformatex(pb, par);
            cb_size -= 22;
            size -= 22;
        } else if cb_size >= 12 && id == 0x1610 {
            let payload_type = pb.rl16();
            if payload_type == 3 {
                par.codec_id = Codec("aac_latm");
            }
            pb.skip(2);
            let struct_type = pb.rl16();
            if struct_type != 0 {
                return Ok(AVERROR_PATCHWELCOME);
            }
            pb.skip(2);
            pb.skip(4);
            cb_size -= 12;
            size -= 12;
        }
        if cb_size > 0 {
            let ret = get_extradata(par, pb, cb_size);
            if ret < 0 {
                return Ok(ret);
            }
            size -= cb_size;
        }
        if size > 0 {
            pb.skip(size as i64);
        }
    } else if id == 0x0165 && size >= 32 {
        size -= 4;
        let ret = get_extradata(par, pb, size);
        if ret < 0 {
            return Ok(ret);
        }
        let extradata = par.extradata.as_deref().expect("extradata was just read");
        let nb_streams = u16::from_le_bytes([extradata[4], extradata[5]]) as i32;
        par.sample_rate = i32::from_le_bytes([extradata[12], extradata[13], extradata[14], extradata[15]]);
        channels = 0;
        bitrate = 0;
        if size < 8 + nb_streams * 20 {
            return Ok(AVERROR_INVALIDDATA);
        }
        for i in 0..nb_streams as usize {
            channels += extradata[8 + i * 20 + 17] as i32;
        }
    }
    par.bit_rate = bitrate as i64;
    if par.sample_rate <= 0 {
        return Ok(AVERROR_INVALIDDATA);
    }
    if par.codec_id == Codec("aac_latm") {
        channels = 0;
        par.sample_rate = 0;
    }
    if par.codec_id == Codec("adpcm_g726") && par.sample_rate != 0 {
        par.bits_per_coded_sample = (par.bit_rate / par.sample_rate as i64) as i32;
    }
    if channels != par.ch_layout.nb_channels {
        par.ch_layout = ChannelLayout::unspec(channels);
    }
    let expected = compute_bitrate(par);
    if expected != 0 && par.bit_rate / 8 != expected / 8 {
        par.bit_rate = expected;
    }
    Ok(0)
}

fn read_bext(pb: &mut Avio, size: i64) -> i32 {
    for length in [256usize, 32, 32, 10, 8] {
        let mut text = vec![0u8; length];
        let ret = pb.read_size(&mut text);
        if ret < 0 {
            return ret;
        }
    }
    pb.rl64();
    if pb.rl16() >= 1 {
        for _ in 0..8 {
            pb.rb64();
        }
        pb.skip(190);
    } else {
        pb.skip(254);
    }
    if size > 602 {
        let history = size - 602;
        if history > 1024 * 1024 {
            return AVERROR_INVALIDDATA;
        }
        let mut text = vec![0u8; history as usize];
        let ret = pb.read_size(&mut text);
        if ret < 0 {
            return ret;
        }
    }
    0
}

fn parse_xma2(pb: &mut Avio, par: &mut CodecParams, size: i64) -> (i32, i64) {
    if size < 36 {
        return (AVERROR_INVALIDDATA, 0);
    }
    par.codec_type = MediaType::Audio;
    par.codec_id = Codec("xma2");
    let version = pb.r8();
    if version != 3 && version != 4 {
        return (AVERROR_INVALIDDATA, 0);
    }
    let num_streams = pb.r8() as i64;
    if size != 32 + if version == 3 { 0 } else { 8 } + 4 * num_streams {
        return (AVERROR_INVALIDDATA, 0);
    }
    pb.skip(10);
    par.sample_rate = pb.rb32() as i32;
    if version == 4 {
        pb.skip(8);
    }
    pb.skip(4);
    let duration = pb.rb32() as i64;
    pb.skip(8);
    let mut channels = 0;
    for _ in 0..num_streams {
        channels += pb.r8() as i32;
        pb.skip(3);
    }
    par.ch_layout = ChannelLayout::unspec(channels);
    if channels <= 0 || par.sample_rate <= 0 {
        return (AVERROR_INVALIDDATA, duration);
    }
    pb.seek(-size, Whence::Cur);
    (get_extradata(par, pb, size as i32).min(0), duration)
}

pub fn read_header(ctx: &mut FormatContext) -> Step<i32> {
    let mut wav = WavState { unaligned: ctx.pb.tell() & 1 != 0, ..WavState::default() };
    let mut rf64 = false;
    let mut bw64 = false;
    let mut data_size: i64 = 0;
    let mut sample_count: i64 = 0;
    let tag = ctx.pb.rl32();
    match &tag.to_le_bytes() {
        b"RIFF" => {}
        b"RIFX" => wav.rifx = true,
        b"RF64" => rf64 = true,
        b"BW64" => bw64 = true,
        _ => return Ok(AVERROR_INVALIDDATA),
    }
    ctx.pb.rl32();
    if ctx.pb.rl32() != mktag(b"WAVE") {
        return Ok(AVERROR_INVALIDDATA);
    }
    if rf64 || bw64 {
        if ctx.pb.rl32() != mktag(b"ds64") {
            return Ok(AVERROR_INVALIDDATA);
        }
        let size = ctx.pb.rl32() as i64;
        if size < 24 {
            return Ok(AVERROR_INVALIDDATA);
        }
        ctx.pb.rl64();
        data_size = ctx.pb.rl64() as i64;
        sample_count = ctx.pb.rl64() as i64;
        if data_size < 0 || sample_count < 0 {
            return Ok(AVERROR_INVALIDDATA);
        }
        ctx.pb.skip(size - 24);
    }
    let index = ctx.new_stream();
    let mut got_fmt = false;
    let mut got_xma2 = false;
    let mut data_ofs: i64 = -1;
    let mut xma2_duration: Option<i64> = None;
    loop {
        let (tag, size) = next_tag(&mut ctx.pb, wav.rifx);
        let mut next_tag_ofs = ctx.pb.tell() + size + (size & 1);
        if ctx.pb.feof() {
            break;
        }
        let mut stop = false;
        match &tag.to_le_bytes() {
            b"fmt " => {
                if !got_xma2 && !got_fmt {
                    let st = &mut ctx.streams[index];
                    let ret = get_wav_header(&mut ctx.pb, &mut st.codecpar, size as i32, wav.rifx)?;
                    if ret < 0 {
                        return Ok(ret);
                    }
                    if st.codecpar.codec_id == Codec("pcm_s16le") {
                        st.request_probe = probe::SCORE_EXTENSION + 1;
                        st.probe_packets_limit(32);
                    }
                    st.need_parsing = Parsing::FullRaw;
                    let rate = st.codecpar.sample_rate as u32;
                    st.set_pts_info(64, 1, rate);
                }
                got_fmt = true;
            }
            b"XMA2" => {
                if !got_fmt && !got_xma2 {
                    let st = &mut ctx.streams[index];
                    st.need_parsing = Parsing::FullRaw;
                    let (ret, duration) = parse_xma2(&mut ctx.pb, &mut st.codecpar, size);
                    xma2_duration = Some(duration);
                    if ret < 0 {
                        return Ok(ret);
                    }
                    let rate = st.codecpar.sample_rate as u32;
                    st.set_pts_info(64, 1, rate);
                }
                got_xma2 = true;
            }
            b"data" => {
                if rf64 || bw64 {
                    wav.data_end = ctx.pb.tell().saturating_add(data_size);
                    next_tag_ofs = wav.data_end + (data_size & 1);
                } else if size > 0 && size != 0xFFFFFFFF {
                    data_size = size;
                    wav.data_end = ctx.pb.tell() + size;
                    next_tag_ofs = wav.data_end + (size & 1);
                } else {
                    data_size = 0;
                    wav.data_end = i64::MAX;
                    next_tag_ofs = i64::MAX;
                }
                data_ofs = ctx.pb.tell();
                if !(rf64 && !bw64) && size == 0 {
                    stop = true;
                }
            }
            b"fact" => {
                if sample_count == 0 {
                    sample_count = if !wav.rifx { ctx.pb.rl32() } else { ctx.pb.rb32() } as i64;
                }
            }
            b"bext" => {
                let ret = read_bext(&mut ctx.pb, size);
                if ret < 0 {
                    return Ok(ret);
                }
            }
            b"SMV0" => {
                if !got_fmt {
                    return Ok(AVERROR_INVALIDDATA);
                }
                if size != mktag(b"0200") as i64 {
                    stop = true;
                } else {
                    return Err(Unemulated("SMV video in WAV"));
                }
            }
            b"LIST" | b"list" => {
                if size < 4 {
                    return Ok(AVERROR_INVALIDDATA);
                }
                ctx.pb.rl32();
            }
            b"cue " if size >= 4 && got_fmt && ctx.streams[index].codecpar.sample_rate > 0 => {
                let nb_cues = ctx.pb.rl32() as i64;
                let fits = size >= nb_cues * 24 + 4;
                for _ in 0..if fits { nb_cues } else { 0 } {
                    ctx.pb.rl32();
                    if ctx.pb.feof() {
                        return Ok(AVERROR_INVALIDDATA);
                    }
                    ctx.pb.skip(16);
                    ctx.pb.rl32();
                }
            }
            _ => {}
        }
        if stop {
            break;
        }
        let file_size = ctx.pb.size();
        if (file_size > 0 && next_tag_ofs >= file_size) || seek_tag(&wav, &mut ctx.pb, next_tag_ofs) < 0 {
            break;
        }
    }
    if !got_fmt && !got_xma2 {
        return Ok(AVERROR_INVALIDDATA);
    }
    if data_ofs < 0 {
        return Ok(AVERROR_INVALIDDATA);
    }
    ctx.pb.seek(data_ofs, Whence::Set);
    if data_size > i64::MAX >> 3 {
        data_size = 0;
    }
    let file_size = ctx.pb.size();
    let st = &mut ctx.streams[index];
    if let Some(duration) = xma2_duration {
        st.duration = duration;
    }
    let par = &mut st.codecpar;
    let channels = par.ch_layout.nb_channels as i64;
    if par.bit_rate > 0 && data_size > 0 && par.sample_rate > 0 && sample_count > 0 && channels > 1 && sample_count % channels == 0 {
        let ratio = 8.0 * data_size as f64 * channels as f64 * par.sample_rate as f64 / sample_count as f64 / par.bit_rate as f64;
        if (ratio - 1.0).abs() < 0.3 {
            sample_count /= channels;
        }
    }
    if data_size > 0 && sample_count != 0 && channels != 0 && (data_size << 3) / sample_count / channels > par.bits_per_coded_sample as i64 + 1 {
        sample_count = 0;
    }
    if par.codec_id == Codec("g729") && sample_count != 0 && (data_size << 3) > sample_count {
        sample_count = 0;
    }
    let bits = codec::exact_bits_per_sample(par.codec_id) as i64;
    if (sample_count == 0 || bits > 0) && channels != 0 && data_size != 0 && bits != 0 && wav.data_end <= file_size {
        sample_count = ((data_size as u64) << 3) as i64 / (channels as u64 * bits as u64) as i64;
    }
    if sample_count != 0 {
        st.duration = sample_count;
    }
    let par = &mut st.codecpar;
    if par.codec_id == Codec("pcm_s32le")
        && par.block_align == par.ch_layout.nb_channels * 4
        && par.bits_per_coded_sample == 32
        && par.extradata.as_ref().is_some_and(|extra| extra.len() == 2 && u16::from_le_bytes([extra[0], extra[1]]) == 1)
    {
        par.codec_id = Codec("pcm_f16le");
        par.bits_per_coded_sample = 16;
    } else if par.codec_id == Codec("pcm_s24le") && par.block_align == par.ch_layout.nb_channels * 4 && par.bits_per_coded_sample == 24 {
        par.codec_id = Codec("pcm_f24le");
    } else if par.codec_id == Codec("xma1") || par.codec_id == Codec("xma2") {
        par.block_align = 2048;
    } else if par.codec_id == Codec("adpcm_ms") && par.ch_layout.nb_channels > 2 && par.block_align < i32::MAX / par.ch_layout.nb_channels {
        par.block_align *= par.ch_layout.nb_channels;
    }
    ctx.wav = wav;
    Ok(0)
}

pub fn read_packet(ctx: &mut FormatContext) -> Step<Result<Packet, i32>> {
    let mut left = ctx.wav.data_end - ctx.pb.tell();
    if left <= 0 {
        let wav = ctx.wav;
        left = find_tag(&wav, &mut ctx.pb, mktag(b"data"));
        if left < 0 {
            return Ok(Err(AVERROR_EOF));
        }
        if i64::MAX - left < ctx.pb.tell() {
            return Ok(Err(AVERROR_INVALIDDATA));
        }
        ctx.wav.data_end = ctx.pb.tell() + left;
    }
    let block_align = ctx.streams[0].codecpar.block_align;
    let mut size = MAX_SIZE;
    if block_align > 1 {
        if size < block_align {
            size = block_align;
        }
        size = (size / block_align) * block_align;
    }
    let size = (size as i64).min(left) as i32;
    let read = ctx.pb.get_packet(size);
    if read.result < 0 {
        return Ok(Err(read.result));
    }
    let mut packet = Packet::new(read.data, read.position, 0);
    if read.corrupt {
        packet.flags |= PKT_FLAG_CORRUPT;
    }
    Ok(Ok(packet))
}

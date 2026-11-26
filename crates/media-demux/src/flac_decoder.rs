use crate::bits::BitReader;
use crate::codec::sample_fmt;
use crate::flac;
use crate::flac::StreamInfo;
use crate::format::CodecParams;

pub struct FlacDecoder {
    stream_info: StreamInfo,
    got_streaminfo: bool,
    pub reached_subframes: bool,
}

pub enum Decoded {
    Consumed(usize),
    Failed,
    ReachedSubframes,
}

fn set_bps(avctx: &mut CodecParams, info: &StreamInfo) {
    avctx.format = if info.bps > 16 { sample_fmt::S32 } else { sample_fmt::S16 };
}

fn metadata_size(buf: &[u8]) -> Option<usize> {
    let mut at = 4usize;
    loop {
        if buf.len() - at < 4 {
            return None;
        }
        let block = flac::block_header(&buf[at..]);
        at += 4;
        if buf.len() - at < block.size {
            return None;
        }
        at += block.size;
        if block.last {
            return Some(at);
        }
    }
}

impl FlacDecoder {
    pub fn init(avctx: &mut CodecParams) -> Option<FlacDecoder> {
        let mut decoder = FlacDecoder { stream_info: StreamInfo::default(), got_streaminfo: false, reached_subframes: false };
        let Some(extradata) = avctx.extradata.clone() else {
            return Some(decoder);
        };
        if extradata.len() < flac::STREAMINFO_SIZE {
            return None;
        }
        let streaminfo = if extradata[..4] != *b"fLaC" {
            &extradata[..]
        } else {
            if extradata.len() < 8 + flac::STREAMINFO_SIZE {
                return None;
            }
            &extradata[8..]
        };
        if !flac::parse_streaminfo(avctx, &mut decoder.stream_info, streaminfo) {
            return None;
        }
        set_bps(avctx, &decoder.stream_info);
        decoder.got_streaminfo = true;
        Some(decoder)
    }

    fn parse_inline_streaminfo(&mut self, avctx: &mut CodecParams, buf: &[u8]) -> bool {
        if buf.len() < flac::STREAMINFO_SIZE + 8 {
            return true;
        }
        let block = flac::block_header(&buf[4..]);
        if block.kind != flac::block::STREAMINFO || block.size != flac::STREAMINFO_SIZE {
            return false;
        }
        if !flac::parse_streaminfo(avctx, &mut self.stream_info, &buf[8..]) {
            return false;
        }
        set_bps(avctx, &self.stream_info);
        self.got_streaminfo = true;
        true
    }

    fn decode_frame(&mut self, avctx: &mut CodecParams, buf: &[u8]) -> Decoded {
        let mut gb = BitReader::bytes(buf);
        let Some(fi) = flac::decode_frame_header(&mut gb) else {
            return Decoded::Failed;
        };
        let info = &mut self.stream_info;
        if info.channels != 0 && fi.channels != info.channels && self.got_streaminfo {
            info.channels = fi.channels;
            flac::set_channel_layout(avctx, fi.channels);
        }
        info.channels = fi.channels;
        flac::set_channel_layout(avctx, fi.channels);
        if info.bps == 0 && fi.bps == 0 {
            return Decoded::Failed;
        }
        let bps = if fi.bps == 0 {
            info.bps
        } else if info.bps != 0 && fi.bps != info.bps {
            return Decoded::Failed;
        } else {
            fi.bps
        };
        if info.bps == 0 {
            info.bps = bps;
            avctx.bits_per_raw_sample = bps;
            set_bps(avctx, info);
        }
        if info.max_blocksize == 0 {
            info.max_blocksize = flac::MAX_BLOCKSIZE;
        }
        if fi.blocksize > info.max_blocksize {
            return Decoded::Failed;
        }
        if info.samplerate == 0 && fi.samplerate == 0 {
            return Decoded::Failed;
        }
        let samplerate = if fi.samplerate == 0 { info.samplerate } else { fi.samplerate };
        info.samplerate = samplerate;
        avctx.sample_rate = samplerate;
        self.got_streaminfo = true;
        self.reached_subframes = true;
        Decoded::ReachedSubframes
    }

    pub fn decode(&mut self, avctx: &mut CodecParams, buf: &[u8]) -> Decoded {
        let size = buf.len();
        if size > 5 && buf[..5] == *b"\x7fFLAC" {
            return Decoded::Consumed(size);
        }
        if size > 0 && buf[0] & 0x7F == flac::block::VORBIS_COMMENT {
            return Decoded::Consumed(size);
        }
        if size < flac::MIN_FRAME_SIZE {
            return Decoded::Consumed(size);
        }
        if buf[..4] == *b"fLaC" {
            if !self.got_streaminfo && !self.parse_inline_streaminfo(avctx, buf) {
                return Decoded::Failed;
            }
            return match metadata_size(buf) {
                Some(consumed) => Decoded::Consumed(consumed),
                None => Decoded::Failed,
            };
        }
        self.decode_frame(avctx, buf)
    }
}

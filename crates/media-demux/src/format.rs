use std::collections::VecDeque;

use crate::avio::AVERROR_EOF;
use crate::avio::Avio;
use crate::avio::EINVAL;
use crate::codec;
use crate::codec::Codec;
use crate::codec::MediaType;
use crate::codec::sample_fmt;
use crate::probe;
use crate::probe::Format;
use crate::rational;
use crate::rational::NOPTS;
use crate::rational::Rational;
use crate::rational::Rounding;
use crate::rational::TIME_BASE;
use crate::wav;
use crate::wav::WavState;

pub const RELATIVE_TS_BASE: i64 = i64::MAX - (1 << 48);
pub const PKT_FLAG_KEY: u32 = 1;
pub const PKT_FLAG_CORRUPT: u32 = 2;
pub const PKT_FLAG_DISCARD: u32 = 4;
const EAGAIN: i32 = -11;
const MAX_REORDER_DELAY: usize = 16;

pub fn is_relative(ts: i64) -> bool {
    ts > RELATIVE_TS_BASE - (1 << 48)
}

#[derive(Debug)]
pub struct Unemulated(pub &'static str);

pub type Step<T> = Result<T, Unemulated>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutOrder {
    Unspec,
    Native,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChannelLayout {
    pub order: LayoutOrder,
    pub nb_channels: i32,
    pub mask: u64,
}

impl ChannelLayout {
    pub const EMPTY: ChannelLayout = ChannelLayout { order: LayoutOrder::Unspec, nb_channels: 0, mask: 0 };

    pub fn from_mask(mask: u64) -> Option<ChannelLayout> {
        if mask == 0 {
            return None;
        }
        Some(ChannelLayout { order: LayoutOrder::Native, nb_channels: mask.count_ones() as i32, mask })
    }

    pub fn unspec(nb_channels: i32) -> ChannelLayout {
        ChannelLayout { order: LayoutOrder::Unspec, nb_channels, mask: 0 }
    }

    fn check(&self) -> bool {
        if self.nb_channels <= 0 {
            return false;
        }
        match self.order {
            LayoutOrder::Native => self.mask.count_ones() as i32 == self.nb_channels,
            LayoutOrder::Unspec => true,
        }
    }
}

#[derive(Clone, Debug)]
pub struct CodecParams {
    pub codec_type: MediaType,
    pub codec_id: Codec,
    pub codec_tag: u32,
    pub extradata: Option<Vec<u8>>,
    pub format: i32,
    pub bit_rate: i64,
    pub bits_per_coded_sample: i32,
    pub bits_per_raw_sample: i32,
    pub ch_layout: ChannelLayout,
    pub sample_rate: i32,
    pub block_align: i32,
    pub frame_size: i32,
    pub initial_padding: i32,
    pub seek_preroll: i32,
    pub width: i32,
}

impl CodecParams {
    fn new() -> Self {
        CodecParams {
            codec_type: MediaType::Unknown,
            codec_id: Codec::NONE,
            codec_tag: 0,
            extradata: None,
            format: sample_fmt::NONE,
            bit_rate: 0,
            bits_per_coded_sample: 0,
            bits_per_raw_sample: 0,
            ch_layout: ChannelLayout::EMPTY,
            sample_rate: 0,
            block_align: 0,
            frame_size: 0,
            initial_padding: 0,
            seek_preroll: 0,
            width: 0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct CodecContext {
    pub params: CodecParams,
    pub delay: i32,
    pub decoder: Option<&'static str>,
    pub channel_conf: bool,
}

impl CodecContext {
    fn new() -> Self {
        CodecContext { params: CodecParams::new(), delay: 0, decoder: None, channel_conf: false }
    }

    fn set_parameters(&mut self, params: &CodecParams) {
        self.params = params.clone();
        self.delay = params.initial_padding;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FoundDecoder {
    Unknown,
    Found,
    Missing(Codec),
}

impl FoundDecoder {
    fn non_negative(self) -> bool {
        !matches!(self, FoundDecoder::Missing(_))
    }

    fn missing(codec: Codec) -> FoundDecoder {
        if codec.is_none() { FoundDecoder::Unknown } else { FoundDecoder::Missing(codec) }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Parsing {
    None,
    FullRaw,
}

#[derive(Clone, Debug)]
pub struct Packet {
    pub data: Vec<u8>,
    pub pts: i64,
    pub dts: i64,
    pub duration: i64,
    pub pos: i64,
    pub flags: u32,
    pub stream_index: usize,
    pub skip_samples: Option<(u32, u32)>,
}

impl Packet {
    pub fn new(data: Vec<u8>, pos: i64, stream_index: usize) -> Self {
        Packet { data, pts: NOPTS, dts: NOPTS, duration: 0, pos, flags: 0, stream_index, skip_samples: None }
    }
}

pub struct Stream {
    pub codecpar: CodecParams,
    pub avctx: CodecContext,
    pub time_base: Rational,
    pub pts_wrap_bits: i32,
    pub start_time: i64,
    pub duration: i64,
    pub first_dts: i64,
    pub cur_dts: i64,
    pub last_ip_pts: i64,
    pub last_ip_duration: i64,
    pts_buffer: [i64; MAX_REORDER_DELAY + 1],
    pub request_probe: i32,
    probe_packets: i32,
    probe_data: Vec<u8>,
    pub need_parsing: Parsing,
    pub need_context_update: bool,
    avctx_inited: bool,
    pub skip_samples: i64,
    pub start_skip_samples: i64,
    pub first_discard_sample: i64,
    update_initial_durations_done: bool,
    codec_info_nb_frames: i32,
    codec_info_duration: i64,
    fps_first_dts: i64,
    fps_first_dts_idx: i32,
    fps_last_dts: i64,
    fps_last_dts_idx: i32,
    found_decoder: FoundDecoder,
    nb_decoded_frames: i32,
    skip_to_keyframe: bool,
}

impl Stream {
    fn new() -> Self {
        Stream {
            codecpar: CodecParams::new(),
            avctx: CodecContext::new(),
            time_base: Rational::new(1, 90000),
            pts_wrap_bits: 33,
            start_time: NOPTS,
            duration: NOPTS,
            first_dts: NOPTS,
            cur_dts: RELATIVE_TS_BASE,
            last_ip_pts: NOPTS,
            last_ip_duration: 0,
            pts_buffer: [NOPTS; MAX_REORDER_DELAY + 1],
            request_probe: 0,
            probe_packets: 2500,
            probe_data: Vec::new(),
            need_parsing: Parsing::None,
            need_context_update: true,
            avctx_inited: false,
            skip_samples: 0,
            start_skip_samples: 0,
            first_discard_sample: 0,
            update_initial_durations_done: false,
            codec_info_nb_frames: 0,
            codec_info_duration: 0,
            fps_first_dts: NOPTS,
            fps_first_dts_idx: 0,
            fps_last_dts: NOPTS,
            fps_last_dts_idx: 0,
            found_decoder: FoundDecoder::Unknown,
            nb_decoded_frames: 0,
            skip_to_keyframe: false,
        }
    }

    pub fn set_pts_info(&mut self, wrap_bits: i32, num: u32, den: u32) {
        let (_, reduced) = rational::reduce(num as i64, den as i64, i32::MAX as i64);
        if reduced.num <= 0 || reduced.den <= 0 {
            return;
        }
        self.time_base = reduced;
        self.pts_wrap_bits = wrap_bits;
    }

    pub fn probe_packets_limit(&mut self, limit: i32) {
        self.probe_packets = self.probe_packets.min(limit);
    }

    fn has_codec_parameters(&self) -> bool {
        let avctx = &self.avctx.params;
        if avctx.codec_id.is_none() {
            return false;
        }
        match avctx.codec_type {
            MediaType::Audio => {
                if avctx.frame_size == 0 && matches!(avctx.codec_id.0, "mp1" | "mp2" | "mp3" | "codec2") {
                    return false;
                }
                if self.found_decoder.non_negative() && avctx.format == sample_fmt::NONE {
                    return false;
                }
                if avctx.sample_rate == 0 || avctx.ch_layout.nb_channels == 0 {
                    return false;
                }
                if self.found_decoder.non_negative() && self.nb_decoded_frames == 0 && avctx.codec_id == Codec("dts") {
                    return false;
                }
                true
            }
            MediaType::Video => avctx.width != 0,
            MediaType::Unknown => true,
        }
    }
}

pub struct FormatContext<'a> {
    pub pb: Avio<'a>,
    pub iformat: Format,
    pub streams: Vec<Stream>,
    pub start_time: i64,
    pub duration: i64,
    pub bit_rate: i64,
    pub data_offset: i64,
    raw_packet_buffer: VecDeque<Packet>,
    raw_packet_buffer_size: i64,
    parse_queue: VecDeque<Packet>,
    packet_buffer: VecDeque<Packet>,
    probesize: i64,
    max_analyze_duration: i64,
    max_ts_probe: i32,
    pub wav: WavState,
}

pub enum Opened<'a> {
    Context(Box<FormatContext<'a>>),
    Failed(i32),
}

fn open_decoder(avctx: &mut CodecContext, decoder: &codec::Decoder) -> Step<i32> {
    let params = &mut avctx.params;
    if params.codec_type != MediaType::Unknown && params.codec_type != MediaType::Audio {
        return Ok(EINVAL);
    }
    params.codec_type = MediaType::Audio;
    if params.sample_rate < 0 || (params.sample_rate == 0 && !decoder.channel_conf) {
        return Ok(EINVAL);
    }
    if params.block_align < 0 {
        return Ok(EINVAL);
    }
    if params.ch_layout.nb_channels == 0 && !decoder.channel_conf {
        return Ok(EINVAL);
    }
    if params.ch_layout.nb_channels != 0 && !params.ch_layout.check() {
        return Ok(EINVAL);
    }
    if params.ch_layout.nb_channels > 512 {
        return Ok(EINVAL);
    }
    if !decoder.name.starts_with("pcm_") {
        return Err(Unemulated("decoder initialization"));
    }
    params.format = decoder.sample_fmt;
    if decoder.raw_bits != 0 {
        params.bits_per_raw_sample = decoder.raw_bits;
    }
    if params.bit_rate == 0 {
        let bits = codec::exact_bits_per_sample(params.codec_id) as i64;
        if bits != 0 {
            let rate = params.sample_rate as i64 * params.ch_layout.nb_channels as i64;
            params.bit_rate = if rate > i64::MAX / bits { 0 } else { rate * bits };
        }
    }
    if params.bits_per_coded_sample < 0 {
        return Ok(EINVAL);
    }
    avctx.decoder = Some(decoder.name);
    avctx.channel_conf = decoder.channel_conf;
    Ok(0)
}

fn frame_duration(params: &CodecParams, frame_bytes: usize) -> i32 {
    let bps = codec::exact_bits_per_sample(params.codec_id) as i64;
    let ch = params.ch_layout.nb_channels as i64;
    if bps > 0 && ch > 0 && frame_bytes > 0 && ch < 32768 && bps < 32768 {
        return ((frame_bytes as i64 * 8) / (bps * ch)).max(0) as i32;
    }
    0
}

impl<'a> FormatContext<'a> {
    pub fn open(data: &'a [u8]) -> Step<Opened<'a>> {
        let mut pb = Avio::new(data);
        let probed = probe::probe_input_buffer(&mut pb);
        if probed.result < 0 {
            return Ok(Opened::Failed(probed.result));
        }
        let iformat = probed.format.expect("a successful probe names a format");
        let mut ctx = FormatContext {
            pb,
            iformat,
            streams: Vec::new(),
            start_time: NOPTS,
            duration: NOPTS,
            bit_rate: 0,
            data_offset: 0,
            raw_packet_buffer: VecDeque::new(),
            raw_packet_buffer_size: 0,
            parse_queue: VecDeque::new(),
            packet_buffer: VecDeque::new(),
            probesize: 5_000_000,
            max_analyze_duration: 60 * 1_000_000,
            max_ts_probe: 50,
            wav: WavState::default(),
        };
        if matches!(iformat, Format::Aac | Format::Flac | Format::Mp3 | Format::Wav) {
            ctx.id3v2_skip();
        }
        let ret = match iformat {
            Format::Wav => wav::read_header(&mut ctx)?,
            _ => return Err(Unemulated("demuxer outside the box scope")),
        };
        if ret < 0 {
            return Ok(Opened::Failed(ret));
        }
        if ctx.data_offset == 0 {
            ctx.data_offset = ctx.pb.tell();
        }
        ctx.raw_packet_buffer_size = 0;
        for st in &mut ctx.streams {
            if st.need_context_update {
                st.avctx.set_parameters(&st.codecpar);
                st.need_context_update = false;
            }
        }
        Ok(Opened::Context(Box::new(ctx)))
    }

    fn id3v2_skip(&mut self) {
        loop {
            let off = self.pb.tell();
            let mut header = [0u8; 10];
            if self.pb.read(&mut header) != 10 {
                self.pb.seek(off, crate::avio::Whence::Set);
                break;
            }
            if !probe::id3v2_match(&header) {
                self.pb.seek(off, crate::avio::Whence::Set);
                break;
            }
            let len = ((header[6] & 0x7f) as i64) << 21
                | ((header[7] & 0x7f) as i64) << 14
                | ((header[8] & 0x7f) as i64) << 7
                | (header[9] & 0x7f) as i64;
            let version = header[3];
            let flags = header[5];
            let mut end = self.pb.tell() + len;
            let parsed = match version {
                2 => flags & 0x40 == 0,
                3 | 4 => {
                    if flags & 0x40 != 0 {
                        let mut extlen: i64 = 0;
                        for _ in 0..4 {
                            extlen = (extlen << 7) + (self.pb.r8() & 0x7f) as i64;
                        }
                        let extlen = extlen as u32 as i32 as i64;
                        let extlen = if version == 4 { extlen - 4 } else { extlen };
                        if extlen < 0 {
                            false
                        } else {
                            self.pb.skip(extlen);
                            len - (extlen + 4) >= 0
                        }
                    } else {
                        true
                    }
                }
                _ => false,
            };
            if parsed && version == 4 && flags & 0x10 != 0 {
                end += 10;
            }
            self.pb.seek(end, crate::avio::Whence::Set);
        }
    }

    pub fn new_stream(&mut self) -> usize {
        let index = self.streams.len();
        self.streams.push(Stream::new());
        index
    }

    fn demux_packet(&mut self) -> Step<Result<Packet, i32>> {
        match self.iformat {
            Format::Wav => wav::read_packet(self),
            _ => Err(Unemulated("demuxer outside the box scope")),
        }
    }

    fn probe_codec(&mut self, index: usize, packet: Option<usize>) {
        if self.streams[index].request_probe <= 0 {
            return;
        }
        let size = {
            let st = &mut self.streams[index];
            st.probe_packets -= 1;
            match packet {
                Some(size) => size,
                None => {
                    st.probe_packets = 0;
                    0
                }
            }
        };
        if packet.is_some() {
            let data = &self.raw_packet_buffer.back().expect("the probed packet was queued").data;
            self.streams[index].probe_data.extend_from_slice(data);
        }
        let end = self.raw_packet_buffer_size >= self.probesize || self.streams[index].probe_packets <= 0;
        let buf_size = self.streams[index].probe_data.len() as u32;
        if end || rational::log2(buf_size) != rational::log2(buf_size - size as u32) {
            let score = self.set_codec_from_probe_data(index);
            let st = &mut self.streams[index];
            if (!st.codecpar.codec_id.is_none() && score > probe::SCORE_STREAM_RETRY) || end {
                st.probe_data.clear();
                st.request_probe = -1;
            }
        }
    }

    fn set_codec_from_probe_data(&mut self, index: usize) -> i32 {
        let st = &mut self.streams[index];
        let mut padded = st.probe_data.clone();
        let size = padded.len();
        padded.resize(size + probe::PADDING, 0);
        let probed = probe::probe_input_format3(&padded, size);
        let Some(format) = probed.format else {
            return 0;
        };
        let codec_id = match format {
            Format::Aac => Codec("aac"),
            Format::Mp3 => Codec("mp3"),
            _ => return 0,
        };
        if st.request_probe > probed.score && st.codecpar.codec_id != codec_id {
            return 0;
        }
        st.codecpar.codec_id = codec_id;
        st.codecpar.codec_type = MediaType::Audio;
        st.need_context_update = true;
        probed.score
    }

    fn read_packet(&mut self) -> Step<Result<Packet, i32>> {
        loop {
            let pending = !self.raw_packet_buffer.is_empty();
            if let Some(front) = self.raw_packet_buffer.front() {
                let index = front.stream_index;
                if self.raw_packet_buffer_size >= self.probesize {
                    self.probe_codec(index, None);
                }
                if self.streams[index].request_probe <= 0 {
                    let packet = self.raw_packet_buffer.pop_front().expect("raw packet buffer front exists");
                    self.raw_packet_buffer_size -= packet.data.len() as i64;
                    return Ok(Ok(packet));
                }
            }
            match self.demux_packet()? {
                Err(err) => {
                    if !pending || err == EAGAIN {
                        return Ok(Err(err));
                    }
                    for index in 0..self.streams.len() {
                        if self.streams[index].probe_packets != 0 || self.streams[index].request_probe > 0 {
                            self.probe_codec(index, None);
                        }
                    }
                }
                Ok(packet) => {
                    if self.streams[packet.stream_index].pts_wrap_bits < 63 && (packet.pts != NOPTS || packet.dts != NOPTS) {
                        return Err(Unemulated("timestamp wrapping"));
                    }
                    let index = packet.stream_index;
                    if self.streams[index].request_probe <= 0 && self.raw_packet_buffer.is_empty() {
                        return Ok(Ok(packet));
                    }
                    let size = packet.data.len();
                    self.raw_packet_buffer_size += size as i64;
                    self.raw_packet_buffer.push_back(packet);
                    self.probe_codec(index, Some(size));
                }
            }
        }
    }

    fn update_initial_timestamps(&mut self, index: usize, dts: i64, mut pts: i64, packet_flags: u32) {
        {
            let st = &self.streams[index];
            if st.first_dts != NOPTS
                || dts == NOPTS
                || st.cur_dts == NOPTS
                || st.cur_dts < i32::MIN as i64 + RELATIVE_TS_BASE
                || dts < i32::MIN as i64 + (st.cur_dts - RELATIVE_TS_BASE)
                || is_relative(dts)
            {
                return;
            }
        }
        let (shift, skip_offset, is_audio_rate) = {
            let st = &mut self.streams[index];
            st.first_dts = dts - (st.cur_dts - RELATIVE_TS_BASE);
            st.cur_dts = dts;
            let shift = (st.first_dts as u64).wrapping_sub(RELATIVE_TS_BASE as u64);
            let rate = st.codecpar.sample_rate;
            let audio = st.codecpar.codec_type == MediaType::Audio && rate != 0;
            let offset = if audio { rational::rescale_q(st.skip_samples, Rational::new(1, rate), st.time_base) } else { 0 };
            (shift, offset, audio)
        };
        if is_relative(pts) {
            pts = (pts as u64).wrapping_add(shift) as i64;
        }
        let mut start_time = self.streams[index].start_time;
        for packet in self.packet_buffer.iter_mut().chain(self.parse_queue.iter_mut()) {
            if packet.stream_index != index {
                continue;
            }
            if is_relative(packet.pts) {
                packet.pts = (packet.pts as u64).wrapping_add(shift) as i64;
            }
            if is_relative(packet.dts) {
                packet.dts = (packet.dts as u64).wrapping_add(shift) as i64;
            }
            if start_time == NOPTS && packet.pts != NOPTS {
                start_time = packet.pts;
                if is_audio_rate {
                    start_time = start_time.saturating_add(skip_offset);
                }
            }
        }
        self.streams[index].start_time = start_time;
        self.update_dts_from_pts(index);
        let st = &mut self.streams[index];
        if st.start_time == NOPTS {
            if st.codecpar.codec_type == MediaType::Audio || packet_flags & PKT_FLAG_DISCARD == 0 {
                st.start_time = pts;
            }
            if is_audio_rate {
                st.start_time = st.start_time.saturating_add(skip_offset);
            }
        }
    }

    fn update_dts_from_pts(&mut self, index: usize) {
        for packet in self.packet_buffer.iter_mut().chain(self.parse_queue.iter_mut()) {
            if packet.stream_index != index {
                continue;
            }
            if packet.pts != NOPTS && packet.dts == NOPTS {
                packet.dts = packet.pts;
            }
        }
    }

    fn update_initial_durations(&mut self, index: usize, duration: i64) {
        let mut cur_dts = RELATIVE_TS_BASE;
        let first_dts = self.streams[index].first_dts;
        if first_dts != NOPTS {
            if self.streams[index].update_initial_durations_done {
                return;
            }
            self.streams[index].update_initial_durations_done = true;
            cur_dts = first_dts;
            let mut stopped: Option<i64> = None;
            for packet in self.packet_buffer.iter().chain(self.parse_queue.iter()) {
                if packet.stream_index == index {
                    if packet.pts != packet.dts || packet.dts != NOPTS || packet.duration != 0 {
                        stopped = Some(packet.dts);
                        break;
                    }
                    cur_dts -= duration;
                }
            }
            match stopped {
                Some(dts) if dts != first_dts => return,
                None => return,
                Some(_) => {}
            }
            self.streams[index].first_dts = cur_dts;
        } else if self.streams[index].cur_dts != RELATIVE_TS_BASE {
            return;
        }
        let first_dts = self.streams[index].first_dts;
        let mut reached_end = true;
        for packet in self.packet_buffer.iter_mut().chain(self.parse_queue.iter_mut()) {
            if packet.stream_index != index {
                continue;
            }
            if (packet.pts == packet.dts || packet.pts == NOPTS)
                && (packet.dts == NOPTS || packet.dts == first_dts || packet.dts == RELATIVE_TS_BASE)
                && packet.duration == 0
                && cur_dts.checked_add(duration).is_some()
            {
                packet.dts = cur_dts;
                packet.pts = cur_dts;
                packet.duration = duration;
            } else {
                reached_end = false;
                break;
            }
            cur_dts = packet.dts + packet.duration;
        }
        if reached_end {
            self.streams[index].cur_dts = cur_dts;
        }
    }

    fn compute_pkt_fields(&mut self, index: usize, packet: &mut Packet) {
        let time_base = self.streams[index].time_base;
        let mut duration = rational::mul_q(Rational::new(packet.duration as i32, 1), time_base);
        if packet.duration <= 0 {
            let st = &self.streams[index];
            let params = if st.avctx_inited { &st.avctx.params } else { &st.codecpar };
            if params.codec_type == MediaType::Audio {
                let frame_size = frame_duration(params, packet.data.len());
                let sample_rate = params.sample_rate;
                if frame_size > 0 && sample_rate > 0 {
                    duration = Rational::new(frame_size, sample_rate);
                    packet.duration = rational::rescale_rnd(
                        1,
                        frame_size as i64 * time_base.den as i64,
                        sample_rate as i64 * time_base.num as i64,
                        Rounding::Down,
                        false,
                    );
                }
            }
        }
        if packet.duration > 0 && (!self.packet_buffer.is_empty() || !self.parse_queue.is_empty()) {
            self.update_initial_durations(index, packet.duration);
        }
        let presentation_delayed = packet.dts != NOPTS && packet.pts != NOPTS && packet.pts > packet.dts;
        if presentation_delayed {
            let st = &mut self.streams[index];
            if packet.dts == NOPTS {
                packet.dts = st.last_ip_pts;
            }
            let (dts, pts, flags) = (packet.dts, packet.pts, packet.flags);
            self.update_initial_timestamps(index, dts, pts, flags);
            let st = &mut self.streams[index];
            if packet.dts == NOPTS {
                packet.dts = st.cur_dts;
            }
            if st.last_ip_duration == 0 && (packet.duration as u64) <= i32::MAX as u64 {
                st.last_ip_duration = packet.duration;
            }
            if packet.dts != NOPTS {
                st.cur_dts = packet.dts.saturating_add(st.last_ip_duration);
            }
            if (packet.duration as u64) <= i32::MAX as u64 {
                st.last_ip_duration = packet.duration;
            }
            st.last_ip_pts = packet.pts;
        } else if packet.pts != NOPTS || packet.dts != NOPTS || packet.duration > 0 {
            if packet.pts == NOPTS {
                packet.pts = packet.dts;
            }
            let (pts, flags) = (packet.pts, packet.flags);
            self.update_initial_timestamps(index, pts, pts, flags);
            let st = &mut self.streams[index];
            if packet.pts == NOPTS {
                packet.pts = st.cur_dts;
            }
            packet.dts = packet.pts;
            if packet.pts != NOPTS && duration.num >= 0 {
                st.cur_dts = rational::add_stable(st.time_base, packet.pts, duration, 1);
            }
        }
        let st = &mut self.streams[index];
        if packet.pts != NOPTS {
            st.pts_buffer[0] = packet.pts;
            if packet.dts == NOPTS {
                packet.dts = st.pts_buffer[0];
            }
        }
        if packet.dts > st.cur_dts {
            st.cur_dts = packet.dts;
        }
        if codec::intra_only(st.codecpar.codec_id) {
            packet.flags |= PKT_FLAG_KEY;
        }
    }

    fn read_frame_internal(&mut self) -> Step<Result<Packet, i32>> {
        let mut got: Option<Packet> = None;
        let mut ret = 0;
        while got.is_none() && self.parse_queue.is_empty() {
            match self.read_packet()? {
                Err(err) => {
                    if err == EAGAIN {
                        return Ok(Err(err));
                    }
                    ret = err;
                    break;
                }
                Ok(mut packet) => {
                    ret = 0;
                    let index = packet.stream_index;
                    if self.streams[index].need_context_update {
                        let st = &mut self.streams[index];
                        if st.avctx.decoder.is_some() {
                            st.avctx.decoder = None;
                            st.found_decoder = FoundDecoder::Unknown;
                        }
                        let params = st.codecpar.clone();
                        st.avctx.set_parameters(&params);
                        st.need_context_update = false;
                    }
                    if self.streams[index].need_parsing != Parsing::None {
                        if codec::has_parser(self.streams[index].codecpar.codec_id) {
                            return Err(Unemulated("codec parser"));
                        }
                        self.streams[index].need_parsing = Parsing::None;
                    }
                    self.compute_pkt_fields(index, &mut packet);
                    let st = &mut self.streams[index];
                    if packet.flags & PKT_FLAG_KEY != 0 {
                        st.skip_to_keyframe = false;
                    }
                    if !st.skip_to_keyframe {
                        got = Some(packet);
                    }
                }
            }
        }
        let mut packet = match got {
            Some(packet) => packet,
            None => match self.parse_queue.pop_front() {
                Some(packet) => packet,
                None => {
                    if ret == AVERROR_EOF && self.pb.error < 0 && self.pb.error != EAGAIN {
                        return Ok(Err(self.pb.error));
                    }
                    return Ok(Err(ret));
                }
            },
        };
        let st = &mut self.streams[packet.stream_index];
        if st.first_discard_sample != 0 && packet.pts != NOPTS {
            return Err(Unemulated("discard sample range"));
        }
        if st.start_skip_samples != 0 && (packet.pts == 0 || packet.pts == RELATIVE_TS_BASE) {
            st.skip_samples = st.start_skip_samples;
        }
        st.skip_samples = st.skip_samples.max(0);
        if st.skip_samples != 0 {
            packet.skip_samples = Some((st.skip_samples as u32, 0));
            st.skip_samples = 0;
        }
        Ok(Ok(packet))
    }

    pub fn read_frame(&mut self) -> Step<Result<Packet, i32>> {
        let mut packet = match self.packet_buffer.pop_front() {
            Some(packet) => packet,
            None => match self.read_frame_internal()? {
                Ok(packet) => packet,
                Err(err) => return Ok(Err(err)),
            },
        };
        if is_relative(packet.dts) {
            packet.dts -= RELATIVE_TS_BASE;
        }
        if is_relative(packet.pts) {
            packet.pts -= RELATIVE_TS_BASE;
        }
        Ok(Ok(packet))
    }

    fn try_decode_frame(&mut self, index: usize) -> Step<i32> {
        let st = &mut self.streams[index];
        if st.avctx.decoder.is_none()
            && !matches!(st.found_decoder, FoundDecoder::Found)
            && (st.found_decoder != FoundDecoder::missing(st.codecpar.codec_id) || st.codecpar.codec_id.is_none())
        {
            let Some(decoder) = codec::find_decoder(st.codecpar.codec_id) else {
                st.found_decoder = FoundDecoder::missing(st.codecpar.codec_id);
                return Ok(-1);
            };
            if open_decoder(&mut st.avctx, &decoder)? < 0 {
                st.found_decoder = FoundDecoder::missing(st.avctx.params.codec_id);
                return Ok(-1);
            }
            st.found_decoder = FoundDecoder::Found;
        } else if st.found_decoder == FoundDecoder::Unknown {
            st.found_decoder = FoundDecoder::Found;
        }
        if !st.found_decoder.non_negative() {
            return Ok(-1);
        }
        let first_frame_conf = st.codec_info_nb_frames == 0 && st.avctx.channel_conf;
        if !st.has_codec_parameters() || first_frame_conf {
            return Err(Unemulated("decoding during stream analysis"));
        }
        Ok(0)
    }

    pub fn find_stream_info(&mut self) -> Step<i32> {
        let old_offset = self.pb.tell();
        let mut flush_codecs = self.probesize > 0;
        let mut count = 0;
        let mut ret: i32 = 0;
        let mut eof_reached = false;
        for index in 0..self.streams.len() {
            let st = &mut self.streams[index];
            if st.request_probe <= 0 && st.need_parsing != Parsing::None && codec::has_parser(st.codecpar.codec_id) {
                return Err(Unemulated("codec parser"));
            }
            let params = st.codecpar.clone();
            st.avctx.set_parameters(&params);
            if st.request_probe <= 0 {
                st.avctx_inited = true;
            }
            let decoder = codec::find_decoder(st.codecpar.codec_id);
            if !st.has_codec_parameters()
                && st.request_probe <= 0
                && let Some(decoder) = decoder
                && st.avctx.decoder.is_none()
            {
                open_decoder(&mut st.avctx, &decoder)?;
            }
        }
        let mut read_size: i64 = 0;
        loop {
            for st in &mut self.streams {
                if st.need_context_update {
                    if st.avctx.decoder.is_some() {
                        st.avctx.decoder = None;
                        st.found_decoder = FoundDecoder::Unknown;
                    }
                    let params = st.codecpar.clone();
                    st.avctx.set_parameters(&params);
                    st.need_context_update = false;
                }
            }
            let mut stopped = false;
            for st in &self.streams {
                if !st.has_codec_parameters() {
                    stopped = true;
                    break;
                }
                if st.first_dts == NOPTS
                    && st.codec_info_nb_frames < self.max_ts_probe
                    && matches!(st.codecpar.codec_type, MediaType::Audio | MediaType::Video)
                {
                    stopped = true;
                    break;
                }
            }
            if !stopped {
                ret = count;
                flush_codecs = false;
                break;
            }
            if read_size >= self.probesize {
                ret = count;
                break;
            }
            let packet = match self.read_frame_internal()? {
                Err(err) if err == EAGAIN => continue,
                Err(err) => {
                    ret = err;
                    eof_reached = true;
                    break;
                }
                Ok(packet) => packet,
            };
            let index = packet.stream_index;
            let size = packet.data.len() as i64;
            let (dts, packet_duration) = (packet.dts, packet.duration);
            self.packet_buffer.push_back(packet);
            read_size += size;
            let st = &mut self.streams[index];
            if !st.avctx_inited {
                let params = st.codecpar.clone();
                st.avctx.set_parameters(&params);
                st.avctx_inited = true;
            }
            if dts != NOPTS && st.codec_info_nb_frames > 1 {
                if st.fps_last_dts != NOPTS && st.fps_last_dts >= dts {
                    st.fps_first_dts = NOPTS;
                    st.fps_last_dts = NOPTS;
                }
                if st.fps_last_dts != NOPTS
                    && st.fps_last_dts_idx > st.fps_first_dts_idx
                    && (dts as u64).wrapping_sub(st.fps_last_dts as u64) / 1000
                        > (st.fps_last_dts as u64).wrapping_sub(st.fps_first_dts as u64)
                            / (st.fps_last_dts_idx - st.fps_first_dts_idx) as u64
                {
                    st.fps_first_dts = NOPTS;
                    st.fps_last_dts = NOPTS;
                }
                if st.fps_first_dts == NOPTS {
                    st.fps_first_dts = dts;
                    st.fps_first_dts_idx = st.codec_info_nb_frames;
                }
                st.fps_last_dts = dts;
                st.fps_last_dts_idx = st.codec_info_nb_frames;
            }
            if st.codec_info_nb_frames > 1 {
                let mut t: i64 = 0;
                if st.time_base.den > 0 {
                    t = rational::rescale_q(st.codec_info_duration, st.time_base, TIME_BASE);
                }
                if t == 0 && st.codec_info_nb_frames > 30 && st.fps_first_dts != NOPTS && st.fps_last_dts != NOPTS {
                    let span = st.fps_last_dts.saturating_sub(st.fps_first_dts);
                    t = t.max(rational::rescale_q(span, st.time_base, TIME_BASE));
                }
                if t >= self.max_analyze_duration {
                    break;
                }
                if packet_duration > 0 && packet_duration < i64::MAX - st.codec_info_duration {
                    st.codec_info_duration += packet_duration;
                }
            }
            self.try_decode_frame(index)?;
            self.streams[index].codec_info_nb_frames += 1;
            count += 1;
        }
        if eof_reached {
            for st in &mut self.streams {
                if !st.has_codec_parameters()
                    && let Some(decoder) = codec::find_decoder(st.codecpar.codec_id)
                    && st.avctx.decoder.is_none()
                {
                    open_decoder(&mut st.avctx, &decoder)?;
                }
            }
        }
        if flush_codecs {
            for index in 0..self.streams.len() {
                if self.streams[index].found_decoder == FoundDecoder::Found {
                    self.try_decode_frame(index)?;
                }
            }
        }
        for st in &mut self.streams {
            if st.avctx.params.codec_type == MediaType::Audio && st.avctx.params.bits_per_coded_sample == 0 {
                st.avctx.params.bits_per_coded_sample = codec::exact_bits_per_sample(st.avctx.params.codec_id);
            }
        }
        self.estimate_timings(old_offset);
        if ret >= 0 && !self.streams.is_empty() {
            ret = -1;
        }
        for st in &mut self.streams {
            if !st.avctx_inited {
                if st.codecpar.codec_type == MediaType::Audio && st.codecpar.format == sample_fmt::NONE {
                    st.codecpar.format = st.avctx.params.format;
                }
                let params = st.codecpar.clone();
                st.avctx.set_parameters(&params);
                ret = 0;
            }
            if st.has_codec_parameters() {
                ret = 0;
            }
        }
        for st in &mut self.streams {
            if st.avctx_inited {
                st.codecpar = st.avctx.params.clone();
                ret = 0;
            }
            st.avctx_inited = false;
        }
        Ok(ret)
    }

    fn estimate_timings(&mut self, _old_offset: i64) {
        let has_duration = self.streams.iter().any(|st| st.duration != NOPTS) || self.duration != NOPTS;
        if has_duration {
            self.update_stream_timings();
            for st in &mut self.streams {
                if st.start_time == NOPTS {
                    if self.start_time != NOPTS {
                        st.start_time = rational::rescale_q(self.start_time, TIME_BASE, st.time_base);
                    }
                    if self.duration != NOPTS {
                        st.duration = rational::rescale_q(self.duration, TIME_BASE, st.time_base);
                    }
                }
            }
        } else {
            self.estimate_timings_from_bit_rate();
        }
        self.update_stream_timings();
    }

    fn estimate_timings_from_bit_rate(&mut self) {
        if self.bit_rate <= 0 {
            let mut bit_rate: i64 = 0;
            for st in &mut self.streams {
                if st.codecpar.bit_rate <= 0 && st.avctx.params.bit_rate > 0 {
                    st.codecpar.bit_rate = st.avctx.params.bit_rate;
                }
                if st.codecpar.bit_rate > 0 {
                    if i64::MAX - st.codecpar.bit_rate < bit_rate {
                        bit_rate = 0;
                        break;
                    }
                    bit_rate += st.codecpar.bit_rate;
                } else if st.codecpar.codec_type == MediaType::Video && st.codec_info_nb_frames > 1 {
                    bit_rate = 0;
                    break;
                }
            }
            self.bit_rate = bit_rate;
        }
        if self.duration == NOPTS && self.bit_rate != 0 {
            let filesize = self.pb.size();
            if filesize > self.data_offset {
                let filesize = filesize - self.data_offset;
                for st in &mut self.streams {
                    if (st.time_base.num as i64) <= i64::MAX / self.bit_rate && st.duration == NOPTS {
                        st.duration = rational::rescale(filesize, 8 * st.time_base.den as i64, self.bit_rate * st.time_base.num as i64);
                    }
                }
            }
        }
    }

    fn update_stream_timings(&mut self) {
        let mut start_time = i64::MAX;
        let mut end_time = i64::MIN;
        let mut duration = i64::MIN;
        for st in &self.streams {
            if st.start_time != NOPTS && st.time_base.den != 0 {
                let start_time1 = rational::rescale_q(st.start_time, st.time_base, TIME_BASE);
                start_time = start_time.min(start_time1);
                let end_time1 = rational::rescale_q_rnd(st.duration, st.time_base, TIME_BASE, Rounding::NearInf, true);
                if end_time1 != NOPTS
                    && (if end_time1 > 0 { start_time1 <= i64::MAX - end_time1 } else { start_time1 >= i64::MIN - end_time1 })
                {
                    end_time = end_time.max(end_time1 + start_time1);
                }
            }
            if st.duration != NOPTS {
                duration = duration.max(rational::rescale_q(st.duration, st.time_base, TIME_BASE));
            }
        }
        if start_time != i64::MAX {
            self.start_time = start_time;
            if end_time != i64::MIN && end_time >= start_time && (end_time as u64).wrapping_sub(start_time as u64) <= i64::MAX as u64 {
                duration = duration.max(end_time - start_time);
            }
        }
        if duration != i64::MIN && duration > 0 && self.duration == NOPTS {
            self.duration = duration;
        }
        let filesize = self.pb.size();
        if filesize > 0 && self.duration > 0 {
            let bitrate = filesize as f64 * 8.0 * 1_000_000.0 / self.duration as f64;
            if bitrate >= 0.0 && bitrate <= i64::MAX as f64 {
                self.bit_rate = bitrate as i64;
            }
        }
    }

    pub fn first_dts(&self, index: usize) -> i64 {
        self.streams[index].first_dts
    }
}

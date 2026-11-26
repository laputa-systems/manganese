use crate::avio::AVERROR_INVALIDDATA;
use crate::avio::Avio;
use crate::codec::Codec;
use crate::codec::MediaType;
use crate::codec::sample_fmt;
use crate::format::CodecParams;
use crate::format::FormatContext;
use crate::format::LayoutOrder;
use crate::format::Opened;
use crate::format::PKT_FLAG_DISCARD;
use crate::format::PKT_FLAG_KEY;
use crate::format::Packet;
use crate::format::Step;
use crate::format::Unemulated;
use crate::mpegaudio;
use crate::probe;
use crate::probe::Format;
use crate::rational;
use crate::rational::MICROS;
use crate::rational::NOPTS;
use crate::rational::Rational;
use crate::rational::TIME_BASE;

const NO_TIMESTAMP: i64 = i64::MIN;
const INFINITE: i64 = i64::MAX;

const PIPELINE_OK: i32 = 0;
const DEMUXER_ERROR_COULD_NOT_OPEN: i32 = 12;
const DEMUXER_ERROR_COULD_NOT_PARSE: i32 = 13;
const DEMUXER_ERROR_NO_SUPPORTED_STREAMS: i32 = 14;
const DEMUXER_ERROR_DETECTED_HLS: i32 = 22;

pub const SCOPED_FORMATS: &[Format] = &[Format::Flac, Format::Mp3, Format::Ogg, Format::Wav];

fn time(value: i64) -> String {
    match value {
        NO_TIMESTAMP => "none".to_string(),
        INFINITE => "inf".to_string(),
        other => other.to_string(),
    }
}

fn convert(time_base: Rational, ts: i64) -> i64 {
    if ts == NOPTS {
        return NO_TIMESTAMP;
    }
    rational::rescale_q(ts, time_base, MICROS)
}

fn hex(data: &[u8]) -> String {
    data.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[derive(Clone, Default)]
struct Ranges(Vec<(i64, i64)>);

impl Ranges {
    fn add(&mut self, start: i64, end: i64) {
        if start == end {
            return;
        }
        let ranges = &mut self.0;
        let mut i = 0;
        while i < ranges.len() && ranges[i].1 < start {
            i += 1;
        }
        if i == ranges.len() {
            ranges.push((start, end));
            return;
        }
        if end < ranges[i].0 {
            ranges.insert(i, (start, end));
            return;
        }
        if start < ranges[i].0 {
            ranges[i].0 = start;
        }
        if ranges[i].1 < end {
            ranges[i].1 = end;
        }
        while i + 1 < ranges.len() && ranges[i + 1].0 <= ranges[i].1 {
            ranges[i].1 = ranges[i].1.max(ranges[i + 1].1);
            ranges.remove(i + 1);
        }
    }

    fn intersection(&self, other: &Ranges) -> Ranges {
        let mut result = Ranges::default();
        let (mut i, mut j) = (0, 0);
        while i < self.0.len() && j < other.0.len() {
            let start = self.0[i].0.max(other.0[j].0);
            let end = self.0[i].1.min(other.0[j].1);
            if start < end {
                result.add(start, end);
            }
            if self.0[i].1 < other.0[j].1 {
                i += 1;
            } else {
                j += 1;
            }
        }
        result
    }
}

struct AudioConfig {
    codec: i32,
    sample_format: i32,
    channel_layout: i32,
    channels: i32,
    samples_per_second: i32,
    extra_data: Vec<u8>,
    seek_preroll: i64,
    codec_delay: i32,
}

impl AudioConfig {
    fn describe(&self) -> String {
        format!(
            "codec={} profile=0 format={} layout={} channels={} rate={} preroll={} delay={} encryption=0 discard_delay=1 extra={}",
            self.codec,
            self.sample_format,
            self.channel_layout,
            self.channels,
            self.samples_per_second,
            self.seek_preroll,
            self.codec_delay,
            hex(&self.extra_data)
        )
    }
}

fn audio_codec(codec: Codec) -> i32 {
    match codec.0 {
        "aac" => 1,
        "mp3" => 2,
        "pcm_u8" | "pcm_s16le" | "pcm_s24le" | "pcm_s32le" | "pcm_f32le" => 3,
        "vorbis" => 4,
        "flac" => 5,
        "pcm_mulaw" => 8,
        "pcm_s16be" => 10,
        "pcm_s24be" => 11,
        "opus" => 12,
        "pcm_alaw" => 14,
        "alac" => 15,
        _ => 0,
    }
}

fn sample_format(format: i32, codec: Codec) -> i32 {
    match format {
        sample_fmt::U8 => 1,
        sample_fmt::S16 => 2,
        sample_fmt::S32 => {
            if codec == Codec("pcm_s24le") {
                8
            } else {
                3
            }
        }
        sample_fmt::FLT => 4,
        sample_fmt::S16P => 5,
        sample_fmt::S32P => 7,
        sample_fmt::FLTP => 6,
        _ => 0,
    }
}

fn bytes_per_channel(format: i32) -> i32 {
    match format {
        1 | 12 => 1,
        2 | 5 => 2,
        3 | 4 | 6 | 7 | 8 => 4,
        _ => 0,
    }
}

const LAYOUT_CHANNELS: [i32; 38] = [
    0, 0, 1, 2, 3, 3, 4, 4, 4, 5, 6, 5, 6, 7, 8, 8, 2, 3, 4, 5, 6, 6, 6, 7, 7, 7, 7, 8, 8, 0, 3, 5, 0, 6, 2, 4, 10, 12,
];
const LAYOUT_UNSUPPORTED: i32 = 1;
const LAYOUT_DISCRETE: i32 = 29;

fn guess_channel_layout(channels: i32) -> i32 {
    if channels > 12 && channels <= 32 {
        return LAYOUT_DISCRETE;
    }
    match channels {
        1 => 2,
        2 => 3,
        3 => 5,
        4 => 8,
        5 => 9,
        6 => 10,
        7 => 23,
        8 => 14,
        9 => LAYOUT_DISCRETE,
        10 => 36,
        11 => LAYOUT_DISCRETE,
        12 => 37,
        _ => LAYOUT_UNSUPPORTED,
    }
}

fn chrome_channel_layout(mask: u64, nb_channels: i32) -> i32 {
    match mask {
        0x4 => 2,
        0x3 => 3,
        0xB => 17,
        0x103 => 4,
        0x7 => 5,
        0xF => 18,
        0x107 => 6,
        0x10F => 19,
        0x603 => 7,
        0x33 => 8,
        0x607 => 9,
        0x60F => 10,
        0x37 => 11,
        0x3F => 12,
        0x707 => 20,
        0x6C3 => 21,
        0x137 => 22,
        0x70F => 23,
        0x13F => 24,
        0x6CB => 25,
        0x637 => 13,
        0x6C7 => 26,
        0x63F => 14,
        0x6CF => 15,
        0xFF => 27,
        0x737 => 28,
        0x60000000 => 16,
        0xC => 34,
        0x10B => 35,
        0x2D03F => 36,
        0x2D63F => 37,
        _ => guess_channel_layout(nb_channels),
    }
}

fn audio_config(par: &CodecParams) -> Step<Option<AudioConfig>> {
    let codec = audio_codec(par.codec_id);
    let format = sample_format(par.format, par.codec_id);
    let mask = if par.ch_layout.order == LayoutOrder::Native { par.ch_layout.mask } else { 0 };
    let mut layout = chrome_channel_layout(mask, par.ch_layout.nb_channels);
    if layout != LAYOUT_DISCRETE && LAYOUT_CHANNELS[layout as usize] != par.ch_layout.nb_channels {
        layout = guess_channel_layout(par.ch_layout.nb_channels);
    }
    let extra_data = match &par.extradata {
        Some(data) if data.is_empty() => return Ok(None),
        Some(data) => data.clone(),
        None => Vec::new(),
    };
    let codec_delay = if codec == 1 { 0 } else { par.initial_padding };
    if codec == 1 {
        return Err(Unemulated("AAC profile"));
    }
    let channels = if layout == LAYOUT_DISCRETE { par.ch_layout.nb_channels } else { LAYOUT_CHANNELS[layout as usize] };
    let config = AudioConfig {
        codec,
        sample_format: format,
        channel_layout: layout,
        channels,
        samples_per_second: par.sample_rate,
        extra_data,
        seek_preroll: rational::rescale(par.seek_preroll as i64, 1_000_000, par.sample_rate.max(1) as i64),
        codec_delay,
    };
    let bytes = bytes_per_channel(config.sample_format);
    let valid = config.codec != 0
        && config.channel_layout != LAYOUT_UNSUPPORTED
        && bytes > 0
        && bytes <= 4
        && (3000..=768000).contains(&config.samples_per_second)
        && config.sample_format != 0
        && config.codec_delay >= 0;
    let supported = matches!(config.codec, 2 | 3 | 4 | 5 | 8 | 10 | 11 | 12 | 14);
    Ok(if valid && supported { Some(config) } else { None })
}

struct DemuxStream {
    time_base: Rational,
    config: AudioConfig,
    enabled: bool,
    duration: i64,
    last_packet_pos: i64,
    last_packet_dts: i64,
    last_packet_timestamp: i64,
    last_packet_duration: i64,
    fixup_chained_ogg: bool,
    initial_start_padding: Option<i64>,
    ranges: Ranges,
    output: String,
}

fn frames_to_time(frames: u32, samples_per_second: i32) -> i64 {
    ((frames as i64 * MICROS.den as i64) as f64 / samples_per_second as f64) as i64
}

fn discard_padding(packet: &Packet, samples_per_second: i32) -> Option<(i64, i64)> {
    if let Some((front, back)) = packet.skip_samples
        && (front != 0 || back != 0)
    {
        return Some((frames_to_time(front, samples_per_second), frames_to_time(back, samples_per_second)));
    }
    if packet.flags & PKT_FLAG_DISCARD != 0 {
        return Some((INFINITE, 0));
    }
    None
}

fn valid_mp3_header(data: &[u8]) -> bool {
    if data.len() < mpegaudio::HEADER_SIZE {
        return false;
    }
    let header = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
    mpegaudio::check_header(header) && (header >> 12) & 0xf != 0
}

fn add_time(a: i64, b: i64) -> i64 {
    if a == INFINITE || a == NO_TIMESTAMP {
        return a;
    }
    a.saturating_add(b)
}

struct Host {
    durations: Vec<i64>,
    buffered: Ranges,
    errors: Vec<i32>,
}

impl DemuxStream {
    fn enqueue(&mut self, packet: &mut Packet, container: Format, start: i64, host: &mut Host, buffered_changed: &mut bool) -> Step<()> {
        let packet_dts = if packet.dts == NOPTS { packet.pts } else { packet.dts };
        if self.last_packet_pos != NOPTS {
            if packet.pos == -1 {
                packet.pos = self.last_packet_pos;
            }
            if packet.pos < self.last_packet_pos {
                return Ok(());
            }
            if self.last_packet_dts != NOPTS && packet.pos == self.last_packet_pos && packet_dts <= self.last_packet_dts {
                return Ok(());
            }
        }
        self.last_packet_pos = packet.pos;
        self.last_packet_dts = packet_dts;
        let mut data_offset = 0;
        if container == Format::Mp3 {
            data_offset = packet.data.iter().take_while(|&&byte| byte == 0).count();
            if !valid_mp3_header(&packet.data[data_offset..]) {
                return Ok(());
            }
        }
        let payload = &packet.data[data_offset..];
        let d = convert(self.time_base, packet.duration);
        let duration = if d <= 1000 { NO_TIMESTAMP } else { d };
        let mut timestamp = convert(self.time_base, packet.pts);
        if self.config.codec_delay != 0 && start < 0 && timestamp != NO_TIMESTAMP {
            timestamp -= start;
        }
        if self.fixup_chained_ogg
            && (timestamp == NO_TIMESTAMP
                || (self.last_packet_timestamp != NO_TIMESTAMP && timestamp < self.last_packet_timestamp))
        {
            timestamp = if self.last_packet_timestamp != NO_TIMESTAMP {
                self.last_packet_timestamp + self.last_packet_duration.max(1)
            } else {
                0
            };
        }
        if timestamp == NO_TIMESTAMP || timestamp == INFINITE {
            host.errors.push(DEMUXER_ERROR_COULD_NOT_PARSE);
            return Ok(());
        }
        let padding = discard_padding(packet, self.config.samples_per_second);
        if self.last_packet_timestamp != NO_TIMESTAMP && self.last_packet_timestamp < timestamp && timestamp >= 0 {
            self.ranges.add(self.last_packet_timestamp, timestamp);
            *buffered_changed = true;
        }
        let key = packet.flags & PKT_FLAG_KEY != 0;
        let (start_padding, end_padding) = padding.unwrap_or((0, 0));
        if self.initial_start_padding.is_none() && start_padding != INFINITE {
            self.initial_start_padding = Some(start_padding);
        }
        self.last_packet_timestamp = timestamp;
        self.last_packet_duration = duration;
        let is_padding = duration == add_time(start_padding, end_padding);
        let new_duration = timestamp - self.initial_start_padding.unwrap_or(0);
        if (!is_padding && new_duration > self.duration) || self.duration == NO_TIMESTAMP {
            self.duration = new_duration;
        }
        let discard = match padding {
            Some((front, back)) => format!("{},{}", time(front), time(back)),
            None => "none".to_string(),
        };
        use sha2::Digest;
        let digest = sha2::Sha256::digest(payload);
        self.output.push_str(&format!(
            "{} {} {} {} {} {}\n",
            time(timestamp),
            time(duration),
            if key { 1 } else { 0 },
            discard,
            payload.len(),
            hex(&digest)
        ));
        Ok(())
    }
}

fn detected_hls(data: &[u8]) -> bool {
    let buffer = &data[..data.len().min(8192)];
    if buffer.len() < 12 || !buffer.starts_with(b"#EXTM3U") {
        return false;
    }
    (7..buffer.len()).any(|offset| {
        let rest = &buffer[offset..];
        rest[0] == b'#'
            && (rest.starts_with(b"#EXT-X-STREAM-INF:") || rest.starts_with(b"#EXT-X-TARGETDURATION:") || rest.starts_with(b"#EXT-X-MEDIA-SEQUENCE:"))
    })
}

fn start_time(ctx: &FormatContext, index: usize) -> i64 {
    let st = &ctx.streams[index];
    let mut start = 0;
    if st.start_time != NOPTS {
        start = convert(st.time_base, st.start_time);
    }
    let first_dts = ctx.first_dts(index);
    if first_dts != NOPTS && !matches!(st.codecpar.codec_id.0, "hevc" | "h264" | "mpeg4") {
        let first_pts = convert(st.time_base, first_dts);
        if first_pts < start {
            start = first_pts;
        }
    }
    start
}

pub fn demux(data: &[u8]) -> Step<String> {
    let mut pb = Avio::new(data);
    let probed = probe::probe_input_buffer(&mut pb).format;
    let mut output = format!("probe {}\n", probed.map_or("none", Format::name));
    if let Some(format) = probed
        && !SCOPED_FORMATS.contains(&format)
    {
        return Ok(output);
    }
    let mut ctx = match FormatContext::open(data)? {
        Opened::Context(ctx) => ctx,
        Opened::Failed(ret) => {
            let status = if ret == AVERROR_INVALIDDATA && detected_hls(data) { DEMUXER_ERROR_DETECTED_HLS } else { DEMUXER_ERROR_COULD_NOT_OPEN };
            output.push_str(&format!("open {status}\n"));
            return Ok(output);
        }
    };
    if ctx.find_stream_info()? < 0 {
        output.push_str(&format!("open {DEMUXER_ERROR_COULD_NOT_PARSE}\n"));
        return Ok(output);
    }
    let mut streams: Vec<Option<DemuxStream>> = Vec::new();
    let mut max_duration: i64 = 0;
    let mut demuxer_start = INFINITE;
    let mut audio_tracks = 0;
    for index in 0..ctx.streams.len() {
        streams.push(None);
        let st = &ctx.streams[index];
        if st.codecpar.codec_id.is_none() {
            continue;
        }
        match st.codecpar.codec_type {
            MediaType::Audio => {}
            MediaType::Video if matches!(st.codecpar.codec_id.0, "gif" | "mjpeg" | "jpegxl" | "png" | "tiff" | "bmp" | "webp") => continue,
            MediaType::Video => return Err(Unemulated("video stream")),
            MediaType::Unknown => continue,
        }
        let Some(config) = audio_config(&st.codecpar)? else {
            continue;
        };
        audio_tracks += 1;
        let stream = DemuxStream {
            time_base: st.time_base,
            config,
            enabled: audio_tracks == 1,
            duration: convert(st.time_base, st.duration),
            last_packet_pos: NOPTS,
            last_packet_dts: NOPTS,
            last_packet_timestamp: NO_TIMESTAMP,
            last_packet_duration: 0,
            fixup_chained_ogg: ctx.iformat == Format::Ogg,
            initial_start_padding: None,
            ranges: Ranges::default(),
            output: String::new(),
        };
        max_duration = max_duration.max(stream.duration);
        let start = start_time(&ctx, index);
        if start != NO_TIMESTAMP && start < demuxer_start {
            demuxer_start = start;
        }
        streams[index] = Some(stream);
    }
    if audio_tracks == 0 {
        output.push_str(&format!("open {DEMUXER_ERROR_NO_SUPPORTED_STREAMS}\n"));
        return Ok(output);
    }
    if ctx.duration != NOPTS {
        max_duration = max_duration.max(convert(TIME_BASE, ctx.duration));
    } else {
        max_duration = INFINITE;
    }
    if demuxer_start == INFINITE || ctx.iformat == Format::Mp3 {
        demuxer_start = 0;
    }
    let liveness = if max_duration != INFINITE { 1 } else { 0 };
    let mut host = Host { durations: vec![max_duration], buffered: Ranges::default(), errors: Vec::new() };
    let mut duration = max_duration;
    let duration_known = max_duration != INFINITE;
    output.push_str(&format!("open {PIPELINE_OK}\nduration {}\nstart {}\n", time(duration), time(demuxer_start.max(0))));
    let order: Vec<usize> = streams
        .iter()
        .enumerate()
        .filter_map(|(i, s)| s.as_ref().filter(|s| s.enabled).map(|_| i))
        .chain(streams.iter().enumerate().filter_map(|(i, s)| s.as_ref().filter(|s| !s.enabled).map(|_| i)))
        .collect();
    for (position, &index) in order.iter().enumerate() {
        let stream = streams[index].as_ref().expect("ordered streams exist");
        output.push_str(&format!(
            "stream {position} liveness={liveness} enabled={} audio {}\n",
            if stream.enabled { 1 } else { 0 },
            stream.config.describe()
        ));
    }
    loop {
        let mut packet = match ctx.read_frame()? {
            Ok(packet) if packet.data.is_empty() => continue,
            Ok(packet) => packet,
            Err(_) => break,
        };
        let Some(stream) = streams.get_mut(packet.stream_index).and_then(Option::as_mut) else {
            continue;
        };
        let mut buffered_changed = false;
        if stream.enabled {
            stream.enqueue(&mut packet, ctx.iformat, demuxer_start, &mut host, &mut buffered_changed)?;
        }
        if duration_known && stream.duration != NO_TIMESTAMP && stream.duration > duration {
            duration = stream.duration;
            host.durations.push(duration);
        }
        if buffered_changed {
            let mut buffered: Option<Ranges> = None;
            for stream in streams.iter().flatten() {
                buffered = Some(match buffered {
                    Some(ranges) => ranges.intersection(&stream.ranges),
                    None => stream.ranges.clone(),
                });
            }
            host.buffered = buffered.unwrap_or_default();
        }
    }
    let mut ended = 0;
    for stream in streams.iter().flatten() {
        if stream.duration != NO_TIMESTAMP && stream.duration > ended {
            ended = stream.duration;
        }
    }
    if duration == INFINITE || ended > duration {
        host.durations.push(ended);
    }
    for &index in &order {
        let stream = streams[index].as_ref().expect("ordered streams exist");
        if stream.enabled {
            let position = order.iter().position(|&i| i == index).expect("stream is ordered");
            output.push_str(&format!("packets {position}\n{}end\n", stream.output));
        }
    }
    output.push_str(&format!("final {}\nbuffered", time(*host.durations.last().expect("init duration is set"))));
    for (start, end) in &host.buffered.0 {
        output.push_str(&format!(" {}-{}", time(*start), time(*end)));
    }
    output.push('\n');
    for error in &host.errors {
        output.push_str(&format!("error {error}\n"));
    }
    Ok(output)
}

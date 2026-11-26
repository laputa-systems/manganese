#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Codec(pub &'static str);

impl Codec {
    pub const NONE: Codec = Codec("none");

    pub fn is_none(self) -> bool {
        self == Codec::NONE
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaType {
    Unknown,
    Video,
    Audio,
}

pub mod sample_fmt {
    pub const NONE: i32 = -1;
    pub const U8: i32 = 0;
    pub const S16: i32 = 1;
    pub const S32: i32 = 2;
    pub const FLT: i32 = 3;
    pub const S16P: i32 = 6;
    pub const S32P: i32 = 7;
    pub const FLTP: i32 = 8;
}

const WAV_TAGS: &[(u32, &str)] = &[
    (0x0001, "pcm_s16le"), (0x0001, "pcm_u8"), (0x0001, "pcm_s24le"), (0x0001, "pcm_s32le"), (0x0001, "pcm_s64le"),
    (0x0002, "adpcm_ms"), (0x0003, "pcm_f32le"), (0x0003, "pcm_f64le"), (0x0006, "pcm_alaw"), (0x0007, "pcm_mulaw"),
    (0x000A, "wmavoice"), (0x0010, "adpcm_ima_oki"), (0x0011, "adpcm_ima_wav"), (0x0011, "adpcm_zork"),
    (0x0017, "adpcm_ima_oki"), (0x0020, "adpcm_yamaha"), (0x0022, "truespeech"), (0x0031, "gsm_ms"),
    (0x0032, "gsm_ms"), (0x0038, "amr_nb"), (0x0042, "g723_1"), (0x0045, "adpcm_g726"), (0x0014, "adpcm_g726"),
    (0x0040, "adpcm_g726"), (0x0041, "g728"), (0x0050, "mp2"), (0x0055, "mp3"), (0x0057, "amr_nb"),
    (0x0058, "amr_wb"), (0x0061, "adpcm_ima_dk4"), (0x0062, "adpcm_ima_dk3"), (0x0064, "adpcm_g726"),
    (0x0069, "adpcm_ima_xbox"), (0x0075, "metasound"), (0x0083, "g729"), (0x00ff, "aac"), (0x0111, "g723_1"),
    (0x0125, "adpcm_sanyo"), (0x0130, "sipr"), (0x0135, "acelp.kelvin"), (0x0160, "wmav1"), (0x0161, "wmav2"),
    (0x0162, "wmapro"), (0x0163, "wmalossless"), (0x0165, "xma1"), (0x0166, "xma2"), (0x0180, "ftr"),
    (0x0200, "adpcm_ct"), (0x0215, "dvaudio"), (0x0216, "dvaudio"), (0x0270, "atrac3"), (0x028E, "msnsiren"),
    (0x028F, "adpcm_g722"), (0x0350, "misc4"), (0x0401, "imc"), (0x0402, "iac"), (0x0500, "on2avc"),
    (0x0501, "on2avc"), (0x1500, "gsm_ms"), (0x1501, "truespeech"), (0x1600, "aac"), (0x1602, "aac_latm"),
    (0x1610, "aac"), (0x2000, "ac3"), (0x2000, "eac3"), (0x2001, "dts"), (0x2048, "sonic"), (0x2048, "sonicls"),
    (0x2222, "g729"), (0x6c75, "pcm_mulaw"), (0x706d, "aac"), (0x4143, "aac"), (0x4180, "ftr"),
    (0x594a, "xan_dpcm"), (0x729A, "g729"), (0x8180, "ftr"), (0xA100, "g723_1"), (0xA106, "aac"),
    (0xA109, "speex"), (0xCD02, "g728"), (0xF1AC, "flac"), (0xFFFE, "dfpwm"),
];

pub const WAV_GUIDS: &[([u8; 16], &str)] = &[
    ([0x2C, 0x80, 0x6D, 0xE0, 0x46, 0xDB, 0xCF, 0x11, 0xB4, 0xD1, 0x00, 0x80, 0x5F, 0x6C, 0xBB, 0xEA], "ac3"),
    ([0xBF, 0xAA, 0x23, 0xE9, 0x58, 0xCB, 0x71, 0x44, 0xA1, 0x19, 0xFF, 0xFA, 0x01, 0xE4, 0xCE, 0x62], "atrac3p"),
    ([0xD2, 0x42, 0xE1, 0x47, 0xBA, 0x36, 0x8D, 0x4D, 0x88, 0xFC, 0x61, 0x65, 0x4F, 0x8C, 0x83, 0x6C], "atrac9"),
    ([0xAF, 0x87, 0xFB, 0xA7, 0x02, 0x2D, 0xFB, 0x42, 0xA4, 0xD4, 0x05, 0xCD, 0x93, 0x84, 0x3B, 0xDD], "eac3"),
    ([0x2B, 0x80, 0x6D, 0xE0, 0x46, 0xDB, 0xCF, 0x11, 0xB4, 0xD1, 0x00, 0x80, 0x5F, 0x6C, 0xBB, 0xEA], "mp2"),
    ([0x82, 0xEC, 0x1F, 0x6A, 0xCA, 0xDB, 0x19, 0x45, 0xBD, 0xE7, 0x56, 0xD3, 0xB3, 0xEF, 0x98, 0x1D], "adpcm_agm"),
    ([0x3A, 0xC1, 0xFA, 0x38, 0x81, 0x1D, 0x43, 0x61, 0xA4, 0x0D, 0xCE, 0x53, 0xCA, 0x60, 0x7C, 0xD1], "dfpwm"),
];

fn toupper4(tag: u32) -> u32 {
    let mut out = 0;
    for shift in [0, 8, 16, 24] {
        let byte = ((tag >> shift) & 0xff) as u8;
        out |= (byte.to_ascii_uppercase() as u32) << shift;
    }
    out
}

fn codec_get_id(tags: &[(u32, &'static str)], tag: u32) -> Codec {
    if let Some((_, name)) = tags.iter().find(|(candidate, _)| *candidate == tag) {
        return Codec(name);
    }
    if let Some((_, name)) = tags.iter().find(|(candidate, _)| toupper4(*candidate) == toupper4(tag)) {
        return Codec(name);
    }
    Codec::NONE
}

pub fn pcm_codec_id(bps: i32, float: bool, signed_flags: i32) -> Codec {
    if bps <= 0 || bps > 64 {
        return Codec::NONE;
    }
    if float {
        return match bps {
            32 => Codec("pcm_f32le"),
            64 => Codec("pcm_f64le"),
            _ => Codec::NONE,
        };
    }
    let bytes = (bps + 7) >> 3;
    if signed_flags & (1 << (bytes - 1)) != 0 {
        match bytes {
            1 => Codec("pcm_s8"),
            2 => Codec("pcm_s16le"),
            3 => Codec("pcm_s24le"),
            4 => Codec("pcm_s32le"),
            8 => Codec("pcm_s64le"),
            _ => Codec::NONE,
        }
    } else {
        match bytes {
            1 => Codec("pcm_u8"),
            2 => Codec("pcm_u16le"),
            3 => Codec("pcm_u24le"),
            4 => Codec("pcm_u32le"),
            _ => Codec::NONE,
        }
    }
}

pub fn wav_codec_get_id(tag: u32, bps: i32) -> Codec {
    let mut id = codec_get_id(WAV_TAGS, tag);
    if id.is_none() {
        return id;
    }
    if id == Codec("pcm_s16le") {
        id = pcm_codec_id(bps, false, !1);
    } else if id == Codec("pcm_f32le") {
        id = pcm_codec_id(bps, true, 0);
    }
    if id == Codec("adpcm_ima_wav") && bps == 8 {
        id = Codec("adpcm_zork");
    }
    id
}

pub fn exact_bits_per_sample(codec: Codec) -> i32 {
    match codec.0 {
        "dfpwm" => 1,
        "adpcm_ct" | "adpcm_ima_oki" | "adpcm_g722" | "adpcm_yamaha" => 4,
        "pcm_alaw" | "pcm_mulaw" | "pcm_s8" | "pcm_u8" => 8,
        "pcm_s16le" | "pcm_s16be" | "pcm_u16le" => 16,
        "pcm_s24le" | "pcm_s24be" | "pcm_u24le" => 24,
        "pcm_s32le" | "pcm_u32le" | "pcm_f32le" | "pcm_f24le" | "pcm_f16le" => 32,
        "pcm_f64le" | "pcm_s64le" => 64,
        _ => 0,
    }
}

pub fn has_parser(codec: Codec) -> bool {
    matches!(codec.0, "aac" | "flac" | "h264" | "mp1" | "mp2" | "mp3" | "mp3adu" | "opus" | "vorbis" | "vp9")
}

pub fn intra_only(codec: Codec) -> bool {
    codec.0.starts_with("pcm_")
}

pub struct Decoder {
    pub name: &'static str,
    pub channel_conf: bool,
    pub sample_fmt: i32,
    pub raw_bits: i32,
}

pub fn find_decoder(codec: Codec) -> Option<Decoder> {
    let pcm = |name, sample_fmt, raw_bits| Some(Decoder { name, channel_conf: false, sample_fmt, raw_bits });
    match codec.0 {
        "pcm_u8" => pcm("pcm_u8", sample_fmt::U8, 0),
        "pcm_s16le" => pcm("pcm_s16le", sample_fmt::S16, 0),
        "pcm_s16be" => pcm("pcm_s16be", sample_fmt::S16, 0),
        "pcm_s24le" => pcm("pcm_s24le", sample_fmt::S32, 24),
        "pcm_s24be" => pcm("pcm_s24be", sample_fmt::S32, 24),
        "pcm_s32le" => pcm("pcm_s32le", sample_fmt::S32, 32),
        "pcm_f32le" => pcm("pcm_f32le", sample_fmt::FLT, 0),
        "pcm_alaw" => pcm("pcm_alaw", sample_fmt::S16, 0),
        "pcm_mulaw" => pcm("pcm_mulaw", sample_fmt::S16, 0),
        "aac" | "flac" | "mp3" | "vorbis" | "opus" | "h264" => Some(Decoder { name: codec.0, channel_conf: codec.0 == "aac", sample_fmt: sample_fmt::NONE, raw_bits: 0 }),
        _ => None,
    }
}

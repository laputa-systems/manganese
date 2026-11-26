use crate::avio::Avio;
use crate::avio::MemoryReader;
use crate::avio::Reader;
use crate::avio::Whence;
use crate::codec::Codec;
use crate::dict;
use crate::dict::Dict;

pub const HEADER_SIZE: i64 = 10;
const FLAG_DATALEN: u32 = 0x0001;
const FLAG_UNSYNCH: u32 = 0x0002;
const FLAG_ENCRYPTION: u32 = 0x0004;
const FLAG_COMPRESSION: u32 = 0x0008;
const ENCODING_ISO8859: u32 = 0;
const ENCODING_UTF16BOM: u32 = 1;
const ENCODING_UTF16BE: u32 = 2;
const ENCODING_UTF8: u32 = 3;
const MIMETYPE_BUFFER: usize = 64;

const METADATA_34: &[(&str, &str)] = &[
    ("COMM", "comment"), ("TALB", "album"), ("TCOM", "composer"), ("TCON", "genre"), ("TCOP", "copyright"),
    ("TENC", "encoded_by"), ("TIT2", "title"), ("TLAN", "language"), ("TPE1", "artist"), ("TPE2", "album_artist"),
    ("TPE3", "performer"), ("TPOS", "disc"), ("TPUB", "publisher"), ("TRCK", "track"), ("TSSE", "encoder"),
    ("USLT", "lyrics"),
];
const METADATA_4: &[(&str, &str)] = &[
    ("TCMP", "compilation"), ("TDRC", "date"), ("TDRL", "date"), ("TDEN", "creation_time"), ("TSOA", "album-sort"),
    ("TSOP", "artist-sort"), ("TSOT", "title-sort"), ("TSST", "disc_subtitle"), ("TIT1", "grouping"),
];
const METADATA_2: &[(&str, &str)] = &[
    ("TAL", "album"), ("TCO", "genre"), ("TCP", "compilation"), ("TT2", "title"), ("TEN", "encoded_by"),
    ("TP1", "artist"), ("TP2", "album_artist"), ("TP3", "performer"), ("TRK", "track"),
];
const MIME_TAGS: &[(&[u8], &str)] = &[
    (b"image/gif", "gif"), (b"image/jpeg", "mjpeg"), (b"image/jpg", "mjpeg"), (b"image/jxl", "jpegxl"),
    (b"image/png", "png"), (b"image/tiff", "tiff"), (b"image/bmp", "bmp"), (b"image/webp", "webp"),
    (b"JPG", "mjpeg"), (b"PNG", "png"),
];
const PNG_SIGNATURE: u64 = 0x89504e470d0a1a0a;

const ISO639_2_CODES: [&[u8; 3]; 504] = [
    b"aar", b"abk", b"ace", b"ach", b"ada", b"ady", b"afa", b"afh", b"afr", b"ain", b"aka", b"akk", b"alb", b"ale",
    b"alg", b"alt", b"amh", b"ang", b"anp", b"apa", b"ara", b"arc", b"arg", b"arm", b"arn", b"arp", b"art", b"arw",
    b"asm", b"ast", b"ath", b"aus", b"ava", b"ave", b"awa", b"aym", b"aze", b"bad", b"bai", b"bak", b"bal", b"bam",
    b"ban", b"baq", b"bas", b"bat", b"bej", b"bel", b"bem", b"ben", b"ber", b"bho", b"bih", b"bik", b"bin", b"bis",
    b"bla", b"bnt", b"bod", b"bos", b"bra", b"bre", b"btk", b"bua", b"bug", b"bul", b"bur", b"byn", b"cad", b"cai",
    b"car", b"cat", b"cau", b"ceb", b"cel", b"ces", b"cha", b"chb", b"che", b"chg", b"chi", b"chk", b"chm", b"chn",
    b"cho", b"chp", b"chr", b"chu", b"chv", b"chy", b"cmc", b"cop", b"cor", b"cos", b"cpe", b"cpf", b"cpp", b"cre",
    b"crh", b"crp", b"csb", b"cus", b"cym", b"cze", b"dak", b"dan", b"dar", b"day", b"del", b"den", b"deu", b"dgr",
    b"din", b"div", b"doi", b"dra", b"dsb", b"dua", b"dum", b"dut", b"dyu", b"dzo", b"efi", b"egy", b"eka", b"ell",
    b"elx", b"eng", b"enm", b"epo", b"est", b"eus", b"ewe", b"ewo", b"fan", b"fao", b"fas", b"fat", b"fij", b"fil",
    b"fin", b"fiu", b"fon", b"fra", b"fre", b"frm", b"fro", b"frr", b"frs", b"fry", b"ful", b"fur", b"gaa", b"gay",
    b"gba", b"gem", b"geo", b"ger", b"gez", b"gil", b"gla", b"gle", b"glg", b"glv", b"gmh", b"goh", b"gon", b"gor",
    b"got", b"grb", b"grc", b"gre", b"grn", b"gsw", b"guj", b"gwi", b"hai", b"hat", b"hau", b"haw", b"heb", b"her",
    b"hil", b"him", b"hin", b"hit", b"hmn", b"hmo", b"hrv", b"hsb", b"hun", b"hup", b"hye", b"iba", b"ibo", b"ice",
    b"ido", b"iii", b"ijo", b"iku", b"ile", b"ilo", b"ina", b"inc", b"ind", b"ine", b"inh", b"ipk", b"ira", b"iro",
    b"isl", b"ita", b"jav", b"jbo", b"jpn", b"jpr", b"jrb", b"kaa", b"kab", b"kac", b"kal", b"kam", b"kan", b"kar",
    b"kas", b"kat", b"kau", b"kaw", b"kaz", b"kbd", b"kha", b"khi", b"khm", b"kho", b"kik", b"kin", b"kir", b"kmb",
    b"kok", b"kom", b"kon", b"kor", b"kos", b"kpe", b"krc", b"krl", b"kro", b"kru", b"kua", b"kum", b"kur", b"kut",
    b"lad", b"lah", b"lam", b"lao", b"lat", b"lav", b"lez", b"lim", b"lin", b"lit", b"lol", b"loz", b"ltz", b"lua",
    b"lub", b"lug", b"lui", b"lun", b"luo", b"lus", b"mac", b"mad", b"mag", b"mah", b"mai", b"mak", b"mal", b"man",
    b"mao", b"map", b"mar", b"mas", b"may", b"mdf", b"mdr", b"men", b"mga", b"mic", b"min", b"mis", b"mkd", b"mkh",
    b"mlg", b"mlt", b"mnc", b"mni", b"mno", b"moh", b"mon", b"mos", b"mri", b"msa", b"mul", b"mun", b"mus", b"mwl",
    b"mwr", b"mya", b"myn", b"myv", b"nah", b"nai", b"nap", b"nau", b"nav", b"nbl", b"nde", b"ndo", b"nds", b"nep",
    b"new", b"nia", b"nic", b"niu", b"nld", b"nno", b"nob", b"nog", b"non", b"nor", b"nqo", b"nso", b"nub", b"nwc",
    b"nya", b"nym", b"nyn", b"nyo", b"nzi", b"oci", b"oji", b"ori", b"orm", b"osa", b"oss", b"ota", b"oto", b"paa",
    b"pag", b"pal", b"pam", b"pan", b"pap", b"pau", b"peo", b"per", b"phi", b"phn", b"pli", b"pol", b"pon", b"por",
    b"pra", b"pro", b"pus", b"que", b"raj", b"rap", b"rar", b"roa", b"roh", b"rom", b"ron", b"rum", b"run", b"rup",
    b"rus", b"sad", b"sag", b"sah", b"sai", b"sal", b"sam", b"san", b"sas", b"sat", b"scn", b"sco", b"sel", b"sem",
    b"sga", b"sgn", b"shn", b"sid", b"sin", b"sio", b"sit", b"sla", b"slk", b"slo", b"slv", b"sma", b"sme", b"smi",
    b"smj", b"smn", b"smo", b"sms", b"sna", b"snd", b"snk", b"sog", b"som", b"son", b"sot", b"spa", b"sqi", b"srd",
    b"srn", b"srp", b"srr", b"ssa", b"ssw", b"suk", b"sun", b"sus", b"sux", b"swa", b"swe", b"syc", b"syr", b"tah",
    b"tai", b"tam", b"tat", b"tel", b"tem", b"ter", b"tet", b"tgk", b"tgl", b"tha", b"tib", b"tig", b"tir", b"tiv",
    b"tkl", b"tlh", b"tli", b"tmh", b"tog", b"ton", b"tpi", b"tsi", b"tsn", b"tso", b"tuk", b"tum", b"tup", b"tur",
    b"tut", b"tvl", b"twi", b"tyv", b"udm", b"uga", b"uig", b"ukr", b"umb", b"und", b"urd", b"uzb", b"vai", b"ven",
    b"vie", b"vol", b"vot", b"wak", b"wal", b"war", b"was", b"wel", b"wen", b"wln", b"wol", b"xal", b"xho", b"yao",
    b"yap", b"yid", b"yor", b"ypk", b"zap", b"zbl", b"zen", b"zha", b"zho", b"znd", b"zul", b"zun", b"zxx", b"zza",
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum ExtraMeta {
    Geob,
    Apic,
    Chap,
    Priv,
}

pub struct Picture {
    pub codec: Codec,
    pub data: Vec<u8>,
}

pub fn matches(buf: &[u8]) -> bool {
    &buf[0..3] == b"ID3"
        && buf[3] != 0xff
        && buf[4] != 0xff
        && buf[6] & 0x80 == 0
        && buf[7] & 0x80 == 0
        && buf[8] & 0x80 == 0
        && buf[9] & 0x80 == 0
}

pub fn tag_len(buf: &[u8]) -> i32 {
    let mut len = ((buf[6] & 0x7f) as i32) << 21
        | ((buf[7] & 0x7f) as i32) << 14
        | ((buf[8] & 0x7f) as i32) << 7
        | (buf[9] & 0x7f) as i32;
    len += HEADER_SIZE as i32;
    if buf[5] & 0x10 != 0 {
        len += HEADER_SIZE as i32;
    }
    len
}

fn is_language(code: &[u8]) -> bool {
    code.len() == 3 && ISO639_2_CODES.binary_search_by(|candidate| candidate[..].cmp(code)).is_ok()
}

fn get_size<R: Reader>(pb: &mut R, len: usize) -> u32 {
    let mut value: u32 = 0;
    for _ in 0..len {
        value = (value << 7).wrapping_add(pb.r8() & 0x7f);
    }
    value
}

fn size_to_syncsafe(size: u32) -> u32 {
    (size & 0x7f) + ((size & (0x7f << 8)) >> 1) + ((size & (0x7f << 16)) >> 2) + ((size & (0x7f << 24)) >> 3)
}

fn is_tag(buf: &[u8]) -> bool {
    !buf.is_empty() && buf.iter().all(|&byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
}

fn check_tag(pb: &mut Avio, offset: i32, len: usize) -> i32 {
    let mut tag = [0u8; 4];
    if len > 4 || pb.seek(offset as i64, Whence::Set) < 0 || pb.read(&mut tag[..len]) < len as i32 {
        return -1;
    }
    if u32::from_be_bytes(tag) == 0 || is_tag(&tag[..len]) { 1 } else { 0 }
}

fn put_utf8(out: &mut Vec<u8>, value: u32) {
    if value < 0x80 {
        out.push(value as u8);
        return;
    }
    let bytes = (31 - value.leading_zeros() + 4) / 5;
    let mut shift = (bytes - 1) * 6;
    out.push(((256 - (256 >> bytes)) | (value >> shift)) as u8);
    while shift >= 6 {
        shift -= 6;
        out.push((0x80 | ((value >> shift) & 0x3f)) as u8);
    }
}

fn c_string(bytes: &[u8]) -> &[u8] {
    match bytes.iter().position(|&byte| byte == 0) {
        Some(end) => &bytes[..end],
        None => bytes,
    }
}

fn decode_str<R: Reader>(pb: &mut R, encoding: u32, maxread: &mut i32) -> Option<Vec<u8>> {
    let mut left = *maxread;
    let mut out = Vec::new();
    let mut ch: u32 = 1;
    if left != 0 {
        match encoding {
            ENCODING_ISO8859 => {
                while left != 0 && ch != 0 {
                    ch = pb.r8();
                    put_utf8(&mut out, ch);
                    left -= 1;
                }
            }
            ENCODING_UTF16BOM | ENCODING_UTF16BE => {
                let mut little_endian = false;
                let mut empty = false;
                if encoding == ENCODING_UTF16BOM {
                    left -= 2;
                    if left < 0 {
                        return None;
                    }
                    match pb.rb16() {
                        0xfffe => little_endian = true,
                        0xfeff => {}
                        0 => empty = true,
                        _ => {
                            *maxread = left;
                            return None;
                        }
                    }
                }
                if !empty {
                    let get = |pb: &mut R, left: &mut i32| {
                        *left -= 2;
                        if *left < 0 {
                            0
                        } else if little_endian {
                            pb.rl16()
                        } else {
                            pb.rb16()
                        }
                    };
                    while left > 1 && ch != 0 {
                        ch = get(pb, &mut left);
                        let hi = ch.wrapping_sub(0xD800);
                        if hi < 0x800 {
                            ch = get(pb, &mut left).wrapping_sub(0xDC00);
                            if ch > 0x3FF || hi > 0x3FF {
                                break;
                            }
                            ch += (hi << 10) + 0x10000;
                        }
                        put_utf8(&mut out, ch);
                    }
                    if left < 0 {
                        left += 2;
                    }
                }
            }
            ENCODING_UTF8 => {
                while left != 0 && ch != 0 {
                    ch = pb.r8();
                    out.push(ch as u8);
                    left -= 1;
                }
            }
            _ => {}
        }
    }
    *maxread = left;
    Some(c_string(&out).to_vec())
}

fn read_ttag<R: Reader>(pb: &mut R, mut taglen: i32, metadata: &mut Dict, key: &[u8]) {
    if taglen < 1 {
        return;
    }
    let encoding = pb.r8();
    taglen -= 1;
    let Some(mut value) = decode_str(pb, encoding, &mut taglen) else {
        return;
    };
    let mut key = key.to_vec();
    if key == b"TXXX" || key == b"TXX" {
        key = value;
        let Some(text) = decode_str(pb, encoding, &mut taglen) else {
            return;
        };
        value = text;
    } else if value.is_empty() {
        return;
    }
    metadata.set(&key, Some(&value), dict::DONT_OVERWRITE);
}

fn read_lang_descr_tag<R: Reader>(pb: &mut R, key: &[u8], mut taglen: i32, metadata: &mut Dict) {
    if taglen < 4 {
        return;
    }
    let encoding = pb.r8();
    let mut language = [0u8; 3];
    if pb.read(&mut language) < 3 {
        return;
    }
    let mut language = c_string(&language).to_vec();
    language.make_ascii_lowercase();
    if language == b"xxx" || language == b"und" {
        language.clear();
    }
    taglen -= 4;
    let Some(descriptor) = decode_str(pb, encoding, &mut taglen) else {
        return;
    };
    let Some(value) = decode_str(pb, encoding, &mut taglen) else {
        return;
    };
    let mut full_key = key.to_vec();
    if !descriptor.is_empty() {
        full_key.push(b'-');
        full_key.extend_from_slice(&descriptor);
        let len = descriptor.len();
        if !language.is_empty() {
            full_key.push(b'-');
            full_key.extend_from_slice(&language);
        } else if (len == 3 && is_language(&descriptor))
            || (len > 4 && descriptor[len - 4] == b'-' && is_language(&descriptor[len - 3..]))
        {
            full_key.extend_from_slice(b"-und");
        }
    } else if language.len() == 3 {
        full_key.push(b'-');
        full_key.extend_from_slice(&language);
    }
    metadata.set(&full_key, Some(&value), dict::DONT_OVERWRITE);
}

fn read_geobtag<R: Reader>(pb: &mut R, mut taglen: i32) {
    if taglen < 1 {
        return;
    }
    let encoding = pb.r8();
    taglen -= 1;
    if decode_str(pb, ENCODING_ISO8859, &mut taglen).is_none() || taglen <= 0 {
        return;
    }
    if decode_str(pb, encoding, &mut taglen).is_none() || taglen <= 0 {
        return;
    }
    if decode_str(pb, encoding, &mut taglen).is_none() || taglen < 0 {
        return;
    }
    if taglen != 0 {
        let mut data = vec![0u8; taglen as usize];
        pb.read(&mut data);
    }
}

fn read_apic<R: Reader>(pb: &mut R, mut taglen: i32, isv34: bool, pictures: &mut Vec<Picture>) {
    let end = pb.tell() + taglen as i64;
    if let Some(picture) = parse_apic(pb, &mut taglen, isv34) {
        pictures.push(picture);
    } else {
        pb.seek(end, Whence::Set);
    }
}

fn parse_apic<R: Reader>(pb: &mut R, taglen: &mut i32, isv34: bool) -> Option<Picture> {
    if *taglen <= 4 || (!isv34 && *taglen <= 6) {
        return None;
    }
    let encoding = pb.r8();
    *taglen -= 1;
    let mimetype = if isv34 {
        let (ret, mimetype) = pb.get_str(*taglen, MIMETYPE_BUFFER);
        if ret < 0 || ret >= *taglen {
            return None;
        }
        *taglen -= ret;
        mimetype
    } else {
        let mut mimetype = [0u8; 3];
        if pb.read(&mut mimetype) < 0 {
            return None;
        }
        *taglen -= 3;
        c_string(&mimetype).to_vec()
    };
    let (_, codec) = MIME_TAGS.iter().find(|(name, _)| name.eq_ignore_ascii_case(&mimetype))?;
    pb.r8();
    *taglen -= 1;
    decode_str(pb, encoding, taglen)?;
    if *taglen == 0 {
        return None;
    }
    let mut data = vec![0u8; *taglen as usize];
    if pb.read(&mut data) != *taglen {
        return None;
    }
    let mut signature = [0u8; 8];
    for (slot, byte) in signature.iter_mut().zip(&data) {
        *slot = *byte;
    }
    let codec = if u64::from_be_bytes(signature) == PNG_SIGNATURE { Codec("png") } else { Codec(codec) };
    Some(Picture { codec, data })
}

fn read_chapter<R: Reader>(pb: &mut R, mut len: i32) {
    if decode_str(pb, ENCODING_ISO8859, &mut len).is_none() || len < 16 {
        return;
    }
    pb.rb32();
    pb.rb32();
    pb.skip(8);
    len -= 16;
    let mut meta = Dict::default();
    while len > 10 {
        let mut tag = [0u8; 4];
        if pb.read(&mut tag) < 4 {
            return;
        }
        let taglen = pb.rb32() as i32;
        pb.skip(2);
        len -= 10;
        if taglen < 0 || taglen > len {
            return;
        }
        if tag[0] == b'T' {
            read_ttag(pb, taglen, &mut meta, c_string(&tag));
        } else {
            pb.skip(taglen as i64);
        }
        len -= taglen;
    }
}

fn read_priv<R: Reader>(pb: &mut R, mut taglen: i32) {
    if decode_str(pb, ENCODING_ISO8859, &mut taglen).is_none() {
        return;
    }
    let mut data = vec![0u8; taglen.max(0) as usize];
    pb.read(&mut data);
}

fn extra_meta(tag: &[u8; 4], isv34: bool) -> Option<ExtraMeta> {
    let table: [(&[u8; 3], &[u8; 4], ExtraMeta); 4] = [
        (b"GEO", b"GEOB", ExtraMeta::Geob),
        (b"PIC", b"APIC", ExtraMeta::Apic),
        (b"CHA", b"CHAP", ExtraMeta::Chap),
        (b"PRI", b"PRIV", ExtraMeta::Priv),
    ];
    table.iter().find(|(tag3, tag4, _)| if isv34 { &tag[..] == &tag4[..] } else { &tag[..3] == &tag3[..] }).map(|entry| entry.2)
}

fn read_frame<R: Reader>(
    pb: &mut R,
    tag: &[u8; 4],
    tlen: i32,
    isv34: bool,
    comm: &[u8],
    metadata: &mut Dict,
    pictures: &mut Vec<Picture>,
) {
    let name = c_string(tag);
    if tag[0] == b'T' {
        read_ttag(pb, tlen, metadata, name);
    } else if &tag[..] == b"USLT" {
        read_lang_descr_tag(pb, b"lyrics", tlen, metadata);
    } else if name == comm {
        read_lang_descr_tag(pb, b"comment", tlen, metadata);
    } else {
        match extra_meta(tag, isv34) {
            Some(ExtraMeta::Geob) => read_geobtag(pb, tlen),
            Some(ExtraMeta::Apic) => read_apic(pb, tlen, isv34, pictures),
            Some(ExtraMeta::Chap) => read_chapter(pb, tlen),
            Some(ExtraMeta::Priv) => read_priv(pb, tlen),
            None => {}
        }
    }
}

fn parse(pb: &mut Avio, metadata: &mut Dict, mut len: i32, version: u8, flags: u8, pictures: &mut Vec<Picture>) {
    let mut end = pb.tell();
    if end > i64::MAX - len as i64 - 10 {
        return;
    }
    end += len as i64;
    let (isv34, taghdrlen, comm): (bool, i32, &[u8]) = match version {
        2 if flags & 0x40 == 0 => (false, 6, b"COM"),
        3 | 4 => (true, 10, b"COMM"),
        _ => {
            pb.seek(end, Whence::Set);
            return;
        }
    };
    let unsync = flags & 0x80 != 0;
    if isv34 && flags & 0x40 != 0 {
        let mut extlen = get_size(pb, 4) as i32;
        if version == 4 {
            extlen -= 4;
        }
        if extlen < 0 {
            pb.seek(end, Whence::Set);
            return;
        }
        pb.skip(extlen as i64);
        len -= extlen + 4;
        if len < 0 {
            pb.seek(end, Whence::Set);
            return;
        }
    }
    let mut buffer_allocated = false;
    while len >= taghdrlen {
        let mut tflags = 0u32;
        let mut tag = [0u8; 4];
        let mut tlen: u32;
        if isv34 {
            if pb.read(&mut tag) < 4 {
                break;
            }
            tlen = pb.rb32();
            if version == 4 && tlen > 0x7f {
                if (tlen as u64) < len as u64 {
                    let cur = pb.tell();
                    if check_tag(pb, (cur + 2 + size_to_syncsafe(tlen) as i64) as i32, 4) == 1 {
                        tlen = size_to_syncsafe(tlen);
                    } else if check_tag(pb, (cur + 2 + tlen as i64) as i32, 4) != 1 {
                        break;
                    }
                    pb.seek(cur, Whence::Set);
                } else {
                    tlen = size_to_syncsafe(tlen);
                }
            }
            tflags = pb.rb16();
        } else {
            if pb.read(&mut tag[..3]) < 3 {
                break;
            }
            tlen = pb.rb24();
        }
        let tunsync = tflags & FLAG_UNSYNCH != 0;
        if tlen > 1 << 28 {
            break;
        }
        len -= taghdrlen + tlen as i32;
        if len < 0 {
            break;
        }
        let next = pb.tell() + tlen as i64;
        if tlen == 0 {
            continue;
        }
        if tflags & FLAG_DATALEN != 0 {
            if tlen < 4 {
                break;
            }
            pb.rb32();
            tlen -= 4;
        }
        let tcomp = tflags & FLAG_COMPRESSION != 0;
        let tencr = tflags & FLAG_ENCRYPTION != 0;
        if tencr || tcomp {
            pb.skip(tlen as i64);
        } else if tag[0] == b'T' || &tag[..] == b"USLT" || c_string(&tag) == comm || extra_meta(&tag, isv34).is_some() {
            if unsync || tunsync {
                if tlen == 0 && !buffer_allocated {
                    pb.seek(next, Whence::Set);
                    continue;
                }
                buffer_allocated |= tlen != 0;
                let mut buffer = vec![0u8; tlen as usize];
                if pb.read(&mut buffer) != tlen as i32 {
                    pb.seek(next, Whence::Set);
                    continue;
                }
                let mut synced = Vec::with_capacity(buffer.len());
                let mut t = 0;
                while t != buffer.len() {
                    synced.push(buffer[t]);
                    t += 1;
                    if t != buffer.len() && buffer[t - 1] == 0xff && buffer[t] == 0 {
                        t += 1;
                    }
                }
                let tlen = synced.len() as i32;
                let mut local = MemoryReader::new(synced);
                read_frame(&mut local, &tag, tlen, isv34, comm, metadata, pictures);
            } else {
                read_frame(pb, &tag, tlen as i32, isv34, comm, metadata, pictures);
            }
        } else if tag[0] == 0 {
            pb.skip(tlen as i64);
            break;
        }
        pb.seek(next, Whence::Set);
    }
    if version == 4 && flags & 0x10 != 0 {
        end += 10;
    }
    pb.seek(end, Whence::Set);
}

fn date_tag(metadata: &Dict, tag: &[u8]) -> Option<Vec<u8>> {
    let index = metadata.get(tag, None, dict::MATCH_CASE)?;
    let value = metadata.value(index);
    (value.len() == 4 && value.iter().all(u8::is_ascii_digit)).then(|| value.to_vec())
}

fn merge_date(metadata: &mut Dict) {
    let Some(year) = date_tag(metadata, b"TYER").or_else(|| date_tag(metadata, b"TYE")) else {
        return;
    };
    let mut date = year;
    metadata.set(b"TYER", None, 0);
    metadata.set(b"TYE", None, 0);
    if let Some(day) = date_tag(metadata, b"TDAT").or_else(|| date_tag(metadata, b"TDA")) {
        date.push(b'-');
        date.extend_from_slice(&day[2..4]);
        date.push(b'-');
        date.extend_from_slice(&day[0..2]);
        metadata.set(b"TDAT", None, 0);
        metadata.set(b"TDA", None, 0);
        if let Some(time) = date_tag(metadata, b"TIME").or_else(|| date_tag(metadata, b"TIM")) {
            date.push(b' ');
            date.extend_from_slice(&time[0..2]);
            date.push(b':');
            date.extend_from_slice(&time[2..4]);
            metadata.set(b"TIME", None, 0);
            metadata.set(b"TIM", None, 0);
        }
    }
    metadata.set(b"date", Some(&date), 0);
}

pub fn read_dict(pb: &mut Avio, metadata: &mut Dict, pictures: &mut Vec<Picture>) {
    loop {
        let off = pb.tell();
        let mut buf = [0u8; HEADER_SIZE as usize];
        if pb.read(&mut buf) != HEADER_SIZE as i32 {
            pb.seek(off, Whence::Set);
            break;
        }
        if !matches(&buf) {
            pb.seek(off, Whence::Set);
            break;
        }
        let len = ((buf[6] & 0x7f) as i32) << 21 | ((buf[7] & 0x7f) as i32) << 14 | ((buf[8] & 0x7f) as i32) << 7 | (buf[9] & 0x7f) as i32;
        parse(pb, metadata, len, buf[3], buf[5], pictures);
    }
    metadata.convert(METADATA_34);
    metadata.convert(METADATA_2);
    metadata.convert(METADATA_4);
    merge_date(metadata);
}

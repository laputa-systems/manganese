use img_jpeg::SavedMarker;

const ICC_SIGNATURE: &[u8] = b"ICC_PROFILE\0";
const EXIF_SIGNATURE: &[u8] = b"Exif\0";

fn read_metadata<'a>(markers: &'a [SavedMarker], target: u8, signature: &[u8], padding: usize, index_bytes: usize) -> Option<Vec<u8>> {
    let header = signature.len() + padding + 2 * index_bytes;
    let mut parts: Vec<Option<&'a [u8]>> = Vec::new();
    let mut found = 0u32;
    let mut expected = 0u32;
    for marker in markers {
        if marker.marker != target || marker.data.len() <= signature.len() || &marker.data[..signature.len()] != signature {
            continue;
        }
        let data = &marker.data;
        if data.len() <= header {
            continue;
        }
        let (index, count) = if index_bytes == 0 {
            (1u32, 1u32)
        } else {
            let offset = signature.len() + padding;
            (data[offset] as u32, data[offset + 1] as u32)
        };
        if count == 0 || index == 0 || index > count {
            return None;
        }
        if expected == 0 {
            expected = count;
            parts = vec![None; expected as usize];
        }
        if count != expected {
            return None;
        }
        if parts[index as usize - 1].is_some() {
            return None;
        }
        parts[index as usize - 1] = Some(&data[header..]);
        found += 1;
        if found == expected {
            break;
        }
    }
    if expected == 0 || found != expected {
        return None;
    }
    Some(parts.into_iter().flat_map(|part| part.expect("complete parts").iter().copied()).collect())
}

pub fn icc_profile(markers: &[SavedMarker]) -> Option<Vec<u8>> {
    read_metadata(markers, 0xE2, ICC_SIGNATURE, 0, 1)
}

pub fn exif(markers: &[SavedMarker]) -> Option<Vec<u8>> {
    read_metadata(markers, 0xE1, EXIF_SIGNATURE, 1, 0)
}

#[derive(Default)]
pub struct ExifMetadata {
    pub origin: Option<u16>,
    pub resolution_unit: Option<u16>,
    pub x_resolution: Option<f32>,
    pub y_resolution: Option<f32>,
    pub pixel_x_dimension: Option<u32>,
    pub pixel_y_dimension: Option<u32>,
}

struct Ifd<'a> {
    data: &'a [u8],
    little: bool,
    offset: usize,
    entries: usize,
}

fn short(data: &[u8], little: bool) -> u16 {
    if little { u16::from_le_bytes([data[0], data[1]]) } else { u16::from_be_bytes([data[0], data[1]]) }
}

fn long(data: &[u8], little: bool) -> u32 {
    let bytes = [data[0], data[1], data[2], data[3]];
    if little { u32::from_le_bytes(bytes) } else { u32::from_be_bytes(bytes) }
}

fn bytes_for_type(kind: u16) -> usize {
    match kind {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 => 4,
        5 | 10 | 12 => 8,
        _ => 0,
    }
}

impl<'a> Ifd<'a> {
    fn make(data: &'a [u8], little: bool, offset: u32) -> Option<Self> {
        let offset = offset as usize;
        if data.len() < offset {
            return None;
        }
        let mut size = data.len() - offset;
        if size < 2 {
            return None;
        }
        let count = short(&data[offset..], little) as usize;
        size -= 2;
        if size < 12 * count {
            return Some(Ifd { data, little, offset, entries: size / 12 });
        }
        Some(Ifd { data, little, offset, entries: count })
    }

    fn entry(&self, index: usize) -> &'a [u8] {
        &self.data[self.offset + 2 + 12 * index..]
    }

    fn tag(&self, index: usize) -> u16 {
        short(self.entry(index), self.little)
    }

    fn value(&self, index: usize, kind: u16) -> Option<&'a [u8]> {
        let entry = self.entry(index);
        let entry_kind = short(&entry[2..], self.little);
        if !(1..=12).contains(&entry_kind) {
            return None;
        }
        let count = long(&entry[4..], self.little);
        let bytes = bytes_for_type(entry_kind) * count as usize;
        let data = if bytes <= 4 {
            &entry[8..]
        } else {
            let offset = long(&entry[8..], self.little) as usize;
            if self.data.len() < offset || self.data.len() - offset < bytes {
                return None;
            }
            &self.data[offset..]
        };
        if entry_kind != kind || count != 1 {
            return None;
        }
        Some(data)
    }

    fn unsigned_short(&self, index: usize) -> Option<u16> {
        self.value(index, 3).map(|data| short(data, self.little))
    }

    fn unsigned_long(&self, index: usize) -> Option<u32> {
        self.value(index, 4).map(|data| long(data, self.little))
    }

    fn unsigned_rational(&self, index: usize) -> Option<f32> {
        self.value(index, 5).map(|data| {
            let numerator = long(data, self.little);
            let denominator = long(&data[4..], self.little);
            if denominator == 0 { 0.0 } else { numerator as f32 / denominator as f32 }
        })
    }
}

fn parse_ifd(exif: &mut ExifMetadata, ifd: Option<Ifd>, root: bool) {
    let Some(ifd) = ifd else { return };
    for i in 0..ifd.entries {
        match ifd.tag(i) {
            0x0112 => {
                if exif.origin.is_none()
                    && let Some(value) = ifd.unsigned_short(i)
                    && (1..=8).contains(&value)
                {
                    exif.origin = Some(value);
                }
            }
            0x8769 => {
                if root && let Some(offset) = ifd.unsigned_long(i) {
                    parse_ifd(exif, Ifd::make(ifd.data, ifd.little, offset), false);
                }
            }
            0x011a => {
                if exif.x_resolution.is_none()
                    && let Some(value) = ifd.unsigned_rational(i)
                {
                    exif.x_resolution = Some(value);
                }
            }
            0x011b => {
                if exif.y_resolution.is_none()
                    && let Some(value) = ifd.unsigned_rational(i)
                {
                    exif.y_resolution = Some(value);
                }
            }
            0x0128 => {
                if exif.resolution_unit.is_none()
                    && let Some(value) = ifd.unsigned_short(i)
                {
                    exif.resolution_unit = Some(value);
                }
            }
            0xa002 => {
                if exif.pixel_x_dimension.is_none()
                    && let Some(value) = ifd.unsigned_short(i)
                {
                    exif.pixel_x_dimension = Some(value as u32);
                }
                if exif.pixel_x_dimension.is_none()
                    && let Some(value) = ifd.unsigned_long(i)
                {
                    exif.pixel_x_dimension = Some(value);
                }
            }
            0xa003 => {
                if exif.pixel_y_dimension.is_none()
                    && let Some(value) = ifd.unsigned_short(i)
                {
                    exif.pixel_y_dimension = Some(value as u32);
                }
                if exif.pixel_y_dimension.is_none()
                    && let Some(value) = ifd.unsigned_long(i)
                {
                    exif.pixel_y_dimension = Some(value);
                }
            }
            _ => {}
        }
    }
}

pub fn parse_exif(data: Option<&[u8]>) -> ExifMetadata {
    let mut exif = ExifMetadata::default();
    if let Some(data) = data
        && data.len() >= 8
        && ((data[0] == b'I' && data[1] == b'I') || (data[0] == b'M' && data[1] == b'M'))
    {
        let little = data[0] == b'I';
        let offset = long(&data[4..], little);
        parse_ifd(&mut exif, Ifd::make(data, little, offset), true);
    }
    exif
}

fn clamp_size_f(value: f32) -> f32 {
    let trivial = 8.0 * f32::EPSILON;
    if value > trivial { value } else { 0.0 }
}

fn clamp_round(value: f32) -> i32 {
    let rounded = value.round();
    if rounded.is_nan() {
        0
    } else if rounded >= i32::MAX as f32 {
        i32::MAX
    } else if rounded <= i32::MIN as f32 {
        i32::MIN
    } else {
        rounded as i32
    }
}

pub fn density_corrected_size(metadata: &ExifMetadata, physical: (u32, u32)) -> (u32, u32) {
    let resolution = (clamp_size_f(metadata.x_resolution.unwrap_or(0.0)), clamp_size_f(metadata.y_resolution.unwrap_or(0.0)));
    let size = ((metadata.pixel_x_dimension.unwrap_or(0) as i32).max(0), (metadata.pixel_y_dimension.unwrap_or(0) as i32).max(0));
    if metadata.resolution_unit != Some(2) || resolution.0 == 0.0 || resolution.1 == 0.0 || size.0 == 0 || size.1 == 0 {
        return physical;
    }
    let from_resolution = (clamp_size_f((physical.0.wrapping_mul(72)) as f32 / resolution.0), clamp_size_f((physical.1.wrapping_mul(72)) as f32 / resolution.1));
    let rounded = (clamp_round(from_resolution.0).max(0), clamp_round(from_resolution.1).max(0));
    if rounded == size {
        return (size.0 as u32, size.1 as u32);
    }
    physical
}

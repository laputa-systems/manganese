//! Baseline JPEG encoder that reproduces libjpeg-turbo 3.1.0's byte output.

const DCTSIZE: usize = 8;
const DCTSIZE2: usize = 64;
const NATURAL: [usize; DCTSIZE2] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34,
    27, 20, 13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44,
    51, 58, 59, 52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

const STD_LUMA: [u16; DCTSIZE2] = [
    16, 11, 10, 16, 24, 40, 51, 61, 12, 12, 14, 19, 26, 58, 60, 55, 14, 13, 16, 24, 40, 57, 69,
    56, 14, 17, 22, 29, 51, 87, 80, 62, 18, 22, 37, 56, 68, 109, 103, 77, 24, 35, 55, 64, 81,
    104, 113, 92, 49, 64, 78, 87, 103, 121, 120, 101, 72, 92, 95, 98, 112, 100, 103, 99,
];
const STD_CHROMA: [u16; DCTSIZE2] = [
    17, 18, 24, 47, 99, 99, 99, 99, 18, 21, 26, 66, 99, 99, 99, 99, 24, 26, 56, 99, 99, 99, 99,
    99, 47, 66, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorSpace {
    Bgra,
    Rgba,
    Rgb,
    YcbCr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Downsample {
    S444,
    S422,
    S420,
}

#[derive(Clone, Copy)]
pub struct Options<'a> {
    pub quality: u8,
    pub downsample: Downsample,
    pub blend_on_black: bool,
    pub icc: Option<&'a [u8]>,
}

#[derive(Debug)]
pub enum Error {
    Dimension,
    PixelCount,
}

#[derive(Clone, Copy)]
struct Quant {
    recip: u32,
    corr: u32,
    shift: u32,
}

fn quant_divisor(divisor: u16) -> Quant {
    if divisor == 1 {
        return Quant { recip: 1, corr: 0, shift: 0 };
    }
    let b = 31 - (divisor as u32).leading_zeros();
    let mut r = 16 + b;
    let base = 1u64 << r;
    let mut fq = base / divisor as u64;
    let fr = base % divisor as u64;
    let mut corr = divisor as u32 / 2;
    if fr == 0 {
        fq >>= 1;
        r -= 1;
    } else if fr <= (divisor / 2) as u64 {
        corr += 1;
    } else {
        fq += 1;
    }
    Quant { recip: fq as u32, corr, shift: r }
}

fn quantize(value: i32, q: &Quant) -> i32 {
    let negative = value < 0;
    let magnitude = value.unsigned_abs() as u64;
    let result = ((magnitude + q.corr as u64) * q.recip as u64) >> q.shift;
    if negative { -(result as i32) } else { result as i32 }
}

fn scaling(quality: u8) -> i32 {
    let q = if quality == 0 { 1 } else { quality as i32 };
    if q < 50 { 5000 / q } else { 200 - q * 2 }
}

fn quant_value(base: u16, scale: i32) -> u16 {
    let mut temp = (base as i32 * scale + 50) / 100;
    if temp <= 0 { temp = 1; }
    if temp > 255 { temp = 255; }
    temp as u16
}

fn quant_tables(scale: i32) -> ([u16; DCTSIZE2], [Quant; DCTSIZE2], [u16; DCTSIZE2], [Quant; DCTSIZE2]) {
    let mut luma = [0u16; DCTSIZE2];
    let mut luma_div = [Quant { recip: 1, corr: 0, shift: 0 }; DCTSIZE2];
    let mut chroma = [0u16; DCTSIZE2];
    let mut chroma_div = [Quant { recip: 1, corr: 0, shift: 0 }; DCTSIZE2];
    for i in 0..DCTSIZE2 {
        luma[i] = quant_value(STD_LUMA[i], scale);
        luma_div[i] = quant_divisor(luma[i] * 8);
        chroma[i] = quant_value(STD_CHROMA[i], scale);
        chroma_div[i] = quant_divisor(chroma[i] * 8);
    }
    (luma, luma_div, chroma, chroma_div)
}

const ONE_HALF: i64 = 1 << 15;
const CBCR_OFFSET: i64 = 128 << 16;

fn fix(value: f64) -> i64 { (value * 65536.0 + 0.5) as i64 }

struct YccTables {
    entries: [i64; 9 * 256],
}

impl YccTables {
    fn new() -> Self {
        let coefficients = [
            (fix(0.29900), 0i64),
            (fix(0.58700), 0),
            (fix(0.11400), ONE_HALF),
            (-fix(0.16874), 0),
            (-fix(0.33126), 0),
            (fix(0.50000), CBCR_OFFSET + ONE_HALF - 1),
            (fix(0.50000), CBCR_OFFSET + ONE_HALF - 1),
            (-fix(0.41869), 0),
            (-fix(0.08131), 0),
        ];
        let mut entries = [0i64; 9 * 256];
        for (channel, (factor, offset)) in coefficients.iter().enumerate() {
            for value in 0..256 {
                entries[channel * 256 + value] = factor * value as i64 + offset;
            }
        }
        YccTables { entries }
    }

    fn convert(&self, r: u8, g: u8, b: u8) -> (u8, u8, u8) {
        let table = &self.entries;
        let y = table[r as usize] + table[256 + g as usize] + table[2 * 256 + b as usize];
        let cb = table[3 * 256 + r as usize] + table[4 * 256 + g as usize] + table[5 * 256 + b as usize];
        let cr = table[6 * 256 + r as usize] + table[7 * 256 + g as usize] + table[8 * 256 + b as usize];
        ((y >> 16) as u8, (cb >> 16) as u8, (cr >> 16) as u8)
    }
}

fn premul(value: u8, alpha: u8) -> u8 {
    let prod = value as u32 * alpha as u32 + 128;
    ((prod + (prod >> 8)) >> 8) as u8
}

struct HuffTable {
    bits: [u8; 17],
    huffval: Vec<u8>,
    encode: [u16; 256],
    size: [u8; 256],
}

impl HuffTable {
    fn empty() -> Self {
        HuffTable { bits: [0; 17], huffval: Vec::new(), encode: [0; 256], size: [0; 256] }
    }

    fn derive(&mut self) {
        let mut huffsize = [0u8; 257];
        let mut huffcode = [0u32; 257];
        let mut position = 0usize;
        for length in 1..=16usize {
            for _ in 0..self.bits[length] {
                huffsize[position] = length as u8;
                position += 1;
            }
        }
        let mut code = 0u32;
        let mut length = huffsize[0] as usize;
        let mut index = 0usize;
        while huffsize[index] != 0 {
            while huffsize[index] as usize == length {
                huffcode[index] = code;
                code += 1;
                index += 1;
            }
            code <<= 1;
            length += 1;
        }
        for i in 0..position {
            let symbol = self.huffval[i] as usize;
            self.encode[symbol] = huffcode[i] as u16;
            self.size[symbol] = huffsize[i];
        }
    }
}

fn optimal_table(freq: &mut [i64; 257]) -> HuffTable {
    const MAX_CLEN: usize = 32;
    let mut bits = [0u8; MAX_CLEN + 1];
    let mut codesize = [0i32; 257];
    let mut nz_index = [0i32; 257];
    let mut others = [-1i32; 257];
    freq[256] = 1;
    let mut count = 0usize;
    for i in 0..257 {
        if freq[i] != 0 {
            nz_index[count] = i as i32;
            freq[count] = freq[i];
            count += 1;
        }
    }
    loop {
        let mut c1 = -1i32;
        let mut c2 = -1i32;
        let mut smallest = 1_000_000_000i64;
        let mut second = 1_000_000_000i64;
        for i in 0..count {
            if freq[i] <= second {
                if freq[i] <= smallest {
                    c2 = c1;
                    second = smallest;
                    smallest = freq[i];
                    c1 = i as i32;
                } else {
                    second = freq[i];
                    c2 = i as i32;
                }
            }
        }
        if c2 < 0 { break; }
        let mut a = c1 as usize;
        let mut b = c2 as usize;
        freq[a] += freq[b];
        freq[b] = 1_000_000_001;
        codesize[a] += 1;
        while others[a] >= 0 {
            a = others[a] as usize;
            codesize[a] += 1;
        }
        others[a] = c2;
        codesize[b] += 1;
        while others[b] >= 0 {
            b = others[b] as usize;
            codesize[b] += 1;
        }
    }
    for i in 0..count {
        bits[codesize[i] as usize] += 1;
    }
    let mut bit_pos = [0i32; MAX_CLEN + 1];
    let mut position = 0i32;
    for i in 1..=MAX_CLEN {
        bit_pos[i] = position;
        position += bits[i] as i32;
    }
    let mut length = MAX_CLEN;
    while length > 16 {
        while bits[length] > 0 {
            let mut shorter = length - 2;
            while bits[shorter] == 0 { shorter -= 1; }
            bits[length] -= 2;
            bits[length - 1] += 1;
            bits[shorter + 1] += 2;
            bits[shorter] -= 1;
        }
        length -= 1;
    }
    let mut largest = 16;
    while bits[largest] == 0 { largest -= 1; }
    bits[largest] -= 1;
    let total: usize = bits[1..=16].iter().map(|&count| count as usize).sum();
    let mut table = HuffTable::empty();
    table.bits.copy_from_slice(&bits[..17]);
    let mut huffval = vec![0u8; total];
    for i in 0..count.saturating_sub(1) {
        let code_length = codesize[i] as usize;
        let position = bit_pos[code_length] as usize;
        huffval[position] = nz_index[i] as u8;
        bit_pos[code_length] += 1;
    }
    table.huffval = huffval;
    table.derive();
    table
}

fn fdct(block: &mut [i32; DCTSIZE2]) {
    const C: i32 = 13;
    const P: i32 = 2;
    let descale = |x: i32, n: u32| (x + (1i32 << (n - 1))) >> n;
    for row in 0..DCTSIZE {
        let o = row * DCTSIZE;
        let t0 = block[o] + block[o + 7];
        let t7 = block[o] - block[o + 7];
        let t1 = block[o + 1] + block[o + 6];
        let t6 = block[o + 1] - block[o + 6];
        let t2 = block[o + 2] + block[o + 5];
        let t5 = block[o + 2] - block[o + 5];
        let t3 = block[o + 3] + block[o + 4];
        let t4 = block[o + 3] - block[o + 4];
        let t10 = t0 + t3;
        let t13 = t0 - t3;
        let t11 = t1 + t2;
        let t12 = t1 - t2;
        block[o] = (t10 + t11) << P;
        block[o + 4] = (t10 - t11) << P;
        let z = (t12 + t13) * 4433;
        block[o + 2] = descale(z + t13 * 6270, (C - P) as u32);
        block[o + 6] = descale(z - t12 * 15137, (C - P) as u32);
        let mut z1 = t4 + t7;
        let mut z2 = t5 + t6;
        let mut z3 = t4 + t6;
        let mut z4 = t5 + t7;
        let z5 = (z3 + z4) * 9633;
        let a4 = t4 * 2446;
        let a5 = t5 * 16819;
        let a6 = t6 * 25172;
        let a7 = t7 * 12299;
        z1 = -z1 * 7373;
        z2 = -z2 * 20995;
        z3 = -z3 * 16069 + z5;
        z4 = -z4 * 3196 + z5;
        block[o + 7] = descale(a4 + z1 + z3, (C - P) as u32);
        block[o + 5] = descale(a5 + z2 + z4, (C - P) as u32);
        block[o + 3] = descale(a6 + z2 + z3, (C - P) as u32);
        block[o + 1] = descale(a7 + z1 + z4, (C - P) as u32);
    }
    for column in 0..DCTSIZE {
        let o = column;
        let t0 = block[o] + block[o + DCTSIZE * 7];
        let t7 = block[o] - block[o + DCTSIZE * 7];
        let t1 = block[o + DCTSIZE] + block[o + DCTSIZE * 6];
        let t6 = block[o + DCTSIZE] - block[o + DCTSIZE * 6];
        let t2 = block[o + DCTSIZE * 2] + block[o + DCTSIZE * 5];
        let t5 = block[o + DCTSIZE * 2] - block[o + DCTSIZE * 5];
        let t3 = block[o + DCTSIZE * 3] + block[o + DCTSIZE * 4];
        let t4 = block[o + DCTSIZE * 3] - block[o + DCTSIZE * 4];
        let t10 = t0 + t3;
        let t13 = t0 - t3;
        let t11 = t1 + t2;
        let t12 = t1 - t2;
        block[o] = descale(t10 + t11, P as u32);
        block[o + DCTSIZE * 4] = descale(t10 - t11, P as u32);
        let z = (t12 + t13) * 4433;
        block[o + DCTSIZE * 2] = descale(z + t13 * 6270, (C + P) as u32);
        block[o + DCTSIZE * 6] = descale(z - t12 * 15137, (C + P) as u32);
        let mut z1 = t4 + t7;
        let mut z2 = t5 + t6;
        let mut z3 = t4 + t6;
        let mut z4 = t5 + t7;
        let z5 = (z3 + z4) * 9633;
        let a4 = t4 * 2446;
        let a5 = t5 * 16819;
        let a6 = t6 * 25172;
        let a7 = t7 * 12299;
        z1 = -z1 * 7373;
        z2 = -z2 * 20995;
        z3 = -z3 * 16069 + z5;
        z4 = -z4 * 3196 + z5;
        block[o + DCTSIZE * 7] = descale(a4 + z1 + z3, (C + P) as u32);
        block[o + DCTSIZE * 5] = descale(a5 + z2 + z4, (C + P) as u32);
        block[o + DCTSIZE * 3] = descale(a6 + z2 + z3, (C + P) as u32);
        block[o + DCTSIZE] = descale(a7 + z1 + z4, (C + P) as u32);
    }
}

struct BitWriter {
    output: Vec<u8>,
    buffer: u64,
    bits: u32,
}

impl BitWriter {
    fn new() -> Self { BitWriter { output: Vec::new(), buffer: 0, bits: 0 } }

    fn put(&mut self, code: u16, size: u8) {
        if size == 0 { return; }
        self.buffer = (self.buffer << size) | code as u64;
        self.bits += size as u32;
        while self.bits >= 8 {
            self.bits -= 8;
            let byte = (self.buffer >> self.bits) as u8;
            self.output.push(byte);
            if byte == 0xFF { self.output.push(0); }
        }
    }

    fn flush(&mut self) {
        if self.bits > 0 {
            let byte = ((self.buffer << (8 - self.bits)) | (0xFFu64 >> self.bits)) as u8;
            self.output.push(byte);
            if byte == 0xFF { self.output.push(0); }
            self.bits = 0;
        }
    }
}

fn nbits(value: u32) -> u8 {
    if value == 0 { 0 } else { 32 - value.leading_zeros() as u8 }
}

fn magnitude_bits(value: i32) -> (u8, u16) {
    let magnitude = value.unsigned_abs();
    let size = nbits(magnitude);
    let bits = if value < 0 {
        ((1u64 << size) - 1 - magnitude as u64) as u16
    } else {
        magnitude as u16
    };
    (size, bits)
}

fn emit_block(
    writer: &mut BitWriter,
    block: &[i32; DCTSIZE2],
    last_dc: &mut i32,
    dc: &HuffTable,
    ac: &HuffTable,
) {
    let (size, bits) = magnitude_bits(block[0] - *last_dc);
    *last_dc = block[0];
    writer.put(dc.encode[size as usize], dc.size[size as usize]);
    writer.put(bits, size);
    let mut run = 0usize;
    for &k in NATURAL.iter().skip(1) {
        if block[k] == 0 {
            run += 16;
            continue;
        }
        while run >= 256 {
            writer.put(ac.encode[0xF0], ac.size[0xF0]);
            run -= 256;
        }
        let (size, bits) = magnitude_bits(block[k]);
        writer.put(ac.encode[run + size as usize], ac.size[run + size as usize]);
        writer.put(bits, size);
        run = 0;
    }
    if run > 0 {
        writer.put(ac.encode[0], ac.size[0]);
    }
}

fn count_block(
    block: &[i32; DCTSIZE2],
    last_dc: &mut i32,
    dc: &mut [i64; 257],
    ac: &mut [i64; 257],
) {
    let (size, _) = magnitude_bits(block[0] - *last_dc);
    *last_dc = block[0];
    dc[size as usize] += 1;
    let mut run = 0usize;
    for &k in NATURAL.iter().skip(1) {
        if block[k] == 0 {
            run += 1;
            continue;
        }
        while run > 15 {
            ac[0xF0] += 1;
            run -= 16;
        }
        let (size, _) = magnitude_bits(block[k]);
        ac[(run << 4) + size as usize] += 1;
        run = 0;
    }
    if run > 0 { ac[0] += 1; }
}

fn put_marker(output: &mut Vec<u8>, marker: u8, payload: &[u8]) {
    output.push(0xFF);
    output.push(marker);
    output.push(((payload.len() + 2) >> 8) as u8);
    output.push((payload.len() + 2) as u8);
    output.extend_from_slice(payload);
}

struct Component {
    blocks_w: usize,
    blocks_h: usize,
    h: usize,
    v: usize,
    plane: Vec<u8>,
}

pub fn encode(
    width: usize,
    height: usize,
    pixels: &[u8],
    color: ColorSpace,
    options: &Options<'_>,
) -> Result<Vec<u8>, Error> {
    if width == 0 || height == 0 { return Err(Error::Dimension); }
    let bpp = match color {
        ColorSpace::Bgra | ColorSpace::Rgba => 4,
        ColorSpace::Rgb | ColorSpace::YcbCr => 3,
    };
    if pixels.len() != width * height * bpp { return Err(Error::PixelCount); }

    let (max_h, max_v): (usize, usize) = match options.downsample {
        Downsample::S444 => (1, 1),
        Downsample::S422 => (2, 1),
        Downsample::S420 => (2, 2),
    };
    let mcu_rows = height.div_ceil(max_v * DCTSIZE);
    let full_rows = max_v * DCTSIZE * mcu_rows;
    let tables = YccTables::new();

    let mut full = [vec![0u8; width * full_rows], vec![0u8; width * full_rows], vec![0u8; width * full_rows]];
    for y in 0..height {
        for x in 0..width {
            let source = (y * width + x) * bpp;
            let (r, g, b) = match color {
                ColorSpace::Bgra => (pixels[source + 2], pixels[source + 1], pixels[source]),
                ColorSpace::Rgba => (pixels[source], pixels[source + 1], pixels[source + 2]),
                ColorSpace::Rgb => (pixels[source], pixels[source + 1], pixels[source + 2]),
                ColorSpace::YcbCr => {
                    full[0][y * width + x] = pixels[source];
                    full[1][y * width + x] = pixels[source + 1];
                    full[2][y * width + x] = pixels[source + 2];
                    continue;
                }
            };
            let (r, g, b) = if options.blend_on_black && bpp == 4 {
                let a = pixels[source + 3];
                (premul(r, a), premul(g, a), premul(b, a))
            } else {
                (r, g, b)
            };
            let (y_value, cb, cr) = tables.convert(r, g, b);
            full[0][y * width + x] = y_value;
            full[1][y * width + x] = cb;
            full[2][y * width + x] = cr;
        }
    }
    for y in height..full_rows {
        for component in full.iter_mut() {
            let last = (height - 1) * width;
            let (head, tail) = component.split_at_mut(last + width);
            let source = &head[last..last + width];
            let start = y * width - (last + width);
            tail[start..start + width].copy_from_slice(source);
        }
    }

    let scale = scaling(options.quality);
    let (luma_values, luma_div, chroma_values, chroma_div) = quant_tables(scale);

    let mut components = Vec::new();
    for ci in 0..3 {
        let (hf, vf) = if ci == 0 { (max_h, max_v) } else { (1, 1) };
        let comp_w = (width * hf).div_ceil(max_h);
        let blocks_w = comp_w.div_ceil(DCTSIZE);
        let comp_h = (height * vf).div_ceil(max_v);
        let blocks_h = comp_h.div_ceil(DCTSIZE);
        let padded_w = blocks_w * DCTSIZE;
        let rows = vf * DCTSIZE * mcu_rows;
        let mut plane = vec![0u8; padded_w * rows];
        for out_row in 0..rows {
            let real_row = out_row.min(comp_h - 1);
            for out_col in 0..padded_w {
                let value = if hf == max_h && vf == max_v {
                    full[ci][real_row * width + out_col.min(width - 1)]
                } else if max_v == 2 {
                    let x = (out_col * 2).min(width - 1);
                    let x1 = (x + 1).min(width - 1);
                    let y0 = real_row * 2;
                    let y1 = (y0 + 1).min(height - 1);
                    let bias = if out_col & 1 == 0 { 1 } else { 2 };
                    let a = full[ci][y0 * width + x] as u32;
                    let b = full[ci][y0 * width + x1] as u32;
                    let c = full[ci][y1 * width + x] as u32;
                    let d = full[ci][y1 * width + x1] as u32;
                    ((a + b + c + d + bias) >> 2) as u8
                } else {
                    let x = (out_col * 2).min(width - 1);
                    let x1 = (x + 1).min(width - 1);
                    let bias = if out_col & 1 == 0 { 0 } else { 1 };
                    let a = full[ci][real_row * width + x] as u32;
                    let b = full[ci][real_row * width + x1] as u32;
                    ((a + b + bias) >> 1) as u8
                };
                plane[out_row * padded_w + out_col] = value;
            }
        }
        components.push(Component { blocks_w, blocks_h, h: hf, v: vf, plane });
    }

    let mcus_per_row = width.div_ceil(max_h * DCTSIZE);
    let mut blocks: Vec<[i32; DCTSIZE2]> = Vec::new();
    let mut block_components: Vec<usize> = Vec::new();
    let mut last_dc = [0i32; 3];
    let mut dc_counts = [[0i64; 257], [0i64; 257]];
    let mut ac_counts = [[0i64; 257], [0i64; 257]];
    for i_mcu in 0..mcu_rows {
        for mcu in 0..mcus_per_row {
            for ci in 0..3 {
                let component = &components[ci];
                let last_col_width = component.blocks_w % component.h;
                let last_col_width = if last_col_width == 0 { component.h } else { last_col_width };
                let last_row_height = component.blocks_h % component.v;
                let last_row_height = if last_row_height == 0 { component.v } else { last_row_height };
                let padded_w = component.blocks_w * DCTSIZE;
                let divisors = if ci == 0 { &luma_div } else { &chroma_div };
                for by in 0..component.v {
                    let bottom = i_mcu + 1 == mcu_rows && by >= last_row_height;
                    let count = if bottom { 0 } else if mcu + 1 == mcus_per_row { last_col_width } else { component.h };
                    for bx in 0..component.h {
                        let mut block = [0i32; DCTSIZE2];
                        if bx < count {
                            let block_x = mcu * component.h + bx;
                            let block_y = i_mcu * component.v + by;
                            let mut workspace = [0i32; DCTSIZE2];
                            for row in 0..DCTSIZE {
                                for col in 0..DCTSIZE {
                                    let source = (block_y * DCTSIZE + row) * padded_w + block_x * DCTSIZE + col;
                                    workspace[row * DCTSIZE + col] = component.plane[source] as i32 - 128;
                                }
                            }
                            fdct(&mut workspace);
                            for i in 0..DCTSIZE2 {
                                block[i] = quantize(workspace[i], &divisors[i]);
                            }
                        } else {
                            block[0] = blocks.last().map_or(0, |previous| previous[0]);
                        }
                        let table = if ci == 0 { 0 } else { 1 };
                        count_block(&block, &mut last_dc[ci], &mut dc_counts[table], &mut ac_counts[table]);
                        blocks.push(block);
                        block_components.push(ci);
                    }
                }
            }
        }
    }

    let dc0 = optimal_table(&mut dc_counts[0]);
    let ac0 = optimal_table(&mut ac_counts[0]);
    let dc1 = optimal_table(&mut dc_counts[1]);
    let ac1 = optimal_table(&mut ac_counts[1]);

    let mut output = Vec::new();
    output.extend_from_slice(&[0xFF, 0xD8]);
    put_marker(&mut output, 0xE0, &[
        0x4A, 0x46, 0x49, 0x46, 0x00, 0x01, 0x01, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00,
    ]);
    if let Some(icc) = options.icc {
        put_marker(&mut output, 0xE2, icc);
    }
    let mut dqt = Vec::with_capacity(65);
    dqt.push(0);
    dqt.extend(NATURAL.iter().map(|&i| luma_values[i] as u8));
    put_marker(&mut output, 0xDB, &dqt);
    let mut dqt = Vec::with_capacity(65);
    dqt.push(1);
    dqt.extend(NATURAL.iter().map(|&i| chroma_values[i] as u8));
    put_marker(&mut output, 0xDB, &dqt);

    let mut sof = Vec::new();
    sof.push(8);
    sof.extend_from_slice(&(height as u16).to_be_bytes());
    sof.extend_from_slice(&(width as u16).to_be_bytes());
    sof.push(3);
    sof.extend_from_slice(&[1, ((max_h as u8) << 4) | max_v as u8, 0]);
    sof.extend_from_slice(&[2, 0x11, 1, 3, 0x11, 1]);
    put_marker(&mut output, 0xC0, &sof);

    for (id, dc, ac) in [(0usize, &dc0, &ac0), (1, &dc1, &ac1)] {
        let mut segment = vec![id as u8];
        segment.extend_from_slice(&dc.bits[1..=16]);
        segment.extend_from_slice(&dc.huffval);
        put_marker(&mut output, 0xC4, &segment);
        let mut segment = vec![0x10 | id as u8];
        segment.extend_from_slice(&ac.bits[1..=16]);
        segment.extend_from_slice(&ac.huffval);
        put_marker(&mut output, 0xC4, &segment);
    }

    let mut sos = Vec::new();
    sos.push(3);
    sos.extend_from_slice(&[1, 0, 2, 0x11, 3, 0x11]);
    sos.extend_from_slice(&[0, 63, 0]);
    put_marker(&mut output, 0xDA, &sos);

    let mut writer = BitWriter::new();
    let mut last = [0i32; 3];
    for (block, &ci) in blocks.iter().zip(block_components.iter()) {
        if ci == 0 {
            emit_block(&mut writer, block, &mut last[0], &dc0, &ac0);
        } else {
            emit_block(&mut writer, block, &mut last[ci], &dc1, &ac1);
        }
    }
    writer.flush();
    output.extend_from_slice(&writer.output);
    output.extend_from_slice(&[0xFF, 0xD9]);
    Ok(output)
}

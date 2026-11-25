#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Color {
    Gray,
    XAlpha,
    GrayAlpha,
    Palette,
    Rgb,
    Rgb565,
    Rgba,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Alpha {
    Opaque,
    Unpremul,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DstAlpha {
    Opaque,
    Premul,
}

#[derive(Clone, Copy, Debug)]
pub struct EncodedInfo {
    pub width: i32,
    pub height: i32,
    pub color: Color,
    pub alpha: Alpha,
    pub bits_per_component: u8,
}

impl EncodedInfo {
    pub fn bits_per_pixel(&self) -> u32 {
        let bits = u32::from(self.bits_per_component);
        match self.color {
            Color::Gray | Color::Palette => bits,
            Color::XAlpha | Color::GrayAlpha => 2 * bits,
            Color::Rgb | Color::Rgb565 => 3 * bits,
            Color::Rgba => 4 * bits,
        }
    }

    pub fn row_bytes(&self, width: i32) -> usize {
        let bits = self.bits_per_pixel();
        let width = width as usize;
        if bits < 16 {
            let per_byte = (8 / bits) as usize;
            width.div_ceil(per_byte)
        } else {
            width * (bits / 8) as usize
        }
    }
}

pub fn mul_div_255_round(a: u32, b: u32) -> u32 {
    let product = a * b + 128;
    (product + (product >> 8)) >> 8
}

fn pack(a: u32, r: u32, g: u32, b: u32) -> u32 {
    a << 24 | r << 16 | g << 8 | b
}

fn premultiply(a: u32, r: u32, g: u32, b: u32) -> u32 {
    if a != 255 {
        pack(a, mul_div_255_round(r, a), mul_div_255_round(g, a), mul_div_255_round(b, a))
    } else {
        pack(a, r, g, b)
    }
}

pub fn needs_premul(dst: DstAlpha, encoded: Alpha) -> bool {
    dst == DstAlpha::Premul && encoded == Alpha::Unpremul
}

pub fn color_table(palette: &[u8], trns: Option<&[u8]>, dst: DstAlpha, info: &EncodedInfo) -> Vec<u32> {
    let colors = (palette.len() / 3).min(256);
    let alphas = trns.map_or(&[][..], |bytes| &bytes[..bytes.len().min(colors)]);
    let premul = needs_premul(dst, info.alpha);
    let mut table = Vec::with_capacity(256);
    for index in 0..colors {
        let (r, g, b) = (u32::from(palette[3 * index]), u32::from(palette[3 * index + 1]), u32::from(palette[3 * index + 2]));
        table.push(if index < alphas.len() {
            let a = u32::from(alphas[index]);
            if premul { premultiply(a, r, g, b) } else { pack(a, r, g, b) }
        } else {
            pack(255, r, g, b)
        });
    }
    let max_colors = 1usize << info.bits_per_component;
    if colors < max_colors {
        let last = table.last().copied().unwrap_or(0xFF00_0000);
        table.resize(max_colors, last);
    }
    table.truncate(max_colors);
    table
}

pub struct Swizzler {
    color: Color,
    bits: u8,
    premultiply: bool,
    table: Option<Vec<u32>>,
    width: usize,
}

impl Swizzler {
    pub fn make(info: &EncodedInfo, table: Option<Vec<u32>>, dst: DstAlpha, width: usize) -> Option<Self> {
        let supported = match info.color {
            Color::Gray => info.bits_per_component == 8,
            Color::XAlpha | Color::GrayAlpha => true,
            Color::Palette => table.is_some() && matches!(info.bits_per_component, 1 | 2 | 4 | 8),
            Color::Rgb | Color::Rgb565 => matches!(info.bits_per_component, 8 | 16),
            Color::Rgba => matches!(info.bits_per_component, 8 | 16),
        };
        supported.then(|| Self {
            color: info.color,
            bits: info.bits_per_component,
            premultiply: info.alpha != Alpha::Opaque && dst == DstAlpha::Premul,
            table,
            width,
        })
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn swizzle(&self, dst: &mut [u8], src: &[u8]) {
        for x in 0..self.width {
            let pixel = match self.color {
                Color::Gray => {
                    let g = u32::from(src[x]);
                    pack(255, g, g, g)
                }
                Color::XAlpha | Color::GrayAlpha => {
                    let (g, a) = (u32::from(src[2 * x]), u32::from(src[2 * x + 1]));
                    if self.premultiply {
                        let p = mul_div_255_round(a, g);
                        pack(a, p, p, p)
                    } else {
                        pack(a, g, g, g)
                    }
                }
                Color::Palette => {
                    let table = self.table.as_ref().expect("palette table");
                    let index = if self.bits == 8 {
                        usize::from(src[x])
                    } else {
                        let bits = usize::from(self.bits);
                        let bit = x * bits;
                        let mask = (1usize << bits) - 1;
                        (usize::from(src[bit / 8]) >> (8 - bits - bit % 8)) & mask
                    };
                    table[index]
                }
                Color::Rgb | Color::Rgb565 => {
                    if self.bits == 16 {
                        pack(255, u32::from(src[6 * x]), u32::from(src[6 * x + 2]), u32::from(src[6 * x + 4]))
                    } else {
                        pack(255, u32::from(src[3 * x]), u32::from(src[3 * x + 1]), u32::from(src[3 * x + 2]))
                    }
                }
                Color::Rgba => {
                    let (step, offsets) = if self.bits == 16 { (8, [0, 2, 4, 6]) } else { (4, [0, 1, 2, 3]) };
                    let base = step * x;
                    let (r, g, b, a) = (
                        u32::from(src[base + offsets[0]]),
                        u32::from(src[base + offsets[1]]),
                        u32::from(src[base + offsets[2]]),
                        u32::from(src[base + offsets[3]]),
                    );
                    if self.premultiply { premultiply(a, r, g, b) } else { pack(a, r, g, b) }
                }
            };
            dst[4 * x..4 * x + 4].copy_from_slice(&pixel.to_le_bytes());
        }
    }
}

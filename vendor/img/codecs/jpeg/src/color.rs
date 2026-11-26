use super::decompress::{ColorSpace, Decompress, Result};
use super::error::Error;
use super::source::Source;

const SCALEBITS: i32 = 16;
const ONE_HALF: i64 = 1 << (SCALEBITS - 1);

fn fix(x: f64) -> i64 {
    (x * (1i64 << SCALEBITS) as f64 + 0.5) as i64
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Conversion {
    YccBgra,
    GrayBgra,
    RgbBgra,
    YcckCmyk,
    Null,
}

pub(crate) struct ColorDeconverter {
    conversion: Conversion,
    components: usize,
    cr_r: Vec<i32>,
    cb_b: Vec<i32>,
    cr_g: Vec<i64>,
    cb_g: Vec<i64>,
}

fn limit(value: i32) -> u8 {
    value.clamp(0, 255) as u8
}

impl ColorDeconverter {
    fn build_ycc_tables(&mut self) {
        for i in 0..256i64 {
            let x = i - 128;
            self.cr_r.push(((fix(1.40200) * x + ONE_HALF) >> SCALEBITS) as i32);
            self.cb_b.push(((fix(1.77200) * x + ONE_HALF) >> SCALEBITS) as i32);
            self.cr_g.push(-fix(0.71414) * x);
            self.cb_g.push(-fix(0.34414) * x + ONE_HALF);
        }
    }

    fn green(&self, cb: usize, cr: usize) -> i32 {
        ((self.cb_g[cb] + self.cr_g[cr]) >> SCALEBITS) as i32
    }

    pub fn convert(&self, rows: &[&[u8]], output: &mut [u8], width: usize) {
        match self.conversion {
            Conversion::YccBgra => {
                for col in 0..width {
                    let y = rows[0][col] as i32;
                    let cb = rows[1][col] as usize;
                    let cr = rows[2][col] as usize;
                    let out = &mut output[col * 4..col * 4 + 4];
                    out[2] = limit(y + self.cr_r[cr]);
                    out[1] = limit(y + self.green(cb, cr));
                    out[0] = limit(y + self.cb_b[cb]);
                    out[3] = 255;
                }
            }
            Conversion::GrayBgra => {
                for col in 0..width {
                    let value = rows[0][col];
                    output[col * 4..col * 4 + 4].copy_from_slice(&[value, value, value, 255]);
                }
            }
            Conversion::RgbBgra => {
                for col in 0..width {
                    output[col * 4..col * 4 + 4].copy_from_slice(&[rows[2][col], rows[1][col], rows[0][col], 255]);
                }
            }
            Conversion::YcckCmyk => {
                for col in 0..width {
                    let y = rows[0][col] as i32;
                    let cb = rows[1][col] as usize;
                    let cr = rows[2][col] as usize;
                    let out = &mut output[col * 4..col * 4 + 4];
                    out[0] = limit(255 - (y + self.cr_r[cr]));
                    out[1] = limit(255 - (y + self.green(cb, cr)));
                    out[2] = limit(255 - (y + self.cb_b[cb]));
                    out[3] = rows[3][col];
                }
            }
            Conversion::Null => {
                let n = self.components;
                for col in 0..width {
                    for (ci, row) in rows.iter().enumerate().take(n) {
                        output[col * n + ci] = row[col];
                    }
                }
            }
        }
    }
}

impl<S: Source> Decompress<S> {
    pub(crate) fn init_color_deconverter(&mut self) -> Result<()> {
        let n = self.num_components;
        let valid = match self.jpeg_color_space {
            ColorSpace::Grayscale => n == 1,
            ColorSpace::Rgb | ColorSpace::YCbCr => n == 3,
            ColorSpace::Cmyk | ColorSpace::Ycck => n == 4,
            _ => n >= 1,
        };
        if !valid {
            return Err(Error::BadJColorSpace);
        }
        let mut cconvert = ColorDeconverter { conversion: Conversion::Null, components: n as usize, cr_r: Vec::new(), cb_b: Vec::new(), cr_g: Vec::new(), cb_g: Vec::new() };
        let lossless = self.master.lossless;
        match self.out_color_space {
            ColorSpace::ExtBgra => {
                if lossless && self.jpeg_color_space != ColorSpace::Rgb {
                    return Err(Error::ConversionNotImplemented);
                }
                self.out_color_components = 4;
                cconvert.conversion = match self.jpeg_color_space {
                    ColorSpace::YCbCr => {
                        cconvert.build_ycc_tables();
                        Conversion::YccBgra
                    }
                    ColorSpace::Grayscale => Conversion::GrayBgra,
                    ColorSpace::Rgb => Conversion::RgbBgra,
                    _ => return Err(Error::ConversionNotImplemented),
                };
            }
            ColorSpace::Cmyk => {
                if lossless && self.jpeg_color_space != self.out_color_space {
                    return Err(Error::ConversionNotImplemented);
                }
                self.out_color_components = 4;
                cconvert.conversion = match self.jpeg_color_space {
                    ColorSpace::Ycck => {
                        cconvert.build_ycc_tables();
                        Conversion::YcckCmyk
                    }
                    ColorSpace::Cmyk => Conversion::Null,
                    _ => return Err(Error::ConversionNotImplemented),
                };
            }
            ColorSpace::Unknown if self.jpeg_color_space == ColorSpace::Unknown => {
                self.out_color_components = n;
            }
            _ => return Err(Error::ConversionNotImplemented),
        }
        self.output_components = self.out_color_components;
        self.cconvert = Some(cconvert);
        Ok(())
    }
}

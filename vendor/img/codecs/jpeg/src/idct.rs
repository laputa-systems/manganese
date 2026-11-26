use super::decompress::{Decompress, IdctKind, Result};
use super::error::Error;
use super::source::Source;
use super::tables::DCTSIZE2;

const F_0_298: i32 = 2446;
const F_0_390: i32 = 3196;
const F_0_541: i32 = 4433;
const F_0_765: i32 = 6270;
const F_0_899: i32 = 7373;
const F_1_175: i32 = 9633;
const F_1_501: i32 = 12299;
const F_1_847: i32 = 15137;
const F_1_961: i32 = 16069;
const F_2_053: i32 = 16819;
const F_2_562: i32 = 20995;
const F_3_072: i32 = 25172;
const DESCALE_P1: u32 = 13 - 2;
const DESCALE_P2: u32 = 13 + 2 + 3;

pub(crate) struct Idct {
    cur_method_islow: Vec<bool>,
}

impl Idct {
    pub fn new(components: usize) -> Self {
        Idct { cur_method_islow: vec![false; components] }
    }
}

fn saturate_i16(value: i32) -> i16 {
    value.clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

fn descale(value: i32, bits: u32) -> i16 {
    saturate_i16(value.wrapping_add(1 << (bits - 1)) >> bits)
}

fn dodct(input: [i16; 8], bits: u32) -> [i16; 8] {
    let [_, in1, in2, in3, _, in5, in6, in7] = input.map(i32::from);
    let tmp3 = in2 * (F_0_541 + F_0_765) + in6 * F_0_541;
    let tmp2 = in2 * F_0_541 + in6 * (F_0_541 - F_1_847);
    let tmp0 = (input[0].wrapping_add(input[4]) as i32) << 13;
    let tmp1 = (input[0].wrapping_sub(input[4]) as i32) << 13;
    let tmp10 = tmp0.wrapping_add(tmp3);
    let tmp13 = tmp0.wrapping_sub(tmp3);
    let tmp11 = tmp1.wrapping_add(tmp2);
    let tmp12 = tmp1.wrapping_sub(tmp2);
    let z3 = input[7].wrapping_add(input[3]) as i32;
    let z4 = input[5].wrapping_add(input[1]) as i32;
    let z3p = z3 * (F_1_175 - F_1_961) + z4 * F_1_175;
    let z4p = z3 * F_1_175 + z4 * (F_1_175 - F_0_390);
    let odd0 = (in7 * (F_0_298 - F_0_899) + in1 * -F_0_899).wrapping_add(z3p);
    let odd1 = (in5 * (F_2_053 - F_2_562) + in3 * -F_2_562).wrapping_add(z4p);
    let odd3 = (in7 * -F_0_899 + in1 * (F_1_501 - F_0_899)).wrapping_add(z4p);
    let odd2 = (in5 * -F_2_562 + in3 * (F_3_072 - F_2_562)).wrapping_add(z3p);
    [
        descale(tmp10.wrapping_add(odd3), bits),
        descale(tmp11.wrapping_add(odd2), bits),
        descale(tmp12.wrapping_add(odd1), bits),
        descale(tmp13.wrapping_add(odd0), bits),
        descale(tmp13.wrapping_sub(odd0), bits),
        descale(tmp12.wrapping_sub(odd1), bits),
        descale(tmp11.wrapping_sub(odd2), bits),
        descale(tmp10.wrapping_sub(odd3), bits),
    ]
}

pub(crate) fn idct_islow(table: &[i16; DCTSIZE2], coef: &[i16; DCTSIZE2], plane: &mut [u8], stride: usize, rows: &[usize], col: usize) {
    let mut workspace = [[0i16; 8]; 8];
    if coef[8..].iter().all(|&value| value == 0) {
        for c in 0..8 {
            let value = coef[c].wrapping_mul(table[c]).wrapping_shl(2);
            for row in workspace.iter_mut() {
                row[c] = value;
            }
        }
    } else {
        for c in 0..8 {
            let column = std::array::from_fn(|r| coef[r * 8 + c].wrapping_mul(table[r * 8 + c]));
            let out = dodct(column, DESCALE_P1);
            for (r, value) in out.into_iter().enumerate() {
                workspace[r][c] = value;
            }
        }
    }
    for (r, row) in workspace.iter().enumerate() {
        let out = dodct(*row, DESCALE_P2);
        let start = rows[r] * stride + col;
        for (c, value) in out.into_iter().enumerate() {
            plane[start + c] = ((value as i32).clamp(-128, 127) + 128) as u8;
        }
    }
}

impl<S: Source> Decompress<S> {
    pub(crate) fn idct_start_pass(&mut self) -> Result<()> {
        match &mut self.idct {
            IdctKind::Lossy(idct) => {
                for ci in 0..self.num_components as usize {
                    let compptr = &mut self.comp_info[ci];
                    if compptr.dct_scaled_size != 8 {
                        return Err(Error::BadDctSize(compptr.dct_scaled_size));
                    }
                    if !compptr.component_needed || idct.cur_method_islow[ci] {
                        continue;
                    }
                    let Some(qtbl) = compptr.quant_table else { continue };
                    idct.cur_method_islow[ci] = true;
                    for i in 0..DCTSIZE2 {
                        compptr.dct_table[i] = qtbl[i] as i16;
                    }
                }
                Ok(())
            }
            IdctKind::Lossless(_) => self.start_pass_lossless(),
            IdctKind::None => Ok(()),
        }
    }
}

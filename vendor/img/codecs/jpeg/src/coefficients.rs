use super::decompress::{CoefKind, ConsumeStatus, Decompress, EntropyKind, Result};
use super::idct::idct_islow;
use super::main_buffer::Lists;
use super::source::Source;
use super::tables::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum DecompressMode {
    OnePass,
    Multi,
    Smooth,
}

pub(crate) struct BlockArray {
    pub blocks_per_row: usize,
    pub data: Vec<[i16; DCTSIZE2]>,
}

impl BlockArray {
    fn index(&self, row: usize, col: usize) -> usize {
        row * self.blocks_per_row + col
    }
}

const SAVED_COEFS: usize = 10;
const Q01_POS: usize = 1;
const Q10_POS: usize = 8;
const Q20_POS: usize = 16;
const Q11_POS: usize = 9;
const Q02_POS: usize = 2;
const Q03_POS: usize = 3;
const Q12_POS: usize = 10;
const Q21_POS: usize = 17;
const Q30_POS: usize = 24;

pub(crate) struct CoefController {
    mcu_ctr: u32,
    mcu_vert_offset: i32,
    mcu_rows_per_imcu_row: i32,
    whole_image: Option<Vec<BlockArray>>,
    mode: DecompressMode,
    coef_bits_latch: Option<Vec<[i32; SAVED_COEFS]>>,
}

impl CoefController {
    pub fn new<S: Source>(cinfo: &Decompress<S>, need_full_buffer: bool) -> Self {
        let whole_image = need_full_buffer.then(|| {
            cinfo
                .comp_info
                .iter()
                .map(|compptr| {
                    let width = round_up(compptr.width_in_blocks as i64, compptr.h_samp_factor as i64) as usize;
                    let height = round_up(compptr.height_in_blocks as i64, compptr.v_samp_factor as i64) as usize;
                    BlockArray { blocks_per_row: width, data: vec![[0i16; DCTSIZE2]; width * height] }
                })
                .collect()
        });
        CoefController { mcu_ctr: 0, mcu_vert_offset: 0, mcu_rows_per_imcu_row: 0, mode: if need_full_buffer { DecompressMode::Multi } else { DecompressMode::OnePass }, whole_image, coef_bits_latch: None }
    }
}

impl<S: Source> Decompress<S> {
    fn coef(&mut self) -> &mut CoefController {
        match &mut self.coef {
            CoefKind::Lossy(coef) => coef,
            _ => unreachable!("lossy coefficient controller"),
        }
    }

    fn start_imcu_row(&mut self) {
        let rows = if self.comps_in_scan > 1 {
            1
        } else if self.input_imcu_row < self.total_imcu_rows - 1 {
            self.comp(0).v_samp_factor
        } else {
            self.comp(0).last_row_height
        };
        let coef = self.coef();
        coef.mcu_rows_per_imcu_row = rows;
        coef.mcu_ctr = 0;
        coef.mcu_vert_offset = 0;
    }

    pub(crate) fn coef_start_input_pass(&mut self) -> Result<()> {
        match self.coef {
            CoefKind::Lossy(_) => {
                self.input_imcu_row = 0;
                self.start_imcu_row();
                Ok(())
            }
            CoefKind::Lossless(_) => self.diff_start_input_pass(),
            CoefKind::None => Ok(()),
        }
    }

    pub(crate) fn coef_start_output_pass(&mut self) {
        if let CoefKind::Lossy(coef) = &self.coef
            && coef.whole_image.is_some()
        {
            let smooth = self.do_block_smoothing && self.smoothing_ok();
            self.coef().mode = if smooth { DecompressMode::Smooth } else { DecompressMode::Multi };
        }
        self.output_imcu_row = 0;
    }

    pub(crate) fn decode_mcu(&mut self) -> Result<bool> {
        match self.entropy.kind {
            EntropyKind::Sequential { .. } => self.decode_mcu_sequential(),
            EntropyKind::Progressive { .. } => self.decode_mcu_progressive(),
            _ => unreachable!("lossy entropy decoder"),
        }
    }

    pub(crate) fn consume_data(&mut self) -> Result<ConsumeStatus> {
        match &self.coef {
            CoefKind::Lossy(coef) if coef.whole_image.is_some() => self.consume_coefficients(),
            CoefKind::Lossless(diff) if diff.has_whole_image() => self.consume_lossless(),
            _ => Ok(ConsumeStatus::Suspended),
        }
    }

    pub(crate) fn decompress_data(&mut self, lists: Lists) -> Result<bool> {
        let status = match &self.coef {
            CoefKind::Lossy(coef) => match coef.mode {
                DecompressMode::OnePass => self.decompress_onepass(lists)?,
                DecompressMode::Multi => self.decompress_multi(lists)?,
                DecompressMode::Smooth => self.decompress_smooth(lists)?,
            },
            CoefKind::Lossless(_) => self.decompress_lossless(lists)?,
            CoefKind::None => unreachable!("coefficient controller"),
        };
        Ok(status != ConsumeStatus::Suspended)
    }

    fn finish_imcu_input_row(&mut self) -> ConsumeStatus {
        self.input_imcu_row += 1;
        if self.input_imcu_row < self.total_imcu_rows {
            self.start_imcu_row();
            return ConsumeStatus::RowCompleted;
        }
        self.finish_input_pass();
        ConsumeStatus::ScanCompleted
    }

    fn decompress_onepass(&mut self, lists: Lists) -> Result<ConsumeStatus> {
        let last_mcu_col = self.mcus_per_row - 1;
        let last_imcu_row = self.total_imcu_rows - 1;
        let start_offset = self.coef().mcu_vert_offset;
        let rows_per = self.coef().mcu_rows_per_imcu_row;
        for yoffset in start_offset..rows_per {
            let start_col = self.coef().mcu_ctr;
            for mcu_col_num in start_col..=last_mcu_col {
                for block in self.mcu_blocks.iter_mut().take(self.blocks_in_mcu as usize) {
                    *block = [0; DCTSIZE2];
                }
                if !self.entropy.insufficient_data {
                    self.master.last_good_imcu_row = self.input_imcu_row;
                }
                if !self.decode_mcu()? {
                    let coef = self.coef();
                    coef.mcu_vert_offset = yoffset;
                    coef.mcu_ctr = mcu_col_num;
                    return Ok(ConsumeStatus::Suspended);
                }
                if mcu_col_num >= self.master.first_imcu_col && mcu_col_num <= self.master.last_imcu_col {
                    let mut blkn = 0usize;
                    for ci in 0..self.comps_in_scan as usize {
                        let compptr = self.comp(ci).clone();
                        if !compptr.component_needed {
                            blkn += compptr.mcu_blocks as usize;
                            continue;
                        }
                        let index = compptr.component_index as usize;
                        let useful_width = if mcu_col_num < last_mcu_col { compptr.mcu_width } else { compptr.last_col_width };
                        let mut base = (yoffset * compptr.dct_scaled_size) as isize;
                        let start = ((mcu_col_num - self.master.first_imcu_col) * compptr.mcu_sample_width as u32) as usize;
                        for yindex in 0..compptr.mcu_height {
                            if self.input_imcu_row < last_imcu_row || yoffset + yindex < compptr.last_row_height {
                                let main = self.main.as_mut().expect("main controller");
                                let rows = main.rows(lists, index, base, 8);
                                let plane = &mut main.planes[index];
                                let mut output_col = start;
                                for xindex in 0..useful_width as usize {
                                    idct_islow(&compptr.dct_table, &self.mcu_blocks[blkn + xindex], &mut plane.data, plane.stride, &rows, output_col);
                                    output_col += compptr.dct_scaled_size as usize;
                                }
                            }
                            blkn += compptr.mcu_width as usize;
                            base += compptr.dct_scaled_size as isize;
                        }
                    }
                }
            }
            self.coef().mcu_ctr = 0;
        }
        self.output_imcu_row += 1;
        Ok(self.finish_imcu_input_row())
    }

    fn consume_coefficients(&mut self) -> Result<ConsumeStatus> {
        let start_offset = self.coef().mcu_vert_offset;
        let rows_per = self.coef().mcu_rows_per_imcu_row;
        for yoffset in start_offset..rows_per {
            let start_col = self.coef().mcu_ctr;
            for mcu_col_num in start_col..self.mcus_per_row {
                let mut positions = [(0usize, 0usize); D_MAX_BLOCKS_IN_MCU as usize];
                let mut blkn = 0usize;
                {
                    let CoefKind::Lossy(coef) = &self.coef else { unreachable!("lossy coefficient controller") };
                    let arrays = coef.whole_image.as_ref().expect("coefficient arrays");
                    for ci in 0..self.comps_in_scan as usize {
                        let compptr = self.comp(ci);
                        let component = compptr.component_index as usize;
                        let start = (mcu_col_num * compptr.mcu_width as u32) as usize;
                        for yindex in 0..compptr.mcu_height {
                            let row = (self.input_imcu_row * compptr.v_samp_factor as u32) as usize + (yindex + yoffset) as usize;
                            for xindex in 0..compptr.mcu_width as usize {
                                positions[blkn] = (component, arrays[component].index(row, start + xindex));
                                blkn += 1;
                            }
                        }
                    }
                }
                {
                    let CoefKind::Lossy(coef) = &self.coef else { unreachable!("lossy coefficient controller") };
                    let arrays = coef.whole_image.as_ref().expect("coefficient arrays");
                    for (slot, &(component, index)) in positions.iter().enumerate().take(blkn) {
                        self.mcu_blocks[slot] = arrays[component].data[index];
                    }
                }
                if !self.entropy.insufficient_data {
                    self.master.last_good_imcu_row = self.input_imcu_row;
                }
                let decoded = self.decode_mcu()?;
                {
                    let CoefKind::Lossy(coef) = &mut self.coef else { unreachable!("lossy coefficient controller") };
                    let arrays = coef.whole_image.as_mut().expect("coefficient arrays");
                    for (slot, &(component, index)) in positions.iter().enumerate().take(blkn) {
                        arrays[component].data[index] = self.mcu_blocks[slot];
                    }
                }
                if !decoded {
                    let coef = self.coef();
                    coef.mcu_vert_offset = yoffset;
                    coef.mcu_ctr = mcu_col_num;
                    return Ok(ConsumeStatus::Suspended);
                }
            }
            self.coef().mcu_ctr = 0;
        }
        Ok(self.finish_imcu_input_row())
    }

    fn decompress_multi(&mut self, lists: Lists) -> Result<ConsumeStatus> {
        let last_imcu_row = self.total_imcu_rows - 1;
        while self.input_scan_number < self.output_scan_number || (self.input_scan_number == self.output_scan_number && self.input_imcu_row <= self.output_imcu_row) {
            if self.consume()? == ConsumeStatus::Suspended {
                return Ok(ConsumeStatus::Suspended);
            }
        }
        for ci in 0..self.num_components as usize {
            let compptr = &self.comp_info[ci];
            if !compptr.component_needed {
                continue;
            }
            let v = compptr.v_samp_factor as u32;
            let block_rows = if self.output_imcu_row < last_imcu_row {
                v
            } else {
                let rows = compptr.height_in_blocks % v;
                if rows == 0 { v } else { rows }
            };
            let CoefKind::Lossy(coef) = &self.coef else { unreachable!("lossy coefficient controller") };
            let array = &coef.whole_image.as_ref().expect("coefficient arrays")[ci];
            let main = self.main.as_mut().expect("main controller");
            for block_row in 0..block_rows {
                let rows = main.rows(lists, ci, (block_row as i32 * compptr.dct_scaled_size) as isize, 8);
                let plane = &mut main.planes[ci];
                let row = (self.output_imcu_row * v + block_row) as usize;
                let mut output_col = 0usize;
                for block_num in self.master.first_mcu_col[ci]..=self.master.last_mcu_col[ci] {
                    idct_islow(&compptr.dct_table, &array.data[array.index(row, block_num as usize)], &mut plane.data, plane.stride, &rows, output_col);
                    output_col += compptr.dct_scaled_size as usize;
                }
            }
        }
        self.output_imcu_row += 1;
        Ok(if self.output_imcu_row < self.total_imcu_rows { ConsumeStatus::RowCompleted } else { ConsumeStatus::ScanCompleted })
    }

    fn smoothing_ok(&mut self) -> bool {
        if !self.progressive_mode || self.coef_bits.is_none() {
            return false;
        }
        let n = self.num_components as usize;
        let mut latch = vec![[0i32; SAVED_COEFS]; 2 * n];
        let mut smoothing_useful = false;
        for ci in 0..n {
            let Some(qtable) = self.comp_info[ci].quant_table else { return false };
            if [0, Q01_POS, Q10_POS, Q20_POS, Q11_POS, Q02_POS, Q03_POS, Q12_POS, Q21_POS, Q30_POS].iter().any(|&pos| qtable[pos] == 0) {
                return false;
            }
            let bits = self.coef_bits.as_ref().expect("progression status");
            if bits[ci][0] < 0 {
                return false;
            }
            latch[ci][0] = bits[ci][0];
            for coefi in 1..SAVED_COEFS {
                latch[ci + n][coefi] = if self.input_scan_number > 1 { bits[ci + n][coefi] } else { -1 };
                latch[ci][coefi] = bits[ci][coefi];
                if bits[ci][coefi] != 0 {
                    smoothing_useful = true;
                }
            }
        }
        self.coef().coef_bits_latch = Some(latch);
        smoothing_useful
    }

    fn decompress_smooth(&mut self, lists: Lists) -> Result<ConsumeStatus> {
        let last_imcu_row = self.total_imcu_rows - 1;
        while self.input_scan_number <= self.output_scan_number && !self.inputctl.eoi_reached {
            if self.input_scan_number == self.output_scan_number {
                let delta = if self.ss == 0 { 2 } else { 0 };
                if self.input_imcu_row > self.output_imcu_row + delta {
                    break;
                }
            }
            if self.consume()? == ConsumeStatus::Suspended {
                return Ok(ConsumeStatus::Suspended);
            }
        }
        let n = self.num_components as usize;
        for ci in 0..n {
            let compptr = self.comp_info[ci].clone();
            if !compptr.component_needed {
                continue;
            }
            let v = compptr.v_samp_factor as u32;
            let block_rows = if self.output_imcu_row + 1 < last_imcu_row || self.output_imcu_row < last_imcu_row {
                v
            } else {
                let rows = compptr.height_in_blocks % v;
                if rows == 0 { v } else { rows }
            };
            let CoefKind::Lossy(coef) = &self.coef else { unreachable!("lossy coefficient controller") };
            let latch = coef.coef_bits_latch.as_ref().expect("latched progression");
            let coef_bits = if self.output_imcu_row > self.master.last_good_imcu_row { latch[ci + n] } else { latch[ci] };
            let change_dc = coef_bits[1..=9].iter().all(|&bits| bits == -1);
            let quanttbl = compptr.quant_table.expect("latched quantization table");
            let q = |pos: usize| quanttbl[pos] as i64;
            let (q00, q01, q10, q20, q11, q02) = (q(0), q(Q01_POS), q(Q10_POS), q(Q20_POS), q(Q11_POS), q(Q02_POS));
            let (q03, q12, q21, q30) = if change_dc { (q(Q03_POS), q(Q12_POS), q(Q21_POS), q(Q30_POS)) } else { (0, 0, 0, 0) };
            let array = &coef.whole_image.as_ref().expect("coefficient arrays")[ci];
            let image_block_rows = block_rows as i64 * self.total_imcu_rows as i64;
            let first = self.master.first_mcu_col[ci] as usize;
            let last_col = self.master.last_mcu_col[ci] as usize;
            let main = self.main.as_mut().expect("main controller");
            for block_row in 0..block_rows as usize {
                let image_block_row = self.output_imcu_row as i64 * block_rows as i64 + block_row as i64;
                let current = (self.output_imcu_row * v) as usize + block_row;
                let prev = if image_block_row > 0 { current - 1 } else { current };
                let prev_prev = if image_block_row > 1 { current - 2 } else { prev };
                let next = if image_block_row < image_block_rows - 1 { current + 1 } else { current };
                let next_next = if image_block_row < image_block_rows - 2 { current + 2 } else { next };
                let dc = |row: usize, col: usize| array.data[array.index(row, col)][0] as i32;
                let mut dc01 = dc(prev_prev, first);
                let (mut dc02, mut dc03, mut dc04, mut dc05) = (dc01, dc01, dc01, dc01);
                let mut dc06 = dc(prev, first);
                let (mut dc07, mut dc08, mut dc09, mut dc10) = (dc06, dc06, dc06, dc06);
                let mut dc11 = dc(current, first);
                let (mut dc12, mut dc13, mut dc14, mut dc15) = (dc11, dc11, dc11, dc11);
                let mut dc16 = dc(next, first);
                let (mut dc17, mut dc18, mut dc19, mut dc20) = (dc16, dc16, dc16, dc16);
                let mut dc21 = dc(next_next, first);
                let (mut dc22, mut dc23, mut dc24, mut dc25) = (dc21, dc21, dc21, dc21);
                let rows = main.rows(lists, ci, (block_row as i32 * compptr.dct_scaled_size) as isize, 8);
                let plane = &mut main.planes[ci];
                let last_block_column = compptr.width_in_blocks as usize - 1;
                let mut output_col = 0usize;
                for block_num in first..=last_col {
                    let mut workspace = array.data[array.index(current, block_num)];
                    if block_num == first && block_num < last_block_column {
                        dc04 = dc(prev_prev, block_num + 1);
                        dc05 = dc04;
                        dc09 = dc(prev, block_num + 1);
                        dc10 = dc09;
                        dc14 = dc(current, block_num + 1);
                        dc15 = dc14;
                        dc19 = dc(next, block_num + 1);
                        dc20 = dc19;
                        dc24 = dc(next_next, block_num + 1);
                        dc25 = dc24;
                    }
                    if block_num + 1 < last_block_column {
                        dc05 = dc(prev_prev, block_num + 2);
                        dc10 = dc(prev, block_num + 2);
                        dc15 = dc(current, block_num + 2);
                        dc20 = dc(next, block_num + 2);
                        dc25 = dc(next_next, block_num + 2);
                    }
                    let estimate = |num: i64, qac: i64, al: i32| -> i16 {
                        let pred = if num >= 0 {
                            let mut pred = ((qac << 7) + num) / (qac << 8);
                            if al > 0 && pred >= 1 << al {
                                pred = (1 << al) - 1;
                            }
                            pred
                        } else {
                            let mut pred = ((qac << 7) - num) / (qac << 8);
                            if al > 0 && pred >= 1 << al {
                                pred = (1 << al) - 1;
                            }
                            -pred
                        };
                        pred as i32 as i16
                    };
                    let (d01, d02, d03, d04, d05) = (dc01 as i64, dc02 as i64, dc03 as i64, dc04 as i64, dc05 as i64);
                    let (d06, d07, d08, d09, d10) = (dc06 as i64, dc07 as i64, dc08 as i64, dc09 as i64, dc10 as i64);
                    let (d11, d12, d13, d14, d15) = (dc11 as i64, dc12 as i64, dc13 as i64, dc14 as i64, dc15 as i64);
                    let (d16, d17, d18, d19, d20) = (dc16 as i64, dc17 as i64, dc18 as i64, dc19 as i64, dc20 as i64);
                    let (d21, d22, d23, d24, d25) = (dc21 as i64, dc22 as i64, dc23 as i64, dc24 as i64, dc25 as i64);
                    let al = coef_bits[1];
                    if al != 0 && workspace[1] == 0 {
                        let num = q00 * if change_dc {
                            -d01 - d02 + d04 + d05 - 3 * d06 + 13 * d07 - 13 * d09 + 3 * d10 - 3 * d11 + 38 * d12 - 38 * d14 + 3 * d15 - 3 * d16 + 13 * d17 - 13 * d19 + 3 * d20 - d21 - d22 + d24 + d25
                        } else {
                            -7 * d11 + 50 * d12 - 50 * d14 + 7 * d15
                        };
                        workspace[1] = estimate(num, q01, al);
                    }
                    let al = coef_bits[2];
                    if al != 0 && workspace[8] == 0 {
                        let num = q00 * if change_dc {
                            -d01 - 3 * d02 - 3 * d03 - 3 * d04 - d05 - d06 + 13 * d07 + 38 * d08 + 13 * d09 - d10 + d16 - 13 * d17 - 38 * d18 - 13 * d19 + d20 + d21 + 3 * d22 + 3 * d23 + 3 * d24 + d25
                        } else {
                            -7 * d03 + 50 * d08 - 50 * d18 + 7 * d23
                        };
                        workspace[8] = estimate(num, q10, al);
                    }
                    let al = coef_bits[3];
                    if al != 0 && workspace[16] == 0 {
                        let num = q00 * if change_dc {
                            d03 + 2 * d07 + 7 * d08 + 2 * d09 - 5 * d12 - 14 * d13 - 5 * d14 + 2 * d17 + 7 * d18 + 2 * d19 + d23
                        } else {
                            -d03 + 13 * d08 - 24 * d13 + 13 * d18 - d23
                        };
                        workspace[16] = estimate(num, q20, al);
                    }
                    let al = coef_bits[4];
                    if al != 0 && workspace[9] == 0 {
                        let num = q00 * if change_dc {
                            -d01 + d05 + 9 * d07 - 9 * d09 - 9 * d17 + 9 * d19 + d21 - d25
                        } else {
                            d10 + d16 - 10 * d17 + 10 * d19 - d02 - d20 + d22 - d24 + d04 - d06 + 10 * d07 - 10 * d09
                        };
                        workspace[9] = estimate(num, q11, al);
                    }
                    let al = coef_bits[5];
                    if al != 0 && workspace[2] == 0 {
                        let num = q00 * if change_dc {
                            2 * d07 - 5 * d08 + 2 * d09 + d11 + 7 * d12 - 14 * d13 + 7 * d14 + d15 + 2 * d17 - 5 * d18 + 2 * d19
                        } else {
                            -d11 + 13 * d12 - 24 * d13 + 13 * d14 - d15
                        };
                        workspace[2] = estimate(num, q02, al);
                    }
                    if change_dc {
                        let al = coef_bits[6];
                        if al != 0 && workspace[3] == 0 {
                            workspace[3] = estimate(q00 * (d07 - d09 + 2 * d12 - 2 * d14 + d17 - d19), q03, al);
                        }
                        let al = coef_bits[7];
                        if al != 0 && workspace[10] == 0 {
                            workspace[10] = estimate(q00 * (d07 - 3 * d08 + d09 - d17 + 3 * d18 - d19), q12, al);
                        }
                        let al = coef_bits[8];
                        if al != 0 && workspace[17] == 0 {
                            workspace[17] = estimate(q00 * (d07 - d09 - 3 * d12 + 3 * d14 + d17 - d19), q21, al);
                        }
                        let al = coef_bits[9];
                        if al != 0 && workspace[24] == 0 {
                            workspace[24] = estimate(q00 * (d07 + 2 * d08 + d09 - d17 - 2 * d18 - d19), q30, al);
                        }
                        let num = q00
                            * (-2 * d01 - 6 * d02 - 8 * d03 - 6 * d04 - 2 * d05 - 6 * d06 + 6 * d07 + 42 * d08 + 6 * d09 - 6 * d10 - 8 * d11 + 42 * d12 + 152 * d13 + 42 * d14 - 8 * d15 - 6 * d16 + 6 * d17 + 42 * d18 + 6 * d19 - 6 * d20 - 2 * d21 - 6 * d22 - 8 * d23 - 6 * d24 - 2 * d25);
                        let pred = if num >= 0 { ((q00 << 7) + num) / (q00 << 8) } else { -(((q00 << 7) - num) / (q00 << 8)) };
                        workspace[0] = pred as i32 as i16;
                    }
                    idct_islow(&compptr.dct_table, &workspace, &mut plane.data, plane.stride, &rows, output_col);
                    dc01 = dc02;
                    dc02 = dc03;
                    dc03 = dc04;
                    dc04 = dc05;
                    dc06 = dc07;
                    dc07 = dc08;
                    dc08 = dc09;
                    dc09 = dc10;
                    dc11 = dc12;
                    dc12 = dc13;
                    dc13 = dc14;
                    dc14 = dc15;
                    dc16 = dc17;
                    dc17 = dc18;
                    dc18 = dc19;
                    dc19 = dc20;
                    dc21 = dc22;
                    dc22 = dc23;
                    dc23 = dc24;
                    dc24 = dc25;
                    output_col += compptr.dct_scaled_size as usize;
                }
            }
        }
        self.output_imcu_row += 1;
        Ok(if self.output_imcu_row < self.total_imcu_rows { ConsumeStatus::RowCompleted } else { ConsumeStatus::ScanCompleted })
    }
}

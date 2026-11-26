use super::decompress::{CoefKind, ConsumeStatus, Decompress, EntropyKind, IdctKind, Result};
use super::error::Error;
use super::huffman::{huff_extend, make_derived_table};
use super::main_buffer::Lists;
use super::source::Source;
use super::tables::*;

pub(crate) struct LosslessDecompressor {
    predictors: Vec<i32>,
    upscale: bool,
}

impl LosslessDecompressor {
    pub fn new(components: usize) -> Self {
        LosslessDecompressor { predictors: vec![0; components], upscale: false }
    }
}

struct SampleArray {
    width: usize,
    rows: usize,
    data: Vec<u16>,
    first_undef_row: usize,
}

pub(crate) struct DiffController {
    mcu_ctr: u32,
    restart_rows_to_go: u32,
    mcu_vert_offset: u32,
    mcu_rows_per_imcu_row: u32,
    diff_buf: Vec<Vec<Vec<i32>>>,
    undiff_buf: Vec<Vec<Vec<i32>>>,
    whole_image: Option<Vec<SampleArray>>,
}

impl DiffController {
    pub fn new<S: Source>(cinfo: &Decompress<S>, need_full_buffer: bool) -> Self {
        let buffers = || {
            cinfo.comp_info.iter().map(|compptr| vec![vec![0i32; round_up(compptr.width_in_blocks as i64, compptr.h_samp_factor as i64) as usize]; compptr.v_samp_factor as usize]).collect::<Vec<_>>()
        };
        let whole_image = need_full_buffer.then(|| {
            cinfo
                .comp_info
                .iter()
                .map(|compptr| {
                    let width = round_up(compptr.width_in_blocks as i64, compptr.h_samp_factor as i64) as usize;
                    let rows = round_up(compptr.height_in_blocks as i64, compptr.v_samp_factor as i64) as usize;
                    SampleArray { width, rows, data: vec![0; width * rows], first_undef_row: 0 }
                })
                .collect()
        });
        DiffController { mcu_ctr: 0, restart_rows_to_go: 0, mcu_vert_offset: 0, mcu_rows_per_imcu_row: 0, diff_buf: buffers(), undiff_buf: buffers(), whole_image }
    }

    pub fn has_whole_image(&self) -> bool {
        self.whole_image.is_some()
    }
}

fn undifference(predictor: i32, initial: i32, diff: &[i32], prev: &[i32], undiff: &mut [i32], width: usize) {
    if predictor == 0 || predictor == 1 {
        let first = if predictor == 0 { initial } else { prev[0] };
        let mut ra = (diff[0].wrapping_add(first)) & 0xFFFF;
        undiff[0] = ra;
        for i in 1..width {
            ra = (diff[i].wrapping_add(ra)) & 0xFFFF;
            undiff[i] = ra;
        }
        return;
    }
    let mut rb = prev[0];
    let mut ra = (diff[0].wrapping_add(rb)) & 0xFFFF;
    undiff[0] = ra;
    for i in 1..width {
        let rc = rb;
        rb = prev[i];
        let (a, b, c) = (ra as i64, rb as i64, rc as i64);
        let prediction = match predictor {
            2 => rb,
            3 => rc,
            4 => (a + b - c) as i32,
            5 => (a + ((b - c) >> 1)) as i32,
            6 => (b + ((a - c) >> 1)) as i32,
            _ => ((a + b) >> 1) as i32,
        };
        ra = (diff[i].wrapping_add(prediction)) & 0xFFFF;
        undiff[i] = ra;
    }
}

impl<S: Source> Decompress<S> {
    fn diff(&mut self) -> &mut DiffController {
        match &mut self.coef {
            CoefKind::Lossless(diff) => diff,
            _ => unreachable!("lossless controller"),
        }
    }

    pub(crate) fn start_pass_lossless(&mut self) -> Result<()> {
        if self.ss < 1 || self.ss > 7 || self.se != 0 || self.ah != 0 || self.al < 0 || self.al >= self.data_precision {
            return Err(Error::BadProgression);
        }
        let al = self.al;
        let IdctKind::Lossless(lossless) = &mut self.idct else { unreachable!("lossless decompressor") };
        for predictor in lossless.predictors.iter_mut() {
            *predictor = 0;
        }
        lossless.upscale = al != 0;
        Ok(())
    }

    pub(crate) fn init_lossless_huffman(&mut self) {
        self.entropy.kind = EntropyKind::Lossless { derived: [None, None, None, None], cur: [0; 10], output_ptr_info: [(0, 0, 0); 10], output_ptr_index: [0; 10], num_output_ptrs: 0 };
    }

    pub(crate) fn start_pass_lhuff_decoder(&mut self) -> Result<()> {
        for ci in 0..self.comps_in_scan as usize {
            let dctbl = self.comp(ci).dc_tbl_no;
            if !(0..NUM_HUFF_TBLS as i32).contains(&dctbl) || self.dc_huff_tbl_ptrs[dctbl as usize].is_none() {
                return Err(Error::NoHuffmanTable(dctbl));
            }
            let table = make_derived_table(self, true, dctbl)?;
            if let EntropyKind::Lossless { derived, .. } = &mut self.entropy.kind {
                derived[dctbl as usize] = Some(table);
            }
        }
        let mut sampn = 0usize;
        let mut ptrn = 0usize;
        let mut info = [(0usize, 0usize, 0usize); 10];
        let mut index = [0usize; 10];
        let mut cur = [0usize; 10];
        while sampn < self.blocks_in_mcu as usize {
            let compptr = self.comp(self.mcu_membership[sampn] as usize);
            let ci = compptr.component_index as usize;
            for yoffset in 0..compptr.mcu_height as usize {
                info[ptrn] = (ci, yoffset, compptr.mcu_width as usize);
                for _ in 0..compptr.mcu_width {
                    index[sampn] = ptrn;
                    cur[sampn] = compptr.dc_tbl_no as usize;
                    sampn += 1;
                }
                ptrn += 1;
            }
        }
        if let EntropyKind::Lossless { cur: c, output_ptr_info, output_ptr_index, num_output_ptrs, .. } = &mut self.entropy.kind {
            *c = cur;
            *output_ptr_info = info;
            *output_ptr_index = index;
            *num_output_ptrs = ptrn;
        }
        self.entropy.bits_left = 0;
        self.entropy.get_buffer = 0;
        self.entropy.insufficient_data = false;
        Ok(())
    }

    fn process_restart_lossless(&mut self) -> Result<bool> {
        self.marker.discarded_bytes = self.marker.discarded_bytes.wrapping_add((self.entropy.bits_left / 8) as u32);
        self.entropy.bits_left = 0;
        if !self.read_restart_marker()? {
            return Ok(false);
        }
        if self.unread_marker == 0 {
            self.entropy.insufficient_data = false;
        }
        Ok(true)
    }

    fn decode_mcus(&mut self, mcu_row_num: usize, mcu_col_num: usize, n: usize) -> Result<usize> {
        let EntropyKind::Lossless { derived, cur, output_ptr_info, output_ptr_index, num_output_ptrs } = &self.entropy.kind else { unreachable!("lossless decoder") };
        let (derived, cur, info, index, num) = (derived.clone(), *cur, *output_ptr_info, *output_ptr_index, *num_output_ptrs);
        let mut positions: Vec<usize> = info[..num].iter().map(|&(_, _, width)| mcu_col_num * width).collect();
        if self.entropy.insufficient_data {
            let diff = self.diff();
            for (ptrn, &(ci, yoffset, width)) in info[..num].iter().enumerate() {
                let row = &mut diff.diff_buf[ci][mcu_row_num + yoffset];
                for value in &mut row[positions[ptrn]..positions[ptrn] + n * width] {
                    *value = 0;
                }
            }
            self.start_pass_lossless()?;
            return Ok(n);
        }
        let mut br = self.load_bit_reader();
        for mcu_num in 0..n {
            for sampn in 0..self.blocks_in_mcu as usize {
                let table = derived[cur[sampn]].clone().expect("derived table");
                let Some(mut s) = self.huff_decode(&mut br, &table) else { return Ok(mcu_num) };
                if s != 0 {
                    if s == 16 {
                        s = 32768;
                    } else {
                        if !self.check_bit_buffer(&mut br, s) {
                            return Ok(mcu_num);
                        }
                        let r = br.get_bits(s);
                        s = huff_extend(r, s);
                    }
                }
                let ptrn = index[sampn];
                let (ci, yoffset, _) = info[ptrn];
                let position = positions[ptrn];
                self.diff().diff_buf[ci][mcu_row_num + yoffset][position] = s;
                positions[ptrn] += 1;
            }
            self.save_bit_reader(&br);
        }
        Ok(n)
    }

    fn start_lossless_imcu_row(&mut self) {
        let rows = if self.comps_in_scan > 1 {
            1
        } else if self.input_imcu_row < self.total_imcu_rows - 1 {
            self.comp(0).v_samp_factor as u32
        } else {
            self.comp(0).last_row_height as u32
        };
        let diff = self.diff();
        diff.mcu_rows_per_imcu_row = rows;
        diff.mcu_ctr = 0;
        diff.mcu_vert_offset = 0;
    }

    pub(crate) fn diff_start_input_pass(&mut self) -> Result<()> {
        self.start_pass_lossless()?;
        if !self.restart_interval.is_multiple_of(self.mcus_per_row) {
            return Err(Error::BadRestart);
        }
        let rows = self.restart_interval / self.mcus_per_row;
        self.diff().restart_rows_to_go = rows;
        self.input_imcu_row = 0;
        self.start_lossless_imcu_row();
        Ok(())
    }

    fn lossless_rows(&mut self) -> Result<ConsumeStatus> {
        let start = self.diff().mcu_vert_offset;
        let rows_per = self.diff().mcu_rows_per_imcu_row;
        for yoffset in start..rows_per {
            if self.restart_interval != 0 && self.diff().restart_rows_to_go == 0 {
                if !self.process_restart_lossless()? {
                    return Ok(ConsumeStatus::Suspended);
                }
                self.start_pass_lossless()?;
                let rows = self.restart_interval / self.mcus_per_row;
                self.diff().restart_rows_to_go = rows;
            }
            let mcu_col_num = self.diff().mcu_ctr as usize;
            let wanted = self.mcus_per_row as usize - mcu_col_num;
            let count = self.decode_mcus(yoffset as usize, mcu_col_num, wanted)?;
            if count != wanted {
                let diff = self.diff();
                diff.mcu_vert_offset = yoffset;
                diff.mcu_ctr += count as u32;
                return Ok(ConsumeStatus::Suspended);
            }
            if self.restart_interval != 0 {
                let diff = self.diff();
                diff.restart_rows_to_go = diff.restart_rows_to_go.wrapping_sub(1);
            }
            self.diff().mcu_ctr = 0;
        }
        Ok(ConsumeStatus::RowCompleted)
    }

    fn undifference_scan_rows(&mut self, mut store: impl FnMut(&mut Self, usize, usize, &[i32])) {
        let last_imcu_row = self.total_imcu_rows - 1;
        let initial = 1i32 << (self.data_precision - self.al - 1);
        let al = self.al;
        for ci in 0..self.comps_in_scan as usize {
            let compptr = self.comp(ci).clone();
            let compi = compptr.component_index as usize;
            let rows = if self.input_imcu_row == last_imcu_row { compptr.last_row_height } else { compptr.v_samp_factor } as usize;
            let v = compptr.v_samp_factor as usize;
            let width = compptr.width_in_blocks as usize;
            for row in 0..rows {
                let prev_row = if row == 0 { v - 1 } else { row - 1 };
                let IdctKind::Lossless(lossless) = &mut self.idct else { unreachable!("lossless decompressor") };
                let predictor = lossless.predictors[compi];
                let upscale = lossless.upscale;
                let ss = self.ss;
                let CoefKind::Lossless(diff) = &mut self.coef else { unreachable!("lossless controller") };
                let prev = diff.undiff_buf[compi][prev_row].clone();
                let mut current = std::mem::take(&mut diff.undiff_buf[compi][row]);
                undifference(predictor, initial, &diff.diff_buf[compi][row], &prev, &mut current, width);
                let samples: Vec<i32> = current[..width].iter().map(|&value| if upscale { value << al } else { value }).collect();
                diff.undiff_buf[compi][row] = current;
                if predictor == 0 {
                    let IdctKind::Lossless(lossless) = &mut self.idct else { unreachable!("lossless decompressor") };
                    lossless.predictors[compi] = ss;
                }
                store(self, compi, row, &samples);
            }
        }
    }

    pub(crate) fn decompress_lossless(&mut self, lists: Lists) -> Result<ConsumeStatus> {
        if self.diff().has_whole_image() {
            return self.output_lossless(lists);
        }
        if self.lossless_rows()? == ConsumeStatus::Suspended {
            return Ok(ConsumeStatus::Suspended);
        }
        self.undifference_scan_rows(|cinfo, compi, row, samples| {
            let main = cinfo.main.as_mut().expect("main controller");
            let index = main.row_index(lists, compi, row as isize);
            let plane = &mut main.planes[compi];
            let start = index * plane.stride;
            for (target, &value) in plane.data[start..start + samples.len()].iter_mut().zip(samples) {
                *target = value as u8;
            }
        });
        Ok(self.finish_lossless_input_row())
    }

    fn finish_lossless_input_row(&mut self) -> ConsumeStatus {
        self.input_imcu_row += 1;
        if self.input_imcu_row < self.total_imcu_rows {
            self.start_lossless_imcu_row();
            return ConsumeStatus::RowCompleted;
        }
        self.finish_input_pass();
        ConsumeStatus::ScanCompleted
    }

    pub(crate) fn consume_lossless(&mut self) -> Result<ConsumeStatus> {
        for ci in 0..self.comps_in_scan as usize {
            let compptr = self.comp(ci).clone();
            let start = (self.input_imcu_row * compptr.v_samp_factor as u32) as usize;
            let end = start + compptr.v_samp_factor as usize;
            let array = &mut self.diff().whole_image.as_mut().expect("sample arrays")[compptr.component_index as usize];
            if end > array.rows {
                return Err(Error::BadVirtualAccess);
            }
            if array.first_undef_row < end {
                if array.first_undef_row < start {
                    return Err(Error::BadVirtualAccess);
                }
                array.first_undef_row = end;
            }
        }
        if self.lossless_rows()? == ConsumeStatus::Suspended {
            return Ok(ConsumeStatus::Suspended);
        }
        let imcu_row = self.input_imcu_row;
        self.undifference_scan_rows(|cinfo, compi, row, samples| {
            let v = cinfo.comp_info[compi].v_samp_factor as u32;
            let array = &mut cinfo.diff().whole_image.as_mut().expect("sample arrays")[compi];
            let start = ((imcu_row * v) as usize + row) * array.width;
            for (target, &value) in array.data[start..start + samples.len()].iter_mut().zip(samples) {
                *target = value as u16;
            }
        });
        Ok(self.finish_lossless_input_row())
    }

    fn output_lossless(&mut self, lists: Lists) -> Result<ConsumeStatus> {
        let last_imcu_row = self.total_imcu_rows - 1;
        while self.input_scan_number < self.output_scan_number || (self.input_scan_number == self.output_scan_number && self.input_imcu_row <= self.output_imcu_row) {
            if self.consume()? == ConsumeStatus::Suspended {
                return Ok(ConsumeStatus::Suspended);
            }
        }
        for ci in 0..self.num_components as usize {
            let compptr = self.comp_info[ci].clone();
            let v = compptr.v_samp_factor as u32;
            let start = (self.output_imcu_row * v) as usize;
            let end = start + v as usize;
            let CoefKind::Lossless(diff) = &self.coef else { unreachable!("lossless controller") };
            let array = &diff.whole_image.as_ref().expect("sample arrays")[ci];
            if end > array.rows || array.first_undef_row < end {
                return Err(Error::BadVirtualAccess);
            }
            let samp_rows = if self.output_imcu_row < last_imcu_row {
                v
            } else {
                let rows = compptr.height_in_blocks % v;
                if rows == 0 { v } else { rows }
            };
            let main = self.main.as_mut().expect("main controller");
            for row in 0..samp_rows as usize {
                let index = main.row_index(lists, ci, row as isize);
                let plane = &mut main.planes[ci];
                let target = index * plane.stride;
                let source = (start + row) * array.width;
                let width = compptr.width_in_blocks as usize;
                for (out, &value) in plane.data[target..target + width].iter_mut().zip(&array.data[source..source + width]) {
                    *out = value as u8;
                }
            }
        }
        self.output_imcu_row += 1;
        Ok(if self.output_imcu_row < self.total_imcu_rows { ConsumeStatus::RowCompleted } else { ConsumeStatus::ScanCompleted })
    }
}

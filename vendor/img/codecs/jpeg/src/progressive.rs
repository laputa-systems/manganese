use super::decompress::{Decompress, EntropyKind, ProgressiveMcu, Result};
use super::error::{Error, Warning};
use super::huffman::{huff_extend, make_derived_table, DerivedTable};
use super::source::Source;
use super::tables::*;
use std::rc::Rc;

impl<S: Source> Decompress<S> {
    pub(crate) fn init_progressive_huffman(&mut self) {
        self.entropy.kind = EntropyKind::Progressive { derived: [None, None, None, None], ac_tbl: 0, mcu: ProgressiveMcu::DcFirst, eobrun: 0 };
        let n = self.num_components as usize;
        let mut bits = vec![[0i32; DCTSIZE2]; 2 * n];
        for row in bits.iter_mut().take(n) {
            *row = [-1; DCTSIZE2];
        }
        self.coef_bits = Some(bits);
    }

    pub(crate) fn start_pass_phuff_decoder(&mut self) -> Result<()> {
        let is_dc_band = self.ss == 0;
        let mut bad = false;
        if is_dc_band {
            if self.se != 0 {
                bad = true;
            }
        } else {
            if self.ss > self.se || self.se >= DCTSIZE2 as i32 {
                bad = true;
            }
            if self.comps_in_scan != 1 {
                bad = true;
            }
        }
        if self.ah != 0 && self.al != self.ah - 1 {
            bad = true;
        }
        if self.al > 13 {
            bad = true;
        }
        if bad {
            return Err(Error::BadProgression);
        }
        let n = self.num_components as usize;
        for ci in 0..self.comps_in_scan as usize {
            let cindex = self.comp(ci).component_index as usize;
            let mut warnings = 0;
            {
                let bits = self.coef_bits.as_mut().expect("progression status");
                if !is_dc_band && bits[cindex][0] < 0 {
                    warnings += 1;
                }
                for coefi in self.ss.min(1)..=self.se.max(9) {
                    let coefi = coefi as usize;
                    bits[cindex + n][coefi] = if self.input_scan_number > 1 { bits[cindex][coefi] } else { 0 };
                }
                for coefi in self.ss..=self.se {
                    let coefi = coefi as usize;
                    let expected = if bits[cindex][coefi] < 0 { 0 } else { bits[cindex][coefi] };
                    if self.ah != expected {
                        warnings += 1;
                    }
                    bits[cindex][coefi] = self.al;
                }
            }
            for _ in 0..warnings {
                self.warn(Warning::BogusProgression);
            }
        }
        let mcu = match (self.ah == 0, is_dc_band) {
            (true, true) => ProgressiveMcu::DcFirst,
            (true, false) => ProgressiveMcu::AcFirst,
            (false, true) => ProgressiveMcu::DcRefine,
            (false, false) => ProgressiveMcu::AcRefine,
        };
        if let EntropyKind::Progressive { mcu: current, .. } = &mut self.entropy.kind {
            *current = mcu;
        }
        for ci in 0..self.comps_in_scan as usize {
            if is_dc_band {
                if self.ah == 0 {
                    let tbl = self.comp(ci).dc_tbl_no;
                    let table = make_derived_table(self, true, tbl)?;
                    if let EntropyKind::Progressive { derived, .. } = &mut self.entropy.kind {
                        derived[tbl as usize] = Some(table);
                    }
                }
            } else {
                let tbl = self.comp(ci).ac_tbl_no;
                let table = make_derived_table(self, false, tbl)?;
                if let EntropyKind::Progressive { derived, ac_tbl, .. } = &mut self.entropy.kind {
                    derived[tbl as usize] = Some(table);
                    *ac_tbl = tbl as usize;
                }
            }
            self.entropy.last_dc_val[ci] = 0;
        }
        self.entropy.bits_left = 0;
        self.entropy.get_buffer = 0;
        self.entropy.insufficient_data = false;
        if let EntropyKind::Progressive { eobrun, .. } = &mut self.entropy.kind {
            *eobrun = 0;
        }
        self.entropy.restarts_to_go = self.restart_interval;
        Ok(())
    }

    fn progressive_table(&self, index: usize) -> Rc<DerivedTable> {
        let EntropyKind::Progressive { derived, .. } = &self.entropy.kind else { unreachable!("progressive decoder") };
        derived[index].clone().expect("derived table")
    }

    fn eobrun(&self) -> u32 {
        let EntropyKind::Progressive { eobrun, .. } = &self.entropy.kind else { unreachable!("progressive decoder") };
        *eobrun
    }

    fn set_eobrun(&mut self, value: u32) {
        if let EntropyKind::Progressive { eobrun, .. } = &mut self.entropy.kind {
            *eobrun = value;
        }
    }

    pub(crate) fn decode_mcu_progressive(&mut self) -> Result<bool> {
        let EntropyKind::Progressive { mcu, .. } = &self.entropy.kind else { unreachable!("progressive decoder") };
        let mcu = *mcu;
        if self.restart_interval != 0 && self.entropy.restarts_to_go == 0 && !self.process_restart_entropy(true)? {
            return Ok(false);
        }
        let done = match mcu {
            ProgressiveMcu::DcFirst => self.decode_dc_first()?,
            ProgressiveMcu::AcFirst => self.decode_ac_first(),
            ProgressiveMcu::DcRefine => self.decode_dc_refine(),
            ProgressiveMcu::AcRefine => self.decode_ac_refine(),
        };
        if !done {
            return Ok(false);
        }
        if self.restart_interval != 0 {
            self.entropy.restarts_to_go = self.entropy.restarts_to_go.wrapping_sub(1);
        }
        Ok(true)
    }

    fn decode_dc_first(&mut self) -> Result<bool> {
        if self.entropy.insufficient_data {
            return Ok(true);
        }
        let al = self.al;
        let mut br = self.load_bit_reader();
        let mut last_dc_val = self.entropy.last_dc_val;
        for blkn in 0..self.blocks_in_mcu as usize {
            let ci = self.mcu_membership[blkn] as usize;
            let tbl = self.progressive_table(self.comp(ci).dc_tbl_no as usize);
            let Some(mut s) = self.huff_decode(&mut br, &tbl) else { return Ok(false) };
            if s != 0 {
                if !self.check_bit_buffer(&mut br, s) {
                    return Ok(false);
                }
                let r = br.get_bits(s);
                s = huff_extend(r, s);
            }
            let last = last_dc_val[ci];
            if (last >= 0 && s > i32::MAX - last) || (last < 0 && s < i32::MIN - last) {
                return Err(Error::BadDctCoefficient);
            }
            s += last;
            last_dc_val[ci] = s;
            self.mcu_blocks[blkn][0] = ((s as u64) << al) as i16;
        }
        self.save_bit_reader(&br);
        self.entropy.last_dc_val = last_dc_val;
        Ok(true)
    }

    fn decode_ac_first(&mut self) -> bool {
        if self.entropy.insufficient_data {
            return true;
        }
        let se = self.se;
        let al = self.al;
        let mut eobrun = self.eobrun();
        if eobrun > 0 {
            eobrun -= 1;
        } else {
            let mut br = self.load_bit_reader();
            let EntropyKind::Progressive { ac_tbl, .. } = &self.entropy.kind else { unreachable!("progressive decoder") };
            let tbl = self.progressive_table(*ac_tbl);
            let mut k = self.ss;
            while k <= se {
                let Some(value) = self.huff_decode(&mut br, &tbl) else { return false };
                let mut r = value >> 4;
                let mut s = value & 15;
                if s != 0 {
                    k += r;
                    if !self.check_bit_buffer(&mut br, s) {
                        return false;
                    }
                    r = br.get_bits(s);
                    s = huff_extend(r, s);
                    self.mcu_blocks[0][NATURAL_ORDER[k as usize]] = ((s as u64) << al) as i16;
                } else if r == 15 {
                    k += 15;
                } else {
                    eobrun = 1 << r;
                    if r != 0 {
                        if !self.check_bit_buffer(&mut br, r) {
                            return false;
                        }
                        r = br.get_bits(r);
                        eobrun += r as u32;
                    }
                    eobrun -= 1;
                    break;
                }
                k += 1;
            }
            self.save_bit_reader(&br);
        }
        self.set_eobrun(eobrun);
        true
    }

    fn decode_dc_refine(&mut self) -> bool {
        let p1 = 1i32 << self.al;
        let mut br = self.load_bit_reader();
        for blkn in 0..self.blocks_in_mcu as usize {
            if !self.check_bit_buffer(&mut br, 1) {
                return false;
            }
            if br.get_bits(1) != 0 {
                self.mcu_blocks[blkn][0] |= p1 as i16;
            }
        }
        self.save_bit_reader(&br);
        true
    }

    fn decode_ac_refine(&mut self) -> bool {
        if self.entropy.insufficient_data {
            return true;
        }
        let se = self.se;
        let p1 = 1i32 << self.al;
        let m1 = (-1i32) << self.al;
        let mut br = self.load_bit_reader();
        let mut eobrun = self.eobrun();
        let EntropyKind::Progressive { ac_tbl, .. } = &self.entropy.kind else { unreachable!("progressive decoder") };
        let tbl = self.progressive_table(*ac_tbl);
        let mut newnz_pos = [0usize; DCTSIZE2];
        let mut num_newnz = 0usize;
        let mut k = self.ss;
        macro_rules! undo {
            () => {{
                while num_newnz > 0 {
                    num_newnz -= 1;
                    self.mcu_blocks[0][newnz_pos[num_newnz]] = 0;
                }
                return false;
            }};
        }
        if eobrun == 0 {
            while k <= se {
                let Some(value) = self.huff_decode(&mut br, &tbl) else { undo!() };
                let mut r = value >> 4;
                let mut s = value & 15;
                if s != 0 {
                    if s != 1 {
                        self.warn(Warning::HuffmanBadCode);
                    }
                    if !self.check_bit_buffer(&mut br, 1) {
                        undo!();
                    }
                    s = if br.get_bits(1) != 0 { p1 } else { m1 };
                } else if r != 15 {
                    eobrun = 1 << r;
                    if r != 0 {
                        if !self.check_bit_buffer(&mut br, r) {
                            undo!();
                        }
                        r = br.get_bits(r);
                        eobrun += r as u32;
                    }
                    break;
                }
                loop {
                    let pos = NATURAL_ORDER[k as usize];
                    let thiscoef = self.mcu_blocks[0][pos];
                    if thiscoef != 0 {
                        if !self.check_bit_buffer(&mut br, 1) {
                            undo!();
                        }
                        if br.get_bits(1) != 0 && (thiscoef as i32 & p1) == 0 {
                            self.mcu_blocks[0][pos] = if thiscoef >= 0 { thiscoef.wrapping_add(p1 as i16) } else { thiscoef.wrapping_add(m1 as i16) };
                        }
                    } else {
                        r -= 1;
                        if r < 0 {
                            break;
                        }
                    }
                    k += 1;
                    if k > se {
                        break;
                    }
                }
                if s != 0 {
                    let pos = NATURAL_ORDER[k as usize];
                    self.mcu_blocks[0][pos] = s as i16;
                    newnz_pos[num_newnz] = pos;
                    num_newnz += 1;
                }
                k += 1;
            }
        }
        if eobrun > 0 {
            while k <= se {
                let pos = NATURAL_ORDER[k as usize];
                let thiscoef = self.mcu_blocks[0][pos];
                if thiscoef != 0 {
                    if !self.check_bit_buffer(&mut br, 1) {
                        undo!();
                    }
                    if br.get_bits(1) != 0 && (thiscoef as i32 & p1) == 0 {
                        self.mcu_blocks[0][pos] = if thiscoef >= 0 { thiscoef.wrapping_add(p1 as i16) } else { thiscoef.wrapping_add(m1 as i16) };
                    }
                }
                k += 1;
            }
            eobrun -= 1;
        }
        self.save_bit_reader(&br);
        self.set_eobrun(eobrun);
        true
    }
}

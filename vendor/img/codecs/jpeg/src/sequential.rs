use super::decompress::{Decompress, EntropyKind, HuffTable, Result};
use super::error::Warning;
use super::huffman::{huff_extend, make_derived_table, BitReader, DerivedTable, BIT_BUF_SIZE, HUFF_LOOKAHEAD};
use super::source::Source;
use super::tables::*;

const BUFSIZE: usize = DCTSIZE2 * 8;

fn std_table(bits: &[u8; 17], values: &[u8]) -> HuffTable {
    let mut huffval = [0u8; 256];
    huffval[..values.len()].copy_from_slice(values);
    HuffTable { bits: *bits, huffval }
}

struct FastReader<'a> {
    data: &'a [u8],
    buffer: usize,
    get_buffer: u64,
    bits_left: i32,
    unread_marker: i32,
}

impl FastReader<'_> {
    fn get_byte(&mut self) {
        let c0 = self.data.get(self.buffer).copied().unwrap_or(0) as u64;
        let c1 = self.data.get(self.buffer + 1).copied().unwrap_or(0) as i32;
        self.buffer += 1;
        self.get_buffer = (self.get_buffer << 8) | c0;
        self.bits_left += 8;
        if c0 == 0xFF {
            self.buffer += 1;
            if c1 != 0 {
                self.unread_marker = c1;
                self.buffer -= 2;
                self.get_buffer &= !0xFF;
            }
        }
    }

    fn fill(&mut self) {
        if self.bits_left <= 16 {
            let bytes = if BIT_BUF_SIZE == 64 { 6 } else { 2 };
            for _ in 0..bytes {
                self.get_byte();
            }
        }
    }

    fn get_bits(&mut self, nbits: i32) -> i32 {
        self.bits_left -= nbits;
        ((self.get_buffer >> self.bits_left) as i32) & ((1 << nbits) - 1)
    }

    fn decode(&mut self, table: &DerivedTable) -> i32 {
        self.fill();
        let look = ((self.get_buffer >> (self.bits_left - HUFF_LOOKAHEAD)) as i32) & ((1 << HUFF_LOOKAHEAD) - 1);
        let entry = table.lookup[look as usize];
        let mut nb = entry >> HUFF_LOOKAHEAD;
        self.bits_left -= nb;
        let mut s = entry & ((1 << HUFF_LOOKAHEAD) - 1);
        if nb > HUFF_LOOKAHEAD {
            s = ((self.get_buffer >> self.bits_left) & ((1u64 << nb) - 1)) as i32;
            while s as i64 > table.maxcode[nb as usize] {
                s <<= 1;
                s |= self.get_bits(1);
                nb += 1;
            }
            s = if nb > 16 { 0 } else { table.huffval[((s as i64 + table.valoffset[nb as usize]) & 0xFF) as usize] as i32 };
        }
        s
    }
}

impl<S: Source> Decompress<S> {
    pub(crate) fn init_sequential_huffman(&mut self) {
        if self.dc_huff_tbl_ptrs[0].is_none() {
            self.dc_huff_tbl_ptrs[0] = Some(std_table(&BITS_DC_LUMINANCE, &VAL_DC_LUMINANCE));
        }
        if self.ac_huff_tbl_ptrs[0].is_none() {
            self.ac_huff_tbl_ptrs[0] = Some(std_table(&BITS_AC_LUMINANCE, &VAL_AC_LUMINANCE));
        }
        if self.dc_huff_tbl_ptrs[1].is_none() {
            self.dc_huff_tbl_ptrs[1] = Some(std_table(&BITS_DC_CHROMINANCE, &VAL_DC_CHROMINANCE));
        }
        if self.ac_huff_tbl_ptrs[1].is_none() {
            self.ac_huff_tbl_ptrs[1] = Some(std_table(&BITS_AC_CHROMINANCE, &VAL_AC_CHROMINANCE));
        }
        self.entropy.kind = EntropyKind::Sequential { dc_derived: [None, None, None, None], ac_derived: [None, None, None, None], dc_cur: [0; 10], ac_cur: [0; 10], dc_needed: [false; 10], ac_needed: [false; 10] };
    }

    pub(crate) fn start_pass_huff_decoder(&mut self) -> Result<()> {
        if self.ss != 0 || self.se != DCTSIZE2 as i32 - 1 || self.ah != 0 || self.al != 0 {
            self.warn(Warning::NotSequential);
        }
        for ci in 0..self.comps_in_scan as usize {
            let dctbl = self.comp(ci).dc_tbl_no;
            let actbl = self.comp(ci).ac_tbl_no;
            let dc = make_derived_table(self, true, dctbl)?;
            if let EntropyKind::Sequential { dc_derived, .. } = &mut self.entropy.kind {
                dc_derived[dctbl as usize] = Some(dc);
            }
            let ac = make_derived_table(self, false, actbl)?;
            if let EntropyKind::Sequential { ac_derived, .. } = &mut self.entropy.kind {
                ac_derived[actbl as usize] = Some(ac);
            }
            self.entropy.last_dc_val[ci] = 0;
        }
        for blkn in 0..self.blocks_in_mcu as usize {
            let ci = self.mcu_membership[blkn] as usize;
            let compptr = self.comp(ci);
            let (dc_tbl, ac_tbl, needed, scaled) = (compptr.dc_tbl_no as usize, compptr.ac_tbl_no as usize, compptr.component_needed, compptr.dct_scaled_size);
            if let EntropyKind::Sequential { dc_cur, ac_cur, dc_needed, ac_needed, .. } = &mut self.entropy.kind {
                dc_cur[blkn] = dc_tbl;
                ac_cur[blkn] = ac_tbl;
                if needed {
                    dc_needed[blkn] = true;
                    ac_needed[blkn] = scaled > 1;
                } else {
                    dc_needed[blkn] = false;
                    ac_needed[blkn] = false;
                }
            }
        }
        self.entropy.bits_left = 0;
        self.entropy.get_buffer = 0;
        self.entropy.insufficient_data = false;
        self.entropy.restarts_to_go = self.restart_interval;
        Ok(())
    }

    pub(crate) fn process_restart_entropy(&mut self, reset_eobrun: bool) -> Result<bool> {
        self.marker.discarded_bytes = self.marker.discarded_bytes.wrapping_add((self.entropy.bits_left / 8) as u32);
        self.entropy.bits_left = 0;
        if !self.read_restart_marker()? {
            return Ok(false);
        }
        for ci in 0..self.comps_in_scan as usize {
            self.entropy.last_dc_val[ci] = 0;
        }
        if reset_eobrun && let EntropyKind::Progressive { eobrun, .. } = &mut self.entropy.kind {
            *eobrun = 0;
        }
        self.entropy.restarts_to_go = self.restart_interval;
        if self.unread_marker == 0 {
            self.entropy.insufficient_data = false;
        }
        Ok(true)
    }

    fn sequential_tables(&self, blkn: usize) -> (std::rc::Rc<DerivedTable>, std::rc::Rc<DerivedTable>, bool, bool) {
        let EntropyKind::Sequential { dc_derived, ac_derived, dc_cur, ac_cur, dc_needed, ac_needed } = &self.entropy.kind else { unreachable!("sequential decoder") };
        (dc_derived[dc_cur[blkn]].clone().expect("derived DC table"), ac_derived[ac_cur[blkn]].clone().expect("derived AC table"), dc_needed[blkn], ac_needed[blkn])
    }

    fn decode_mcu_slow(&mut self) -> bool {
        let mut br = self.load_bit_reader();
        let mut last_dc_val = self.entropy.last_dc_val;
        for blkn in 0..self.blocks_in_mcu as usize {
            let (dctbl, actbl, dc_needed, ac_needed) = self.sequential_tables(blkn);
            let Some(mut s) = self.huff_decode(&mut br, &dctbl) else { return false };
            if s != 0 {
                if !self.check_bit_buffer(&mut br, s) {
                    return false;
                }
                let r = br.get_bits(s);
                s = huff_extend(r, s);
            }
            if dc_needed {
                let ci = self.mcu_membership[blkn] as usize;
                s = s.wrapping_add(last_dc_val[ci]);
                last_dc_val[ci] = s;
                self.mcu_blocks[blkn][0] = s as i16;
            }
            if ac_needed {
                let mut k = 1usize;
                while k < DCTSIZE2 {
                    let Some(value) = self.huff_decode(&mut br, &actbl) else { return false };
                    let mut r = value >> 4;
                    let mut s = value & 15;
                    if s != 0 {
                        k += r as usize;
                        if !self.check_bit_buffer(&mut br, s) {
                            return false;
                        }
                        r = br.get_bits(s);
                        s = huff_extend(r, s);
                        self.mcu_blocks[blkn][NATURAL_ORDER[k]] = s as i16;
                    } else {
                        if r != 15 {
                            break;
                        }
                        k += 15;
                    }
                    k += 1;
                }
            } else {
                let mut k = 1usize;
                while k < DCTSIZE2 {
                    let Some(value) = self.huff_decode(&mut br, &actbl) else { return false };
                    let r = value >> 4;
                    let s = value & 15;
                    if s != 0 {
                        k += r as usize;
                        if !self.check_bit_buffer(&mut br, s) {
                            return false;
                        }
                        br.bits_left -= s;
                    } else {
                        if r != 15 {
                            break;
                        }
                        k += 15;
                    }
                    k += 1;
                }
            }
        }
        self.save_bit_reader(&br);
        self.entropy.last_dc_val = last_dc_val;
        true
    }

    fn decode_mcu_fast(&mut self) -> bool {
        let br = self.load_bit_reader();
        let mut last_dc_val = self.entropy.last_dc_val;
        let tables: Vec<_> = (0..self.blocks_in_mcu as usize).map(|blkn| self.sequential_tables(blkn)).collect();
        let membership = self.mcu_membership;
        let mut fast = FastReader { data: self.src.data(), buffer: br.next, get_buffer: br.get_buffer, bits_left: br.bits_left, unread_marker: self.unread_marker };
        let mut blocks = self.mcu_blocks;
        for (blkn, (dctbl, actbl, dc_needed, ac_needed)) in tables.iter().enumerate() {
            let mut s = fast.decode(dctbl);
            if s != 0 {
                fast.fill();
                let r = fast.get_bits(s);
                s = huff_extend(r, s);
            }
            if *dc_needed {
                let ci = membership[blkn] as usize;
                s = s.wrapping_add(last_dc_val[ci]);
                last_dc_val[ci] = s;
                blocks[blkn][0] = s as i16;
            }
            let mut k = 1usize;
            while k < DCTSIZE2 {
                let value = fast.decode(actbl);
                let mut r = value >> 4;
                let mut s = value & 15;
                if s != 0 {
                    k += r as usize;
                    fast.fill();
                    if *ac_needed {
                        r = fast.get_bits(s);
                        s = huff_extend(r, s);
                        blocks[blkn][NATURAL_ORDER[k]] = s as i16;
                    } else {
                        fast.bits_left -= s;
                    }
                } else {
                    if r != 15 {
                        break;
                    }
                    k += 15;
                }
                k += 1;
            }
        }
        let FastReader { buffer, get_buffer, bits_left, unread_marker, .. } = fast;
        self.mcu_blocks = blocks;
        if unread_marker != 0 {
            self.unread_marker = 0;
            return false;
        }
        let consumed = buffer - br.next;
        let br = BitReader { next: buffer, bytes: br.bytes - consumed, get_buffer, bits_left };
        self.save_bit_reader(&br);
        self.entropy.last_dc_val = last_dc_val;
        true
    }

    pub(crate) fn decode_mcu_sequential(&mut self) -> Result<bool> {
        let mut usefast = true;
        if self.restart_interval != 0 {
            if self.entropy.restarts_to_go == 0 && !self.process_restart_entropy(false)? {
                return Ok(false);
            }
            usefast = false;
        }
        if self.src_state.bytes_in_buffer < BUFSIZE * self.blocks_in_mcu as usize || self.unread_marker != 0 {
            usefast = false;
        }
        if !self.entropy.insufficient_data && !(usefast && self.decode_mcu_fast()) && !self.decode_mcu_slow() {
            return Ok(false);
        }
        if self.restart_interval != 0 {
            self.entropy.restarts_to_go = self.entropy.restarts_to_go.wrapping_sub(1);
        }
        Ok(true)
    }
}

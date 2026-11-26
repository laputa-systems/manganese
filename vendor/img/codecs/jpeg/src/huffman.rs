use super::decompress::{Decompress, Result};
use super::error::{Error, Warning};
use super::source::Source;
use super::tables::NUM_HUFF_TBLS;
use std::rc::Rc;

pub(crate) const HUFF_LOOKAHEAD: i32 = 8;
// Chromium's jconfigint.h sizes size_t from __WORDSIZE, which musl's <stdint.h> leaves
// undefined, so its libjpeg-turbo uses the 32-bit bit buffer even on x86-64.
pub(crate) const BIT_BUF_SIZE: i32 = 32;
const MIN_GET_BITS: i32 = BIT_BUF_SIZE - 7;

pub(crate) struct DerivedTable {
    pub maxcode: [i64; 18],
    pub valoffset: [i64; 18],
    pub lookup: [i32; 1 << HUFF_LOOKAHEAD],
    pub huffval: [u8; 256],
}

#[derive(Clone, Copy)]
pub(crate) struct BitReader {
    pub next: usize,
    pub bytes: usize,
    pub get_buffer: u64,
    pub bits_left: i32,
}

impl BitReader {
    pub fn get_bits(&mut self, nbits: i32) -> i32 {
        self.bits_left -= nbits;
        ((self.get_buffer >> self.bits_left) as i32) & ((1 << nbits) - 1)
    }

    pub fn peek_bits(&self, nbits: i32) -> i32 {
        ((self.get_buffer >> (self.bits_left - nbits)) as i32) & ((1 << nbits) - 1)
    }
}

pub(crate) fn huff_extend(x: i32, s: i32) -> i32 {
    if x < (1 << (s - 1)) { x.wrapping_add((-1i32 << s).wrapping_add(1)) } else { x }
}

pub(crate) fn make_derived_table<S: Source>(cinfo: &Decompress<S>, is_dc: bool, tblno: i32) -> Result<Rc<DerivedTable>> {
    if !(0..NUM_HUFF_TBLS as i32).contains(&tblno) {
        return Err(Error::NoHuffmanTable(tblno));
    }
    let table = if is_dc { cinfo.dc_huff_tbl_ptrs[tblno as usize].as_ref() } else { cinfo.ac_huff_tbl_ptrs[tblno as usize].as_ref() };
    let Some(htbl) = table else { return Err(Error::NoHuffmanTable(tblno)) };
    let mut huffsize = [0u8; 257];
    let mut huffcode = [0u32; 257];
    let mut p = 0usize;
    for l in 1..=16u8 {
        let i = htbl.bits[l as usize] as usize;
        if p + i > 256 {
            return Err(Error::BadHuffmanTable);
        }
        for _ in 0..i {
            huffsize[p] = l;
            p += 1;
        }
    }
    huffsize[p] = 0;
    let numsymbols = p;
    let mut code: u32 = 0;
    let mut si = huffsize[0] as i32;
    p = 0;
    while huffsize[p] != 0 {
        while huffsize[p] as i32 == si {
            huffcode[p] = code;
            p += 1;
            code = code.wrapping_add(1);
        }
        if code as i64 >= 1i64 << si {
            return Err(Error::BadHuffmanTable);
        }
        code <<= 1;
        si += 1;
    }
    let mut dtbl = DerivedTable { maxcode: [0; 18], valoffset: [0; 18], lookup: [(HUFF_LOOKAHEAD + 1) << HUFF_LOOKAHEAD; 1 << HUFF_LOOKAHEAD], huffval: htbl.huffval };
    p = 0;
    for l in 1..=16usize {
        if htbl.bits[l] != 0 {
            dtbl.valoffset[l] = p as i64 - huffcode[p] as i64;
            p += htbl.bits[l] as usize;
            dtbl.maxcode[l] = huffcode[p - 1] as i64;
        } else {
            dtbl.maxcode[l] = -1;
        }
    }
    dtbl.valoffset[17] = 0;
    dtbl.maxcode[17] = 0xFFFFF;
    p = 0;
    for l in 1..=HUFF_LOOKAHEAD {
        for _ in 1..=htbl.bits[l as usize] as i32 {
            let start = (huffcode[p] << (HUFF_LOOKAHEAD - l)) as usize;
            dtbl.lookup[start..start + (1 << (HUFF_LOOKAHEAD - l))].fill((l << HUFF_LOOKAHEAD) | htbl.huffval[p] as i32);
            p += 1;
        }
    }
    if is_dc {
        let limit = if cinfo.master.lossless { 16 } else { 15 };
        for &sym in &htbl.huffval[..numsymbols] {
            if sym as i32 > limit {
                return Err(Error::BadHuffmanTable);
            }
        }
    }
    Ok(Rc::new(dtbl))
}

impl<S: Source> Decompress<S> {
    pub(crate) fn load_bit_reader(&self) -> BitReader {
        BitReader { next: self.src_state.next_input_byte, bytes: self.src_state.bytes_in_buffer, get_buffer: self.entropy.get_buffer, bits_left: self.entropy.bits_left }
    }

    pub(crate) fn save_bit_reader(&mut self, br: &BitReader) {
        self.src_state.next_input_byte = br.next;
        self.src_state.bytes_in_buffer = br.bytes;
        self.entropy.get_buffer = br.get_buffer;
        self.entropy.bits_left = br.bits_left;
    }

    pub(crate) fn fill_bit_buffer(&mut self, br: &mut BitReader, nbits: i32) -> bool {
        let mut next = br.next;
        let mut bytes = br.bytes;
        let mut get_buffer = br.get_buffer;
        let mut bits_left = br.bits_left;
        let mut hit_marker = self.unread_marker != 0;
        if !hit_marker {
            while bits_left < MIN_GET_BITS {
                if bytes == 0 {
                    if !self.src.fill_input_buffer(&mut self.src_state) {
                        return false;
                    }
                    next = self.src_state.next_input_byte;
                    bytes = self.src_state.bytes_in_buffer;
                }
                bytes -= 1;
                let mut c = self.byte_at(next) as i32;
                next += 1;
                if c == 0xFF {
                    loop {
                        if bytes == 0 {
                            if !self.src.fill_input_buffer(&mut self.src_state) {
                                return false;
                            }
                            next = self.src_state.next_input_byte;
                            bytes = self.src_state.bytes_in_buffer;
                        }
                        bytes -= 1;
                        c = self.byte_at(next) as i32;
                        next += 1;
                        if c != 0xFF {
                            break;
                        }
                    }
                    if c == 0 {
                        c = 0xFF;
                    } else {
                        self.unread_marker = c;
                        hit_marker = true;
                        break;
                    }
                }
                get_buffer = (get_buffer << 8) | c as u64;
                bits_left += 8;
            }
        }
        if hit_marker && nbits > bits_left {
            if !self.entropy.insufficient_data {
                self.warn(Warning::HitMarker);
                self.entropy.insufficient_data = true;
            }
            get_buffer <<= MIN_GET_BITS - bits_left;
            bits_left = MIN_GET_BITS;
        }
        *br = BitReader { next, bytes, get_buffer, bits_left };
        true
    }

    pub(crate) fn check_bit_buffer(&mut self, br: &mut BitReader, nbits: i32) -> bool {
        if br.bits_left < nbits {
            return self.fill_bit_buffer(br, nbits);
        }
        true
    }

    fn huff_decode_slow(&mut self, br: &mut BitReader, table: &DerivedTable, min_bits: i32) -> i32 {
        let mut l = min_bits;
        if !self.check_bit_buffer(br, l) {
            return -1;
        }
        let mut code = br.get_bits(l) as i64;
        while code > table.maxcode[l as usize] {
            code <<= 1;
            if !self.check_bit_buffer(br, 1) {
                return -1;
            }
            code |= br.get_bits(1) as i64;
            l += 1;
        }
        if l > 16 {
            self.warn(Warning::HuffmanBadCode);
            return 0;
        }
        table.huffval[(code + table.valoffset[l as usize]) as usize] as i32
    }

    pub(crate) fn huff_decode(&mut self, br: &mut BitReader, table: &DerivedTable) -> Option<i32> {
        let mut nb = 0;
        let mut slow = false;
        if br.bits_left < HUFF_LOOKAHEAD {
            if !self.fill_bit_buffer(br, 0) {
                return None;
            }
            if br.bits_left < HUFF_LOOKAHEAD {
                nb = 1;
                slow = true;
            }
        }
        if !slow {
            let look = br.peek_bits(HUFF_LOOKAHEAD) as usize;
            nb = table.lookup[look] >> HUFF_LOOKAHEAD;
            if nb <= HUFF_LOOKAHEAD {
                br.bits_left -= nb;
                return Some(table.lookup[look] & ((1 << HUFF_LOOKAHEAD) - 1));
            }
        }
        let result = self.huff_decode_slow(br, table, nb);
        if result < 0 { None } else { Some(result) }
    }
}

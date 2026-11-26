use super::decompress::{Component, ConsumeStatus, Decompress, HuffTable, Result, SavedMarker};
use super::error::{Error, Warning};
use super::source::Source;
use super::tables::*;

const M_SOF0: i32 = 0xC0;
const M_SOF1: i32 = 0xC1;
const M_SOF2: i32 = 0xC2;
const M_SOF3: i32 = 0xC3;
const M_DHT: i32 = 0xC4;
const M_SOF5: i32 = 0xC5;
const M_SOF6: i32 = 0xC6;
const M_SOF7: i32 = 0xC7;
const M_JPG: i32 = 0xC8;
const M_SOF9: i32 = 0xC9;
const M_SOF10: i32 = 0xCA;
const M_SOF11: i32 = 0xCB;
const M_DAC: i32 = 0xCC;
const M_SOF13: i32 = 0xCD;
const M_SOF14: i32 = 0xCE;
const M_SOF15: i32 = 0xCF;
const M_RST0: i32 = 0xD0;
const M_RST7: i32 = 0xD7;
const M_SOI: i32 = 0xD8;
const M_EOI: i32 = 0xD9;
const M_SOS: i32 = 0xDA;
const M_DQT: i32 = 0xDB;
const M_DNL: i32 = 0xDC;
const M_DRI: i32 = 0xDD;
const M_APP0: i32 = 0xE0;
const M_APP14: i32 = 0xEE;
const M_APP15: i32 = 0xEF;
const M_COM: i32 = 0xFE;
const M_TEM: i32 = 0x01;

const APP0_DATA_LEN: usize = 14;
const APP14_DATA_LEN: usize = 12;
const APPN_DATA_LEN: usize = 14;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Processor {
    Skip,
    Interesting,
    Save,
}

pub(crate) struct MarkerReader {
    pub saw_soi: bool,
    pub saw_sof: bool,
    pub next_restart_num: i32,
    pub discarded_bytes: u32,
    process_com: Processor,
    process_appn: [Processor; 16],
    length_limit_com: u32,
    length_limit_appn: [u32; 16],
    cur_marker: Option<SavedMarker>,
    bytes_read: u32,
}

impl MarkerReader {
    pub fn new() -> Self {
        let mut process_appn = [Processor::Skip; 16];
        process_appn[0] = Processor::Interesting;
        process_appn[14] = Processor::Interesting;
        MarkerReader {
            saw_soi: false,
            saw_sof: false,
            next_restart_num: 0,
            discarded_bytes: 0,
            process_com: Processor::Skip,
            process_appn,
            length_limit_com: 0,
            length_limit_appn: [0; 16],
            cur_marker: None,
            bytes_read: 0,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Input {
    pub next: usize,
    pub bytes: usize,
}

impl<S: Source> Decompress<S> {
    pub(crate) fn input_vars(&self) -> Input {
        Input { next: self.src_state.next_input_byte, bytes: self.src_state.bytes_in_buffer }
    }

    pub(crate) fn input_sync(&mut self, input: Input) {
        self.src_state.next_input_byte = input.next;
        self.src_state.bytes_in_buffer = input.bytes;
    }

    pub(crate) fn make_byte_avail(&mut self, input: &mut Input) -> bool {
        if input.bytes == 0 {
            if !self.src.fill_input_buffer(&mut self.src_state) {
                return false;
            }
            *input = self.input_vars();
        }
        true
    }

    pub(crate) fn input_byte(&mut self, input: &mut Input) -> Option<i32> {
        if !self.make_byte_avail(input) {
            return None;
        }
        input.bytes -= 1;
        let value = self.byte_at(input.next) as i32;
        input.next += 1;
        Some(value)
    }

    pub(crate) fn input_2bytes(&mut self, input: &mut Input) -> Option<i32> {
        let high = self.input_byte(input)?;
        let low = self.input_byte(input)?;
        Some((high << 8) + low)
    }

    pub fn save_markers(&mut self, marker_code: i32, length_limit: u32) {
        let mut length_limit = length_limit;
        let processor = if length_limit != 0 {
            if marker_code == M_APP0 && (length_limit as usize) < APP0_DATA_LEN {
                length_limit = APP0_DATA_LEN as u32;
            } else if marker_code == M_APP14 && (length_limit as usize) < APP14_DATA_LEN {
                length_limit = APP14_DATA_LEN as u32;
            }
            Processor::Save
        } else if marker_code == M_APP0 || marker_code == M_APP14 {
            Processor::Interesting
        } else {
            Processor::Skip
        };
        if marker_code == M_COM {
            self.marker.process_com = processor;
            self.marker.length_limit_com = length_limit;
        } else if (M_APP0..=M_APP15).contains(&marker_code) {
            self.marker.process_appn[(marker_code - M_APP0) as usize] = processor;
            self.marker.length_limit_appn[(marker_code - M_APP0) as usize] = length_limit;
        }
    }

    pub(crate) fn reset_marker_reader(&mut self) {
        self.comp_info.clear();
        self.comp_info_allocated = false;
        self.input_scan_number = 0;
        self.unread_marker = 0;
        self.marker.saw_soi = false;
        self.marker.saw_sof = false;
        self.marker.discarded_bytes = 0;
        self.marker.cur_marker = None;
    }

    fn get_soi(&mut self) -> Result<bool> {
        if self.marker.saw_soi {
            return Err(Error::SoiDuplicate);
        }
        self.restart_interval = 0;
        self.jpeg_color_space = super::decompress::ColorSpace::Unknown;
        self.saw_jfif_marker = false;
        self.jfif_major_version = 1;
        self.jfif_minor_version = 1;
        self.density_unit = 0;
        self.x_density = 1;
        self.y_density = 1;
        self.saw_adobe_marker = false;
        self.adobe_transform = 0;
        self.marker.saw_soi = true;
        Ok(true)
    }

    fn get_sof(&mut self, is_prog: bool, is_lossless: bool, is_arith: bool) -> Result<bool> {
        let mut input = self.input_vars();
        if self.marker.saw_sof {
            return Err(Error::SofDuplicate);
        }
        self.progressive_mode = is_prog;
        self.master.lossless = is_lossless;
        self.arith_code = is_arith;
        let Some(length) = self.input_2bytes(&mut input) else { return Ok(false) };
        let Some(precision) = self.input_byte(&mut input) else { return Ok(false) };
        self.data_precision = precision;
        let Some(height) = self.input_2bytes(&mut input) else { return Ok(false) };
        self.image_height = height as u32;
        let Some(width) = self.input_2bytes(&mut input) else { return Ok(false) };
        self.image_width = width as u32;
        let Some(count) = self.input_byte(&mut input) else { return Ok(false) };
        self.num_components = count;
        let length = length as i64 - 8;
        if self.image_height == 0 || self.image_width == 0 || self.num_components <= 0 {
            return Err(Error::EmptyImage);
        }
        if length != (self.num_components * 3) as i64 {
            return Err(Error::BadLength);
        }
        if !self.comp_info_allocated {
            self.comp_info = vec![Component::default(); self.num_components as usize];
            self.comp_info_allocated = true;
        }
        for ci in 0..self.num_components as usize {
            self.comp_info[ci].component_index = ci as i32;
            let Some(id) = self.input_byte(&mut input) else { return Ok(false) };
            self.comp_info[ci].component_id = id;
            let Some(c) = self.input_byte(&mut input) else { return Ok(false) };
            self.comp_info[ci].h_samp_factor = (c >> 4) & 15;
            self.comp_info[ci].v_samp_factor = c & 15;
            let Some(table) = self.input_byte(&mut input) else { return Ok(false) };
            self.comp_info[ci].quant_tbl_no = table;
        }
        self.marker.saw_sof = true;
        self.input_sync(input);
        Ok(true)
    }

    fn get_sos(&mut self) -> Result<bool> {
        let mut input = self.input_vars();
        if !self.marker.saw_sof {
            return Err(Error::SosNoSof);
        }
        let Some(length) = self.input_2bytes(&mut input) else { return Ok(false) };
        let Some(n) = self.input_byte(&mut input) else { return Ok(false) };
        if length != n * 2 + 6 || n < 1 || n > MAX_COMPS_IN_SCAN as i32 {
            return Err(Error::BadLength);
        }
        self.comps_in_scan = n;
        self.cur_comp_info = [None; MAX_COMPS_IN_SCAN];
        for i in 0..n as usize {
            let Some(cc) = self.input_byte(&mut input) else { return Ok(false) };
            let Some(c) = self.input_byte(&mut input) else { return Ok(false) };
            let mut found = None;
            let limit = (self.num_components as usize).min(MAX_COMPS_IN_SCAN);
            for ci in 0..limit {
                if cc == self.comp_info[ci].component_id && self.cur_comp_info[ci].is_none() {
                    found = Some(ci);
                    break;
                }
            }
            let Some(ci) = found else { return Err(Error::BadComponentId(cc)) };
            self.cur_comp_info[i] = Some(ci);
            self.comp_info[ci].dc_tbl_no = (c >> 4) & 15;
            self.comp_info[ci].ac_tbl_no = c & 15;
            if self.cur_comp_info[..i].contains(&Some(ci)) {
                return Err(Error::BadComponentId(cc));
            }
        }
        let Some(ss) = self.input_byte(&mut input) else { return Ok(false) };
        self.ss = ss;
        let Some(se) = self.input_byte(&mut input) else { return Ok(false) };
        self.se = se;
        let Some(c) = self.input_byte(&mut input) else { return Ok(false) };
        self.ah = (c >> 4) & 15;
        self.al = c & 15;
        self.marker.next_restart_num = 0;
        self.input_scan_number += 1;
        self.input_sync(input);
        Ok(true)
    }

    fn get_dht(&mut self) -> Result<bool> {
        let mut input = self.input_vars();
        let Some(length) = self.input_2bytes(&mut input) else { return Ok(false) };
        let mut length = length as i64 - 2;
        while length > 16 {
            let Some(mut index) = self.input_byte(&mut input) else { return Ok(false) };
            let mut bits = [0u8; 17];
            let mut count: i64 = 0;
            for value in bits.iter_mut().skip(1) {
                let Some(b) = self.input_byte(&mut input) else { return Ok(false) };
                *value = b as u8;
                count += b as i64;
            }
            length -= 1 + 16;
            if count > 256 || count > length {
                return Err(Error::BadHuffmanTable);
            }
            let mut huffval = [0u8; 256];
            for value in huffval.iter_mut().take(count as usize) {
                let Some(b) = self.input_byte(&mut input) else { return Ok(false) };
                *value = b as u8;
            }
            length -= count;
            let table = HuffTable { bits, huffval };
            if index & 0x10 != 0 {
                index -= 0x10;
                if !(0..NUM_HUFF_TBLS as i32).contains(&index) {
                    return Err(Error::DhtIndex(index));
                }
                self.ac_huff_tbl_ptrs[index as usize] = Some(table);
            } else {
                if !(0..NUM_HUFF_TBLS as i32).contains(&index) {
                    return Err(Error::DhtIndex(index));
                }
                self.dc_huff_tbl_ptrs[index as usize] = Some(table);
            }
        }
        if length != 0 {
            return Err(Error::BadLength);
        }
        self.input_sync(input);
        Ok(true)
    }

    fn get_dqt(&mut self) -> Result<bool> {
        let mut input = self.input_vars();
        let Some(length) = self.input_2bytes(&mut input) else { return Ok(false) };
        let mut length = length as i64 - 2;
        while length > 0 {
            let Some(n) = self.input_byte(&mut input) else { return Ok(false) };
            let prec = n >> 4;
            let n = n & 0x0F;
            if n >= NUM_QUANT_TBLS as i32 {
                return Err(Error::DqtIndex(n));
            }
            let mut table = self.quant_tbl_ptrs[n as usize].unwrap_or([0; DCTSIZE2]);
            for i in 0..DCTSIZE2 {
                let value = if prec != 0 { self.input_2bytes(&mut input) } else { self.input_byte(&mut input) };
                let Some(value) = value else {
                    self.quant_tbl_ptrs[n as usize] = Some(table);
                    return Ok(false);
                };
                table[NATURAL_ORDER[i]] = value as u16;
            }
            self.quant_tbl_ptrs[n as usize] = Some(table);
            length -= DCTSIZE2 as i64 + 1;
            if prec != 0 {
                length -= DCTSIZE2 as i64;
            }
        }
        if length != 0 {
            return Err(Error::BadLength);
        }
        self.input_sync(input);
        Ok(true)
    }

    fn get_dri(&mut self) -> Result<bool> {
        let mut input = self.input_vars();
        let Some(length) = self.input_2bytes(&mut input) else { return Ok(false) };
        if length != 4 {
            return Err(Error::BadLength);
        }
        let Some(value) = self.input_2bytes(&mut input) else { return Ok(false) };
        self.restart_interval = value as u32;
        self.input_sync(input);
        Ok(true)
    }

    fn examine_app0(&mut self, data: &[u8], datalen: usize) {
        if datalen >= APP0_DATA_LEN && data[0] == 0x4A && data[1] == 0x46 && data[2] == 0x49 && data[3] == 0x46 && data[4] == 0 {
            self.saw_jfif_marker = true;
            self.jfif_major_version = data[5];
            self.jfif_minor_version = data[6];
            self.density_unit = data[7];
            self.x_density = ((data[8] as u16) << 8) + data[9] as u16;
            self.y_density = ((data[10] as u16) << 8) + data[11] as u16;
            if self.jfif_major_version != 1 {
                self.warn(Warning::JfifMajor);
            }
        }
    }

    fn examine_app14(&mut self, data: &[u8], datalen: usize) {
        if datalen >= APP14_DATA_LEN && data[0] == 0x41 && data[1] == 0x64 && data[2] == 0x6F && data[3] == 0x62 && data[4] == 0x65 {
            self.saw_adobe_marker = true;
            self.adobe_transform = data[11];
        }
    }

    fn get_interesting_appn(&mut self) -> Result<bool> {
        let mut input = self.input_vars();
        let Some(length) = self.input_2bytes(&mut input) else { return Ok(false) };
        let mut length = length as i64 - 2;
        let numtoread = if length >= APPN_DATA_LEN as i64 {
            APPN_DATA_LEN
        } else if length > 0 {
            length as usize
        } else {
            0
        };
        let mut b = [0u8; APPN_DATA_LEN];
        for value in b.iter_mut().take(numtoread) {
            let Some(v) = self.input_byte(&mut input) else { return Ok(false) };
            *value = v as u8;
        }
        length -= numtoread as i64;
        match self.unread_marker {
            M_APP0 => self.examine_app0(&b, numtoread),
            M_APP14 => self.examine_app14(&b, numtoread),
            marker => return Err(Error::UnknownMarker(marker)),
        }
        self.input_sync(input);
        if length > 0 {
            self.src.skip_input_data(&mut self.src_state, length);
        }
        Ok(true)
    }

    fn save_marker(&mut self) -> Result<bool> {
        let mut input = self.input_vars();
        let mut length: i64 = 0;
        let mut bytes_read: u32;
        let data_length: u32;
        match self.marker.cur_marker.as_ref() {
            None => {
                let Some(value) = self.input_2bytes(&mut input) else { return Ok(false) };
                length = value as i64 - 2;
                bytes_read = 0;
                if length >= 0 {
                    let mut limit = if self.unread_marker == M_COM {
                        self.marker.length_limit_com
                    } else {
                        self.marker.length_limit_appn[(self.unread_marker - M_APP0) as usize]
                    };
                    if (length as u32) < limit {
                        limit = length as u32;
                    }
                    self.marker.cur_marker = Some(SavedMarker { marker: self.unread_marker as u8, original_length: length as u32, data: vec![0; limit as usize] });
                    self.marker.bytes_read = 0;
                    data_length = limit;
                } else {
                    data_length = 0;
                }
            }
            Some(marker) => {
                bytes_read = self.marker.bytes_read;
                data_length = marker.data.len() as u32;
            }
        }
        while bytes_read < data_length {
            self.input_sync(input);
            self.marker.bytes_read = bytes_read;
            if !self.make_byte_avail(&mut input) {
                return Ok(false);
            }
            while bytes_read < data_length && input.bytes > 0 {
                let byte = self.byte_at(input.next);
                self.marker.cur_marker.as_mut().expect("marker in progress").data[bytes_read as usize] = byte;
                input.next += 1;
                input.bytes -= 1;
                bytes_read += 1;
            }
        }
        let data = match self.marker.cur_marker.take() {
            Some(marker) => {
                length = marker.original_length as i64 - data_length as i64;
                let data = marker.data.clone();
                self.marker_list.push(marker);
                data
            }
            None => Vec::new(),
        };
        match self.unread_marker {
            M_APP0 => self.examine_app0(&data, data_length as usize),
            M_APP14 => self.examine_app14(&data, data_length as usize),
            _ => {}
        }
        self.input_sync(input);
        if length > 0 {
            self.src.skip_input_data(&mut self.src_state, length);
        }
        Ok(true)
    }

    fn skip_variable(&mut self) -> Result<bool> {
        let mut input = self.input_vars();
        let Some(length) = self.input_2bytes(&mut input) else { return Ok(false) };
        let length = length as i64 - 2;
        self.input_sync(input);
        if length > 0 {
            self.src.skip_input_data(&mut self.src_state, length);
        }
        Ok(true)
    }

    fn run_processor(&mut self, processor: Processor) -> Result<bool> {
        match processor {
            Processor::Skip => self.skip_variable(),
            Processor::Interesting => self.get_interesting_appn(),
            Processor::Save => self.save_marker(),
        }
    }

    pub(crate) fn next_marker(&mut self) -> Result<bool> {
        let mut input = self.input_vars();
        let mut c;
        loop {
            let Some(value) = self.input_byte(&mut input) else { return Ok(false) };
            c = value;
            while c != 0xFF {
                self.marker.discarded_bytes += 1;
                self.input_sync(input);
                let Some(value) = self.input_byte(&mut input) else { return Ok(false) };
                c = value;
            }
            loop {
                let Some(value) = self.input_byte(&mut input) else { return Ok(false) };
                c = value;
                if c != 0xFF {
                    break;
                }
            }
            if c != 0 {
                break;
            }
            self.marker.discarded_bytes += 2;
            self.input_sync(input);
        }
        if self.marker.discarded_bytes != 0 {
            self.warn(Warning::ExtraneousData);
            self.marker.discarded_bytes = 0;
        }
        self.unread_marker = c;
        self.input_sync(input);
        Ok(true)
    }

    fn first_marker(&mut self) -> Result<bool> {
        let mut input = self.input_vars();
        let Some(c) = self.input_byte(&mut input) else { return Ok(false) };
        let Some(c2) = self.input_byte(&mut input) else { return Ok(false) };
        if c != 0xFF || c2 != M_SOI {
            return Err(Error::NoSoi);
        }
        self.unread_marker = c2;
        self.input_sync(input);
        Ok(true)
    }

    pub(crate) fn read_markers(&mut self) -> Result<ConsumeStatus> {
        loop {
            if self.unread_marker == 0 {
                if !self.marker.saw_soi {
                    if !self.first_marker()? {
                        return Ok(ConsumeStatus::Suspended);
                    }
                } else if !self.next_marker()? {
                    return Ok(ConsumeStatus::Suspended);
                }
            }
            let done = match self.unread_marker {
                M_SOI => self.get_soi()?,
                M_SOF0 | M_SOF1 => self.get_sof(false, false, false)?,
                M_SOF2 => self.get_sof(true, false, false)?,
                M_SOF3 => self.get_sof(false, true, false)?,
                M_SOF9 => self.get_sof(false, false, true)?,
                M_SOF10 => self.get_sof(true, false, true)?,
                M_SOF11 => self.get_sof(false, true, true)?,
                M_SOF5 | M_SOF6 | M_SOF7 | M_JPG | M_SOF13 | M_SOF14 | M_SOF15 => return Err(Error::SofUnsupported(self.unread_marker)),
                M_SOS => {
                    if !self.get_sos()? {
                        return Ok(ConsumeStatus::Suspended);
                    }
                    self.unread_marker = 0;
                    return Ok(ConsumeStatus::ReachedSos);
                }
                M_EOI => {
                    self.unread_marker = 0;
                    return Ok(ConsumeStatus::ReachedEoi);
                }
                M_DAC => self.skip_variable()?,
                M_DHT => self.get_dht()?,
                M_DQT => self.get_dqt()?,
                M_DRI => self.get_dri()?,
                M_APP0..=M_APP15 => {
                    let processor = self.marker.process_appn[(self.unread_marker - M_APP0) as usize];
                    self.run_processor(processor)?
                }
                M_COM => {
                    let processor = self.marker.process_com;
                    self.run_processor(processor)?
                }
                M_RST0..=M_RST7 | M_TEM => true,
                M_DNL => self.skip_variable()?,
                marker => return Err(Error::UnknownMarker(marker)),
            };
            if !done {
                return Ok(ConsumeStatus::Suspended);
            }
            self.unread_marker = 0;
        }
    }

    pub(crate) fn read_restart_marker(&mut self) -> Result<bool> {
        if self.unread_marker == 0 && !self.next_marker()? {
            return Ok(false);
        }
        if self.unread_marker == M_RST0 + self.marker.next_restart_num {
            self.unread_marker = 0;
        } else if !self.resync_to_restart(self.marker.next_restart_num)? {
            return Ok(false);
        }
        self.marker.next_restart_num = (self.marker.next_restart_num + 1) & 7;
        Ok(true)
    }

    fn resync_to_restart(&mut self, desired: i32) -> Result<bool> {
        let mut marker = self.unread_marker;
        self.warn(Warning::MustResync);
        loop {
            let action = if marker < M_SOF0 {
                2
            } else if !(M_RST0..=M_RST7).contains(&marker) || marker == M_RST0 + ((desired + 1) & 7) || marker == M_RST0 + ((desired + 2) & 7) {
                3
            } else if marker == M_RST0 + ((desired - 1) & 7) || marker == M_RST0 + ((desired - 2) & 7) {
                2
            } else {
                1
            };
            match action {
                1 => {
                    self.unread_marker = 0;
                    return Ok(true);
                }
                2 => {
                    if !self.next_marker()? {
                        return Ok(false);
                    }
                    marker = self.unread_marker;
                }
                _ => return Ok(true),
            }
        }
    }
}

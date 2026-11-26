use super::coefficients::CoefController;
use super::color::ColorDeconverter;
use super::error::{Error, Warning};
use super::huffman::DerivedTable;
use super::idct::Idct;
use super::lossless::{DiffController, LosslessDecompressor};
use super::main_buffer::MainController;
use super::marker::MarkerReader;
use super::source::{Source, SourceState};
use super::tables::*;
use super::upsample::Upsampler;
use std::fmt;
use std::rc::Rc;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorSpace {
    Unknown,
    Grayscale,
    Rgb,
    YCbCr,
    Cmyk,
    Ycck,
    ExtBgra,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum GlobalState {
    Start = 200,
    InHeader = 201,
    Ready = 202,
    Preload = 203,
    Prescan = 204,
    Scanning = 205,
    RawOk = 206,
    BufImage = 207,
    BufPost = 208,
    Stopping = 210,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsumeStatus {
    Suspended,
    ReachedSos,
    ReachedEoi,
    RowCompleted,
    ScanCompleted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadHeader {
    Suspended,
    HeaderOk,
    TablesOnly,
}

#[derive(Clone, Debug)]
pub struct SavedMarker {
    pub marker: u8,
    pub original_length: u32,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct Component {
    pub component_id: i32,
    pub component_index: i32,
    pub h_samp_factor: i32,
    pub v_samp_factor: i32,
    pub quant_tbl_no: i32,
    pub dc_tbl_no: i32,
    pub ac_tbl_no: i32,
    pub width_in_blocks: u32,
    pub height_in_blocks: u32,
    pub dct_scaled_size: i32,
    pub downsampled_width: u32,
    pub downsampled_height: u32,
    pub component_needed: bool,
    pub mcu_width: i32,
    pub mcu_height: i32,
    pub mcu_blocks: i32,
    pub mcu_sample_width: i32,
    pub last_col_width: i32,
    pub last_row_height: i32,
    pub(crate) quant_table: Option<[u16; DCTSIZE2]>,
    pub(crate) dct_table: [i16; DCTSIZE2],
}

impl Default for Component {
    fn default() -> Self {
        Component {
            component_id: 0,
            component_index: 0,
            h_samp_factor: 0,
            v_samp_factor: 0,
            quant_tbl_no: 0,
            dc_tbl_no: 0,
            ac_tbl_no: 0,
            width_in_blocks: 0,
            height_in_blocks: 0,
            dct_scaled_size: 0,
            downsampled_width: 0,
            downsampled_height: 0,
            component_needed: false,
            mcu_width: 0,
            mcu_height: 0,
            mcu_blocks: 0,
            mcu_sample_width: 0,
            last_col_width: 0,
            last_row_height: 0,
            quant_table: None,
            dct_table: [0; DCTSIZE2],
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct HuffTable {
    pub bits: [u8; 17],
    pub huffval: [u8; 256],
}

#[derive(Default)]
pub(crate) struct Master {
    pub lossless: bool,
    pub first_imcu_col: u32,
    pub last_imcu_col: u32,
    pub first_mcu_col: [u32; MAX_COMPONENTS as usize],
    pub last_mcu_col: [u32; MAX_COMPONENTS as usize],
    pub last_good_imcu_row: u32,
    pub pass_number: i32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConsumeMode {
    Markers,
    Data,
}

pub(crate) struct InputController {
    pub consume: ConsumeMode,
    pub has_multiple_scans: bool,
    pub eoi_reached: bool,
    pub inheaders: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProgressiveMcu {
    DcFirst,
    AcFirst,
    DcRefine,
    AcRefine,
}

pub(crate) enum EntropyKind {
    None,
    Sequential { dc_derived: [Option<Rc<DerivedTable>>; NUM_HUFF_TBLS], ac_derived: [Option<Rc<DerivedTable>>; NUM_HUFF_TBLS], dc_cur: [usize; 10], ac_cur: [usize; 10], dc_needed: [bool; 10], ac_needed: [bool; 10] },
    Progressive { derived: [Option<Rc<DerivedTable>>; NUM_HUFF_TBLS], ac_tbl: usize, mcu: ProgressiveMcu, eobrun: u32 },
    Lossless { derived: [Option<Rc<DerivedTable>>; NUM_HUFF_TBLS], cur: [usize; 10], output_ptr_info: [(usize, usize, usize); 10], output_ptr_index: [usize; 10], num_output_ptrs: usize },
}

pub(crate) struct Entropy {
    pub kind: EntropyKind,
    pub get_buffer: u64,
    pub bits_left: i32,
    pub last_dc_val: [i32; MAX_COMPS_IN_SCAN],
    pub restarts_to_go: u32,
    pub insufficient_data: bool,
}

pub(crate) enum CoefKind {
    None,
    Lossy(CoefController),
    Lossless(DiffController),
}

pub(crate) enum IdctKind {
    None,
    Lossy(Idct),
    Lossless(LosslessDecompressor),
}

pub type ProgressMonitor = Box<dyn FnMut(i32) -> Result<()>>;

pub struct Decompress<S> {
    pub src: S,
    pub src_state: SourceState,
    pub count_corrupt_warnings: bool,
    pub corrupt_warnings: u32,
    pub progress_monitor: Option<ProgressMonitor>,
    pub(crate) global_state: GlobalState,
    pub image_width: u32,
    pub image_height: u32,
    pub num_components: i32,
    pub jpeg_color_space: ColorSpace,
    pub out_color_space: ColorSpace,
    pub scale_num: u32,
    pub scale_denom: u32,
    pub buffered_image: bool,
    pub do_fancy_upsampling: bool,
    pub do_block_smoothing: bool,
    pub output_width: u32,
    pub output_height: u32,
    pub out_color_components: i32,
    pub output_components: i32,
    pub rec_outbuf_height: i32,
    pub output_scanline: u32,
    pub input_scan_number: i32,
    pub input_imcu_row: u32,
    pub output_scan_number: i32,
    pub output_imcu_row: u32,
    pub coef_bits: Option<Vec<[i32; DCTSIZE2]>>,
    pub(crate) quant_tbl_ptrs: [Option<[u16; DCTSIZE2]>; NUM_QUANT_TBLS],
    pub(crate) dc_huff_tbl_ptrs: [Option<HuffTable>; NUM_HUFF_TBLS],
    pub(crate) ac_huff_tbl_ptrs: [Option<HuffTable>; NUM_HUFF_TBLS],
    pub data_precision: i32,
    pub comp_info: Vec<Component>,
    pub(crate) comp_info_allocated: bool,
    pub progressive_mode: bool,
    pub arith_code: bool,
    pub restart_interval: u32,
    pub saw_jfif_marker: bool,
    pub jfif_major_version: u8,
    pub jfif_minor_version: u8,
    pub density_unit: u8,
    pub x_density: u16,
    pub y_density: u16,
    pub saw_adobe_marker: bool,
    pub adobe_transform: u8,
    pub marker_list: Vec<SavedMarker>,
    pub max_h_samp_factor: i32,
    pub max_v_samp_factor: i32,
    pub min_dct_scaled_size: i32,
    pub total_imcu_rows: u32,
    pub comps_in_scan: i32,
    pub(crate) cur_comp_info: [Option<usize>; MAX_COMPS_IN_SCAN],
    pub mcus_per_row: u32,
    pub mcu_rows_in_scan: u32,
    pub blocks_in_mcu: i32,
    pub(crate) mcu_membership: [i32; D_MAX_BLOCKS_IN_MCU as usize],
    pub ss: i32,
    pub se: i32,
    pub ah: i32,
    pub al: i32,
    pub unread_marker: i32,
    pub(crate) master: Master,
    pub(crate) inputctl: InputController,
    pub(crate) marker: MarkerReader,
    pub(crate) entropy: Entropy,
    pub(crate) mcu_blocks: [[i16; DCTSIZE2]; D_MAX_BLOCKS_IN_MCU as usize],
    pub(crate) coef: CoefKind,
    pub(crate) idct: IdctKind,
    pub(crate) upsample: Option<Upsampler>,
    pub(crate) cconvert: Option<ColorDeconverter>,
    pub(crate) main: Option<MainController>,
}

impl<S: fmt::Debug> fmt::Debug for Decompress<S> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Decompress")
            .field("src", &self.src)
            .field("image_width", &self.image_width)
            .field("image_height", &self.image_height)
            .field("num_components", &self.num_components)
            .field("out_color_space", &self.out_color_space)
            .finish_non_exhaustive()
    }
}

impl<S: Source> Decompress<S> {
    pub fn new(src: S) -> Self {
        let mut decompress = Decompress {
            src,
            src_state: SourceState::default(),
            count_corrupt_warnings: false,
            corrupt_warnings: 0,
            progress_monitor: None,
            global_state: GlobalState::Start,
            image_width: 0,
            image_height: 0,
            num_components: 0,
            jpeg_color_space: ColorSpace::Unknown,
            out_color_space: ColorSpace::Unknown,
            scale_num: 1,
            scale_denom: 1,
            buffered_image: false,
            do_fancy_upsampling: true,
            do_block_smoothing: true,
            output_width: 0,
            output_height: 0,
            out_color_components: 0,
            output_components: 0,
            rec_outbuf_height: 0,
            output_scanline: 0,
            input_scan_number: 0,
            input_imcu_row: 0,
            output_scan_number: 0,
            output_imcu_row: 0,
            coef_bits: None,
            quant_tbl_ptrs: [None; NUM_QUANT_TBLS],
            dc_huff_tbl_ptrs: [None, None, None, None],
            ac_huff_tbl_ptrs: [None, None, None, None],
            data_precision: 8,
            comp_info: Vec::new(),
            comp_info_allocated: false,
            progressive_mode: false,
            arith_code: false,
            restart_interval: 0,
            saw_jfif_marker: false,
            jfif_major_version: 1,
            jfif_minor_version: 1,
            density_unit: 0,
            x_density: 1,
            y_density: 1,
            saw_adobe_marker: false,
            adobe_transform: 0,
            marker_list: Vec::new(),
            max_h_samp_factor: 0,
            max_v_samp_factor: 0,
            min_dct_scaled_size: 0,
            total_imcu_rows: 0,
            comps_in_scan: 0,
            cur_comp_info: [None; MAX_COMPS_IN_SCAN],
            mcus_per_row: 0,
            mcu_rows_in_scan: 0,
            blocks_in_mcu: 0,
            mcu_membership: [0; D_MAX_BLOCKS_IN_MCU as usize],
            ss: 0,
            se: 0,
            ah: 0,
            al: 0,
            unread_marker: 0,
            master: Master::default(),
            inputctl: InputController { consume: ConsumeMode::Markers, has_multiple_scans: false, eoi_reached: false, inheaders: true },
            marker: MarkerReader::new(),
            entropy: Entropy { kind: EntropyKind::None, get_buffer: 0, bits_left: 0, last_dc_val: [0; MAX_COMPS_IN_SCAN], restarts_to_go: 0, insufficient_data: false },
            mcu_blocks: [[0; DCTSIZE2]; D_MAX_BLOCKS_IN_MCU as usize],
            coef: CoefKind::None,
            idct: IdctKind::None,
            upsample: None,
            cconvert: None,
            main: None,
        };
        decompress.reset_marker_reader();
        decompress
    }

    pub(crate) fn warn(&mut self, warning: Warning) {
        if self.count_corrupt_warnings && warning.is_corrupt_data() {
            self.corrupt_warnings += 1;
        }
    }

    pub(crate) fn byte_at(&self, position: usize) -> u8 {
        self.src.data()[position]
    }

    pub(crate) fn progress(&mut self) -> Result<()> {
        let scan = self.input_scan_number;
        match self.progress_monitor.as_mut() {
            Some(monitor) => monitor(scan),
            None => Ok(()),
        }
    }

    fn default_decompress_parms(&mut self) {
        match self.num_components {
            1 => {
                self.jpeg_color_space = ColorSpace::Grayscale;
                self.out_color_space = ColorSpace::Grayscale;
            }
            3 => {
                if self.saw_jfif_marker {
                    self.jpeg_color_space = ColorSpace::YCbCr;
                } else if self.saw_adobe_marker {
                    match self.adobe_transform {
                        0 => self.jpeg_color_space = ColorSpace::Rgb,
                        1 => self.jpeg_color_space = ColorSpace::YCbCr,
                        _ => {
                            self.warn(Warning::AdobeTransform);
                            self.jpeg_color_space = ColorSpace::YCbCr;
                        }
                    }
                } else {
                    let cid0 = self.comp_info[0].component_id;
                    let cid1 = self.comp_info[1].component_id;
                    let cid2 = self.comp_info[2].component_id;
                    if cid0 == 1 && cid1 == 2 && cid2 == 3 {
                        self.jpeg_color_space = if self.master.lossless { ColorSpace::Rgb } else { ColorSpace::YCbCr };
                    } else if cid0 == 82 && cid1 == 71 && cid2 == 66 {
                        self.jpeg_color_space = ColorSpace::Rgb;
                    } else {
                        self.jpeg_color_space = if self.master.lossless { ColorSpace::Rgb } else { ColorSpace::YCbCr };
                    }
                }
                self.out_color_space = ColorSpace::Rgb;
            }
            4 => {
                if self.saw_adobe_marker {
                    match self.adobe_transform {
                        0 => self.jpeg_color_space = ColorSpace::Cmyk,
                        2 => self.jpeg_color_space = ColorSpace::Ycck,
                        _ => {
                            self.warn(Warning::AdobeTransform);
                            self.jpeg_color_space = ColorSpace::Ycck;
                        }
                    }
                } else {
                    self.jpeg_color_space = ColorSpace::Cmyk;
                }
                self.out_color_space = ColorSpace::Cmyk;
            }
            _ => {
                self.jpeg_color_space = ColorSpace::Unknown;
                self.out_color_space = ColorSpace::Unknown;
            }
        }
        self.scale_num = 1;
        self.scale_denom = 1;
        self.buffered_image = false;
        self.do_fancy_upsampling = true;
        self.do_block_smoothing = true;
    }

    pub fn read_header(&mut self, require_image: bool) -> Result<ReadHeader> {
        if self.global_state != GlobalState::Start && self.global_state != GlobalState::InHeader {
            return Err(Error::BadState);
        }
        match self.consume_input()? {
            ConsumeStatus::ReachedSos => Ok(ReadHeader::HeaderOk),
            ConsumeStatus::ReachedEoi => {
                if require_image {
                    return Err(Error::NoImage);
                }
                self.abort();
                Ok(ReadHeader::TablesOnly)
            }
            _ => Ok(ReadHeader::Suspended),
        }
    }

    fn abort(&mut self) {
        self.global_state = GlobalState::Start;
        self.marker_list.clear();
    }

    pub fn consume_input(&mut self) -> Result<ConsumeStatus> {
        match self.global_state {
            GlobalState::Start | GlobalState::InHeader => {
                if self.global_state == GlobalState::Start {
                    self.reset_input_controller();
                    self.global_state = GlobalState::InHeader;
                }
                let status = self.consume()?;
                if status == ConsumeStatus::ReachedSos {
                    self.default_decompress_parms();
                    self.global_state = GlobalState::Ready;
                }
                Ok(status)
            }
            GlobalState::Ready => Ok(ConsumeStatus::ReachedSos),
            GlobalState::Preload | GlobalState::Prescan | GlobalState::Scanning | GlobalState::RawOk | GlobalState::BufImage | GlobalState::BufPost | GlobalState::Stopping => self.consume(),
        }
    }

    pub(crate) fn consume(&mut self) -> Result<ConsumeStatus> {
        match self.inputctl.consume {
            ConsumeMode::Markers => self.consume_markers(),
            ConsumeMode::Data => self.consume_data(),
        }
    }

    pub fn input_complete(&self) -> bool {
        self.inputctl.eoi_reached
    }

    pub fn has_multiple_scans(&self) -> Result<bool> {
        if self.global_state < GlobalState::Ready || self.global_state > GlobalState::Stopping {
            return Err(Error::BadState);
        }
        Ok(self.inputctl.has_multiple_scans)
    }

    pub fn finish_decompress(&mut self) -> Result<bool> {
        if (self.global_state == GlobalState::Scanning || self.global_state == GlobalState::RawOk) && !self.buffered_image {
            if self.output_scanline < self.output_height {
                return Err(Error::TooLittleData);
            }
            self.finish_output_pass();
            self.global_state = GlobalState::Stopping;
        } else if self.global_state == GlobalState::BufImage {
            self.global_state = GlobalState::Stopping;
        } else if self.global_state != GlobalState::Stopping {
            return Err(Error::BadState);
        }
        while !self.inputctl.eoi_reached {
            if self.consume()? == ConsumeStatus::Suspended {
                return Ok(false);
            }
        }
        self.src.term_source();
        self.abort();
        Ok(true)
    }

    pub fn calc_output_dimensions(&mut self) -> Result<()> {
        if self.global_state != GlobalState::Ready {
            return Err(Error::BadState);
        }
        if !self.master.lossless {
            let n = self.scale_num * DCTSIZE as u32;
            let mut size = 16u32;
            for candidate in 1..=15u32 {
                if n <= self.scale_denom * candidate {
                    size = candidate;
                    break;
                }
            }
            self.output_width = div_round_up(self.image_width as i64 * size as i64, DCTSIZE as i64) as u32;
            self.output_height = div_round_up(self.image_height as i64 * size as i64, DCTSIZE as i64) as u32;
            self.min_dct_scaled_size = size as i32;
            for ci in 0..self.num_components as usize {
                let compptr = &self.comp_info[ci];
                let mut ssize = self.min_dct_scaled_size;
                while ssize < DCTSIZE as i32
                    && (self.max_h_samp_factor * self.min_dct_scaled_size) % (compptr.h_samp_factor * ssize * 2) == 0
                    && (self.max_v_samp_factor * self.min_dct_scaled_size) % (compptr.v_samp_factor * ssize * 2) == 0
                {
                    ssize *= 2;
                }
                self.comp_info[ci].dct_scaled_size = ssize;
            }
            for ci in 0..self.num_components as usize {
                let compptr = &mut self.comp_info[ci];
                compptr.downsampled_width = div_round_up(self.image_width as i64 * (compptr.h_samp_factor * compptr.dct_scaled_size) as i64, (self.max_h_samp_factor * DCTSIZE as i32) as i64) as u32;
                compptr.downsampled_height = div_round_up(self.image_height as i64 * (compptr.v_samp_factor * compptr.dct_scaled_size) as i64, (self.max_v_samp_factor * DCTSIZE as i32) as i64) as u32;
            }
        } else {
            self.output_width = self.image_width;
            self.output_height = self.image_height;
        }
        self.out_color_components = match self.out_color_space {
            ColorSpace::Grayscale => 1,
            ColorSpace::Rgb | ColorSpace::YCbCr => 3,
            ColorSpace::ExtBgra => 4,
            ColorSpace::Cmyk | ColorSpace::Ycck => 4,
            ColorSpace::Unknown => self.num_components,
        };
        self.output_components = self.out_color_components;
        self.rec_outbuf_height = 1;
        Ok(())
    }

    fn master_selection(&mut self) -> Result<()> {
        if self.master.lossless {
            self.scale_num = 1;
            self.scale_denom = 1;
        }
        self.calc_output_dimensions()?;
        let samples_per_row = self.output_width as i64 * self.out_color_components as i64;
        if samples_per_row != samples_per_row as u32 as i64 {
            return Err(Error::WidthOverflow);
        }
        self.master.pass_number = 0;
        self.init_color_deconverter()?;
        self.init_upsampler()?;
        let full = self.inputctl.has_multiple_scans || self.buffered_image;
        if self.master.lossless {
            self.idct = IdctKind::Lossless(LosslessDecompressor::new(self.num_components as usize));
            if self.arith_code {
                return Err(Error::ArithNotImplemented);
            }
            self.init_lossless_huffman();
            self.coef = CoefKind::Lossless(DiffController::new(self, full));
        } else {
            self.idct = IdctKind::Lossy(Idct::new(self.num_components as usize));
            for compptr in self.comp_info.iter_mut() {
                compptr.dct_table = [0; DCTSIZE2];
            }
            if self.arith_code {
                return Err(Error::ArithNotImplemented);
            }
            if self.progressive_mode {
                self.init_progressive_huffman();
            } else {
                self.init_sequential_huffman();
            }
            self.coef = CoefKind::Lossy(CoefController::new(self, full));
        }
        self.init_main_controller()?;
        self.start_input_pass()?;
        self.master.first_imcu_col = 0;
        self.master.last_imcu_col = self.mcus_per_row.wrapping_sub(1);
        self.master.last_good_imcu_row = 0;
        Ok(())
    }

    fn prepare_for_output_pass(&mut self) -> Result<()> {
        self.idct_start_pass()?;
        self.coef_start_output_pass();
        self.upsample_start_pass();
        self.main_start_pass();
        Ok(())
    }

    fn finish_output_pass(&mut self) {
        self.master.pass_number += 1;
    }

    pub fn start_decompress(&mut self) -> Result<bool> {
        if self.global_state == GlobalState::Ready {
            self.master_selection()?;
            if self.buffered_image {
                self.global_state = GlobalState::BufImage;
                return Ok(true);
            }
            self.global_state = GlobalState::Preload;
        }
        if self.global_state == GlobalState::Preload {
            if self.inputctl.has_multiple_scans {
                loop {
                    self.progress()?;
                    let status = self.consume()?;
                    if status == ConsumeStatus::Suspended {
                        return Ok(false);
                    }
                    if status == ConsumeStatus::ReachedEoi {
                        break;
                    }
                }
            }
            self.output_scan_number = self.input_scan_number;
        } else if self.global_state != GlobalState::Prescan {
            return Err(Error::BadState);
        }
        self.output_pass_setup()
    }

    fn output_pass_setup(&mut self) -> Result<bool> {
        if self.global_state != GlobalState::Prescan {
            self.prepare_for_output_pass()?;
            self.output_scanline = 0;
            self.global_state = GlobalState::Prescan;
        }
        self.global_state = GlobalState::Scanning;
        Ok(true)
    }

    pub fn read_scanlines(&mut self, row: &mut [u8]) -> Result<u32> {
        if self.master.lossless {
            if self.data_precision > 8 || self.data_precision < 2 {
                return Err(Error::BadPrecision(self.data_precision));
            }
        } else if self.data_precision != 8 {
            return Err(Error::BadPrecision(self.data_precision));
        }
        if self.global_state != GlobalState::Scanning {
            return Err(Error::BadState);
        }
        if self.output_scanline >= self.output_height {
            self.warn(Warning::TooMuchData);
            return Ok(0);
        }
        self.progress()?;
        let mut row_ctr = 0;
        self.main_process_data(row, &mut row_ctr, 1)?;
        self.output_scanline += row_ctr;
        Ok(row_ctr)
    }

    pub fn start_output(&mut self, scan_number: i32) -> Result<bool> {
        if self.global_state != GlobalState::BufImage && self.global_state != GlobalState::Prescan {
            return Err(Error::BadState);
        }
        let mut scan_number = scan_number.max(1);
        if self.inputctl.eoi_reached && scan_number > self.input_scan_number {
            scan_number = self.input_scan_number;
        }
        self.output_scan_number = scan_number;
        self.output_pass_setup()
    }

    pub fn finish_output(&mut self) -> Result<bool> {
        if (self.global_state == GlobalState::Scanning || self.global_state == GlobalState::RawOk) && self.buffered_image {
            self.finish_output_pass();
            self.global_state = GlobalState::BufPost;
        } else if self.global_state != GlobalState::BufPost {
            return Err(Error::BadState);
        }
        while self.input_scan_number <= self.output_scan_number && !self.inputctl.eoi_reached {
            if self.consume()? == ConsumeStatus::Suspended {
                return Ok(false);
            }
        }
        self.global_state = GlobalState::BufImage;
        Ok(true)
    }

    fn initial_setup(&mut self) -> Result<()> {
        let data_unit: i32 = if self.master.lossless { 1 } else { DCTSIZE as i32 };
        if self.image_height > JPEG_MAX_DIMENSION || self.image_width > JPEG_MAX_DIMENSION {
            return Err(Error::ImageTooBig);
        }
        if self.master.lossless {
            if self.data_precision < 2 || self.data_precision > 16 {
                return Err(Error::BadPrecision(self.data_precision));
            }
        } else if self.data_precision != 8 && self.data_precision != 12 {
            return Err(Error::BadPrecision(self.data_precision));
        }
        if self.num_components > MAX_COMPONENTS {
            return Err(Error::BadComponentCount);
        }
        self.max_h_samp_factor = 1;
        self.max_v_samp_factor = 1;
        for compptr in &self.comp_info {
            if compptr.h_samp_factor <= 0 || compptr.h_samp_factor > MAX_SAMP_FACTOR || compptr.v_samp_factor <= 0 || compptr.v_samp_factor > MAX_SAMP_FACTOR {
                return Err(Error::BadSampling);
            }
            self.max_h_samp_factor = self.max_h_samp_factor.max(compptr.h_samp_factor);
            self.max_v_samp_factor = self.max_v_samp_factor.max(compptr.v_samp_factor);
        }
        self.min_dct_scaled_size = data_unit;
        for ci in 0..self.num_components as usize {
            let compptr = &mut self.comp_info[ci];
            compptr.dct_scaled_size = data_unit;
            compptr.width_in_blocks = div_round_up(self.image_width as i64 * compptr.h_samp_factor as i64, (self.max_h_samp_factor * data_unit) as i64) as u32;
            compptr.height_in_blocks = div_round_up(self.image_height as i64 * compptr.v_samp_factor as i64, (self.max_v_samp_factor * data_unit) as i64) as u32;
            self.master.first_mcu_col[ci] = 0;
            self.master.last_mcu_col[ci] = compptr.width_in_blocks.wrapping_sub(1);
            compptr.downsampled_width = div_round_up(self.image_width as i64 * compptr.h_samp_factor as i64, self.max_h_samp_factor as i64) as u32;
            compptr.downsampled_height = div_round_up(self.image_height as i64 * compptr.v_samp_factor as i64, self.max_v_samp_factor as i64) as u32;
            compptr.component_needed = true;
            compptr.quant_table = None;
        }
        self.total_imcu_rows = div_round_up(self.image_height as i64, (self.max_v_samp_factor * data_unit) as i64) as u32;
        self.inputctl.has_multiple_scans = self.comps_in_scan < self.num_components || self.progressive_mode;
        Ok(())
    }

    fn per_scan_setup(&mut self) -> Result<()> {
        let data_unit: i32 = if self.master.lossless { 1 } else { DCTSIZE as i32 };
        if self.comps_in_scan == 1 {
            let ci = self.cur_comp_info[0].expect("scan component");
            let compptr = &mut self.comp_info[ci];
            self.mcus_per_row = compptr.width_in_blocks;
            self.mcu_rows_in_scan = compptr.height_in_blocks;
            compptr.mcu_width = 1;
            compptr.mcu_height = 1;
            compptr.mcu_blocks = 1;
            compptr.mcu_sample_width = compptr.dct_scaled_size;
            compptr.last_col_width = 1;
            let mut tmp = (compptr.height_in_blocks % compptr.v_samp_factor as u32) as i32;
            if tmp == 0 {
                tmp = compptr.v_samp_factor;
            }
            compptr.last_row_height = tmp;
            self.blocks_in_mcu = 1;
            self.mcu_membership[0] = 0;
        } else {
            if self.comps_in_scan <= 0 || self.comps_in_scan > MAX_COMPS_IN_SCAN as i32 {
                return Err(Error::BadComponentCount);
            }
            self.mcus_per_row = div_round_up(self.image_width as i64, (self.max_h_samp_factor * data_unit) as i64) as u32;
            self.mcu_rows_in_scan = div_round_up(self.image_height as i64, (self.max_v_samp_factor * data_unit) as i64) as u32;
            self.blocks_in_mcu = 0;
            for ci in 0..self.comps_in_scan as usize {
                let index = self.cur_comp_info[ci].expect("scan component");
                let compptr = &mut self.comp_info[index];
                compptr.mcu_width = compptr.h_samp_factor;
                compptr.mcu_height = compptr.v_samp_factor;
                compptr.mcu_blocks = compptr.mcu_width * compptr.mcu_height;
                compptr.mcu_sample_width = compptr.mcu_width * compptr.dct_scaled_size;
                let mut tmp = (compptr.width_in_blocks % compptr.mcu_width as u32) as i32;
                if tmp == 0 {
                    tmp = compptr.mcu_width;
                }
                compptr.last_col_width = tmp;
                let mut tmp = (compptr.height_in_blocks % compptr.mcu_height as u32) as i32;
                if tmp == 0 {
                    tmp = compptr.mcu_height;
                }
                compptr.last_row_height = tmp;
                let mut mcublks = compptr.mcu_blocks;
                if self.blocks_in_mcu + mcublks > D_MAX_BLOCKS_IN_MCU {
                    return Err(Error::BadMcuSize);
                }
                while mcublks > 0 {
                    self.mcu_membership[self.blocks_in_mcu as usize] = ci as i32;
                    self.blocks_in_mcu += 1;
                    mcublks -= 1;
                }
            }
        }
        Ok(())
    }

    fn latch_quant_tables(&mut self) -> Result<()> {
        for ci in 0..self.comps_in_scan as usize {
            let index = self.cur_comp_info[ci].expect("scan component");
            if self.comp_info[index].quant_table.is_some() {
                continue;
            }
            let qtblno = self.comp_info[index].quant_tbl_no;
            if !(0..NUM_QUANT_TBLS as i32).contains(&qtblno) || self.quant_tbl_ptrs[qtblno as usize].is_none() {
                return Err(Error::NoQuantTable(qtblno));
            }
            self.comp_info[index].quant_table = self.quant_tbl_ptrs[qtblno as usize];
        }
        Ok(())
    }

    pub(crate) fn start_input_pass(&mut self) -> Result<()> {
        self.per_scan_setup()?;
        if !self.master.lossless {
            self.latch_quant_tables()?;
        }
        self.entropy_start_pass()?;
        self.coef_start_input_pass()?;
        self.inputctl.consume = ConsumeMode::Data;
        Ok(())
    }

    fn entropy_start_pass(&mut self) -> Result<()> {
        match self.entropy.kind {
            EntropyKind::Sequential { .. } => self.start_pass_huff_decoder(),
            EntropyKind::Progressive { .. } => self.start_pass_phuff_decoder(),
            EntropyKind::Lossless { .. } => self.start_pass_lhuff_decoder(),
            EntropyKind::None => Ok(()),
        }
    }

    pub(crate) fn finish_input_pass(&mut self) {
        self.inputctl.consume = ConsumeMode::Markers;
    }

    fn consume_markers(&mut self) -> Result<ConsumeStatus> {
        if self.inputctl.eoi_reached {
            return Ok(ConsumeStatus::ReachedEoi);
        }
        let status = self.read_markers()?;
        match status {
            ConsumeStatus::ReachedSos => {
                if self.inputctl.inheaders {
                    self.initial_setup()?;
                    self.inputctl.inheaders = false;
                } else {
                    if !self.inputctl.has_multiple_scans {
                        return Err(Error::EoiExpected);
                    }
                    self.start_input_pass()?;
                }
            }
            ConsumeStatus::ReachedEoi => {
                self.inputctl.eoi_reached = true;
                if self.inputctl.inheaders {
                    if self.marker.saw_sof {
                        return Err(Error::SofNoSos);
                    }
                } else if self.output_scan_number > self.input_scan_number {
                    self.output_scan_number = self.input_scan_number;
                }
            }
            _ => {}
        }
        Ok(status)
    }

    fn reset_input_controller(&mut self) {
        self.inputctl.consume = ConsumeMode::Markers;
        self.inputctl.has_multiple_scans = false;
        self.inputctl.eoi_reached = false;
        self.inputctl.inheaders = true;
        self.reset_marker_reader();
        self.coef_bits = None;
    }

    pub(crate) fn comp(&self, scan_index: usize) -> &Component {
        &self.comp_info[self.cur_comp_info[scan_index].expect("scan component")]
    }
}

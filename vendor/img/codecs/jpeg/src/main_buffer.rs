use super::decompress::{Decompress, Result};
use super::error::Error;
use super::source::Source;

pub(crate) struct Plane {
    pub data: Vec<u8>,
    pub stride: usize,
}

impl Plane {
    pub fn new(width: usize, rows: usize) -> Self {
        Plane { data: vec![0; width * rows], stride: width }
    }

    pub fn row(&self, row: usize) -> &[u8] {
        &self.data[row * self.stride..(row + 1) * self.stride]
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Lists {
    Identity,
    X(usize),
}

const CTX_PREPARE_FOR_IMCU: u8 = 0;
const CTX_PROCESS_IMCU: u8 = 1;
const CTX_POSTPONED_ROW: u8 = 2;

pub(crate) struct MainController {
    pub planes: Vec<Plane>,
    pub rgroup: Vec<usize>,
    identity: Vec<Vec<usize>>,
    xbuffer: [Vec<Vec<usize>>; 2],
    context: bool,
    buffer_full: bool,
    rowgroup_ctr: u32,
    whichptr: usize,
    context_state: u8,
    rowgroups_avail: u32,
    imcu_row_ctr: u32,
}

impl MainController {
    pub fn row_index(&self, lists: Lists, ci: usize, index: isize) -> usize {
        match lists {
            Lists::Identity => self.identity[ci][index as usize],
            Lists::X(which) => self.xbuffer[which][ci][(index + self.rgroup[ci] as isize) as usize],
        }
    }

    pub fn rows(&self, lists: Lists, ci: usize, start: isize, count: usize) -> Vec<usize> {
        (0..count).map(|i| self.row_index(lists, ci, start + i as isize)).collect()
    }

    fn set_x(&mut self, which: usize, ci: usize, index: isize, value: usize) {
        let bias = self.rgroup[ci] as isize;
        self.xbuffer[which][ci][(index + bias) as usize] = value;
    }

    fn get_x(&self, which: usize, ci: usize, index: isize) -> usize {
        self.xbuffer[which][ci][(index + self.rgroup[ci] as isize) as usize]
    }
}

impl<S: Source> Decompress<S> {
    pub(crate) fn init_main_controller(&mut self) -> Result<()> {
        let context = self.upsample.as_ref().expect("upsampler").need_context_rows;
        let m = self.min_dct_scaled_size as usize;
        let ngroups = if context {
            if self.min_dct_scaled_size < 2 {
                return Err(Error::NotImplemented);
            }
            m + 2
        } else {
            m
        };
        let mut planes = Vec::new();
        let mut rgroups = Vec::new();
        let mut identity = Vec::new();
        let mut x0 = Vec::new();
        let mut x1 = Vec::new();
        for compptr in &self.comp_info {
            let rgroup = (compptr.v_samp_factor * compptr.dct_scaled_size / self.min_dct_scaled_size) as usize;
            let rows = rgroup * ngroups;
            planes.push(Plane::new(compptr.width_in_blocks as usize * compptr.dct_scaled_size as usize, rows));
            identity.push((0..rows).collect());
            rgroups.push(rgroup);
            if context {
                x0.push(vec![0usize; rgroup * (m + 4)]);
                x1.push(vec![0usize; rgroup * (m + 4)]);
            }
        }
        self.main = Some(MainController {
            planes,
            rgroup: rgroups,
            identity,
            xbuffer: [x0, x1],
            context,
            buffer_full: false,
            rowgroup_ctr: 0,
            whichptr: 0,
            context_state: CTX_PREPARE_FOR_IMCU,
            rowgroups_avail: 0,
            imcu_row_ctr: 0,
        });
        Ok(())
    }

    fn make_funny_pointers(&mut self) {
        let m = self.min_dct_scaled_size as isize;
        let main = self.main.as_mut().expect("main controller");
        for ci in 0..self.comp_info.len() {
            let rgroup = main.rgroup[ci] as isize;
            for i in 0..rgroup * (m + 2) {
                let value = main.identity[ci][i as usize];
                main.set_x(0, ci, i, value);
                main.set_x(1, ci, i, value);
            }
            for i in 0..rgroup * 2 {
                let a = main.identity[ci][(rgroup * m + i) as usize];
                let b = main.identity[ci][(rgroup * (m - 2) + i) as usize];
                main.set_x(1, ci, rgroup * (m - 2) + i, a);
                main.set_x(1, ci, rgroup * m + i, b);
            }
            for i in 0..rgroup {
                let first = main.get_x(0, ci, 0);
                main.set_x(0, ci, i - rgroup, first);
            }
        }
    }

    fn set_wraparound_pointers(&mut self) {
        let m = self.min_dct_scaled_size as isize;
        let main = self.main.as_mut().expect("main controller");
        for ci in 0..self.comp_info.len() {
            let rgroup = main.rgroup[ci] as isize;
            for i in 0..rgroup {
                let v = main.get_x(0, ci, rgroup * (m + 1) + i);
                main.set_x(0, ci, i - rgroup, v);
                let v = main.get_x(1, ci, rgroup * (m + 1) + i);
                main.set_x(1, ci, i - rgroup, v);
                let v = main.get_x(0, ci, i);
                main.set_x(0, ci, rgroup * (m + 2) + i, v);
                let v = main.get_x(1, ci, i);
                main.set_x(1, ci, rgroup * (m + 2) + i, v);
            }
        }
    }

    fn set_bottom_pointers(&mut self) {
        let min = self.min_dct_scaled_size;
        let main = self.main.as_mut().expect("main controller");
        let which = main.whichptr;
        for (ci, compptr) in self.comp_info.iter().enumerate() {
            let imcu_height = compptr.v_samp_factor * compptr.dct_scaled_size;
            let rgroup = (imcu_height / min) as isize;
            let mut rows_left = (compptr.downsampled_height % imcu_height as u32) as isize;
            if rows_left == 0 {
                rows_left = imcu_height as isize;
            }
            if ci == 0 {
                main.rowgroups_avail = ((rows_left - 1) / rgroup + 1) as u32;
            }
            for i in 0..rgroup * 2 {
                let v = main.get_x(which, ci, rows_left - 1);
                main.set_x(which, ci, rows_left + i, v);
            }
        }
    }

    pub(crate) fn main_start_pass(&mut self) {
        let context = self.main.as_ref().expect("main controller").context;
        if context {
            self.make_funny_pointers();
            let main = self.main.as_mut().expect("main controller");
            main.whichptr = 0;
            main.context_state = CTX_PREPARE_FOR_IMCU;
            main.imcu_row_ctr = 0;
        }
        let main = self.main.as_mut().expect("main controller");
        main.buffer_full = false;
        main.rowgroup_ctr = 0;
    }

    pub(crate) fn main_process_data(&mut self, output: &mut [u8], out_row_ctr: &mut u32, out_rows_avail: u32) -> Result<()> {
        if self.main.as_ref().expect("main controller").context {
            self.process_data_context_main(output, out_row_ctr, out_rows_avail)
        } else {
            self.process_data_simple_main(output, out_row_ctr, out_rows_avail)
        }
    }

    fn process_data_simple_main(&mut self, output: &mut [u8], out_row_ctr: &mut u32, out_rows_avail: u32) -> Result<()> {
        if !self.main.as_ref().expect("main controller").buffer_full {
            if !self.decompress_data(Lists::Identity)? {
                return Ok(());
            }
            self.main.as_mut().expect("main controller").buffer_full = true;
        }
        let rowgroups_avail = self.min_dct_scaled_size as u32;
        let mut rowgroup_ctr = self.main.as_ref().expect("main controller").rowgroup_ctr;
        self.sep_upsample(Lists::Identity, &mut rowgroup_ctr, output, out_row_ctr, out_rows_avail);
        let main = self.main.as_mut().expect("main controller");
        main.rowgroup_ctr = rowgroup_ctr;
        if main.rowgroup_ctr >= rowgroups_avail {
            main.buffer_full = false;
            main.rowgroup_ctr = 0;
        }
        Ok(())
    }

    fn process_data_context_main(&mut self, output: &mut [u8], out_row_ctr: &mut u32, out_rows_avail: u32) -> Result<()> {
        let which = self.main.as_ref().expect("main controller").whichptr;
        if !self.main.as_ref().expect("main controller").buffer_full {
            if !self.decompress_data(Lists::X(which))? {
                return Ok(());
            }
            let main = self.main.as_mut().expect("main controller");
            main.buffer_full = true;
            main.imcu_row_ctr += 1;
        }
        let m = self.min_dct_scaled_size as u32;
        let mut state = self.main.as_ref().expect("main controller").context_state;
        if state == CTX_POSTPONED_ROW {
            let which = self.main.as_ref().expect("main controller").whichptr;
            let mut ctr = self.main.as_ref().expect("main controller").rowgroup_ctr;
            self.sep_upsample(Lists::X(which), &mut ctr, output, out_row_ctr, out_rows_avail);
            let main = self.main.as_mut().expect("main controller");
            main.rowgroup_ctr = ctr;
            if main.rowgroup_ctr < main.rowgroups_avail {
                return Ok(());
            }
            main.context_state = CTX_PREPARE_FOR_IMCU;
            if *out_row_ctr >= out_rows_avail {
                return Ok(());
            }
            state = CTX_PREPARE_FOR_IMCU;
        }
        if state == CTX_PREPARE_FOR_IMCU {
            let total = self.total_imcu_rows;
            let main = self.main.as_mut().expect("main controller");
            main.rowgroup_ctr = 0;
            main.rowgroups_avail = m - 1;
            if main.imcu_row_ctr == total {
                self.set_bottom_pointers();
            }
            self.main.as_mut().expect("main controller").context_state = CTX_PROCESS_IMCU;
        }
        let which = self.main.as_ref().expect("main controller").whichptr;
        let mut ctr = self.main.as_ref().expect("main controller").rowgroup_ctr;
        self.sep_upsample(Lists::X(which), &mut ctr, output, out_row_ctr, out_rows_avail);
        let main = self.main.as_mut().expect("main controller");
        main.rowgroup_ctr = ctr;
        if main.rowgroup_ctr < main.rowgroups_avail {
            return Ok(());
        }
        if main.imcu_row_ctr == 1 {
            self.set_wraparound_pointers();
        }
        let main = self.main.as_mut().expect("main controller");
        main.whichptr ^= 1;
        main.buffer_full = false;
        main.rowgroup_ctr = m + 1;
        main.rowgroups_avail = m + 2;
        main.context_state = CTX_POSTPONED_ROW;
        Ok(())
    }
}

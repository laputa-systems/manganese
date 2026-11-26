use super::decompress::{Decompress, Result};
use super::error::Error;
use super::main_buffer::{Lists, MainController, Plane};
use super::source::Source;
use super::tables::round_up;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Method {
    Noop,
    Fullsize,
    H2v1,
    H2v2,
    H2v1Fancy,
    H2v2Fancy,
    H1v2Fancy,
    Int,
}

#[derive(Clone, Copy)]
pub(crate) enum ColorSource {
    Missing,
    Own,
    Input { lists: Lists, base: isize },
}

pub(crate) struct Upsampler {
    pub need_context_rows: bool,
    methods: Vec<Method>,
    pub color_buf: Vec<Option<Plane>>,
    pub color_src: Vec<ColorSource>,
    next_row_out: i32,
    rows_to_go: u32,
    rowgroup_height: Vec<i32>,
    h_expand: Vec<usize>,
    v_expand: Vec<usize>,
}

struct Input<'a> {
    main: &'a MainController,
    lists: Lists,
    ci: usize,
    base: isize,
}

impl Input<'_> {
    fn row(&self, index: isize) -> &[u8] {
        self.main.planes[self.ci].row(self.main.row_index(self.lists, self.ci, self.base + index))
    }
}

fn copy_row(plane: &mut Plane, from: usize, to: usize, width: usize) {
    let stride = plane.stride;
    plane.data.copy_within(from * stride..from * stride + width, to * stride);
}

impl<S: Source> Decompress<S> {
    pub(crate) fn init_upsampler(&mut self) -> Result<()> {
        let do_fancy = self.do_fancy_upsampling && self.min_dct_scaled_size > 1;
        let n = self.num_components as usize;
        let mut upsampler = Upsampler {
            need_context_rows: false,
            methods: vec![Method::Noop; n],
            color_buf: (0..n).map(|_| None).collect(),
            color_src: vec![ColorSource::Missing; n],
            next_row_out: 0,
            rows_to_go: 0,
            rowgroup_height: vec![0; n],
            h_expand: vec![0; n],
            v_expand: vec![0; n],
        };
        for ci in 0..n {
            let compptr = &self.comp_info[ci];
            let h_in_group = compptr.h_samp_factor * compptr.dct_scaled_size / self.min_dct_scaled_size;
            let v_in_group = compptr.v_samp_factor * compptr.dct_scaled_size / self.min_dct_scaled_size;
            let h_out_group = self.max_h_samp_factor;
            let v_out_group = self.max_v_samp_factor;
            upsampler.rowgroup_height[ci] = v_in_group;
            let mut need_buffer = true;
            let method = if !compptr.component_needed {
                need_buffer = false;
                Method::Noop
            } else if h_in_group == h_out_group && v_in_group == v_out_group {
                need_buffer = false;
                Method::Fullsize
            } else if h_in_group * 2 == h_out_group && v_in_group == v_out_group {
                if do_fancy && compptr.downsampled_width > 2 { Method::H2v1Fancy } else { Method::H2v1 }
            } else if h_in_group == h_out_group && v_in_group * 2 == v_out_group && do_fancy {
                upsampler.need_context_rows = true;
                Method::H1v2Fancy
            } else if h_in_group * 2 == h_out_group && v_in_group * 2 == v_out_group {
                if do_fancy && compptr.downsampled_width > 2 {
                    upsampler.need_context_rows = true;
                    Method::H2v2Fancy
                } else {
                    Method::H2v2
                }
            } else if h_out_group % h_in_group == 0 && v_out_group % v_in_group == 0 {
                upsampler.h_expand[ci] = (h_out_group / h_in_group) as usize;
                upsampler.v_expand[ci] = (v_out_group / v_in_group) as usize;
                Method::Int
            } else {
                return Err(Error::FractionalSamplingNotImplemented);
            };
            upsampler.methods[ci] = method;
            if need_buffer {
                let width = round_up(self.output_width as i64, self.max_h_samp_factor as i64) as usize;
                upsampler.color_buf[ci] = Some(Plane::new(width, self.max_v_samp_factor as usize));
                upsampler.color_src[ci] = ColorSource::Own;
            }
        }
        self.upsample = Some(upsampler);
        Ok(())
    }

    pub(crate) fn upsample_start_pass(&mut self) {
        let upsampler = self.upsample.as_mut().expect("upsampler");
        upsampler.next_row_out = self.max_v_samp_factor;
        upsampler.rows_to_go = self.output_height;
    }

    pub(crate) fn sep_upsample(&mut self, lists: Lists, in_row_group_ctr: &mut u32, output: &mut [u8], out_row_ctr: &mut u32, out_rows_avail: u32) {
        let max_v = self.max_v_samp_factor;
        let output_width = self.output_width as usize;
        let main = self.main.as_ref().expect("main controller");
        let upsampler = self.upsample.as_mut().expect("upsampler");
        if upsampler.next_row_out >= max_v {
            for ci in 0..self.comp_info.len() {
                let compptr = &self.comp_info[ci];
                let input = Input { main, lists, ci, base: *in_row_group_ctr as isize * upsampler.rowgroup_height[ci] as isize };
                match upsampler.methods[ci] {
                    Method::Noop => upsampler.color_src[ci] = ColorSource::Missing,
                    Method::Fullsize => upsampler.color_src[ci] = ColorSource::Input { lists, base: input.base },
                    method => {
                        let plane = upsampler.color_buf[ci].as_mut().expect("upsample buffer");
                        upsample_component(method, &input, plane, max_v as usize, output_width, compptr.downsampled_width as usize, upsampler.h_expand[ci], upsampler.v_expand[ci]);
                    }
                }
            }
            upsampler.next_row_out = 0;
        }
        let mut num_rows = (max_v - upsampler.next_row_out) as u32;
        if num_rows > upsampler.rows_to_go {
            num_rows = upsampler.rows_to_go;
        }
        let avail = out_rows_avail - *out_row_ctr;
        if num_rows > avail {
            num_rows = avail;
        }
        let start = upsampler.next_row_out;
        for k in 0..num_rows as usize {
            let rows: Vec<&[u8]> = (0..self.comp_info.len())
                .map(|ci| match upsampler.color_src[ci] {
                    ColorSource::Missing => &[][..],
                    ColorSource::Own => upsampler.color_buf[ci].as_ref().expect("upsample buffer").row(start as usize + k),
                    ColorSource::Input { lists, base } => main.planes[ci].row(main.row_index(lists, ci, base + start as isize + k as isize)),
                })
                .collect();
            let cconvert = self.cconvert.as_ref().expect("color deconverter");
            let width = output.len() / out_rows_avail as usize;
            let row = (*out_row_ctr as usize + k) * width;
            cconvert.convert(&rows, &mut output[row..row + width], output_width);
        }
        *out_row_ctr += num_rows;
        upsampler.rows_to_go -= num_rows;
        upsampler.next_row_out += num_rows as i32;
        if upsampler.next_row_out >= max_v {
            *in_row_group_ctr += 1;
        }
    }
}

#[expect(clippy::too_many_arguments, reason = "libjpeg's upsample signature is the ported interface")]
fn upsample_component(method: Method, input: &Input, output: &mut Plane, max_v: usize, output_width: usize, downsampled_width: usize, h_expand: usize, v_expand: usize) {
    match method {
        Method::H2v1 => {
            for inrow in 0..max_v {
                let inptr = input.row(inrow as isize);
                let stride = output.stride;
                let out = &mut output.data[inrow * stride..(inrow + 1) * stride];
                let mut o = 0;
                let mut i = 0;
                while o < output_width {
                    out[o] = inptr[i];
                    out[o + 1] = inptr[i];
                    o += 2;
                    i += 1;
                }
            }
        }
        Method::H2v2 => {
            let mut inrow = 0;
            let mut outrow = 0;
            while outrow < max_v {
                let inptr = input.row(inrow as isize);
                let stride = output.stride;
                {
                    let out = &mut output.data[outrow * stride..(outrow + 1) * stride];
                    let mut o = 0;
                    let mut i = 0;
                    while o < output_width {
                        out[o] = inptr[i];
                        out[o + 1] = inptr[i];
                        o += 2;
                        i += 1;
                    }
                }
                copy_row(output, outrow, outrow + 1, output_width);
                inrow += 1;
                outrow += 2;
            }
        }
        Method::Int => {
            let mut inrow = 0;
            let mut outrow = 0;
            while outrow < max_v {
                let inptr = input.row(inrow as isize);
                let stride = output.stride;
                {
                    let out = &mut output.data[outrow * stride..(outrow + 1) * stride];
                    let mut o = 0;
                    let mut i = 0;
                    while o < output_width {
                        let value = inptr[i];
                        i += 1;
                        for _ in 0..h_expand {
                            out[o] = value;
                            o += 1;
                        }
                    }
                }
                for extra in 1..v_expand {
                    copy_row(output, outrow, outrow + extra, output_width);
                }
                inrow += 1;
                outrow += v_expand;
            }
        }
        Method::H2v1Fancy => {
            for inrow in 0..max_v {
                let inptr = input.row(inrow as isize);
                let stride = output.stride;
                let out = &mut output.data[inrow * stride..(inrow + 1) * stride];
                let mut invalue = inptr[0] as i32;
                out[0] = invalue as u8;
                out[1] = ((invalue * 3 + inptr[1] as i32 + 2) >> 2) as u8;
                let mut o = 2;
                let mut i = 1;
                for _ in 0..downsampled_width - 2 {
                    invalue = inptr[i] as i32 * 3;
                    out[o] = ((invalue + inptr[i - 1] as i32 + 1) >> 2) as u8;
                    out[o + 1] = ((invalue + inptr[i + 1] as i32 + 2) >> 2) as u8;
                    o += 2;
                    i += 1;
                }
                invalue = inptr[i] as i32;
                out[o] = ((invalue * 3 + inptr[i - 1] as i32 + 1) >> 2) as u8;
                out[o + 1] = invalue as u8;
            }
        }
        Method::H1v2Fancy => {
            let mut inrow = 0isize;
            let mut outrow = 0;
            while outrow < max_v {
                for v in 0..2 {
                    let inptr0 = input.row(inrow);
                    let (inptr1, bias) = if v == 0 { (input.row(inrow - 1), 1) } else { (input.row(inrow + 1), 2) };
                    let stride = output.stride;
                    let out = &mut output.data[outrow * stride..(outrow + 1) * stride];
                    outrow += 1;
                    for col in 0..downsampled_width {
                        let sum = inptr0[col] as i32 * 3 + inptr1[col] as i32;
                        out[col] = ((sum + bias) >> 2) as u8;
                    }
                }
                inrow += 1;
            }
        }
        Method::H2v2Fancy => {
            let mut inrow = 0isize;
            let mut outrow = 0;
            while outrow < max_v {
                for v in 0..2 {
                    let inptr0 = input.row(inrow);
                    let inptr1 = if v == 0 { input.row(inrow - 1) } else { input.row(inrow + 1) };
                    let stride = output.stride;
                    let out = &mut output.data[outrow * stride..(outrow + 1) * stride];
                    outrow += 1;
                    let mut thiscolsum = inptr0[0] as i32 * 3 + inptr1[0] as i32;
                    let mut nextcolsum = inptr0[1] as i32 * 3 + inptr1[1] as i32;
                    out[0] = ((thiscolsum * 4 + 8) >> 4) as u8;
                    out[1] = ((thiscolsum * 3 + nextcolsum + 7) >> 4) as u8;
                    let mut lastcolsum = thiscolsum;
                    thiscolsum = nextcolsum;
                    let mut o = 2;
                    for i in 2..downsampled_width {
                        nextcolsum = inptr0[i] as i32 * 3 + inptr1[i] as i32;
                        out[o] = ((thiscolsum * 3 + lastcolsum + 8) >> 4) as u8;
                        out[o + 1] = ((thiscolsum * 3 + nextcolsum + 7) >> 4) as u8;
                        o += 2;
                        lastcolsum = thiscolsum;
                        thiscolsum = nextcolsum;
                    }
                    out[o] = ((thiscolsum * 3 + lastcolsum + 8) >> 4) as u8;
                    out[o + 1] = ((thiscolsum * 4 + 7) >> 4) as u8;
                }
                inrow += 1;
            }
        }
        Method::Noop | Method::Fullsize => {}
    }
}

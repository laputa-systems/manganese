use img_png::chromium::{Level, Writer};
use std::cell::RefCell;
use std::io::{self, Write};
use std::rc::Rc;

#[derive(Clone, Copy, Eq, PartialEq)]
enum Alpha {
    Premul,
    Opaque,
    Unpremul,
}

#[derive(Clone, Default)]
struct Sink(Rc<RefCell<Vec<u8>>>);

impl Write for Sink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn unpremul(reciprocal: f32, channel: u8) -> u8 {
    let normalized = f32::from(channel) * (1.0f32 / 255.0f32);
    let answer = (normalized * reciprocal * 255.0f32).min(255.0f32);
    answer.round_ties_even() as u32 as u8
}

fn row_to_rgba(row: &[u8], alpha: Alpha) -> Vec<u8> {
    let mut out = Vec::with_capacity(row.len());
    for pixel in row.as_chunks::<4>().0 {
        let (b, g, r, a) = (pixel[0], pixel[1], pixel[2], pixel[3]);
        match alpha {
            Alpha::Premul => {
                let normalized_alpha = f32::from(a) * (1.0f32 / 255.0f32);
                let reciprocal = if normalized_alpha != 0.0 { 1.0f32 / normalized_alpha } else { 0.0 };
                out.extend_from_slice(&[unpremul(reciprocal, r), unpremul(reciprocal, g), unpremul(reciprocal, b), a]);
            }
            Alpha::Unpremul => out.extend_from_slice(&[r, g, b, a]),
            Alpha::Opaque => out.extend_from_slice(&[r, g, b]),
        }
    }
    out
}

pub fn encode(width: u32, height: u32, mode: u32, pixels: &[u8]) -> Option<Vec<u8>> {
    let kind = match mode % 3 {
        0 => Alpha::Premul,
        1 => Alpha::Opaque,
        _ => Alpha::Unpremul,
    };
    let operation = mode / 3;
    let level = match operation {
        0 | 4 => Level::Level1WithUpFilter,
        1 | 3 => Level::Balanced,
        _ => Level::High,
    };
    let kind = if operation >= 3 && kind != Alpha::Opaque && pixels.as_chunks::<4>().0.iter().all(|pixel| pixel[3] == 255) {
        Alpha::Opaque
    } else {
        kind
    };
    let color = if kind == Alpha::Opaque { png_codec::ColorType::Rgb } else { png_codec::ColorType::Rgba };
    let sink = Sink::default();
    let mut writer = Writer::new(sink.clone(), width, height, color, png_codec::BitDepth::Eight, level, &[]).ok()?;
    for row in pixels.chunks_exact(width as usize * 4) {
        writer.write_row(&row_to_rgba(row, kind)).ok()?;
    }
    writer.finish().ok()?;
    let bytes = sink.0.borrow().clone();
    Some(bytes)
}

#![allow(missing_docs)]

use std::borrow::Cow;
use std::io::{BufReader, ErrorKind, Read, Seek, Write};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    Success,
    FormatError,
    ParameterError,
    LimitsExceeded,
    IncompleteInput,
    OtherIoError,
}

impl From<&png::DecodingError> for Outcome {
    fn from(error: &png::DecodingError) -> Self {
        match error {
            png::DecodingError::IoError(error) if error.kind() == ErrorKind::UnexpectedEof => Outcome::IncompleteInput,
            png::DecodingError::IoError(_) => Outcome::OtherIoError,
            png::DecodingError::Format(_) => Outcome::FormatError,
            png::DecodingError::Parameter(_) => Outcome::ParameterError,
            png::DecodingError::LimitsExceeded => Outcome::LimitsExceeded,
        }
    }
}

fn transformations(info: &png::Info) -> png::Transformations {
    let mut result = match info.color_type {
        png::ColorType::Rgba | png::ColorType::GrayscaleAlpha if (info.bit_depth as u8) < 8 => png::Transformations::EXPAND,
        png::ColorType::Rgb | png::ColorType::Grayscale => png::Transformations::EXPAND,
        png::ColorType::Indexed | png::ColorType::Rgba | png::ColorType::GrayscaleAlpha => png::Transformations::IDENTITY,
    };
    if info.bit_depth == png::BitDepth::Sixteen && matches!(info.color_type, png::ColorType::Grayscale | png::ColorType::GrayscaleAlpha) {
        result |= png::Transformations::STRIP_16;
    }
    result
}

pub struct Reader<R: Read + Seek> {
    reader: png::Reader<BufReader<R>>,
    last_interlace_info: Option<png::InterlaceInfo>,
}

impl<R: Read + Seek> Reader<R> {
    pub fn new(input: R) -> Result<Self, Outcome> {
        let mut options = png::DecodeOptions::default();
        options.set_ignore_text_chunk(true);
        let mut decoder = png::Decoder::new_with_options(BufReader::with_capacity(32 * 1024, input), options);
        let info = decoder.read_header_info().map_err(|error| Outcome::from(&error))?;
        let transformations = transformations(info);
        decoder.set_transformations(transformations);
        let reader = decoder.read_info().map_err(|error| Outcome::from(&error))?;
        Ok(Self { reader, last_interlace_info: None })
    }

    pub fn info(&self) -> &png::Info<'static> {
        self.reader.info()
    }

    pub fn output_color_type(&self) -> (png::ColorType, png::BitDepth) {
        self.reader.output_color_type()
    }

    pub fn next_frame_info(&mut self) -> Outcome {
        match self.reader.next_frame_info() {
            Ok(_) => Outcome::Success,
            Err(error) => Outcome::from(&error),
        }
    }

    pub fn next_interlaced_row(&mut self) -> Result<Option<&[u8]>, Outcome> {
        match self.reader.next_interlaced_row() {
            Ok(Some(row)) => {
                self.last_interlace_info = Some(*row.interlace());
                Ok(Some(row.data()))
            }
            Ok(None) => {
                self.last_interlace_info = None;
                Ok(None)
            }
            Err(error) => Err(Outcome::from(&error)),
        }
    }

    pub fn expand_last_interlaced_row(&self, image: &mut [u8], stride: usize, row: &[u8], bits_per_pixel: u8) {
        let Some(png::InterlaceInfo::Adam7(info)) = self.last_interlace_info.as_ref() else {
            panic!("expand_last_interlaced_row needs an interlaced row");
        };
        png::expand_interlaced_row(image, stride, row, info, bits_per_pixel);
    }

    pub fn splat_last_interlaced_row(&self, image: &mut [u8], stride: usize, row: &[u8], bits_per_pixel: u8) {
        let Some(png::InterlaceInfo::Adam7(info)) = self.last_interlace_info.as_ref() else {
            panic!("splat_last_interlaced_row needs an interlaced row");
        };
        png::splat_interlaced_row(image, stride, row, info, bits_per_pixel);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Level {
    Level1WithUpFilter,
    Fastest,
    Fast,
    Balanced,
    High,
}

pub struct Writer<W: Write + 'static> {
    inner: png::StreamWriter<'static, W>,
}

impl<W: Write + 'static> Writer<W> {
    pub fn new(output: W, width: u32, height: u32, color: png::ColorType, depth: png::BitDepth, level: Level, icc: &[u8]) -> Result<Self, png::EncodingError> {
        let mut info = png::Info::with_size(width, height);
        info.color_type = color;
        info.bit_depth = depth;
        if !icc.is_empty() {
            info.icc_profile = Some(Cow::Owned(icc.to_owned()));
        }
        let mut encoder = png::Encoder::with_info(output, info)?;
        match level {
            Level::Level1WithUpFilter => {
                encoder.set_deflate_compression(png::DeflateCompression::Level(1));
                encoder.set_filter(png::Filter::Up);
            }
            Level::Fastest => encoder.set_compression(png::Compression::Fastest),
            Level::Fast => encoder.set_compression(png::Compression::Fast),
            Level::Balanced => encoder.set_compression(png::Compression::Balanced),
            Level::High => encoder.set_compression(png::Compression::High),
        }
        let writer = encoder.write_header()?;
        Ok(Self { inner: writer.into_stream_writer()? })
    }

    pub fn write_row(&mut self, row: &[u8]) -> Result<(), png::EncodingError> {
        self.inner.write_all(row).map_err(png::EncodingError::IoError)
    }

    pub fn finish(self) -> Result<(), png::EncodingError> {
        self.inner.finish()
    }
}

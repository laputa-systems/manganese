//! Pure-Rust JPEG decoder matching Chromium's libjpeg-turbo behavior for the
//! Manganese jpeg-decode box. The public state mirrors libjpeg's decompression
//! struct so the browser policy adapter can read header and progress fields.
#![expect(missing_docs, reason = "this snapshot mirrors libjpeg's public state; the adapter reads its fields directly")]

mod coefficients;
mod color;
mod decompress;
mod error;
mod huffman;
mod idct;
mod lossless;
mod main_buffer;
mod marker;
mod progressive;
mod sequential;
mod source;
mod tables;
mod upsample;

pub use decompress::{ColorSpace, Component, ConsumeStatus, Decompress, ReadHeader, SavedMarker};
pub use error::{Error, Warning};
pub use source::{Source, SourceState};

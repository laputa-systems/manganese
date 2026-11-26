use std::fmt;

use zlib_rs::Deflate;
use zlib_rs::DeflateConfig;
use zlib_rs::DeflateFlush;
use zlib_rs::Inflate;
use zlib_rs::InflateFlush;
use zlib_rs::Status;
use zlib_rs::Strategy;

fn window_bits(bits: u8) -> u8 {
    bits.max(9)
}

pub struct Deflater {
    stream: Deflate,
    context_takeover: bool,
}

impl fmt::Debug for Deflater {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Deflater").field("context_takeover", &self.context_takeover).finish_non_exhaustive()
    }
}

impl Deflater {
    pub fn new(bits: u8, context_takeover: bool) -> Self {
        let config = DeflateConfig {
            level: -1,
            window_bits: -i32::from(window_bits(bits)),
            mem_level: 8,
            strategy: Strategy::Default,
            ..DeflateConfig::default()
        };
        Self { stream: Deflate::new_with_config(config), context_takeover }
    }

    pub fn compress(&mut self, input: &[u8]) -> Option<Vec<u8>> {
        if input.is_empty() {
            if !self.context_takeover {
                self.stream.reset();
            }
            return Some(vec![0]);
        }
        let mut out = Vec::new();
        let mut offset = 0;
        loop {
            let mut buffer = [0u8; 1 << 16];
            let before_in = self.stream.total_in();
            let before_out = self.stream.total_out();
            let status = self.stream.compress(&input[offset..], &mut buffer, DeflateFlush::NoFlush).ok()?;
            let consumed = (self.stream.total_in() - before_in) as usize;
            let produced = (self.stream.total_out() - before_out) as usize;
            out.extend_from_slice(&buffer[..produced]);
            offset += consumed;
            if offset == input.len() && produced < buffer.len() {
                break;
            }
            if matches!(status, Status::StreamEnd) || (consumed == 0 && produced == 0) {
                return None;
            }
        }
        loop {
            let mut buffer = [0u8; 1 << 16];
            let before_out = self.stream.total_out();
            let status = self.stream.compress(&[], &mut buffer, DeflateFlush::SyncFlush).ok()?;
            let produced = (self.stream.total_out() - before_out) as usize;
            out.extend_from_slice(&buffer[..produced]);
            if matches!(status, Status::BufError) || produced < buffer.len() {
                break;
            }
            if produced == 0 {
                return None;
            }
        }
        if out.len() < 4 {
            return None;
        }
        out.truncate(out.len() - 4);
        if !self.context_takeover {
            self.stream.reset();
        }
        Some(out)
    }
}

pub struct Inflater {
    stream: Inflate,
}

impl fmt::Debug for Inflater {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Inflater").finish_non_exhaustive()
    }
}

impl Inflater {
    pub fn new(bits: u8) -> Self {
        Self { stream: Inflate::new(false, window_bits(bits)) }
    }

    pub fn inflate(&mut self, input: &[u8]) -> Option<Vec<u8>> {
        let mut data = Vec::with_capacity(input.len() + 4);
        data.extend_from_slice(input);
        data.extend_from_slice(&[0, 0, 0xff, 0xff]);
        let mut out = Vec::new();
        let mut offset = 0;
        loop {
            let mut buffer = [0u8; 1 << 16];
            let before_in = self.stream.total_in();
            let before_out = self.stream.total_out();
            let status = self.stream.decompress(&data[offset..], &mut buffer, InflateFlush::NoFlush).ok()?;
            let consumed = (self.stream.total_in() - before_in) as usize;
            let produced = (self.stream.total_out() - before_out) as usize;
            out.extend_from_slice(&buffer[..produced]);
            offset += consumed;
            if matches!(status, Status::StreamEnd) {
                break;
            }
            if offset == data.len() && produced < buffer.len() {
                break;
            }
            if consumed == 0 && produced == 0 {
                return None;
            }
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIRST: &[u8] = b"{\"id\":1,\"result\":{\"protocolVersion\":\"1.3\"}}";

    #[test]
    fn deflate_matches_chromiums_sync_flush() {
        let mut deflater = Deflater::new(15, true);
        let compressed = deflater.compress(FIRST).expect("the reply deflates");
        let mut inflater = Inflater::new(15);
        assert_eq!(inflater.inflate(&compressed).expect("the reply inflates"), FIRST);
        let second = deflater.compress(FIRST).expect("the second reply deflates");
        assert!(second.len() < compressed.len());
        assert_eq!(inflater.inflate(&second).expect("the second reply inflates"), FIRST);
    }

    #[test]
    fn deflate_matches_the_frozen_browsers_two_replies() {
        fn hex(text: &str) -> Vec<u8> {
            (0..text.len()).step_by(2).map(|index| u8::from_str_radix(&text[index..index + 2], 16).expect("a byte")).collect()
        }
        let first = br#"{"id":1,"result":{"protocolVersion":"1.3","product":"Chrome/156.0.8078.79","revision":"@a21ce95c24ce3964416f09ae1862f53f319eb4d0","userAgent":"Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) HeadlessChrome/156.0.0.0 Safari/537.36","jsVersion":"15.6.75.27"}}"#;
        let mut deflater = Deflater::new(15, true);
        assert_eq!(deflater.compress(first).expect("the first reply deflates"), hex("548fb16ac33014457f45bc2901234b9625d9eed2d0a18124534b9bad28f273a246b58c6497d0907f2f860ced7ae11ccebd826ba1e119444c931fa1b9c210c3186cf06f18930b3d34c0a9806cdedbc98ed0c0d329862fccb95494d18ae98aea1a66c5b7bb138fa6e0166b698bd2a2a8555972d5b1da20af54d149d1095ee3a16c196430258cab23f6b379177e9cf726979491c59ef307b275fd7421974a7da8724956c3e0f11d0f1b37e652682a14596cd6afbb6d46bc3b2379467b0e4bb246d37a4ce95f28a38cbc98ce4477472183cff4e7a5a48a6a490b0db7db2f00"));
        let second = br#"{"id":2,"result":{"protocolVersion":"1.3","product":"Chrome/156.0.8078.79","revision":"@a21ce95c24ce3964416f09ae1862f53f319eb4d0","userAgent":"Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) HeadlessChrome/156.0.0.0 Safari/537.36","jsVersion":"15.6.75.27"}}"#;
        assert_eq!(deflater.compress(second).expect("the second reply deflates"), hex("aa06878bd168b8a0850b00"));
        let mut reset = Deflater::new(15, false);
        assert_eq!(reset.compress(first).expect("the first reset reply deflates"), hex("548fb16ac33014457f45bc2901234b9625d9eed2d0a18124534b9bad28f273a246b58c6497d0907f2f860ced7ae11ccebd826ba1e119444c931fa1b9c210c3186cf06f18930b3d34c0a9806cdedbc98ed0c0d329862fccb95494d18ae98aea1a66c5b7bb138fa6e0166b698bd2a2a8555972d5b1da20af54d149d1095ee3a16c196430258cab23f6b379177e9cf726979491c59ef307b275fd7421974a7da8724956c3e0f11d0f1b37e652682a14596cd6afbb6d46bc3b2379467b0e4bb246d37a4ce95f28a38cbc98ce4477472183cff4e7a5a48a6a490b0db7db2f00"));
        assert_eq!(reset.compress(second).expect("the second reset reply deflates"), hex("548fb16ac33014457f45bc2901234b9625d9eed2d0a18124534b9bad28f273a246b58c6497d0907f2f860ced7ae11ccebd826ba129328898263f427385218631d8e0df3026177a68805301d9bcb7931da181a7530c5f9873a928a315d315d535cc8a6f77271e4dc12dd6d216a54551abb2e4aa63b5415ea9a293a213bcc643d932c8604a185747ec67f32efc38ef4d2e29238b3de70f64ebfae9422e95fa50e592ac86c1e33b1e366ecca5d05428b2d8ac5f77db8c787746f28cf61c96648da6f598d2bf50461979319d89ee8e42069fe9cf4b4915d592161a6eb75f00"));
    }

    #[test]
    fn deflate_resets_context_when_negotiated() {
        let mut deflater = Deflater::new(15, false);
        let first = deflater.compress(FIRST).expect("the first reply deflates");
        let second = deflater.compress(FIRST).expect("the second reply deflates");
        assert_eq!(first, second);
    }
}

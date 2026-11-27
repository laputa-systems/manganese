//! Browser-facing WebP decode output, built on the pure-Rust webpkit codec.
//!
//! A still or an animation's first composited frame is decoded to straight
//! RGBA by webpkit, then exported in premultiplied blue-green-red-alpha byte
//! order with libwebp's fixed-point alpha multiply. This is the pixel layout
//! a software canvas consumes when it reads a decoded WebP through libwebp.

#![deny(unsafe_code)]

use webpkit::{ImageInfo, decode, probe, probe_animation};

/// The header facts a browser reads before it paints a decoded WebP.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Features {
    pub width: u32,
    pub height: u32,
    pub has_alpha: bool,
    pub frame_count: u32,
}

/// A decoded WebP's header facts and premultiplied BGRA pixels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decoded {
    pub features: Features,
    pub bgra: Vec<u8>,
}

/// Return the header facts a decoder would report for `input`, without
/// decoding any pixels.
pub fn features(input: &[u8]) -> Option<Features> {
    let info: ImageInfo = probe(input).ok()?;
    let frame_count = if info.is_animated {
        probe_animation(input).ok()?.frame_count
    } else {
        1
    };
    Some(Features {
        width: info.dimensions.width(),
        height: info.dimensions.height(),
        has_alpha: info.has_alpha,
        frame_count: frame_count.max(1) as u32,
    })
}

/// Decode `input` to a browser-facing premultiplied BGRA frame, or `None`
/// when the container or bitstream does not decode.
pub fn decode_bgra(input: &[u8]) -> Option<Decoded> {
    let features = features(input)?;
    let image = decode(input).ok()?;
    let dimensions = image.dimensions();
    if dimensions.width() != features.width || dimensions.height() != features.height {
        return None;
    }
    let rgba = image.into_pixels();
    let premultiply = features.has_alpha;
    let mut bgra = Vec::with_capacity(rgba.len());
    for pixel in rgba.as_chunks::<4>().0 {
        let (r, g, b, a) = (pixel[0], pixel[1], pixel[2], pixel[3]);
        if premultiply && a != 255 {
            let multiplier = u32::from(a) * 32897;
            bgra.push(((u32::from(b) * multiplier) >> 23) as u8);
            bgra.push(((u32::from(g) * multiplier) >> 23) as u8);
            bgra.push(((u32::from(r) * multiplier) >> 23) as u8);
        } else {
            bgra.push(b);
            bgra.push(g);
            bgra.push(r);
        }
        bgra.push(a);
    }
    Some(Decoded { features, bgra })
}

#[cfg(test)]
mod tests {
    use super::{decode_bgra, features};

    #[test]
    fn decodes_a_lossless_still_to_opaque_bgra() {
        let rgba: Vec<u8> = (0..32u32 * 24 * 4)
            .map(|i| {
                let quad = i / 4;
                if i % 4 == 3 { 255 } else { (quad * 7 % 251) as u8 }
            })
            .collect();
        let webp = webpkit::encode_lossless_rgba(32, 24, &rgba).unwrap();
        let features = features(&webp).unwrap();
        assert_eq!((features.width, features.height), (32, 24));
        let decoded = decode_bgra(&webp).unwrap();
        assert_eq!(decoded.bgra.len(), rgba.len());
        for (quad, source) in decoded.bgra.as_chunks::<4>().0.iter().zip(rgba.as_chunks::<4>().0) {
            assert_eq!(quad, &[source[2], source[1], source[0], source[3]]);
        }
    }

    #[test]
    fn premultiplies_a_translucent_pixel_with_libwebp_rounding() {
        let rgba = [200u8, 100, 50, 2];
        let webp = webpkit::encode_lossless_rgba(1, 1, &rgba).unwrap();
        let decoded = decode_bgra(&webp).unwrap();
        let multiplier = 2u32 * 32897;
        assert_eq!(
            decoded.bgra,
            vec![
                ((50u32 * multiplier) >> 23) as u8,
                ((100u32 * multiplier) >> 23) as u8,
                ((200u32 * multiplier) >> 23) as u8,
                2,
            ]
        );
    }

    #[test]
    fn rejects_a_non_webp_input() {
        assert!(features(b"not a webp").is_none());
        assert!(decode_bgra(b"not a webp").is_none());
    }
}

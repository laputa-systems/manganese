use img_gif::{Blend, Decoder, Disposal, Status};

fn encode(indices: &[u8], minimum: u8) -> Vec<u8> {
    let clear = 1usize << minimum;
    let end = clear + 1;
    let mut output = Vec::new();
    let mut bits = 0u32;
    let mut count = 0u32;
    let mut push = |code: usize, width: u32| {
        for offset in 0..width {
            if (code >> offset) & 1 != 0 {
                bits |= 1 << count;
            }
            count += 1;
            if count == 8 {
                output.push(bits as u8);
                bits = 0;
                count = 0;
            }
        }
    };
    push(clear, minimum as u32 + 1);
    for &index in indices {
        push(index as usize, minimum as u32 + 1);
    }
    push(end, minimum as u32 + 1);
    if count > 0 {
        output.push(bits as u8);
    }
    let mut blocks = Vec::new();
    for chunk in output.chunks(255) {
        blocks.push(chunk.len() as u8);
        blocks.extend_from_slice(chunk);
    }
    blocks.push(0);
    blocks
}

fn gif(width: u16, height: u16, rows: &[&[u8]], interlace: bool, transparent: Option<u8>) -> Vec<u8> {
    let mut data = b"GIF89a".to_vec();
    data.extend_from_slice(&width.to_le_bytes());
    data.extend_from_slice(&height.to_le_bytes());
    data.extend_from_slice(&[0x81, 0, 0]);
    data.extend_from_slice(&[0, 0, 0, 255, 0, 0, 0, 0, 0, 255, 255, 255]);
    data.extend_from_slice(&[0x21, 0xf9, 4, if transparent.is_some() { 1 } else { 0 }, 10, 0, transparent.unwrap_or(0), 0]);
    data.push(0x2c);
    data.extend_from_slice(&[0, 0, 0, 0]);
    data.extend_from_slice(&width.to_le_bytes());
    data.extend_from_slice(&height.to_le_bytes());
    data.push(if interlace { 0x40 } else { 0 });
    data.push(8);
    let indices: Vec<u8> = rows.iter().flat_map(|row| row.iter().copied()).collect();
    data.extend(encode(&indices, 8));
    data.push(0x3b);
    data
}

#[test]
fn palette_order_and_transparent_binary_alpha() {
    let data = gif(2, 1, &[&[0, 1]], false, Some(1));
    let decoder = Decoder::new(&data).unwrap();
    assert_eq!(decoder.width, 2);
    assert_eq!(decoder.height, 1);
    let frame = &decoder.frames[0];
    assert_eq!(frame.disposal, Disposal::Keep);
    assert_eq!(frame.blend, Blend::SrcOver);
    assert_eq!(frame.duration_ms, 100);
    assert!(!frame.opaque_within_bounds);
    let mut pixels = [0u8; 8];
    assert_eq!(decoder.decode_frame(&data, 0, &mut pixels, 8, Blend::Src), Status::Complete);
    assert_eq!(pixels, [0, 0, 0, 255, 0, 0, 0, 0]);
}

#[test]
fn interlaced_rows_follow_gif_pass_order() {
    let data = gif(1, 4, &[&[0], &[1], &[1], &[1]], true, None);
    let decoder = Decoder::new(&data).unwrap();
    let mut pixels = [0u8; 16];
    assert_eq!(decoder.decode_frame(&data, 0, &mut pixels, 4, Blend::Src), Status::Complete);
    assert_eq!(&pixels[0..4], &[0, 0, 0, 255]);
    assert_eq!(&pixels[4..8], &[0, 0, 255, 255]);
    assert_eq!(&pixels[12..16], &[0, 0, 255, 255]);
}

#[test]
fn loop_count_matches_animation_semantics() {
    let mut data = gif(1, 1, &[&[0]], false, None);
    let loop_block = [0x21, 0xff, 11, b'N', b'E', b'T', b'S', b'C', b'A', b'P', b'E', b'2', b'.', b'0', 3, 1, 2, 0, 0];
    data.splice(25..25, loop_block);
    let decoder = Decoder::new(&data).unwrap();
    assert_eq!(decoder.num_animation_loops(), 3);
    let mut forever = gif(1, 1, &[&[0]], false, None);
    let forever_block = [0x21, 0xff, 11, b'N', b'E', b'T', b'S', b'C', b'A', b'P', b'E', b'2', b'.', b'0', 3, 1, 0, 0, 0];
    forever.splice(25..25, forever_block);
    assert_eq!(Decoder::new(&forever).unwrap().num_animation_loops(), 0);
    assert!(matches!(Decoder::new(&data[..10]), Err(Status::ShortRead)));
    assert!(matches!(Decoder::new(b"BAD89a"), Err(Status::Error(_))));
}

fn image(x: u16, y: u16, width: u16, height: u16, minimum: u8) -> Vec<u8> {
    let mut data = vec![0x2c];
    data.extend_from_slice(&x.to_le_bytes());
    data.extend_from_slice(&y.to_le_bytes());
    data.extend_from_slice(&width.to_le_bytes());
    data.extend_from_slice(&height.to_le_bytes());
    data.push(0);
    data.push(minimum);
    data.push(0);
    data
}

fn raw_gif(canvas_width: u16, canvas_height: u16, images: &[Vec<u8>]) -> Vec<u8> {
    let mut data = b"GIF89a".to_vec();
    data.extend_from_slice(&canvas_width.to_le_bytes());
    data.extend_from_slice(&canvas_height.to_le_bytes());
    data.extend_from_slice(&[0, 0, 0]);
    for image in images {
        data.extend_from_slice(image);
    }
    data.push(0x3b);
    data
}

#[test]
fn empty_canvas_without_frames_is_rejected() {
    assert!(matches!(Decoder::new(&raw_gif(0, 0, &[])), Err(Status::Error(_))));
}

#[test]
fn empty_canvas_expands_to_first_frame() {
    let decoder = Decoder::new(&raw_gif(0, 0, &[image(0, 0, 3, 2, 1)])).unwrap();
    assert_eq!((decoder.width, decoder.height), (3, 2));
}

#[test]
fn zero_area_frame_completes_with_empty_stream() {
    let data = raw_gif(6, 1, &[image(255, 0, 6, 0, 1)]);
    let decoder = Decoder::new(&data).unwrap();
    let mut pixels = [0u8; 16];
    assert_eq!(decoder.decode_frame(&data, 0, &mut pixels, 8, Blend::Src), Status::Complete);
}

#[test]
fn clipped_zero_width_frame_stays_partial() {
    let data = raw_gif(2, 18, &[image(0, 1, 2, 18, 1), image(2, 3, 2, 16, 1)]);
    let decoder = Decoder::new(&data).unwrap();
    assert_eq!(decoder.frames[1].width, 0);
    let mut pixels = [0u8; 144];
    assert_eq!(decoder.decode_frame(&data, 1, &mut pixels, 8, Blend::Src), Status::ShortRead);
}

#[test]
fn invalid_literal_width_keeps_frame_and_fails_decode() {
    let data = raw_gif(6, 1, &[image(0, 0, 6, 1, 1), image(0, 0, 6, 1, 102)]);
    let decoder = Decoder::new(&data).unwrap();
    assert_eq!(decoder.frames.len(), 2);
    let mut pixels = [0u8; 24];
    assert_eq!(
        decoder.decode_frame(&data, 1, &mut pixels, 24, Blend::Src),
        Status::Error(img_gif::Error::BadLiteralWidth),
    );
}

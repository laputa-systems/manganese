//! Scratch diagnostic (untracked): print the work-count ledger around one
//! lossless encode so the production cost split is visible.
//! Run: cargo run --release --features work-count --example workledger-scratch -- DUMP WIDTH HEIGHT LEVEL
use webpkit::{Dimensions, Effort, Encoder, ImageRef, Metadata, PixelLayout, work_count};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&args[1]).unwrap();
    let w = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
    let h = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
    let rgba = &bytes[8..];
    let level: u8 = args[2].parse().unwrap();
    let expected = w as usize * h as usize * 4;
    assert_eq!(rgba.len(), expected);

    let dims = Dimensions::new(w, h).unwrap();
    let img = ImageRef::new(dims, PixelLayout::Rgba8, rgba).unwrap();

    // warm-up: one full encode, then reset the ledger
    let out = Encoder::lossless().effort(Effort::level(level)).metadata(Metadata::none()).encode_ref(img).unwrap();
    println!("encoded {} bytes ({}x{}, level {})", out.len(), w, h, level);

    work_count::reset();
    let t = std::time::Instant::now();
    let out = Encoder::lossless().effort(Effort::level(level)).metadata(Metadata::none()).encode_ref(img).unwrap();
    if let Some(path) = args.get(3) {
        std::fs::write(path, &out).unwrap();
    }
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    let snap = work_count::snapshot();
    println!("time = {ms:.1} ms, bytes = {}", out.len());
    for (i, v) in snap.iter().enumerate() {
        if *v != 0 {
            println!("{i:2} {:24} {}", webpkit::work_count::field_names()[i], v);
        }
    }
}

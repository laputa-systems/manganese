//! Scratch diagnostic (untracked): print the lossy work-count ledger and wall
//! around one encode. Run: cargo run --release --features work-count --example workledger-lossy-scratch -- DUMP WIDTH HEIGHT QUALITY
use webpkit::{Dimensions, Encoder, ImageRef, Metadata, PixelLayout, work_count};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&args[1]).unwrap();
    let w = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
    let h = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
    let rgba = &bytes[8..];
    let quality: u8 = args[2].parse().unwrap();
    let effort = args.get(3).map_or(webpkit::Effort::AUTO, |s| {
        if s == "auto" { webpkit::Effort::AUTO } else { webpkit::Effort::level(s.parse().unwrap()) }
    });
    let tuning = if args.get(4).map(String::as_str) == Some("afast") {
        webpkit::LossyTuning::new().with_alpha_filter(webpkit::AlphaFilterMode::Fast)
    } else {
        webpkit::LossyTuning::default()
    };
    let dims = Dimensions::new(w, h).unwrap();
    let img = ImageRef::new(dims, PixelLayout::Rgba8, rgba).unwrap();
    let out = Encoder::lossy().quality(quality).effort(effort).tuning(tuning).metadata(Metadata::none()).encode_ref(img).unwrap();
    println!("encoded {} bytes ({}x{}, q{}, effort {:?})", out.len(), w, h, quality, effort);
    work_count::reset();
    let t = std::time::Instant::now();
    let out = Encoder::lossy().quality(quality).effort(effort).tuning(tuning).metadata(Metadata::none()).encode_ref(img).unwrap();
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    let snap = work_count::snapshot();
    println!("time = {ms:.1} ms, bytes = {}", out.len());
    for (i, v) in snap.iter().enumerate() {
        if *v != 0 {
            println!("{i:2} {:24} {}", webpkit::work_count::field_names()[i], v);
        }
    }
}

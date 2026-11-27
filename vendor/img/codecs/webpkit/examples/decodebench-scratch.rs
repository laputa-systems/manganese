//! Scratch diagnostic (untracked): time `webpkit::decode` on a .webp file.
//! Run: cargo run --release --example decodebench-scratch -- FILE [ITERS]
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let data = std::fs::read(&args[1])?;
    let iters: usize = args.get(2).map_or(Ok(5), |s| s.parse())?;
    let img = webpkit::decode(&data)?;
    let (w, h) = (img.dimensions().width(), img.dimensions().height());
    let mut best = f64::MAX;
    for _ in 0..iters {
        let t = Instant::now();
        let out = webpkit::decode(&data)?;
        assert_eq!(out.as_bytes().len(), img.as_bytes().len());
        best = best.min(t.elapsed().as_secs_f64() * 1000.0);
    }
    println!("{w}x{h} {} bytes -> best decode {best:.1} ms", data.len());
    Ok(())
}

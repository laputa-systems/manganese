use std::env;
use std::fs;
use std::io::{self, Write};
use std::process::{Command, Stdio};
use tinyland_remote::{read_compositor, write_host, CompositorMessage, HostMessage, Size};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args_os().skip(1);
    let dir = std::path::PathBuf::from(args.next().ok_or("missing run directory")?);
    let command = args.next().ok_or("missing command")?;
    let mut child = Command::new(command).args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn()?;
    let mut input = child.stdout.take().ok_or("missing stdout")?;
    let mut output = child.stdin.take().ok_or("missing stdin")?;
    let mut size: Option<Size> = None;
    let mut frame = Vec::new();
    while let Some(message) = read_compositor(&mut input, size)? {
        let serial = match message {
            CompositorMessage::Ready { size: s } => { size = Some(s); frame = vec![0; s.pixel_len()]; continue; }
            CompositorMessage::Frame { serial, pixels } => { frame = pixels; serial }
            CompositorMessage::Damage { serial, rects } => {
                let s = size.ok_or("damage before ready")?;
                for (r, pixels) in rects {
                    let stride = r.width as usize * 4;
                    for y in 0..r.height as usize {
                        let dest = ((r.y as usize + y) * s.width() as usize + r.x as usize) * 4;
                        frame[dest..dest + stride].copy_from_slice(&pixels[y * stride..(y + 1) * stride]);
                    }
                }
                serial
            }
            CompositorMessage::Cursor { .. } => continue,
        };
        write_host(&mut output, &HostMessage::Presented { serial })?;
        output.flush()?;
        let s = size.ok_or("frame before ready")?;
        let mut rgba = frame.clone();
        for pixel in rgba.chunks_exact_mut(4) { pixel.swap(0, 2); pixel[3] = 255; }
        fs::write(dir.join("frame.tmp"), &rgba)?;
        fs::rename(dir.join("frame.tmp"), dir.join("frame.rgba"))?;
        fs::write(dir.join("frame-size"), format!("{} {}\n", s.width(), s.height()))?;
    }
    drop(output);
    let status = child.wait()?;
    if !status.success() { return Err(io::Error::other(format!("compositor exited {status}")).into()); }
    Ok(())
}

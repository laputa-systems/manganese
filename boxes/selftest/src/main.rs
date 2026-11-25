use std::io::{self, Read, Write};

fn main() -> io::Result<()> {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let mut magic = [0; 4];
    input.read_exact(&mut magic)?;
    if &magic != b"MGB1" { return Err(io::Error::other("unsupported protocol")); }
    output.write_all(b"MGB1")?;
    loop {
        let mut length = [0; 4];
        if input.read(&mut length[..1])? == 0 { break; }
        input.read_exact(&mut length[1..])?;
        let size = u32::from_be_bytes(length) as usize;
        let mut record = vec![0; size];
        input.read_exact(&mut record)?;
        for byte in &mut record { *byte = byte.to_ascii_uppercase(); }
        if std::env::args().any(|arg| arg == "--nondeterministic") {
            record.extend_from_slice(std::process::id().to_string().as_bytes());
        }
        output.write_all(&(record.len() as u32).to_be_bytes())?;
        output.write_all(&record)?;
        output.flush()?;
    }
    Ok(())
}

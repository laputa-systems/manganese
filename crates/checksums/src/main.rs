use std::io::{self, Read, Write};

fn record(input: &[u8]) -> Result<Vec<u8>, &'static str> {
    if input.len() < 8 { return Err("short record"); }
    let op = input[0];
    let offset = usize::from(input[1]);
    if offset > 63 { return Err("invalid offset"); }
    let chunks = u16::from_be_bytes([input[2], input[3]]);
    let mut checksum = u32::from_be_bytes(input[4..8].try_into().map_err(|_| "seed")?);
    let mut pos = 8usize;
    let mut combined = Vec::new();
    for _ in 0..chunks {
        let end = pos.checked_add(4).filter(|end| *end <= input.len()).ok_or("short length")?;
        let len = u32::from_be_bytes(input[pos..end].try_into().map_err(|_| "length")?) as usize;
        pos = end;
        let end = pos.checked_add(len).filter(|end| *end <= input.len()).ok_or("short data")?;
        let mut aligned = vec![0; offset + len + 63];
        let base = (64 - (aligned.as_ptr() as usize % 64)) % 64 + offset;
        aligned[base..base + len].copy_from_slice(&input[pos..end]);
        let data = &aligned[base..base + len];
        if op == 0 { checksum = crc32c::crc32c_append(checksum, data); }
        else { combined.extend_from_slice(data); }
        pos = end;
    }
    if pos != input.len() { return Err("trailing input"); }
    let mut aligned = vec![0; combined.len() + offset + 63];
    let base = (64 - (aligned.as_ptr() as usize % 64)) % 64 + offset;
    aligned[base..base + combined.len()].copy_from_slice(&combined);
    let data = &aligned[base..base + combined.len()];
    match op {
        0 => Ok(checksum.to_be_bytes().to_vec()),
        1 => Ok(persistent_hash(data).to_be_bytes().to_vec()),
        2 => Ok(cityhasher::hash::<u64>(data).to_be_bytes().to_vec()),
        3 => Ok(leveldb_hash(data, checksum).to_be_bytes().to_vec()),
        _ => Err("invalid operation"),
    }
}

fn persistent_hash(data: &[u8]) -> u32 {
    if data.is_empty() { return 0; }
    let mut h = data.len() as u32;
    let mut chunks = data.chunks_exact(4);
    for chunk in &mut chunks {
        h = h.wrapping_add(u32::from(u16::from_le_bytes([chunk[0], chunk[1]])));
        let tmp = (u32::from(u16::from_le_bytes([chunk[2], chunk[3]])) << 11) ^ h;
        h = (h << 16) ^ tmp;
        h = h.wrapping_add(h >> 11);
    }
    let rem = chunks.remainder();
    match rem.len() {
        3 => {
            h = h.wrapping_add(u32::from(u16::from_le_bytes([rem[0], rem[1]])));
            h ^= h << 16;
            h ^= (rem[2] as i8 as i32 as u32) << 18;
            h = h.wrapping_add(h >> 11);
        }
        2 => {
            h = h.wrapping_add(u32::from(u16::from_le_bytes([rem[0], rem[1]])));
            h ^= h << 11;
            h = h.wrapping_add(h >> 17);
        }
        1 => {
            h = h.wrapping_add(rem[0] as i8 as i32 as u32);
            h ^= h << 10;
            h = h.wrapping_add(h >> 1);
        }
        _ => {}
    }
    h ^= h << 3;
    h = h.wrapping_add(h >> 5);
    h ^= h << 4;
    h = h.wrapping_add(h >> 17);
    h ^= h << 25;
    h.wrapping_add(h >> 6)
}

fn leveldb_hash(data: &[u8], seed: u32) -> u32 {
    let m = 0xc6a4a793u32;
    let mut h = seed ^ (data.len() as u32).wrapping_mul(m);
    let mut chunks = data.chunks_exact(4);
    for chunk in &mut chunks {
        h = h.wrapping_add(u32::from_le_bytes(chunk.try_into().expect("four bytes")));
        h = h.wrapping_mul(m);
        h ^= h >> 16;
    }
    let rem = chunks.remainder();
    for (index, byte) in rem.iter().enumerate() { h = h.wrapping_add(u32::from(*byte) << (index * 8)); }
    if !rem.is_empty() { h = h.wrapping_mul(m); h ^= h >> 24; }
    h
}

fn main() -> io::Result<()> {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let mut header = [0; 4];
    input.read_exact(&mut header)?;
    if &header != b"MGB1" { return Err(io::Error::other("invalid protocol")); }
    output.write_all(b"MGB1")?;
    output.flush()?;
    loop {
        if input.read(&mut header[..1])? == 0 { break; }
        input.read_exact(&mut header[1..])?;
        let mut bytes = vec![0; u32::from_be_bytes(header) as usize];
        input.read_exact(&mut bytes)?;
        let result = record(&bytes).map_err(io::Error::other)?;
        output.write_all(&(result.len() as u32).to_be_bytes())?;
        output.write_all(&result)?;
        output.flush()?;
    }
    Ok(())
}

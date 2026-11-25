use std::io::{self, Read, Write};
use numerics::{shortest, ShortestStyle};

fn main() -> io::Result<()> {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let mut magic = [0; 4];
    input.read_exact(&mut magic)?;
    if &magic != b"MGB1" { return Err(io::ErrorKind::InvalidData.into()); }
    output.write_all(b"MGB1")?;
    output.flush()?;
    loop {
        let mut length = [0; 4];
        match input.read(&mut length[..1])? {
            0 => return Ok(()),
            1 => input.read_exact(&mut length[1..])?,
            _ => unreachable!(),
        }
        let length = u32::from_be_bytes(length) as usize;
        if length != 11 { return Err(io::ErrorKind::InvalidData.into()); }
        let mut request = [0; 11];
        input.read_exact(&mut request)?;
        let style = match request[0] {
            0 => ShortestStyle::Base,
            2 => ShortestStyle::EcmaScript,
            _ => return Err(io::ErrorKind::InvalidData.into()),
        };
        let bits = u64::from_be_bytes(request[1..9].try_into().unwrap());
        let value = shortest(f64::from_bits(bits), style);
        let mut response = vec![u8::from(value.is_some())];
        if let Some(value) = value { response.extend(value.as_bytes()); }
        output.write_all(&(response.len() as u32).to_be_bytes())?;
        output.write_all(&response)?;
        output.flush()?;
    }
}

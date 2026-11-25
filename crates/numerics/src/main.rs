use std::io::{self, Read, Write};
use numerics::{fixed, parse_double, precision, shortest, shortest_exponential, PrecisionStyle, ShortestStyle};

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
        if length == 0 || length > 65536 { return Err(io::ErrorKind::InvalidData.into()); }
        let mut request = vec![0; length];
        input.read_exact(&mut request)?;
        if request[0] == 5 {
            let (value, consumed) = parse_double(&request[1..]);
            output.write_all(&12_u32.to_be_bytes())?;
            output.write_all(&(consumed as u32).to_be_bytes())?;
            output.write_all(&value.to_bits().to_be_bytes())?;
            output.flush()?;
            continue;
        }
        if length != 11 { return Err(io::ErrorKind::InvalidData.into()); }
        let bits = u64::from_be_bytes(request[1..9].try_into().unwrap());
        let number = f64::from_bits(bits);
        let places = u16::from_be_bytes(request[9..11].try_into().unwrap());
        let value = match request[0] {
            0 => shortest(number, ShortestStyle::Base),
            1 => places.checked_sub(1).and_then(|places| fixed(number, places.into())),
            2 => shortest(number, ShortestStyle::EcmaScript),
            3 => precision(number, places.saturating_sub(1).into(), PrecisionStyle::Precision),
            4 if places == 0 => Some(shortest_exponential(number)),
            4 => precision(number, places.into(), PrecisionStyle::Exponential),
            _ => return Err(io::ErrorKind::InvalidData.into()),
        };
        let mut response = vec![u8::from(value.is_some())];
        if let Some(value) = value { response.extend(value.as_bytes()); }
        output.write_all(&(response.len() as u32).to_be_bytes())?;
        output.write_all(&response)?;
        output.flush()?;
    }
}

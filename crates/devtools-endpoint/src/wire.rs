use std::io::{self, Read, Write};

pub const MAX_PAUSE_MS: u16 = 500;
pub const MAX_IDLE_MS: u16 = 2000;

pub struct Segment {
    pub data: Vec<u8>,
    pub pause_ms: u16,
}

pub struct Script {
    pub segments: Vec<Segment>,
    pub idle_ms: u16,
}

fn take<'a>(input: &mut &'a [u8], count: usize) -> Result<&'a [u8], &'static str> {
    if input.len() < count {
        return Err("truncated request");
    }
    let (head, rest) = input.split_at(count);
    *input = rest;
    Ok(head)
}

fn u16_at(input: &mut &[u8]) -> Result<u16, &'static str> {
    Ok(u16::from_be_bytes(take(input, 2)?.try_into().map_err(|_| "truncated request")?))
}

impl Script {
    pub fn decode(mut input: &[u8]) -> Result<Self, &'static str> {
        if take(&mut input, 1)? != [1] {
            return Err("unknown request version");
        }
        let count = u16_at(&mut input)?;
        let mut segments = Vec::with_capacity(usize::from(count));
        for _ in 0..count {
            let length = u32::from_be_bytes(take(&mut input, 4)?.try_into().map_err(|_| "truncated request")?);
            let data = take(&mut input, length as usize)?.to_vec();
            segments.push(Segment { data, pause_ms: u16_at(&mut input)?.min(MAX_PAUSE_MS) });
        }
        let idle_ms = u16_at(&mut input)?.min(MAX_IDLE_MS);
        if !input.is_empty() {
            return Err("trailing request bytes");
        }
        Ok(Self { segments, idle_ms })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![1];
        out.extend_from_slice(&(self.segments.len() as u16).to_be_bytes());
        for segment in &self.segments {
            out.extend_from_slice(&(segment.data.len() as u32).to_be_bytes());
            out.extend_from_slice(&segment.data);
            out.extend_from_slice(&segment.pause_ms.to_be_bytes());
        }
        out.extend_from_slice(&self.idle_ms.to_be_bytes());
        out
    }
}

pub fn read_frame(input: &mut impl Read) -> io::Result<Option<Vec<u8>>> {
    let mut length = [0; 4];
    match input.read_exact(&mut length) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    let mut frame = vec![0; u32::from_be_bytes(length) as usize];
    input.read_exact(&mut frame)?;
    Ok(Some(frame))
}

pub fn write_frame(output: &mut impl Write, frame: &[u8]) -> io::Result<()> {
    let length = u32::try_from(frame.len()).map_err(|_| io::Error::other("frame over 4 GiB"))?;
    output.write_all(&length.to_be_bytes())?;
    output.write_all(frame)?;
    output.flush()
}

/// Serve MGB1 requests until stdin closes. A failed request stops the adapter before it answers, so
/// the runner reports the failure instead of comparing an empty reply.
pub fn serve_box(mut answer: impl FnMut(&[u8]) -> io::Result<Vec<u8>>) -> io::Result<()> {
    let mut stdin = io::stdin().lock();
    let mut stdout = io::stdout().lock();
    let mut magic = [0; 4];
    stdin.read_exact(&mut magic)?;
    if &magic != b"MGB1" {
        return Err(io::Error::other("not a box protocol stream"));
    }
    stdout.write_all(b"MGB1")?;
    stdout.flush()?;
    while let Some(request) = read_frame(&mut stdin)? {
        write_frame(&mut stdout, &answer(&request)?)?;
    }
    Ok(())
}

use crate::{Fuzz, Lane, Spec, compare, load_blob, lock_items, name};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

struct Peer {
    child: Child,
    input: Option<ChildStdin>,
    output: ChildStdout,
    header: bool,
}

impl Peer {
    fn start(path: &Path) -> Result<Self, String> {
        let mut child = Command::new(path).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().map_err(|e| format!("{}: {e}", path.display()))?;
        let input = child.stdin.take().ok_or("missing stdin")?;
        let output = child.stdout.take().ok_or("missing stdout")?;
        Ok(Self { child, input: Some(input), output, header: false })
    }

    fn record(&mut self, record: &[u8]) -> Result<Vec<u8>, String> {
        let input = self.input.as_mut().ok_or("closed adapter")?;
        if !self.header { input.write_all(b"MGB1").map_err(|e| format!("write header: {e}"))?; }
        let len = u32::try_from(record.len()).map_err(|_| "record too large")?;
        input.write_all(&len.to_be_bytes()).and_then(|_| input.write_all(record)).and_then(|_| input.flush()).map_err(|e| format!("write record: {e}"))?;
        if !self.header {
            let mut header = [0; 4];
            self.output.read_exact(&mut header).map_err(|e| format!("read header: {e}"))?;
            if &header != b"MGB1" { return Err("invalid adapter protocol header".into()); }
            self.header = true;
        }
        let mut length = [0; 4];
        self.output.read_exact(&mut length).map_err(|e| format!("read length: {e}"))?;
        let mut result = vec![0; u32::from_be_bytes(length) as usize];
        self.output.read_exact(&mut result).map_err(|e| format!("read record: {e}"))?;
        Ok(result)
    }

    fn finish(&mut self) -> Result<(), String> {
        drop(self.input.take());
        let status = self.child.wait().map_err(|e| format!("wait adapter: {e}"))?;
        if !status.success() { return Err(format!("adapter exited {status}")); }
        Ok(())
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

struct Random(u64);
impl Random {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize { (self.next() % n as u64) as usize }
}

fn generate(random: &mut Random, seeds: &[Vec<u8>], max: usize) -> Vec<u8> {
    if seeds.is_empty() {
        let len = random.below(max + 1);
        return (0..len).map(|_| random.next() as u8).collect();
    }
    let seed = &seeds[random.below(seeds.len())];
    let mut value = seed.clone();
    for _ in 0..1 + random.below(8) {
        match random.below(5) {
            0 if !value.is_empty() => { let i = random.below(value.len()); value[i] ^= 1 << random.below(8); }
            1 if value.len() < max => { let i = random.below(value.len() + 1); value.insert(i, random.next() as u8); }
            2 if !value.is_empty() => { let i = random.below(value.len()); value.remove(i); }
            3 => {
                let other = &seeds[random.below(seeds.len())];
                let start = random.below(other.len() + 1);
                let end = start + random.below(other.len() - start + 1);
                let pos = random.below(value.len() + 1);
                let count = (end - start).min(max - value.len());
                value.splice(pos..pos, other[start..start + count].iter().copied());
            }
            4 => { let len = random.below(value.len() + 1); value.truncate(len); }
            _ => (),
        }
    }
    if value == *seed {
        if value.is_empty() { value.push(0); } else { value[0] ^= 1; }
    }
    value
}

fn outputs(oracle: &mut Peer, candidate: &mut Peer, input: &[u8]) -> Result<(Vec<u8>, Vec<u8>), String> {
    Ok((oracle.record(input).map_err(|e| format!("oracle: {e}"))?, candidate.record(input).map_err(|e| format!("candidate: {e}"))?))
}

fn display(bytes: &[u8]) -> String { bytes.iter().flat_map(|b| std::ascii::escape_default(*b)).map(char::from).collect() }
fn hex(bytes: &[u8]) -> String { bytes.iter().map(|b| format!("{b:02x}")).collect() }

pub fn run(id: &str, store: &Path, candidate_override: Option<PathBuf>) -> Result<(), String> {
    if !name(id) { return Err("invalid box id".into()); }
    let spec = Spec::parse(&fs::read_to_string(Path::new("boxes").join(id).join("box.toml")).map_err(|e| format!("box spec: {e}"))?)?;
    if let Fuzz::Driver { program } = &spec.fuzz {
        return drive(program, &spec, candidate_override);
    }
    let (seeds, max) = match &spec.fuzz {
        Fuzz::Bytes { max_len } => (Vec::new(), *max_len),
        Fuzz::Driver { .. } => unreachable!("handled above"),
        Fuzz::Corpus { seeds, max_len } => {
            let items = lock_items(&fs::read_to_string("goldens.lock").map_err(|e| format!("goldens.lock: {e}"))?, id)?;
            let mut loaded = Vec::new();
            for seed in seeds {
                let item = items.iter().find(|i| &i.id == seed && i.lane == Lane::Seen).ok_or_else(|| format!("fuzz seed {seed} is not a seen item"))?;
                let value = load_blob(store, &item.input_digest, item.input_size)?;
                if value.len() > *max_len { return Err(format!("fuzz seed {seed} exceeds fuzz_max_len")); }
                loaded.push(value);
            }
            (loaded, *max_len)
        }
    };
    let number = |key: &str, default: u64| -> Result<u64, String> {
        match std::env::var(key) { Ok(s) if !s.is_empty() => s.parse().map_err(|_| format!("invalid {key}")), Ok(_) | Err(std::env::VarError::NotPresent) => Ok(default), Err(e) => Err(format!("{key}: {e}")) }
    };
    let seed = number("SEED", SystemTime::now().duration_since(UNIX_EPOCH).map_err(|e| e.to_string())?.as_nanos() as u64)?;
    let cases = number("CASES", 10000)?;
    if cases == 0 { return Err("CASES must be positive".into()); }
    println!("fuzz box={id} seed={seed} cases={cases}");
    let mut random = Random(seed);
    let mut oracle = Peer::start(Path::new(&spec.oracle))?;
    let mut candidate = Peer::start(candidate_override.as_deref().unwrap_or_else(|| Path::new(&spec.candidate)))?;
    for index in 0..cases {
        let mut input = generate(&mut random, &seeds, max);
        let (a, b) = outputs(&mut oracle, &mut candidate, &input)?;
        if compare(&spec.comparator, &a, &b) { continue; }
        let mut size = input.len();
        while size > 0 {
            let mut offset = 0;
            while offset + size <= input.len() {
                let mut trial = input.clone();
                trial.drain(offset..offset + size);
                let (a, b) = outputs(&mut oracle, &mut candidate, &trial)?;
                if !compare(&spec.comparator, &a, &b) { input = trial; offset = 0; } else { offset += 1; }
            }
            size /= 2;
        }
        let (a, b) = outputs(&mut oracle, &mut candidate, &input)?;
        if compare(&spec.comparator, &a, &b) { return Err("divergence did not reproduce during shrinking".into()); }
        fs::create_dir_all(".work/tmp").map_err(|e| e.to_string())?;
        let path = format!(".work/tmp/fuzz-{id}-{seed}.bin");
        fs::write(&path, &input).map_err(|e| format!("write {path}: {e}"))?;
        return Err(format!("divergence seed={seed} case={index}\nminimized input hex={} escaped=\"{}\"\noracle hex={} escaped=\"{}\"\ncandidate hex={} escaped=\"{}\"\nsaved {path}", hex(&input), display(&input), hex(&a), display(&a), hex(&b), display(&b)));
    }
    oracle.finish()?;
    candidate.finish()?;
    println!("fuzz passed {cases} cases seed={seed}");
    Ok(())
}

/// A box whose inputs are structured records generates and compares them in its own driver, which
/// receives the box's oracle and candidate paths and the SEED and CASES the lane was given.
fn drive(program: &str, spec: &Spec, candidate_override: Option<PathBuf>) -> Result<(), String> {
    let candidate = candidate_override.unwrap_or_else(|| PathBuf::from(&spec.candidate));
    let status = Command::new(program)
        .env("FUZZ_ORACLE", &spec.oracle)
        .env("FUZZ_CANDIDATE", candidate)
        .status()
        .map_err(|e| format!("fuzz driver {program}: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("fuzz driver {program} reported a divergence or failed ({status})")) }
}

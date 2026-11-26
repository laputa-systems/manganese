use crate::process::{Limits, Peer, Scratch, env_number, kill_group, parse_positive};
use crate::{Declaration, Fuzz, Lane, ORACLE_RESAMPLES, OracleAgreement, Spec, compare, load_blob, lock_items, name, oracle_agreement, sha256};
use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const DEFAULT_DRIVER_TIMEOUT_SECS: u64 = 24 * 60 * 60;

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
    Ok((oracle.record(input)?, candidate.record(input)?))
}

/// The case budget every fuzz kind runs under, validated before any driver starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Budget {
    seed: u64,
    cases: u64,
}

impl Budget {
    fn parse(seed: Option<&str>, cases: Option<&str>, default_seed: u64) -> Result<Self, String> {
        let seed = match seed.filter(|s| !s.is_empty()) {
            Some(s) => s.parse().map_err(|_| format!("SEED must be an unsigned integer, got {s:?}"))?,
            None => default_seed,
        };
        let cases = match cases.filter(|s| !s.is_empty()) { Some(s) => parse_positive("CASES", s)?, None => 10000 };
        Ok(Self { seed, cases })
    }

    fn from_env() -> Result<Self, String> {
        let var = |key: &str| match std::env::var(key) {
            Ok(value) => Ok(Some(value)),
            Err(std::env::VarError::NotPresent) => Ok(None),
            Err(e) => Err(format!("{key}: {e}")),
        };
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|e| e.to_string())?.as_nanos() as u64;
        Self::parse(var("SEED")?.as_deref(), var("CASES")?.as_deref(), now)
    }
}

/// Keep the request that was in flight when a case failed, with what identifies its run.
fn preserved(id: &str, budget: Budget, index: u64, input: &[u8], identities: &str, failure: &str) -> String {
    let path = format!(".work/tmp/fuzz-{id}-{}-case{index}.bin", budget.seed);
    let saved = fs::create_dir_all(".work/tmp").and_then(|_| fs::write(&path, input));
    let saved = match saved { Ok(()) => format!("saved {path}"), Err(e) => format!("could not save {path}: {e}") };
    format!("{failure}\nseed={} case={index} input hex={}\n{identities}\n{saved}", budget.seed, hex(input))
}

fn identity(role: &str, path: &Path) -> String {
    match fs::read(path) {
        Ok(bytes) => format!("{role} {} sha256:{}", path.display(), sha256(&bytes)),
        Err(e) => format!("{role} {} unreadable: {e}", path.display()),
    }
}

fn display(bytes: &[u8]) -> String { bytes.iter().flat_map(|b| std::ascii::escape_default(*b)).map(char::from).collect() }
fn hex(bytes: &[u8]) -> String { bytes.iter().map(|b| format!("{b:02x}")).collect() }

pub fn run(id: &str, store: &Path, candidate_override: Option<PathBuf>) -> Result<(), String> {
    if !name(id) { return Err("invalid box id".into()); }
    let spec = Spec::parse(&fs::read_to_string(Path::new("boxes").join(id).join("box.toml")).map_err(|e| format!("box spec: {e}"))?)?;
    let budget = Budget::from_env()?;
    let candidate_path = candidate_override.unwrap_or_else(|| PathBuf::from(&spec.candidate));
    let identities = format!("{}\n{}", identity("oracle", Path::new(&spec.oracle)), identity("candidate", &candidate_path));
    if let Fuzz::Driver { program } = &spec.fuzz {
        let deadline = Duration::from_secs(env_number("FUZZ_TIMEOUT_SECS", DEFAULT_DRIVER_TIMEOUT_SECS)?);
        return drive(id, program, &spec, &candidate_path, budget, &identities, Path::new("."), deadline);
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
    let Budget { seed, cases } = budget;
    println!("fuzz box={id} seed={seed} cases={cases}");
    let limits = Limits::from_env()?;
    let mut random = Random(seed);
    let mut scratch = Scratch::create(&format!("{id}-fuzz"))?;
    let mut oracle = Peer::start(Path::new(&spec.oracle), &[], "oracle", limits, &scratch.fresh()?)?;
    let mut candidate = Peer::start(&candidate_path, &[], "candidate", limits, &scratch.fresh()?)?;
    let declared = matches!(spec.varying, Declaration::Present(_));
    let mut variations = 0u64;
    for index in 0..cases {
        let mut input = generate(&mut random, &seeds, max);
        let (a, b) = outputs(&mut oracle, &mut candidate, &input).map_err(|e| preserved(id, budget, index, &input, &identities, &e))?;
        if compare(&spec.comparator, &a, &b) { continue; }
        let mut answers = vec![a];
        for _ in 0..ORACLE_RESAMPLES { answers.push(oracle.record(&input).map_err(|e| preserved(id, budget, index, &input, &identities, &e))?); }
        match oracle_agreement(&answers, &b, declared, &spec.comparator) {
            OracleAgreement::Stable => {}
            OracleAgreement::DeclaredVariation => {
                variations += 1;
                let path = format!(".work/tmp/fuzz-{id}-{seed}-variation{index}.bin");
                fs::create_dir_all(".work/tmp").and_then(|_| fs::write(&path, &input)).map_err(|e| format!("write {path}: {e}"))?;
                eprintln!("oracle variation seed={seed} case={index}: the candidate gave one of the oracle's answers; saved {path}");
                if variations * 100 > cases { return Err(preserved(id, budget, index, &input, &identities, &format!("the oracle varied on {variations} cases, over 1% of {cases}"))); }
                continue;
            }
            OracleAgreement::UndeclaredVariation => return Err(preserved(id, budget, index, &input, &identities, "the oracle varies on this input and the spec declares no varying relationship")),
            OracleAgreement::UnmatchedVariation => return Err(preserved(id, budget, index, &input, &identities, "the oracle varies on this input and the candidate gave none of its answers")),
        }
        let mut size = input.len();
        while size > 0 {
            let mut offset = 0;
            while offset + size <= input.len() {
                let mut trial = input.clone();
                trial.drain(offset..offset + size);
                let (a, b) = outputs(&mut oracle, &mut candidate, &trial).map_err(|e| preserved(id, budget, index, &trial, &identities, &e))?;
                if !compare(&spec.comparator, &a, &b) { input = trial; offset = 0; } else { offset += 1; }
            }
            size /= 2;
        }
        let (a, b) = outputs(&mut oracle, &mut candidate, &input).map_err(|e| preserved(id, budget, index, &input, &identities, &e))?;
        if compare(&spec.comparator, &a, &b) { return Err(preserved(id, budget, index, &input, &identities, "divergence did not reproduce during shrinking")); }
        fs::create_dir_all(".work/tmp").map_err(|e| e.to_string())?;
        let path = format!(".work/tmp/fuzz-{id}-{seed}.bin");
        fs::write(&path, &input).map_err(|e| format!("write {path}: {e}"))?;
        return Err(format!("divergence seed={seed} case={index}\nminimized input hex={} escaped=\"{}\"\noracle hex={} escaped=\"{}\"\ncandidate hex={} escaped=\"{}\"\n{identities}\nsaved {path}", hex(&input), display(&input), hex(&a), display(&a), hex(&b), display(&b)));
    }
    oracle.finish()?;
    candidate.finish()?;
    println!("fuzz passed {cases} cases seed={seed} oracle_variations={variations}");
    Ok(())
}

/// A box whose inputs are structured records generates and compares them in its own driver. The
/// driver receives the oracle and candidate paths and the validated SEED and CASES, and must write
/// `seed=`, `requested=` and `completed=` lines to FUZZ_RESULT; an exit status alone proves nothing.
fn drive(id: &str, program: &str, spec: &Spec, candidate: &Path, budget: Budget, identities: &str, root: &Path, deadline: Duration) -> Result<(), String> {
    let scratch = root.join(".work/tmp");
    let result = scratch.join(format!("fuzz-{id}-{}-{}.result", budget.seed, std::process::id()));
    fs::create_dir_all(&scratch).map_err(|e| format!("create {}: {e}", scratch.display()))?;
    match fs::remove_file(&result) { Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(format!("clear {}: {e}", result.display())), _ => {} }
    let mut child = Command::new(program)
        .env("FUZZ_ORACLE", &spec.oracle)
        .env("FUZZ_CANDIDATE", candidate)
        .env("SEED", budget.seed.to_string())
        .env("CASES", budget.cases.to_string())
        .env("FUZZ_RESULT", &result)
        .process_group(0)
        .spawn()
        .map_err(|e| format!("fuzz driver {program}: {e}"))?;
    struct DriverGuard<'a> { child: &'a mut Child }
    impl Drop for DriverGuard<'_> {
        fn drop(&mut self) {
            kill_group(self.child);
            let _ = self.child.wait();
        }
    }
    let guard = DriverGuard { child: &mut child };
    let started = Instant::now();
    let status = loop {
        match guard.child.try_wait().map_err(|e| format!("wait fuzz driver: {e}"))? {
            Some(status) => break status,
            None if started.elapsed() < deadline => std::thread::sleep(Duration::from_millis(100)),
            None => return Err(format!("fuzz driver {program} exceeded FUZZ_TIMEOUT_SECS={}\n{identities}", deadline.as_secs())),
        }
    };
    let report = fs::read_to_string(&result).map_err(|e| format!("fuzz driver {program} wrote no result ({e}); exit {status}\n{identities}"))?;
    let completed = driver_completed(&report, budget)?;
    if !status.success() { return Err(format!("fuzz driver {program} reported a divergence or failed ({status}) after {completed} of {} cases\n{identities}", budget.cases)); }
    if completed != budget.cases { return Err(format!("fuzz driver {program} completed {completed} of {} cases", budget.cases)); }
    println!("fuzz driver completed {completed} cases seed={}", budget.seed);
    Ok(())
}

fn driver_completed(report: &str, budget: Budget) -> Result<u64, String> {
    let field = |key: &str| -> Result<u64, String> {
        let mut values = report.lines().filter_map(|line| line.strip_prefix(key)?.strip_prefix('='));
        let value = values.next().ok_or_else(|| format!("fuzz result has no {key}"))?;
        if values.next().is_some() { return Err(format!("fuzz result repeats {key}")); }
        value.parse().map_err(|_| format!("fuzz result {key} is not an unsigned integer"))
    };
    if field("seed")? != budget.seed { return Err("fuzz result is for another seed".into()); }
    if field("requested")? != budget.cases { return Err("fuzz result requested another case count".into()); }
    let completed = field("completed")?;
    if completed > budget.cases { return Err("fuzz result completed more cases than requested".into()); }
    Ok(completed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn alive(pid: u32) -> bool {
        match std::process::Command::new("ps").args(["-o", "stat=", "-p", &pid.to_string()]).output() {
            Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
                .trim()
                .split_whitespace()
                .next()
                .is_some_and(|state| !state.starts_with('Z')),
            _ => false,
        }
    }

    #[test]
    fn a_timed_out_driver_takes_its_peers_with_it() {
        let dir = std::env::temp_dir().join(format!("goldens-drive-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let pidfile = dir.join("peer.pid");
        let program = dir.join("driver");
        fs::write(&program, format!("#!/bin/sh\nsleep 30 &\necho $! > '{}'\nwait\n", pidfile.display())).unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        let spec = Spec::parse(&format!(r#"
seam = "transport cleanup"
streaming = "none: synthetic driver"
reentrancy = "none: synthetic driver"
callbacks = "none: synthetic driver"
threading = "none: synthetic driver"
wire_version = 1
corpus_sources = "none: synthetic driver"
environment = "none: synthetic driver"
comparison = "exact"
observable_surface = "synthetic driver"
varying_relationships = "none: synthetic driver"
ordering = "none: synthetic driver"
oracle_defects = "none: synthetic driver"
improvements = "none: synthetic driver"
anchor = "none: synthetic driver"
arm64_evidence = "harness: synthetic driver"
holdout_percent = 30
oracle = "/usr/bin/true"
candidate = "/usr/bin/true"
fuzz = "driver"
fuzz_driver = "{}"
acceptance_cases = 1
"#, program.display())).unwrap();
        let started = Instant::now();
        let error = drive("cleanup", &program.to_string_lossy(), &spec, Path::new("/usr/bin/true"), Budget { seed: 11, cases: 1 }, "identities", &dir, Duration::from_secs(1)).unwrap_err();
        assert!(error.contains("exceeded FUZZ_TIMEOUT_SECS"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(10), "the driver was not bounded");
        let pid = fs::read_to_string(&pidfile).unwrap().trim().parse::<u32>().unwrap();
        assert!(!alive(pid), "peer {pid} survived the driver's deadline");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn budgets_need_a_positive_case_count_and_an_unsigned_seed() {
        assert_eq!(Budget::parse(Some("5"), Some("12"), 9), Ok(Budget { seed: 5, cases: 12 }));
        assert_eq!(Budget::parse(None, None, 9), Ok(Budget { seed: 9, cases: 10000 }));
        for (seed, cases) in [(None, Some("0")), (None, Some("-1")), (None, Some("x")), (Some("-3"), None), (Some("1e3"), None)] {
            assert!(Budget::parse(seed, cases, 9).is_err(), "{seed:?} {cases:?}");
        }
    }

    #[test]
    fn a_driver_result_must_match_its_budget() {
        let budget = Budget { seed: 7, cases: 3 };
        assert_eq!(driver_completed("seed=7\nrequested=3\ncompleted=3\n", budget), Ok(3));
        assert_eq!(driver_completed("seed=7\nrequested=3\ncompleted=1\n", budget), Ok(1));
        for report in ["", "seed=8\nrequested=3\ncompleted=3", "seed=7\nrequested=0\ncompleted=0", "seed=7\nrequested=3\ncompleted=4",
                       "seed=7\nrequested=3\ncompleted=-1", "seed=7\nrequested=3\ncompleted=3\ncompleted=3", "seed=7\nrequested=3"] {
            assert!(driver_completed(report, budget).is_err(), "{report:?}");
        }
    }
}

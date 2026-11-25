mod digest;
mod fuzz;
mod spec;
pub use digest::sha256;
pub use spec::{Anchor, Arm64Evidence, Comparator, Declaration, Fuzz, Spec};
pub use fuzz::run as fuzz;

mod process;
mod sandbox;
use std::collections::BTreeSet;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lane { Seen, Holdout }

#[derive(Debug)]
struct Item {
    id: String,
    input_digest: String,
    input_size: usize,
    output_digest: String,
    output_size: usize,
    lane: Lane,
}

fn name(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

fn lock_items(text: &str, id: &str) -> Result<Vec<Item>, String> {
    let mut items = Vec::new();
    let mut ids = BTreeSet::new();
    for (line, s) in text.lines().enumerate() {
        if s.trim().is_empty() || s.trim_start().starts_with('#') { continue; }
        let fields: Vec<_> = s.split_whitespace().collect();
        if fields.len() != 7 { return Err(format!("goldens.lock:{}: expected seven fields", line + 1)); }
        if !name(fields[0]) || !name(fields[2]) { return Err(format!("goldens.lock:{}: invalid name", line + 1)); }
        let lane = match fields[1] { "seen" => Lane::Seen, "holdout" => Lane::Holdout, _ => return Err(format!("goldens.lock:{}: invalid lane", line + 1)) };
        for hash in [fields[3], fields[5]] {
            if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) { return Err(format!("goldens.lock:{}: invalid digest", line + 1)); }
        }
        let input_size = fields[4].parse().map_err(|_| format!("goldens.lock:{}: invalid input size", line + 1))?;
        let output_size = fields[6].parse().map_err(|_| format!("goldens.lock:{}: invalid output size", line + 1))?;
        if fields[0] != id { continue; }
        if !ids.insert(fields[2].to_owned()) { return Err(format!("duplicate item {}", fields[2])); }
        items.push(Item { id: fields[2].to_owned(), input_digest: fields[3].to_owned(), input_size, output_digest: fields[5].to_owned(), output_size, lane });
    }
    if items.is_empty() { return Err(format!("no items for {id}")); }
    let seen: BTreeSet<_> = items.iter().filter(|i| i.lane == Lane::Seen).flat_map(|i| [&i.input_digest, &i.output_digest]).collect();
    if items.iter().filter(|i| i.lane == Lane::Holdout).any(|i| seen.contains(&i.input_digest)) {
        return Err("a holdout input is a seen blob".into());
    }
    Ok(items)
}

fn load_blob(store: &Path, digest: &str, size: usize) -> Result<Vec<u8>, String> {
    if size > u32::MAX as usize { return Err(format!("blob {digest} exceeds protocol record limit")); }
    let file = fs::File::open(store.join("sha256").join(digest)).map_err(|e| format!("blob {digest}: {e}"))?;
    let mut bytes = Vec::with_capacity(size.min(1024 * 1024));
    file.take(size as u64 + 1).read_to_end(&mut bytes).map_err(|e| format!("blob {digest}: {e}"))?;
    if bytes.len() != size || sha256(&bytes) != digest { return Err(format!("blob {digest}: digest or size mismatch")); }
    Ok(bytes)
}

fn scoreboard(text: &str) -> Result<BTreeSet<String>, String> {
    let mut lines = text.lines().filter(|s| !s.trim().is_empty());
    if lines.next().map(str::trim) != Some("version = 1") { return Err("invalid scoreboard version".into()); }
    let line = lines.next().ok_or("missing scoreboard passing")?.trim();
    let list = line.strip_prefix("passing = [").and_then(|s| s.strip_suffix(']')).ok_or("invalid scoreboard")?;
    if lines.next().is_some() { return Err("unexpected scoreboard field".into()); }
    let mut result = BTreeSet::new();
    if !list.trim().is_empty() {
        for item in list.split(',') {
            let item = item.trim().strip_prefix('"').and_then(|s| s.strip_suffix('"')).ok_or("invalid scoreboard item")?;
            if !name(item) || !result.insert(item.to_owned()) { return Err("invalid or duplicate scoreboard item".into()); }
        }
    }
    Ok(result)
}

fn invoke(program: &Path, args: &[String], record: &[u8], limits: process::Limits, scratch: &mut process::Scratch) -> Result<Vec<u8>, String> {
    let dir = scratch.fresh()?;
    let result = process::Peer::start(program, args, &program.display().to_string(), limits, &dir).and_then(|peer| peer.exchange(record));
    // The adapter has exited; its scratch held only that run's working files.
    let _ = fs::remove_dir_all(&dir);
    result
}

fn compare(comparator: &Comparator, got: &[u8], expected: &[u8]) -> bool {
    match comparator {
        Comparator::Exact { .. } => got == expected,
        Comparator::Tolerance { bound, .. } => {
            let number = |s: &[u8]| std::str::from_utf8(s).ok()?.parse::<f64>().ok().filter(|n| n.is_finite());
            match (number(got), number(expected)) { (Some(a), Some(b)) => (a - b).abs() <= *bound, _ => false }
        }
    }
}

pub struct RunOptions {
    pub box_id: String,
    pub lane: Lane,
    pub store: PathBuf,
    pub oracle_override: Option<PathBuf>,
    pub candidate_override: Option<PathBuf>,
    pub oracle_args: Vec<String>,
    pub ratchet: bool,
    pub probe_holdout: Option<PathBuf>,
}

pub fn run(options: &RunOptions) -> Result<(), String> {
    if !name(&options.box_id) { return Err("invalid box id".into()); }
    let boxdir = Path::new("boxes").join(&options.box_id);
    let spec = Spec::parse(&fs::read_to_string(boxdir.join("box.toml")).map_err(|e| format!("box spec: {e}"))?)?;
    let items = lock_items(&fs::read_to_string("goldens.lock").map_err(|e| format!("goldens.lock: {e}"))?, &options.box_id)?;
    let seen_count = items.iter().filter(|i| i.lane == Lane::Seen).count();
    let holdout_count = items.len() - seen_count;
    if seen_count == 0 || holdout_count * 100 < items.len() * usize::from(spec.holdout_percent) { return Err("corpus does not meet holdout fraction".into()); }
    if options.ratchet && options.lane != Lane::Seen { return Err("ratchet is only valid for the seen lane".into()); }
    let _ratchet_lock = if options.ratchet { Some(sandbox::lock_ratchet(&options.box_id).map_err(|e| format!("ratchet lock: {e}"))?) } else { None };
    let old = scoreboard(&fs::read_to_string(boxdir.join("scoreboard.toml")).map_err(|e| format!("scoreboard: {e}"))?)?;
    let seen_ids: BTreeSet<_> = items.iter().filter(|i| i.lane == Lane::Seen).map(|i| i.id.clone()).collect();
    if !old.is_subset(&seen_ids) { return Err("scoreboard contains an unknown seen item".into()); }
    let oracle = options.oracle_override.clone().unwrap_or_else(|| PathBuf::from(&spec.oracle)).canonicalize().map_err(|e| format!("oracle path: {e}"))?;
    let candidate = options.candidate_override.clone().unwrap_or_else(|| PathBuf::from(&spec.candidate)).canonicalize().map_err(|e| format!("candidate path: {e}"))?;
    let limits = process::Limits::from_env()?;
    let mut scratch = process::Scratch::create(&options.box_id)?;
    let score_dir = if options.ratchet { Some(fs::File::open(&boxdir).map_err(|e| format!("open scoreboard directory: {e}"))?) } else { None };
    let mut corpus = Vec::new();
    for item in items.iter().filter(|item| item.lane == options.lane) {
        corpus.push((item, load_blob(&options.store, &item.input_digest, item.input_size)?, load_blob(&options.store, &item.output_digest, item.output_size)?));
    }
    if options.lane == Lane::Seen {
        sandbox::restrict(&[&oracle, &candidate], &options.store, scratch.root()).map_err(|e| format!("L0 isolation unavailable: {e}"))?;
        if let Some(path) = &options.probe_holdout {
            return match fs::read(path) { Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => Ok(()), Err(e) => Err(format!("holdout probe failed for another reason: {e}")), Ok(_) => Err("L0 read a holdout blob".into()) };
        }
    }
    for (item, input, golden) in &corpus {
        let first = invoke(&oracle, &options.oracle_args, input, limits, &mut scratch).map_err(|e| format!("oracle {}: {e}", item.id))?;
        let second = invoke(&oracle, &options.oracle_args, input, limits, &mut scratch).map_err(|e| format!("oracle {}: {e}", item.id))?;
        if first != second { return Err(format!("oracle self-disagreement: {}", item.id)); }
        if first != *golden { return Err(format!("oracle differs from recorded golden: {}", item.id)); }
    }
    let mut passing = BTreeSet::new();
    let mut failing = Vec::new();
    for (item, input, golden) in &corpus {
        let got = invoke(&candidate, &[], input, limits, &mut scratch).map_err(|e| format!("candidate {}: {e}", item.id))?;
        if compare(&spec.comparator, &got, golden) { passing.insert(item.id.clone()); } else { failing.push(item.id.clone()); }
    }
    let regressions: Vec<_> = old.difference(&passing).cloned().collect();
    if options.lane == Lane::Seen && !regressions.is_empty() { return Err(format!("scoreboard regression: {}", regressions.join(", "))); }
    if options.ratchet {
        let items = passing.iter().map(|id| format!("\"{id}\"")).collect::<Vec<_>>().join(", ");
        let text = format!("version = 1\npassing = [{items}]\n");
        let temp = boxdir.join(format!(".scoreboard.{}.tmp", std::process::id()));
        let result = (|| -> Result<(), String> {
            let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&temp).map_err(|e| format!("create scoreboard: {e}"))?;
            file.write_all(text.as_bytes()).and_then(|_| file.sync_all()).map_err(|e| format!("write scoreboard: {e}"))?;
            fs::rename(&temp, boxdir.join("scoreboard.toml")).map_err(|e| format!("replace scoreboard: {e}"))?;
            score_dir.as_ref().ok_or("missing scoreboard directory")?.sync_all().map_err(|e| format!("sync scoreboard directory: {e}"))?;
            Ok(())
        })();
        if result.is_err() { let _ = fs::remove_file(temp); }
        result?;
    }
    if !failing.is_empty() { return Err(format!("golden mismatch: {}", failing.join(", "))); }
    println!("lane={} items={} passed={}", if options.lane == Lane::Seen { "seen" } else { "holdout" }, corpus.len(), passing.len());
    Ok(())
}

#[cfg(test)]
mod lock_tests {
    use super::lock_items;

    fn row(lane: &str, item: &str, input: char, output: char) -> String {
        format!("b {lane} {item} {} 1 {} 1\n", input.to_string().repeat(64), output.to_string().repeat(64))
    }

    #[test]
    fn holdout_outputs_may_equal_seen_outputs() {
        let text = row("seen", "s", 'a', 'c') + &row("holdout", "h", 'b', 'c');
        assert_eq!(lock_items(&text, "b").unwrap().len(), 2);
    }

    #[test]
    fn holdout_inputs_never_appear_among_seen_blobs() {
        for text in [row("seen", "s", 'a', 'c') + &row("holdout", "h", 'a', 'd'), row("seen", "s", 'a', 'c') + &row("holdout", "h", 'c', 'd')] {
            assert_eq!(lock_items(&text, "b").unwrap_err(), "a holdout input is a seen blob");
        }
    }
}

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use leveldb::engine::{DB, LdbIterator, ReadOptions, WriteBatch};
use leveldb::ChromiumOptions;

pub const ORIGIN: &str = "https://profile.test";
pub const ALPHA: &str = "hydrogen-alpha";
pub const BETA: &str = "hydrogen-beta";
pub const CHANGED_ALPHA: &str = "manganese-alpha-λ";
pub const GAMMA: &str = "manganese-gamma";
pub const INDEXED: &str = "hydrogen-record";

static FIXTURE: OnceLock<PathBuf> = OnceLock::new();

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().expect("a repository root")
}

pub fn scratch(name: &str) -> PathBuf {
    let directory = repo_root().join(".work/tmp/asm-profile-handoff").join(format!("{name}-{}", std::process::id()));
    fs::create_dir_all(&directory).expect("a scratch directory");
    directory
}

#[derive(Debug)]
pub struct BrowserOutcome {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

impl BrowserOutcome {
    pub fn detail(&self) -> String {
        format!("exit ok={} stdout={} stderr={}", self.ok, self.stdout.trim(), self.stderr.trim())
    }
}

pub fn browser(root: &Path, mode: &str, profile: &Path, expect: Option<(&str, &str)>) -> BrowserOutcome {
    let script = root.join("oracle/harness/profile-handoff/session.py");
    let mut command = Command::new("timeout");
    command.arg("300").arg("python3").arg("-B").arg(script).arg(mode).arg(profile);
    if let Some((key, value)) = expect {
        command.arg(key).arg(value);
    }
    let output = command.output().expect("the browser driver starts");
    BrowserOutcome {
        ok: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

pub fn copy_profile(source: &Path, destination: &Path) {
    if destination.exists() {
        fs::remove_dir_all(destination).expect("the destination profile is removable");
    }
    fs::create_dir_all(destination.parent().expect("a parent directory")).expect("a parent directory");
    let mut copy = Command::new("cp");
    copy.arg("-a").arg(source).arg(destination);
    assert!(copy.status().expect("cp starts").success(), "the profile copy succeeds");
}

pub fn hydrogen_profile(root: &Path) -> PathBuf {
    let prepared = FIXTURE.get_or_init(|| {
        let profile = scratch("hydrogen");
        let outcome = browser(root, "write", &profile, None);
        assert!(outcome.ok, "the frozen browser wrote a profile: {}", outcome.detail());
        assert!(outcome.stdout.contains(ALPHA), "the browser observes its own alpha: {}", outcome.stdout);
        assert!(outcome.stdout.contains(INDEXED), "the browser observes its own IndexedDB record: {}", outcome.stdout);
        profile
    });
    let copy = scratch("case").join(format!("hydrogen-{}", FIXTURE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
    copy_profile(prepared, &copy);
    copy
}

static FIXTURE_COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub fn local_storage_db(profile: &Path) -> PathBuf {
    profile.join("Default/Local Storage/leveldb")
}

pub fn indexeddb_db(profile: &Path) -> PathBuf {
    profile.join("Default/IndexedDB/https_profile.test_0.indexeddb.leveldb")
}

pub fn storage_key(name: &str) -> Vec<u8> {
    let mut key = format!("_{ORIGIN}\0\x01").into_bytes();
    key.extend_from_slice(name.as_bytes());
    key
}

pub fn encode_text(value: &str) -> Vec<u8> {
    let mut encoded = vec![1u8];
    encoded.extend_from_slice(value.as_bytes());
    encoded
}

pub fn decode_text(encoded: &[u8]) -> Option<String> {
    if encoded.first() != Some(&1) {
        return None;
    }
    std::str::from_utf8(&encoded[1..]).ok().map(str::to_string)
}

pub fn open(profile: &Path) -> DB {
    leveldb::open_db(&local_storage_db(profile), &ChromiumOptions::default())
        .expect("the Rust implementation opens the Hydrogen database")
}

pub fn raw_get(profile: &Path, key: &[u8]) -> Option<Vec<u8>> {
    let mut db = open(profile);
    let value = db.get_with_options(key, ReadOptions { verify_checksums: true, fill_cache: true }).expect("a clean get");
    db.close().expect("the database closes");
    value.map(|bytes| bytes.to_vec())
}

pub fn entries(profile: &Path) -> BTreeMap<String, String> {
    let mut db = open(profile);
    let mut iter = db
        .new_iter_with_options(ReadOptions { verify_checksums: true, fill_cache: true })
        .expect("an iterator over the recovered database");
    let prefix = format!("_{ORIGIN}\0\x01");
    let mut found = BTreeMap::new();
    iter.seek_to_first();
    while iter.valid() {
        let (key, value) = iter.current().expect("a valid iterator entry");
        if let Some(name) = std::str::from_utf8(&key).ok().and_then(|key| key.strip_prefix(&prefix)) {
            if let Some(text) = decode_text(&value) {
                found.insert(name.to_string(), text);
            }
        }
        iter.advance();
    }
    assert!(iter.status().is_ok(), "the iterator reads cleanly");
    db.close().expect("the database closes");
    found
}

pub fn write(profile: &Path, changes: &[(&str, Option<&str>)]) {
    let mut db = open(profile);
    let mut batch = WriteBatch::default();
    for (name, value) in changes {
        let key = storage_key(name);
        match value {
            Some(text) => batch.put(&key, &encode_text(text)),
            None => batch.delete(&key),
        }
    }
    db.write(batch, true).expect("a synchronous batch write");
    db.flush_background_work();
    db.close().expect("the database closes after flushing");
}

pub fn write_raw(profile: &Path, name: &str, value: &[u8]) {
    let mut db = open(profile);
    let mut batch = WriteBatch::default();
    batch.put(&storage_key(name), value);
    db.write(batch, true).expect("a synchronous raw batch write");
    db.flush_background_work();
    db.close().expect("the database closes after flushing");
}

pub fn truncate_active_log(profile: &Path) -> PathBuf {
    let directory = local_storage_db(profile);
    let mut logs: Vec<PathBuf> = fs::read_dir(&directory)
        .expect("the database directory is readable")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|extension| extension == "log"))
        .collect();
    logs.sort();
    let log = logs.pop().expect("an active write-ahead log");
    let file = OpenOptions::new().read(true).write(true).open(&log).expect("the log opens");
    let size = file.metadata().expect("the log size").len();
    assert!(size > 32, "the log holds committed records before the interruption");
    file.set_len(size - 16).expect("the last write is cut short");
    log
}

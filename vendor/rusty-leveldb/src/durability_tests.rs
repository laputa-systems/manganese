use crate::env::{Env, FileLock, Logger, RandomAccess, WritableFile};
use crate::{in_memory, MemEnv, Result, Status, StatusCode, WriteBatch, DB};
use std::cell::RefCell;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;

#[derive(Clone, Debug, PartialEq)]
enum Event {
    SyncFile(PathBuf),
    SyncDir(PathBuf),
    Rename(PathBuf, PathBuf),
}

#[derive(Default)]
struct Trace {
    events: Vec<Event>,
    fail_extension: Option<&'static str>,
    fail_manifest: bool,
}

#[derive(Clone, Default)]
struct RecordingEnv {
    inner: Rc<MemEnv>,
    trace: Rc<RefCell<Trace>>,
}

struct RecordingFile {
    inner: Box<dyn WritableFile>,
    path: PathBuf,
    trace: Rc<RefCell<Trace>>,
}

impl Write for RecordingFile {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.inner.write(data)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl WritableFile for RecordingFile {
    fn sync(&mut self) -> Result<()> {
        let mut trace = self.trace.borrow_mut();
        trace.events.push(Event::SyncFile(self.path.clone()));
        if (self.path.extension().and_then(|s| s.to_str()) == trace.fail_extension
            && trace.fail_extension.is_some())
            || (trace.fail_manifest && self.path.file_name().and_then(|s| s.to_str())
                .is_some_and(|s| s.starts_with("MANIFEST-")))
        {
            return Err(Status::new(StatusCode::IOError, "injected sync failure"));
        }
        drop(trace);
        self.inner.sync()
    }
}

impl RecordingEnv {
    fn record(&self, path: &Path, inner: Box<dyn WritableFile>) -> Box<dyn WritableFile> {
        Box::new(RecordingFile {
            inner,
            path: path.to_owned(),
            trace: self.trace.clone(),
        })
    }
}

impl Env for RecordingEnv {
    fn open_sequential_file(&self, p: &Path) -> Result<Box<dyn Read>> {
        self.inner.open_sequential_file(p)
    }

    fn open_random_access_file(&self, p: &Path) -> Result<Box<dyn RandomAccess>> {
        self.inner.open_random_access_file(p)
    }

    fn open_writable_file(&self, p: &Path) -> Result<Box<dyn WritableFile>> {
        Ok(self.record(p, self.inner.open_writable_file(p)?))
    }

    fn open_appendable_file(&self, p: &Path) -> Result<Box<dyn WritableFile>> {
        Ok(self.record(p, self.inner.open_appendable_file(p)?))
    }

    fn sync_dir(&self, p: &Path) -> Result<()> {
        self.trace.borrow_mut().events.push(Event::SyncDir(p.to_owned()));
        self.inner.sync_dir(p)
    }

    fn exists(&self, p: &Path) -> Result<bool> { self.inner.exists(p) }
    fn children(&self, p: &Path) -> Result<Vec<PathBuf>> { self.inner.children(p) }
    fn size_of(&self, p: &Path) -> Result<usize> { self.inner.size_of(p) }
    fn delete(&self, p: &Path) -> Result<()> { self.inner.delete(p) }
    fn mkdir(&self, p: &Path) -> Result<()> { self.inner.mkdir(p) }
    fn rmdir(&self, p: &Path) -> Result<()> { self.inner.rmdir(p) }

    fn rename(&self, from: &Path, to: &Path) -> Result<()> {
        self.trace.borrow_mut().events.push(Event::Rename(from.to_owned(), to.to_owned()));
        self.inner.rename(from, to)
    }

    fn lock(&self, p: &Path) -> Result<FileLock> { self.inner.lock(p) }
    fn unlock(&self, lock: FileLock) -> Result<()> { self.inner.unlock(lock) }
    fn new_logger(&self, p: &Path) -> Result<Logger> { self.inner.new_logger(p) }
    fn micros(&self) -> u64 { self.inner.micros() }
    fn sleep_for(&self, micros: u32) { self.inner.sleep_for(micros) }
}

fn open() -> (DB, RecordingEnv) {
    let env = RecordingEnv::default();
    let mut options = in_memory();
    options.env = Rc::new(Box::new(env.clone()));
    (DB::open("db", options).unwrap(), env)
}

fn file_event(event: &Event, extension: &str) -> bool {
    matches!(event, Event::SyncFile(path) if path.extension().and_then(|s| s.to_str()) == Some(extension))
}

fn manifest_event(event: &Event) -> bool {
    matches!(event, Event::SyncFile(path) if path.file_name().and_then(|s| s.to_str()).is_some_and(|s| s.starts_with("MANIFEST-")))
}

#[test]
fn synchronous_write_syncs_log_and_nonsynchronous_write_flushes_log() {
    let (mut db, env) = open();
    env.trace.borrow_mut().events.clear();
    db.put(b"first", b"value").unwrap();
    assert!(!env.trace.borrow().events.iter().any(|e| file_event(e, "log")));

    let log = env.children(Path::new("db")).unwrap().into_iter()
        .find(|p| p.extension().and_then(|s| s.to_str()) == Some("log")).unwrap();
    assert!(env.size_of(&Path::new("db").join(log)).unwrap() > 0);

    let mut batch = WriteBatch::new();
    batch.put(b"second", b"value");
    db.write(batch, true).unwrap();
    assert!(env.trace.borrow().events.iter().any(|e| file_event(e, "log")));
}

#[test]
fn sync_failure_is_returned_and_prevents_further_writes() {
    let (mut db, env) = open();
    env.trace.borrow_mut().fail_extension = Some("log");
    let mut batch = WriteBatch::new();
    batch.put(b"failed", b"value");
    assert!(db.write(batch, true).is_err());
    assert!(db.get(b"failed").unwrap().is_none());
    assert!(db.put(b"later", b"value").is_err());
}

#[test]
fn manifest_and_current_are_synced_before_installation() {
    let (_db, env) = open();
    let trace = env.trace.borrow();
    let events = &trace.events;
    let dir = events.iter().position(|e| matches!(e, Event::SyncDir(_))).unwrap();
    let manifest = events.iter().position(manifest_event).unwrap();
    let current = events.iter().position(|e| file_event(e, "dbtmp")).unwrap();
    let rename = events.iter().position(|e| matches!(e, Event::Rename(_, to) if to.file_name().is_some_and(|n| n == "CURRENT"))).unwrap();
    assert!(dir < manifest && manifest < current && current < rename);
}

#[test]
fn compaction_syncs_table_before_manifest_installation() {
    let (mut db, env) = open();
    db.put(b"key", b"value").unwrap();
    env.trace.borrow_mut().events.clear();
    db.compact_range(b"", b"\xff").unwrap();
    let trace = env.trace.borrow();
    let events = &trace.events;
    let table = events.iter().position(|e| file_event(e, "ldb")).unwrap();
    let dir = events.iter().position(|e| matches!(e, Event::SyncDir(_))).unwrap();
    let manifest = events.iter().position(manifest_event).unwrap();
    assert!(table < dir && dir < manifest);
}

#[test]
fn table_sync_failure_aborts_compaction() {
    let (mut db, env) = open();
    db.put(b"key", b"value").unwrap();
    env.trace.borrow_mut().fail_extension = Some("ldb");
    assert!(db.compact_range(b"", b"\xff").is_err());
    assert_eq!(db.get(b"key").unwrap().as_deref(), Some(b"value".as_slice()));
    assert!(db.put(b"later", b"value").is_err());
}

#[test]
fn manifest_sync_failure_retains_uninstalled_entries_and_stops_writes() {
    let (mut db, env) = open();
    db.put(b"key", b"value").unwrap();
    env.trace.borrow_mut().fail_manifest = true;
    assert!(db.compact_range(b"", b"\xff").is_err());
    assert_eq!(db.get(b"key").unwrap().as_deref(), Some(b"value".as_slice()));
    assert!(db.put(b"later", b"value").is_err());
}

#[test]
fn failed_current_sync_removes_temporary_file() {
    let env = RecordingEnv::default();
    env.trace.borrow_mut().fail_extension = Some("dbtmp");
    let mut options = in_memory();
    options.env = Rc::new(Box::new(env.clone()));
    assert!(DB::open("db", options).is_err());
    assert!(!env.exists(Path::new("db/000001.dbtmp")).unwrap());
}

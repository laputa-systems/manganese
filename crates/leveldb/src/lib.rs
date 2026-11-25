use rusty_leveldb::{BloomPolicy, CompressorId, FilterPolicy, LdbIterator, Options, ReadOptions, Status, StatusCode, WriteBatch, DB, compressor};
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::thread;
use std::time::{Duration, Instant};

pub use rusty_leveldb as engine;

const REBUILD_SUFFIX: &str = "__tmp_for_rebuild";
const RETRY_LIMIT: Duration = Duration::from_millis(1000);
const RETRY_PAUSE: Duration = Duration::from_millis(10);
const NON_TABLE_CACHE_FILES: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compression {
    None,
    Snappy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Code {
    NotFound = 1,
    Corruption = 2,
    IoError = 3,
    InvalidArgument = 4,
    NotSupported = 5,
}

pub fn code(status: &Status) -> Code {
    match status.code {
        StatusCode::NotFound => Code::NotFound,
        StatusCode::Corruption | StatusCode::CompressionError | StatusCode::InvalidData => Code::Corruption,
        StatusCode::InvalidArgument | StatusCode::AlreadyExists => Code::InvalidArgument,
        StatusCode::NotSupported => Code::NotSupported,
        StatusCode::IOError | StatusCode::LockError | StatusCode::PermissionDenied => Code::IoError,
        StatusCode::OK | StatusCode::AsyncError | StatusCode::Unknown => {
            panic!("rusty-leveldb returned a status with no LevelDB equivalent: {status}")
        }
    }
}

#[derive(Clone, Debug)]
pub struct ChromiumOptions {
    pub paranoid_checks: bool,
    pub reuse_logs: bool,
    pub compression: Compression,
    pub bloom_filter: bool,
    pub create_if_missing: bool,
    pub error_if_exists: bool,
    pub write_buffer_size: usize,
    pub max_open_files: usize,
}

impl Default for ChromiumOptions {
    fn default() -> Self {
        Self {
            paranoid_checks: false,
            reuse_logs: true,
            compression: Compression::Snappy,
            bloom_filter: false,
            create_if_missing: false,
            error_if_exists: false,
            write_buffer_size: 4 << 20,
            max_open_files: 1000,
        }
    }
}

impl ChromiumOptions {
    pub fn engine_options(&self) -> Options {
        let mut options = Options {
            paranoid_checks: self.paranoid_checks,
            reuse_logs: self.reuse_logs,
            reuse_manifest: self.reuse_logs,
            create_if_missing: self.create_if_missing,
            error_if_exists: self.error_if_exists,
            write_buffer_size: self.write_buffer_size.clamp(64 << 10, 1 << 30),
            max_open_files: self.max_open_files.clamp(64 + NON_TABLE_CACHE_FILES, 50000),
            compressor: match self.compression {
                Compression::None => compressor::NoneCompressor::ID,
                Compression::Snappy => compressor::SnappyCompressor::ID,
            },
            ..Options::default()
        };
        options.filter_policy = self
            .bloom_filter
            .then(|| -> Rc<Box<dyn FilterPolicy>> { Rc::new(Box::new(BloomPolicy::new(10))) });
        options
    }
}

pub fn rebuild_path(path: &Path) -> PathBuf {
    let mut name = OsString::from(path.as_os_str());
    name.push(REBUILD_SUFFIX);
    PathBuf::from(name)
}

fn retry<T>(mut attempt: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    let limit = Instant::now() + RETRY_LIMIT;
    loop {
        match attempt() {
            Ok(value) => return Ok(value),
            Err(error) if Instant::now() >= limit => return Err(error),
            Err(_) => thread::sleep(RETRY_PAUSE),
        }
    }
}

fn rename(from: &Path, to: &Path) -> rusty_leveldb::Result<()> {
    if !from.exists() {
        return Ok(());
    }
    retry(|| fs::rename(from, to)).map_err(Status::from)
}

fn recognized(name: &str) -> bool {
    fn number(digits: &str) -> bool {
        !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) && digits.parse::<u64>().is_ok()
    }
    if matches!(name, "CURRENT" | "LOCK" | "LOG" | "LOG.old") {
        return true;
    }
    if let Some(digits) = name.strip_prefix("MANIFEST-") {
        return number(digits);
    }
    match name.split_once('.') {
        Some((digits, extension)) => number(digits) && matches!(extension, "log" | "sst" | "ldb" | "dbtmp"),
        None => false,
    }
}

pub fn destroy_db(path: &Path, options: &ChromiumOptions) -> rusty_leveldb::Result<()> {
    let Ok(entries) = fs::read_dir(path).and_then(|entries| entries.collect::<io::Result<Vec<_>>>()) else {
        return Ok(());
    };
    let engine = options.engine_options();
    let lock_path = path.join("LOCK");
    let lock = engine.env.lock(&lock_path)?;
    let mut result = Ok(());
    for entry in entries {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name != "LOCK" && recognized(name) {
            if let Err(error) = fs::remove_file(entry.path()) {
                if result.is_ok() {
                    result = Err(Status::from(error));
                }
            }
        }
    }
    let _ = engine.env.unlock(lock);
    let _ = fs::remove_file(&lock_path);
    let _ = fs::remove_dir(path);
    result
}

pub fn repair_db(path: &Path, options: &ChromiumOptions) -> rusty_leveldb::Result<()> {
    rusty_leveldb::repair_db(path, options.engine_options())
}

fn open(path: &Path, options: &ChromiumOptions) -> rusty_leveldb::Result<DB> {
    DB::open(path, options.engine_options())
}

pub fn open_db(path: &Path, options: &ChromiumOptions) -> rusty_leveldb::Result<DB> {
    let rebuild = rebuild_path(path);
    if rebuild.exists() && !path.join("CURRENT").exists() {
        destroy_db(path, options)?;
        rename(&rebuild, path)?;
    }
    let mut result = open(path, options);
    if result.is_err() && rebuild.exists() {
        destroy_db(path, options)?;
        rename(&rebuild, path)?;
        result = open(path, options);
    }
    if rebuild.exists() {
        let _ = destroy_db(&rebuild, options);
    }
    result
}

pub fn rewrite_db(path: &Path, options: &ChromiumOptions, db: &mut Option<DB>) -> rusty_leveldb::Result<()> {
    let rebuild = rebuild_path(path);
    if rebuild.exists() {
        destroy_db(&rebuild, options)?;
    }
    let mut copy = open_db(&rebuild, options)?;
    let mut result = Ok(());
    {
        let source = db.as_mut().expect("rewrite requires an open database");
        let mut iter = source.new_iter_with_options(ReadOptions::default())?;
        iter.seek_to_first();
        while iter.valid() {
            let (key, value) = iter.current().expect("a valid iterator has an entry");
            let mut batch = WriteBatch::default();
            batch.put(&key, &value);
            if let Err(error) = copy.write(batch, false) {
                result = Err(error);
                break;
            }
            iter.advance();
        }
    }
    drop(copy);
    if let Err(error) = result {
        let _ = destroy_db(&rebuild, options);
        return Err(error);
    }
    *db = None;
    destroy_db(path, options)?;
    rename(&rebuild, path)?;
    *db = Some(open_db(path, options)?);
    Ok(())
}

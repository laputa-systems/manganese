use rusty_leveldb::{compressor, repair_db, CompressorId, LdbIterator, Options, ReadOptions, Status, StatusCode, WriteBatch, DB};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;

fn options() -> Options {
    Options {
        paranoid_checks: true,
        compressor: compressor::SnappyCompressor::ID,
        ..Options::default()
    }
}

fn run_cli(operation: &str, path: &str) -> Result<(), Box<dyn std::error::Error>> {
    if operation == "repair" {
        repair_db(path, options())?;
        return Ok(());
    }
    if operation != "read" {
        return Err("invalid operation".into());
    }
    let mut db = DB::open(path, options())?;
    let mut iter = db.new_iter_with_options(ReadOptions { verify_checksums: true, fill_cache: true })?;
    let mut output = io::stdout().lock();
    while iter.advance() {
        let (key, value) = iter.current().ok_or("invalid iterator")?;
        for byte in key { write!(output, "{byte:02x}")?; }
        write!(output, " ")?;
        for byte in value { write!(output, "{byte:02x}")?; }
        writeln!(output)?;
    }
    iter.status()?;
    Ok(())
}

struct Input<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Input<'a> {
    fn take(&mut self, length: usize) -> Result<&'a [u8], &'static str> {
        let end = self.offset.checked_add(length).filter(|&end| end <= self.bytes.len()).ok_or("short input")?;
        let result = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(result)
    }

    fn byte(&mut self) -> Result<u8, &'static str> { Ok(self.take(1)?[0]) }
    fn short(&mut self) -> Result<usize, &'static str> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().map_err(|_| "short integer")?) as usize)
    }
    fn blob(&mut self) -> Result<&'a [u8], &'static str> {
        let size = u32::from_be_bytes(self.take(4)?.try_into().map_err(|_| "short integer")?) as usize;
        self.take(size)
    }
}

fn blob(output: &mut Vec<u8>, data: &[u8]) -> Result<(), &'static str> {
    let length = u32::try_from(data.len()).map_err(|_| "output too large")?;
    output.extend_from_slice(&length.to_be_bytes());
    output.extend_from_slice(data);
    Ok(())
}

fn code(error: &Status) -> u8 {
    match error.code {
        StatusCode::NotFound => 1,
        StatusCode::Corruption | StatusCode::CompressionError | StatusCode::InvalidData => 2,
        StatusCode::InvalidArgument => 4,
        StatusCode::NotSupported => 5,
        _ => 3,
    }
}

fn write_status(output: &mut Vec<u8>, result: rusty_leveldb::Result<()>) {
    output.push(result.as_ref().err().map(code).unwrap_or(0));
}

fn recognized_file(name: &str) -> bool {
    if matches!(name, "CURRENT" | "LOCK" | "LOG" | "LOG.old") { return true; }
    if let Some(number) = name.strip_prefix("MANIFEST-") {
        return !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit());
    }
    if let Some((number, extension)) = name.split_once('.') {
        return !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit())
            && matches!(extension, "log" | "ldb" | "sst" | "dbtmp");
    }
    false
}

fn destroy(path: &Path, opt: &Options) -> rusty_leveldb::Result<()> {
    let children: Vec<_> = match fs::read_dir(path).and_then(|entries| entries.collect()) {
        Ok(children) => children,
        Err(_) => return Ok(()),
    };
    let lock_path = path.join("LOCK");
    let lock = opt.env.lock(&lock_path)?;
    let mut result = Ok(());
    for entry in children {
        let name = entry.file_name();
        if let Some(name) = name.to_str() {
            if name != "LOCK" && recognized_file(name) {
                if let Err(error) = fs::remove_file(entry.path()) {
                    if result.is_ok() { result = Err(Status::from(error)); }
                }
            }
        }
    }
    let _ = opt.env.unlock(lock);
    let _ = fs::remove_file(lock_path);
    let _ = fs::remove_dir(path);
    result
}

fn record(data: &[u8], root: &Path) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut input = Input { bytes: data, offset: 0 };
    let flags = input.byte()?;
    let mut opt = options();
    opt.paranoid_checks = flags & 1 != 0;
    opt.reuse_logs = flags & 2 != 0;
    opt.compressor = if flags & 4 != 0 { compressor::SnappyCompressor::ID } else { compressor::NoneCompressor::ID };
    let read_options = ReadOptions { verify_checksums: flags & 8 != 0, fill_cache: flags & 16 == 0 };
    let path = root.join("db");
    match fs::remove_dir_all(&path) { Ok(()) => {}, Err(e) if e.kind() == io::ErrorKind::NotFound => {}, Err(e) => return Err(e.into()) }
    fs::create_dir_all(&path)?;
    for _ in 0..input.short()? {
        let name = std::str::from_utf8(input.blob()?)?;
        if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\\') {
            return Err("invalid fixture filename".into());
        }
        fs::write(path.join(name), input.blob()?)?;
    }
    let mut db: Option<DB> = None;
    let mut snapshots = HashMap::new();
    let mut output = Vec::new();
    for _ in 0..input.short()? {
        match input.byte()? {
            0 => {
                snapshots.clear();
                db = None;
                match DB::open(&path, opt.clone()) {
                    Ok(open) => { db = Some(open); output.push(0); }
                    Err(error) => output.push(code(&error)),
                }
            }
            1 | 2 => {
                let operation = data[input.offset - 1];
                let sync = input.byte()? != 0;
                let key = input.blob()?;
                let mut batch = WriteBatch::default();
                if operation == 1 { batch.put(key, input.blob()?); } else { batch.delete(key); }
                write_status(&mut output, db.as_mut().ok_or("write without open database")?.write(batch, sync));
            }
            3 => {
                let sync = input.byte()? != 0;
                let mut batch = WriteBatch::default();
                for _ in 0..input.short()? {
                    let operation = input.byte()?;
                    let key = input.blob()?;
                    match operation {
                        1 => batch.put(key, input.blob()?),
                        2 => batch.delete(key),
                        _ => return Err("invalid batch operation".into()),
                    }
                }
                write_status(&mut output, db.as_mut().ok_or("batch without open database")?.write(batch, sync));
            }
            4 => {
                let snapshot = input.byte()?;
                let key = input.blob()?;
                let database = db.as_mut().ok_or("get without open database")?;
                let result = if snapshot == 255 { database.get_with_options(key, read_options) }
                    else { database.get_at_with_options(snapshots.get(&snapshot).ok_or("unknown snapshot")?, key, read_options) };
                match result {
                    Ok(Some(value)) => { output.push(0); blob(&mut output, &value)?; }
                    Ok(None) => output.push(1),
                    Err(error) => output.push(code(&error)),
                }
            }
            5 => {
                let id = input.byte()?;
                if id == 255 { return Err("invalid snapshot id".into()); }
                snapshots.insert(id, db.as_mut().ok_or("snapshot without open database")?.get_snapshot());
                output.push(0);
            }
            6 => { snapshots.remove(&input.byte()?); output.push(0); }
            7 => {
                let snapshot = input.byte()?;
                let direction = input.byte()?;
                let key = input.blob()?;
                let database = db.as_mut().ok_or("iterator without open database")?;
                let result = if snapshot == 255 { database.new_iter_with_options(read_options) }
                    else { database.new_iter_at_with_options(snapshots.get(&snapshot).ok_or("unknown snapshot")?.clone(), read_options) };
                match result {
                    Err(error) => output.push(code(&error)),
                    Ok(mut iter) => {
                        match direction {
                            0 => iter.seek_to_first(),
                            1 => { iter.seek_to_first(); while iter.valid() {
                                let current = iter.current().ok_or("invalid iterator")?;
                                if !iter.advance() { iter.seek(&current.0); break; }
                            } },
                            2 => iter.seek(key),
                            _ => return Err("invalid iterator direction".into()),
                        }
                        let mut entries = Vec::new();
                        while iter.valid() {
                            let (key, value) = iter.current().ok_or("invalid iterator")?;
                            entries.push((key, value));
                            if direction == 1 { iter.prev(); } else { iter.advance(); }
                        }
                        match iter.status() {
                            Err(error) => output.push(code(&error)),
                            Ok(()) => {
                                output.push(0);
                                output.extend_from_slice(&u32::try_from(entries.len())?.to_be_bytes());
                                for (key, value) in entries { blob(&mut output, &key)?; blob(&mut output, &value)?; }
                            }
                        }
                    }
                }
            }
            8 => {
                let from = input.blob()?;
                let to = input.blob()?;
                write_status(&mut output, db.as_mut().ok_or("compact without open database")?.compact_range(from, to));
            }
            9 => {
                snapshots.clear();
                let result = db.as_mut().ok_or("close without open database")?.close();
                db = None;
                write_status(&mut output, result);
            }
            10 => {
                snapshots.clear();
                db = None;
                write_status(&mut output, repair_db(&path, opt.clone()));
            }
            11 => {
                snapshots.clear();
                db = None;
                write_status(&mut output, destroy(&path, &opt));
            }
            12 => {
                if db.is_some() { return Err("alteration requires a closed database".into()); }
                let name = std::str::from_utf8(input.blob()?)?;
                if !recognized_file(name) { return Err("invalid alteration filename".into()); }
                let length = u64::from_be_bytes(input.take(8)?.try_into()?);
                let result = fs::OpenOptions::new().write(true).open(path.join(name)).and_then(|file| file.set_len(length));
                write_status(&mut output, result.map_err(Status::from));
            }
            _ => return Err("invalid operation".into()),
        }
    }
    if input.offset != data.len() { return Err("trailing input".into()); }
    Ok(output)
}

fn serve() -> Result<(), Box<dyn std::error::Error>> {
    let root = env::var_os("GOLDENS_SCRATCH").ok_or("GOLDENS_SCRATCH is required")?;
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let mut header = [0; 4];
    input.read_exact(&mut header)?;
    if &header != b"MGB1" { return Err("invalid protocol".into()); }
    output.write_all(b"MGB1")?;
    output.flush()?;
    loop {
        if input.read(&mut header[..1])? == 0 { break; }
        input.read_exact(&mut header[1..])?;
        let mut request = vec![0; u32::from_be_bytes(header) as usize];
        input.read_exact(&mut request)?;
        let response = record(&request, Path::new(&root))?;
        output.write_all(&u32::try_from(response.len())?.to_be_bytes())?;
        output.write_all(&response)?;
        output.flush()?;
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args().collect();
    if args.len() == 3 {
        run_cli(&args[1], &args[2])
    } else if args.len() == 1 { serve() } else { Err("expected operation and database path".into()) }
}

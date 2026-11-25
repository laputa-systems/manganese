use leveldb::engine::{LdbIterator, ReadOptions, Snapshot, WriteBatch, DB};
use leveldb::{ChromiumOptions, Compression, code, destroy_db, open_db, rebuild_path, repair_db, rewrite_db};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

type Failure = Box<dyn std::error::Error>;

const CLOSED: u8 = 6;
const NO_SNAPSHOT: u8 = 255;

struct Input<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Input<'a> {
    fn take(&mut self, length: usize) -> Result<&'a [u8], Failure> {
        let end = self.offset.checked_add(length).filter(|&end| end <= self.bytes.len()).ok_or("short input")?;
        let result = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(result)
    }

    fn byte(&mut self) -> Result<u8, Failure> {
        Ok(self.take(1)?[0])
    }

    fn short(&mut self) -> Result<u16, Failure> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into()?))
    }

    fn int(&mut self) -> Result<u32, Failure> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into()?))
    }

    fn long(&mut self) -> Result<u64, Failure> {
        Ok(u64::from_be_bytes(self.take(8)?.try_into()?))
    }

    fn blob(&mut self) -> Result<&'a [u8], Failure> {
        let length = self.int()? as usize;
        self.take(length)
    }
}

fn blob(output: &mut Vec<u8>, data: &[u8]) -> Result<(), Failure> {
    output.extend_from_slice(&u32::try_from(data.len())?.to_be_bytes());
    output.extend_from_slice(data);
    Ok(())
}

fn status(result: &leveldb::engine::Result<()>) -> u8 {
    match result {
        Ok(()) => 0,
        Err(error) => code(error) as u8,
    }
}

fn read_options(flags: u8) -> ReadOptions {
    ReadOptions { verify_checksums: flags & 1 != 0, fill_cache: flags & 2 != 0 }
}

fn file_kind(name: &str) -> Option<(u8, u64)> {
    fn number(digits: &str) -> Option<u64> {
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) { None } else { digits.parse().ok() }
    }
    if name == "CURRENT" {
        return Some((3, 0));
    }
    if let Some(digits) = name.strip_prefix("MANIFEST-") {
        return number(digits).map(|n| (2, n));
    }
    let (digits, extension) = name.split_once('.')?;
    let kind = match extension {
        "log" => 0,
        "ldb" | "sst" => 1,
        "dbtmp" => 4,
        _ => return None,
    };
    number(digits).map(|n| (kind, n))
}

fn select_file(directory: &Path, kind: u8, rank: u8) -> Option<PathBuf> {
    let mut files: Vec<(u64, PathBuf)> = fs::read_dir(directory)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let (found, number) = file_kind(entry.file_name().to_str()?)?;
            (found == kind).then(|| (number, entry.path()))
        })
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0));
    files.into_iter().nth(rank as usize).map(|(_, path)| path)
}

fn entry(output: &mut Vec<u8>, iter: &impl LdbIterator) -> Result<(), Failure> {
    let (key, value) = iter.current().ok_or("a valid iterator has an entry")?;
    blob(output, &key)?;
    blob(output, &value)
}

fn seek_to_last(iter: &mut impl LdbIterator) {
    iter.seek_to_first();
    while iter.valid() {
        let (key, _) = iter.current().expect("a valid iterator has an entry");
        if !iter.advance() {
            iter.seek(&key);
            break;
        }
    }
}

fn iterate(input: &mut Input, iter: &mut impl LdbIterator, output: &mut Vec<u8>) -> Result<(), Failure> {
    for _ in 0..input.byte()? {
        let movement = input.byte()?;
        match movement {
            0 => iter.seek_to_first(),
            1 => seek_to_last(iter),
            2 => iter.seek(input.blob()?),
            3 => {
                if iter.valid() {
                    iter.advance();
                }
            }
            4 => {
                if iter.valid() {
                    iter.prev();
                }
            }
            5 | 6 => {
                let mut entries = Vec::new();
                let mut count = 0u32;
                while iter.valid() {
                    entry(&mut entries, iter)?;
                    count += 1;
                    if movement == 5 { iter.advance(); } else { iter.prev(); }
                }
                output.extend_from_slice(&count.to_be_bytes());
                output.extend_from_slice(&entries);
                continue;
            }
            _ => return Err("invalid iterator movement".into()),
        }
        if iter.valid() {
            output.push(1);
            entry(output, iter)?;
        } else {
            output.push(0);
        }
    }
    output.push(status(&iter.status()));
    Ok(())
}

fn fixture_name_ok(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains('/')
}

fn remove_tree(path: &Path) -> io::Result<()> {
    match fs::remove_dir_all(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

fn xor_file(path: &Path, offset: u64, mask: &[u8]) -> io::Result<()> {
    let mut file = fs::OpenOptions::new().read(true).write(true).open(path)?;
    let size = file.metadata()?.len();
    if offset >= size {
        return Ok(());
    }
    let length = mask.len().min((size - offset) as usize);
    let mut bytes = vec![0; length];
    file.seek(SeekFrom::Start(offset))?;
    file.read_exact(&mut bytes)?;
    for (byte, flip) in bytes.iter_mut().zip(mask) {
        *byte ^= flip;
    }
    file.seek(SeekFrom::Start(offset))?;
    file.write_all(&bytes)
}

struct Session {
    options: ChromiumOptions,
    path: PathBuf,
    db: Option<DB>,
    snapshots: HashMap<u8, Snapshot>,
}

impl Session {
    fn close(&mut self) {
        self.snapshots.clear();
        if let Some(mut db) = self.db.take() {
            let _ = db.close();
        }
    }

    fn snapshot(&self, id: u8) -> Result<Option<Snapshot>, Failure> {
        if id == NO_SNAPSHOT {
            return Ok(None);
        }
        Ok(Some(self.snapshots.get(&id).ok_or("unknown snapshot")?.clone()))
    }
}

fn record(data: &[u8], root: &Path) -> Result<Vec<u8>, Failure> {
    let mut input = Input { bytes: data, offset: 0 };
    let flags = input.byte()?;
    if flags & !63 != 0 {
        return Err("invalid option flags".into());
    }
    let options = ChromiumOptions {
        paranoid_checks: flags & 1 != 0,
        reuse_logs: flags & 2 != 0,
        compression: if flags & 4 != 0 { Compression::Snappy } else { Compression::None },
        bloom_filter: flags & 8 != 0,
        create_if_missing: flags & 16 != 0,
        error_if_exists: flags & 32 != 0,
        write_buffer_size: input.int()? as usize,
        max_open_files: input.int()? as usize,
    };
    let path = root.join("db");
    let rebuild = rebuild_path(&path);
    remove_tree(&path)?;
    remove_tree(&rebuild)?;
    for _ in 0..input.short()? {
        let directory = if input.byte()? != 0 { &rebuild } else { &path };
        let name = std::str::from_utf8(input.blob()?)?;
        let contents = input.blob()?;
        if !fixture_name_ok(name) {
            return Err("invalid fixture filename".into());
        }
        fs::create_dir_all(directory)?;
        fs::write(directory.join(name), contents)?;
    }
    let mut session = Session { options, path, db: None, snapshots: HashMap::new() };
    let mut output = Vec::new();
    for _ in 0..input.short()? {
        let operation = input.byte()?;
        match operation {
            0 => {
                session.close();
                match open_db(&session.path, &session.options) {
                    Ok(db) => {
                        session.db = Some(db);
                        output.push(0);
                    }
                    Err(error) => output.push(code(&error) as u8),
                }
            }
            1..=3 => {
                let sync = input.byte()? != 0;
                let mut batch = WriteBatch::default();
                let count = if operation == 3 { input.short()? } else { 1 };
                for _ in 0..count {
                    let action = if operation == 3 { input.byte()? } else { operation };
                    let key = input.blob()?;
                    match action {
                        1 => batch.put(key, input.blob()?),
                        2 => batch.delete(key),
                        _ => return Err("invalid batch operation".into()),
                    }
                }
                output.push(match session.db.as_mut() {
                    Some(db) => status(&db.write(batch, sync)),
                    None => CLOSED,
                });
            }
            4 => {
                let read = read_options(input.byte()?);
                let id = input.byte()?;
                let key = input.blob()?;
                let snapshot = session.snapshot(id)?;
                let Some(db) = session.db.as_mut() else {
                    output.push(CLOSED);
                    continue;
                };
                let result = match snapshot {
                    Some(snapshot) => db.get_at_with_options(&snapshot, key, read),
                    None => db.get_with_options(key, read),
                };
                match result {
                    Ok(Some(value)) => {
                        output.push(0);
                        blob(&mut output, &value)?;
                    }
                    Ok(None) => output.push(1),
                    Err(error) => output.push(code(&error) as u8),
                }
            }
            5 => {
                let id = input.byte()?;
                if id == NO_SNAPSHOT {
                    return Err("invalid snapshot id".into());
                }
                let Some(db) = session.db.as_mut() else {
                    output.push(CLOSED);
                    continue;
                };
                let snapshot = db.get_snapshot();
                session.snapshots.insert(id, snapshot);
                output.push(0);
            }
            6 => {
                session.snapshots.remove(&input.byte()?);
                output.push(0);
            }
            7 => {
                let read = read_options(input.byte()?);
                let id = input.byte()?;
                let snapshot = session.snapshot(id)?;
                let Some(db) = session.db.as_mut() else {
                    for _ in 0..input.byte()? {
                        if input.byte()? == 2 {
                            input.blob()?;
                        }
                    }
                    output.push(CLOSED);
                    continue;
                };
                let iter = match snapshot {
                    Some(snapshot) => db.new_iter_at_with_options(snapshot, read),
                    None => db.new_iter_with_options(read),
                };
                let mut iter = iter?;
                output.push(0);
                iterate(&mut input, &mut iter, &mut output)?;
            }
            8 => {
                let bounds = input.byte()?;
                let begin = if bounds & 1 != 0 { Some(input.blob()?) } else { None };
                let end = if bounds & 2 != 0 { Some(input.blob()?) } else { None };
                let Some(db) = session.db.as_mut() else {
                    output.push(CLOSED);
                    continue;
                };
                let _ = db.compact_range(begin.unwrap_or(b""), end.unwrap_or(&[0xff; 64]));
                output.push(0);
            }
            9 => {
                session.close();
                output.push(0);
            }
            10 => {
                session.close();
                output.push(status(&repair_db(&session.path, &session.options)));
            }
            11 => {
                session.close();
                output.push(status(&destroy_db(&session.path, &session.options)));
            }
            12..=14 => {
                if session.db.is_some() {
                    return Err("file alteration requires a closed database".into());
                }
                let kind = input.byte()?;
                let rank = input.byte()?;
                let target = select_file(&session.path, kind, rank);
                let found = match operation {
                    12 => {
                        let keep = input.long()?;
                        target.map(|target| -> io::Result<()> {
                            let file = fs::OpenOptions::new().write(true).open(&target)?;
                            let size = file.metadata()?.len();
                            file.set_len(keep.min(size))
                        })
                    }
                    13 => {
                        let offset = input.long()?;
                        let mask = input.blob()?;
                        target.map(|target| xor_file(&target, offset, mask))
                    }
                    _ => target.map(|target| fs::remove_file(target)),
                };
                match found {
                    Some(result) => {
                        result?;
                        output.push(0);
                    }
                    None => output.push(1),
                }
            }
            15 => {
                if session.db.is_none() {
                    output.push(CLOSED);
                    continue;
                }
                session.snapshots.clear();
                output.push(status(&rewrite_db(&session.path, &session.options, &mut session.db)));
            }
            16 => {
                session.close();
                let mut files: Vec<(String, PathBuf)> = Vec::new();
                for entry in fs::read_dir(&session.path).into_iter().flatten() {
                    let entry = entry?;
                    let name = entry.file_name().into_string().map_err(|_| "non-UTF-8 database file name")?;
                    if !matches!(name.as_str(), "LOCK" | "LOG" | "LOG.old") {
                        files.push((name, entry.path()));
                    }
                }
                files.sort();
                output.push(0);
                output.extend_from_slice(&u16::try_from(files.len())?.to_be_bytes());
                for (name, file) in files {
                    blob(&mut output, name.as_bytes())?;
                    blob(&mut output, &fs::read(file)?)?;
                }
            }
            _ => return Err("invalid operation".into()),
        }
    }
    if input.offset != data.len() {
        return Err("trailing input".into());
    }
    session.close();
    Ok(output)
}

fn serve() -> Result<(), Failure> {
    let root = env::var_os("GOLDENS_SCRATCH").ok_or("GOLDENS_SCRATCH is required")?;
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let mut header = [0; 4];
    input.read_exact(&mut header)?;
    if &header != b"MGB1" {
        return Err("invalid protocol".into());
    }
    output.write_all(b"MGB1")?;
    output.flush()?;
    loop {
        if input.read(&mut header[..1])? == 0 {
            return Ok(());
        }
        input.read_exact(&mut header[1..])?;
        let mut request = vec![0; u32::from_be_bytes(header) as usize];
        input.read_exact(&mut request)?;
        let response = record(&request, Path::new(&root))?;
        output.write_all(&u32::try_from(response.len())?.to_be_bytes())?;
        output.write_all(&response)?;
        output.flush()?;
    }
}

fn hex(output: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    bytes.iter().try_for_each(|byte| write!(output, "{byte:02x}"))
}

fn read_database(path: &Path) -> Result<(), Failure> {
    let mut output = io::stdout().lock();
    let mut db = match open_db(path, &ChromiumOptions::default()) {
        Ok(db) => db,
        Err(error) => {
            writeln!(output, "open {}", code(&error) as u8)?;
            return Ok(());
        }
    };
    let mut iter = db.new_iter_with_options(ReadOptions { verify_checksums: true, fill_cache: true })?;
    iter.seek_to_first();
    while iter.valid() {
        let (key, value) = iter.current().ok_or("a valid iterator has an entry")?;
        hex(&mut output, &key)?;
        write!(output, " ")?;
        hex(&mut output, &value)?;
        writeln!(output)?;
        iter.advance();
    }
    writeln!(output, "status {}", status(&iter.status()))?;
    Ok(())
}

fn main() -> Result<(), Failure> {
    let args: Vec<_> = env::args_os().collect();
    match args.as_slice() {
        [_] => serve(),
        [_, operation, path] if operation == "read" => read_database(Path::new(path)),
        _ => Err("usage: leveldb [read DATABASE]".into()),
    }
}


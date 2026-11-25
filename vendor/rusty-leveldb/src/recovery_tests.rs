use crate::log::{LogReader, LogWriter};
use crate::{in_memory, Options, StatusCode, WriteBatch, DB};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

fn database_with_records(records: &[(&[u8], &[u8])]) -> (Options, PathBuf, Vec<u8>) {
    let options = in_memory();
    {
        let mut db = DB::open("db", options.clone()).unwrap();
        for (key, value) in records {
            db.put(key, value).unwrap();
        }
    }
    let log = options.env.children(Path::new("db")).unwrap().into_iter()
        .find(|p| p.extension().and_then(|s| s.to_str()) == Some("log")).unwrap();
    let path = Path::new("db").join(log);
    let mut contents = Vec::new();
    options.env.open_sequential_file(&path).unwrap().read_to_end(&mut contents).unwrap();
    (options, path, contents)
}

fn overwrite(options: &Options, path: &Path, contents: &[u8]) {
    options.env.open_writable_file(path).unwrap().write_all(contents).unwrap();
}

fn assert_corrupt(options: Options) {
    match DB::open("db", options) {
        Err(error) => assert_eq!(error.code, StatusCode::Corruption),
        Ok(_) => panic!("corrupt database opened"),
    }
}

#[test]
fn nonparanoid_recovery_skips_a_corrupt_block_and_continues() {
    let large = vec![b'x'; 40000];
    let (mut options, log, mut contents) = database_with_records(&[
        (b"first", b"one"), (b"middle", &large), (b"last", b"three"),
    ]);
    contents[0] ^= 1;
    overwrite(&options, &log, &contents);
    options.paranoid_checks = false;
    let mut db = DB::open("db", options).unwrap();
    assert!(db.get(b"first").is_none());
    assert!(db.get(b"middle").is_none());
    assert_eq!(db.get(b"last").as_deref(), Some(b"three".as_slice()));
}

#[test]
fn paranoid_recovery_rejects_a_corrupt_log() {
    let (mut options, log, mut contents) = database_with_records(&[(b"first", b"one")]);
    contents[0] ^= 1;
    overwrite(&options, &log, &contents);
    options.paranoid_checks = true;
    assert_corrupt(options);
}

#[test]
fn truncated_log_tail_preserves_complete_commits_in_both_modes() {
    let (_, _, full) = database_with_records(&[(b"first", b"one"), (b"last", b"three")]);
    let first_end = 7 + u16::from_le_bytes([full[4], full[5]]) as usize;
    for paranoid in [false, true] {
        for cut in first_end..full.len() {
            let (mut options, log, _) = database_with_records(&[
                (b"first", b"one"), (b"last", b"three"),
            ]);
            overwrite(&options, &log, &full[..cut]);
            options.paranoid_checks = paranoid;
            let mut db = DB::open("db", options).unwrap();
            assert_eq!(db.get(b"first").as_deref(), Some(b"one".as_slice()));
            assert!(db.get(b"last").is_none());
        }
    }
}

#[test]
fn malformed_batch_is_reported_or_skipped_without_panicking() {
    for paranoid in [false, true] {
        let (mut options, log, _) = database_with_records(&[]);
        let mut malformed = vec![0; 12];
        malformed[0] = 1;
        malformed[8] = 1;
        malformed.extend_from_slice(&[1, 100, b'x']);
        let mut valid = WriteBatch::new();
        valid.put(b"last", b"three");
        let mut contents = Vec::new();
        {
            let mut writer = LogWriter::new(&mut contents);
            writer.add_record(&malformed).unwrap();
            writer.add_record(&valid.encode(2)).unwrap();
        }
        overwrite(&options, &log, &contents);
        options.paranoid_checks = paranoid;
        if paranoid {
            assert_corrupt(options);
        } else {
            let mut db = DB::open("db", options).unwrap();
            assert_eq!(db.get(b"last").as_deref(), Some(b"three".as_slice()));
        }
    }
}

#[test]
fn nonparanoid_recovery_keeps_entries_parsed_before_a_bad_count() {
    let (mut options, log, _) = database_with_records(&[]);
    let mut first = WriteBatch::new();
    first.put(b"first", b"one");
    let mut malformed = first.encode(1);
    malformed[8] = 2;
    let mut second = WriteBatch::new();
    second.put(b"second", b"two");
    let mut contents = Vec::new();
    {
        let mut writer = LogWriter::new(&mut contents);
        writer.add_record(&malformed).unwrap();
        writer.add_record(&second.encode(3)).unwrap();
    }
    overwrite(&options, &log, &contents);
    options.paranoid_checks = false;
    let mut db = DB::open("db", options).unwrap();
    assert_eq!(db.get(b"first").as_deref(), Some(b"one".as_slice()));
    assert_eq!(db.get(b"second").as_deref(), Some(b"two".as_slice()));
}

#[test]
fn corrupt_manifest_record_is_not_silently_discarded() {
    let (options, _, _) = database_with_records(&[]);
    let manifest = options.env.children(Path::new("db")).unwrap().into_iter()
        .find(|p| p.file_name().and_then(|s| s.to_str()).is_some_and(|s| s.starts_with("MANIFEST-"))).unwrap();
    let path = Path::new("db").join(manifest);
    let mut contents = Vec::new();
    options.env.open_sequential_file(&path).unwrap().read_to_end(&mut contents).unwrap();
    let start = contents.len();
    {
        let mut writer = LogWriter::new_with_off(&mut contents, start);
        writer.add_record(b"bad manifest record").unwrap();
    }
    contents[start] ^= 1;
    overwrite(&options, &path, &contents);
    assert_corrupt(options);
}

#[test]
fn log_reader_accepts_short_underlying_reads() {
    struct ShortReader<R>(R);

    impl<R: Read> Read for ShortReader<R> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let count = buf.len().min(2);
            self.0.read(&mut buf[..count])
        }
    }

    let mut bytes = Vec::new();
    LogWriter::new(&mut bytes).add_record(b"complete record").unwrap();
    let mut reader = LogReader::new(ShortReader(bytes.as_slice()), true);
    let mut record = Vec::new();
    assert_eq!(reader.read(&mut record).unwrap(), 15);
    assert_eq!(record, b"complete record");
}

#[test]
fn log_reader_stops_after_an_io_error() {
    struct FailingReader;

    impl Read for FailingReader {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("injected read failure"))
        }
    }

    let mut reader = LogReader::new(FailingReader, true);
    let mut record = Vec::new();
    assert!(reader.read_record(&mut record).is_err());
    assert_eq!(reader.read_record(&mut record).unwrap(), None);
}

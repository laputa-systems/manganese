// Copyright (c) 2011 The LevelDB Authors. See LEVELDB-LICENSE for redistribution terms.

use crate::crc;
use crate::db_impl::build_table;
use crate::env::{RandomAccess, WritableFile};
use crate::error::{err, Result, Status, StatusCode};
use crate::log::{unmask_crc, LogReader, LogWriter};
use crate::memtable::MemTable;
use crate::options::Options;
use crate::table_builder::{TableBuilder, FULL_FOOTER_LENGTH, MAGIC_FOOTER_ENCODED};
use crate::table_cache::table_file_name;
use crate::types::{parse_file_name, FileMetaData, FileNum, FileType, SequenceNumber};
use crate::version_edit::VersionEdit;
use crate::version_set::{manifest_file_name, set_current_file};
use crate::write_batch::WriteBatch;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

/// Rebuilds a damaged database descriptor from its surviving logs and tables.
pub fn repair_db<P: AsRef<Path>>(name: P, opt: Options) -> Result<()> {
    Repairer::new(name.as_ref(), opt).run()
}

struct TableInfo {
    meta: FileMetaData,
    max_sequence: SequenceNumber,
}

struct Repairer {
    name: PathBuf,
    opt: Options,
    manifests: Vec<PathBuf>,
    logs: Vec<FileNum>,
    table_numbers: Vec<FileNum>,
    tables: Vec<TableInfo>,
    next_file: FileNum,
}

impl Repairer {
    fn new(name: &Path, opt: Options) -> Self {
        Self {
            name: name.to_owned(), opt, manifests: Vec::new(), logs: Vec::new(),
            table_numbers: Vec::new(), tables: Vec::new(), next_file: 1,
        }
    }

    fn run(mut self) -> Result<()> {
        self.find_files()?;
        for number in self.logs.clone() {
            let path = self.name.join(format!("{number:06}.log"));
            if let Err(error) = self.convert_log(number) {
                log!(self.opt.log, "ignoring log conversion error: {}", error);
            }
            self.archive(&path);
        }
        for number in self.table_numbers.clone() {
            self.scan_table(number);
        }
        self.write_descriptor()
    }

    fn find_files(&mut self) -> Result<()> {
        let files = self.opt.env.children(&self.name)?;
        if files.is_empty() {
            return err(StatusCode::IOError, "repair found no files");
        }
        for file in files {
            let Ok((number, kind)) = parse_file_name(&file) else { continue; };
            if kind == FileType::Descriptor {
                self.manifests.push(self.name.join(file));
                continue;
            }
            self.next_file = self.next_file.max(number.checked_add(1)
                .ok_or_else(|| Status::new(StatusCode::Corruption, "file number overflow"))?);
            match kind {
                FileType::Log => self.logs.push(number),
                FileType::Table if number != 0 => self.table_numbers.push(number),
                _ => {}
            }
        }
        Ok(())
    }

    fn allocate_file(&mut self) -> Result<FileNum> {
        let number = self.next_file;
        self.next_file = number.checked_add(1)
            .ok_or_else(|| Status::new(StatusCode::Corruption, "file number overflow"))?;
        Ok(number)
    }

    fn convert_log(&mut self, number: FileNum) -> Result<()> {
        let path = self.name.join(format!("{number:06}.log"));
        let file = self.opt.env.open_sequential_file(&path)?;
        let mut reader = LogReader::new(file, false);
        let mut mem = MemTable::new(self.opt.cmp.clone());
        let mut record = Vec::new();
        let mut batch = WriteBatch::new();
        loop {
            let length = match reader.read_record(&mut record) {
                Ok(Some(length)) => length,
                Ok(None) => break,
                Err(error) => {
                    log!(self.opt.log, "ignoring damaged log record: {}", error);
                    continue;
                }
            };
            if length < 12 { continue; }
            batch.set_contents(&record);
            if let Err(error) = batch.insert_recovered(batch.sequence(), &mut mem) {
                log!(self.opt.log, "ignoring damaged write batch: {}", error);
            }
        }
        let table_number = self.allocate_file()?;
        let meta = build_table(&self.name, &self.opt, mem.iter(), table_number)?;
        if meta.size > 0 {
            self.table_numbers.push(table_number);
        }
        Ok(())
    }

    fn scan_table(&mut self, number: FileNum) {
        let primary = table_file_name(&self.name, number);
        let legacy = primary.with_extension("sst");
        let (path, size) = match self.opt.env.size_of(&primary) {
            Ok(size) => (primary, size),
            Err(_) => match self.opt.env.size_of(&legacy) {
                Ok(size) => (legacy, size),
                Err(error) => {
                    log!(self.opt.log, "dropping missing table: {}", error);
                    self.archive(&primary);
                    self.archive(&legacy);
                    return;
                }
            },
        };
        let scan = self.opt.env.open_random_access_file(&path)
            .and_then(|file| scan_table(file.as_ref(), size, &self.opt));
        let (entries, damaged) = match scan {
            Ok(result) => result,
            Err(error) => {
                log!(self.opt.log, "dropping unreadable table: {}", error);
                self.archive(&path);
                return;
            }
        };
        let mut info = TableInfo {
            meta: FileMetaData { num: number, size, ..Default::default() },
            max_sequence: 0,
        };
        let mut found = false;
        for (key, _) in &entries {
            if key.len() < 8 { continue; }
            let tag = u64::from_le_bytes(key[key.len() - 8..].try_into().unwrap());
            if tag & 0xff > 1 { continue; }
            if !found {
                info.meta.smallest = key.clone().into();
                found = true;
            }
            info.meta.largest = key.clone().into();
            info.max_sequence = info.max_sequence.max(tag >> 8);
        }
        if damaged {
            self.repair_table(&path, info, &entries);
        } else {
            self.tables.push(info);
        }
    }

    fn repair_table(&mut self, original: &Path, mut info: TableInfo, entries: &[(Vec<u8>, Vec<u8>)]) {
        let number = match self.allocate_file() {
            Ok(number) => number,
            Err(error) => {
                log!(self.opt.log, "table repair failed: {}", error);
                return;
            }
        };
        let copy = table_file_name(&self.name, number);
        let result = (|| -> Result<()> {
            let file = self.opt.env.open_writable_file(&copy)?;
            let mut builder = TableBuilder::new(self.opt.clone(), BufWriter::new(file));
            for (key, value) in entries {
                if key.len() >= 8 { builder.add(key, value)?; }
            }
            self.archive(original);
            if builder.entries() == 0 {
                drop(builder);
                let _ = self.opt.env.delete(&copy);
                return Ok(());
            }
            let (size, mut file) = builder.finish_with_writer()?;
            file.sync()?;
            info.meta.size = size;
            self.opt.env.rename(&copy, &table_file_name(&self.name, info.meta.num))?;
            self.tables.push(info);
            Ok(())
        })();
        if let Err(error) = result {
            let _ = self.opt.env.delete(&copy);
            log!(self.opt.log, "table repair failed: {}", error);
        }
    }

    fn write_descriptor(&mut self) -> Result<()> {
        let mut edit = VersionEdit::new();
        edit.set_comparator_name(self.opt.cmp.id());
        edit.set_log_num(0);
        edit.set_next_file(self.next_file);
        edit.set_last_seq(self.tables.iter().map(|table| table.max_sequence).max().unwrap_or(0));
        for table in &self.tables {
            edit.add_file(0, table.meta.clone());
        }
        let temp = self.name.join("000001.dbtmp");
        let write = (|| -> Result<()> {
            let file = self.opt.env.open_writable_file(&temp)?;
            let mut writer = LogWriter::new(file);
            writer.add_record(&edit.encode())?;
            writer.sync()
        })();
        if let Err(error) = write {
            let _ = self.opt.env.delete(&temp);
            return Err(error);
        }
        for manifest in self.manifests.clone() {
            self.archive(&manifest);
        }
        if let Err(error) = self.opt.env.rename(&temp, &manifest_file_name(&self.name, 1)) {
            let _ = self.opt.env.delete(&temp);
            return Err(error);
        }
        self.opt.env.sync_dir(&self.name)?;
        set_current_file(self.opt.env.as_ref().as_ref(), &self.name, 1)
    }

    fn archive(&self, path: &Path) {
        let Some(filename) = path.file_name() else { return; };
        let directory = self.name.join("lost");
        let _ = self.opt.env.mkdir(&directory);
        if let Err(error) = self.opt.env.rename(path, &directory.join(filename)) {
            log!(self.opt.log, "archive failed: {}", error);
        }
    }
}

type Entry = (Vec<u8>, Vec<u8>);

fn read_at(file: &dyn RandomAccess, file_size: usize, offset: usize, size: usize) -> Result<Vec<u8>> {
    if offset.checked_add(size).is_none_or(|end| end > file_size) {
        return err(StatusCode::Corruption, "table block outside file");
    }
    let mut bytes = vec![0; size];
    let mut done = 0;
    while done < size {
        let n = file.read_at(offset + done, &mut bytes[done..])?;
        if n == 0 {
            return err(StatusCode::IOError, "short table read");
        }
        done += n;
    }
    Ok(bytes)
}

fn varint(data: &[u8], cursor: &mut usize, max_bytes: usize) -> Result<u64> {
    let mut value = 0u64;
    for index in 0..max_bytes {
        let byte = *data.get(*cursor)
            .ok_or_else(|| Status::new(StatusCode::Corruption, "truncated varint"))?;
        *cursor += 1;
        value |= u64::from(byte & 0x7f) << (index * 7);
        if byte & 0x80 == 0 { return Ok(value); }
    }
    err(StatusCode::Corruption, "invalid varint")
}

fn handle(data: &[u8], cursor: &mut usize) -> Result<(usize, usize)> {
    let offset = usize::try_from(varint(data, cursor, 10)?)
        .map_err(|_| Status::new(StatusCode::Corruption, "block offset too large"))?;
    let size = usize::try_from(varint(data, cursor, 10)?)
        .map_err(|_| Status::new(StatusCode::Corruption, "block size too large"))?;
    Ok((offset, size))
}

fn read_block(file: &dyn RandomAccess, file_size: usize, opt: &Options,
              offset: usize, size: usize, verify: bool) -> Result<Vec<u8>> {
    let total = size.checked_add(5)
        .ok_or_else(|| Status::new(StatusCode::Corruption, "block size overflow"))?;
    let block = read_at(file, file_size, offset, total)?;
    let compression = block[size];
    if verify {
        let expected = u32::from_le_bytes(block[size + 1..size + 5].try_into().unwrap());
        let mut digest = crc::digest();
        digest.update(&block[..size]);
        digest.update(&[compression]);
        if unmask_crc(expected) != digest.finalize() {
            return err(StatusCode::Corruption, "table block checksum mismatch");
        }
    }
    if compression == 1 {
        let length = snap::raw::decompress_len(&block[..size])?;
        if length > size.saturating_mul(64) {
            return err(StatusCode::Corruption, "invalid snappy expansion");
        }
    }
    opt.compressor_list.get(compression)?.decode(block[..size].to_vec())
}

fn block_entries(data: &[u8]) -> (Vec<Entry>, bool) {
    let mut entries = Vec::new();
    let result = (|| -> Result<()> {
        if data.len() < 4 {
            return err(StatusCode::Corruption, "short table block");
        }
        let count = u32::from_le_bytes(data[data.len() - 4..].try_into().unwrap()) as usize;
        if count > (data.len() - 4) / 4 {
            return err(StatusCode::Corruption, "invalid restart array");
        }
        if count == 0 { return Ok(()); }
        let end = data.len() - 4 - count * 4;
        let mut cursor = u32::from_le_bytes(data[end..end + 4].try_into().unwrap()) as usize;
        if cursor > end {
            return err(StatusCode::Corruption, "invalid restart offset");
        }
        let mut previous = Vec::new();
        while cursor < end {
            let shared = varint(&data[..end], &mut cursor, 5)? as u32 as usize;
            let unshared = varint(&data[..end], &mut cursor, 5)? as u32 as usize;
            let value_size = varint(&data[..end], &mut cursor, 5)? as u32 as usize;
            if shared > previous.len() || unshared > end - cursor
                || value_size > end - cursor - unshared {
                return err(StatusCode::Corruption, "invalid table entry");
            }
            previous.truncate(shared);
            previous.extend_from_slice(&data[cursor..cursor + unshared]);
            cursor += unshared;
            let value = data[cursor..cursor + value_size].to_vec();
            cursor += value_size;
            entries.push((previous.clone(), value));
        }
        Ok(())
    })();
    (entries, result.is_err())
}

fn scan_table(file: &dyn RandomAccess, size: usize, opt: &Options) -> Result<(Vec<Entry>, bool)> {
    if size < FULL_FOOTER_LENGTH {
        return err(StatusCode::Corruption, "short table footer");
    }
    let footer = read_at(file, size, size - FULL_FOOTER_LENGTH, FULL_FOOTER_LENGTH)?;
    if footer[40..] != MAGIC_FOOTER_ENCODED {
        return err(StatusCode::Corruption, "bad table magic");
    }
    let mut cursor = 0;
    let _meta = handle(&footer[..40], &mut cursor)?;
    let (offset, length) = handle(&footer[..40], &mut cursor)?;
    let index = read_block(file, size, opt, offset, length, false)?;
    let (blocks, mut damaged) = block_entries(&index);
    let mut entries = Vec::new();
    for (_, encoded) in blocks {
        let mut cursor = 0;
        let block = handle(&encoded, &mut cursor)
            .and_then(|(offset, length)| read_block(file, size, opt, offset, length, opt.paranoid_checks));
        match block {
            Ok(block) => {
                let (mut found, error) = block_entries(&block);
                entries.append(&mut found);
                damaged |= error;
            }
            Err(_) => damaged = true,
        }
    }
    Ok((entries, damaged))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blockhandle::BlockHandle;
    use crate::table_block;
    use crate::table_builder::{Footer, FULL_FOOTER_LENGTH};
    use crate::types::LdbIterator;
    use crate::{in_memory, ReadOptions, DB};
    use std::io::{Read, Write};
    use std::path::PathBuf;

    fn read_file(opt: &Options, path: &Path) -> Vec<u8> {
        let mut data = Vec::new();
        opt.env.open_sequential_file(path).unwrap().read_to_end(&mut data).unwrap();
        data
    }

    fn overwrite(opt: &Options, path: &Path, data: &[u8]) {
        opt.env.open_writable_file(path).unwrap().write_all(data).unwrap();
    }

    fn table_database() -> (Options, PathBuf, Vec<u8>) {
        let mut opt = in_memory();
        opt.block_size = 128;
        {
            let mut db = DB::open("db", opt.clone()).unwrap();
            for number in 0..40 {
                let key = format!("key{number:02}");
                db.put(key.as_bytes(), &[number as u8; 100]).unwrap();
            }
            db.compact_range(b"", b"\xff").unwrap();
        }
        let name = opt.env.children(Path::new("db")).unwrap().into_iter()
            .find(|p| p.extension().and_then(|s| s.to_str()) == Some("ldb")).unwrap();
        let path = Path::new("db").join(name);
        let bytes = read_file(&opt, &path);
        (opt, path, bytes)
    }

    fn corrupt_first_data_checksum(opt: &Options, path: &Path, bytes: &mut [u8]) {
        let footer = Footer::decode(&bytes[bytes.len() - FULL_FOOTER_LENGTH..]).unwrap();
        let file = opt.env.open_random_access_file(path).unwrap();
        let index = table_block::read_table_block(opt.clone(), file.as_ref(), &footer.index).unwrap();
        let mut iter = index.iter();
        assert!(iter.advance());
        let (_, encoded) = iter.current().unwrap();
        let (handle, _) = BlockHandle::decode(&encoded).unwrap();
        bytes[handle.offset() + handle.size() + 1] ^= 1;
    }

    fn corrupt_first_entry_with_valid_checksum(opt: &Options, path: &Path, bytes: &mut [u8]) {
        let footer = Footer::decode(&bytes[bytes.len() - FULL_FOOTER_LENGTH..]).unwrap();
        let file = opt.env.open_random_access_file(path).unwrap();
        let index = table_block::read_table_block(opt.clone(), file.as_ref(), &footer.index).unwrap();
        let mut iter = index.iter();
        assert!(iter.advance());
        let (_, encoded) = iter.current().unwrap();
        let (handle, _) = BlockHandle::decode(&encoded).unwrap();
        let start = handle.offset();
        let end = start + handle.size();
        bytes[start] = 0x7f;
        let mut digest = crate::crc::digest();
        digest.update(&bytes[start..=end]);
        bytes[end + 1..end + 5].copy_from_slice(&crate::log::mask_crc(digest.finalize()).to_le_bytes());
    }

    #[test]
    fn repairs_logs_and_archives_old_files() {
        let opt = in_memory();
        {
            let mut db = DB::open("db", opt.clone()).unwrap();
            db.put(b"keep", b"value").unwrap();
            db.put(b"deleted", b"value").unwrap();
            db.delete(b"deleted").unwrap();
        }
        opt.env.delete(Path::new("db/CURRENT")).unwrap();
        repair_db("db", opt.clone()).unwrap();
        let archived = opt.env.children(Path::new("db/lost")).unwrap();
        assert!(archived.iter().any(|p| p.extension().and_then(|s| s.to_str()) == Some("log")));
        assert!(archived.iter().any(|p| p.file_name().and_then(|s| s.to_str()).is_some_and(|s| s.starts_with("MANIFEST-"))));
        let mut db = DB::open("db", opt).unwrap();
        assert_eq!(db.get(b"keep").unwrap().as_deref(), Some(b"value".as_slice()));
        assert!(db.get(b"deleted").unwrap().is_none());
    }

    #[test]
    fn repairs_and_opens_a_legacy_sst_table() {
        let (opt, path, _) = table_database();
        let legacy = path.with_extension("sst");
        opt.env.rename(&path, &legacy).unwrap();
        opt.env.delete(Path::new("db/CURRENT")).unwrap();
        repair_db("db", opt.clone()).unwrap();
        let mut db = DB::open("db", opt).unwrap();
        assert_eq!(db.get(b"key00").unwrap().as_deref(), Some([0u8; 100].as_slice()));
        assert_eq!(db.get(b"key39").unwrap().as_deref(), Some([39u8; 100].as_slice()));
    }

    #[test]
    fn paranoid_repair_archives_damaged_table_and_salvages_other_blocks() {
        let (mut opt, path, mut bytes) = table_database();
        corrupt_first_data_checksum(&opt, &path, &mut bytes);
        overwrite(&opt, &path, &bytes);
        opt.env.delete(Path::new("db/CURRENT")).unwrap();
        opt.paranoid_checks = true;
        repair_db("db", opt.clone()).unwrap();
        let archived = Path::new("db/lost").join(path.file_name().unwrap());
        assert!(opt.env.exists(&archived).unwrap());
        let mut db = DB::open("db", opt).unwrap();
        assert!(db.get(b"key00").unwrap().is_none());
        assert_eq!(db.get(b"key39").unwrap().as_deref(), Some([39u8; 100].as_slice()));
    }

    #[test]
    fn corrupted_table_get_reports_corruption() {
        let (opt, path, mut bytes) = table_database();
        corrupt_first_data_checksum(&opt, &path, &mut bytes);
        overwrite(&opt, &path, &bytes);
        let mut db = DB::open("db", opt).unwrap();
        let snapshot = db.get_snapshot();
        assert_eq!(db.get_at(&snapshot, b"key00").unwrap_err().code, StatusCode::Corruption);
        assert_eq!(db.get(b"key00").unwrap_err().code, StatusCode::Corruption);
    }

    #[test]
    fn corrupted_table_iterator_reports_sticky_status() {
        let (opt, path, mut bytes) = table_database();
        corrupt_first_data_checksum(&opt, &path, &mut bytes);
        overwrite(&opt, &path, &bytes);
        let mut db = DB::open("db", opt).unwrap();
        let mut iter = db.new_iter().unwrap();
        while iter.advance() {}
        assert_eq!(iter.status().unwrap_err().code, StatusCode::Corruption);
        iter.reset();
        assert_eq!(iter.status().unwrap_err().code, StatusCode::Corruption);
    }

    #[test]
    fn seek_and_reverse_iteration_preserve_corruption_status() {
        let (opt, path, mut bytes) = table_database();
        corrupt_first_data_checksum(&opt, &path, &mut bytes);
        overwrite(&opt, &path, &bytes);
        let mut db = DB::open("db", opt).unwrap();
        let mut sought = db.new_iter().unwrap();
        sought.seek(b"key00");
        assert_eq!(sought.status().unwrap_err().code, StatusCode::Corruption);
        let mut reverse = db.new_iter().unwrap();
        reverse.seek(b"key39");
        assert!(reverse.valid());
        while reverse.prev() {}
        assert_eq!(reverse.status().unwrap_err().code, StatusCode::Corruption);
    }

    #[test]
    fn read_options_control_checksum_verification_and_cache_fill() {
        let (opt, path, mut bytes) = table_database();
        corrupt_first_data_checksum(&opt, &path, &mut bytes);
        overwrite(&opt, &path, &bytes);
        let mut db = DB::open("db", opt).unwrap();
        let unchecked = ReadOptions { verify_checksums: false, fill_cache: false };
        assert_eq!(db.get_with_options(b"key00", unchecked).unwrap().as_deref(), Some([0u8; 100].as_slice()));
        let checked = ReadOptions { verify_checksums: true, fill_cache: false };
        assert_eq!(db.get_with_options(b"key00", checked).unwrap_err().code, StatusCode::Corruption);
        let mut iter = db.new_iter_with_options(unchecked).unwrap();
        iter.seek(b"key00");
        assert_eq!(iter.current().unwrap().0.as_ref(), b"key00");
        assert!(iter.status().is_ok());
    }

    #[test]
    fn truncated_table_footer_returns_corruption() {
        let (opt, path, bytes) = table_database();
        let mut db = DB::open("db", opt.clone()).unwrap();
        overwrite(&opt, &path, &bytes[..8]);
        let snapshot = db.get_snapshot();
        assert_eq!(db.get_at(&snapshot, b"key00").unwrap_err().code, StatusCode::Corruption);
    }

    #[test]
    fn malformed_block_with_valid_checksum_reports_corruption() {
        let (opt, path, mut bytes) = table_database();
        corrupt_first_entry_with_valid_checksum(&opt, &path, &mut bytes);
        overwrite(&opt, &path, &bytes);
        let mut db = DB::open("db", opt).unwrap();
        let snapshot = db.get_snapshot();
        assert_eq!(db.get_at(&snapshot, b"key00").unwrap_err().code, StatusCode::Corruption);
        let mut iter = db.new_iter().unwrap();
        while iter.advance() {}
        assert_eq!(iter.status().unwrap_err().code, StatusCode::Corruption);
    }

    #[test]
    fn missing_live_table_is_not_reported_as_a_missing_key() {
        let (opt, path, _) = table_database();
        let mut db = DB::open("db", opt.clone()).unwrap();
        opt.env.delete(&path).unwrap();
        let snapshot = db.get_snapshot();
        assert!(db.get_at(&snapshot, b"key00").is_err());
    }

    #[test]
    fn repair_reports_an_empty_directory() {
        assert!(repair_db("missing", in_memory()).is_err());
    }
}

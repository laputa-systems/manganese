use crate::cmp::{Cmp, InternalKeyCmp};
use crate::error::{err, Result, Status, StatusCode};
use crate::key_types::{parse_internal_key, parse_internal_key_checked, InternalKey, LookupKey, UserKey, ValueType};
use crate::options::ReadOptions;
use crate::table_cache::TableCache;
use crate::table_reader::TableIterator;
use crate::types::{ErrorIter, FileMetaData, FileNum, LdbIterator, Shared, MAX_SEQUENCE_NUMBER, NUM_LEVELS};

use bytes::Bytes;
use std::cmp::Ordering;
use std::default::Default;
use std::rc::Rc;

/// FileMetaHandle is a reference-counted FileMetaData object with interior mutability. This is
/// necessary to provide a shared metadata container that can be modified while referenced by e.g.
/// multiple versions.
pub type FileMetaHandle = Shared<FileMetaData>;

/// Contains statistics about seeks occurred in a file.
pub struct GetStats {
    file: Option<FileMetaHandle>,
    level: usize,
}

pub struct Version {
    table_cache: Shared<TableCache>,
    user_cmp: Rc<Box<dyn Cmp>>,
    pub files: [Vec<FileMetaHandle>; NUM_LEVELS],

    pub file_to_compact: Option<FileMetaHandle>,
    pub file_to_compact_lvl: usize,
    pub compaction_score: Option<f64>,
    pub compaction_level: Option<usize>,
}


impl Version {
    pub fn new(cache: Shared<TableCache>, ucmp: Rc<Box<dyn Cmp>>) -> Version {
        Version {
            table_cache: cache,
            user_cmp: ucmp,
            files: Default::default(),
            file_to_compact: None,
            file_to_compact_lvl: 0,
            compaction_score: None,
            compaction_level: None,
        }
    }

    pub fn num_level_bytes(&self, l: usize) -> usize {
        assert!(l < NUM_LEVELS);
        total_size(self.files[l].iter())
    }

    pub fn num_level_files(&self, l: usize) -> usize {
        assert!(l < NUM_LEVELS);
        self.files[l].len()
    }

    /// get returns the value for the specified key using the persistent tables contained in this
    /// Version.
    #[allow(unused_assignments)]
    pub fn get(&self, key: InternalKey<'_>) -> Result<Option<(Bytes, GetStats)>> {
        self.get_with_options(key, ReadOptions::verified())
    }

    pub(crate) fn get_with_options(&self, key: InternalKey<'_>, options: ReadOptions) -> Result<Option<(Bytes, GetStats)>> {
        let levels = self.get_overlapping(key);
        let ikey = key;
        let ukey = parse_internal_key(ikey).2;

        let mut stats = GetStats {
            file: None,
            level: 0,
        };

        for (level, files) in levels.iter().enumerate() {
            let mut last_read = None;
            let mut last_read_level: usize = 0;
            for f in files {
                if last_read.is_some() && stats.file.is_none() {
                    stats.file.clone_from(&last_read);
                    stats.level = last_read_level;
                }
                last_read_level = level;
                last_read = Some(f.clone());

                // We receive both key and value from the table. Because we're using InternalKey
                // keys, we now need to check whether the found entry's user key is equal to the
                // one we're looking for (get() just returns the next-bigger key).
                if let Some((k, v)) = self.table_cache.borrow_mut().get_with_options(f.borrow().num, ikey, options)? {
                    // We don't need to check the sequence number; get() will not return an entry
                    // with a higher sequence number than the one in the supplied key.
                    let Some((typ, _, foundkey)) = parse_internal_key_checked(&k) else {
                        return err(StatusCode::Corruption, "corrupted key for lookup");
                    };
                    if typ == ValueType::TypeValue
                        && self.user_cmp.cmp(foundkey, ukey) == Ordering::Equal
                    {
                        return Ok(Some((v, stats)));
                    } else if typ == ValueType::TypeDeletion
                        && self.user_cmp.cmp(foundkey, ukey) == Ordering::Equal
                    {
                        // Skip looking once we have found a deletion.
                        return Ok(None);
                    }
                }
            }
        }
        Ok(None)
    }

    /// get_overlapping returns the files overlapping key in each level.
    fn get_overlapping(&self, key: InternalKey<'_>) -> [Vec<FileMetaHandle>; NUM_LEVELS] {
        let mut levels: [Vec<FileMetaHandle>; NUM_LEVELS] = Default::default();
        let ikey = key;
        let ukey = parse_internal_key(key).2;

        let files = &self.files[0];
        levels[0].reserve(files.len());
        for f_ in files {
            let f = f_.borrow();
            let (fsmallest, flargest) = (
                parse_internal_key(&f.smallest).2,
                parse_internal_key(&f.largest).2,
            );
            if self.user_cmp.cmp(ukey, fsmallest) >= Ordering::Equal
                && self.user_cmp.cmp(ukey, flargest) <= Ordering::Equal
            {
                levels[0].push(f_.clone());
            }
        }
        // Sort by newest first.
        levels[0].sort_by_key(|a| std::cmp::Reverse(a.borrow().num));

        let icmp = InternalKeyCmp(self.user_cmp.clone());
        (1..NUM_LEVELS).for_each(|level| {
            let files = &self.files[level];
            if let Some(ix) = find_file(&icmp, files, ikey) {
                let f = files[ix].borrow();
                let fsmallest = parse_internal_key(&f.smallest).2;
                if self.user_cmp.cmp(ukey, fsmallest) >= Ordering::Equal {
                    levels[level].push(files[ix].clone());
                }
            }
        });

        levels
    }

    /// level_summary returns a summary of the distribution of tables and bytes in this version.
    pub fn level_summary(&self) -> String {
        let mut acc = String::with_capacity(256);
        for level in 0..NUM_LEVELS {
            let fs = &self.files[level];
            if fs.is_empty() {
                continue;
            }
            let filedesc: Vec<(FileNum, usize)> = fs
                .iter()
                .map(|f| (f.borrow().num, f.borrow().size))
                .collect();
            let desc = format!(
                "level {}: {} files, {} bytes ({:?}); ",
                level,
                fs.len(),
                total_size(fs.iter()),
                filedesc
            );
            acc.push_str(&desc);
        }
        acc
    }

    pub fn pick_memtable_output_level(&self, min: UserKey<'_>, max: UserKey<'_>) -> usize {
        let mut level = 0;
        if !self.overlap_in_level(0, Some(min), Some(max)) {
            // Go to next level as long as there is no overlap in that level and a limited overlap
            // in the next-higher level.
            let start = LookupKey::new(min, MAX_SEQUENCE_NUMBER);
            let limit = LookupKey::new_full(max, 0, ValueType::TypeDeletion);

            const MAX_MEM_COMPACT_LEVEL: usize = 2;
            while level < MAX_MEM_COMPACT_LEVEL {
                if self.overlap_in_level(level + 1, Some(min), Some(max)) {
                    break;
                }
                if level + 2 < NUM_LEVELS {
                    let overlaps = self.overlapping_inputs(
                        level + 2,
                        Some(start.internal_key()),
                        Some(limit.internal_key()),
                    );
                    let size = total_size(overlaps.iter());
                    if size > 10 * (2 << 20) {
                        break;
                    }
                }
                level += 1;
            }
        }
        level
    }

    /// record_read_sample returns true if there is a new file to be compacted. It counts the
    /// number of files overlapping a key, and which level contains the first overlap.
    #[allow(unused_assignments)]
    pub fn record_read_sample(&mut self, key: InternalKey<'_>) -> bool {
        let mut contained_in = 0;
        let mut i = 0;
        let mut first_file = None;
        let mut first_file_level = None;
        self.get_overlapping(key).iter().for_each(|level| {
            if !level.is_empty() && first_file.is_none() && first_file_level.is_none() {
                first_file = Some(level[0].clone());
                first_file_level = Some(i);
            }
            contained_in += level.len();
            i += 1;
        });

        if contained_in > 1 {
            self.update_stats(GetStats {
                file: first_file,
                level: first_file_level.unwrap_or(0),
            })
        } else {
            false
        }
    }

    /// update_stats updates the number of seeks, and remembers files with too many seeks as
    /// compaction candidates. It returns true if a compaction makes sense.
    pub fn update_stats(&mut self, stats: GetStats) -> bool {
        if let Some(file) = stats.file {
            if file.borrow().allowed_seeks <= 1 && self.file_to_compact.is_none() {
                self.file_to_compact = Some(file);
                self.file_to_compact_lvl = stats.level;
                return true;
            } else if file.borrow().allowed_seeks > 0 {
                file.borrow_mut().allowed_seeks -= 1;
            }
        }
        false
    }

    /// max_next_level_overlapping_bytes returns how many bytes of tables are overlapped in l+1 by
    /// tables in l, for the maximum case.
    fn max_next_level_overlapping_bytes(&self) -> usize {
        let mut max = 0;
        for lvl in 1..NUM_LEVELS - 1 {
            for f in &self.files[lvl] {
                let f = f.borrow();
                let ols = self.overlapping_inputs(lvl + 1, Some(&f.smallest), Some(&f.largest));
                let sum = total_size(ols.iter());
                if sum > max {
                    max = sum;
                }
            }
        }
        max
    }

    /// overlap_in_level returns true if the specified level's files overlap the range [smallest;
    /// largest]. An absent bound is unbounded.
    pub fn overlap_in_level(
        &self,
        level: usize,
        smallest: Option<UserKey<'_>>,
        largest: Option<UserKey<'_>>,
    ) -> bool {
        assert!(level < NUM_LEVELS);
        some_file_overlaps_range(
            &InternalKeyCmp(self.user_cmp.clone()),
            level > 0,
            &self.files[level],
            smallest,
            largest,
        )
    }

    /// overlapping_inputs returns all files that may contain keys between begin and end. An
    /// absent bound is unbounded.
    pub fn overlapping_inputs(
        &self,
        level: usize,
        begin: Option<InternalKey<'_>>,
        end: Option<InternalKey<'_>>,
    ) -> Vec<FileMetaHandle> {
        assert!(level < NUM_LEVELS);
        let mut user_begin = begin.map(|key| parse_internal_key(key).2.to_vec());
        let mut user_end = end.map(|key| parse_internal_key(key).2.to_vec());
        let mut inputs = vec![];
        let mut i = 0;
        while i < self.files[level].len() {
            let handle = self.files[level][i].clone();
            i += 1;
            let f = handle.borrow();
            let file_start = parse_internal_key(&f.smallest).2;
            let file_limit = parse_internal_key(&f.largest).2;
            if user_begin.as_ref().is_some_and(|b| self.user_cmp.cmp(file_limit, b) == Ordering::Less) {
                continue;
            }
            if user_end.as_ref().is_some_and(|e| self.user_cmp.cmp(file_start, e) == Ordering::Greater) {
                continue;
            }
            inputs.push(handle.clone());
            if level == 0 {
                if user_begin.as_ref().is_some_and(|b| self.user_cmp.cmp(file_start, b) == Ordering::Less) {
                    user_begin = Some(file_start.to_vec());
                    inputs.clear();
                    i = 0;
                } else if user_end.as_ref().is_some_and(|e| self.user_cmp.cmp(file_limit, e) == Ordering::Greater) {
                    user_end = Some(file_limit.to_vec());
                    inputs.clear();
                    i = 0;
                }
            }
        }
        inputs
    }

    /// new_concat_iter returns an iterator that iterates over the files in a level. Note that this
    /// only really makes sense for levels > 0.
    fn new_concat_iter(&self, level: usize, options: ReadOptions) -> VersionIter {
        new_version_iter_with_options(
            self.files[level].clone(),
            self.table_cache.clone(),
            self.user_cmp.clone(),
            options,
        )
    }

    /// new_iters returns a set of iterators that can be merged to yield all entries in this
    /// version.
    pub fn new_iters(&self) -> Result<Vec<Box<dyn LdbIterator>>> {
        self.new_iters_with_options(ReadOptions::verified())
    }

    pub(crate) fn new_iters_with_options(&self, options: ReadOptions) -> Result<Vec<Box<dyn LdbIterator>>> {
        let mut iters: Vec<Box<dyn LdbIterator>> = vec![];
        for f in &self.files[0] {
            let table = self.table_cache.borrow_mut().get_table(f.borrow().num);
            match table {
                Ok(table) => iters.push(Box::new(table.iter_with_options(options))),
                Err(error) => iters.push(Box::new(ErrorIter(error))),
            }
        }

        for l in 1..NUM_LEVELS {
            if !self.files[l].is_empty() {
                iters.push(Box::new(self.new_concat_iter(l, options)));
            }
        }

        Ok(iters)
    }
}

/// new_version_iter returns an iterator over the entries in the specified ordered list of table
/// files.
pub fn new_version_iter(
    files: Vec<FileMetaHandle>,
    cache: Shared<TableCache>,
    ucmp: Rc<Box<dyn Cmp>>,
) -> VersionIter {
    new_version_iter_with_options(files, cache, ucmp, ReadOptions::verified())
}

pub(crate) fn new_version_iter_with_options(
    files: Vec<FileMetaHandle>,
    cache: Shared<TableCache>,
    ucmp: Rc<Box<dyn Cmp>>,
    options: ReadOptions,
) -> VersionIter {
    VersionIter {
        files,
        cache,
        cmp: InternalKeyCmp(ucmp),
        current: None,
        current_ix: 0,
        error: None,
        options,
    }
}

/// VersionIter iterates over the entries in an ordered list of table files (specifically, for
/// example, the tables in a level).
///
/// Note that VersionIter returns entries of type Deletion.
pub struct VersionIter {
    // NOTE: Maybe we need to change this to Rc to support modification of the file set after
    // creation of the iterator. Versions should be immutable, though.
    files: Vec<FileMetaHandle>,
    cache: Shared<TableCache>,
    cmp: InternalKeyCmp,

    current: Option<TableIterator>,
    current_ix: usize,
    error: Option<Status>,
    options: ReadOptions,
}

impl VersionIter {
    fn remember(&mut self, error: Status) {
        if self.error.is_none() {
            self.error = Some(error);
        }
    }

    fn remember_current(&mut self) {
        if let Some(error) = self.current.as_ref().and_then(|iter| iter.status().err()) {
            self.remember(error);
        }
    }
}

impl LdbIterator for VersionIter {
    fn seek_to_last(&mut self) {
        self.remember_current();
        self.current = None;
        self.current_ix = self.files.len();
        while self.current_ix > 0 {
            self.current_ix -= 1;
            let result = self.cache.borrow_mut().get_table(self.files[self.current_ix].borrow().num);
            match result {
                Ok(table) => {
                    let mut iter = table.iter_with_options(self.options);
                    iter.seek_to_last();
                    let valid = iter.valid();
                    self.current = Some(iter);
                    if valid { return; }
                    self.remember_current();
                    self.current = None;
                }
                Err(error) => self.remember(error),
            }
        }
        self.reset();
    }
    fn advance(&mut self) -> bool {
        assert!(!self.files.is_empty());
        loop {
            if let Some(ref mut t) = self.current {
                if t.advance() {
                    return true;
                }
                self.remember_current();
                if self.current_ix + 1 >= self.files.len() {
                    return false;
                }
                self.current_ix += 1;
                self.current = None;
            }
            let result = self.cache.borrow_mut().get_table(self.files[self.current_ix].borrow().num);
            match result {
                Ok(table) => self.current = Some(table.iter_with_options(self.options)),
                Err(error) => {
                    self.remember(error);
                    if self.current_ix + 1 >= self.files.len() {
                        return false;
                    }
                    self.current_ix += 1;
                }
            }
        }
    }
    fn current(&self) -> Option<(Bytes, Bytes)> {
        if let Some(ref t) = self.current {
            t.current()
        } else {
            None
        }
    }
    fn seek(&mut self, key: &[u8]) {
        if let Some(ix) = find_file(&self.cmp, &self.files, key) {
            self.remember_current();
            self.current = None;
            self.current_ix = ix;
            let result = self.cache.borrow_mut().get_table(self.files[ix].borrow().num);
            match result {
                Ok(table) => {
                    let mut iter = table.iter_with_options(self.options);
                    iter.seek(key);
                    let valid = iter.valid();
                    self.current = Some(iter);
                    if valid { return; }
                }
                Err(error) => self.remember(error),
            }
            self.remember_current();
            self.current = None;
            if ix + 1 < self.files.len() {
                self.current_ix = ix + 1;
                if self.advance() {
                    return;
                }
            }
        }
        self.reset();
    }
    fn reset(&mut self) {
        self.remember_current();
        self.current = None;
        self.current_ix = 0;
    }
    fn valid(&self) -> bool {
        self.current.as_ref().map(|t| t.valid()).unwrap_or(false)
    }
    fn prev(&mut self) -> bool {
        if let Some(ref mut t) = self.current {
            if t.prev() {
                return true;
            }
        }
        self.remember_current();
        self.current = None;
        while self.current_ix > 0 {
            self.current_ix -= 1;
            let result = self.cache.borrow_mut().get_table(self.files[self.current_ix].borrow().num);
            match result {
                Ok(table) => {
                    let mut iter = table.iter_with_options(self.options);
                    iter.seek_to_last();
                    let valid = iter.valid();
                    self.current = Some(iter);
                    if valid { return true; }
                    self.remember_current();
                    self.current = None;
                }
                Err(error) => self.remember(error),
            }
        }
        self.reset();
        false
    }
    fn status(&self) -> Result<()> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        self.current.as_ref().map_or(Ok(()), |iter| iter.status())
    }
}

/// total_size returns the sum of sizes of the given files.
pub fn total_size<'a, I: Iterator<Item = &'a FileMetaHandle>>(files: I) -> usize {
    files.fold(0, |a, f| a + f.borrow().size)
}

fn after_file(cmp: &InternalKeyCmp, key: Option<UserKey<'_>>, f: &FileMetaHandle) -> bool {
    let f = f.borrow();
    key.is_some_and(|key| cmp.cmp_inner(key, parse_internal_key(&f.largest).2) == Ordering::Greater)
}

fn before_file(cmp: &InternalKeyCmp, key: Option<UserKey<'_>>, f: &FileMetaHandle) -> bool {
    let f = f.borrow();
    key.is_some_and(|key| cmp.cmp_inner(key, parse_internal_key(&f.smallest).2) == Ordering::Less)
}

/// find_file returns the index of the file in files that potentially contains the internal key
/// key. files must not overlap and be ordered ascendingly. If no file can contain the key, None is
/// returned.
fn find_file(
    cmp: &InternalKeyCmp,
    files: &[FileMetaHandle],
    key: InternalKey<'_>,
) -> Option<usize> {
    let (mut left, mut right) = (0, files.len());
    while left < right {
        let mid = (left + right) / 2;
        if cmp.cmp(&files[mid].borrow().largest, key) == Ordering::Less {
            left = mid + 1;
        } else {
            right = mid;
        }
    }
    if right < files.len() {
        Some(right)
    } else {
        None
    }
}

/// some_file_overlaps_range returns true if any of the given files contains keys in the range
/// [smallest; largest]; disjoint files are sorted and searched by binary search.
fn some_file_overlaps_range(
    cmp: &InternalKeyCmp,
    disjoint: bool,
    files: &[FileMetaHandle],
    smallest: Option<UserKey<'_>>,
    largest: Option<UserKey<'_>>,
) -> bool {
    if !disjoint {
        return files.iter().any(|f| !(after_file(cmp, smallest, f) || before_file(cmp, largest, f)));
    }
    let index = match smallest {
        Some(smallest) => find_file(cmp, files, LookupKey::new(smallest, MAX_SEQUENCE_NUMBER).internal_key()),
        None => (!files.is_empty()).then_some(0),
    };
    index.is_some_and(|index| !before_file(cmp, largest, &files[index]))
}

#[cfg(test)]
pub mod testutil {
    use super::*;
    use crate::cache::Cache;
    use crate::cmp::DefaultCmp;
    use crate::env::Env;
    use crate::key_types::ValueType;
    use crate::options::{self, Options};
    use crate::table_builder::TableBuilder;
    use crate::table_cache::table_file_name;
    use crate::types::{share, FileMetaData, FileNum};

    use std::path::Path;

    pub fn new_file(
        num: u64,
        smallest: &[u8],
        smallestix: u64,
        largest: &[u8],
        largestix: u64,
    ) -> FileMetaHandle {
        share(FileMetaData {
            allowed_seeks: 10,
            size: 163840,
            num,
            smallest: LookupKey::new(smallest, smallestix)
                .internal_key()
                .to_vec()
                .into(),
            largest: LookupKey::new(largest, largestix)
                .internal_key()
                .to_vec()
                .into(),
        })
    }

    /// write_table creates a table with the given number and contents (must be sorted!) in the
    /// memenv. The sequence numbers given to keys start with startseq.
    pub fn write_table(
        me: &dyn Env,
        contents: &[(&[u8], &[u8], ValueType)],
        startseq: u64,
        num: FileNum,
    ) -> FileMetaHandle {
        let dst = me
            .open_writable_file(Path::new(&table_file_name("db", num)))
            .unwrap();
        let mut seq = startseq;
        let keys: Vec<Vec<u8>> = contents
            .iter()
            .map(|&(k, _, typ)| {
                seq += 1;
                LookupKey::new_full(k, seq - 1, typ).internal_key().to_vec()
            })
            .collect();

        let mut tbl = TableBuilder::new(options::for_test(), dst);
        for i in 0..contents.len() {
            tbl.add(&keys[i], contents[i].1).unwrap();
        }

        let f = new_file(
            num,
            contents[0].0,
            startseq,
            contents[contents.len() - 1].0,
            startseq + (contents.len() - 1) as u64,
        );
        f.borrow_mut().size = tbl.finish().unwrap();
        f
    }

    pub fn make_version() -> (Version, Options) {
        let opts = options::for_test();
        let env = opts.env.clone();

        // The different levels overlap in a sophisticated manner to be able to test compactions
        // and so on.
        // The sequence numbers are in "natural order", i.e. highest levels have lowest sequence
        // numbers.

        // Level 0 (overlapping)
        let f2: &[(&[u8], &[u8], ValueType)] = &[
            (b"aac", b"val1", ValueType::TypeDeletion),
            (b"aax", b"val2", ValueType::TypeValue),
            (b"aba", b"val3", ValueType::TypeValue),
            (b"bab", b"val4", ValueType::TypeValue),
            (b"bba", b"val5", ValueType::TypeValue),
        ];
        let t2 = write_table(env.as_ref().as_ref(), f2, 26, 2);
        let f1: &[(&[u8], &[u8], ValueType)] = &[
            (b"aaa", b"val1", ValueType::TypeValue),
            (b"aab", b"val2", ValueType::TypeValue),
            (b"aac", b"val3", ValueType::TypeValue),
            (b"aba", b"val4", ValueType::TypeValue),
        ];
        let t1 = write_table(env.as_ref().as_ref(), f1, 22, 1);
        // Level 1
        let f3: &[(&[u8], &[u8], ValueType)] = &[
            (b"aaa", b"val0", ValueType::TypeValue),
            (b"cab", b"val2", ValueType::TypeValue),
            (b"cba", b"val3", ValueType::TypeValue),
        ];
        let t3 = write_table(env.as_ref().as_ref(), f3, 19, 3);
        let f4: &[(&[u8], &[u8], ValueType)] = &[
            (b"daa", b"val1", ValueType::TypeValue),
            (b"dab", b"val2", ValueType::TypeValue),
            (b"dba", b"val3", ValueType::TypeValue),
        ];
        let t4 = write_table(env.as_ref().as_ref(), f4, 16, 4);
        let f5: &[(&[u8], &[u8], ValueType)] = &[
            (b"eaa", b"val1", ValueType::TypeValue),
            (b"eab", b"val2", ValueType::TypeValue),
            (b"fab", b"val3", ValueType::TypeValue),
        ];
        let t5 = write_table(env.as_ref().as_ref(), f5, 13, 5);
        // Level 2
        let f6: &[(&[u8], &[u8], ValueType)] = &[
            (b"cab", b"val1", ValueType::TypeValue),
            (b"fab", b"val2", ValueType::TypeValue),
            (b"fba", b"val3", ValueType::TypeValue),
        ];
        let t6 = write_table(env.as_ref().as_ref(), f6, 10, 6);
        let f7: &[(&[u8], &[u8], ValueType)] = &[
            (b"gaa", b"val1", ValueType::TypeValue),
            (b"gab", b"val2", ValueType::TypeValue),
            (b"gba", b"val3", ValueType::TypeValue),
            (b"gca", b"val4", ValueType::TypeDeletion),
            (b"gda", b"val5", ValueType::TypeValue),
        ];
        let t7 = write_table(env.as_ref().as_ref(), f7, 5, 7);
        // Level 3 (2 * 2 entries, for iterator behavior).
        let f8: &[(&[u8], &[u8], ValueType)] = &[
            (b"haa", b"val1", ValueType::TypeValue),
            (b"hba", b"val2", ValueType::TypeValue),
        ];
        let t8 = write_table(env.as_ref().as_ref(), f8, 3, 8);
        let f9: &[(&[u8], &[u8], ValueType)] = &[
            (b"iaa", b"val1", ValueType::TypeValue),
            (b"iba", b"val2", ValueType::TypeValue),
        ];
        let t9 = write_table(env.as_ref().as_ref(), f9, 1, 9);

        let cache = TableCache::new("db", opts.clone(), share(Cache::new(128)), 100);
        let mut v = Version::new(share(cache), Rc::new(Box::new(DefaultCmp)));
        v.files[0] = vec![t1, t2];
        v.files[1] = vec![t3, t4, t5];
        v.files[2] = vec![t6, t7];
        v.files[3] = vec![t8, t9];
        (v, opts)
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::*;
    use super::*;

    use crate::cmp::DefaultCmp;
    use crate::error::Result;
    use crate::merging_iter::MergingIter;
    use crate::options;
    use crate::test_util::{test_iterator_properties, LdbIteratorIter};

    #[test]
    fn test_version_concat_iter() {
        let v = make_version().0;

        let expected_entries = [0, 9, 8, 4];
        (1..4).for_each(|l| {
            let mut iter = v.new_concat_iter(l, ReadOptions::verified());
            let iter = LdbIteratorIter::wrap(&mut iter);
            assert_eq!(iter.count(), expected_entries[l]);
        });
    }

    #[test]
    fn test_version_concat_iter_properties() {
        let v = make_version().0;
        let iter = v.new_concat_iter(3, ReadOptions::verified());
        test_iterator_properties(iter);
    }

    #[test]
    fn test_version_max_next_level_overlapping() {
        let v = make_version().0;
        assert_eq!(216, v.max_next_level_overlapping_bytes());
    }

    #[test]
    fn test_version_all_iters() {
        let v = make_version().0;
        let iters = v.new_iters().unwrap();
        let mut opt = options::for_test();
        opt.cmp = Rc::new(Box::new(InternalKeyCmp(Rc::new(Box::new(DefaultCmp)))));

        let mut miter = MergingIter::new(opt.cmp.clone(), iters);
        assert_eq!(LdbIteratorIter::wrap(&mut miter).count(), 30);

        // Check that all elements are in order.
        let init = LookupKey::new(b"000", MAX_SEQUENCE_NUMBER);
        let cmp = InternalKeyCmp(Rc::new(Box::new(DefaultCmp)));
        LdbIteratorIter::wrap(&mut miter).fold(init.internal_key().to_vec(), |b, (k, _)| {
            assert!(cmp.cmp(&b, &k) == Ordering::Less);
            k
        });
    }

    #[test]
    fn test_version_summary() {
        let v = make_version().0;
        let expected = "level 0: 2 files, 479 bytes ([(1, 230), (2, 249)]); level 1: 3 files, 645 \
                        bytes ([(3, 216), (4, 214), (5, 215)]); level 2: 2 files, 464 bytes ([(6, \
                        216), (7, 248)]); level 3: 2 files, 396 bytes ([(8, 198), (9, 198)]); ";
        assert_eq!(expected, &v.level_summary());
    }

    #[test]
    fn test_version_get_simple() {
        let v = make_version().0;
        type GetCase<'a> = (&'a [u8], u64, Result<Option<Vec<u8>>>);
        let cases: &[GetCase] = &[
            (b"aaa", 1, Ok(None)),
            (b"aaa", 100, Ok(Some("val1".as_bytes().to_vec()))),
            (b"aaa", 21, Ok(Some("val0".as_bytes().to_vec()))),
            (b"aab", 0, Ok(None)),
            (b"aab", 100, Ok(Some("val2".as_bytes().to_vec()))),
            (b"aac", 100, Ok(None)),
            (b"aac", 25, Ok(Some("val3".as_bytes().to_vec()))),
            (b"aba", 100, Ok(Some("val3".as_bytes().to_vec()))),
            (b"aba", 25, Ok(Some("val4".as_bytes().to_vec()))),
            (b"daa", 100, Ok(Some("val1".as_bytes().to_vec()))),
            (b"dab", 1, Ok(None)),
            (b"dac", 100, Ok(None)),
            (b"gba", 100, Ok(Some("val3".as_bytes().to_vec()))),
            // deleted key
            (b"gca", 100, Ok(None)),
            (b"gbb", 100, Ok(None)),
        ];

        cases
            .iter()
            .for_each(|c| match v.get(LookupKey::new(c.0, c.1).internal_key()) {
                Ok(Some((val, _))) => assert_eq!(c.2.as_ref().unwrap().as_ref().unwrap(), &val),
                Ok(None) => assert!(c.2.as_ref().unwrap().as_ref().is_none()),
                Err(_) => assert!(c.2.is_err()),
            });
    }

    #[test]
    fn unrelated_tombstone_does_not_hide_an_older_table_value() {
        use crate::types::share;

        let (mut version, mut opt) = make_version();
        let newer = write_table(opt.env.as_ref().as_ref(), &[
            (b"aaa", b"new", ValueType::TypeValue),
            (b"aac", b"", ValueType::TypeDeletion),
            (b"aaz", b"new", ValueType::TypeValue),
        ], 31, 10);
        version.files[0].push(newer);
        opt.filter_policy = None;
        version.table_cache = share(crate::table_cache::TableCache::new(
            "db", opt, share(crate::cache::Cache::new(128)), 100));
        let key = LookupKey::new(b"aab", 100);
        let value = version.get(key.internal_key()).unwrap().unwrap().0;
        assert_eq!(value.as_ref(), b"val2");
    }

    #[test]
    fn test_version_get_overlapping_basic() {
        let v = make_version().0;

        // Overlapped by tables 1 and 2.
        let ol = v.get_overlapping(LookupKey::new(b"aay", 50).internal_key());
        // Check that sorting order is newest-first in L0.
        assert_eq!(2, ol[0][0].borrow().num);
        // Check that table from L1 matches.
        assert_eq!(3, ol[1][0].borrow().num);

        let ol = v.get_overlapping(LookupKey::new(b"cb", 50).internal_key());
        assert_eq!(3, ol[1][0].borrow().num);
        assert_eq!(6, ol[2][0].borrow().num);

        let ol = v.get_overlapping(LookupKey::new(b"x", 50).internal_key());
        (0..NUM_LEVELS).for_each(|i| {
            assert!(ol[i].is_empty());
        });
    }

    #[test]
    fn test_version_overlap_in_level() {
        let v = make_version().0;

        for &(level, (k1, k2), want) in &[
            (0, ("000".as_bytes(), "003".as_bytes()), false),
            (0, ("aa0".as_bytes(), "abx".as_bytes()), true),
            (1, ("012".as_bytes(), "013".as_bytes()), false),
            (1, ("abc".as_bytes(), "def".as_bytes()), true),
            (2, ("xxx".as_bytes(), "xyz".as_bytes()), false),
            (2, ("gac".as_bytes(), "gaz".as_bytes()), true),
        ] {
            if want {
                assert!(v.overlap_in_level(level, Some(k1), Some(k2)));
            } else {
                assert!(!v.overlap_in_level(level, Some(k1), Some(k2)));
            }
        }
    }

    #[test]
    fn test_version_pick_memtable_output_level() {
        let v = make_version().0;

        for c in [
            ("000".as_bytes(), "abc".as_bytes(), 0),
            ("gab".as_bytes(), "hhh".as_bytes(), 1),
            ("000".as_bytes(), "111".as_bytes(), 2),
        ]
        .iter()
        {
            assert_eq!(c.2, v.pick_memtable_output_level(c.0, c.1));
        }
    }

    #[test]
    fn test_version_overlapping_inputs() {
        let v = make_version().0;

        time_test!("overlapping-inputs");
        {
            // Range is expanded in overlapping level-0 files.
            let from = LookupKey::new("aab".as_bytes(), MAX_SEQUENCE_NUMBER);
            let to = LookupKey::new("aae".as_bytes(), 0);
            let r = v.overlapping_inputs(0, Some(from.internal_key()), Some(to.internal_key()));
            assert_eq!(r.len(), 2);
            assert_eq!(r[0].borrow().num, 1);
            assert_eq!(r[1].borrow().num, 2);
        }
        {
            let from = LookupKey::new("cab".as_bytes(), MAX_SEQUENCE_NUMBER);
            let to = LookupKey::new("cbx".as_bytes(), 0);
            // expect one file.
            let r = v.overlapping_inputs(1, Some(from.internal_key()), Some(to.internal_key()));
            assert_eq!(r.len(), 1);
            assert_eq!(r[0].borrow().num, 3);
        }
        {
            let from = LookupKey::new("cab".as_bytes(), MAX_SEQUENCE_NUMBER);
            let to = LookupKey::new("ebx".as_bytes(), 0);
            let r = v.overlapping_inputs(1, Some(from.internal_key()), Some(to.internal_key()));
            // Assert that correct number of files and correct files were returned.
            assert_eq!(r.len(), 3);
            assert_eq!(r[0].borrow().num, 3);
            assert_eq!(r[1].borrow().num, 4);
            assert_eq!(r[2].borrow().num, 5);
        }
        {
            let from = LookupKey::new("hhh".as_bytes(), MAX_SEQUENCE_NUMBER);
            let to = LookupKey::new("ijk".as_bytes(), 0);
            let r = v.overlapping_inputs(2, Some(from.internal_key()), Some(to.internal_key()));
            assert_eq!(r.len(), 0);
            let r = v.overlapping_inputs(1, Some(from.internal_key()), Some(to.internal_key()));
            assert_eq!(r.len(), 0);
        }
    }

    #[test]
    fn test_version_record_read_sample() {
        let mut v = make_version().0;
        let k = LookupKey::new("aab".as_bytes(), MAX_SEQUENCE_NUMBER);
        let only_in_one = LookupKey::new("cax".as_bytes(), MAX_SEQUENCE_NUMBER);

        assert!(!v.record_read_sample(k.internal_key()));
        assert!(!v.record_read_sample(only_in_one.internal_key()));

        for fs in v.files.iter() {
            for f in fs {
                f.borrow_mut().allowed_seeks = 0;
            }
        }
        assert!(v.record_read_sample(k.internal_key()));
    }

    #[test]
    fn test_version_key_ordering() {
        time_test!();
        let fmh = new_file(1, &[1, 0, 0], 0, &[2, 0, 0], 1);
        let cmp = InternalKeyCmp(Rc::new(Box::new(DefaultCmp)));

        // Keys before file.
        for k in &[&[0][..], &[1], &[1, 0], &[0, 9, 9, 9]] {
            assert!(before_file(&cmp, Some(k), &fmh));
            assert!(!after_file(&cmp, Some(k), &fmh));
        }
        // Keys in file.
        for k in &[
            &[1, 0, 0][..],
            &[1, 0, 1],
            &[1, 2, 3, 4],
            &[1, 9, 9],
            &[2, 0, 0],
        ] {
            assert!(!before_file(&cmp, Some(k), &fmh));
            assert!(!after_file(&cmp, Some(k), &fmh));
        }
        // Keys after file.
        for k in &[&[2, 0, 1][..], &[9, 9, 9], &[9, 9, 9, 9]] {
            assert!(!before_file(&cmp, Some(k), &fmh));
            assert!(after_file(&cmp, Some(k), &fmh));
        }
    }

    #[test]
    fn test_version_file_overlaps() {
        time_test!();

        let files_disjoint = [
            new_file(1, &[2, 0, 0], 0, &[3, 0, 0], 1),
            new_file(2, &[3, 0, 1], 0, &[4, 0, 0], 1),
            new_file(3, &[4, 0, 1], 0, &[5, 0, 0], 1),
        ];
        let files_joint = [
            new_file(1, &[2, 0, 0], 0, &[3, 0, 0], 1),
            new_file(2, &[2, 5, 0], 0, &[4, 0, 0], 1),
            new_file(3, &[3, 5, 1], 0, &[5, 0, 0], 1),
        ];
        let cmp = InternalKeyCmp(Rc::new(Box::new(DefaultCmp)));

        assert!(some_file_overlaps_range(
            &cmp,
            false,
            &files_joint,
            Some(&[2, 5, 0]),
            Some(&[3, 1, 0])
        ));
        assert!(some_file_overlaps_range(
            &cmp,
            false,
            &files_joint,
            Some(&[2, 5, 0]),
            Some(&[7, 0, 0])
        ));
        assert!(some_file_overlaps_range(
            &cmp,
            false,
            &files_joint,
            Some(&[0, 0]),
            Some(&[2, 0, 0])
        ));
        assert!(some_file_overlaps_range(
            &cmp,
            false,
            &files_joint,
            Some(&[0, 0]),
            Some(&[7, 0, 0])
        ));
        assert!(!some_file_overlaps_range(
            &cmp,
            false,
            &files_joint,
            Some(&[0, 0]),
            Some(&[0, 5])
        ));
        assert!(!some_file_overlaps_range(
            &cmp,
            false,
            &files_joint,
            Some(&[6, 0]),
            Some(&[7, 5])
        ));

        assert!(some_file_overlaps_range(
            &cmp,
            true,
            &files_disjoint,
            Some(&[2, 0, 1]),
            Some(&[2, 5, 0])
        ));
        assert!(some_file_overlaps_range(
            &cmp,
            true,
            &files_disjoint,
            Some(&[3, 0, 1]),
            Some(&[4, 9, 0])
        ));
        assert!(some_file_overlaps_range(
            &cmp,
            true,
            &files_disjoint,
            Some(&[2, 0, 1]),
            Some(&[6, 5, 0])
        ));
        assert!(some_file_overlaps_range(
            &cmp,
            true,
            &files_disjoint,
            Some(&[0, 0, 1]),
            Some(&[2, 5, 0])
        ));
        assert!(some_file_overlaps_range(
            &cmp,
            true,
            &files_disjoint,
            Some(&[0, 0, 1]),
            Some(&[6, 5, 0])
        ));
        assert!(!some_file_overlaps_range(
            &cmp,
            true,
            &files_disjoint,
            Some(&[0, 0, 1]),
            Some(&[0, 1])
        ));
        assert!(!some_file_overlaps_range(
            &cmp,
            true,
            &files_disjoint,
            Some(&[6, 0, 1]),
            Some(&[7, 0, 1])
        ));
    }
}

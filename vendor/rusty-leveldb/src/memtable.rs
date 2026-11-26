use crate::cmp::{Cmp, MemtableKeyCmp};
use crate::key_types::{build_memtable_key, parse_internal_key, parse_memtable_key, ValueType};
use crate::key_types::{LookupKey, UserKey};
use crate::skipmap::{SkipMap, SkipMapIter};
use crate::types::{current_key_val, LdbIterator, SequenceNumber};

use std::rc::Rc;

use bytes::Bytes;
use integer_encoding::FixedInt;

const ARENA_BLOCK_SIZE: usize = 4096;
const ARENA_ALIGN: usize = 8;
const SKIPLIST_MAX_HEIGHT: usize = 12;
const SKIPLIST_BRANCHING: u32 = 4;
const SKIPLIST_NODE_SIZE: usize = 16;
const SKIPLIST_LINK_SIZE: usize = 8;

/// Tracks the memory LevelDB's arena reports for a memtable with the same insertions, so that
/// memtables fill up and are flushed at the same writes as in LevelDB.
struct ArenaUsage {
    usage: usize,
    offset: usize,
    remaining: usize,
    seed: u32,
}

impl ArenaUsage {
    fn new() -> ArenaUsage {
        let mut arena = ArenaUsage {
            usage: 0,
            offset: 0,
            remaining: 0,
            seed: 0xdeadbeef & 0x7fff_ffff,
        };
        arena.allocate_aligned(SKIPLIST_NODE_SIZE + SKIPLIST_LINK_SIZE * (SKIPLIST_MAX_HEIGHT - 1));
        arena
    }

    fn allocate(&mut self, bytes: usize) {
        if bytes <= self.remaining {
            self.offset += bytes;
            self.remaining -= bytes;
        } else {
            self.allocate_fallback(bytes);
        }
    }

    fn allocate_aligned(&mut self, bytes: usize) {
        let slop = (ARENA_ALIGN - self.offset % ARENA_ALIGN) % ARENA_ALIGN;
        let needed = bytes + slop;
        if needed <= self.remaining {
            self.offset += needed;
            self.remaining -= needed;
        } else {
            self.allocate_fallback(bytes);
        }
    }

    fn allocate_fallback(&mut self, bytes: usize) {
        if bytes > ARENA_BLOCK_SIZE / 4 {
            self.usage += bytes + size_of::<usize>();
            return;
        }
        self.usage += ARENA_BLOCK_SIZE + size_of::<usize>();
        self.offset = bytes;
        self.remaining = ARENA_BLOCK_SIZE - bytes;
    }

    fn next_random(&mut self) -> u32 {
        const M: u64 = 2147483647;
        const A: u64 = 16807;
        let product = u64::from(self.seed) * A;
        let mut seed = (product >> 31) + (product & M);
        if seed > M {
            seed -= M;
        }
        self.seed = seed as u32;
        self.seed
    }

    fn add_entry(&mut self, key_len: usize, value_len: usize) {
        let internal_key_len = key_len + 8;
        self.allocate(integer_encoding::VarInt::required_space(internal_key_len) + internal_key_len + integer_encoding::VarInt::required_space(value_len) + value_len);
        let mut height = 1;
        while height < SKIPLIST_MAX_HEIGHT && self.next_random() % SKIPLIST_BRANCHING == 0 {
            height += 1;
        }
        self.allocate_aligned(SKIPLIST_NODE_SIZE + SKIPLIST_LINK_SIZE * (height - 1));
    }
}

/// Provides Insert/Get/Iterate, based on the SkipMap implementation.
/// MemTable uses MemtableKeys internally, that is, it stores key and value in the [Skipmap] key.
pub struct MemTable {
    map: SkipMap,
    arena: ArenaUsage,
}

impl MemTable {
    /// Returns a new MemTable.
    /// This wraps opt.cmp inside a MemtableKey-specific comparator.
    pub fn new(cmp: Rc<Box<dyn Cmp>>) -> MemTable {
        MemTable::new_raw(Rc::new(Box::new(MemtableKeyCmp(cmp))))
    }

    /// Doesn't wrap the comparator in a MemtableKeyCmp.
    fn new_raw(cmp: Rc<Box<dyn Cmp>>) -> MemTable {
        MemTable {
            map: SkipMap::new(cmp),
            arena: ArenaUsage::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn approx_mem_usage(&self) -> usize {
        self.arena.usage
    }

    pub fn add(&mut self, seq: SequenceNumber, t: ValueType, key: UserKey<'_>, value: &[u8]) {
        self.arena.add_entry(key.len(), value.len());
        self.map
            .insert(build_memtable_key(key, value, t, seq), Vec::new())
    }

    /// get returns the value for the given entry and whether the entry is marked as deleted. This
    /// is to distinguish between not-found and found-deleted.
    #[allow(unused_variables)]
    pub fn get(&self, key: &LookupKey) -> (Option<Bytes>, bool) {
        let mut iter = self.map.iter();
        iter.seek(key.memtable_key());

        if let Some((foundkey, _)) = current_key_val(&iter) {
            let (fkeylen, fkeyoff, tag, vallen, valoff) = parse_memtable_key(&foundkey);

            // Compare user key -- if equal, proceed
            // We only care about user key equality here
            if key.user_key() == &foundkey[fkeyoff..fkeyoff + fkeylen] {
                if tag & 0xff == ValueType::TypeValue as u64 {
                    return (
                        Some(foundkey[valoff..valoff + vallen].to_vec().into()),
                        false,
                    );
                } else {
                    return (None, true);
                }
            }
        }
        (None, false)
    }

    pub fn iter(&self) -> MemtableIterator {
        MemtableIterator {
            skipmapiter: self.map.iter(),
        }
    }
}

/// MemtableIterator is an iterator over a MemTable. It is mostly concerned with converting to and
/// from the MemtableKey format used in the inner map; all key-taking or -returning methods deal
/// with InternalKeys.
///
/// This iterator does not skip deleted entries.
pub struct MemtableIterator {
    skipmapiter: SkipMapIter,
}

impl LdbIterator for MemtableIterator {
    fn seek_to_last(&mut self) {
        self.skipmapiter.seek_to_last();
    }
    fn advance(&mut self) -> bool {
        if !self.skipmapiter.advance() {
            return false;
        }
        self.skipmapiter.valid()
    }
    fn reset(&mut self) {
        self.skipmapiter.reset();
    }
    fn prev(&mut self) -> bool {
        self.skipmapiter.prev()
    }
    fn valid(&self) -> bool {
        self.skipmapiter.valid()
    }
    /// current returns the current key (in InternalKey format) and value.
    fn current(&self) -> Option<(Bytes, Bytes)> {
        if !self.valid() {
            return None;
        }

        if let Some((key_bytes, _val_bytes)) = self.skipmapiter.current() {
            let (keylen, keyoff, _, vallen, valoff) = parse_memtable_key(&key_bytes);
            let internal_key =
                Bytes::copy_from_slice(&key_bytes[keyoff..keyoff + keylen + u64::required_space()]);
            let value = Bytes::copy_from_slice(&key_bytes[valoff..valoff + vallen]);
            Some((internal_key, value))
        } else {
            None
        }
    }
    /// seek takes an InternalKey.
    fn seek(&mut self, to: &[u8]) {
        // Assemble the correct memtable key from the supplied InternalKey.
        let (_, seq, ukey) = parse_internal_key(to);
        self.skipmapiter
            .seek(LookupKey::new(ukey, seq).memtable_key());
    }
}

/// shift_left moves s[mid..] to s[0..s.len()-mid]. The new size is s.len()-mid.
fn shift_left(s: &mut Vec<u8>, mid: usize) {
    for i in mid..s.len() {
        s.swap(i, i - mid);
    }
    let newlen = s.len() - mid;
    s.truncate(newlen);
}

#[cfg(test)]
#[allow(unused_variables)]
mod tests {
    use super::*;
    use crate::key_types::{parse_tag, truncate_to_userkey};
    use crate::options;
    use crate::test_util::{test_iterator_properties, LdbIteratorIter};

    #[test]
    fn test_shift_left() {
        let mut v = vec![1, 2, 3, 4, 5];
        shift_left(&mut v, 1);
        assert_eq!(v, vec![2, 3, 4, 5]);

        let mut v = vec![1, 2, 3, 4, 5];
        shift_left(&mut v, 4);
        assert_eq!(v, vec![5]);
    }

    fn get_memtable() -> MemTable {
        let mut mt = MemTable::new(options::for_test().cmp);
        let entries = [
            (ValueType::TypeValue, 115, "abc", "122"),
            (ValueType::TypeValue, 120, "abc", "123"),
            (ValueType::TypeValue, 121, "abd", "124"),
            (ValueType::TypeDeletion, 122, "abe", "125"),
            (ValueType::TypeValue, 123, "abf", "126"),
        ];

        for e in entries.iter() {
            mt.add(e.1, e.0, e.2.as_bytes(), e.3.as_bytes());
        }
        mt
    }

    #[test]
    fn test_memtable_parse_tag() {
        let tag = (12345 << 8) | 1;
        assert_eq!(parse_tag(tag), (ValueType::TypeValue, 12345));
    }

    #[test]
    fn test_memtable_add() {
        let mut mt = MemTable::new(options::for_test().cmp);
        mt.add(
            123,
            ValueType::TypeValue,
            "abc".as_bytes(),
            "123".as_bytes(),
        );

        assert_eq!(
            mt.map.iter().next().unwrap().0,
            &[11, 97, 98, 99, 1, 123, 0, 0, 0, 0, 0, 0, 3, 49, 50, 51]
        );
        assert_eq!(
            mt.iter().next().unwrap().0,
            &[97, 98, 99, 1, 123, 0, 0, 0, 0, 0, 0]
        );
    }

    #[test]
    fn test_memtable_add_get() {
        let mt = get_memtable();

        // Smaller sequence number doesn't find entry
        if let Some(v) = mt.get(&LookupKey::new("abc".as_bytes(), 110)).0 {
            eprintln!("{:?}", v);
            panic!("found");
        }

        if let Some(v) = mt.get(&LookupKey::new("abf".as_bytes(), 110)).0 {
            eprintln!("{:?}", v);
            panic!("found");
        }

        // Bigger sequence number falls back to next smaller
        if let Some(v) = mt.get(&LookupKey::new("abc".as_bytes(), 116)).0 {
            assert_eq!(v, "122".as_bytes());
        } else {
            panic!("not found");
        }

        // Exact match works
        if let (Some(v), deleted) = mt.get(&LookupKey::new("abc".as_bytes(), 120)) {
            assert_eq!(v, "123".as_bytes());
            assert!(!deleted);
        } else {
            panic!("not found");
        }

        if let (None, deleted) = mt.get(&LookupKey::new("abe".as_bytes(), 122)) {
            assert!(deleted);
        } else {
            panic!("found deleted");
        }

        if let Some(v) = mt.get(&LookupKey::new("abf".as_bytes(), 129)).0 {
            assert_eq!(v, "126".as_bytes());
        } else {
            panic!("not found");
        }
    }

    #[test]
    fn test_memtable_iterator_init() {
        let mt = get_memtable();
        let mut iter = mt.iter();

        assert!(!iter.valid());
        iter.next();
        assert!(iter.valid());
        assert_eq!(
            current_key_val(&iter).unwrap().0,
            vec![97, 98, 99, 1, 120, 0, 0, 0, 0, 0, 0].as_slice()
        );
        iter.reset();
        assert!(!iter.valid());
    }

    #[test]
    fn test_memtable_iterator_seek() {
        let mt = get_memtable();
        let mut iter = mt.iter();

        assert!(!iter.valid());

        iter.seek(LookupKey::new("abc".as_bytes(), 400).internal_key());
        let (mut gotkey, gotval) = current_key_val(&iter).unwrap();
        truncate_to_userkey(&mut gotkey);
        assert_eq!(
            ("abc".as_bytes(), "123".as_bytes()),
            (gotkey.as_slice(), gotval.as_slice())
        );

        iter.seek(LookupKey::new("xxx".as_bytes(), 400).internal_key());
        assert!(!iter.valid());

        iter.seek(LookupKey::new("abd".as_bytes(), 400).internal_key());
        let (mut gotkey, gotval) = current_key_val(&iter).unwrap();
        truncate_to_userkey(&mut gotkey);
        assert_eq!(
            ("abd".as_bytes(), "124".as_bytes()),
            (gotkey.as_slice(), gotval.as_slice())
        );
    }

    #[test]
    fn test_memtable_iterator_fwd() {
        let mt = get_memtable();
        let mut iter = mt.iter();

        let expected = [
            "123".as_bytes(), /* i.e., the abc entry with
                               * higher sequence number comes first */
            "122".as_bytes(),
            "124".as_bytes(),
            // deleted entry:
            "125".as_bytes(),
            "126".as_bytes(),
        ];
        for (i, (k, v)) in LdbIteratorIter::wrap(&mut iter).enumerate() {
            assert_eq!(v, expected[i]);
        }
    }

    #[test]
    fn test_memtable_iterator_reverse() {
        let mt = get_memtable();
        let mut iter = mt.iter();

        // Bigger sequence number comes first
        iter.next();
        assert!(iter.valid());
        assert_eq!(
            current_key_val(&iter).unwrap().0,
            vec![97, 98, 99, 1, 120, 0, 0, 0, 0, 0, 0].as_slice()
        );

        iter.next();
        assert!(iter.valid());
        assert_eq!(
            current_key_val(&iter).unwrap().0,
            vec![97, 98, 99, 1, 115, 0, 0, 0, 0, 0, 0].as_slice()
        );

        iter.next();
        assert!(iter.valid());
        assert_eq!(
            current_key_val(&iter).unwrap().0,
            vec![97, 98, 100, 1, 121, 0, 0, 0, 0, 0, 0].as_slice()
        );

        iter.prev();
        assert!(iter.valid());
        assert_eq!(
            current_key_val(&iter).unwrap().0,
            vec![97, 98, 99, 1, 115, 0, 0, 0, 0, 0, 0].as_slice()
        );

        iter.prev();
        assert!(iter.valid());
        assert_eq!(
            current_key_val(&iter).unwrap().0,
            vec![97, 98, 99, 1, 120, 0, 0, 0, 0, 0, 0].as_slice()
        );

        iter.prev();
        assert!(!iter.valid());
    }

    #[test]
    fn test_memtable_parse_key() {
        let key = vec![11, 1, 2, 3, 1, 123, 0, 0, 0, 0, 0, 0, 3, 4, 5, 6];
        let (keylen, keyoff, tag, vallen, valoff) = parse_memtable_key(&key);
        assert_eq!(keylen, 3);
        assert_eq!(&key[keyoff..keyoff + keylen], vec![1, 2, 3].as_slice());
        assert_eq!(tag, (123 << 8) | 1);
        assert_eq!(vallen, 3);
        assert_eq!(&key[valoff..valoff + vallen], vec![4, 5, 6].as_slice());
    }

    #[test]
    fn test_memtable_iterator_behavior() {
        let mut mt = MemTable::new(options::for_test().cmp);
        let entries = [
            (115, "abc", "122"),
            (120, "abd", "123"),
            (121, "abe", "124"),
            (123, "abf", "126"),
        ];

        for e in entries.iter() {
            mt.add(e.0, ValueType::TypeValue, e.1.as_bytes(), e.2.as_bytes());
        }

        test_iterator_properties(mt.iter());
    }
}

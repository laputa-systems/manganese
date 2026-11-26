use crate::block::BlockContents;
use crate::filter::BoxedFilterPolicy;

use std::rc::Rc;

use integer_encoding::FixedInt;

const FILTER_BASE_LOG2: u32 = 11;
const FILTER_BASE: u32 = 1 << FILTER_BASE_LOG2; // 2 KiB

/// For a given byte offset, returns the index of the filter that includes the key at that offset.
#[inline]
fn get_filter_index(offset: usize, base_lg2: u32) -> u32 {
    // divide by 2048
    (offset >> base_lg2 as usize) as u32
}

/// A Filter Block is built like this:
///
/// [filter0, filter1, filter2, ..., offset of filter0, offset of filter1, ..., offset of offsets
/// array, log2 of FILTER_BASE]
///
/// where offsets are 4 bytes, offset of offsets is 4 bytes, and log2 of FILTER_BASE is 1 byte.
/// Two consecutive filter offsets may be the same.
pub struct FilterBlockBuilder {
    policy: BoxedFilterPolicy,
    // filters, concatenated
    filters: Vec<u8>,
    filter_offsets: Vec<usize>,

    // Reset on every start_block()
    key_offsets: Vec<usize>,
    keys: Vec<u8>,
}

impl FilterBlockBuilder {
    pub fn new(fp: BoxedFilterPolicy) -> FilterBlockBuilder {
        FilterBlockBuilder {
            policy: fp,
            // some pre-allocation
            filters: Vec::with_capacity(1024),
            filter_offsets: Vec::with_capacity(1024),
            key_offsets: Vec::with_capacity(1024),
            keys: Vec::with_capacity(1024),
        }
    }

    pub fn size_estimate(&self) -> usize {
        self.filters.len() + 4 * self.filter_offsets.len() + 4 + 1
    }

    pub fn filter_name(&self) -> &'static str {
        self.policy.name()
    }

    pub fn add_key(&mut self, key: &[u8]) {
        self.key_offsets.push(self.keys.len());
        self.keys.extend_from_slice(key);
    }

    pub fn start_block(&mut self, offset: usize) {
        let filter_ix = get_filter_index(offset, FILTER_BASE_LOG2);
        assert!(filter_ix >= self.filter_offsets.len() as u32);

        while filter_ix > self.filter_offsets.len() as u32 {
            self.generate_filter();
        }
    }

    fn generate_filter(&mut self) {
        self.filter_offsets.push(self.filters.len());
        if self.keys.is_empty() {
            return;
        }

        let filter = self.policy.create_filter(&self.keys, &self.key_offsets);
        self.filters.extend_from_slice(&filter);

        self.keys.clear();
        self.key_offsets.clear();
    }

    pub fn finish(mut self) -> Vec<u8> {
        if !self.keys.is_empty() {
            self.generate_filter();
        }

        let mut result = self.filters;
        let offsets_offset = result.len();
        let mut ix = result.len();
        result.resize(ix + 4 * self.filter_offsets.len() + 5, 0);

        // Put filter offsets at the end
        for offset in self.filter_offsets.into_iter() {
            (offset as u32).encode_fixed(&mut result[ix..ix + 4]);
            ix += 4;
        }

        (offsets_offset as u32).encode_fixed(&mut result[ix..ix + 4]);
        ix += 4;
        result[ix] = FILTER_BASE_LOG2 as u8;

        result
    }
}

#[derive(Clone)]
pub struct FilterBlockReader {
    policy: BoxedFilterPolicy,
    block: Rc<BlockContents>,

    offsets_offset: usize,
    filters: usize,
    filter_base_lg2: u32,
}

impl FilterBlockReader {
    pub fn new_owned(pol: BoxedFilterPolicy, data: Vec<u8>) -> FilterBlockReader {
        FilterBlockReader::new(pol, Rc::new(data.into()))
    }

    /// Parses a filter block. Like LevelDB, a block too short for its trailer or whose offset
    /// array lies outside the block holds no filters, so every key may match.
    pub fn new(pol: BoxedFilterPolicy, data: Rc<BlockContents>) -> FilterBlockReader {
        let mut reader = FilterBlockReader {
            policy: pol,
            block: data,
            filter_base_lg2: 0,
            offsets_offset: 0,
            filters: 0,
        };
        let n = reader.block.len();
        if n < 5 {
            return reader;
        }
        reader.filter_base_lg2 = reader.block[n - 1] as u32;
        let offsets_offset = u32::decode_fixed(&reader.block[n - 5..n - 1]) as usize;
        if offsets_offset > n - 5 {
            return reader;
        }
        reader.offsets_offset = offsets_offset;
        reader.filters = (n - 5 - offsets_offset) / 4;
        reader
    }

    /// Returns number of filters
    pub fn num(&self) -> u32 {
        self.filters as u32
    }

    /// Returns the offset of the offset with index i.
    fn offset_of(&self, i: u32) -> usize {
        let offset_offset = self.offsets_offset + 4 * i as usize;
        u32::decode_fixed(&self.block[offset_offset..offset_offset + 4]) as usize
    }

    /// blk_offset is the offset of the block containing key. Returns whether the key matches the
    /// filter for the block at blk_offset.
    ///
    /// As in LevelDB, an out-of-range index or a malformed filter range may match, and an empty
    /// filter matches nothing.
    pub fn key_may_match(&self, blk_offset: usize, key: &[u8]) -> bool {
        let index = (blk_offset as u64).wrapping_shr(self.filter_base_lg2);
        if index >= self.filters as u64 {
            return true;
        }
        let index = index as u32;
        let filter_begin = self.offset_of(index);
        let filter_end = self.offset_of(index + 1);
        if filter_begin <= filter_end && filter_end <= self.offsets_offset {
            self.policy.key_may_match(key, &self.block[filter_begin..filter_end])
        } else {
            filter_begin != filter_end
        }
    }
}

#[cfg(test)]
mod tests {
    use super::get_filter_index;
    use super::FILTER_BASE_LOG2;
    use super::*;
    use crate::filter::BloomPolicy;

    #[test]
    fn test_filter_index() {
        assert_eq!(get_filter_index(3777, FILTER_BASE_LOG2), 1);
        assert_eq!(get_filter_index(10000, FILTER_BASE_LOG2), 4);
    }

    fn get_keys() -> Vec<&'static [u8]> {
        vec![b"abcd", b"efgh", b"ijkl", b"mnopqrstuvwxyz"]
    }

    fn produce_filter_block() -> Vec<u8> {
        let keys = get_keys();
        let mut bld = FilterBlockBuilder::new(Rc::new(Box::new(BloomPolicy::new(32))));

        bld.start_block(0);

        for k in keys.iter() {
            bld.add_key(k);
        }

        // second block
        bld.start_block(5000);

        for k in keys.iter() {
            bld.add_key(k);
        }

        bld.finish()
    }

    #[test]
    fn test_filter_block_builder() {
        let result = produce_filter_block();
        // 2 blocks of 4 filters of 4 bytes plus 1B for `k`; plus three filter offsets (because of
        //   the block offsets of 0 and 5000); plus footer
        assert_eq!(result.len(), 2 * (get_keys().len() * 4 + 1) + (3 * 4) + 5);
        assert_eq!(
            result,
            vec![
                234, 195, 25, 155, 61, 141, 173, 140, 221, 28, 222, 92, 220, 112, 234, 227, 22,
                234, 195, 25, 155, 61, 141, 173, 140, 221, 28, 222, 92, 220, 112, 234, 227, 22, 0,
                0, 0, 0, 17, 0, 0, 0, 17, 0, 0, 0, 34, 0, 0, 0, 11,
            ]
        );
    }

    #[test]
    fn malformed_filter_blocks_match_every_key() {
        let policy: BoxedFilterPolicy = Rc::new(Box::new(BloomPolicy::new(32)));
        for block in [vec![], vec![0; 4], vec![0, 0, 0, 0xff, 11], vec![1, 2, 3, 0xff, 0xff, 0xff, 0xff, 11]] {
            let reader = FilterBlockReader::new_owned(policy.clone(), block);
            assert_eq!(reader.num(), 0);
            assert!(reader.key_may_match(0, b"abcd"));
        }
        let mut reversed = produce_filter_block();
        let offsets = u32::decode_fixed(&reversed[reversed.len() - 5..reversed.len() - 1]) as usize;
        reversed[offsets..offsets + 4].copy_from_slice(&40u32.to_le_bytes());
        let reader = FilterBlockReader::new_owned(policy, reversed);
        assert!(reader.key_may_match(0, b"xsb"));
    }

    #[test]
    fn an_empty_filter_matches_no_key() {
        let mut bld = FilterBlockBuilder::new(Rc::new(Box::new(BloomPolicy::new(32))));
        bld.start_block(0);
        bld.start_block(5000);
        bld.add_key(b"abcd");
        let reader = FilterBlockReader::new_owned(Rc::new(Box::new(BloomPolicy::new(32))), bld.finish());
        assert!(!reader.key_may_match(0, b"abcd"));
        assert!(reader.key_may_match(5000, b"abcd"));
    }

    #[test]
    fn test_filter_block_build_read() {
        let result = produce_filter_block();
        let reader = FilterBlockReader::new_owned(Rc::new(Box::new(BloomPolicy::new(32))), result);

        assert_eq!(
            reader.offset_of(get_filter_index(5121, FILTER_BASE_LOG2)),
            17
        ); // third block in third filter

        let unknown_keys: [&[u8]; 3] = [b"xsb", b"9sad", b"assssaaaass"];

        for block_offset in vec![0, 1024, 5000, 6025].into_iter() {
            for key in get_keys().iter() {
                assert!(
                    reader.key_may_match(block_offset, key),
                    "{} {:?} ",
                    block_offset,
                    key
                );
            }
            for key in unknown_keys.iter() {
                assert!(!reader.key_may_match(block_offset, key));
            }
        }
    }
}

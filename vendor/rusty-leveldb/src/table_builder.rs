use crate::block::BlockContents;
use crate::block_builder::BlockBuilder;
use crate::blockhandle::BlockHandle;
use crate::cmp::InternalKeyCmp;
use crate::compressor::{self, CompressorId};
use crate::crc;
use crate::error::Result;
use crate::filter::{BoxedFilterPolicy, InternalFilterPolicy};
use crate::filter_block::FilterBlockBuilder;
use crate::key_types::InternalKey;
use crate::log::mask_crc;
use crate::options::Options;

use std::cmp::Ordering;
use std::io::Write;
use std::rc::Rc;

use integer_encoding::FixedIntWriter;

pub const FOOTER_LENGTH: usize = 40;
pub const FULL_FOOTER_LENGTH: usize = FOOTER_LENGTH + 8;
pub const MAGIC_FOOTER_NUMBER: u64 = 0xdb4775248b80fb57;
pub const MAGIC_FOOTER_ENCODED: [u8; 8] = [0x57, 0xfb, 0x80, 0x8b, 0x24, 0x75, 0x47, 0xdb];

pub const TABLE_BLOCK_COMPRESS_LEN: usize = 1;
pub const TABLE_BLOCK_CKSUM_LEN: usize = 4;

/// Footer is a helper for encoding/decoding a table footer.
#[derive(Debug, Clone)]
pub struct Footer {
    pub meta_index: BlockHandle,
    pub index: BlockHandle,
}

/// A Table footer contains a pointer to the metaindex block, another pointer to the index block,
/// and a magic number:
/// [ { table data ... , METAINDEX blockhandle, INDEX blockhandle, PADDING bytes } = 40 bytes,
/// MAGIC_FOOTER_ENCODED ]
impl Footer {
    pub fn new(metaix: BlockHandle, index: BlockHandle) -> Footer {
        Footer {
            meta_index: metaix,
            index,
        }
    }

    pub fn decode(from: &[u8]) -> Option<Footer> {
        assert!(from.len() >= FULL_FOOTER_LENGTH);
        assert_eq!(&from[FOOTER_LENGTH..], &MAGIC_FOOTER_ENCODED);
        let (meta, metalen) = BlockHandle::decode(&from[0..])?;
        let (ix, _) = BlockHandle::decode(&from[metalen..])?;

        Some(Footer {
            meta_index: meta,
            index: ix,
        })
    }

    pub fn encode(&self, to: &mut [u8]) {
        assert!(to.len() >= FOOTER_LENGTH + 8);

        let s1 = self.meta_index.encode_to(to);
        let s2 = self.index.encode_to(&mut to[s1..]);

        (s1 + s2..FOOTER_LENGTH).for_each(|i| {
            to[i] = 0;
        });
        (FOOTER_LENGTH..FULL_FOOTER_LENGTH).for_each(|i| {
            to[i] = MAGIC_FOOTER_ENCODED[i - FOOTER_LENGTH];
        });
    }
}

/// A table consists of DATA BLOCKs, META BLOCKs, a METAINDEX BLOCK, an INDEX BLOCK and a FOOTER.
///
/// DATA BLOCKs, META BLOCKs, INDEX BLOCK and METAINDEX BLOCK are built using the code in
/// the `block` module.
///
/// The FOOTER consists of a BlockHandle that points to the metaindex block, another pointing to
/// the index block, padding to fill up to 40 B and at the end the 8B magic number
/// 0xdb4775248b80fb57.
pub struct TableBuilder<Dst: Write> {
    opt: Options,
    dst: Dst,

    offset: usize,
    num_entries: usize,
    last_key: Vec<u8>,
    pending_index_entry: bool,
    pending_handle: BlockHandle,

    data_block: Option<BlockBuilder>,
    index_block: Option<BlockBuilder>,
    filter_block: Option<FilterBlockBuilder>,
}

impl<Dst: Write> TableBuilder<Dst> {
    pub fn new_no_filter(mut opt: Options, dst: Dst) -> TableBuilder<Dst> {
        opt.filter_policy = None;
        TableBuilder::new(opt, dst)
    }
}

/// TableBuilder is used for building a new SSTable. It groups entries into blocks,
/// calculating checksums and bloom filters.
impl<Dst: Write> TableBuilder<Dst> {
    /// Create a new table builder.
    /// The comparator in opt will be wrapped in a InternalKeyCmp, and the filter policy
    /// in an InternalFilterPolicy.
    pub fn new(mut opt: Options, dst: Dst) -> TableBuilder<Dst> {
        opt.cmp = Rc::new(Box::new(InternalKeyCmp(opt.cmp.clone())));
        opt.filter_policy = opt
            .filter_policy
            .map(|policy| -> BoxedFilterPolicy { Rc::new(Box::new(InternalFilterPolicy::new(policy))) });
        TableBuilder::new_raw(opt, dst)
    }

    /// Like new(), but doesn't wrap the comparator in an InternalKeyCmp (for testing)
    pub fn new_raw(opt: Options, dst: Dst) -> TableBuilder<Dst> {
        let mut index_opt = opt.clone();
        index_opt.block_restart_interval = 1;
        TableBuilder {
            opt: opt.clone(),
            dst,
            offset: 0,
            last_key: vec![],
            pending_index_entry: false,
            pending_handle: BlockHandle::new(0, 0),
            num_entries: 0,
            data_block: Some(BlockBuilder::new(opt.clone())),
            filter_block: opt.filter_policy.clone().map(FilterBlockBuilder::new),
            index_block: Some(BlockBuilder::new(index_opt)),
        }
    }

    pub fn entries(&self) -> usize {
        self.num_entries
    }

    pub fn file_size(&self) -> usize {
        self.offset
    }

    pub fn size_estimate(&self) -> usize {
        let mut size = 0;
        if let Some(ref b) = self.data_block {
            size += b.size_estimate();
        }
        if let Some(ref b) = self.index_block {
            size += b.size_estimate();
        }
        if let Some(ref b) = self.filter_block {
            size += b.size_estimate();
        }
        size + self.offset + FULL_FOOTER_LENGTH
    }

    /// Add a key to the table. The key as to be lexically greater or equal to the last one added.
    pub fn add(&mut self, key: InternalKey<'_>, val: &[u8]) -> Result<()> {
        assert!(self.data_block.is_some());

        if self.num_entries > 0 {
            assert!(self.opt.cmp.cmp(key, &self.last_key) == Ordering::Greater);
        }

        if self.pending_index_entry {
            let sep = self.opt.cmp.find_shortest_sep(&self.last_key, key);
            self.add_index_entry(&sep);
        }

        if let Some(ref mut fblock) = self.filter_block {
            fblock.add_key(key);
        }

        self.last_key.clear();
        self.last_key.extend_from_slice(key);
        self.num_entries += 1;
        let dblock = self.data_block.as_mut().unwrap();
        dblock.add(key, val);

        if dblock.size_estimate() >= self.opt.block_size {
            self.flush()?;
        }
        Ok(())
    }

    fn add_index_entry(&mut self, key: &[u8]) {
        let mut handle_enc = [0_u8; 20];
        let enc_len = self.pending_handle.encode_to(&mut handle_enc);
        self.index_block
            .as_mut()
            .unwrap()
            .add(key, &handle_enc[0..enc_len]);
        self.pending_index_entry = false;
    }

    fn flush(&mut self) -> Result<()> {
        assert!(self.data_block.is_some());
        if self.data_block.as_ref().unwrap().entries() == 0 {
            return Ok(());
        }
        assert!(!self.pending_index_entry);

        let block = self.data_block.take().unwrap();
        let contents = block.finish();
        self.data_block = Some(BlockBuilder::new(self.opt.clone()));
        self.pending_handle = self.write_block(contents)?;
        self.pending_index_entry = true;

        if let Some(ref mut fblock) = self.filter_block {
            fblock.start_block(self.offset);
        }

        Ok(())
    }

    /// Compresses a block with the configured compressor unless that saves less than an eighth
    /// of its size, as LevelDB does, and writes it.
    fn write_block(&mut self, block: BlockContents) -> Result<BlockHandle> {
        let compressor_list = self.opt.compressor_list.clone();
        if self.opt.compressor != compressor::NoneCompressor::ID {
            let compressor = compressor_list.get(self.opt.compressor)?;
            let compressed = compressor.encode(block.to_vec())?;
            if compressed.len() < block.len() - block.len() / 8 {
                return self.write_raw_block(&compressed, self.opt.compressor);
            }
        }
        self.write_raw_block(&block, compressor::NoneCompressor::ID)
    }

    /// Calculates the checksum, writes the block to disk and updates the offset.
    fn write_raw_block(&mut self, data: &[u8], ctype: u8) -> Result<BlockHandle> {
        let mut digest = crc::digest();

        digest.update(data);
        digest.update(&[ctype; TABLE_BLOCK_COMPRESS_LEN]);

        self.dst.write_all(data)?;
        self.dst.write_all(&[ctype; TABLE_BLOCK_COMPRESS_LEN])?;
        self.dst.write_fixedint(mask_crc(digest.finalize()))?;

        let handle = BlockHandle::new(self.offset, data.len());
        self.offset += data.len() + TABLE_BLOCK_COMPRESS_LEN + TABLE_BLOCK_CKSUM_LEN;

        Ok(handle)
    }

    pub fn finish(self) -> Result<usize> {
        self.finish_with_writer().map(|(size, _)| size)
    }

    pub(crate) fn finish_with_writer(mut self) -> Result<(usize, Dst)> {
        assert!(self.data_block.is_some());

        self.flush()?;

        let mut filter_handle = None;
        if let Some(fblock) = self.filter_block.take() {
            let filter_key = format!("filter.{}", fblock.filter_name());
            let fblock_data = fblock.finish();
            filter_handle = Some((filter_key, self.write_raw_block(&fblock_data, compressor::NoneCompressor::ID)?));
        }

        let mut meta_ix_block = BlockBuilder::new(self.opt.clone());
        if let Some((filter_key, fblock_handle)) = filter_handle {
            let mut handle_enc = [0_u8; 20];
            let enc_len = fblock_handle.encode_to(&mut handle_enc);
            meta_ix_block.add(filter_key.as_bytes(), &handle_enc[0..enc_len]);
        }
        let meta_ix_handle = self.write_block(meta_ix_block.finish())?;

        if self.pending_index_entry {
            let succ = self.opt.cmp.find_short_succ(&self.last_key);
            self.add_index_entry(&succ);
        }
        let index_cont = self.index_block.take().unwrap().finish();
        let ix_handle = self.write_block(index_cont)?;

        let footer = Footer::new(meta_ix_handle, ix_handle);
        let mut buf = [0; FULL_FOOTER_LENGTH];
        footer.encode(&mut buf);

        self.dst.write_all(&buf[..])?;
        self.offset += buf.len();
        self.dst.flush()?;
        Ok((self.offset, self.dst))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blockhandle::BlockHandle;
    use crate::options;

    #[test]
    fn test_footer() {
        let f = Footer::new(BlockHandle::new(44, 4), BlockHandle::new(55, 5));
        let mut buf = [0; 48];
        f.encode(&mut buf[..]);

        let f2 = Footer::decode(&buf).unwrap();
        assert_eq!(f2.meta_index.offset(), 44);
        assert_eq!(f2.meta_index.size(), 4);
        assert_eq!(f2.index.offset(), 55);
        assert_eq!(f2.index.size(), 5);
    }

    #[test]
    fn test_table_builder() {
        let mut d = Vec::with_capacity(512);
        let mut opt = options::for_test();
        opt.block_restart_interval = 3;
        opt.compressor = compressor::SnappyCompressor::ID;
        let mut b = TableBuilder::new_raw(opt, &mut d);

        let data = [
            ("abc", "def"),
            ("abe", "dee"),
            ("bcd", "asa"),
            ("dcc", "a00"),
        ];
        let data2 = [
            ("abd", "def"),
            ("abf", "dee"),
            ("ccd", "asa"),
            ("dcd", "a00"),
        ];

        for i in 0..data.len() {
            b.add(data[i].0.as_bytes(), data[i].1.as_bytes()).unwrap();
            b.add(data2[i].0.as_bytes(), data2[i].1.as_bytes()).unwrap();
        }

        let estimate = b.size_estimate();

        assert_eq!(143, estimate);
        assert!(b.filter_block.is_some());

        let actual = b.finish().unwrap();
        assert_eq!(219, actual);
    }

    #[test]
    #[should_panic]
    fn test_bad_input() {
        let mut d = Vec::with_capacity(512);
        let mut opt = options::for_test();
        opt.block_restart_interval = 3;
        let mut b = TableBuilder::new_raw(opt, &mut d);

        // Test two equal consecutive keys
        let data = [
            ("abc", "def"),
            ("abc", "dee"),
            ("bcd", "asa"),
            ("bsr", "a00"),
        ];

        for &(k, v) in data.iter() {
            b.add(k.as_bytes(), v.as_bytes()).unwrap();
        }
        b.finish().unwrap();
    }
}

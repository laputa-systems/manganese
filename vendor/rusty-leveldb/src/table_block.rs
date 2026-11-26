use crate::block::Block;
use crate::blockhandle::BlockHandle;
use crate::crc;
use crate::env::RandomAccess;
use crate::error::{err, Result, StatusCode};
use crate::log::unmask_crc;
use crate::options::Options;
use crate::table_builder;

use integer_encoding::FixedInt;

/// Reads a table block from a random-access source.
/// A table block consists of [bytes..., compress (1B), checksum (4B)]; the handle only refers to
/// the location and length of [bytes...].
pub fn read_table_block(
    opt: Options,
    f: &dyn RandomAccess,
    location: &BlockHandle,
) -> Result<Block> {
    read_table_block_with_checksum(opt, f, location, true)
}

pub(crate) fn read_table_block_with_checksum(
    opt: Options,
    f: &dyn RandomAccess,
    location: &BlockHandle,
    verify_checksum: bool,
) -> Result<Block> {
    let contents = read_block_contents(&opt, f, location, verify_checksum)?;
    Block::try_new(opt, contents.into())
}

/// Reads a block and its trailer, verifies the trailer checksum when requested, and returns the
/// decompressed contents.
pub(crate) fn read_block_contents(
    opt: &Options,
    f: &dyn RandomAccess,
    location: &BlockHandle,
    verify_checksum: bool,
) -> Result<Vec<u8>> {
    let data_size = location.size();
    let trailer_size =
        table_builder::TABLE_BLOCK_COMPRESS_LEN + table_builder::TABLE_BLOCK_CKSUM_LEN;
    let total_read_size = data_size.checked_add(trailer_size)
        .ok_or_else(|| crate::Status::new(StatusCode::Corruption, "table block size overflow"))?;

    let mut combined_buf = vec![0; total_read_size];
    let bytes_read = f.read_at(location.offset(), &mut combined_buf)?;

    if bytes_read != total_read_size {
        return err(
            StatusCode::Corruption,
            "Failed to read complete block and trailer",
        );
    }

    let block_data_slice = &combined_buf[0..data_size];
    let compress_type = combined_buf[data_size];
    let cksum_slice =
        &combined_buf[data_size + table_builder::TABLE_BLOCK_COMPRESS_LEN..total_read_size];

    let expected_cksum = unmask_crc(u32::decode_fixed(cksum_slice));

    if verify_checksum && !verify_table_block(block_data_slice, compress_type, expected_cksum) {
        return err(
            StatusCode::Corruption,
            &format!(
                "checksum verification failed for block at {}",
                location.offset()
            ),
        );
    }
    let compressor_list = opt.compressor_list.clone();
    let Ok(compressor) = compressor_list.get(compress_type) else {
        return err(StatusCode::Corruption, "bad block type");
    };

    // The compressor's decode method expects a Vec<u8>.
    // If block_data_slice is empty, to_vec() is fine.
    compressor.decode(block_data_slice.to_vec())
}

/// Verify checksum of block
fn verify_table_block(data: &[u8], compression: u8, want: u32) -> bool {
    let mut digest = crc::digest();
    digest.update(data);
    digest.update(&[compression; 1]);
    digest.finalize() == want
}

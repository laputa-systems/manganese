use rusty_leveldb::compressor::SnappyCompressor;
use rusty_leveldb::{Compressor, CompressorId};

const BLOCK_SIZE: usize = 1 << 16;
const MIN_HASH_TABLE_SIZE: usize = 1 << 8;
const MAX_HASH_TABLE_SIZE: usize = 1 << 15;
const INPUT_MARGIN: usize = 15;
const HASH_MAGIC: u32 = 0x1e35a7bd;
const LITERAL: u8 = 0;
const COPY_1_BYTE_OFFSET: u32 = 1;
const COPY_2_BYTE_OFFSET: u32 = 2;

pub struct ChromiumSnappy;

impl CompressorId for ChromiumSnappy {
    const ID: u8 = 1;
}

impl Compressor for ChromiumSnappy {
    fn encode(&self, block: Vec<u8>) -> rusty_leveldb::Result<Vec<u8>> {
        Ok(compress(&block))
    }

    fn decode(&self, block: Vec<u8>) -> rusty_leveldb::Result<Vec<u8>> {
        SnappyCompressor.decode(block)
    }
}

fn load32(input: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(input[at..at + 4].try_into().expect("four bytes"))
}

fn table_size(input_size: usize) -> usize {
    if input_size > MAX_HASH_TABLE_SIZE {
        MAX_HASH_TABLE_SIZE
    } else if input_size < MIN_HASH_TABLE_SIZE {
        MIN_HASH_TABLE_SIZE
    } else {
        input_size.next_power_of_two()
    }
}

fn slot(bytes: u32, table_size: usize) -> usize {
    (HASH_MAGIC.wrapping_mul(bytes) >> 16) as usize >> 1 & (table_size - 1)
}

pub fn compress(input: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(32 + input.len() + input.len() / 6);
    let mut length = u32::try_from(input.len()).expect("snappy input fits in 32 bits");
    while length >= 0x80 {
        output.push(length as u8 | 0x80);
        length >>= 7;
    }
    output.push(length as u8);
    for fragment in input.chunks(BLOCK_SIZE) {
        compress_fragment(fragment, &mut output);
    }
    output
}

fn emit_literal(output: &mut Vec<u8>, literal: &[u8]) {
    let n = literal.len() - 1;
    if n < 60 {
        output.push(LITERAL | (n as u8) << 2);
    } else {
        let count = (n.ilog2() as usize >> 3) + 1;
        output.push(LITERAL | ((59 + count) as u8) << 2);
        output.extend_from_slice(&(n as u32).to_le_bytes()[..count]);
    }
    output.extend_from_slice(literal);
}

fn emit_copy_at_most_64(output: &mut Vec<u8>, offset: usize, length: usize) {
    if length < 12 && offset < 2048 {
        output.push((COPY_1_BYTE_OFFSET + ((length as u32 - 4) << 2) + ((offset as u32 >> 8) << 5)) as u8);
        output.push(offset as u8);
    } else {
        let word = COPY_2_BYTE_OFFSET + ((length as u32 - 1) << 2) + ((offset as u32) << 8);
        output.extend_from_slice(&word.to_le_bytes()[..3]);
    }
}

fn emit_copy(output: &mut Vec<u8>, offset: usize, mut length: usize) {
    if length < 12 {
        emit_copy_at_most_64(output, offset, length);
        return;
    }
    while length >= 68 {
        emit_copy_at_most_64(output, offset, 64);
        length -= 64;
    }
    if length > 64 {
        emit_copy_at_most_64(output, offset, 60);
        length -= 60;
    }
    emit_copy_at_most_64(output, offset, length);
}

fn match_length(input: &[u8], candidate: usize, ip: usize) -> usize {
    input[ip..].iter().zip(&input[candidate..]).take_while(|(a, b)| a == b).count()
}

fn compress_fragment(input: &[u8], output: &mut Vec<u8>) {
    let size = table_size(input.len());
    let mut table = vec![0u16; size];
    let end = input.len();
    let mut ip = 0;
    let mut next_emit = 0;
    if end >= INPUT_MARGIN {
        let limit = end - INPUT_MARGIN;
        'outer: loop {
            next_emit = ip;
            ip += 1;
            let mut skip: u32 = 32;
            let mut candidate = None;
            if ip + 16 <= limit {
                for i in 0..16 {
                    let at = ip + i;
                    let bytes = load32(input, at);
                    let entry = slot(bytes, size);
                    let found = table[entry] as usize;
                    table[entry] = at as u16;
                    if load32(input, found) == bytes {
                        candidate = Some(found);
                        ip = at;
                        break;
                    }
                }
                if candidate.is_none() {
                    ip += 16;
                    skip += 16;
                }
            }
            let mut candidate = match candidate {
                Some(found) => found,
                None => loop {
                    let bytes = load32(input, ip);
                    let entry = slot(bytes, size);
                    let step = (skip >> 5) as usize;
                    skip += skip >> 5;
                    let next = ip + step;
                    if next > limit {
                        break 'outer;
                    }
                    let found = table[entry] as usize;
                    table[entry] = ip as u16;
                    if load32(input, found) == bytes {
                        break found;
                    }
                    ip = next;
                },
            };
            emit_literal(output, &input[next_emit..ip]);
            loop {
                let base = ip;
                let matched = 4 + match_length(input, candidate + 4, ip + 4);
                ip += matched;
                emit_copy(output, base - candidate, matched);
                next_emit = ip;
                if ip >= limit {
                    break 'outer;
                }
                let previous = slot(load32(input, ip - 1), size);
                table[previous] = (ip - 1) as u16;
                let entry = slot(load32(input, ip), size);
                candidate = table[entry] as usize;
                table[entry] = ip as u16;
                if load32(input, ip) != load32(input, candidate) {
                    break;
                }
            }
        }
    }
    if next_emit < end {
        emit_literal(output, &input[next_emit..]);
    }
}

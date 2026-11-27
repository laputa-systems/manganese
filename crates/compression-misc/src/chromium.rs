const BLOCK_SIZE: usize = 1 << 16;
const INPUT_MARGIN: usize = 15;
const MIN_TABLE_SIZE: usize = 1 << 8;
const MAX_TABLE_SIZE: usize = 1 << 15;
const MAGIC: u32 = 0x1e35a7bd;

fn load32(input: &[u8], position: usize) -> u32 {
    u32::from_le_bytes(input[position..position + 4].try_into().unwrap())
}

fn load64(input: &[u8], position: usize) -> u64 {
    u64::from_le_bytes(input[position..position + 8].try_into().unwrap())
}

fn table_size(fragment_size: usize) -> usize {
    if fragment_size > MAX_TABLE_SIZE {
        MAX_TABLE_SIZE
    } else if fragment_size < MIN_TABLE_SIZE {
        MIN_TABLE_SIZE
    } else {
        2usize << (usize::BITS - 1 - (fragment_size - 1).leading_zeros())
    }
}

fn table_entry(table_len: usize, bytes: u32) -> usize {
    let hash = MAGIC.wrapping_mul(bytes) >> 16;
    ((hash as usize) & (2 * (table_len - 1))) / 2
}

fn emit_literal(output: &mut Vec<u8>, input: &[u8], start: usize, length: usize, fast: bool) {
    let n = length - 1;
    if fast && length <= 16 {
        output.push((n as u8) << 2);
        output.extend_from_slice(&input[start..start + length]);
        return;
    }
    if n < 60 {
        output.push((n as u8) << 2);
    } else {
        let count = (((usize::BITS - 1 - n.leading_zeros()) >> 3) + 1) as usize;
        output.push(((59 + count) as u8) << 2);
        output.extend_from_slice(&n.to_le_bytes()[..count]);
    }
    output.extend_from_slice(&input[start..start + length]);
}

fn emit_copy_at_most_64(output: &mut Vec<u8>, offset: usize, length: usize, short: bool) {
    if short {
        let mut value = ((length << 2) + (offset << 8)) as u32;
        let copy1 = 1u32.wrapping_sub(4 << 2).wrapping_add(((offset >> 3) & 0xe0) as u32);
        let copy2 = 2u32.wrapping_sub(1 << 2);
        value = value.wrapping_add(if offset < 2048 { copy1 } else { copy2 });
        let bytes = value.to_le_bytes();
        output.extend_from_slice(&bytes[..if offset < 2048 { 2 } else { 3 }]);
    } else {
        let value = 2u32.wrapping_add(((length - 1) << 2) as u32).wrapping_add((offset << 8) as u32);
        output.extend_from_slice(&value.to_le_bytes()[..3]);
    }
}

fn emit_copy(output: &mut Vec<u8>, offset: usize, mut length: usize) {
    if length < 12 {
        emit_copy_at_most_64(output, offset, length, true);
        return;
    }
    while length >= 68 {
        emit_copy_at_most_64(output, offset, 64, false);
        length -= 64;
    }
    if length > 64 {
        emit_copy_at_most_64(output, offset, 60, false);
        length -= 60;
    }
    emit_copy_at_most_64(output, offset, length, length < 12);
}

struct Match {
    matched: usize,
    data: u64,
}

fn find_match_length(input: &[u8], candidate: usize, start: usize, limit: usize, incoming: u64) -> Match {
    let mut position = start;
    let mut matched = 0usize;
    let mut data = incoming;
    if position + 16 <= limit {
        let first = load64(input, candidate);
        let second = load64(input, position);
        if first != second {
            let difference = first ^ second;
            let shift = difference.trailing_zeros() as usize;
            let bytes = shift >> 3;
            let later = load64(input, position + 4);
            let chosen = if difference as u32 == 0 { later } else { second };
            return Match { matched: bytes, data: chosen >> (shift & 24) };
        }
        matched = 8;
        position += 8;
    }
    while position + 16 <= limit {
        let first = load64(input, candidate + matched);
        let second = load64(input, position);
        if first == second {
            position += 8;
            matched += 8;
        } else {
            let difference = first ^ second;
            let shift = difference.trailing_zeros() as usize;
            let bytes = shift >> 3;
            let later = load64(input, position + 4);
            let chosen = if difference as u32 == 0 { later } else { second };
            matched += bytes;
            return Match { matched, data: chosen >> (shift & 24) };
        }
    }
    while position < limit {
        if input[candidate + matched] == input[position] {
            position += 1;
            matched += 1;
        } else {
            if position + 8 <= limit {
                data = load64(input, position);
            }
            return Match { matched, data };
        }
    }
    Match { matched, data }
}

fn compress_fragment(input: &[u8], output: &mut Vec<u8>, table: &mut [u16]) {
    let size = input.len();
    if size < INPUT_MARGIN {
        emit_literal(output, input, 0, size, false);
        return;
    }
    let ip_limit = size - INPUT_MARGIN;
    let mut ip = 0usize;
    let mut preload = load32(input, 1) as u64;
    'remainder: loop {
        let next_emit = ip;
        ip += 1;
        let mut data = load64(input, ip);
        let mut skip = 32u32;
        let mut candidate: usize;
        if ip + 16 <= ip_limit {
            for j in 0..4 {
                for k in 0..4 {
                    let index = 4 * j + k;
                    let word = if index == 0 { preload as u32 } else { data as u32 };
                    let slot = table_entry(table.len(), word);
                    candidate = table[slot] as usize;
                    table[slot] = (ip + index) as u16;
                    if load32(input, candidate) == word {
                        output.push((index as u8) << 2);
                        output.extend_from_slice(&input[next_emit..next_emit + index + 1]);
                        ip += index;
                        loop {
                            let found = find_match_length(input, candidate + 4, ip + 4, size, data);
                            let matched = 4 + found.matched;
                            let begin = ip;
                            ip += matched;
                            emit_copy(output, begin - candidate, matched);
                            if ip >= ip_limit {
                                break 'remainder;
                            }
                            let slot = table_entry(table.len(), load32(input, ip - 1));
                            table[slot] = (ip - 1) as u16;
                            let slot = table_entry(table.len(), found.data as u32);
                            candidate = table[slot] as usize;
                            table[slot] = ip as u16;
                            preload = found.data >> 8;
                            if found.data as u32 != load32(input, candidate) {
                                break;
                            }
                        }
                        continue 'remainder;
                    }
                    data >>= 8;
                }
                data = load64(input, ip + 4 * j + 4);
            }
            ip += 16;
            skip += 16;
        }
        loop {
            let slot = table_entry(table.len(), data as u32);
            let between = (skip >> 5) as usize;
            skip += between as u32;
            let next_ip = ip + between;
            if next_ip > ip_limit {
                ip = next_emit;
                break 'remainder;
            }
            candidate = table[slot] as usize;
            table[slot] = ip as u16;
            if data as u32 == load32(input, candidate) {
                break;
            }
            data = load32(input, next_ip) as u64;
            ip = next_ip;
        }
        emit_literal(output, input, next_emit, ip - next_emit, true);
        loop {
            let found = find_match_length(input, candidate + 4, ip + 4, size, data);
            let matched = 4 + found.matched;
            let base = ip;
            ip += matched;
            emit_copy(output, base - candidate, matched);
            if ip >= ip_limit {
                break 'remainder;
            }
            let slot = table_entry(table.len(), load32(input, ip - 1));
            table[slot] = (ip - 1) as u16;
            let slot = table_entry(table.len(), found.data as u32);
            candidate = table[slot] as usize;
            table[slot] = ip as u16;
            preload = found.data >> 8;
            if found.data as u32 != load32(input, candidate) {
                break;
            }
        }
    }
    if ip < size {
        emit_literal(output, input, ip, size - ip, false);
    }
}

fn encode_varint(output: &mut Vec<u8>, mut value: u32) {
    while value >= 0x80 {
        output.push((value as u8) | 0x80);
        value >>= 7;
    }
    output.push(value as u8);
}

pub fn compress(input: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(32 + input.len() + input.len() / 6);
    encode_varint(&mut output, input.len() as u32);
    let mut table = vec![0u16; table_size(input.len().min(BLOCK_SIZE))];
    for fragment in input.chunks(BLOCK_SIZE) {
        table.fill(0);
        compress_fragment(fragment, &mut output, &mut table);
    }
    output
}

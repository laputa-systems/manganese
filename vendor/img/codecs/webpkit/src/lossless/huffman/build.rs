//! Length-limited canonical Huffman code-length assignment for the encoder.
//!
//! Given a symbol frequency histogram this produces a per-symbol *code length*
//! vector (0 = unused) whose lengths form a complete prefix code no deeper than a
//! caller-supplied limit. Only the multiset of lengths matters downstream:
//! [`crate::lossless::huffman::canonical`] turns lengths into canonical codes and the
//! decoder's [`crate::lossless::huffman::decode::HuffmanTable::build`] rebuilds the same
//! table from lengths alone, so this module never assigns codes itself.
//!
//! The depth limit (15 for the main channel codes, 7 for the code-length meta
//! alphabet) is enforced with libwebp's `count_min` clamp: build an optimal tree,
//! and if it is too deep raise a floor on every used weight and rebuild. Raising
//! the floor flattens the frequency distribution; once every weight is clamped to
//! the same value the tree is as balanced as possible (depth `ceil(log2 n)`),
//! which is well within both limits for our alphabets (`n <= 280`), so the loop is
//! guaranteed to terminate.

use alloc::collections::BinaryHeap;
use core::cmp::Reverse;

use crate::lossless::prelude::*;

/// Sentinel child/marker for a leaf node (a real symbol index is always small).
const NONE: usize = usize::MAX;

/// One node of the Huffman work tree, stored in a flat pool addressed by index.
///
/// A leaf has `symbol` set to its histogram index and both children `NONE`; an
/// internal node has `symbol == NONE` and `left`/`right` pointing into the pool.
struct Node {
    /// Total (possibly `count_min`-clamped) frequency under this node.
    count: u64,
    /// Left child pool index, or [`NONE`] for a leaf.
    left: usize,
    /// Right child pool index, or [`NONE`] for a leaf.
    right: usize,
    /// Histogram index for a leaf, or [`NONE`] for an internal node.
    symbol: usize,
}

/// Walk the tree rooted at `root` and record each leaf's depth as its code length.
///
/// Returns a vector of length `alphabet_size` (0 for symbols not in the tree).
fn assign_depths(pool: &[Node], root: usize, alphabet_size: usize) -> Vec<u32> {
    let mut lengths = vec![0u32; alphabet_size];
    let mut stack = vec![(root, 0u32)];
    while let Some((idx, depth)) = stack.pop() {
        let node = &pool[idx];
        if node.symbol == NONE {
            stack.push((node.left, depth + 1));
            stack.push((node.right, depth + 1));
        } else {
            lengths[node.symbol] = depth;
        }
    }
    lengths
}


/// The number of used symbols, upper-bounding the leaf count for capacity sizing.
fn pool_hint(histogram: &[u32]) -> usize {
    histogram.iter().filter(|&&freq| freq != 0).count()
}

/// Build an optimal Huffman tree over the used symbols with every weight floored
/// at `count_min`, and return per-symbol depths (code lengths).
///
/// Requires at least two used symbols (the caller handles the 0/1 cases); with
/// fewer, no meaningful tree exists.
fn huffman_code_lengths(histogram: &[u32], count_min: u64) -> Vec<u32> {
    let mut pool: Vec<Node> = Vec::with_capacity(2 * pool_hint(histogram) - 1);
    for (symbol, &freq) in histogram.iter().enumerate() {
        if freq != 0 {
            pool.push(Node {
                count: u64::from(freq).max(count_min),
                left: NONE,
                right: NONE,
                symbol,
            });
        }
    }
    debug_assert!(pool.len() >= 2, "caller must handle the 0/1-symbol cases");

    // Min-heap on `(count, pool index)`; the two smallest entries are unique at
    // every step, so the merge sequence — and the resulting lengths — are exactly
    // the ones the quadratic scan selected, at O(n log n) instead of O(n^2).
    let mut heap: BinaryHeap<Reverse<(u64, usize)>> = pool
        .iter()
        .enumerate()
        .map(|(idx, node)| Reverse((node.count, idx)))
        .collect();
    while heap.len() > 1 {
        let Reverse(a) = heap.pop().expect("heap has at least two");
        let Reverse(b) = heap.pop().expect("heap has at least two");
        let parent = pool.len();
        pool.push(Node {
            count: pool[a.1].count + pool[b.1].count,
            left: a.1,
            right: b.1,
            symbol: NONE,
        });
        heap.push(Reverse((pool[parent].count, parent)));
    }

    assign_depths(&pool, heap.pop().expect("one tree remains").0.1, histogram.len())
}

/// Perturb `counts` so the tree built from them run-length-encodes smaller —
/// libwebp's `OptimizeHuffmanForRle`. Strides of near-equal counts collapse to
/// their rounded mean (extending equal-length runs the CL encoder loves), and
/// existing good RLE strides are left untouched. `good_for_rle` is scratch,
/// zeroed by the caller or here; both slices share the histogram length.
pub(crate) fn optimize_huffman_for_rle(counts: &mut [u32], good_for_rle: &mut [u8]) {
    let length = counts
        .iter()
        .rposition(|&c| c != 0)
        .map_or(0, |i| i + 1);
    if length == 0 {
        return;
    }
    good_for_rle.fill(0);
    // Mark runs already encodable by the CL repeat codes: zero runs >= 5 and
    // nonzero runs >= 7.
    let mut symbol = counts[0];
    let mut stride = 0usize;
    for (i, &count) in counts
        .iter()
        .chain(core::iter::once(&0u32))
        .enumerate()
        .take(length + 1)
    {
        if i == length || count != symbol {
            if (symbol == 0 && stride >= 5) || (symbol != 0 && stride >= 7) {
                for slot in good_for_rle.iter_mut().take(i).skip(i - stride) {
                    *slot = 1;
                }
            }
            stride = 1;
            if i != length {
                symbol = count;
            }
        } else {
            stride += 1;
        }
    }
    // Collapse near-equal count strides to their rounded mean.
    let mut stride = 0usize;
    let mut limit = u64::from(counts[0]);
    let mut sum = 0u64;
    for i in 0..=length {
        let collapsible = i != length
            && counts[i].abs_diff(limit as u32) < 4;
        let boundary = i == length
            || good_for_rle[i] == 1
            || (i != 0 && good_for_rle[i - 1] == 1)
            || !collapsible;
        if boundary {
            if stride >= 4 || (stride >= 3 && sum == 0) {
                let mut count = (sum + stride as u64 / 2) / stride as u64;
                if count < 1 {
                    count = 1;
                }
                if sum == 0 {
                    count = 0;
                }
                for slot in counts.iter_mut().take(i).skip(i - stride) {
                    *slot = count as u32;
                }
            }
            stride = 0;
            sum = 0;
            limit = if i + 3 < length {
                (u64::from(counts[i])
                    + u64::from(counts[i + 1])
                    + u64::from(counts[i + 2])
                    + u64::from(counts[i + 3])
                    + 2)
                    / 4
            } else if i < length {
                u64::from(counts[i])
            } else {
                0
            };
        }
        stride += 1;
        if i != length {
            sum += u64::from(counts[i]);
            if stride >= 4 {
                limit = (sum + stride as u64 / 2) / stride as u64;
            }
        }
    }
}

/// Assign length-limited canonical code lengths for `histogram`.
///
/// Returns a vector the same length as `histogram`; entry `i` is the code length
/// for symbol `i` (0 for an unused symbol, i.e. `histogram[i] == 0`). Used
/// symbols receive lengths in `1..=limit`, and when more than one symbol is used
/// the lengths form a *complete* prefix code (Kraft equality holds).
///
/// A lone used symbol is given length 1 (0 means "unused", so it cannot be used
/// here); the serializer collapses such a single-symbol alphabet to zero emitted
/// bits per occurrence, matching the decoder's 0-bit leaf.
///
/// `limit` must be large enough to express a balanced tree over the used symbols
/// (`limit >= ceil(log2 n)`); the encoder always passes 15 or 7, which is ample
/// for the VP8L alphabets (`n <= 280`).
#[must_use]
pub(crate) fn build_code_lengths(histogram: &[u32], limit: u32) -> Vec<u32> {
    let num_used = histogram.iter().filter(|&&freq| freq != 0).count();
    let mut lengths = vec![0u32; histogram.len()];
    match num_used {
        0 => return lengths,
        1 => {
            if let Some(idx) = histogram.iter().position(|&freq| freq != 0) {
                lengths[idx] = 1;
            }
            return lengths;
        },
        _ => {},
    }

    // Optimal tree first; raise the weight floor and retry while too deep.
    let mut count_min = 1u64;
    loop {
        let candidate = huffman_code_lengths(histogram, count_min);
        if candidate.iter().copied().max().unwrap_or(0) <= limit {
            return candidate;
        }
        count_min <<= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::build_code_lengths;
    use crate::lossless::constants::HUFFMAN_TABLE_BITS;
    use crate::lossless::huffman::decode::HuffmanTable;

    /// The longest code length present (0 for an all-unused alphabet).
    fn max_length(lengths: &[u32]) -> u32 {
        lengths.iter().copied().max().unwrap_or(0)
    }

    /// The decoder must be able to rebuild a table from these lengths (this proves
    /// the lengths are a complete, prefix-free code). Only meaningful when at
    /// least one symbol is used.
    fn assert_decoder_accepts(lengths: &[u32]) {
        assert!(
            HuffmanTable::build(lengths, HUFFMAN_TABLE_BITS).is_some(),
            "decoder must accept lengths {lengths:?}"
        );
    }

    /// For a complete prefix code over more than one symbol, Kraft's inequality is
    /// an equality: `sum 2^(maxlen - len) == 2^maxlen`.
    fn assert_kraft_when_multi(lengths: &[u32]) {
        let used: Vec<u32> = lengths.iter().copied().filter(|&l| l != 0).collect();
        if used.len() <= 1 {
            return;
        }
        let max_len = used.iter().copied().max().unwrap();
        let sum: u64 = used.iter().map(|&l| 1u64 << (max_len - l)).sum();
        assert_eq!(
            sum,
            1u64 << max_len,
            "Kraft equality must hold for {lengths:?}"
        );
    }

    #[test]
    fn empty_alphabet_is_all_zero() {
        assert_eq!(build_code_lengths(&[0, 0, 0], 15), vec![0, 0, 0]);
        assert_eq!(build_code_lengths(&[], 15), Vec::<u32>::new());
    }

    #[test]
    fn single_used_symbol_gets_length_one() {
        let mut hist = vec![0u32; 10];
        hist[3] = 42;
        let lengths = build_code_lengths(&hist, 15);
        let mut expected = vec![0u32; 10];
        expected[3] = 1;
        assert_eq!(lengths, expected);
        // A single-symbol alphabet still builds (as a 0-bit leaf in the decoder).
        assert_decoder_accepts(&lengths);
    }

    #[test]
    fn two_used_symbols_each_get_length_one() {
        let lengths = build_code_lengths(&[5, 9], 15);
        assert_eq!(lengths, vec![1, 1]);
        assert_kraft_when_multi(&lengths);
        assert_decoder_accepts(&lengths);
    }

    #[test]
    fn several_histograms_produce_decodable_codes() {
        let cases: [&[u32]; 5] = [
            &[1, 1, 1, 1],
            &[10, 1, 1, 1, 1],
            &[100, 50, 25, 12, 6, 3, 1],
            &[3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3],
            &[0, 7, 0, 0, 2, 5, 0, 9, 1, 0],
        ];
        for hist in cases {
            let lengths = build_code_lengths(hist, 15);
            assert_eq!(lengths.len(), hist.len());
            for (i, &l) in lengths.iter().enumerate() {
                assert_eq!(l == 0, hist[i] == 0, "used-ness must match for {hist:?}");
            }
            assert!(max_length(&lengths) <= 15, "must respect the 15-bit limit");
            assert_kraft_when_multi(&lengths);
            assert_decoder_accepts(&lengths);
        }
    }

    #[test]
    fn fibonacci_weights_are_depth_limited() {
        // Fibonacci frequencies yield a maximally unbalanced ("caterpillar")
        // Huffman tree whose natural depth is ~num_symbols - 1, far past 15. The
        // count_min clamp must flatten it to <= 15 while staying decodable.
        let mut hist = Vec::new();
        let (mut a, mut b) = (1u32, 1u32);
        for _ in 0..40 {
            hist.push(a);
            let next = a.saturating_add(b);
            a = b;
            b = next;
        }
        let lengths = build_code_lengths(&hist, 15);
        assert!(max_length(&lengths) <= 15, "count_min must cap depth at 15");
        assert_kraft_when_multi(&lengths);
        assert_decoder_accepts(&lengths);
    }

    #[test]
    fn skewed_nineteen_entries_respect_limit_seven() {
        // The 19-symbol code-length meta alphabet, skewed enough that the
        // unconstrained tree would exceed 7, encoded under the 7-bit meta limit.
        let hist: [u32; 19] = [
            1000, 500, 250, 120, 60, 30, 15, 8, 4, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1,
        ];
        let lengths = build_code_lengths(&hist, 7);
        assert!(
            max_length(&lengths) <= 7,
            "must respect the 7-bit meta limit"
        );
        assert_kraft_when_multi(&lengths);
        assert_decoder_accepts(&lengths);
    }
}

#[cfg(test)]
mod proptests {
    use super::build_code_lengths;
    use crate::lossless::constants::HUFFMAN_TABLE_BITS;
    use crate::lossless::huffman::decode::HuffmanTable;
    use proptest::prelude::*;

    proptest! {
        /// Whatever the histogram, the output is well-formed: unused symbols get
        /// length 0, used symbols land in 1..=15, more-than-one-symbol alphabets
        /// satisfy Kraft equality, and the decoder rebuilds a table from them.
        #[test]
        fn produces_valid_length_limited_codes(
            freqs in proptest::collection::vec(0u32..=4096, 1..=280)
        ) {
            let lengths = build_code_lengths(&freqs, 15);
            prop_assert_eq!(lengths.len(), freqs.len());

            let mut used = 0usize;
            for (i, &l) in lengths.iter().enumerate() {
                if freqs[i] == 0 {
                    prop_assert_eq!(l, 0, "unused symbol must have length 0");
                } else {
                    prop_assert!((1..=15).contains(&l), "used length must be in 1..=15");
                    used += 1;
                }
            }

            if used > 1 {
                let max_len = lengths.iter().copied().max().unwrap_or(0);
                let sum: u64 = lengths
                    .iter()
                    .filter(|&&l| l != 0)
                    .map(|&l| 1u64 << (max_len - l))
                    .sum();
                prop_assert_eq!(sum, 1u64 << max_len, "Kraft equality must hold");
            }

            if used >= 1 {
                prop_assert!(
                    HuffmanTable::build(&lengths, HUFFMAN_TABLE_BITS).is_some(),
                    "decoder must accept the generated lengths"
                );
            }
        }
    }
}

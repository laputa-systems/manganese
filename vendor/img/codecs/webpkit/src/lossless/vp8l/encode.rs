//! VP8L payload encoder (Tier 0/1/2): literals, an optional subtract-green
//! transform, LZ77 back-references, and a color cache.
//!
//! This is the exact producer for [`crate::lossless::vp8l::decode::decode`]. For each of
//! the two tiers — Tier 0 (raw pixels) and Tier 1 (subtract-green) — the encoder
//! parses the pixels into [`Token`]s once, uses the [`Histogram`] cost model to
//! pick a color-cache size for each reference strategy, then materializes a
//! small set of candidates and keeps the smallest by actual bytes. Because the
//! all-literal / no-cache candidate is always in the set, a solid image still
//! collapses to an empty pixel-data section and the encoder never does worse
//! than the plain Tier 0/1 baseline.
//!
//! Bit-exactness rests on traps handled elsewhere and relied on here:
//! [`crate::lossless::huffman::canonical::emit_codes`] returns `(0, 0)` for any alphabet
//! with `<= 1` used symbol (a single-valued channel costs zero bits per pixel);
//! [`crate::lossless::transform::subtract_green::forward`] subtracts per channel so
//! borrows never propagate from blue into red; and [`resolve`] is the one place
//! the color cache is applied, so the histogram and emit passes insert pixels in
//! the identical order the decoder does.
#![expect(
    clippy::cast_possible_truncation,
    reason = "channel indices are masked to 8 bits, prefix symbols index alphabets \
              bounded by the format, and pixel counts fit the validated 14-bit VP8L \
              dimensions, so every narrowing cast here is value-preserving"
)]

use crate::lossless::bit_io::writer::BitWriter;
use crate::lossless::color_cache::ColorCache;
use crate::lossless::constants::{
    ALPHABET_SIZE, COLOR_INDEXING_TRANSFORM, CROSS_COLOR_TRANSFORM, DEEP_CHAIN_MIN_PIXELS,
    MAX_CACHE_BITS, MAX_CHAIN, MAX_CHAIN_OPTIMAL, MIN_TRANSFORM_BITS, NUM_LENGTH_CODES,
    NUM_LITERAL_CODES, NUM_TRANSFORM_BITS, PREDICTOR_TRANSFORM, SUBTRACT_GREEN_TRANSFORM,
    VP8L_IMAGE_SIZE_BITS, VP8L_MAGIC_BYTE, VP8L_VERSION_BITS, subsample_size,
};
use crate::lossless::histogram::{Histogram, shannon_bits};
use crate::lossless::huffman::build::{build_code_lengths, optimize_huffman_for_rle};
use crate::lossless::huffman::canonical::emit_codes;
use crate::lossless::huffman::serialize::{serialized_bits, write_huffman_code};
use crate::lossless::lz77::{PlaneCodeMap, prefix_encode};
use crate::lossless::prelude::*;
use crate::lossless::transform::{cross_color, palette, predictor, subtract_green};
use crate::lossless::vp8l::backref::{Resolved, Token, parse, parse_lz77, parse_optimal, parse_rle, resolve};
use crate::lossless::vp8l::meta::{self, MetaPlan};
use crate::lossless::work::work;

/// Maximum Huffman code length for the five main channel codes. VP8L caps code
/// lengths at 15 bits; kept in lockstep with the decoder's
/// [`crate::lossless::constants::MAX_ALLOWED_CODE_LENGTH`] by the assertion below.
const MAX_MAIN_CODE_LENGTH: u32 = 15;
const _: () = assert!(crate::lossless::constants::MAX_ALLOWED_CODE_LENGTH == 15);

/// The 2-bit transform-type field width in the VP8L transform header.
const TRANSFORM_TYPE_BITS: u32 = 2;
/// Width of the transmitted color-cache-bits field.
const CACHE_BITS_FIELD_WIDTH: u32 = 4;
/// Width of the transmitted `num_colors - 1` field of a color-indexing transform.
const NUM_COLORS_FIELD_WIDTH: u32 = 8;

/// One transform to write into the VP8L transform list, carrying whatever nested
/// sub-image the decoder reads back for it. Written in vec order by
/// [`write_transforms`]; the decoder applies the inverses in reverse.
///
/// Predictor/cross-color carry a per-tile sub-image (`subsample_size(width, bits)`
/// wide); color-indexing carries the `num_colors`-wide color map. Subtract-green
/// is pointwise and carries no sub-image.
enum TransformPlan {
    /// Spatial predictor: per-tile mode sub-image (mode packed as `(m & 0xf) << 8`).
    Predictor { bits: u32, tile_data: Vec<u32> },
    /// Cross-color decorrelation: per-tile multiplier-code sub-image.
    CrossColor { bits: u32, tile_data: Vec<u32> },
    /// Subtract-green: pointwise, no sub-image.
    SubtractGreen,
    /// Color indexing (palette): a `num_colors`-wide delta-coded color-map sub-image.
    ColorIndexing { num_colors: u32, colormap: Vec<u32> },
}

/// Encode `argb` (native `0xAARRGGBB`, row-major, `width * height` pixels) as a
/// raw VP8L payload beginning at the `0x2f` signature.
///
/// Both tiers are optimized independently and the smaller result is returned; on
/// a tie Tier 0 wins. The output is a valid VP8L bitstream that round-trips
/// through [`crate::lossless::vp8l::decode::decode`].
#[must_use]
pub(crate) fn encode(width: u32, height: u32, argb: &[u32]) -> Vec<u8> {
    encode_with(width, height, argb, true, MAX_CHAIN_OPTIMAL, false)
}

/// Encode as [`encode`], but with `use_lz77` cleared (the `Fast` method) the
/// encoder skips the LZ77 / color-cache search and emits only the literal +
/// subtract-green tiers — faster, slightly larger.
///
/// `chain_cap` bounds the optimal-parse DP staircase walk the LZ77 search
/// runs; callers pick it with [`effective_chain_cap`] or a fixed level cap.
/// `meta_huffman` offers the multi-group meta-Huffman shot on each tier's
/// token stream — the ALPH stream's search, where the single-channel plane is
/// exactly the runny content the grouping exploits.
#[must_use]
pub(crate) fn encode_with(
    width: u32,
    height: u32,
    argb: &[u32],
    use_lz77: bool,
    chain_cap: u32,
    meta_huffman: bool,
) -> Vec<u8> {
    // Alpha-used is a header advisory computed once; subtract-green leaves the
    // alpha channel untouched, so both tiers share the same value.
    let alpha_used = argb.iter().any(|&p| p >> 24 != 0xff);
    let source = PixelSource {
        width,
        height,
        argb,
    };
    let switches = StreamSwitches {
        use_subtract_green: false,
        use_lz77,
        chain_cap,
        meta_huffman,
    };
    let tier0 = encode_stream(source, alpha_used, switches);
    let tier1 = encode_stream(
        source,
        alpha_used,
        StreamSwitches {
            use_subtract_green: true,
            ..switches
        },
    );
    if tier1.len() < tier0.len() {
        tier1
    } else {
        tier0
    }
}

/// Byte length of the fixed VP8L stream header: the `0x2f` signature (8 bits),
/// two 14-bit dimension fields, the 1-bit alpha-used advisory and the 3-bit
/// version — exactly 40 bits, a whole 5 bytes. Because it is byte-aligned the bit
/// writer starts the transform list at byte 5, so a *headerless* level-0 stream is
/// byte-for-byte the tail of a normal stream past this prefix.
const VP8L_HEADER_BYTES: usize = 5;
const _: () = assert!(
    8 + 2 * VP8L_IMAGE_SIZE_BITS as usize + 1 + VP8L_VERSION_BITS as usize == VP8L_HEADER_BYTES * 8
);

/// Encode an alpha plane as a HEADERLESS level-0 VP8L stream — the lossless
/// (`method = 1`) body of an `ALPH` chunk, i.e. the bytes AFTER its 1-byte header.
///
/// Each alpha byte rides in the GREEN lane of a synthetic ARGB image; the other
/// three lanes are a single constant (`0`), so red/blue/alpha collapse to zero-bit
/// single-symbol codes and only the alpha (green) channel costs anything. The same
/// Tier 0/1 LZ77 + color-cache search as [`encode`] runs — the 5-byte header is a
/// constant prefix that never perturbs the candidate ranking — and dropping that
/// header yields exactly what [`crate::lossless::vp8l::decode::decode_alpha_stream`] reads
/// back: it drives the image-stream decoder directly and extracts `(argb >> 8)`.
///
/// Even if the search were to pick the subtract-green tier, the round-trip still
/// holds: subtract-green leaves the green lane untouched, so the recovered green is
/// the source alpha regardless.
#[must_use]
pub(crate) fn encode_alpha_stream(alpha: &[u8], width: u32, height: u32) -> Vec<u8> {
    let argb: Vec<u32> = alpha.iter().map(|&a| u32::from(a) << 8).collect();
    let mut full = encode_with(width, height, &argb, true, MAX_CHAIN_OPTIMAL, true);
    full.split_off(VP8L_HEADER_BYTES)
}

/// The native ARGB pixels and dimensions of one encode pass.
#[derive(Clone, Copy)]
struct PixelSource<'a> {
    width: u32,
    height: u32,
    argb: &'a [u32],
}

/// The tier switches that select which candidates [`encode_stream`] searches.
#[derive(Clone, Copy)]
struct StreamSwitches {
    use_subtract_green: bool,
    use_lz77: bool,
    chain_cap: u32,
    meta_huffman: bool,
}

/// Optimize and encode one tier. When `use_subtract_green` is set the pixels are
/// copied and transformed in place; otherwise the caller's slice is borrowed.
///
/// The pixels are parsed once per reference strategy; the cost model picks a
/// cache size for each; then up to three candidates are materialized and the
/// smallest kept. The `(literal, no-cache)` candidate is always first, so this
/// never regresses the plain literal baseline.
fn encode_stream(
    source: PixelSource<'_>,
    alpha_used: bool,
    switches: StreamSwitches,
) -> Vec<u8> {
    let PixelSource {
        width,
        height,
        argb,
    } = source;
    let StreamSwitches {
        use_subtract_green,
        use_lz77,
        chain_cap,
        meta_huffman,
    } = switches;
    // Borrow for Tier 0; own a transformed copy for Tier 1. `owned` is only
    // read on the branch that initializes it, so Tier 0 allocates nothing extra.
    let owned;
    let pixels: &[u32] = if use_subtract_green {
        let mut copy = argb.to_vec();
        subtract_green::forward(&mut copy);
        owned = copy;
        &owned
    } else {
        argb
    };

    // This tier's transform list: a lone subtract-green, or nothing. Both leave
    // the working width equal to the image width, so `header_width == model.width`.
    let sg = [TransformPlan::SubtractGreen];
    let plans: &[TransformPlan] = if use_subtract_green { &sg } else { &[] };

    if !use_lz77 {
        // Fast method: the literal, no-cache baseline only (per tier).
        let tokens_literal = parse(pixels, false);
        let model_literal = RefModel::new(&tokens_literal, pixels, width);
        return emit_stream(
            width,
            height,
            alpha_used,
            plans,
            0,
            &model_literal,
            &model_literal.histogram(0),
        );
    }
    // Full LZ77 + color-cache search; the pixels are already at the image width,
    // so the header and working widths coincide.
    if meta_huffman {
        search_lz77_variant(
            width,
            height,
            alpha_used,
            plans,
            width,
            pixels,
            true,
            true,
            true,
            false,
            chain_cap,
        )
        .1
    } else {
        search_lz77(width, height, alpha_used, plans, width, pixels, chain_cap)
    }
}

/// The RLE/previous-row token stream, offered only when its copies cover at
/// least 1% of the pixels — below that the model/cache/emit cost cannot pay
/// for itself (measured: photo content sits under 0.3%, run-heavy UI content
/// near 100%).
fn rle_candidate(pixels: &[u32], width: u32) -> Option<Vec<Token>> {
    let tokens = parse_rle(pixels, width);
    let copy_px: usize = tokens
        .iter()
        .map(|t| match t {
            Token::Copy { length, .. } => *length as usize,
            Token::Literal(_) => 0,
        })
        .sum();
    (copy_px * 100 >= pixels.len()).then_some(tokens)
}

/// Full LZ77 + color-cache candidate search over already-transformed `pixels`
/// laid out at `working_width` (which a color-indexing transform reduces below
/// `header_width`; every other family leaves them equal), returning the smallest
/// stream. `optimal_parse` adds the cost-model DP candidate; `meta_huffman` adds
/// the multi-group meta-Huffman shots — both breadth knobs, gated so a lower
/// effort level searches less. Parses the reference strategies, lets the cost
/// model pick a cache size for each, and keeps the smallest. The
/// `(literal, no-cache)` candidate is always first, so this never regresses the
/// plain literal baseline for the given `plans`. Returns `(plain, meta)`:
/// `plain` folds only the non-meta-Huffman candidates — exactly what the same
/// call with `meta_huffman: false` returns — while `meta` additionally folds
/// the meta-Huffman shots, both out of one shared parse/DP/search pass.
#[expect(
    clippy::too_many_arguments,
    reason = "threads the emit context (dimensions, plans, working width) and the \
              breadth gates through unchanged; bundling it would only hide the \
              data flow the named wrappers rely on"
)]
fn search_lz77_variant(
    header_width: u32,
    header_height: u32,
    alpha_used: bool,
    plans: &[TransformPlan],
    working_width: u32,
    pixels: &[u32],
    optimal_parse: bool,
    meta_huffman: bool,
    fine_meta: bool,
    reparse: bool,
    chain_cap: u32,
) -> (Vec<u8>, Vec<u8>) {
    let emit = |cache_bits, model: &RefModel<'_>, histogram: &Histogram| {
        emit_stream(
            header_width,
            header_height,
            alpha_used,
            plans,
            cache_bits,
            model,
            histogram,
        )
    };
    // Baseline: all literals, with and without a cache — the non-regression floor.
    // The cache search hands back the winning size's histogram, reused by that
    // candidate's emit (the @0 emit still builds its own, since the winner may be
    // > 0). `plain` and `best` fold the same non-meta candidates in the same
    // order; `best` additionally folds the meta-Huffman shots, so the pair
    // equals the `meta_huffman: false` and `true` results of one shared pass.
    let literal_best = {
        let tokens_literal = parse(pixels, false);
        let model_literal = RefModel::new(&tokens_literal, pixels, working_width);
        let (cache_literal, hist_literal) = best_cache_bits(&model_literal);
        let mut best = emit(0, &model_literal, &model_literal.histogram(0));
        if cache_literal > 0 {
            keep_smaller(
                &mut best,
                emit(cache_literal, &model_literal, &hist_literal),
            );
        }
        best
    };
    let mut plain = literal_best.clone();
    let mut best = literal_best;
    // The greedy LZ77 stream. Kept alive as the DP seed and the meta-Huffman input;
    // its hash chain is carried alongside so the DP reuses it instead of rebuilding
    // the same pure-function-of-pixels chain (byte-identical either way).
    let (tokens_lz77, chain) = parse_lz77(pixels);
    let model_lz77 = RefModel::new(&tokens_lz77, pixels, working_width);
    let (cache_lz77, hist_lz77) = best_cache_bits(&model_lz77);
    let lz77_stream = emit(cache_lz77, &model_lz77, &hist_lz77);
    keep_smaller(&mut plain, lz77_stream.clone());
    keep_smaller(&mut best, lz77_stream);
    // libwebp's third candidate family: run-length/previous-row copies only.
    // Cheap near distances beat longer far ones on run-heavy content; the
    // stream is self-floored against the candidates already folded in.
    let ysize = pixels.len() as u32 / working_width;
    if let Some(tokens_rle) = rle_candidate(pixels, working_width) {
        let model_rle = RefModel::new(&tokens_rle, pixels, working_width);
        let (cache_rle, hist_rle) = best_cache_bits(&model_rle);
        let rle_stream = emit(cache_rle, &model_rle, &hist_rle);
        keep_smaller(&mut plain, rle_stream.clone());
        keep_smaller(&mut best, rle_stream);
        // cwebp -z9's camera stream is exactly this shape: near-distance copies
        // under a multi-group meta-Huffman plan. The plain RLE stream loses to
        // the DP stream, but its token histogram is what meta grouping exploits
        // — the shot is self-floored, so it only pays where it wins.
        if meta_huffman {
            meta_shot(
                &mut best,
                header_width,
                header_height,
                alpha_used,
                plans,
                cache_rle,
                &model_rle,
                fine_meta,
                pixels,
                working_width,
                ysize,
            );
        }
    }
    // Cost-model-driven optimal parse: a self-floored extra candidate (chosen only
    // when strictly smaller by real bytes), seeded from the greedy stream. Large
    // images open the deeper staircase walk at every effort level.
    if optimal_parse {
        let optimal_chain_cap = effective_chain_cap(chain_cap, pixels.len());
        let tokens_dp = parse_optimal(pixels, working_width, &tokens_lz77, &chain, optimal_chain_cap);
        let shallow_dp = (chain_cap == MAX_CHAIN_OPTIMAL && pixels.len() < DEEP_CHAIN_MIN_PIXELS)
            .then(|| parse_optimal(pixels, working_width, &tokens_lz77, &chain, MAX_CHAIN));
        // Re-parse against the histogram the DP stream itself implies: the DP
        // prices matches under the greedy stream's distribution, which
        // systematically misprices the matches an optimal parse prefers.
        // Iterating to a token fixed point recovers most of the gap (each pass
        // is one more DP walk); intermediate streams are discarded un-emitted —
        // only the converged stream and the first-pass stream are candidates.
        let reparsed = if reparse && tokens_dp != tokens_lz77 {
            let mut prev = tokens_dp.clone();
            for _ in 0..REPARSE_MAX_PASSES {
                let next = parse_optimal(pixels, working_width, &prev, &chain, optimal_chain_cap);
                if next == prev {
                    break;
                }
                prev = next;
            }
            (prev != tokens_dp).then_some(prev)
        } else {
            None
        };
        drop(chain);
        let model_dp = RefModel::new(&tokens_dp, pixels, working_width);
        let (cache_dp, hist_dp) = best_cache_bits(&model_dp);
        let dp_stream = emit(cache_dp, &model_dp, &hist_dp);
        keep_smaller(&mut plain, dp_stream.clone());
        keep_smaller(&mut best, dp_stream);
        if let Some(tokens_re) = reparsed {
            let model_re = RefModel::new(&tokens_re, pixels, working_width);
            let (cache_re, hist_re) = best_cache_bits(&model_re);
            let re_stream = emit(cache_re, &model_re, &hist_re);
            keep_smaller(&mut plain, re_stream.clone());
            keep_smaller(&mut best, re_stream);
            if meta_huffman {
                meta_shot(
                    &mut best,
                    header_width,
                    header_height,
                    alpha_used,
                    plans,
                    cache_re,
                    &model_re,
                    fine_meta,
                    pixels,
                    working_width,
                    ysize,
                );
            }
        }
        if let Some(tokens_shallow) = shallow_dp {
            let model_shallow = RefModel::new(&tokens_shallow, pixels, working_width);
            let (cache_shallow, hist_shallow) = best_cache_bits(&model_shallow);
            let shallow_stream = emit(cache_shallow, &model_shallow, &hist_shallow);
            keep_smaller(&mut plain, shallow_stream.clone());
            keep_smaller(&mut best, shallow_stream);
            if meta_huffman {
                meta_shot(
                    &mut best,
                    header_width,
                    header_height,
                    alpha_used,
                    plans,
                    cache_shallow,
                    &model_shallow,
                    false,
                    pixels,
                    working_width,
                    ysize,
                );
            }
        }
        if meta_huffman {
            // A meta-Huffman shot on each of the greedy and DP streams — both
            // self-floored against the single-group candidates already folded in.
            // The finer entropy grid is offered to the DP shot only: it is the
            // stream that almost always wins, and the ~1000-block merge is the
            // expensive part of meta planning.
            meta_shot(
                &mut best,
                header_width,
                header_height,
                alpha_used,
                plans,
                cache_lz77,
                &model_lz77,
                false,
                pixels,
                working_width,
                ysize,
            );
            meta_shot(
                &mut best,
                header_width,
                header_height,
                alpha_used,
                plans,
                cache_dp,
                &model_dp,
                fine_meta,
                pixels,
                working_width,
                ysize,
            );
        }
    } else if meta_huffman {
        // No DP candidate at this breadth: the meta-Huffman shot rides the greedy
        // stream alone.
        meta_shot(
            &mut best,
            header_width,
            header_height,
            alpha_used,
            plans,
            cache_lz77,
            &model_lz77,
            fine_meta,
            pixels,
            working_width,
            ysize,
        );
    }
    (plain, best)
}

/// Fold a meta-Huffman candidate for `model` into `best` when [`meta::plan`] finds
/// a beneficial grouping — self-floored, so it only wins by real emitted bytes.
#[expect(
    clippy::too_many_arguments,
    reason = "threads the emit context (dimensions, plans, cache) and the model \
              through unchanged; bundling it would only hide the data flow"
)]
fn meta_shot(
    best: &mut Vec<u8>,
    header_width: u32,
    header_height: u32,
    alpha_used: bool,
    plans: &[TransformPlan],
    cache_bits: u32,
    model: &RefModel<'_>,
    finer: bool,
    pixels: &[u32],
    working_width: u32,
    ysize: u32,
) {
    let mut candidates = meta::bits_candidates(working_width, ysize, finer);
    let base_len = meta::bits_candidates(working_width, ysize, false).len();
    debug_assert!(candidates.len() == base_len || candidates.len() == base_len + 1);
    if candidates.len() == base_len + 1 {
        let finer_bits = candidates.remove(0);
        if let Some(plan) =
            meta::plan_at_bits(model.tokens, pixels, working_width, ysize, cache_bits, finer_bits)
        {
            keep_smaller(
                best,
                emit_stream_meta(
                    header_width,
                    header_height,
                    alpha_used,
                    plans,
                    cache_bits,
                    model,
                    &plan,
                ),
            );
        }
    }
    let mut candidates = candidates.into_iter();
    let Some(bits) = candidates.next() else { return };
    let Some(plan) =
        meta::plan_at_bits(model.tokens, pixels, working_width, ysize, cache_bits, bits)
    else {
        return;
    };
    keep_smaller(
        best,
        emit_stream_meta(
            header_width,
            header_height,
            alpha_used,
            plans,
            cache_bits,
            model,
            &plan,
        ),
    );
    // Coarser grids pay only when the fine grid's statistics already collapsed
    // into few groups — then a bigger block shrinks the entropy image without
    // losing the split. Photos keep many groups and skip the extra emits.
    let blocks = (plan.entropy_xsize * subsample_size(ysize, plan.bits)) as usize;
    if plan.group_histograms.len() * 8 > blocks {
        return;
    }
    for bits in candidates {
        if let Some(plan) =
            meta::plan_at_bits(model.tokens, pixels, working_width, ysize, cache_bits, bits)
        {
            keep_smaller(
                best,
                emit_stream_meta(
                    header_width,
                    header_height,
                    alpha_used,
                    plans,
                    cache_bits,
                    model,
                    &plan,
                ),
            );
        }
    }
}

/// The Balanced search: LZ77 + color cache + optimal parse, no meta-Huffman. The
/// transform-free form is the [`encode`] floor; the byte-identical wrapper keeps
/// the historical name for the direct cost-model tests.
fn search_lz77(
    header_width: u32,
    header_height: u32,
    alpha_used: bool,
    plans: &[TransformPlan],
    working_width: u32,
    pixels: &[u32],
    chain_cap: u32,
) -> Vec<u8> {
    search_lz77_variant(
        header_width,
        header_height,
        alpha_used,
        plans,
        working_width,
        pixels,
        true,
        false,
        false,
        false,
        chain_cap,
    )
    .0
}

/// The full-breadth search: [`search_lz77`] plus the meta-Huffman shots (the
/// historical `search_lz77_best`). Production families call [`family_search`]
/// with a [`Breadth`]; this named form backs the direct cost-model tests.
#[cfg(test)]
fn search_lz77_best(
    header_width: u32,
    header_height: u32,
    alpha_used: bool,
    plans: &[TransformPlan],
    working_width: u32,
    pixels: &[u32],
) -> Vec<u8> {
    search_lz77_variant(
        header_width,
        header_height,
        alpha_used,
        plans,
        working_width,
        pixels,
        true,
        true,
        false,
        false,
        MAX_CHAIN_OPTIMAL,
    )
    .1
}

/// Tile-bits candidates for the predictor/cross-color families. [`keep_smaller`]
/// ranks by REAL emitted bytes, so widening this array can only shrink the result.
const TRANSFORM_TILE_BITS_SWEEP: [u32; 4] = [2, 3, 4, 5];
/// Extra optimal-parse passes seeded by the previous pass's token stream when
/// [`Breadth::reparse`] is on. Four passes capture the steep part of the
/// convergence curve; the fixed-point check normally stops earlier.
const REPARSE_MAX_PASSES: usize = 4;
/// Extended grid sweep tried only on small images (<= [`SMALL_TILE_SWEEP_MAX_PIXELS`]).
/// The fine grids capture the sub-megapixel synthetics' gradient structure (the
/// transparent.png lossless candidate dropped 128 -> 110 B), while for photographic
/// or large fixtures the extra four grids double the planning wall with no byte win.
const TRANSFORM_TILE_BITS_SMALL: [u32; 8] = [2, 3, 4, 5, 6, 7, 8, 9];
const SMALL_TILE_SWEEP_MAX_PIXELS: u64 = 16_384;
const _: () = {
    let mut i = 0;
    while i < TRANSFORM_TILE_BITS_SWEEP.len() {
        assert!(
            TRANSFORM_TILE_BITS_SWEEP[i] >= MIN_TRANSFORM_BITS
                && TRANSFORM_TILE_BITS_SWEEP[i] < MIN_TRANSFORM_BITS + (1 << NUM_TRANSFORM_BITS)
        );
        i += 1;
    }
};

/// How wide the Tier 3 search casts its net at one effort level: which tile sizes
/// the predictor/cross-color families sweep, whether cross-color runs at all, and
/// whether the per-family search adds the optimal-parse and meta-Huffman shots.
/// Produced by [`schedule`]; every field is monotone in the level, so a higher
/// level only ever *adds* candidates.
#[derive(Clone, Copy)]
struct Breadth {
    /// A prefix of [`TRANSFORM_TILE_BITS_SWEEP`] (nested across levels) the
    /// predictor and cross-color families sweep.
    tile_bits: &'static [u32],
    /// Whether the cross-color family is evaluated.
    cross_color: bool,
    /// Whether each family search adds the cost-model optimal-parse candidate.
    optimal_parse: bool,
    /// Whether each family search adds the meta-Huffman shots.
    meta_huffman: bool,
    /// Whether the DP-token meta shot also tries one entropy-grid step finer
    /// than [`meta::choose_bits`] — where libwebp's picks land on photos. The
    /// ~1000-block merge is quadratic, so this stays a top-level-only knob.
    fine_meta: bool,
    /// Whether the optimal parse re-runs against the histogram its own first
    /// pass implies, iterated to a fixed point — the cost model the DP prices
    /// is the greedy stream's, which misprices the matches the DP itself
    /// prefers. Each extra pass is one more DP walk, so this stays a deep-level
    /// knob.
    reparse: bool,
    /// Hop cap for the optimal-parse DP's staircase walk. Small images at low
    /// levels keep [`MAX_CHAIN`] so their wall stays flat; effort level ≥ 3 or a
    /// large image opens [`MAX_CHAIN_OPTIMAL`] (see [`search_lz77_variant`]).
    chain_cap: u32,
}

/// Map an effort level (`0..=9`) to its [`Breadth`]. Monotone: each higher level's
/// candidate set is a superset of the one below (wider tile sweep, then
/// cross-color, then meta-Huffman), so the real-byte winner never grows with the
/// level. Level 9 enables the full search — byte-for-byte the historical
/// `encode_best` — pinned by `level_nine_is_full_breadth`.
/// Level-to-breadth map for a specific image: widens the full tile-bit sweep on
/// small images only. The pure [`schedule`] tier logic is unchanged; only the
/// tile-bits set widens for tiny inputs, where the extra four grids cost
/// sub-millisecond planning.
fn schedule_for(level: u8, pixels: u64) -> Breadth {
    let breadth = schedule(level);
    if pixels > SMALL_TILE_SWEEP_MAX_PIXELS
        || breadth.tile_bits.len() != TRANSFORM_TILE_BITS_SWEEP.len()
    {
        return breadth;
    }
    Breadth {
        tile_bits: &TRANSFORM_TILE_BITS_SMALL,
        ..breadth
    }
}

const fn schedule(level: u8) -> Breadth {
    const T4: &[u32] = &[4];
    const T34: &[u32] = &[3, 4];
    const T234: &[u32] = &[2, 3, 4];
    const FULL: &[u32] = &TRANSFORM_TILE_BITS_SWEEP;
    let chain_cap = if level >= 3 { MAX_CHAIN_OPTIMAL } else { MAX_CHAIN };
    match level {
        0 => Breadth {
            tile_bits: T4,
            cross_color: false,
            optimal_parse: false,
            meta_huffman: false,
            fine_meta: false,
            reparse: false,
            chain_cap,
        },
        1 => Breadth {
            tile_bits: T4,
            cross_color: true,
            optimal_parse: true,
            meta_huffman: true,
            fine_meta: false,
            reparse: false,
            chain_cap,
        },
        2 => Breadth {
            tile_bits: T34,
            cross_color: true,
            optimal_parse: true,
            meta_huffman: true,
            fine_meta: false,
            reparse: false,
            chain_cap,
        },
        3 => Breadth {
            tile_bits: T234,
            cross_color: true,
            optimal_parse: true,
            meta_huffman: true,
            fine_meta: false,
            reparse: false,
            chain_cap,
        },
        // Level 6 is the product Fast-effort band for sub-400K-pixel images —
        // the fine meta grid's quadratic merge would double its wall for ~1%
        // bytes, so the finer precision waits for the explicit deep levels.
        4..=6 => Breadth {
            tile_bits: FULL,
            cross_color: true,
            optimal_parse: true,
            meta_huffman: true,
            fine_meta: false,
            reparse: false,
            chain_cap,
        },
        _ => Breadth {
            tile_bits: FULL,
            cross_color: true,
            optimal_parse: true,
            meta_huffman: true,
            fine_meta: true,
            reparse: true,
            chain_cap,
        },
    }
}

/// One independent `Effort` candidate family. Evaluated in the fixed order
/// [`build_best_tasks`] emits, so [`reduce_task_minima`] is deterministic and the
/// serial and rayon evaluators agree byte-for-byte.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum BestTask {
    Floor,
    Palette,
    Predictor(u32),
    CrossColor(u32),
    PredictorCross(u32, u32),
}

/// The canonical candidate task order for `breadth`: floor, palette, then predictor
/// (and, when enabled, cross-color) once per swept tile size.
fn build_best_tasks(breadth: Breadth) -> Vec<BestTask> {
    let mut tasks = vec![BestTask::Floor, BestTask::Palette];
    tasks.extend(breadth.tile_bits.iter().map(|&b| BestTask::Predictor(b)));
    if breadth.cross_color {
        tasks.extend(breadth.tile_bits.iter().map(|&b| BestTask::CrossColor(b)));
        // Predictor+cross-color compositions over cc in {p-1, p, p+1, p+2} —
        // cwebp's own picks span both directions (coffee p=3/cc=4, astronaut
        // p=2/cc=4); each task emits both the SG-first and no-SG compositions.
        // Measured: dropping p-1 regresses coffee ~800 B; dropping p+1/p+2
        // regresses astronaut ~9 KB. They must stay at every cross-color level —
        // gating them to the meta breadth regressed coffee L1 by ~19 KB (the
        // optimal-parse-only composed stream wins there without meta-Huffman).
        for &p in breadth.tile_bits {
            for d in -1i32..=2 {
                let cc = p as i32 + d;
                if (MIN_TRANSFORM_BITS as i32..=(MIN_TRANSFORM_BITS + 7) as i32).contains(&cc) {
                    tasks.push(BestTask::PredictorCross(p, cc as u32));
                }
            }
        }
    }
    tasks
}

/// One transformed pixel stream awaiting its [`family_search`]: the transform
/// plans to transmit and the working-width pixel buffer they produce. Specs are
/// breadth-independent — the effort level only chooses which specs get searched.
struct BufferSpec {
    plans: Vec<TransformPlan>,
    working_width: u32,
    pixels: Vec<u32>,
}

/// Evaluate one task into its ordered candidate streams at `breadth`. The four
/// `*_specs` helpers hold the family transform logic once; the serial and rayon
/// evaluators share them through [`run_best_task`].
fn run_best_task(
    task: BestTask,
    breadth: Breadth,
    source: PixelSource<'_>,
    alpha_used: bool,
    planes: Option<&PlaneCache>,
    cc: Option<&CrossColorCache>,
    spec_mask: &[bool],
) -> Vec<Vec<u8>> {
    let PixelSource {
        width,
        height,
        argb,
    } = source;
    if matches!(task, BestTask::Floor) {
        return floor_streams(breadth, width, height, alpha_used, argb);
    }
    task_specs(task, width, height, argb, planes, cc, false)
        .into_iter()
        .zip(spec_mask.iter().copied().chain(core::iter::repeat(true)))
        .filter(|(_, keep)| *keep)
        .map(|(spec, _)| {
            family_search(
                breadth,
                width,
                height,
                alpha_used,
                &spec.plans,
                spec.working_width,
                &spec.pixels,
            )
        })
        .collect()
}

/// The transformed streams one task would search, in emission order. `Floor`
/// has no specs — it is the [`encode_with`] floor evaluated unconditionally.
fn task_specs(
    task: BestTask,
    width: u32,
    height: u32,
    argb: &[u32],
    planes: Option<&PlaneCache>,
    cc: Option<&CrossColorCache>,
    cheap_cc: bool,
) -> Vec<BufferSpec> {
    match task {
        BestTask::Floor => Vec::new(),
        BestTask::Palette => palette_specs(width, argb),
        BestTask::Predictor(bits) => predictor_specs(width, height, argb, bits, planes),
        BestTask::CrossColor(bits) => {
            cross_color_specs(width, height, argb, bits, cc, cheap_cc)
        },
        BestTask::PredictorCross(p_bits, cc_bits) => predictor_cross_specs(
            PixelSource {
                width,
                height,
                argb,
            },
            p_bits,
            cc_bits,
            planes,
            cc,
            cheap_cc,
        ),
    }
}

/// The optimal-parse DP's staircase hop cap for an encode of `pixels` at a
/// breadth that hands over `breadth_cap`: large images always get the deep walk.
fn effective_chain_cap(breadth_cap: u32, pixels: usize) -> u32 {
    if pixels >= DEEP_CHAIN_MIN_PIXELS {
        MAX_CHAIN_OPTIMAL
    } else {
        breadth_cap
    }
}

/// Order-0 entropy (bits) of the four byte planes of a transformed pixel
/// stream — the cheap pre-score that ranks transform candidates before any
/// LZ77/DP work runs. Fixed-point via [`shannon_bits`], so the score is an
/// exact integer identical on every target and `no_std` build.
fn stream_entropy(pixels: &[u32]) -> u64 {
    let mut counts = [[0u32; 256]; 4];
    for &p in pixels {
        for (i, b) in p.to_le_bytes().iter().enumerate() {
            counts[i][*b as usize] += 1;
        }
    }
    let total = pixels.len() as u64;
    counts.iter().map(|plane| shannon_bits(total, plane)).sum()
}

/// Run one transform family's search at `breadth`: adds the optimal-parse and
/// meta-Huffman candidates only when the level enables them.
fn family_search(
    breadth: Breadth,
    header_width: u32,
    header_height: u32,
    alpha_used: bool,
    plans: &[TransformPlan],
    working_width: u32,
    pixels: &[u32],
) -> Vec<u8> {
    search_lz77_variant(
        header_width,
        header_height,
        alpha_used,
        plans,
        working_width,
        pixels,
        breadth.optimal_parse,
        breadth.meta_huffman,
        breadth.fine_meta,
        breadth.reparse,
        breadth.chain_cap,
    )
    .1
}

/// The transform-free floor (always first, so it seeds `best` and wins ties). The
/// floor itself is the full [`encode`] search, so it is present at every breadth;
/// the two no-transform meta-Huffman shots are added only when the level enables
/// them (otherwise they are subsumed by the floor).
fn floor_streams(
    breadth: Breadth,
    width: u32,
    height: u32,
    alpha_used: bool,
    argb: &[u32],
) -> Vec<Vec<u8>> {
    // Each representation is searched once: the floor tiers always run the
    // optimal parse ([`search_lz77`]), and the meta-enabled breadth folds the
    // meta-Huffman shots into the same pass, so the plain floor streams and the
    // meta-augmented streams come out of a single parse/DP/search per buffer.
    debug_assert!(breadth.optimal_parse || !breadth.meta_huffman);
    let chain_cap = effective_chain_cap(breadth.chain_cap, argb.len());
    let floor_pair = |plans: &[TransformPlan], pixels: &[u32]| {
        search_lz77_variant(
            width,
            height,
            alpha_used,
            plans,
            width,
            pixels,
            true,
            breadth.meta_huffman,
            breadth.fine_meta,
            breadth.reparse,
            chain_cap,
        )
    };
    // The raw and subtract-green halves are independent full searches; running
    // them together keeps the floor task from setting the critical path when
    // the caller is already inside the rayon task pool. Results are identical —
    // only the scheduling differs.
    let mut sg = argb.to_vec();
    subtract_green::forward(&mut sg);
    let sg_plans = [TransformPlan::SubtractGreen];
    #[cfg(feature = "rayon")]
    let ((raw_plain, raw_meta), (sg_plain, sg_meta)) =
        rayon::join(|| floor_pair(&[], argb), || floor_pair(&sg_plans, &sg));
    #[cfg(not(feature = "rayon"))]
    let ((raw_plain, raw_meta), (sg_plain, sg_meta)) =
        (floor_pair(&[], argb), floor_pair(&sg_plans, &sg));
    // [`encode_with`] keeps Tier 0 unless Tier 1 is strictly smaller.
    let floor = if sg_plain.len() < raw_plain.len() {
        sg_plain
    } else {
        raw_plain
    };
    if !breadth.meta_huffman {
        return vec![floor];
    }
    vec![floor, raw_meta, sg_meta]
}

/// The palette family (empty when the image has > 256 distinct colors).
fn palette_specs(width: u32, argb: &[u32]) -> Vec<BufferSpec> {
    if let Some(p) = palette::forward(argb, width) {
        vec![BufferSpec {
            plans: vec![TransformPlan::ColorIndexing {
                num_colors: p.num_colors,
                colormap: p.colormap,
            }],
            working_width: subsample_size(width, p.bits),
            pixels: p.bundled,
        }]
    } else {
        Vec::new()
    }
}

/// Predictor-forward results shared across best-tasks. `Predictor(b)` and every
/// `PredictorCross(b, _)` composition need `predictor::forward(argb, b)`, and the
/// composed legs need `predictor::forward(subtract_green(argb), b)`; at full
/// breadth those forwards run 5x/4x redundantly per tile size. The cache computes
/// each `(source, bits)` pair once and the tasks borrow the planes. `None` when
/// the planes would exceed [`PLANE_CACHE_MAX_PIXELS`]; callers then compute
/// inline exactly as before.
struct PlaneCache {
    /// `(bits, residual, tile_data)` for `predictor::forward(argb, bits)`.
    raw: Vec<(u32, Vec<u32>, Vec<u32>)>,
    /// Same for `predictor::forward(subtract_green(argb), bits)`; empty unless
    /// the breadth composes cross-color.
    sg_pred: Vec<(u32, Vec<u32>, Vec<u32>)>,
}

/// Bound on cached predictor planes (in pixels) so huge images keep the old
/// compute-per-task memory profile instead of holding `2 * tile_bits` planes.
const PLANE_CACHE_MAX_PIXELS: usize = 1 << 26;

fn forward_all(
    source: &[u32],
    width: u32,
    height: u32,
    tile_bits: &[u32],
) -> Vec<(u32, Vec<u32>, Vec<u32>)> {
    #[cfg(feature = "rayon")]
    return {
        use rayon::prelude::*;
        tile_bits
            .par_iter()
            .map(|&b| {
                let (residual, tile_data) = predictor::forward(source, width, height, b);
                (b, residual, tile_data)
            })
            .collect()
    };
    #[cfg(not(feature = "rayon"))]
    return tile_bits
        .iter()
        .map(|&b| {
            let (residual, tile_data) = predictor::forward(source, width, height, b);
            (b, residual, tile_data)
        })
        .collect();
}

impl PlaneCache {
    fn build(breadth: Breadth, width: u32, height: u32, argb: &[u32]) -> Option<Self> {
        let planes = breadth
            .tile_bits
            .len()
            .saturating_mul(if breadth.cross_color { 2 } else { 1 });
        if argb.len().saturating_mul(planes) > PLANE_CACHE_MAX_PIXELS {
            return None;
        }
        let sg_px = breadth.cross_color.then(|| {
            let mut px = argb.to_vec();
            subtract_green::forward(&mut px);
            px
        });
        let raw = forward_all(argb, width, height, breadth.tile_bits);
        let sg_pred = match &sg_px {
            Some(px) => forward_all(px, width, height, breadth.tile_bits),
            None => Vec::new(),
        };
        Some(Self { raw, sg_pred })
    }

    fn raw(&self, bits: u32) -> Option<&(u32, Vec<u32>, Vec<u32>)> {
        self.raw.iter().find(|e| e.0 == bits)
    }

    fn sg_pred(&self, bits: u32) -> Option<&(u32, Vec<u32>, Vec<u32>)> {
        self.sg_pred.iter().find(|e| e.0 == bits)
    }
}

/// One cached cross-color forward: the transformed pixels and their coverage.
type CrossColorForward = (Vec<u32>, Vec<u32>);
/// Cached cross-color forwards, keyed by `(forward kind, axis bits, tile bits)`.
type CrossColorMap = alloc::collections::BTreeMap<(u8, u32, u32), CrossColorForward>;

/// Cross-color forward results shared across the entropy pre-score and the
/// kept-task search, mirroring [`PlaneCache`]: every spec production needs
/// `cross_color::forward` outputs, and that forward dominates transform cost
/// on photo-like content. Entries compute lazily on first use inside the
/// parallel estimate pass — no barrier — and the kept-task search clones
/// them. `None` when the distinct forwards would exceed the pixel bound;
/// callers then compute inline exactly as before.
struct CrossColorCache {
    #[cfg(feature = "std")]
    map: std::sync::Mutex<CrossColorMap>,
    #[cfg(not(feature = "std"))]
    map: core::cell::RefCell<CrossColorMap>,
}

const CROSS_COLOR_CACHE_MAX_PIXELS: usize = 1 << 26;

impl CrossColorCache {
    fn new(tasks: &[BestTask], pixels: usize) -> Option<Self> {
        let mut jobs: Vec<(u8, u32, u32)> = Vec::new();
        for task in tasks {
            match task {
                BestTask::CrossColor(bits) => jobs.push((0, 0, *bits)),
                BestTask::PredictorCross(p_bits, cc_bits) => {
                    jobs.push((1, *p_bits, *cc_bits));
                    jobs.push((2, *p_bits, *cc_bits));
                },
                _ => {},
            }
        }
        jobs.sort_unstable();
        jobs.dedup();
        if jobs.is_empty() || jobs.len().saturating_mul(pixels) > CROSS_COLOR_CACHE_MAX_PIXELS {
            return None;
        }
        #[cfg(feature = "std")]
        let map = std::sync::Mutex::new(alloc::collections::BTreeMap::new());
        #[cfg(not(feature = "std"))]
        let map = core::cell::RefCell::new(alloc::collections::BTreeMap::new());
        Some(Self { map })
    }

    fn forward(
        &self,
        kind: u8,
        a: u32,
        bits: u32,
        input: &[u32],
        width: u32,
        height: u32,
    ) -> (Vec<u32>, Vec<u32>) {
        let key = (kind, a, bits);
        #[cfg(feature = "std")]
        {
            {
                let map = self
                    .map
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Some(hit) = map.get(&key) {
                    return (hit.0.clone(), hit.1.clone());
                }
            }
            let out = cross_color::forward(input, width, height, bits);
            self.map
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(key, (out.0.clone(), out.1.clone()));
            out
        }
        #[cfg(not(feature = "std"))]
        {
            let mut map = self.map.borrow_mut();
            if let Some(hit) = map.get(&key) {
                return (hit.0.clone(), hit.1.clone());
            }
            let out = cross_color::forward(input, width, height, bits);
            map.insert(key, (out.0.clone(), out.1.clone()));
            out
        }
    }
}

fn cc_forward(
    cc: Option<&CrossColorCache>,
    kind: u8,
    a: u32,
    bits: u32,
    input: &[u32],
    width: u32,
    height: u32,
) -> (Vec<u32>, Vec<u32>) {
    match cc {
        Some(c) => c.forward(kind, a, bits, input, width, height),
        None => cross_color::forward(input, width, height, bits),
    }
}

/// Borrow the cached `(residual, tile_data)` for `bits`, or the freshly computed
/// `owned` fallback. Exactly one of `hit`/`owned` is `Some`.
fn plane<'a>(
    hit: Option<&'a (u32, Vec<u32>, Vec<u32>)>,
    owned: &'a Option<(Vec<u32>, Vec<u32>)>,
) -> (&'a [u32], &'a [u32]) {
    match (hit, owned) {
        (Some((_, px, td)), _) => (px.as_slice(), td.as_slice()),
        (None, Some(o)) => (o.0.as_slice(), o.1.as_slice()),
        (None, None) => unreachable!("predictor plane neither cached nor computed"),
    }
}

/// The predictor family at one tile size: predictor alone, then predictor +
/// subtract-green (spatial before pointwise).
fn predictor_specs(
    width: u32,
    height: u32,
    argb: &[u32],
    bits: u32,
    planes: Option<&PlaneCache>,
) -> Vec<BufferSpec> {
    let hit = planes.and_then(|p| p.raw(bits));
    let owned = hit
        .is_none()
        .then(|| predictor::forward(argb, width, height, bits));
    let (residual, tile_data) = plane(hit, &owned);
    let mut residual_sg = residual.to_vec();
    subtract_green::forward(&mut residual_sg);
    vec![
        BufferSpec {
            plans: vec![TransformPlan::Predictor {
                bits,
                tile_data: tile_data.to_vec(),
            }],
            working_width: width,
            pixels: residual.to_vec(),
        },
        BufferSpec {
            plans: vec![
                TransformPlan::Predictor {
                    bits,
                    tile_data: tile_data.to_vec(),
                },
                TransformPlan::SubtractGreen,
            ],
            working_width: width,
            pixels: residual_sg,
        },
    ]
}

/// The predictor+cross-color composition at one `(predictor, cross-color)` tile
/// pair — the combination libwebp's `-lossless` stacks (predictor residuals feed
/// the cross-color multiplier search; standalone cross-color cannot compete with
/// predictor+subtract-green on photographic content). Emitted in libwebp's
/// application order — subtract-green, then predictor, then cross-color — and
/// self-floored by real bytes like every other candidate.
fn predictor_cross_specs(
    source: PixelSource<'_>,
    p_bits: u32,
    cc_bits: u32,
    planes: Option<&PlaneCache>,
    cc: Option<&CrossColorCache>,
    cheap_cc: bool,
) -> Vec<BufferSpec> {
    let PixelSource {
        width,
        height,
        argb,
    } = source;
    // libwebp's order: subtract-green first, then predictor, then cross-color.
    let sg_hit = planes.and_then(|p| p.sg_pred(p_bits));
    let sg_owned = sg_hit.is_none().then(|| {
        let mut sg_px = argb.to_vec();
        subtract_green::forward(&mut sg_px);
        predictor::forward(&sg_px, width, height, p_bits)
    });
    let (pres, ptile) = plane(sg_hit, &sg_owned);
    let (stored, ctile) = if cheap_cc {
        cross_color::forward_estimate(pres, width, height, cc_bits)
    } else {
        cc_forward(cc, 2, p_bits, cc_bits, pres, width, height)
    };
    let with_sg = BufferSpec {
        plans: vec![
            TransformPlan::SubtractGreen,
            TransformPlan::Predictor {
                bits: p_bits,
                tile_data: ptile.to_vec(),
            },
            TransformPlan::CrossColor {
                bits: cc_bits,
                tile_data: ctile,
            },
        ],
        working_width: width,
        pixels: stored,
    };
    // The same composition without subtract-green — cwebp picks [P, CC] on
    // astronaut.png where SG hurts rather than helps.
    let raw_hit = planes.and_then(|p| p.raw(p_bits));
    let raw_owned = raw_hit
        .is_none()
        .then(|| predictor::forward(argb, width, height, p_bits));
    let (pres3, ptile3) = plane(raw_hit, &raw_owned);
    let (stored3, ctile3) = if cheap_cc {
        cross_color::forward_estimate(pres3, width, height, cc_bits)
    } else {
        cc_forward(cc, 1, p_bits, cc_bits, pres3, width, height)
    };
    let no_sg = BufferSpec {
        plans: vec![
            TransformPlan::Predictor {
                bits: p_bits,
                tile_data: ptile3.to_vec(),
            },
            TransformPlan::CrossColor {
                bits: cc_bits,
                tile_data: ctile3,
            },
        ],
        working_width: width,
        pixels: stored3,
    };
    vec![with_sg, no_sg]
}

/// The cross-color family at one tile size: cross-color alone, then cross-color +
/// subtract-green.
fn cross_color_specs(
    width: u32,
    height: u32,
    argb: &[u32],
    bits: u32,
    cc: Option<&CrossColorCache>,
    cheap_cc: bool,
) -> Vec<BufferSpec> {
    let (stored, tile_data) = if cheap_cc {
        cross_color::forward_estimate(argb, width, height, bits)
    } else {
        cc_forward(cc, 0, 0, bits, argb, width, height)
    };
    let mut stored_sg = stored.clone();
    subtract_green::forward(&mut stored_sg);
    vec![
        BufferSpec {
            plans: vec![TransformPlan::CrossColor {
                bits,
                tile_data: tile_data.clone(),
            }],
            working_width: width,
            pixels: stored,
        },
        BufferSpec {
            plans: vec![
                TransformPlan::CrossColor { bits, tile_data },
                TransformPlan::SubtractGreen,
            ],
            working_width: width,
            pixels: stored_sg,
        },
    ]
}

/// Fold one family's ordered candidate streams into that family's minimum by REAL
/// emitted bytes (tie -> the earliest stream), or `None` for an empty family (only
/// palette, when the image exceeds 256 colors). This is [`keep_smaller`] applied
/// left-to-right, so folding per-family minima and then folding those across
/// families yields the identical winner as flattening every stream and folding
/// once — but never holds more than one family's streams live at a time.
fn keep_smallest(streams: Vec<Vec<u8>>) -> Option<Vec<u8>> {
    let mut streams = streams.into_iter();
    let mut best = streams.next()?;
    for candidate in streams {
        keep_smaller(&mut best, candidate);
    }
    Some(best)
}

/// Fold the per-family minima (already in canonical task order) into the single
/// smallest stream, ties resolving to the earliest family — empty families are
/// skipped, so the seed is the first non-empty family's minimum (the floor
/// family, always present), and the floor still wins global ties exactly as when
/// every stream was flattened and folded together.
fn reduce_task_minima(minima: impl IntoIterator<Item = Option<Vec<u8>>>) -> Vec<u8> {
    minima
        .into_iter()
        .flatten()
        .reduce(|mut best, candidate| {
            keep_smaller(&mut best, candidate);
            best
        })
        .unwrap_or_default()
}

/// Transform candidates fully searched per encode after the entropy
/// pre-score ranks them. Selection is per transform family — each family's
/// est-best task always runs, and further tasks run while their estimate
/// stays within 6% of the family best and 2.5x of the global best, under
/// [`FAMILY_CAP`]. Family diversity matters because order-0 entropy cannot
/// see LZ77 match structure: on flat synthetic content a pure-predictor
/// residual can out-encode every cross-color composition while scoring
/// slightly worse.
const FAMILY_CAP: usize = 4;

/// One task's pre-search score: the cheapest order-0 entropy among its
/// transformed buffers, or `None` for the spec-less floor.
fn estimate_task(
    task: BestTask,
    width: u32,
    height: u32,
    argb: &[u32],
    planes: Option<&PlaneCache>,
    cc: Option<&CrossColorCache>,
    cheap_cc: bool,
) -> Option<u64> {
    estimate_specs(task, width, height, argb, planes, cc, cheap_cc)
        .into_iter()
        .reduce(u64::min)
}

/// Per-spec order-0 entropy estimates, aligned with [`task_specs`] order —
/// empty for `Floor` (and palette-less `Palette`), which carry no specs.
fn estimate_specs(
    task: BestTask,
    width: u32,
    height: u32,
    argb: &[u32],
    planes: Option<&PlaneCache>,
    cc: Option<&CrossColorCache>,
    cheap_cc: bool,
) -> Vec<u64> {
    task_specs(task, width, height, argb, planes, cc, cheap_cc)
        .iter()
        .map(|spec| stream_entropy(&spec.pixels))
        .collect()
}

/// The real-estimate pass for one task: per-spec order-0 entropy, plus — when
/// `with_covs` — the coverage-adjusted band score, computed inside the same
/// spec materialization so the wide-path keep bands never re-derive spec
/// pixels. Returns `None` for spec-less tasks, matching `estimate_specs`.
fn estimate_specs_scored(
    task: BestTask,
    width: u32,
    height: u32,
    argb: &[u32],
    planes: Option<&PlaneCache>,
    cc: Option<&CrossColorCache>,
    with_covs: bool,
) -> Option<(Vec<u64>, Vec<u64>)> {
    let specs = task_specs(task, width, height, argb, planes, cc, false);
    if specs.is_empty() {
        return None;
    }
    let mut ents = Vec::with_capacity(specs.len());
    let mut covs = Vec::new();
    for spec in &specs {
        let entropy = stream_entropy(&spec.pixels);
        ents.push(entropy);
        if with_covs {
            covs.push(coverage_adjusted(entropy, match_coverage(&spec.pixels)));
        }
    }
    Some((ents, covs))
}

/// Turn per-task estimates into the keep mask: `Floor`/`Palette` always
/// kept; every other task ranked by its best buffer's order-0 entropy
/// estimate, then each family keeps its best task plus near-ties under the
/// bands above. Ties resolve to the earlier task so selection is
/// deterministic and identical on the serial and rayon paths.
fn select_from_estimates(
    tasks: &[BestTask],
    estimates: &[Option<u64>],
    band_family_best: bool,
) -> Vec<bool> {
    let mut keep = vec![false; tasks.len()];
    let mut scored: [Vec<(u64, usize)>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    let mut global_best = u64::MAX;
    for (i, task) in tasks.iter().enumerate() {
        let family = match task {
            BestTask::Floor | BestTask::Palette => {
                keep[i] = true;
                continue;
            }
            BestTask::Predictor(_) => 0,
            BestTask::CrossColor(_) => 1,
            BestTask::PredictorCross(..) => 2,
        };
        let Some(est) = estimates[i] else { continue };
        global_best = global_best.min(est);
        scored[family].push((est, i));
    }
    for candidates in scored.iter_mut() {
        candidates.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        let Some(&(best_est, first)) = candidates.first() else { continue };
        // On wide task sets the family best is no longer unconditional: a
        // family whose best estimate stands more than ~1.5x above the
        // global best has never produced the byte winner on the corpus,
        // and searching it costs a full family_search per spec. Narrow
        // task sets keep the unconditional hedge — their estimates are too
        // coarse to trust the band. Near-ties keep the wider 2.5x band.
        if band_family_best && best_est.saturating_mul(2) > global_best.saturating_mul(3) {
            continue;
        }
        keep[first] = true;
        for &(est, i) in candidates.iter().skip(1).take(FAMILY_CAP.saturating_sub(1)) {
            // est <= best_est * 1.06 and est <= global_best * 2.5, in integers.
            if est.saturating_mul(50) > best_est.saturating_mul(53)
                || est.saturating_mul(2) > global_best.saturating_mul(5)
            {
                break;
            }
            keep[i] = true;
        }
    }
    keep
}

/// Provisional keep mask from the cheap coarse-forward estimates: `Floor`/
/// `Palette` always, plus each family's [`PROVISIONAL_CAP`] best tasks. Only
/// provisionally-kept tasks pay for a real cross-color forward + entropy in
/// the second estimate pass; the final [`select_from_estimates`] then runs on
/// real estimates exactly as before. The cap is deliberately wider than
/// [`FAMILY_CAP`] — the coarse multipliers shift entropy estimates by a few
/// percent, so finalists need margin the real pass then trims.
const PROVISIONAL_CAP: usize = 8;

/// Below this pixel count every estimate band stays off: the full search is
/// cheap at this size, small-sample entropy estimates are least reliable, and
/// bands keyed to the task set's own minimum tighten as higher levels add
/// tasks — pruning there broke level monotonicity (generated_transparent
/// L3 > L2, both in the wide path and in [`select_from_estimates`]).
const PRUNE_MIN_PIXELS: usize = 0x40000;

/// Whether the cheap pass can actually prune: it pays only when some family has
/// more members than [`PROVISIONAL_CAP`]. Below that every task is provisional
/// anyway, so the real estimates run directly and the coarse pass is skipped.
fn provisional_prunes(tasks: &[BestTask]) -> bool {
    let mut counts = [0usize; 3];
    for task in tasks {
        match task {
            BestTask::Predictor(_) => counts[0] += 1,
            BestTask::CrossColor(_) => counts[1] += 1,
            BestTask::PredictorCross(..) => counts[2] += 1,
            _ => {}
        }
    }
    counts.iter().any(|&c| c > PROVISIONAL_CAP)
}

fn select_provisional(tasks: &[BestTask], estimates: &[Option<u64>]) -> Vec<bool> {
    let mut keep = vec![false; tasks.len()];
    let mut scored: [Vec<(u64, usize)>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for (i, task) in tasks.iter().enumerate() {
        let family = match task {
            BestTask::Floor | BestTask::Palette => {
                keep[i] = true;
                continue;
            }
            BestTask::Predictor(_) => 0,
            BestTask::CrossColor(_) => 1,
            BestTask::PredictorCross(..) => 2,
        };
        let Some(est) = estimates[i] else { continue };
        scored[family].push((est, i));
    }
    for candidates in scored.iter_mut() {
        candidates.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        for &(_, i) in candidates.iter().take(PROVISIONAL_CAP) {
            keep[i] = true;
        }
    }
    keep
}

fn select_keep(
    tasks: &[BestTask],
    estimates: &[Option<u64>],
    pixels: usize,
    level: u8,
) -> Vec<bool> {
    debug_assert!(pixels < PRUNE_MIN_PIXELS);
    let mut keep = select_from_estimates(tasks, estimates, false);
    let index_of = |t: &BestTask| tasks.iter().position(|c| c == t);
    for lower in 0..level {
        let lower_tasks = build_best_tasks(schedule_for(lower, pixels as u64));
        let lower_estimates: Vec<Option<u64>> = lower_tasks
            .iter()
            .map(|t| index_of(t).and_then(|i| estimates[i]))
            .collect();
        let lower_keep = select_from_estimates(&lower_tasks, &lower_estimates, false);
        for (t, &k) in lower_tasks.iter().zip(&lower_keep) {
            if k && let Some(i) = index_of(t) {
                keep[i] = true;
            }
        }
    }
    keep
}

/// Per-task, per-spec scores: one entry per kept task, `None` when the task was
/// pruned, each holding one score per spec the task would search.
type SpecScores = Vec<Option<Vec<u64>>>;

/// Per-spec keep masks from the real estimates, aligned with [`task_specs`]
/// order per task: a spec survives when its order-0 entropy is within ~5% of
/// the task's best spec (`est * 20 <= min * 21`). Sibling specs that tie on
/// entropy — the predictor+cross-color with/without-subtract-green pair —
/// always both survive; only clear losers skip `family_search`.
fn spec_keep_masks(spec_ests: &[Option<Vec<u64>>]) -> Vec<Vec<bool>> {
    spec_ests
        .iter()
        .map(|opt| match opt {
            Some(ests) => {
                let min = ests.iter().copied().min().unwrap_or(0);
                ests.iter().map(|&e| e * 20 <= min * 21).collect()
            }
            None => Vec::new(),
        })
        .collect()
}

/// Sampled LZ77 match coverage in per-mille: the fraction of sampled pixel
/// positions whose pixel-pair already occurred within a 16K-pixel window
/// earlier in the stream. Order-0 entropy cannot see backreference
/// redundancy; this is the cheap signal that can. A direct-mapped table
/// keeps one (key, position) entry per slot — collisions overwrite like a
/// depth-1 hash chain, which is all an estimate needs.
fn match_coverage(pixels: &[u32]) -> u64 {
    const STRIDE: usize = 4;
    const WINDOW: usize = 16384;
    const BITS: u32 = 16;
    let mut table = vec![u128::MAX; 1usize << BITS];
    let (mut hits, mut total) = (0u64, 0u64);
    let mut i = 0usize;
    while i + 1 < pixels.len() {
        let key = (u64::from(pixels[i]) << 32) | u64::from(pixels[i + 1]);
        let h = (key ^ (key >> 32))
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_shr(64 - BITS) as usize;
        let e = table[h];
        let pos = (e & 0xFFFF_FFFF) as usize;
        if e >> 32 == u128::from(key) && i.wrapping_sub(pos) <= WINDOW {
            hits += 1;
        }
        table[h] = (u128::from(key) << 32) | u128::from(i as u32);
        i += STRIDE;
        total += 1;
    }
    if total == 0 {
        return 0;
    }
    hits * 1024 / total
}

/// Coverage-discounted bit estimate from an already-computed order-0 entropy
/// and sampled match coverage: order-0 entropy scaled by match coverage.
/// Used ONLY as a coarse task-standing score — never inside per-spec ordering,
/// where the ~3% reordering it causes provably loses byte winners
/// (`validation_chelsea`).
fn coverage_adjusted(entropy: u64, coverage: u64) -> u64 {
    entropy
        .saturating_mul(1024u64.saturating_sub(coverage))
        / 1024
}

/// Second keep band over the per-task coverage-adjusted scores: a kept
/// scored task whose best estimate stands more than ~15% above the best kept
/// estimate skips the full `family_search`. The cross-color-alone family
/// trails by 1.5-2.4x on this signal on every corpus image and has never
/// produced the byte winner; the kept near-tie cluster sits within ~11%.
/// `None`-scored tasks (Floor/Palette, fully masked) always pass.
fn coverage_band(scores: &[Option<u64>]) -> Vec<bool> {
    let Some(min) = scores.iter().flatten().min() else {
        return vec![true; scores.len()];
    };
    scores
        .iter()
        .map(|s| s.is_none_or(|v| v * 20 <= min * 23))
        .collect()
}

/// Distinct task sets across levels `0..=level`, coarsest first. Levels that
/// share a [`Breadth`] task set collapse to one entry — their keep decisions
/// are identical, so simulating each distinct set once covers every level.
fn breadth_task_sets(level: u8, pixels: u64) -> Vec<Vec<BestTask>> {
    let mut sets: Vec<Vec<BestTask>> = Vec::new();
    for l in 0..=level {
        let tasks = build_best_tasks(schedule_for(l, pixels));
        if sets.last() != Some(&tasks) {
            sets.push(tasks);
        }
    }
    sets
}

/// Provisional keep mask for the estimate pass: below [`PRUNE_MIN_PIXELS`]
/// every task is estimated; at or above it, the union of each distinct
/// breadth's own provisional selection — a task any coarser level would have
/// estimated for real must be estimated now, or that level's keep decision
/// cannot be reproduced exactly.
fn provisional_keep(
    tasks: &[BestTask],
    level: u8,
    pixels: usize,
    cheap: &[Option<u64>],
) -> Vec<bool> {
    if pixels < PRUNE_MIN_PIXELS {
        return vec![true; tasks.len()];
    }
    let index_of = |t: &BestTask| tasks.iter().position(|c| c == t);
    let mut keep = vec![false; tasks.len()];
    for set in breadth_task_sets(level, pixels as u64) {
        if provisional_prunes(&set) {
            let cheap_b: Vec<Option<u64>> = set
                .iter()
                .map(|t| index_of(t).and_then(|i| cheap.get(i).copied().flatten()))
                .collect();
            for (t, &k) in set.iter().zip(&select_provisional(&set, &cheap_b)) {
                if k && let Some(i) = index_of(t) {
                    keep[i] = true;
                }
            }
        } else {
            for t in &set {
                if let Some(i) = index_of(t) {
                    keep[i] = true;
                }
            }
        }
    }
    keep
}

/// Final keep mask plus per-spec masks. Below [`PRUNE_MIN_PIXELS`] this is
/// [`select_keep`]'s union of coarser-level band selections. At or above it,
/// every distinct breadth's full band pipeline — provisional cap, estimate
/// bands, spec masks, coverage band — is simulated from the level-invariant
/// per-task data and the results unioned, so the searched set at any level is
/// a superset of the set any lower level searched. The [`Effort::level`]
/// contract ("a higher level never yields a larger file") needs exactly that
/// nesting: each level's own bands tighten as its task set grows, and without
/// the union a task a lower level kept can silently drop out (measured:
/// utrecht-meetup L1 -> L2 +7,608 B, retina L1 -> L2 +1,834 B,
/// goose-head L3 -> L4 +5,326 B).
fn final_keep(
    tasks: &[BestTask],
    level: u8,
    pixels: usize,
    cheap: &[Option<u64>],
    spec_ests: &[Option<Vec<u64>>],
    spec_covs: &[Option<Vec<u64>>],
) -> (Vec<bool>, Vec<Vec<bool>>) {
    let spec_masks = spec_keep_masks(spec_ests);
    let estimates: Vec<Option<u64>> = spec_ests
        .iter()
        .map(|o| o.as_ref().and_then(|v| v.iter().copied().min()))
        .collect();
    if pixels < PRUNE_MIN_PIXELS {
        return (select_keep(tasks, &estimates, pixels, level), spec_masks);
    }
    let index_of = |t: &BestTask| tasks.iter().position(|c| c == t);
    let mut keep = vec![false; tasks.len()];
    for set in breadth_task_sets(level, pixels as u64) {
        let wide_b = provisional_prunes(&set);
        let prov_b: Vec<bool> = if wide_b {
            let cheap_b: Vec<Option<u64>> = set
                .iter()
                .map(|t| index_of(t).and_then(|i| cheap.get(i).copied().flatten()))
                .collect();
            select_provisional(&set, &cheap_b)
        } else {
            vec![true; set.len()]
        };
        let ests_b: Vec<Option<u64>> = set
            .iter()
            .zip(&prov_b)
            .map(|(t, &p)| {
                if !p {
                    return None;
                }
                index_of(t).and_then(|i| estimates[i])
            })
            .collect();
        let mut keep_b = select_from_estimates(&set, &ests_b, wide_b);
        if wide_b {
            let scores: Vec<Option<u64>> = set
                .iter()
                .zip(&keep_b)
                .map(|(t, &k)| -> Option<u64> {
                    if !k || matches!(t, BestTask::Floor | BestTask::Palette) {
                        return None;
                    }
                    let i = index_of(t)?;
                    let covs = spec_covs.get(i)?.as_ref()?;
                    covs.iter()
                        .enumerate()
                        .filter(|(j, _)| {
                            spec_masks
                                .get(i)
                                .and_then(|m| m.get(*j))
                                .copied()
                                .unwrap_or(true)
                        })
                        .map(|(_, c)| *c)
                        .min()
                })
                .collect();
            let band = coverage_band(&scores);
            for (k, b) in keep_b.iter_mut().zip(band) {
                *k &= b;
            }
        }
        for (t, &k) in set.iter().zip(&keep_b) {
            if k && let Some(i) = index_of(t) {
                keep[i] = true;
            }
        }
    }
    (keep, spec_masks)
}

/// Run every task serially (default) or across a rayon thread pool (feature
/// `rayon`), folding each family to its own minimum and then across families, so
/// the peak never holds every candidate stream at once — only the running best
/// plus the family currently being evaluated.
///
/// Both paths preserve task order: the serial map folds families in emission
/// order, and `Vec::into_par_iter().map().collect()` is an
/// `IndexedParallelIterator`, so `minima[i]` is always
/// `keep_smallest(run_best_task(tasks[i], ..))` regardless of scheduling — hence
/// rayon-on output equals rayon-off byte-for-byte.
#[cfg(not(feature = "rayon"))]
fn evaluate_best_tasks(
    tasks: Vec<BestTask>,
    breadth: Breadth,
    width: u32,
    height: u32,
    alpha_used: bool,
    argb: &[u32],
    level: u8,
) -> Vec<u8> {
    // Serial: `reduce_task_minima` drives the lazy map, so each family's streams
    // are produced, folded to one, and dropped before the next family runs.
    let planes = PlaneCache::build(breadth, width, height, argb);
    let cc = CrossColorCache::new(&tasks, argb.len());
    let pixels = argb.len();
    // The cheap coarse-forward pass only pays when some distinct breadth's
    // provisional cap can actually prune; the coverage scores ride the same
    // spec materialization under the same condition.
    let need_cheap = pixels >= PRUNE_MIN_PIXELS
        && breadth_task_sets(level, pixels as u64)
            .iter()
            .any(|set| provisional_prunes(set));
    let cheap: Vec<Option<u64>> = if need_cheap {
        tasks
            .iter()
            .map(|t| estimate_task(*t, width, height, argb, planes.as_ref(), None, true))
            .collect()
    } else {
        Vec::new()
    };
    let prov = provisional_keep(&tasks, level, pixels, &cheap);
    let spec_pairs: Vec<Option<(Vec<u64>, Vec<u64>)>> = tasks
        .iter()
        .zip(&prov)
        .map(|(t, &p)| {
            if !p {
                return None;
            }
            estimate_specs_scored(*t, width, height, argb, planes.as_ref(), cc.as_ref(), need_cheap)
        })
        .collect();
    let (spec_ests, spec_covs): (SpecScores, SpecScores) = spec_pairs
        .into_iter()
        .map(|o| match o {
            Some((ents, covs)) => (Some(ents), need_cheap.then_some(covs)),
            None => (None, None),
        })
        .unzip();
    let (keep, spec_masks) = final_keep(&tasks, level, pixels, &cheap, &spec_ests, &spec_covs);
    reduce_task_minima(
        tasks
            .into_iter()
            .zip(keep)
            .zip(spec_masks)
            .map(|((t, k), mask)| {
                if k {
                    keep_smallest(run_best_task(
                        t,
                        breadth,
                        PixelSource {
                            width,
                            height,
                            argb,
                        },
                        alpha_used,
                        planes.as_ref(),
                        cc.as_ref(),
                        &mask,
                    ))
                } else {
                    None
                }
            }),
    )
}
#[cfg(feature = "rayon")]
fn evaluate_best_tasks(
    tasks: Vec<BestTask>,
    breadth: Breadth,
    width: u32,
    height: u32,
    alpha_used: bool,
    argb: &[u32],
    level: u8,
) -> Vec<u8> {
    use rayon::prelude::*;
    debug_assert!(matches!(tasks.first(), Some(BestTask::Floor)));
    // The floor family is never pruned (`cheap_task_score` scores it `None`, so
    // every keep band passes it) and its search depends on nothing the estimate
    // pipeline computes, so it runs joined against the whole estimate+eval
    // pipeline instead of queueing as one more task — on wide task sets this
    // fills the estimate phase's otherwise-idle workers and removes one full
    // search from the eval wave. `join` returns both sides, so task order and
    // the `minima[0]` slot are unchanged.
    let (floor_min, mut minima) = rayon::join(
        || {
            keep_smallest(run_best_task(
                BestTask::Floor,
                breadth,
                PixelSource {
                    width,
                    height,
                    argb,
                },
                alpha_used,
                None,
                None,
                &[],
            ))
        },
        || {
            // Parallel: each worker folds its family to a single stream, so the
            // collected vec holds one stream per non-empty family (not every
            // candidate); `collect` keeps task order, so the fold below resolves
            // ties identically to serial.
            let planes = PlaneCache::build(breadth, width, height, argb);
            let cc = CrossColorCache::new(&tasks, argb.len());
            let pixels = argb.len();
            // The cheap coarse-forward pass only pays when some distinct
            // breadth's provisional cap can actually prune; the coverage
            // scores ride the same spec materialization under the same
            // condition.
            let need_cheap = pixels >= PRUNE_MIN_PIXELS
                && breadth_task_sets(level, pixels as u64)
                    .iter()
                    .any(|set| provisional_prunes(set));
            let cheap: Vec<Option<u64>> = if need_cheap {
                tasks
                    .par_iter()
                    .map(|t| estimate_task(*t, width, height, argb, planes.as_ref(), None, true))
                    .collect()
            } else {
                Vec::new()
            };
            let prov = provisional_keep(&tasks, level, pixels, &cheap);
            let spec_pairs: Vec<Option<(Vec<u64>, Vec<u64>)>> = tasks
                .par_iter()
                .zip(&prov)
                .map(|(t, &p)| {
                    if !p {
                        return None;
                    }
                    estimate_specs_scored(
                        *t,
                        width,
                        height,
                        argb,
                        planes.as_ref(),
                        cc.as_ref(),
                        need_cheap,
                    )
                })
                .collect();
            let (spec_ests, spec_covs): (SpecScores, SpecScores) = spec_pairs
                .into_iter()
                .map(|o| match o {
                    Some((ents, covs)) => (Some(ents), need_cheap.then_some(covs)),
                    None => (None, None),
                })
                .unzip();
            let (keep, spec_masks) =
                final_keep(&tasks, level, pixels, &cheap, &spec_ests, &spec_covs);
            tasks
                .into_par_iter()
                .zip(keep)
                .zip(spec_masks)
                .enumerate()
                .map(|(i, ((t, k), mask))| {
                    if i == 0 {
                        None
                    } else if k {
                        keep_smallest(run_best_task(
                            t,
                            breadth,
                            PixelSource {
                                width,
                                height,
                                argb,
                            },
                            alpha_used,
                            planes.as_ref(),
                            cc.as_ref(),
                            &mask,
                        ))
                    } else {
                        None
                    }
                })
                .collect::<Vec<Option<Vec<u8>>>>()
        },
    );
    if !minima.is_empty() {
        minima[0] = floor_min;
    }
    reduce_task_minima(minima)
}

/// Tier 3 encode at breadth `level` (`0..=9`): return the SMALLEST stream among the
/// transform-free floor and each forward-transform family (palette, predictor,
/// cross-color) the level's [`Breadth`] enables. The floor — the full [`encode`]
/// search — is always in the candidate set and ranked by real emitted bytes, so
/// this is never larger than [`encode`] at any level, and its spatial predictor is
/// always evaluated (no path skips it). Level 9 enables every family.
///
/// Palette is mutually exclusive with the spatial transforms (it reduces the
/// working width; the others do not), so the families are evaluated as separate
/// candidates rather than combined into one stream. When a pointwise
/// subtract-green rides on a spatial transform, the plans are built — and the
/// forwards applied — in the mandated `PALETTE -> PREDICTOR -> CROSS_COLOR ->
/// SUBTRACT_GREEN` order, which the decoder inverts in reverse.
#[must_use]
pub(crate) fn encode_best_at(level: u8, width: u32, height: u32, argb: &[u32]) -> Vec<u8> {
    let breadth = schedule_for(level, u64::from(width) * u64::from(height));
    encode_best_breadth(level, breadth, width, height, argb)
}

/// [`encode_best_at`] for an AUTO-resolved level. AUTO's size cap tops out at
/// level 6 — below the [`Breadth::reparse`] band — so the iterated parse the
/// explicit deep levels get would never run on mid-size images. AUTO adds
/// `reparse` at levels 4-6 (the deepest breadths it can resolve) while the
/// image stays under [`AUTO_REPARSE_PIXEL_CAP`], bounding the extra passes to
/// the same sub-256K-pixel envelope the level-6 Fast band already pays; larger
/// AUTO levels keep the single-pass parse because the passes multiply an
/// already-heavy optimal parse. Explicit `Effort::level` keeps the cheaper
/// breadth at every level — the product Fast band shares levels 1 and 6.
pub(crate) fn encode_best_auto(level: u8, width: u32, height: u32, argb: &[u32]) -> Vec<u8> {
    let pixels = u64::from(width) * u64::from(height);
    let mut breadth = schedule_for(level, pixels);
    if level >= 4 && pixels <= AUTO_REPARSE_PIXEL_CAP {
        breadth.reparse = true;
    }
    encode_best_breadth(level, breadth, width, height, argb)
}

/// Pixel ceiling for the AUTO-only reparse breadth: the 256K-pixel band where
/// AUTO resolves levels 4-6 and a worst-case reparse stays in the same
/// tens-of-seconds envelope as the explicit level-6 encode.
const AUTO_REPARSE_PIXEL_CAP: u64 = 0x40000;

fn encode_best_breadth(
    level: u8,
    breadth: Breadth,
    width: u32,
    height: u32,
    argb: &[u32],
) -> Vec<u8> {
    let alpha_used = argb.iter().any(|&p| p >> 24 != 0xff);
    evaluate_best_tasks(
        build_best_tasks(breadth),
        breadth,
        width,
        height,
        alpha_used,
        argb,
        level,
    )
}

/// Full-breadth ([`schedule`] level 9) Tier 3 encode — byte-for-byte the historical
/// `encode_best`, the reference the byte-stability and round-trip tests pin.
#[cfg(test)]
pub(crate) fn encode_best(width: u32, height: u32, argb: &[u32]) -> Vec<u8> {
    encode_best_at(crate::effort::MAX_LEVEL, width, height, argb)
}

/// Replace `best` with `candidate` when the candidate is strictly smaller.
fn keep_smaller(best: &mut Vec<u8>, candidate: Vec<u8>) {
    if candidate.len() < best.len() {
        *best = candidate;
    }
}

/// One back-reference's cache-independent bitstream cost: the length and
/// distance prefix symbols and their extra-bit widths. Computed once per copy by
/// [`RefModel::new`] so the per-cache-size cost passes never recompute
/// `prefix_encode` / the O(120) `distance_to_plane_code` scan.
#[derive(Clone, Copy)]
struct CopyCost {
    length_symbol: u32,
    length_bits: u32,
    dist_symbol: u32,
    dist_bits: u32,
}

/// The cost model for one parsed token stream, reused across every candidate
/// cache size and by the final emit.
///
/// A back-reference's length/distance symbols and extra bits do not depend on
/// the color cache, so [`Self::new`] resolves them once into [`CopyCost`]s; only
/// the literal-vs-cache decision and the per-pixel cache-insert order vary with
/// the cache size, and those are replayed per candidate in [`Self::histogram`].
/// This hoists the `prefix_encode` / `distance_to_plane_code` work the old
/// per-cache-size `accumulate` recomputed for all `1..=MAX_CACHE_BITS + 1`
/// candidates (times two strategies, times two tiers).
pub(crate) struct RefModel<'a> {
    /// The parsed token stream (borrowed for the model's lifetime).
    tokens: &'a [Token],
    /// The pixels the tokens were parsed from, for the copies' cache inserts.
    pixels: &'a [u32],
    /// Image width, for the header and the distance plane-code mapping.
    width: u32,
    /// The resolved cost of each `Copy` token, in token order.
    copies: Vec<CopyCost>,
}

impl<'a> RefModel<'a> {
    /// Resolve the cache-independent cost of every back-reference in one pass.
    pub(crate) fn new(tokens: &'a [Token], pixels: &'a [u32], width: u32) -> Self {
        // `width` is fixed, so resolve every copy's distance through one reverse
        // map instead of the per-copy 120-entry rescan.
        let plane_map = PlaneCodeMap::new(width);
        let copies = tokens
            .iter()
            .filter_map(|&token| match token {
                Token::Copy { length, distance } => {
                    let (length_symbol, length_bits, _) = prefix_encode(length);
                    let plane_code = plane_map.plane_code(distance);
                    let (dist_symbol, dist_bits, _) = prefix_encode(plane_code);
                    Some(CopyCost {
                        length_symbol,
                        length_bits,
                        dist_symbol,
                        dist_bits,
                    })
                },
                Token::Literal(_) => None,
            })
            .collect();
        Self {
            tokens,
            pixels,
            width,
            copies,
        }
    }

    /// Build the symbol histogram for a `cache_bits`-wide cache (`0` = disabled).
    ///
    /// Copies contribute their pre-resolved [`CopyCost`]s (no `prefix_encode` /
    /// `distance_to_plane_code` recomputation); only the literals' cache
    /// decisions and the copies' cache inserts are replayed here, in the exact
    /// output order [`resolve`] uses (literal: check-then-insert; copy:
    /// `pixels[pos..pos + length]`). The result is therefore bit-identical to
    /// walking every unit through `resolve` — pinned by the
    /// `ref_model_matches_accumulate` proptest — so the histogram feeding the
    /// Huffman builder can never disagree with the [`emit_tokens`] pass.
    pub(crate) fn histogram(&self, cache_bits: u32) -> Histogram {
        let cache_codes = if cache_bits > 0 {
            1usize << cache_bits
        } else {
            0
        };
        work!(HistogramAlloc);
        let mut histogram = Histogram::new(ALPHABET_SIZE[0] + cache_codes);
        let mut cache = (cache_bits > 0).then(|| {
            work!(ColorCacheAlloc);
            ColorCache::new(cache_bits)
        });
        self.accumulate_into(cache_bits, &mut histogram, cache.as_mut());
        histogram
    }

    /// Fill an already-zeroed `histogram` from the token stream under a
    /// `cache_bits`-wide cache (`cache` is `Some` iff `cache_bits > 0`, and must
    /// be reset to `cache_bits` on entry). This is the single walk shared by the
    /// allocating [`Self::histogram`] and the buffer-reusing [`best_cache_bits`]
    /// sweep; both therefore produce byte-identical bins.
    fn accumulate_into(
        &self,
        cache_bits: u32,
        histogram: &mut Histogram,
        mut cache: Option<&mut ColorCache>,
    ) {
        work!(HistogramPass);
        let mut pos = 0usize;
        let mut next_copy = 0usize;
        for &token in self.tokens {
            match token {
                Token::Literal(argb) => {
                    match cache.as_deref_mut() {
                        None => histogram.add_literal(argb),
                        Some(cache) => {
                            let key = ColorCache::index(argb, cache_bits);
                            if cache.get(key) == argb {
                                histogram.add_cache(key as u16);
                            } else {
                                histogram.add_literal(argb);
                            }
                            cache.insert(argb);
                        },
                    }
                    pos += 1;
                },
                Token::Copy { length, .. } => {
                    let cost = self.copies[next_copy];
                    next_copy += 1;
                    histogram.add_length(cost.length_symbol, cost.length_bits);
                    histogram.add_distance(cost.dist_symbol, cost.dist_bits);
                    if let Some(cache) = cache.as_deref_mut() {
                        for pixel in &self.pixels[pos..pos + length as usize] {
                            cache.insert(*pixel);
                        }
                    }
                    pos += length as usize;
                },
            }
        }
    }
}

/// Choose the color-cache size (`0` = disabled) that minimizes the model's
/// estimated encoded size, returning it alongside the winning size's histogram.
/// The scan is ascending from `0` and updates only on a strict improvement, so
/// ties prefer the smaller size (a cache that does not help is left off). This
/// ordering is load-bearing: changing it would change the selected cache size
/// and hence the emitted bytes.
///
/// The returned histogram is a truncated snapshot of the scratch at the winning
/// size, bin-identical to [`RefModel::histogram`] for `best_bits`; handing it
/// back lets [`emit_coded_pixels`] skip re-accumulating the same bins — pinned by
/// `best_cache_bits_histogram_matches_fresh`.
fn best_cache_bits(model: &RefModel<'_>) -> (u32, Histogram) {
    // Reuse one max-size scratch histogram + cache across all MAX_CACHE_BITS + 1
    // candidates instead of allocating a fresh pair per size (a fresh-per-size pass
    // would do ~12 heap allocations per model, times three models, times two tiers,
    // times every Best transform family). The green bins are zero-padded to the largest
    // alphabet; since `estimate_bits` skips zero-count bins, the reused estimate
    // is byte-identical to a per-size exact histogram — pinned by
    // `best_cache_bits_reuse_matches_fresh`.
    let mut hist = Histogram::new(ALPHABET_SIZE[0] + (1usize << MAX_CACHE_BITS));
    let mut cache = ColorCache::new(MAX_CACHE_BITS);

    let mut best_bits = 0;
    work!(CacheBitsBuild);
    hist.reset();
    model.accumulate_into(0, &mut hist, None);
    let mut best_cost = hist.estimate_bits();
    // Snapshot the winning scratch, truncated to that size's green alphabet
    // (`ALPHABET_SIZE[0] + (1 << bits)`, or `ALPHABET_SIZE[0]` for no cache): the
    // bins past it are always zero, so this equals a fresh per-size histogram.
    let mut best_hist = hist.snapshot_truncated(ALPHABET_SIZE[0]);
    for bits in 1..=MAX_CACHE_BITS {
        work!(CacheBitsBuild);
        hist.reset();
        cache.reset(bits);
        model.accumulate_into(bits, &mut hist, Some(&mut cache));
        let cost = hist.estimate_bits();
        if cost < best_cost {
            best_cost = cost;
            best_bits = bits;
            best_hist = hist.snapshot_truncated(ALPHABET_SIZE[0] + (1usize << bits));
        }
    }
    (best_bits, best_hist)
}

/// Serialize a level-0 VP8L payload: the 5-byte header (carrying the ORIGINAL
/// image dimensions), the `plans` transform list, the color-cache header, the
/// single-group meta-Huffman bit, and the coded pixel data.
///
/// The coded pixels are laid out at the model's WORKING width (`model.width`),
/// which a color-indexing transform reduces below `header_width`; every other
/// transform family leaves them equal. The cost model supplies the symbol
/// histogram and [`emit_tokens`] re-walks the same tokens through [`resolve`].
fn emit_stream(
    header_width: u32,
    header_height: u32,
    alpha_used: bool,
    plans: &[TransformPlan],
    cache_bits: u32,
    model: &RefModel<'_>,
    histogram: &Histogram,
) -> Vec<u8> {
    let mut bw = BitWriter::new();
    write_header(&mut bw, header_width, header_height, alpha_used);
    write_transforms(&mut bw, plans, header_width, header_height);
    write_color_cache(&mut bw, cache_bits);
    bw.write_bits(0, 1); // no meta-Huffman
    emit_coded_pixels(&mut bw, cache_bits, model, histogram);
    bw.into_bytes()
}

/// Level-0 multi-group (meta-Huffman) stream: like [`emit_stream`] but writes the
/// meta bit = 1, the 3-bit precision, the entropy sub-image, and one Huffman group
/// per cluster. The single-group [`emit_stream`] is untouched, so Balanced/Fast
/// bytes are unaffected.
fn emit_stream_meta(
    header_width: u32,
    header_height: u32,
    alpha_used: bool,
    plans: &[TransformPlan],
    cache_bits: u32,
    model: &RefModel<'_>,
    plan: &MetaPlan,
) -> Vec<u8> {
    let mut bw = BitWriter::new();
    write_header(&mut bw, header_width, header_height, alpha_used);
    write_transforms(&mut bw, plans, header_width, header_height);
    write_color_cache(&mut bw, cache_bits);
    bw.write_bits(1, 1); // meta-Huffman present
    bw.write_bits(plan.bits - MIN_TRANSFORM_BITS, NUM_TRANSFORM_BITS); // inverse of read_bits(3)+2
    let entropy_pixels: Vec<u32> = plan.groups.iter().map(|&g| u32::from(g) << 8).collect();
    emit_subimage(&mut bw, plan.entropy_xsize, &entropy_pixels);
    emit_coded_pixels_meta(&mut bw, cache_bits, model, plan);
    bw.into_bytes()
}

/// Emit one Huffman group per cluster (all five channel codes, in group-id order,
/// exactly as the decoder reads them) followed by the group-selected pixel tokens.
fn emit_coded_pixels_meta(
    bw: &mut BitWriter,
    cache_bits: u32,
    model: &RefModel<'_>,
    plan: &MetaPlan,
) {
    let prefix_sets: Vec<[Prefix; 5]> = plan
        .group_histograms
        .iter()
        .map(|h| {
            [
                Prefix::from_histogram(h.green()),
                Prefix::from_histogram(h.red()),
                Prefix::from_histogram(h.blue()),
                Prefix::from_histogram(h.alpha()),
                Prefix::from_histogram(h.dist()),
            ]
        })
        .collect();
    for set in &prefix_sets {
        for prefix in set {
            write_huffman_code(bw, &prefix.lengths);
        }
    }
    emit_tokens_meta(
        bw,
        model.tokens,
        model.pixels,
        cache_bits,
        model.width,
        &prefix_sets,
        plan,
    );
}

/// Emit every token's bits, selecting the group's prefix set by the unit's
/// start-position block — byte-identical selection to the decoder's `select_group`.
fn emit_tokens_meta(
    bw: &mut BitWriter,
    tokens: &[Token],
    pixels: &[u32],
    cache_bits: u32,
    width: u32,
    prefix_sets: &[[Prefix; 5]],
    plan: &MetaPlan,
) {
    resolve(tokens, pixels, cache_bits, width, |pos, unit| {
        let x = pos as u32 % width;
        let y = pos as u32 / width;
        let block = ((y >> plan.bits) * plan.entropy_xsize + (x >> plan.bits)) as usize;
        let [green, red, blue, alpha, dist] = &prefix_sets[plan.groups[block] as usize];
        match unit {
            Resolved::Literal(argb) => {
                green.emit(bw, ((argb >> 8) & 0xff) as usize);
                red.emit(bw, ((argb >> 16) & 0xff) as usize);
                blue.emit(bw, (argb & 0xff) as usize);
                alpha.emit(bw, ((argb >> 24) & 0xff) as usize);
            },
            Resolved::Copy {
                length_symbol,
                length_extra,
                dist_symbol,
                dist_extra,
            } => {
                green.emit(bw, NUM_LITERAL_CODES + length_symbol as usize);
                bw.write_bits(length_extra.0, length_extra.1);
                dist.emit(bw, dist_symbol as usize);
                bw.write_bits(dist_extra.0, dist_extra.1);
            },
            Resolved::Cache(key) => {
                green.emit(bw, NUM_LITERAL_CODES + NUM_LENGTH_CODES + usize::from(key));
            },
        }
    });
}

/// Emit the entropy-coded tail shared by every image stream: the five channel
/// Huffman codes followed by the pixel tokens. The pixel width comes from the
/// model (the working width), and the caller has already written the color-cache
/// header (plus, for level-0 streams, the meta-Huffman bit) ahead of this. The
/// `histogram` must be the model's histogram at `cache_bits` (as returned by
/// [`RefModel::histogram`] or [`best_cache_bits`]); the caller supplies it so the
/// bins built for the cache-size search can be reused instead of re-accumulated.
fn emit_coded_pixels(
    bw: &mut BitWriter,
    cache_bits: u32,
    model: &RefModel<'_>,
    histogram: &Histogram,
) {
    let prefixes = [
        Prefix::from_histogram(histogram.green()),
        Prefix::from_histogram(histogram.red()),
        Prefix::from_histogram(histogram.blue()),
        Prefix::from_histogram(histogram.alpha()),
        Prefix::from_histogram(histogram.dist()),
    ];
    for prefix in &prefixes {
        write_huffman_code(bw, &prefix.lengths);
    }
    emit_tokens(
        bw,
        model.tokens,
        model.pixels,
        cache_bits,
        model.width,
        &prefixes,
    );
}

/// Emit a nested (`is_level0 = false`) sub-image of `width`-wide `pixels`: the
/// color-cache header (always absent) then the coded pixels, with NO transform
/// list and NO meta-Huffman bit — exactly what [`crate::lossless::vp8l::decode`] reads for a
/// transform's tile grid or a palette's color map. The height is implicit in
/// `pixels.len() / width`. The pixels are coded with no color cache; the
/// literal, greedy-LZ77 and optimal-parse token streams are each emitted to
/// scratch writers and the smallest kept, since copies pay off on the
/// run-heavy tile grids and entropy image but lose on scattered ones.
fn emit_subimage(bw: &mut BitWriter, width: u32, pixels: &[u32]) {
    write_color_cache(bw, 0);
    let literals: Vec<Token> = pixels.iter().map(|&p| Token::Literal(p)).collect();
    let (lz77, chain) = parse_lz77(pixels);
    let mut options = vec![literals];
    if lz77.iter().any(|t| matches!(t, Token::Copy { .. })) {
        options.push(parse_optimal(pixels, width, &lz77, &chain, MAX_CHAIN));
        options.push(lz77);
    }
    let tokens = options
        .iter()
        .min_by_key(|tokens| subimage_bits(width, pixels, tokens))
        .expect("the literal candidate is always present");
    let model = RefModel::new(tokens, pixels, width);
    emit_coded_pixels(bw, 0, &model, &model.histogram(0));
}

/// The exact bit count [`emit_subimage`] would write for one token stream,
/// measured by emitting into a scratch writer — backward references are legal
/// in sub-images, and the literal/LZ77 pick is floored on real bytes rather
/// than a cost model.
fn subimage_bits(width: u32, pixels: &[u32], tokens: &[Token]) -> u64 {
    let mut scratch = BitWriter::new();
    write_color_cache(&mut scratch, 0);
    let model = RefModel::new(tokens, pixels, width);
    emit_coded_pixels(&mut scratch, 0, &model, &model.histogram(0));
    scratch.bit_len()
}

/// Write the fixed VP8L header: signature, `width - 1`, `height - 1`, the
/// alpha-used advisory bit, and the (always-zero) version field.
fn write_header(bw: &mut BitWriter, width: u32, height: u32, alpha_used: bool) {
    bw.write_bits(u32::from(VP8L_MAGIC_BYTE), 8);
    bw.write_bits(width - 1, VP8L_IMAGE_SIZE_BITS);
    bw.write_bits(height - 1, VP8L_IMAGE_SIZE_BITS);
    bw.write_bits(u32::from(alpha_used), 1);
    bw.write_bits(0, VP8L_VERSION_BITS);
}

/// Write the transform list: for each plan (in vec order) a present bit, its
/// 2-bit type code, and any header + nested sub-image, then the terminating "no
/// more transforms" bit. `width` is the ORIGINAL image width, used to size the
/// predictor/cross-color tile grid via [`subsample_size`]. `_height` completes
/// the transform's logical geometry (mirroring the decoder's `ysize`) but every
/// sub-image's row count is implicit in its emitted slice length, so no arm reads
/// it.
fn write_transforms(bw: &mut BitWriter, plans: &[TransformPlan], width: u32, _height: u32) {
    for plan in plans {
        bw.write_bits(1, 1); // one transform present
        match plan {
            TransformPlan::Predictor { bits, tile_data } => {
                bw.write_bits(PREDICTOR_TRANSFORM, TRANSFORM_TYPE_BITS);
                bw.write_bits(*bits - MIN_TRANSFORM_BITS, NUM_TRANSFORM_BITS);
                emit_subimage(bw, subsample_size(width, *bits), tile_data);
            },
            TransformPlan::CrossColor { bits, tile_data } => {
                bw.write_bits(CROSS_COLOR_TRANSFORM, TRANSFORM_TYPE_BITS);
                bw.write_bits(*bits - MIN_TRANSFORM_BITS, NUM_TRANSFORM_BITS);
                emit_subimage(bw, subsample_size(width, *bits), tile_data);
            },
            TransformPlan::SubtractGreen => {
                bw.write_bits(SUBTRACT_GREEN_TRANSFORM, TRANSFORM_TYPE_BITS);
            },
            TransformPlan::ColorIndexing {
                num_colors,
                colormap,
            } => {
                bw.write_bits(COLOR_INDEXING_TRANSFORM, TRANSFORM_TYPE_BITS);
                bw.write_bits(*num_colors - 1, NUM_COLORS_FIELD_WIDTH);
                emit_subimage(bw, *num_colors, colormap);
            },
        }
    }
    bw.write_bits(0, 1); // no (further) transforms
}

/// Write the color-cache header: a present bit and, if set, the 4-bit size.
fn write_color_cache(bw: &mut BitWriter, cache_bits: u32) {
    if cache_bits > 0 {
        bw.write_bits(1, 1);
        bw.write_bits(cache_bits, CACHE_BITS_FIELD_WIDTH);
    } else {
        bw.write_bits(0, 1);
    }
}

/// Emit every token's bits, matching the decoder's read order exactly.
fn emit_tokens(
    bw: &mut BitWriter,
    tokens: &[Token],
    pixels: &[u32],
    cache_bits: u32,
    width: u32,
    prefixes: &[Prefix; 5],
) {
    let [green, red, blue, alpha, dist] = prefixes;
    resolve(tokens, pixels, cache_bits, width, |_pos, unit| match unit {
        Resolved::Literal(argb) => {
            green.emit(bw, ((argb >> 8) & 0xff) as usize);
            red.emit(bw, ((argb >> 16) & 0xff) as usize);
            blue.emit(bw, (argb & 0xff) as usize);
            alpha.emit(bw, ((argb >> 24) & 0xff) as usize);
        },
        Resolved::Copy {
            length_symbol,
            length_extra,
            dist_symbol,
            dist_extra,
        } => {
            green.emit(bw, NUM_LITERAL_CODES + length_symbol as usize);
            bw.write_bits(length_extra.0, length_extra.1);
            dist.emit(bw, dist_symbol as usize);
            bw.write_bits(dist_extra.0, dist_extra.1);
        },
        Resolved::Cache(key) => {
            green.emit(bw, NUM_LITERAL_CODES + NUM_LENGTH_CODES + usize::from(key));
        },
    });
}

/// The cheaper of the direct and RLE-perturbed histogram's code lengths,
/// priced as `Σ count·length + serialized header bits` — the same total the
/// emitted stream pays, so the choice can never lose to the direct build.
fn best_code_lengths(histogram: &[u32]) -> Vec<u32> {
    let direct = build_code_lengths(histogram, MAX_MAIN_CODE_LENGTH);
    let mut perturbed = histogram.to_vec();
    let mut rle = vec![0u8; histogram.len()];
    optimize_huffman_for_rle(&mut perturbed, &mut rle);
    if perturbed.as_slice() == histogram {
        return direct;
    }
    let rle_lengths = build_code_lengths(&perturbed, MAX_MAIN_CODE_LENGTH);
    let cost = |lengths: &[u32]| {
        histogram
            .iter()
            .zip(lengths)
            .map(|(&c, &l)| u64::from(c) * u64::from(l))
            .sum::<u64>()
            + serialized_bits(lengths)
    };
    if cost(&rle_lengths) < cost(&direct) {
        rle_lengths
    } else {
        direct
    }
}

/// The prefix code for one channel: per-symbol lengths (for serialization) plus
/// the LSB-first `(code, emit_len)` pairs used to emit each symbol.
struct Prefix {
    /// Per-symbol code lengths (`0` = unused), sized to the channel alphabet.
    lengths: Vec<u32>,
    /// LSB-first emit `(code, len)` per symbol. `len == 0` for a `<= 1`-symbol
    /// alphabet, so a single-valued channel costs zero bits per pixel.
    codes: Vec<(u32, u32)>,
}

impl Prefix {
    /// Build the length-limited (`<= 15`) prefix code for one channel histogram.
    /// libwebp's `VP8LCreateHuffmanTree` first perturbs the counts so the
    /// resulting lengths run-length-encode smaller; both assignments are priced
    /// exactly (pixel bits + serialized header bits) and the cheaper kept.
    fn from_histogram(histogram: &[u32]) -> Self {
        let lengths = best_code_lengths(histogram);
        let codes = emit_codes(&lengths);
        Self { lengths, codes }
    }

    /// Emit `symbol`'s code; a strict no-op for a single-symbol alphabet, whose
    /// entries are `(0, 0)` and `write_bits(_, 0)` writes nothing.
    fn emit(&self, bw: &mut BitWriter, symbol: usize) {
        let (code, len) = self.codes[symbol];
        bw.write_bits(code, len);
    }
}

#[cfg(test)]
mod tests {
    use crate::lossless::constants::MAX_CHAIN_OPTIMAL;
    use super::{
        PixelSource, RefModel, StreamSwitches, TransformPlan, best_cache_bits, emit_stream,
        emit_stream_meta, encode, encode_best, encode_best_at, encode_stream,
    };
    use crate::effort::MAX_LEVEL;
    use crate::lossless::transform::predictor;
    use crate::lossless::vp8l::backref::parse;
    use crate::lossless::vp8l::decode::decode;
    use crate::lossless::vp8l::meta;

    /// Pack channels into a native ARGB pixel (`0xAARRGGBB`).
    fn argb(a: u32, r: u32, g: u32, b: u32) -> u32 {
        (a << 24) | (r << 16) | (g << 8) | b
    }

    /// Encode then decode and assert an exact round-trip.
    fn assert_round_trip(width: u32, height: u32, pixels: &[u32]) {
        let payload = encode(width, height, pixels);
        let decoded = decode(&payload).expect("our encoder must emit decodable VP8L");
        assert_eq!((decoded.width, decoded.height), (width, height));
        assert_eq!(decoded.argb.as_slice(), pixels);
    }

    /// `alpha_used` as the encoder computes it (used to drive `encode_stream`).
    fn alpha_used(pixels: &[u32]) -> bool {
        pixels.iter().any(|&p| p >> 24 != 0xff)
    }

    /// The all-literal, no-cache, no-transform baseline stream.
    fn literal_baseline(width: u32, height: u32, pixels: &[u32]) -> Vec<u8> {
        let tokens = parse(pixels, false);
        let model = RefModel::new(&tokens, pixels, width);
        emit_stream(
            width,
            height,
            alpha_used(pixels),
            &[],
            0,
            &model,
            &model.histogram(0),
        )
    }

    #[test]
    fn round_trips_a_1x1_pixel() {
        assert_round_trip(1, 1, &[argb(255, 50, 100, 200)]);
    }

    #[test]
    fn solid_image_has_empty_pixel_data() {
        // Every channel is a single-symbol code, so the pixel-data section is
        // empty regardless of pixel count. LZ77/cache must not inflate this: the
        // 1x1 and 4x4 payloads differ only in the 14-bit dimension fields and
        // must have identical length.
        let color = argb(255, 10, 20, 30);
        let small = encode(1, 1, &[color]);
        let large = encode(4, 4, &[color; 16]);
        assert_eq!(small.len(), large.len());
        assert_round_trip(1, 1, &[color]);
        assert_round_trip(4, 4, &[color; 16]);
    }

    #[test]
    fn round_trips_a_gradient() {
        let pixels: Vec<u32> = (0..64u32).map(|v| argb(255, v * 2, v, 255 - v)).collect();
        assert_round_trip(8, 8, &pixels);
    }

    #[test]
    fn round_trips_all_transparent_preserving_rgb() {
        // Alpha is 0 everywhere (so alpha_used is set) but the distinct RGB must
        // survive: the encoder never discards fully-transparent color data.
        let pixels: Vec<u32> = (0..16u32).map(|v| argb(0, v, 63 - v, v * 3)).collect();
        assert!(alpha_used(&pixels));
        assert_round_trip(4, 4, &pixels);
    }

    #[test]
    fn round_trips_a_single_row() {
        let pixels: Vec<u32> = (0..20u32).map(|v| argb(255, v, 100, 200 - v)).collect();
        assert_round_trip(20, 1, &pixels);
    }

    #[test]
    fn round_trips_a_single_column() {
        let pixels: Vec<u32> = (0..20u32).map(|v| argb(255, 5, v, v)).collect();
        assert_round_trip(1, 20, &pixels);
    }

    #[test]
    fn tier1_chosen_when_subtract_green_shrinks() {
        // Grayscale ramp (r == g == b): subtract-green zeroes red and blue into
        // single-symbol codes, leaving only green to code, so Tier 1 is smaller.
        let pixels: Vec<u32> = (0..32u32).map(|v| argb(255, v, v, v)).collect();
        let au = alpha_used(&pixels);
        let tier0 = encode_stream(
            PixelSource {
                width: 32,
                height: 1,
                argb: &pixels,
            },
            au,
            StreamSwitches {
                use_subtract_green: false,
                use_lz77: true,
                chain_cap: MAX_CHAIN_OPTIMAL,
                meta_huffman: false,
            },
        );
        let tier1 = encode_stream(
            PixelSource {
                width: 32,
                height: 1,
                argb: &pixels,
            },
            au,
            StreamSwitches {
                use_subtract_green: true,
                use_lz77: true,
                chain_cap: MAX_CHAIN_OPTIMAL,
                meta_huffman: false,
            },
        );
        assert!(
            tier1.len() < tier0.len(),
            "subtract-green must shrink a grayscale ramp"
        );
        assert_eq!(encode(32, 1, &pixels), tier1);
        assert_round_trip(32, 1, &pixels);
    }

    #[test]
    fn tier0_chosen_when_only_green_varies() {
        // Only green varies; red/blue/alpha are constant. Subtract-green would
        // spread green's variation into red and blue, enlarging the stream, so
        // Tier 0 wins (also exercising the tie-goes-to-Tier-0 `<=` boundary).
        let pixels: Vec<u32> = (0..32u32).map(|v| argb(255, 0x40, v, 0x80)).collect();
        let au = alpha_used(&pixels);
        let tier0 = encode_stream(
            PixelSource {
                width: 32,
                height: 1,
                argb: &pixels,
            },
            au,
            StreamSwitches {
                use_subtract_green: false,
                use_lz77: true,
                chain_cap: MAX_CHAIN_OPTIMAL,
                meta_huffman: false,
            },
        );
        let tier1 = encode_stream(
            PixelSource {
                width: 32,
                height: 1,
                argb: &pixels,
            },
            au,
            StreamSwitches {
                use_subtract_green: true,
                use_lz77: true,
                chain_cap: MAX_CHAIN_OPTIMAL,
                meta_huffman: false,
            },
        );
        assert!(tier0.len() <= tier1.len());
        assert_eq!(encode(32, 1, &pixels), tier0);
        assert_round_trip(32, 1, &pixels);
    }

    #[test]
    fn lz77_shrinks_a_repeating_pattern() {
        // A 4-color tile repeated 64 times: back-references must beat literals.
        let tile = [
            argb(255, 10, 20, 30),
            argb(255, 40, 50, 60),
            argb(255, 70, 80, 90),
            argb(255, 100, 110, 120),
        ];
        let pixels: Vec<u32> = tile.iter().cycle().take(256).copied().collect();
        let full = encode(64, 4, &pixels);
        let baseline = literal_baseline(64, 4, &pixels);
        assert!(
            full.len() < baseline.len(),
            "LZ77 must shrink a repeating pattern: {} vs {}",
            full.len(),
            baseline.len()
        );
        assert_round_trip(64, 4, &pixels);
    }

    #[test]
    fn cost_model_enables_cache_for_a_scattered_palette() {
        // Twelve colors scattered with no runs (mirroring the color_cache_scatter
        // fixture): a cache hit codes a whole pixel with one green symbol, so the
        // cost model must turn the cache on — the path dwebp validates at gen time.
        let pixels: Vec<u32> = (0..1024u32)
            .map(|i| {
                let (x, y) = (i % 32, i / 32);
                let c = (x.wrapping_mul(5).wrapping_add(y.wrapping_mul(11))) % 12;
                argb(255, c * 20 + 8, c * 13 + 17, c * 7 + 29)
            })
            .collect();
        let tokens = parse(&pixels, false);
        let (bits, _) = best_cache_bits(&RefModel::new(&tokens, &pixels, 32));
        assert!(
            bits > 0,
            "cost model must enable a cache for a scattered palette"
        );
        assert_round_trip(32, 32, &pixels);
    }

    #[test]
    fn forced_cache_stream_round_trips() {
        // Force a cache-coded stream (two alternating colors) and confirm the
        // decoder's cache path replays it exactly. This exercises the encoder's
        // cache emission and insert order independent of the cost model's choice.
        let a = argb(255, 1, 2, 3);
        let b = argb(255, 250, 240, 230);
        let pixels: Vec<u32> = (0..40u32).map(|i| if i % 2 == 0 { a } else { b }).collect();
        let tokens = parse(&pixels, false);
        let model = RefModel::new(&tokens, &pixels, 40);
        let stream = emit_stream(40, 1, false, &[], 6, &model, &model.histogram(6));
        let decoded = decode(&stream).expect("a cache-coded stream must decode");
        assert_eq!((decoded.width, decoded.height), (40, 1));
        assert_eq!(decoded.argb, pixels);
    }

    #[test]
    fn predictor_subimage_stream_round_trips() {
        // Hand-build a level-0 stream carrying a predictor transform whose per-tile
        // mode grid is emitted as a nested (is_level0 = false) sub-image, then
        // decode it back to the source. This proves `emit_stream` +
        // `write_transforms` + `emit_subimage` reproduce the decoder's nested read
        // order end-to-end, before the tiered `encode_best` driver exists.
        let width = 4u32;
        let height = 4u32;
        let source: Vec<u32> = (0..16u32)
            .map(|v| argb(255, v * 4, v * 2, 100 + v))
            .collect();
        // bits = 2 -> a single 4x4 tile covers the image; `forward` picks the mode.
        let bits = 2u32;
        let (residual, tile_data) = predictor::forward(&source, width, height, bits);

        // The coded pixels are the residual; the header carries the ORIGINAL dims,
        // and the predictor tile grid rides along as the nested sub-image.
        let tokens = parse(&residual, false);
        let model = RefModel::new(&tokens, &residual, width);
        let plans = [TransformPlan::Predictor { bits, tile_data }];
        let stream = emit_stream(
            width,
            height,
            alpha_used(&source),
            &plans,
            0,
            &model,
            &model.histogram(0),
        );

        let decoded = decode(&stream).expect("a predictor sub-image stream must decode");
        assert_eq!((decoded.width, decoded.height), (width, height));
        assert_eq!(decoded.argb, source);
    }

    #[test]
    fn never_regresses_the_literal_baseline() {
        // Whatever the input, the chosen stream is never larger than the plain
        // literal baseline that is always among the candidates.
        let cases: [(u32, u32, Vec<u32>); 3] = [
            (
                8,
                8,
                (0..64u32).map(|v| argb(255, v, v * 3, v * 7)).collect(),
            ),
            (16, 1, vec![argb(255, 5, 5, 5); 16]),
            (
                4,
                4,
                (0..16u32).map(|v| argb(255, v % 3, v % 3, v % 3)).collect(),
            ),
        ];
        for (w, h, pixels) in cases {
            assert!(encode(w, h, &pixels).len() <= literal_baseline(w, h, &pixels).len());
            assert_round_trip(w, h, &pixels);
        }
    }

    /// Encode with the Tier 3 `Best` driver, decode, and assert an exact
    /// round-trip through the transform families it may emit.
    fn assert_best_round_trip(width: u32, height: u32, pixels: &[u32]) {
        let payload = encode_best(width, height, pixels);
        let decoded = decode(&payload).expect("encode_best must emit decodable VP8L");
        assert_eq!((decoded.width, decoded.height), (width, height));
        assert_eq!(decoded.argb.as_slice(), pixels);
    }

    #[test]
    fn best_never_regresses_the_floor() {
        // The Tier 0/1/2 floor is always a candidate, so Best is never larger than
        // Balanced (`encode`) for any input.
        let cases: [(u32, u32, Vec<u32>); 3] = [
            (
                8,
                8,
                (0..64u32).map(|v| argb(255, v, v * 3, v * 7)).collect(),
            ),
            (16, 1, vec![argb(255, 5, 5, 5); 16]),
            (
                4,
                4,
                (0..16u32).map(|v| argb(255, v % 3, v % 3, v % 3)).collect(),
            ),
        ];
        for (w, h, pixels) in cases {
            assert!(encode_best(w, h, &pixels).len() <= encode(w, h, &pixels).len());
            assert_best_round_trip(w, h, &pixels);
        }
    }

    #[test]
    fn best_palette_beats_the_literal_baseline_on_a_small_palette() {
        // A 16x16 field of eight scattered colors. Palette coding packs 4-bit
        // indices two-per-byte (bits = 1) plus a tiny color map, which is far
        // smaller than coding every pixel's four channels as a literal.
        let colors = [
            argb(255, 10, 20, 30),
            argb(255, 200, 40, 60),
            argb(255, 70, 220, 90),
            argb(255, 100, 110, 240),
            argb(255, 33, 66, 99),
            argb(255, 240, 240, 10),
            argb(255, 15, 250, 250),
            argb(255, 250, 15, 130),
        ];
        let pixels: Vec<u32> = (0..256usize)
            .map(|i| colors[(i * 7 + i / 16) % colors.len()])
            .collect();
        let best = encode_best(16, 16, &pixels);
        let baseline = literal_baseline(16, 16, &pixels);
        assert!(
            best.len() < baseline.len(),
            "palette Best must beat the literal baseline: {} vs {}",
            best.len(),
            baseline.len()
        );
        assert_best_round_trip(16, 16, &pixels);
    }

    #[test]
    fn best_predictor_shrinks_a_smooth_gradient() {
        // A smooth 16x16 planar gradient `v = (x + y) * 8`: the gradient predictor
        // (mode 12) predicts every interior pixel exactly, collapsing the residual
        // to zero, which the subtract-green / LZ77 Balanced floor cannot match.
        let pixels: Vec<u32> = (0..256u32)
            .map(|i| {
                let v = (i % 16 + i / 16) * 8;
                argb(255, v, v, v)
            })
            .collect();
        let best = encode_best(16, 16, &pixels);
        let balanced = encode(16, 16, &pixels);
        assert!(
            best.len() < balanced.len(),
            "predictor Best must shrink a smooth gradient below Balanced: {} vs {}",
            best.len(),
            balanced.len()
        );
        assert_best_round_trip(16, 16, &pixels);
    }

    #[test]
    fn best_cross_color_helps_a_correlated_image() {
        // Green ramps 0..126 (kept below 128 so the signed multiplier stays
        // positive), with red = green / 2 and blue = green / 4. Cross-color cancels
        // red and blue to zero (green_to_red = 16, green_to_blue = 8), leaving only
        // green to code — smaller than the subtract-green Balanced floor, which
        // instead spreads green's variation into red and blue.
        let pixels: Vec<u32> = (0..64u32)
            .map(|i| {
                let g = i * 2;
                argb(255, g >> 1, g, g >> 2)
            })
            .collect();
        let best = encode_best(8, 8, &pixels);
        let balanced = encode(8, 8, &pixels);
        assert!(
            best.len() < balanced.len(),
            "cross-color Best must beat Balanced on a correlated image: {} vs {}",
            best.len(),
            balanced.len()
        );
        assert_best_round_trip(8, 8, &pixels);
    }

    /// A 16x16 image whose top half is red-dominant and bottom half blue-dominant
    /// yields >=2 Huffman groups; `emit_stream_meta` round-trips it exactly.
    #[test]
    fn emit_meta_two_groups_round_trips() {
        let mut pixels = Vec::with_capacity(256);
        for y in 0..16u32 {
            for x in 0..16u32 {
                pixels.push(if y < 8 {
                    argb(255, (x * 16 + y) & 0xff, x & 7, 0)
                } else {
                    argb(255, 0, x & 7, (x * 16 + (y - 8)) & 0xff)
                });
            }
        }
        let tokens = crate::lossless::vp8l::backref::parse(&pixels, true);
        let plan =
            meta::plan(&tokens, &pixels, 16, 16, 0).expect("regional image should plan >=2 groups");
        assert!(
            plan.group_histograms.len() >= 2,
            "expected >=2 groups, got {}",
            plan.group_histograms.len()
        );
        let model = RefModel::new(&tokens, &pixels, 16);
        let bytes = emit_stream_meta(16, 16, alpha_used(&pixels), &[], 0, &model, &plan);
        let decoded = decode(&bytes).expect("meta stream must decode");
        assert_eq!((decoded.width, decoded.height), (16, 16));
        assert_eq!(decoded.argb.as_slice(), pixels.as_slice());
    }

    #[test]
    fn alpha_stream_round_trips_headerless() {
        // A headerless VP8L alpha stream carries the plane in the green lane and must
        // decode byte-for-byte through `decode_alpha_stream`, which drives the
        // image-stream decoder directly (no 5-byte header) and extracts the green
        // lane. Covers a flat run, a ramp, and a scattered pattern.
        use crate::lossless::vp8l::decode::decode_alpha_stream;
        let cases: [(u32, u32, Vec<u8>); 3] = [
            (4, 4, vec![0x80u8; 16]),
            (8, 2, (0..16u32).map(|v| (v * 15) as u8).collect()),
            (
                5,
                5,
                (0..25u32)
                    .map(|v| (v.wrapping_mul(37) ^ 0x5a) as u8)
                    .collect(),
            ),
        ];
        for (w, h, alpha) in cases {
            let stream = super::encode_alpha_stream(&alpha, w, h);
            let decoded = decode_alpha_stream(&stream, w, h).expect("alpha stream must decode");
            assert_eq!(decoded, alpha, "{w}x{h} alpha round-trip");
        }
    }

    #[test]
    fn encode_best_output_is_byte_stable() {
        // Golden FNV-1a-64 of `encode_best`'s exact output — the full Tier-3 search
        // (breadth level 9). This pins the predictor/cross-color/palette forward
        // transforms, the subtract-green→predictor→cross-color composition, and
        // their (integer, deterministic) selection heuristics against silent byte
        // drift. The scattered case exercises the palette family, the gradient the
        // predictor family.
        const EXPECTED: [u64; 4] = [
            0xaa74_a8dc_ab4f_7a97,
            0xda6c_1b70_8587_811d,
            0xa302_bc55_3a2a_6980,
            0xa5c5_1ec6_3637_64c7,
        ];
        let got: [u64; 4] =
            byte_stability_cases().map(|(w, h, pixels)| fnv1a64(&encode_best(w, h, &pixels)));
        assert_eq!(
            got, EXPECTED,
            "encode_best output bytes drifted from the committed golden"
        );
    }

    #[test]
    fn level_nine_is_full_breadth() {
        // The breadth schedule must saturate at level 9: `encode_best_at(9)` is the
        // full transform search, byte-for-byte the historical `encode_best`. The
        // level-9 default is the golden the whole effort collapse rests on, so pin it
        // against the committed `encode_best` output directly.
        for (w, h, pixels) in byte_stability_cases() {
            assert_eq!(
                encode_best_at(MAX_LEVEL, w, h, &pixels),
                encode_best(w, h, &pixels),
                "level 9 must reproduce encode_best byte-for-byte ({w}x{h})"
            );
        }
    }

    #[test]
    fn encode_best_at_is_monotone_and_floored() {
        // Higher levels only ADD candidates, so the chosen size is non-increasing in
        // the level; and every level keeps the transform-free floor, so no level is
        // larger than `encode`. Both hold for structured and noisy inputs, and every
        // level round-trips.
        use crate::lossless::vp8l::decode::decode;
        let cases: [(u32, u32, Vec<u32>); 3] = [
            // A smooth gradient the predictor collapses.
            (
                16,
                16,
                (0..256u32)
                    .map(|i| {
                        let v = (i % 16 + i / 16) * 8;
                        argb(255, v, v, v)
                    })
                    .collect(),
            ),
            // A scattered small palette.
            (
                16,
                16,
                (0..256usize)
                    .map(|i| {
                        let c = (i * 7 + i / 16) as u32 % 6;
                        argb(255, c * 20 + 8, c * 13 + 17, c * 7 + 29)
                    })
                    .collect(),
            ),
            // Near-noise: the floor wins, so every level ties at `encode`.
            (
                12,
                12,
                (0..144u32)
                    .map(|i| {
                        let s = i.wrapping_mul(2_654_435_761);
                        argb(255, s & 0xff, (s >> 8) & 0xff, (s >> 16) & 0xff)
                    })
                    .collect(),
            ),
        ];
        for (w, h, pixels) in cases {
            let floor = encode(w, h, &pixels).len();
            let mut prev = usize::MAX;
            for level in 0..=MAX_LEVEL {
                let bytes = encode_best_at(level, w, h, &pixels);
                assert!(
                    bytes.len() <= prev,
                    "level {level} ({}) must not exceed level {} ({prev}) on {w}x{h}",
                    bytes.len(),
                    level.wrapping_sub(1)
                );
                assert!(
                    bytes.len() <= floor,
                    "level {level} ({}) must not exceed the floor ({floor}) on {w}x{h}",
                    bytes.len()
                );
                let decoded = decode(&bytes).expect("every level must decode");
                assert_eq!(decoded.argb.as_slice(), pixels.as_slice());
                prev = bytes.len();
            }
        }
    }

    #[test]
    fn task_minima_fold_is_index_canonical() {
        // Folding each family to its own minimum (`keep_smallest`) and then across
        // families (`reduce_task_minima`) reproduces the flat fold exactly: ties
        // keep the earliest stream within a family and the earliest family across
        // them, a strictly shorter later candidate wins, and an empty family folds
        // to `None` and is skipped. The family shapes mirror `run_best_task` output.
        use super::{keep_smallest, reduce_task_minima};
        // Within-family tie -> earliest stream.
        assert_eq!(
            keep_smallest(vec![vec![3u8; 4], vec![4u8; 4]]),
            Some(vec![3u8; 4])
        );
        // An empty family (e.g. no palette on a > 256-color image) folds to None.
        assert_eq!(keep_smallest(Vec::new()), None);
        let families: Vec<Vec<Vec<u8>>> = vec![
            vec![vec![0u8; 5], vec![1u8; 5]], // family min = [0; 5] (tie -> first)
            Vec::new(),                       // empty family, skipped
            vec![vec![2u8; 5]],
            vec![vec![9u8; 3]], // strictly smaller -> global winner
        ];
        let minima: Vec<Option<Vec<u8>>> = families.into_iter().map(keep_smallest).collect();
        assert_eq!(minima[1], None);
        assert_eq!(reduce_task_minima(minima), vec![9u8; 3]);
        // Cross-family tie resolves to the earliest family's minimum.
        let ties: Vec<Vec<Vec<u8>>> = vec![vec![vec![7u8; 4]], vec![vec![8u8; 4]]];
        let ties_minima: Vec<Option<Vec<u8>>> = ties.into_iter().map(keep_smallest).collect();
        assert_eq!(reduce_task_minima(ties_minima), vec![7u8; 4]);
    }

    #[test]
    fn transform_selection_keeps_family_best_and_respects_bands() {
        // The keep mask always retains Floor/Palette and each family's
        // est-best task when it stands within ~1.5x of the global best
        // estimate, keeps near-ties within the family/global bands, and
        // caps each family at FAMILY_CAP.
        use super::{BestTask, select_from_estimates, FAMILY_CAP};
        let mut tasks = vec![
            BestTask::Floor,
            BestTask::Palette,
            BestTask::Predictor(4),
            BestTask::Predictor(5),
            BestTask::CrossColor(4),
            BestTask::CrossColor(5),
        ];
        let mut estimates = vec![
            None,
            Some(100_000),
            Some(40_000),
            Some(41_600),
            Some(500_000),
            Some(600_000),
        ];
        for k in 0..(FAMILY_CAP + 1) {
            tasks.push(BestTask::PredictorCross(4, 4 + k as u32));
            estimates.push(Some(30_000 + k as u64 * 400));
        }
        let keep = select_from_estimates(&tasks, &estimates, true);
        assert!(keep[0] && keep[1], "floor and palette always run");
        assert!(keep[2] && keep[3], "predictor near-ties stay inside the band");
        assert!(
            !keep[4] && !keep[5],
            "a family best beyond ~1.5x of the global best is pruned with its near-ties"
        );
        let pc_kept = keep[tasks.len() - FAMILY_CAP - 1..]
            .iter()
            .filter(|&&k| k)
            .count();
        assert_eq!(pc_kept, FAMILY_CAP, "the family cap bounds near-ties");
        assert!(
            !keep[tasks.len() - 1],
            "the (cap + 1)-th near-tie is pruned"
        );
    }

    #[cfg(feature = "rayon")]
    #[test]
    fn serial_and_parallel_evaluation_agree() {
        use super::{
            CrossColorCache, PixelSource, PlaneCache, build_best_tasks, encode_best,
            estimate_specs_scored, estimate_task, evaluate_best_tasks, final_keep, keep_smallest,
            provisional_keep, reduce_task_minima, run_best_task, schedule,
        };
        let breadth = schedule(crate::effort::MAX_LEVEL);
        for (w, h, pixels) in byte_stability_cases() {
            let alpha = pixels.iter().any(|&p| p >> 24 != 0xff);
            // Independent serial reference: the same provisional/estimate/keep
            // pipeline the evaluator computes, folding each kept family's own
            // minimum in task order.
            let planes = PlaneCache::build(breadth, w, h, &pixels);
            let tasks = build_best_tasks(breadth);
            let cc = CrossColorCache::new(&tasks, pixels.len());
            let need_cheap = pixels.len() >= super::PRUNE_MIN_PIXELS
                && super::breadth_task_sets(crate::effort::MAX_LEVEL, pixels.len() as u64)
                    .iter()
                    .any(|set| super::provisional_prunes(set));
            let cheap: Vec<Option<u64>> = if need_cheap {
                tasks
                    .iter()
                    .map(|t| estimate_task(*t, w, h, &pixels, planes.as_ref(), None, true))
                    .collect()
            } else {
                Vec::new()
            };
            let prov = provisional_keep(&tasks, crate::effort::MAX_LEVEL, pixels.len(), &cheap);
            let spec_pairs: Vec<Option<(Vec<u64>, Vec<u64>)>> = tasks
                .iter()
                .zip(&prov)
                .map(|(t, &p)| {
                    if !p {
                        return None;
                    }
                    estimate_specs_scored(*t, w, h, &pixels, planes.as_ref(), cc.as_ref(), need_cheap)
                })
                .collect();
            let (spec_ests, spec_covs): (SpecScores, SpecScores) = spec_pairs
                .into_iter()
                .map(|o| match o {
                    Some((ents, covs)) => (Some(ents), need_cheap.then_some(covs)),
                    None => (None, None),
                })
                .unzip();
            let (keep, spec_masks) = final_keep(
                &tasks,
                crate::effort::MAX_LEVEL,
                pixels.len(),
                &cheap,
                &spec_ests,
                &spec_covs,
            );
            let serial = reduce_task_minima(
                tasks
                    .into_iter()
                    .zip(keep)
                    .zip(spec_masks)
                    .map(|((t, k), mask)| {
                        if k {
                            keep_smallest(run_best_task(
                                t,
                                breadth,
                                PixelSource {
                                    width: w,
                                    height: h,
                                    argb: &pixels,
                                },
                                alpha,
                                planes.as_ref(),
                                cc.as_ref(),
                                &mask,
                            ))
                        } else {
                            None
                        }
                    }),
            );
            // The rayon evaluator (feature on) must match the serial reference and
            // the public entry point, byte-for-byte.
            let parallel =
                evaluate_best_tasks(build_best_tasks(breadth), breadth, w, h, alpha, &pixels, crate::effort::MAX_LEVEL);
            assert_eq!(
                serial, parallel,
                "rayon evaluation must equal serial, byte-for-byte"
            );
            assert_eq!(parallel, encode_best(w, h, &pixels));
        }
    }

    /// FNV-1a-64 of `bytes` — integer-only and deterministic, mirroring the
    /// `xtask corpus-sweep` byte oracle. Used to pin the encoder's exact output.
    fn fnv1a64(bytes: &[u8]) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for &b in bytes {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x100_0000_01b3);
        }
        h
    }

    /// The four canonical byte-stability inputs, one per encoder path: a solid
    /// block (literal), a gradient (subtract-green), a scattered 12-color field
    /// (color cache), and a repeating 4-color tile (LZ77). Each is an input other
    /// tests already prove round-trips, so this only pins the exact bytes.
    fn byte_stability_cases() -> [(u32, u32, Vec<u32>); 4] {
        let solid = vec![argb(255, 10, 20, 30); 16];
        let gradient: Vec<u32> = (0..64u32).map(|v| argb(255, v * 2, v, 255 - v)).collect();
        let scattered: Vec<u32> = (0..1024u32)
            .map(|i| {
                let (x, y) = (i % 32, i / 32);
                let c = (x * 5 + y * 11) % 12;
                argb(255, c * 20 + 8, c * 13 + 17, c * 7 + 29)
            })
            .collect();
        let tile = [
            argb(255, 10, 20, 30),
            argb(255, 40, 50, 60),
            argb(255, 70, 80, 90),
            argb(255, 100, 110, 120),
        ];
        let repeating: Vec<u32> = tile.iter().cycle().take(256).copied().collect();
        [
            (4, 4, solid),
            (8, 8, gradient),
            (32, 32, scattered),
            (64, 4, repeating),
        ]
    }

    #[test]
    fn search_lz77_returns_the_literal_cache_winner() {
        // A scattered 16-color field (a PRNG over a fixed palette): color-cache-
        // coding every pixel as a literal — the colors are all frequent, so each
        // gets a short cache code — strictly beats both the greedy-LZ77 and
        // optimal-DP candidates, whose cache-unaware matches cost more than the
        // cache hits they displace. The winning stream is therefore exactly the
        // literal model's cache candidate, produced ONLY by the `if cache_literal >
        // 0` branch (both in `search_lz77` and `search_lz77_best`). Mutating that
        // guard to `==`/`<` stops the branch firing and the search falls back to the
        // strictly larger LZ77/DP candidate, breaking the exact-bytes assertions.
        use super::{search_lz77, search_lz77_best};
        use crate::lossless::vp8l::backref::{parse_lz77, parse_optimal};
        let mut state = 3u64 | 1;
        let pixels: Vec<u32> = (0..256usize)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                let c = (state >> 40) as u32 % 16;
                argb(255, c * 20 + 8, c * 13 + 17, c * 7 + 29)
            })
            .collect();
        let (w, h) = (16u32, 16u32);
        let au = alpha_used(&pixels);
        let tokens_lit = parse(&pixels, false);
        let model_lit = RefModel::new(&tokens_lit, &pixels, w);
        let (bits, hist) = best_cache_bits(&model_lit);
        assert!(bits > 0, "the cost model must enable a cache here");
        let lit_cache = emit_stream(w, h, au, &[], bits, &model_lit, &hist);
        // The cache genuinely helps: the literal @cache candidate is smaller than
        // the literal @0 baseline the mutated guard would leave as `best`.
        let lit_zero = emit_stream(w, h, au, &[], 0, &model_lit, &model_lit.histogram(0));
        assert!(
            lit_cache.len() < lit_zero.len(),
            "the cache must shrink the stream"
        );
        // ...and both the greedy-LZ77 and optimal-DP candidates are strictly larger,
        // so the literal-cache candidate is the UNIQUE winner: no other branch of
        // the search reproduces its exact bytes.
        let (tokens_lz, chain) = parse_lz77(&pixels);
        let model_lz = RefModel::new(&tokens_lz, &pixels, w);
        let (b_lz, h_lz) = best_cache_bits(&model_lz);
        assert!(emit_stream(w, h, au, &[], b_lz, &model_lz, &h_lz).len() > lit_cache.len());
        let tokens_dp = parse_optimal(&pixels, w, &tokens_lz, &chain, MAX_CHAIN_OPTIMAL);
        let model_dp = RefModel::new(&tokens_dp, &pixels, w);
        let (b_dp, h_dp) = best_cache_bits(&model_dp);
        assert!(emit_stream(w, h, au, &[], b_dp, &model_dp, &h_dp).len() > lit_cache.len());
        // Both the Balanced (`search_lz77`) and Best (`search_lz77_best`) searches
        // must therefore return exactly the literal-cache candidate.
        assert_eq!(search_lz77(w, h, au, &[], w, &pixels, MAX_CHAIN_OPTIMAL), lit_cache);
        assert_eq!(search_lz77_best(w, h, au, &[], w, &pixels), lit_cache);
        assert_round_trip(w, h, &pixels);
    }

    #[test]
    fn search_lz77_best_meta_shot_shrinks_a_bimodal_image() {
        // A 64x32 image whose top half is a single flat color and whose bottom half
        // is a high-entropy multi-channel field: two region-specialized Huffman
        // groups beat one shared code by a wide margin, so the meta-Huffman
        // candidate is the strict winner of `search_lz77_best`. That shot is only
        // reached because `ysize = pixels.len() / working_width` recovers the true
        // height; mutating `/` (to `%` -> 0, or `*` -> a huge value) makes
        // `meta::plan` see a degenerate geometry and return `None`, dropping the
        // winning candidate so the result stops beating the meta-free `search_lz77`.
        use super::{search_lz77, search_lz77_best};
        let (w, h) = (64u32, 32u32);
        let mut pixels = Vec::with_capacity((w * h) as usize);
        for y in 0..h {
            for x in 0..w {
                pixels.push(if y < h / 2 {
                    argb(255, 10, 20, 30)
                } else {
                    argb(255, (x * 3) & 0xff, (x * 7 + y) & 0xff, (y * 5) & 0xff)
                });
            }
        }
        let au = alpha_used(&pixels);
        let with_meta = search_lz77_best(w, h, au, &[], w, &pixels);
        let no_meta = search_lz77(w, h, au, &[], w, &pixels, MAX_CHAIN_OPTIMAL);
        assert!(
            with_meta.len() < no_meta.len(),
            "meta-Huffman must shrink the bimodal image: {} vs {}",
            with_meta.len(),
            no_meta.len()
        );
        let decoded = decode(&with_meta).expect("meta-winning stream must decode");
        assert_eq!((decoded.width, decoded.height), (w, h));
        assert_eq!(decoded.argb.as_slice(), pixels.as_slice());
    }

    #[test]
    fn emit_meta_varying_alpha_round_trips() {
        // Like `emit_meta_two_groups_round_trips`, but every pixel carries a
        // different alpha value, so the alpha channel is genuinely multi-symbol
        // within each group. That makes the meta-emit's alpha extraction
        // `((argb >> 24) & 0xff)` load-bearing: corrupting the shift (`>>`->`<<`
        // forces 0) or the mask (`&`->`|` forces 0xff, `&`->`^` flips the byte)
        // emits the wrong alpha symbol and the round-trip breaks.
        let (w, h) = (256u32, 256u32);
        let mut pixels = Vec::with_capacity((w * h) as usize);
        for y in 0..h {
            for x in 0..w {
                let a = 1 + ((x * 5 + y * 3) & 0x3f);
                pixels.push(if y < h / 2 {
                    argb(a, (x * 16 + y) & 0xff, x & 0x3f, 0)
                } else {
                    argb(a, 0, x & 0x3f, (x * 16 + (y - h / 2)) & 0xff)
                });
            }
        }
        // The alpha channel must actually vary, else the mutation would be invisible.
        assert!(pixels.iter().any(|&p| p >> 24 != pixels[0] >> 24));
        let tokens = crate::lossless::vp8l::backref::parse(&pixels, true);
        let plan =
            meta::plan(&tokens, &pixels, w, h, 0).expect("regional image should plan >=2 groups");
        assert!(plan.group_histograms.len() >= 2);
        let model = RefModel::new(&tokens, &pixels, w);
        let bytes = emit_stream_meta(w, h, alpha_used(&pixels), &[], 0, &model, &plan);
        let decoded = decode(&bytes).expect("meta stream must decode");
        assert_eq!((decoded.width, decoded.height), (w, h));
        assert_eq!(decoded.argb.as_slice(), pixels.as_slice());
    }

    #[test]
    fn emit_meta_cache_reference_uses_the_correct_symbol() {
        // The color-cache branch of `emit_tokens_meta` codes a cache reference as the
        // green symbol `NUM_LITERAL_CODES + NUM_LENGTH_CODES + key` (= 280 + key).
        // Flipping the first `+` to `-` emits `NUM_LITERAL_CODES - NUM_LENGTH_CODES +
        // key` (= 232 + key) — a green LITERAL symbol — so the decoder reads a
        // different unit and the stream stops round-tripping. Random images seldom
        // exercise the meta cache path, so pin it deterministically: a 256x256 image
        // whose top and bottom halves use DISJOINT palettes (>=2 Huffman groups), with
        // every color duplicated in place so an all-literal parse under an 8-bit color
        // cache scores a guaranteed hit on the second pixel of every pair. Encoded via
        // the meta path with that cache, an exact round-trip catches the wrong cache
        // symbol on every run, at any case count.
        use crate::lossless::vp8l::backref::{Resolved, resolve};
        let (w, h) = (256u32, 256u32);
        let top = [
            argb(255, 10, 20, 30),
            argb(255, 200, 40, 60),
            argb(255, 70, 220, 90),
            argb(255, 100, 110, 240),
        ];
        let bottom = [
            argb(255, 33, 66, 99),
            argb(255, 240, 240, 10),
            argb(255, 15, 250, 250),
            argb(255, 250, 15, 130),
        ];
        let mut pixels = Vec::with_capacity((w * h) as usize);
        for y in 0..h {
            for x in 0..w {
                let palette = if y < h / 2 { &top } else { &bottom };
                // Duplicated pairs (0,0,1,1,2,2,3,3,...): every odd x repeats its
                // predecessor, so an all-literal parse makes it a cache hit.
                pixels.push(palette[((x / 2) % 4) as usize]);
            }
        }
        let cache_bits = 8u32;
        // All-literal parse: adjacent duplicates become cache hits (an LZ77 parse would
        // fold them into a copy), so the cache branch of the meta emit is exercised.
        let tokens = parse(&pixels, false);
        // The cache branch must genuinely fire, else the mutation would be invisible.
        let mut cache_hit_count = 0usize;
        resolve(&tokens, &pixels, cache_bits, w, |_pos, unit| {
            if matches!(unit, Resolved::Cache(_)) {
                cache_hit_count += 1;
            }
        });
        assert!(
            cache_hit_count > 0,
            "the meta cache branch must be exercised"
        );
        let plan = meta::plan(&tokens, &pixels, w, h, cache_bits)
            .expect("two disjoint-palette halves must plan >=2 groups");
        assert!(
            plan.group_histograms.len() >= 2,
            "expected >=2 groups, got {}",
            plan.group_histograms.len()
        );
        let model = RefModel::new(&tokens, &pixels, w);
        let bytes = emit_stream_meta(w, h, alpha_used(&pixels), &[], cache_bits, &model, &plan);
        let decoded = decode(&bytes).expect("meta cache stream must decode");
        assert_eq!((decoded.width, decoded.height), (w, h));
        assert_eq!(decoded.argb.as_slice(), pixels.as_slice());
    }

    #[test]
    fn cross_color_streams_are_nonempty_and_round_trip() {
        // The cross-color family must yield its two ordered candidates (cross-color
        // alone, and cross-color + subtract-green), each a valid stream that decodes
        // back to the source. A function-replacement mutant returning `vec![]`
        // produces no candidates — caught by the length assert; the decodes pin that
        // both streams are correct.
        use super::{BestTask, PixelSource, run_best_task, schedule};
        let pixels: Vec<u32> = (0..64u32)
            .map(|i| {
                let g = i * 2;
                argb(255, g >> 1, g, g >> 2)
            })
            .collect();
        let streams = run_best_task(
            BestTask::CrossColor(3),
            schedule(crate::effort::MAX_LEVEL),
            PixelSource {
                width: 8,
                height: 8,
                argb: &pixels,
            },
            alpha_used(&pixels),
            None,
            None,
            &[],
        );
        assert_eq!(
            streams.len(),
            2,
            "cross-color family must emit two candidates"
        );
        for stream in &streams {
            let decoded = decode(stream).expect("each cross-color stream must decode");
            assert_eq!((decoded.width, decoded.height), (8, 8));
            assert_eq!(decoded.argb.as_slice(), pixels.as_slice());
        }
    }

    #[test]
    fn cross_color_transform_tile_bits_field_round_trips() {
        // A level-0 stream carrying a cross-color transform at tile bits = 2 over a
        // 32-wide image, so the tile grid is `subsample_size(32, 2) = 8` wide. The
        // field is written as `bits - MIN_TRANSFORM_BITS` (= 0) and the decoder
        // reads back `read_bits(3) + 2`. Corrupting the subtraction changes the
        // decoded tile bits and thus the grid width the decoder reads the sub-image
        // at (`-`->`+` writes 4 -> reads bits 6 -> width `subsample_size(32,6)=1`;
        // `-`->`/` writes 1 -> reads bits 3 -> width `subsample_size(32,3)=4`), so
        // the tile sub-image is misparsed and the round-trip fails.
        use crate::lossless::transform::cross_color;
        let width = 32u32;
        let height = 8u32;
        // Left half is red ~= green/2, right half is red ~= green: cross-color picks
        // a DIFFERENT per-tile multiplier for the two regions, so the tile sub-image
        // carries real (multi-symbol) pixel bits and its grid width is load-bearing.
        let source: Vec<u32> = (0..(width * height))
            .map(|i| {
                let x = i % width;
                let g = (i * 2) & 0x7f;
                if x < 16 {
                    argb(255, g >> 1, g, g >> 2)
                } else {
                    argb(255, g, g, 0)
                }
            })
            .collect();
        let bits = 2u32;
        let (stored, tile_data) = cross_color::forward(&source, width, height, bits);
        assert!(
            tile_data
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                > 1,
            "the tile grid must carry >1 distinct multiplier for the field to matter"
        );
        let tokens = parse(&stored, false);
        let model = RefModel::new(&tokens, &stored, width);
        let plans = [TransformPlan::CrossColor { bits, tile_data }];
        let stream = emit_stream(
            width,
            height,
            alpha_used(&source),
            &plans,
            0,
            &model,
            &model.histogram(0),
        );
        let decoded = decode(&stream).expect("a cross-color sub-image stream must decode");
        assert_eq!((decoded.width, decoded.height), (width, height));
        assert_eq!(decoded.argb, source);
    }

    #[test]
    fn encode_output_is_byte_stable() {
        // Golden FNV-1a-64 of `encode`'s exact output bytes, captured from the
        // pre-perf-refactor build. The RefModel / lazy-carry changes are designed
        // to be byte-invariant, so these must never move; a mismatch here
        // localizes an accidental output change faster than the full corpus sweep.
        const EXPECTED: [u64; 4] = [
            0xaa74_a8dc_ab4f_7a97,
            0x3178_9c56_b7ea_c5a9,
            0x0759_bd12_ffbe_1b95,
            0x4271_5c37_9803_15bd,
        ];
        let got: [u64; 4] =
            byte_stability_cases().map(|(w, h, pixels)| fnv1a64(&encode(w, h, &pixels)));
        assert_eq!(
            got, EXPECTED,
            "encode output bytes drifted from the committed golden"
        );
    }
}

#[cfg(test)]
mod proptests {
    use super::{
        BestTask, CrossColorCache, PixelSource, PlaneCache, RefModel, StreamSwitches,
        TransformPlan, best_cache_bits, emit_stream, emit_stream_meta, encode, encode_best,
        encode_best_at, encode_with, floor_streams, meta_shot, parse_lz77, run_best_task, schedule,
        search_lz77_variant, task_specs,
    };
    use crate::lossless::vp8l::backref::parse_optimal;
    use crate::effort::MAX_LEVEL;
    use crate::lossless::constants::{ALPHABET_SIZE, MAX_CACHE_BITS, MAX_CHAIN_OPTIMAL};
    use crate::lossless::histogram::Histogram;
    use crate::lossless::vp8l::backref::{Resolved, Token, parse, resolve};
    use crate::lossless::vp8l::decode::decode;
    use crate::lossless::vp8l::meta;
    use proptest::prelude::*;

    /// `alpha_used` as the encoder computes it (the header advisory bit).
    fn alpha_used(pixels: &[u32]) -> bool {
        pixels.iter().any(|&p| p >> 24 != 0xff)
    }

    /// A straightforward per-cache-size histogram builder: it walks every unit
    /// through `resolve`, recomputing each copy's prefix symbols. The production
    /// [`RefModel::histogram`] hoists that cache-independent work but must yield a
    /// bit-identical histogram; `ref_model_matches_accumulate` pins the identity.
    fn reference_accumulate(
        tokens: &[Token],
        pixels: &[u32],
        cache_bits: u32,
        width: u32,
    ) -> Histogram {
        let cache_codes = if cache_bits > 0 {
            1usize << cache_bits
        } else {
            0
        };
        let mut histogram = Histogram::new(ALPHABET_SIZE[0] + cache_codes);
        resolve(tokens, pixels, cache_bits, width, |_pos, unit| match unit {
            Resolved::Literal(argb) => histogram.add_literal(argb),
            Resolved::Copy {
                length_symbol,
                length_extra,
                dist_symbol,
                dist_extra,
            } => {
                histogram.add_length(length_symbol, length_extra.1);
                histogram.add_distance(dist_symbol, dist_extra.1);
            },
            Resolved::Cache(key) => histogram.add_cache(key),
        });
        histogram
    }

    /// Deterministic repetition-prone pixels from a seed (small palette keeps the
    /// LZ77 and color-cache paths well exercised).
    fn seeded_pixels(seed: u64, count: usize, palette: u32) -> Vec<u32> {
        let mut state = seed | 1;
        (0..count)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                (state >> 40) as u32 % palette
            })
            .collect()
    }

    /// One structure-bearing image per `variant`, so the Best proptest exercises
    /// the palette, predictor, and cross-color emits rather than random noise
    /// (where the floor always wins and no transform is emitted):
    /// `0` a scattered small palette, `1` a smooth grayscale gradient, `2` a
    /// green<->red/blue correlated ramp.
    fn structured_pixels(variant: u8, seed: u64, width: u32, height: u32) -> Vec<u32> {
        let count = (width * height) as usize;
        match variant % 3 {
            0 => {
                let palette = [
                    0xff10_2030u32,
                    0xff20_4030,
                    0xff70_a0c0,
                    0xffa0_3060,
                    0xff30_c090,
                    0xfff0_f010,
                ];
                seeded_pixels(seed, count, palette.len() as u32)
                    .into_iter()
                    .map(|idx| palette[idx as usize])
                    .collect()
            },
            1 => (0..count as u32)
                .map(|i| {
                    let v = ((i % width + i / width) * 4) & 0xff;
                    0xff00_0000 | (v << 16) | (v << 8) | v
                })
                .collect(),
            _ => (0..count as u32)
                .map(|i| {
                    // Keep green < 128 so the signed cross-color multiplier stays
                    // positive and the red = g/2, blue = g/4 correlation cancels.
                    let g = (i * 2) & 0x7f;
                    0xff00_0000 | ((g >> 1) << 16) | (g << 8) | (g >> 2)
                })
                .collect(),
        }
    }

    proptest! {
        /// Any small image survives a full encode -> decode round-trip
        /// unchanged; strategy, cache-size, and tier selection never break the
        /// identity. A small palette keeps LZ77 and cache paths well exercised.
        #[test]
        fn encode_decode_round_trip(
            width in 1u32..=16,
            height in 1u32..=16,
            seed in any::<u64>(),
            palette in 1u32..=8,
        ) {
            let count = (width * height) as usize;
            let pixels = seeded_pixels(seed, count, palette);

            let payload = encode(width, height, &pixels);
            let decoded = decode(&payload).expect("encoder must emit decodable VP8L");
            prop_assert_eq!(decoded.width, width);
            prop_assert_eq!(decoded.height, height);
            prop_assert_eq!(&decoded.argb, &pixels);

            // The `Fast` method (`use_lz77` cleared) skips the LZ77 / color-cache
            // search and emits only the literal + subtract-green tiers; it must
            // still be exactly lossless.
            let fast = encode_with(width, height, &pixels, false, MAX_CHAIN_OPTIMAL, false);
            let decoded_fast = decode(&fast).expect("Fast encoder must emit decodable VP8L");
            prop_assert_eq!(decoded_fast.width, width);
            prop_assert_eq!(decoded_fast.height, height);
            prop_assert_eq!(decoded_fast.argb, pixels);
        }

        /// The Tier 3 `Best` driver round-trips exactly over palette, gradient,
        /// and correlated images — the transform families it emits (color
        /// indexing, predictor, cross-color, each possibly with subtract-green)
        /// are all inverted losslessly by the decoder.
        #[test]
        fn encode_best_round_trip(
            width in 1u32..=16,
            height in 1u32..=16,
            seed in any::<u64>(),
            variant in 0u8..3,
        ) {
            let pixels = structured_pixels(variant, seed, width, height);
            let payload = encode_best(width, height, &pixels);
            let decoded = decode(&payload).expect("encode_best must emit decodable VP8L");
            prop_assert_eq!(decoded.width, width);
            prop_assert_eq!(decoded.height, height);
            prop_assert_eq!(decoded.argb, pixels);
        }

        /// encode_best is deterministic across repeated calls — a tautology
        /// serially, but under the rayon CI job it hammers the work-stealer.
        #[test]
        fn encode_best_is_repeatable(width in 1u32..=16, height in 1u32..=16, seed in any::<u64>(), variant in 0u8..3) {
            let pixels = structured_pixels(variant, seed, width, height);
            let a = encode_best(width, height, &pixels);
            for _ in 0..3 {
                prop_assert_eq!(&encode_best(width, height, &pixels), &a);
            }
        }

        /// Representative breadth levels round-trip exactly, stay deterministic, and
        /// never exceed the transform-free floor — the non-regression the breadth
        /// schedule owes to `keep_smaller` (its monotonicity across all ten levels is
        /// pinned deterministically by `encode_best_at_is_monotone_and_floored`).
        #[test]
        fn encode_best_at_round_trips_and_floors_every_level(
            width in 1u32..=16, height in 1u32..=16, seed in any::<u64>(), variant in 0u8..3,
        ) {
            let pixels = structured_pixels(variant, seed, width, height);
            let floor = encode(width, height, &pixels).len();
            for level in [0u8, MAX_LEVEL / 2, MAX_LEVEL] {
                let bytes = encode_best_at(level, width, height, &pixels);
                prop_assert!(bytes.len() <= floor, "level {} exceeded the floor", level);
                prop_assert_eq!(&encode_best_at(level, width, height, &pixels), &bytes);
                let decoded = decode(&bytes).expect("every level must decode");
                prop_assert_eq!(decoded.width, width);
                prop_assert_eq!(decoded.height, height);
                prop_assert_eq!(decoded.argb, pixels.clone());
            }
        }

        /// Whenever the planner finds a grouping, the multi-group emit decodes back
        /// to the source — the meta emit is correct for every plan that arises.
        #[test]
        fn meta_emit_decodes_to_source(
            width in 2u32..=16, height in 2u32..=16, seed in any::<u64>(), variant in 0u8..3,
        ) {
            let pixels = structured_pixels(variant, seed, width, height);
            let tokens = crate::lossless::vp8l::backref::parse(&pixels, true);
            if let Some(plan) = meta::plan(&tokens, &pixels, width, height, 0) {
                let model = RefModel::new(&tokens, &pixels, width);
                let bytes = emit_stream_meta(width, height, alpha_used(&pixels), &[], 0, &model, &plan);
                let decoded = decode(&bytes).expect("meta emit must decode");
                prop_assert_eq!(decoded.width, width);
                prop_assert_eq!(decoded.height, height);
                prop_assert_eq!(decoded.argb, pixels);
            }
        }

        /// `RefModel::histogram` (hoisted cache-independent bins) is bit-identical
        /// to a `resolve`-driven `accumulate` at every cache size, for both parse
        /// strategies — the mechanical proof the hoist is behavior-neutral, so the
        /// histogram (hence the emitted bytes) cannot move.
        #[test]
        fn ref_model_matches_accumulate(
            width in 1u32..=16,
            seed in any::<u64>(),
            len in 1usize..=200,
            palette in 1u32..=8,
            use_lz77 in any::<bool>(),
        ) {
            let pixels = seeded_pixels(seed, len, palette);
            let tokens = parse(&pixels, use_lz77);
            let model = RefModel::new(&tokens, &pixels, width);
            for cache_bits in 0..=MAX_CACHE_BITS {
                prop_assert_eq!(
                    model.histogram(cache_bits),
                    reference_accumulate(&tokens, &pixels, cache_bits, width),
                    "histogram mismatch at cache_bits={}",
                    cache_bits
                );
            }
        }

        /// The buffer-reusing `best_cache_bits` chooses exactly the size a fresh
        /// per-size histogram sweep would. The reused scratch is a max-size
        /// zero-padded histogram, and `estimate_bits` skips zero-count bins, so the
        /// estimate (hence the selected cache size, hence the emitted bytes) is
        /// unchanged by the allocation-eliding refactor.
        #[test]
        fn best_cache_bits_reuse_matches_fresh(
            width in 1u32..=16,
            seed in any::<u64>(),
            len in 1usize..=200,
            palette in 1u32..=8,
            use_lz77 in any::<bool>(),
        ) {
            let pixels = seeded_pixels(seed, len, palette);
            let tokens = parse(&pixels, use_lz77);
            let model = RefModel::new(&tokens, &pixels, width);
            // Fresh per-size sweep: a freshly allocated histogram each iteration.
            let mut fresh_bits = 0;
            let mut best = model.histogram(0).estimate_bits();
            for bits in 1..=MAX_CACHE_BITS {
                let cost = model.histogram(bits).estimate_bits();
                if cost < best {
                    best = cost;
                    fresh_bits = bits;
                }
            }
            prop_assert_eq!(best_cache_bits(&model).0, fresh_bits);
        }

        /// The histogram `best_cache_bits` hands back (a truncated snapshot of the
        /// reused max-size scratch at the winning size) is bin-identical to a fresh
        /// `RefModel::histogram` at that same size — so [`emit_coded_pixels`]
        /// reusing it instead of rebuilding cannot move a single emitted byte.
        #[test]
        fn best_cache_bits_histogram_matches_fresh(
            width in 1u32..=16,
            seed in any::<u64>(),
            len in 1usize..=200,
            palette in 1u32..=8,
            use_lz77 in any::<bool>(),
        ) {
            let pixels = seeded_pixels(seed, len, palette);
            let tokens = parse(&pixels, use_lz77);
            let model = RefModel::new(&tokens, &pixels, width);
            let (bits, hist) = best_cache_bits(&model);
            prop_assert_eq!(hist, model.histogram(bits));
        }
    }

    #[test]
    fn dump_task_bytes() {
        let Ok(path) = std::env::var("WEBPKIT_DUMP") else {
            return;
        };
        let bytes = std::fs::read(&path).unwrap();
        let w = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let h = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        let rgba = &bytes[8..];
        let argb: Vec<u32> = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| {
                u32::from(p[3]) << 24
                    | u32::from(p[0]) << 16
                    | u32::from(p[1]) << 8
                    | u32::from(p[2])
            })
            .collect();
        let breadth = schedule(9);
        let planes = PlaneCache::build(breadth, w, h, &argb);
        let tasks = [
            BestTask::Predictor(2),
            BestTask::Predictor(3),
            BestTask::Predictor(4),
            BestTask::Predictor(5),
            BestTask::CrossColor(3),
            BestTask::CrossColor(4),
            BestTask::CrossColor(5),
            BestTask::PredictorCross(3, 4),
            BestTask::PredictorCross(4, 4),
            BestTask::PredictorCross(5, 4),
        ];
        let cc = CrossColorCache::new(&tasks, argb.len());
        let alpha_used = argb.iter().any(|&p| p >> 24 != 0xff);
        for task in tasks {
            let streams = run_best_task(
                task,
                breadth,
                PixelSource {
                    width: w,
                    height: h,
                    argb: &argb,
                },
                alpha_used,
                planes.as_ref(),
                cc.as_ref(),
                &[true],
            );
            let lens: Vec<usize> = streams.iter().map(Vec::len).collect();
            println!("{task:?}: {lens:?}");
        }
        let floor = floor_streams(breadth, w, h, alpha_used, &argb);
        println!("floor: {:?}", floor.iter().map(Vec::len).collect::<Vec<_>>());
    }

    #[test]
    fn dump_alpha_stream_bytes() {
        let Ok(path) = std::env::var("WEBPKIT_DUMP") else {
            return;
        };
        let bytes = std::fs::read(&path).unwrap();
        let w = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let h = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        let rgba = &bytes[8..];
        // Alpha plane as the green lane, exactly like encode_alpha_stream.
        let alpha: Vec<u8> = rgba.as_chunks::<4>().0.iter().map(|p| p[3]).collect();
        let argb: Vec<u32> = alpha.iter().map(|&a| u32::from(a) << 8).collect();
        let au = argb.iter().any(|&p| p >> 24 != 0xff);
        let current = encode_with(w, h, &argb, true, MAX_CHAIN_OPTIMAL, false).len();
        let mut sg = argb.clone();
        crate::lossless::transform::subtract_green::forward(&mut sg);
        let sg_plan = [TransformPlan::SubtractGreen];
        println!("alpha stream {}x{}: current={}", w, h, current);
        for (name, meta, fine, reparse) in [
            ("meta", true, false, false),
            ("meta-fine", true, true, false),
        ] {
            let t = std::time::Instant::now();
            let t0 = search_lz77_variant(w, h, au, &[], w, &argb, true, meta, fine, reparse, MAX_CHAIN_OPTIMAL).1.len();
            let t1 = search_lz77_variant(w, h, au, &sg_plan, w, &sg, true, meta, fine, reparse, MAX_CHAIN_OPTIMAL).1.len();
            println!("  {name}: t0={t0} t1={t1} min={} ({:.1}s)", t0.min(t1), t.elapsed().as_secs_f64());
        }
        let t = std::time::Instant::now();
        let _cur = encode_with(w, h, &argb, true, MAX_CHAIN_OPTIMAL, false);
        println!("  current-rerun: {:.1}s", t.elapsed().as_secs_f64());
        let _ = alpha;
    }

    #[test]
    fn dump_reparse_bytes() {
        let Ok(path) = std::env::var("WEBPKIT_DUMP") else {
            return;
        };
        let bytes = std::fs::read(&path).unwrap();
        let w = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let h = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        let rgba = &bytes[8..];
        let argb: Vec<u32> = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| {
                u32::from(p[3]) << 24
                    | u32::from(p[0]) << 16
                    | u32::from(p[1]) << 8
                    | u32::from(p[2])
            })
            .collect();
        let breadth = schedule(9);
        let planes = PlaneCache::build(breadth, w, h, &argb);
        let tasks = [BestTask::Predictor(3), BestTask::Predictor(5)];
        let cc = CrossColorCache::new(&tasks, argb.len());
        let alpha_used = argb.iter().any(|&p| p >> 24 != 0xff);
        for task in tasks {
            for spec in task_specs(task, w, h, &argb, planes.as_ref(), cc.as_ref(), false) {
                let pixels = &spec.pixels;
                let ww = spec.working_width;
                let ysize = pixels.len() as u32 / ww;
                let (tokens_lz77, chain) = parse_lz77(pixels);
                let cap = crate::lossless::constants::MAX_CHAIN_OPTIMAL;
                let mut passes: Vec<(&'static str, Vec<Token>)> = vec![("lz77", tokens_lz77.clone())];
                let mut cur = tokens_lz77;
                for k in 1..=4 {
                    cur = parse_optimal(pixels, ww, &cur, &chain, cap);
                    passes.push((match k { 1 => "dp1", 2 => "dp2", 3 => "dp3", _ => "dp4" }, cur.clone()));
                }
                for (name, tokens) in &passes {
                    let model = RefModel::new(tokens, pixels, ww);
                    let (cache, hist) = best_cache_bits(&model);
                    let mut best = emit_stream(w, h, alpha_used, &spec.plans, cache, &model, &hist);
                    let plain0 = emit_stream(w, h, alpha_used, &spec.plans, 0, &model, &model.histogram(0));
                    if plain0.len() < best.len() {
                        best = plain0;
                    }
                    meta_shot(
                        &mut best, w, h, alpha_used, &spec.plans, cache, &model, true,
                        pixels, ww, ysize,
                    );
                    println!("{task:?} spec: {name} -> {} B", best.len());
                }
            }
        }
    }
}

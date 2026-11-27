//! The crate's single `unsafe` module (owner grant, webp hillclimb 2026-11-15):
//! an aarch64 NEON first-mismatch kernel for the LZ77 match-length compare that
//! dominates `parse_optimal`'s wall on match-dense images.
//!
//! Byte-exactness contract: [`match_length_neon`] returns the identical
//! first-differing index the scalar `while len < max_len && pixels[src + len] ==
//! pixels[dst + len]` walk produces — only the step width differs. `unsafe` never
//! leaves this file; every other module stays under `#![deny(unsafe_code)]`.

#![allow(unsafe_code)]

use core::arch::aarch64::{veorq_u32, vgetq_lane_u32, vld1q_u32, vmaxvq_u32};

/// Length of the exact match of the pixels at `dst` against those at `src`
/// (`src < dst`), capped at `max_len` — the NEON-wide equivalent of the scalar
/// walk in `backref::match_length`.
///
/// # Safety-relevant contract ( upheld by the sole caller, `backref` )
///
/// `src < dst` and `dst + max_len <= pixels.len()`, hence `src + max_len <
/// pixels.len()`. Every `vld1q_u32` below loads four `u32` starting at
/// `base + len` with `len + 4 <= max_len`, so both loads stay inside `pixels`.
/// `vgetq_lane_u32` only ever indexes lanes 0..4 of a live vector.
pub(super) fn match_length_neon(pixels: &[u32], src: usize, dst: usize, max_len: usize) -> usize {
    // SAFETY: see the contract above — all vector loads are in bounds by the
    // `len + 4 <= max_len` loop guard plus the caller's `dst + max_len <= len`.
    unsafe {
        let p = pixels.as_ptr();
        let mut len = 0usize;
        while len + 4 <= max_len {
            let a = vld1q_u32(p.add(src + len));
            let b = vld1q_u32(p.add(dst + len));
            // `vmaxvq_u32(veorq)` is nonzero iff any of the four lanes differ.
            if vmaxvq_u32(veorq_u32(a, b)) != 0 {
                // `vgetq_lane_u32` takes a const lane index — unroll the four.
                macro_rules! lane_diff {
                    ($k:literal) => {
                        if vgetq_lane_u32(a, $k) != vgetq_lane_u32(b, $k) {
                            return len + $k;
                        }
                    };
                }
                lane_diff!(0);
                lane_diff!(1);
                lane_diff!(2);
                lane_diff!(3);
                // Unreachable: `vmaxvq` saw a differing lane, so one must return.
                // Defensive fallthrough, never a wrong length.
                return len;
            }
            len += 4;
        }
        // Scalar tail for the last `max_len % 4` positions (and all of
        // `max_len < 4`); identical comparisons to the scalar walk.
        while len < max_len && pixels[src + len] == pixels[dst + len] {
            len += 1;
        }
        len
    }
}

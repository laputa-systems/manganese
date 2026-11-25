# zlib-rs snapshot

Repository: https://github.com/laputa-systems/zlib-rs
Base: 496c33cd8c5f3fa2ef688498533981d22a07593f
Branch: manganese/chromium-deflate
Pushed commit: cf79d59 (branch pushed to the fork, 2026-11-25).
Patch: opt-in `chromium-compat` feature: Chromium's level table, ANZAC hash with a 15-bit minimum mask, three-byte matches with TOO_FAR, madler's deflate_fast for levels 1 to 3, and Chromium's deflate_rle for Z_RLE. Inflate preserves the original error message when the decoder reaches its bad state.

The snapshot contains the `zlib-rs` package sources and license. Its manifest is reduced to the dependency-free features used by this workspace for offline builds.

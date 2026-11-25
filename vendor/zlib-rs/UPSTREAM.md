# zlib-rs snapshot

Repository: https://github.com/laputa-systems/zlib-rs
Base: 496c33cd8c5f3fa2ef688498533981d22a07593f
Branch: manganese/chromium-deflate
Local commit: f5decac
Patch: opt-in `chromium-compat` feature: Chromium's level table, ANZAC hash with a 15-bit minimum mask, three-byte matches with TOO_FAR, and madler's deflate_fast for levels 1 to 3.

The snapshot contains the `zlib-rs` package sources and license. Its manifest is reduced to the dependency-free features used by this workspace for offline builds.

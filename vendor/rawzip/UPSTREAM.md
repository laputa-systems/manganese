# rawzip snapshot

Repository: https://github.com/laputa-systems/rawzip (fork of https://github.com/nickbabcock/rawzip).

Base: upstream v0.5.2 line at the fork's rebase point `d76d92b`.

Branch: master. Commit: 8f27f4142b6a963b98aa7292934064de0c0be5c8 (pushed). The snapshot contains the crate root and `src/` byte-identical to that commit, with the dev-dependency declarations removed. No local patches.

The crate is pure Rust with no runtime dependencies and `#![forbid(unsafe_code)]`. Chromium-specific path normalization, entry policy and DEFLATE selection stay in `crates/zip`.

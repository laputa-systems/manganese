Repository: https://github.com/KillingSpark/zstd-rs

Fork: https://github.com/laputa-systems/zstd-rs (created 2026-11-25).

Base: v0.9.1, aa145e8757c6982aab6a8336d3bba43d1a4c6fae.

Branch: manganese/streaming-output, pushed to the fork. Commit: 7745aae.

The patches make decoded bytes available immediately while retaining the window needed by later matches, support raw-block prefixes and raw dictionary content, and reject reserved sequence mode bits. The snapshot omits upstream development dependencies and benchmark declarations.

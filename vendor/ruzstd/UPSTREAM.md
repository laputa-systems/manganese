Repository: https://github.com/KillingSpark/zstd-rs

Intended fork: https://github.com/laputa-systems/zstd-rs (not yet created; the owner creates and pushes it).

Base: v0.9.1, aa145e8757c6982aab6a8336d3bba43d1a4c6fae.

Branch: manganese/streaming-output. Local commit: d0f7dca.

The patches make decoded bytes available immediately while retaining the window needed by later matches, support raw-block prefixes and raw dictionary content, and reject reserved sequence mode bits. The snapshot omits upstream development dependencies and benchmark declarations.

# rust-brotli snapshot

Repository: https://github.com/laputa-systems/rust-brotli
Base: 190a0364e0af3542ab9258d18fc16c12785be10c
Branch: manganese/box-package
Pushed commit: e525445 (branch pushed to the fork, 2026-11-25).
Patch: give the fork package a distinct name while preserving its library name; add a decoder-only feature so the box does not carry encoder sources and generated encoder tables; re-export the decoder's `BrotliDecoderIsFinished`.

The snapshot contains the decoder-facing library source and licenses. Its manifest is reduced to the dependencies and features used by this workspace.

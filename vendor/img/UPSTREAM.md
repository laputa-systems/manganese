# img snapshot

Repository: https://github.com/joshuarli/img
Base: 3213d3c81481fb0f3873f0f4c20e952ea108df91
Branch: master (pushed 2026-11-25; the three commits are cd28108 and its two parents, rebased onto master 137ad9e).
Commit: cd2810830a63 on master. The snapshot is the `codecs/png` crate, byte-identical to `codecs/png` at that commit.
Patch: three local commits on the base. The first makes the native codecs (jpegli, libjpeg-turbo, dav1d, libavif) optional behind a default-on `native-codecs` feature and adds `codecs/png`, the streaming PNG codec exported as `img::png_stream`. The second adds `png_stream::chromium`, a port of Chromium's rust_png_ffi reader and writer: the same png crate options and transformations, `next_interlaced_row`, `next_frame_info`, the Adam7 expand and splat helpers, and the Level1WithUpFilter, Fastest, Fast, Balanced and High encoder levels. The third allows missing documentation on the module.

The snapshot contains the `img-png` package sources (`codecs/png`) and the img licenses, the PNG path only. The package depends on the png crate (pinned to 0.18.1, the version Chromium vendors) and nothing else, so it has no `-sys` or C dependency.

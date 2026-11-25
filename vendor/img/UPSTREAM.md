# img snapshot

Repository: https://github.com/joshuarli/img
Base: 3213d3c81481fb0f3873f0f4c20e952ea108df91
Branch: manganese/pure-rust-png
Local commit: 4f65375b1f3c9b3992a21191c099d07b1d8b89a0
Patch: three local commits on the base. The first makes the native codecs (jpegli, libjpeg-turbo, dav1d, libavif) optional behind a default-on `native-codecs` feature and adds `codecs/png`, the streaming PNG codec exported as `img::png_stream`. The second adds `png_stream::chromium`, a port of Chromium's rust_png_ffi reader and writer: the same png crate options and transformations, `next_interlaced_row`, `next_frame_info`, the Adam7 expand and splat helpers, and the Level1WithUpFilter, Fastest, Fast, Balanced and High encoder levels. The third allows missing documentation on the module.

The snapshot contains the `img-png` package sources (`codecs/png`) and the img licenses, the PNG path only. The package depends on the png crate (pinned to 0.18.1, the version Chromium vendors) and nothing else, so it has no `-sys` or C dependency.

# img snapshot

Repository: https://github.com/joshuarli/img
Base: 3213d3c81481fb0f3873f0f4c20e952ea108df91
Branch: master (pushed 2026-11-25 and 2026-11-26; the PNG commits are cd28108 and its two parents, rebased onto master 137ad9e, and the JPEG commit is 7c3034e).
Commit: 7c3034e84ce55170e6ffbecbbe6994cb99c8cf3c on master. The snapshot is the `codecs/png` and `codecs/jpeg` crates, byte-identical to those directories at that commit.
Patch: four local commits on the base. The first makes the native codecs (jpegli, libjpeg-turbo, dav1d, libavif) optional behind a default-on `native-codecs` feature and adds `codecs/png`, the streaming PNG codec exported as `img::png_stream`. The second adds `png_stream::chromium`, a port of Chromium's rust_png_ffi reader and writer: the same png crate options and transformations, `next_interlaced_row`, `next_frame_info`, the Adam7 expand and splat helpers, and the Level1WithUpFilter, Fastest, Fast, Balanced and High encoder levels. The third allows missing documentation on the module. The fourth adds `codecs/jpeg`, the pure-Rust libjpeg-turbo-exact streaming decoder exported as `img::jpeg_stream`, with Chromium's header, orientation, density and ICC metadata behavior and a suspending source.

The snapshot contains the `img-png` and `img-jpeg` package sources (`codecs/png` and `codecs/jpeg`) and the img licenses. Both packages are pure Rust: `img-png` depends on the png crate (pinned to 0.18.1, the version Chromium vendors) and `img-jpeg` has no external dependency, so neither has a `-sys` or C dependency.

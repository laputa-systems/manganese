# Smolith snapshot

Repository: https://github.com/laputa-systems/smolith
Base: 20cbbd098753faac48542ea346606fc0a8ccd87d
Branch: main
Pushed commit: 294d6c3ed53537aa50493c1a0bd20fc82fc37332 (pushed to main, 2026-11-26).
Patch: HTTP/1 client wire hooks in laputa-hyper-futures-lite. `hyper::ext::Http1RequestHead` writes a request head byte for byte and frames its body as declared; `hyper::ext::frame_response` hands every transport read of a response, including end of stream and read errors before the first byte, to a caller's `ResponseFraming`, which parses the head, frames the body and decides whether the connection is kept. The runtime crates are unchanged from the base.

The snapshot contains the Smolith runtime crates and h12tiny-core with their licenses, laputa-fastwebsockets-futures-lite without its fuzz, CI and toolchain files, and laputa-hyper-futures-lite's manifest, license, readme, sources and tests; every file is byte-identical to the pushed commit. Its workspace lists only those crates; the `h2` entry stays for Hyper's optional HTTP/2 feature, which this workspace does not enable.

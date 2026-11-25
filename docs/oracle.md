# Oracle, golden boxes and iteration lanes

This document explains how Manganese proves parity with the frozen oracle and how a change moves from an edit to an accepted box. The rules it implements are in [the requirements](requirements.md) (U15, U16, T15, T16); the order of work is in [the roadmap](roadmap.md).

## Repository layout

| Path | Contents |
|---|---|
| `crates/` | Manganese crates, one per golden box or assembly, in one Cargo workspace on the pinned toolchain (`rust-toolchain.toml`). |
| `boxes/<id>/` | One box spec (U15): `box.toml` with the seam, call protocol version, corpus sources, comparator class and tolerance, and the box's oracle harness sources. |
| `oracle/build/` | The frozen build tooling that produced the oracle archives, unchanged since the freeze. `oracle/build/SNAPSHOT` records its provenance. Python, GN and Docker are allowed here and nowhere else. |
| `oracle/capture/` | Capture patches applied on top of `oracle/build/`'s patch series, behind runtime switches, for golden capture. |
| `oracle/frozen/` | Frozen evidence: the arm64 package revision 2 sidecars, manifest and member list, and `artifacts.lock`. |
| `oracle/workloads/` | Frozen workload inputs written for this repository, such as the native CDP inventory. |
| `goldens.lock` | Digests of every corpus item and golden output. |
| `scripts/` | POSIX shell entry points: host bootstrap, artifact fetch and publish, checks. |

## The frozen oracle

The browser-level oracle is the frozen qualified archive per architecture, not a rebuild. Chromium builds are not reproducible, so a rebuilt `chrome` is a slightly different browser. Its digests are in [the requirements](requirements.md) (A01) and `oracle/frozen/artifacts.lock`.

The source oracle is `oracle/build/` driven on x86_64 only. It compiles harness targets and capture hooks against the identical tree, inputs and options, and it can produce a full archive. That archive is qualified with the frozen `test-fast` and `qualify-package.py`, and stands in for the frozen x86_64 archive if the archive is unavailable. It runs in its own digest-pinned Alpine builder image, so the host needs only Docker, bash and `python3`. The frozen tree and its options never change, so after the first cold build Ninja rebuilds only what a harness or capture hook touches.

arm64 parity is checked against the frozen arm64 archive on an arm64 Linux host. That proof cannot run on the x86_64 campaign host, so it is a **Needs environment** item in the roadmap.

## Artifact store

Large inputs and outputs never enter git. They are content-addressed blobs in the owner's OCI registry (owner, 2026-11-24): fetched anonymously from `https://oci-blobs.17166969.xyz/sha256/<hex>` and published with `stuff publish oci --layout <dir> --repository <name> --tag <tag>`. The repositories are `manganese/oracle-inputs` (archives, Chromium and toolchain source inputs, builder packages), `manganese/corpora` and `manganese/goldens`. `scripts/artifact fetch` verifies the SHA-256 and size of every download into `$MANGANESE_STORE` (default `$HOME/.local/share/manganese/store`). That store is durable; `.work/` is disposable. Publishing needs `stuff` credentials and the registry's private endpoint. Without them, new blobs stay in the local store and are listed for the owner to publish.

## Golden boxes

A golden box wraps one oracle subsystem behind a seam Chromium already has. Seams where Chromium already serializes data come first, because their formats exist and every real page exercises them:

| Seam | Box | Corpus sources |
|---|---|---|
| Pure functions: compression, image and media codecs, color transforms, font sanitizing, shaping and rasterization, URL parsing, text encodings, MIME sniffing, the HTML tokenizer and tree builder, the CSS tokenizer and parser, cookie and header parsing, certificate verification | One box each | Conformance suites (html5lib-tests, WPT data, Wycheproof), Chromium's unit-test data, and the seed corpora of Chromium's in-tree libFuzzer targets, each of which is a ready-made harness entry point |
| CDP messages | DevTools endpoint | A recording proxy between each frozen client workload (appliance contract 7.5) and the oracle |
| NetLog and wire captures | HTTP/1.1, HTTP/2, TLS, cookies, cache | `--log-net-log` and packet captures through a SOCKS5 proxy for the same workloads |
| Mojo messages | IPC wire format and validation | Messages recorded at the oracle's Mojo boundary |
| Seccomp-BPF programs | Sandbox policy per process type | The programs Chromium's policy compiler emits, evaluated as decision tables over syscall, argument class and architecture, plus broker outcomes; byte identity is an optional witness |
| `PaintOpBuffer` | 2D raster (I01) | Paint streams serialized from real pages |
| GPU command buffer | Software WebGL | Command streams from WebGL conformance and real pages |
| Layout and DOM dumps | Style, layout and DOM | Chromium web_tests expectations and dumps from real pages |
| `d8` stdin and stdout | V8 | Test262, mjsunit and captured scripts |

Capture hooks are batched: one oracle rebuild per milestone adds every hook that milestone needs. Existing dump facilities (NetLog, DOM and layout dumps, `PaintOpBuffer` serialization, command-buffer capture, tracing) and in-tree fuzzer targets come first, because they change no core file and so invalidate little of the oracle build.

A box must model the seam's real call protocol, not only its inputs and outputs. That means streaming chunks, pauses for script, re-entrancy, callbacks and threading, traced from Hydrogen running real workloads. A crate that passes coarse goldens but cannot sit behind the real protocol is not done. Subsystems too entangled for a box (DOM, bindings, V8, GC and the event loop) use behavioral oracles instead: WPT and web_tests results, and Test262 through `d8`.

Each box has a ratcheting scoreboard that fails on any regression, a holdout corpus the hill-climbing lane never sees, and a differential-fuzz lane against the live oracle harness. A holdout case that exposes a failure becomes a regression case once fixed, and the holdout is replenished so it stays unseen.

### Comparators

Comparator classes come from F16: exact for anything a page or server can hash, tolerance only where the manifest allows it. Every exact comparison names the observable surface that justifies it. For example, zlib deflate output is exact because `CompressionStream` and `toDataURL('image/png')` expose it, while a cache's internal file layout is compared only through what the browser reads back. `box.toml` also declares the environment inputs that change the answer, the values that legitimately vary and the relationships checked in their place, the orderings that matter, and the expected oracle defects and required improvements with their own absolute assertions (U15). Before a candidate is compared, the oracle harness must agree with itself across repeated runs and processes; a box whose oracle disagrees with itself has a missing environment input.

### Oracle equivalence

Goldens come from rebuilt and instrumented builds, so three builds are tied together (U17):

1. The frozen production archive is authoritative.
2. A clean rebuild from `oracle/build/` must agree with it on every surface the M0 inventories record and on the frozen `test-fast`.
3. Each capture-hook batch must leave the enclosing behavior unchanged with hooks enabled and disabled.

Each box also has an anchor check that ties its harness to the frozen archive through a public surface: for example, decoding a corpus image through the harness and through `getImageData` on the frozen browser. A rebuilt or instrumented oracle carries its own identity in every receipt.

### arm64 evidence

The source oracle is x86_64-only, but F16 is per architecture. Each architecture-sensitive box records one evidence path in `box.toml`:

| Path | What it establishes |
|---|---|
| Pinned arm64 captures | Comparison at the internal seam |
| Public APIs of the frozen arm64 browser (canvas, Web Audio, WebGL, DOM) | Comparison at the observable surface |
| A narrowly scoped arm64 harness | Internal behavior no browser API exposes |
| An arm64 assembly boundary | Acceptance at a larger, named boundary |

The frozen arm64 archive runs in an arm64 Linux container (proved 2026-11-24), so the second path needs no arm64 Chromium build. Emulated arm64 is not evidence: an emulated CPU reports different features, so SIMD dispatch differs.

## Iteration lanes

Most work happens in L0. Day-to-day work never builds Chromium.

| Lane | What runs | Where and how long | Needs Chromium |
|---|---|---|---|
| L0 goldens | `cargo nextest` for one crate against its stored goldens, with kache as `RUSTC_WRAPPER` | The campaign host, seconds | No |
| L1 live oracle | The holdout corpus and differential fuzzing of one crate against its box's prebuilt oracle harness | The campaign host, minutes | Prebuilt harness only |
| L2 assembly | Rust-only assemblies against their own goldens | The campaign host, minutes to an hour | No |
| L3 contract | A Manganese archive against the appliance contract in containers: the frozen verifier, the launch harness, readiness, sandbox and shutdown, the frozen client workloads and the persona report, compared with the oracle archive under the same harness | The campaign host, from M8 | No |
| L4 appliance | The archive in the appliance's production images, both architectures | Owner-run (**Needs environment**) | No |

The launch harness runs an archive the way the appliance does (appliance contract 2 to 4). It uses a pristine Alpine container, user `browser` with uid 1000, the frozen switches, `/profile` and `/home/browser/Downloads`, and Docker's seccomp profile disabled so the browser can build its own sandbox. On 2026-11-24 it ran the frozen arm64 archive to readiness, with a sandboxed renderer (`NoNewPrivs 1`, `Seccomp 2`, own PID namespace), in 8.4 s. `make oracle-launch ARCH=<arch> MODE=headless` (`oracle/launch/launch.py`) runs it: the frozen verifier extracts the archive (1.7), the browser container has no interface besides loopback, and a sidecar sharing its network and PID namespaces plays the supervisor (`oracle/launch/probe.py`) for readiness, the sandbox witness and `Browser.close`. It checks clauses 1.7, 1.8 and 2 to 4 and writes a JSON report under `.work/tmp/launch/`. Later harnesses reuse it: `launch.open_session` verifies and starts a browser container, `Session.start_browser` launches the browser with the supervisor's switches plus any extra ones, `Session.sidecar` runs a command in a container that shares the browser's namespaces, and `oracle/launch/cdp.py` is a standard-library CDP client for those sidecars. `oracle/launch/appliance.py` holds the appliance's launch environment and switches in the order the supervisor passes them. The oracle's DevTools HTTP server keeps the connection open after a `Connection: close` response, so the probe reads by `Content-Length`, as the supervisor does.

kache enters in two places. It is the `RUSTC_WRAPPER` for the workspace. It also replaces ccache as `cc_wrapper` in the oracle build, where the frozen Gate A cache-reuse check and kache's planned Chromium case are its acceptance. Chromium's GN rules call rustc through their own wrapper scripts, so caching the oracle's Rust needs a small capture-series patch that puts kache in front of rustc. Every miss, wrong hit or unsupported compiler invocation found here is a kache ticket, fixed in kache first.

Smolith enters through the first service box, the DevTools HTTP and WebSocket endpoint on h12tiny's server (S01). The frozen client workloads exercise it end to end, and the oracle's `/json/protocol` crash lives there. img and play enter through the M2 image and audio boxes: their native-backed paths are L0 references, and their pure-Rust paths hill-climb to oracle exactness in their own repositories before Manganese pins them.

## Moving one box to parity

1. Write `boxes/<id>/box.toml` and the box's harness, build the harness in the oracle lane, and record goldens for the corpus and a separate holdout set.
2. Start the crate from the registry default, or from a mechanical port where no candidate is exact.
3. Hill-climb in L0 until the scoreboard is exact, or within the box's approved tolerance.
4. Run L1 on the holdout set and under differential fuzzing. Merge only when both are clean.
5. Feed the crate into the next assembly.

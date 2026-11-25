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

Run `make oracle-equivalence REBUILD=/absolute/path/to/archive.tar.xz` with the archive's `.sha256`, `.acceptance.json` and `test-fast.json` beside it. The report and probe outputs are written under `.work/tmp/equivalence/`; the frozen contract report is cached by the `oracle/launch` tree revision. Persona capture validates the declared wire and debugger relationships before comparing the normalized exact fields. The ignored `test-fast` fields and substitutions are listed in `oracle/probes/equivalence/test-fast-ignored.json` and `test-fast-normalize.json`. The ABI census and CDP trace are compared item by item; a CDP field is keyed by its method or event kind, name, message section and path, and its observed JSON type set must match. Only variability the frozen evidence recorded (`abi/x86_64-unstable.txt`, `cdp/unstable.tsv`) is tolerated. An item the rebuild alone saw in only some of its runs is reported as unresolved: it stays a discrepancy until it is classified, and a rebuild never widens its own tolerance. `oracle/probes/equivalence/selftest.py`, run by `make check`, holds the negative controls.

For a negative control, pass `EQUIVALENCE_ARGS='--inject /absolute/path/to/injection.json'`. The JSON object names `inventory` (`capability` or `persona`), `file` (a JSON filename), and `content` (the replacement evidence object); the driver substitutes it after capture and before comparison.

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
| L0 goldens | The runner checks one crate against stored goldens and its prebuilt oracle harness, with kache as `RUSTC_WRAPPER` | The campaign host, seconds | Prebuilt harness only |
| L1 live oracle | The holdout corpus and differential fuzzing of one crate against its box's prebuilt oracle harness | The campaign host, minutes | Prebuilt harness only |
| L2 assembly | Rust-only assemblies against their own goldens | The campaign host, minutes to an hour | No |
| L3 contract | A Manganese archive against the appliance contract in containers: the frozen verifier, the launch harness, readiness, sandbox and shutdown, the frozen client workloads and the persona report, compared with the oracle archive under the same harness | The campaign host, from M8 | No |
| L4 appliance | The archive in the appliance's production images, both architectures | Owner-run (**Needs environment**) | No |

The launch harness runs an archive the way the appliance does (appliance contract 2 to 4). It uses a pristine Alpine container, user `browser` with uid 1000, the frozen switches, `/profile` and `/home/browser/Downloads`, and Docker's seccomp profile disabled so the browser can build its own sandbox. On 2026-11-24 it ran the frozen arm64 archive to readiness, with a sandboxed renderer (`NoNewPrivs 1`, `Seccomp 2`, own PID namespace), in 8.4 s. `make oracle-launch ARCH=<arch> MODE=headless` (`oracle/launch/launch.py`) runs it: the frozen verifier extracts the archive (1.7), the browser container has no interface besides loopback, and a sidecar sharing its network and PID namespaces plays the supervisor (`oracle/launch/probe.py`) for readiness, the sandbox witness and `Browser.close`. It checks clauses 1.7, 1.8 and 2 to 4 and writes a JSON report under `.work/tmp/launch/`. Later harnesses reuse it: `launch.open_session` verifies the archive once per digest (the extraction, about 1 GB, is shared read-only under `.work/tmp/browser-roots/<sha256>/`) and starts a browser container, `Session.start_browser` launches the browser with the supervisor's switches plus any extra ones, `Session.sidecar` runs a command in a container that shares the browser's namespaces, and `oracle/launch/cdp.py` is a standard-library CDP client for those sidecars. `oracle/launch/appliance.py` holds the appliance's launch environment and switches in the order the supervisor passes them. For x86_64, `make oracle-launch ARCH=x86_64 MODE=headed` builds Tinyland at the contract revision from the local `~/repos/laputa-systems/tinyland` clone with the pinned Rust toolchain, and mounts it and host XKB data separately from the browser archive. `Session.start_browser("headed")` starts the browser under its remote compositor; the host acknowledges frames and records the last frame as RGBA and the visible frame observed before shutdown in the run directory. The report includes frame dimensions and GPU log lines. The oracle's DevTools HTTP server keeps the connection open after a `Connection: close` response, so the probe reads by `Content-Length`, as the supervisor does.

`make contract ARCHIVE=hydrogen-r2-amd64` runs the profile, egress, CDP, audio and durability checks against the frozen archive; `ARCHIVE` also accepts a local archive with its two sidecars. `CLAUSES='6.4 7.2'` selects a subset. The report and proxy connection evidence are under `.work/tmp/contract/`. A `defect 7.3` result records the frozen Hydrogen `/json/protocol` crash; other archives must survive the repeated requests. The client workloads run in independent sessions and report each supported operation, inapplicable operations with reasons, and observed oracle differences. The extension report records each manifest feature and the loading controls. The extension session is headed, because headless passes `--disable-extensions` (contract 3.2), and adds `--load-extension` and `--disable-extensions-except` as contract 7.6 names. It also adds `--ignore-certificate-errors`, the one switch the appliance never passes: the suite's pages and `externally_connectable` origin need a real https fixture server. That deviation goes once those hosts are served through CDP Fetch interception, as the audio and capability fixtures are. The headed render and long soak are excluded. `make contract-selftest` runs negative controls for the assertions.

kache enters in two places. It is the `RUSTC_WRAPPER` for the workspace. It also replaces ccache as `cc_wrapper` in the oracle build, where the frozen Gate A cache-reuse check and kache's planned Chromium case are its acceptance. Chromium's GN rules call rustc through their own wrapper scripts, so caching the oracle's Rust needs a small capture-series patch that puts kache in front of rustc. Every miss, wrong hit or unsupported compiler invocation found here is a kache ticket, fixed in kache first.

The source oracle is `oracle/build/` with the capture series applied: `oracle/source-oracle tree` copies `oracle/build/` to `.work/oracle/build` and applies each patch named in `oracle/capture/series`, and `oracle/source-oracle PHASE...` runs `chromium-build` phases there for `chromium-wayland-release` on amd64, with `JOBS` defaulting to the host's CPU count. Its builder image is tagged with the digest of the frozen tree and the series, and it uses its own volumes (`manganese-oracle-work-amd64` and `manganese-oracle-cache-amd64`), so it never touches another checkout's Chromium volumes. A prepared source tree records the series digest it was prepared with, and a different series prepares it again. `oracle/capture/kache.patch` installs kache in the builder image, sets `cc_wrapper` and a new `rust_compiler_wrapper` GN argument to it (a Chromium patch, `capture-kache-rustc-wrapper.patch`, makes `rustc_wrapper.py` run rustc through it), and replaces Gate A's ccache hit check with a kache one: an unchanged C, C++ and rustc compile must each be a local hit that restores the same bytes. `make oracle-gate-a` runs the image, source and Gate A phases; with `KACHE_DISABLED=1` the hit check fails.

The kache binaries come from a commit of the owned kache repository named in `oracle/capture/kache.lock` with their digests. `oracle/source-oracle kache` builds that commit from `KACHE_REPO` (default `~/repos/kache`) in a detached worktree and stores the binaries by digest in the artifact store; `tree` stages them into the image's build context, and a build whose digests differ from the lock fails. The image tag covers the lock, but the prepared source does not, so a kache change rebuilds the image and keeps the source and its build output. The image sets `KACHE_CC_PATH_NORMALIZE=0` and `KACHE_RUSTC_PATH_NORMALIZE=0`: kache's default path mapping adds `-ffile-prefix-map`, and any macro prefix map makes clang drop `.` components from `__FILE__` (`-include ../../x.h` becomes `../../x.h` instead of `./../../x.h`), so mapped objects differ from the raw compiler's. With mapping off and kache's default `SOURCE_DATE_EPOCH` handling, sampled objects are byte-identical to the raw compiler's. Gate A's hit check compiles trivial probes, which kache cached even when it passed every real Chromium compile through on unmodeled flags. `oracle/source-oracle kache-report` classifies the events of the last build and fails when more than 1% of its C/C++ compiles were not cached; run it after every build.

Smolith enters through the first service box, the DevTools HTTP and WebSocket endpoint on h12tiny's server (S01). The frozen client workloads exercise it end to end, and the oracle's `/json/protocol` crash lives there. img and play enter through the M2 image and audio boxes: their native-backed paths are L0 references, and their pure-Rust paths hill-climb to oracle exactness in their own repositories before Manganese pins them.

## Moving one box to parity

1. Write `boxes/<id>/box.toml` and the box's harness, build the harness in the oracle lane, and record goldens for the corpus and a separate holdout set.
2. Start the crate from the registry default, or from a mechanical port where no candidate is exact.
3. Hill-climb in L0 until the scoreboard is exact, or within the box's approved tolerance.
4. Run L1 on the holdout set and under differential fuzzing. Merge only when both are clean.
5. Feed the crate into the next assembly.

## Box runner reference

`boxes/<id>/box.toml` uses one top-level key per declaration. The runner accepts
basic double-quoted strings without escapes and decimal integers. It rejects
unknown, duplicate, empty and omitted keys. `seam`, `streaming`, `reentrancy`,
`callbacks`, `threading`, `corpus_sources`, `environment`,
`varying_relationships`, `ordering`, `oracle_defects`, `improvements` and
`anchor` describe the U15 contract. For an inapplicable declaration, write
`none: ` followed by a reason. A present defect or improvement includes its
absolute assertion after `assert: `. Present varying values include their
relationship check after `check: `. `wire_version` is `1`.

`comparison` is `exact` or `tolerance`; `observable_surface` names the output's
observable surface. The tolerance class also needs `tolerance_bound`, a
nonnegative finite decimal encoded as a string. Tolerance compares a single
finite UTF-8 number by absolute difference. `arm64_evidence` starts with
`captures: `, `public_api: `, `harness: ` or `assembly: ` and names that path.
`holdout_percent` is an integer from 30 to 99. `oracle` and `candidate` name
executable paths relative to the repository root. The selftest spec is a
complete example.

`goldens.lock` has one whitespace-separated row per corpus item: box ID, lane
(`seen` or `holdout`), item ID, input SHA-256, input byte count, output SHA-256
and output byte count. The runner reads the blobs from `sha256/<digest>` under
`MANGANESE_STORE`, defaulting to `~/.local/share/manganese/store`. It validates
each loaded blob's length and digest and refuses a holdout input that is also a
seen input or output. Outputs may coincide across the sets: small output spaces
(an empty checksum, a shared error response) make that unavoidable, and an
unseen input still tests the candidate. The actual holdout share must meet the spec. The
synthetic selftest generates its blobs in a temporary store under `.work/tmp`.

Protocol version 1 begins with the four bytes `MGB1` on both stdin and stdout.
Each request and response is a four-byte big-endian length followed by that
many bytes, at most 2^32-1 bytes. A peer returns one response for each request, in
order, and writes diagnostics only to stderr. EOF closes the request stream.
The golden runner starts a fresh process for each item and sends one request. It runs
the oracle twice in separate processes for every item and checks all oracle
results before starting any candidate. Oracle output must equal the recorded
golden even for a tolerance comparator.

Run `make box BOX=<id>` for L0 or `make box BOX=<id> LANE=holdout` for L1.
L0 loads seen blobs only, then applies Linux Landlock before invoking either
adapter. The sandbox permits read and execution of the adapter binaries and
runtime libraries but denies access to the blob store. L0 fails closed when
Landlock is unavailable. L1 loads holdout blobs and runs the same oracle and
candidate comparisons. Run `make box-fuzz BOX=<id>` for L1 differential
fuzzing against the prebuilt oracle named by the spec. Build the harness beforehand. The lane starts one
oracle and one candidate and sends each a stream of requests. `SEED` selects a
decimal u64 seed; otherwise the runner chooses and prints one. `CASES` defaults
to 10000. On divergence the runner minimizes byte removals and prints the seed,
zero-based case index, input and outputs, and saves the input as
`.work/tmp/fuzz-<box>-<seed>.bin`. `INJECT=1` selects the selftest injected
candidate as a negative control.

Each spec declares `fuzz = "bytes"` or `fuzz = "corpus"` and an integer
`fuzz_max_len` from 1 to 1048576. The bytes generator chooses random lengths
and bytes. The corpus generator also requires `fuzz_seeds`, a space-separated
string of seen item IDs in `goldens.lock`. It mutates those inputs with bit
flips, insertions, deletions, splices and truncations. Holdout items cannot be
used as seeds.

A box whose wire records carry structure that byte mutation would only break declares `fuzz = "driver"` and `fuzz_driver = "<executable>"` instead, with no `fuzz_max_len`. The runner starts the driver with `FUZZ_ORACLE` and `FUZZ_CANDIDATE` naming the two programs and passes `SEED` and `CASES` through; the driver generates, sends and compares the records itself, prints its seed and the diverging case, and exits nonzero on a divergence. The zlib box's driver, `oracle/harness/zlib/fuzz.py`, mutates seen deflate inputs and invalid streams and samples the declared level, window, memLevel, strategy and flush matrix.

`boxes/<id>/scoreboard.toml` records version 1 and the set of passing seen
items. A previously passing item that fails is a regression. Run
`make box BOX=<id> LANE=ratchet` to atomically record newly passing items;
ordinary runs do not update the scoreboard. `make box BOX=selftest` runs the
framework's synthetic acceptance cases. `make assembly A=<id>` runs the named
assembly crate's tests; each assembly owns its golden comparisons.

`make cdp-trace` records the frozen client, extension, native automation and audio CDP exchanges through a loopback proxy and reduces them to the sorted inventory in `oracle/frozen/cdp/`. String values are recorded as types, and event counts are omitted.

# Roadmap

This is the single plan for this repository: all current and future work, and the handoff for resuming it. Campaigns are sections below, and their items interleave in **Sequence**. "Resume the roadmap" means continue from **Status**.

## Status

- **State:** running 2026-11-26 in the [Manganese](#campaign-manganese) campaign, M2 and M3 box lanes. Resume with "resume the roadmap"; first command `make check lint` on main.
- **Owner direction, 2026-11-26:** ignore perf gates for this campaign; maximize lanes for feature completion.
- **Owner steering, 2026-11-26 (review of d5fc446):** keep the three active lanes with their acceptance boundaries; keep the reuse boundary (general mechanisms and fixes in owned projects, Chromium policy in thin Manganese adapters). Queue, in order: (1) DevTools compressed-WebSocket negotiation/message gap plus wiring the existing 100-request `/json/protocol` improvement test into `make accept`; (2) shared fuzz-driver transport repair — bounded writes, peer cleanup, in-flight input preservation, trailing-record rejection — sequenced after the active lanes because `oracle/harness/fuzzdriver.py` and `scripts/accept` are shared; (3) extract one trusted HTTPS fixture from the cert-verify anchor for http1, TLS handshake, HTTP/2 and certificate verification; (4) public-root CT success coverage and implementation; (5) a small `headless-fetch` service assembly and a bidirectional Hydrogen/Manganese profile handoff to drive API extraction; (6) publish outstanding goldens and replay an accepted box on a clean host. Treat the kache work as successful and revisit it only on a concrete regression. Keep partial-scope qualifications visible as the accepted list grows.
- **Last accepted revision:** 4ff10fe (fuzz-driver transport repair and its standards fixes integrated after jpeg-decode re-acceptance, shared HTTPS fixture extraction and cert-verify re-acceptance); `make check lint` passes and `make box` passes for jpeg-decode, http1, leveldb, devtools-endpoint and runtime on the integrated tree.
- **Accepted:** P0, M0, M1 (U17 re-accepted under the fixed comparison), `kache-chromium` (99.73% warm hits; measurements in docs/oracle.md), and boxes `zlib`, `brotli`, `zstd`, `png`, `jpeg-decode`, `checksums`, `numerics`, `encoding`, `protobuf`, `url`, `devtools-endpoint`, `runtime`, `leveldb`, `http1`, `cert-verify`, and `media-demux` for WAV and FLAC (other formats are compared on the probe result only). Each box's `boxes/<id>/acceptance.txt` records its source-harness, frozen-browser and integration levels; frozen-browser agreement is met for zlib, brotli, zstd, png (narrow corpus), jpeg-decode, leveldb, http1, cert-verify and media-demux (WAV, FLAC); integration (arm64, `make closure`) is pending for all. devtools-endpoint now builds from `vendor/smolith` (laputa-systems/smolith 294d6c3, which adds the HTTP/1 wire hooks http1 uses) instead of the a88c056 git rev. http1 has no https evidence: neither harness nor anchor has trusted TLS. leveldb's Snappy output depends on the CPU's hash instructions, an arm64 evidence risk.
- **Parked lane `lane/m3-sqlite`** (P01, worktree `.work/wt/m3-sqlite`, 48 commits ending `082a52d`): Turso fork `manganese/chromium-sqlite` is pushed at `02c26e911`; the preserved lane and its fork work stay intact and unintegrated. Seen 395/395, holdout 260/260, anchor 20/20 and `make check lint` are green, but three fuzz attempts with distinct hypotheses each exposed a new real divergence, so the box is parked under the three-attempt charter rule rather than patched further. Remaining causes: seed 4103 case 1745 (`.work/tmp/fuzz-sqlite-4103-case1745.bin`) — the `schema_write_in_txn` rollback-abort rule from `d2699ac` is too broad, and disabling it regresses case 3435; seed 1 case 5678 (`.work/tmp/fuzz-sqlite-1-case5678.bin`) — `ALTER TABLE s RENAME COLUMN v TO w` on a WITHOUT ROWID table succeeds in the candidate but fails in the oracle. Retry condition: establish the discriminator between cases 3435 and 1745 from the pinned inputs, make WITHOUT ROWID rename-column fail with SQLite's error, then run both 20000-case seeds, seen, holdout, anchor and `make accept BOX=sqlite` on the integrated tree. Owner decisions still in force: FTS3/FTS5 excluded, deleted-hot-journal crash dropped as undefined, REGEXP exact through a pure-Rust backtracking engine.
- **cert-verify limits:** CT enforcement is exercised only through Chrome Root Store roots as targets (no record can carry an SCT from a known log); revocation only through the CRLSet block list; chains that need an ML-DSA signature verified are out of range until a pure-Rust ML-DSA verifier exists; the anchor cannot cover Chrome Root Store chains. Embedded trust data (Chrome Root Store version 40, CT log list, CRLSet SPKIs) is regenerated with `make cert-verify-trust-data`. webpki is consumed as a git rev (26ac739), so a fresh host needs one `cargo fetch` before offline builds.
- **Integration alert:** media-demux MP3 and Ogg code merged at c2130d0, but `make accept BOX=media-demux` on the integrated tree diverged at fuzz seed 1795716203641059976 case 15591 (Ogg packet discard padding `6500,0` vs `none`); the lane is fixing that case and the receipt must be re-run on main before MP3/Ogg count as accepted.
- **Fuzz transport:** `oracle/harness/fuzzdriver.py` and `crates/goldens/src/fuzz.rs` now bound writes, kill the driver's whole process group on any exit path, preserve the in-flight input before cleanup, reject trailing stdout; the three negative controls pass and `make box-fuzz BOX=selftest` runs 10,000 cases clean.
- **Shared HTTPS fixture:** `oracle/harness/https/` now owns the trust policy, chain generation and SOCKS5 TLS path that the cert-verify anchor used to implement for itself; `make cert-verify-anchor` still agrees 21/21 and `make https-fixture-witness` agrees on a verified navigation in the source harness and the frozen browser. It is the reference fixture for http1 over TLS, TLS-handshake and HTTP/2; those boxes still need their own clients and wire observations.
- **Lanes:** `lane/m2-media-demux-3` (K02, K03, `.work/wt/m2-media-demux-3`, from 971a70c) adds MP3 with AAC/ADTS, then Ogg, accepting each format separately. The candidate ports FFmpeg 8.0's demux paths, because exactness needs FFmpeg's own probe and stream-analysis behavior; play's symphonia-core (vendor/symphonia, MPL-2.0, named by decision 16) supplies the FLAC CRCs. The anchor avoids zero-length and short streams whose duration changes race the page's read in the frozen browser; those cases stay compared in the corpus. Hydrogen's FLAC decoding rejects streams without STREAMINFO and frames whose rate or channels differ from it, which the audio-decode box (K09) must account for.
- **Delegation:** box lanes run as native expert subagents (owner direction 2026-11-25). Worker-tier bridge lanes are unavailable for parser and storage work: OpenCode Go is at its monthly limit and Codex refuses the work as a cybersecurity risk, so worker lanes use native subagents.
- **Next slice:** integrate sqlite and the media-demux formats as each is accepted; then the owner's queued corrections (DevTools, fuzz-driver transport, improvement-test wiring, public-root CT, service assembly and profile handoff), the remaining M2 leaf boxes (`jpeg-encode`, `webp`, `gif`, `regex`, `zip`, `structured-data`, `compression-misc`), and M3 `http2` and `tls-handshake` using the shared HTTPS fixture's evidence.
- **Owed records:** golden blobs for http1, leveldb, jpeg-decode, media-demux, cert-verify and sqlite are only in the local store (N6 publish). `Upstream draft:` issues are owed in the leveldb-rs fork (`manganese/chromium-recovery` now 9c32133) and the Turso fork.
- **Open obligations:** H2/TLS needs trusted HTTPS reference captures from the shared cert-verify fixture (the wire normalizer reports `http2.status = "unobservable-without-trust"`); N3's arm64 inventories need an architecture-parameterization slice first; owned-project fixes are pushed (owner, 2026-11-25): kache to `main` (9631014; the oracle pins binaries from 0174436, byte-identical to the earlier local build), img to `master` (7c3034e adds the pure-Rust `img::jpeg_stream` decoder, f2cf102 applies its standards follow-up; see vendor/img/UPSTREAM.md), and fork branches laputa-systems/zlib-rs `manganese/chromium-deflate` cf79d59, laputa-systems/rust-brotli `manganese/box-package` e525445, laputa-systems/zstd-rs `manganese/streaming-output` 7745aae and laputa-systems/leveldb-rs `manganese/chromium-recovery` 9c32133 (both forks created 2026-11-25), laputa-systems/turso `manganese/chromium-sqlite` 15da7b3cb (created 2026-11-26), and laputa-systems/smolith `main` 15bd571. zstd-rs drafts are filed as laputa-systems/zstd-rs#1 to #3 (0086727, 5ce9eb6, f2439b1; each reproduction fails at upstream tip 881ce5d and passes on the fork). d0f7dca needs none: upstream #137 already rejects reserved sequence mode bits. 7745aae (accept empty compressed blocks) is unresolved: stock libzstd 1.5.7 and upstream ruzstd both reject a hand-built frame with an empty compressed block, while the zstd box agrees with Chromium's decoder exactly; at the next zstd oracle build, feed that frame to the oracle and either file the draft or revert the commit with a regression case.

## Charter

- Decide and keep going. Record a decision only when a later slice depends on it.
- Within the owner's execution grant (Execution boundary), proceed without waiting for approval. A plan does not provide missing permission. Park an item that needs authority outside the grant.
- Hard stops are few, and each one parks the item: an irreversible or outside-the-repo action the grant does not name, a boundary this roadmap forbids, or a loop that cannot check anything. Pushing this repository is forbidden; owned-project fixes are pushed (Execution boundary).
- Fix egregious problems met on the way, in their own commit, to these thresholds: a warm L0 command over 120 s for one crate; a warm `make check` over 10 min; a test that fails once in 20 reruns; a kache hit rate under 90% on an unchanged rebuild. Record anything else and leave it.
- Parity before optimization.
- Parallel lanes for the disjoint sets named in **Sequence**, serial otherwise, each in a worktree at `.work/wt/<lane>`, never `/tmp`, following the orchestrate worktree lifecycle.
- Every commit is a Conventional Commit with the workstream as its scope (`ground`, `oracle`, `lanes`, `box`, `assembly`, `contract`, `release`). The item ID belongs in the body. Code comments never cite the roadmap.
- Run the parent steering checkpoint after each accepted slice, on resume, and whenever a slice fails twice.
- A failing slice gets its cause before any retry (root-cause). An attempt is one submission of the slice's acceptance command, made after a stated remediation hypothesis; ordinary red-green iterations in L0 while hill-climbing are not attempts. After 3 failed attempts with 3 distinct hypotheses, park the slice on its branch with the cause and a retry condition, and take the next ready slice.

## Sequence

Every item belongs to the Manganese campaign. Acceptance commands are run on the integrated tree, together with `make check lint`. Items marked *box* own `crates/<id>/` and `boxes/<id>/` and accept with `make box BOX=<id>`. Items marked *asm* own `crates/asm-<id>/` and accept with `make assembly A=<id>`. Both targets come from `m1-workspace`. Every other `make` target an acceptance command names is created by the item that owns it.

**Disjoint sets.** Box and assembly items within one milestone own disjoint directories and run in parallel. These files are shared and coordinator-owned; lanes propose changes to them and the parent merges serially: `Cargo.toml` (workspace members), `Cargo.lock`, `goldens.lock`, `oracle/capture/series`, `Makefile`, and this roadmap's **Status**. Items in P0, M0 and M1 touch shared harness code and run serially unless the table says otherwise.

J05, J06 and J08 are closed by removal (decision: supervisor and compositor are environment), so they have no item.

### P0 — ground

| ID | TODOs | Owns | Prerequisites | Acceptance |
|---|---|---|---|---|
| `p0-ground` | A15 | Baseline section of this roadmap | none | `sh scripts/setup-dev && make check lint` on the campaign host; record the timings |
| `p0-artifacts` | T13, T14 | `scripts/artifact`, `scripts/artifact-selftest` | `p0-ground` | `make artifact-selftest`: fetch from a `file://` blob base verifies digest and size, a corrupted blob fails, and a name missing from the lock fails |
| `p0-launch` | U01 | `oracle/launch/` | `p0-artifacts` | `make oracle-launch ARCH=x86_64 MODE=headless`: appliance contract 2 to 4 hold for the frozen x86_64 archive (readiness, sandbox witness, `Browser.close` exit within 8 s), and the run writes a JSON report |

### M0 — freeze the remaining evidence

M0 items may add files under `oracle/frozen/<inventory>/` and extend `oracle/frozen/SHA256SUMS`; nothing else in the frozen tree changes. They run in parallel once `p0-launch` is accepted (disjoint `oracle/frozen/<inventory>/` directories and `oracle/probes/<inventory>/` harnesses).

| ID | TODOs | Owns | Prerequisites | Acceptance |
|---|---|---|---|---|
| `m0-workloads` | A02, A08, S12 | `oracle/workloads/clients/`, `oracle/launch/contract` | `p0-launch` | `make contract ARCHIVE=hydrogen-r2-amd64` passes appliance contract 5 to 8 against the oracle itself (excluding 8.2, which is headed, and 8.3, which runs at M8) |
| `m0-cdp-trace` | S02 | `oracle/probes/cdp/`, `oracle/frozen/cdp/` | `m0-workloads` | `make cdp-trace`: the union of methods, events and parameter shapes from every frozen client workload is written to `oracle/frozen/cdp/` and identical across two runs |
| `m0-abi` | B04 | `oracle/probes/abi/`, `oracle/frozen/abi/` | `m0-workloads` | `make abi-census`: the syscall, flag, ioctl, socket-family, `/proc`, `/sys` and `madvise` census of the oracle under every workload is written to `oracle/frozen/abi/x86_64.txt` and identical across two runs |
| `m0-persona` | F16 | `oracle/probes/persona/`, `oracle/frozen/persona/` | `m0-workloads` | `make persona-inventory`: every F16 surface recorded with its comparator class (exact or tolerance with bound); two runs give identical exact surfaces |
| `m0-probes` | K01, L01, M01, R11, A12 | `oracle/probes/capability/`, `oracle/frozen/capability/` | `p0-launch` | `make capability-probes`: WebGL availability and strings per mode, codec answers (`canPlayType`, MSE, WebCodecs, MediaRecorder), XSLT, `chrome.notifications`, Privacy Sandbox interfaces and the global interface and property inventory are recorded, and the proposed-cuts table rows are marked compatible or not from them |
| `m0-census` | A01, A03, A04, A05, A06, A07, A09, A10 | `oracle/probes/census/`, `oracle/frozen/census/` | `m1-source-oracle` | `make census`: GN target, generated-source, link and staged-file graph for `chromium-wayland-release` on x86_64 (A03 to A05), with every node classified per A05 |

### M1 — oracle, golden store and lanes

| ID | TODOs | Owns | Prerequisites | Acceptance |
|---|---|---|---|---|
| `m1-kache-oracle` | T16 | `oracle/capture/kache.patch` | `p0-ground` | `make oracle-gate-a`: the frozen Gate A cache-reuse check passes with kache as `cc_wrapper` and as the rustc wrapper |
| `m1-source-oracle` | T15 | `oracle/Makefile` targets | `m1-kache-oracle` | `make source-oracle`: `oracle/build` produces an x86_64 package revision 2 archive that passes the frozen `test-fast` and `qualify-package.py`; a second run with no input change is an incremental no-op |
| `m1-oracle-equivalence` | U17 | `oracle/probes/equivalence/` | `m1-source-oracle`, `m0-persona`, `m0-probes`, `m0-cdp-trace`, `m0-abi` | `make oracle-equivalence`: the clean rebuild agrees with the frozen x86_64 archive on every M0 inventory and on the frozen `test-fast`, and a capture build with hooks disabled and enabled agrees with the clean rebuild on the enclosing behavior; each discrepancy found is recorded under the rebuild's own identity |
| `m1-workspace` | U15, A11, A13, A14, B01, T01, T02, T03, T06 | `crates/goldens/`, `boxes/selftest/`, `make box`, `make assembly` | `p0-artifacts` | `make box BOX=selftest`: a synthetic box passes with exact goldens; its negative control (a deliberately wrong crate) fails; a scoreboard regression fails; the holdout set is unreadable from the L0 lane; a `box.toml` missing any U15 declaration is rejected; and an oracle harness that disagrees with itself across two runs fails before any candidate is compared |
| `m1-fuzz-lane` | U05 | `crates/goldens/` fuzz runner | `m1-workspace`, `m1-oracle-equivalence` | `make box-fuzz BOX=selftest`: 10 000 differential cases against a prebuilt harness with no divergence, and an injected divergence is reported with a minimized input |
| `m1-zlib` | Q01 | box `zlib` | `m1-fuzz-lane` | `make box BOX=zlib` (Q01): zlib-rs output matches the oracle's zlib inflate and deflate on the corpus, the holdout set and 10 000 fuzz cases, and the anchor check agrees with `CompressionStream` output from the frozen archive |

### M2 — leaf boxes (*box*, parallel)

| ID | TODOs | Registry start |
|---|---|---|
| `brotli` | Q02 | Laputa rust-brotli |
| `zstd` | Q03 | ruzstd |
| `compression-misc` | Q04 | snap, lz4_flex as the census requires |
| `structured-data` | Q05 | serde_json, quick-xml |
| `regex` | Q06 | regex |
| `numerics` | Q07 | double-conversion port |
| `checksums` | Q08 | crc32fast, crc32c |
| `zip` | Q10 | Laputa rawzip |
| `png` | M03 | img (png 0.18.1) |
| `jpeg-decode`, `jpeg-encode` | M02 | img, exact with the oracle's libjpeg-turbo |
| `webp` | M04 | img (image-webp, webpkit) |
| `gif` | M05 | img |
| `avif` | M06 | img's AVIF container reader with K08 |
| `jxl` | M07 (conditional on `m0-probes`) | jxl-oxide |
| `bmp-ico` | M08 | img |
| `color` | M10, M11 | skcms port |
| `font-sanitize` | H05 | OTS and WOFF2 port |
| `shaping` | H01, H02 | Laputa HarfRust, Fontations |
| `glyph-outlines` | H03 | Skrifa outlines, hinting and metrics, exact with the oracle's FreeType or Skrifa path |
| `font-match` | H04, H08 | rust-fontconfig against the frozen configuration |
| `font-subset` | H06 | write-fonts, skera |
| `url` | F12 | url, idna |
| `encoding` | F13 | encoding_rs |
| `protobuf` | P09 | prost |
| `cert-verify` | O04, O05 | Laputa webpki |
| `media-demux` | K02, K03 | play's Symphonia |
| `video-h264` | K05 | OxideAV H.264 |
| `video-hevc` | K06 (conditional on `m0-probes`) | removal expected |
| `video-vpx` | K07 | OxideAV VP8/VP9 |
| `video-av1` | K08 | rav1d-safe |
| `audio-decode` | K09, K10 | play's Symphonia, `ruopus`, `rusty-aac` |
| `audio-dsp` | K11 | Chromium Web Audio DSP port |
| `yuv` | K12 | libyuv port |
| `codec-licensing` | K19, K20 | records only |
| `common-codecs` | B09 | std, existing crates |
| `native-support` | Q09 | removal of each native support library by consumer |

### M3 — services on Smolith (*box* unless marked)

| ID | TODOs | Note |
|---|---|---|
| `devtools-endpoint` | S01, S09 | First Smolith slice; survives 100 `/json/protocol` requests (contract 7.3) |
| `runtime` | B02 | Smolith task runtime with named sequences |
| `http1` | N04 | Wire-exact with the oracle (F16) |
| `http2` | N05 | Wire-exact; browser wire profile in Smolith |
| `tls-handshake` | O01, O02, O03 | ClientHello exact; browser handshake profile in the Laputa rustls fork |
| `websocket` | N07 | |
| `socks-egress` | N02, N03 | Contract 6 |
| `cookies` | N11 | |
| `http-cache` | N10 | |
| `net-policy` | N12, N13, N14 | |
| `fetch` | N09, N15, N16 | |
| `net-foundation` | N01, N17 | |
| `sqlite` | P01 | Opens Hydrogen profile databases unchanged (contract 5.3) |
| `leveldb` | P02 | Same |
| `storage-service` | P03, P04, P07, P10 | |
| `profile-format` | P05, P08 | Hydrogen and Manganese reopen each other's profiles |
| `downloads` | P06 | |
| `web-crypto` | O06, O08, O09 | |
| `adblock` | R03, R04 | `adblock.enabled` from Preferences (contract 5.1) |
| `relay-compat` | S11 | |
| `services` | D03 | GPU and network stay out of process |
| `diagnostics` | B10 | |
| `headless-fetch` (*asm*) | M3 exit | Serves CDP and fetches through a SOCKS5 relay; matches the oracle's CDP, NetLog and wire captures for the same workloads |

### M4 — platform, sandbox and IPC (*box* unless marked)

| ID | TODOs | Note |
|---|---|---|
| `mojo` | D01, D02, D07 | Mojo wire format and validation against recorded messages |
| `sandbox` | D04, D06, D08, D09 | Seccomp-BPF policy accepted by its decisions per syscall, argument class and architecture, plus broker outcomes and negative controls; byte-identical programs are an optional witness |
| `linux-abi` | B04, B08 | Inside the frozen ABI census (contract 10) |
| `process-model` | B05, B06, B07, C01, C05 | |
| `ozone-wayland` | J01, J02, J03, J09 | Against Tinyland at the frozen revision |
| `input` | J04, J07 | kbvm |
| `process-host` (*asm*) | U04 | Launches sandboxed browser, renderer, GPU, network and utility roles with shared memory and IPC, and shuts down cleanly; contract 4.2 witness and negative controls; `make abi-check A=process-host` shows every observed syscall, flag combination, ioctl and `madvise` advice inside the frozen census (contract 10). The same assembly on the appliance kernel is N8 |

### M5 — graphics and compositing (*box* unless marked)

| ID | TODOs | Note |
|---|---|---|
| `raster-2d` | I01, I02 | `PaintOpBuffer` to pixels, Skia-exact |
| `glyph-masks` | H03 | Glyph coverage masks through the Skia-exact text-mask stage; needs `raster-2d` and `glyph-outlines` |
| `compositor` | I03, I04, I05, I14 | |
| `webgl` | I06 to I13, I15 to I17 | GPU command buffer to pixels, exact with the oracle's software path |
| `replay` (*asm*) | M5 exit | Captured paint and compositor streams from real pages match Hydrogen's screenshots exactly |

### M6 — Blink and content (*box* unless marked)

| ID | TODOs | Note |
|---|---|---|
| `blink-foundation` | F01, C03 | Oilpan in Rust |
| `html-parser` | F02 | html5lib-tests plus the oracle's own dumps |
| `dom` | F03, F05, F06, F14 | |
| `bindings` | F04, E09 | Against native V8 through the test-only bridge. The bridge's own acceptance: callback re-entry, cross-heap roots and weak wrappers, exception propagation in both directions, microtask checkpoints and worker termination, each with a fixture that fails when the bridge is wrong |
| `css` | G01, G02 | |
| `layout` | G03, G04, G05, G06 | Layout dumps |
| `paint` | G07, G08, G09 | |
| `navigation` | F07, F08, F09, F10, F11, F15, D05 | |
| `media-pipeline` | K04, K13 to K18 | |
| `svg` | M09 | resvg/usvg components where compatible; live SVG DOM through Blink |
| `image-qualification` | M12 | Canvas export, screenshots and animated image timing; needs every M2 image box |
| `text-qualification` | H09 | Real pages, selection, accessibility and screenshots in both modes; needs `glyph-masks`, `layout` and `font-match` |
| `shell` | R01, R02, R06, R07, R08, R09, B03 | |
| `extensions` | R05 | Extension suite tests (contract 7.6) |
| `cdp-domains` | S02 to S08, S10, S12 to S14 | |
| `static-renderer` (*asm*) | M6 | JS-off pages match Hydrogen's screenshots and layout dumps |
| `blink-bridged` (*asm*) | M6 exit | Rust Blink with the V8 bridge passes the WPT subset, the extension suite and CDP DOM and Runtime transcripts |

### M7 — V8, last (*box* unless marked)

| ID | TODOs | Note |
|---|---|---|
| `v8-parser` | E01 | Test262 parse-phase tests, early errors and their exact messages |
| `v8-runtime` | E02 | Tagged values, objects, shapes and builtins; numeric primitives exact (`Math`, number formatting) |
| `v8-ignition` | E03 | Interpreter execution, exceptions, interrupts and microtask order on Test262 and mjsunit subsets |
| `v8-heap` | E06, C04 | Handles, rooting, barriers, weak references, finalization and collection under stress |
| `v8-baseline` | E04 | Baseline compilation and calling conventions on both ISAs |
| `v8-optimizing` | E05, E14 | Safepoints, deoptimization, OSR and tier transitions; outputs equal to the interpreter's |
| `v8-regexp` | E07 | ECMAScript regexp semantics, exact with Irregexp |
| `v8-wasm` | E08 | Wasm spec tests and the JS/Wasm embedding |
| `v8-snapshot` | E10 | Reproducible builtins and snapshot generation |
| `v8-inspector` | E11 | Debugger, profiler and Runtime transcripts |
| `v8-scheduling` | E12, E13 | Termination, workers, SharedArrayBuffer gating; component-reuse boundary |
| `v8` | none | Outer box: the full `d8` behavior on Test262, mjsunit and captured scripts; each unit above leaves it visibly incomplete until all pass |
| `intl` | H07 | Intl output exact with ICU4C |
| `rust-v8` (*asm*) | M7 exit | Rust V8 replaces the bridge in every M6 assembly; the bridge is deleted |

### M8 — integration and release

| ID | TODOs | Owns | Prerequisites | Acceptance |
|---|---|---|---|---|
| `m8-browser` | T11, U06 | `crates/manganese/` | M7 | `make assembly A=browser`: the full browser passes the M6 and M7 assembly goldens |
| `m8-archive` | T04, T05, T07, T08, T12 | `release/` | `m8-browser` | `make archive`: Cargo-only offline build (`cargo build --release --locked --offline --target x86_64-unknown-linux-musl`) produces an archive that the frozen verifier accepts, with the ELF rules of contract 1.4 and 1.8 |
| `m8-contract` | U02, U03, U07, U08, U09, U10, U11, J10 | none | `m8-archive` | `make contract ARCHIVE=<manganese archive>` gives the same results as on the oracle archive for every clause except the improvement clauses, which pass their own absolute assertions (contract, Expected differences): 100 `/json/protocol` requests survived, and the 60-minute soak with no lost CDP session and no unbounded footprint growth |
| `m8-persona` | U16 | none | `m8-archive` | `make persona ARCHIVE=<manganese archive>`: the U16 report shows no unapproved difference from the oracle |
| `m8-closure` | C02, C06, C07, C08, R10, T09, T10, U12, U13, U14 | none | `m8-archive` | `make closure`: T02 to T10, T13, T14 and the remaining U items; two clean builds give identical digests |

### M9 — conditional transport

| ID | TODOs | Acceptance |
|---|---|---|
| `m9-transport` | N06, N08, O07, L02 to L07 | Default outcome: honest unsupported behavior, proved by the capability probes. Implementation only after an owner decision approves a host datagram relay |

### Optimization and polish (after M8)

| ID | Acceptance |
|---|---|
| `opt-loop` | L0, L1 and oracle-lane timings recorded against the P0 baseline; any loop more than 2× its baseline is fixed |
| `opt-browser-log` | Browser timings (startup, workload wall time, footprint) measured against the oracle and logged in the receipts; not gating |

### Close

| ID | Acceptance |
|---|---|
| `close-dead` | No stage-era adapter, bridge or unused crate remains; `cargo machete`-style dependency audit and graph absence proof |
| `close-review` | `ultrareview` over the tree; P1 and P2 findings fixed |
| `close-report` | Final report: closure fields from the requirements for every TODO ID |

## Campaign: Manganese

### Goal

Build the browser that [the requirements](requirements.md) define, prove it against the frozen oracle box by box as [the oracle](oracle.md) describes, and deliver an archive that satisfies [the appliance contract](appliance-contract.md) on both architectures. State: **ready**.

### Decisions

**Owner, 2026-11-24.**

1. Requirements are frozen at manifest version 1; the oracle is the production package revision 2 archive per architecture (A01).
2. The project is self-contained: Chromium is only the oracle, and nothing depends on the repositories it was frozen from.
3. Method: golden boxes and hill-climbing first, bottom-up Rust-only integration last.
4. Order: tooling and oracle, then leaf libraries, services, platform, graphics, Blink, and V8 last. The V8 inspector and debugger are retained.
5. Nothing reaches the appliance until the assembled browser passes the appliance contract.
6. Persona parity is required (F16).
7. GPU and network services stay out of process in both modes.
8. Hydrogen profiles open unchanged, so SQLite and LevelDB file formats stay.
9. Crash reporting is excluded.
10. A narrow MV3 extension runtime is retained (contract 7.6). WebMCP is excluded.
11. QUIC, HTTP/3, WebTransport and WebRTC transport stay deferred (M9).
12. `chrome` is a musl-dynamic PIE (contract 1.4, 1.8).
13. Drop-in scope is the production appliance images only.
14. Artifacts live in the owner's OCI registry through `stuff`.
15. The campaign runs on an isolated x86_64 Alpine Linux host; the source oracle is x86_64-only.
16. Dogfood and improve owned projects: kache, Smolith, img (in place of zune and other image crates), play's vendored Symphonia, `ruopus` and `rusty-aac` (in place of upstream Symphonia), stuff, and the Laputa forks.

**Prescribed** (reason; what would reverse it).

- The appliance supervisor and its compositor are environment, not Manganese roles; J05, J06 and J08 are removed. A drop-in replacement cannot fold in the consumer's own processes. Reversed only by an owner decision to change the appliance.
- Manganese archive revisions start at 100, so they never collide with Hydrogen's in names or receipts.
- `oracle/build/` is a verbatim snapshot of the tooling that built the oracle. Pruning it risks a build that no longer matches the oracle.
- The Rust toolchain is `nightly-2026-11-09`, the pin the owned Rust projects share. Executables link with mold.
- An owned-project fix ships in that project, because the point is better owned tooling for every consumer, not only Manganese (owner, 2026-11-25). A fix to a `joshuarli` or `laputa-systems` project is pushed to its default branch once the project's own checks pass, without a pull request. A fix to a third-party project goes on a `manganese/<topic>` branch of its `laputa-systems` fork, pushed, with an `Upstream draft:` issue in the fork. Manganese consumes the pushed revision through its pin: a git `rev`, or a snapshot under `vendor/<project>/` whose `UPSTREAM.md` names the repository, base, branch and pushed commit.
- `/json/protocol` survives repeated requests although the oracle crashed (contract 7.3). It is an expected difference, because a crashing endpoint is not a persona surface.
- A profile Manganese writes must reopen in Hydrogen, so rollback stays safe (contract 5.3).
- Capture hooks are batched: one oracle rebuild per milestone carries every hook that milestone needs.
- A box that stays inexact after 3 attempts is parked with its evidence. The comparator is never relaxed; only an owner decision can change a surface's class.
- Exactness attaches to the observable surface: a box compares exactly only what a page, server or the appliance can observe, and names that surface. The seccomp policy is accepted by its decisions, not its bytes.
- Golden trust follows U17: the frozen archive is authoritative; a rebuild or capture build earns trust through `m1-oracle-equivalence` and each box's anchor check, and keeps its own identity in receipts.
- arm64 evidence uses the frozen arm64 archive's public surfaces unless a box names another path; emulated arm64 is never evidence, because the emulated CPU changes SIMD dispatch.
- The improvement clauses (contract 7.3, 8.3) have absolute assertions; every other contract clause is compared with the oracle.
- Browser performance is measured and logged, never gating, before the optimization phase.
- Perf gates do not constrain this campaign: `perf-gate-ready` readings never limit lane width or hold work, and lanes stay maximized to finish features (owner, 2026-11-26; feature completion outranks host headroom and timing gates; reversed only by an owner decision restoring the gates).
- Every new exact comparison of an internal representation must name the consumer that requires that exactness: a page, a server, an appliance interface, or another implementation reopening a file. Native byte output alone is not a consumer. Already-accepted evidence stays accepted; storage work now favors cross-reader and cross-writer reopen, recovery and semantic evidence over additional CPU-specific reproduction of private database layout (owner, 2026-11-26; reversed only by an owner decision restoring byte-equality scope).

**Delegated** (the executor decides inside the bound and logs only decisions a later slice depends on).

- Order of boxes inside a milestone, and lane width up to the host's headroom.
- A crate's internal design, provided it stays behind its box's protocol and passes the per-crate Cargo purity check (requirements, build policy).
- Corpus composition per box, with at least 30% of items in the holdout set and every conformance suite the box names included.
- New dependencies: pure Rust, permissive license, no `-sys` or `cc` build, and no native transitive dependency (T02). No performance gate applies to the browser; a dependency must not push a warm L0 run past the 120 s threshold.
- Whether a fix belongs in an owned project or in Manganese, following the co-development table in the requirements.

### Baseline

- **Known red:** none.
- **Proven at preflight, 2026-11-24, on an Apple Silicon Mac with OrbStack:**
  - The frozen arm64 archive runs to readiness with a sandboxed renderer (`NoNewPrivs 1`, `Seccomp 2`, own PID namespace) in a pristine Alpine container under the appliance's launch switches: 8.4 s for the whole container run.
  - The frozen verifier accepts the frozen arm64 archive on Alpine with `python3` and `llvm`: 12.9 s including package installation.
  - A static-PIE musl binary segfaults under `ld-musl-aarch64.so.1`, which is the evidence behind decision 12.
- **Loop timings, pristine `alpine:3.24` capsules from a fresh clone, privileged with their own dockerd.** x86_64 is native on the campaign host (32-core Threadripper, Alpine 3.25, 2026-11-25); aarch64 is native on Apple Silicon (2026-11-24):

  | Step | x86_64 | aarch64 |
  |---|---|---|
  | `sh scripts/setup-dev`, cold | 15 s | 21 s |
  | `make check`, cold | 2 s | 2 s |
  | `make check`, warm | 1 s | 1 s |
  | `make lint` | 0 s | 0 s |
  | `sh scripts/setup-dev`, rerun | 0 s | 0 s |

  `git status --porcelain` is empty after the gates. Reproduce with a privileged `alpine:3.24` container that clones the repository as a uid 1000 user in the `wheel` and `docker` groups and times each step through `su -l`.
- **Campaign host, 2026-11-25.** The host is a shared workstation, not a dedicated machine, so `scripts/setup-dev` is not run on it directly: it would replace the host's own kache build (`kache 1.0.0`; the pin is `2.0.0-rc.2`) and rerun `kache init` for the host's other projects. The host instead has the additive parts (`mold`, the static unwinder alias, the pinned toolchain), and `make check lint` passes there in 2 s with the host's kache. The oracle builder image carries the pinned kache release itself.

### Acceptance

| Deliverable | Command | Passes when | Fails when |
|---|---|---|---|
| Frozen requirements intact | `make check` | Every frozen byte matches its manifest and every step prints `ok` | Any manifest mismatch, broken link, or host-capability failure |
| Oracle runnable | `make oracle-launch ARCH=x86_64 MODE=headless` | Contract 2 to 4 hold for the oracle | Readiness, sandbox or shutdown fails |
| Each box at parity | `make box BOX=<id>` | Corpus and holdout exact (or within approved tolerance) and fuzz clean | Any golden mismatch, scoreboard regression or fuzz divergence |
| Each assembly | `make assembly A=<id>` | Its goldens match | Any mismatch |
| Drop-in archive | `make contract ARCHIVE=<archive>` | Same results as the oracle archive under the same harness | Any contract clause differs outside the expected differences |
| Persona | `make persona ARCHIVE=<archive>` | No unapproved difference | Any unapproved difference |
| Closure | `make closure` | Requirements definition-of-done table holds; reproducible digests | Any native implementation object, extra `DT_NEEDED`, hidden subprocess or digest drift |

### Needs environment

| ID | Proof | Command the owner runs |
|---|---|---|
| N3 | arm64 parity | On an arm64 Alpine host, or an arm64 Linux VM on Apple Silicon (never emulation): `make oracle-launch ARCH=aarch64`; the M0 inventories against the frozen arm64 archive (`make persona-inventory capability-probes cdp-trace abi-census ARCH=aarch64`); each architecture-sensitive box's arm64 evidence path (oracle, arm64 evidence); and `make contract ARCHIVE=<arm64 manganese archive>` |
| N4 | Appliance acceptance | Build both production appliance images from the Manganese archive, then run the appliance's interop, profile-mode, audio-worklet, render-pass, egress, adblock and soak suites |
| N5 | Shadowdriver workloads | Provide `GH_TOKEN` with read access to `joshuarli/shadowdriver` on the campaign host, or run `make contract CLIENTS=shadowdriver` on a host that has it |
| N6 | Publish corpora and goldens | On a host with `stuff` credentials: `scripts/artifact publish manganese/goldens:<tag> <files>` for the store entries the executor lists |
| N8 | Appliance-kernel platform check | After `process-host` is accepted: run that assembly under the appliance kernel on both architectures (role launch, sandbox, IPC and shared memory, the census's synchronization and mapping operations, clean shutdown). It is a disposable diagnostic fixture, not a shipped image |

### Execution boundary and non-goals

**Authorized.**

- Edit this repository and commit.
- Create worktrees under `.work/wt/` and scratch under `.work/tmp/`.
- Fetch pinned inputs by digest or revision: the oracle's source inputs from `oracle/build` locks, crates through `Cargo.lock`, the pinned clients (contract 7.5), the owned projects, and frozen artifacts.
- Run Docker builds and privileged containers on the campaign host.
- Run `scripts/artifact publish` to the `manganese/*` OCI repositories when credentials exist.
- Commit in local clones of the owned projects, following each project's `AGENTS.md`, and push those fixes: to the default branch of a `joshuarli` or `laputa-systems` project after its checks pass, or to a `manganese/<topic>` branch of a `laputa-systems` fork of a third-party project (create the fork with `gh repo fork OWNER/REPO --org laputa-systems --clone=false`), with an `Upstream draft:` issue in the fork.

**Forbidden.**

- Pushing this repository's branches.
- Opening issues, pull requests, comments or reviews in repositories outside the `joshuarli` and `laputa-systems` namespaces.
- Publishing anywhere other than `manganese/*`.
- Signing in to real accounts or contacting live services beyond fetching pinned inputs.
- Changing `docs/requirements.md` or `docs/appliance-contract.md`, apart from Status lines the owner asks for.
- Editing `oracle/build/`, `oracle/frozen/` or `oracle/workloads/` except where an M0 item says so.
- Patching Chromium to call Manganese code.

**Non-goals.** Browser performance parity, size, startup or energy targets; a general-purpose browser; desktop, PDF or printing; Servo, Boa or interpreter-only substitutes; the appliance's diagnostic image; arm64 builds of the source oracle.

**Contract changes for the owner.**

- `chrome` is a musl-dynamic PIE.
- No `chrome_crashpad_handler`.
- GPU and network run out of process in headless mode too.
- `/json/protocol` survives repeated requests.
- Archive revisions start at 100.
- The diagnostic image needs its own change (contract, Consumer changes outside scope).

### Reference

| Path | Purpose |
|---|---|
| `scripts/setup-dev` | Alpine bootstrap: packages, Docker, pinned Rust, pinned kache |
| `scripts/check` | `make check` steps |
| `scripts/artifact` | Fetch, verify and publish artifacts by digest |
| `oracle/build/` | Frozen build tooling; `chromium-build` is its driver and `scripts/install-runtime.py` the appliance's verifier |
| `oracle/frozen/` | Frozen evidence and `artifacts.lock` |
| `oracle/workloads/cdp/appliance-native.txt` | The appliance's native CDP methods and events |
| `.cargo/config.toml` | musl-dynamic PIE link flags |

## Later

- Move the persona and requirements forward to a newer Chromium: a new manifest version, a new oracle freeze and regenerated goldens.

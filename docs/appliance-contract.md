# Appliance contract

**Status:** frozen, manifest version 1, 2026-11-24. This is everything the appliance requires of the browser archive it runs. A Manganese archive that satisfies every clause replaces Hydrogen in the appliance's production images with no change to the appliance. Each clause states what the frozen oracle does; Manganese must do the same unless the clause says otherwise. Clauses are numbered for citation from tests and receipts.

Scope (owner, 2026-11-24): the production appliance images on aarch64 and x86_64. The appliance's full-Linux diagnostic image is out of scope; see [Consumer changes outside scope](#consumer-changes-outside-scope).

## 1. Archive

1.1. **Name.** `chromium-<version>-r<revision>-linux-<arch>-musl.tar.xz`, where `<version>` is `156.0.8078.79` (the frozen persona), `<revision>` is a decimal integer and `<arch>` is `aarch64` or `x86_64`. The appliance matches `chromium-[0-9.]+-r([0-9]+)-linux-<arch>-musl\.tar\.xz` exactly. Manganese revisions start at 100 so receipts never confuse them with Hydrogen's.

1.2. **Root.** Every member sits under one root directory named after the archive without `.tar.xz`. No member escapes the root, no link points outside it, and no special files or duplicate entries exist.

1.3. **Manifest.** `<root>/package-manifest.json` with `schema: 1`, `status: "complete"`, `product: "chromium"`, `product_version: "156.0.8078.79"`, `architecture` (`arm64` or `amd64`), `launcher: "chromium"`, `executable: "chrome"`, and `files`, a list of `{path, size, sha256, mode}` that names every file in the archive except the manifest itself. Every listed digest must match, and the archive may contain no unlisted file. `files` must list `chrome` and `chromium`. The frozen example is [oracle/frozen/arm64-r2/package-manifest.json](../oracle/frozen/arm64-r2/package-manifest.json).

1.4. **ELF audit.** Every ELF file's program interpreter, if it has one, is `/lib/ld-musl-aarch64.so.1` or `/lib/ld-musl-x86_64.so.1` to match its architecture. No ELF file names a glibc library (`libc.so.6`, `libstdc++.so.6`, `libgcc_s.so.1`) in `DT_NEEDED`. Owner, 2026-11-24: Manganese's `chrome` is a musl-dynamic PIE whose interpreter is musl's loader and whose only `DT_NEEDED` entry is `libc.musl-<arch>.so.1`.

1.5. **Checksum sidecar.** `<archive>.sha256` beside the archive, whose first two whitespace-separated fields are the archive's SHA-256 in lowercase hex and the archive's file name.

1.6. **Acceptance sidecar.** `<archive>.acceptance.json` with `schema: 1`, `status: "complete"`, `product: "chromium"`, `product_version`, `architecture` (`arm64` or `amd64`), `archive` (the file name), `archive_root`, `archive_sha256` (equal to the sidecar digest), `report_sha256`, `checks` (each of `product-identity`, `wayland-window`, `page-rendering`, `sandbox-renderer`, `clean-shutdown`, `headless-devtools`, `headless-page`, `headless-sandbox-renderer`, `headless-clean-exit`, `headless-identity`, `headless-user-agent` true) and `identities` (`executable_version: "Chromium 156.0.8078.79"`, `devtools_browser: "Chrome/156.0.8078.79"`). The frozen example is [oracle/frozen/arm64-r2/](../oracle/frozen/arm64-r2/).

1.7. **Verifier.** The appliance verifies every archive with the frozen installer: `python3 oracle/build/scripts/install-runtime.py --archive <archive> --verify-extract <dir>` must exit 0. That one command checks 1.2 through 1.6.

1.8. **Version probe.** `chrome --version` prints `Chromium 156.0.8078.79` and exits 0, with only `HOME` and `LD_LIBRARY_PATH=<root>/lib` set and no display. The x86_64 image builder runs it as `<rootfs>/lib/ld-musl-x86_64.so.1 <rootfs>/opt/browser/chrome --version`, and the aarch64 builder runs it directly. Both must work. A static PIE segfaults under the first form (proved 2026-11-24), which is why 1.4 requires a musl-dynamic PIE.

1.9. **Tolerated layout.** The aarch64 image builder deletes `libVkLayer_khronos_validation.so`, `angledata/VkLayer_khronos_validation.json`, `libVkICD_mock_icd.so`, `angledata/VkICD_mock_icd.json`, `pyproto` and `gen` if present. The x86_64 builder deduplicates files in `lib/` that are identical to its root filesystem's libraries, and tolerates an empty or missing `lib/`. Manganese ships none of those files.

## 2. Installation and identity

2.1. The archive root is installed at `/opt/browser`. `/usr/local/bin/browser` is a symlink to `/opt/browser/chrome`; the supervisor refuses to run anything whose resolved file name is not `chrome`.

2.2. The browser runs as user `browser`, uid 1000, gid 27, with `HOME=/home/browser` and `USER=browser`. The supervisor also sets `LD_LIBRARY_PATH=/opt/browser/lib`, `FONTCONFIG_FILE=/opt/browser/fontconfig/fonts.conf` and `FONTCONFIG_PATH=/opt/browser/fontconfig`. Manganese starts correctly whether or not those paths exist.

2.3. The image provides musl's loader and libc and nothing else the browser may load. The root filesystem is disposable.

2.4. `/profile` is a persistent volume owned by `browser`, mounted before launch. `/home/browser/Downloads` is the default download directory, where the host may mount a share. `/tmp` is a tmpfs.

2.5. Readiness, diagnostics and logs are the supervisor's. The browser's stdout and stderr are appended to a log file, and the browser must not depend on reading them back.

## 3. Launch

3.1. **Every mode** passes `--remote-debugging-port=9221`, `--user-data-dir=/profile`, `--no-first-run`, `--no-default-browser-check`, and the start URL as the last argument (default `about:blank`). Both production modes pass `--proxy-server=socks5://127.0.0.1:1080` and `--proxy-bypass-list=<-loopback>`.

3.2. **Headless** adds, in this order: `--headless=new`, `--disable-gpu`, `--in-process-gpu`, `--disable-background-timer-throttling`, `--disable-frame-rate-limit`, `--animation-duration-scale=0`, `--wm-window-animations-disabled`, `--disable-smooth-scrolling`, `--cc-scroll-animation-duration-in-seconds=0`, `--force-prefers-reduced-motion`, `--hide-scrollbars`, `--disk-cache-dir=/tmp/cache`, `--disable-breakpad`, `--disable-background-networking`, `--disable-component-update`, `--disable-sync`, `--disable-default-apps`, `--disable-extensions`, `--disable-component-extensions-with-background-pages`, `--metrics-recording-only`, `--no-pings`, `--password-store=basic`, `--disable-hang-monitor`, `--enable-features=NetworkServiceInProcess2,LazyKeyedServiceInstantiation,LazyKeyedServiceInstantiationExtensions,LazyKeyedServiceInstantiationExtensionsApi,LazyKeyedServiceInstantiationStorageNotification,LazyKeyedServiceInstantiationVisitedUrlRanking,LazyKeyedServiceInstantiationOptimizationGuide,LazyKeyedServiceInstantiationAutofillAndPassword,LazyKeyedServiceInstantiationSafetyHub` and `--disable-features=SpareRendererForSitePerProcess,OptimizationHints,MediaRouter,AutofillServerCommunication,SegmentationPlatform,overlay-scrollbars`. Stdin is `/dev/null`.

3.3. **Headed** adds `--ozone-platform=wayland`, `--start-maximized`, `--use-gl=angle`, `--use-angle=swiftshader` and `--disable-gpu-compositing`. The compositor runs the browser as its client, with `XDG_RUNTIME_DIR=/run/user/1000` and the Wayland socket `wayland-1` in it.

3.4. **Optional diagnostic.** The appliance may add `--trace-startup=startup,toplevel`, `--trace-startup-duration=2`, `--trace-startup-format=json` and `--trace-startup-file=/home/browser/Downloads/chromium-startup.json`. The browser then writes a JSON trace there.

3.5. **Switch semantics.** Manganese accepts every switch above and gives it the oracle's observable effect, with two exceptions. Owner, 2026-11-24: GPU and network services stay in separate processes in both modes, so `--in-process-gpu` and `NetworkServiceInProcess2` are accepted and have no effect. Unknown switches are ignored, as Chromium ignores them.

## 4. Readiness and shutdown

4.1. **Readiness probe.** The supervisor polls every 2 ms for up to 60 s. It connects to `127.0.0.1:9221` with a 1 s timeout, sends `GET /json/version HTTP/1.1`, `Host: 127.0.0.1:9221` and `Connection: close`, and requires the first 12 response bytes to be `HTTP/1.1 200` within a 2 s read timeout.

4.2. **Sandbox witness.** Readiness also requires a renderer process. The supervisor scans `/proc/*/cmdline`, splitting every argument on spaces so that a rewritten `argv[0]` still matches, and takes the first process with the word `--type=renderer`. That process's `/proc/<pid>/status` must contain `NoNewPrivs:` `1` and `Seccomp:` `2`. The frozen acceptance also requires the renderer in its own PID namespace (`NSpid` with more than one entry).

4.3. **Shutdown.** The supervisor reads `webSocketDebuggerUrl` from the `/json/version` body (the response carries `Content-Length`), opens that WebSocket, and sends the text frame `{"id":1,"method":"Browser.close"}`. The browser must commit pending profile writes and exit with its whole process tree within 8 s. Otherwise the supervisor sends SIGTERM and waits 7 s; in headless mode it then sends SIGKILL. A browser that exits before readiness fails the session.

4.4. **Watch diagnostic.** When enabled, the supervisor repeats the 4.1 probe every 5 s.

## 5. Profile

5.1. Before every launch the supervisor reads `/profile/Default/Preferences` as JSON (an empty object if absent), sets `adblock.enabled` to true or false, keeps every other key, and replaces the file atomically. Manganese reads Chromium's `Preferences` format and turns native adblocking on or off from that key.

5.2. `/profile/Local State` may exist and is owned by `browser`.

5.3. Owner, 2026-11-24: a profile Hydrogen wrote opens in Manganese unchanged, with cookies (including HttpOnly), local storage, IndexedDB, service workers, cache and signed-in sessions intact. A profile Manganese wrote must reopen in Hydrogen, so rollback is safe.

5.4. A profile survives headed, then headless, then headed again: an HttpOnly cookie and local storage persist, and a download made headed lands in `/home/browser/Downloads`.

## 6. Egress

6.1. The guest has no NIC. All traffic, including loopback destinations, goes through the SOCKS5 proxy at `127.0.0.1:1080` with hostnames sent to the proxy. The host resolves names and applies destination and private-address policy. There is no guest DNS, UDP or QUIC path.

6.2. Idle: with no page activity, the browser opens no outbound connection in the 120 s after readiness.

6.3. Browsing: connections go only to the hosts the visited pages use.

6.4. Adblock: with `adblock.enabled` true, a page's `no-cors` fetch of `https://pagead2.googlesyndication.com/pagead/js/adsbygoogle.js` opens no connection to that host; with it false, it opens one.

## 7. CDP

7.1. **Endpoints.** `/json/version`, `/json/list`, `/json/protocol` and WebSocket upgrades on `/devtools/browser/<id>` and per-target paths. `webSocketDebuggerUrl` is built from the request's `Host` header, so clients behind the relay reach the browser.

7.2. **`/json/version`** answers, headless: `Browser: Chrome/156.0.8078.79`, `Protocol-Version: 1.3`, `User-Agent: Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) HeadlessChrome/156.0.0.0 Safari/537.36`, `V8-Version: 15.6.75.27`, `WebKit-Version: 537.36 (@a21ce95c24ce3964416f09ae1862f53f319eb4d0)`, `File-Path-Style: posix`. Headed reports `Chrome/156.0.0.0` in place of `HeadlessChrome/156.0.0.0`. The platform token is `X11; Linux x86_64` on both architectures.

7.3. **Robustness.** One hundred consecutive `GET /json/protocol` requests leave the browser running; the oracle crashed within four under the appliance kernel, so this is a deliberate improvement. Messages up to 32 MiB each way. Many concurrent and abruptly closed connections, with reconnects and no leaked sessions.

7.4. **Native automation.** The appliance's own automation uses exactly the methods and events in [oracle/workloads/cdp/appliance-native.txt](../oracle/workloads/cdp/appliance-native.txt). Input actions send `Page.bringToFront` first. `Runtime.evaluate` runs in the main world with `awaitPromise`. Target, frame and document identifiers stay stable for a document's lifetime.

7.5. **Frozen client workloads.** Each must give the same results on Manganese as on the oracle. The pinned clients are:

| Client | Pin |
|---|---|
| browser-use | `git+https://github.com/browser-use/browser-use@070be98109c03f04a95f704533e8f6e9868d0525`, Python 3.12 |
| agent-browser | 0.43.0, source `1cc19526f3641c8543d55aa6700e7c55b37ac356`; release assets `agent-browser-linux-musl-x64` (sha256 `10b3b1d5fa8f53f7bc44273bcefaa64299ab77071bc83e75d96de5ed66025c89`) and `agent-browser-linux-musl-arm64` (sha256 `f1d146ad95c1df627b3055475fcb971dfd02f757a1eef756a8f2bd9e19ffff4a`) |
| Extension suite and CLI | `https://github.com/joshuarli/extensions` at `5205b197073a61f9733a382f3ed1971d4b67e95a`, Playwright 1.64.1 |
| Shadowdriver SDK and crawler | `joshuarli/shadowdriver` at `ee03cdbd33ef3d096ef41d855c8a670444baa4a5` (private; needs read access) |

The workloads each client runs:

- **Discovery.** `/json/version` reports `Chrome/156.0.8078.79`, and the targets list.
- **Forms.** Fill a text field and click submit on a same-origin page that also embeds a cross-origin frame; the fixture server records the submission.
- **Accessibility.** A full AX tree, and the client's own accessibility snapshot.
- **Screenshot.** A non-empty PNG.
- **Dialogs.** A `confirm()` dialog seen and accepted.
- **Popups.** `window.open` yields a new target.
- **Frames.** A same-origin iframe's text is read, and a cross-origin iframe appears as its own target and is clicked.
- **Page JavaScript.** It reads `navigator.userAgent`.
- **Sessions.** The session survives the client closing; 10 attach and detach cycles leave no open sockets.
- **Files.** A 150 000-byte upload through a file input arrives intact, verified by SHA-256. A blob download lands in the downloads directory.
- **Profiles.** Cookies stay in their profile across two sessions.

7.6. **Extensions** (owner, 2026-11-24). Unpacked MV3 loading through `--load-extension` with `--disable-extensions-except`, and through CDP `Extensions.loadUnpacked` and `Extensions.getExtensions`; `Extensions.triggerAction`. The suite's manifest uses: a module service worker, action popup with title and badge, options page, `activeTab`, `scripting.executeScript`, `runtime` messaging (`sendMessage`, `onMessage`, `connect`, `onConnect`, `getContexts`, `getURL`, `openOptionsPage`, `lastError`), `externally_connectable` with `onMessageExternal`, `storage.local`, `tabs` (`query`, `get`, `create`, `update`, `onUpdated`, `onRemoved`), `windows.update`, `downloads.download`, `cookies.get`, observational `webRequest.onSendHeaders`, static `declarativeNetRequest` rule resources, `offscreen.createDocument`, `notifications.create` and `clipboardWrite`. Headless launches with `--disable-extensions` (3.2).

7.7. **WebMCP.** Absent: `document.modelContext` is undefined in production, and the appliance reports `WEBMCP_UNAVAILABLE`.

## 8. Web behavior the appliance tests

8.1. **Audio worklet survival.** Open three tabs on a page served through `Fetch.enable`, `Fetch.requestPaused` and `Fetch.fulfillRequest` at origin `https://audio.test`. Each page builds `new AudioContext({sampleRate: 48000})`. It loads a pass-through `AudioWorkletProcessor` from a blob URL, which posts its quantum count every 256 quanta. It connects a 220 Hz sine `OscillatorNode` through the worklet node (one input, one output, two channels) to the destination, and resumes. A fourth tab runs three dedicated workers that each spin for 30 ms per task. `startAudio()`, evaluated with a user gesture and `awaitPromise`, must report state `running` and a worklet node. Then, every 5 s for 65 s: no `Target.targetCrashed` or `Target.targetDestroyed` for any of the four targets, each context still `running` with its worklet, and `currentTime` advancing. Each context must advance at least 20 s of audio time, and all four targets must still be listed. No audio device exists; the output is null with a real-time clock.

8.2. **Render passes.** While scrolling a headed session, one render pass per frame (the oracle: 283 passes over 283 frames).

8.3. **Long run.** A 60-minute run with 8 concurrent sessions and tab churn loses no CDP session and shows no unbounded footprint growth. The oracle failed this under the appliance kernel: a headless crawl lost CDP after about 35 minutes, and the footprint grew about 437 MiB an hour.

8.4. **Durability.** IndexedDB, service-worker registrations and Cache Storage survive restart. A browser crash, an interrupted write and a full profile disk either preserve state or fail explicitly; none silently starts a fresh profile.

8.5. **Text.** The frozen archive embeds 25 Noto faces (Sans, Serif, Serif Display, Mono, Math, Symbols and Symbols 2; list in [oracle/frozen/arm64-r2/archive-members.txt](../oracle/frozen/arm64-r2/archive-members.txt)) behind its own fontconfig configuration, and no CJK or color-emoji font, so pasted CJK text and emoji render as missing-glyph boxes. That is the frozen persona until a manifest version changes the font set.

## 9. Wayland

9.1. **Peer.** Tinyland at `4796621b96537bb78232e3d01ca0e96297216ffd`, run as `tinyland --remote --width <w> --height <h> --socket wayland-1 -- <client>`. It prints `TINYLAND_COMPOSITOR_READY` to its log when it accepts clients. Viewports run from 1 to 4096 pixels per side.

9.2. **Required interfaces** and the requests the oracle sends:

| Interface | Requests |
|---|---|
| `wl_compositor` | `create_surface`, `create_region` |
| `wl_surface` | `attach`, `damage`, `commit`, `frame`, `set_opaque_region`, `set_input_region`, `destroy` |
| `wl_region` | `add`, `destroy` |
| `wl_shm`, `wl_shm_pool`, `wl_buffer` | `create_pool`, `create_buffer`, `resize`, `destroy` |
| `wl_seat` (v5) | `get_pointer`, `get_keyboard`, `release` |
| `wl_pointer` | `release`; events enter, leave, motion, button, axis, frame |
| `wl_keyboard` | `release`; events keymap, enter, leave, key, modifiers, repeat_info |
| `wl_output` | `release` |
| `xdg_wm_base` | `get_xdg_surface`, `create_positioner`, `pong` |
| `xdg_surface` | `get_toplevel`, `get_popup`, `set_window_geometry`, `ack_configure` |
| `xdg_toplevel` | `set_title`, `set_app_id`, `set_min_size`, `set_max_size`, `unset_maximized` |
| `xdg_positioner` | `set_size`, `set_anchor_rect`, `set_anchor`, `set_gravity`, `set_constraint_adjustment` |
| `xdg_popup` | `destroy`; events configure, popup_done |
| `wl_data_device_manager`, `wl_data_device` | `get_data_device`, `release`; event selection |

9.3. **Window policy.** One primary toplevel fills the output. A second toplevel from `window.open` is shown over the primary and returns focus when it closes. Popups follow their positioner within the output and receive pointer input.

9.4. **Optional interfaces** the oracle binds when offered: `wp_viewporter`, `wp_fractional_scale_manager_v1`, `wp_presentation`, `wp_cursor_shape_manager_v1` (otherwise `wl_pointer.set_cursor` with cursor surfaces), `wp_content_type_manager_v1`, `wp_single_pixel_buffer_manager_v1`, `xdg_activation_v1`, `zwp_text_input_manager_v3`, `zwp_primary_selection_device_manager_v1`, `zxdg_decoration_manager_v1`, `org_kde_kwin_server_decoration_manager`, `zxdg_output_manager_v1`, `zwp_pointer_gestures_v1`, `zwp_tablet_manager_v2`, `zwp_pointer_constraints_v1`, `zwp_relative_pointer_manager_v1`, `zwp_idle_inhibit_manager_v1`, `zwp_keyboard_shortcuts_inhibit_manager_v1`, and `zxdg_exporter`/`zxdg_importer` v1 and v2. Manganese implements those Tinyland advertises (J03).

9.5. The compositor does not draw the pointer into frames. It forwards the focused client's cursor image and hotspot, so the browser must set cursors through the interfaces above.

## 10. Kernel ABI

10.1. The appliance kernel implements only the Linux ABI subset Hydrogen uses. Manganese may use only the syscalls, flag combinations, ioctls, socket families, `/proc` and `/sys` paths and `madvise` advice in the frozen census (M0 item `m0-abi`), captured from Hydrogen on native Linux under every workload in this contract.

10.2. Known kernel behavior: `madvise` takes effect only for `MADV_DONTNEED` and hugepage hints, and returns 0 without effect for all other advice. `ptrace` is not implemented. Seccomp filter mode, `no_new_privs`, and user and PID namespaces are, as the sandbox witness (4.2) shows.

## 11. Persona

11.1. Pages and servers must not be able to tell Manganese from the oracle (owner, 2026-11-24; requirements F16). The identity strings in 7.2 are part of it. The M0 surface inventory (`m0-persona`) records the rest, and the U16 report enforces it.

## Expected differences

- Crashpad: the oracle runs `chrome_crashpad_handler`, and its renderers carry `--crashpad-handler-pid`; Manganese has neither (owner, 2026-11-24).
- GPU and network services run out of process in headless mode too (3.5).
- `/json/protocol` survives repeated requests (7.3).

## Consumer changes outside scope

The appliance's full-Linux diagnostic image does two things the production images do not. It deletes the archive's `lib/libxkbcommon.so.0` and refuses an archive without one. It also runs `chrome --headless --no-sandbox --use-gl=disabled --user-data-dir=<dir> --dump-dom about:blank` and requires a fontconfig cache under `/opt/browser/fontconfig/cache`. Manganese does neither (owner, 2026-11-24), so that image needs its own change before it can run Manganese.

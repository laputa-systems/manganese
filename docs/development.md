# Development

A campaign host is an isolated Alpine Linux machine, x86_64 for the full campaign (the source oracle builds only on x86_64) or aarch64 for arm64 parity work. Docker must be able to run privileged containers.

## Set up a host

```sh
git clone https://github.com/laputa-systems/manganese
cd manganese
sh scripts/setup-dev
make check lint
```

`scripts/setup-dev` is idempotent. It installs the packages below, starts Docker (through OpenRC when the host runs it, otherwise `dockerd` directly), installs the pinned Rust toolchain from `rust-toolchain.toml` through rustup, and installs kache from a pinned release after checking its SHA-256. It refuses any distribution other than Alpine. When it starts `dockerd` itself on an overlay-backed root, as inside a container, it uses the `vfs` storage driver, which is correct but slow and disk-hungry for the oracle builder image. Add `~/.cargo/bin` to `PATH` afterwards.

| Package | Consumer |
|---|---|
| `bash`, `python3` | The frozen build driver `oracle/build/chromium-build` and the frozen verifier `oracle/build/scripts/install-runtime.py` |
| `llvm` | `llvm-readelf`, used by the frozen verifier and by `scripts/check` |
| `docker`, `docker-cli-buildx` | The oracle builder image, the launch harness and container workloads |
| `build-base`, `mold` | Linking Rust executables as musl-dynamic PIEs (`.cargo/config.toml`); GCC's static `libgcc_eh.a` supplies the unwinder through the alias `scripts/setup-dev` installs in `/usr/local/lib/manganese/static-unwind` |
| `rustup` | The pinned nightly toolchain and its musl targets |
| `curl`, `ca-certificates`, `git`, `tar`, `xz` | Fetching pinned inputs and artifacts |
| `make`, `shellcheck` | `make check lint` |

## Checks

`make check` verifies that the frozen tree and evidence are byte-identical to their manifests, that the artifact lock is well formed, and that every relative documentation link resolves. It also checks that the frozen verifier runs on this host's Python, and that the oracle builder's pinned base image pulls and runs under Docker. Finally, a probe program built through kache must link as a musl-dynamic PIE and run both directly and through musl's loader. `make lint` runs shellcheck on the scripts.

## Artifacts

`scripts/artifact fetch NAME` downloads a frozen artifact listed in `oracle/frozen/artifacts.lock` by digest into `$MANGANESE_STORE` (default `~/.local/share/manganese/store`) and verifies it. Publishing (`scripts/artifact publish REPOSITORY:TAG FILE...`) needs `oras`, the `stuff` CLI and its credentials, and access to the registry's private endpoint; a host without them keeps new artifacts in the local store.

## Optional access

- Read access to the private `joshuarli/shadowdriver` repository, through `GH_TOKEN`, for the Shadowdriver client workloads.
- `stuff` credentials, for publishing corpora and goldens.

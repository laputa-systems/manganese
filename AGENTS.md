# Manganese

Manganese is a Rust reimplementation of a reduced Chromium browser that drops into the appliance in place of the Hydrogen archive. Read in this order:

- [Roadmap](docs/roadmap.md): the single plan and its **Status**. "Resume the roadmap" means continue from Status.
- [Requirements](docs/requirements.md): the frozen product requirements, TODO inventory and registry.
- [Appliance contract](docs/appliance-contract.md): what the archive must satisfy to replace Hydrogen.
- [Oracle](docs/oracle.md): golden boxes, the frozen oracle and the iteration lanes.
- [Development](docs/development.md): host bootstrap and checks.

## Rules

- `oracle/build/`, `oracle/frozen/` and `oracle/workloads/` are frozen. `make check` fails if any byte changes. The roadmap names the only slices that may add frozen evidence.
- Python, GN and Docker build logic live under `oracle/` only. Everything else is Rust or POSIX shell.
- Large files never enter git; they live in the artifact store (`scripts/artifact`).
- Scratch and worktrees go under `.work/`, which is disposable. Durable records go in the roadmap, commit messages or `docs/`.
- Run `make check lint` before committing. Do not run formatters.

Repository: https://github.com/laputa-systems/turso (fork of https://github.com/tursodatabase/turso)

Base: 9e928387c549fb3c303c902b741918039a4e4dc4 (2026-11-25, 0.9.0-pre.6).

Branch: manganese/chromium-sqlite.

Pushed commit: 0180639b8 (core: zero dropped cell content under a secure_delete feature).

The snapshot holds the crates Manganese builds: core (turso_core), ast (turso_ast), macros (turso_macros), sqlite/parser (turso_parser), sqlite/frontend (turso_sqlite) and extensions/core (turso_ext), with the upstream license files. Its root manifest lists only those members, and sqlite/version is the SQLite version file turso_core reads at build time; everything else matches the pushed commit.

Fork commits on the branch, oldest first: an optional aegis feature (its cc build dependency is unconditional), a git-source-id feature for the build script's git query, branches built with std (its no-std nightly path calls a removed intrinsic), schema text stored as SQLite writes it for CREATE TABLE and CREATE INDEX, SQLite's sqlite_sequence layout for AUTOINCREMENT outside MVCC, and a secure_delete feature that zeroes dropped cells.

Repository: https://github.com/laputa-systems/turso (fork of https://github.com/tursodatabase/turso)

Base: 9e928387c549fb3c303c902b741918039a4e4dc4 (2026-11-25, 0.9.0-pre.6).

Branch: manganese/chromium-sqlite.

Pushed commit: 15da7b3cb (core: report SQLite 3.53.4).

The snapshot holds the crates Manganese builds: core (turso_core), ast (turso_ast), macros (turso_macros), sqlite/parser (turso_parser), sqlite/frontend (turso_sqlite) and extensions/core (turso_ext), with the upstream license files and sqlite/version. Its root manifest lists only those members; every other file in those crates matches the pushed commit.

Fork commits on the branch, oldest first:

- Build: an optional aegis feature (its cc build dependency is unconditional), a git-source-id feature for the build script's git query, and branches built with std (its no-std nightly path calls a removed intrinsic).
- Schema text: CREATE TABLE, CREATE INDEX and CREATE TRIGGER stored as written, as SQLite stores them, and ALTER TABLE ADD COLUMN spliced into the stored text; SQLite's sqlite_sequence layout for AUTOINCREMENT outside MVCC.
- secure_delete: a feature that zeroes dropped cells, as SQLITE_SECURE_DELETE does.
- Rollback journal: DatabaseOpts::with_rollback_journal keeps legacy-header databases in SQLite's rollback-journal mode (journal format and sync order, hot-journal playback, exclusive and normal locking finalization, journal_mode switches, locking_mode, VACUUM through the journal, removal of an empty -wal on the last close).
- Values: SQLite 3.53.4's REAL to TEXT and TEXT to REAL conversions everywhere, concat_ws keeping empty arguments, SQLITE_ENABLE_ICU's upper, lower, LIKE and REGEXP as a dialect option (icu_casemap 2.2), and SQLite version 3.53.4.

The lane branches manganese/rollback-journal and manganese/sqlite-text in the local clone hold the original commits that were cherry-picked onto the branch.

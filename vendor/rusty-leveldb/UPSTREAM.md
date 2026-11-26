Repository: https://github.com/dermesser/leveldb-rs

Base: 458e7ac9571402c1b0b0df77319ae76731f1f8df (v4.0.2).

Branch: manganese/chromium-recovery of the fork https://github.com/laputa-systems/leveldb-rs, pushed 2026-11-26. Commit: 9c32133 (these vendor commits replayed onto 458e7ac).

The durability patch gives writable files an explicit sync, persists synchronous log writes and table and descriptor installation, and stops later writes after an uncertain log failure. Recovery validates log records and batches, reports corruption in paranoid mode, and skips corrupted blocks in nonparanoid mode. The initial, unqualified repair port adds log conversion, table salvage, file archiving and descriptor reconstruction and recognizes legacy `.sst` files. The repair port retains the LevelDB BSD license in `LEVELDB-LICENSE`.

Read errors propagate through `get`, table, version, merging and public iterators, with sticky iterator status and per-read checksum and cache options. Compaction checks iterator status before installing its output. Repair still needs qualification against Chromium's implementation.

Lock failures retain the `std::io::Error` returned by the operating-system lock operation. The obsolete `errno` dependency and its `errno-dragonfly`/`cc` transitive build are removed.

Disk locks use POSIX record locks on Unix, with a process-wide file-identity table, so they exclude Chromium's file locks as well as other instances in the same process. Other platforms use the standard library's file lock.

A deletion encountered during table lookup hides an older value only when its user key matches the requested key.

`Options::filter_policy` is optional: without a policy no filter block is written or read, as in LevelDB. Table opening verifies the index block checksum only under `paranoid_checks`, reads the metaindex only when a policy is set, and a damaged metaindex or filter block disables filtering instead of failing the table. Malformed filter blocks and Bloom filters shorter than two bytes follow LevelDB's matching rules instead of panicking. `Snapshot` is exported so callers can hold snapshots by name.

Iteration follows LevelDB. The merging iterator changes direction by repositioning every other child, as MergingIterator::Next and Prev do. Memtable iterators return deletion entries in both directions, seek_to_first and seek_to_last pass through boxed iterators, and level and table iterators seek to their last entry and skip empty or damaged blocks backward. The database iterator reports and skips a corrupt internal key, and a level-0 table that cannot be opened becomes an iterator that reports the error.

Tables are written as LevelDB writes them. A data block is flushed once it reaches the block size, its index entry uses LevelDB's shortest separator or successor, index blocks use a restart interval of 1, and a compressed block is stored only when it saves at least an eighth. An unknown block type is corruption.

Memtables account memory as LevelDB's arena does, so they fill at the same writes. Writes, recovery and compactions allocate file numbers, switch logs, choose levels (including boundary inputs and level-0 overlap), drop entries, split outputs, and keep files after a background error as LevelDB does. `compact_key_range` treats an absent bound as unbounded and compacts every level above the deepest overlapping level. Background work runs inline by default. With `BackgroundWork::Deferred` it runs only when an operation must wait for it or `DB::flush_background_work` is called, and work still pending when the database is dropped is abandoned, as at LevelDB shutdown.

Opening decides `create_if_missing` and `error_if_exists` by the CURRENT file, a descriptor that does not exist is corruption, and a comparator name other than the options' is an invalid argument. The oldest snapshot is the smallest live sequence number. Deleted files in a version edit are encoded in order. `Table`, `TableIterator`, `LogReader` and `WriteBatch::try_for_each` are public so that individual files can be read.

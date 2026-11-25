Repository: https://github.com/dermesser/leveldb-rs

Base: 458e7ac9571402c1b0b0df77319ae76731f1f8df (v4.0.2).

Branch: manganese/chromium-recovery. Local commit: pending: the parent replays these vendor commits.

The durability patch gives writable files an explicit sync, persists synchronous log writes and table and descriptor installation, and stops later writes after an uncertain log failure. Recovery validates log records and batches, reports corruption in paranoid mode, and skips corrupted blocks in nonparanoid mode. The initial, unqualified repair port adds log conversion, table salvage, file archiving and descriptor reconstruction and recognizes legacy `.sst` files. The repair port retains the LevelDB BSD license in `LEVELDB-LICENSE`.

Read errors propagate through `get`, table, version, merging and public iterators, with sticky iterator status and per-read checksum and cache options. Compaction checks iterator status before installing its output. Repair still needs qualification against Chromium's implementation.

Lock failures retain the `std::io::Error` returned by the operating-system lock operation. The obsolete `errno` dependency and its `errno-dragonfly`/`cc` transitive build are removed.

Disk locks use POSIX record locks on Unix, with a process-wide file-identity table, so they exclude Chromium's file locks as well as other instances in the same process. Other platforms use the standard library's file lock.

A deletion encountered during table lookup hides an older value only when its user key matches the requested key.

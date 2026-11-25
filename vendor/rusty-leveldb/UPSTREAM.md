Repository: https://github.com/dermesser/leveldb-rs

Base: 458e7ac9571402c1b0b0df77319ae76731f1f8df (v4.0.2).

Branch: manganese/chromium-recovery. Local commit: pending: the parent replays these vendor commits.

The durability patch gives writable files an explicit sync, persists synchronous log writes and table and descriptor installation, and stops later writes after an uncertain log failure. Recovery validates log records and batches, reports corruption in paranoid mode, and skips corrupted blocks in nonparanoid mode. The initial, unqualified repair port adds log conversion, table salvage, file archiving and descriptor reconstruction and recognizes legacy `.sst` files. Normal reads and iterators still suppress table errors, so the repair path is not integrated or qualified. The repair port retains the LevelDB BSD license in `LEVELDB-LICENSE`.

The read-error blocker reproduces with a disk database opened with `paranoid_checks = true` and `block_size = 128`: write keys `key00` through `key39`, compact and close, flip the first byte of the table, then reopen. `get_at` for `key00` returns `Ok(None)` and iteration returns 38 of 40 entries without an error status. Chromium's checksum-verifying read reports corruption. Repair requires fallible status to propagate through table, version, merging and public iterators, together with the caller's read options; this is separate from the three authorized fork changes.

The default dependency graph also includes `errno-dragonfly` and its `cc` build through `errno`. Remove that native transitive dependency before adding this crate to Manganese's `Cargo.lock`; the current snapshot has not been added to the workspace dependencies.

Repository: https://github.com/dermesser/leveldb-rs

Base: 458e7ac9571402c1b0b0df77319ae76731f1f8df (v4.0.2).

Branch: manganese/chromium-recovery. Local commit: pending: the parent replays these vendor commits.

The durability patch gives writable files an explicit sync, persists synchronous log writes and table and descriptor installation, and stops later writes after an uncertain log failure. Recovery validates log records and batches, reports corruption in paranoid mode, and skips corrupted blocks in nonparanoid mode. The initial, unqualified repair port adds log conversion, table salvage, file archiving and descriptor reconstruction and recognizes legacy `.sst` files. Normal reads and iterators still suppress table errors, so the repair path is not integrated or qualified. The repair port retains the LevelDB BSD license in `LEVELDB-LICENSE`.

Repository: https://github.com/dermesser/leveldb-rs

Base: 458e7ac9571402c1b0b0df77319ae76731f1f8df (v4.0.2).

Branch: manganese/chromium-recovery. Local commit: pending: the parent replays these vendor commits.

The durability patch gives writable files an explicit sync, persists synchronous log writes and table and descriptor installation, and stops later writes after an uncertain log failure. Recovery and repair fixes follow in separate commits.

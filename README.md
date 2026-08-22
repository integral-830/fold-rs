# Fold-RS

Fold-RS is an LSM-tree-based storage engine implemented in Rust, built with
durability and crash recovery as its first priorities. It iscurrently
**synchronous and single-threaded**.

> **Project status:** actively developed, pre-1.0. Core write/read/recovery
> paths are implemented and tested; compaction and background operation are
> not yet implemented. See [Status](#status) below.

## Features

Implemented today:

- Write-ahead log (WAL) with per-record CRC32 validation and synchronous
  (`sync_all()`-per-write) durability
- WAL generations / rotation (`wal.NNNNNNNN.log`)
- Crash-safe WAL recovery, including safe handling of a truncated final
  record and replay across multiple WAL generations
- Ordered in-memory Memtable (`BTreeMap`) with tombstone support and live
  size accounting
- Immutable SSTables, published atomically via temp-file-then-rename
  (`NNNNNNNN.sst.tmp` → `NNNNNNNN.sst`)
- mmap-based SSTable reads (via [`memmap2`](https://crates.io/crates/memmap2))
- Sparse SSTable index for `O(log B)` block lookup
- Bloom filters per SSTable (false positives allowed, false negatives are a
  correctness bug)
- Newest-to-oldest multi-SSTable reads with correct tombstone shadowing
- Automatic Memtable flush at a configurable-in-code size threshold
- Orphaned `.sst.tmp` cleanup on startup
- A read-amplification benchmark demonstrating the cost of growing SSTable
  count in the absence of compaction

Not yet implemented (see [ARCHITECTURE.md](ARCHITECTURE.md) for design
intent):

- Compaction
- Background flushing / background compaction
- A manifest / version set
- A block cache
- Concurrent writers, transactions, snapshot isolation

## Documentation

| Document                           | Contents                                                                                                                                                         |
| ---------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| [ARCHITECTURE.md](ARCHITECTURE.md) | Component design, write/delete/flush paths, WAL rotation, recovery, durability guarantee, read path, tombstone semantics, compaction as future work, limitations |
| [FORMAT.md](FORMAT.md)             | Exact on-disk byte layouts: WAL record format, SSTable file layout, data block/index/Bloom format, the 32-byte footer, temp-file protocol                        |
| [RESULTS.md](RESULTS.md)           | Benchmark results: WAL append latency, read-amplification measurements, SSTable size verification, multi-flush test                                              |

## Project Layout

```
src/
  storage_engine.rs   # StorageEngine: put/get/delete, flush, recovery, orchestration
  memtable.rs          # In-memory sorted Memtable (BTreeMap<Bytes, Entry>)
  error.rs             # StorageError / Result

  wal/
    format.rs          # WAL record encode/decode, CRC32
    writer.rs           # WalWriter (append + sync_all)
    reader.rs           # WalReader (sequential replay, truncation-safe)

  sstable/
    writer.rs           # SstableWriter (blocks, index, bloom, footer)
    reader.rs           # SstableReader (mmap-backed lookups)
    footer.rs            # 32-byte footer format + magic
    mod.rs

  bloom.rs             # Bloom filter implementation

  bin/
    crash_writer.rs     # Standalone writer used by crash-safety integration tests

benches/
  wal_append.rs         # WAL append latency by value size
  recovery.rs            # Recovery-path benchmark
  read_amplification.rs # get() latency vs. SSTable count
  memtable.rs             # Memtable operation benchmark
  crc32.rs                # CRC32 benchmark

tests/
  recovery_test.rs
  sstable_reader_test.rs
  sstable_writer_test.rs
```

## Usage

Fold-RS is currently a library crate (`fold-rs`), used programmatically via
`StorageEngine`:

```rust
use fold_rs::storage_engine::StorageEngine;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut engine = StorageEngine::open("/path/to/data-dir")?;

    engine.put(b"key1", b"value1")?;
    engine.put(b"key2", b"value2")?;

    assert_eq!(engine.get(b"key1")?.as_deref(), Some(&b"value1"[..]));

    engine.delete(b"key1")?;
    assert_eq!(engine.get(b"key1")?, None);

    Ok(())
}
```

`StorageEngine`'s methods return `fold_rs::error::Result<T>` (an alias for
`Result<T, fold_rs::error::StorageError>`). `StorageError` implements the
standard `std::error::Error` trait via `thiserror`, so `?` works directly
against `Box<dyn std::error::Error>` as shown above — you don't need to name
`fold_rs::error::Result` explicitly unless you want to match on a specific
`StorageError` variant (`Io`, `KeyTooLarge`, `ValueTooLarge`).

- `StorageEngine::open(dir)` creates the data directory if needed, cleans up
  any orphaned `.sst.tmp` files, and replays WAL generations to rebuild
  in-memory state — see [ARCHITECTURE.md §8](ARCHITECTURE.md#8-recovery).
- Every `put`/`delete` is durable on return (`sync_all()` on the WAL before
  the call succeeds) — see
  [ARCHITECTURE.md §9](ARCHITECTURE.md#9-durability-guarantee).
- `get()` returns `Ok(Some(value))`, `Ok(None)` (key absent or deleted), or an
  `Err` on I/O failure.

## Building and Testing

```bash
# Build the library
cargo build

# Run the test suite
cargo test
```

## Running Benchmarks

Most benchmarks run with the standard `cargo bench`:

```bash
cargo bench --bench wal_append
cargo bench --bench recovery
cargo bench --bench memtable
cargo bench --bench crc32
```

The read-amplification benchmark relies on the test-only `flush_for_test()`
helper, which is gated behind the `bench-utils` Cargo feature and **must** be
enabled explicitly:

```bash
cargo bench --features bench-utils --bench read_amplification
```

> ⚠️ `cargo bench -- --bench-utils` is **not** equivalent — that passes
> `--bench-utils` as an argument to the benchmark binary instead of enabling
> the Cargo feature, and will not work.

See [RESULTS.md](RESULTS.md) for recorded results from these benchmarks.

## Status

Fold-RS's current priorities, in order, are:

1. **Correctness** — write/read/delete semantics, tombstone shadowing, and
   crash recovery are covered by unit and integration tests, including a
   repeated (20-iteration) crash-safety test built around the
   `crash_writer` binary.
2. **Durability** — every write is fsync'd before being acknowledged.
3. **Crash recovery** — WAL replay, orphaned temp-file cleanup, and
   truncated-record handling are all explicitly tested.
4. **Explicit, understandable systems-level behavior** — favoring a simple,
   inspectable implementation over premature optimization.

The engine does not yet perform compaction, so SSTable count — and therefore
read amplification — grows monotonically with the amount of data written and
flushed. This is a known, measured, and intentional current limitation; see
[RESULTS.md §5](RESULTS.md#5-read-amplification-benchmark) for the measured
effect and [ARCHITECTURE.md §14](ARCHITECTURE.md#14-compaction-future-work--not-implemented)
for the planned design. Compaction is the next major subsystem planned for
this project.

For the full list of current limitations and planned future work, see
[ARCHITECTURE.md §16–17](ARCHITECTURE.md#16-known-limitations).

## License

MIT License

Copyright (c) 2026 Ayush Verma

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

# Fold-RS Architecture

## 1. Overview

Fold-RS is an LSM-tree-based storage engine implemented in Rust. It is currently
**synchronous and single-threaded**, and is being built with the following priorities,
in order:

1. Correctness
2. Durability
3. Crash recovery
4. Explicit, understandable systems-level behavior

Core components implemented today:

- Write-ahead log (WAL) with generation-based rotation
- An ordered, in-memory Memtable (`BTreeMap`)
- Immutable, mmap-read SSTables
- Bloom filters per SSTable
- Sparse SSTable indexes
- Newest-to-oldest multi-SSTable reads
- Tombstone-based deletes
- WAL recovery on startup
- Atomic SSTable publication via temp-file-then-rename

The following subsystems are **not yet implemented** and are discussed only as
future work in this document: compaction, background flushing, background
compaction, a manifest/version set, a block cache, and concurrent writers.

See [FORMAT.md](FORMAT.md) for exact on-disk byte layouts and
[RESULTS.md](RESULTS.md) for benchmark results.

## 2. Architecture Diagram

```
Client
  |
  +-- put/get/delete
          |
          v
   StorageEngine
      |
      +------------------+
      |                  |
      v                  v
   WalWriter          Memtable
      |                  |
      |                  | flush threshold (4 MiB)
      |                  v
      |             SstableWriter
      |                  |
      |                  v
      |              .sst.tmp
      |                  |
      |               rename
      |                  |
      |                  v
      |              .sst file
      |                  |
      |                  v
      |             SstableReader
      |                  |
      |        +---------+---------+
      |        |         |         |
      |        v         v         v
      |      Bloom     Index     Blocks
      |
      +--> Recovery (on startup)
```

## 3. Core Components

### 3.1 `StorageEngine` (`src/storage_engine.rs`)

```rust
pub struct StorageEngine {
    dir: PathBuf,
    memtable: Memtable,
    sstables: Vec<SstableReader>,
    wal: WalWriter,
    current_wal_path: PathBuf,
    next_sstable_seq: AtomicU64,
    next_wal_seq: AtomicU64,
}
```

`StorageEngine` owns all mutable state for a database directory:

- the WAL writer and the path of the currently-active WAL
- the active Memtable
- a `Vec<SstableReader>`, stored **oldest → newest**
- sequence counters for SSTable and WAL file naming (`AtomicU64`)
- the data directory

Reads traverse `sstables` **newest → oldest** (i.e. in reverse of storage order),
since more recent SSTables may shadow older ones. WAL sequencing is tracked
independently of SSTable sequencing.

A test/benchmark-only entry point is exposed behind the `bench-utils` Cargo feature:

```rust
[features]
bench-utils = []

#[cfg(feature = "bench-utils")]
pub fn flush_for_test(&mut self) -> io::Result<()> {
    self.flush()
}
```

Benchmarks that need this helper must be run with the feature enabled:

```bash
cargo bench --features bench-utils --bench read_amplification
```

Note: `cargo bench -- --bench-utils` is **not** equivalent — that passes
`--bench-utils` as an argument to the benchmark executable rather than enabling
the Cargo feature, and will not work.

### 3.2 `Memtable` (`src/memtable.rs`)

The Memtable is a sorted, in-memory map:

```rust
BTreeMap<Bytes, Entry>

pub enum Entry {
    Value(Bytes),
    Tombstone,
}

pub enum LookupResult {
    Found(Bytes),
    Tombstone,
    NotInMemtable,
}
```

Public surface: `new()`, `put()`, `delete()`, `get()`, `iter()`, `is_empty()`,
`size_bytes()`.

The `BTreeMap` keeps keys sorted, so `iter()` yields sorted keys — this is what
makes Memtable contents directly consumable by the SSTable writer during flush.

**The Memtable does not own WAL responsibilities.** WAL appends are the
responsibility of `StorageEngine`, not the Memtable itself.

#### Size accounting

```rust
const ENTRY_OVERHEAD_BYTES: usize = 48;
```

`calc_entry_size = key.len() + value_len + ENTRY_OVERHEAD_BYTES`, where
`value_len = value.len()` for a `Value` entry and `0` for a `Tombstone`.

On overwrite of an existing key, the old entry's size is subtracted and the new
entry's size is added. As a result, `size_bytes()` reflects the current
**logical** contents of the Memtable, not the cumulative bytes ever written to it.

### 3.3 Write-Ahead Log (`src/wal/format.rs`, `writer.rs`, `reader.rs`)

See [FORMAT.md §1–4](FORMAT.md#1-wal-record-format) for the exact record layout.

`WalWriter` appends a record with:

```
File::write_all()
then
file.sync_all()
```

Every record is followed by `sync_all()`, meaning **every write is durable on
return** under the current durability model (see §5).

`WalReader`:

- reads records sequentially
- recomputes the CRC32 for each record and validates it
- returns owned records (`WalRecordOwned`)
- safely stops at a truncated final record instead of erroring
- stops on CRC mismatch
- never exposes a partially-written final record to the caller

### 3.4 SSTables (`src/sstable/writer.rs`, `reader.rs`, `footer.rs`)

See [FORMAT.md §5–10](FORMAT.md#5-sstable-file-layout) for the exact file layout.

`SstableWriter` workflow:

```
SstableWriter::new(...)
SstableWriter::add(...)   // called once per sorted Memtable entry
SstableWriter::finish()
```

`finish()` writes, in order: data blocks, sparse index, Bloom filter, footer.

Files are written to a temporary path and only made visible via an atomic
rename (see §7).

`SstableReader`:

```rust
pub struct SstableReader {
    mmap: Mmap,
    index: Vec<(Bytes, u64)>,
    bloom: BloomFilter,
    footer: Footer,
}
```

`mmap` owns the memory-mapped file; every `&[u8]` slice used during lookups
borrows from it. Because SSTables are only made visible after an atomic rename,
readers can rely on the underlying file never mutating out from under them.

## 4. Write Path (`put`)

```
put(key, value)
  |
  v
wal.append(Put, key, value)
  |
  v
sync_all()
  |
  v
memtable.put(key, value)
  |
  v
check memtable.size_bytes()
  |
  +-- below FLUSH_THRESHOLD_BYTES --> return Ok
  |
  +-- at/above threshold --> flush(), then rotate WAL
```

## 5. Delete Path (`delete`)

Deletes follow the identical shape, writing a tombstone instead of a value:

```
delete(key)
  |
  v
wal.append(Delete, key, [])
  |
  v
sync_all()
  |
  v
memtable.delete(key)   // inserts Entry::Tombstone
  |
  v
check memtable.size_bytes()
  |
  +-- threshold reached --> flush(), then rotate WAL
```

A tombstone is not a lazy stand-in for "no value" — see §11 for why it must be
treated as authoritative during reads.

## 6. Flush Path

```rust
const FLUSH_THRESHOLD_BYTES: usize = 4 * 1024 * 1024; // 4 MiB
```

The threshold is checked **after** the Memtable mutation on every `put` and
`delete`. When it is reached, the engine flushes the current Memtable to a new
SSTable and rotates the WAL (§7).

The 4 MiB threshold bounds:

- approximate active-Memtable memory usage
- the volume of WAL data that would need to be replayed for unflushed writes
- the approximate amount of mutable data represented before the next SSTable
  is produced

It does **not** mean SSTables end up exactly 4 MiB — SSTable size also depends
on key/value sizes and per-record/block/index overhead.

### 4 MiB Memtable threshold vs. 4 KiB SSTable blocks

These two constants control independent things and are easy to conflate:

| Constant | Value | Controls |
|---|---|---|
| Memtable flush threshold | 4 MiB | how much mutable state accumulates before a flush |
| SSTable target block size | 4 KiB | the granularity of immutable data blocks inside an SSTable |

A single 4 MiB Memtable flush produces many ~4 KiB blocks. Blocks are **not**
required to be exactly 4096 bytes, because records are never split across a
block boundary — see [FORMAT.md §6](FORMAT.md#6-data-block-format).

## 7. WAL Rotation

WAL files are named by generation:

```
wal.00000001.log
wal.00000002.log
wal.00000003.log
```

The correct conceptual ordering around a flush is:

```
1. Flush current Memtable.
2. Ensure resulting SSTable is durable (write .sst.tmp, sync, rename).
3. Create next WAL.
4. Switch future writes to new WAL.
5. Delete old WAL.
```

**Invariant:** the old WAL must never be deleted before the corresponding
SSTable has been durably established. If a crash happens between steps, the
WAL remains a valid recovery source for data that has not yet reached a
durable SSTable.

The current implementation is synchronous, so it can safely use
`mem::replace()` to freeze/reset the active Memtable in place as part of this
sequence.

### Future: background flushing

If flushing moves to a background thread, freezing the Memtable and rotating
the WAL must happen *before* the freeze is handed off, to avoid a window where
the active Memtable has been replaced but no WAL is yet active for new writes:

```
Freeze M0
  |
Create new WAL
  |
Switch active WAL
  |
Resume writes
  |
Flush frozen M0
  |
Delete old WAL
```

## 8. Recovery

On startup, `StorageEngine::open()`:

1. Creates the data directory if needed.
2. Removes any orphaned `*.sst.tmp` files left by a crash mid-flush.
3. Discovers SSTables: scans `*.sst`, parses the numeric sequence from each
   filename, sorts by sequence, and opens a reader for each — stored
   **oldest → newest**. `next_sstable_seq` is set to `max(sequence) + 1`.
   - e.g. `00000001.sst`, `00000002.sst`, `00000015.sst` → `next_sstable_seq = 16`.
4. Discovers WAL generations: scans `wal.*.log`, sorts numerically, and
   replays every WAL in ascending generation order into a fresh Memtable.

Normally exactly one WAL exists. A crash during flush/rotation can leave more
than one; ascending-order replay is functionally safe in that case because
later WAL records simply overwrite earlier state in the fresh Memtable,
provided the WAL ordering itself is correct.

**Known conservatism:** replay does not currently consult any record of which
WAL generations are already represented by durable SSTables, so already-flushed
WAL records may be replayed unnecessarily after a crash. This can produce
redundant Memtable memory and an unnecessary future flush, but does not affect
correctness. A future manifest/version set is the intended fix (§16).

### Crash safety around SSTable publication

```
write .sst.tmp
  |
  v
sync
  |
  v
rename
  |
  v
final .sst
```

- Crash **before** rename → the `.sst.tmp` is incomplete/irrelevant and is
  removed at startup; the WAL remains the sole recovery source for that data.
- Crash **after** rename but **before** old-WAL cleanup → both the new SSTable
  and the old WAL may exist simultaneously. Conservative recovery replays the
  WAL regardless, which is safe but may redo work already captured in the
  SSTable.

## 9. Durability Guarantee

Current write ordering:

```
put/delete
  |
  v
wal.append()
  |
  v
sync_all()
  |
  v
memtable mutation
  |
  v
success
```

The WAL record is made durable (via `sync_all()`) **before** the corresponding
Memtable mutation is considered successful. After a crash, the durable WAL can
fully reconstruct in-memory state up to the last successfully-acknowledged
write. This is referred to internally as the Week 5 Day 6 durability guarantee.

This gives strong durability at the cost of write throughput, since every
single write incurs a synchronous fsync — see [RESULTS.md](RESULTS.md) for
measured WAL append latency, and §15 for the tradeoff and future mitigations.

## 10. SSTable Ordering

- SSTables are stored in `Vec<SstableReader>` **oldest → newest**.
- Reads iterate this vector **newest → oldest** (`.iter().rev()`), so that the
  most recent version of a key is found first.
- This ordering is what makes overwrite and delete semantics correct without
  any compaction: a newer SSTable's entry for a key always shadows an older
  SSTable's entry for the same key.

## 11. Tombstone Semantics

A tombstone is **authoritative** — it is not equivalent to "key not found,"
and it is not something that can be skipped over while continuing to search
older layers.

Read algorithm invariant:

- **Found** → stop searching, return the value.
- **Tombstone** → stop searching, return "not found" (`None`) to the caller.
- **Absent** (key not present in this layer at all) → continue to the next,
  older layer.

Only true absence allows the search to continue. This matters across every
layer boundary:

- **Memtable tombstone over older SSTable value:** if the Memtable holds a
  tombstone for `k` and an older SSTable holds `k -> v1`, the Memtable
  tombstone stops the search before the SSTable is ever consulted.
- **Newer SSTable tombstone over older SSTable value:** if SSTable B (newer)
  holds `k -> Tombstone` and SSTable A (older) holds `k -> v1`, B is checked
  first (§10) and its tombstone stops the search. Continuing on to A would
  incorrectly resurrect deleted data.

This is why a tombstone cannot simply be dropped once written — it must
persist for as long as an older layer might still contain the key it shadows.
Compaction is the only thing that can eventually retire a tombstone safely
(§14).

## 12. Read Path (`get`)

```rust
match self.memtable.get(key) {
    Found(v) => return Ok(Some(v)),
    Tombstone => return Ok(None),
    NotInMemtable => {}
}

for sstable in self.sstables.iter().rev() {
    match sstable.get(key) {
        Some(Found(v)) => return Ok(Some(v)),
        Some(Tombstone) => return Ok(None),
        None => continue,
    }
}

Ok(None)
```

1. Check the Memtable first (it holds the newest data).
2. If not found there, check SSTables newest → oldest.
3. Any `Found` or `Tombstone` result stops the search immediately (§11).
4. If every layer reports absence, the key does not exist.

### Inside a single SSTable's `get()`

```
1. Bloom filter check
     !bloom.may_contain(key)  ->  return None  (definite absence)
2. find_block_offset(key)     ->  locate candidate block via sparse index
3. scan_block(block_offset, key)
```

- A **Bloom false positive** is safe: the subsequent index lookup and block
  scan verify the real key and simply return "not found" if it isn't actually
  present.
- A **Bloom false negative would be a correctness bug** — the filter must
  never claim a present key is absent.

`find_block_offset()` uses binary search over the sparse index:

- empty index → `None`
- `binary_search_by()` exact match at `i` → `index[i].offset`
- no exact match, insertion point `0` → `None` (key is before the first block)
- no exact match, insertion point `i` → `index[i - 1].offset`

This is `O(log B)` over the number of index entries (blocks), not over the
number of records.

`scan_block()` then walks records within the located block in sorted order:

- `record.key < target` → continue to the next record
- `record.key == target` → return `Found`/`Tombstone` as appropriate
- `record.key > target` → return `None` immediately (all later keys in a
  sorted block are also greater, so no further scanning is needed)

## 13. Read Amplification

Because there is currently no compaction, every `put`/`delete` sequence that
crosses the flush threshold produces one more SSTable, and every SSTable read
that misses the Bloom filter's fast path still costs an index lookup and
potential block scan. In the worst case, a `get()` for a key that only exists
in the oldest SSTable must consult:

1. the Memtable (miss)
2. every newer SSTable, in order (Bloom-filtered misses)
3. finally the oldest SSTable (hit)

So lookup cost grows with the number of SSTables — this is **read
amplification**. It is measured directly in `benches/read_amplification.rs`;
see [RESULTS.md §5](RESULTS.md#5-read-amplification-benchmark) for numbers.

## 14. Compaction (Future Work — Not Implemented)

Compaction is intentionally not implemented yet. The planned initial strategy
is **size-tiered compaction**:

```
4 L0 SSTables
  |
  v
k-way merge
  |
  v
1 larger SSTable
```

Because SSTables are already internally sorted, a k-way merge is a natural
fit. For duplicate keys across inputs, the newest version wins:

```
older k -> v1
newer k -> v2
        =>
output  k -> v2
```

### Tombstones during compaction

A tombstone cannot be dropped merely because it represents a deletion. Given:

```
older: k -> v1
newer: k -> Tombstone
```

the tombstone must survive as long as any older, not-yet-compacted layer could
still contain `k`; dropping it early would resurrect `v1`. A tombstone can only
be safely dropped once compaction has established that no older remaining
layer contains that key.

### Why compaction matters

```
Without compaction:                  With compaction:

more flushes                         many SSTables
  |                                     |
  v                                     v
more SSTables                        merge
  |                                     |
  v                                     v
more Bloom checks                    fewer SSTables
  |                                     |
  v                                     v
more possible false positives        lower read amplification
  |
  v
more possible block reads
  |
  v
higher read latency
```

The tradeoff is that compaction introduces **write amplification**, since
existing data is rewritten during merges. In exchange, it collapses obsolete
versions of keys and eventually enables safe removal of obsolete tombstones.

## 15. Concurrency Model

**Current:**

- Synchronous
- Single-threaded
- No background flush
- No background compaction
- No concurrent writers

**Future direction:**

```
Active Memtable
  |
  | freeze
  v
Immutable Memtable
  |
  | background flush
  v
SSTable
```

with the WAL-rotation ordering from §7 (freeze → new WAL → switch → resume
writes → flush frozen Memtable → delete old WAL) used to avoid any window
where writes could be accepted without an active WAL.

## 16. Known Limitations

- No compaction
- No manifest/version set
- No background flushing
- No background compaction
- No block cache
- No Bloom cache
- No group commit
- No batched WAL fsync
- No configurable durability mode
- No concurrent writers
- No transactions
- No snapshot isolation
- No level-based SSTable hierarchy
- Conservative WAL recovery can replay data already represented in durable
  SSTables (see §8)

## 17. Future Work

In rough priority order, based on the limitations above:

1. **Compaction** (§14) — the next major subsystem. Reduces SSTable count,
   collapses obsolete key versions, safely retires obsolete tombstones, and
   reduces read amplification at the cost of write amplification.
2. **Manifest / version set** — tracks which WAL generations are covered by
   durable SSTables, removing the conservative-replay limitation in §8.
3. **Background flushing** and **background compaction**, using the freeze/
   rotate ordering described in §7 and §15.
4. **Block cache** / **Bloom cache** to reduce repeated mmap/page-fault and
   hashing cost on hot keys.
5. **Group commit / batched WAL fsync / configurable durability**, to improve
   on the current per-write `sync_all()` cost (§9, and see
   [RESULTS.md §2](RESULTS.md#2-wal-append-benchmark)).
6. **Concurrent writers**, **transactions**, and **snapshot isolation**.
7. A **level-based SSTable hierarchy**, once compaction exists.

## Engineering Tradeoffs Summary

| Design choice | Benefit | Cost |
|---|---|---|
| WAL + `sync_all()` on every write | Strong durability | Lower write throughput |
| `BTreeMap` Memtable | Sorted iteration for free | Tree/allocation overhead |
| 4 MiB flush threshold | Bounds mutable state | SSTable count grows over time |
| 4 KiB target blocks | Fine-grained block access | More index metadata |
| Bloom filters | Skip definite misses cheaply | Memory + hashing cost |
| mmap SSTable reads | Efficient read-only access | Requires immutable-file discipline |
| Immutable SSTables | Simpler reads/recovery | Requires compaction eventually |
| Newest-first lookup | Correct version resolution | Read amplification |
| No compaction (yet) | Simpler implementation, measurable baseline | Increasing SSTable count over time |

## Final Architectural Summary

```
Client
  |
  v
StorageEngine
  |
  +--> WAL
  |      |
  |      +--> durable records
  |
  +--> Memtable
  |      |
  |      +--> sorted BTreeMap
  |      |
  |      +--> flush at 4 MiB
  |
  +--> SSTables
         |
         +--> immutable
         +--> mmap
         +--> Bloom filter
         +--> sparse index
         +--> sorted blocks
         +--> newest-first lookup
```

Write correctness comes from: WAL durability, an ordered Memtable, immutable
SSTables, newest-first version resolution, and tombstone shadowing.

Read performance comes from: Bloom filtering, sparse indexing, target-sized
blocks, and mmap.

**Current architectural limitation:** no compaction. Therefore more flushes
lead to more SSTables, which leads to more read amplification — demonstrated
experimentally in [RESULTS.md](RESULTS.md). Compaction is the next major
subsystem planned for Fold-RS.

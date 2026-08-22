# Fold-RS On-Disk Format

This document describes the exact byte-level layout of Fold-RS's two on-disk
formats: the write-ahead log (WAL) and SSTables. All multi-byte integers are
**little-endian** unless noted otherwise. See [ARCHITECTURE.md](ARCHITECTURE.md)
for how these formats are used in the read/write/recovery paths.

## 1. WAL Record Format

Implementation: `src/wal/format.rs`, `src/wal/writer.rs`, `src/wal/reader.rs`

Each WAL record is laid out as:

```
+-------------+-------------+-------------+---------------+----------+------------+
| CRC32       | key_len     | val_len     | record_type   | key      | value      |
| 4 bytes     | 4 bytes     | 4 bytes     | 1 byte         | key_len  | val_len    |
+-------------+-------------+-------------+---------------+----------+------------+
```

- `CRC32`: `u32`, little-endian
- `key_len`: `u32`, little-endian, length of `key` in bytes
- `val_len`: `u32`, little-endian, length of `value` in bytes
- `record_type`: `u8`
- `key`: `key_len` raw bytes
- `value`: `val_len` raw bytes

Header size is `HEADER_SIZE = 4 + 4 + 4 + 1 = 13` bytes (CRC32 + key_len +
val_len + record_type), before the variable-length key/value payload.

## 2. CRC Coverage

The CRC32 field covers **everything from `key_len` onward** — i.e.
`key_len || val_len || record_type || key || value`. The 4-byte CRC field
itself is excluded from its own calculation.

On write, the record is serialized with a placeholder `0u32` in the CRC
position, then `crc32fast::hash()` is computed over the bytes from
`key_len` to the end of `value`, and the real CRC is written back into the
first 4 bytes.

On read, `WalReader` recomputes the CRC over the same byte range and compares
it to the stored value; a mismatch causes reading to stop at that record
(§4 below).

## 3. WAL Record Types

```rust
#[repr(u8)]
pub enum RecordType {
    Put = 0,
    Delete = 1,
}
```

- `0` = `Put` — `value` holds the written value.
- `1` = `Delete` — represents a tombstone; `value` is conventionally empty
  (`val_len = 0`).

## 4. WAL Reader Behavior / Truncation Handling

`WalReader` reads records sequentially from the start of a WAL file and:

- recomputes and validates the CRC32 for every record
- returns fully-owned records (`WalRecordOwned`, with `Vec<u8>` key/value)
- **stops safely** if the final record's header or payload is truncated
  (e.g. the process crashed mid-`write_all`), rather than erroring
- **stops** if a record's CRC does not validate
- never surfaces a partial/incomplete final record to callers — a truncated
  or corrupt final record is treated as "no more records," not as an error
  that fails the whole recovery

This truncation tolerance is what allows recovery to proceed cleanly after a
crash mid-append: everything durably written and CRC-valid up to that point is
recovered; the incomplete tail is silently dropped.

## 5. WAL Generation Naming

WAL files are named by monotonically increasing generation number, zero-padded
to 8 digits:

```
wal.00000001.log
wal.00000002.log
wal.00000003.log
```

On startup, all `wal.*.log` files are discovered, sorted numerically by
generation, and replayed in ascending order into a fresh Memtable (see
[ARCHITECTURE.md §8](ARCHITECTURE.md#8-recovery)). Normally only one WAL file
exists at a time; more than one can exist transiently after a crash during
flush/rotation.

> Note: an older test in the codebase's history assumed a single fixed
> `wal.log` filename. The current, authoritative naming convention is the
> multi-generation `wal.NNNNNNNN.log` scheme described above — any test or
> tooling must use this convention, not the legacy single-file name.

## 6. WAL Durability

Implementation: `src/wal/writer.rs`

Every append is followed immediately by a full sync:

```rust
File::write_all(&record_bytes)?;
file.sync_all()?;
```

This means the current design provides **synchronous, per-record durability**
— there is no batching or group commit yet (see
[ARCHITECTURE.md §17](ARCHITECTURE.md#17-future-work) for planned
improvements, and [RESULTS.md §2](RESULTS.md#2-wal-append-benchmark) for the
resulting latency).

## 7. SSTable File Layout

Implementation: `src/sstable/writer.rs`, `src/sstable/reader.rs`,
`src/sstable/footer.rs`

An SSTable file is a single contiguous file laid out as:

```
+----------------+--------+-------+----------------+
| data blocks    | index  | bloom | 32-byte footer |
+----------------+--------+-------+----------------+
```

- **Data blocks** come first, containing the sorted key/value (or tombstone)
  records.
- **Index** is the sparse block index (§8).
- **Bloom** is the serialized Bloom filter (§9).
- **Footer** is a fixed 32-byte trailer (§10) that lets a reader locate the
  index and Bloom sections without a separate metadata file.

`SstableReader::open()` reads the file in this order:

1. `File::open(path)`
2. `unsafe { Mmap::map(&file) }`
3. Parse the **last 32 bytes** of the mapped file as the `Footer`.
4. Verify the footer's magic number; an invalid magic returns an
   `InvalidFormat`-style error (via `read_from`'s `InvalidData` error, wrapped
   as `StorageError::Io`).
5. Deserialize the sparse index using `footer.index_offset`.
6. Deserialize the Bloom filter using `footer.bloom_offset`.
7. Retain the `Mmap` for the lifetime of the reader; all subsequent key
   lookups borrow slices directly from it.

## 8. Data Block Format

Target block size: **4096 bytes** (4 KiB) — see
[ARCHITECTURE.md §6](ARCHITECTURE.md#6-flush-path) for why this is
independent of the 4 MiB Memtable flush threshold.

Each record within a data block is encoded as:

```
+-------------+---------------+---------------+----------+------------+
| key_len     | value_len     | record_type   | key      | value      |
| u32         | u32           | u8            | key_len  | value_len  |
+-------------+---------------+---------------+----------+------------+
```

- `key_len`: `u32`, little-endian
- `value_len`: `u32`, little-endian (`0` for a tombstone)
- `record_type`: `u8` — `0` = value, `1` = tombstone
- `key`: `key_len` raw bytes
- `value`: `value_len` raw bytes

Records within a block are **sorted**, because blocks are built directly from
sorted Memtable iteration (`BTreeMap::iter()`).

**Block boundaries are not fixed at exactly 4096 bytes.** A block's end is
determined by:

- the offset of the *next* index entry (i.e. the start of the next block), or
- for the final block, the boundary given by the index/end-of-data-region.

Because records are never split across a block boundary, actual block sizes
vary slightly around the 4 KiB target.

### Block scan algorithm (`scan_block`)

Given a target key, scan records in order:

- `record.key < target` → continue to the next record
- `record.key == target` → return `Found(value)` or `Tombstone`
- `record.key > target` → return `None` immediately (sorted order guarantees
  no later record in the block can match)

## 9. Index Format

The sparse index maps each block's **first key** to that block's byte offset
within the file:

```
first_key   offset
a           0
f           4096
m           8192
t           12288
```

It is "sparse" because it holds one entry per block, not one entry per record.

### Locating a block (`find_block_offset`)

For a target key `p`, binary search the index for the block whose key range
could contain `p`:

- empty index → `None`
- `binary_search_by()` finds an exact match at position `i` → use
  `index[i].offset`
- no exact match, insertion point `0` → `None` (target is smaller than every
  block's first key; key cannot be in this SSTable)
- no exact match, insertion point `i` (`i > 0`) → use `index[i - 1].offset`
  (the block whose first key is the largest key ≤ target)

Example: for `p` such that `m ≤ p < t`, the block starting at offset `8192`
(first key `m`) is selected.

This lookup is `O(log B)` where `B` is the number of blocks (index entries),
independent of the total record count.

## 10. Bloom Filter Placement / Format

Implementation: `src/bloom.rs`

The serialized Bloom filter is stored as its own section between the index and
the footer (see the layout in §7). Its byte offset within the file is recorded
in the footer's `bloom_offset` field (§11) so the reader can locate and
deserialize it directly from the mmap.

Semantics used during lookup (`SstableReader::get`):

- `bloom.may_contain(key) == false` → the key is **definitely absent** from
  this SSTable; `get()` returns `None` immediately without touching the index
  or any data block.
- `bloom.may_contain(key) == true` → the key **might** be present; the reader
  proceeds to the index lookup and block scan to confirm.

**Core invariant:** false positives are acceptable (they only cost an extra,
ultimately-negative index/block lookup); **false negatives are not** — a false
negative would silently hide data that actually exists, which is a
correctness bug, not a performance issue.

## 11. Footer Format

**Correction:** the SSTable footer is **32 bytes**, not 24 bytes.

Implementation: `src/sstable/footer.rs`

```rust
pub const SSTABLE_MAGIC: u64 = 0x53535441424C4B31;
pub const FOOTER_SIZE: usize = 32;

pub struct Footer {
    pub index_offset: u64,
    pub bloom_offset: u64,
    pub version: u8,
}
```

Exact 32-byte layout, in order:

```
Offset  Size   Field
------  -----  -----------------------
0       8      index_offset (u64, LE)
8       8      bloom_offset (u64, LE)
16      1      version (u8)
17      7      padding / reserved (zero-filled)
24      8      magic (u64, LE) = SSTABLE_MAGIC
------  -----  -----------------------
                Total: 32 bytes
```

- `index_offset`: byte offset of the start of the sparse index section.
- `bloom_offset`: byte offset of the start of the Bloom filter section.
- `version`: single-byte format version.
- 7 bytes of zero-filled padding, reserved for future use.
- `magic`: fixed 8-byte constant `SSTABLE_MAGIC` (`0x53535441424C4B31`),
  written last (occupying the final 8 bytes of the file).

### Reading the footer

A reader takes the **last 32 bytes** of the file (`mmap[len - 32 .. len]`),
parses it into a `Footer`, and validates `magic == SSTABLE_MAGIC`. An invalid
magic value causes `Footer::read_from` to return an error (invalid/corrupt
SSTable), rather than allowing the file to be treated as a valid SSTable.

## 12. Offsets

All offsets stored in the footer (`index_offset`, `bloom_offset`) are absolute
byte offsets from the start of the SSTable file, consistent with how they are
used to slice directly into the mmap'd file (`&mmap[offset..]`).

## 13. Magic / Version

- **Magic:** `SSTABLE_MAGIC = 0x53535441424C4B31` (`u64`, little-endian),
  stored in the final 8 bytes of every SSTable file. Used to validate that a
  file is a well-formed Fold-RS SSTable before trusting any other footer
  field.
- **Version:** a single `u8` in the footer, reserved for future format
  evolution. The current writer always sets a fixed value; readers do not yet
  branch on it, but its presence allows future format changes to be
  distinguished from the current layout.

## 14. Temporary-File Protocol

SSTables are made durable and visible via a temp-file-then-rename protocol:

```
00000015.sst.tmp
  |
  | write all data blocks, index, bloom, footer
  |
  | sync
  v
rename(00000015.sst.tmp -> 00000015.sst)
```

- While being written, an SSTable exists only as `NNNNNNNN.sst.tmp`.
- Once fully written and synced, it is atomically renamed to its final
  `NNNNNNNN.sst` name — this rename is what makes the SSTable visible/valid to
  the rest of the system.
- If a crash occurs before the rename, the `.sst.tmp` file is incomplete and
  is removed during the next startup's orphan-cleanup pass (see
  [ARCHITECTURE.md §8](ARCHITECTURE.md#8-recovery)).
- SSTable filenames use an 8-digit zero-padded sequence number, e.g.
  `00000001.sst`, `00000002.sst`, ..., `00000015.sst`, discovered and sorted
  numerically on startup to determine `next_sstable_seq`.

Because SSTables are only ever observed in their final, fully-written form
(never partially written), `SstableReader` can safely assume immutability for
the lifetime of its mmap.

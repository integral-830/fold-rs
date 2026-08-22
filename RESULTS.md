# Fold-RS Benchmark Results

This document reports measured benchmark results for Fold-RS. All numbers
below are specific to **Fold-RS** and its current, un-compacted,
single-threaded, synchronous-durability implementation. See
[ARCHITECTURE.md](ARCHITECTURE.md) for the design these numbers are measuring,
and [FORMAT.md](FORMAT.md) for the underlying on-disk formats.

> **Note on measurement methodology:** the timings in this document are
> [Criterion](https://github.com/bheisler/criterion.rs) central-tendency
> timing estimates from `cargo bench`, not explicit request-latency
> percentiles. They are **not** labeled p50/p99 anywhere in this document,
> because they were not collected as per-request latency samples. A future
> benchmark that captures individual operation latencies would be required to
> report true p50/p99 figures.

## 1. Benchmark Environment

- Benchmarks live in `benches/`: `wal_append.rs`, `recovery.rs`,
  `mmap_vs_read.rs`, `read_amplification.rs`.
- The WAL append results in §2 were measured on Apple Silicon.
- Benchmarks that use the `flush_for_test()` helper require the `bench-utils`
  Cargo feature:

  ```bash
  cargo bench --features bench-utils --bench read_amplification
  ```

  `cargo bench -- --bench-utils` does **not** work — see
  [ARCHITECTURE.md §3.1](ARCHITECTURE.md#31-storageengine-srcstorage_enginers)
  for why.

## 2. WAL Append Benchmark

File: `benches/wal_append.rs`

With `sync_all()` called on every single append (current durability model,
[ARCHITECTURE.md §9](ARCHITECTURE.md#9-durability-guarantee)), measured on
Apple Silicon:

| Value size | Latency / op | Throughput |
|---|---|---|
| 100 B | 1.4593 – 1.5123 ms/op | ~675 ops/sec |
| 1 KiB | 2.5962 – 2.6778 ms/op | ~380 ops/sec |
| 10 KiB | 3.0930 – 3.2056 ms/op | ~318 ops/sec |

This establishes a **durability-heavy baseline**: throughput is dominated by
fsync cost, not by serialization or in-memory work, and scales down as value
size grows (larger writes still ultimately pay one fsync each). Potential
future improvements, in order of likely impact:

- group commit
- write batching
- background/async fsync
- a configurable durability mode (trading some durability for throughput when
  acceptable to the caller)

## 3. SSTable Size Verification

Worked example, from the SSTable writer test suite:

- 1000 entries
- 16-byte keys
- 100-byte values

Expected approximate SSTable size: **~127,100 bytes**, with tolerance allowed
for block-boundary rounding and index/Bloom/footer metadata overhead (see
[FORMAT.md §7–11](FORMAT.md#7-sstable-file-layout) for what makes up that
overhead).

The corresponding test also reads the final 8 bytes of the produced file and
verifies the SSTable magic number (`SSTABLE_MAGIC`, [FORMAT.md §13](FORMAT.md#13-magic--version)),
validating end-to-end that: records → blocks → index → Bloom filter → footer
add up to the expected final file size and produce a well-formed file.

## 4. Multi-Flush Test

Writing approximately **50 MiB** of random keys/values against the 4 MiB
flush threshold ([ARCHITECTURE.md §6](ARCHITECTURE.md#6-flush-path)) is
expected to produce approximately **12 flush generations** (SSTables), with a
possible final partial generation.

The test verifies:

- SSTable count is approximately 12, ±1 acceptable
- keys from **every** flush generation are still retrievable
- both the oldest and newest generations are individually verifiable
- the read path is exercised across multiple SSTables (i.e. this test doubles
  as a correctness check on newest-first, multi-SSTable reads —
  [ARCHITECTURE.md §12](ARCHITECTURE.md#12-read-path-get))

## 5. Read Amplification Benchmark

File: `benches/read_amplification.rs`

The benchmark measures `get()` latency as a function of the number of
SSTables present, using distinct keys per SSTable and lookups spread across
generations (older keys must pass through every newer SSTable's Bloom
filter/index before reaching the SSTable that actually holds them).

### Representative Criterion central timings

| SSTables | Latency | Relative to 1 SSTable |
|---|---|---|
| 1 | 281.8 ns | 1.00x |
| 5 | 394.5 ns | 1.40x |
| 10 | 527.7 ns | 1.87x |
| 20 | 823.2 ns | 2.92x |

### Measured raw ranges across repeated runs

| SSTables | Range |
|---|---|
| 1 | ~281.46 – 282.33 ns |
| 5 | ~390.20 – 394.73 ns |
| 10 | ~527.07 – 538.17 ns |
| 20 | ~820.70 – 880.96 ns |

## 6. Read Latency Table

See §5 above — this is the same data, presented as the primary read-latency
reference table for Fold-RS's current (uncompacted) SSTable configuration.

## 7. Relative Degradation

Latency growth is **not** linear in SSTable count but grows faster than
sub-linear scaling would predict at low counts: going from 1 → 5 SSTables
(5x more SSTables) costs 1.40x latency, while 1 → 20 SSTables (20x more
SSTables) costs 2.92x latency. In other words, each additional SSTable adds a
diminishing but still non-trivial fixed cost, consistent with the Bloom-check
+ index-lookup overhead described in §8.

## 8. Interpretation

More SSTables mean more Bloom filter checks per `get()` (one per SSTable
consulted before a hit or exhaustion) and, for any SSTable whose Bloom check
doesn't rule the key out, additional index and potential block-scan work.
This produces the measurable latency growth in §5–7. This is exactly the
**read amplification** effect described in
[ARCHITECTURE.md §13](ARCHITECTURE.md#13-read-amplification): without
compaction, every flush adds a fixed amount of read cost to lookups that miss
in newer layers.

## 9. Benchmark Limitations

- These are Criterion-reported central timing estimates (mean/median as
  computed by Criterion's own statistics), **not** explicit p50/p99
  request-latency percentiles collected from raw per-operation samples. Do
  not re-label them as p50/p99 in any downstream document or dashboard.
- The read-amplification benchmark uses distinct keys per SSTable generation
  by design, which isolates the "must search N SSTables" cost cleanly but
  does not model a workload with heavy key overwrite/update locality.
- WAL append numbers (§2) are specific to the measured hardware (Apple
  Silicon) and to the current unbatched `sync_all()`-per-write implementation;
  they are a baseline, not a ceiling — see the future-work list in §2.
- No benchmark yet exists for the effect of compaction, since compaction is
  not implemented ([ARCHITECTURE.md §14](ARCHITECTURE.md#14-compaction-future-work--not-implemented)).

### A note on unrelated historical data

An earlier, separate project ("Boson," a Redis-like server) has its own
historical `PING_MBULK` benchmark results (roughly 97k–143k req/s, with a best
observed ~142,857 req/s and latency figures down to sub-millisecond p50/p99).
**These numbers are not Fold-RS results** and must not be mixed into Fold-RS
documentation, dashboards, or comparisons — they measure an unrelated system
with an unrelated workload.

## 10. Motivation for Compaction

The read-amplification numbers in §5–8 are, concretely, what compaction is
meant to fix. Without compaction, every Memtable flush permanently adds one
more SSTable that every future `get()` must potentially check, so read
latency grows with total writes over time even if the working set of keys
stays small. Compaction ([ARCHITECTURE.md §14](ARCHITECTURE.md#14-compaction-future-work--not-implemented))
would periodically merge SSTables via k-way merge, keeping only the newest
version of each key, which:

- reduces the SSTable count a `get()` must traverse in the worst case
- reduces read amplification back toward the 1-SSTable baseline shown in §5
- collapses obsolete key versions, reducing on-disk size
- eventually allows obsolete tombstones to be safely dropped

at the cost of **write amplification**, since compaction rewrites existing
data. This tradeoff, and compaction's design, are described in full in
[ARCHITECTURE.md §14](ARCHITECTURE.md#14-compaction-future-work--not-implemented).

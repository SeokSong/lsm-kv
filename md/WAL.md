# Write-Ahead Log (WAL)

**Status:** In progress. Encode, decode, and append + fsync are done and tested (24 tests). Replay is next.

## Purpose

<!-- What problem does the WAL solve? What would be lost without it? -->
The purpose of the WAL is to ensure that our writes are durable. The memtable sits on RAM which can be erased at any moment. the benefit of the WAL is that it is a log that sits ondisk. this means whenever we add to it, it is appended a O(1) operation (not atomic); however, it is still quite fast compared to a normal write operation. this allows us to keep a record of all the operations that should happen on disk without actually preforming the expensive operation itself. This allows us to confidentily use RAM without missing any operations or unknowing not having a copy due to a crash

## Record format

<!-- Byte-by-byte layout, widths, byte order, what the CRC covers. Include an example record. -->

- WAL Record Format: Little Endian
    - Header: 10 bytes
        - CRC-32: 4 bytes
        - type: 1 byte
        - keyLen: 1 byte
        - valueLen: 4 bytes
    - key: keyLen bytes
    - value: valueLen bytes

- Limits
    - Key: up to 16 bytes (fits in keyLen u8)
    - Value: up to 65,536 bytes (fits in valueLen u32)
    - Both are checked at compile time against the field widths

- CRC-32
    - Covers every byte after the CRC field (type, lengths, key, value)
    - Uses crc32fast (standard CRC-32). Tested against Castagnoli (crc32c), crc32fast was 3-4x faster on the Mac, see [BENCHMARKS.md](BENCHMARKS.md)
    - Encode writes 4 zero bytes, appends the rest, then fills in the CRC

- Type byte
    - 01 = Put
    - 02 = Delete (TOMBSTONE)
    - A tombstone is a normal record with type 02, valueLen 0, and no value bytes
    - This allows users to put whatever value without ever accidentally interacting with a tombstone

| Record | Bytes |
|---|---|
| Key | `01 \| 00 2C FD DC \| 00 05 \| 00 0C \| 03 \| 00 01` |
| PUT (2,022 bytes) | `CC CC CC CC \| 01 \| 0C \| D0 07 00 00 \| 12 key bytes \| 2,000 value bytes` |
| TOMBSTONE (22 bytes) | `CC CC CC CC \| 02 \| 0C \| 00 00 00 00 \| 12 key bytes` |

## fsync policy

<!-- v1 policy and why. Which variants will you benchmark? -->
- V1: fsync every write
    - append only returns Ok after write + fsync, so a put is durable when it returns
    - Uses `sync_data`, which flushes the bytes and the new file length (all replay needs)
    - Same code on every OS, Rust's std picks the system call. Linux (target): `sync_data` = `fdatasync`. macOS (dev): `F_FULLFSYNC`, since plain `fsync` on macOS doesn't force data to the drive
    - CI runs the tests on Linux and macOS on every push and PR
- Benchmark variants
    - Group commit: one fsync for many records
    - Periodic: fsync every N ms (breaks "durable when put returns")
- fsync is almost the entire cost of a put

| Step (2 KB record, Mac) | Time |
|---|---|
| CRC-32 (crc32fast) | ~0.03 us |
| Write without fsync | ~3 us |
| Write + fsync | ~3.0 ms p50, 4.8 ms p99 |
| Throughput with fsync | ~350 appends/s |

- What this means
    - Live F1 ingest is ~1.4 puts/s (20 cars x 2 streams, one chunk per ~28 s sector), so 350/s is plenty
    - Replaying a race at max speed is ~6,000 puts, ~17 s, almost all of it waiting on fsync. Group commit is the variant that matters
    - Mac `F_FULLFSYNC` is slow, rerun on Linux before using these numbers

## Segments and cleanup

<!-- When does a new segment start? When is an old one safe to delete? -->
A new segment should start when a flush job is started.
We are safe to delete a segment after it has been written to the manifest and fsync is compete. If we delete pior to this, we would not be able to restore if fsync or update to manifest fails. Leading us to have incorrect data. 

- V1 for now: one file (`wal.log`). Segments come with flush
- When the WAL file is created, its parent directory is fsynced too. Otherwise a crash can lose the new file's directory entry even if its contents were fsynced

## Failure cases

<!-- Torn tail, mid-log corruption, fsync failure: what happens in each? -->
- Torn writes
    - We should be able to tell by the lengths by checking the number of bytes left in the file verse the number o bytes the length tells us to expect. We should see that the written lenght does not match the length so we will cut the WAL off at the end of the last valid record
    - decode returns `Ok(None)` when there aren't enough bytes for the header or for the full record
    - Replay (next) will truncate the file at the end of the last valid record

- Mid log corruption 
    - We will use CRC to check for corruption of our data
    - if CRC fails
        - stop and return corruption error (v1)
        - WALRecoveryMode (v2)
    - decode returns `Err(Corruption { reason })`

| CorruptionReason | When |
|---|---|
| `LengthLimit(Key)` / `LengthLimit(Value)` | keyLen or valueLen over the limit |
| `CrcMismatch` | Stored CRC doesn't match the bytes |
| `UnknownType` | Type byte isn't 01 or 02 (checked after the CRC) |
| `TombstoneValue` | Type 02 with valueLen > 0 |

- Fsync Failure
    - when fsync fails 
        - return error to caller (`WalError::Io`)
        - stop accepting writes, mark store as failed as WAL is not trustable anymore
        - Every later append returns `WalError::Failed`
    - A failed `write_all` is handled the same way. A partial write would leave half a record in the file, and anything written after it would look like mid-log corruption

- Bad input
    - A key or value over the limit returns `WalError::Size` before anything is written
    - The writer stays healthy, since nothing reached the disk

## Interface

<!-- The functions this module exposes (names and signatures, no bodies). -->
- Record
    - `Record { key: Vec<u8>, op: Op }`
    - `Op::Put { value: Vec<u8> }` / `Op::Delete`
- Codec
    - `encode(record: &Record) -> Result<Vec<u8>, WalError>`
    - `decode(encoded_record: &[u8]) -> Result<Option<(Record, usize)>, WalError>`
        - `Ok(Some((record, bytes_used)))`: valid record
        - `Ok(None)`: torn tail, not enough bytes
        - `Err(Corruption { reason })`: bytes are there but wrong
- WalWriter
    - `WalWriter::open(path: &Path) -> Result<WalWriter, WalError>`: creates the file if needed, appends to the end otherwise
    - `append(&mut self, record: &Record) -> Result<(), WalError>`: durable when it returns Ok
- Replay (next)
    - Replay will return a vector for V1 and a iterator for v2 with V1 vs V2 benchmarks
    - Torn tail is not an error, replay returns the records before it as `Ok(...)`
    - Corruption returns `Err(...)` with the reason and the file offset of the bad record
- WalError
    - `Corruption { reason }`, `Io(std::io::Error)`, `Size { .. }`, `Failed`
    - `From<std::io::Error>` lets `?` turn I/O errors into `WalError::Io`

## Tests

<!-- What will prove this works? Include crash/truncation tests. -->
- Codec (`src/wal/codec/tests.rs`): 18 tests, done
    - Encode: Put and Tombstone layouts byte for byte, key and value at and over the limit
    - Decode: Put/Delete round trip with bytes_used, limits round trip, every truncation point returns `Ok(None)`, each CorruptionReason, bit flips, back-to-back records, corrupt length in range

- Writer (`src/wal/writer/tests.rs`): 6 tests, done
    - File holds exactly the encoded record
    - Several appends come back in order
    - Reopen appends to the end, doesn't overwrite
    - Key too big writes nothing and the writer keeps working
    - Failed writer refuses appends
    - Open in a missing directory returns `Io`

- Replay (next)
    1. Write, close, reopen, replay returns the records
    2. Torn write test
        - write N records, truncate the file at every possible byte offset, replay each one and check that you get back exactly the records before the cut 
    3. After a torn tail, append a new record and replay returns old records + the new one
    4. Bit flip in middle of log returns corruption error with the right offset
    5. Empty WAL replay

- Integration Test
    1. Test put(key, value)
        - Should write to WAL 
    2. Test torn write from put(key, value)
    3. Test Fsync fail - (out of scope for v1, a real fsync failure can't be triggered on demand)

## Open questions

- Corruption errors require a offset. This will be handled by the replay instead of the WAL, since decode only sees a buffer
- A corrupted length that is still under the limit (e.g. valueLen 2000 -> 3000) at the end of the file looks exactly like a torn tail, so decode returns `Ok(None)`. Covered by `decode_corrupt_in_range`
- If the last record has all its bytes but fails the CRC, is it a torn tail or corruption?
- Rerun the fsync numbers on Linux
- Group commit design: who waits, and how many records per fsync

## Decisions

- encode borrows the record (`&Record`). After encoding, put still needs the same record to insert into the memtable
- Record is a struct with the key plus an `Op` enum, so `record.key` is always available and Put/Delete are named variants

## See also

DESIGN.md, section 6 · [BENCHMARKS.md](BENCHMARKS.md) · [RECOVERY.md](RECOVERY.md)

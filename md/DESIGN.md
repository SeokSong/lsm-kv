# lsm-kv Design Doc

Last updated Nov 16. Details live in the component docs (linked per section). This doc keeps the decisions and the reasons.

## 0. North star

- Compare my own sim telemetry against real F1 drivers and against friends who install the app
- Any car against any car: users drive whatever they own. Laps line up by track, never by car
- One Windows app with lsm-kv embedded, no server. Real F1 laps fetched from OpenF1 on demand, friends share by file. See [PRODUCT.md](PRODUCT.md)

## 1. Goals and non-goals
- Goals:
    - A write is durable when a put returns
    - A delete is durable when a delete returns
    - Durable operations are still written to disk even after a crash
    - Crash recovered within 50ms (idk if this is reasonable)
        - Not measured yet. Recovery is: load the manifest, open the SSTables (footer, index, bloom: ~50-90 us each), replay the WAL segments. Measure with the week 5 crash tests
    - Write is idempotency
    - Delete is idempotency
    - idempotency ensures when we crash we are resume where we left off
    - A finished session (iRacing, a friend's export, a historical race) loads all or nothing (bulk load)

- Non-goals (v1):
    - TCP connection
        - No live connection, we care about getting all our packets
    - A server: everything runs on the user's PC (decided Nov 16, [PRODUCT.md](PRODUCT.md))
    - Snapshots / reading the store as of an older seq
    - Value compression
- Moved out of the non-goals:
    - Multi-threading: done in v1 (PR 3b). The store is shared across threads as `Arc<Store>`, see [CONCURRENCY.md](CONCURRENCY.md)
    - The FastF1 replay script: FastF1 needs a paid F1TV plan since Oct 2026. Real F1 data comes from OpenF1 (2023 and later) instead

## 2. API
- put(key, value) -> Result<()>
- get(key) -> Result<Option<value>>
- delete(key) -> Result<()>
- Key/value types and size limits: 
    - Key: 16 bytes
    - Values: 64 KB
- scan(start, end) -> Resuilt(Iterator(key, value))
    - Not built yet (week 5). Start inclusive, end exclusive. The merge iterator and `SstReader::range` it builds on are done
- Bulk load (done): `begin_ingest()` -> `Ingest::put(key, value)` in key order -> `Ingest::commit()`. See [BULK_LOAD.md](BULK_LOAD.md)
- `open(dir, config)`, `close()`, `stats()`, `last_seq()`. Full list: [STORE.md](STORE.md)

## 3. Architecture overview
    - SSTables 
        - Sorted String Tables
    - Memtable
        - Whenever memtable reaches or gets close to capacity, start a background process job of flush 
    - Write ahead log 
        - Append the operation to disk
        - Confirm write is successful 
        - remove from log after flsuh and recored in manifest 

```
put / delete ──► WAL segment (append + fsync) ──► active memtable
                                                     │ full: swap
                                                     ▼
                                           frozen memtable queue ──► flush thread ──► SSTable
                                                                                         │
bulk load (a finished session) ─────────────────────────────────────────► SSTables ──────┤
                                                                                         ▼
get / scan ◄── active memtable, frozen memtables, SSTables (newest max_seq first) ◄── MANIFEST lists
                                                                                    live files
```

- Bootstrap actions
    - Whenever KV is started up, check WAL to ensure we have no prior processed that didnt get processed due to crash 
    - In order: load the manifest, delete every file it doesn't list, open the listed SSTables, replay every listed WAL segment, flush the older ones, keep the newest as the active one. See [RECOVERY.md](RECOVERY.md)

- Write: 
    - user uses put(key, value) -> write operation to WAL
                                fsync
                                -> write to memtable
                                <- return success write
                                -> check memtable threshold against capacity
                                    -> if threshold >= capacity 
                                        -> start background Flush job 
    - As built: the size check happens before the append. If the put would push the memtable over the threshold, the memtable is swapped first, so a put always lands in one memtable. A rejected put uses no seq
- Flush:
    - job is started
        - Freeze memtable 
        - start a new memtable and WAL segment 
        - write to SSTable 
            - if job crashes
                - Delete any SSTable file the manifest doesnt list
        - fsync 
        - update manifest 
        - if job crashes
                - Remove old WAL Segment up to which data is flushed
        - delete old WAL segment 
    - As built: the new segment is listed in the manifest before its file exists. A failed SSTable write is retried with a growing wait (newer memtables can flush meanwhile), a failed manifest save marks the store failed. See [FLUSH.md](FLUSH.md)

- Compaction 
    - Merge SSTables together
        - Merge 2 storted linked list
        - Keep newest
        - drop TOMESTOME after SSTables pick up this TOMBSTONE?
    - Decided (not built): leveled, its own thread. A k-way merge (heap) of any number of files, the highest seq per key wins. A tombstone is dropped only when compacting into the bottom level that holds its key range. Output files split by namespace + session. See [COMPACTION.md](COMPACTION.md), the merge itself is done ([MERGE.md](MERGE.md))
    
- Get 
    - Check the active memtable to see if key exist
    - Check the memtable being flushed
    - Check SSTables newest to oldest
        - Check against bloom filter to see if this exist in SSTables
    - if not found or TOMBSTONE return OK(None)
    - As built: "newest" means highest `max_seq`, not file order (a retried flush can land after a newer one). A file whose first / last key range doesn't hold the key is skipped before the bloom filter

- Delete

    - write 
        - put(key, TOMESTONE)
        -  return Ok()

- Manifest 
    - Write
        - temp write
        - fsync
        - rename 
        - fsync
    - Lists every live WAL segment and SSTable (with seq and key ranges), next file number, last seq. See [MANIFEST.md](MANIFEST.md)

## 4. Write path
- Put / delete: [STORE.md](STORE.md#write-path-put--delete). Bulk load: [BULK_LOAD.md](BULK_LOAD.md)

## 5. Read path
- Get: [STORE.md](STORE.md#read-path-get). Scan: week 5, on top of [MERGE.md](MERGE.md)

## 6. Data Model

- Key: Big Endian   
    - namespace: 1 byte
        - 0x01 telemetry
            - session_id: 4 bytes 
            - car_idx: 2 bytes
            - lap: 2 bytes 
            - sector: 1 byte
            - seq: 2 bytes
        - 0x02 position
            - session_id: 4 bytes
            - car_idx: 2 bytes
            - lap: 2 bytes 
            - sector: 1 byte
            - seq: 2 bytes
        - 0x03 session metadata
            - session_id: 4 bytes 
        - 0x04 driver map
            - session_id: 4 bytes
            - car_idx 2 bytes
        - 0x05 sim telemetry (iRacing, added Nov 16)
            - session_id: 4 bytes
            - driver_id: 4 bytes
            - lap: 2 bytes
            - sector: 1 byte
            - chunk: 2 bytes
        - 0x06 sim position
            - same as 0x05
        - 0x07 sim session metadata
            - session_id: 4 bytes
            - driver_id: 4 bytes

| Field | Real F1 (0x01-0x04) | Sim (0x05-0x07) |
|---|---|---|
| session_id | OpenF1 `session_key` (Spa 2023 race: 9141). Unique, not increasing over time | Session start from the `.ibt` header, Unix seconds. Unique per driver, sorts a driver's sessions by date |
| car_idx / driver_id | Driver number (2 B) | iRacing customer id of the user's own car (4 B). Same person on any PC, only shared when the user exports a session |
| lap | Lap number | `Lap` channel |
| sector | 1-3 | 1-N from iRacing's split points (Spa: 4) |
| seq / chunk | Chunk number inside the sector, from 0 | Same |

- seq vs chunk: the last field numbers the chunks of one sector. It's called `chunk` in the sim keys so it isn't confused with the store's sequence numbers. Same meaning in the F1 keys
- Sim keys are 14 bytes, F1 keys 12, both inside the 16-byte limit
- Session before driver in sim keys: the 5-byte split rule (namespace + session) then gives each session its own files, same as F1, so export stays cheap without a per-namespace setting
- Sim data never sorts next to F1 data, separate namespaces

- Big Endian Decision
    - Big Endian naturally sorts 0 to 255; where as little endian scrambles the order. due to this since our seqences are numerically ordered, if we use big endian we can compare the raw bytes with no decoding; however, with little endian we would require a decoding for every comparsion. A comparision is a very popular thing done in "put" and "get". This would get expensive when we could just use big endian and get this decoding for free

Why this key. The goal of our search will really be driver specific. We want to see how much a driver brakes, where they put their car and etc.
This also gives us flexability as if we want to construct everything from T1 to T2 we can use run multiple of these scans

- Example
    01 | 00 2C FD DC | 00 05 | 00 0C | 03 | 00 01
    01 | 00 2C FD DC | 00 05 | 00 0C | 03 | 00 02
    01 | 00 2C FD DC | 00 05 | 00 0C | 03 | 00 03
    01 | 00 2C FD DC | 00 05 | 00 0C | 03 | 00 04
    01 | 00 2C FD DC | 00 05 | 00 0C | 03 | 00 05
    01 | 00 2C FD DC | 00 05 | 00 0C | 03 | 00 06

- Sim example: the W13 Spa file (session started 2026-11-16 22:51:59 UTC = 1794869519), driver 123456, lap 2, sector 3, chunk 0
    05 | 6A FB 89 0F | 00 01 E2 40 | 00 02 | 03 | 00 00

### Value format (column chunks), decided Nov 16

- Value Format: Little Endian
    - Live view reads from forwarder directly and store is durable record
    - SOA
        - header_lenght u16
        - version 2 byte
        - start u64
        - end u64
        - count u16
        - schema_id
            - Replaced by a column list in the header: different cars and sources have different channels (F1: DRS, GT3: ABS / TC, OpenF1: no steering), a fixed schema id would need a new id for every channel set

start(c)    = header_len + count × (w₀ + w₁ + … + w_{c−1})   ← only the columns BEFORE c
sample(c,i) = start(c) + i × w_c

| Offset | Field | Size | |
|---|---|---|---|
| 0 | header_len | u16 | Bytes before the first column. Lets a reader skip header fields added later |
| 2 | version | u16 | 1 |
| 4 | start | u64 | Session time of the first row, microseconds |
| 12 | end | u64 | Session time of the last row, microseconds |
| 20 | count | u16 | Rows in this chunk |
| 22 | column_count | u16 | |
| 24 | columns | 3 B each | Column id (u16) + type (u8), from the channel catalog |
| header_len | column data | count x width each | Column 0's rows, then column 1's, ... (the formulas above) |

- Header is 24 + 3 x column_count bytes
- Every chunk has the `time` column. Other columns are whatever the source has
- A reader skips column ids it doesn't know (an older app reading a newer friend's export), using the type byte for the width
- No value CRC, same as before: the WAL and SSTable CRCs cover values

| Type byte | Type | Width |
|---|---|---|
| 1 | u8 | 1 |
| 2 | i8 | 1 |
| 3 | u16 | 2 |
| 4 | u32 | 4 |
| 5 | i32 | 4 |
| 6 | f32 | 4 |
| 7 | f64 | 8 |

Channel catalog (ids are part of the format, never reused):

| Id | Column | Type | Unit | iRacing | OpenF1 |
|---|---|---|---|---|---|
| 1 | time | u32 | us since `start` | `SessionTime` | `date` |
| 2 | lap_dist | f32 | m | `LapDist` | computed at ingest (from speed and time) |
| 3 | lap_dist_pct | f32 | 0..1 | `LapDistPct` | computed at ingest |
| 4 | speed | f32 | m/s | `Speed` | `speed` (km/h converted) |
| 5 | throttle | f32 | 0..1 | `Throttle` | `throttle` / 100 |
| 6 | brake | f32 | 0..1 | `Brake` | `brake` / 100 |
| 7 | steering | f32 | rad | `SteeringWheelAngle` | none |
| 8 | gear | i8 | -1 reverse, 0 neutral | `Gear` | `n_gear` |
| 9 | rpm | f32 | rpm | `RPM` | `rpm` |
| 10 | drs | u8 | source value | none | `drs` |
| 20 | lat | f64 | degrees | `Lat` | none |
| 21 | lon | f64 | degrees | `Lon` | none |
| 22 | alt | f32 | m | `Alt` | none |
| 23 | x | f32 | source units | none | `x` |
| 24 | y | f32 | source units | none | `y` |
| 25 | z | f32 | source units | none | `z` |

- Ids 1-19: telemetry stream (0x01 / 0x05), 20-39: position stream (0x02 / 0x06), 40+: car specific (ABS, TC, ERS, ...) added when a source has them
- f64 for lat / lon: f32 would round to ~0.5 m at Spa's latitude, too coarse next to a 2 m racing line gap
- Raw samples are stored: exactly what the source gave, at its own rate. Lining laps up on a distance grid happens when comparing, and can be cached as derived data later. Nothing is lost, and the grid can change

Chunking:
- A chunk never spans two sectors: the key says which sector it is
- At most 512 rows per chunk, fewer if a row is wide: rows = min(512, (64 KiB - header) / row bytes)
- The 64 KB value limit is why: one iRacing sector at 60 Hz is ~2,400 rows (~134 KB with ~14 channels), over the limit. One OpenF1 sector at ~3.7 Hz is ~150 rows, one chunk

| Stream | Columns | Bytes per row | 512 rows |
|---|---|---|---|
| iRacing telemetry | time, lap_dist, lap_dist_pct, speed, throttle, brake, steering, gear, rpm | 33 | ~17 KB, 8.5 s |
| iRacing position | time, lat, lon, alt | 24 | ~12 KB |
| OpenF1 car | time, lap_dist, lap_dist_pct, speed, throttle, brake, gear, rpm, drs | 30 | one sector per chunk |
| OpenF1 location | time, x, y, z | 16 | one sector per chunk |

- An hour of iRacing driving: ~420 telemetry + ~420 position chunks, ~12 MB. A race of OpenF1 data: ~13 MB (all 20 cars)

- WAL Record Format: Little Endian
    - type: 1 byte
    - cells 
        - keyLen: 1 bytes
        - valueLen: 4 bytes
        - key
        - value

    - CRC-32: 4 byte
        - Maybe test standard vs Castagnoli
            - Tested: crc32fast (standard) is 3-4x faster than crc32c on the Mac, and never slower on x86 Linux. The WAL uses crc32fast ([BENCHMARKS.md](BENCHMARKS.md))
        - Cover every byte but the CRC-32 byte
        - if CRC fails
            - stop and return corruption error (v1)
            - WALRecoveryMode (v2)
    - As built (seq added in PR 1): `| CRC-32 4B | seq 8B | type 1B | keyLen 1B | valueLen 4B | key | value |`, an 18-byte header. Seqs must strictly increase inside a segment, replay refuses otherwise. See [WAL.md](WAL.md)

    - Torn writes
        - We should be able to tell by the lengths by checking the number of bytes left in the file verse the number o bytes the length tells us to expect. We should see that the written lenght does not match the length so we will cut the WAL off at the end of the last valid record
    - Fysnc
        - Policy - V1 fsync every write, group commit and perodic are benchmark variants
            - write
            - group commit
        - Measured: fsync is the whole cost of a put (~3 ms on the Mac, ~0.3 ms on GitHub's Linux runners). Group commit would help many writers at once, but bulk data now goes through bulk load and live data is ~1.4 puts/s, so group commit is low priority
        - when fsync fails 
            - abort 
            - return error to caller
            - stop accepting writes, mark store as failed as WAL is not trustable anymore
    
    - TOMBSTONE
        - cell
            - key: keyLen
            - valueLen 

        - DB's read a flag bit before loading data payload 
        - Standard cell will have a regular bit; however we will use a DELETED cell 
        - This allows users to put whatever value without ever accidentally interacting with a tombstone
        

Key : 01 | 00 2C FD DC | 00 05 | 00 0C | 03 | 00 01
PUT : | CC CC CC CC | 00 00 00 00 00 00 00 07 (seq) | 01 | 0C | D0 07 00 00 | 12 key bytes | 2,000 value bytes | 
TOMBSTONE: | CC CC CC CC | 00 00 00 00 00 00 00 08 (seq) | 02 | 0C | 00 00 00 00 | 12 key bytes |
(seq shown in reading order, it's little endian on disk)

- Scan
    - A user should be able to scan across a race
    - user should be ale to request a key range and we will deliver all the data for start to end 
    - One lap of one driver is one contiguous key range: `namespace | session | car | lap`. Its chunks come back in sector and chunk order


CRC
 We will not have a value CRC but a WAL CRC and SSTable CRC. this then assumes WAL and SSTable CRC will cover values as well 


## 7. On-disk formats
SSTable
4.17 Hz, p50 of 106 samples per sector, max 242
    - OpenF1 car data is ~3.7 Hz (measured on Spa 2023). iRacing is 60 Hz, see the chunking rule in section 6

Block Size: 16kb
per car, one stream:  53 laps × 3 sectors × ~2 KB  ≈ 320 KB
× 20 cars                                           ≈ 6.4 MB
× 2 streams (car + pos)                             ≈ 13 MB per race


Index last as we append the newest is closet to the end
Bloom filter lets start with 6 bits we want a low false positive rate but make this easy to edit

6bits ~ 5-6% error
    - Measured: missing-key gets are ~10x faster with the filter (0.2 us vs 2.1 us, page cache warm)

Footer fields 22 bytes
- index offset: 4 bytes
- bloom offset: 4 bytes
- index_len: 4 bytes
- bloom_len: 4 byte
- format: 2 bytes
- magic_number: 4 bytes 

SSTable size: 32mb
~2,000 blocks ~2.5races
    - Measured: a full 32 MiB SSTable is 1,854 blocks

- Entry (in a block): `| type 1B | shared 1B | unshared 1B | valueLen 4B | seq 8B | key suffix | value |`. shared = bytes of the key the same as the previous key (prefix compression), reset at every restart point (every 16th entry). See [SSTABLE.md](SSTABLE.md)

Manifest: rewrite over old one, this will make crash recovery much easier, but write a bit slower 
    - Layout and rules: [MANIFEST.md](MANIFEST.md). Version 2 (compaction) adds a level per SSTable

key : value, key: value

Blocking
[block 1 key: value, key: value, key: value][block 2 key: value, key: value]

Indexing
[block 1 key: value, key: value, key: value_0 -> offset 0 len 4096][block 2 key: value, key: value_1 -> offset 1 len 4100]
    - Each index entry holds the block's last key: a lookup finds the first block whose last key is >= the key, one binary search, no need to also store first keys

Bloom filter
bit array
[0]* 26

Footer
file_szie - footer_size
footer_size = index offset +  len, bloom offset + len, format version, magic number

## 8. Durability and crash recovery
- Durable when put / delete returns: WAL append + fsync first
- Every file is written as tmp, fsync, rename, fsync dir. A crash never leaves a half-written file under a real name
- The manifest is the truth: files it doesn't list are deleted at open, listed files that are missing or the wrong size fail open
- Tested at every flush step and for bulk load before commit. Real `kill -9` tests in week 5. See [RECOVERY.md](RECOVERY.md)

## 9. Flush and compaction policy
- Flush: when the next put would push the memtable over 32 MiB, one frozen memtable waiting at a time (puts wait when the queue is full). [FLUSH.md](FLUSH.md)
- Compaction: L0 -> L1 at 4 files, deeper levels by size (10x per level). [COMPACTION.md](COMPACTION.md)
- Bulk-loaded files skip both: they go straight to the bottom level (once levels exist)

## 10. Concurrency model (v1)
- Three locks, always taken WAL -> Manifest -> State, one flush thread, a compaction thread planned. 13 rules. [CONCURRENCY.md](CONCURRENCY.md), diagram in [diagrams/threads-and-locks.drawio](diagrams/threads-and-locks.drawio)

## 11. Testing and benchmark plan
- Tests: unit tests per module, crash tests at every flush step, model checks (merge), deadlock guards with timeouts, deliberate breaks to check the tests can fail, repeated runs for flaky tests (`scripts/repeat-tests.sh`). 200 tests, CI on Linux and macOS
- Benchmarks: criterion, in the Linux dev container and on GitHub Actions runners (x86, real fsync). [BENCHMARKS.md](BENCHMARKS.md), how to run them in [DEVELOPMENT.md](DEVELOPMENT.md)

## 12. Alternatives considered and rejected
Cassandra - Because our use case has 1 node 1 writer we do not need this 
- use LWW: Last Write Wins
    - If 2 entries of a key
        - 1 data
        - 1 tombstone
    - If data timestamp is newer, we will return data
    - if tombstone timestamp is new, we will return tombstone
    - As built: "timestamp" is the store's sequence number, not a clock. The highest seq wins

| Rejected | For | Why |
|---|---|---|
| Size-tiered compaction | Leveled | Every file overlaps, a scan of one driver's race would read every file |
| A central server | A local app with file sharing | Free, nothing to run, no redistributing F1 data, other drivers' info never leaves the PC |
| Requiring Docker for users | A native installer | Docker can't read iRacing's live shared memory, heavy to install. Docker is for development only |
| FastF1 | OpenF1 | FastF1 needs a paid F1TV plan since Oct 2026 |
| Callers write SSTables for bulk load | The store writes them | Every imported entry gets checked, the store controls seqs |
| A fixed `schema_id` per value | A column list in each chunk | Any car / source can add channels without a new schema |
| One chunk per sector | At most 512 rows per chunk | An iRacing sector at 60 Hz is over the 64 KB value limit |
| Resampling at ingest | Raw samples | Nothing lost, the comparison grid can change later |
| A random id per install for sim keys | iRacing customer id | Same person on any PC, friends recognise each other |

## 13. Open questions
- Sim key order (session before driver) and session id (start time in Unix seconds): chosen so each session gets its own files. Two of a driver's sessions can't start in the same second, but double check against real `.ibt` files
- Session metadata values (0x03, 0x07): track id, track name, car, date, source, out / in lap flags. Format decided with the ingest PR
- OpenF1 lap distance is computed from speed and time, and drifts. Correct it per lap with the official lap length
- Track identity across sources: iRacing `TrackID` vs OpenF1 `circuit_key`, and lap lengths differ (iRacing Spa 6.929 km). Laps line up by `lap_dist_pct`
- Recovery time goal (50 ms): measure in week 5

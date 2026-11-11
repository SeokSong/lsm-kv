# lsm-kv Design Doc

## 1. Goals and non-goals
- Goals:
    - A write is durable when a put returns
    - A delete is durable when a delete returns
    - Durable operations are still written to disk even after a crash
    - Crash recovered within 50ms (idk if this is reasonable)
    - Write is idempotency
    - Delete is idempotency
    - idempotency ensures when we crash we are resume where we left off




- Non-goals (v1):
    - Physon script to simulate with FastF1 data
        - put
        - get
        - scan
        - delete 
    - TCP connection
        - No live connection, we care about getting all our packets
    - Multi-threading (V2)

## 2. API
- put(key, value) -> Result<()>
- get(key) -> Result<Option<value>>
- delete(key) -> Result<()>
- Key/value types and size limits: 
    - Key: 16 bytes
    - Values: 64 KB
- scan(start, end) -> Resuilt(Iterator(key, value))

## 3. Architecture overview
    - SSTables 
        - Sorted String Tables
    - Memtable
        - Whenever memtable reaches or gets close to capacity, start a background process job of flush 
    - Write ahead log 
        - Append the operation to disk
        - Confirm write is successful 
        - remove from log after flsuh and recored in manifest 
(A simple diagram or a numbered write path and read path.)

- Bootstrap actions
    - Whenever KV is started up, check WAL to ensure we have no prior processed that didnt get processed due to crash 

- Write: 
    - user uses put(key, value) -> write operation to WAL
                                fsync
                                -> write to memtable
                                <- return success write
                                -> check memtable threshold against capacity
                                    -> if threshold >= capacity 
                                        -> start background Flush job 
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
    


- Compaction 
    - Merge SSTables together
        - Merge 2 storted linked list
        - Keep newest
        - drop TOMESTOME after SSTables pick up this TOMBSTONE?
    
- Get 
    - Check the active memtable to see if key exist
    - Check the memtable being flushed
    - Check SSTables newest to oldest
        - Check against bloom filter to see if this exist in SSTables
    - if not found or TOMBSTONE return OK(None)

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


## 4. Write path
## 5. Read path
## 6. Data Model
- WAL record format:

start(c)    = header_len + count × (w₀ + w₁ + … + w_{c−1})   ← only the columns BEFORE c
sample(c,i) = start(c) + i × w_c

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

- Value Format: Little Endian
    - Live view reads from forwarder directly and store is durable record
    - SOA
        - header_lenght u16
        - version 2 byte
        - start u64
        - end u64
        - count u16
        - schema_id

- WAL Record Format: Little Endian
    - type: 1 byte
    - cells 
        - keyLen: 1 bytes
        - valueLen: 4 bytes
        - key
        - value

    - CRC-32: 4 byte
        - Maybe test standard vs Castagnoli
        - Cover every byte but the CRC-32 byte
        - if CRC fails
            - stop and return corruption error (v1)
            - WALRecoveryMode (v2)

    - Torn writes
        - We should be able to tell by the lengths by checking the number of bytes left in the file verse the number o bytes the length tells us to expect. We should see that the written lenght does not match the length so we will cut the WAL off at the end of the last valid record
    - Fysnc
        - Policy - V1 fsync every write, group commit and perodic are benchmark variants
            - write
            - group commit
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
PUT : | CC CC CC CC | 01 | 0C | D0 07 00 00 | 12 key bytes | 2,000 value bytes | 
TOMBSTONE: | CC CC CC CC | 02 | 0C | 00 00 00 00 | 12 key bytes |

- Scan
    - A user should be able to scan across a race
    - user should be ale to request a key range and we will deliver all the data for start to end 


CRC
 We will not have a value CRC but a WAL CRC and SSTable CRC. this then assumes WAL and SSTable CRC will cover values as well 


## 7. On-disk formats
SSTable
4.17 Hz, p50 of 106 samples per sector, max 242

Block Size: 16kb
per car, one stream:  53 laps × 3 sectors × ~2 KB  ≈ 320 KB
× 20 cars                                           ≈ 6.4 MB
× 2 streams (car + pos)                             ≈ 13 MB per race


Index last as we append the newest is closet to the end
Bloom filter lets start with 6 bits we want a low false positive rate but make this easy to edit

6bits ~ 5-6% error

Footer fields 22 bytes
- index offset: 4 bytes
- bloom offset: 4 bytes
- index_len: 4 bytes
- bloom_len: 4 byte
- format: 2 bytes
- magic_number: 4 bytes 

SSTable size: 32mb
~2,000 blocks ~2.5races

Manifest: rewrite over old one, this will make crash recovery much easier, but write a bit slower 

key : value, key: value

Blocking
[block 1 key: value, key: value, key: value][block 2 key: value, key: value]

Indexing
[block 1 key: value, key: value, key: value_0 -> offset 0 len 4096][block 2 key: value, key: value_1 -> offset 1 len 4100]

Bloom filter
bit array
[0]* 26

Footer
file_szie - footer_size
footer_size = index offset +  len, bloom offset + len, format version, magic number



## 11. Alternatives considered and rejected
Cassandra - Because our use case has 1 node 1 writer we do not need this 
- use LWW: Last Write Wins
    - If 2 entries of a key
        - 1 data
        - 1 tombstone
    - If data timestamp is newer, we will return data
    - if tombstone timestamp is new, we will return tombstone

<!-- ## 7. Durability and crash recovery -->
<!-- ## 8. Flush and compaction policy
## 9. Concurrency model (v1)
## 10. Testing and benchmark plan

<!-- ## 12. Open questions --> -->

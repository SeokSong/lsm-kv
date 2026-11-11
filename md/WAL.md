# Write-Ahead Log (WAL)

**Status:**

## Purpose

<!-- What problem does the WAL solve? What would be lost without it? -->
The purpose of the WAL is to ensure that our writes are durable. The memtable sits on RAM which can be erased at any moment. the benefit of the WAL is that it is a log that sits ondisk. this means whenever we add to it, it is appended a O(1) operation (not atomic); however, it is still quite fast compared to a normal write operation. this allows us to keep a record of all the operations that should happen on disk without actually preforming the expensive operation itself. This allows us to confidentily use RAM without missing any operations or unknowing not having a copy due to a crash

## Record format

<!-- Byte-by-byte layout, widths, byte order, what the CRC covers. Include an example record. -->

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

## fsync policy

<!-- v1 policy and why. Which variants will you benchmark? -->
- Fysnc
    - Policy - V1 fsync every write, group commit and perodic are benchmark variants
        - write
        - group commit
    - when fsync fails 
        - abort 
        - return error to caller
        - stop accepting writes, mark store as failed as WAL is not trustable anymore
        

## Segments and cleanup

<!-- When does a new segment start? When is an old one safe to delete? -->
A new segment should start when a flush job is started.
We are safe to delete a segment after it has been written to the manifest and fsync is compete. If we delete pior to this, we would not be able to restore if fsync or update to manifest fails. Leading us to have incorrect data. 

## Failure cases

<!-- Torn tail, mid-log corruption, fsync failure: what happens in each? -->
- Torn writes
    - We should be able to tell by the lengths by checking the number of bytes left in the file verse the number o bytes the length tells us to expect. We should see that the written lenght does not match the length so we will cut the WAL off at the end of the last valid record



- Mid log corruption 
    - We will use CRC to check for corruption of our data
    - if CRC fails
        - stop and return corruption error (v1)
        - WALRecoveryMode (v2)
    
- Fsync Failure
    - when fsync fails 
        - abort 
        - return error to caller
        - stop accepting writes, mark store as failed as WAL is not trustable anymore
        


## Interface

<!-- The functions this module exposes (names and signatures, no bodies). -->
Record - 
- Write a record durably
enum Record {
    key {12 bytes},
    value {Optional<2000 bytes>},
}

- Read every record back at start up

- Delete or switch segments 

Replay will return a vector for V1 and a iterator for v2 with V1 vs V2 benchmarks

The error type needs to tell apart "torn tail, stopped cleanly" from "corruption in the middle". Are those both errors? Is one of them really success?

Replay(T, E)

is torn tail really an error 
No because we will just return the last correct entry 
would return Ok(...)

Corruption would reutrn Err(...) with a variant like Corruption
Byte offset - tells us what the last good byte was 

I/O Errors can occur

## Tests

<!-- What will prove this works? Include crash/truncation tests. -->
- Unit Test
    1. Base Case: Ensure WAL can write a single record 
        - Add record
        - Replay returns the record we just added
    2. Torn write test
        - write N records, truncate the file at every possible byte offset, replat each one and check that you get back exactly the records before the cut 
    3. Write, close, reopen, replay turns record
    4. Bit filp in middle of log
        - return corruption error
    5. Empty WAL replay
    6. put then delete of a key
    7. key longer than 16 bytes
        - rejected
    8. - encode put and Tombstone examples 
        - check they match against example 1_1
        - round trip, 
            - encode
            - decode
            - check against original

- Integration Test
    1. Test put(key, value)
        - Should write to WAL 
       
    2. Test torn write from put(key, value)
    3. Test Fsync fail - (out of scope for v1)

Example for 1_1
Key : 01 | 00 2C FD DC | 00 05 | 00 0C | 03 | 00 01
PUT : | CC CC CC CC | 01 | 0C | D0 07 00 00 | 12 key bytes | 2,000 value bytes | 
TOMBSTONE: | CC CC CC CC | 02 | 0C | 00 00 00 00 | 12 key bytes |


## Open questions

// thinking
// Should we take ownership over record?
// If encode takes ownership, after the function is over it will go out of scope
// If we borrow it the memory will stay after the function
// so the question is do we need this address still after the function?
// yes because all encoding does is make this address readable for our disk, this does not mean were done with the address yet

## See also

DESIGN.md, section 6

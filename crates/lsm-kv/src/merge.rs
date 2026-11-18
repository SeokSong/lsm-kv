// k-way merge of sorted sources (memtables, SSTables) into one sorted stream with one
// entry per key: the one with the highest seq. tombstones are yielded too, the caller
// decides what to do with them (compaction keeps them above the bottom level, scan skips them)
mod error;

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use crate::memtable::Memtable;
use crate::sstable::SstError;
use crate::wal::Record;

pub use error::MergeError;

// what the merge reads: entries in strictly increasing key order, at most one per key.
// SstIter already is one, memtables go through memtable_source / memtable_range_source
pub type Source<'a> = Box<dyn Iterator<Item = Result<Record, SstError>> + 'a>;

// every memtable entry, copied out as a Record. the copy is what get does too (BENCHMARKS.md)
pub fn memtable_source(memtable: &Memtable) -> Source<'_> {
    Box::new(memtable.iter().map(|(key, seq, op)| {
        Ok(Record {
            seq,
            key: key.to_vec(),
            op: op.clone(),
        })
    }))
}

// memtable entries with keys in [start, end), like Memtable::range
pub fn memtable_range_source<'a>(memtable: &'a Memtable, start: &[u8], end: &[u8]) -> Source<'a> {
    Box::new(memtable.range(start, end).map(|(key, seq, op)| {
        Ok(Record {
            seq,
            key: key.to_vec(),
            op: op.clone(),
        })
    }))
}

pub fn merge(sources: Vec<Source<'_>>) -> MergeIter<'_> {
    let source_count: usize = sources.len();
    MergeIter {
        sources,
        heap: BinaryHeap::with_capacity(source_count),
        last_keys: vec![None; source_count],
        refill: (0..source_count).collect(),
        done: false,
    }
}

pub struct MergeIter<'a> {
    sources: Vec<Source<'a>>,
    // the current entry of every source that isn't finished, at most one per source
    heap: BinaryHeap<HeapEntry>,
    // the last key each source gave, to check it's sorted (MergeError::OutOfOrder)
    last_keys: Vec<Option<Vec<u8>>>,
    // sources whose entry was taken off the heap, pulled at the start of the next call.
    // pulling later, not right away, means a read error comes after the entries before it,
    // not in place of one. starts as every source, so the first call fills the heap
    refill: Vec<usize>,
    // every source finished, or an error was yielded
    done: bool,
}

struct HeapEntry {
    record: Record,
    source: usize,
}

// BinaryHeap pops the greatest entry, so "greater" here means: smaller key first, and for
// the same key, higher seq first. the newest version of the smallest key is always on top
impl Ord for HeapEntry {
    fn cmp(&self, other: &HeapEntry) -> Ordering {
        other
            .record
            .key
            .cmp(&self.record.key)
            .then(self.record.seq.cmp(&other.record.seq))
    }
}

impl PartialOrd for HeapEntry {
    fn partial_cmp(&self, other: &HeapEntry) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for HeapEntry {
    fn eq(&self, other: &HeapEntry) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for HeapEntry {}

impl Iterator for MergeIter<'_> {
    type Item = Result<Record, MergeError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        match self.next_entry() {
            Ok(Some(record)) => Some(Ok(record)),
            Ok(None) => {
                self.done = true;
                None
            }
            // stop after an error: carrying on would leave keys out without anyone noticing
            Err(error) => {
                self.done = true;
                Some(Err(error))
            }
        }
    }
}

impl MergeIter<'_> {
    fn next_entry(&mut self) -> Result<Option<Record>, MergeError> {
        while let Some(source) = self.refill.pop() {
            self.pull(source)?;
        }

        let Some(newest) = self.heap.pop() else {
            return Ok(None);
        };
        // its next entry has a bigger key (sources are strictly increasing), so it can wait
        self.refill.push(newest.source);

        // every other entry with this key is an older version: skip it. every source's
        // current entry is on the heap, so all of them are found here
        while let Some(top) = self.heap.peek()
            && top.record.key == newest.record.key
        {
            let older: HeapEntry = self.heap.pop().unwrap();
            if older.record.seq == newest.record.seq {
                return Err(MergeError::DuplicateSeq {
                    seq: older.record.seq,
                });
            }
            self.refill.push(older.source);
        }

        Ok(Some(newest.record))
    }

    // moves the next entry of a source onto the heap, nothing if it's finished
    fn pull(&mut self, source: usize) -> Result<(), MergeError> {
        let record: Record = match self.sources[source].next() {
            None => return Ok(()),
            Some(Err(error)) => return Err(MergeError::Sst(error)),
            Some(Ok(record)) => record,
        };

        // strictly increasing: a smaller key means unsorted, an equal key means a duplicate
        if let Some(last_key) = &self.last_keys[source]
            && record.key.as_slice() <= last_key.as_slice()
        {
            return Err(MergeError::OutOfOrder { source });
        }
        self.last_keys[source] = Some(record.key.clone());

        self.heap.push(HeapEntry { record, source });
        Ok(())
    }
}

#[cfg(test)]
mod tests;

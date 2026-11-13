use std::collections::BTreeMap;
use std::ops::Bound;

use crate::wal::{Op, Record};

// what get found for a key. Deleted means a tombstone: stop searching, don't fall
// through to older SSTables that might still have a value
#[derive(Debug, PartialEq)]
pub enum Lookup<'a> {
    Found(&'a [u8]),
    Deleted,
    NotFound,
}

// latest state of each recent key, sorted by key. one entry per key, a newer write
// replaces the old one. BTreeMap for v1, the structure stays hidden behind this API
#[derive(Debug, Default)]
pub struct Memtable {
    entries: BTreeMap<Vec<u8>, Op>,
    // running total of key + value bytes, updated on every write so it's never re-measured
    size: usize,
}

impl Memtable {
    pub fn new() -> Memtable {
        Memtable::default()
    }

    pub fn put(&mut self, key: Vec<u8>, value: Vec<u8>) {
        self.insert(key, Op::Put { value });
    }

    // a delete is a tombstone write, even if the key isn't in this memtable,
    // since an older SSTable might still have a value for it
    pub fn delete(&mut self, key: Vec<u8>) {
        self.insert(key, Op::Delete);
    }

    // takes ownership so replayed records move in without copying
    pub fn apply(&mut self, record: Record) {
        self.insert(record.key, record.op);
    }

    pub fn get(&self, key: &[u8]) -> Lookup<'_> {
        match self.entries.get(key) {
            Some(Op::Put { value }) => Lookup::Found(value),
            Some(Op::Delete) => Lookup::Deleted,
            None => Lookup::NotFound,
        }
    }

    // every entry in key order, tombstones included (flush writes them to the SSTable too)
    pub fn iter(&self) -> impl Iterator<Item = (&[u8], &Op)> {
        self.entries.iter().map(|(key, op)| (key.as_slice(), op))
    }

    // keys in [start, end): start inclusive, end exclusive, same as the lap scan design.
    // use<'a>: the iterator only borrows the memtable, start and end can be dropped right after
    pub fn range<'a>(
        &'a self,
        start: &[u8],
        end: &[u8],
    ) -> impl Iterator<Item = (&'a [u8], &'a Op)> + use<'a> {
        // BTreeMap::range panics if start > end, an empty range is the right answer instead
        let end: &[u8] = if end < start { start } else { end };

        self.entries
            .range::<[u8], _>((Bound::Included(start), Bound::Excluded(end)))
            .map(|(key, op)| (key.as_slice(), op))
    }

    // key + value bytes, compared against the flush threshold
    pub fn approximate_size(&self) -> usize {
        self.size
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn insert(&mut self, key: Vec<u8>, op: Op) {
        let key_len: usize = key.len();
        let new_value_len: usize = value_len(&op);

        // insert hands back the old entry on overwrite, so the counter is fixed up for free
        match self.entries.insert(key, op) {
            Some(old_op) => {
                self.size = self.size - value_len(&old_op) + new_value_len;
            }
            None => {
                self.size += key_len + new_value_len;
            }
        }
    }
}

fn value_len(op: &Op) -> usize {
    match op {
        Op::Put { value } => value.len(),
        Op::Delete => 0,
    }
}

#[cfg(test)]
mod tests;

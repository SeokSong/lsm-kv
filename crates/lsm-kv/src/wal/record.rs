#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    // sequence number: goes up by one on every put or delete, across every WAL segment and
    // SSTable. a higher seq always means a newer write, whatever order files are flushed in
    pub(crate) seq: u64,
    pub(crate) key: Vec<u8>,
    pub(crate) op: Op,
}

// read-only from outside the crate: anyone can read a record, only the store makes one,
// so seqs always come from the store
impl Record {
    pub fn seq(&self) -> u64 {
        self.seq
    }

    pub fn key(&self) -> &[u8] {
        &self.key
    }

    pub fn op(&self) -> &Op {
        &self.op
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Put { value: Vec<u8> },
    Delete,
}

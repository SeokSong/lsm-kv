#[derive(Debug, PartialEq)]
pub struct Record {
    key: Vec<u8>,
    op: Op,
}

#[derive(Debug, PartialEq)]
pub enum Op {
    Put { value: Vec<u8> },
    Delete,
}

#[derive(Debug)]
pub enum WalError {
    Corruption { offset: u64 },
    Io(std::io::Error),
}

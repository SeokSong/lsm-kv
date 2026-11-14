#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub(crate) key: Vec<u8>,
    pub(crate) op: Op,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Put { value: Vec<u8> },
    Delete,
}

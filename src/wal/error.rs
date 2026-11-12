#[derive(Debug)]
pub enum SizeField {
    Key,
    Value,
}

#[derive(Debug)]
pub enum WalError {
    Corruption {
        offset: u64,
    },
    Io(std::io::Error),
    Size {
        incorrect_size: usize,
        correct_max_size: usize,
        which_field: SizeField,
    },
}

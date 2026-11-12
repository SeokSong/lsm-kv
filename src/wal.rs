mod codec;
mod error;
mod record;

pub use codec::encode;
pub use error::{SizeField, WalError};
pub use record::{Op, Record};

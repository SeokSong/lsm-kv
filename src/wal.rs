mod codec;
mod error;
mod record;

pub use codec::decode;
pub use codec::encode;
pub use error::{CorruptionReason, SizeField, WalError};
pub use record::{Op, Record};

// sorted, immutable files on disk. file layout:
// | data blocks (each with a CRC) | bloom filter (CRC) | index (CRC) | footer 22B |
mod block;
mod bloom;
mod error;
mod index;
mod reader;
mod writer;

pub use error::{SstCorruption, SstError};
pub use reader::{SstIter, SstLookup, SstReader};
pub use writer::{SstInfo, SstWriter};

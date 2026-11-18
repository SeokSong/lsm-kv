// race data on top of the store: reading sources (.ibt files, later OpenF1), laps and
// sectors on the track table's lines, the column chunk value format, keys, and import.
// the store itself (crates/lsm-kv) knows nothing about any of it
pub mod chunk;
pub mod ibt;
pub mod track;

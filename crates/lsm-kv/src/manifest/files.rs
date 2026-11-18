use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use super::Manifest;

pub const MANIFEST_FILE_NAME: &str = "MANIFEST";
const WAL_EXTENSION: &str = "wal";
const SST_EXTENSION: &str = "sst";
const TMP_EXTENSION: &str = "tmp";

// what a file in the store directory is, judged by its name
#[derive(Debug, PartialEq)]
pub enum FileKind {
    Manifest,
    Wal(u64),
    Sst(u64),
    // any "*.tmp": a write (SSTable or manifest) that never got renamed into place
    Temp,
}

// zero padded so the directory lists in file number order: 000007.wal
pub fn wal_file_name(file_number: u64) -> String {
    format!("{file_number:06}.{WAL_EXTENSION}")
}

pub fn sst_file_name(file_number: u64) -> String {
    format!("{file_number:06}.{SST_EXTENSION}")
}

// None for anything the store didn't make, those files are left alone
pub fn parse_file_name(name: &str) -> Option<FileKind> {
    if name == MANIFEST_FILE_NAME {
        return Some(FileKind::Manifest);
    }
    let (stem, extension) = name.rsplit_once('.')?;
    if extension == TMP_EXTENSION {
        return Some(FileKind::Temp);
    }

    // digits only, so "1e3.sst" or "+7.wal" aren't taken for store files
    if stem.is_empty() || !stem.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let file_number: u64 = stem.parse().ok()?;
    match extension {
        WAL_EXTENSION => Some(FileKind::Wal(file_number)),
        SST_EXTENSION => Some(FileKind::Sst(file_number)),
        _ => None,
    }
}

// store files in dir that the manifest doesn't list: SSTables from a flush that crashed
// before the manifest update, WAL segments already flushed, leftover .tmp files.
// a segment that was created but never listed is safe too: the swap saves the manifest
// before any put goes into a new segment, so it can't hold an acked write
pub fn obsolete_files(dir: &Path, manifest: &Manifest) -> std::io::Result<Vec<PathBuf>> {
    let live_wals: HashSet<u64> = manifest.wal_segments.iter().copied().collect();
    let live_ssts: HashSet<u64> = manifest
        .sstables
        .iter()
        .map(|sst| sst.file_number)
        .collect();

    let mut obsolete: Vec<PathBuf> = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        // a name that isn't valid UTF-8 can't be one of ours
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let is_obsolete: bool = match parse_file_name(&name) {
            Some(FileKind::Temp) => true,
            Some(FileKind::Wal(file_number)) => !live_wals.contains(&file_number),
            Some(FileKind::Sst(file_number)) => !live_ssts.contains(&file_number),
            Some(FileKind::Manifest) | None => false,
        };
        if is_obsolete {
            obsolete.push(entry.path());
        }
    }

    // read_dir order is up to the OS, sorted keeps logs and tests stable
    obsolete.sort();
    Ok(obsolete)
}

#[cfg(test)]
mod tests;

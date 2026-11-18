use super::*;
use crate::manifest::SstMeta;
use tempfile::TempDir;

fn sst_meta(file_number: u64) -> SstMeta {
    SstMeta {
        file_number,
        file_size: 100,
        min_seq: 1,
        max_seq: 1,
        first_key: vec![0x01],
        last_key: vec![0x01],
    }
}

#[test]
fn names_are_zero_padded() {
    assert_eq!(wal_file_name(7), "000007.wal");
    assert_eq!(sst_file_name(8), "000008.sst");
    // past 6 digits the name just gets longer
    assert_eq!(sst_file_name(1_234_567), "1234567.sst");
}

#[test]
fn parse_store_files() {
    assert_eq!(parse_file_name("000007.wal"), Some(FileKind::Wal(7)));
    assert_eq!(parse_file_name("000008.sst"), Some(FileKind::Sst(8)));
    assert_eq!(
        parse_file_name("1234567.sst"),
        Some(FileKind::Sst(1_234_567))
    );
    assert_eq!(parse_file_name("MANIFEST"), Some(FileKind::Manifest));
    assert_eq!(parse_file_name("MANIFEST.tmp"), Some(FileKind::Temp));
    assert_eq!(parse_file_name("000008.sst.tmp"), Some(FileKind::Temp));
}

#[test]
fn parse_ignores_other_files() {
    for name in [
        "wal.log",
        "notes.txt",
        ".sst",
        "abc.sst",
        "+7.wal",
        "1e3.sst",
        "000007.WAL",
        "99999999999999999999999.sst", // bigger than u64
        "MANIFEST.old",
        "",
    ] {
        assert_eq!(parse_file_name(name), None, "{name:?}");
    }
}

#[test]
fn names_round_trip() {
    for file_number in [1, 42, 999_999, 1_000_000, u64::MAX] {
        assert_eq!(
            parse_file_name(&wal_file_name(file_number)),
            Some(FileKind::Wal(file_number))
        );
        assert_eq!(
            parse_file_name(&sst_file_name(file_number)),
            Some(FileKind::Sst(file_number))
        );
    }
}

#[test]
fn obsolete_files_are_the_unlisted_store_files() {
    let dir: TempDir = TempDir::new().unwrap();
    let manifest: Manifest = Manifest {
        next_file_number: 8,
        last_seq: 1,
        wal_segments: vec![6, 7],
        sstables: vec![sst_meta(3), sst_meta(5)],
    };
    for name in [
        "MANIFEST",
        "000003.sst", // live
        "000005.sst", // live
        "000006.wal", // live
        "000007.wal", // live
        "000004.sst", // flush finished, crashed before the manifest update
        "000002.wal", // flushed, crashed before the delete
        "000005.sst.tmp",
        "MANIFEST.tmp",
        "notes.txt", // not ours, left alone
        "wal.log",   // not ours either
    ] {
        fs::write(dir.path().join(name), b"x").unwrap();
    }

    let obsolete: Vec<PathBuf> = obsolete_files(dir.path(), &manifest).unwrap();

    let expected: Vec<PathBuf> = ["000002.wal", "000004.sst", "000005.sst.tmp", "MANIFEST.tmp"]
        .iter()
        .map(|name| dir.path().join(name))
        .collect();
    assert_eq!(obsolete, expected);
}

#[test]
fn nothing_obsolete_in_a_clean_directory() {
    let dir: TempDir = TempDir::new().unwrap();
    let manifest: Manifest = Manifest {
        next_file_number: 3,
        last_seq: 1,
        wal_segments: vec![2],
        sstables: vec![sst_meta(1)],
    };
    for name in ["MANIFEST", "000001.sst", "000002.wal"] {
        fs::write(dir.path().join(name), b"x").unwrap();
    }

    assert!(obsolete_files(dir.path(), &manifest).unwrap().is_empty());
}

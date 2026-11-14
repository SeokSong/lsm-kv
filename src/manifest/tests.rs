use super::*;
use crate::config::SstableConfig;
use crate::sstable::SstWriter;
use crate::wal::Op;
use tempfile::TempDir;

#[test]
fn load_missing_manifest_is_a_new_store() {
    let dir: TempDir = TempDir::new().unwrap();

    assert!(Manifest::load(dir.path()).unwrap().is_none());
}

#[test]
fn save_then_load() {
    let dir: TempDir = TempDir::new().unwrap();
    let mut manifest: Manifest = Manifest::new();
    let wal_segment: u64 = manifest.new_file_number();
    manifest.wal_segments.push(wal_segment);

    manifest.save(dir.path()).unwrap();

    assert_eq!(Manifest::load(dir.path()).unwrap(), Some(manifest));
    // the temp file was renamed into place, not left behind
    assert!(!dir.path().join("MANIFEST.tmp").exists());
}

#[test]
fn save_replaces_the_old_manifest() {
    let dir: TempDir = TempDir::new().unwrap();
    let mut manifest: Manifest = Manifest::new();
    let first_segment: u64 = manifest.new_file_number();
    manifest.wal_segments.push(first_segment);
    manifest.save(dir.path()).unwrap();

    // the swap: a second segment goes live
    let second_segment: u64 = manifest.new_file_number();
    manifest.wal_segments.push(second_segment);
    manifest.save(dir.path()).unwrap();

    let loaded: Manifest = Manifest::load(dir.path()).unwrap().unwrap();
    assert_eq!(loaded.wal_segments, vec![1, 2]);
    assert_eq!(loaded.next_file_number, 3);
}

#[test]
fn crash_before_rename_keeps_the_old_manifest() {
    let dir: TempDir = TempDir::new().unwrap();
    let mut manifest: Manifest = Manifest::new();
    let wal_segment: u64 = manifest.new_file_number();
    manifest.wal_segments.push(wal_segment);
    manifest.save(dir.path()).unwrap();

    // a crash mid-save leaves a half-written MANIFEST.tmp, the real one is untouched
    fs::write(dir.path().join("MANIFEST.tmp"), b"half a manife").unwrap();

    assert_eq!(Manifest::load(dir.path()).unwrap(), Some(manifest.clone()));
    // and the leftover is cleaned up as garbage
    assert_eq!(
        obsolete_files(dir.path(), &manifest).unwrap(),
        vec![dir.path().join("MANIFEST.tmp")]
    );
}

#[test]
fn invalid_manifest_is_never_saved() {
    let dir: TempDir = TempDir::new().unwrap();
    let mut manifest: Manifest = Manifest::new();
    // file 1 was never handed out
    manifest.wal_segments.push(1);

    let result = manifest.save(dir.path());

    assert!(matches!(
        result,
        Err(ManifestError::Invalid(
            ManifestCorruption::FileNumberNotAllocated(1)
        ))
    ));
    assert!(!dir.path().join(MANIFEST_FILE_NAME).exists());
}

#[test]
fn corrupt_manifest_fails_load() {
    let dir: TempDir = TempDir::new().unwrap();
    Manifest::new().save(dir.path()).unwrap();

    let path: PathBuf = dir.path().join(MANIFEST_FILE_NAME);
    let mut bytes: Vec<u8> = fs::read(&path).unwrap();
    bytes[8] ^= 0x01;
    fs::write(&path, &bytes).unwrap();

    assert!(matches!(
        Manifest::load(dir.path()),
        Err(ManifestError::Corruption(ManifestCorruption::Crc))
    ));
}

#[test]
fn new_file_numbers_count_up_from_one() {
    let mut manifest: Manifest = Manifest::new();

    assert_eq!(manifest.new_file_number(), 1);
    assert_eq!(manifest.new_file_number(), 2);
    assert_eq!(manifest.next_file_number, 3);
}

#[test]
fn sst_meta_from_a_real_sstable() {
    let dir: TempDir = TempDir::new().unwrap();
    let mut manifest: Manifest = Manifest::new();
    let file_number: u64 = manifest.new_file_number();
    let path: PathBuf = dir.path().join(sst_file_name(file_number));

    let mut writer: SstWriter = SstWriter::create(&path, &SstableConfig::default()).unwrap();
    writer.add(&[0x01, 0x01], 9, &Op::Delete).unwrap();
    writer
        .add(&[0x01, 0x02], 4, &Op::Put { value: vec![1; 10] })
        .unwrap();
    let info: SstInfo = writer.finish().unwrap();

    let meta: SstMeta = SstMeta::new(file_number, &info);
    manifest.last_seq = meta.max_seq;
    manifest.sstables.push(meta.clone());
    manifest.save(dir.path()).unwrap();

    assert_eq!(meta.min_seq, 4);
    assert_eq!(meta.max_seq, 9);
    assert_eq!(meta.first_key, vec![0x01, 0x01]);
    assert_eq!(meta.last_key, vec![0x01, 0x02]);
    assert_eq!(meta.file_size, fs::metadata(&path).unwrap().len());
    assert_eq!(Manifest::load(dir.path()).unwrap(), Some(manifest.clone()));
    assert!(obsolete_files(dir.path(), &manifest).unwrap().is_empty());
}

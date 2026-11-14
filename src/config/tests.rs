use super::*;
use std::path::PathBuf;

fn config_file(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("config")
        .join(name)
}

#[test]
fn default_values() {
    let config: Config = Config::default();

    assert_eq!(config.memtable.flush_threshold_bytes, 32 * 1024 * 1024);
    assert_eq!(config.memtable.max_frozen, 1);
    assert_eq!(config.sstable.block_size_bytes, 16 * 1024);
    assert_eq!(config.sstable.restart_interval, 16);
    assert_eq!(config.sstable.bloom_bits_per_key, 6);
    assert_eq!(config.flush.max_attempts, 3);
    assert_eq!(config.flush.retry_backoff_ms, 100);
}

#[test]
fn default_toml_matches_code_defaults() {
    // keeps config/default.toml and Default from drifting apart
    let config: Config = Config::load(&config_file("default.toml")).unwrap();

    assert_eq!(config, Config::default());
}

#[test]
fn profile_only_overrides_what_it_sets() {
    let config: Config = Config::load(&config_file("memtable-16mb.toml")).unwrap();

    assert_eq!(config.memtable.flush_threshold_bytes, 16 * 1024 * 1024);
    // not in the file, keeps the default
    assert_eq!(config.memtable.max_frozen, 1);

    let config: Config = Config::load(&config_file("memtable-64mb.toml")).unwrap();
    assert_eq!(config.memtable.flush_threshold_bytes, 64 * 1024 * 1024);
}

#[test]
fn empty_file_is_all_defaults() {
    let config: Config = Config::from_toml_str("").unwrap();

    assert_eq!(config, Config::default());
}

#[test]
fn unknown_setting_is_rejected() {
    // a typo should fail loudly instead of silently running with the default
    let result = Config::from_toml_str("[memtable]\nflush_threshhold_bytes = 1024\n");

    assert!(matches!(result, Err(ConfigError::Parse(_))));
}

#[test]
fn zero_flush_threshold_is_rejected() {
    let result = Config::from_toml_str("[memtable]\nflush_threshold_bytes = 0\n");

    assert!(matches!(
        result,
        Err(ConfigError::Invalid {
            field: "memtable.flush_threshold_bytes",
            ..
        })
    ));
}

#[test]
fn zero_max_frozen_is_rejected() {
    let result = Config::from_toml_str("[memtable]\nmax_frozen = 0\n");

    assert!(matches!(
        result,
        Err(ConfigError::Invalid {
            field: "memtable.max_frozen",
            ..
        })
    ));
}

#[test]
fn missing_file_is_io_error() {
    let result = Config::load(&config_file("does-not-exist.toml"));

    assert!(matches!(result, Err(ConfigError::Io(_))));
}

#[test]
fn zero_block_size_is_rejected() {
    let result = Config::from_toml_str("[sstable]\nblock_size_bytes = 0\n");

    assert!(matches!(
        result,
        Err(ConfigError::Invalid {
            field: "sstable.block_size_bytes",
            ..
        })
    ));
}

#[test]
fn zero_restart_interval_is_rejected() {
    let result = Config::from_toml_str("[sstable]\nrestart_interval = 0\n");

    assert!(matches!(
        result,
        Err(ConfigError::Invalid {
            field: "sstable.restart_interval",
            ..
        })
    ));
}

#[test]
fn zero_bloom_bits_turns_filter_off() {
    // 0 is allowed: no bloom filter
    let config: Config = Config::from_toml_str("[sstable]\nbloom_bits_per_key = 0\n").unwrap();

    assert_eq!(config.sstable.bloom_bits_per_key, 0);
}

#[test]
fn zero_flush_attempts_is_rejected() {
    let result = Config::from_toml_str("[flush]\nmax_attempts = 0\n");

    assert!(matches!(
        result,
        Err(ConfigError::Invalid {
            field: "flush.max_attempts",
            ..
        })
    ));
}

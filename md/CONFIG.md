# Config

**Status:** Done for the memtable and SSTable settings (11 tests). New settings get added as each component needs them.

## Purpose

- Tuning settings live in TOML files so benchmarks can change them without recompiling
- Format constants stay in code: `MAX_KEY_LEN`, `MAX_VALUE_LEN`, `HEADER_LEN`, type bytes
    - They define the on-disk format, changing them makes existing files unreadable
    - The compile-time asserts depend on them

| | Examples | Where |
|---|---|---|
| Format constants | key/value limits, WAL header layout, type bytes | Code (`src/wal/codec.rs`) |
| Tuning settings | memtable flush threshold, max frozen memtables | `config/*.toml` |

## Why TOML

- Rust ecosystem standard (Cargo uses it), maintained `toml` crate
- `serde_yaml` was archived by its author in 2024, YAML would mean depending on a fork

## Files

| File | Use |
|---|---|
| `config/default.toml` | Defaults, a test checks it matches `Config::default()` in code |
| `config/memtable-16mb.toml` | Benchmark profile: 16 MiB flush threshold |
| `config/memtable-64mb.toml` | Benchmark profile: 64 MiB flush threshold |

- A profile only lists what it changes, every missing setting keeps its default
- An empty file is all defaults

## Settings

| Setting | Default | Meaning |
|---|---|---|
| `memtable.flush_threshold_bytes` | 33,554,432 (32 MiB) | Flush once the memtable's key + value bytes reach this. One flush ~ one SSTable |
| `memtable.max_frozen` | 1 | Frozen memtables allowed to wait for the background flush before put has to wait |
| `sstable.block_size_bytes` | 16,384 (16 KiB) | Soft target: a block is closed once it reaches this, a bigger value gets a block to itself |
| `sstable.restart_interval` | 16 | Every Nth entry in a block stores its full key (prefix compression restart point) |
| `sstable.bloom_bits_per_key` | 6 | Bloom filter bits per key, ~5-6% false positives. 10 is ~1%. 0 turns the filter off |

- Later: fsync policy (every write / group commit / periodic)

## Errors

- `ConfigError::Io`: file can't be read
- `ConfigError::Parse`: bad TOML or an unknown setting
    - Unknown settings are rejected (`deny_unknown_fields`) so a typo fails loudly instead of silently running with the default
- `ConfigError::Invalid { field, reason }`
    - `flush_threshold_bytes` = 0
    - `max_frozen` = 0, the first flush could never start
    - `sstable.block_size_bytes` = 0, `sstable.restart_interval` = 0

## Interface

- `Config::default() -> Config`
- `Config::load(path: &Path) -> Result<Config, ConfigError>`
- `Config::from_toml_str(text: &str) -> Result<Config, ConfigError>`
- `config.memtable.*`, `config.sstable.*`

## Tests

- `src/config/tests.rs`: 11 tests, done
    - Default values
    - `config/default.toml` matches the code defaults
    - Profiles only override what they set
    - Empty file is all defaults
    - Unknown setting is rejected
    - Zero flush threshold and zero max frozen are rejected
    - Zero block size and zero restart interval are rejected, zero bloom bits is allowed (filter off)
    - Missing file is an Io error

## See also

[MEMTABLE.md](MEMTABLE.md) · [BENCHMARKS.md](BENCHMARKS.md)

use std::fs;
use std::path::Path;

use serde::Deserialize;

// tuning settings that are safe to change between runs. format constants
// (MAX_KEY_LEN, HEADER_LEN, type bytes) stay in code since they define the on-disk format
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub memtable: MemtableConfig,
    pub sstable: SstableConfig,
    pub flush: FlushConfig,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MemtableConfig {
    // flush once the memtable's key + value bytes reach this
    pub flush_threshold_bytes: usize,
    // frozen memtables allowed to wait for flush before put has to wait
    pub max_frozen: usize,
}

impl Default for MemtableConfig {
    fn default() -> MemtableConfig {
        MemtableConfig {
            flush_threshold_bytes: 32 * 1024 * 1024,
            max_frozen: 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SstableConfig {
    // soft target: a block is closed once it reaches this, a bigger value gets a block to itself
    pub block_size_bytes: usize,
    // every Nth entry in a block stores its full key so lookups can binary search to it
    pub restart_interval: usize,
    // bloom filter bits per key, 0 turns the filter off
    pub bloom_bits_per_key: usize,
}

impl Default for SstableConfig {
    fn default() -> SstableConfig {
        SstableConfig {
            block_size_bytes: 16 * 1024,
            restart_interval: 16,
            bloom_bits_per_key: 6,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FlushConfig {
    // tries per frozen memtable before the store is marked failed
    pub max_attempts: u32,
    // wait before the 2nd try, doubles after each failure: 100, 200, 400 ms
    pub retry_backoff_ms: u64,
}

impl Default for FlushConfig {
    fn default() -> FlushConfig {
        FlushConfig {
            max_attempts: 3,
            retry_backoff_ms: 100,
        }
    }
}

#[derive(Debug)]
pub enum ConfigError {
    Io(std::io::Error),
    Parse(toml::de::Error),
    Invalid {
        field: &'static str,
        reason: &'static str,
    },
}

impl From<std::io::Error> for ConfigError {
    fn from(error: std::io::Error) -> ConfigError {
        ConfigError::Io(error)
    }
}

impl From<toml::de::Error> for ConfigError {
    fn from(error: toml::de::Error) -> ConfigError {
        ConfigError::Parse(error)
    }
}

impl Config {
    // any setting missing from the file keeps its default, so a profile only lists what it changes
    pub fn load(path: &Path) -> Result<Config, ConfigError> {
        let text: String = fs::read_to_string(path)?;
        Config::from_toml_str(&text)
    }

    pub fn from_toml_str(text: &str) -> Result<Config, ConfigError> {
        let config: Config = toml::from_str(text)?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if self.memtable.flush_threshold_bytes == 0 {
            return Err(ConfigError::Invalid {
                field: "memtable.flush_threshold_bytes",
                reason: "must be greater than 0",
            });
        }
        // with no room for a frozen memtable, the first flush could never start
        if self.memtable.max_frozen == 0 {
            return Err(ConfigError::Invalid {
                field: "memtable.max_frozen",
                reason: "must be at least 1",
            });
        }
        if self.sstable.block_size_bytes == 0 {
            return Err(ConfigError::Invalid {
                field: "sstable.block_size_bytes",
                reason: "must be greater than 0",
            });
        }
        if self.sstable.restart_interval == 0 {
            return Err(ConfigError::Invalid {
                field: "sstable.restart_interval",
                reason: "must be at least 1",
            });
        }
        // 0 tries would mark the store failed before a flush ever ran
        if self.flush.max_attempts == 0 {
            return Err(ConfigError::Invalid {
                field: "flush.max_attempts",
                reason: "must be at least 1",
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;

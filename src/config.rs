use std::fs;
use std::path::Path;

use serde::Deserialize;

// tuning settings that are safe to change between runs. format constants
// (MAX_KEY_LEN, HEADER_LEN, type bytes) stay in code since they define the on-disk format
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub memtable: MemtableConfig,
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
        Ok(())
    }
}

#[cfg(test)]
mod tests;

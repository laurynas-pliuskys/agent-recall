use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize)]
pub struct WebServerConfig {
    pub path: String,
    pub url: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct IndexConfig {
    #[serde(default = "IndexConfig::default_auto_index")]
    pub auto_index_on_startup: bool,
    #[serde(default = "IndexConfig::default_writer_heap_mb")]
    pub writer_heap_mb: u32,
    #[serde(default)]
    pub enable_tagging: bool,
    pub cache_dir: Option<PathBuf>,
    pub claude_dir: Option<PathBuf>,
}

impl IndexConfig {
    fn default_auto_index() -> bool {
        true
    }

    fn default_writer_heap_mb() -> u32 {
        512
    }
}

impl Default for IndexConfig {
    fn default() -> Self {
        Self {
            auto_index_on_startup: true,
            writer_heap_mb: Self::default_writer_heap_mb(),
            enable_tagging: false,
            cache_dir: None,
            claude_dir: None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LockingConfig {
    #[serde(default = "LockingConfig::default_enabled")]
    pub enabled: bool,
    pub lock_file: Option<PathBuf>,
}

impl LockingConfig {
    fn default_enabled() -> bool {
        true
    }
}

impl Default for LockingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            lock_file: None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LimitsConfig {
    #[serde(default = "LimitsConfig::default_per_file_chars")]
    pub per_file_chars: usize,
    #[serde(default = "LimitsConfig::default_tool_result_max_chars")]
    pub tool_result_max_chars: usize,
    #[serde(default = "LimitsConfig::default_tool_input_max_chars")]
    pub tool_input_max_chars: usize,
}

impl LimitsConfig {
    fn default_per_file_chars() -> usize {
        150_000
    }
    fn default_tool_result_max_chars() -> usize {
        2000
    }
    fn default_tool_input_max_chars() -> usize {
        200
    }
}

impl Default for LimitsConfig {
    fn default() -> Self {
        Self {
            per_file_chars: 150_000,
            tool_result_max_chars: 2000,
            tool_input_max_chars: 200,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct SearchConfig {
    #[serde(default)]
    pub exclude_patterns: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct Config {
    pub web_server: Option<WebServerConfig>,
    #[serde(default)]
    pub index: IndexConfig,
    #[serde(default)]
    pub locking: LockingConfig,
    #[serde(default)]
    pub limits: LimitsConfig,
    #[serde(default)]
    pub search: SearchConfig,
}

impl Config {
    pub fn load() -> Result<Self> {
        let config_dir = dirs::config_dir()
            .ok_or_else(|| anyhow!("Could not determine config directory"))?
            .join("agent-recall");

        let config_path = config_dir.join("config.yaml");

        let config = if config_path.exists() {
            let config_content = fs::read_to_string(&config_path)?;
            serde_yaml::from_str(&config_content)?
        } else {
            // Create default config if it doesn't exist
            fs::create_dir_all(&config_dir)?;
            let default_config = Self::default();
            let config_content = serde_yaml::to_string(&default_config)?;
            fs::write(&config_path, config_content)?;
            default_config
        };

        Ok(config)
    }

    pub fn get_cache_dir(&self) -> Result<PathBuf> {
        if let Some(cache_dir) = &self
            .index
            .cache_dir
        {
            return Ok(cache_dir.clone());
        }

        let cache = dirs::cache_dir().ok_or_else(|| anyhow!("Could not find cache directory"))?;
        Ok(cache.join("agent-recall"))
    }

    pub fn get_claude_dir(&self) -> Result<PathBuf> {
        if let Some(claude_dir) = &self
            .index
            .claude_dir
        {
            return Ok(claude_dir.clone());
        }

        let home = dirs::home_dir().ok_or_else(|| anyhow!("Could not find home directory"))?;

        let claude_dir = home.join(".claude");
        if claude_dir.exists() {
            return Ok(claude_dir);
        }

        let config_claude_dir = home
            .join(".config")
            .join("claude");
        if config_claude_dir.exists() {
            return Ok(config_claude_dir);
        }

        Ok(claude_dir) // Return default even if it doesn't exist
    }

    pub fn get_lock_file_path(&self) -> Result<PathBuf> {
        if let Some(lock_file) = &self
            .locking
            .lock_file
        {
            return Ok(lock_file.clone());
        }

        let cache_dir = self.get_cache_dir()?;
        Ok(cache_dir.join("index.lock"))
    }

    pub fn get_writer_heap_size(&self) -> usize {
        (self
            .index
            .writer_heap_mb as usize)
            * 1024
            * 1024
    }
}

// Global config instance
use once_cell::sync::OnceCell;
static CONFIG: OnceCell<Config> = OnceCell::new();

pub fn get_config() -> &'static Config {
    CONFIG.get_or_init(|| Config::load().unwrap_or_default())
}

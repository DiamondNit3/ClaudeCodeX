use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub default_provider: String,
    pub default_model: String,
    pub permission_profile: String,
    pub max_agent_turns: usize,
    pub providers: BTreeMap<String, ProviderConfig>,
    #[serde(default)]
    pub mcp: McpConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub kind: ProviderKind,
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderKind {
    Openai,
    Anthropic,
    LocalOpenaiCompatible,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpConfig {
    #[serde(default)]
    pub servers: BTreeMap<String, McpServerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "transport", rename_all = "kebab-case")]
pub enum McpServerConfig {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
    },
    Http {
        url: String,
        #[serde(default)]
        bearer_token_env: Option<String>,
    },
}

impl AppConfig {
    pub fn load_or_default() -> Result<Self> {
        let path = Self::config_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }

        let raw = fs::read_to_string(&path)
            .with_context(|| format!("failed to read config at {}", path.display()))?;
        toml::from_str(&raw)
            .with_context(|| format!("failed to parse config at {}", path.display()))
    }

    pub fn write_default_config() -> Result<PathBuf> {
        let path = Self::config_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let config = Self::default();
        fs::write(&path, toml::to_string_pretty(&config)?)?;
        Ok(path)
    }

    pub fn config_path() -> Result<PathBuf> {
        Ok(config_root()?.join("config.toml"))
    }

    pub fn data_dir() -> Result<PathBuf> {
        Ok(data_root()?)
    }
}

fn config_root() -> Result<PathBuf> {
    if cfg!(windows) {
        let appdata = std::env::var_os("APPDATA").context("APPDATA is not set")?;
        return Ok(PathBuf::from(appdata)
            .join(username_segment())
            .join("ClaudeCodeX"));
    }

    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".config").join("ClaudeCodeX"))
}

fn data_root() -> Result<PathBuf> {
    if cfg!(windows) {
        let appdata = std::env::var_os("APPDATA").context("APPDATA is not set")?;
        return Ok(PathBuf::from(appdata)
            .join(username_segment())
            .join("ClaudeCodeX")
            .join("data"));
    }

    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home)
        .join(".local")
        .join("share")
        .join("ClaudeCodeX"))
}

fn username_segment() -> String {
    std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "username".to_string())
}

impl Default for AppConfig {
    fn default() -> Self {
        let mut providers = BTreeMap::new();
        providers.insert(
            "openai".to_string(),
            ProviderConfig {
                kind: ProviderKind::Openai,
                api_key_env: Some("OPENAI_API_KEY".to_string()),
                base_url: Some("https://api.openai.com/v1".to_string()),
                reasoning_effort: Some("high".to_string()),
                max_output_tokens: Some(8192),
            },
        );
        providers.insert(
            "anthropic".to_string(),
            ProviderConfig {
                kind: ProviderKind::Anthropic,
                api_key_env: Some("ANTHROPIC_API_KEY".to_string()),
                base_url: Some("https://api.anthropic.com/v1".to_string()),
                reasoning_effort: None,
                max_output_tokens: Some(8192),
            },
        );
        providers.insert(
            "local".to_string(),
            ProviderConfig {
                kind: ProviderKind::LocalOpenaiCompatible,
                api_key_env: None,
                base_url: Some("http://localhost:11434/v1".to_string()),
                reasoning_effort: None,
                max_output_tokens: Some(4096),
            },
        );

        Self {
            default_provider: "openai".to_string(),
            default_model: "gpt-5.5".to_string(),
            permission_profile: "ask".to_string(),
            max_agent_turns: 8,
            providers,
            mcp: McpConfig::default(),
        }
    }
}

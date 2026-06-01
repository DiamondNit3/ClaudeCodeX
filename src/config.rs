use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::str::FromStr;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub default_provider: String,
    pub default_model: String,
    pub permission_profile: String,
    #[serde(default)]
    pub effort: EffortLevel,
    pub max_agent_turns: usize,
    pub providers: BTreeMap<String, ProviderConfig>,
    #[serde(default)]
    pub model_profiles: BTreeMap<String, ModelProfile>,
    #[serde(default)]
    pub mcp: McpConfig,
    #[serde(default)]
    pub hooks: HooksConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub kind: ProviderKind,
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub effort: Option<EffortLevel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
    Ollama,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelProfile {
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default = "default_true")]
    pub supports_system: bool,
    #[serde(default)]
    pub prefer_think_false: bool,
    #[serde(default)]
    pub effort: Option<EffortLevel>,
    #[serde(default)]
    pub tool_protocol: ToolProtocol,
    #[serde(default)]
    pub max_tool_prompt_size: Option<usize>,
    #[serde(default)]
    pub reasoning_field: bool,
    #[serde(default)]
    pub context_budget: Option<usize>,
    #[serde(default)]
    pub notes: Option<String>,
}

impl Default for ModelProfile {
    fn default() -> Self {
        Self {
            provider: None,
            supports_system: true,
            prefer_think_false: false,
            effort: None,
            tool_protocol: ToolProtocol::Xml,
            max_tool_prompt_size: None,
            reasoning_field: false,
            context_budget: None,
            notes: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToolProtocol {
    #[default]
    Xml,
    SimpleJson,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EffortLevel {
    Low,
    #[default]
    Medium,
    High,
    Max,
}

impl EffortLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Max => "max",
        }
    }

    pub fn openai_reasoning_effort(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High | Self::Max => "high",
        }
    }
}

impl std::fmt::Display for EffortLevel {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for EffortLevel {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "low" => Ok(Self::Low),
            "medium" | "default" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "max" | "maximum" => Ok(Self::Max),
            other => anyhow::bail!("unknown effort level `{other}`; use low, medium, high, or max"),
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpConfig {
    #[serde(default)]
    pub servers: BTreeMap<String, McpServerConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HooksConfig {
    #[serde(default)]
    pub pre_tool: Vec<HookCommand>,
    #[serde(default)]
    pub post_tool: Vec<HookCommand>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookCommand {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
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

    pub fn resolve_effort(&self, provider_name: &str, model_name: &str) -> EffortLevel {
        if let Some(effort) = self
            .model_profiles
            .get(model_name)
            .and_then(|profile| profile.effort)
        {
            return effort;
        }

        if let Some(provider) = self.providers.get(provider_name) {
            if let Some(effort) = provider.effort {
                return effort;
            }
            if let Some(effort) = provider
                .reasoning_effort
                .as_deref()
                .and_then(|value| EffortLevel::from_str(value).ok())
            {
                return effort;
            }
        }

        self.effort
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
                effort: Some(EffortLevel::High),
                reasoning_effort: None,
                max_output_tokens: Some(8192),
            },
        );
        providers.insert(
            "anthropic".to_string(),
            ProviderConfig {
                kind: ProviderKind::Anthropic,
                api_key_env: Some("ANTHROPIC_API_KEY".to_string()),
                base_url: Some("https://api.anthropic.com/v1".to_string()),
                effort: None,
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
                effort: None,
                reasoning_effort: None,
                max_output_tokens: Some(4096),
            },
        );
        providers.insert(
            "ollama".to_string(),
            ProviderConfig {
                kind: ProviderKind::Ollama,
                api_key_env: None,
                base_url: Some("http://localhost:11434".to_string()),
                effort: None,
                reasoning_effort: None,
                max_output_tokens: Some(1024),
            },
        );

        let mut model_profiles = BTreeMap::new();
        model_profiles.insert(
            "qwen3.5:0.8b".to_string(),
            ModelProfile {
                provider: Some("ollama".to_string()),
                supports_system: false,
                prefer_think_false: true,
                effort: Some(EffortLevel::Low),
                tool_protocol: ToolProtocol::SimpleJson,
                max_tool_prompt_size: Some(1200),
                reasoning_field: true,
                context_budget: Some(4096),
                notes: Some("Small local model profile optimized for short prompts.".to_string()),
            },
        );

        Self {
            default_provider: "openai".to_string(),
            default_model: "gpt-5.5".to_string(),
            permission_profile: "ask".to_string(),
            effort: EffortLevel::Medium,
            max_agent_turns: 8,
            providers,
            model_profiles,
            mcp: McpConfig::default(),
            hooks: HooksConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_effort_by_profile_provider_global_order() {
        let mut config = AppConfig::default();
        config.effort = EffortLevel::Low;
        config.providers.get_mut("openai").unwrap().effort = Some(EffortLevel::High);
        config.model_profiles.insert(
            "custom".to_string(),
            ModelProfile {
                provider: Some("openai".to_string()),
                effort: Some(EffortLevel::Max),
                ..ModelProfile::default()
            },
        );

        assert_eq!(config.resolve_effort("openai", "custom"), EffortLevel::Max);
        assert_eq!(
            config.resolve_effort("openai", "unknown"),
            EffortLevel::High
        );
        assert_eq!(
            config.resolve_effort("missing", "unknown"),
            EffortLevel::Low
        );
    }

    #[test]
    fn parses_legacy_reasoning_effort() {
        let mut config = AppConfig::default();
        let provider = config.providers.get_mut("local").unwrap();
        provider.effort = None;
        provider.reasoning_effort = Some("high".to_string());

        assert_eq!(config.resolve_effort("local", "unknown"), EffortLevel::High);
    }
}

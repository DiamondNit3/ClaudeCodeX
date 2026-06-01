use crate::config::{AppConfig, ProviderConfig, ProviderKind};
use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelMessage {
    pub role: MessageRole,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone)]
pub struct ModelRequest {
    pub model: String,
    pub messages: Vec<ModelMessage>,
}

#[derive(Debug, Clone)]
pub struct ModelResponse {
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct ProviderSummary {
    pub name: String,
    pub kind: String,
    pub base_url: String,
    pub api_key_env: Option<String>,
}

impl std::fmt::Display for ProviderSummary {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let key = self
            .api_key_env
            .as_ref()
            .map(|value| format!(" key=${value}"))
            .unwrap_or_else(|| " key=<none>".to_string());
        write!(
            formatter,
            "{}: {} {}{}",
            self.name, self.kind, self.base_url, key
        )
    }
}

#[async_trait]
pub trait ModelProvider: Send + Sync {
    fn summary(&self) -> ProviderSummary;
    async fn generate(&self, request: ModelRequest) -> Result<ModelResponse>;
}

pub struct ProviderRegistry {
    providers: BTreeMap<String, Arc<dyn ModelProvider>>,
}

impl ProviderRegistry {
    pub fn from_config(config: &AppConfig) -> Result<Self> {
        let mut providers: BTreeMap<String, Arc<dyn ModelProvider>> = BTreeMap::new();
        for (name, provider_config) in &config.providers {
            let provider: Arc<dyn ModelProvider> = match provider_config.kind {
                ProviderKind::Openai => Arc::new(OpenAiProvider::new(name, provider_config)?),
                ProviderKind::Anthropic => Arc::new(AnthropicProvider::new(name, provider_config)?),
                ProviderKind::LocalOpenaiCompatible => {
                    Arc::new(LocalOpenAiProvider::new(name, provider_config)?)
                }
            };
            providers.insert(name.clone(), provider);
        }
        Ok(Self { providers })
    }

    pub fn get(&self, name: &str) -> Result<Arc<dyn ModelProvider>> {
        self.providers
            .get(name)
            .cloned()
            .with_context(|| format!("provider `{name}` is not configured"))
    }

    pub fn summaries(&self) -> Vec<ProviderSummary> {
        self.providers
            .values()
            .map(|provider| provider.summary())
            .collect()
    }
}

struct OpenAiProvider {
    name: String,
    client: Client,
    base_url: String,
    api_key_env: String,
    reasoning_effort: Option<String>,
    max_output_tokens: Option<u32>,
}

impl OpenAiProvider {
    fn new(name: &str, config: &ProviderConfig) -> Result<Self> {
        Ok(Self {
            name: name.to_string(),
            client: Client::new(),
            base_url: config
                .base_url
                .clone()
                .unwrap_or_else(|| "https://api.openai.com/v1".to_string()),
            api_key_env: config
                .api_key_env
                .clone()
                .unwrap_or_else(|| "OPENAI_API_KEY".to_string()),
            reasoning_effort: config.reasoning_effort.clone(),
            max_output_tokens: config.max_output_tokens,
        })
    }
}

#[async_trait]
impl ModelProvider for OpenAiProvider {
    fn summary(&self) -> ProviderSummary {
        ProviderSummary {
            name: self.name.clone(),
            kind: "openai-responses".to_string(),
            base_url: self.base_url.clone(),
            api_key_env: Some(self.api_key_env.clone()),
        }
    }

    async fn generate(&self, request: ModelRequest) -> Result<ModelResponse> {
        let api_key = std::env::var(&self.api_key_env)
            .with_context(|| format!("missing ${}", self.api_key_env))?;
        let instructions = request
            .messages
            .iter()
            .filter(|message| matches!(message.role, MessageRole::System))
            .map(|message| message.content.clone())
            .collect::<Vec<_>>()
            .join("\n\n");
        let input = request
            .messages
            .iter()
            .filter(|message| !matches!(message.role, MessageRole::System))
            .map(|message| format!("{}: {}", role_name(&message.role), &message.content))
            .collect::<Vec<_>>();

        let mut body = json!({
            "model": request.model,
            "instructions": instructions,
            "input": input.join("\n\n")
        });
        if let Some(effort) = &self.reasoning_effort {
            body["reasoning"] = json!({ "effort": effort });
        }
        if let Some(max_output_tokens) = self.max_output_tokens {
            body["max_output_tokens"] = json!(max_output_tokens);
        }

        let response = self
            .client
            .post(format!("{}/responses", self.base_url.trim_end_matches('/')))
            .bearer_auth(api_key)
            .json(&body)
            .send()
            .await?
            .error_for_status()?;

        let body: Value = response.json().await?;
        Ok(ModelResponse {
            text: extract_openai_response_text(&body)?,
        })
    }
}

struct AnthropicProvider {
    name: String,
    client: Client,
    base_url: String,
    api_key_env: String,
    max_output_tokens: u32,
}

impl AnthropicProvider {
    fn new(name: &str, config: &ProviderConfig) -> Result<Self> {
        Ok(Self {
            name: name.to_string(),
            client: Client::new(),
            base_url: config
                .base_url
                .clone()
                .unwrap_or_else(|| "https://api.anthropic.com/v1".to_string()),
            api_key_env: config
                .api_key_env
                .clone()
                .unwrap_or_else(|| "ANTHROPIC_API_KEY".to_string()),
            max_output_tokens: config.max_output_tokens.unwrap_or(8192),
        })
    }
}

#[async_trait]
impl ModelProvider for AnthropicProvider {
    fn summary(&self) -> ProviderSummary {
        ProviderSummary {
            name: self.name.clone(),
            kind: "anthropic-messages".to_string(),
            base_url: self.base_url.clone(),
            api_key_env: Some(self.api_key_env.clone()),
        }
    }

    async fn generate(&self, request: ModelRequest) -> Result<ModelResponse> {
        let api_key = std::env::var(&self.api_key_env)
            .with_context(|| format!("missing ${}", self.api_key_env))?;
        let system = request
            .messages
            .iter()
            .filter(|message| matches!(message.role, MessageRole::System))
            .map(|message| message.content.clone())
            .collect::<Vec<_>>()
            .join("\n\n");
        let messages = request
            .messages
            .iter()
            .filter(|message| !matches!(message.role, MessageRole::System))
            .map(|message| {
                json!({
                    "role": match &message.role {
                        MessageRole::Assistant => "assistant",
                        _ => "user",
                    },
                    "content": &message.content
                })
            })
            .collect::<Vec<_>>();

        let response = self
            .client
            .post(format!("{}/messages", self.base_url.trim_end_matches('/')))
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&json!({
                "model": request.model,
                "max_tokens": self.max_output_tokens,
                "system": system,
                "messages": messages
            }))
            .send()
            .await?
            .error_for_status()?;

        let body: Value = response.json().await?;
        Ok(ModelResponse {
            text: extract_anthropic_text(&body)?,
        })
    }
}

struct LocalOpenAiProvider {
    name: String,
    client: Client,
    base_url: String,
    api_key_env: Option<String>,
    max_output_tokens: Option<u32>,
}

impl LocalOpenAiProvider {
    fn new(name: &str, config: &ProviderConfig) -> Result<Self> {
        Ok(Self {
            name: name.to_string(),
            client: Client::new(),
            base_url: config
                .base_url
                .clone()
                .unwrap_or_else(|| "http://localhost:11434/v1".to_string()),
            api_key_env: config.api_key_env.clone(),
            max_output_tokens: config.max_output_tokens,
        })
    }
}

#[async_trait]
impl ModelProvider for LocalOpenAiProvider {
    fn summary(&self) -> ProviderSummary {
        ProviderSummary {
            name: self.name.clone(),
            kind: "local-openai-compatible".to_string(),
            base_url: self.base_url.clone(),
            api_key_env: self.api_key_env.clone(),
        }
    }

    async fn generate(&self, request: ModelRequest) -> Result<ModelResponse> {
        let system_context = request
            .messages
            .iter()
            .filter(|message| matches!(message.role, MessageRole::System))
            .map(|message| message.content.clone())
            .collect::<Vec<_>>()
            .join("\n\n");

        let mut messages = Vec::new();
        if !system_context.is_empty() {
            messages.push(json!({
                "role": "user",
                "content": format!("Harness and project instructions:\n{system_context}")
            }));
        }

        messages.extend(
            request
                .messages
                .iter()
                .filter(|message| !matches!(message.role, MessageRole::System))
                .map(|message| {
                    json!({
                        "role": role_name(&message.role),
                        "content": &message.content
                    })
                }),
        );

        let mut body = json!({
            "model": request.model,
            "messages": messages,
            "stream": false
        });
        if let Some(max_tokens) = self.max_output_tokens {
            body["max_tokens"] = json!(max_tokens);
        }

        let mut builder = self
            .client
            .post(format!(
                "{}/chat/completions",
                self.base_url.trim_end_matches('/')
            ))
            .json(&body);

        if let Some(env_name) = &self.api_key_env {
            if let Ok(key) = std::env::var(env_name) {
                builder = builder.bearer_auth(key);
            }
        }

        let response = builder.send().await?.error_for_status()?;
        let body: Value = response.json().await?;
        Ok(ModelResponse {
            text: extract_local_openai_text(&body),
        })
    }
}

fn role_name(role: &MessageRole) -> &'static str {
    match role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "user",
    }
}

fn extract_openai_response_text(body: &Value) -> Result<String> {
    if let Some(text) = body.get("output_text").and_then(Value::as_str) {
        return Ok(text.to_string());
    }

    let mut chunks = Vec::new();
    if let Some(output) = body.get("output").and_then(Value::as_array) {
        for item in output {
            if let Some(content) = item.get("content").and_then(Value::as_array) {
                for block in content {
                    if let Some(text) = block.get("text").and_then(Value::as_str) {
                        chunks.push(text.to_string());
                    }
                }
            }
        }
    }

    if chunks.is_empty() {
        bail!("OpenAI response did not contain text output");
    }
    Ok(chunks.join(""))
}

fn extract_anthropic_text(body: &Value) -> Result<String> {
    let mut chunks = Vec::new();
    if let Some(content) = body.get("content").and_then(Value::as_array) {
        for block in content {
            if block.get("type").and_then(Value::as_str) == Some("text") {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    chunks.push(text.to_string());
                }
            }
        }
    }

    if chunks.is_empty() {
        bail!("Anthropic response did not contain text output");
    }
    Ok(chunks.join(""))
}

fn extract_local_openai_text(body: &Value) -> String {
    let message = &body["choices"][0]["message"];
    let content = message["content"].as_str().unwrap_or_default();
    if !content.trim().is_empty() {
        return content.to_string();
    }
    message["reasoning"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

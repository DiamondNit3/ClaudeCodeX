use crate::config::{AppConfig, EffortLevel, ModelProfile, ProviderConfig, ProviderKind};
use crate::tools::ToolSpec;
use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use futures_util::StreamExt;
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
    pub profile: ModelProfile,
    pub effort: EffortLevel,
    pub tools: Vec<ToolSpec>,
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
    pub capabilities: ProviderCapabilities,
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
            "{}: {} {}{} caps={}",
            self.name, self.kind, self.base_url, key, self.capabilities
        )
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ProviderCapabilities {
    pub streaming: bool,
    pub native_tools: bool,
}

impl std::fmt::Display for ProviderCapabilities {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut caps = Vec::new();
        if self.streaming {
            caps.push("streaming");
        }
        if self.native_tools {
            caps.push("native-tools");
        }
        if caps.is_empty() {
            caps.push("text-tools");
        }
        formatter.write_str(&caps.join(","))
    }
}

#[async_trait]
pub trait ModelProvider: Send + Sync {
    fn summary(&self) -> ProviderSummary;
    fn capabilities(&self) -> ProviderCapabilities;
    async fn generate(&self, request: ModelRequest) -> Result<ModelResponse>;
    async fn generate_stream(
        &self,
        request: ModelRequest,
        on_chunk: &mut (dyn FnMut(String) + Send),
    ) -> Result<ModelResponse> {
        let response = self.generate(request).await?;
        on_chunk(response.text.clone());
        Ok(response)
    }
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
                ProviderKind::Ollama => Arc::new(OllamaProvider::new(name, provider_config)?),
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
            capabilities: self.capabilities(),
        }
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            streaming: false,
            native_tools: true,
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
        body["reasoning"] = json!({ "effort": request.effort.openai_reasoning_effort() });
        let max_output_tokens = self
            .max_output_tokens
            .unwrap_or_else(|| default_openai_output_tokens(request.effort));
        if max_output_tokens > 0 {
            body["max_output_tokens"] = json!(max_output_tokens);
        }
        if !request.tools.is_empty() {
            body["tools"] = json!(openai_tool_specs(&request.tools));
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
    max_output_tokens: Option<u32>,
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
            max_output_tokens: config.max_output_tokens,
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
            capabilities: self.capabilities(),
        }
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            streaming: false,
            native_tools: true,
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

        let mut body = json!({
            "model": request.model,
            "max_tokens": self
                .max_output_tokens
                .unwrap_or_else(|| default_anthropic_output_tokens(request.effort)),
            "system": system,
            "messages": messages
        });
        if !request.tools.is_empty() {
            body["tools"] = json!(anthropic_tool_specs(&request.tools));
        }

        let response = self
            .client
            .post(format!("{}/messages", self.base_url.trim_end_matches('/')))
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
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
            capabilities: self.capabilities(),
        }
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            streaming: false,
            native_tools: false,
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
                "role": if request.profile.supports_system { "system" } else { "user" },
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
        } else {
            body["max_tokens"] = json!(default_local_output_tokens(request.effort));
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

    async fn generate_stream(
        &self,
        request: ModelRequest,
        on_chunk: &mut (dyn FnMut(String) + Send),
    ) -> Result<ModelResponse> {
        let response = self.generate(request).await?;
        on_chunk(response.text.clone());
        Ok(response)
    }
}

struct OllamaProvider {
    name: String,
    client: Client,
    base_url: String,
    max_output_tokens: Option<u32>,
}

impl OllamaProvider {
    fn new(name: &str, config: &ProviderConfig) -> Result<Self> {
        Ok(Self {
            name: name.to_string(),
            client: Client::new(),
            base_url: config
                .base_url
                .clone()
                .unwrap_or_else(|| "http://localhost:11434".to_string()),
            max_output_tokens: config.max_output_tokens,
        })
    }
}

#[async_trait]
impl ModelProvider for OllamaProvider {
    fn summary(&self) -> ProviderSummary {
        ProviderSummary {
            name: self.name.clone(),
            kind: "ollama".to_string(),
            base_url: self.base_url.clone(),
            api_key_env: None,
            capabilities: self.capabilities(),
        }
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            streaming: true,
            native_tools: false,
        }
    }

    async fn generate(&self, request: ModelRequest) -> Result<ModelResponse> {
        let body = ollama_body(&request, false, self.max_output_tokens);
        let response = self
            .client
            .post(format!("{}/api/chat", self.base_url.trim_end_matches('/')))
            .json(&body)
            .send()
            .await?
            .error_for_status()?;
        let body: Value = response.json().await?;
        Ok(ModelResponse {
            text: extract_ollama_text(&body, request.profile.reasoning_field),
        })
    }

    async fn generate_stream(
        &self,
        request: ModelRequest,
        on_chunk: &mut (dyn FnMut(String) + Send),
    ) -> Result<ModelResponse> {
        let include_reasoning = request.profile.reasoning_field;
        let body = ollama_body(&request, true, self.max_output_tokens);
        let response = self
            .client
            .post(format!("{}/api/chat", self.base_url.trim_end_matches('/')))
            .json(&body)
            .send()
            .await?
            .error_for_status()?;
        let mut stream = response.bytes_stream();
        let mut buffered = String::new();
        let mut text = String::new();

        while let Some(chunk) = stream.next().await {
            buffered.push_str(&String::from_utf8_lossy(&chunk?));
            while let Some(index) = buffered.find('\n') {
                let line = buffered[..index].trim().to_string();
                buffered = buffered[index + 1..].to_string();
                if let Some(delta) = extract_ollama_stream_delta(&line, include_reasoning) {
                    on_chunk(delta.clone());
                    text.push_str(&delta);
                }
            }
        }
        if !buffered.trim().is_empty() {
            if let Some(delta) = extract_ollama_stream_delta(buffered.trim(), include_reasoning) {
                on_chunk(delta.clone());
                text.push_str(&delta);
            }
        }

        Ok(ModelResponse { text })
    }
}

fn ollama_body(request: &ModelRequest, stream: bool, max_output_tokens: Option<u32>) -> Value {
    let mut messages = Vec::new();
    let system_context = request
        .messages
        .iter()
        .filter(|message| matches!(message.role, MessageRole::System))
        .map(|message| message.content.clone())
        .collect::<Vec<_>>()
        .join("\n\n");

    if !system_context.is_empty() {
        messages.push(json!({
            "role": if request.profile.supports_system { "system" } else { "user" },
            "content": system_context
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
        "stream": stream
    });
    body["think"] = json!(ollama_think(
        request.effort,
        request.profile.prefer_think_false
    ));
    let num_predict =
        max_output_tokens.unwrap_or_else(|| default_ollama_output_tokens(request.effort));
    body["options"] = json!({
        "num_predict": num_predict,
        "num_ctx": default_ollama_context_tokens(request.effort)
    });
    body
}

fn role_name(role: &MessageRole) -> &'static str {
    match role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "user",
    }
}

fn openai_tool_specs(tools: &[ToolSpec]) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            json!({
                "type": "function",
                "name": tool.name,
                "description": tool.description,
                "parameters": generic_tool_parameters(&tool.name)
            })
        })
        .collect()
}

fn anthropic_tool_specs(tools: &[ToolSpec]) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            json!({
                "name": tool.name,
                "description": tool.description,
                "input_schema": generic_tool_parameters(&tool.name)
            })
        })
        .collect()
}

fn generic_tool_parameters(tool: &str) -> Value {
    let required = match tool {
        "read_file" => vec!["path"],
        "write_file" => vec!["path", "content"],
        "edit_file" => vec!["path"],
        "apply_patch" => vec!["path", "patch"],
        "glob" => vec!["pattern"],
        "grep" => vec!["query"],
        "shell" => vec!["command"],
        _ => Vec::new(),
    };
    json!({
        "type": "object",
        "properties": {
            "path": {"type": "string"},
            "content": {"type": "string"},
            "old": {"type": "string"},
            "new": {"type": "string"},
            "patch": {"type": "string"},
            "pattern": {"type": "string"},
            "query": {"type": "string"},
            "command": {"type": "string"}
        },
        "required": required
    })
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
        if let Some(tool_calls) = extract_openai_tool_calls(body) {
            return Ok(tool_calls);
        }
        bail!("OpenAI response did not contain text output");
    }
    let mut text = chunks.join("");
    if let Some(tool_calls) = extract_openai_tool_calls(body) {
        if !text.trim().is_empty() {
            text.push('\n');
        }
        text.push_str(&tool_calls);
    }
    Ok(text)
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
        if let Some(tool_calls) = extract_anthropic_tool_calls(body) {
            return Ok(tool_calls);
        }
        bail!("Anthropic response did not contain text output");
    }
    let mut text = chunks.join("");
    if let Some(tool_calls) = extract_anthropic_tool_calls(body) {
        if !text.trim().is_empty() {
            text.push('\n');
        }
        text.push_str(&tool_calls);
    }
    Ok(text)
}

fn extract_openai_tool_calls(body: &Value) -> Option<String> {
    let output = body.get("output")?.as_array()?;
    let mut calls = Vec::new();
    for item in output {
        if item.get("type").and_then(Value::as_str) == Some("function_call") {
            let name = item.get("name").and_then(Value::as_str)?;
            let raw_args = item
                .get("arguments")
                .and_then(Value::as_str)
                .unwrap_or("{}");
            let arguments = serde_json::from_str::<Value>(raw_args).unwrap_or_else(|_| json!({}));
            calls.push(format!(
                "<tool_call>{}</tool_call>",
                json!({"tool": name, "arguments": arguments})
            ));
        }
    }
    if calls.is_empty() {
        None
    } else {
        Some(calls.join("\n"))
    }
}

fn extract_anthropic_tool_calls(body: &Value) -> Option<String> {
    let content = body.get("content")?.as_array()?;
    let mut calls = Vec::new();
    for block in content {
        if block.get("type").and_then(Value::as_str) == Some("tool_use") {
            let name = block.get("name").and_then(Value::as_str)?;
            let arguments = block.get("input").cloned().unwrap_or_else(|| json!({}));
            calls.push(format!(
                "<tool_call>{}</tool_call>",
                json!({"tool": name, "arguments": arguments})
            ));
        }
    }
    if calls.is_empty() {
        None
    } else {
        Some(calls.join("\n"))
    }
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

fn extract_ollama_text(body: &Value, include_reasoning: bool) -> String {
    let message = &body["message"];
    let content = message["content"].as_str().unwrap_or_default();
    if !content.trim().is_empty() {
        return content.to_string();
    }
    if include_reasoning {
        return message["reasoning"]
            .as_str()
            .unwrap_or_default()
            .to_string();
    }
    String::new()
}

fn extract_ollama_stream_delta(line: &str, include_reasoning: bool) -> Option<String> {
    let value = serde_json::from_str::<Value>(line).ok()?;
    let message = value.get("message")?;
    if let Some(content) = message.get("content").and_then(Value::as_str) {
        if !content.is_empty() {
            return Some(content.to_string());
        }
    }
    if include_reasoning {
        if let Some(reasoning) = message.get("reasoning").and_then(Value::as_str) {
            if !reasoning.is_empty() {
                return Some(reasoning.to_string());
            }
        }
    }
    None
}

fn default_openai_output_tokens(effort: EffortLevel) -> u32 {
    match effort {
        EffortLevel::Low => 2048,
        EffortLevel::Medium => 4096,
        EffortLevel::High => 8192,
        EffortLevel::Max => 16000,
    }
}

fn default_anthropic_output_tokens(effort: EffortLevel) -> u32 {
    match effort {
        EffortLevel::Low => 2048,
        EffortLevel::Medium => 4096,
        EffortLevel::High => 8192,
        EffortLevel::Max => 12000,
    }
}

fn default_local_output_tokens(effort: EffortLevel) -> u32 {
    match effort {
        EffortLevel::Low => 512,
        EffortLevel::Medium => 1024,
        EffortLevel::High => 2048,
        EffortLevel::Max => 4096,
    }
}

fn default_ollama_output_tokens(effort: EffortLevel) -> u32 {
    match effort {
        EffortLevel::Low => 512,
        EffortLevel::Medium => 1024,
        EffortLevel::High => 2048,
        EffortLevel::Max => 4096,
    }
}

fn default_ollama_context_tokens(effort: EffortLevel) -> u32 {
    match effort {
        EffortLevel::Low => 4096,
        EffortLevel::Medium => 8192,
        EffortLevel::High => 16384,
        EffortLevel::Max => 32768,
    }
}

fn ollama_think(effort: EffortLevel, prefer_think_false: bool) -> bool {
    if prefer_think_false {
        return false;
    }
    matches!(effort, EffortLevel::High | EffortLevel::Max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_max_effort_to_provider_defaults() {
        assert_eq!(EffortLevel::Max.openai_reasoning_effort(), "high");
        assert_eq!(default_openai_output_tokens(EffortLevel::Max), 16000);
        assert_eq!(default_anthropic_output_tokens(EffortLevel::Max), 12000);
        assert_eq!(default_local_output_tokens(EffortLevel::Max), 4096);
        assert_eq!(default_ollama_context_tokens(EffortLevel::Max), 32768);
    }

    #[test]
    fn ollama_thinking_tracks_effort_and_profile_preference() {
        assert!(!ollama_think(EffortLevel::Medium, false));
        assert!(ollama_think(EffortLevel::High, false));
        assert!(!ollama_think(EffortLevel::Max, true));
    }

    #[test]
    fn parses_ollama_stream_delta() {
        let line = r#"{"message":{"content":"hello"}}"#;
        assert_eq!(
            extract_ollama_stream_delta(line, false).as_deref(),
            Some("hello")
        );
    }
}

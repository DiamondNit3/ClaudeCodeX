use crate::config::{AppConfig, EffortLevel, ModelProfile, ToolProtocol};
use crate::context::ProjectContext;
use crate::parser::parse_model_output;
use crate::permissions::{PermissionEngine, PermissionProfile};
use crate::preview::PreviewServer;
use crate::providers::{MessageRole, ModelMessage, ModelRequest, ProviderRegistry};
use crate::session::{self, Session};
use crate::tools::{ToolCall, ToolRegistry, ToolResult};
use crate::ui::{self, FooterInfo, HeaderInfo};
use anyhow::{bail, Result};
use serde::Serialize;
use serde_json::json;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::Command;

pub async fn run_interactive(
    config: AppConfig,
    providers: ProviderRegistry,
    workspace: PathBuf,
) -> Result<()> {
    let context = ProjectContext::load(&workspace)?;
    let session = Session::create()?;
    let permissions = PermissionEngine::new(
        PermissionProfile::parse(&config.permission_profile),
        workspace.clone(),
        true,
    );
    let tools = ToolRegistry::new(workspace.clone(), permissions);
    let mut state = AgentState::new(config, context, session);
    state.append_metadata()?;

    render_header(&state);

    loop {
        print!("{}", ui::prompt());
        io::stdout().flush()?;
        let mut input = String::new();
        if io::stdin().read_line(&mut input)? == 0 {
            break;
        }
        let input = input.trim();
        if input.is_empty() {
            continue;
        }
        if input.starts_with('/') {
            if handle_slash_command(input, &mut state, &providers)? {
                break;
            }
            render_footer(&state);
            continue;
        }

        let output = run_agent_turn(&mut state, &providers, &tools, input, true).await?;
        if !output.trim().is_empty() {
            println!("{output}");
        }
        render_footer(&state);
    }

    Ok(())
}

pub async fn run_resume(
    config: AppConfig,
    providers: ProviderRegistry,
    workspace: PathBuf,
    session_id: String,
) -> Result<()> {
    let context = ProjectContext::load(&workspace)?;
    let session = Session::open(&session_id)?;
    let permissions = PermissionEngine::new(
        PermissionProfile::parse(&config.permission_profile),
        workspace.clone(),
        true,
    );
    let tools = ToolRegistry::new(workspace, permissions);
    let mut state = AgentState::new(config, context, session);
    state.restore_transcript()?;

    render_header(&state);
    loop {
        print!("{}", ui::prompt());
        io::stdout().flush()?;
        let mut input = String::new();
        if io::stdin().read_line(&mut input)? == 0 {
            break;
        }
        let input = input.trim();
        if input.is_empty() {
            continue;
        }
        if input.starts_with('/') {
            if handle_slash_command(input, &mut state, &providers)? {
                break;
            }
            render_footer(&state);
            continue;
        }
        let output = run_agent_turn(&mut state, &providers, &tools, input, true).await?;
        if !output.trim().is_empty() {
            println!("{output}");
        }
        render_footer(&state);
    }
    Ok(())
}

pub async fn run_exec(
    config: AppConfig,
    providers: ProviderRegistry,
    workspace: PathBuf,
    task: String,
) -> Result<()> {
    let context = ProjectContext::load(&workspace)?;
    let session = Session::create()?;
    let permissions = PermissionEngine::new(
        PermissionProfile::parse(&config.permission_profile),
        workspace.clone(),
        false,
    );
    let tools = ToolRegistry::new(workspace, permissions);
    let mut state = AgentState::new(config, context, session);
    state.append_metadata()?;
    let output = run_agent_turn(&mut state, &providers, &tools, &task, false).await?;
    println!("{output}");
    eprintln!("session: {}", state.session.path().display());
    Ok(())
}

struct AgentState {
    config: AppConfig,
    context: ProjectContext,
    session: Session,
    selected_provider: String,
    selected_model: String,
    effort_override: Option<EffortLevel>,
    transcript: Vec<ModelMessage>,
    preview: Option<PreviewServer>,
}

impl AgentState {
    fn new(config: AppConfig, context: ProjectContext, session: Session) -> Self {
        let selected_provider = config
            .model_profiles
            .get(&config.default_model)
            .and_then(|profile| profile.provider.clone())
            .unwrap_or_else(|| config.default_provider.clone());
        Self {
            selected_provider,
            selected_model: config.default_model.clone(),
            effort_override: None,
            config,
            context,
            session,
            transcript: Vec::new(),
            preview: None,
        }
    }

    fn append_metadata(&self) -> Result<()> {
        self.session.append(
            "metadata",
            json!({
                "provider": &self.selected_provider,
                "model": &self.selected_model,
                "effort": self.current_effort().as_str(),
                "permission_profile": &self.config.permission_profile,
                "workspace": &self.context.workspace,
            }),
        )
    }

    fn restore_transcript(&mut self) -> Result<()> {
        for event in session::read_events(&self.session)? {
            match event.kind.as_str() {
                "metadata" => {
                    if let Some(provider) = event.payload.get("provider").and_then(|v| v.as_str()) {
                        self.selected_provider = provider.to_string();
                    }
                    if let Some(model) = event.payload.get("model").and_then(|v| v.as_str()) {
                        self.selected_model = model.to_string();
                    }
                    if let Some(effort) = event.payload.get("effort").and_then(|v| v.as_str()) {
                        self.effort_override = effort.parse().ok();
                    }
                }
                "effort" => {
                    if let Some(effort) = event.payload.get("text").and_then(|v| v.as_str()) {
                        self.effort_override = effort.parse().ok();
                    }
                }
                "user" => {
                    if let Some(text) = event.payload.get("text").and_then(|v| v.as_str()) {
                        self.transcript.push(ModelMessage {
                            role: MessageRole::User,
                            content: text.to_string(),
                        });
                    }
                }
                "assistant" => {
                    if let Some(text) = event.payload.get("text").and_then(|v| v.as_str()) {
                        self.transcript.push(ModelMessage {
                            role: MessageRole::Assistant,
                            content: text.to_string(),
                        });
                    }
                }
                "tool_result" => {
                    self.transcript.push(ModelMessage {
                        role: MessageRole::Tool,
                        content: event.payload.to_string(),
                    });
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn system_messages(&self) -> Vec<ModelMessage> {
        let profile = self.model_profile();
        if profile.tool_protocol == ToolProtocol::SimpleJson {
            return vec![ModelMessage {
                role: MessageRole::System,
                content: local_model_prompt(
                    &self.context.workspace.display().to_string(),
                    &self.context.render_for_prompt(),
                    profile.max_tool_prompt_size,
                ),
            }];
        }

        vec![
            ModelMessage {
                role: MessageRole::System,
                content: base_prompt().to_string(),
            },
            ModelMessage {
                role: MessageRole::System,
                content: format!(
                    "Workspace: {}\n\nProject instructions:\n{}",
                    self.context.workspace.display(),
                    self.context.render_for_prompt()
                ),
            },
            ModelMessage {
                role: MessageRole::System,
                content: ToolRegistry::tool_manifest().to_string(),
            },
        ]
    }

    fn model_profile(&self) -> ModelProfile {
        self.config
            .model_profiles
            .get(&self.selected_model)
            .cloned()
            .unwrap_or_default()
    }

    fn current_effort(&self) -> EffortLevel {
        self.effort_override.unwrap_or_else(|| {
            self.config
                .resolve_effort(&self.selected_provider, &self.selected_model)
        })
    }
}

async fn run_agent_turn(
    state: &mut AgentState,
    providers: &ProviderRegistry,
    tools: &ToolRegistry,
    user_input: &str,
    render_ui: bool,
) -> Result<String> {
    state
        .session
        .append("user", EventText { text: user_input })?;
    state.transcript.push(ModelMessage {
        role: MessageRole::User,
        content: user_input.to_string(),
    });

    let mut final_visible = String::new();
    for _ in 0..state.config.max_agent_turns {
        let provider = providers.get(&state.selected_provider)?;
        let mut messages = state.system_messages();
        messages.extend(state.transcript.clone());
        let animation = render_ui.then(|| ui::ActivityAnimation::start("thinking"));
        let response_result = provider
            .generate(ModelRequest {
                model: state.selected_model.clone(),
                messages,
                profile: state.model_profile(),
                effort: state.current_effort(),
            })
            .await;
        if let Some(animation) = animation {
            animation.stop();
        }
        let response = response_result?;

        state.session.append(
            "assistant",
            EventText {
                text: &response.text,
            },
        )?;
        state.transcript.push(ModelMessage {
            role: MessageRole::Assistant,
            content: response.text.clone(),
        });

        let parsed = parse_model_output(&response.text)?;
        if !parsed.visible_text.trim().is_empty() {
            if !final_visible.is_empty() {
                final_visible.push_str("\n\n");
            }
            final_visible.push_str(parsed.visible_text.trim());
        }

        let mut calls = parsed.calls;
        if calls.is_empty() {
            if let Some(call) = infer_file_write(user_input, &response.text) {
                state.session.append("inferred_tool_call", &call)?;
                calls.push(call);
            }
        }
        if calls.is_empty() {
            return Ok(final_visible);
        }

        let mut tool_results = Vec::new();
        for mut call in calls {
            normalize_local_tool_path(user_input, &mut call);
            if render_ui {
                ui::working_for_tool(&call);
                ui::render_tool_call(&call);
            }
            state.session.append("tool_call", &call)?;
            let result = tools.execute(call).await;
            if render_ui {
                ui::render_tool_result(&result);
            }
            state.session.append("tool_result", &result)?;
            tool_results.push(result);
        }

        state.transcript.push(ModelMessage {
            role: MessageRole::Tool,
            content: render_tool_results(&tool_results)?,
        });

        if state.model_profile().tool_protocol == ToolProtocol::SimpleJson
            && tool_results
                .iter()
                .any(|result| result.success && result.tool == "write_file")
        {
            let summary = tool_results
                .iter()
                .find(|result| result.success && result.tool == "write_file")
                .map(|result| result.content.clone())
                .unwrap_or_else(|| "write_file completed".to_string());
            return Ok(if final_visible.trim().is_empty() {
                summary
            } else {
                format!("{}\n\n{}", final_visible.trim(), summary)
            });
        }
    }

    bail!(
        "agent reached max turn limit ({}) before completing",
        state.config.max_agent_turns
    )
}

fn handle_slash_command(
    input: &str,
    state: &mut AgentState,
    providers: &ProviderRegistry,
) -> Result<bool> {
    let mut parts = input.split_whitespace();
    let command = parts.next().unwrap_or_default();
    match command {
        "/exit" | "/quit" => Ok(true),
        "/help" => {
            ui::render_grouped_help();
            Ok(false)
        }
        "/clear" => {
            ui::clear_screen()?;
            render_header(state);
            Ok(false)
        }
        "/context" => {
            println!("{}", state.context.summary());
            println!("{}", state.context.render_for_prompt());
            Ok(false)
        }
        "/providers" => {
            for summary in providers.summaries() {
                println!("{summary}");
            }
            Ok(false)
        }
        "/effort" => {
            let effort = parts.next();
            match effort {
                Some(value) => {
                    let parsed: EffortLevel = value.parse()?;
                    state.effort_override = Some(parsed);
                    state.session.append(
                        "effort",
                        EventText {
                            text: parsed.as_str(),
                        },
                    )?;
                    println!("effort: {parsed}");
                }
                None => println!("effort: {}", state.current_effort()),
            }
            Ok(false)
        }
        "/model" => {
            let provider = parts.next();
            let model = parts.next();
            match (provider, model) {
                (Some(provider), Some(model)) => {
                    providers.get(provider)?;
                    state.selected_provider = provider.to_string();
                    state.selected_model = model.to_string();
                    println!(
                        "model: {} {}",
                        state.selected_provider, state.selected_model
                    );
                }
                _ => println!(
                    "model: {} {}",
                    state.selected_provider, state.selected_model
                ),
            }
            Ok(false)
        }
        "/permissions" => {
            println!("permissions: {}", state.config.permission_profile);
            Ok(false)
        }
        "/session" => {
            println!("{}", state.session.path().display());
            Ok(false)
        }
        "/compact" => {
            state.session.append(
                "compact",
                EventText {
                    text: "manual compaction marker",
                },
            )?;
            println!("compaction marker appended");
            Ok(false)
        }
        "/mascot" => {
            ui::render_mascot_preview();
            Ok(false)
        }
        "/status" => {
            println!("{}", git_status(&state.context.workspace));
            Ok(false)
        }
        "/diff" => {
            ui::render_diff(&git_diff(&state.context.workspace));
            Ok(false)
        }
        "/preview" => {
            let path = parts.next().unwrap_or("index.html");
            if state.preview.is_none() {
                state.preview = Some(PreviewServer::start(state.context.workspace.clone())?);
            }
            let url = state.preview.as_ref().unwrap().url_for(path)?;
            println!("preview  {url}");
            Ok(false)
        }
        other => {
            println!("unknown command `{other}`");
            Ok(false)
        }
    }
}

fn render_header(state: &AgentState) {
    let session_short = short_session_id(state);
    ui::render_header(HeaderInfo {
        version: env!("CARGO_PKG_VERSION"),
        provider: &state.selected_provider,
        model: &state.selected_model,
        effort: state.current_effort().as_str(),
        permissions: &state.config.permission_profile,
        workspace: &state.context.workspace,
        context_files: state.context.instruction_files.len(),
        session_short: &session_short,
        mode: "interactive",
    });
}

fn render_footer(state: &AgentState) {
    let session_short = short_session_id(state);
    let branch = git_branch(&state.context.workspace);
    let repo_state = git_repo_state(&state.context.workspace);
    ui::render_footer(FooterInfo {
        provider: &state.selected_provider,
        model: &state.selected_model,
        effort: state.current_effort().as_str(),
        permissions: &state.config.permission_profile,
        branch: &branch,
        repo_state: &repo_state,
        session_short: &session_short,
    });
}

fn short_session_id(state: &AgentState) -> String {
    state.session.id.to_string().chars().take(8).collect()
}

fn git_branch(workspace: &PathBuf) -> String {
    let branch = run_git(workspace, &["rev-parse", "--abbrev-ref", "HEAD"]);
    if branch.starts_with("fatal:") || branch.trim().is_empty() {
        return "no-git".to_string();
    }
    branch.lines().next().unwrap_or("no-git").to_string()
}

fn git_repo_state(workspace: &PathBuf) -> String {
    let status = run_git(workspace, &["status", "--short"]);
    if status.starts_with("fatal:") {
        return "no-git".to_string();
    }
    if status.trim().is_empty() {
        "clean".to_string()
    } else {
        "dirty".to_string()
    }
}

fn git_status(workspace: &PathBuf) -> String {
    let status = run_git(workspace, &["status", "--short", "--branch"]);
    if status.trim().is_empty() {
        "clean".to_string()
    } else {
        status
    }
}

fn git_diff(workspace: &PathBuf) -> String {
    let mut output = run_git(workspace, &["diff", "--stat"]);
    let diff = run_git(workspace, &["diff", "--"]);
    if !output.trim().is_empty() && !diff.trim().is_empty() {
        output.push('\n');
    }
    output.push_str(&diff);
    output
}

fn run_git(workspace: &PathBuf, args: &[&str]) -> String {
    let Ok(output) = Command::new("git")
        .args(args)
        .current_dir(workspace)
        .output()
    else {
        return "git unavailable".to_string();
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stdout.trim().is_empty() {
        stderr.trim().to_string()
    } else {
        stdout.trim_end().to_string()
    }
}

fn render_tool_results(results: &[ToolResult]) -> Result<String> {
    Ok(format!(
        "Tool results:\n{}",
        serde_json::to_string_pretty(results)?
    ))
}

fn local_model_prompt(workspace: &str, instructions: &str, max_chars: Option<usize>) -> String {
    let mut prompt = format!(
        r#"You are ClaudeCodeX. Keep responses short.

Workspace: {workspace}

Use one JSON action when you need a tool:
{{"action":"write_file","path":"file","content":"text"}}
{{"action":"read_file","path":"file"}}
{{"action":"shell","command":"cmd"}}

Return final text only when the task is done.

Project instructions:
{instructions}"#
    );
    if let Some(max_chars) = max_chars {
        if prompt.len() > max_chars {
            prompt.truncate(max_chars);
            prompt.push_str("\n...[truncated]");
        }
    }
    prompt
}

fn infer_file_write(user_input: &str, response: &str) -> Option<ToolCall> {
    let path = infer_target_path(user_input)?;
    let content = clean_file_content(response);
    if !looks_like_file_content(&content, &path) {
        return None;
    }
    Some(ToolCall {
        tool: "write_file".to_string(),
        arguments: json!({
            "path": path,
            "content": content
        }),
    })
}

fn normalize_local_tool_path(user_input: &str, call: &mut ToolCall) {
    if call.tool != "write_file" {
        return;
    }
    let Some(current) = call.arguments.get("path").and_then(|value| value.as_str()) else {
        return;
    };
    if !matches!(current, "file" | "output" | "untitled" | "page") {
        return;
    }
    if let Some(target) = infer_target_path(user_input) {
        call.arguments["path"] = json!(target);
    }
}

fn infer_target_path(user_input: &str) -> Option<String> {
    let lowered = user_input.to_ascii_lowercase();
    if !(lowered.contains("create")
        || lowered.contains("write")
        || lowered.contains("generate")
        || lowered.contains("make"))
    {
        return None;
    }

    for token in user_input.split_whitespace() {
        let cleaned = token.trim_matches(|ch: char| {
            ch == '"' || ch == '\'' || ch == '`' || ch == ',' || ch == '.' || ch == ':'
        });
        if cleaned.ends_with(".html")
            || cleaned.ends_with(".css")
            || cleaned.ends_with(".js")
            || cleaned.ends_with(".json")
            || cleaned.ends_with(".md")
        {
            return Some(cleaned.replace('\\', "/"));
        }
    }

    if lowered.contains("web page") || lowered.contains("html") {
        return Some("index.html".to_string());
    }
    None
}

fn clean_file_content(response: &str) -> String {
    let trimmed = response.trim();
    if let Some(block) = first_fenced_block(trimmed) {
        return block;
    }
    if let Some(stripped) = strip_fence(trimmed, "html") {
        return stripped;
    }
    if let Some(stripped) = strip_fence(trimmed, "") {
        return stripped;
    }
    trimmed.to_string()
}

fn first_fenced_block(text: &str) -> Option<String> {
    let start = text.find("```")?;
    let after_start = &text[start + 3..];
    let body_start = after_start
        .find('\n')
        .map(|index| start + 3 + index + 1)
        .unwrap_or(start + 3);
    let end = text[body_start..].find("```")? + body_start;
    Some(text[body_start..end].trim().to_string())
}

fn strip_fence(text: &str, language: &str) -> Option<String> {
    let start = if language.is_empty() {
        "```"
    } else {
        return text
            .strip_prefix(&format!("```{language}"))
            .and_then(|rest| rest.strip_suffix("```"))
            .map(|value| value.trim().to_string());
    };
    text.strip_prefix(start)
        .and_then(|rest| rest.strip_suffix("```"))
        .map(|value| value.trim().to_string())
}

fn looks_like_file_content(content: &str, path: &str) -> bool {
    let trimmed = content.trim_start();
    if path.ends_with(".html") {
        return trimmed.starts_with("<!DOCTYPE html")
            || trimmed.starts_with("<!doctype html")
            || trimmed.starts_with("<html");
    }
    if path.ends_with(".json") {
        return trimmed.starts_with('{') || trimmed.starts_with('[');
    }
    !trimmed.is_empty()
}

fn base_prompt() -> &'static str {
    r#"You are ClaudeCodeX, a terminal-only coding agent harness.

Your job is to solve software tasks in the current workspace. Be direct, inspect before editing, use tools when needed, and verify changes with focused commands when possible.

You must respect the harness permission profile. Do not claim that a tool ran unless a tool result says it ran. Keep final answers concise and grounded in observed results.

This is not a private provider prompt clone. Use your native strengths, but follow this visible tool protocol and complete the user's coding task end to end."#
}

#[derive(Serialize)]
struct EventText<T: Serialize> {
    text: T,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_multiple_tool_calls() {
        let parsed = parse_model_output(
            r#"hi <tool_call>{"tool":"read_file","arguments":{"path":"Cargo.toml"}}</tool_call>
            <tool_call>{"tool":"git_status","arguments":{}}</tool_call>"#,
        )
        .unwrap();
        let calls = parsed.calls;
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].tool, "read_file");
        assert_eq!(calls[1].tool, "git_status");
    }

    #[test]
    fn strips_tool_call_blocks() {
        let parsed = parse_model_output(
            r#"before <tool_call>{"tool":"git_status","arguments":{}}</tool_call> after"#,
        )
        .unwrap();
        assert_eq!(parsed.visible_text.trim(), "before  after");
    }

    #[test]
    fn infers_html_write_for_web_page() {
        let call = infer_file_write(
            "Create a simple web page",
            "```html\n<!DOCTYPE html><html><body></body></html>\n```",
        )
        .unwrap();
        assert_eq!(call.tool, "write_file");
        assert_eq!(call.arguments["path"], "index.html");
    }

    #[test]
    fn infers_first_html_fence_before_explanation() {
        let call = infer_file_write(
            "Create index.html",
            "```html\n<!DOCTYPE html><html><body>ok</body></html>\n```\n\nI created it.",
        )
        .unwrap();
        assert_eq!(
            call.arguments["content"],
            "<!DOCTYPE html><html><body>ok</body></html>"
        );
    }

    #[test]
    fn normalizes_vague_local_file_path() {
        let mut call = ToolCall {
            tool: "write_file".to_string(),
            arguments: json!({"path":"file","content":"x"}),
        };
        normalize_local_tool_path("Create index.html", &mut call);
        assert_eq!(call.arguments["path"], "index.html");
    }
}

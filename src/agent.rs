use crate::config::AppConfig;
use crate::context::ProjectContext;
use crate::permissions::{PermissionEngine, PermissionProfile};
use crate::providers::{MessageRole, ModelMessage, ModelRequest, ProviderRegistry};
use crate::session::Session;
use crate::tools::{ToolCall, ToolRegistry, ToolResult};
use anyhow::{bail, Result};
use serde::Serialize;
use std::io::{self, Write};
use std::path::PathBuf;

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

    println!("ClaudeCodeX terminal harness");
    println!("session: {}", state.session.id);
    println!("{}", state.context.summary());
    println!("Type /help for commands, /exit to quit.");

    loop {
        print!("ccx> ");
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
            continue;
        }

        let output = run_agent_turn(&mut state, &providers, &tools, input).await?;
        if !output.trim().is_empty() {
            println!("{output}");
        }
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
    let output = run_agent_turn(&mut state, &providers, &tools, &task).await?;
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
    transcript: Vec<ModelMessage>,
}

impl AgentState {
    fn new(config: AppConfig, context: ProjectContext, session: Session) -> Self {
        Self {
            selected_provider: config.default_provider.clone(),
            selected_model: config.default_model.clone(),
            config,
            context,
            session,
            transcript: Vec::new(),
        }
    }

    fn system_messages(&self) -> Vec<ModelMessage> {
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
}

async fn run_agent_turn(
    state: &mut AgentState,
    providers: &ProviderRegistry,
    tools: &ToolRegistry,
    user_input: &str,
) -> Result<String> {
    state.session.append("user", EventText { text: user_input })?;
    state.transcript.push(ModelMessage {
        role: MessageRole::User,
        content: user_input.to_string(),
    });

    let mut final_visible = String::new();
    for _ in 0..state.config.max_agent_turns {
        let provider = providers.get(&state.selected_provider)?;
        let mut messages = state.system_messages();
        messages.extend(state.transcript.clone());
        let response = provider
            .generate(ModelRequest {
                model: state.selected_model.clone(),
                messages,
            })
            .await?;

        state.session.append("assistant", EventText { text: &response.text })?;
        state.transcript.push(ModelMessage {
            role: MessageRole::Assistant,
            content: response.text.clone(),
        });

        let visible = strip_tool_calls(&response.text);
        if !visible.trim().is_empty() {
            if !final_visible.is_empty() {
                final_visible.push_str("\n\n");
            }
            final_visible.push_str(visible.trim());
        }

        let calls = parse_tool_calls(&response.text)?;
        if calls.is_empty() {
            return Ok(final_visible);
        }

        let mut tool_results = Vec::new();
        for call in calls {
            state.session.append("tool_call", &call)?;
            let result = tools.execute(call).await;
            state.session.append("tool_result", &result)?;
            tool_results.push(result);
        }

        state.transcript.push(ModelMessage {
            role: MessageRole::Tool,
            content: render_tool_results(&tool_results)?,
        });
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
            println!("/help                 show commands");
            println!("/exit                 quit");
            println!("/context              show loaded project context");
            println!("/providers            list configured providers");
            println!("/model [provider] [model] switch or show model");
            println!("/permissions          show active permission profile");
            println!("/session              show session path");
            println!("/compact              append a manual compaction marker");
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
        "/model" => {
            let provider = parts.next();
            let model = parts.next();
            match (provider, model) {
                (Some(provider), Some(model)) => {
                    providers.get(provider)?;
                    state.selected_provider = provider.to_string();
                    state.selected_model = model.to_string();
                    println!("model: {} {}", state.selected_provider, state.selected_model);
                }
                _ => println!("model: {} {}", state.selected_provider, state.selected_model),
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
        other => {
            println!("unknown command `{other}`");
            Ok(false)
        }
    }
}

fn parse_tool_calls(text: &str) -> Result<Vec<ToolCall>> {
    let mut calls = Vec::new();
    let mut remaining = text;
    while let Some(start) = remaining.find("<tool_call>") {
        let after_start = &remaining[start + "<tool_call>".len()..];
        let Some(end) = after_start.find("</tool_call>") else {
            bail!("tool call tag was opened but not closed");
        };
        let raw = after_start[..end].trim();
        calls.push(serde_json::from_str(raw)?);
        remaining = &after_start[end + "</tool_call>".len()..];
    }
    Ok(calls)
}

fn strip_tool_calls(text: &str) -> String {
    let mut output = String::new();
    let mut remaining = text;
    while let Some(start) = remaining.find("<tool_call>") {
        output.push_str(&remaining[..start]);
        let after_start = &remaining[start + "<tool_call>".len()..];
        let Some(end) = after_start.find("</tool_call>") else {
            return output;
        };
        remaining = &after_start[end + "</tool_call>".len()..];
    }
    output.push_str(remaining);
    output
}

fn render_tool_results(results: &[ToolResult]) -> Result<String> {
    Ok(format!(
        "Tool results:\n{}",
        serde_json::to_string_pretty(results)?
    ))
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
        let calls = parse_tool_calls(
            r#"hi <tool_call>{"tool":"read_file","arguments":{"path":"Cargo.toml"}}</tool_call>
            <tool_call>{"tool":"git_status","arguments":{}}</tool_call>"#,
        )
        .unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].tool, "read_file");
        assert_eq!(calls[1].tool, "git_status");
    }

    #[test]
    fn strips_tool_call_blocks() {
        let stripped = strip_tool_calls(
            r#"before <tool_call>{"tool":"git_status","arguments":{}}</tool_call> after"#,
        );
        assert_eq!(stripped.trim(), "before  after");
    }
}

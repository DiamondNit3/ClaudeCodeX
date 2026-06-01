use crate::config::AppConfig;
use crate::context::ProjectContext;
use crate::permissions::{PermissionEngine, PermissionProfile};
use crate::providers::{MessageRole, ModelMessage, ModelRequest, ProviderRegistry};
use crate::session::Session;
use crate::tools::{ToolCall, ToolRegistry, ToolResult};
use crate::ui::{self, FooterInfo, HeaderInfo};
use anyhow::{bail, Result};
use serde::Serialize;
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
        if render_ui {
            ui::thinking();
        }
        let response = provider
            .generate(ModelRequest {
                model: state.selected_model.clone(),
                messages,
            })
            .await?;

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
        "/status" => {
            println!("{}", git_status(&state.context.workspace));
            Ok(false)
        }
        "/diff" => {
            ui::render_diff(&git_diff(&state.context.workspace));
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
    run_git(workspace, &["rev-parse", "--abbrev-ref", "HEAD"])
        .lines()
        .next()
        .unwrap_or("no-git")
        .to_string()
}

fn git_repo_state(workspace: &PathBuf) -> String {
    let status = run_git(workspace, &["status", "--short"]);
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

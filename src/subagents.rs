use crate::config::{AppConfig, ToolProtocol};
use crate::context::ProjectContext;
use crate::hooks::HookRunner;
use crate::parser::parse_model_output;
use crate::permissions::{PermissionEngine, PermissionProfile};
use crate::providers::{MessageRole, ModelMessage, ModelRequest, ProviderRegistry};
use crate::tools::{ToolCall, ToolRegistry, ToolResult};
use anyhow::{bail, Result};
use serde::Serialize;
use serde_json::json;
use std::path::PathBuf;

const MAX_SUBAGENT_TURNS: usize = 4;

#[derive(Debug, Clone, Serialize)]
pub struct SubagentReport {
    pub kind: String,
    pub task: String,
    pub report: String,
    pub tool_calls: usize,
}

pub async fn run_subagent(
    config: &AppConfig,
    providers: &ProviderRegistry,
    workspace: PathBuf,
    kind: &str,
    task: &str,
    render_output: bool,
) -> Result<SubagentReport> {
    let kind = normalize_kind(kind);
    let context = ProjectContext::load(&workspace)?;
    let selected_provider = config
        .model_profiles
        .get(&config.default_model)
        .and_then(|profile| profile.provider.clone())
        .unwrap_or_else(|| config.default_provider.clone());
    let profile = config
        .model_profiles
        .get(&config.default_model)
        .cloned()
        .unwrap_or_default();
    let effort = config.resolve_effort(&selected_provider, &config.default_model);
    let permissions = PermissionEngine::new(PermissionProfile::ReadOnly, workspace.clone(), false);
    let tools = ToolRegistry::new(
        workspace,
        permissions,
        HookRunner::new(
            config.hooks.pre_tool.clone(),
            config.hooks.post_tool.clone(),
        ),
    );

    let mut transcript = subagent_system_messages(&kind, &context, profile.tool_protocol.clone());
    transcript.push(ModelMessage {
        role: MessageRole::User,
        content: format!("Task: {task}\n\nReturn a concise subagent report."),
    });

    let mut tool_call_count = 0;
    let mut last_report = String::new();
    for _ in 0..MAX_SUBAGENT_TURNS.min(config.max_agent_turns.max(1)) {
        let provider = providers.get(&selected_provider)?;
        let response = provider
            .generate(ModelRequest {
                model: config.default_model.clone(),
                messages: transcript.clone(),
                profile: profile.clone(),
                effort,
                tools: ToolRegistry::read_only_tool_specs(),
            })
            .await?;

        let parsed = parse_model_output(&response.text)?;
        let visible = parsed.visible_text.trim();
        if !visible.is_empty() {
            last_report = visible.to_string();
        } else if !response.text.trim().is_empty() {
            last_report = response.text.trim().to_string();
        }
        transcript.push(ModelMessage {
            role: MessageRole::Assistant,
            content: response.text.clone(),
        });

        if parsed.calls.is_empty() {
            if let Some(call) = infer_subagent_tool_call(&response.text) {
                tool_call_count += 1;
                let result = execute_subagent_tool(&tools, call).await;
                transcript.push(ModelMessage {
                    role: MessageRole::Tool,
                    content: format!(
                        "Tool results:\n{}",
                        serde_json::to_string_pretty(&[result])?
                    ),
                });
                continue;
            }
            let report = if visible.is_empty() {
                response.text.trim().to_string()
            } else {
                visible.to_string()
            };
            if render_output {
                println!("{report}");
            }
            return Ok(SubagentReport {
                kind,
                task: task.to_string(),
                report,
                tool_calls: tool_call_count,
            });
        }

        let mut results = Vec::new();
        for call in parsed.calls {
            tool_call_count += 1;
            let result = execute_subagent_tool(&tools, call).await;
            results.push(result);
        }
        transcript.push(ModelMessage {
            role: MessageRole::Tool,
            content: format!("Tool results:\n{}", serde_json::to_string_pretty(&results)?),
        });
    }

    if last_report.trim().is_empty() {
        bail!("subagent `{kind}` reached turn limit before producing a report")
    }
    Ok(SubagentReport {
        kind,
        task: task.to_string(),
        report: format!("Subagent reached its turn limit. Last report:\n{last_report}"),
        tool_calls: tool_call_count,
    })
}

fn infer_subagent_tool_call(response: &str) -> Option<ToolCall> {
    let lowered = response.to_ascii_lowercase();
    if lowered.contains("read_file")
        && (lowered.contains("coding harness")
            || lowered.contains("workspace")
            || lowered.contains("downloads"))
    {
        return Some(ToolCall {
            tool: "glob".to_string(),
            arguments: json!({"pattern": "src/**/*.rs"}),
            call_id: None,
        });
    }
    if lowered.contains("git_status") {
        return Some(ToolCall {
            tool: "git_status".to_string(),
            arguments: json!({}),
            call_id: None,
        });
    }
    if lowered.contains("git_diff") {
        return Some(ToolCall {
            tool: "git_diff".to_string(),
            arguments: json!({}),
            call_id: None,
        });
    }
    None
}

async fn execute_subagent_tool(tools: &ToolRegistry, call: ToolCall) -> ToolResult {
    if !matches!(
        call.tool.as_str(),
        "read_file" | "glob" | "grep" | "git_status" | "git_diff"
    ) {
        return ToolResult {
            tool: call.tool,
            call_id: call.call_id,
            success: false,
            content: "subagents are read-only; allowed tools are read_file, glob, grep, git_status, git_diff"
                .to_string(),
            full_content: None,
            truncated: false,
        };
    }
    tools.execute(call).await
}

fn subagent_system_messages(
    kind: &str,
    context: &ProjectContext,
    tool_protocol: ToolProtocol,
) -> Vec<ModelMessage> {
    let prompt = if tool_protocol == ToolProtocol::SimpleJson {
        format!(
            "You are a ClaudeCodeX {kind} subagent. Inspect only. Do not edit files.\n\nWorkspace: {}\n\nUse one JSON action when needed. Use glob to list files. Use read_file only for an exact file path:\n{{\"action\":\"glob\",\"pattern\":\"src/**/*.rs\"}}\n{{\"action\":\"read_file\",\"path\":\"src/main.rs\"}}\n{{\"action\":\"grep\",\"query\":\"text\",\"path\":\"src\"}}\n{{\"action\":\"git_status\"}}\n{{\"action\":\"git_diff\"}}\n\nProject instructions:\n{}",
            context.workspace.display(),
            context.render_for_prompt()
        )
    } else {
        format!(
            "{}\n\nWorkspace: {}\n\nProject instructions:\n{}\n\n{}",
            subagent_prompt(kind),
            context.workspace.display(),
            context.render_for_prompt(),
            read_only_tool_manifest()
        )
    };
    vec![ModelMessage {
        role: MessageRole::System,
        content: prompt,
    }]
}

fn subagent_prompt(kind: &str) -> String {
    let role = match kind {
        "search" => "Find relevant files, symbols, and evidence. Prefer grep/glob/read_file. Report only what matters.",
        "review" => "Review code and diffs for bugs, regressions, missing tests, and safety risks. Findings first.",
        "test-debug" => "Diagnose test failures from available files and git context. Suggest focused checks, but do not run shell commands.",
        "plan" => "Create a concrete implementation plan grounded in the current workspace.",
        _ => "Investigate the task and return a concise report grounded in observed files.",
    };
    format!(
        "You are a ClaudeCodeX {kind} subagent. {role}\nDo not modify files. Do not claim a tool result unless it appears in tool results. Return a concise report with Evidence and Recommendation sections."
    )
}

fn read_only_tool_manifest() -> &'static str {
    r#"Read-only tools use this exact text protocol:
<tool_call>{"tool":"read_file","arguments":{"path":"src/main.rs"}}</tool_call>
<tool_call>{"tool":"glob","arguments":{"pattern":"src/**/*.rs"}}</tool_call>
<tool_call>{"tool":"grep","arguments":{"query":"TODO","path":"src"}}</tool_call>
<tool_call>{"tool":"git_status","arguments":{}}</tool_call>
<tool_call>{"tool":"git_diff","arguments":{}}</tool_call>"#
}

fn normalize_kind(kind: &str) -> String {
    match kind {
        "planning" => "plan".to_string(),
        "debug" => "test-debug".to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_common_subagent_aliases() {
        assert_eq!(normalize_kind("planning"), "plan");
        assert_eq!(normalize_kind("debug"), "test-debug");
    }

    #[test]
    fn read_only_manifest_excludes_write_tools() {
        let manifest = read_only_tool_manifest();
        assert!(manifest.contains("read_file"));
        assert!(!manifest.contains("write_file"));
        assert!(!manifest.contains("shell"));
    }

    #[test]
    fn recovers_workspace_read_as_glob() {
        let call =
            infer_subagent_tool_call(r#"{"action":"read_file","path":"coding harness"}"#).unwrap();
        assert_eq!(call.tool, "glob");
    }
}

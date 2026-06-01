use crate::hooks::HookRunner;
use crate::patch::{apply_edit, diff_summary};
use crate::permissions::PermissionEngine;
use crate::safety::classify_command;
use anyhow::{Context, Result};
use glob::glob;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;
use tokio::time::{timeout, Duration};
use walkdir::WalkDir;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub tool: String,
    #[serde(default)]
    pub arguments: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool: String,
    pub success: bool,
    pub content: String,
}

pub struct ToolRegistry {
    workspace: PathBuf,
    permissions: PermissionEngine,
    hooks: HookRunner,
}

impl ToolRegistry {
    pub fn new(workspace: PathBuf, permissions: PermissionEngine, hooks: HookRunner) -> Self {
        Self {
            workspace,
            permissions,
            hooks,
        }
    }

    pub fn tool_manifest() -> &'static str {
        r#"Available tools use this exact text protocol:
<tool_call>{"tool":"read_file","arguments":{"path":"src/main.rs"}}</tool_call>
<tool_call>{"tool":"write_file","arguments":{"path":"notes.txt","content":"..."}}</tool_call>
<tool_call>{"tool":"edit_file","arguments":{"path":"src/main.rs","old":"exact old text","new":"replacement text"}}</tool_call>
<tool_call>{"tool":"apply_patch","arguments":{"path":"src/main.rs","patch":"@@\n old\n-new\n+new"}}</tool_call>
<tool_call>{"tool":"glob","arguments":{"pattern":"src/**/*.rs"}}</tool_call>
<tool_call>{"tool":"grep","arguments":{"query":"TODO","path":"src"}}</tool_call>
<tool_call>{"tool":"shell","arguments":{"command":"cargo test"}}</tool_call>
<tool_call>{"tool":"git_status","arguments":{}}</tool_call>
<tool_call>{"tool":"git_diff","arguments":{}}</tool_call>

Use tools when needed to inspect or modify the workspace. After tool results, continue until the task is complete."#
    }

    pub fn native_tool_specs() -> Vec<ToolSpec> {
        vec![
            ToolSpec::new("read_file", "Read a UTF-8 text file from the workspace"),
            ToolSpec::new("write_file", "Write a UTF-8 text file in the workspace"),
            ToolSpec::new(
                "edit_file",
                "Replace exact old text or apply a patch to a file",
            ),
            ToolSpec::new(
                "apply_patch",
                "Apply a guarded unified diff patch to one file",
            ),
            ToolSpec::new("glob", "List workspace files matching a glob"),
            ToolSpec::new("grep", "Search workspace files for text"),
            ToolSpec::new("shell", "Run a shell command after permission checks"),
            ToolSpec::new("git_status", "Show git status"),
            ToolSpec::new("git_diff", "Show git diff"),
        ]
    }

    pub fn read_only_tool_specs() -> Vec<ToolSpec> {
        vec![
            ToolSpec::new("read_file", "Read a UTF-8 text file from the workspace"),
            ToolSpec::new("glob", "List workspace files matching a glob"),
            ToolSpec::new("grep", "Search workspace files for text"),
            ToolSpec::new("git_status", "Show git status"),
            ToolSpec::new("git_diff", "Show git diff"),
        ]
    }

    pub async fn execute(&self, call: ToolCall) -> ToolResult {
        let tool_name = call.tool.clone();
        if let Err(error) = self.hooks.run_pre_tool(&self.workspace, &call) {
            return ToolResult {
                tool: tool_name,
                success: false,
                content: format!("{error:#}"),
            };
        }
        let result = match call.tool.as_str() {
            "read_file" => self.read_file(&call.arguments).await,
            "write_file" => self.write_file(&call.arguments).await,
            "edit_file" => self.edit_file(&call.arguments).await,
            "apply_patch" => self.apply_patch(&call.arguments).await,
            "glob" => self.glob_files(&call.arguments).await,
            "grep" => self.grep_files(&call.arguments).await,
            "shell" => self.shell(&call.arguments).await,
            "git_status" => self.git_status().await,
            "git_diff" => self.git_diff().await,
            other => {
                return ToolResult {
                    tool: tool_name,
                    success: false,
                    content: format!("unknown tool `{other}`"),
                }
            }
        };

        let result = match result {
            Ok(content) => ToolResult {
                tool: tool_name,
                success: true,
                content,
            },
            Err(error) => ToolResult {
                tool: tool_name,
                success: false,
                content: format!("{error:#}"),
            },
        };
        if let Err(error) = self.hooks.run_post_tool(&self.workspace, &result) {
            return ToolResult {
                tool: result.tool,
                success: false,
                content: format!("{error:#}"),
            };
        }
        result
    }

    async fn read_file(&self, args: &Value) -> Result<String> {
        let path = self.path_arg(args)?;
        self.permissions.check_read_path(&path)?;
        fs::read_to_string(self.resolve(&path)?)
            .with_context(|| format!("failed to read {}", path.display()))
    }

    async fn write_file(&self, args: &Value) -> Result<String> {
        let path = self.path_arg(args)?;
        let content = string_arg(args, "content")?;
        self.permissions.check_write_path(&path)?;
        let resolved = self.resolve(&path)?;
        if let Some(parent) = resolved.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&resolved, content)?;
        Ok(format!(
            "wrote {}\n{} lines, {} bytes",
            resolved.display(),
            content.lines().count(),
            content.len()
        ))
    }

    async fn edit_file(&self, args: &Value) -> Result<String> {
        let path = self.path_arg(args)?;
        self.permissions.check_write_path(&path)?;
        let resolved = self.resolve(&path)?;
        let original = fs::read_to_string(&resolved)?;
        let old = args.get("old").and_then(Value::as_str);
        let new = args.get("new").and_then(Value::as_str);
        let patch = args.get("patch").and_then(Value::as_str);
        let updated = apply_edit(&original, old, new, patch)
            .with_context(|| format!("failed to edit {}", resolved.display()))?;
        let line_count = updated.lines().count();
        let byte_count = updated.len();
        let summary = diff_summary(&original, &updated);
        fs::write(&resolved, updated)?;
        Ok(format!(
            "edited {}\n{}\n{} lines, {} bytes",
            resolved.display(),
            summary,
            line_count,
            byte_count
        ))
    }

    async fn apply_patch(&self, args: &Value) -> Result<String> {
        let path = self.path_arg(args)?;
        let patch = string_arg(args, "patch")?;
        self.permissions.check_write_path(&path)?;
        let resolved = self.resolve(&path)?;
        let original = fs::read_to_string(&resolved)?;
        let updated = apply_edit(&original, None, None, Some(patch))
            .with_context(|| format!("failed to patch {}", resolved.display()))?;
        let summary = diff_summary(&original, &updated);
        fs::write(&resolved, updated)?;
        Ok(format!("patched {}\n{}", resolved.display(), summary))
    }

    async fn glob_files(&self, args: &Value) -> Result<String> {
        let pattern = string_arg(args, "pattern")?;
        let full_pattern = self.workspace.join(pattern);
        let pattern = full_pattern.to_string_lossy().replace('\\', "/");
        let mut matches = Vec::new();
        for entry in glob(&pattern)? {
            let path = entry?;
            if path.is_file() {
                matches.push(display_relative(&self.workspace, &path));
            }
            if matches.len() >= 200 {
                matches.push("<truncated at 200 matches>".to_string());
                break;
            }
        }
        Ok(matches.join("\n"))
    }

    async fn grep_files(&self, args: &Value) -> Result<String> {
        let query = string_arg(args, "query")?;
        let path = args
            .get("path")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        self.permissions.check_read_path(&path)?;
        let root = self.resolve(&path)?;
        let mut matches = Vec::new();

        for entry in WalkDir::new(root)
            .into_iter()
            .filter_map(|entry| entry.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            let Ok(content) = fs::read_to_string(path) else {
                continue;
            };
            for (line_index, line) in content.lines().enumerate() {
                if line.contains(query) {
                    matches.push(format!(
                        "{}:{}:{}",
                        display_relative(&self.workspace, path),
                        line_index + 1,
                        line.trim()
                    ));
                    if matches.len() >= 200 {
                        matches.push("<truncated at 200 matches>".to_string());
                        return Ok(matches.join("\n"));
                    }
                }
            }
        }

        Ok(matches.join("\n"))
    }

    async fn shell(&self, args: &Value) -> Result<String> {
        let command = string_arg(args, "command")?;
        self.permissions.check_shell(command)?;
        run_shell(&self.workspace, command).await
    }

    async fn git_status(&self) -> Result<String> {
        run_shell(&self.workspace, "git status --short --branch").await
    }

    async fn git_diff(&self) -> Result<String> {
        run_shell(&self.workspace, "git diff --stat; git diff --").await
    }

    fn path_arg(&self, args: &Value) -> Result<PathBuf> {
        Ok(PathBuf::from(string_arg(args, "path")?))
    }

    fn resolve(&self, path: &Path) -> Result<PathBuf> {
        let resolved = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.workspace.join(path)
        };
        Ok(resolved)
    }
}

fn string_arg<'a>(args: &'a Value, name: &str) -> Result<&'a str> {
    args.get(name)
        .and_then(Value::as_str)
        .with_context(|| format!("missing string argument `{name}`"))
}

async fn run_shell(workspace: &Path, command: &str) -> Result<String> {
    let mut process = if cfg!(windows) {
        let mut process = Command::new("powershell");
        process.args(["-NoProfile", "-Command", command]);
        process
    } else {
        let mut process = Command::new("sh");
        process.args(["-lc", command]);
        process
    };

    let output = timeout(
        Duration::from_secs(300),
        process
            .current_dir(workspace)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output(),
    )
    .await
    .context("command timed out after 300 seconds")??;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let status = output.status.code().unwrap_or(-1);
    let risk = classify_command(command).to_string();
    Ok(json!({
        "status": status,
        "risk": risk,
        "stdout": stdout,
        "stderr": stderr
    })
    .to_string())
}

fn display_relative(workspace: &Path, path: &Path) -> String {
    path.strip_prefix(workspace)
        .unwrap_or(path)
        .display()
        .to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
}

impl ToolSpec {
    fn new(name: &str, description: &str) -> Self {
        Self {
            name: name.to_string(),
            description: description.to_string(),
        }
    }
}

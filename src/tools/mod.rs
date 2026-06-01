use crate::hooks::HookRunner;
use crate::patch::{apply_edit, diff_summary};
use crate::permissions::PermissionEngine;
use crate::safety::classify_command;
use anyhow::{Context, Result};
use glob::glob;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use tokio::process::Command;
use tokio::time::{timeout, Duration};
use walkdir::WalkDir;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub tool: String,
    #[serde(default)]
    pub arguments: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    pub success: bool,
    pub content: String,
    #[serde(default)]
    pub full_content: Option<String>,
    #[serde(default)]
    pub truncated: bool,
}

pub struct ToolRegistry {
    workspace: PathBuf,
    permissions: PermissionEngine,
    hooks: HookRunner,
    cache: Mutex<HashMap<String, String>>,
}

impl ToolRegistry {
    pub fn new(workspace: PathBuf, permissions: PermissionEngine, hooks: HookRunner) -> Self {
        Self {
            workspace,
            permissions,
            hooks,
            cache: Mutex::new(HashMap::new()),
        }
    }

    pub fn tool_manifest() -> &'static str {
        r#"Available tools use this exact text protocol:
<tool_call>{"tool":"read_file","arguments":{"path":"src/main.rs"}}</tool_call>
<tool_call>{"tool":"read_file","arguments":{"path":"src/main.rs","start_line":1,"line_count":80}}</tool_call>
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
        let call_id = call.call_id.clone();
        if let Err(error) = self.hooks.run_pre_tool(&self.workspace, &call) {
            return ToolResult {
                tool: tool_name,
                call_id,
                success: false,
                content: format!("{error:#}"),
                full_content: None,
                truncated: false,
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
                    call_id,
                    success: false,
                    content: format!("unknown tool `{other}`"),
                    full_content: None,
                    truncated: false,
                }
            }
        };

        let result = match result {
            Ok(content) => {
                let (visible, full_content, truncated) = truncate_tool_content(&content);
                ToolResult {
                    tool: tool_name,
                    call_id,
                    success: true,
                    content: visible,
                    full_content,
                    truncated,
                }
            }
            Err(error) => ToolResult {
                tool: tool_name,
                call_id,
                success: false,
                content: format!("{error:#}"),
                full_content: None,
                truncated: false,
            },
        };
        if let Err(error) = self.hooks.run_post_tool(&self.workspace, &result) {
            return ToolResult {
                tool: result.tool,
                call_id: result.call_id,
                success: false,
                content: format!("{error:#}"),
                full_content: None,
                truncated: false,
            };
        }
        result
    }

    async fn read_file(&self, args: &Value) -> Result<String> {
        let path = self.path_arg(args)?;
        self.permissions.check_read_path(&path)?;
        let content = fs::read_to_string(self.resolve(&path)?)
            .with_context(|| format!("failed to read {}", path.display()))?;
        if let Some(start_line) = args.get("start_line").and_then(Value::as_u64) {
            let line_count = args
                .get("line_count")
                .and_then(Value::as_u64)
                .unwrap_or(120) as usize;
            return Ok(read_window(&content, start_line as usize, line_count));
        }
        Ok(content)
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
        let cache_key = format!("grep:{}:{}", query, path.display());
        if let Some(value) = self
            .cache
            .lock()
            .ok()
            .and_then(|cache| cache.get(&cache_key).cloned())
        {
            return Ok(value);
        }
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
                        let output = matches.join("\n");
                        if let Ok(mut cache) = self.cache.lock() {
                            cache.insert(cache_key, output.clone());
                        }
                        return Ok(output);
                    }
                }
            }
        }

        let output = matches.join("\n");
        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(cache_key, output.clone());
        }
        Ok(output)
    }

    async fn shell(&self, args: &Value) -> Result<String> {
        let command = string_arg(args, "command")?;
        self.permissions.check_shell(command)?;
        run_shell(&self.workspace, command).await
    }

    async fn git_status(&self) -> Result<String> {
        self.cached_shell("git_status", "git status --short --branch")
            .await
    }

    async fn git_diff(&self) -> Result<String> {
        self.cached_shell("git_diff", "git diff --stat; git diff --")
            .await
    }

    async fn cached_shell(&self, key: &str, command: &str) -> Result<String> {
        if let Some(value) = self
            .cache
            .lock()
            .ok()
            .and_then(|cache| cache.get(key).cloned())
        {
            return Ok(value);
        }
        let output = run_shell(&self.workspace, command).await?;
        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(key.to_string(), output.clone());
        }
        Ok(output)
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

fn read_window(content: &str, start_line: usize, line_count: usize) -> String {
    let start = start_line.saturating_sub(1);
    content
        .lines()
        .enumerate()
        .skip(start)
        .take(line_count)
        .map(|(index, line)| format!("{}:{}", index + 1, line))
        .collect::<Vec<_>>()
        .join("\n")
}

fn truncate_tool_content(content: &str) -> (String, Option<String>, bool) {
    const MAX_VISIBLE: usize = 12_000;
    if content.len() <= MAX_VISIBLE {
        return (content.to_string(), None, false);
    }
    let mut visible = content.chars().take(MAX_VISIBLE).collect::<String>();
    visible.push_str("\n<truncated: full output stored in session full_content>");
    (visible, Some(content.to_string()), true)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_window_adds_line_numbers() {
        assert_eq!(read_window("a\nb\nc", 2, 2), "2:b\n3:c");
    }

    #[test]
    fn truncates_large_tool_output() {
        let content = "x".repeat(13_000);
        let (visible, full, truncated) = truncate_tool_content(&content);
        assert!(truncated);
        assert!(visible.contains("<truncated"));
        assert_eq!(full.as_deref(), Some(content.as_str()));
    }
}

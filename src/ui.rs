use crate::tools::{ToolCall, ToolResult};
use anyhow::Result;
use crossterm::{
    cursor::MoveTo,
    execute,
    style::Stylize,
    terminal::{Clear, ClearType},
};
use serde_json::Value;
use std::io;
use std::path::Path;

pub struct HeaderInfo<'a> {
    pub version: &'a str,
    pub provider: &'a str,
    pub model: &'a str,
    pub permissions: &'a str,
    pub workspace: &'a Path,
    pub context_files: usize,
    pub session_short: &'a str,
    pub mode: &'a str,
}

pub struct FooterInfo<'a> {
    pub provider: &'a str,
    pub model: &'a str,
    pub permissions: &'a str,
    pub branch: &'a str,
    pub repo_state: &'a str,
    pub session_short: &'a str,
}

pub fn render_header(info: HeaderInfo<'_>) {
    println!("{}  {}", "ClaudeCodeX".bold(), info.version.dim());
    println!(
        "{}   {:<22} {}   {}",
        "model".dim(),
        format!("{}:{}", info.provider, info.model).cyan(),
        "permissions".dim(),
        info.permissions.yellow()
    );
    println!(
        "{}    {:<22} {}       {} instruction file{}",
        "repo".dim(),
        repo_name(info.workspace).cyan(),
        "context".dim(),
        info.context_files,
        if info.context_files == 1 { "" } else { "s" }
    );
    println!(
        "{} {}               {}          {}",
        "session".dim(),
        info.session_short,
        "mode".dim(),
        info.mode
    );
    println!();
}

pub fn render_footer(info: FooterInfo<'_>) {
    println!(
        "{} | {} | {} | {} | session {}",
        format!("{}:{}", info.provider, info.model).cyan(),
        info.permissions.yellow(),
        info.branch,
        color_repo_state(info.repo_state),
        info.session_short
    );
}

pub fn prompt() -> String {
    format!("{}", "ccx > ".cyan())
}

pub fn thinking() {
    println!("{}", "thinking...".dim());
}

pub fn working_for_tool(call: &ToolCall) {
    println!("{}", working_message(call).dim());
}

pub fn render_tool_call(call: &ToolCall) {
    let target = tool_target(call);
    if target.is_empty() {
        println!("{} {}", "*".cyan(), call.tool.as_str().bold());
    } else {
        println!(
            "{} {}  {}",
            "*".cyan(),
            call.tool.as_str().bold(),
            target.dim()
        );
    }
}

pub fn render_tool_result(result: &ToolResult) {
    let status = if result.success {
        "ok".green()
    } else {
        "failed".red()
    };
    println!("  {} {}", status, summarize_tool_result(result));
}

pub fn render_grouped_help() {
    println!("{}", "Session".bold());
    println!("  /session       show session path");
    println!("  /compact       append compaction marker");
    println!("  /clear         clear screen");
    println!("  /exit          quit");
    println!();
    println!("{}", "Model".bold());
    println!("  /model         show or switch model");
    println!("  /providers     list providers");
    println!();
    println!("{}", "Workspace".bold());
    println!("  /context       show loaded instructions");
    println!("  /status        show git status");
    println!("  /diff          show git diff");
    println!("  /preview       serve a file locally");
    println!();
    println!("{}", "Security".bold());
    println!("  /permissions   show permission profile");
}

pub fn render_diff(diff: &str) {
    if diff.trim().is_empty() {
        println!("{}", "no diff".green());
        return;
    }

    for line in diff.lines() {
        if line.starts_with("+++") || line.starts_with("---") || line.starts_with("@@") {
            println!("{}", line.cyan());
        } else if line.starts_with('+') {
            println!("{}", line.green());
        } else if line.starts_with('-') {
            println!("{}", line.red());
        } else {
            println!("{line}");
        }
    }
}

pub fn clear_screen() -> Result<()> {
    execute!(io::stdout(), Clear(ClearType::All), MoveTo(0, 0))?;
    Ok(())
}

fn repo_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("workspace")
        .to_string()
}

fn color_repo_state(state: &str) -> String {
    if state == "clean" {
        format!("{}", state.green())
    } else {
        format!("{}", state.yellow())
    }
}

fn working_message(call: &ToolCall) -> &'static str {
    match call.tool.as_str() {
        "read_file" | "glob" | "grep" | "git_status" | "git_diff" => "inspecting files...",
        "write_file" | "edit_file" => "applying edit...",
        "shell" => "running command...",
        _ => "using tool...",
    }
}

fn tool_target(call: &ToolCall) -> String {
    match call.tool.as_str() {
        "read_file" | "write_file" | "edit_file" => arg(&call.arguments, "path"),
        "glob" => arg(&call.arguments, "pattern"),
        "grep" => {
            let query = arg(&call.arguments, "query");
            let path = arg(&call.arguments, "path");
            if path.is_empty() {
                query
            } else {
                format!("{query} in {path}")
            }
        }
        "shell" => arg(&call.arguments, "command"),
        _ => String::new(),
    }
}

fn summarize_tool_result(result: &ToolResult) -> String {
    if !result.success {
        return first_line_or_count(&result.content);
    }

    match result.tool.as_str() {
        "read_file" => format!("{} lines", result.content.lines().count()),
        "write_file" | "edit_file" => summarize_file_mutation(&result.content),
        "glob" => format!("{} matches", non_empty_line_count(&result.content)),
        "grep" => format!("{} matches", non_empty_line_count(&result.content)),
        "shell" | "git_status" | "git_diff" => summarize_shell_json(&result.content),
        _ => first_line_or_count(&result.content),
    }
}

fn summarize_file_mutation(content: &str) -> String {
    let mut lines = content.lines().filter(|line| !line.trim().is_empty());
    let first = lines.next().unwrap_or("updated");
    let second = lines.next().unwrap_or("");
    if second.is_empty() {
        first.to_string()
    } else {
        format!("{first}  {second}")
    }
}

fn summarize_shell_json(content: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(content) else {
        return first_line_or_count(content);
    };

    let status = value.get("status").and_then(Value::as_i64).unwrap_or(-1);
    let stdout = value
        .get("stdout")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let stderr = value
        .get("stderr")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let preview = stdout
        .lines()
        .chain(stderr.lines())
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");

    if preview.is_empty() {
        format!("exit {status}")
    } else {
        format!("exit {status}  {}", truncate(preview.trim(), 96))
    }
}

fn first_line_or_count(content: &str) -> String {
    content
        .lines()
        .find(|line| !line.trim().is_empty())
        .map(|line| truncate(line.trim(), 120))
        .unwrap_or_else(|| "no output".to_string())
}

fn non_empty_line_count(content: &str) -> usize {
    content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count()
}

fn arg(args: &Value, name: &str) -> String {
    args.get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn truncate(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

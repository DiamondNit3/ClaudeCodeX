use crate::tools::{ToolCall, ToolResult};
use anyhow::Result;
use crossterm::{
    cursor::{MoveTo, MoveUp},
    execute,
    style::Stylize,
    terminal::{self, Clear, ClearType},
};
use serde_json::Value;
use std::io::{self, Write};
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use std::thread;
use std::time::Duration;

static MASCOT_TICK: AtomicUsize = AtomicUsize::new(0);
const MASCOT_HEIGHT: usize = 4;
const MASCOT_WIDTH: usize = 12;
const MASCOT_FRAMES: [[&str; MASCOT_HEIGHT]; 3] = [
    [
        r"\  _    _  /",
        r" \(o)--(o)/ ",
        r"  /  ==  \  ",
        r" /_/    \_\ ",
    ],
    [
        r" \ _    _ / ",
        r"  (o)--(o)  ",
        r"  /  ==  \  ",
        r" /_/    \_\ ",
    ],
    [
        r"  _      _  ",
        r" (o)--(o)   ",
        r" /  ==  \   ",
        r"/_/    \_\  ",
    ],
];

pub struct HeaderInfo<'a> {
    pub version: &'a str,
    pub provider: &'a str,
    pub model: &'a str,
    pub effort: &'a str,
    pub permissions: &'a str,
    pub workspace: &'a Path,
    pub context_files: usize,
    pub session_short: &'a str,
    pub mode: &'a str,
}

pub struct FooterInfo<'a> {
    pub provider: &'a str,
    pub model: &'a str,
    pub effort: &'a str,
    pub permissions: &'a str,
    pub mode: &'a str,
    pub branch: &'a str,
    pub repo_state: &'a str,
    pub session_short: &'a str,
}

pub fn render_header(info: HeaderInfo<'_>) {
    let mascot = mascot_frame(0);
    println!(
        "{}  {}  {}",
        mascot[0].red(),
        "ClaudeCodeX".bold(),
        info.version.dim()
    );
    println!(
        "{}  {}   {:<22} {}       {}",
        mascot[1].red(),
        "model".dim(),
        format!("{}:{}", info.provider, info.model).cyan(),
        "effort".dim(),
        info.effort.magenta()
    );
    println!(
        "{}  {} {:<22} {}       {} instruction file{}",
        mascot[2].red(),
        "permissions".dim(),
        info.permissions.yellow(),
        "context".dim(),
        info.context_files,
        if info.context_files == 1 { "" } else { "s" }
    );
    println!(
        "{}  {}    {}",
        mascot[3].red(),
        "repo".dim(),
        repo_name(info.workspace).cyan()
    );
    println!(
        "{}  {} {}               {}          {}",
        mascot_indent(),
        "session".dim(),
        info.session_short,
        "mode".dim(),
        info.mode
    );
    println!();
}

pub fn render_footer(info: FooterInfo<'_>) {
    println!(
        "{} | effort {} | {} | mode {} | {} | {} | session {}",
        format!("{}:{}", info.provider, info.model).cyan(),
        info.effort.magenta(),
        info.permissions.yellow(),
        info.mode.cyan(),
        info.branch,
        color_repo_state(info.repo_state),
        info.session_short
    );
}

pub fn prompt_box(mode: &str) -> String {
    let top = prompt_border_top(mode);
    format!("{}\n{} ", top.dark_grey(), "│".cyan())
}

pub fn close_prompt_box() {
    println!("{}", prompt_border_bottom().dark_grey());
}

pub fn working_for_tool(call: &ToolCall) {
    println!(
        "{} {}",
        mascot_frame(MASCOT_TICK.fetch_add(1, Ordering::Relaxed))[0].red(),
        working_message(call).dim()
    );
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

pub fn render_stream_chunk(chunk: &str) {
    print!("{chunk}");
    let _ = io::stdout().flush();
}

pub fn render_grouped_help() {
    println!("{}", "Session".bold());
    println!("  /session       show session path");
    println!("  /compact       append compaction marker");
    println!("  /plan          turn plan mode on, off, or show status");
    println!("  /approve       implement the pending plan");
    println!("  /reject        discard the pending plan");
    println!("  /mascot        preview terminal mascot");
    println!("  /clear         clear screen");
    println!("  /exit          quit");
    println!();
    println!("{}", "Model".bold());
    println!("  /model         show or switch model");
    println!("  /effort        show or set effort");
    println!("  /providers     list providers");
    println!();
    println!("{}", "Workspace".bold());
    println!("  /context       show loaded instructions");
    println!("  /status        show git status");
    println!("  /review        review current git diff");
    println!("  /diff          show git diff");
    println!("  /preview       serve a file locally");
    println!("  /skills        list reusable workflow skills");
    println!("  /subagent      run helper subagent");
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

pub struct ActivityAnimation {
    stop: Arc<AtomicBool>,
    handle: thread::JoinHandle<()>,
}

impl ActivityAnimation {
    pub fn start(label: &'static str) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_thread = Arc::clone(&stop);
        let handle = thread::spawn(move || {
            let mut rendered = false;
            let mut tick = 0;
            while !stop_for_thread.load(Ordering::Relaxed) {
                if rendered {
                    clear_mascot_block();
                }
                render_mascot_block(tick, Some(label));
                rendered = true;
                tick += 1;
                thread::sleep(Duration::from_millis(140));
            }
            if rendered {
                clear_mascot_block();
            }
        });

        Self { stop, handle }
    }

    pub fn stop(self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.handle.join();
        let _ = io::stdout().flush();
    }
}

pub fn render_mascot_preview() {
    let mut rendered = false;
    for tick in 0..16 {
        if rendered {
            clear_mascot_block();
        }
        render_mascot_block(tick, Some("ClaudeCodeX"));
        rendered = true;
        thread::sleep(Duration::from_millis(110));
    }
}

pub fn clear_screen() -> Result<()> {
    execute!(io::stdout(), Clear(ClearType::All), MoveTo(0, 0))?;
    Ok(())
}

fn render_mascot_block(tick: usize, label: Option<&str>) {
    let frame = mascot_frame(tick);
    for (index, line) in frame.iter().enumerate() {
        if index == 0 {
            if let Some(label) = label {
                let label = if label == "ClaudeCodeX" {
                    label.to_string()
                } else {
                    format!("{label}...")
                };
                println!("{} {}", line.red(), label.dim());
                continue;
            }
        }
        println!("{}", line.red());
    }
    let _ = io::stdout().flush();
}

fn clear_mascot_block() {
    let _ = execute!(
        io::stdout(),
        MoveUp(MASCOT_HEIGHT as u16),
        Clear(ClearType::FromCursorDown)
    );
}

fn mascot_indent() -> String {
    " ".repeat(MASCOT_WIDTH)
}

fn mascot_frame(tick: usize) -> &'static [&'static str; MASCOT_HEIGHT] {
    &MASCOT_FRAMES[tick % MASCOT_FRAMES.len()]
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

fn prompt_border_top(mode: &str) -> String {
    let label = format!(" ccx · {mode} ");
    let width = prompt_box_width();
    let fill = width.saturating_sub(label.chars().count() + 1);
    format!("╭{label}{}", "─".repeat(fill))
}

fn prompt_border_bottom() -> String {
    let width = prompt_box_width();
    format!("╰{}", "─".repeat(width.saturating_sub(1)))
}

fn prompt_box_width() -> usize {
    terminal::size()
        .map(|(width, _)| usize::from(width))
        .unwrap_or(80)
        .clamp(40, 96)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mascot_frames_keep_stable_width() {
        for frame in MASCOT_FRAMES {
            for line in frame {
                assert_eq!(line.chars().count(), MASCOT_WIDTH);
            }
        }
    }

    #[test]
    fn prompt_borders_have_stable_width() {
        let top = prompt_border_top("agent");
        let bottom = prompt_border_bottom();
        assert_eq!(top.chars().count(), bottom.chars().count());
        assert!(top.contains("ccx"));
    }
}

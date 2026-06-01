use crate::config::HookCommand;
use crate::tools::{ToolCall, ToolResult};
use anyhow::{bail, Context, Result};
use serde_json::json;
use std::path::Path;
use std::process::{Command, Stdio};

#[derive(Debug, Clone, Default)]
pub struct HookRunner {
    pre_tool: Vec<HookCommand>,
    post_tool: Vec<HookCommand>,
}

impl HookRunner {
    pub fn new(pre_tool: Vec<HookCommand>, post_tool: Vec<HookCommand>) -> Self {
        Self {
            pre_tool,
            post_tool,
        }
    }

    pub fn run_pre_tool(&self, workspace: &Path, call: &ToolCall) -> Result<()> {
        for hook in &self.pre_tool {
            let payload = json!({
                "phase": "pre-tool",
                "tool": call.tool,
                "arguments": call.arguments
            });
            run_hook(workspace, hook, &payload.to_string())?;
        }
        Ok(())
    }

    pub fn run_post_tool(&self, workspace: &Path, result: &ToolResult) -> Result<()> {
        for hook in &self.post_tool {
            let payload = json!({
                "phase": "post-tool",
                "tool": result.tool,
                "success": result.success,
                "content": result.content
            });
            run_hook(workspace, hook, &payload.to_string())?;
        }
        Ok(())
    }
}

fn run_hook(workspace: &Path, hook: &HookCommand, payload: &str) -> Result<()> {
    let mut command = Command::new(&hook.command);
    command
        .args(&hook.args)
        .current_dir(workspace)
        .env("CCX_HOOK_PAYLOAD", payload)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output = command
        .output()
        .with_context(|| format!("failed to run hook `{}`", hook.command))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "hook `{}` denied operation with status {:?}: {}",
            hook.command,
            output.status.code(),
            stderr.trim()
        );
    }

    Ok(())
}

use anyhow::Result;
use std::path::Path;
use std::process::Command;

pub fn run_subagent(workspace: &Path, kind: &str, task: &str) -> Result<()> {
    match kind {
        "search" => search_subagent(workspace, task),
        "review" => {
            crate::review::run_review(workspace, &[])?;
            Ok(())
        }
        "test-debug" => test_debug_subagent(workspace),
        "plan" | "planning" => {
            println!("subagent: planning");
            println!("task: {task}");
            println!("1. inspect relevant files");
            println!("2. make scoped edits");
            println!("3. run focused checks");
            println!("4. summarize results");
            Ok(())
        }
        other => {
            println!("unknown subagent `{other}`; use search, review, test-debug, or plan");
            Ok(())
        }
    }
}

fn search_subagent(workspace: &Path, task: &str) -> Result<()> {
    let query = task
        .split_whitespace()
        .find(|word| word.len() > 3)
        .unwrap_or(task);
    let output = Command::new("git")
        .args(["grep", "-n", query])
        .current_dir(workspace)
        .output()?;
    println!("{}", String::from_utf8_lossy(&output.stdout));
    Ok(())
}

fn test_debug_subagent(workspace: &Path) -> Result<()> {
    let command = if workspace.join("Cargo.toml").exists() {
        "cargo test"
    } else if workspace.join("package.json").exists() {
        "npm test"
    } else {
        "git status --short"
    };
    println!("suggested check: {command}");
    Ok(())
}

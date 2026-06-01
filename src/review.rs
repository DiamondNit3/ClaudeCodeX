use anyhow::Result;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn run_review(workspace: &Path, paths: &[PathBuf]) -> Result<()> {
    let diff = if paths.is_empty() {
        run_git(workspace, &["diff", "--"])?
    } else {
        let mut args = vec!["diff", "--"];
        let owned = paths
            .iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        args.extend(owned.iter().map(String::as_str));
        run_git(workspace, &args)?
    };

    let findings = review_diff(&diff);
    if findings.is_empty() {
        println!("No findings.");
        if diff.trim().is_empty() {
            println!("No git diff was available to review.");
        }
        return Ok(());
    }

    for finding in findings {
        println!(
            "[{}] {}: {}",
            finding.severity, finding.location, finding.message
        );
    }
    Ok(())
}

pub fn review_diff(diff: &str) -> Vec<Finding> {
    let mut findings = Vec::new();
    let mut file = "diff".to_string();
    let mut new_line = 0usize;

    for line in diff.lines() {
        if let Some(path) = line.strip_prefix("+++ b/") {
            file = path.to_string();
            new_line = 0;
            continue;
        }
        if line.starts_with("@@") {
            if let Some(index) = line.find('+') {
                let tail = &line[index + 1..];
                let number = tail
                    .split(|ch| ch == ',' || ch == ' ')
                    .next()
                    .unwrap_or("0")
                    .parse::<usize>()
                    .unwrap_or(0);
                new_line = number.saturating_sub(1);
            }
            continue;
        }
        if line.starts_with('+') && !line.starts_with("+++") {
            new_line += 1;
            let lowered = line.to_ascii_lowercase();
            if lowered.contains("unwrap()") || lowered.contains("expect(") {
                findings.push(Finding::new(
                    "P2",
                    &file,
                    new_line,
                    "new panic path needs justification",
                ));
            }
            if lowered.contains("todo!") || lowered.contains("unimplemented!") {
                findings.push(Finding::new(
                    "P1",
                    &file,
                    new_line,
                    "placeholder code can ship as a runtime failure",
                ));
            }
            if lowered.contains("danger-full-access") {
                findings.push(Finding::new(
                    "P1",
                    &file,
                    new_line,
                    "danger-full-access path should be tightly scoped",
                ));
            }
        } else if !line.starts_with('-') {
            new_line += 1;
        }
    }

    findings
}

#[derive(Debug, Clone)]
pub struct Finding {
    severity: &'static str,
    location: String,
    message: &'static str,
}

impl Finding {
    fn new(severity: &'static str, file: &str, line: usize, message: &'static str) -> Self {
        Self {
            severity,
            location: format!("{file}:{line}"),
            message,
        }
    }
}

fn run_git(workspace: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(workspace)
        .output()?;
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_added_unwraps() {
        let diff = "+++ b/src/main.rs\n@@ -1 +1 @@\n+let value = item.unwrap();";
        assert_eq!(review_diff(diff).len(), 1);
    }
}

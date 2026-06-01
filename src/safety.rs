use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CommandRisk {
    ReadOnly,
    Write,
    Network,
    PackageInstall,
    Destructive,
    Credential,
    Privilege,
}

impl std::fmt::Display for CommandRisk {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::ReadOnly => "read-only",
            Self::Write => "write",
            Self::Network => "network",
            Self::PackageInstall => "package-install",
            Self::Destructive => "destructive",
            Self::Credential => "credential",
            Self::Privilege => "privilege",
        })
    }
}

pub fn classify_command(command: &str) -> CommandRisk {
    let lowered = command.to_ascii_lowercase();
    let compact = lowered.replace(' ', "");

    if contains_any(
        &lowered,
        &["sudo ", "runas ", "set-executionpolicy", "takeown "],
    ) {
        return CommandRisk::Privilege;
    }
    if contains_any(
        &lowered,
        &[
            " ssh ",
            "ssh-key",
            "id_rsa",
            "id_ed25519",
            "token",
            "secret",
        ],
    ) || contains_any(&compact, &["cat~/.ssh", "type%userprofile%\\.ssh"])
    {
        return CommandRisk::Credential;
    }
    if contains_any(
        &lowered,
        &[
            "rm -rf",
            "git clean",
            "format ",
            "del /s",
            "remove-item",
            "rd /s",
            "rmdir /s",
        ],
    ) {
        return CommandRisk::Destructive;
    }
    if contains_any(
        &lowered,
        &[
            "npm install",
            "pnpm add",
            "yarn add",
            "cargo install",
            "pip install",
            "uv add",
            "winget install",
            "choco install",
        ],
    ) {
        return CommandRisk::PackageInstall;
    }
    if contains_any(
        &lowered,
        &[
            "curl ",
            "wget ",
            "invoke-webrequest",
            "irm ",
            "git clone",
            "git fetch",
            "git pull",
        ],
    ) {
        return CommandRisk::Network;
    }
    if contains_any(
        &lowered,
        &[
            ">",
            "out-file",
            "set-content",
            "add-content",
            "new-item",
            "move-item",
            "copy-item",
            "git commit",
            "git add",
            "cargo fmt",
            "rustfmt",
        ],
    ) {
        return CommandRisk::Write;
    }

    CommandRisk::ReadOnly
}

pub fn ensure_not_protected(path: &Path, workspace: &Path, danger_full_access: bool) -> Result<()> {
    if danger_full_access {
        return Ok(());
    }

    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace.join(path)
    };
    let normalized = normalize_for_policy(&absolute);
    let text = normalized.to_string_lossy().to_ascii_lowercase();

    if protected_component(&normalized) {
        bail!("protected path denied: {}", path.display());
    }
    if text.ends_with(".env") || text.contains("\\.ssh\\") || text.contains("/.ssh/") {
        bail!("credential path denied: {}", path.display());
    }
    if cfg!(windows) {
        let root = std::env::var("SystemRoot")
            .unwrap_or_else(|_| "C:\\Windows".to_string())
            .to_ascii_lowercase();
        if text.starts_with(&root) || text.starts_with("c:\\program files") {
            bail!("system path denied: {}", path.display());
        }
    } else if text.starts_with("/etc/") || text.starts_with("/usr/") || text.starts_with("/var/") {
        bail!("system path denied: {}", path.display());
    }

    Ok(())
}

fn protected_component(path: &Path) -> bool {
    path.components().any(|component| {
        let Component::Normal(value) = component else {
            return false;
        };
        let value = value.to_string_lossy().to_ascii_lowercase();
        matches!(
            value.as_str(),
            ".git"
                | ".hg"
                | ".svn"
                | ".ssh"
                | ".gnupg"
                | "id_rsa"
                | "id_ed25519"
                | "known_hosts"
                | "credentials"
        )
    })
}

fn normalize_for_policy(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn contains_any(value: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| value.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_common_command_risks() {
        assert_eq!(classify_command("cargo test"), CommandRisk::ReadOnly);
        assert_eq!(classify_command("npm install"), CommandRisk::PackageInstall);
        assert_eq!(classify_command("git clean -fdx"), CommandRisk::Destructive);
        assert_eq!(
            classify_command("curl https://example.com"),
            CommandRisk::Network
        );
    }

    #[test]
    fn denies_protected_paths() {
        let workspace = PathBuf::from("C:/work/project");
        assert!(ensure_not_protected(Path::new(".git/config"), &workspace, false).is_err());
        assert!(ensure_not_protected(Path::new("src/main.rs"), &workspace, false).is_ok());
    }
}

use crate::safety::{classify_command, ensure_not_protected, CommandRisk};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PermissionProfile {
    ReadOnly,
    Ask,
    AcceptEdits,
    WorkspaceWrite,
    FullAccess,
    DangerFullAccess,
}

impl PermissionProfile {
    pub fn parse(value: &str) -> Self {
        match value {
            "read-only" => Self::ReadOnly,
            "accept-edits" => Self::AcceptEdits,
            "workspace-write" => Self::WorkspaceWrite,
            "full-access" => Self::FullAccess,
            "danger-full-access" => Self::DangerFullAccess,
            _ => Self::Ask,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PermissionEngine {
    profile: PermissionProfile,
    workspace: PathBuf,
    interactive: bool,
    always_allow: Arc<Mutex<HashSet<String>>>,
}

impl PermissionEngine {
    pub fn new(profile: PermissionProfile, workspace: PathBuf, interactive: bool) -> Self {
        Self {
            profile,
            workspace,
            interactive,
            always_allow: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    pub fn check_read_path(&self, path: &Path) -> Result<()> {
        self.ensure_workspace_path(path)?;
        ensure_not_protected(
            path,
            &self.workspace,
            self.profile == PermissionProfile::DangerFullAccess,
        )
    }

    pub fn check_write_path(&self, path: &Path) -> Result<()> {
        if self.profile != PermissionProfile::DangerFullAccess {
            ensure_not_protected(path, &self.workspace, false)?;
        }
        match self.profile {
            PermissionProfile::ReadOnly => bail!("write denied by read-only permission profile"),
            PermissionProfile::DangerFullAccess => Ok(()),
            PermissionProfile::Ask => {
                self.ensure_workspace_path(path)?;
                self.ask_tool("write_file", &path.display().to_string(), "write_file:*")
            }
            _ => self.ensure_workspace_path(path),
        }
    }

    pub fn check_shell(&self, command: &str) -> Result<()> {
        let risk = classify_command(command);
        if self.profile != PermissionProfile::DangerFullAccess
            && matches!(
                risk,
                CommandRisk::Destructive | CommandRisk::Credential | CommandRisk::Privilege
            )
        {
            bail!("shell denied by command risk policy: {risk}");
        }
        match self.profile {
            PermissionProfile::ReadOnly => bail!("shell denied by read-only permission profile"),
            PermissionProfile::DangerFullAccess | PermissionProfile::FullAccess => Ok(()),
            PermissionProfile::Ask
            | PermissionProfile::AcceptEdits
            | PermissionProfile::WorkspaceWrite => self.ask_tool(
                "shell",
                &format!("[risk:{risk}] {command}"),
                &format!("shell:{command}"),
            ),
        }
    }

    fn ensure_workspace_path(&self, path: &Path) -> Result<()> {
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.workspace.join(path)
        };
        let normalized = normalize_existing_or_parent(&absolute)?;
        let workspace = normalize_existing_or_parent(&self.workspace)?;
        if normalized.starts_with(&workspace) {
            Ok(())
        } else {
            bail!(
                "path {} is outside workspace {}",
                path.display(),
                workspace.display()
            )
        }
    }

    fn ask_tool(&self, tool: &str, target: &str, always_key: &str) -> Result<()> {
        if self
            .always_allow
            .lock()
            .map(|allowed| allowed.contains(always_key))
            .unwrap_or(false)
        {
            return Ok(());
        }
        if !self.interactive {
            bail!("{tool} {target} denied in non-interactive mode");
        }
        println!("{tool}  {target}");
        print!("allow? [y]es / [n]o / [a]lways ");
        io::stdout().flush()?;
        let mut answer = String::new();
        io::stdin().read_line(&mut answer)?;
        match answer.trim().to_ascii_lowercase().as_str() {
            "y" | "yes" => Ok(()),
            "a" | "always" => {
                if let Ok(mut allowed) = self.always_allow.lock() {
                    allowed.insert(always_key.to_string());
                }
                Ok(())
            }
            _ => bail!("operation denied by user"),
        }
    }
}

fn normalize_existing_or_parent(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return Ok(path.canonicalize()?);
    }
    if let Some(parent) = path.parent() {
        if parent.exists() {
            return Ok(parent.canonicalize()?);
        }
    }
    Ok(path.to_path_buf())
}

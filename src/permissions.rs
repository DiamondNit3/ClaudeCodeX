use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

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
}

impl PermissionEngine {
    pub fn new(profile: PermissionProfile, workspace: PathBuf, interactive: bool) -> Self {
        Self {
            profile,
            workspace,
            interactive,
        }
    }

    pub fn check_read_path(&self, path: &Path) -> Result<()> {
        self.ensure_workspace_path(path)
    }

    pub fn check_write_path(&self, path: &Path) -> Result<()> {
        match self.profile {
            PermissionProfile::ReadOnly => bail!("write denied by read-only permission profile"),
            PermissionProfile::DangerFullAccess => Ok(()),
            PermissionProfile::Ask => {
                self.ensure_workspace_path(path)?;
                self.ask(&format!("Allow file write to {}?", path.display()))
            }
            _ => self.ensure_workspace_path(path),
        }
    }

    pub fn check_shell(&self, command: &str) -> Result<()> {
        match self.profile {
            PermissionProfile::ReadOnly => bail!("shell denied by read-only permission profile"),
            PermissionProfile::DangerFullAccess | PermissionProfile::FullAccess => Ok(()),
            PermissionProfile::Ask
            | PermissionProfile::AcceptEdits
            | PermissionProfile::WorkspaceWrite => {
                self.ask(&format!("Allow shell command `{command}`?"))
            }
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
            bail!("path {} is outside workspace {}", path.display(), workspace.display())
        }
    }

    fn ask(&self, question: &str) -> Result<()> {
        if !self.interactive {
            bail!("{question} denied in non-interactive mode");
        }
        print!("{question} [y/N] ");
        io::stdout().flush()?;
        let mut answer = String::new();
        io::stdin().read_line(&mut answer)?;
        if matches!(answer.trim(), "y" | "Y" | "yes" | "YES") {
            Ok(())
        } else {
            bail!("operation denied by user")
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

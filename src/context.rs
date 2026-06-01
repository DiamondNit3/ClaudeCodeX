use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectContext {
    pub workspace: PathBuf,
    pub instruction_files: Vec<InstructionFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstructionFile {
    pub path: PathBuf,
    pub content: String,
}

impl ProjectContext {
    pub fn load(workspace: &Path) -> Result<Self> {
        let candidates = [
            "AGENTS.md",
            ".ccx/AGENTS.md",
            "CLAUDE.md",
            ".cursor/rules",
        ];

        let mut instruction_files = Vec::new();
        for candidate in candidates {
            let path = workspace.join(candidate);
            if path.is_file() {
                instruction_files.push(InstructionFile {
                    path,
                    content: fs::read_to_string(workspace.join(candidate))?,
                });
            }
        }

        Ok(Self {
            workspace: workspace.to_path_buf(),
            instruction_files,
        })
    }

    pub fn render_for_prompt(&self) -> String {
        if self.instruction_files.is_empty() {
            return "No project instruction files were found.".to_string();
        }

        let mut rendered = String::new();
        for file in &self.instruction_files {
            rendered.push_str("\n--- ");
            rendered.push_str(&file.path.display().to_string());
            rendered.push_str(" ---\n");
            rendered.push_str(&file.content);
            if !file.content.ends_with('\n') {
                rendered.push('\n');
            }
        }
        rendered
    }

    pub fn summary(&self) -> String {
        if self.instruction_files.is_empty() {
            return "context: no project instruction files loaded".to_string();
        }
        let files = self
            .instruction_files
            .iter()
            .map(|file| file.path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        format!("context: loaded {}", files)
    }
}

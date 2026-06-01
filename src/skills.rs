use crate::config::AppConfig;
use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    pub path: PathBuf,
    pub preview: String,
}

pub fn list_skills(workspace: &Path) -> Result<Vec<Skill>> {
    let mut skills = Vec::new();
    for dir in skill_dirs(workspace)? {
        if !dir.exists() {
            continue;
        }
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("md") {
                continue;
            }
            let content = fs::read_to_string(&path).unwrap_or_default();
            skills.push(Skill {
                name: path
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .unwrap_or("skill")
                    .to_string(),
                path,
                preview: content.lines().take(3).collect::<Vec<_>>().join(" "),
            });
        }
    }
    skills.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(skills)
}

pub fn print_skills(workspace: &Path) -> Result<()> {
    let skills = list_skills(workspace)?;
    if skills.is_empty() {
        println!("No skills found.");
        return Ok(());
    }
    for skill in skills {
        println!(
            "{}  {}\n  {}",
            skill.name,
            skill.path.display(),
            skill.preview
        );
    }
    Ok(())
}

fn skill_dirs(workspace: &Path) -> Result<Vec<PathBuf>> {
    Ok(vec![
        workspace.join(".ccx").join("skills"),
        AppConfig::data_dir()?.join("skills"),
    ])
}

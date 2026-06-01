use crate::config::AppConfig;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
struct TaskMetadata {
    id: Uuid,
    task: String,
    status: String,
    log: PathBuf,
    created_ms: u128,
}

pub fn spawn_task(workspace: &Path, task: &str) -> Result<()> {
    let id = Uuid::new_v4();
    let dir = tasks_dir()?;
    fs::create_dir_all(&dir)?;
    let log = dir.join(format!("{id}.log"));
    let metadata = TaskMetadata {
        id,
        task: task.to_string(),
        status: "running".to_string(),
        log: log.clone(),
        created_ms: now_ms()?,
    };
    fs::write(
        dir.join(format!("{id}.json")),
        serde_json::to_string_pretty(&metadata)?,
    )?;

    let exe = std::env::current_exe()?;
    let log_file = OpenOptions::new().create(true).append(true).open(&log)?;
    let err_file = log_file.try_clone()?;
    Command::new(exe)
        .arg("task")
        .arg("worker")
        .arg(id.to_string())
        .arg(task)
        .current_dir(workspace)
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(err_file))
        .spawn()
        .context("failed to spawn background task")?;

    println!("{id}  running  {}", log.display());
    Ok(())
}

pub fn list_tasks() -> Result<()> {
    let dir = tasks_dir()?;
    if !dir.exists() {
        println!("No tasks found.");
        return Ok(());
    }
    for entry in fs::read_dir(&dir)? {
        let path = entry?.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let metadata: TaskMetadata = serde_json::from_str(&fs::read_to_string(&path)?)?;
        let status = if dir.join(format!("{}.cancel", metadata.id)).exists() {
            "cancel-requested"
        } else {
            &metadata.status
        };
        println!("{}  {}  {}", metadata.id, status, metadata.task);
    }
    Ok(())
}

pub fn show_task(id_prefix: &str) -> Result<()> {
    let metadata = find_task(id_prefix)?;
    println!("{}  {}  {}", metadata.id, metadata.status, metadata.task);
    if metadata.log.exists() {
        println!("{}", fs::read_to_string(&metadata.log)?);
    }
    Ok(())
}

pub fn tail_task(id_prefix: &str, lines: usize) -> Result<()> {
    let metadata = find_task(id_prefix)?;
    println!("{}  {}  {}", metadata.id, metadata.status, metadata.task);
    if metadata.log.exists() {
        let content = fs::read_to_string(&metadata.log)?;
        let tail = content
            .lines()
            .rev()
            .take(lines)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        println!("{tail}");
    }
    Ok(())
}

pub fn run_worker(id: &str, task: &str) -> Result<()> {
    update_status(id, "running")?;
    let metadata = find_task(id)?;
    let exe = std::env::current_exe()?;
    let output = Command::new(exe).arg("exec").arg(task).output()?;
    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&metadata.log)?;
    use std::io::Write;
    log.write_all(&output.stdout)?;
    log.write_all(&output.stderr)?;
    if output.status.success() {
        update_status(id, "complete")?;
    } else {
        update_status(id, "failed")?;
    }
    Ok(())
}

pub fn cancel_task(id_prefix: &str) -> Result<()> {
    let metadata = find_task(id_prefix)?;
    fs::write(
        tasks_dir()?.join(format!("{}.cancel", metadata.id)),
        "cancel requested",
    )?;
    println!("cancel requested for {}", metadata.id);
    Ok(())
}

fn find_task(id_prefix: &str) -> Result<TaskMetadata> {
    let dir = tasks_dir()?;
    for entry in fs::read_dir(&dir)? {
        let path = entry?.path();
        let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        if stem.starts_with(id_prefix)
            && path.extension().and_then(|value| value.to_str()) == Some("json")
        {
            return Ok(serde_json::from_str(&fs::read_to_string(path)?)?);
        }
    }
    anyhow::bail!("no task found for `{id_prefix}`")
}

fn update_status(id_prefix: &str, status: &str) -> Result<()> {
    let dir = tasks_dir()?;
    let mut metadata = find_task(id_prefix)?;
    metadata.status = status.to_string();
    fs::write(
        dir.join(format!("{}.json", metadata.id)),
        serde_json::to_string_pretty(&metadata)?,
    )?;
    Ok(())
}

fn tasks_dir() -> Result<PathBuf> {
    Ok(AppConfig::data_dir()?.join("tasks"))
}

fn now_ms() -> Result<u128> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis())
}

use crate::config::AppConfig;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

#[derive(Debug)]
pub struct Session {
    pub id: Uuid,
    path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEvent {
    pub timestamp_ms: u128,
    pub kind: String,
    pub payload: Value,
}

impl Session {
    pub fn create() -> Result<Self> {
        let id = Uuid::new_v4();
        let dir = sessions_dir()?;
        fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{id}.jsonl"));
        Ok(Self { id, path })
    }

    pub fn open(id_or_prefix: &str) -> Result<Self> {
        let path = find_session_path(id_or_prefix)?;
        let id_text = path
            .file_stem()
            .and_then(|value| value.to_str())
            .context("session file name is not valid UTF-8")?;
        let id = Uuid::parse_str(id_text)?;
        Ok(Self { id, path })
    }

    pub fn append<T: Serialize>(&self, kind: &str, payload: T) -> Result<()> {
        let event = SessionEvent {
            timestamp_ms: SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis(),
            kind: kind.to_string(),
            payload: serde_json::to_value(payload)?,
        };
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .with_context(|| format!("failed to open session {}", self.path.display()))?;
        writeln!(file, "{}", serde_json::to_string(&event)?)?;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn append_memory(&self, kind: &str, text: &str) -> Result<()> {
        let dir = AppConfig::data_dir()?.join("memory");
        fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{}.jsonl", self.id));
        let payload = serde_json::json!({
            "timestamp_ms": SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis(),
            "kind": kind,
            "text": text
        });
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        writeln!(file, "{}", serde_json::to_string(&payload)?)?;
        Ok(())
    }
}

pub fn read_events(session: &Session) -> Result<Vec<SessionEvent>> {
    let file = fs::File::open(session.path())?;
    let mut events = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line?;
        if !line.trim().is_empty() {
            events.push(serde_json::from_str(&line)?);
        }
    }
    Ok(events)
}

pub fn print_sessions(filter: Option<&str>) -> Result<()> {
    let dir = sessions_dir()?;
    if !dir.exists() {
        println!("No sessions found.");
        return Ok(());
    }

    let mut entries = fs::read_dir(&dir)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().and_then(|s| s.to_str()) == Some("jsonl"))
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.metadata().and_then(|m| m.modified()).ok());
    entries.reverse();

    for entry in entries {
        let path = entry.path();
        let id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        if let Some(filter) = filter {
            if !id.contains(filter) {
                continue;
            }
        }
        let first_user = first_user_message(&path).unwrap_or_default();
        println!("{id}  {}", first_user.replace('\n', " "));
    }
    Ok(())
}

fn first_user_message(path: &Path) -> Result<String> {
    let file = fs::File::open(path)?;
    for line in BufReader::new(file).lines() {
        let line = line?;
        let event: SessionEvent = serde_json::from_str(&line)?;
        if event.kind == "user" {
            if let Some(text) = event.payload.get("text").and_then(|value| value.as_str()) {
                return Ok(text.chars().take(96).collect());
            }
        }
    }
    Ok(String::new())
}

fn find_session_path(id_or_prefix: &str) -> Result<PathBuf> {
    let dir = sessions_dir()?;
    let direct = dir.join(format!("{id_or_prefix}.jsonl"));
    if direct.exists() {
        return Ok(direct);
    }

    let mut matches = fs::read_dir(&dir)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_stem()
                .and_then(|value| value.to_str())
                .map(|id| id.starts_with(id_or_prefix))
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();
    matches.sort();

    match matches.len() {
        1 => Ok(matches.remove(0)),
        0 => anyhow::bail!("no session found for `{id_or_prefix}`"),
        _ => anyhow::bail!("session prefix `{id_or_prefix}` is ambiguous"),
    }
}

fn sessions_dir() -> Result<PathBuf> {
    Ok(AppConfig::data_dir()?.join("sessions"))
}

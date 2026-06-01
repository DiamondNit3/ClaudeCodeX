use anyhow::Result;
use serde::Serialize;
use std::path::Path;
use std::time::Instant;

#[derive(Debug, Serialize)]
struct BenchResult {
    name: &'static str,
    ok: bool,
    elapsed_ms: u128,
}

pub fn run_bench(workspace: &Path) -> Result<()> {
    let mut results = Vec::new();
    results.push(measure("workspace-present", || Ok(workspace.exists()))?);
    results.push(measure("git-present", || {
        Ok(std::process::Command::new("git")
            .arg("--version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false))
    })?);
    results.push(measure("cargo-manifest", || {
        Ok(workspace.join("Cargo.toml").exists())
    })?);
    println!("{}", serde_json::to_string_pretty(&results)?);
    Ok(())
}

fn measure(name: &'static str, check: impl FnOnce() -> Result<bool>) -> Result<BenchResult> {
    let start = Instant::now();
    let ok = check()?;
    Ok(BenchResult {
        name,
        ok,
        elapsed_ms: start.elapsed().as_millis(),
    })
}

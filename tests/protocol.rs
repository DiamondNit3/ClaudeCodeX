use std::process::Command;

#[test]
fn manifest_names_ccx_binary() {
    let manifest = std::fs::read_to_string("Cargo.toml").unwrap();
    assert!(manifest.contains("name = \"ccx\""));
    assert!(manifest.contains("path = \"src/main.rs\""));
}

#[test]
fn docs_include_github_backup_target() {
    let roadmap = std::fs::read_to_string("docs/ROADMAP.md").unwrap();
    assert!(roadmap.contains("DiamondNit3/ClaudeCodeX"));
}

#[test]
fn git_is_available_for_harness_tools() {
    let output = Command::new("git").arg("--version").output().unwrap();
    assert!(output.status.success());
}

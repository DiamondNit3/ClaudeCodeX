use anyhow::{bail, Result};

pub fn apply_edit(
    original: &str,
    old: Option<&str>,
    new: Option<&str>,
    patch: Option<&str>,
) -> Result<String> {
    if let Some(patch) = patch {
        return apply_unified_patch(original, patch);
    }

    let old = old.ok_or_else(|| anyhow::anyhow!("missing `old` text or `patch`"))?;
    let new = new.ok_or_else(|| anyhow::anyhow!("missing `new` text"))?;
    if !original.contains(old) {
        bail!("old text was not found");
    }
    Ok(original.replacen(old, new, 1))
}

pub fn apply_unified_patch(original: &str, patch: &str) -> Result<String> {
    let mut output = original.to_string();
    let hunks = parse_hunks(patch)?;
    if hunks.is_empty() {
        bail!("patch did not contain any hunks");
    }

    for hunk in hunks {
        let old_block = hunk
            .lines
            .iter()
            .filter(|line| matches!(line.kind, HunkLineKind::Context | HunkLineKind::Remove))
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let new_block = hunk
            .lines
            .iter()
            .filter(|line| matches!(line.kind, HunkLineKind::Context | HunkLineKind::Add))
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        if !output.contains(&old_block) {
            bail!("patch hunk context was not found");
        }
        output = output.replacen(&old_block, &new_block, 1);
    }

    Ok(output)
}

pub fn diff_summary(original: &str, updated: &str) -> String {
    let original_lines = original.lines().count();
    let updated_lines = updated.lines().count();
    let changed = original
        .lines()
        .zip(updated.lines())
        .filter(|(left, right)| left != right)
        .count()
        + original_lines.abs_diff(updated_lines);
    format!(
        "changed {changed} line{}\n{} -> {} lines",
        if changed == 1 { "" } else { "s" },
        original_lines,
        updated_lines
    )
}

#[derive(Debug)]
struct Hunk {
    lines: Vec<HunkLine>,
}

#[derive(Debug)]
struct HunkLine {
    kind: HunkLineKind,
    text: String,
}

#[derive(Debug, PartialEq, Eq)]
enum HunkLineKind {
    Context,
    Add,
    Remove,
}

fn parse_hunks(patch: &str) -> Result<Vec<Hunk>> {
    let mut hunks = Vec::new();
    let mut current: Option<Hunk> = None;

    for raw in patch.lines() {
        if raw.starts_with("@@") {
            if let Some(hunk) = current.take() {
                hunks.push(hunk);
            }
            current = Some(Hunk { lines: Vec::new() });
            continue;
        }
        if raw.starts_with("---") || raw.starts_with("+++") || raw.starts_with("diff ") {
            continue;
        }
        let Some(hunk) = current.as_mut() else {
            continue;
        };
        let Some(marker) = raw.chars().next() else {
            hunk.lines.push(HunkLine {
                kind: HunkLineKind::Context,
                text: String::new(),
            });
            continue;
        };
        let text = raw.get(1..).unwrap_or_default().to_string();
        match marker {
            ' ' => hunk.lines.push(HunkLine {
                kind: HunkLineKind::Context,
                text,
            }),
            '-' => hunk.lines.push(HunkLine {
                kind: HunkLineKind::Remove,
                text,
            }),
            '+' => hunk.lines.push(HunkLine {
                kind: HunkLineKind::Add,
                text,
            }),
            _ => {}
        }
    }

    if let Some(hunk) = current {
        hunks.push(hunk);
    }

    Ok(hunks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_exact_replace() {
        let updated = apply_edit("one\ntwo\n", Some("two"), Some("three"), None).unwrap();
        assert_eq!(updated, "one\nthree\n");
    }

    #[test]
    fn applies_simple_unified_patch() {
        let patch = "@@\n one\n-two\n+three";
        let updated = apply_unified_patch("one\ntwo", patch).unwrap();
        assert_eq!(updated, "one\nthree");
    }
}

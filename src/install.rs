//! Installing the Claude Code hooks and the git pre-commit hook.

use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::path::Path;

/// `.claude/settings.json` is shared with the team; teammates without `decisions` get a no-op.
fn guarded(event: &str) -> String {
    format!("command -v decisions >/dev/null 2>&1 && decisions hook {event} || true")
}

const PRE_COMMIT_MARKER: &str = "# decisions";

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Installed,
    AlreadyPresent,
    Skipped(String),
}

pub fn claude(root: &Path) -> Result<Outcome> {
    let path = root.join(".claude").join("settings.json");
    let mut settings: Value = if path.exists() {
        serde_json::from_str(&std::fs::read_to_string(&path)?)
            .with_context(|| format!("{} is not valid JSON", path.display()))?
    } else {
        json!({})
    };
    let Some(obj) = settings.as_object_mut() else {
        return Ok(Outcome::Skipped(format!(
            "{} is not a JSON object",
            path.display()
        )));
    };
    let hooks = obj.entry("hooks").or_insert_with(|| json!({}));
    let mut changed = false;
    changed |= add(hooks, "SessionStart", None, &guarded("session-start"))?;
    changed |= add(
        hooks,
        "PreToolUse",
        Some("Edit|Write|MultiEdit"),
        &guarded("pre-tool-use"),
    )?;
    changed |= add(hooks, "Stop", None, &guarded("stop"))?;
    if !changed {
        return Ok(Outcome::AlreadyPresent);
    }
    std::fs::create_dir_all(path.parent().expect("has parent"))?;
    std::fs::write(&path, serde_json::to_string_pretty(&settings)? + "\n")?;
    Ok(Outcome::Installed)
}

fn add(hooks: &mut Value, event: &str, matcher: Option<&str>, command: &str) -> Result<bool> {
    let hooks = hooks
        .as_object_mut()
        .context("`hooks` in settings.json is not an object")?;
    let list = hooks
        .entry(event)
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .with_context(|| format!("`hooks.{event}` is not an array"))?;
    let present = list.iter().any(|g| {
        g["hooks"]
            .as_array()
            .is_some_and(|hs| hs.iter().any(|h| h["command"] == command))
    });
    if present {
        return Ok(false);
    }
    let mut group = json!({ "hooks": [{ "type": "command", "command": command }] });
    if let Some(m) = matcher {
        group["matcher"] = json!(m);
    }
    list.push(group);
    Ok(true)
}

pub fn pre_commit(git_dir: &Path) -> Result<Outcome> {
    let path = git_dir.join("hooks").join("pre-commit");
    if path.exists() {
        if std::fs::read_to_string(&path)
            .unwrap_or_default()
            .contains(PRE_COMMIT_MARKER)
        {
            return Ok(Outcome::AlreadyPresent);
        }
        return Ok(Outcome::Skipped(format!(
            "{} exists; add `decisions check --staged` to it",
            path.display()
        )));
    }
    std::fs::create_dir_all(path.parent().expect("has parent"))?;
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\n{PRE_COMMIT_MARKER}: refuse commits that break accepted decisions\ncommand -v decisions >/dev/null 2>&1 || exit 0\nexec decisions check --staged\n"
        ),
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(Outcome::Installed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".claude")).unwrap();
        std::fs::write(
            dir.path().join(".claude/settings.json"),
            r#"{"model":"x","hooks":{"Stop":[{"hooks":[{"type":"command","command":"other"}]}]}}"#,
        )
        .unwrap();
        assert_eq!(claude(dir.path()).unwrap(), Outcome::Installed);
        assert_eq!(claude(dir.path()).unwrap(), Outcome::AlreadyPresent);
        let v: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.path().join(".claude/settings.json")).unwrap())
                .unwrap();
        assert_eq!(v["model"], "x");
        assert_eq!(v["hooks"]["Stop"].as_array().unwrap().len(), 2);
        assert_eq!(v["hooks"]["PreToolUse"][0]["matcher"], "Edit|Write|MultiEdit");
        assert!(pre_commit(dir.path()).is_ok());
        assert_eq!(pre_commit(dir.path()).unwrap(), Outcome::AlreadyPresent);
    }
}

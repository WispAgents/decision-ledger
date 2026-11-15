//! Claude Code hooks. Every hook fails open: if anything goes wrong, the session continues.

use super::Outcome;
use crate::check::{Checker, Target, Violation};
use crate::diff::LineRange;
use crate::git::DiffBase;
use crate::message;
use crate::record::Record;
use crate::repo::Repo;
use crate::scope::{self, Scope};
use crate::session::Session;
use anyhow::Result;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::process::ExitCode;

/// Decisions shown per file and at session start; the rest are listed by count.
const SHOW_MAX: usize = 8;
/// Times the Stop hook sends a turn back before letting it end.
const STOP_MAX_ATTEMPTS: u32 = 3;

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum Event {
    SessionStart,
    PreToolUse,
    Stop,
}

pub fn run(dir: &Path, event: Event) -> Outcome {
    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);
    let payload: Value = serde_json::from_str(&input).unwrap_or(json!({}));
    match handle(dir, event, &payload) {
        Ok(Some(answer)) => println!("{answer}"),
        Ok(None) => {}
        Err(e) => eprintln!("decisions hook error (ignored): {e:#}"),
    }
    Ok(ExitCode::SUCCESS)
}

/// The JSON to print, if any.
pub fn handle(dir: &Path, event: Event, payload: &Value) -> Result<Option<Value>> {
    let dir = payload["cwd"].as_str().map(Path::new).unwrap_or(dir);
    let repo = Repo::open(dir)?;
    let session_id = payload["session_id"].as_str().unwrap_or("default");
    let state = repo.state_dir();
    let mut session = Session::load(&state, session_id);

    // The ledger itself is reviewed by humans; agents propose through `decisions add --propose`.
    if let Event::PreToolUse = event
        && let Some(rel) = edited_path(&repo, payload)
        && is_ledger_record(&rel)
    {
        return Ok(Some(deny(format!(
            "`{rel}` is an accepted decision record; agents don't edit it. To change a decision, run \
             `decisions supersede <id> --decision \"...\" --reason \"...\"`; to record a new one, \
             `decisions add --propose \"...\"`. A human reviews both."
        ))));
    }

    let records = repo.ledger.live_lenient();
    if records.is_empty() {
        return Ok(None);
    }
    let answer = match event {
        Event::SessionStart => session_start(&records, &mut session, payload),
        Event::PreToolUse => pre_tool_use(&repo, &records, &mut session, payload),
        Event::Stop => stop(&repo, &records, &mut session)?,
    };
    session.save(&state, session_id)?;
    Ok(answer)
}

fn context(event: &str, text: String) -> Value {
    json!({ "hookSpecificOutput": { "hookEventName": event, "additionalContext": text } })
}

fn deny(reason: String) -> Value {
    json!({ "hookSpecificOutput": { "hookEventName": "PreToolUse", "permissionDecision": "deny", "permissionDecisionReason": reason } })
}

/// The repo-relative path an edit targets; `None` for files outside the repository.
fn edited_path(repo: &Repo, payload: &Value) -> Option<String> {
    let file = payload["tool_input"]["file_path"].as_str()?;
    repo.git.relative_in_repo(Path::new(file))
}

/// `decisions/*.yml`, but not proposals or the gitignore.
fn is_ledger_record(rel: &str) -> bool {
    rel.strip_prefix("decisions/")
        .is_some_and(|rest| !rest.contains('/') && (rest.ends_with(".yml") || rest.ends_with(".yaml")))
}

fn session_start(records: &[Record], session: &mut Session, payload: &Value) -> Option<Value> {
    // After /compact or /clear the model no longer has what it was shown.
    if matches!(payload["source"].as_str(), Some("compact" | "clear")) {
        session.shown.clear();
    }
    let wide: Vec<&Record> = records.iter().filter(|r| Scope::of(r).is_repo_wide()).collect();
    let scoped = records.len() - wide.len();
    let shown: Vec<&Record> = wide.iter().take(SHOW_MAX).copied().collect();
    session.shown.extend(shown.iter().map(|r| r.id.clone()));
    Some(context(
        "SessionStart",
        message::session(&shown, scoped, wide.len() - shown.len()),
    ))
}

fn pre_tool_use(repo: &Repo, records: &[Record], session: &mut Session, payload: &Value) -> Option<Value> {
    let rel = edited_path(repo, payload)?;
    session.edited.insert(rel.clone());
    let current = repo.read(&rel);
    let after = apply_edit(
        payload["tool_name"].as_str().unwrap_or(""),
        &payload["tool_input"],
        current.as_deref(),
    );

    // Check what the file would look like after this edit, before it happens.
    if let Some(after) = &after {
        let added = added_lines(current.as_deref().unwrap_or(""), after);
        let checker = Checker::new(records);
        if !checker.is_empty() && !added.is_empty() {
            let target = Target {
                path: rel.clone(),
                content: after.clone(),
                added: Some(added),
                before: current.clone(),
            };
            let found = checker.check(&target);
            let active: Vec<&Violation> = found.iter().filter(|v| v.overridden.is_none()).collect();
            if !active.is_empty() {
                return Some(deny(message::violations(&active, records, true)));
            }
        }
    }

    // Otherwise, show the decisions for this file that the agent hasn't seen yet.
    let content = after.as_deref().or(current.as_deref());
    let new: Vec<&Record> = scope::applicable(records, &rel, content)
        .into_iter()
        .map(|(r, _)| r)
        .filter(|r| !session.shown.contains(&r.id))
        .collect();
    if new.is_empty() {
        return None;
    }
    let shown: Vec<&Record> = new.iter().take(SHOW_MAX).copied().collect();
    session.shown.extend(shown.iter().map(|r| r.id.clone()));
    Some(context(
        "PreToolUse",
        message::scoped(&rel, &shown, new.len() - shown.len()),
    ))
}

fn stop(repo: &Repo, records: &[Record], session: &mut Session) -> Result<Option<Value>> {
    let edited: Vec<String> = session.edited.iter().cloned().collect();
    if edited.is_empty() {
        return Ok(None);
    }
    let targets = repo.targets(&DiffBase::Head, Some(&edited))?;
    let checker = Checker::new(records);
    let found: Vec<Violation> = targets.iter().flat_map(|t| checker.check(t)).collect();
    if let Err(e) = super::check::log_overrides(repo, &found) {
        eprintln!("decisions: couldn't log overrides: {e:#}");
    }
    let active: Vec<&Violation> = found.iter().filter(|v| v.overridden.is_none()).collect();
    if active.is_empty() || session.stop_attempts >= STOP_MAX_ATTEMPTS {
        session.stop_attempts = 0;
        if !active.is_empty() {
            eprintln!("decisions: turn ended with {} broken decisions", active.len());
        }
        return Ok(None);
    }
    session.stop_attempts += 1;
    Ok(Some(
        json!({ "decision": "block", "reason": message::violations(&active, records, false) }),
    ))
}

/// The file after an Edit, MultiEdit or Write, or `None` if the edit wouldn't apply.
fn apply_edit(tool: &str, input: &Value, current: Option<&str>) -> Option<String> {
    let replace = |text: &str, e: &Value| -> Option<String> {
        let old = e["old_string"].as_str()?;
        let new = e["new_string"].as_str()?;
        if old.is_empty() {
            return Some(format!("{new}{text}"));
        }
        if !text.contains(old) {
            return None;
        }
        Some(if e["replace_all"].as_bool().unwrap_or(false) {
            text.replace(old, new)
        } else {
            text.replacen(old, new, 1)
        })
    };
    match tool {
        "Write" => input["content"].as_str().map(str::to_owned),
        "Edit" => replace(current.unwrap_or(""), input),
        "MultiEdit" => input["edits"]
            .as_array()?
            .iter()
            .try_fold(current.unwrap_or("").to_owned(), |text, e| replace(&text, e)),
        _ => None,
    }
}

/// Lines of `after` that aren't in `before` (as a multiset), as ranges.
fn added_lines(before: &str, after: &str) -> Vec<LineRange> {
    let mut have: HashMap<&str, usize> = HashMap::new();
    // Trimmed, so re-indenting existing code doesn't count as adding it.
    for l in before.lines() {
        *have.entry(l.trim()).or_default() += 1;
    }
    let mut out: Vec<LineRange> = Vec::new();
    for (i, l) in after.lines().enumerate() {
        let n = i as u32 + 1;
        let left = have.entry(l.trim()).or_default();
        if *left > 0 {
            *left -= 1;
            continue;
        }
        match out.last_mut() {
            Some(r) if r.end + 1 == n => r.end = n,
            _ => out.push(LineRange::new(n, n)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simulates_edits() {
        let cur = "a\nb\nc\n";
        let edit = json!({"old_string": "b", "new_string": "B\nB2"});
        assert_eq!(apply_edit("Edit", &edit, Some(cur)).unwrap(), "a\nB\nB2\nc\n");
        assert!(apply_edit("Edit", &json!({"old_string": "zz", "new_string": "y"}), Some(cur)).is_none());
        let multi = json!({"edits": [{"old_string": "a", "new_string": "A"}, {"old_string": "c", "new_string": "C"}]});
        assert_eq!(apply_edit("MultiEdit", &multi, Some(cur)).unwrap(), "A\nb\nC\n");
        assert_eq!(apply_edit("Write", &json!({"content": "x"}), None).unwrap(), "x");
    }

    #[test]
    fn added_line_ranges() {
        assert_eq!(
            added_lines("a\nb\nc", "a\nX\nY\nb\nc\nZ"),
            vec![LineRange::new(2, 3), LineRange::new(6, 6)]
        );
        assert!(added_lines("a\nb", "b\na").is_empty());
        assert!(
            added_lines("import x", "if (y) {\n  import x\n}")
                .iter()
                .all(|r| r.start != 2 || r.end != 2)
        );
    }
}

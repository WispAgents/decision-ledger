//! Per-session hook state in `.git/decisions/sessions/<id>.json`: which decisions the agent has
//! already been shown (so each is shown once), which files it edited (the Stop hook checks only
//! those, not the human's own uncommitted work), and how often the turn was sent back.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Session {
    #[serde(default)]
    pub shown: BTreeSet<String>,
    #[serde(default)]
    pub edited: BTreeSet<String>,
    #[serde(default)]
    pub stop_attempts: u32,
}

impl Session {
    pub fn load(state: &Path, id: &str) -> Self {
        std::fs::read_to_string(file(state, id))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, state: &Path, id: &str) -> Result<()> {
        let path = file(state, id);
        std::fs::create_dir_all(path.parent().expect("has parent"))?;
        // Write then rename, so concurrent hooks never read half a file.
        let tmp = path.with_extension(format!("json.{}", std::process::id()));
        std::fs::write(&tmp, serde_json::to_string(self)?)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }
}

/// Session ids come from hook input; keep only safe filename characters.
fn file(state: &Path, id: &str) -> PathBuf {
    let safe: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(128)
        .collect();
    let safe = if safe.is_empty() {
        "default".to_owned()
    } else {
        safe
    };
    state.join("sessions").join(format!("{safe}.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_with_unsafe_id() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Session::default();
        s.shown.insert("D-0001".into());
        s.save(dir.path(), "../../x").unwrap();
        assert!(dir.path().join("sessions/______x.json").exists());
        assert!(Session::load(dir.path(), "../../x").shown.contains("D-0001"));
        assert!(Session::load(dir.path(), "other").shown.is_empty());
    }
}

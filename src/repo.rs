//! The repository a command works on.

use crate::git::Git;
use crate::record::{Ledger, Record};
use anyhow::Result;
use std::path::{Path, PathBuf};

pub struct Repo {
    pub git: Git,
    pub ledger: Ledger,
}

impl Repo {
    pub fn open(dir: &Path) -> Result<Self> {
        let git = Git::open(dir)?;
        let ledger = Ledger::new(git.root());
        Ok(Self { git, ledger })
    }

    /// Local, uncommitted state: `.git/decisions/`.
    pub fn state_dir(&self) -> PathBuf {
        self.git.git_dir().join("decisions")
    }

    pub fn live(&self) -> Result<Vec<Record>> {
        self.ledger.live()
    }

    /// Current content of a tracked or untracked file in the working tree.
    pub fn read(&self, rel: &str) -> Option<String> {
        std::fs::read_to_string(self.git.root().join(rel)).ok()
    }
}

use crate::check::Target;
use crate::git::{Content, DiffBase};

impl Repo {
    /// Files with added lines relative to `base` (optionally only `only`), as check targets.
    pub fn targets(&self, base: &DiffBase, only: Option<&[String]>) -> Result<Vec<Target>> {
        let files: Vec<(String, Vec<crate::diff::LineRange>)> = match only {
            // One file failing (deleted since, outside the repo) doesn't stop the others.
            Some(paths) => paths
                .iter()
                .filter_map(|p| match self.git.added_lines_for(base, p) {
                    Ok(added) => Some((p.clone(), added)),
                    Err(e) => {
                        eprintln!("decisions: skipping {p}: {e:#}");
                        None
                    }
                })
                .collect(),
            None => self.git.added_lines(base)?,
        };
        let side = base.content();
        let before_rev = base.before_rev(&self.git);
        let mut out = Vec::new();
        for (path, added) in files {
            if added.is_empty() || path.starts_with(&format!("{}/", Ledger::DIR)) {
                continue;
            }
            let content = match &side {
                Content::WorkingTree => self.read(&path),
                Content::Index => self.git.show_index(&path),
                Content::Revision(rev) => self.git.show(rev, &path),
            };
            let Some(content) = content else { continue };
            let before = if crate::manifest::is_manifest(&path) {
                before_rev.as_deref().and_then(|rev| self.git.show(rev, &path))
            } else {
                None
            };
            out.push(Target {
                path,
                content,
                added: Some(added),
                before,
            });
        }
        Ok(out)
    }

    /// Ledger files a diff changes (CI lists them so code owners can gate them).
    pub fn ledger_changes(&self, base: &DiffBase) -> Vec<String> {
        self.git
            .diff(base, Some(Ledger::DIR))
            .map(|files| {
                files
                    .into_iter()
                    .filter_map(|f| f.new_path.or(f.old_path))
                    .filter(|p| !p.contains("/pending/"))
                    .collect()
            })
            .unwrap_or_default()
    }
}

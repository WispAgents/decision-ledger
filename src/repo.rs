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
        let files = match only {
            Some(paths) => paths
                .iter()
                .map(|p| Ok((p.clone(), self.git.added_lines_for(base, p)?)))
                .collect::<Result<Vec<_>>>()?,
            None => self.git.added_lines(base)?,
        };
        let side = base.content();
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
            if let Some(content) = content {
                out.push(Target {
                    path,
                    content,
                    added: Some(added),
                });
            }
        }
        Ok(out)
    }
}

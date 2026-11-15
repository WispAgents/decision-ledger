use super::Outcome;
use crate::extract::{self, llm, source};
use crate::record::parse_since;
use crate::repo::Repo;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(clap::Args)]
pub struct Args {
    /// Only texts after this date (YYYY-MM-DD). Default: where the last run stopped, or 90 days ago.
    #[arg(long)]
    since: Option<String>,
    /// GitHub repo as `owner/name` (default: detected from the remote; skipped if there is none).
    #[arg(long)]
    github: Option<String>,
    #[arg(long)]
    no_github: bool,
    #[arg(long)]
    no_git: bool,
    #[arg(long)]
    no_sessions: bool,
    /// Claude Code projects directory (default ~/.claude/projects).
    #[arg(long)]
    sessions_dir: Option<PathBuf>,
    /// Pages of 100 GitHub items per stream.
    #[arg(long, default_value_t = 10)]
    pages: u32,
    /// Read at most this many texts (newest first).
    #[arg(long, default_value_t = 400)]
    max_texts: usize,
    /// auto, claude, api or none.
    #[arg(long, default_value = "auto")]
    llm: String,
    #[arg(long)]
    model: Option<String>,
    /// Count the texts and stop before calling the model.
    #[arg(long)]
    dry_run: bool,
}

/// Where each source stopped last time: `.git/decisions/cursor.json`.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Cursor {
    commits: i64,
    github: i64,
    sessions: i64,
}

pub fn run(dir: &Path, args: Args) -> Outcome {
    let repo = Repo::open(dir)?;
    let cursor_path = repo.state_dir().join("cursor.json");
    let cursor: Cursor = std::fs::read_to_string(&cursor_path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let floor = match &args.since {
        Some(d) => Some(parse_since(d)?),
        None => None,
    };
    let default_start = crate::record::now() - 90 * 86_400;
    let start = |last: i64| floor.unwrap_or(if last > 0 { last } else { default_start });

    let mut texts = Vec::new();
    if !args.no_git {
        let found = source::commits(&repo.git, start(cursor.commits), 2000)?;
        println!("commits: {}", found.len());
        texts.extend(found);
    }
    if !args.no_github {
        let name = match &args.github {
            Some(n) => Some(n.clone()),
            None => source::github_name(repo.git.root()).ok(),
        };
        match name {
            Some(name) => {
                let found = source::github(&name, start(cursor.github), args.pages)?;
                println!("github {name}: {} PRs, reviews, issues and comments", found.len());
                texts.extend(found);
            }
            None => println!("github: no GitHub remote found, skipped (pass --github owner/name)"),
        }
    }
    if !args.no_sessions {
        let dir = args.sessions_dir.clone().or_else(default_sessions_dir);
        if let Some(dir) = dir {
            let root =
                std::fs::canonicalize(repo.git.root()).unwrap_or_else(|_| repo.git.root().to_path_buf());
            let found = source::sessions(&dir, &root, start(cursor.sessions));
            println!("claude code sessions: {} messages you typed", found.len());
            texts.extend(found);
        }
    }

    let newest = |kind: &dyn Fn(&str) -> bool| texts.iter().filter(|t| kind(&t.kind)).map(|t| t.time).max();
    let next = Cursor {
        commits: newest(&|k| k == "commit")
            .unwrap_or(cursor.commits)
            .max(cursor.commits),
        github: newest(&|k| ["pr", "review", "issue", "issue_comment"].contains(&k))
            .unwrap_or(cursor.github)
            .max(cursor.github),
        sessions: newest(&|k| k == "session")
            .unwrap_or(cursor.sessions)
            .max(cursor.sessions),
    };

    texts.retain(source::worth_reading);
    texts.sort_by_key(|t| std::cmp::Reverse(t.time));
    texts.truncate(args.max_texts);
    println!("{} texts worth reading", texts.len());
    if args.dry_run || texts.is_empty() {
        return Ok(ExitCode::SUCCESS);
    }
    let Some(model) = llm::backend(&args.llm, args.model.as_deref())? else {
        println!("no model backend (install Claude Code or set ANTHROPIC_API_KEY); nothing extracted");
        return Ok(ExitCode::SUCCESS);
    };
    eprintln!("extracting with {} …", model.describe());
    let mut known = repo.ledger.records()?;
    known.extend(repo.ledger.pending()?);
    let out = extract::extract(model.as_ref(), &texts, &known, &repo.ledger.rejections()?, 8, 4);
    for p in &out.proposals {
        p.save(&repo.ledger.pending_path(&p.id)?)?;
    }
    println!(
        "{} proposals from {} texts in {} calls ({} failed, {} duplicates skipped)",
        out.proposals.len(),
        out.texts,
        out.calls,
        out.failed_calls,
        out.duplicates
    );
    for p in &out.proposals {
        let sup = p
            .supersedes
            .as_deref()
            .map(|s| format!(" (replaces {s})"))
            .unwrap_or_default();
        println!("  {}  {}{sup}", p.id, p.decision);
    }
    // Only move the cursor when every call worked, so nothing is skipped silently.
    if out.failed_calls == 0 {
        save_cursor(&cursor_path, &next)?;
    } else {
        println!("some calls failed; the next run reads the same texts again");
    }
    if !out.proposals.is_empty() {
        println!("\nnext: `decisions review`");
    }
    Ok(ExitCode::SUCCESS)
}

fn save_cursor(path: &Path, c: &Cursor) -> Result<()> {
    std::fs::create_dir_all(path.parent().expect("has parent"))?;
    std::fs::write(path, serde_json::to_string(c)?)?;
    Ok(())
}

fn default_sessions_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    let dir = PathBuf::from(home).join(".claude").join("projects");
    dir.is_dir().then_some(dir)
}

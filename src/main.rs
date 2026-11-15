//! `decisions`: a reviewed ledger of project decisions, put in front of coding agents at edit
//! time, checked before commit and in CI, and changed only on purpose.

mod check;
mod cmd;
mod diff;
mod extract;
mod git;
mod install;
mod lang;
mod message;
mod pattern;
mod record;
mod repo;
mod scope;
mod session;

use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(name = "decisions", version, about, long_about = None)]
struct Cli {
    /// Repository to work on (defaults to the current directory).
    #[arg(long, global = true, value_name = "DIR")]
    repo: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create decisions/ and install the Claude Code and git hooks.
    Init(cmd::init::Args),
    /// Record a decision (accepted, or `--propose` for review).
    Add(cmd::add::Args),
    /// Propose decisions from commits, GitHub PRs/reviews/issues and Claude Code sessions.
    Extract(cmd::extract::Args),
    /// Go through proposals one by one: accept, edit, reject or skip.
    Review,
    /// Accept proposals; a superseding proposal retires the decision it replaces.
    Accept {
        #[arg(required = true)]
        ids: Vec<String>,
    },
    /// Reject a proposal so extraction doesn't bring it back.
    Reject {
        id: String,
        #[arg(long)]
        reason: String,
    },
    /// Propose a replacement for an accepted decision.
    Supersede(cmd::supersede::Args),
    /// List decisions (`--all` includes superseded ones and proposals).
    List {
        #[arg(long)]
        all: bool,
    },
    /// Show one decision in full.
    Show { id: String },
    /// Decisions that apply to a file.
    For {
        path: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Check added lines against accepted decisions (pre-commit, CI).
    Check(cmd::check::Args),
    /// Decisions whose scope no longer exists, that are often overridden, or that are broken.
    Stale,
    /// Claude Code hook entry point (reads the hook JSON on stdin).
    Hook { event: cmd::hook::Event },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let dir = cli.repo.unwrap_or_else(|| PathBuf::from("."));
    let result = match cli.command {
        Command::Init(args) => cmd::init::run(&dir, args),
        Command::Add(args) => cmd::add::run(&dir, args),
        Command::Extract(args) => cmd::extract::run(&dir, args),
        Command::Review => cmd::review::interactive(&dir),
        Command::Accept { ids } => cmd::review::accept_cmd(&dir, &ids),
        Command::Reject { id, reason } => cmd::review::reject_cmd(&dir, &id, &reason),
        Command::Supersede(args) => cmd::supersede::run(&dir, args),
        Command::List { all } => cmd::list::list(&dir, all),
        Command::Show { id } => cmd::list::show(&dir, &id),
        Command::For { path, json } => cmd::list::for_path(&dir, &path, json),
        Command::Check(args) => cmd::check::run(&dir, args),
        Command::Stale => cmd::stale::run(&dir),
        Command::Hook { event } => cmd::hook::run(&dir, event),
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("decisions: {e:#}");
            ExitCode::from(2)
        }
    }
}

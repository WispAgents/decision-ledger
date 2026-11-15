use super::Outcome;
use crate::install::{self, Outcome as Hook};
use crate::repo::Repo;
use std::path::Path;
use std::process::ExitCode;

#[derive(clap::Args)]
pub struct Args {
    /// Don't touch .claude/settings.json.
    #[arg(long)]
    no_claude: bool,
    /// Don't install the git pre-commit hook.
    #[arg(long)]
    no_git_hook: bool,
}

pub fn run(dir: &Path, args: Args) -> Outcome {
    let repo = Repo::open(dir)?;
    std::fs::create_dir_all(repo.ledger.pending_dir())?;
    let gitignore = repo.ledger.dir.join(".gitignore");
    if !gitignore.exists() {
        std::fs::write(
            &gitignore,
            "# Proposals stay local until a human accepts them.\npending/\n",
        )?;
    }
    println!("created {}", repo.ledger.dir.display());
    if !args.no_claude {
        report(
            "Claude Code hooks (.claude/settings.json)",
            install::claude(repo.git.root())?,
        );
    }
    if !args.no_git_hook {
        report("git pre-commit hook", install::pre_commit(repo.git.git_dir())?);
    }
    println!(
        "\nnext: `decisions add \"...\"` to record a decision, or `decisions extract` to propose some from history"
    );
    Ok(ExitCode::SUCCESS)
}

fn report(what: &str, outcome: Hook) {
    match outcome {
        Hook::Installed => println!("installed {what}"),
        Hook::AlreadyPresent => println!("{what}: already installed"),
        Hook::Skipped(why) => println!("skipped {what}: {why}"),
    }
}

use super::Outcome;
use crate::check::{Checker, Target, Violation};
use crate::extract::llm;
use crate::git::DiffBase;
use crate::record::{Record, now};
use crate::repo::Repo;
use crate::scope;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

#[derive(clap::Args)]
pub struct Args {
    /// Files to check (default: every file with added lines).
    paths: Vec<String>,
    /// Check staged changes (pre-commit).
    #[arg(long, conflicts_with = "diff")]
    staged: bool,
    /// Check added lines in a revision range, e.g. `origin/main...HEAD` (CI).
    #[arg(long, value_name = "RANGE")]
    diff: Option<String>,
    #[arg(long, value_enum, default_value = "text")]
    format: Format,
    /// Also ask a model whether the change goes against decisions that have no
    /// deterministic checks. Reports only; never fails the check.
    #[arg(long)]
    judge: bool,
    #[arg(long, default_value = "auto")]
    llm: String,
}

#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Format {
    Text,
    Json,
    Github,
}

/// One line of `.git/decisions/overrides.jsonl`.
#[derive(Debug, Serialize, Deserialize)]
pub struct Override {
    pub time: i64,
    pub decision: String,
    pub path: String,
    pub code: String,
    pub reason: String,
}

pub fn run(dir: &Path, args: Args) -> Outcome {
    let repo = Repo::open(dir)?;
    let records = repo.live()?;
    let base = if args.staged {
        DiffBase::Staged
    } else if let Some(r) = &args.diff {
        DiffBase::Range(r.clone())
    } else {
        DiffBase::Head
    };
    let only: Vec<String> = args
        .paths
        .iter()
        .map(|p| repo.git.relative(Path::new(p)))
        .collect();
    let targets = repo.targets(&base, (!only.is_empty()).then_some(only.as_slice()))?;
    let checker = Checker::new(&records);
    let found: Vec<Violation> = targets.iter().flat_map(|t| checker.check(t)).collect();
    log_overrides(&repo, &found)?;
    let active: Vec<&Violation> = found.iter().filter(|v| v.overridden.is_none()).collect();
    let judged = if args.judge {
        judge(&records, &targets, &args.llm)?
    } else {
        Vec::new()
    };

    match args.format {
        Format::Json => {
            let v: Vec<_> = found.iter().map(|v| json!({"decision": v.decision, "path": v.path, "line": v.line, "what": v.what, "code": v.code, "overridden": v.overridden})).collect();
            let j: Vec<_> = judged
                .iter()
                .map(|j| json!({"decision": j.id, "path": j.path, "line": j.line, "why": j.why}))
                .collect();
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"violations": v, "possible": j}))?
            );
        }
        Format::Github => {
            for v in &active {
                let text = records
                    .iter()
                    .find(|r| r.id == v.decision)
                    .map(Record::line)
                    .unwrap_or_default();
                println!(
                    "::error file={},line={},title={}::{}",
                    prop(&v.path),
                    v.line,
                    prop(&format!("decision {}", v.decision)),
                    esc(&format!("{}. {text}", v.what))
                );
            }
            for j in &judged {
                println!(
                    "::warning file={},line={},title={}::{}",
                    prop(&j.path),
                    j.line,
                    prop(&format!("possibly against {}", j.id)),
                    esc(&j.why)
                );
            }
        }
        Format::Text => {
            for v in &found {
                let tag = if v.overridden.is_some() {
                    " [overridden]"
                } else {
                    ""
                };
                println!(
                    "{}:{}  {} {}{tag}: {}",
                    v.path, v.line, v.decision, v.what, v.code
                );
            }
            for j in &judged {
                println!("{}:{}  possibly against {}: {}", j.path, j.line, j.id, j.why);
            }
            if found.is_empty() && judged.is_empty() {
                println!(
                    "no decisions broken ({} files, {} decisions with checks)",
                    targets.len(),
                    records.iter().filter(|r| !r.checks.is_empty()).count()
                );
            }
        }
    }
    Ok(if active.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

pub fn log_overrides(repo: &Repo, found: &[Violation]) -> Result<()> {
    let entries: Vec<Override> = found
        .iter()
        .filter_map(|v| {
            v.overridden.as_ref().map(|reason| Override {
                time: now(),
                decision: v.decision.clone(),
                path: v.path.clone(),
                code: v.code.clone(),
                reason: reason.clone(),
            })
        })
        .collect();
    if entries.is_empty() {
        return Ok(());
    }
    let path = repo.state_dir().join("overrides.jsonl");
    std::fs::create_dir_all(repo.state_dir())?;
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    for e in entries {
        writeln!(f, "{}", serde_json::to_string(&e)?)?;
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct Judged {
    id: String,
    #[serde(default)]
    path: String,
    #[serde(default)]
    line: u32,
    why: String,
}

/// Ask a model about decisions without deterministic checks, one call per file.
fn judge(records: &[Record], targets: &[Target], backend: &str) -> Result<Vec<Judged>> {
    let Some(model) = llm::backend(backend, None)? else {
        eprintln!("decisions: --judge needs Claude Code or ANTHROPIC_API_KEY; skipped");
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for t in targets {
        let applicable: Vec<&Record> = scope::applicable(records, &t.path, Some(&t.content))
            .into_iter()
            .map(|(r, _)| r)
            .filter(|r| r.checks.is_empty())
            .collect();
        if applicable.is_empty() {
            continue;
        }
        let added: String = t
            .content
            .lines()
            .enumerate()
            .filter(|(i, _)| {
                t.added
                    .as_ref()
                    .is_none_or(|r| r.iter().any(|r| r.contains(*i as u32 + 1)))
            })
            .map(|(i, l)| format!("{:>5} | {l}\n", i + 1))
            .collect();
        let decisions: String = applicable.iter().map(|r| format!("- {}\n", r.line())).collect();
        let prompt = format!(
            "These project decisions apply to `{}`:\n{decisions}\nThese lines were added (line number | code):\n{added}\n\
             List only clear violations of a decision by the added lines. Reply with only JSON: \
             {{\"violations\": [{{\"id\": \"D-0001\", \"line\": 12, \"why\": \"<one sentence>\"}}]}}",
            t.path
        );
        let reply = model.complete(&prompt)?;
        let start = reply.find('{').context("no JSON from the judge")?;
        let end = reply.rfind('}').context("no JSON from the judge")?;
        #[derive(Deserialize)]
        struct R {
            violations: Vec<Judged>,
        }
        let parsed: R = serde_json::from_str(&reply[start..=end]).context("unexpected judge reply")?;
        out.extend(
            parsed
                .violations
                .into_iter()
                .filter(|j| applicable.iter().any(|r| r.id == j.id))
                .map(|mut j| {
                    j.path = t.path.clone();
                    j
                }),
        );
    }
    Ok(out)
}

fn esc(s: &str) -> String {
    s.replace('%', "%25").replace('\r', "%0D").replace('\n', "%0A")
}

fn prop(s: &str) -> String {
    esc(s).replace(',', "%2C").replace(':', "%3A")
}

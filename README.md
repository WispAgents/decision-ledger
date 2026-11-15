# decision-ledger

`decisions` keeps a reviewed ledger of project decisions in your repo. It puts the relevant ones in
front of coding agents right before they edit a file, blocks edits that break a decision's checks,
and makes changing a decision an explicit, reviewed step instead of silent drift.

```
decided in a PR, review, commit or session ──► decisions extract / add ──► you review
                                                                              │
              agent edits src/billing/x.ts ◄── PreToolUse: D-0012 applies here┘
                    │  breaks a check? ──► edit denied, with the decision and how to supersede it
                    └► pre-commit and CI run the same checks on added lines
```

Status: v0.1. The validation study that came first ([study/](study/report.md)) found that on public
repos, only 4.5% of human corrections of agent code were already written down before the agent ran.
The ledger is most useful when a team records decisions as they make them (`decisions add`, or let
`extract` pick them up from PRs and reviews), not as a way to mine old history.

## Install

```sh
cargo install --locked --git https://github.com/WispAgents/decision-ledger decision-ledger
```

Needs `git`. Optional: `gh` (logged in) to read GitHub PRs, reviews and issues; Claude Code (`claude`)
or `ANTHROPIC_API_KEY` for `extract` and `check --judge`.

## Quick start

```sh
cd your-repo
decisions init                     # decisions/, Claude Code hooks, git pre-commit hook

decisions add "Billing code calls the payments client, never raw fetch." \
  --reason "the client adds idempotency keys" \
  --scope "src/billing/**" \
  --forbid-pattern 'TypeScript:fetch($$$A)'

decisions extract                  # propose decisions from commits, PRs, reviews, issues, your sessions
decisions review                   # accept / edit / reject, one card per proposal
git commit decisions/ -m "decisions"
```

## A decision

One YAML file per decision, `decisions/D-0001.yml`, reviewed like code:

```yaml
id: D-0001
decision: Billing code calls the payments client, never raw fetch.
reason: the client adds idempotency keys
rejected:
- a shared fetch wrapper per module
scope:
- src/billing/**          # path globs, or symbol / module names like `apiClient`
category: convention      # convention | constraint | api_change | architecture | scope
status: accepted          # accepted | superseded (proposals live in decisions/pending/)
supersedes: D-0000
checks:                   # optional; without checks a decision is shown, not enforced
  forbid_imports: [axios]
  forbid_deps: [redis]    # checked in package.json, pyproject.toml, requirements*.txt, Cargo.toml, go.mod
  forbid_patterns:
  - language: TypeScript
    pattern: fetch($$$A)  # ast-grep pattern
sources:
- https://github.com/acme/app/pull/214
author: Ana
date: 2026-11-15
```

Scope: a glob matches the path. A bare directory (`src/billing`) means everything under it. A
symbol (`apiClient`, `Query.commands`) matches files whose path names it or whose content mentions
it. No scope, or `**`, is repo-wide.

## What the agent sees (Claude Code)

`decisions init` adds three hooks to `.claude/settings.json`. Each is a no-op for teammates without
`decisions` installed.

| Hook | What happens |
|---|---|
| `SessionStart` | Repo-wide decisions go into the session, plus how to propose a new one (`decisions add --propose`) |
| `PreToolUse` on Edit / Write / MultiEdit | The edit is simulated first. If an added line breaks a decision's checks, the edit is **denied** with the decision and the way out. Otherwise the decisions scoped to that file are added as context, each once per session |
| `Stop` | Re-checks only the files this session edited. A broken decision sends the turn back (up to 3 times) |

Agents can't edit accepted records under `decisions/`; they propose with `decisions add --propose`
or `decisions supersede`. Pre-commit and CI judge a change by the ledger it started from, so a branch
can't retire a decision and break it in the same diff. CI also flags any ledger change.

Tested live with Claude Code 2.1.322: asked to write raw `fetch` in `src/billing/`, the edit was
denied, the file left unchanged, and the agent explained the decision to the user instead of working
around it. Scoped context through `PreToolUse` `additionalContext` needs a recent Claude Code
(2.1 or later); older versions ignore it, but the deny still works.

**Known limits:** the Stop hook only re-checks files the agent changed through Edit / Write /
MultiEdit. Changes made through Bash (`sed -i`, code generators) are caught by pre-commit and CI,
not in the session. Import detection is line-based for JS/TS, Python, Go and Rust.

## Changing a decision

Agents and people don't edit around a decision; they propose a replacement:

```sh
decisions supersede D-0002 --decision "Use Redis for the cache." --reason "multi-node deploys"
decisions review          # accepting it marks D-0002 superseded by the new id
```

The old decision stays in force until a human accepts the new one. To keep one line that breaks a
decision on purpose, add a comment with a reason: `// decisions-ignore D-0001: webhook replay needs raw fetch`.
Overrides are logged, and `decisions stale` flags decisions that are overridden often.

## Commands

| Command | |
|---|---|
| `init` | Create `decisions/`, install hooks |
| `add <decision> [--scope] [--reason] [--forbid-import] [--forbid-dep] [--forbid-pattern Lang:pattern] [--propose]` | Record a decision; `--propose` writes a proposal instead |
| `extract [--since DATE] [--github owner/name] [--no-git] [--no-github] [--no-sessions] [--dry-run]` | Propose decisions from commits, GitHub, and what you typed in Claude Code sessions. Resumes where it stopped |
| `review`, `accept <ids>`, `reject <id> --reason` | Act on proposals; rejected ones aren't proposed again |
| `supersede <id> --decision --reason` | Propose a replacement |
| `list [--all]`, `show <id>`, `for <path> [--json]` | Read the ledger |
| `check [paths] [--staged \| --diff RANGE] [--format text\|json\|github] [--judge]` | Check added lines; exit 1 on a broken decision. `--judge` also asks a model about decisions without checks (report only) |
| `stale` | Scopes that match nothing, decisions overridden often, broken supersede links |

## CI

```yaml
# .github/workflows/decisions.yml
on: pull_request
jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with: { fetch-depth: 0 }
      - uses: WispAgents/decision-ledger/action@main
```

## How extraction works

`extract` reads texts it hasn't read before (tracked by id in `.git/decisions/seen.json`, back to
90 days before the first run), oldest first, 400 per run: commit messages, GitHub PRs, review
comments, issues and issue comments written by the repo's owners, members and collaborators (via
`gh api`), and the messages you typed in Claude Code sessions for this repo. Bots and short texts
are skipped. A batch that keeps failing is retried on later runs, at most 3 times. A
model (`claude -p`, or the Anthropic API with `claude-opus-5-5`) returns decision records, using the
prompt measured in the study: 82% of extracted records were real, durable decisions. Existing
decisions are passed in, so a text that changes one comes back as a proposal that supersedes it.
Near-duplicates of existing, pending or rejected decisions are dropped. Nothing is accepted
automatically.

## Development

```sh
cargo test                # unit + end-to-end on a scripted repo (fake `claude` on PATH)
cargo clippy --all-targets -- -D warnings
```

[PLAN.md](PLAN.md) is the build plan; [study/](study/) holds the validation study and its data.

License: Apache-2.0.

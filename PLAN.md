# Decision Ledger: build plan

Single source of truth for the build. Re-read before each stage and tick items as they land.
The validation study that came first is in [study/report.md](study/report.md) (verdict there: premise weak on
public data; built anyway at the user's request, so the product must be useful with hand-written
decisions too, not only extracted ones).

## Goal

A CLI, `decisions`, that keeps a reviewed ledger of project decisions in the repo and puts the
relevant ones in front of coding agents at edit time, checks changes against them, and makes
changing a decision explicit.

## Decisions (fixed unless this file changes)

- **Rust**, single crate, binary `decisions`. ast-grep crates (0.51) as a library for pattern checks.
  Git and diff helpers adapted from ingrain (config-independent diffs, rename detection).
- **Storage:** `decisions/D-0001.yml` committed, one record per file, reviewed like code.
  Proposals in `decisions/pending/` (gitignored). Local state in `.git/decisions/`
  (extract cursor, sessions, override log).
- **Record:** id, decision, reason, rejected (alternatives), scope (globs or symbols), category,
  status (accepted | superseded), supersedes / superseded_by, sources, author, date,
  optional `checks`: `forbid_imports`, `forbid_deps`, `forbid_patterns` (ast-grep), `require_patterns`? (no: keep to forbids).
- **Extraction:** the prompt validated in the study (82% precision). Backends: `claude -p` (default),
  Anthropic API (`ANTHROPIC_API_KEY`, `claude-opus-5-5`), `none`. Never used by checks or hooks.
- **Hooks (Claude Code):**
  - SessionStart: inject repo-wide decisions (scope `**`), capped.
  - PreToolUse Edit|Write|MultiEdit: simulate the edit; deterministic check failures → `deny` with
    the decision and how to supersede; otherwise inject scoped decisions not yet shown this session
    as `additionalContext`.
  - Stop: re-check files the session edited; block up to N times.
  - All hooks fail open and no-op if `decisions` isn't installed.
- **Changing a decision:** `decisions supersede D-0012 --decision ... --reason ...` writes a pending
  record; only a human `accept` makes it live and marks the old one superseded.
- **Checks only flag added lines.** Override: `decisions-ignore D-0012: <reason>` comment.
- Optional LLM judge (`check --judge`) for decisions without deterministic checks; off by default.

## Commands

init · add · extract · review / accept / reject · list · show · for <path> · check · supersede ·
stale · hook <event>

## Stages

- [x] 1. Crate skeleton, record model + store, scope matching
- [x] 2. git/diff helpers, deterministic checks (imports, deps, patterns), overrides
- [x] 3. Extraction: sources (git log, GitHub PRs/reviews/issues, Claude transcripts), LLM, dedup, cursor
- [x] 4. CLI commands
- [x] 5. Hooks + install (Claude Code settings, pre-commit), GitHub Action, CI
- [x] 6. Tests: unit + end-to-end on a scripted repo; live check that Claude Code receives the context
- [x] 7. Independent review, fixes
- [x] 8. README, push

## Log
- 2026-11-15: Built the crate; 29 unit + 3 end-to-end tests (fake `claude` on PATH for extract).
- Live check with Claude Code 2.1.322: SessionStart context made the agent consult the ledger and refuse;
  when pushed to write raw fetch, PreToolUse denied the Write and the agent reported the decision.
- Study files moved to study/ (the study PLAN.md was accidentally overwritten, then restored from git).
- Review (Claude subagent): 22 findings. Fixed 1-21: symlinked/new-file paths, outside-repo edits breaking Stop,
  agents editing the ledger, CI judged by the base ledger, id-based extraction state, language-aware imports,
  parsed manifests, safe accept order, lenient hooks, trimmed added lines, /compact reset, judge fail-soft,
  session cwd filter, maintainer-only GitHub texts, escaping, unique proposal ids, atomic session writes.
  Not fixed (documented): Stop doesn't see Bash edits (#22).
- Live re-check after the rewrite: Write with raw fetch still denied by PreToolUse.

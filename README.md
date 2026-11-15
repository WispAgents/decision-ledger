# decision-ledger

Validation study for a **Decision Ledger**: project memory that records durable decisions (conventions,
constraints, API changes, scope choices) from PRs, reviews, issues and commits, and injects the relevant
ones before a coding agent edits a file.

**Result: don't build it (yet).** On 245 human corrections of agent code from 7 public repos, only
**4.5%** had their reason written down anywhere before the agent wrote the code (bar: 20%). The
extractor itself works: 82% of extracted records are real durable decisions, and it recovered 11/11
gold decisions. Full write-up: [report.md](report.md). Pre-registered bars: [PLAN.md](PLAN.md).

## Contents

| File | What it does |
|---|---|
| `fetch.py` | Fetch a repo's PRs, review comments, issues and issue comments via `gh api` |
| `pack.py` | Sample corrections and build context packs (stated fix reason + candidate prior records) |
| `judge.py`, `JUDGE.md` | Label each pack: kind of correction, and whether its reason was recorded before |
| `extract.py` | The decision extractor (`claude -p`, JSON records: decision, reason, scope, category) |
| `judge_records.py` | Rate extracted records: durable / one-off / wrong, scope ok / bad |
| `data/` | Sample, labels and extraction outputs behind the report |

## Requirements

Python 3.10+, `git`, `gh` (logged in), Claude Code (`claude`) for extraction and judging. Corrections
come from [`ingrain mine --pr-refs --github`](https://github.com/WispAgents/ingrain).

## Reproduce

```sh
python3 -I fetch.py owner/name /tmp/ledger/corpus 100
python3 -I pack.py <clones_dir> /tmp/ledger/corpus /tmp/ledger/packs 35 <owner_name ...>
python3 -I judge.py /tmp/ledger/packs/packs.jsonl labels.jsonl 5
python3 -I extract.py texts.jsonl extracted.jsonl 8
```

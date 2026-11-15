//! Deterministic checks of changed code against live decisions.
//!
//! Only added lines count, so existing code never fails a check. A line (or the line above)
//! carrying `decisions-ignore D-0012: <reason>` overrides that decision there; a reason is required.

use crate::diff::LineRange;
use crate::lang;
use crate::pattern::{CompiledRule, Parsed};
use crate::record::Record;
use crate::scope::Scope;
use std::collections::HashMap;

/// One file as it will be after the change.
pub struct Target {
    pub path: String,
    pub content: String,
    /// Added lines (1-based). `None` means the whole file counts as added.
    pub added: Option<Vec<LineRange>>,
}

impl Target {
    fn is_added(&self, line: u32) -> bool {
        self.added
            .as_ref()
            .is_none_or(|r| r.iter().any(|r| r.contains(line)))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub decision: String,
    pub path: String,
    pub line: u32,
    /// What broke the decision, e.g. "imports `axios`".
    pub what: String,
    pub code: String,
    /// Set when the line carries a `decisions-ignore` comment with a reason.
    pub overridden: Option<String>,
}

/// Compiled checks for a set of decisions, reusable across files.
pub struct Checker<'a> {
    records: Vec<&'a Record>,
    patterns: HashMap<String, Vec<CompiledRule>>,
}

impl<'a> Checker<'a> {
    /// Live decisions with checks. Patterns that don't compile are reported and skipped.
    pub fn new(records: &'a [Record]) -> Self {
        let records: Vec<&Record> = records
            .iter()
            .filter(|r| r.is_live() && !r.checks.is_empty())
            .collect();
        let mut patterns = HashMap::new();
        for r in &records {
            let compiled: Vec<CompiledRule> = r
                .checks
                .forbid_patterns
                .iter()
                .enumerate()
                .filter_map(|(i, p)| {
                    let yaml = format!(
                        "id: {}-{i}\nlanguage: {}\nrule:\n  pattern: {}\nmessage: x\n",
                        r.id,
                        p.language,
                        serde_json::to_string(&p.pattern).expect("string serializes")
                    );
                    CompiledRule::compile(&yaml)
                        .map_err(|e| eprintln!("decisions: {} pattern `{}` skipped: {e:#}", r.id, p.pattern))
                        .ok()
                })
                .collect();
            patterns.insert(r.id.clone(), compiled);
        }
        Self { records, patterns }
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn check(&self, target: &Target) -> Vec<Violation> {
        let lines: Vec<&str> = target.content.lines().collect();
        let manifest = is_manifest(&target.path);
        let language = lang::detect(&target.path);
        let parsed = language.map(|l| Parsed::new(l, &target.content));
        let mut out = Vec::new();
        for r in &self.records {
            // Dependency decisions are about the project, so they apply to every manifest.
            if manifest {
                for (i, line) in lines.iter().enumerate() {
                    let n = i as u32 + 1;
                    if !target.is_added(n) {
                        continue;
                    }
                    for dep in &r.checks.forbid_deps {
                        if declares_dep(&target.path, line, dep) {
                            out.push(violation(
                                r,
                                target,
                                &lines,
                                n,
                                format!("adds dependency `{dep}`"),
                            ));
                        }
                    }
                }
            }
            if Scope::of(r)
                .matches(&target.path, Some(&target.content))
                .is_none()
            {
                continue;
            }
            for (i, line) in lines.iter().enumerate() {
                let n = i as u32 + 1;
                if !target.is_added(n) {
                    continue;
                }
                if let Some(module) = imported_module(line) {
                    for forbidden in &r.checks.forbid_imports {
                        if module_matches(&module, forbidden) {
                            out.push(violation(r, target, &lines, n, format!("imports `{module}`")));
                        }
                    }
                }
            }
            if let Some(parsed) = &parsed {
                for rule in self.patterns.get(&r.id).into_iter().flatten() {
                    if !rule.applies_to(&target.path) {
                        continue;
                    }
                    for hit in rule.scan(parsed) {
                        let added = (hit.start_line..=hit.end_line).any(|l| target.is_added(l));
                        if added {
                            out.push(violation(
                                r,
                                target,
                                &lines,
                                hit.start_line,
                                format!("matches `{}`", hit.matched),
                            ));
                        }
                    }
                }
            }
        }
        out.sort_by(|a, b| {
            a.path
                .cmp(&b.path)
                .then(a.line.cmp(&b.line))
                .then(a.decision.cmp(&b.decision))
        });
        out.dedup_by(|a, b| a.decision == b.decision && a.path == b.path && a.line == b.line);
        out
    }
}

fn violation(r: &Record, t: &Target, lines: &[&str], line: u32, what: String) -> Violation {
    Violation {
        decision: r.id.clone(),
        path: t.path.clone(),
        line,
        what,
        code: lines
            .get(line as usize - 1)
            .map(|l| l.trim().chars().take(160).collect())
            .unwrap_or_default(),
        overridden: ignore_reason(lines, line, &r.id),
    }
}

/// `decisions-ignore <id>: <reason>` on the line or the line above.
fn ignore_reason(lines: &[&str], line: u32, id: &str) -> Option<String> {
    let idx = line as usize - 1;
    for i in [idx.checked_sub(1), Some(idx)].into_iter().flatten() {
        let Some(text) = lines.get(i) else { continue };
        for (pos, _) in text.match_indices("decisions-ignore") {
            let rest = text[pos + "decisions-ignore".len()..].trim_start();
            let Some(after) = rest.strip_prefix(id) else {
                continue;
            };
            if after.starts_with(|c: char| c.is_alphanumeric()) {
                continue;
            }
            let reason = after.trim_start().strip_prefix(':').map(|r| {
                r.split("decisions-ignore")
                    .next()
                    .unwrap_or("")
                    .trim()
                    .trim_end_matches(';')
                    .trim_end_matches("*/")
                    .trim_end_matches("-->")
                    .trim()
                    .to_owned()
            });
            if let Some(r) = reason.filter(|r| !r.is_empty()) {
                return Some(r);
            }
        }
    }
    None
}

/// The module a line imports, for JS/TS, Python, Go and Rust import syntax.
fn imported_module(line: &str) -> Option<String> {
    let t = line.trim();
    let quoted = |s: &str| -> Option<String> {
        let start = s.find(['\'', '"', '`'])?;
        let q = s[start..].chars().next()?;
        let rest = &s[start + 1..];
        Some(rest[..rest.find(q)?].to_owned())
    };
    if t.starts_with("import ") || t.starts_with("export ") {
        if let Some(pos) = t.find(" from ") {
            return quoted(&t[pos..]);
        }
        if t.starts_with("import ") && (t.contains('\'') || t.contains('"')) {
            return quoted(t); // `import 'x'` (JS) or Go single-line `import "x"`
        }
        // Python `import a.b as c`, `import a, b`
        let rest = t.strip_prefix("import ")?;
        let first = rest.split([',', ' ']).next()?.trim();
        return (!first.is_empty() && !first.starts_with('{') && !first.starts_with('*'))
            .then(|| first.to_owned());
    }
    if let Some(rest) = t.strip_prefix("from ") {
        return rest.split_whitespace().next().map(str::to_owned);
    }
    if let Some(pos) = t.find("require(").or_else(|| t.find("import(")) {
        return quoted(&t[pos..]);
    }
    if let Some(rest) = t.strip_prefix("use ").or_else(|| t.strip_prefix("pub use ")) {
        return Some(
            rest.trim_end_matches(';')
                .split(['{', ' '])
                .next()?
                .trim_end_matches("::")
                .to_owned(),
        );
    }
    // A line inside a Go import block: `"net/http"` or `alias "net/http"`.
    if t.ends_with('"') && t.matches('"').count() == 2 && !t.contains(['=', '(', ':']) {
        return quoted(t);
    }
    None
}

fn module_matches(module: &str, forbidden: &str) -> bool {
    module == forbidden
        || [".", "/", "::"].iter().any(|sep| module.starts_with(&format!("{forbidden}{sep}")))
        // Relative paths: forbidding `api/` should catch `../api/client`.
        || (forbidden.contains('/') && module.contains(forbidden.trim_start_matches("./")))
}

fn is_manifest(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    matches!(
        name,
        "package.json" | "pyproject.toml" | "Cargo.toml" | "go.mod" | "Pipfile" | "setup.py" | "setup.cfg"
    ) || (name.starts_with("requirements") && name.ends_with(".txt"))
}

/// Whether an (added) manifest line declares `dep`.
fn declares_dep(path: &str, line: &str, dep: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    let t = line.trim().trim_start_matches(['"', '\'']);
    let dep_lower = dep.to_lowercase();
    let lower = t.to_lowercase();
    let starts_with_name = |s: &str| {
        s.starts_with(&dep_lower)
            && s[dep_lower.len()..]
                .chars()
                .next()
                .is_none_or(|c| !(c.is_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '/'))
    };
    match name {
        "package.json" => line.contains(&format!("\"{dep}\"")) && line.contains(':'),
        "go.mod" => lower.trim_start_matches("require ").starts_with(&dep_lower),
        _ => starts_with_name(&lower),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{Checks, PatternCheck, Status};

    fn rec(checks: Checks, scope: &[&str]) -> Record {
        Record {
            id: "D-0007".into(),
            decision: "x".into(),
            scope: scope.iter().map(|s| s.to_string()).collect(),
            status: Status::Accepted,
            checks,
            ..Default::default()
        }
    }

    fn target(path: &str, content: &str, added: Option<Vec<LineRange>>) -> Target {
        Target {
            path: path.into(),
            content: content.into(),
            added,
        }
    }

    #[test]
    fn imports() {
        assert_eq!(
            imported_module("import axios from 'axios';").as_deref(),
            Some("axios")
        );
        assert_eq!(
            imported_module("import { a } from \"../api/client\"").as_deref(),
            Some("../api/client")
        );
        assert_eq!(
            imported_module("const x = require('node-fetch')").as_deref(),
            Some("node-fetch")
        );
        assert_eq!(
            imported_module("from requests.adapters import X").as_deref(),
            Some("requests.adapters")
        );
        assert_eq!(imported_module("import os, sys").as_deref(), Some("os"));
        assert_eq!(
            imported_module("use reqwest::Client;").as_deref(),
            Some("reqwest::Client")
        );
        assert_eq!(imported_module("\t\"net/http\"").as_deref(), Some("net/http"));
        assert_eq!(imported_module("let x = 1;"), None);
        assert!(module_matches("requests.adapters", "requests"));
        assert!(!module_matches("requests_toolbelt", "requests"));
        assert!(module_matches("../api/client", "api/"));
    }

    #[test]
    fn checks_only_added_lines_in_scope() {
        let records = vec![rec(
            Checks {
                forbid_imports: vec!["axios".into()],
                forbid_patterns: vec![PatternCheck {
                    language: "TypeScript".into(),
                    pattern: "fetch($$$A)".into(),
                }],
                ..Default::default()
            },
            &["src/**"],
        )];
        let checker = Checker::new(&records);
        let content = "import axios from 'axios';\nconst a = fetch(u);\nconst b = fetch(v);\n";
        let found = checker.check(&target("src/a.ts", content, Some(vec![LineRange::new(3, 3)])));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].line, 3);
        let all = checker.check(&target("src/a.ts", content, None));
        assert_eq!(all.len(), 3);
        assert!(checker.check(&target("test/a.ts", content, None)).is_empty());
    }

    #[test]
    fn deps_apply_to_manifests_regardless_of_scope() {
        let records = vec![rec(
            Checks {
                forbid_deps: vec!["axios".into(), "redis".into()],
                ..Default::default()
            },
            &["src/**"],
        )];
        let checker = Checker::new(&records);
        let pkg = "{\n  \"dependencies\": {\n    \"axios\": \"^1.0.0\"\n  }\n}\n";
        assert_eq!(checker.check(&target("package.json", pkg, None)).len(), 1);
        let req = "redis==5.0\nredis-om==1\n";
        assert_eq!(checker.check(&target("requirements.txt", req, None)).len(), 1);
        let py = "dependencies = [\n  \"redis>=5\",\n]\n";
        assert_eq!(checker.check(&target("pyproject.toml", py, None)).len(), 1);
    }

    #[test]
    fn overrides_need_a_reason() {
        let records = vec![rec(
            Checks {
                forbid_imports: vec!["axios".into()],
                ..Default::default()
            },
            &[],
        )];
        let checker = Checker::new(&records);
        let with = "// decisions-ignore D-0007: legacy upload path\nimport axios from 'axios';\n";
        let without = "import axios from 'axios'; // decisions-ignore D-0007\n";
        assert_eq!(
            checker.check(&target("a.ts", with, None))[0]
                .overridden
                .as_deref(),
            Some("legacy upload path")
        );
        assert_eq!(checker.check(&target("a.ts", without, None))[0].overridden, None);
    }
}

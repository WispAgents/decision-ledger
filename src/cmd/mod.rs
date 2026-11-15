//! One module per subcommand.

pub mod add;
pub mod check;
pub mod extract;
pub mod hook;
pub mod init;
pub mod list;
pub mod review;
pub mod stale;
pub mod supersede;

pub type Outcome = anyhow::Result<std::process::ExitCode>;

use crate::record::{Category, PatternCheck};
use anyhow::{Context, Result, bail};

/// `--category` values.
pub fn parse_category(s: &str) -> Result<Category> {
    Ok(match s {
        "convention" => Category::Convention,
        "constraint" => Category::Constraint,
        "api_change" | "api-change" => Category::ApiChange,
        "architecture" => Category::Architecture,
        "scope" => Category::Scope,
        other => {
            bail!("unknown category `{other}` (convention, constraint, api_change, architecture, scope)")
        }
    })
}

/// `--forbid-pattern Language:pattern`, e.g. `TypeScript:fetch($$$A)`.
pub fn parse_pattern(s: &str) -> Result<PatternCheck> {
    let (language, pattern) = s
        .split_once(':')
        .context("--forbid-pattern takes `Language:pattern`, e.g. `TypeScript:fetch($$$A)`")?;
    if crate::lang::parse(language).is_none() {
        bail!("unknown language `{language}`");
    }
    Ok(PatternCheck {
        language: language.to_owned(),
        pattern: pattern.to_owned(),
    })
}

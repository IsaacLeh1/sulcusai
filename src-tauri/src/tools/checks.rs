// SPDX-License-Identifier: AGPL-3.0-only
//! `check_changes`: the fast checks for just the files a chat touched, with
//! a short pass or fail and the first errors. A project names its checks in
//! `.sulcus/checks.json`; its rules (no em dashes, no assistant
//! attribution, no secrets) are on unless that file turns them off.
//!
//! ```json
//! { "rules": { "em_dash": true, "attribution": true, "secrets": true },
//!   "checks": [ { "name": "types", "command": "node node_modules/typescript/bin/tsc --noEmit -p .", "when": ["ts", "tsx"] },
//!               { "name": "tests", "command": "node node_modules/vitest/vitest.mjs run {files}", "when": ["ts"] } ] }
//! ```

use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{json, Value};

use super::{arg_str, Ctx, Outcome};

pub fn check_changes_params() -> Value {
    json!({ "type": "object", "required": ["path"], "properties": {
        "path": { "type": "string", "description": "The project folder." },
        "files": { "type": "array", "items": { "type": "string" }, "description": "Files to check (default: the ones this chat changed there)." } } })
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub rules: Rules,
    pub checks: Vec<Check>,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Rules {
    pub em_dash: bool,
    pub attribution: bool,
    pub secrets: bool,
}

impl Default for Rules {
    fn default() -> Self {
        Rules { em_dash: true, attribution: true, secrets: true }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct Check {
    pub name: String,
    pub command: String,
    /// File extensions that make it run (empty: always).
    #[serde(default)]
    pub when: Vec<String>,
}

pub fn load(root: &Path) -> Config {
    std::fs::read_to_string(root.join(".sulcus").join("checks.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

static SECRET: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(sk-(ant-)?[A-Za-z0-9_-]{20,}|ghp_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{30,}|AKIA[0-9A-Z]{16}|xox[baprs]-[A-Za-z0-9-]{10,}|AIza[0-9A-Za-z_-]{35}|-----BEGIN (RSA |EC |OPENSSH )?PRIVATE KEY-----)").unwrap()
});
static ATTRIBUTION: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"(?i)(co-authored-by:\s*claude|generated with \[?claude|noreply@anthropic\.com)").unwrap());

/// What breaks the rules in `text` (a commit message or added lines).
pub fn check_rules(rules: &Rules, label: &str, text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let at = || if text.lines().count() > 1 { format!("{label} line {}", i + 1) } else { label.to_string() };
        if rules.em_dash && line.contains('\u{2014}') {
            out.push(format!("{}: an em dash (use a comma, colon or parentheses)", at()));
        }
        if rules.attribution && ATTRIBUTION.is_match(line) {
            out.push(format!("{}: assistant attribution", at()));
        }
        if rules.secrets && SECRET.is_match(line) {
            out.push(format!("{}: what looks like a key or secret", at()));
        }
    }
    out
}

pub fn rule_problems(label: &str, text: &str) -> Vec<String> {
    check_rules(&Rules::default(), label, text)
}

/// The rules on the lines a file adds since the last commit (all of it
/// when it's new), so old text the chat didn't write isn't blamed on it.
pub fn added_rule_problems(root: &Path, file: &Path, text: &str) -> Vec<String> {
    let rules = load(root).rules;
    let label = file.strip_prefix(root).unwrap_or(file).display().to_string();
    let added = match super::git::git(root, &["diff", "HEAD", "--no-color", "-U0", "--", &file.display().to_string()]) {
        Ok((true, d, _)) if !d.trim().is_empty() => d.lines().filter(|l| l.starts_with('+') && !l.starts_with("+++")).map(|l| &l[1..]).collect::<Vec<_>>().join("\n"),
        Ok((true, _, _)) if super::git::git(root, &["ls-files", "--error-unmatch", &file.display().to_string()]).is_ok_and(|r| r.0) => String::new(),
        _ => text.to_string(),
    };
    check_rules(&rules, &label, &added)
}

fn ext(p: &Path) -> String {
    p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
}

/// The checks that apply to these files: the project's own, or a type
/// check by the files' languages when it names none.
pub fn checks_for(root: &Path, cfg: &Config, files: &[PathBuf]) -> Vec<Check> {
    let exts: Vec<String> = files.iter().map(|f| ext(f)).collect();
    let picked: Vec<Check> = cfg.checks.iter().filter(|c| c.when.is_empty() || c.when.iter().any(|w| exts.contains(&w.trim_start_matches('.').to_ascii_lowercase()))).cloned().collect();
    if !cfg.checks.is_empty() {
        return picked;
    }
    let mut out = Vec::new();
    if exts.iter().any(|e| matches!(e.as_str(), "ts" | "tsx")) && root.join("tsconfig.json").exists() {
        out.push(Check { name: "types".into(), command: super::code::checker(root, "typescript").unwrap().1, when: vec![] });
    }
    if exts.iter().any(|e| e == "rs") {
        if let Some(dir) = files.iter().find(|f| ext(f) == "rs").and_then(|f| f.ancestors().find(|a| a.join("Cargo.toml").exists() && a.starts_with(root))) {
            out.push(Check { name: "rust".into(), command: format!("cargo check --message-format=short --quiet --manifest-path \"{}\"", dir.join("Cargo.toml").display()), when: vec![] });
        }
    }
    out
}

pub async fn run(args: &Value, ctx: &Ctx<'_>) -> Outcome {
    match check(args, ctx).await {
        Ok(o) => o,
        Err(e) => Outcome::error("Check changes", e),
    }
}

async fn check(args: &Value, ctx: &Ctx<'_>) -> Result<Outcome, String> {
    let dir = ctx.sandbox.resolve(arg_str(args, "path")?)?;
    let root = super::git::repo_root(&dir).unwrap_or_else(|_| dir.clone());
    let named: Vec<PathBuf> = args.get("files").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(|f| ctx.sandbox.resolve(f)).collect::<Result<_, _>>()?;
    let files = if named.is_empty() {
        let conn = ctx.memory.db.lock().unwrap();
        let mut v: Vec<PathBuf> = crate::db::chat_changes(&conn, ctx.memory.cipher, ctx.memory.chat_id).into_iter().filter(|c| !c.undone).map(|c| PathBuf::from(c.path)).filter(|p| p.starts_with(&root) && p.is_file()).collect();
        v.sort();
        v.dedup();
        v
    } else {
        named
    };
    if files.is_empty() {
        return Err("This chat hasn't changed any files there; name the files to check.".into());
    }
    let cfg = load(&root);
    let mut problems = Vec::new();
    for f in &files {
        if let Ok(text) = std::fs::read_to_string(f) {
            problems.extend(added_rule_problems(&root, f, &text));
        }
    }
    let mut report = vec![if problems.is_empty() { "Rules: pass".to_string() } else { format!("Rules: fail\n  {}", problems.join("\n  ")) }];
    let mut failed = !problems.is_empty();
    let list: Vec<String> = files.iter().map(|f| format!("\"{}\"", f.display())).collect();
    for c in checks_for(&root, &cfg, &files) {
        let cmd = c.command.replace("{files}", &list.join(" "));
        let run = super::shell::run_check(&cmd, &root, &[], "", std::time::Duration::from_secs(900)).await?;
        let all = format!("{}\n{}", run.out, run.err);
        let found = super::code::parse_problems(&all);
        if run.code == Some(0) {
            report.push(format!("{}: pass", c.name));
        } else {
            failed = true;
            report.push(if found.is_empty() { format!("{}: fail\n{}", c.name, super::git::page(&all, 1, 30)) } else { format!("{}: fail, {}", c.name, super::code::problems_text(&found, 10)) });
        }
    }
    let verdict = if failed { "FAIL" } else { "PASS" };
    Ok(Outcome::ok(format!("{verdict} for {} file{}.\n{}", files.len(), if files.len() == 1 { "" } else { "s" }, report.join("\n")), format!("Checked changes: {verdict}"), "text", None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_commit_with_an_em_dash_or_a_key_is_refused() {
        assert!(rule_problems("m", "Fix the parser \u{2014} again").iter().any(|p| p.contains("em dash")));
        assert!(rule_problems("m", "key = sk-ant-abcdefghijklmnopqrstuvwx").iter().any(|p| p.contains("secret")));
        assert!(rule_problems("m", "Co-Authored-By: Claude <noreply@anthropic.com>").iter().any(|p| p.contains("attribution")));
        assert!(rule_problems("m", "Fix the parser, again").is_empty());
    }

    #[test]
    fn a_project_file_can_turn_rules_off_and_name_checks() {
        let cfg: Config = serde_json::from_str(r#"{ "rules": { "em_dash": false }, "checks": [ { "name": "t", "command": "x {files}", "when": ["ts"] } ] }"#).unwrap();
        assert!(!cfg.rules.em_dash && cfg.rules.secrets);
        assert!(check_rules(&cfg.rules, "m", "a \u{2014} b").is_empty());
        assert_eq!(checks_for(Path::new("."), &cfg, &[PathBuf::from("a.ts")]).len(), 1);
        assert!(checks_for(Path::new("."), &cfg, &[PathBuf::from("a.rs")]).is_empty());
    }
}

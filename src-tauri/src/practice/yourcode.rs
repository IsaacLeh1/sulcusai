// SPDX-License-Identifier: AGPL-3.0-only
//! Practice on the person's own code: which file defines a function, what
//! it takes and what calls it, found by searching and reading the folders
//! they picked with the file tools chats use (search_files, find_files,
//! read_file).
//!
//! The folders are scanned once, with a simple parse of Rust, TypeScript,
//! JavaScript and Python definitions, and the scan is the answer key. Each
//! try runs for real: the model's tool calls are carried out on the folders
//! (read only, in small pieces), the results go back to it, and its final
//! answer is checked against the scan.
//!
//! A teacher (a bigger model on this PC) tries the tasks the student
//! missed the same way. Its right tries are then written the student's way:
//! each of its turns laid out by the student's own template, with the same
//! tool results after it, so the student learns a whole lookup it couldn't
//! do itself.
//!
//! The code itself isn't trained on: a small model would remember it
//! fuzzily, that memory goes stale as the code changes, and it gets worse
//! at chatting. Reading the files is exact and current, so what's practised
//! is the looking.
//!
//! Dependencies, build output, lockfiles and anything that may hold a
//! secret (.env files, keys, tokens, or text that looks like one) are never
//! scanned or shown.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, LazyLock};

use regex::Regex;
use serde_json::{json, Value};

use super::tasks::{self, Check, Reply, Rng, Skill, Task};
use super::{rescued, Attempt, Model, Turn, TURNS};
use crate::sandbox::Sandbox;
use crate::tools::fs::walk_files;

/// Folders never read, ignored by git or not: dependencies, build output
/// and caches.
const SKIP_DIRS: &[&str] = &[
    "node_modules", "target", ".git", "dist", "build", "out", ".next", ".nuxt", ".svelte-kit", ".output", "coverage", "__pycache__", ".venv", "venv",
    "vendor", "bower_components", ".turbo", ".cache", ".parcel-cache", ".gradle", ".mypy_cache", ".pytest_cache", ".tox", "site-packages",
];
const LOCKFILES: &[&str] = &[
    "package-lock.json", "npm-shrinkwrap.json", "yarn.lock", "pnpm-lock.yaml", "bun.lockb", "cargo.lock", "poetry.lock", "pipfile.lock", "uv.lock", "composer.lock",
    "gemfile.lock", "go.sum", "flake.lock",
];
/// Kinds of file that hold keys, certificates and passwords.
const KEY_FILES: &[&str] = &["pem", "key", "p12", "pfx", "crt", "cer", "der", "jks", "keystore", "kdbx", "gpg", "asc", "ppk", "ovpn", "token", "env"];
/// Words in a file's name that mean it may hold a secret.
const SECRET_WORDS: &[&str] = &["secret", "password", "passwd", "credential", "apikey", "api_key", "api-key", "private_key", "private-key", "privatekey"];
/// Text that looks like a key or token: private keys, the usual service
/// key shapes, and a long quoted value given to something named like one.
pub(crate) static SECRET_TEXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"-----BEGIN [A-Z ]*PRIVATE KEY-----",
        r"|(?-u:\b)(?:sk|pk|rk)_(?:live|test)_[0-9A-Za-z]{16,}",
        r"|(?-u:\b)sk-[A-Za-z0-9_-]{20,}",
        r"|(?-u:\b)gh[pousr]_[A-Za-z0-9]{30,}|(?-u:\b)github_pat_[A-Za-z0-9_]{30,}",
        r"|(?-u:\b)AKIA[0-9A-Z]{16}(?-u:\b)",
        r"|(?-u:\b)xox[abprs]-[A-Za-z0-9-]{10,}",
        r"|(?-u:\b)AIza[0-9A-Za-z_-]{35}",
        r"|(?-u:\b)ya29\.[0-9A-Za-z_-]{20,}",
        r"|(?-u:\b)eyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}",
        r#"|(?i:(?-u:\b)(?:api[_-]?key|secret|token|passw(?:or)?d|client[_-]?secret|access[_-]?key)(?-u:\b)["']?\s*[:=]\s*["'][^"'\s]{12,}["'])"#,
        // The same without quotes, as in a .env file (CLIENT_SECRET=abc…):
        // a long run of key characters, not code (no dots or brackets).
        r"|(?i:(?-u:\b)[A-Z0-9_]*(?:api[_-]?key|secret|token|passw(?:or)?d|access[_-]?key|private[_-]?key)(?-u:\b)\s*[:=]\s*[A-Za-z0-9_+/=-]{16,}(?-u:\b))",
    ))
    .unwrap()
});
const MAX_FILE: u64 = 512 * 1024;
const MAX_TEXT: usize = 64 * 1024 * 1024;
const MAX_WALK: usize = 20_000;
/// Tool results stay small, so a whole try can fit a 512-token training
/// example: a few matches, a few lines. A match line with its path is about
/// 35 tokens; a listing of 12 files was 150.
const MATCHES: usize = 4;
const MATCH_CHARS: usize = 100;
const READ_LINES: usize = 12;
const LINE_CHARS: usize = 120;
const FOUND: usize = 5;
/// Tool rounds before it has to answer, and calls carried out a round.
const ROUNDS: usize = 6;
const CALLS: usize = 3;
/// The longest single reply.
const TURN: u32 = 512;
/// Stands for the model's turn when its template lays out what follows.
const MARK: &str = "@@sulcusai-turn@@";
/// Names too common to ask about: a call to `.parse(` or `.render(` may be
/// someone else's.
const COMMON: &[&str] = &[
    "parse", "write", "flush", "clone", "build", "start", "close", "apply", "reset", "fetch", "split", "insert", "remove", "update", "delete", "render", "format",
    "filter", "reduce", "extend", "append", "concat", "replace", "search", "match", "lines", "chars", "bytes", "count", "first", "values", "entries", "contains",
    "includes", "length", "spawn", "listen", "connect", "resolve", "reject", "catch", "finally", "tostring", "valueof", "default", "unwrap", "expect", "collect",
    "handle", "process", "execute", "invoke", "dispatch", "create", "store", "check", "verify", "validate", "encode", "decode", "serialize", "deserialize",
    "display", "describe", "print", "debug", "error", "label", "title", "status", "value", "index", "state", "event", "after", "before", "setup", "teardown",
    "constructor", "to_string", "to_owned", "is_empty", "from_str",
];
const JS_KEYWORDS: &[&str] = &["if", "for", "while", "switch", "catch", "function", "return", "with", "else", "do", "new", "typeof", "await", "yield", "super", "import", "export"];

static IDENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[A-Za-z_$][A-Za-z0-9_$]*").unwrap());
static RUST_FN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?-u:\b)fn\s+([A-Za-z_][A-Za-z0-9_]*)").unwrap());
static JS_FUNCTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?-u:\b)function(?-u:\b)\s*\*?\s*([A-Za-z_$][A-Za-z0-9_$]*)").unwrap());
static JS_CONST: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?-u:\b)(?:const|let|var)\s+([A-Za-z_$][A-Za-z0-9_$]*)\s*(?::[^=;]*?)?=\s*(?:async\s+)?").unwrap());
static JS_METHOD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^[ \t]*(?:(?:public|private|protected|static|async|override|readonly|abstract|get|set)[ \t]+)*\*?[ \t]*([A-Za-z_$][A-Za-z0-9_$]*)[ \t]*(?:<[^>\n]*>)?[ \t]*\(").unwrap()
});
static PY_DEF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^([ \t]*)(?:async[ \t]+)?def[ \t]+([A-Za-z_][A-Za-z0-9_]*)[ \t]*\(").unwrap());
static ANSWER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(?-u:\b)answer\s*:\s*(.+)$").unwrap());
static PATHISH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[A-Za-z0-9_./:@+\-]+\.[A-Za-z0-9]+").unwrap());

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lang {
    Rust,
    TypeScript,
    JavaScript,
    Python,
}

impl Lang {
    fn of(path: &Path) -> Option<Lang> {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "rs" => Some(Lang::Rust),
            "ts" | "tsx" | "mts" | "cts" => Some(Lang::TypeScript),
            "js" | "jsx" | "mjs" | "cjs" => Some(Lang::JavaScript),
            "py" => Some(Lang::Python),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Lang::Rust => "Rust",
            Lang::TypeScript => "TypeScript",
            Lang::JavaScript => "JavaScript",
            Lang::Python => "Python",
        }
    }

    fn js(self) -> bool {
        matches!(self, Lang::TypeScript | Lang::JavaScript)
    }
}

/// A file the tools may show.
struct File {
    path: PathBuf,
    /// As the file tools show it ("project/src/main.rs").
    shown: String,
    text: String,
    lang: Option<Lang>,
    /// No other file has its path inside its folder (two folders may).
    rel_alone: bool,
    /// No other file has its name.
    name_alone: bool,
}

/// A function as the scan read it.
#[derive(Debug, Clone, Default)]
struct Def {
    name: String,
    file: usize,
    /// Its name's line, from 1.
    line: usize,
    /// Lines from its name to the end of its parameters.
    sig_lines: usize,
    /// Its parameters' names in order, `self` left out; none when one isn't
    /// a plain name (a destructured object, a tuple pattern).
    params: Option<Vec<String>>,
    has_self: bool,
    /// Its body's first and last lines; none for a declaration.
    body: Option<(usize, usize)>,
}

/// What a task asks for.
#[derive(Clone)]
enum Pick {
    Def(usize),
    /// A function and the functions that call it.
    Callers(usize, Vec<usize>),
}

/// The person's folders, scanned: the files the tools may show and the
/// functions in them.
pub struct Index {
    sandbox: Sandbox,
    files: Vec<File>,
    by_path: HashMap<String, usize>,
    defs: Vec<Def>,
    /// Every function name in the code.
    names: HashSet<String>,
    /// The tasks each level can ask.
    pools: [Vec<Pick>; tasks::LEVELS as usize],
    /// Files left out because they may hold a secret, as the tools would
    /// show them.
    secret: Vec<String>,
    tally: Tally,
}

/// How the tries went since it was last read, for the log.
#[derive(Default)]
struct Tally {
    tries: AtomicU32,
    /// Answered after looking.
    answered: AtomicU32,
    /// Answered without looking.
    guessed: AtomicU32,
    /// Ran out of room (in practice: past the 512-token training window).
    no_room: AtomicU32,
    /// Never answered, or wrote a tool call that couldn't be read.
    lost: AtomicU32,
    calls: AtomicU32,
    /// The longest request, in tokens.
    request: AtomicU32,
}

fn key(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        s.to_lowercase()
    } else {
        s
    }
}

/// A name that says the file may hold a password, key or token.
fn secret_name(name: &str) -> bool {
    let n = name.to_lowercase();
    let (stem, ext) = match n.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s, e),
        _ => (n.as_str(), ""),
    };
    n == ".env"
        || n.starts_with(".env.")
        || KEY_FILES.contains(&ext)
        || ["id_rsa", "id_dsa", "id_ecdsa", "id_ed25519"].iter().any(|k| n.starts_with(k))
        || [".npmrc", ".pypirc", ".netrc", ".git-credentials", ".htpasswd", ".pgpass"].contains(&n.as_str())
        || SECRET_WORDS.iter().any(|w| stem.contains(w))
        || matches!(stem, "token" | "tokens")
        || stem.ends_with("_token")
        || stem.ends_with("-token")
}

/// Dependencies, build output or a lockfile.
fn skipped(rel: &Path) -> bool {
    let name = rel.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    rel.components().any(|c| SKIP_DIRS.contains(&c.as_os_str().to_string_lossy().to_lowercase().as_str())) || LOCKFILES.contains(&name.as_str()) || name.ends_with(".lock")
}

fn short(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

impl Index {
    /// Reads the folders. Run off the async threads: a big project takes a
    /// few seconds.
    pub fn scan(folders: &[String]) -> Result<Index, String> {
        let sandbox = Sandbox::new(folders);
        if sandbox.is_empty() {
            return Err("Those folders can't be found.".into());
        }
        let mut files = Vec::new();
        let (mut secret, mut total) = (Vec::new(), 0);
        for path in walk_files(sandbox.roots(), None, MAX_WALK)? {
            let rel = sandbox.roots().iter().find_map(|r| path.strip_prefix(r).ok()).unwrap_or(path.as_path());
            if skipped(rel) {
                continue;
            }
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            if secret_name(&name) {
                secret.push(sandbox.display(&path));
                continue;
            }
            if std::fs::metadata(&path).map_or(true, |m| m.len() > MAX_FILE) {
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else { continue };
            if bytes.iter().take(8192).any(|&b| b == 0) {
                continue;
            }
            let text = String::from_utf8_lossy(&bytes).into_owned();
            if SECRET_TEXT.is_match(&text) {
                secret.push(sandbox.display(&path));
                continue;
            }
            let lang = Lang::of(&path);
            // Minified or generated bundles aren't anyone's code to look through.
            if lang.is_some() && (name.contains(".min.") || text.lines().any(|l| l.len() > 3000)) {
                continue;
            }
            total += text.len();
            if total > MAX_TEXT {
                break;
            }
            files.push(File { shown: sandbox.display(&path), path, text, lang, rel_alone: true, name_alone: true });
        }
        Ok(Index::build(sandbox, files, secret))
    }

    fn build(sandbox: Sandbox, mut files: Vec<File>, secret: Vec<String>) -> Index {
        // In path order, so the same folders make the same test.
        files.sort_by(|a, b| a.shown.cmp(&b.shown));
        let rel = |f: &File| f.shown.split_once('/').map_or(f.shown.to_lowercase(), |(_, r)| r.to_lowercase());
        let base = |f: &File| f.shown.rsplit('/').next().unwrap_or("").to_lowercase();
        let mut rels: HashMap<String, usize> = HashMap::new();
        let mut bases: HashMap<String, usize> = HashMap::new();
        for f in &files {
            *rels.entry(rel(f)).or_default() += 1;
            *bases.entry(base(f)).or_default() += 1;
        }
        for f in &mut files {
            f.rel_alone = rels[&rel(f)] == 1;
            f.name_alone = bases[&base(f)] == 1;
        }

        let mut defs = Vec::new();
        let mut codes: Vec<Option<String>> = Vec::with_capacity(files.len());
        for (i, f) in files.iter().enumerate() {
            let Some(lang) = f.lang else {
                codes.push(None);
                continue;
            };
            let code = blank(&f.text, lang);
            defs.extend(parse(&code, lang).into_iter().map(|d| Def { file: i, ..d }));
            codes.push(Some(code));
        }
        let names: HashSet<String> = defs.iter().map(|d| d.name.clone()).collect();
        let pools = pools(&files, &defs, &codes, &names);
        let by_path = files.iter().enumerate().map(|(i, f)| (key(&f.path), i)).collect();
        Index { sandbox, files, by_path, defs, names, pools, secret, tally: Tally::default() }
    }

    /// "Tries at your code: 60; 41 answered after looking, …", counted since
    /// the last time it was asked.
    pub fn tally(&self) -> String {
        let t = &self.tally;
        let take = |n: &AtomicU32| n.swap(0, Ordering::Relaxed);
        format!(
            "Tries at your code: {}; {} answered after looking, {} without looking, {} ran out of room, {} never answered; {} tool calls; requests up to {} tokens",
            take(&t.tries),
            take(&t.answered),
            take(&t.guessed),
            take(&t.no_room),
            take(&t.lost),
            take(&t.calls),
            take(&t.request)
        )
    }

    /// Enough to make a test and practice from.
    pub fn usable(&self) -> bool {
        self.pools[0].len() + self.pools[1].len() >= 4 && self.pools.iter().map(Vec::len).sum::<usize>() >= 12
    }

    /// "412 files, 3,050 functions; 3 files left out because they may hold
    /// passwords or keys".
    pub fn summary(&self) -> String {
        let code = self.files.iter().filter(|f| f.lang.is_some()).count();
        let mut s = format!("{code} files with code, {} functions", self.defs.len());
        let n = self.secret.len();
        if n > 0 {
            let _ = write!(s, "; {n} file{} left out because {} may hold passwords or keys", if n == 1 { "" } else { "s" }, if n == 1 { "it" } else { "they" });
        }
        s
    }

    fn folders(&self) -> String {
        let names: Vec<String> = self.sandbox.roots().iter().map(|r| r.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| r.display().to_string())).collect();
        match names.as_slice() {
            [one] => format!("the shared folder {one}"),
            [rest @ .., last] => format!("the shared folders {} and {last}", rest.join(", ")),
            [] => "the shared folders".into(),
        }
    }

    fn visible(&self, p: &Path) -> Option<usize> {
        self.by_path.get(&key(p)).copied()
    }

    /// A file tool, run the way chats run it, but read only and in small
    /// pieces. The text the model gets back.
    pub(super) fn run(&self, name: &str, args: &Value) -> String {
        let result = match name {
            "search_files" => self.search(args),
            "find_files" => self.find(args),
            "read_file" => self.read(args),
            other => Err(format!("There's no tool called {other} here. Use search_files, find_files or read_file.")),
        };
        result.unwrap_or_else(|e| format!("Error: {e}"))
    }

    fn roots(&self, path: Option<&str>) -> Result<Vec<PathBuf>, String> {
        match path.map(str::trim).filter(|p| !p.is_empty()) {
            Some(p) => Ok(vec![self.sandbox.resolve(p)?]),
            None => Ok(self.sandbox.roots().to_vec()),
        }
    }

    /// The files it may show under these roots, matching the glob.
    fn listed(&self, roots: &[PathBuf], glob: Option<&str>) -> Result<Vec<usize>, String> {
        if roots.len() == 1 && roots[0].is_file() {
            return Ok(self.visible(&roots[0]).into_iter().collect());
        }
        Ok(walk_files(roots, glob.filter(|g| !g.trim().is_empty()), MAX_WALK)?.iter().filter_map(|p| self.visible(p)).collect())
    }

    fn search(&self, args: &Value) -> Result<String, String> {
        let pattern = text_arg(args, "pattern").ok_or("search_files needs a pattern.")?;
        let ignore_case = args.get("ignore_case").and_then(Value::as_bool).unwrap_or(false);
        let re = regex::RegexBuilder::new(pattern)
            .case_insensitive(ignore_case)
            .size_limit(1 << 20)
            .build()
            .map_err(|e| format!("That pattern isn't a valid regular expression: {e}"))?;
        let files = self.listed(&self.roots(text_arg(args, "path"))?, text_arg(args, "glob"))?;
        let mut out = String::new();
        let mut hits = 0;
        'files: for f in files {
            for (i, line) in self.files[f].text.lines().enumerate() {
                if re.is_match(line) {
                    if hits == MATCHES {
                        let _ = writeln!(out, "… stopped at {MATCHES} matches. Narrow the pattern or path.");
                        break 'files;
                    }
                    let _ = writeln!(out, "{}:{}: {}", self.files[f].shown, i + 1, short(line.trim(), MATCH_CHARS));
                    hits += 1;
                }
            }
        }
        Ok(if hits == 0 { format!("No matches for {pattern}.") } else { out })
    }

    fn find(&self, args: &Value) -> Result<String, String> {
        let pattern = text_arg(args, "pattern").ok_or("find_files needs a pattern.")?;
        let found = self.listed(&self.roots(text_arg(args, "path"))?, Some(pattern))?;
        if found.is_empty() {
            return Ok(format!("No files match {pattern}."));
        }
        let mut lines: Vec<String> = found.iter().take(FOUND).map(|f| self.files[*f].shown.clone()).collect();
        if found.len() > FOUND {
            lines.push(format!("… {} more. Narrow the pattern.", found.len() - FOUND));
        }
        Ok(lines.join("\n"))
    }

    fn read(&self, args: &Value) -> Result<String, String> {
        let asked = text_arg(args, "path").ok_or("read_file needs a path.")?;
        let path = self.sandbox.resolve(asked)?;
        if path.is_dir() {
            return Err("That's a folder. Use find_files to see what's in it.".into());
        }
        let shown = self.sandbox.display(&path);
        let Some(f) = self.visible(&path) else {
            return Err(if path.exists() {
                format!("{shown} is left out of practice: it's build output or a lockfile, or it may hold passwords or keys.")
            } else {
                format!("{shown} doesn't exist.")
            });
        };
        let lines: Vec<&str> = self.files[f].text.lines().collect();
        if lines.is_empty() {
            return Ok("(empty file)".into());
        }
        let start = num_arg(args, "start_line").map_or(1, |n| n.max(1));
        if start > lines.len() {
            return Err(format!("The file has {} lines, so there's nothing from line {start} on.", lines.len()));
        }
        let last = start + READ_LINES - 1;
        let end = num_arg(args, "end_line").map_or(last, |n| n.max(start)).min(last).min(lines.len());
        let mut out = String::new();
        for (i, line) in lines.iter().enumerate().take(end).skip(start - 1) {
            let _ = writeln!(out, "{:>5}\t{}", i + 1, short(line, LINE_CHARS));
        }
        if end < lines.len() {
            let _ = writeln!(out, "… {} more lines. Use start_line={} to continue.", lines.len() - end, end + 1);
        }
        Ok(out)
    }

    /// The answer names this file: its path as shown, inside its folder, in
    /// full, or by name alone when no other file has that name.
    fn names_file(&self, answer: &str, f: usize) -> bool {
        let file = &self.files[f];
        let shown = file.shown.to_lowercase();
        let rel = shown.split_once('/').map_or(shown.clone(), |(_, r)| r.to_string());
        let name = shown.rsplit('/').next().unwrap_or("").to_string();
        let answer = answer.replace('\\', "/");
        PATHISH.find_iter(&answer).any(|m| {
            let p = m.as_str().trim_start_matches("./").trim_end_matches(['.', ':']).to_lowercase();
            p == shown || p.ends_with(&format!("/{shown}")) || (file.rel_alone && (p == rel || p.ends_with(&format!("/{rel}")))) || (file.name_alone && p == name)
        })
    }
}

fn text_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

fn num_arg(args: &Value, key: &str) -> Option<usize> {
    match args.get(key)? {
        Value::Number(n) => n.as_u64().map(|n| n as usize),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

// ---------- reading code ----------

/// Blanks `out[from..to]`, keeping line breaks.
fn wipe(out: &mut [u8], from: usize, to: usize) {
    let to = to.min(out.len());
    for x in &mut out[from.min(to)..to] {
        if *x != b'\n' && *x != b'\r' {
            *x = b' ';
        }
    }
}

/// Where a string that opened before `from` closes: its closing quote (or
/// the end of the line, or of the text, if it doesn't close).
fn close_quote(b: &[u8], from: usize, q: u8, one_line: bool) -> usize {
    let mut j = from;
    while j < b.len() {
        match b[j] {
            b'\\' => j += 2,
            x if x == q => return j,
            b'\n' if one_line => return j,
            _ => j += 1,
        }
    }
    b.len()
}

fn find_from(b: &[u8], from: usize, what: &[u8]) -> Option<usize> {
    b.get(from..)?.windows(what.len()).position(|w| w == what).map(|p| p + from)
}

fn ident_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'$'
}

/// The code with comments and the insides of strings blanked out (same
/// lines and columns), so names in them aren't taken for code.
fn blank(text: &str, lang: Lang) -> String {
    let b = text.as_bytes();
    let n = b.len();
    let mut out = b.to_vec();
    let mut i = 0;
    while i < n {
        let c = b[i];
        let next = b.get(i + 1).copied().unwrap_or(0);
        if lang == Lang::Python {
            if c == b'#' {
                let end = find_from(b, i, b"\n").unwrap_or(n);
                wipe(&mut out, i, end);
                i = end;
            } else if c == b'"' || c == b'\'' {
                if b.get(i..i + 3) == Some(&[c, c, c][..]) {
                    let close = find_from(b, i + 3, &[c, c, c]).unwrap_or(n);
                    wipe(&mut out, i + 3, close);
                    i = close + 3;
                } else {
                    let close = close_quote(b, i + 1, c, true);
                    wipe(&mut out, i + 1, close);
                    i = close + 1;
                }
            } else {
                i += 1;
            }
            continue;
        }
        if c == b'/' && next == b'/' {
            let end = find_from(b, i, b"\n").unwrap_or(n);
            wipe(&mut out, i, end);
            i = end;
        } else if c == b'/' && next == b'*' {
            // Rust's block comments nest.
            let (mut depth, mut j) = (1, i + 2);
            while j < n && depth > 0 {
                if b[j] == b'*' && b.get(j + 1) == Some(&b'/') {
                    depth -= 1;
                    j += 2;
                } else if lang == Lang::Rust && b[j] == b'/' && b.get(j + 1) == Some(&b'*') {
                    depth += 1;
                    j += 2;
                } else {
                    j += 1;
                }
            }
            wipe(&mut out, i, j);
            i = j;
        } else if c == b'"' || (lang.js() && (c == b'\'' || c == b'`')) {
            let close = close_quote(b, i + 1, c, lang.js() && c != b'`');
            wipe(&mut out, i + 1, close);
            i = close + 1;
        } else if lang == Lang::Rust && c == b'r' && (next == b'"' || next == b'#') && (i == 0 || !ident_byte(b[i - 1])) {
            let hashes = b[i + 1..].iter().take_while(|&&x| x == b'#').count();
            if b.get(i + 1 + hashes) != Some(&b'"') {
                i += 1;
                continue;
            }
            let mut end_mark = vec![b'"'];
            end_mark.extend(std::iter::repeat_n(b'#', hashes));
            let open = i + 2 + hashes;
            let close = find_from(b, open, &end_mark).unwrap_or(n);
            wipe(&mut out, open, close);
            i = close + end_mark.len();
        } else if lang == Lang::Rust && c == b'\'' {
            // A character ('a', '\n', 'é'), or else a lifetime ('a).
            let len = text.get(i + 1..).and_then(|s| s.chars().next()).map_or(1, char::len_utf8);
            if next == b'\\' {
                let close = find_from(b, i + 2, b"'").filter(|j| j - i < 12).unwrap_or(i + 1);
                wipe(&mut out, i + 1, close);
                i = close + 1;
            } else if b.get(i + 1 + len) == Some(&b'\'') {
                wipe(&mut out, i + 1, i + 1 + len);
                i += 2 + len;
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    String::from_utf8(out).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

fn line_starts(s: &str) -> Vec<usize> {
    std::iter::once(0).chain(s.bytes().enumerate().filter(|(_, c)| *c == b'\n').map(|(i, _)| i + 1)).collect()
}

/// The line (from 1) a byte is on.
fn line_of(starts: &[usize], pos: usize) -> usize {
    starts.partition_point(|s| *s <= pos)
}

fn skip_ws(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && b[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}

/// The bracket closing the one at `open`.
fn closing(b: &[u8], open: usize, o: u8, c: u8) -> Option<usize> {
    let mut depth = 0usize;
    for (j, x) in b.iter().enumerate().skip(open) {
        if *x == o {
            depth += 1;
        } else if *x == c {
            depth -= 1;
            if depth == 0 {
                return Some(j);
            }
        }
    }
    None
}

/// Just past the `>` closing the generics that open at `i` (an arrow's
/// `->` or `=>` doesn't close them).
fn skip_angles(b: &[u8], i: usize) -> Option<usize> {
    let mut depth = 0;
    let mut j = i;
    while j < b.len() && j < i + 600 {
        match b[j] {
            b'-' | b'=' if b.get(j + 1) == Some(&b'>') => j += 1,
            b'<' => depth += 1,
            b'>' => {
                depth -= 1;
                if depth == 0 {
                    return Some(j + 1);
                }
            }
            b'{' | b';' => return None,
            _ => {}
        }
        j += 1;
    }
    None
}

/// After a parameter list: the body's opening and closing braces, or none
/// for a declaration (`;` comes first). A return type in between may hold
/// brackets and generics.
fn body_after(b: &[u8], from: usize) -> Option<(usize, usize)> {
    let mut depth = 0i32;
    let mut j = from;
    while j < b.len() && j < from + 2000 {
        match b[j] {
            b'-' | b'=' if b.get(j + 1) == Some(&b'>') => j += 1,
            b'(' | b'[' | b'<' => depth += 1,
            b')' | b']' | b'>' => depth -= 1,
            b'{' if depth <= 0 => return closing(b, j, b'{', b'}').map(|end| (j, end)),
            b'{' => {
                j = closing(b, j, b'{', b'}')?;
            }
            b';' | b'}' if depth <= 0 => return None,
            _ => {}
        }
        j += 1;
    }
    None
}

/// Splits on commas outside brackets and generics.
fn split_top(s: &str) -> Vec<&str> {
    let b = s.as_bytes();
    let (mut parts, mut depth, mut from) = (Vec::new(), 0i32, 0);
    let mut j = 0;
    while j < b.len() {
        match b[j] {
            b'-' | b'=' if b.get(j + 1) == Some(&b'>') => j += 1,
            b'(' | b'[' | b'{' | b'<' => depth += 1,
            b')' | b']' | b'}' | b'>' => depth -= 1,
            b',' if depth == 0 => {
                parts.push(&s[from..j]);
                from = j + 1;
            }
            _ => {}
        }
        j += 1;
    }
    parts.push(&s[from..]);
    parts.into_iter().map(str::trim).filter(|p| !p.is_empty()).collect()
}

fn plain_ident(s: &str) -> bool {
    IDENT.find(s).is_some_and(|m| m.start() == 0 && m.end() == s.len()) && s != "_"
}

static RUST_SELF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^&?\s*(?:'[A-Za-z_]+\s+)?(?:mut\s+)?self(?-u:\b)").unwrap());
static RUST_ATTR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"#\[[^\]]*\]\s*").unwrap());

/// Parameter names, and whether the first is the value it's called on
/// (`self`, `this`, `cls`).
fn params_of(inner: &str, lang: Lang) -> (Option<Vec<String>>, bool) {
    let (mut names, mut has_self, mut plain) = (Vec::new(), false, true);
    for (i, p) in split_top(inner).into_iter().enumerate() {
        let name = match lang {
            Lang::Rust => {
                let p = RUST_ATTR.replace_all(p, "");
                if i == 0 && RUST_SELF.is_match(&p) {
                    has_self = true;
                    continue;
                }
                let head = p.split(':').next().unwrap_or("").trim();
                head.trim_start_matches("mut ").trim_start_matches("ref ").trim().to_string()
            }
            Lang::Python => {
                if p == "*" || p == "/" {
                    continue;
                }
                let p = p.trim_start_matches('*');
                let name = p.split([':', '=']).next().unwrap_or("").trim().to_string();
                if i == 0 && (name == "self" || name == "cls") {
                    has_self = true;
                    continue;
                }
                name
            }
            _ => {
                let mut p = p.trim_start_matches("...").trim();
                while let Some(rest) = ["public ", "private ", "protected ", "readonly ", "override "].iter().find_map(|m| p.strip_prefix(m)) {
                    p = rest.trim_start();
                }
                let name = p.split(['?', ':', '=']).next().unwrap_or("").trim().to_string();
                if i == 0 && name == "this" {
                    has_self = true;
                    continue;
                }
                name
            }
        };
        if plain_ident(&name) {
            names.push(name);
        } else {
            plain = false;
        }
    }
    (plain.then_some(names), has_self)
}

/// Adds a function, once for each name and line.
fn add(out: &mut Vec<Def>, def: Def) {
    if !out.iter().any(|d| d.line == def.line && d.name == def.name) {
        out.push(def);
    }
}

/// The functions in a file's code (comments and strings blanked).
fn parse(code: &str, lang: Lang) -> Vec<Def> {
    let b = code.as_bytes();
    let starts = line_starts(code);
    let mut out: Vec<Def> = Vec::new();
    // Byte positions to lines.
    let span = |r: Option<(usize, usize)>| r.map(|(a, z)| (line_of(&starts, a), line_of(&starts, z)));
    // A function named at `at`, its parameters between `open` and `close`.
    let make = |name: &str, at: usize, open: usize, close: usize, body: Option<(usize, usize)>| {
        let line = line_of(&starts, at);
        let (params, has_self) = params_of(&code[open + 1..close], lang);
        Def { name: name.to_string(), line, sig_lines: line_of(&starts, close) - line + 1, params, has_self, body, ..Def::default() }
    };
    // The parameter list starting at `i`, after any generics.
    let params_at = |i: usize| -> Option<(usize, usize)> {
        let mut i = skip_ws(b, i);
        if b.get(i) == Some(&b'<') {
            i = skip_ws(b, skip_angles(b, i)?);
        }
        (b.get(i) == Some(&b'(')).then_some(())?;
        Some((i, closing(b, i, b'(', b')')?))
    };
    match lang {
        Lang::Rust => {
            for c in RUST_FN.captures_iter(code) {
                let name = c.get(1).unwrap();
                if let Some((open, close)) = params_at(name.end()) {
                    add(&mut out, make(name.as_str(), name.start(), open, close, span(body_after(b, close + 1))));
                }
            }
        }
        Lang::Python => {
            let lines: Vec<&str> = code.lines().collect();
            let indent = |l: &str| l.len() - l.trim_start().len();
            for c in PY_DEF.captures_iter(code) {
                let (lead, name) = (c.get(1).unwrap().as_str().len(), c.get(2).unwrap());
                let open = c.get(0).unwrap().end() - 1;
                let Some(close) = closing(b, open, b'(', b')') else { continue };
                let Some(colon) = (close + 1..b.len().min(close + 400)).find(|j| b[*j] == b':') else { continue };
                let line = line_of(&starts, colon);
                let rest = code[colon + 1..].lines().next().unwrap_or("").trim();
                let body = if !rest.is_empty() {
                    Some((line, line))
                } else {
                    let mut last = None;
                    for (k, l) in lines.iter().enumerate().skip(line) {
                        if l.trim().is_empty() {
                            continue;
                        }
                        if indent(l) <= lead {
                            break;
                        }
                        last = Some(k + 1);
                    }
                    last.map(|z| (line + 1, z))
                };
                add(&mut out, make(name.as_str(), name.start(), open, close, body));
            }
        }
        Lang::TypeScript | Lang::JavaScript => {
            // A body right after the parameters (or a return type).
            let braced = |close: usize| -> Option<(usize, usize)> {
                let (open, end) = body_after(b, close + 1)?;
                let between = code[close + 1..open].trim();
                (between.is_empty() || between.starts_with(':')).then_some((open, end))
            };
            for c in JS_FUNCTION.captures_iter(code) {
                let name = c.get(1).unwrap();
                if let Some((open, close)) = params_at(name.end()) {
                    add(&mut out, make(name.as_str(), name.start(), open, close, span(braced(close))));
                }
            }
            for c in JS_CONST.captures_iter(code) {
                let name = c.get(1).unwrap();
                let mut i = c.get(0).unwrap().end();
                if code[i..].starts_with("function") {
                    i = skip_ws(b, i + "function".len());
                    if b.get(i) == Some(&b'*') {
                        i += 1;
                    }
                    i = skip_ws(b, i);
                    while i < b.len() && ident_byte(b[i]) {
                        i += 1;
                    }
                    if let Some((open, close)) = params_at(i) {
                        add(&mut out, make(name.as_str(), name.start(), open, close, span(braced(close))));
                    }
                    continue;
                }
                if let Some((open, close)) = params_at(i) {
                    if let Some(body) = arrow_body(b, close + 1) {
                        add(&mut out, make(name.as_str(), name.start(), open, close, span(Some(body))));
                    }
                } else if let Some(m) = IDENT.find_at(code, i).filter(|m| m.start() == i) {
                    // One parameter without brackets: `x => …`.
                    let j = skip_ws(b, m.end());
                    if code[j..].starts_with("=>") {
                        if let Some(body) = arrow_body(b, j) {
                            let line = line_of(&starts, name.start());
                            add(&mut out, Def { name: name.as_str().to_string(), line, sig_lines: 1, params: Some(vec![m.as_str().to_string()]), body: span(Some(body)), ..Def::default() });
                        }
                    }
                }
            }
            for c in JS_METHOD.captures_iter(code) {
                let name = c.get(1).unwrap();
                if JS_KEYWORDS.contains(&name.as_str()) {
                    continue;
                }
                let open = c.get(0).unwrap().end() - 1;
                let Some(close) = closing(b, open, b'(', b')') else { continue };
                // A method has a body right after; a call at the start of a line doesn't.
                if let Some(body) = braced(close) {
                    add(&mut out, make(name.as_str(), name.start(), open, close, span(Some(body))));
                }
            }
        }
    }
    out.sort_by_key(|d| d.line);
    out
}

/// An arrow function's body, from just past its parameters: braces, or an
/// expression running to the end of the statement.
fn arrow_body(b: &[u8], from: usize) -> Option<(usize, usize)> {
    // Past any return type, to the arrow.
    let mut depth = 0i32;
    let mut j = from;
    let arrow = loop {
        match b.get(j)? {
            b'=' if b.get(j + 1) == Some(&b'>') && depth == 0 => break j,
            b'-' | b'=' if b.get(j + 1) == Some(&b'>') => j += 1,
            b'(' | b'[' | b'{' | b'<' => depth += 1,
            b')' | b']' | b'}' | b'>' => depth -= 1,
            b';' => return None,
            _ => {}
        }
        if j > from + 400 {
            return None;
        }
        j += 1;
    };
    let start = skip_ws(b, arrow + 2);
    if b.get(start) == Some(&b'{') {
        return closing(b, start, b'{', b'}').map(|end| (start, end));
    }
    let mut depth = 0i32;
    let mut j = start;
    while j < b.len() {
        match b[j] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' if depth == 0 => break,
            b')' | b']' | b'}' => depth -= 1,
            b';' | b',' | b'\n' if depth == 0 => break,
            _ => {}
        }
        j += 1;
    }
    Some((start, j.saturating_sub(1).max(start)))
}

/// The kinds of task each level asks, from the scan:
/// 1. which file defines a function few files mention;
/// 2. the same for one many files mention (a plain search finds too much);
/// 3. its parameters, written on one line;
/// 4. its parameters, written over several lines (only reading shows them);
/// 5. the one function that calls it (reading shows which);
/// 6. all two or three functions that call it.
fn pools(files: &[File], defs: &[Def], codes: &[Option<String>], names: &HashSet<String>) -> [Vec<Pick>; tasks::LEVELS as usize] {
    let mut defined: HashMap<&str, usize> = HashMap::new();
    for d in defs {
        *defined.entry(d.name.as_str()).or_default() += 1;
    }
    // Files that mention each function's name, in code or not.
    let mut mentions: HashMap<&str, usize> = HashMap::new();
    for f in files {
        let mut seen = HashSet::new();
        for m in IDENT.find_iter(&f.text) {
            if let Some(n) = names.get(m.as_str()) {
                if seen.insert(n.as_str()) {
                    *mentions.entry(n.as_str()).or_default() += 1;
                }
            }
        }
    }
    // Where each is called: its name and then "(", in code, not where it's defined.
    let def_at: HashSet<(usize, usize, &str)> = defs.iter().map(|d| (d.file, d.line, d.name.as_str())).collect();
    let mut calls: HashMap<&str, Vec<(usize, usize)>> = HashMap::new();
    for (f, code) in codes.iter().enumerate() {
        let Some(code) = code else { continue };
        let (b, starts) = (code.as_bytes(), line_starts(code));
        for m in IDENT.find_iter(code) {
            let Some(n) = names.get(m.as_str()) else { continue };
            if b.get(skip_ws(b, m.end())) != Some(&b'(') {
                continue;
            }
            let line = line_of(&starts, m.start());
            if !def_at.contains(&(f, line, n.as_str())) {
                calls.entry(n.as_str()).or_default().push((f, line));
            }
        }
    }
    let mut by_file: Vec<Vec<usize>> = vec![Vec::new(); files.len()];
    for (i, d) in defs.iter().enumerate() {
        by_file[d.file].push(i);
    }
    // The innermost function whose body holds this line.
    let holder = |file: usize, line: usize, not: Option<usize>| -> Option<usize> {
        by_file[file]
            .iter()
            .copied()
            .filter(|i| Some(*i) != not && defs[*i].body.is_some_and(|(a, z)| a <= line && line <= z))
            .min_by_key(|i| defs[*i].body.map_or(0, |(a, z)| z - a))
    };
    let alone = |i: usize| defined[defs[i].name.as_str()] == 1;
    let good = |i: usize| {
        let n = &defs[i].name;
        alone(i) && defs[i].body.is_some() && n.chars().count() >= 5 && n.chars().any(|c| c.is_ascii_lowercase()) && !COMMON.contains(&n.to_lowercase().as_str())
    };
    let line_text = |i: usize| files[defs[i].file].text.lines().nth(defs[i].line - 1).unwrap_or("").trim().chars().count();

    let mut pools: [Vec<Pick>; tasks::LEVELS as usize] = Default::default();
    for (i, d) in defs.iter().enumerate() {
        if !good(i) {
            continue;
        }
        let mentioned = mentions.get(d.name.as_str()).copied().unwrap_or(0);
        pools[if mentioned <= 2 { 0 } else { 1 }].push(Pick::Def(i));
        if d.params.as_ref().is_some_and(|p| (1..=5).contains(&p.len())) && !d.has_self {
            if d.sig_lines == 1 && line_text(i) <= MATCH_CHARS {
                pools[2].push(Pick::Def(i));
            } else if d.sig_lines >= 2 {
                pools[3].push(Pick::Def(i));
            }
        }
        let sites = calls.get(d.name.as_str()).map_or(&[][..], Vec::as_slice);
        if sites.is_empty() || sites.len() > MATCHES {
            continue;
        }
        let held: Option<Vec<usize>> = sites.iter().map(|(f, l)| holder(*f, *l, None)).collect();
        let Some(mut callers) = held else { continue };
        callers.sort_unstable();
        callers.dedup();
        // Not recursive, and each caller is one of a kind and not inside another function.
        if callers.contains(&i) || !callers.iter().all(|c| alone(*c) && holder(defs[*c].file, defs[*c].line, Some(*c)).is_none()) {
            continue;
        }
        match callers.len() {
            1 if sites.iter().any(|(_, l)| *l != defs[callers[0]].line) => pools[4].push(Pick::Callers(i, callers)),
            2 | 3 => pools[5].push(Pick::Callers(i, callers)),
            _ => {}
        }
    }
    pools
}

// ---------- tasks ----------

/// The answer a task wants, checked against the scan.
enum Key {
    File(usize),
    Params(Vec<String>),
    Callers { of: String, want: Vec<String> },
}

/// A task's check: the answer, and the scanned folders it's checked against
/// (and its tool calls carried out in).
pub struct Look {
    key: Key,
    code: Arc<Index>,
}

/// The tools the model gets: the chat file tools, read only, cut down to
/// their names and parameters (search_files says it takes a regex). With
/// Qwen3 4B 2507's template, a request with the chat descriptions takes
/// 430 of a training example's 512 tokens; with a few words each and the
/// required lists, 350; like this, about 300. Right tries on his repo took
/// 170 to 240 tokens after the request. Shorter still cost answers: with
/// every description gone and the wording cut too, it got 1 of 12 test tasks
/// right instead of 5.
pub(super) fn definitions() -> Value {
    let f = |name: &str, properties: Value| json!({ "type": "function", "function": { "name": name, "parameters": { "type": "object", "properties": properties } } });
    let (text, number) = (json!({ "type": "string" }), json!({ "type": "integer" }));
    let mut search = f("search_files", json!({ "pattern": text, "glob": text }));
    search["function"]["description"] = json!("Regex search of file contents.");
    json!([search, f("find_files", json!({ "pattern": text })), f("read_file", json!({ "path": text, "start_line": number, "end_line": number }))])
}

/// A task about the person's code at this level (or the nearest easier one
/// the scan has tasks for).
pub fn task(code: &Arc<Index>, level: u32, rng: &mut Rng) -> Task {
    let level = level.clamp(1, tasks::LEVELS);
    let kind = (1..=level).rev().chain(level + 1..=tasks::LEVELS).find(|l| !code.pools[*l as usize - 1].is_empty()).unwrap_or(1);
    let pool = &code.pools[kind as usize - 1];
    let pick = if pool.is_empty() { Pick::Def(0) } else { rng.pick(pool).clone() };
    let lang = |d: usize| code.files[code.defs[d].file].lang.map_or("", Lang::name);
    // Worded briefly: the request and a whole try must fit 512 tokens.
    let (question, what, key) = match (kind, pick) {
        (1 | 2, Pick::Def(d)) => (format!("Which file in my code defines the {} function `{}`?", lang(d), code.defs[d].name), "path", Key::File(code.defs[d].file)),
        (_, Pick::Def(d)) => (
            format!("What parameters does the {} function `{}` in my code take?", lang(d), code.defs[d].name),
            "names in order, comma-separated",
            Key::Params(code.defs[d].params.clone().unwrap_or_default()),
        ),
        (_, Pick::Callers(y, callers)) => {
            let of = code.defs[y].name.clone();
            let want: Vec<String> = callers.iter().map(|c| code.defs[*c].name.clone()).collect();
            if want.len() == 1 {
                (format!("Which function in my code calls the {} function `{of}`? Only one does.", lang(y)), "name", Key::Callers { of, want })
            } else {
                (format!("Which functions in my code call the {} function `{of}`? There are {}.", lang(y), want.len()), "names, comma-separated", Key::Callers { of, want })
            }
        }
    };
    // "In my code" and "don't guess" cost 7 tokens and are worth them: without
    // them, Qwen3 4B 2507 got 1 of the 12 test tasks right instead of 5.
    let user = format!("{question} My code is in {}. Look it up, don't guess. End with: Answer: <{what}>", code.folders());
    Task { skill: Skill::Project, level, user, tools: Some(definitions()), check: Check::Look(Look { key, code: code.clone() }) }
}

/// The last "Answer: …" line's answer, with markdown's bold and code marks
/// taken off (underscores stay: they're part of names).
fn answer_of(text: &str) -> Option<String> {
    let line = text.lines().rev().map(|l| l.replace("**", "").replace('`', "")).find(|l| ANSWER.is_match(l))?;
    let a = ANSWER.captures(&line)?[1].trim().trim_matches(|c: char| matches!(c, '"' | '\'' | '“' | '”' | ' ')).trim_end_matches('.').to_string();
    (!a.is_empty()).then_some(a)
}

/// Parameter names as an answer lists them ("path, strict", "path: &Path,
/// strict: bool", "(path, strict)", "path and strict").
fn names_listed(answer: &str) -> Vec<String> {
    let a = answer.trim();
    let a = a.strip_prefix('(').and_then(|x| x.strip_suffix(')')).unwrap_or(a);
    let mut out = Vec::new();
    for part in split_top(a) {
        for piece in part.split(" and ") {
            let piece = piece.trim().trim_start_matches("and ").trim_start_matches("or ").trim_start_matches(['&', '*', '.']).trim_start_matches("mut ");
            if let Some(m) = IDENT.find(piece) {
                if m.as_str() != "self" {
                    out.push(m.as_str().to_string());
                }
            }
        }
    }
    out
}

impl Look {
    /// The final reply's answer is right.
    pub fn passes(&self, text: &str) -> bool {
        let Some(a) = answer_of(text) else { return false };
        match &self.key {
            Key::File(f) => self.code.names_file(&a, *f),
            Key::Params(want) => names_listed(&a) == *want,
            Key::Callers { of, want } => {
                let named: HashSet<&str> = IDENT.find_iter(&a).map(|m| m.as_str()).filter(|n| n != of && self.code.names.contains(*n)).collect();
                named.len() == want.len() && want.iter().all(|w| named.contains(w.as_str()))
            }
        }
    }

    #[cfg(test)]
    fn right_answer(&self) -> String {
        match &self.key {
            Key::File(f) => self.code.files[*f].shown.clone(),
            Key::Params(p) | Key::Callers { want: p, .. } => p.join(", "),
        }
    }
}

// ---------- a try ----------

/// What comes after the model's turn in a layout where `MARK` stands for it.
fn after_mark(laid: &str) -> Option<&str> {
    laid.rfind(MARK).map(|i| &laid[i + MARK.len()..])
}

async fn layout(model: &Model, messages: &[Value], tools: &Value) -> Result<String, String> {
    let body = json!({ "messages": messages, "tools": tools, "add_generation_prompt": true, "chat_template_kwargs": { "enable_thinking": model.think } });
    let v = model.call("/apply-template", body).await?;
    v["prompt"].as_str().map(str::to_string).ok_or_else(|| "The engine didn't lay out the request.".to_string())
}

/// One try at a task about the person's code: the model's replies, with the
/// tool calls in them carried out on the folders and the results passed
/// back, until it answers (or runs out of rounds or room). `room`: the
/// tokens it may use after the request, tool results included.
///
/// Everything after the request is kept as one text, exactly as the model
/// saw and wrote it: each tool result is laid out by its own chat template
/// (asked to lay out the conversation with a stand-in for the turn just
/// written, and taking what follows it), so the trainer finds every one of
/// its turns.
pub(super) async fn episode(model: &Model, task: &Task, look: &Look, prompt: &str, room: u32, temperature: f64, seed: u64) -> Result<Attempt, String> {
    Ok(tried(model, task, look, prompt, room, temperature, seed, false).await?.0)
}

/// A turn of a try with tool calls: what the model said (its thinking left
/// out) and the results its calls got.
pub(super) struct Round {
    reply: Reply,
    results: Vec<String>,
}

/// A teacher's right try, to be written the student's way.
pub(super) struct Looked {
    rounds: Vec<Round>,
    /// Its last turn: the answer.
    answer: String,
}

/// The try, and its turns with tool calls. `rescue`: a call written into the
/// text is run the way chats run it (`rescued`). Only for a teacher, whose
/// turns are written out again the student's way: the student's own turns
/// are learnt as written, so they must be in its form.
#[allow(clippy::too_many_arguments)]
async fn tried(model: &Model, task: &Task, look: &Look, prompt: &str, room: u32, temperature: f64, seed: u64, rescue: bool) -> Result<(Attempt, Vec<Round>), String> {
    let tools = task.tools.clone().unwrap_or_else(definitions);
    let mut rounds = Vec::new();
    let mut messages = vec![json!({ "role": "system", "content": tasks::SYSTEM }), json!({ "role": "user", "content": task.user })];
    let t = &look.code.tally;
    let count = |n: &AtomicU32| n.fetch_add(1, Ordering::Relaxed);
    count(&t.tries);
    let asked = model.tokens(prompt).await?;
    t.request.fetch_max(asked, Ordering::Relaxed);
    let limit = asked + room;
    let mut text = prompt.to_string();
    let mut looked = false;
    let gave_up = |text: String, why: &AtomicU32| {
        count(why);
        Attempt { text, cut: true, all: None }
    };
    for round in 0..ROUNDS {
        let left = limit.saturating_sub(model.tokens(&text).await?);
        if left < 16 {
            return Ok((gave_up(String::new(), &t.no_room), rounds));
        }
        let a = model.reply(&text, left.min(TURN), temperature, seed.wrapping_add(round as u64 * 7919)).await?;
        if a.cut {
            return Ok((gave_up(a.text, &t.no_room), rounds));
        }
        let Some(mut reply) = tasks::read(&a.text) else { return Ok((gave_up(a.text, &t.lost), rounds)) };
        if rescue {
            reply = rescued(reply, &tools);
        }
        if reply.calls.is_empty() {
            // An answer given without looking is a guess, which isn't what's practised.
            count(if looked { &t.answered } else { &t.guessed });
            let all = format!("{}{}", &text[prompt.len()..], a.text);
            return Ok((Attempt { text: a.text, cut: !looked, all: Some(all) }, rounds));
        }
        t.calls.fetch_add(reply.calls.len() as u32, Ordering::Relaxed);
        let results: Vec<String> = tokio::task::block_in_place(|| {
            reply
                .calls
                .iter()
                .enumerate()
                .map(|(i, (name, args))| if i < CALLS { look.code.run(name, args) } else { format!("Error: At most {CALLS} tool calls at once. Call it again after reading these results.") })
                .collect()
        });
        looked |= results.iter().any(|r| !r.starts_with("Error:"));
        let at = messages.len();
        messages.push(json!({ "role": "assistant", "content": MARK }));
        for (i, r) in results.iter().enumerate() {
            messages.push(json!({ "role": "tool", "tool_call_id": format!("call_{round}_{i}"), "content": r }));
        }
        let laid = layout(model, &messages, &tools).await?;
        let tail = after_mark(&laid).ok_or("This model's chat template doesn't lay out tool results, so it can't practise on code.")?;
        text.push_str(&a.text);
        text.push_str(tail);
        // Later layouts show this turn as it was written.
        messages[at]["content"] = json!(a.text);
        rounds.push(Round { reply, results });
    }
    Ok((gave_up(String::new(), &t.lost), rounds))
}

/// The teacher's tries at a task about the person's code, as the student's
/// were (each a whole try with its tool calls carried out), at each of
/// `temperatures` until one is right. `room`: the tokens it may use after
/// its request.
pub(super) async fn teach(teacher: &Model, task: &Task, look: &Look, room: u32, temperatures: &[f64], rng: &mut Rng) -> Result<Option<Looked>, String> {
    let prompt = teacher.prompt(task, None).await?;
    for temperature in temperatures {
        let (a, rounds) = tried(teacher, task, look, &prompt, room, *temperature, rng.next(), true).await?;
        let answer = if a.cut { None } else { tasks::read(&a.text).filter(|r| tasks::passes(task, r)) };
        if let Some(answer) = answer {
            return Ok(Some(Looked { rounds, answer: answer.text }));
        }
    }
    Ok(None)
}

/// Where a layout's latest turn of the model's begins, the generation
/// prompt at its end left aside: just past that turn's start, and past an
/// empty thinking block the template may write in it (the request already
/// ends with one when the model has thinking).
fn last_turn<'a>(laid: &'a str, turn: &Turn) -> Option<&'a str> {
    let starts: Vec<usize> = laid.match_indices(turn.start).map(|(i, _)| i).collect();
    let at = starts.len().checked_sub(2).map(|k| starts[k] + turn.start.len())?;
    let piece = &laid[at..];
    Some(match piece.strip_prefix("<think>").and_then(|p| p.find("</think>").map(|i| &p[i + "</think>".len()..])) {
        Some(rest) => rest.trim_start_matches('\n'),
        None => piece,
    })
}

/// A teacher's right try in the student's own words: each of its turns as
/// the student's template writes it (its tool calls in the student's form),
/// the same tool results laid out after each the student's way, then its
/// answer. `prompt`: the request as the student's template laid it out;
/// `room`: the tokens a training example has left after it. The text after
/// the request, or none when written that way it's no longer right or
/// doesn't fit.
pub(super) async fn in_students_words(student: &Model, task: &Task, prompt: &str, room: u32, taught: &Looked) -> Result<Option<String>, String> {
    let Some(turn) = TURNS.iter().find(|t| prompt.contains(t.start)) else { return Ok(None) };
    let looked = taught.rounds.iter().any(|r| r.results.iter().any(|x| !x.starts_with("Error:")));
    let right = tasks::read(&taught.answer).is_some_and(|r| tasks::passes(task, &r));
    if !looked || !right {
        return Ok(None);
    }
    let tools = task.tools.clone().unwrap_or_else(definitions);
    let mut messages = vec![json!({ "role": "system", "content": tasks::SYSTEM }), json!({ "role": "user", "content": task.user })];
    let mut text = String::new();
    for (k, round) in taught.rounds.iter().enumerate() {
        let calls: Vec<Value> = round.reply.calls.iter().enumerate().map(|(i, (name, args))| json!({ "id": format!("call_{k}_{i}"), "type": "function", "function": { "name": name, "arguments": args.to_string() } })).collect();
        messages.push(json!({ "role": "assistant", "content": round.reply.text, "tool_calls": calls }));
        for (i, r) in round.results.iter().enumerate() {
            messages.push(json!({ "role": "tool", "tool_call_id": format!("call_{k}_{i}"), "content": r }));
        }
        let laid = layout(student, &messages, &tools).await?;
        let Some(piece) = last_turn(&laid, turn) else { return Ok(None) };
        text.push_str(piece);
    }
    text.push_str(&taught.answer);
    if student.tokens(&format!("{prompt}{text}")).await? > student.tokens(prompt).await? + room {
        return Ok(None);
    }
    Ok(Some(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small project with code in each language, and things the scan must
    /// leave alone.
    fn project() -> (PathBuf, Arc<Index>) {
        let root = std::env::temp_dir().join(format!("yourcode-{}", uuid::Uuid::new_v4().simple())).join("proj");
        let put = |rel: &str, text: &str| {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        };
        put(
            "src/lib.rs",
            r##"// fn commented_out(x: u32) {}
/* fn blocked_out(y: u8) { /* nested */ } */
pub fn load_config(path: &Path, strict: bool) -> Result<Config, String> {
    let text = read_settings(path)?;
    parse_config_text(&text, strict)
}

fn read_settings(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| e.to_string())
}

pub(crate) fn parse_config_text<'a, T: AsRef<str>>(
    text: &'a str,
    strict: bool,
) -> Result<Config, String> {
    let s = "fn fake_in_string(a: u8) { read_settings(x) }";
    let c = '{';
    let raw = r#"fn raw_fake() {}"#;
    Ok(Config { strict })
}

impl Config {
    pub fn merge_with(&mut self, other: Config, keep_old: bool) {
        let _ = (other, keep_old);
    }
}
"##,
        );
        put(
            "web/app.ts",
            r#"export function renderPanel(title: string, count = 3): string {
  return formatTitle(title) + count;
}
const formatTitle = (t: string) => t.toUpperCase();
export const fetchUsers = async (page: number, size?: number): Promise<User[]> => {
  const r = await getJson(`/users?page=${page}`);
  return r;
};
class Store {
  async saveItem(item: Item, force: boolean): Promise<void> {
    formatTitle(item.name);
  }
}
function withProps({ a, b }: Props) {
  return a + b;
}
"#,
        );
        put(
            "tools/report.py",
            r#"def build_report(rows, *, title="x, y", **extra):
    return summarize_rows(rows)


def summarize_rows(rows):
    # summarize_rows(rows) in a comment doesn't count
    """Nor summarize_rows(rows) in a docstring."""
    return len(rows)


class Exporter:
    def export_csv(self, path, rows):
        build_report(rows)
"#,
        );
        put("node_modules/pkg/index.js", "function hiddenDependency(a) { return a; }\n");
        put("target/debug/build_helper.rs", "fn built_output(a: u8) {}\n");
        put(".env", "API_KEY=abc\n");
        // Written in pieces, so this file doesn't look like it holds a key.
        put("keys/server.pem", concat!("-----BEGIN PRIVATE", " KEY-----\n"));
        put("src/settings.rs", concat!("const GITHUB: &str = \"gh", "p_abcdefghijklmnopqrstuvwxyz0123456789\";\nfn leaked_value() {}\n"));
        put("Cargo.lock", "[[package]]\nname = \"fn_in_lock\"\n");
        put(".gitignore", "generated/\n");
        put("generated/gen.rs", "fn generated_code(a: u8) {}\n");
        let code = Index::scan(&[root.display().to_string()]).unwrap();
        (root, Arc::new(code))
    }

    fn def<'a>(code: &'a Index, name: &str) -> &'a Def {
        code.defs.iter().find(|d| d.name == name).unwrap_or_else(|| panic!("{name} not found"))
    }

    #[test]
    fn the_scan_leaves_out_dependencies_build_output_lockfiles_and_secrets() {
        let (root, code) = project();
        let shown: Vec<&str> = code.files.iter().map(|f| f.shown.as_str()).collect();
        assert!(shown.contains(&"proj/src/lib.rs") && shown.contains(&"proj/web/app.ts") && shown.contains(&"proj/tools/report.py"), "{shown:?}");
        for gone in ["node_modules", "target", ".env", "server.pem", "settings.rs", "Cargo.lock", "generated"] {
            assert!(!shown.iter().any(|s| s.contains(gone)), "{gone} in {shown:?}");
        }
        let mut secret = code.secret.clone();
        secret.sort();
        assert_eq!(secret, ["proj/.env", "proj/keys/server.pem", "proj/src/settings.rs"], "the file with a token in it too");
        for hidden in ["hiddenDependency", "built_output", "leaked_value", "generated_code", "fn_in_lock"] {
            assert!(!code.names.contains(hidden), "{hidden}");
        }
        assert!(code.summary().contains("3 files left out"));
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    #[test]
    fn reads_definitions_in_each_language() {
        let (root, code) = project();
        let load = def(&code, "load_config");
        assert_eq!((load.params.clone().unwrap(), load.sig_lines, load.line), (vec!["path".to_string(), "strict".to_string()], 1, 3));
        assert_eq!(load.body, Some((3, 6)));
        let parse = def(&code, "parse_config_text");
        assert_eq!(parse.params.clone().unwrap(), ["text", "strict"]);
        assert_eq!(parse.sig_lines, 4);
        let merge = def(&code, "merge_with");
        assert!(merge.has_self);
        assert_eq!(merge.params.clone().unwrap(), ["other", "keep_old"]);
        for fake in ["commented_out", "blocked_out", "fake_in_string", "raw_fake"] {
            assert!(!code.names.contains(fake), "{fake} is in a comment or string");
        }

        assert_eq!(def(&code, "renderPanel").params.clone().unwrap(), ["title", "count"]);
        assert_eq!(def(&code, "formatTitle").params.clone().unwrap(), ["t"]);
        assert_eq!(def(&code, "fetchUsers").params.clone().unwrap(), ["page", "size"]);
        assert_eq!(def(&code, "saveItem").params.clone().unwrap(), ["item", "force"]);
        assert_eq!(def(&code, "withProps").params, None, "destructured");

        assert_eq!(def(&code, "build_report").params.clone().unwrap(), ["rows", "title", "extra"]);
        let export = def(&code, "export_csv");
        assert!(export.has_self);
        assert_eq!(export.params.clone().unwrap(), ["path", "rows"]);
        assert_eq!(def(&code, "summarize_rows").body, Some((6, 8)));
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    #[test]
    fn finds_what_calls_what() {
        let (root, code) = project();
        let callers = |name: &str| -> Option<Vec<String>> {
            let y = code.defs.iter().position(|d| d.name == name)?;
            code.pools.iter().flatten().find_map(|p| match p {
                Pick::Callers(i, c) if *i == y => Some(c.iter().map(|c| code.defs[*c].name.clone()).collect()),
                _ => None,
            })
        };
        assert_eq!(callers("read_settings").unwrap(), ["load_config"], "the call in a string doesn't count");
        assert_eq!(callers("parse_config_text").unwrap(), ["load_config"]);
        let mut both = callers("formatTitle").unwrap();
        both.sort();
        assert_eq!(both, ["renderPanel", "saveItem"]);
        assert_eq!(callers("summarize_rows").unwrap(), ["build_report"], "comments and docstrings don't count");
        assert_eq!(callers("build_report").unwrap(), ["export_csv"]);
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    /// Every task's right answer passes its check, at every level.
    #[test]
    fn every_task_has_a_right_answer() {
        let (root, code) = project();
        let mut rng = Rng::new(5);
        for level in 1..=tasks::LEVELS {
            for _ in 0..30 {
                let t = task(&code, level, &mut rng);
                let Check::Look(look) = &t.check else { panic!() };
                let answer = look.right_answer();
                let reply = tasks::read(&format!("I searched and read it.\n\nAnswer: {answer}")).unwrap();
                assert!(tasks::passes(&t, &reply), "level {level}: {} / {answer}", t.user);
                assert!(!tasks::passes(&t, &tasks::read("Answer: something_else.rs, nothing").unwrap()), "{}", t.user);
                assert!(t.user.contains("the shared folder proj") && t.user.contains("End with: Answer: <"), "{}", t.user);
            }
        }
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    #[test]
    fn checks_take_answers_as_written_and_refuse_wrong_ones() {
        let (root, code) = project();
        let look = |key: Key| Look { key, code: code.clone() };
        let lib = code.files.iter().position(|f| f.shown == "proj/src/lib.rs").unwrap();
        let file = look(Key::File(lib));
        for right in ["proj/src/lib.rs", "`src/lib.rs`", "**src/lib.rs**", "It's in src/lib.rs (line 3).", "lib.rs", r"C:\work\proj\src\lib.rs"] {
            assert!(file.passes(&format!("Answer: {right}")), "{right}");
        }
        for wrong in ["src/main.rs", "proj/web/app.ts", "the library file"] {
            assert!(!file.passes(&format!("Answer: {wrong}")), "{wrong}");
        }
        assert!(!file.passes("It's src/lib.rs."), "the answer line was asked for");

        let params = look(Key::Params(vec!["path".into(), "strict".into()]));
        for right in ["path, strict", "`path: &Path`, `strict: bool`", "(path, strict)", "path and strict", "path: &Path, strict: bool"] {
            assert!(params.passes(&format!("Answer: {right}")), "{right}");
        }
        for wrong in ["strict, path", "path", "path, strict, extra", "self, path"] {
            assert!(!params.passes(&format!("Answer: {wrong}")), "{wrong}");
        }
        let generic = look(Key::Params(vec!["map".into(), "limit".into()]));
        assert!(generic.passes("Answer: map: HashMap<String, u32>, limit: usize"), "commas inside generics");

        let callers = look(Key::Callers { of: "read_settings".into(), want: vec!["load_config".into()] });
        assert!(callers.passes("Answer: load_config"));
        assert!(callers.passes("Answer: `load_config` (in src/lib.rs) calls read_settings"));
        assert!(!callers.passes("Answer: load_config, parse_config_text"), "one too many");
        assert!(!callers.passes("Answer: parse_config_text"));
        let two = look(Key::Callers { of: "formatTitle".into(), want: vec!["renderPanel".into(), "saveItem".into()] });
        assert!(two.passes("Answer: saveItem, renderPanel"));
        assert!(!two.passes("Answer: renderPanel"));
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    #[test]
    fn tools_run_read_only_in_small_pieces() {
        let (root, code) = project();
        let found = code.run("search_files", &json!({ "pattern": "fn " }));
        assert!(found.contains("proj/src/lib.rs:3: pub fn load_config(path: &Path, strict: bool)"), "{found}");
        assert!(found.lines().count() <= MATCHES + 1);
        assert!(found.contains("stopped at"), "{found}");
        assert!(!code.run("search_files", &json!({ "pattern": "GITHUB" })).contains("settings"), "a file with a token is never shown");
        assert!(code.run("search_files", &json!({ "pattern": "def ", "glob": "*.py" })).starts_with("proj/tools/report.py:1:"));
        assert!(code.run("search_files", &json!({ "pattern": "(" })).starts_with("Error: That pattern isn't a valid regular expression"));

        let read = code.run("read_file", &json!({ "path": "proj/src/lib.rs", "start_line": 3, "end_line": 4 }));
        assert_eq!(read.lines().next().unwrap(), "    3\tpub fn load_config(path: &Path, strict: bool) -> Result<Config, String> {");
        assert!(read.contains("more lines. Use start_line=5"));
        let long = code.run("read_file", &json!({ "path": "src/lib.rs", "start_line": "2", "end_line": 400 }));
        assert_eq!(long.lines().filter(|l| l.contains('\t')).count(), READ_LINES, "{long}");
        assert!(code.run("read_file", &json!({ "path": "proj/.env" })).contains("left out of practice"));
        assert!(code.run("read_file", &json!({ "path": "proj/node_modules/pkg/index.js" })).contains("left out of practice"));
        assert!(code.run("read_file", &json!({ "path": "../../outside.txt" })).starts_with("Error:"));
        assert!(code.run("read_file", &json!({ "path": "proj/src" })).contains("folder"));

        assert_eq!(code.run("find_files", &json!({ "pattern": "*.py" })), "proj/tools/report.py");
        assert_eq!(code.run("find_files", &json!({ "pattern": "*.lock" })), "No files match *.lock.");
        assert!(code.run("write_file", &json!({ "path": "x", "content": "y" })).starts_with("Error: There's no tool called write_file"));
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    #[test]
    fn secret_looking_names_are_left_out() {
        for name in [".env", ".env.local", "server.key", "id_rsa", "id_ed25519.pub", "client_secret.json", "db-password.txt", "github_token", "credentials.json", ".npmrc", "cert.pem", "api_key.ts", "prod.env"] {
            assert!(secret_name(name), "{name}");
        }
        for name in ["main.rs", "tokenizer.rs", "keyboard.ts", "sitekeys.rs", "README.md", "environment.py"] {
            assert!(!secret_name(name), "{name}");
        }
        assert!(skipped(Path::new("web/node_modules/x/index.js")) && skipped(Path::new("pnpm-lock.yaml")) && skipped(Path::new("app/dist/main.js")));
        assert!(!skipped(Path::new("src/builder.rs")));
    }

    /// Keys in text, quoted in code or bare in a .env file; ordinary code isn't.
    #[test]
    fn keys_in_text_are_seen_quoted_or_bare() {
        for text in [
            "const apiKey = \"abcdef0123456789xyz\";",
            "CLIENT_SECRET=Zx9q2LmN8pR4tV6wY1bC",
            "OPENAI_API_KEY=abcdEFGH1234ijklMNOP5678",
            "DB_PASSWORD: s3cretPassw0rdValue1",
            "-----BEGIN OPENSSH PRIVATE KEY-----",
        ] {
            assert!(SECRET_TEXT.is_match(text), "{text}");
        }
        for text in ["let token = next_token(&mut lexer);", "password = form.password.value", "API_KEY=", "secret: true"] {
            assert!(!SECRET_TEXT.is_match(text), "{text}");
        }
    }

    /// What follows the model's turn is taken from its template's layout.
    #[test]
    fn the_layout_after_a_turn_is_kept_as_laid_out() {
        let laid = format!("<|im_start|>user\nq<|im_end|>\n<|im_start|>assistant\n{MARK}<|im_end|>\n<|im_start|>user\n<tool_response>\nfound\n</tool_response><|im_end|>\n<|im_start|>assistant\n");
        assert_eq!(after_mark(&laid).unwrap(), "<|im_end|>\n<|im_start|>user\n<tool_response>\nfound\n</tool_response><|im_end|>\n<|im_start|>assistant\n");
        assert!(after_mark("no stand-in").is_none());
    }

    /// A real folder's scan: `SULCUSAI_SCAN=C:\dev\sulcusai cargo test --lib
    /// scan_a_real_folder -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn scan_a_real_folder() {
        let folder = std::env::var("SULCUSAI_SCAN").expect("SULCUSAI_SCAN");
        let started = std::time::Instant::now();
        let code = Arc::new(Index::scan(&[folder]).unwrap());
        println!("{} in {:?}; usable: {}", code.summary(), started.elapsed(), code.usable());
        println!("left out as secret-looking: {:?}", code.secret);
        println!("tasks a level: {:?}", code.pools.iter().map(Vec::len).collect::<Vec<_>>());
        let mut rng = Rng::new(super::super::TEST_SEED);
        for level in 1..=tasks::LEVELS {
            let t = task(&code, level, &mut rng);
            let Check::Look(look) = &t.check else { panic!() };
            println!("\n{level}: {}\n   → {}", t.user, look.right_answer());
        }
    }

    /// Real tries on a real folder, printed with their sizes, against a
    /// running llama-server (started with --jinja):
    /// `SULCUSAI_SCAN=C:\dev\sulcusai SULCUSAI_TEST_PORT=8199
    /// SULCUSAI_TEST_KEY=… cargo test --lib e2e_code_tries -- --ignored
    /// --nocapture`. Each task is tried with practice's room (what fits a
    /// training example) and the test's.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn e2e_code_tries() {
        let folder = std::env::var("SULCUSAI_SCAN").expect("SULCUSAI_SCAN");
        let port = std::env::var("SULCUSAI_TEST_PORT").expect("SULCUSAI_TEST_PORT").parse().unwrap();
        let key = std::env::var("SULCUSAI_TEST_KEY").unwrap_or_default();
        let code = Arc::new(Index::scan(&[folder]).unwrap());
        let model = Model::new(crate::engine::Endpoint { port, key, ctx: super::super::ENGINE_CTX, extra: Value::Null, cloud: None }, false).await;
        let mut rng = Rng::new(super::super::TEST_SEED ^ std::env::var("SULCUSAI_SEED").ok().and_then(|n| n.parse().ok()).unwrap_or(0));
        let each: usize = std::env::var("SULCUSAI_TRIES").ok().and_then(|n| n.parse().ok()).unwrap_or(2);
        let show = std::env::var("SULCUSAI_SHOW").is_ok();
        // For each level: tasks, right in the test's room, right in practice (as practice tries).
        let mut score = [(0, 0, 0); tasks::LEVELS as usize];
        for level in 1..=tasks::LEVELS {
            for _ in 0..each {
                let t = task(&code, level, &mut rng);
                let Check::Look(look) = &t.check else { panic!() };
                let prompt = model.prompt(&t, None).await.unwrap();
                let asked = model.tokens(&prompt).await.unwrap();
                let room = super::super::TRAIN_CTX.saturating_sub(asked + 4);
                let s = &mut score[level as usize - 1];
                s.0 += 1;
                // SULCUSAI_RESCUE: run calls written as text, as for a teacher.
                let rescue = std::env::var("SULCUSAI_RESCUE").is_ok();
                let a = tried(&model, &t, look, &prompt, super::super::TEST_REPLY, 0.0, 1, rescue).await.unwrap().0;
                let right = |a: &Attempt| !a.cut && tasks::read(&a.text).is_some_and(|r| tasks::passes(&t, &r));
                let all = a.all.clone().unwrap_or_default();
                let used = if all.is_empty() { 0 } else { model.tokens(&format!("{prompt}{all}")).await.unwrap() };
                s.1 += right(&a) as usize;
                println!("\n=== level {level}: {}\n    want: {}\n--- test: right {}, request {asked} + try = {used} tokens", t.user, look.right_answer(), right(&a));
                if show {
                    println!("{}", if all.is_empty() { &a.text } else { &all });
                }
                for k in 0..super::super::ATTEMPTS {
                    let p = tried(&model, &t, look, &prompt, room, 0.8, rng.next(), rescue).await.unwrap().0;
                    if right(&p) {
                        s.2 += 1;
                        let used = model.tokens(&format!("{prompt}{}", p.all.clone().unwrap_or_default())).await.unwrap();
                        println!("--- practice: right on try {}, {used} tokens", k + 1);
                        // SULCUSAI_DUMP: the training lines practice would keep (all right after a miss,
                        // some right at once), for running the trainer by hand.
                        let keep = k > 0 || rng.chance(super::super::EASY_SHARE);
                        if let (Ok(path), Some(line), true) = (std::env::var("SULCUSAI_DUMP"), super::super::example(&model.bos, &prompt, p.all.as_deref().unwrap_or_default()), keep) {
                            use std::io::Write as _;
                            let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
                            writeln!(f, "{line}").unwrap();
                        }
                        if show {
                            println!("{}", p.all.unwrap_or_default());
                        }
                        break;
                    }
                }
            }
        }
        println!("\nlevel: tasks, right in the test, right in practice's room: {score:?}");
        println!("{}", code.tally());
    }

    /// Practice's test of "your code" alone (its 12 tasks, at temperature 0,
    /// with the engine's whole window), against a running llama-server, with
    /// or without an adapter (`--lora`): the before and after of a run, for a
    /// trainer run by hand. The training lines' tasks are listed if
    /// `SULCUSAI_DUMP` names them, to check none is a test task.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn e2e_code_test() {
        let folder = std::env::var("SULCUSAI_SCAN").expect("SULCUSAI_SCAN");
        let port = std::env::var("SULCUSAI_TEST_PORT").expect("SULCUSAI_TEST_PORT").parse().unwrap();
        let key = std::env::var("SULCUSAI_TEST_KEY").unwrap_or_default();
        let code = Arc::new(Index::scan(&[folder]).unwrap());
        let model = Model::new(crate::engine::Endpoint { port, key, ctx: super::super::ENGINE_CTX, extra: Value::Null, cloud: None }, false).await;
        let mut rng = Rng::new(super::super::TEST_SEED);
        let mut tests: Vec<Task> = super::super::TEST_LEVELS.iter().map(|l| task(&code, *l, &mut rng)).collect();
        // To compare tool definitions: SULCUSAI_TOOLS, a file of others.
        let tools: Option<Value> = std::env::var("SULCUSAI_TOOLS").ok().map(|p| serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap());
        for t in &mut tests {
            if let Some(defs) = &tools {
                t.tools = Some(defs.clone());
            }
        }
        if let Ok(dump) = std::env::var("SULCUSAI_DUMP") {
            let lines = std::fs::read_to_string(dump).unwrap();
            let practised = |t: &Task| {
                let quoted = json!(t.user).to_string();
                lines.contains(&quoted[1..quoted.len() - 1])
            };
            assert!(!tests.iter().any(practised), "a test task was practised");
        }
        let mut right = Vec::new();
        for (i, t) in tests.iter().enumerate() {
            let prompt = model.prompt(t, None).await.unwrap();
            let a = model.attempt(t, &prompt, super::super::TEST_REPLY, 0.0, i as u64 + 1).await.unwrap();
            let ok = !a.cut && tasks::read(&a.text).is_some_and(|r| tasks::passes(t, &r));
            println!("level {}: {} | {}", t.level, if ok { "right" } else { "wrong" }, t.user.split('?').next().unwrap_or(""));
            right.push(ok);
        }
        println!("\n{} of {} right; {}", right.iter().filter(|r| **r).count(), right.len(), code.tally());
    }

    /// A teacher's turn is taken from the student's layout of the
    /// conversation: from its latest turn (not the generation prompt at the
    /// end) to the end, any empty thinking block left out.
    #[test]
    fn a_turn_is_taken_from_the_students_layout() {
        let chatml = &super::super::TURNS[0];
        // As Qwen3 4B 2507's template lays out a try after its first round.
        let head = "<|im_start|>system\nYou are a helpful assistant.<|im_end|>\n<|im_start|>user\nWhich file defines `x`?<|im_end|>\n<|im_start|>assistant\n";
        let call = "<tool_call>\n{\"name\": \"search_files\", \"arguments\": {\"pattern\": \"fn x\"}}\n</tool_call>";
        let after = "<|im_end|>\n<|im_start|>user\n<tool_response>\nproj/src/a.rs:3: fn x() {\n</tool_response><|im_end|>\n<|im_start|>assistant\n";
        let laid = format!("{head}{call}{after}");
        assert_eq!(last_turn(&laid, chatml).unwrap(), format!("{call}{after}"));
        // A template that writes an empty thinking block into the turn.
        let thinking = format!("{head}<think>\n\n</think>\n\n{call}{after}");
        assert_eq!(last_turn(&thinking, chatml).unwrap(), format!("{call}{after}"));
        // After a second round, only the latest turn: the pieces join up.
        let second = "<tool_call>\n{\"name\": \"read_file\", \"arguments\": {\"path\": \"proj/src/a.rs\"}}\n</tool_call>";
        let after2 = "<|im_end|>\n<|im_start|>user\n<tool_response>\n    3\tfn x() {\n</tool_response><|im_end|>\n<|im_start|>assistant\n";
        let laid2 = format!("{laid}{second}{after2}");
        let joined = format!("{head}{}{}", last_turn(&laid, chatml).unwrap(), last_turn(&laid2, chatml).unwrap());
        assert_eq!(joined, laid2);
        assert!(last_turn(head, chatml).is_none(), "no turn of the model's yet");
        let gemma = &super::super::TURNS[1];
        let g = "<start_of_turn>user\nq<end_of_turn>\n<start_of_turn>model\nlooking<end_of_turn>\n<start_of_turn>user\nresult<end_of_turn>\n<start_of_turn>model\n";
        assert_eq!(last_turn(g, gemma).unwrap(), "looking<end_of_turn>\n<start_of_turn>user\nresult<end_of_turn>\n<start_of_turn>model\n");
    }

    /// qwen2.5-coder 14B writes its calls as a ```json block, then copies the
    /// answer line's placeholder: for a teacher, that's the call it meant.
    #[test]
    fn a_call_written_as_text_is_run_for_a_teacher() {
        let written = "To find the file that defines the Rust function `background_endpoint`, I will use the `search_files` function.\n\n```json\n{\n  \"name\": \"search_files\",\n  \"arguments\": {\n    \"pattern\": \"fn background_endpoint\",\n    \"glob\": \"sulcusai/**/*\"\n  }\n}\n```\n\nAnswer: <path>";
        let r = rescued(tasks::read(written).unwrap(), &definitions());
        assert_eq!(r.calls, vec![("search_files".to_string(), json!({ "pattern": "fn background_endpoint", "glob": "sulcusai/**/*" }))]);
        assert_eq!(r.text, "To find the file that defines the Rust function `background_endpoint`, I will use the `search_files` function.");
        let plain = rescued(tasks::read("It's in proj/src/a.rs.\n\nAnswer: proj/src/a.rs").unwrap(), &definitions());
        assert!(plain.calls.is_empty() && plain.text.ends_with("Answer: proj/src/a.rs"));
        let other = rescued(tasks::read("```json\n{\"name\": \"write_file\", \"arguments\": {\"path\": \"x\"}}\n```").unwrap(), &definitions());
        assert!(other.calls.is_empty(), "only tools that were offered");
    }

    /// A model's own right try, re-laid by its own template the way a
    /// teacher's is, comes out as it wrote it. Against a running
    /// llama-server, as `e2e_code_tries`.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn e2e_a_relaid_try_matches_the_original() {
        let folder = std::env::var("SULCUSAI_SCAN").expect("SULCUSAI_SCAN");
        let port = std::env::var("SULCUSAI_TEST_PORT").expect("SULCUSAI_TEST_PORT").parse().unwrap();
        let key = std::env::var("SULCUSAI_TEST_KEY").unwrap_or_default();
        let code = Arc::new(Index::scan(&[folder]).unwrap());
        let model = Model::new(crate::engine::Endpoint { port, key, ctx: super::super::ENGINE_CTX, extra: Value::Null, cloud: None }, false).await;
        let mut rng = Rng::new(super::super::TEST_SEED);
        let (mut same, mut tried_n) = (0, 0);
        for level in [1, 2, 3, 1, 2, 3, 4, 5] {
            let t = task(&code, level, &mut rng);
            let Check::Look(look) = &t.check else { panic!() };
            let prompt = model.prompt(&t, None).await.unwrap();
            let (a, rounds) = tried(&model, &t, look, &prompt, super::super::TEST_REPLY, 0.0, 1, false).await.unwrap();
            let Some(answer) = (!a.cut).then(|| tasks::read(&a.text)).flatten().filter(|r| tasks::passes(&t, r)) else { continue };
            tried_n += 1;
            let looked = Looked { rounds, answer: answer.text };
            let relaid = in_students_words(&model, &t, &prompt, super::super::TEST_REPLY, &looked).await.unwrap().expect("still right and fits");
            let original = a.all.unwrap();
            if relaid == original {
                same += 1;
            } else {
                let at = relaid.chars().zip(original.chars()).take_while(|(x, y)| x == y).count();
                println!("level {level}: differs at char {at}\n  re-laid: {:?}\n  its own: {:?}", relaid.chars().skip(at.saturating_sub(40)).take(120).collect::<String>(), original.chars().skip(at.saturating_sub(40)).take(120).collect::<String>());
            }
        }
        println!("{same} of {tried_n} right tries re-laid exactly as written");
        assert!(tried_n > 0);
    }

    /// The tool definitions keep the chat tools' names and parameters.
    #[test]
    fn the_tools_are_the_chat_file_tools() {
        let defs = definitions();
        let names: Vec<&str> = defs.as_array().unwrap().iter().map(|d| d["function"]["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["search_files", "find_files", "read_file"]);
        for d in defs.as_array().unwrap() {
            let real = crate::tools::find(d["function"]["name"].as_str().unwrap()).unwrap();
            let real_params = (real.params)();
            for p in d["function"]["parameters"]["properties"].as_object().unwrap().keys() {
                assert!(real_params["properties"].get(p).is_some(), "{p}");
            }
        }
    }
}

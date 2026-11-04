//! "Manage itself" for one chat (off by default): per-job budgets, a
//! repeat detector, a plan with checks the app runs, effort per step, and
//! working notes kept on disk so a cleared context resumes cheaply.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[cfg(test)]
mod tests;

/// Who manages the chat. Only `Itself` runs in this part; the others are
/// stored so the choice survives until the managed form is built.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum ManagerKind {
    #[default]
    Itself,
    Chat(String),
    Model(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub on: bool,
    pub token_budget: u64,
    pub minutes_budget: u32,
    pub repeat_limit: u32,
    /// Share of the context window past which the next turn starts from the handoff.
    pub clear_at: f32,
    pub manager: ManagerKind,
}

impl Default for Settings {
    fn default() -> Self {
        Self { on: false, token_budget: 200_000, minutes_budget: 30, repeat_limit: 3, clear_at: 0.75, manager: ManagerKind::Itself }
    }
}

pub fn folder(data: &Path, chat_id: &str) -> PathBuf {
    // Chat ids are uuids; anything else is reduced so it can't leave the folder.
    let safe: String = chat_id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
    data.join("manager").join(safe)
}

pub fn load(data: &Path, chat_id: &str) -> Settings {
    std::fs::read_to_string(folder(data, chat_id).join("settings.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

pub fn save(data: &Path, chat_id: &str, s: &Settings) -> Result<(), String> {
    let dir = folder(data, chat_id);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let s = Settings { token_budget: s.token_budget.max(1_000), minutes_budget: s.minutes_budget.max(1), repeat_limit: s.repeat_limit.clamp(2, 10), clear_at: s.clear_at.clamp(0.3, 0.95), ..s.clone() };
    std::fs::write(dir.join("settings.json"), serde_json::to_string_pretty(&s).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetState {
    Fine,
    /// Past 80%: tell the model once to wrap up and write its handoff.
    Warn,
    Spent,
}

pub struct Budget {
    tokens: u64,
    max_tokens: u64,
    started: Instant,
    max_time: Duration,
    warned: bool,
}

impl Budget {
    pub fn new(s: &Settings) -> Self {
        Self { tokens: 0, max_tokens: s.token_budget, started: Instant::now(), max_time: Duration::from_secs(u64::from(s.minutes_budget) * 60), warned: false }
    }

    pub fn add_tokens(&mut self, n: u64) {
        self.tokens = self.tokens.saturating_add(n);
    }

    pub fn check(&mut self) -> BudgetState {
        self.check_at(self.started.elapsed())
    }

    pub fn check_at(&mut self, elapsed: Duration) -> BudgetState {
        let share = (self.tokens as f64 / self.max_tokens.max(1) as f64).max(elapsed.as_secs_f64() / self.max_time.as_secs_f64().max(1.0));
        if share >= 1.0 {
            BudgetState::Spent
        } else if share >= 0.8 && !self.warned {
            self.warned = true;
            BudgetState::Warn
        } else {
            BudgetState::Fine
        }
    }
}

/// The same failing action, counted; the limit-th time escalates.
#[derive(Default)]
pub struct Repeats {
    counts: Vec<(String, u32)>,
}

pub fn action_key(tool: &str, args: &str) -> String {
    format!("{tool}:{}", args.split_whitespace().collect::<Vec<_>>().join(" "))
}

impl Repeats {
    /// Counts a failure; true once the same action has failed `limit` times.
    pub fn failed(&mut self, key: &str, limit: u32) -> bool {
        let n = match self.counts.iter_mut().find(|(k, _)| k == key) {
            Some((_, n)) => {
                *n += 1;
                *n
            }
            None => {
                self.counts.push((key.to_string(), 1));
                1
            }
        };
        n >= limit
    }

    /// Something changed, so earlier failures may not repeat.
    pub fn progressed(&mut self) {
        self.counts.clear();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Size {
    Lookup,
    Edit,
    Feature,
    Hard,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Check {
    Run { command: String },
    File { path: String },
    Contains { path: String, text: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum StepState {
    #[default]
    Waiting,
    Working,
    Done,
    Stuck,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub text: String,
    pub size: Option<Size>,
    pub checks: Vec<Check>,
    #[serde(default)]
    pub state: StepState,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Plan {
    pub steps: Vec<Step>,
}

/// Reads the plan block the prompt asks for:
/// ```plan
/// 1. [edit] change the parser
///    check: run cargo test parser
///    check: file src/parser.rs
///    check: contains src/parser.rs :: fn parse
/// ```
pub fn parse_plan(reply: &str) -> Option<Plan> {
    let body = fenced(reply, "plan").into_iter().next()?;
    let mut steps: Vec<Step> = Vec::new();
    for line in body.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if let Some(c) = line.strip_prefix("check:") {
            let c = c.trim();
            let check = if let Some(cmd) = c.strip_prefix("run ") {
                Check::Run { command: cmd.trim().to_string() }
            } else if let Some(p) = c.strip_prefix("file ") {
                Check::File { path: p.trim().to_string() }
            } else if let Some(rest) = c.strip_prefix("contains ") {
                let (p, t) = rest.split_once("::")?;
                Check::Contains { path: p.trim().to_string(), text: t.trim().to_string() }
            } else {
                continue;
            };
            if let Some(s) = steps.last_mut() {
                s.checks.push(check);
            }
            continue;
        }
        let rest = line.trim_start_matches(|c: char| c.is_ascii_digit()).trim_start_matches(['.', ')', '-', '*']).trim();
        let (size, text) = match rest.strip_prefix('[').and_then(|r| r.split_once(']')) {
            Some((tag, t)) => (size_of(tag), t.trim()),
            None => (None, rest),
        };
        if !text.is_empty() {
            steps.push(Step { text: text.to_string(), size, checks: Vec::new(), state: StepState::Waiting });
        }
    }
    (!steps.is_empty()).then_some(Plan { steps })
}

fn size_of(tag: &str) -> Option<Size> {
    match tag.trim().to_ascii_lowercase().as_str() {
        "lookup" => Some(Size::Lookup),
        "edit" => Some(Size::Edit),
        "feature" => Some(Size::Feature),
        "hard" => Some(Size::Hard),
        _ => None,
    }
}

/// Bodies of ```<tag> fenced blocks (the tag may carry words after it).
fn fenced<'a>(text: &'a str, tag: &str) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("```") {
        let after = &rest[i + 3..];
        let Some(nl) = after.find('\n') else { break };
        let head = after[..nl].trim();
        let body_start = &after[nl + 1..];
        let Some(end) = body_start.find("```") else { break };
        if head == tag || head.starts_with(&format!("{tag} ")) {
            out.push(&body_start[..end]);
        }
        rest = &body_start[end + 3..];
    }
    out
}

// The done-check runner is wired into the turn in the next part (plan window).
#[cfg_attr(not(test), allow(dead_code))]
pub enum Outcome {
    Pass,
    Fail(String),
    /// A command: run through run_command so its permission ask stays.
    NeedsRun(String),
}

/// File checks run here; paths resolve against the chat's folder and may not leave it.
#[cfg_attr(not(test), allow(dead_code))]
pub fn run_check(check: &Check, root: &Path) -> Outcome {
    let inside = |p: &str| {
        let full = root.join(p);
        full.components().all(|c| !matches!(c, std::path::Component::ParentDir)).then_some(full)
    };
    match check {
        Check::Run { command } => Outcome::NeedsRun(command.clone()),
        Check::File { path } => match inside(path) {
            Some(f) if f.exists() => Outcome::Pass,
            _ => Outcome::Fail(format!("{path} is missing")),
        },
        Check::Contains { path, text } => match inside(path).and_then(|f| std::fs::read_to_string(f).ok()) {
            Some(s) if s.contains(text.as_str()) => Outcome::Pass,
            Some(_) => Outcome::Fail(format!("{path} doesn't contain \"{text}\"")),
            None => Outcome::Fail(format!("{path} can't be read")),
        },
    }
}

const EFFORTS: [&str; 4] = ["low", "medium", "high", "xhigh"];

/// Lowest effort expected to pass the step, one higher after a failed check.
pub fn pick_effort(size: Size, failed_before: bool) -> &'static str {
    let base = match size {
        Size::Lookup => 0,
        Size::Edit => 1,
        Size::Feature | Size::Hard => 2,
    };
    // Only hard steps reach the top level.
    let top = if size == Size::Hard { 3 } else { 2 };
    EFFORTS[(base + usize::from(failed_before)).min(top)]
}

/// From the person's own list (cheapest first), never a model off it.
// Cloud requests pick their model before the loop; applying this per step comes with the managed form.
#[cfg_attr(not(test), allow(dead_code))]
pub fn pick_model(size: Size, failed_before: bool, list: &[String]) -> Option<&str> {
    if list.is_empty() {
        return None;
    }
    let tier = match size {
        Size::Lookup | Size::Edit => 0,
        Size::Feature => 1,
        Size::Hard => 2,
    } + usize::from(failed_before);
    let i = (tier * (list.len() - 1)).div_ceil(3).min(list.len() - 1);
    Some(list[i].as_str())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Note {
    State,
    Decision,
    Index,
}

impl Note {
    fn file(self) -> &'static str {
        match self {
            Note::State => "state.md",
            Note::Decision => "decisions.md",
            Note::Index => "index.md",
        }
    }
}

/// ```notes state|decision|index blocks: what to write, and the reply without them.
pub fn take_notes(reply: &str) -> (Vec<(Note, String)>, String) {
    let mut notes = Vec::new();
    let mut clean = reply.to_string();
    for kind in [("state", Note::State), ("decision", Note::Decision), ("index", Note::Index)] {
        let tag = format!("notes {}", kind.0);
        for body in fenced(reply, &tag) {
            notes.push((kind.1, body.trim().to_string()));
            let block = format!("```{tag}\n{body}```");
            clean = clean.replace(&block, "");
        }
    }
    (notes, clean.trim().to_string())
}

/// State is replaced (it's the current picture); decisions and the index grow.
pub fn write_notes(data: &Path, chat_id: &str, notes: &[(Note, String)]) -> Result<(), String> {
    let dir = folder(data, chat_id);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    for (kind, body) in notes {
        let path = dir.join(kind.file());
        if *kind == Note::State {
            std::fs::write(&path, body).map_err(|e| e.to_string())?;
        } else {
            let mut old = std::fs::read_to_string(&path).unwrap_or_default();
            if !old.is_empty() && !old.ends_with('\n') {
                old.push('\n');
            }
            old.push_str(body);
            old.push('\n');
            std::fs::write(&path, old).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

pub fn save_plan(data: &Path, chat_id: &str, plan: &Plan) -> Result<(), String> {
    let dir = folder(data, chat_id);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("plan.json"), serde_json::to_string_pretty(plan).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

pub fn load_plan(data: &Path, chat_id: &str) -> Option<Plan> {
    serde_json::from_str(&std::fs::read_to_string(folder(data, chat_id).join("plan.json")).ok()?).ok()
}

pub struct Handoff {
    pub plan: String,
    pub state: String,
    pub next: String,
    pub questions: String,
}

impl Handoff {
    pub fn is_complete(&self) -> bool {
        [&self.plan, &self.state, &self.next].iter().all(|s| !s.trim().is_empty())
            // "none" is a real answer for open questions; empty is not.
            && !self.questions.trim().is_empty()
    }

    /// What stands in for the cleared history: the plan, the state, and only
    /// the index lines that mention the next step's words.
    pub fn from_notes(data: &Path, chat_id: &str, questions: &str) -> Option<Self> {
        let plan = load_plan(data, chat_id)?;
        let dir = folder(data, chat_id);
        let state = std::fs::read_to_string(dir.join("state.md")).unwrap_or_default();
        let next = plan.steps.iter().find(|s| s.state != StepState::Done).map(|s| s.text.clone()).unwrap_or_default();
        let plan_text = plan.steps.iter().enumerate().map(|(i, s)| format!("{}. [{:?}] {}", i + 1, s.state, s.text)).collect::<Vec<_>>().join("\n");
        Some(Self { plan: plan_text, state, next, questions: questions.to_string() })
    }

    pub fn text(&self, data: &Path, chat_id: &str) -> String {
        let index = std::fs::read_to_string(folder(data, chat_id).join("index.md")).unwrap_or_default();
        let words: Vec<String> = self.next.split_whitespace().filter(|w| w.len() > 3).map(str::to_lowercase).collect();
        let picked: Vec<&str> = index.lines().filter(|l| words.iter().any(|w| l.to_lowercase().contains(w))).collect();
        format!("plan:\n{}\nstate:\n{}\nnext: {}\nopen: {}\nindex:\n{}", self.plan, self.state.trim(), self.next, self.questions.trim(), picked.join("\n"))
    }
}

/// Clean break: every step done, or the context past the person's level.
pub fn should_clear(s: &Settings, plan: Option<&Plan>, context_share: f32) -> bool {
    s.on && (context_share >= s.clear_at || plan.is_some_and(|p| !p.steps.is_empty() && p.steps.iter().all(|st| st.state == StepState::Done)))
}

/// Reports by exception, for the managed form (not sent yet).
#[allow(dead_code)]
pub enum Report {
    Decision { question: String, recommend: String },
    Resource { what: String },
    Finished { evidence: Vec<String> },
    Problem { what: String },
}

pub fn prompt_section() -> &'static str {
    "\n\nThis chat manages itself. Before working, write a plan in a ```plan block: numbered steps, each tagged \
     [lookup], [edit], [feature] or [hard], each followed by lines that prove it is done: \
     `check: run <command>`, `check: file <path>` or `check: contains <path> :: <text>`. \
     A step is done only when its checks pass. Keep working notes in compact shorthand (fixed keys, paths and \
     function names, no prose): ```notes state (what is done, where things are), ```notes decision \
     (one line: step, choice, alternatives, why) and ```notes index (topic -> path:line). Update them at each \
     finished step. Don't repeat an action that failed; try something else or say what blocks you."
}

pub fn budget_note(state: BudgetState, s: &Settings) -> Option<String> {
    match state {
        BudgetState::Fine => None,
        BudgetState::Warn => Some("Most of this job's budget is used. Finish the current step, update the state note, and stop at a clean point.".into()),
        BudgetState::Spent => Some(format!(
            "This job reached its budget ({} tokens or {} minutes, set in Manage itself), so I'm pausing here. Say \"continue\" to keep going.",
            s.token_budget, s.minutes_budget
        )),
    }
}

pub fn repeat_note(tool: &str, why: &str, limit: u32) -> String {
    format!("The same step failed {limit} times ({tool}: {why}). I stopped instead of trying it again. Tell me how you'd like to go on.")
}

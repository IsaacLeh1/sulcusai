// SPDX-License-Identifier: AGPL-3.0-only
//! Tools the model can call, what each one risks, and how they run.

mod app;
mod archives;
mod findx;
pub mod browser;
pub mod programs;
pub mod chats;
pub mod comms;
mod compute;
mod documents;
mod library;
pub(crate) mod fs;
pub(crate) mod guard;
pub mod allowances;
pub(crate) mod lookup;
pub(crate) mod media;
mod meetings;
mod memory;
mod notes;
mod personal;
mod scheduled;
pub(crate) mod shell;
mod site;
mod web;
mod writing;

use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

use rusqlite::Connection;

use serde::Serialize;
use serde_json::{json, Value};

use crate::checkpoint::Recorder;
use crate::crypto::Cipher;
use crate::engine::JobRef;
use crate::sandbox::Sandbox;

/// How much a tool can affect. Decides when the user is asked first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    /// Looks at files; changes nothing.
    Read,
    /// Creates, changes, moves or deletes files (undoable).
    Write,
    /// Runs a program; effects can't be undone automatically.
    Execute,
    /// Saves to the assistant's own memory, which the user can review and edit.
    Memory,
    /// A tool from a connector (MCP server); what it does is up to that program.
    Connector,
    /// Reads or works another program on this PC (presses, types, keys).
    Programs,
    /// Submits a form, buys, sends, posts or deletes on a website. Always
    /// asks, in every mode, and is never allowed for a whole chat.
    Submit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Plan,
    Auto,
    Bypass,
}

impl Mode {
    pub fn parse(s: &str) -> Mode {
        match s {
            "plan" => Mode::Plan,
            "bypass" => Mode::Bypass,
            _ => Mode::Auto,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    Run,
    Ask,
    Refuse,
}

/// The run-mode rules. `allowed` holds risks the user already approved for this chat.
pub fn permission(mode: Mode, risk: Risk, allowed_for_chat: bool) -> Permission {
    match (mode, risk) {
        (_, Risk::Read | Risk::Memory) => Permission::Run,
        (Mode::Plan, _) => Permission::Refuse,
        (_, Risk::Submit) => Permission::Ask,
        (Mode::Bypass, _) => Permission::Run,
        (Mode::Auto, _) if allowed_for_chat => Permission::Run,
        (Mode::Auto, _) => Permission::Ask,
    }
}

/// A call the user sees before it runs even in Auto mode: deleting, a
/// command the guard flags (`guard::asks_why`), and the calls that reach
/// past this PC's files and say they ask first: signing in to an account,
/// a task that runs again and again, an event in the person's calendar, and
/// replacing a note's whole text (notes have no Undo).
pub fn destructive_call(name: &str, args: &Value) -> bool {
    match name {
        "delete_path" | "delete_chat" | "connect_account" | "schedule_task" | "calendar_create_event" | "update_note" => true,
        "run_command" => args.get("command").and_then(Value::as_str).is_none_or(guard::destructive),
        _ => false,
    }
}

/// What a call would send off the PC that needs the person's word first
/// (`guard::egress`): a command that uploads, posts, pushes, copies to
/// another machine or reaches the local network.
pub(crate) fn egress_call(name: &str, args: &Value, sb: &crate::sandbox::Sandbox) -> Option<guard::Egress> {
    match name {
        "run_command" => {
            let command = args.get("command").and_then(Value::as_str)?;
            let dir = shell::workdir(args, sb).ok();
            guard::egress(command, dir.as_deref())
        }
        _ => None,
    }
}

/// The card for a command that sends something off the PC: what goes and
/// where, plainly, and a warning when it's a secret.
pub(crate) fn egress_preview(e: &guard::Egress, command: &str) -> Preview {
    let (title, note) = match &e.secret {
        Some(kind) => (
            format!("Warning: this would send {kind} to {}", e.to),
            format!("It holds a secret. Allow it only if you mean to give it to {}.", e.to),
        ),
        None => (format!("Send {} to {}", e.what.join(", "), e.to), "Asking because this sends something off your PC.".to_string()),
    };
    Preview { title, kind: "command", detail: Some(command.to_string()), note: Some(note) }
}

pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub risk: Risk,
    pub params: fn() -> Value,
}

/// Tools for making something on the PC and checking it: on a small memory,
/// a request to build is offered only these (every tool's description took
/// more than half of an 8K window, leaving no room for the files).
pub fn for_building(name: &str) -> bool {
    (matches!(group(name), "files" | "web") && !matches!(name, "wikipedia" | "research" | "market_quote" | "find_places")) || matches!(name, "request_folder" | "open_path")
}

/// Lookups on this PC that change nothing, ask nothing, send nothing off
/// it and don't use the graphics card: several in one step run at the same
/// time (`Turn::run`). Web lookups aren't here: they're reviewed before they
/// go, and in privacy mode that asks. Nor is searching documents (its
/// embedding model may be on the card).
pub fn parallel_safe(name: &str) -> bool {
    matches!(
        name,
        "list_dir" | "read_file" | "find_files" | "search_files" | "search_memory" | "search_notes" | "read_note" | "search_meetings" | "read_meeting" | "read_document" | "search_chats" | "read_chat" | "read_chat_notes" | "calculate"
    )
}

/// The most of a tool's result the model gets, in characters, before
/// `fit_result`'s share of its memory (whichever is smaller). Listings and
/// searches rarely need more; a file or a page can use more, and can be
/// read on from where it was cut.
pub fn result_limit(name: &str) -> usize {
    match name {
        "read_file" | "read_document" | "read_note" | "read_meeting" | "read_chat" => 60_000,
        "fetch_page" | "browser_read" | "research" => 24_000,
        "search_files" | "find_files" | "web_search" | "search_documents" | "search_notes" | "search_chats" | "search_memory" | "email_search" => 12_000,
        "list_dir" => 8_000,
        "command_output" | "run_command" => 16_000,
        _ => 40_000,
    }
}

/// Checks a call's arguments against its declared parameters before
/// anything is asked or run: required fields present, and each given field
/// the declared JSON type. What's plainly meant is fixed in place (`"10"`
/// or `"[10]"` for a number, `3.0` for a whole number, `7` for text), since
/// a tool reading `as_u64` would otherwise skip it without a word. A list
/// given as text is left for the tool, which splits it. Anything else goes
/// back to the model to fix.
pub fn check_args(def: &ToolDef, args: &mut Value) -> Result<(), String> {
    let params = (def.params)();
    let Some(given) = args.as_object_mut() else {
        return Err("The arguments must be a JSON object of named fields.".into());
    };
    let missing: Vec<&str> = params["required"]
        .as_array()
        .map(|r| r.iter().filter_map(Value::as_str).filter(|k| given.get(*k).is_none_or(Value::is_null)).collect())
        .unwrap_or_default();
    if !missing.is_empty() {
        return Err(format!("{} needs {}.", def.name, missing.join(", ")));
    }
    let mut wrong = Vec::new();
    for (key, value) in given.iter_mut() {
        let types: Vec<&str> = match &params["properties"][key.as_str()]["type"] {
            Value::String(t) => vec![t.as_str()],
            Value::Array(ts) => ts.iter().filter_map(Value::as_str).collect(),
            _ => continue,
        };
        if value.is_null() || types.iter().any(|t| is_type(value, t)) {
            continue;
        }
        if types.contains(&"array") && (value.is_string() || value.is_object()) {
            if let Some(Ok(list @ Value::Array(_))) = value.as_str().map(serde_json::from_str::<Value>) {
                *value = list;
            }
            continue;
        }
        match types.iter().find_map(|t| fixed(value, t)) {
            Some(v) => *value = v,
            None => wrong.push(format!("{key} should be {}", types.iter().map(|t| a_type(t)).collect::<Vec<_>>().join(" or "))),
        }
    }
    if wrong.is_empty() { Ok(()) } else { Err(format!("In {}: {}.", def.name, wrong.join("; "))) }
}

fn is_type(v: &Value, t: &str) -> bool {
    match t {
        "string" => v.is_string(),
        "integer" => v.is_i64() || v.is_u64(),
        "number" => v.is_number(),
        "boolean" => v.is_boolean(),
        "array" => v.is_array(),
        "object" => v.is_object(),
        _ => true,
    }
}

/// The value as type `t`, when that's plainly what was meant.
fn fixed(v: &Value, t: &str) -> Option<Value> {
    let text = v.as_str().map(str::trim);
    match t {
        "string" => matches!(v, Value::Number(_) | Value::Bool(_)).then(|| Value::String(v.to_string())),
        "integer" => match v {
            Value::Number(n) => n.as_f64().filter(|f| f.fract() == 0.0 && f.abs() < 9e15).map(|f| Value::from(f as i64)),
            _ => text?.trim_matches(['[', ']', '#']).parse::<i64>().ok().map(Value::from),
        },
        "number" => text?.parse::<f64>().ok().and_then(serde_json::Number::from_f64).map(Value::Number),
        "boolean" => match text?.to_ascii_lowercase().as_str() {
            "true" => Some(Value::Bool(true)),
            "false" => Some(Value::Bool(false)),
            _ => None,
        },
        "object" => serde_json::from_str::<Value>(text?).ok().filter(Value::is_object),
        _ => None,
    }
}

fn a_type(t: &str) -> &'static str {
    match t {
        "string" => "text",
        "integer" => "a whole number",
        "number" => "a number",
        "boolean" => "true or false",
        "array" => "a list",
        "object" => "an object",
        _ => "another type",
    }
}

/// Tools for making pictures, video and music (and looking up what things
/// look like): what a small memory is offered for a picture request, so the
/// conversation (what the person wanted from the start) still fits.
pub fn for_pictures(name: &str) -> bool {
    matches!(group(name), "media" | "web") && !matches!(name, "request_api_key" | "research" | "market_quote" | "find_places")
}

/// What a small memory leaves out first when the instructions and every tool
/// don't fit with room for the chat (an 8K model with every feature on
/// couldn't even start), in order, each with what a request says when it does
/// need them: those stay. Names ending in "_" are prefixes. Never left out:
/// calculate, the web search and files tools, and connectors.
pub static SPARE: std::sync::LazyLock<Vec<(&'static [&'static str], regex::Regex)>> = std::sync::LazyLock::new(|| {
    let g = |p: &str| regex::Regex::new(&format!("(?i){p}")).unwrap();
    vec![
        (&["delegate"][..], g(r"\bhelpers?\b|delegate")),
        (&["for_each"][..], g(r"\beach\b|\bevery\b|\ball (the |my )?(files|rows|items|documents|pages)\b")),
        (&["search_chats", "read_chat", "message_chat", "delete_chat"], g(r"\bchats?\b|conversation")),
        (&["ask_person", "work_with_person", "message_person"], g(r"\bassistant\b|\btogether\b|\bwith (my|our) \w+|\bmessage\b|\btext\b|\btell\b")),
        (&["search_meetings", "read_meeting"], g(r"meeting|transcript")),
        (&["browser_"], g(r"browser|log ?in|sign ?in|click|fill (in|out)|web ?page|go to|book |order |buy ")),
        // People ask about their own life without saying "email": "when is
        // Sam's party?", "what did my mom ask me?".
        (&["email_"], g(r"e-?mails?|inbox|\bmail\b|messages? from|reply to|send (it|this|a message)|\b(asked|sent|wrote|told|invited) (me|us)\b|\binvit|\bparty\b|\bmy (mom|mum|dad|mother|father|parents|wife|husband|partner|boss|friend|brother|sister|son|daughter|kids?|landlord|teacher|doctor)\b")),
        (&["calendar_"], g(r"calendar|meeting|events?\b|appointment|schedule|free time|busy|tomorrow|today|tonight|next week|this week|\bparty\b|birthday|wedding|dinner|\bwhen is\b|\bwhen'?s\b")),
        (&["create_note", "search_notes", "read_note", "update_note"], g(r"\bnotes?\b|jot|write (it |that )?down")),
        (&["personal_records", "personal_add"], g(r"package|deliver|tracking|booking|flight|hotel|\btrips?\b|birthday|application|interview|calories|\bate\b|workout|exercise|\bran\b|weight|sleep|pulse|recipe|\bmeals?\b|\bcar\b|plants?\b|\bwater\b|\bpets?\b|\bdog\b|\bcat\b|journal|meditat|\blog\b")),
        (&["create_task", "list_tasks", "complete_task"], g(r"\btasks?\b|to-?dos?|remind")),
        (&["schedule_task"], g(r"\bevery\b|\beach (day|morning|evening|night|week)|daily|weekly|hourly|remind me|schedule")),
        (&["create_image", "edit_image", "create_video", "create_music"], g(r"picture|image|photo|draw|paint|sketch|logo|wallpaper|video|clip|animat|song|music|sound|lyrics")),
        (&["research"], g(r"research|compare|comparison|in.depth|report|pros and cons|\b(best|better|worse)\b|which (one )?(is|should)|\bvs\.?\b|versus")),
        (&["market_quote"], g(r"stock|shares?\b|price|crypto|bitcoin|ethereum|currenc|exchange rate|\bmarkets?\b|\bindex\b|nasdaq|s&p|\bdow\b|\bgold\b|silver|\boil\b|forex")),
        (&["create_document", "read_document"], g(r"document|\bword\b|docx|pdf|excel|xlsx|spreadsheet|powerpoint|pptx|slides?\b|resume|letter")),
        (&["search_documents"], g(r"document|\bfiles?\b|pdf|report|contract|lease|policy|manual|notes|library|according to|\bin (my|our|the) \w+")),
        (&["run_code"], g(r"chart|graph|plot|csv|json|\bdata\b|analy|\bcode\b|script|javascript|\bsort|\bcount|statistic|average|median")),
        (&["app_status", "open_page", "connect_account"], g(r"sulcus|settings|connect|account|install|\bmodels?\b|feature")),
        (&["move_path", "make_folder", "delete_path", "command_output", "stop_command", "find_files", "search_files"], g(r"\bmove|rename|folder|delete|remove|\bfind|search|files?\b|server|running")),
        (&["weather"], g(r"weather|forecast|temperature|\brain|snow|wind|sunny|\bhot\b|\bcold\b")),
        (&["find_places"], g(r"\bnear\b|nearby|nearest|closest|close to|around (here|me)|where('?s| is| can i)|address|directions|\bmaps?\b|places?\b|restaurants?|coffee|caf[eé]|\bgas\b|pharmacy|hospital|store|shop|open now|how far")),
        (&["news_feeds"], g(r"news|headlines?|brief|feeds?")),
        (&["wikipedia"], g(r"who (is|was|are|were)|what (is|was|are|were)|wiki|history of|tell me about|meaning of")),
        (&["remember", "search_memory"], g(r"remember|forget|recall|my name|about me|last time")),
    ]
});

/// Whether a tool is one of a SPARE group's.
pub fn in_group(name: &str, group: &[&str]) -> bool {
    group.iter().any(|g| if g.ends_with('_') { name.starts_with(g) } else { name == *g })
}

/// Which group a tool belongs to; groups are offered only when available.
pub(crate) fn group(name: &str) -> &'static str {
    match name {
        "remember" | "search_memory" => "memory",
        "read_chat_notes" | "update_chat_notes" | "check_plan_step" => "chatnotes",
        "load_skill" => "skills",
        "search_meetings" | "read_meeting" => "meetings",
        "create_note" | "search_notes" | "read_note" | "update_note" => "notes",
        "create_task" | "list_tasks" | "complete_task" => "tasks",
        "personal_records" | "personal_add" => "personal",
        "web_search" | "fetch_page" | "request_web" | "weather" | "request_api_key" | "wikipedia" | "research" | "market_quote" | "find_places" | "news_feeds" => "web",
        "calculate" | "run_code" => "compute",
        n if n.starts_with("browser_") => "browser",
        n if n.starts_with("program_") || n == "programs_list" || n == "use_screen" => "programs",
        "create_document" | "read_document" => "documents",
        "search_documents" => "library",
        n if n.starts_with("email_") => "email",
        n if n.starts_with("calendar_") => "calendar",
        n if n.starts_with("cloud_file") => "cloud_files",
        "create_image" | "edit_image" | "create_video" | "create_music" => "media",
        "app_status" | "connect_account" | "open_page" | "request_folder" | "open_path" | "schedule_task" => "app",
        "ask_person" | "work_with_person" | "message_person" => "people",
        "search_chats" | "read_chat" | "message_chat" | "delete_chat" => "chats",
        "check_writing" | "check_scam" | "pc_health" | "score_practice" => "writing",
        _ => "files",
    }
}

pub const TOOLS: &[ToolDef] = &[
    ToolDef { name: "check_writing", risk: Risk::Read, params: writing::check_writing_params,
        description: "Check a text on this PC: reading ease, school grade, tone and grammar slips with fixes. Use it when the user asks how their writing reads or to proofread." },
    ToolDef { name: "check_scam", risk: Risk::Read, params: writing::check_scam_params,
        description: "Look for the signs scams share in a message or link (pressure, unusual payments, asks for codes, lookalike links, mismatched senders). Runs on this PC. Never call a message safe; say what was or wasn't found." },
    ToolDef { name: "pc_health", risk: Risk::Read, params: writing::pc_health_params,
        description: "Check this PC's disk space, memory, swap and time since the last restart, with advice. Changes nothing." },
    ToolDef { name: "score_practice", risk: Risk::Memory, params: writing::score_practice_params,
        description: "When practising a language with the user: score their translation against your reference, word by word, and record it in their progress. Give the English sentence first, wait for their answer, then call this." },
    ToolDef { name: "app_status", risk: Risk::Read, params: app::app_status_params,
        description: "What's set up in SulcusAI: connected email and calendar accounts, how many emails are copied, the internet mode and which features are on." },
    ToolDef { name: "connect_account", risk: Risk::Write, params: app::connect_account_params,
        description: "Connect the user's Gmail/Google Calendar or Outlook/Microsoft 365 by opening that sign-in page in their browser. The user is asked first and signs in themselves; wait for the result." },
    ToolDef { name: "schedule_task", risk: Risk::Write, params: scheduled::schedule_task_params,
        description: "Set up something to happen by itself at a set time, once or again and again, when the user asks for that (\"every morning at 8 give me the news\", \"message Josh at 12 AM tomorrow: happy birthday\", \"make me a song Friday at 6\"). For one time, use repeat once with date and time; write prompt as exactly what to do then, and list in actions what it will send or make (message_person, email_send, calendar_create_event, create_music, …). If anything isn't clear (who, the exact time, what to say, or what it may do by itself), ask the user first instead of guessing. The user approves it before it's saved." },
    ToolDef { name: "calculate", risk: Risk::Read, params: compute::calculate_params,
        description: "Work out math exactly: arithmetic, percentages, powers, roots, money and dates. Use it for every calculation instead of doing it in your head." },
    ToolDef { name: "run_code", risk: Risk::Read, params: compute::run_code_params,
        description: "Run JavaScript on this PC to work something out: longer calculations, counting, dates, sorting or analysing data (including CSV or JSON files in shared folders), and charts shown in the chat. No internet." },
    ToolDef { name: "open_page", risk: Risk::Read, params: app::open_page_params,
        description: "Show the user a page of SulcusAI, e.g. mail to add another kind of email account, or models to install models." },
    ToolDef { name: "list_dir", risk: Risk::Read, params: fs::list_dir_params,
        description: "List a folder's contents. With no path, lists the shared folders." },
    ToolDef { name: "read_file", risk: Risk::Read, params: fs::read_file_params,
        description: "Read a text file. Each line starts with its number and a tab; the numbers are not part of the file, so never copy them into edits. Use start_line/end_line for long files." },
    ToolDef { name: "find_files", risk: Risk::Read, params: fs::find_files_params,
        description: "Find files by name with a glob such as **/*.rs or *report*. Respects .gitignore." },
    ToolDef { name: "search_files", risk: Risk::Read, params: fs::search_files_params,
        description: "Search file contents with a regular expression. Returns path:line: text matches. Respects .gitignore." },
    ToolDef { name: "find_indexed", risk: Risk::Read, params: findx::find_indexed_params,
        description: "Search the Find index (folders the person added under Settings, Find): file names, document text and picture descriptions. Fast, but only covers indexed folders." },
    ToolDef { name: "cite_pdf", risk: Risk::Read, params: findx::cite_pdf_params,
        description: "Find the passages of a PDF that answer a question, each marked with its page, so the answer can name its pages." },
    ToolDef { name: "list_archive", risk: Risk::Read, params: archives::list_archive_params,
        description: "List what's inside a .zip, .tar, .tar.gz or .tgz archive in a shared folder, without unpacking it." },
    ToolDef { name: "extract_archive", risk: Risk::Write, params: archives::extract_archive_params,
        description: "Unpack a .zip, .tar, .tar.gz or .tgz archive into a folder (by default one named after it, beside it). Entries that would land outside that folder stop it." },
    ToolDef { name: "write_file", risk: Risk::Write, params: fs::write_file_params,
        description: "Create a file, or replace a file's entire contents. Prefer edit_file for changes to existing files." },
    ToolDef { name: "edit_file", risk: Risk::Write, params: fs::edit_file_params,
        description: "Replace exact text in a file. old_text must match exactly (including spaces) and be unique unless replace_all is true; empty old_text adds new_text at the end. Read the file first." },
    ToolDef { name: "move_path", risk: Risk::Write, params: fs::move_path_params,
        description: "Move or rename a file or folder." },
    ToolDef { name: "make_folder", risk: Risk::Write, params: fs::make_folder_params,
        description: "Create a folder (and any missing parent folders)." },
    ToolDef { name: "delete_path", risk: Risk::Write, params: fs::delete_path_params,
        description: if cfg!(windows) { "Move a file or folder to the Recycle Bin." } else { "Move a file or folder to the Trash." } },
    ToolDef { name: "run_command", risk: Risk::Execute, params: shell::run_command_params,
        description: if cfg!(windows) {
            "Run a PowerShell command in a shared folder and return its output, for example to build, test, use git or run scripts. Runs hidden; no interactive input."
        } else {
            "Run a bash command in a shared folder and return its output, for example to build, test, use git or run scripts. No interactive input."
        } },
    ToolDef { name: "command_output", risk: Risk::Read, params: shell::command_output_params,
        description: "What a command started with background: true has printed since, and whether it's still running." },
    ToolDef { name: "stop_command", risk: Risk::Read, params: shell::stop_command_params,
        description: "Stop a command started with background: true (and anything it started)." },
    ToolDef { name: "request_folder", risk: Risk::Read, params: request_folder_params,
        description: "Ask the user for a folder to work in when you need files and none fits: to build or change something on their PC (a website, an app, documents), or to reach a folder that isn't shared. They approve it in a popup (and can pick another); then the file tools and run_command work there in this same reply." },
    ToolDef { name: "preview_site", risk: Risk::Read, params: site::preview_site_params,
        description: "Show a website you built: serves its folder on this PC, opens it in a Preview panel beside the chat (the user can open it in their browser from there), and checks it in a hidden window for script errors, missing files, blocked outside requests and buttons that do nothing. Call it after writing a site, fix every problem it reports, and call it again." },
    ToolDef { name: "open_path", risk: Risk::Write, params: open_path_params,
        description: "Open a file from a shared folder, or a web address, for the user: in their browser or the app that opens it. Use it to show them what you made, e.g. the site you just started at http://localhost:5173. Never runs programs." },
    ToolDef { name: "remember", risk: Risk::Memory, params: memory::remember_params,
        description: "Save one lasting fact about the user, their work or their preferences, to recall in future chats. One short sentence. Not about a one-off task (a file you just made or ran). Never save secrets." },
    ToolDef { name: "search_memory", risk: Risk::Read, params: memory::search_memory_params,
        description: "Search what you remember from earlier chats." },
    ToolDef { name: "search_meetings", risk: Risk::Read, params: meetings::search_meetings_params,
        description: "Search the user's recorded meetings (titles, notes and transcripts) by words. Returns matching meetings with ids." },
    ToolDef { name: "read_meeting", risk: Risk::Read, params: meetings::read_meeting_params,
        description: "Read a recorded meeting's notes (summary, decisions, action items) or its full transcript, by id." },
    ToolDef { name: "personal_records", risk: Risk::Read, params: personal::personal_records_params,
        description: "Look up the user's Personal records of one kind (deliveries, bookings, birthdays, applications, food, workouts, health log, car, plants, pets, journal...)." },
    ToolDef { name: "personal_add", risk: Risk::Memory, params: personal::personal_add_params,
        description: "Add a Personal record when the user asks to log something: food, a workout, a health reading, a plant, a pet, a journal entry, a contact." },
    ToolDef { name: "create_note", risk: Risk::Memory, params: notes::create_note_params,
        description: "Save a note in the user's Notes (Markdown). Use when they ask you to note, jot down or save something." },
    ToolDef { name: "search_notes", risk: Risk::Read, params: notes::search_notes_params,
        description: "Find the user's notes by words. Returns titles, ids and a preview." },
    ToolDef { name: "read_note", risk: Risk::Read, params: notes::read_note_params,
        description: "Read one of the user's notes in full, by id." },
    ToolDef { name: "update_note", risk: Risk::Write, params: notes::update_note_params,
        description: "Replace a note's text (read it first, then send the whole new text)." },
    ToolDef { name: "create_task", risk: Risk::Memory, params: notes::create_task_params,
        description: "Add a task to the user's to-do list, with an optional due date, priority and reminder." },
    ToolDef { name: "list_tasks", risk: Risk::Read, params: notes::list_tasks_params,
        description: "List the user's tasks with their ids and due dates." },
    ToolDef { name: "complete_task", risk: Risk::Memory, params: notes::complete_task_params,
        description: "Mark one of the user's tasks done, by id." },
    ToolDef { name: "web_search", risk: Risk::Read, params: web::web_search_params,
        description: "Search the web for current information. Returns titles, addresses and snippets to cite." },
    ToolDef { name: "weather", risk: Risk::Read, params: web::weather_params,
        description: "Current weather and the forecast for a place, with exact temperatures. Use it for any weather question instead of web_search." },
    ToolDef { name: "wikipedia", risk: Risk::Read, params: lookup::wikipedia_params,
        description: "Look up a person, place, thing, event or term in Wikipedia: quick, reliable facts. Use it for who or what something is." },
    ToolDef { name: "research", risk: Risk::Read, params: lookup::research_params,
        description: "Research a question in depth: runs several web searches at once, reads the best pages and returns what they say with their addresses. Use it for questions that need more than one search (comparisons, reports, recommendations, recent events)." },
    ToolDef { name: "market_quote", risk: Risk::Read, params: lookup::market_quote_params,
        description: "Live prices for stocks, funds, indexes, crypto, currencies and commodities (with a chart of how they moved when given a range). Use it for any price or exchange rate instead of web_search." },
    ToolDef { name: "find_places", risk: Risk::Read, params: lookup::find_places_params,
        description: "Find places on the map from OpenStreetMap: kinds of places near the user or a city (\"coffee\", \"gas station\", \"pharmacy\"), a business or landmark, or an address, with distances, hours and map and directions links. Use it for anything near, nearest or where something is. It knows where the user is: don't ask them." },
    ToolDef { name: "news_feeds", risk: Risk::Read, params: lookup::news_feeds_params,
        description: "The newest headlines from the news feeds the user saved in the Library (for the morning brief or \"what's in the news\")." },
    ToolDef { name: "fetch_page", risk: Risk::Read, params: web::fetch_page_params,
        description: "Read a web page as text, by its address. Use it on search results to get the details." },
    ToolDef { name: "request_web", risk: Risk::Read, params: web::request_web_params,
        description: "Web access is off in this chat. If the task needs current information from the internet, or something the user mentions that you don't know or aren't sure about (a name, a game, a character, a term), call this: the user is asked in a popup. If they allow it, web_search, fetch_page and weather are available right away to finish the task; if not, answer without the web." },
    ToolDef { name: "request_api_key", risk: Risk::Read, params: site::request_api_key_params,
        description: "Only when a site you're building needs a service that requires an API key (the free sources under Live data need none): call this instead of writing a key or a placeholder into the files, or asking for it in the chat. The user is told what the site connects to and what it needs, and gives the key in a popup (or says no). SulcusAI keeps it and adds it to the site's requests to that service through /__sulcus/fetch." },
    ToolDef { name: "browser_open", risk: Risk::Read, params: browser::browser_open_params,
        description: "Open a website in the built-in browser, which the user can watch. Use it to use a site (fill in a search, click through pages); for just reading, web_search and fetch_page are quicker. Returns the page's text and its links, buttons and fields, numbered." },
    ToolDef { name: "browser_read", risk: Risk::Read, params: browser::browser_read_params,
        description: "Read the browser's current page again: its text and its numbered links, buttons and fields." },
    ToolDef { name: "browser_click", risk: Risk::Read, params: browser::browser_click_params,
        description: "Click a link or button in the browser, by its number from the page. Anything that submits a form, buys, sends, posts or deletes asks the user first." },
    ToolDef { name: "browser_type", risk: Risk::Read, params: browser::browser_type_params,
        description: "Type into a field in the browser by its number (replacing what's there), or pick a dropdown choice. Never for passwords or payment details: ask the user to type those in the browser window." },
    ToolDef { name: "browser_back", risk: Risk::Read, params: browser::browser_back_params,
        description: "Go back to the previous page in the browser." },
    ToolDef { name: "browser_scroll", risk: Risk::Read, params: browser::browser_scroll_params,
        description: "Scroll the browser's page down or up (by screens), or to an element by its number, to see more of it. Returns the page again." },
    ToolDef { name: "browser_key", risk: Risk::Read, params: browser::browser_key_params,
        description: "Press a key in the browser's page: Escape to close a pop-up, Tab to move to the next field, arrows in a list or menu, PageDown to read on. Returns the page again." },
    ToolDef { name: "browser_text", risk: Risk::Read, params: browser::browser_none_params,
        description: "The whole page's text in the user's browser, for long articles and tables the numbered reading shortens. Works through the SulcusAI extension only." },
    ToolDef { name: "browser_tabs", risk: Risk::Read, params: browser::browser_none_params,
        description: "List SulcusAI's tabs in the user's browser (its own tab group only; the user's other tabs aren't shown), with ids." },
    ToolDef { name: "browser_switch_tab", risk: Risk::Read, params: browser::browser_switch_tab_params,
        description: "Make one of SulcusAI's tabs (an id from browser_tabs) this chat's tab. Returns the page." },
    ToolDef { name: "browser_new_tab", risk: Risk::Read, params: browser::browser_open_params,
        description: "Open a page in a new tab in SulcusAI's group in the user's browser, keeping the current one. Returns the page." },
    ToolDef { name: "browser_close_tab", risk: Risk::Read, params: browser::browser_none_params,
        description: "Close this chat's tab in the user's browser." },
    ToolDef { name: "browser_wait", risk: Risk::Read, params: browser::browser_wait_params,
        description: "Wait a few seconds for a page that's still loading or changing, then read it again." },
    ToolDef { name: "programs_list", risk: Risk::Programs, params: programs::programs_list_params,
        description: "List the program windows open on the user's PC, with their numbers." },
    ToolDef { name: "program_open", risk: Risk::Programs, params: programs::program_open_params,
        description: "Start a program on the user's PC by its Start-menu name (Notepad, Calculator, Excel...) and read its window." },
    ToolDef { name: "program_read", risk: Risk::Programs, params: programs::program_read_params,
        description: "Read a program's window: its text and its numbered buttons, fields, menus, tabs and list items. Use it on a window from programs_list, or to see the one used last again." },
    ToolDef { name: "program_click", risk: Risk::Programs, params: programs::program_click_params,
        description: "Press a button, menu item, tab, checkbox or list item in the program's window, by its number. Anything that sends, buys or deletes asks the user first. Returns the window again (or a box it opened)." },
    ToolDef { name: "program_type", risk: Risk::Programs, params: programs::program_type_params,
        description: "Type into a field or document in the program's window by its number. A field's text is replaced; a document gets the text added at the end (replace: true to put it in place of everything, which asks the user). It checks the text went in. Never for passwords: ask the user to type those." },
    ToolDef { name: "program_key", risk: Risk::Programs, params: programs::program_key_params,
        description: "Press keys in the program's window used last, e.g. ctrl+s to save, escape to close a menu, alt+f for the File menu, tab, enter." },
    ToolDef { name: "program_scroll", risk: Risk::Programs, params: programs::program_scroll_params,
        description: "Scroll the program's window, or a list or document in it by its number, down or up to see more." },
    ToolDef { name: "use_screen", risk: Risk::Programs, params: programs::use_screen_params,
        description: "Do a task on the user's screen by looking at it and using the mouse and keyboard like a person, step by step: for what program_read can't read (games, remote desktops, windows drawn as pictures). Slower than the program_* tools, so try those first. Give the whole task; it returns what it did and what the screen shows at the end." },
    ToolDef { name: "create_document", risk: Risk::Write, params: documents::create_document_params,
        description: "Make a Word (.docx), PDF, Markdown, Excel (.xlsx, with working formulas and charts) or PowerPoint (.pptx) file in a shared folder. Give markdown for documents, sheets for spreadsheets, slides for decks." },
    ToolDef { name: "search_documents", risk: Risk::Read, params: library::search_params,
        description: "Search the user's documents library (the folders and files they added: PDFs, Word, PowerPoint, Excel, notes) for passages that answer a question. Use it for questions about their own documents. Answer only from what it finds, and cite each fact with the source link it gives." },
    ToolDef { name: "read_document", risk: Risk::Read, params: documents::read_document_params,
        description: "Read the text of a Word, Excel, PowerPoint, PDF, CSV or text file in a shared folder (spreadsheets come back as tables with their formulas)." },
    ToolDef { name: "email_search", risk: Risk::Read, params: comms::email_search_params,
        description: "Search the user's email (a copy kept on this PC) by sender, subject or words. Returns ids, dates, senders and subjects; empty query lists the newest." },
    ToolDef { name: "email_read", risk: Risk::Read, params: comms::email_read_params,
        description: "Read one email in full, by id." },
    ToolDef { name: "email_send", risk: Risk::Submit, params: comms::email_send_params,
        description: "Send an email from the user's account. The user always sees it and approves before it goes. For replies pass reply_to_id and start the subject with Re:." },
    ToolDef { name: "calendar_events", risk: Risk::Read, params: comms::calendar_events_params,
        description: "List the user's calendar events for a day or range (default: the next 7 days)." },
    ToolDef { name: "calendar_free_time", risk: Risk::Read, params: comms::calendar_free_time_params,
        description: "Find free time between 9 and 5 on a day or range." },
    ToolDef { name: "calendar_create_event", risk: Risk::Write, params: comms::calendar_create_event_params,
        description: "Add an event to the user's calendar. Inviting people always asks the user first." },
    ToolDef { name: "cloud_files_search", risk: Risk::Read, params: crate::cloudfiles::search_params,
        description: "Search the user's Google Drive and OneDrive files by name or words in them. Returns ids, names, where each is and when it changed; empty query lists the most recent." },
    ToolDef { name: "cloud_file_read", risk: Risk::Read, params: crate::cloudfiles::read_params,
        description: "Read the text of one Google Drive or OneDrive file by its id from cloud_files_search (documents, spreadsheets, slides, PDFs and text)." },
    ToolDef { name: "create_image", risk: Risk::Memory, params: media::create_image_params,
        description: "Make a picture on this PC from a description. The picture appears in the chat." },
    ToolDef { name: "edit_image", risk: Risk::Memory, params: media::edit_image_params,
        description: "Change a picture from this chat by instruction (\"make it night\", \"add a hat\"), restyle it, make it 4x bigger (upscale) or remove its background. Makes a new picture; the original stays." },
    ToolDef { name: "create_video", risk: Risk::Memory, params: media::create_video_params,
        description: "Make a short video clip (a few seconds) on this PC from a description, or bring a picture to life. Takes a few minutes." },
    ToolDef { name: "create_music", risk: Risk::Memory, params: media::create_music_params,
        description: "Make a song on this PC (with lyrics and vocals, or instrumental, up to 10 minutes) or a sound effect: its style, mood, instruments, voice, tempo, key and beat can be set. Also works on a song already made: a cover in another style, a part made again, more after its end, a part added or taken out." },
    ToolDef { name: "load_skill", risk: Risk::Read, params: load_skill_params,
        description: "Read the instructions for one of your skills, by name, or one of the skill's own files." },
    ToolDef { name: "ask_person", risk: Risk::Submit, params: crate::sync::people::ask::ask_params,
        description: "Ask the assistant of someone the user is linked with in People. It answers on their PC, with their model and only what they allow, and they may need to approve first. Use it for what only they would know, like details from chats they share or their plans. The question goes to them, so include only what's needed. Waits up to 3 minutes." },
    ToolDef { name: "work_with_person", risk: Risk::Submit, params: crate::sync::people::together::work_params,
        description: "Work something out together with the assistant of someone the user is linked with in People, such as a time to meet: the two assistants take turns, each with its own user's information, until they agree (up to 8 turns). Give the goal and what you already know. Both people can watch it in People." },
    ToolDef { name: "message_person", risk: Risk::Submit, params: crate::sync::people::message_params,
        description: "Send someone the user is linked with in People a message from the user, when the user asks for that. The user sees it before it goes (a timed action sends what they approved when they set it up)." },
    ToolDef { name: "read_chat_notes", risk: Risk::Read, params: app::app_status_params,
        description: "Read this chat's notes: what the user asked for and what's left, what's known about the project, and the plan." },
    ToolDef { name: "update_chat_notes", risk: Risk::Memory, params: crate::chatnotes::update_params,
        description: "Write in this chat's notes (its own memory, shown to the user): what they asked for and what's still to do, what you learn about the project, or a plan of steps (\"- [ ] step\", with what each involves). A section is replaced, or added to with add." },
    ToolDef { name: "check_plan_step", risk: Risk::Memory, params: crate::chatnotes::check_params,
        description: "Check off a step of the plan in this chat's notes once it's done (by its number or words from it)." },
    ToolDef { name: "search_chats", risk: Risk::Read, params: chats::search_chats_params,
        description: "Look through the user's other chats on this PC: titles and what was said. Leave the query out to list the latest ones." },
    ToolDef { name: "read_chat", risk: Risk::Read, params: chats::read_chat_params,
        description: "Read the latest messages of another of the user's chats." },
    ToolDef { name: "message_chat", risk: Risk::Read, params: chats::message_chat_params,
        description: "Ask or tell the assistant in another of the user's chats (or a new chat) something: it works on it there, with its own conversation and tools, and its reply comes back to you. Use it to split work or to use what that chat knows. One at a time; give it everything it needs." },
    ToolDef { name: "delete_chat", risk: Risk::Write, params: chats::delete_chat_params,
        description: "Delete one of the user's other chats, only when they ask you to. They always see which chat and approve it first; it can't be undone." },
    ToolDef { name: "for_each", risk: Risk::Read, params: for_each_params,
        description: "Do the same instruction for each item of a list (files, rows, names; up to 30 at a time), each by its own helper with a fresh context, and get all their reports back. Use it when the user wants one thing done for every file in a folder or every row of a list: get the items first (for example with list_files), then pass them. Helpers only read; make any changes yourself afterwards." },
    ToolDef { name: "delegate", risk: Risk::Read, params: delegate_params,
        description: "Hand a self-contained research task to a helper agent, such as finding where something is defined or summarizing a set of files. The helper has its own fresh context and read-only tools, and returns a short report. Use it for digging that would otherwise fill this conversation." },
];

fn load_skill_params() -> Value {
    json!({ "type": "object", "required": ["name"], "properties": {
        "name": { "type": "string" },
        "file": { "type": "string", "description": "One of the skill's own files to read instead (as its instructions name it, e.g. references/forms.md)." } } })
}

fn delegate_params() -> Value {
    json!({ "type": "object", "required": ["task"], "properties": {
        "task": { "type": "string", "description": "What the helper should find out, with any details it needs. It can't see this conversation." } } })
}

fn for_each_params() -> Value {
    json!({ "type": "object", "required": ["instruction", "items"], "properties": {
        "instruction": { "type": "string", "description": "What to do for each item, with everything a helper needs. Helpers can't see this conversation." },
        "items": { "type": "array", "items": { "type": "string" }, "description": "The items, each a file path (starting with the folder name), a row or a name." } } })
}

/// Tools a helper agent may use: read-only, and no further helpers.
pub fn helper_definitions(files: bool) -> Value {
    let mut defs = definitions(Mode::Plan, files, false);
    if let Some(list) = defs.as_array_mut() {
        list.retain(|d| !matches!(d["function"]["name"].as_str(), Some("delegate" | "for_each" | "stop_command" | "preview_site")));
    }
    defs
}

fn request_folder_params() -> Value {
    json!({ "type": "object", "required": ["reason"], "properties": {
        "reason": { "type": "string", "description": "In a few words, what it's for, e.g. \"build your portfolio website\"." },
        "path": { "type": "string", "description": "The folder you'd use: a full path, or a new folder's name to make on their Desktop (e.g. \"portfolio-site\"). Leave it out to suggest the Desktop." } } })
}

fn open_path_params() -> Value {
    json!({ "type": "object", "required": ["target"], "properties": {
        "target": { "type": "string", "description": "A file or folder in a shared folder (e.g. site/index.html), or a web address starting with http:// or https://." } } })
}

/// Sound and video open_path may open too, beside the documents, pages and
/// pictures the app's own Open allows (`docs::OPENABLE`).
const OPENS_MEDIA: &[&str] = &["mp3", "wav", "ogg", "m4a", "flac", "mp4", "webm", "mov"];

/// Why open_path won't open this file, if it won't: only folders and the
/// kinds above. Anything else may run something when opened (programs,
/// scripts, shortcuts, consoles, installers), however it's named.
pub(crate) fn not_openable(path: &std::path::Path) -> Option<&'static str> {
    if path.is_dir() {
        return None;
    }
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if crate::docs::OPENABLE.contains(&ext.as_str()) || OPENS_MEDIA.contains(&ext.as_str()) {
        return None;
    }
    Some("Only folders, documents, web pages, pictures, sound and video can be opened. To run something, use run_command, where the user sees what runs.")
}

/// open_path: shows the user a file (in its usual app) or a web address
/// (in their browser).
fn open_path(args: &Value, ctx: &Ctx<'_>) -> Outcome {
    let target = match arg_str(args, "target") {
        Ok(t) => t.trim().to_string(),
        Err(e) => return Outcome::error("Open something", e),
    };
    let title = format!("Opened {target}");
    if target.starts_with("http://") || target.starts_with("https://") {
        if let Err(e) = crate::offline::check(crate::offline::Keep::Web) {
            return Outcome::error("Open something", e);
        }
        // The same rule as the browser tools: the connectivity level, or this chat's globe.
        if !crate::browser::allowed_for_chat(&ctx.memory.db.lock().unwrap(), ctx.memory.chat_id) {
            return Outcome::error(title, "This chat is offline, so web pages can't be opened from it. Give the user the address to open themselves.");
        }
        let url = match reqwest::Url::parse(&target) {
            Ok(u) => u,
            Err(_) => return Outcome::error(title, format!("That isn't a web address: {target}")),
        };
        return match crate::open_with_system(std::ffi::OsStr::new(url.as_str())) {
            Ok(()) => Outcome::ok(format!("Opened {target} in the user's browser."), title, "text", None),
            Err(e) => Outcome::error(title, e),
        };
    }
    let path = match ctx.sandbox.resolve(&target) {
        Ok(p) => p,
        Err(e) => return Outcome::error(title, e),
    };
    if !path.exists() {
        return Outcome::error(title, format!("{} doesn't exist.", ctx.sandbox.display(&path)));
    }
    if let Some(why) = not_openable(&path) {
        return Outcome::error(title, why);
    }
    match crate::open_with_system(path.as_os_str()) {
        Ok(()) => Outcome::ok(format!("Opened {} for the user.", ctx.sandbox.display(&path)), title, "text", None),
        Err(e) => Outcome::error(title, e),
    }
}

pub fn find(name: &str) -> Option<&'static ToolDef> {
    TOOLS.iter().find(|t| t.name == name)
}

/// OpenAI-style definitions for the tools this mode may use. File tools
/// need a shared folder; memory tools need memory turned on.
pub fn definitions(mode: Mode, files: bool, memory: bool) -> Value {
    let list: Vec<Value> = TOOLS
        .iter()
        .filter(|t| permission(mode, t.risk, false) != Permission::Refuse)
        .filter(|t| match group(t.name) {
            "memory" => memory,
            // Added by the agent when a plugin provides skills, or when there are meetings.
            "skills" | "meetings" | "notes" | "tasks" | "personal" | "web" | "browser" | "programs" | "email" | "calendar" | "cloud_files" | "documents" | "library" | "media" | "app" | "people" | "chats" | "chatnotes" | "compute" | "writing" => false,
            _ => files,
        })
        .map(|t| json!({ "type": "function", "function": { "name": t.name, "description": t.description, "parameters": (t.params)() } }))
        .collect();
    Value::Array(list)
}

/// Whether any command the assistant started is still running (an update
/// waits for them).
pub fn commands_running() -> bool {
    shell::running()
}

/// What SulcusAI is and what's connected, for the system prompt.
pub fn app_note(state: &crate::AppState, c: &Cipher) -> String {
    app::note(state, c)
}

/// What the approval card shows before a risky tool runs.
#[derive(Debug, Clone, Serialize)]
pub struct Preview {
    pub title: String,
    /// "diff", "command", "text", "folder" (a folder request: detail is the reason, note the folder)
    /// or "key" (an API key request: detail is the reason, note the host).
    pub kind: &'static str,
    pub detail: Option<String>,
    pub note: Option<String>,
}

/// The result: text for the model, plus what the window shows.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub text: String,
    pub meta: Value,
}

impl Outcome {
    pub fn ok(text: impl Into<String>, title: impl Into<String>, kind: &str, detail: Option<String>) -> Outcome {
        Outcome { text: text.into(), meta: json!({ "title": title.into(), "status": "ok", "kind": kind, "detail": detail }) }
    }

    pub fn error(title: impl Into<String>, message: impl Into<String>) -> Outcome {
        let message = message.into();
        Outcome {
            text: format!("Error: {message}"),
            meta: json!({ "title": title.into(), "status": "error", "kind": "text", "detail": message }),
        }
    }

    pub fn denied(title: impl Into<String>) -> Outcome {
        Outcome {
            text: "The user declined this action. Don't try it again unless they ask; suggest another approach or ask what they'd like.".into(),
            meta: json!({ "title": title.into(), "status": "denied", "kind": "text" }),
        }
    }
}

pub struct Ctx<'a> {
    pub sandbox: &'a Sandbox,
    pub recorder: &'a Recorder<'a>,
    pub cancel: &'a AtomicBool,
    pub job: Option<&'a JobRef>,
    pub memory: MemoryCtx<'a>,
    /// For the browser tools.
    pub app: Option<&'a tauri::AppHandle>,
    pub state: &'a std::sync::Arc<crate::AppState>,
}

pub struct MemoryCtx<'a> {
    pub db: &'a Mutex<Connection>,
    pub cipher: &'a Cipher,
    pub chat_id: &'a str,
    pub project_id: Option<&'a str>,
}

/// A unified diff of two texts, as approval cards show it.
pub fn text_diff(old: &str, new: &str) -> String {
    fs::diff("note", old, new)
}

/// The approval preview for rewriting a note (it needs the stored note).
pub fn notes_preview(name: &str, args: &Value, db: &Mutex<Connection>, cipher: &Cipher) -> Result<Preview, String> {
    let id = args.get("note_id").and_then(Value::as_str).unwrap_or("");
    let note = crate::notes::get_note(&db.lock().unwrap(), cipher, id);
    notes::preview(name, args, note)
}

pub fn preview(name: &str, args: &Value, sandbox: &Sandbox) -> Result<Preview, String> {
    match name {
        "connect_account" => app::preview(args),
        "schedule_task" => scheduled::preview(args),
        "create_document" => documents::preview(args, sandbox),
        "extract_archive" => archives::preview(args, sandbox),
        "run_command" => shell::preview(args, sandbox),
        "ask_person" | "work_with_person" | "message_person" => Ok(people_preview(name, args)),
        "use_screen" => Ok(Preview {
            title: format!("Use the screen: {}", args.get("task").and_then(Value::as_str).unwrap_or("a task").chars().take(80).collect::<String>()),
            kind: "text",
            detail: None,
            note: Some("Lets the assistant look at the screen and use the mouse and keyboard, step by step, in this chat. SulcusAI's windows step aside while it works; it asks again before anything that sends, buys or deletes, and never types passwords. A strip at the top of the screen shows each step, with Stop.".into()),
        }),
        n if n.starts_with("program_") || n == "programs_list" => Ok(Preview {
            title: match n {
                "programs_list" => "See which programs are open".into(),
                "program_open" => format!("Open {}", args.get("name").and_then(Value::as_str).unwrap_or("a program")),
                "program_read" => "Read a program's window".into(),
                "program_type" => format!("Type “{}” in a program", args.get("text").and_then(Value::as_str).unwrap_or_default().chars().take(60).collect::<String>()),
                "program_key" => format!("Press {} in a program", args.get("keys").and_then(Value::as_str).unwrap_or("keys")),
                "program_scroll" => "Scroll in a program".into(),
                _ => "Press something in a program".into(),
            },
            kind: "text",
            detail: None,
            note: Some("Lets the assistant see and use the programs open on this PC in this chat (Allow for this chat covers the rest). It asks again before anything that sends, buys or deletes, and never types passwords. A strip at the top of the screen shows while it works, with Stop.".into()),
        }),
        "open_path" => Ok(Preview {
            title: format!("Open {}", args.get("target").and_then(Value::as_str).unwrap_or("something")),
            kind: "text",
            detail: args.get("target").and_then(Value::as_str).map(str::to_string),
            note: Some("Opens in your browser, or the app that usually opens this kind of file.".into()),
        }),
        _ => fs::preview(name, args, sandbox),
    }
}

/// What the approval card shows before something goes to another person's assistant.
fn people_preview(name: &str, args: &Value) -> Preview {
    let person = args.get("person").and_then(Value::as_str).unwrap_or("them");
    if name == "message_person" {
        return Preview {
            title: format!("Send {person} a message in People"),
            kind: "text",
            detail: args.get("text").and_then(Value::as_str).map(str::to_string),
            note: Some(format!("It goes to {person}'s SulcusAI now, or as soon as their app is reached.")),
        };
    }
    let (title, field) = if name == "ask_person" { (format!("Ask {person}'s assistant"), "question") } else { (format!("Work this out with {person}'s assistant"), "goal") };
    Preview {
        title,
        kind: "text",
        detail: args.get(field).and_then(Value::as_str).map(str::to_string),
        note: Some(format!("This goes to {person}'s SulcusAI, and {person} can read it. Their assistant answers with their own model and only what they allow.")),
    }
}

pub async fn run(name: &str, args: &Value, ctx: &Ctx<'_>) -> Outcome {
    match name {
        "ask_person" => crate::sync::people::ask::tool(args, ctx).await,
        "message_person" => crate::sync::people::message_tool(args, ctx),
        "command_output" => shell::output(args),
        "stop_command" => shell::stop(args),
        "open_path" => open_path(args, ctx),
        "preview_site" => site::run(args, ctx).await,
        "search_chats" => chats::search(args, ctx),
        "read_chat_notes" => crate::chatnotes::read(ctx),
        "update_chat_notes" => crate::chatnotes::update(args, ctx),
        "check_plan_step" => crate::chatnotes::check_step(args, ctx),
        "delete_chat" => chats::delete(args, ctx),
        "read_chat" => chats::read(args, ctx),
        // Run by the agent itself (it needs the model that's loaded).
        "message_chat" => Outcome::error("Ask another chat", "Other chats are asked from the chat itself."),
        // Asked of the user by the agent itself.
        "request_folder" => Outcome::error("Ask for a folder", "Folders are asked for in the chat."),
        "request_api_key" => Outcome::error("Ask for a key", "Keys are asked for in the chat."),
        // Its assistant runs tools too, so the call is boxed.
        "work_with_person" => Box::pin(crate::sync::people::together::tool(args, ctx)).await,
        "run_command" => shell::run(args, ctx).await,
        "remember" | "search_memory" => memory::run(name, args, &ctx.memory),
        "personal_records" | "personal_add" => personal::run(name, args, &ctx.memory),
        "search_meetings" | "read_meeting" => meetings::run(name, args, &ctx.memory),
        "create_note" | "search_notes" | "read_note" | "update_note" | "create_task" | "list_tasks" | "complete_task" => {
            notes::run(name, args, &ctx.memory)
        }
        "web_search" | "fetch_page" | "request_web" | "weather" => web::run(name, args, &ctx.memory, ctx.app).await,
        "wikipedia" | "research" | "market_quote" | "find_places" | "news_feeds" => {
            let prepared = {
                let conn = ctx.memory.db.lock().unwrap();
                crate::web::prepare(&conn, ctx.memory.cipher, ctx.memory.chat_id)
            };
            match prepared {
                Ok((client, settings, key)) => lookup::run(name, args, lookup::Web { client, settings, key, app: ctx.app.cloned() }, &ctx.memory).await,
                Err(e) => Outcome::error("Web access is off", e),
            }
        }
        "calculate" | "run_code" => compute::run(name, args, ctx).await,
        "check_writing" | "check_scam" | "pc_health" | "score_practice" => writing::run(name, args, &ctx.memory),
        "schedule_task" => scheduled::run(args, ctx),
        "search_documents" => library::search(args, ctx).await,
        "create_document" | "read_document" => {
            let name = name.to_string();
            let args = args.clone();
            tokio::task::block_in_place(|| documents::run(&name, &args, ctx))
        }
        n if n.starts_with("email_") || n.starts_with("calendar_") => comms::run(name, args, ctx.state, ctx.memory.cipher).await,
        n if n.starts_with("cloud_file") => crate::cloudfiles::run_tool(name, args, ctx.state, ctx.memory.cipher).await,
        "app_status" | "connect_account" | "open_page" => app::run(name, args, ctx.state, ctx.memory.cipher).await,
        "create_image" | "edit_image" | "create_video" | "create_music" => media::run(name, args, ctx).await,
        n if n.starts_with("browser_") => {
            let db = ctx.memory.db;
            browser::run(name, args, ctx.app, ctx.memory.chat_id, |summary| crate::db::log_action(&db.lock().unwrap(), "network", summary)).await
        }
        n if n.starts_with("program_") || n == "programs_list" => {
            let db = ctx.memory.db;
            programs::run(name, args, ctx.app, ctx.memory.chat_id, |summary| crate::db::log_action(&db.lock().unwrap(), "programs", summary)).await
        }
        "find_indexed" | "cite_pdf" => {
            let (name, args) = (name.to_string(), args.clone());
            tokio::task::block_in_place(|| findx::run(&name, &args, ctx))
        }
        "list_archive" | "extract_archive" => {
            let (name, args) = (name.to_string(), args.clone());
            tokio::task::block_in_place(|| archives::run(&name, &args, ctx))
        }
        _ => {
            // File work is quick but blocking; keep it off the async threads.
            let name = name.to_string();
            let args = args.clone();
            tokio::task::block_in_place(|| fs::run(&name, &args, ctx))
        }
    }
}

/// A readable title for a step that failed before or while running.
pub fn failed_title(name: &str, args: &Value) -> String {
    let target = ["path", "from", "pattern", "command"]
        .iter()
        .find_map(|k| args.get(*k).and_then(Value::as_str))
        .map(|t| t.chars().take(60).collect::<String>())
        .unwrap_or_default();
    let verb = match name {
        "list_dir" => "list",
        "read_file" => "read",
        "find_files" | "search_files" => "search for",
        "remember" => return "Couldn't save a memory".into(),
        "delegate" => return "A helper couldn't finish".into(),
        "for_each" => return "Couldn't do it for each item".into(),
        "search_memory" => return "Couldn't search memory".into(),
        "search_meetings" | "read_meeting" => return "Couldn't look up a meeting".into(),
        "create_note" | "search_notes" | "read_note" | "update_note" => return "Couldn't use the notes".into(),
        "create_task" | "list_tasks" | "complete_task" => return "Couldn't use the task list".into(),
        "web_search" => return "Couldn't search the web".into(),
        "search_documents" => return "Couldn't search your documents".into(),
        "fetch_page" => return "Couldn't open a web page".into(),
        "weather" => return "Couldn't check the weather".into(),
        "wikipedia" => return "Couldn't look it up on Wikipedia".into(),
        "research" => return "Couldn't research that".into(),
        "market_quote" => return "Couldn't check the prices".into(),
        "find_places" => return "Couldn't look up places".into(),
        "news_feeds" => return "Couldn't read the news feeds".into(),
        "calculate" => return "Couldn't calculate that".into(),
        "run_code" => return "The code couldn't run".into(),
        "schedule_task" => return "Couldn't schedule the task".into(),
        n if n.starts_with("browser_") => return "Couldn't use the browser".into(),
        "create_document" => return "Couldn't make the document".into(),
        "read_document" => return "Couldn't read the document".into(),
        n if n.starts_with("email_") => return "Couldn't use the email".into(),
        n if n.starts_with("calendar_") => return "Couldn't use the calendar".into(),
        "create_image" | "edit_image" => return "Couldn't make the picture".into(),
        "create_video" => return "Couldn't make the video".into(),
        "create_music" => return "Couldn't make the music".into(),
        "write_file" => "write",
        "edit_file" => "edit",
        "move_path" => "move",
        "make_folder" => "create folder",
        "delete_path" => "delete",
        "run_command" => "run",
        _ => return format!("Couldn't use {name}"),
    };
    format!("Couldn't {verb} {target}").trim_end().to_string()
}

pub fn arg_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key).and_then(Value::as_str).ok_or_else(|| format!("Missing \"{key}\"."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_are_checked_against_the_tool() {
        let read = find("read_file").unwrap();
        let mut ok = json!({ "path": "a.txt", "start_line": 3, "extra": "fine" });
        assert!(check_args(read, &mut ok).is_ok());
        let err = check_args(read, &mut json!({ "start_line": 3 })).unwrap_err();
        assert!(err.contains("needs path"), "{err}");
        let err = check_args(read, &mut json!({ "path": ["a.txt", "b.txt"] })).unwrap_err();
        assert!(err.contains("path should be text"), "{err}");
        assert!(check_args(read, &mut json!(["a.txt"])).is_err());
        // What's plainly meant is fixed in place.
        let mut loose = json!({ "path": "a.txt", "start_line": "10", "end_line": 20.0 });
        check_args(read, &mut loose).unwrap();
        assert_eq!((loose["start_line"].as_u64(), loose["end_line"].as_u64()), (Some(10), Some(20)));
        let err = check_args(read, &mut json!({ "path": "a.txt", "start_line": "ten" })).unwrap_err();
        assert!(err.contains("start_line should be a whole number"), "{err}");
        let search = find("search_files").unwrap();
        let mut flag = json!({ "pattern": "x", "ignore_case": "TRUE" });
        check_args(search, &mut flag).unwrap();
        assert_eq!(flag["ignore_case"], json!(true));
        let mv = find("move_path").unwrap();
        assert!(check_args(mv, &mut json!({ "from": "a", "to": null })).unwrap_err().contains("needs to"));
        // The loose forms tools already read: a window's number, an element's "[12]", a list as text.
        let window = find("program_read").unwrap();
        let mut by_number = json!({ "window": 3 });
        check_args(window, &mut by_number).unwrap();
        assert_eq!(by_number["window"], json!("3"));
        let click = find("program_click").unwrap();
        let mut bracketed = json!({ "ref": "[12]" });
        check_args(click, &mut bracketed).unwrap();
        assert_eq!(bracketed["ref"], json!(12));
        let lists = ToolDef { name: "t", description: "", risk: Risk::Read, params: || json!({ "properties": { "days": { "type": "array" } } }) };
        let mut text = json!({ "days": "monday, friday" });
        check_args(&lists, &mut text).unwrap();
        assert_eq!(text["days"], json!("monday, friday"));
        let mut quoted = json!({ "days": "[\"monday\"]" });
        check_args(&lists, &mut quoted).unwrap();
        assert_eq!(quoted["days"], json!(["monday"]));
        // Every tool's own declared parameters can be read this way.
        for t in TOOLS {
            let _ = check_args(t, &mut json!({}));
        }
    }

    #[test]
    fn parallel_tools_are_real_read_only_tools() {
        for name in ["list_dir", "read_file", "find_files", "search_files", "search_memory", "search_notes", "read_note", "search_meetings", "read_meeting", "read_document", "search_chats", "read_chat", "read_chat_notes", "calculate"] {
            let t = find(name).unwrap_or_else(|| panic!("{name} is not a tool"));
            assert!(t.risk == Risk::Read, "{name} must be Risk::Read");
            assert!(parallel_safe(name));
        }
        for name in ["web_search", "fetch_page", "write_file", "run_command", "search_documents"] {
            assert!(!parallel_safe(name), "{name}");
        }
    }

    #[test]
    fn run_modes_follow_the_design() {
        use Permission::*;
        assert_eq!(permission(Mode::Plan, Risk::Read, false), Run);
        assert_eq!(permission(Mode::Plan, Risk::Write, true), Refuse, "plan never changes anything");
        assert_eq!(permission(Mode::Auto, Risk::Write, false), Ask);
        assert_eq!(permission(Mode::Auto, Risk::Execute, true), Run, "approved for this chat");
        assert_eq!(permission(Mode::Bypass, Risk::Execute, false), Run);
    }

    /// A send names where it goes; a secret going out is a warning.
    #[test]
    fn a_send_is_shown_and_a_secret_is_a_warning() {
        let plain = guard::Egress { to: "files.example.com".into(), what: vec!["notes.txt".into()], secret: None };
        let p = egress_preview(&plain, "curl -F f=@notes.txt https://files.example.com");
        assert_eq!(p.title, "Send notes.txt to files.example.com");
        assert_eq!(p.detail.as_deref(), Some("curl -F f=@notes.txt https://files.example.com"));
        let key = guard::Egress { secret: Some("an SSH key".into()), ..plain };
        let w = egress_preview(&key, "scp ~/.ssh/id_rsa box:");
        assert_eq!(w.title, "Warning: this would send an SSH key to files.example.com");
        assert!(w.note.unwrap().contains("secret"));
        // Only a command can send this way, and only when it does.
        let sb = Sandbox::new(&[std::env::temp_dir().to_string_lossy().into_owned()]);
        assert!(egress_call("run_command", &serde_json::json!({ "command": "git push" }), &sb).is_some());
        assert!(egress_call("run_command", &serde_json::json!({ "command": "git status" }), &sb).is_none());
        assert!(egress_call("write_file", &serde_json::json!({ "path": "a.txt", "content": "git push" }), &sb).is_none());
    }

    #[test]
    fn a_small_memory_keeps_the_tools_a_request_names() {
        let needed = |tool: &str, asked: &str| SPARE.iter().find(|(g, _)| in_group(tool, g)).is_some_and(|(_, re)| re.is_match(asked));
        assert!(in_group("browser_open", &["browser_"]) && !in_group("open_page", &["browser_"]));
        assert!(needed("market_quote", "What's Apple's stock price right now?"));
        assert!(needed("email_search", "any new emails from Abby?"));
        assert!(needed("schedule_task", "Every weekday at 8am, give me the news"));
        assert!(needed("run_code", "make a bar chart of my sales"));
        assert!(needed("research", "Which is better, the iPhone 17 or the Pixel 10?"));
        assert!(!needed("email_search", "What's 18.5% of 2,340?"));
        assert!(needed("email_search", "When is sam's party?") && needed("calendar_events", "When is sam's party?"));
        assert!(needed("email_search", "What did my mom ask me?"));
        assert!(!needed("email_search", "Who is Link from Zelda?"));
        assert!(!needed("browser_open", "Who is Link from Zelda?"));
        assert!(needed("find_places", "Where's the nearest coffee shop?") && needed("find_places", "gas stations near Provo"));
        assert!(!needed("find_places", "What's 18.5% of 2,340?"));
        // Never left out.
        for always in ["calculate", "web_search", "fetch_page", "read_file", "write_file", "run_command", "preview_site", "request_api_key"] {
            assert!(!SPARE.iter().any(|(g, _)| in_group(always, g)), "{always}");
        }
    }

    #[test]
    fn plan_mode_only_offers_read_tools() {
        let defs = definitions(Mode::Plan, true, true);
        let names: Vec<&str> = defs.as_array().unwrap().iter().map(|d| d["function"]["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"read_file"));
        assert!(!names.contains(&"write_file") && !names.contains(&"run_command"));
        assert!(names.contains(&"remember"), "saving a memory is allowed while planning");
        // Everything except load_skill and the meeting, note, task, web, browser, programs, email, calendar, Drive and OneDrive, document, media, app, People and chat tools, which the agent adds when they apply.
        assert_eq!(definitions(Mode::Auto, true, true).as_array().unwrap().len(), TOOLS.len() - 80);
    }

    #[test]
    fn tool_groups_depend_on_folders_and_memory() {
        let names = |d: Value| -> Vec<String> { d.as_array().unwrap().iter().map(|x| x["function"]["name"].as_str().unwrap().to_string()).collect() };
        let only_memory = names(definitions(Mode::Auto, false, true));
        assert_eq!(only_memory, vec!["remember", "search_memory"]);
        assert!(!names(definitions(Mode::Auto, true, false)).iter().any(|n| n == "remember"));
        assert!(definitions(Mode::Auto, false, false).as_array().unwrap().is_empty());
    }

    #[test]
    fn helpers_get_read_tools_but_no_helpers_of_their_own() {
        let defs = helper_definitions(true);
        let names: Vec<&str> = defs.as_array().unwrap().iter().map(|d| d["function"]["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"read_file") && names.contains(&"search_files"));
        assert!(!names.contains(&"delegate") && !names.contains(&"write_file") && !names.contains(&"remember"));
        let main: Vec<String> = definitions(Mode::Auto, true, true).as_array().unwrap().iter().map(|d| d["function"]["name"].as_str().unwrap().to_string()).collect();
        assert!(main.iter().any(|n| n == "delegate"));
    }

    #[test]
    fn every_tool_has_an_object_schema() {
        for t in TOOLS {
            assert_eq!((t.params)()["type"], "object", "{}", t.name);
        }
    }

    #[test]
    fn open_path_opens_documents_media_and_folders_only() {
        use std::path::Path;
        for ok in ["site/index.html", "report.DOCX", "chart.png", "notes.md", "song.mp3", "clip.mp4"] {
            assert!(not_openable(Path::new(ok)).is_none(), "{ok}");
        }
        // Everything else, including what a block-list missed (Python, consoles, remote desktop, web queries).
        for no in ["run.py", "run.pyw", "x.msc", "x.rdp", "x.iqy", "x.slk", "x.application", "x.appinstaller", "x.exe", "x.lnk", "x.url", "README", "x.html.exe"] {
            assert!(not_openable(Path::new(no)).is_some(), "{no}");
        }
        assert!(not_openable(&std::env::temp_dir()).is_none(), "a folder opens in File Explorer");
    }
}

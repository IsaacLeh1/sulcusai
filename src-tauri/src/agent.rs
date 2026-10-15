// SPDX-License-Identifier: AGPL-3.0-only
//! The agent loop: the model replies, may call tools, sees their results,
//! and continues until it answers without calling a tool.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::oneshot;

use crate::chat::{self, ContextInfo, Delta};
use crate::checkpoint::Recorder;
use crate::crypto::Cipher;
use crate::db::{self, Message, ToolCall};
use crate::engine::Endpoint;
use crate::features::Feature;
use crate::sandbox::Sandbox;
use crate::tools::{self, Mode, Outcome, Permission, Preview, Risk};
use crate::connectors::{self, OfferedTool, Skill, ToolMode};
use crate::{memory, projects, AppState};


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    /// Allow this kind of action for the rest of the chat.
    AllowForChat,
    Deny,
}

impl Decision {
    pub fn parse(s: &str) -> Option<Decision> {
        match s {
            "allow" => Some(Decision::Allow),
            "always" => Some(Decision::AllowForChat),
            "deny" => Some(Decision::Deny),
            _ => None,
        }
    }
}

/// An approval waiting for the user.
#[derive(Debug, Clone, Serialize)]
pub struct PendingApproval {
    pub chat_id: String,
    pub call_id: String,
    pub tool: String,
    pub risk: Risk,
    pub preview: Preview,
}

/// Shared between the loop and the approval command.
#[derive(Default)]
pub struct Approvals {
    waiting: Mutex<HashMap<String, (PendingApproval, oneshot::Sender<Decision>)>>,
    /// Risks the user approved for a whole chat.
    allowed: Mutex<HashMap<String, HashSet<Risk>>>,
}

impl Approvals {
    /// What's waiting for an answer (tests answer these like the user would).
    #[cfg(test)]
    pub fn waiting(&self) -> Vec<PendingApproval> {
        self.waiting.lock().unwrap().values().map(|(p, _)| p.clone()).collect()
    }

    pub fn answer(&self, call_id: &str, d: Decision) -> bool {
        match self.waiting.lock().unwrap().remove(call_id) {
            Some((p, tx)) => {
                if d == Decision::AllowForChat {
                    self.allowed.lock().unwrap().entry(p.chat_id).or_default().insert(p.risk);
                }
                tx.send(d).is_ok()
            }
            None => false,
        }
    }

    pub fn pending(&self, chat_id: &str) -> Vec<PendingApproval> {
        self.waiting.lock().unwrap().values().filter(|(p, _)| p.chat_id == chat_id).map(|(p, _)| p.clone()).collect()
    }

    fn allowed(&self, chat_id: &str, risk: Risk) -> bool {
        self.allowed.lock().unwrap().get(chat_id).is_some_and(|s| s.contains(&risk))
    }

    pub fn forget_chat(&self, chat_id: &str) {
        self.allowed.lock().unwrap().remove(chat_id);
    }
}

/// Sends an event to the window (or to a test).
pub type Emit = Arc<dyn Fn(&str, Value) + Send + Sync>;

pub struct Turn {
    pub emit: Emit,
    pub state: Arc<AppState>,
    pub chat_id: String,
    /// The user message that started this turn; checkpoints hang off it.
    pub turn_id: String,
    pub cipher: Cipher,
    pub ep: Endpoint,
    pub mode: Mode,
    pub use_tools: bool,
    /// The model was started with its image encoder and can see pictures.
    pub vision: bool,
    pub base: String,
    pub about: String,
    pub cancel: Arc<AtomicBool>,
}

pub struct TurnResult {
    pub last: Option<Message>,
    pub context: ContextInfo,
    pub tps: Option<f64>,
    pub cancelled: bool,
}

/// Extra system-prompt text describing the tools and the current mode.
pub fn tool_instructions(sandbox: &Sandbox, mode: Mode) -> String {
    if sandbox.is_empty() {
        return "\n\nNo folders are shared with you, so you can't open or change files. If the user asks for file or coding work, \
                tell them to share a folder with the 📁 button under the message box."
            .into();
    }
    let folders: Vec<String> = sandbox.roots().iter().map(|r| format!("{}/", sandbox.display(r))).collect();
    let mode_text = match mode {
        Mode::Plan => "You are in PLAN mode: you can look but not change anything. Investigate with the read-only tools, \
                       then reply with a short numbered plan of the changes you would make. The user approves the plan before anything changes.",
        Mode::Auto => "The app asks the user before any change or command runs, so just call the tool when you need it. \
                       If they decline, don't retry the same action.",
        Mode::Bypass => "Changes and commands run without asking the user, so be careful and keep changes to what was asked.",
    };
    format!(
        "\n\n## Working with files\nYou can use tools in these folders the user shared: {}. Write paths starting with the folder name, \
         for example {}README.md.\n- Look before you change anything: list folders, read files, search.\n\
         - For small changes use edit_file, and read the file first so old_text matches exactly.\n\
         - After changing code, run its build or tests with run_command when that makes sense.\n\
         - Finish with a short summary of what you did.\n{mode_text}",
        folders.join(", "),
        folders[0]
    )
}

fn now() -> i64 {
    db::now_ms()
}

/// A tool result cut to about a third of the model's memory, so one long
/// email, page or file can't crowd out the conversation (small models have
/// 8K tokens or less; about 3 characters a token, to be safe).
fn fit_result(mut outcome: Outcome, ctx: u32) -> Outcome {
    let max = (ctx as usize / 3) * 3;
    let total = outcome.text.chars().count();
    if max > 0 && total > max {
        let kept: String = outcome.text.chars().take(max).collect();
        outcome.text = format!("{kept}

[Cut here: {} more characters weren't shown, to fit your memory. Answer from what's above.]", total - max);
    }
    outcome
}

/// A tool call's name and arguments, with the arguments' formatting evened out.
fn call_key(call: &ToolCall) -> String {
    let args = serde_json::from_str::<Value>(&call.arguments).map(|v| v.to_string()).unwrap_or_else(|_| call.arguments.clone());
    format!("{}{}", call.name, args)
}

impl Turn {
    fn emit(&self, event: &str, payload: Value) {
        (self.emit)(event, payload);
    }

    /// Before each step: wait out the heat guard while the PC is over its
    /// limit, and rest briefly while adaptive cooling has stepped down.
    async fn rest_if_hot(&self) {
        let limits = self.state.limits();
        crate::perf::cool_down(&limits, |what| {
            self.emit("chat:status", json!({ "chat_id": self.chat_id, "status": "cooling", "detail": what }));
        })
        .await;
        let pause = crate::perf::step_pause(&self.state.heat.get());
        if !pause.is_zero() && !self.cancel.load(Ordering::Relaxed) {
            tokio::time::sleep(pause).await;
        }
    }

    fn save(&self, m: &Message) -> Result<(), String> {
        db::add_message(&self.state.db.lock().unwrap(), &self.cipher, m)
    }

    pub async fn run(self) -> Result<TurnResult, String> {
        let (folders, project, memories, memory_on, request, features) = {
            let conn = self.state.db.lock().unwrap();
            let chat = db::chat(&conn, &self.cipher, &self.chat_id).ok_or("That chat no longer exists.")?;
            let project = chat.project_id.as_deref().and_then(|p| projects::get(&conn, &self.cipher, p));
            let on = crate::features::enabled(&conn);
            let memory_on = on.contains(&Feature::Memory) && db::settings(&conn).memory_enabled && !chat.incognito;
            let memories = if memory_on { memory::list(&conn, &self.cipher, chat.project_id.as_deref()) } else { Vec::new() };
            let request = db::messages(&conn, &self.cipher, &self.chat_id)
                .into_iter()
                .find(|m| m.id == self.turn_id)
                .map(|m| m.content)
                .unwrap_or_default();
            let folders = if on.contains(&Feature::Files) { projects::chat_folders(&conn, &chat) } else { Vec::new() };
            (folders, project, memories, memory_on, request, on)
        };
        let sandbox = Sandbox::new(&folders);
        let files = self.use_tools && !sandbox.is_empty();
        let memory_tools = self.use_tools && memory_on;
        let mut defs = tools::definitions(self.mode, files, memory_tools).as_array().cloned().unwrap_or_default();
        let mut extras = Extras::default();
        if self.use_tools && features.contains(&Feature::Connectors) {
            let (mcp_defs, mcp_map, problems) = connectors::offer(&self.state, &self.cipher).await;
            defs.extend(mcp_defs);
            extras.mcp = mcp_map;
            extras.problems = problems;
            extras.skills = connectors::active_skills(&self.state.db.lock().unwrap(), &self.cipher);
            if !connectors::split_skills(&extras.skills).1.is_empty() {
                if let Some(d) = tools::find("load_skill") {
                    defs.push(json!({ "type": "function", "function": { "name": d.name, "description": d.description, "parameters": (d.params)() } }));
                }
            }
        }
        // Knowing the app itself: what's connected, and connecting accounts.
        let mut extra: Vec<&str> = if self.use_tools { vec!["app_status", "open_page"] } else { Vec::new() };
        if self.use_tools && self.mode != tools::Mode::Plan && (features.contains(&Feature::Email) || features.contains(&Feature::Calendar)) {
            extra.push("connect_account");
        }
        if self.use_tools && features.contains(&Feature::Notes) {
            extra.extend(["create_note", "search_notes", "read_note"]);
            if self.mode != tools::Mode::Plan {
                extra.push("update_note");
            }
        }
        if self.use_tools && features.contains(&Feature::Tasks) {
            extra.extend(["create_task", "list_tasks", "complete_task"]);
        }
        if self.use_tools && features.contains(&Feature::Meetings) && crate::meeting::count(&self.state.db.lock().unwrap()) > 0 {
            extra.extend(["search_meetings", "read_meeting"]);
        }
        if files && features.contains(&Feature::Documents) {
            extra.push("read_document");
            if self.mode != tools::Mode::Plan {
                extra.push("create_document");
            }
        }
        if self.use_tools && features.contains(&Feature::Email) && crate::mail::count(&self.state.db.lock().unwrap()) > 0 {
            extra.extend(["email_search", "email_read"]);
            if self.mode != tools::Mode::Plan {
                extra.push("email_send");
            }
        }
        if self.use_tools && features.contains(&Feature::Calendar) {
            extra.extend(["calendar_events", "calendar_free_time"]);
            if self.mode != tools::Mode::Plan {
                extra.push("calendar_create_event");
            }
        }
        // Making pictures, video and music, once a model for them is installed.
        let mut media_note = Vec::new();
        if self.use_tools && features.contains(&Feature::Images) && crate::media::can_make(&self.state, crate::media::Kind::Image) {
            extra.extend(["create_image", "edit_image"]);
            media_note.push("pictures (create_image, edit_image)");
        }
        if self.use_tools && features.contains(&Feature::Video) && crate::media::can_make(&self.state, crate::media::Kind::Video) {
            extra.push("create_video");
            media_note.push("short video clips (create_video)");
        }
        if self.use_tools && features.contains(&Feature::Music) && crate::media::has_kind(&self.state, crate::media::Kind::Music) {
            extra.push("create_music");
            media_note.push("music and sound effects (create_music)");
        }
        if self.use_tools && (features.contains(&Feature::Browser) || features.contains(&Feature::BrowserControl)) && crate::browser::allowed_for_chat(&self.state.db.lock().unwrap(), &self.chat_id) {
            extra.extend(["browser_open", "browser_read", "browser_click", "browser_type", "browser_back"]);
        }
        // Web tools when this chat may go online; otherwise a way to ask.
        if self.use_tools {
            if crate::web::allowed_for_chat(&self.state.db.lock().unwrap(), &self.chat_id) {
                extra.extend(["weather", "web_search", "fetch_page"]);
            } else {
                extra.push("request_web");
            }
        }
        {
            for name in extra {
                if let Some(d) = tools::find(name) {
                    defs.push(json!({ "type": "function", "function": { "name": d.name, "description": d.description, "parameters": (d.params)() } }));
                }
            }
        }
        // Kinds of action advanced mode never allows aren't offered at all.
        let adv = crate::advanced::get(&self.state);
        defs.retain(|d| {
            let name = d.pointer("/function/name").and_then(Value::as_str).unwrap_or_default();
            !adv.forbids(tools::find(name).map_or(Risk::Connector, |t| t.risk))
        });
        let mut tools = Some(Value::Array(defs)).filter(|t| t.as_array().is_some_and(|a| !a.is_empty()));

        let mut about = self.about.clone();
        if let Some(p) = &project {
            about.push_str(&projects::prompt_section(p));
        }
        if memory_tools {
            about.push_str(&memory::prompt_section(&memory::relevant(&memories, &request)));
        }
        if !media_note.is_empty() {
            about.push_str(&format!(
                "\n\nYou can make {} on this PC. When the user asks for one, call the tool with a vivid, specific description \
                 (subject, setting, style, lighting) instead of describing it in words. The result appears in the chat by itself, \
                 so don't paste links or ids; say in a sentence what you made.",
                media_note.join(", ")
            ));
        }
        if self.use_tools {
            about.push_str(&tools::app_note(&self.state, &self.cipher));
            about.push_str(&tool_instructions(&sandbox, self.mode));
            about.push_str(&connectors::skills_prompt(&extras.skills));
            if !extras.problems.is_empty() {
                about.push_str(&format!(
                    "\n\nThese connectors couldn't start, so their tools aren't available; tell the user if they ask for them: {}",
                    extras.problems.join("; ")
                ));
            }
        }
        let project_id = project.as_ref().map(|p| p.id.clone());
        let system = if about.trim().is_empty() { self.base.clone() } else { format!("{}\n\n{}", self.base, about.trim_start()) };
        let checkpoint_dir = self.state.paths.data.join("checkpoints");

        let mut result = TurnResult { last: None, context: ContextInfo::default(), tps: None, cancelled: false };
        let max_steps = adv.max_steps();
        // Calls already made this turn, to catch a model asking for the same thing again.
        let mut done_calls: Vec<String> = Vec::new();
        let mut repeats = 0;
        for step in 0..max_steps {
            self.rest_if_hot().await;
            let history = db::messages(&self.state.db.lock().unwrap(), &self.cipher, &self.chat_id);
            let (kept, mut info) = chat::fit_history(&self.ep, &self.base, &about, tools.as_ref(), &history).await?;
            self.state.contexts.lock().unwrap().insert(self.chat_id.clone(), info.clone());
            self.emit("chat:context", json!({ "chat_id": self.chat_id, "context": info }));

            let mut message_id = uuid::Uuid::new_v4().to_string();
            // Set when the model stalled: one more try that can only be a lookup.
            let mut forced: Option<(Endpoint, chat::ForcedLookup)> = None;
            let mut first = None;
            let finished = loop {
                self.emit("chat:start", json!({ "chat_id": self.chat_id, "message_id": message_id }));
                let mut messages = chat::api_messages(&system, &kept);
                let see = |id: &str| crate::media::image_data_url(&self.state, &self.cipher, id).ok();
                let see: Option<&dyn Fn(&str) -> Option<String>> = if self.vision { Some(&see) } else { None };
                chat::attach_images(&mut messages, &kept, see);
                if let Some((_, f)) = &forced {
                    messages.push(json!({ "role": "user", "content": f.nudge }));
                }
                // The forced try writes JSON, not words for the user: don't show it.
                let quiet = forced.is_some();
                let (ep, offer) = match &forced {
                    Some((ep, _)) => (ep, None),
                    None => (&self.ep, tools.as_ref()),
                };
                let result = chat::stream(ep, messages, offer, &self.cancel, |d| {
                    if quiet {
                        return;
                    }
                    let (content, thinking) = match d {
                        Delta::Content(t) => (Some(t), None),
                        Delta::Thinking(t) => (None, Some(t)),
                    };
                    self.emit("chat:delta", json!({ "chat_id": self.chat_id, "message_id": message_id, "content": content, "thinking": thinking }));
                })
                .await;
                if let Some((_, f)) = &forced {
                    // Its call, or (if the engine or model let us down) the first reply.
                    let first = first.take().expect("the first reply is kept for a forced try");
                    break match result {
                        Ok(mut r) => match chat::forced_call(&r.content, &f.names) {
                            Some(mut call) => {
                                call.id = format!("call_{}", uuid::Uuid::new_v4().simple());
                                r.tool_calls = vec![call];
                                r.content.clear();
                                r
                            }
                            None => first,
                        },
                        Err(_) => first,
                    };
                }
                let finished = result?;
                // A model that asks the user for details about their own mail
                // or calendar, or says it can't access them, instead of looking
                // gets one more try that can only be a lookup. The reply is dropped.
                let asked = history.iter().rev().find(|m| m.role == "user").map(|m| m.content.as_str()).unwrap_or("");
                if step == 0 && tools.is_some() && self.ep.cloud.is_none() && finished.tool_calls.is_empty() && !finished.cancelled && chat::stalls(asked, &finished.content, &tools.as_ref().and_then(Value::as_array).into_iter().flatten().filter_map(|t| t.pointer("/function/name").and_then(Value::as_str).map(str::to_string)).collect::<Vec<_>>()) {
                    let looks_only = |n: &str| n != "open_page" && tools::find(n).is_some_and(|t| t.risk == Risk::Read);
                    if let Some(f) = tools.as_ref().and_then(|t| chat::forced_lookup(t, asked, looks_only)) {
                        let mut ep = self.ep.clone();
                        ep.extra["response_format"] = f.response_format.clone();
                        ep.extra["max_tokens"] = json!(400);
                        forced = Some((ep, f));
                        first = Some(finished);
                        message_id = uuid::Uuid::new_v4().to_string();
                        self.emit("agent:step", json!({ "chat_id": self.chat_id }));
                        continue;
                    }
                }
                break finished;
            };

            let calls = finished.tool_calls.clone();
            let message = Message {
                id: message_id,
                chat_id: self.chat_id.clone(),
                role: "assistant".into(),
                content: finished.content,
                thinking: (!finished.thinking.is_empty()).then_some(finished.thinking),
                created_at: now(),
                tool_calls: (!calls.is_empty()).then(|| calls.clone()),
                meta: finished.raw.map(|r| json!({ "cloud_blocks": r })),
                ..Default::default()
            };
            if !message.content.is_empty() || message.thinking.is_some() || message.tool_calls.is_some() {
                self.save(&message)?;
            }
            if let (Some(p), Some(c)) = (finished.prompt_tokens, finished.completion_tokens) {
                info.last_total = Some(p + c);
            }
            result.context = info;
            result.tps = finished.tps.or(result.tps);
            result.last = Some(message);
            if finished.cancelled {
                result.cancelled = true;
                // Every call needs a result, or the next request is malformed.
                for call in &calls {
                    self.save_result(call, Outcome::denied(call.name.clone()).with_text("Stopped by the user before this ran."))?;
                }
                break;
            }
            if calls.is_empty() {
                break;
            }
            self.emit("agent:step", json!({ "chat_id": self.chat_id }));

            let recorder = Recorder {
                db: &self.state.db,
                cipher: &self.cipher,
                dir: checkpoint_dir.clone(),
                chat_id: &self.chat_id,
                turn_id: &self.turn_id,
            };
            for call in &calls {
                let key = call_key(call);
                let repeat = done_calls.contains(&key) && tools::find(&call.name).is_some_and(|t| t.risk == Risk::Read);
                let outcome = if self.cancel.load(Ordering::Relaxed) {
                    Outcome::denied(call.name.clone()).with_text("Stopped by the user before this ran.")
                } else if repeat {
                    repeats += 1;
                    Outcome::ok(
                        format!("You already ran {} with these arguments; its result is above. Answer the user with it now, or try different arguments.", call.name),
                        format!("Skipped a repeat of {}", call.name),
                        "text",
                        None,
                    )
                } else {
                    done_calls.push(key);
                    self.execute(call, &sandbox, &recorder, project_id.as_deref(), &extras).await
                };
                // The user let it use the web: the web tools, from the next step on.
                if outcome.meta["web_granted"] == true {
                    if let Some(Value::Array(defs)) = tools.as_mut() {
                        defs.retain(|d| d.pointer("/function/name").and_then(Value::as_str) != Some("request_web"));
                        for name in ["weather", "web_search", "fetch_page"] {
                            if let Some(d) = tools::find(name) {
                                defs.push(json!({ "type": "function", "function": { "name": d.name, "description": d.description, "parameters": (d.params)() } }));
                            }
                        }
                    }
                }
                let outcome = fit_result(outcome, self.ep.ctx);
                self.save_result(call, outcome)?;
                self.emit("agent:step", json!({ "chat_id": self.chat_id }));
            }
            // Going in circles: stop rather than spend minutes repeating itself.
            if repeats >= 3 {
                let note = Message {
                    id: uuid::Uuid::new_v4().to_string(),
                    chat_id: self.chat_id.clone(),
                    role: "assistant".into(),
                    content: "I kept repeating the same lookup, so I stopped. Try asking in a different way, or pick a larger model in Models.".into(),
                    created_at: now(),
                    ..Default::default()
                };
                self.save(&note)?;
                result.last = Some(note);
                break;
            }
            if self.cancel.load(Ordering::Relaxed) {
                result.cancelled = true;
                break;
            }
            if step == max_steps - 1 {
                let note = Message {
                    id: uuid::Uuid::new_v4().to_string(),
                    chat_id: self.chat_id.clone(),
                    role: "assistant".into(),
                    content: format!("I've taken {max_steps} steps on this, so I'm pausing here. Say \"continue\" if you'd like me to keep going."),
                    created_at: now(),
                    ..Default::default()
                };
                self.save(&note)?;
                result.last = Some(note);
            }
        }
        Ok(result)
    }

    fn save_result(&self, call: &ToolCall, outcome: Outcome) -> Result<(), String> {
        let mut meta = outcome.meta;
        meta["tool"] = json!(call.name);
        self.save(&Message {
            id: uuid::Uuid::new_v4().to_string(),
            chat_id: self.chat_id.clone(),
            role: "tool".into(),
            content: outcome.text,
            created_at: now(),
            tool_call_id: Some(call.id.clone()),
            meta: Some(meta),
            ..Default::default()
        })
    }

    async fn execute(&self, call: &ToolCall, sandbox: &Sandbox, recorder: &Recorder<'_>, project_id: Option<&str>, extras: &Extras) -> Outcome {
        if let Some(t) = extras.mcp.get(&call.name) {
            return self.execute_connector(call, t).await;
        }
        if call.name == "request_web" {
            return self.request_web(call).await;
        }
        if call.name == "load_skill" {
            let args: Value = serde_json::from_str(&call.arguments).unwrap_or_default();
            let wanted = args.get("name").and_then(Value::as_str).unwrap_or("");
            return match extras.skills.iter().find(|s| s.name.eq_ignore_ascii_case(wanted)) {
                Some(s) => Outcome::ok(s.body.clone(), format!("Loaded the {} skill", s.name), "text", None),
                None => Outcome::error("Couldn't load a skill", format!("There's no skill called {wanted}.")),
            };
        }
        let Some(def) = tools::find(&call.name) else {
            return Outcome::error(call.name.clone(), format!("There is no tool called {}.", call.name));
        };
        let args: Value = match serde_json::from_str(if call.arguments.trim().is_empty() { "{}" } else { &call.arguments }) {
            Ok(v) => v,
            Err(e) => return Outcome::error(format!("Couldn't use {}", call.name), format!("The arguments weren't valid JSON ({e}). Try again.")),
        };
        // Clicking or typing in the browser: refuse private fields, and ask
        // first when it submits, buys, sends or deletes.
        let mut risk = def.risk;
        let mut browser_preview = None;
        if matches!(call.name.as_str(), "browser_click" | "browser_type") {
            match tools::browser::assess(self.state.app.get(), &call.name, &args).await {
                Ok(None) => {}
                Ok(Some(p)) => {
                    risk = Risk::Submit;
                    browser_preview = Some(p);
                }
                Err(e) => return Outcome::error(tools::failed_title(&call.name, &args), e),
            }
        } else if matches!(call.name.as_str(), "email_send" | "calendar_create_event") {
            // Sending mail and inviting people always ask; a plain event follows the mode.
            match tools::comms::assess(&call.name, &args, &self.state, &self.cipher) {
                Ok((r, p)) => {
                    risk = r;
                    browser_preview = Some(p);
                }
                Err(e) => return Outcome::error(tools::failed_title(&call.name, &args), e),
            }
        }
        let allowed = self.state.approvals.allowed(&self.chat_id, risk);
        let adv = crate::advanced::get(&self.state);
        if adv.forbids(risk) {
            return Outcome::error(
                tools::failed_title(&call.name, &args),
                "This kind of action is turned off in Settings → Advanced. Tell the user, and don't try another way around it.",
            );
        }
        let mut permission = tools::permission(self.mode, risk, allowed);
        if permission == Permission::Run && adv.must_ask(risk) {
            permission = Permission::Ask;
        }
        match permission {
            Permission::Refuse => {
                return Outcome::error(tools::failed_title(&call.name, &args), "Plan mode can't change anything. Put this step in your plan instead.");
            }
            Permission::Ask => {
                // A preview that fails (say, edit text not found) goes straight back to the model.
                let preview = match if let Some(p) = browser_preview.take() {
                    Ok(p)
                } else if call.name == "update_note" {
                    tools::notes_preview(&call.name, &args, &self.state.db, &self.cipher)
                } else {
                    tools::preview(&call.name, &args, sandbox)
                } {
                    Ok(p) => p,
                    Err(e) => return Outcome::error(tools::failed_title(&call.name, &args), e),
                };
                let title = preview.title.clone();
                match self.ask(call, risk, preview).await {
                    Decision::Deny => {
                        self.log(&format!("Declined: {title}"));
                        return Outcome::denied(title);
                    }
                    Decision::Allow | Decision::AllowForChat => {}
                }
            }
            Permission::Run => {}
        }
        let ctx = tools::Ctx {
            sandbox,
            recorder,
            cancel: &self.cancel,
            job: self.state.job(),
            memory: tools::MemoryCtx { db: &self.state.db, cipher: &self.cipher, chat_id: &self.chat_id, project_id },
            app: self.state.app.get(),
            state: &self.state,
        };
        self.emit("agent:tool_start", json!({ "chat_id": self.chat_id, "call_id": call.id, "tool": call.name }));
        let outcome = if call.name == "delegate" {
            match tools::arg_str(&args, "task") {
                Ok(task) => self.run_helper(&call.id, task, &ctx).await,
                Err(e) => Outcome::error(tools::failed_title(&call.name, &args), e),
            }
        } else {
            tools::run(&call.name, &args, &ctx).await
        };
        if let Some(title) = outcome.meta.get("title").and_then(Value::as_str) {
            self.log(title);
        }
        outcome
    }

    /// A helper agent: fresh context, read-only tools, one task, a short
    /// report back. Its steps show inside the delegate card, not the chat.
    async fn run_helper(&self, call_id: &str, task: &str, ctx: &tools::Ctx<'_>) -> Outcome {
        let short: String = task.chars().take(60).collect();
        let title = format!("Helper: {short}");
        let defs = tools::helper_definitions(!ctx.sandbox.is_empty());
        let folders: Vec<String> = ctx.sandbox.roots().iter().map(|r| format!("{}/", ctx.sandbox.display(r))).collect();
        let system = format!(
            "{}\n\nYou are a helper agent working for the main assistant. Do only the task below, using the read-only tools \
             on these folders: {}. Write paths starting with the folder name. Then reply with a concise, factual report of \
             what you found, citing file paths and line numbers. You can't change anything.",
            self.base,
            if folders.is_empty() { "(none shared)".to_string() } else { folders.join(", ") }
        );
        let msg = |role: &str, content: String| Message {
            id: uuid::Uuid::new_v4().to_string(),
            role: role.into(),
            content,
            ..Default::default()
        };
        let mut history = vec![msg("user", task.to_string())];
        let mut steps: Vec<String> = Vec::new();
        for _ in 0..crate::advanced::get(&self.state).helper_steps() {
            if self.cancel.load(Ordering::Relaxed) {
                return Outcome::error(title, "Stopped by the user.");
            }
            let kept = match chat::fit_history(&self.ep, &system, "", Some(&defs), &history).await {
                Ok((kept, _)) => kept,
                Err(e) => return Outcome::error(title, e),
            };
            let finished = match chat::stream(&self.ep, chat::api_messages(&system, &kept), Some(&defs), &self.cancel, |_| {}).await {
                Ok(f) => f,
                Err(e) => return Outcome::error(title, e),
            };
            let calls = finished.tool_calls.clone();
            let mut assistant = msg("assistant", finished.content.clone());
            assistant.tool_calls = (!calls.is_empty()).then(|| calls.clone());
            history.push(assistant);
            if calls.is_empty() {
                let detail = if steps.is_empty() { None } else { Some(steps.iter().map(|s| format!("• {s}")).collect::<Vec<_>>().join("\n")) };
                let report = if finished.content.trim().is_empty() { "The helper finished without a report.".to_string() } else { finished.content };
                return Outcome::ok(report, title, "text", detail);
            }
            for c in &calls {
                let args: Value = serde_json::from_str(if c.arguments.trim().is_empty() { "{}" } else { &c.arguments }).unwrap_or_default();
                let read_only = tools::find(&c.name).is_some_and(|d| d.risk == Risk::Read) && c.name != "delegate";
                let out = if read_only {
                    tools::run(&c.name, &args, ctx).await
                } else {
                    Outcome::error(tools::failed_title(&c.name, &args), "Helpers can only look, not change anything.")
                };
                let step = out.meta.get("title").and_then(Value::as_str).unwrap_or(&c.name).to_string();
                self.emit("agent:helper", json!({ "chat_id": self.chat_id, "call_id": call_id, "step": step }));
                steps.push(step);
                let mut result = msg("tool", out.text);
                result.tool_call_id = Some(c.id.clone());
                history.push(result);
            }
        }
        Outcome::ok(
            format!("The helper ran out of steps. What it looked at:\n{}", steps.join("\n")),
            title,
            "text",
            Some(steps.join("\n")),
        )
    }

    /// A connector tool: the user's per-tool setting plus the run mode decide.
    async fn execute_connector(&self, call: &ToolCall, t: &OfferedTool) -> Outcome {
        let title = format!("{} ({})", t.tool, t.connector_name);
        let args: Value = match serde_json::from_str(if call.arguments.trim().is_empty() { "{}" } else { &call.arguments }) {
            Ok(v) => v,
            Err(e) => return Outcome::error(format!("Couldn't use {title}"), format!("The arguments weren't valid JSON ({e}).")),
        };
        let permission = match (self.mode, t.mode) {
            (_, ToolMode::Off) => Permission::Refuse,
            (Mode::Plan, _) if !t.read_only => Permission::Refuse,
            (Mode::Plan, _) | (Mode::Bypass, _) | (Mode::Auto, ToolMode::Allow) => Permission::Run,
            (Mode::Auto, _) if self.state.approvals.allowed(&self.chat_id, Risk::Connector) => Permission::Run,
            (Mode::Auto, ToolMode::Ask) => Permission::Ask,
        };
        match permission {
            Permission::Refuse => return Outcome::error(format!("Couldn't use {title}"), "This connector tool isn't allowed in this mode."),
            Permission::Ask => {
                let preview = Preview {
                    title: format!("Use {} from the {} connector", t.tool, t.connector_name),
                    kind: "text",
                    detail: Some(serde_json::to_string_pretty(&args).unwrap_or_default()),
                    note: Some("Connectors are programs on this PC; what this does is up to the connector.".into()),
                };
                if self.ask(call, Risk::Connector, preview).await == Decision::Deny {
                    self.log(&format!("Declined: {title}"));
                    return Outcome::denied(format!("Use {title}"));
                }
            }
            Permission::Run => {}
        }
        self.emit("agent:tool_start", json!({ "chat_id": self.chat_id, "call_id": call.id, "tool": call.name }));
        self.log(&format!("Used {title}"));
        match connectors::call(&self.state, &t.connector_id, &t.tool, args).await {
            Ok((false, text)) => Outcome::ok(text.clone(), format!("Used {title}"), "text", Some(text)),
            Ok((true, text)) => Outcome::error(format!("{title} reported a problem"), text),
            Err(e) => Outcome::error(format!("Couldn't use {title}"), e),
        }
    }

    /// Web access is off and the model needs it: the user is asked in a
    /// popup. A yes turns web on for this chat and the turn carries on with
    /// the web tools; a no lets the model answer without them.
    async fn request_web(&self, call: &ToolCall) -> Outcome {
        let args: Value = serde_json::from_str(&call.arguments).unwrap_or_default();
        let reason = args.get("reason").and_then(Value::as_str).map(str::trim).filter(|r| !r.is_empty()).unwrap_or("this request").to_string();
        let preview = Preview {
            title: format!("Use the web for: {reason}"),
            kind: "text",
            detail: Some(reason.clone()),
            note: Some("Turns on web access for this chat only. Searches and pages go out from this PC.".into()),
        };
        if self.ask(call, Risk::Read, preview).await == Decision::Deny {
            self.log(&format!("Kept the web off (asked for: {reason})"));
            return Outcome::ok(
                "The user kept web access off. Answer as well as you can without it, and say in a sentence what you couldn't check.",
                format!("Web stayed off: {reason}"),
                "text",
                None,
            );
        }
        {
            let conn = self.state.db.lock().unwrap();
            if let Err(e) = db::set_chat_web(&conn, &self.chat_id, true) {
                return Outcome::error("Couldn't turn on web access", e);
            }
            db::log_action(&conn, "privacy", "Turned on web access for one chat (the assistant asked)");
        }
        self.emit("chat:web", json!({ "chat_id": self.chat_id, "web": true }));
        let mut o = Outcome::ok(
            "The user turned on web access for this chat. Go ahead now: use web_search, fetch_page or weather to finish what they asked.",
            format!("Web turned on: {reason}"),
            "text",
            None,
        );
        o.meta["web_granted"] = json!(true);
        o
    }

    async fn ask(&self, call: &ToolCall, risk: Risk, preview: Preview) -> Decision {
        let (tx, mut rx) = oneshot::channel();
        let pending = PendingApproval { chat_id: self.chat_id.clone(), call_id: call.id.clone(), tool: call.name.clone(), risk, preview };
        self.state.approvals.waiting.lock().unwrap().insert(call.id.clone(), (pending.clone(), tx));
        self.emit("agent:approval", serde_json::to_value(&pending).unwrap_or_default());
        let decision = loop {
            tokio::select! {
                d = &mut rx => break d.unwrap_or(Decision::Deny),
                _ = tokio::time::sleep(Duration::from_millis(250)) => {
                    if self.cancel.load(Ordering::Relaxed) {
                        self.state.approvals.waiting.lock().unwrap().remove(&call.id);
                        break Decision::Deny;
                    }
                }
            }
        };
        self.emit("agent:approval_done", json!({ "chat_id": self.chat_id, "call_id": call.id }));
        decision
    }

    /// Agent actions go in the activity log, encrypted (they name files).
    fn log(&self, summary: &str) {
        db::log_action_enc(&self.state.db.lock().unwrap(), &self.cipher, "agent", summary);
    }
}

/// Tools that come from outside the built-in set, for one turn.
#[derive(Default)]
struct Extras {
    mcp: std::collections::HashMap<String, OfferedTool>,
    skills: Vec<Skill>,
    problems: Vec<String>,
}

impl Outcome {
    fn with_text(mut self, text: &str) -> Outcome {
        self.text = text.to_string();
        self
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn long_tool_results_are_cut_to_fit() {
        let long = Outcome::ok("x".repeat(60_000), "Read an email", "text", None);
        let cut = fit_result(long, 8192);
        assert!(cut.text.chars().count() < 8_400, "{}", cut.text.len());
        assert!(cut.text.contains("[Cut here: 51810 more characters"));
        let short = fit_result(Outcome::ok("hello", "t", "text", None), 8192);
        assert_eq!(short.text, "hello");
    }

    use super::*;

    #[test]
    fn instructions_explain_folders_and_mode() {
        let d = std::env::temp_dir().join(format!("sulcusai-ag-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(d.join("proj")).unwrap();
        let sb = Sandbox::new(&[d.join("proj").display().to_string()]);
        let plan = tool_instructions(&sb, Mode::Plan);
        assert!(plan.contains("proj/") && plan.contains("PLAN mode"));
        assert!(tool_instructions(&Sandbox::new(&[]), Mode::Auto).contains("📁"));
    }

    #[test]
    fn approving_for_the_chat_remembers_the_risk() {
        let a = Approvals::default();
        let (tx, _rx) = oneshot::channel();
        let p = PendingApproval {
            chat_id: "c".into(),
            call_id: "k".into(),
            tool: "write_file".into(),
            risk: Risk::Write,
            preview: Preview { title: "t".into(), kind: "text", detail: None, note: None },
        };
        a.waiting.lock().unwrap().insert("k".into(), (p, tx));
        assert_eq!(a.pending("c").len(), 1);
        assert!(a.answer("k", Decision::AllowForChat));
        assert!(a.allowed("c", Risk::Write));
        assert!(!a.allowed("c", Risk::Execute));
        assert!(!a.answer("k", Decision::Allow), "already answered");
        a.forget_chat("c");
        assert!(!a.allowed("c", Risk::Write));
    }
}

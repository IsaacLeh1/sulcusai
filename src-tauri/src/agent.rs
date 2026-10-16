// SPDX-License-Identifier: AGPL-3.0-only
//! The agent loop: the model replies, may call tools, sees their results,
//! and continues until it answers without calling a tool.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
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
use crate::compact;
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
        return if mode == Mode::Plan {
            "\n\nNo folders are shared with you, so you can't open files yet. If you need to look at the user's files to plan, \
             call request_folder with the folder you'd look in; they approve it in a popup."
                .into()
        } else {
            "\n\nNo folders are shared with you yet. When the user wants something made or changed on their PC (a website, an app, \
             a script, documents), don't just paste the code: call request_folder with a folder to work in (for example a new folder \
             on their Desktop) and say what it's for. They approve it in a popup; then you can create the files, install what's needed \
             and run it in this same reply."
                .into()
        };
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
         - When asked to build something (a website, an app, a script), do it on their PC: make the files, install what's needed and run it. \
         Start servers and watchers with run_command and background: true, then open the result for the user with open_path. Don't stop at showing code.\n\
         - Need a folder that isn't shared? Call request_folder.\n\
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
        let (folders, project, memories, memory_on, request, features, incognito) = {
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
            (folders, project, memories, memory_on, request, on, chat.incognito)
        };
        let mut sandbox = Sandbox::new(&folders);
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
        let mut extra: Vec<&str> = if self.use_tools { vec!["app_status", "open_page", "request_folder"] } else { Vec::new() };
        if self.use_tools && self.mode != tools::Mode::Plan {
            extra.push("open_path");
        }
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
        // Asking (and working with) the assistants of people linked in People.
        let people_note = if self.use_tools && self.mode != tools::Mode::Plan && features.contains(&Feature::People) {
            crate::sync::people::assistant_note(&self.state)
        } else {
            None
        };
        if people_note.is_some() {
            extra.extend(["ask_person", "work_with_person"]);
        }
        // The user's other chats: searching, reading, and asking their assistants.
        let chats_note = self.use_tools && features.contains(&Feature::ChatTeam) && !incognito;
        if chats_note {
            extra.extend(["search_chats", "read_chat"]);
            if self.mode != tools::Mode::Plan && !tools::chats::is_asked(&self.chat_id) {
                extra.push("message_chat");
            }
        }
        // Web tools when this chat may go online; otherwise a way to ask.
        if self.use_tools {
            if crate::web::allowed_for_chat(&self.state.db.lock().unwrap(), &self.chat_id) {
                extra.extend(["weather", "web_search", "fetch_page"]);
            } else if !crate::offline::blocks(crate::offline::Keep::Web) {
                // Fully offline: a yes couldn't turn the web on anyway.
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
                 (who and what, where, what's happening, what's around, the light) instead of describing it in words; a fuller \
                 description makes a better picture. Pictures come out as realistic photos unless they ask for another look \
                 (cartoon, anime, painting, watercolor, sketch, 3D, pixel art): then set style. When they want it to look real, \
                 like a photo someone took, or it shows people or an everyday scene, use style phone_photo. \
                 To change a picture they attached or one made here (add a hat, change the background, make it night), \
                 call edit_image with what to change. The result appears in the chat by itself, with a button to save it, \
                 so don't paste links or ids; say in a sentence what you made. If making it fails, say so in a sentence and \
                 don't guess at causes or send them to other pages.",
                media_note.join(", ")
            ));
        }
        if let Some(note) = &people_note {
            about.push_str(note);
        }
        if chats_note {
            about.push_str(if tools::chats::is_asked(&self.chat_id) {
                "\n\nYou can look through the user's other chats with search_chats and read_chat."
            } else {
                "\n\nThe user's other chats on this PC work with you: look through them with search_chats and read_chat, and give another \
                 chat's assistant a piece of work or a question with message_chat (or start a new chat for it). It works there with \
                 the same model, one at a time, and its reply comes back to you. Use it when the user asks, or to split up a big job."
            });
        } else if self.use_tools && !incognito {
            // Chats don't work together: other chats' notes aren't this chat's work.
            about.push_str(
                "\n\nThis chat works on its own. Notes other chats left for picking up their work (HANDOFF.md, next steps, \
                 session notes) aren't for you: don't open or continue them unless the user asks you to pick up where another \
                 chat left off.",
            );
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
        // A compacted chat: the latest summary stands in for what came before it.
        if let (_, Some(summary)) = compact::after_last(db::messages(&self.state.db.lock().unwrap(), &self.cipher, &self.chat_id)) {
            about.push_str(&compact::prompt_section(&summary));
        }
        let project_id = project.as_ref().map(|p| p.id.clone());
        let system_of = |about: &str| if about.trim().is_empty() { self.base.clone() } else { format!("{}\n\n{}", self.base, about.trim_start()) };
        let mut system = system_of(&about);
        let checkpoint_dir = self.state.paths.data.join("checkpoints");

        let mut result = TurnResult { last: None, context: ContextInfo::default(), tps: None, cancelled: false };
        let max_steps = adv.max_steps();
        // Calls already made this turn, to catch a model asking for the same thing again.
        let mut done_calls: Vec<String> = Vec::new();
        let mut repeats = 0;
        for step in 0..max_steps {
            // What the person added while it worked: part of the conversation from here.
            let added = crate::inbox::take(&self.chat_id);
            if !added.is_empty() {
                for a in added {
                    let mut meta = json!({ "added": true });
                    if !a.images.is_empty() {
                        meta["images"] = json!(a.images);
                    }
                    let text = if a.text.is_empty() { "(See the picture.)".to_string() } else { a.text };
                    self.save(&Message { id: uuid::Uuid::new_v4().to_string(), chat_id: self.chat_id.clone(), role: "user".into(), content: text, created_at: now(), meta: Some(meta), ..Default::default() })?;
                }
                self.emit("agent:step", json!({ "chat_id": self.chat_id }));
                self.emit("chat:inbox", json!({ "chat_id": self.chat_id, "waiting": crate::inbox::unread(&self.chat_id) }));
            }
            self.rest_if_hot().await;
            let (history, _) = compact::after_last(db::messages(&self.state.db.lock().unwrap(), &self.cipher, &self.chat_id));
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
                let can_edit = tools.as_ref().and_then(Value::as_array).is_some_and(|t| t.iter().any(|d| d.pointer("/function/name").and_then(Value::as_str) == Some("edit_image")));
                chat::attach_images(&mut messages, &kept, see, can_edit);
                if let Some((_, f)) = &forced {
                    messages.push(json!({ "role": "user", "content": f.nudge }));
                }
                // The forced try writes JSON, not words for the user: don't show it.
                let quiet = forced.is_some();
                let (ep, offer) = match &forced {
                    Some((ep, _)) => (ep, None),
                    None => (&self.ep, tools.as_ref()),
                };
                // The model has read the person's messages once it starts answering
                // (or calls a tool without a word): the window shows "Read".
                let newest_user = kept.iter().rev().find(|m| m.role == "user").map(|m| m.created_at);
                let read = AtomicBool::new(false);
                let tell_read = || {
                    if let (false, Some(up_to)) = (read.swap(true, Ordering::Relaxed), newest_user) {
                        self.emit("chat:read", json!({ "chat_id": self.chat_id, "up_to": up_to }));
                    }
                };
                let result = chat::stream(ep, messages, offer, &self.cancel, |d| {
                    tell_read();
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
                if result.as_ref().is_ok_and(|r| !r.cancelled) {
                    tell_read();
                }
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
                let offered: Vec<String> = tools.as_ref().and_then(Value::as_array).into_iter().flatten().filter_map(|t| t.pointer("/function/name").and_then(Value::as_str).map(str::to_string)).collect();
                // Asked to make something and pasted it (or steps) instead: one
                // more try that can only make it on the PC. The pasted reply is dropped.
                if step == 0 && tools.is_some() && self.ep.cloud.is_none() && self.mode != tools::Mode::Plan && finished.tool_calls.is_empty() && !finished.cancelled && chat::builds_in_chat(asked, &finished.content, &offered) {
                    if let Some(f) = tools.as_ref().and_then(chat::forced_build) {
                        let mut ep = self.ep.clone();
                        ep.extra["response_format"] = f.response_format.clone();
                        ep.extra["max_tokens"] = json!((self.ep.ctx / 2).clamp(1024, 8192));
                        forced = Some((ep, f));
                        first = Some(finished);
                        message_id = uuid::Uuid::new_v4().to_string();
                        self.emit("agent:step", json!({ "chat_id": self.chat_id }));
                        continue;
                    }
                }
                if step == 0 && tools.is_some() && self.ep.cloud.is_none() && finished.tool_calls.is_empty() && !finished.cancelled && chat::stalls(asked, &finished.content, &offered) {
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
                // Something was added while it wrote this: it answers that too.
                if crate::inbox::waiting(&self.chat_id) && step < max_steps - 1 {
                    self.emit("agent:step", json!({ "chat_id": self.chat_id }));
                    continue;
                }
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
                // The user shared a folder: the file tools work there from the next step on.
                if outcome.meta["folder_granted"] == true {
                    let folders = {
                        let conn = self.state.db.lock().unwrap();
                        db::chat(&conn, &self.cipher, &self.chat_id).map(|c| projects::chat_folders(&conn, &c)).unwrap_or_default()
                    };
                    let before = tool_instructions(&sandbox, self.mode);
                    sandbox = Sandbox::new(&folders);
                    about = about.replacen(&before, &tool_instructions(&sandbox, self.mode), 1);
                    system = system_of(&about);
                    if let Some(Value::Array(defs)) = tools.as_mut() {
                        let mut add = tools::definitions(self.mode, true, false).as_array().cloned().unwrap_or_default();
                        if features.contains(&Feature::Documents) {
                            for name in ["read_document", "create_document"] {
                                if let Some(d) = tools::find(name).filter(|d| tools::permission(self.mode, d.risk, false) != Permission::Refuse) {
                                    add.push(json!({ "type": "function", "function": { "name": d.name, "description": d.description, "parameters": (d.params)() } }));
                                }
                            }
                        }
                        let has = |d: &Value, defs: &[Value]| defs.iter().any(|x| x.pointer("/function/name") == d.pointer("/function/name"));
                        for d in add {
                            let name = d.pointer("/function/name").and_then(Value::as_str).unwrap_or_default();
                            if !has(&d, defs) && !adv.forbids(tools::find(name).map_or(Risk::Connector, |t| t.risk)) {
                                defs.push(d);
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
        if call.name == "request_folder" {
            return self.request_folder(call, sandbox).await;
        }
        if call.name == "message_chat" {
            return self.message_chat(call).await;
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
        // Chats not working together: another chat's handoff note needs the person's word.
        if let Some(refused) = self.handoff_gate(call, &args, sandbox).await {
            return refused;
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

    /// Asks another of the user's chats (or a new one): it works on it there
    /// with the model that's loaded, and its reply comes back. Stopping this
    /// reply stops that one too. (Boxed, with its type spelled out: that
    /// chat's turn runs this same agent.)
    fn message_chat<'a>(&'a self, call: &'a ToolCall) -> std::pin::Pin<Box<dyn std::future::Future<Output = Outcome> + Send + 'a>> {
        Box::pin(self.ask_chat(call))
    }

    async fn ask_chat(&self, call: &ToolCall) -> Outcome {
        let args: Value = serde_json::from_str(&call.arguments).unwrap_or_default();
        let text = args.get("message").and_then(Value::as_str).map(str::trim).unwrap_or("").to_string();
        if text.is_empty() {
            return Outcome::error("Ask another chat", "Say what to ask or tell it (message).");
        }
        let Some(app) = self.state.app.get().cloned() else {
            return Outcome::error("Ask another chat", "Other chats can't be reached right now.");
        };
        let new_title = args.get("new_chat_title").and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty());
        let wanted = args.get("chat").and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty());
        let (target, from_title) = {
            let conn = self.state.db.lock().unwrap();
            let here = db::chat(&conn, &self.cipher, &self.chat_id);
            let from_title = here.as_ref().map_or_else(|| "another chat".to_string(), |c| c.title.clone());
            let target = match (wanted, new_title) {
                (Some(w), _) => tools::chats::find(&conn, &self.cipher, &self.chat_id, w),
                (None, Some(title)) => {
                    let model = here.as_ref().and_then(|c| c.model_id.clone());
                    let project = here.as_ref().and_then(|c| c.project_id.clone());
                    db::create_chat_in(&conn, &self.cipher, model, project, false).and_then(|c| {
                        db::set_chat_title(&conn, &self.cipher, &c.id, title)?;
                        Ok(db::Chat { title: title.to_string(), ..c })
                    })
                }
                (None, None) => Err("Name the chat to ask (chat), or give a title for a new one (new_chat_title).".to_string()),
            };
            (target, from_title)
        };
        let target = match target {
            Ok(t) => t,
            Err(e) => return Outcome::error("Ask another chat", e),
        };
        if new_title.is_some() {
            use tauri::Emitter;
            app.emit("chat:titled", json!({ "chat_id": target.id, "title": target.title })).ok();
        }
        if self.state.generations.lock().unwrap().contains_key(&target.id) {
            return Outcome::error(
                format!("Ask \"{}\"", target.title),
                format!("\"{}\" is busy answering something else. Try again in a moment, or ask another chat.", target.title),
            );
        }
        self.emit("agent:tool_start", json!({ "chat_id": self.chat_id, "call_id": call.id, "tool": call.name }));
        let asking = crate::Asking { ep: self.ep.clone(), tools: self.use_tools, vision: self.vision, from: (self.chat_id.clone(), from_title) };
        let reply = crate::run_asked(&app, &self.state, target.id.clone(), text, asking);
        tokio::pin!(reply);
        let reply = loop {
            tokio::select! {
                r = &mut reply => break r,
                _ = tokio::time::sleep(Duration::from_millis(250)) => {
                    if self.cancel.load(Ordering::Relaxed) {
                        if let Some(flag) = self.state.generations.lock().unwrap().get(&target.id) {
                            flag.store(true, Ordering::Relaxed);
                        }
                    }
                }
            }
        };
        let title = format!("Asked \"{}\"", target.title);
        self.log(&title);
        let mut o = match reply {
            Ok(Some(m)) if !m.content.trim().is_empty() => {
                Outcome::ok(format!("\"{}\" answered:\n{}", target.title, m.content.trim()), title, "text", Some(m.content.trim().to_string()))
            }
            Ok(_) => Outcome::error(title, format!("\"{}\" stopped without an answer.", target.title)),
            Err(e) => Outcome::error(title, e),
        };
        o.meta["open_chat"] = json!(target.id);
        o.meta["open_title"] = json!(target.title);
        o
    }

    /// The model needs a folder it can't reach: the user is asked in a popup
    /// (and can pick another). A yes shares it, and the turn carries on with
    /// the file tools there.
    async fn request_folder(&self, call: &ToolCall, sandbox: &Sandbox) -> Outcome {
        let args: Value = serde_json::from_str(&call.arguments).unwrap_or_default();
        let reason = args.get("reason").and_then(Value::as_str).map(str::trim).filter(|r| !r.is_empty()).unwrap_or("this request").to_string();
        let asked = args.get("path").and_then(Value::as_str).map(str::trim).filter(|p| !p.is_empty()).unwrap_or("");
        let desktop = crate::workspace::desktop(&self.state);
        let suggested = if Path::new(asked).is_absolute() {
            PathBuf::from(asked)
        } else if asked.is_empty() {
            match &desktop {
                Some(d) => d.clone(),
                None => return Outcome::error("Ask for a folder", "Couldn't find the Desktop. Ask again with a full path."),
            }
        } else if Path::new(asked).components().all(|c| matches!(c, std::path::Component::Normal(_))) {
            match &desktop {
                Some(d) => d.join(asked),
                None => return Outcome::error("Ask for a folder", "Couldn't find the Desktop. Ask again with a full path."),
            }
        } else {
            return Outcome::error("Ask for a folder", "Give a full path, or just a new folder's name to make on the Desktop.");
        };
        // Already shared (or inside a shared folder): nothing to ask.
        let shared = sandbox.roots().iter().find(|r| {
            let existing = suggested.ancestors().find(|a| a.exists()).and_then(|a| dunce::canonicalize(a).ok());
            existing.is_some_and(|e| e.starts_with(r))
        });
        if let Some(root) = shared {
            return Outcome::ok(
                format!(
                    "{} is inside {}/, which is already shared with you. Work there directly: write_file makes any folders it needs.",
                    suggested.display(),
                    sandbox.display(root)
                ),
                format!("Already shared: {}", sandbox.display(root)),
                "text",
                None,
            );
        }
        crate::workspace::offer_folder(&call.id, &suggested);
        // The popup (FolderPermission) shows the reason and the folder.
        let preview = Preview { title: format!("Use a folder for: {reason}"), kind: "folder", detail: Some(reason.clone()), note: Some(suggested.display().to_string()) };
        if self.ask(call, Risk::Read, preview).await == Decision::Deny {
            crate::workspace::chosen_folder(&call.id);
            self.log(&format!("Didn't share a folder (asked for: {reason})"));
            return Outcome::ok(
                "The user didn't share a folder. Don't try to reach their files another way: do what you can without them \
                 (for code, show it in the reply and say where it would go), and say in a sentence what you couldn't do.",
                format!("No folder shared: {reason}"),
                "text",
                None,
            );
        }
        let chosen = crate::workspace::chosen_folder(&call.id).unwrap_or(suggested);
        if !crate::features::is_on(&self.state.db.lock().unwrap(), Feature::Files) {
            if let Err(e) = crate::features::set(&self.state.db.lock().unwrap(), Feature::Files, true) {
                return Outcome::error("Couldn't share the folder", e);
            }
            self.emit("features:changed", json!({}));
        }
        let shown = match crate::workspace::share_folder(&self.state, &chosen, true, true) {
            Ok(s) => s,
            Err(e) => return Outcome::error("Couldn't share the folder", format!("{e} Tell the user, and ask them to share one with the 📁 button.")),
        };
        self.emit("folders:changed", json!({}));
        let name = Path::new(&shown).file_name().map_or_else(|| shown.clone(), |n| n.to_string_lossy().to_string());
        let mut o = Outcome::ok(
            format!(
                "The user shared {shown}. Go ahead now: it's {name}/ for the file tools (write files as {name}/index.html, and so on), \
                 and run_command can run there (folder \"{name}\")."
            ),
            format!("Shared a folder: {name}"),
            "text",
            None,
        );
        o.meta["folder_granted"] = json!(true);
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

    /// Reading another chat's handoff note (read_file, or a command naming
    /// one) while chats don't work together: allowed when the person's own
    /// message asks to pick up where another chat left off, else only from a
    /// card they answer (in every mode, Bypass too).
    async fn handoff_gate(&self, call: &ToolCall, args: &Value, sandbox: &Sandbox) -> Option<Outcome> {
        let (path, from) = match call.name.as_str() {
            "read_file" => {
                let path = sandbox.resolve(args.get("path")?.as_str()?).ok()?;
                let from = crate::handoff_guard::guarded(&self.state, &self.cipher, &self.chat_id, &path)?;
                (Some(path), from)
            }
            "run_command" => {
                let command = args.get("command")?.as_str()?;
                if !crate::handoff_guard::command_names_one(command) || crate::features::is_on(&self.state.db.lock().unwrap(), Feature::ChatTeam) {
                    return None;
                }
                (None, "another chat".to_string())
            }
            _ => return None,
        };
        let asked = {
            let conn = self.state.db.lock().unwrap();
            db::messages(&conn, &self.cipher, &self.chat_id).into_iter().rev().find(|m| m.role == "user").map(|m| m.content).unwrap_or_default()
        };
        let name = path.as_ref().map_or_else(|| "a handoff note".to_string(), |p| p.file_name().unwrap_or_default().to_string_lossy().into_owned());
        if !crate::handoff_guard::asked_to_pick_up(&asked) {
            let preview = Preview {
                title: format!("Let this chat read {name}, a handoff from {from}?"),
                kind: "handoff",
                detail: Some(format!(
                    "{name} is a note {from} left for picking up its work. Chats don't share work while “Chats work together” is off. Allow it only if you want this chat to continue where that one left off."
                )),
                note: None,
            };
            if self.ask(call, Risk::Read, preview).await == Decision::Deny {
                self.log(&format!("Kept {name} (another chat's handoff) from this chat"));
                return Some(Outcome::denied(format!("Read {name}")).with_text(
                    "That's a handoff note another chat left, and the user didn't let this chat pick it up. Don't read it another way (search, commands); carry on with what they asked here.",
                ));
            }
        }
        if let Some(p) = &path {
            crate::handoff_guard::allow(&self.state, &self.cipher, &self.chat_id, p);
        }
        None
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
        // No folder: it asks for one rather than pasting code.
        assert!(tool_instructions(&Sandbox::new(&[]), Mode::Auto).contains("call request_folder"));
        assert!(tool_instructions(&Sandbox::new(&[]), Mode::Plan).contains("call request_folder"));
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

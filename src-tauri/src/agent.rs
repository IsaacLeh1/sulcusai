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
use crate::tools::allowances::{self, Effect};
use crate::tools::{self, Mode, Outcome, Permission, Preview, Risk};
use crate::connectors::{self, OfferedTool, Skill, ToolMode};
use crate::compact;
use crate::{memory, projects, AppState};

mod approvals;
mod execute;
mod instructions;
mod prepare;
mod requests;
#[cfg(test)]
mod tests;

pub use approvals::{Approvals, Decision, PendingApproval};
use instructions::tool_instructions;

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
    /// Nobody's watching (a scheduled task): nothing waits for an answer
    /// to a popup; what would need one is declined.
    pub unattended: bool,
    /// Approved ahead (a timed action the user set up): these tools run
    /// without asking, and nothing else that changes or sends runs at all.
    pub pre_approved: Option<Vec<String>>,
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

fn now() -> i64 {
    db::now_ms()
}

/// A tool result cut to about a third of the model's memory, so one long
/// email, page or file can't crowd out the conversation (small models have
/// 8K tokens or less; about 3 characters a token, to be safe), and to the
/// tool's own limit. A command keeps its end as well as its start: that's
/// where errors are.
fn fit_result(mut outcome: Outcome, name: &str, ctx: u32) -> Outcome {
    let share = (ctx as usize / 3) * 3;
    let max = if share == 0 { tools::result_limit(name) } else { share.min(tools::result_limit(name)) };
    let total = outcome.text.chars().count();
    if total <= max {
        return outcome;
    }
    let left = total - max;
    outcome.text = if matches!(name, "run_command" | "command_output") && max > 4_000 {
        let head: String = outcome.text.chars().take(2_000).collect();
        let tail: String = outcome.text.chars().skip(total - (max - 2_000)).collect();
        format!("{head}\n\n[Cut here: {left} more characters of output weren't shown, to fit your memory. The end follows.]\n\n{tail}")
    } else {
        let kept: String = outcome.text.chars().take(max).collect();
        let next = if name == "read_file" { "Read on with start_line if you need the rest." } else { "Answer from what's above." };
        format!("{kept}\n\n[Cut here: {left} more characters weren't shown, to fit your memory. {next}]")
    };
    outcome
}

/// Tools that reach the internet: a failure is often the network, worth one retry.
fn is_network_tool(name: &str) -> bool {
    matches!(name, "web_search" | "fetch_page" | "wikipedia" | "research" | "market_quote" | "find_places" | "weather" | "news")
}

/// The end of the run of calls that can go together starting at `from`
/// (exclusive): consecutive calls that are each `ok`. A run of one isn't
/// worth it, so the caller only acts when the run is 2 or longer.
fn run_end(ok: &[bool], from: usize) -> usize {
    ok[from..].iter().position(|b| !b).map_or(ok.len(), |n| from + n)
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
            let msgs = db::messages(&conn, &self.cipher, &self.chat_id);
            let request = msgs.iter().find(|m| m.id == self.turn_id).map(|m| m.content.clone()).unwrap_or_default();
            // Their last few messages, newest first: "do it for me" refers back to the ask.
            let recent: Vec<String> = msgs.iter().rev().filter(|m| m.role == "user").take(3).map(|m| m.content.clone()).collect();
            let folders = if on.contains(&Feature::Files) { (projects::chat_folders(&conn, &chat), projects::general_folders(&conn, &chat)) } else { Default::default() };
            (folders, project, memories, memory_on, (request, recent), on, chat.incognito)
        };
        // Every chat's folders are marked as other work (`tool_instructions`).
        let (folders, general) = folders;
        let (request, recent_asks) = request;
        // Asked to make a site, an app or code (not a document): the document
        // tool stays out, or small models reach for it and fail.
        // Their own words to edit (lyrics, a poem, a text pasted in): answered
        // in words. Lyrics full of imagery made pictures, "lyrics" made songs,
        // and "make these better" read as a build that wanted a folder.
        let edits_text = chat::edits_text(&request);
        // Asked to write a song: its words, in the chat (not the song made).
        let writes_song = if edits_text { None } else { chat::writes_song_in(&request, &recent_asks.iter().skip(1).map(String::as_str).collect::<Vec<_>>()) };
        let build_ask = chat::is_build_ask(&recent_asks.iter().map(String::as_str).collect::<Vec<_>>()) && !chat::wants_a_document(&request) && !edits_text;
        // The home folder is open for searching and reading (the person
        // allowed it); changes stay in the shared folders.
        let file_feature = features.contains(&Feature::Files);
        let mut sandbox = if file_feature { Sandbox::new(&folders).with_home() } else { Sandbox::new(&folders) }.with_general(&general);
        // An online model gets what it reads: keys and saved logins stay closed to it.
        if self.ep.cloud.is_some() {
            sandbox = sandbox.for_online();
        }
        let files = self.use_tools && file_feature && (!sandbox.is_empty() || sandbox.home().is_some());
        let memory_tools = self.use_tools && memory_on;
        let mut defs = tools::definitions(self.mode, files, memory_tools).as_array().cloned().unwrap_or_default();
        // Nothing shared yet: finding, reading and running, but nothing that
        // writes (that needs a shared folder: request_folder).
        if sandbox.is_empty() {
            defs.retain(|d| d.pointer("/function/name").and_then(Value::as_str).is_none_or(|n| tools::find(n).is_none_or(|t| t.risk != tools::Risk::Write) || tools::group(n) != "files"));
        }
        let mut extras = Extras { allowances: allowances::list(&self.state.db.lock().unwrap(), &self.cipher), ..Default::default() };
        if self.use_tools && features.contains(&Feature::Connectors) {
            let (mcp_defs, mcp_map, problems) = connectors::offer(&self.state, &self.cipher).await;
            defs.extend(mcp_defs);
            extras.mcp = mcp_map;
            extras.problems = problems;
            extras.skills = connectors::active_skills(&self.state.db.lock().unwrap(), &self.cipher);
            // Longer skills are loaded when used; skills with files of their own read them with it too.
            if !connectors::split_skills(&extras.skills).1.is_empty() || extras.skills.iter().any(|s| !s.files.is_empty()) {
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
        // Exact math and a code sandbox, in every chat with tools.
        if self.use_tools {
            extra.extend(["calculate", "run_code"]);
        }
        if self.use_tools && self.mode != tools::Mode::Plan && features.contains(&Feature::Scheduled) {
            extra.push("schedule_task");
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
        // The documents library, when it holds something.
        let library_note = (self.use_tools && features.contains(&Feature::Documents) && crate::doclib::has_documents(&self.state))
            .then(|| crate::doclib::summary(&self.state))
            .flatten()
            .map(|held| {
                format!(
                    "\n\nThe user's documents library holds {held}. For questions about their own documents, reports, contracts or notes, \
                     search it with search_documents first and answer from what it finds; if it finds nothing, say so. After each fact, put \
                     its source link exactly as the search gives it, like [Lease.pdf, page 3](doc:…), so they can open the file."
                )
            });
        if library_note.is_some() {
            extra.push("search_documents");
        }
        if self.use_tools && features.contains(&Feature::Meetings) && crate::meeting::count(&self.state.db.lock().unwrap()) > 0 {
            extra.extend(["search_meetings", "read_meeting"]);
        }
        if files && features.contains(&Feature::Documents) {
            extra.push("read_document");
            if self.mode != tools::Mode::Plan && !build_ask {
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
            extra.extend(["browser_open", "browser_read", "browser_click", "browser_type", "browser_back", "browser_scroll", "browser_key"]);
        }
        let programs_offered = self.use_tools && features.contains(&Feature::Programs) && cfg!(windows);
        if programs_offered {
            extra.extend(["programs_list", "program_open", "program_read", "program_click", "program_type", "program_key", "program_scroll"]);
            // Using the screen itself needs a model that sees pictures.
            let chat_model = crate::db::chat(&self.state.db.lock().unwrap(), &self.cipher, &self.chat_id).and_then(|c| c.model_id);
            if crate::vision::can_read(&self.state, chat_model.as_deref()) {
                extra.push("use_screen");
            }
        }
        // Asking (and working with) the assistants of people linked in People.
        let people_note = if self.use_tools && self.mode != tools::Mode::Plan && features.contains(&Feature::People) {
            crate::sync::people::assistant_note(&self.state)
        } else {
            None
        };
        if people_note.is_some() {
            extra.extend(["ask_person", "work_with_person", "message_person"]);
        }
        // This chat's notes: its memory and plan, kept up to date as it works.
        if self.use_tools && !incognito {
            extra.extend(["read_chat_notes", "update_chat_notes", "check_plan_step"]);
        }
        // The user's other chats: searching, reading, and asking their assistants.
        let chats_note = self.use_tools && features.contains(&Feature::ChatTeam) && !incognito;
        if chats_note {
            extra.extend(["search_chats", "read_chat"]);
            if self.mode != tools::Mode::Plan && !tools::chats::is_asked(&self.chat_id) {
                extra.push("message_chat");
            }
        }
        // Chats the user linked with this one: read and messaged by name,
        // with or without the feature.
        let linked = if self.use_tools && !incognito {
            crate::projects::linked(&self.state.db.lock().unwrap(), &self.cipher, &self.chat_id)
        } else {
            Vec::new()
        };
        if !linked.is_empty() {
            for name in ["read_chat", "message_chat"] {
                let may = name == "read_chat" || (self.mode != tools::Mode::Plan && !tools::chats::is_asked(&self.chat_id));
                if may && !extra.contains(&name) {
                    extra.push(name);
                }
            }
        }
        // Web tools when this chat may go online; otherwise a way to ask.
        if self.use_tools {
            if crate::web::allowed_for_chat(&self.state.db.lock().unwrap(), &self.chat_id) {
                extra.extend(["weather", "web_search", "fetch_page", "wikipedia", "research", "market_quote", "find_places"]);
            } else if !crate::offline::blocks(crate::offline::Keep::Web) {
                // Fully offline: a yes couldn't turn the web on anyway.
                extra.push("request_web");
            }
        }
        // Words to edit or write: nothing to make, save or run, so nothing that would.
        let can_sing = extra.contains(&"create_music");
        if edits_text || writes_song.is_some() {
            extra.retain(|n| !matches!(*n, "create_image" | "edit_image" | "create_video" | "create_music" | "create_document" | "request_folder" | "open_path" | "run_code"));
            defs.retain(|d| d.pointer("/function/name").and_then(Value::as_str).is_none_or(|n| tools::group(n) != "files" || tools::find(n).is_none_or(|t| t.risk == tools::Risk::Read)));
            media_note.clear();
        }
        // A small memory and a request to build: only the tools for building
        // (and none of the notes about pictures, people or other chats), so the
        // files it writes have room.
        let small = self.ep.cloud.is_none() && self.ep.ctx < crate::TOOL_CTX;
        let lean = build_ask && small;
        if lean {
            extra.retain(|n| tools::for_building(n));
            defs.retain(|d| d.pointer("/function/name").and_then(Value::as_str).is_some_and(tools::for_building));
            media_note.clear();
        }
        // Likewise a picture request: the tools for pictures, so the chat (what
        // the person asked for from the start) isn't pushed out of memory and
        // the pictures keep to it.
        let lean = lean || (small && !media_note.is_empty() && chat::asks_for_picture(&request));
        if lean && !build_ask {
            extra.retain(|n| tools::for_pictures(n));
            defs.retain(|d| d.pointer("/function/name").and_then(Value::as_str).is_some_and(tools::for_pictures));
        }
        let people_note = people_note.filter(|_| !lean);
        let chats_note = chats_note && !lean;
        let library_note = library_note.filter(|_| !lean);
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
        // Unattended, the tools that ask the person for something aren't offered.
        if self.unattended {
            defs.retain(|d| !d.pointer("/function/name").and_then(Value::as_str).unwrap_or_default().starts_with("request_"));
        }
        let mut tools = Some(Value::Array(defs)).filter(|t| t.as_array().is_some_and(|a| !a.is_empty()));

        let mut about = self.about.clone();
        // Notes about tools a small memory may leave out (fit_small), with the tools they're about.
        let mut notes: Vec<(&[&str], String)> = Vec::new();
        if let Some(p) = &project {
            about.push_str(&projects::prompt_section(p));
        }
        // Kept apart too, so privacy mode can leave them out for an online model.
        // On this PC, what it remembers goes with the newest message instead:
        // it's picked anew for each message, and a change in the instructions
        // made the engine read them and every tool again (half a minute and
        // more for a model partly in system memory).
        let recalled = if memory_tools { memory::relevant(&memories, &request) } else { Vec::new() };
        let local = self.ep.cloud.is_none();
        let memory_part = if memory_tools { memory::prompt_section(if local { &[] } else { &recalled }) } else { String::new() };
        let recall_note = if local { memory::recall_note(&recalled) } else { String::new() };
        // This chat's notes go with the newest message too: they change as it works.
        let recall_note = if self.use_tools && !incognito {
            let notes = crate::chatnotes::get(&self.state.db.lock().unwrap(), &self.cipher, &self.chat_id);
            format!("{recall_note}{}", crate::chatnotes::prompt_section(&notes))
        } else {
            recall_note
        };
        about.push_str(&memory_part);
        let mut accounts_part = String::new();
        if !media_note.is_empty() {
            let text = format!(
                "\n\nYou can make {} on this PC. When the user asks for one, call the tool with a vivid, specific description \
                 (who and what, where, what's happening, what's around, the light) instead of describing it in words; a fuller \
                 description makes a better picture. Pictures come out as realistic photos unless they ask for another look \
                 (cartoon, anime, painting, watercolor, sketch, 3D, pixel art): then set style. When they want it to look real, \
                 like a photo someone took, or it shows people or an everyday scene, use style phone_photo. \
                 To change a picture they attached or one made here (add a hat, change the background, make it night), \
                 call edit_image with what to change. Picture models don't know names: when the request names a character, \
                 game, show, brand, person or place, describe exactly how it looks (colors, clothes, hair, shapes) in the \
                 description, and if you aren't sure how it looks, look it up first. \
                 Write each description in full, on its own: what the user wants now, plus anything they asked for earlier that \
                 should stay (the same character, the same style), and nothing from earlier pictures they didn't ask to keep. \
                 To change a picture they already have, use edit_image rather than making a new one. \
                 The result appears in the chat by itself, with a button to save it, \
                 so don't paste links or ids; say in a sentence what you made. If making it fails, say so in a sentence and \
                 don't guess at causes or send them to other pages.",
                media_note.join(", ")
            );
            about.push_str(&text);
            notes.push((&["create_image", "edit_image", "create_video", "create_music"], text));
        }
        if programs_offered {
            let text = "\n\nYou can work the programs on the user's PC: programs_list shows the open windows, program_open starts \
                 one by its Start-menu name, and program_read reads a window as text with numbered buttons, fields, menus and \
                 items. Then program_click, program_type, program_scroll and program_key act by those numbers; each returns the \
                 window again (or a box it opened), so check it before the next step. For websites use the browser instead. \
                 To write something new in an editor (Notepad, Word), start a new document first (program_key ctrl+n): a file \
                 that's already open is the user's, so change it only when they ask. Never type passwords: ask the user to."
                .to_string();
            about.push_str(&text);
            notes.push((&["programs_list", "program_open", "program_read", "program_click", "program_type", "program_key", "program_scroll"], text));
        }
        if edits_text {
            about.push_str(
                "\n\nThe user wants their own words edited (lyrics, a poem or a text): reply in the chat with the revised words, \
                 keeping their meaning, story and imagery, and say briefly what you changed. Don't make a song, a picture or a file \
                 from them, and don't ask for folders: everything you need is in their message and this chat.",
            );
        }
        // A song to write is words in the chat: no tools this turn (a 9B
        // model saved them to a note, and said it couldn't make songs).
        if writes_song.is_some() {
            tools = None;
        }
        // Said in their message too: small models went on writing the song
        // when only the instructions said to ask first.
        let song_nudge = matches!(writes_song, Some(chat::SongAsk::Unclear)).then_some(can_sing).map(|sing| {
            if sing {
                "[Before writing anything, ask me whether I want just the lyrics or the complete song to listen to (you can make the whole song here on this PC), \
                 and ask about anything you need that I didn't say: what it's about, the style, the mood, who sings. Only ask for now.]"
            } else {
                "[Write the song's words here in the chat.]"
            }
        });
        match writes_song {
            Some(chat::SongAsk::Words) => about.push_str(
                "\n\nThe user asked you to write a song's words: reply in the chat with them (a title, then each section labelled, \
                 like [Verse 1] and [Chorus]). Don't make the song itself, a picture or a file. After the words, offer in one short \
                 sentence to make it into a song they can hear, if they'd like.",
            ),
            // Not said whether they want the words or the song: asked first.
            Some(chat::SongAsk::Unclear) if can_sing => about.push_str(
                "\n\nThe user asked you to write a song without saying whether they want just the lyrics or the complete song \
                 to listen to (made here on this PC). Don't write or make it yet: ask them in one short message which they want, \
                 and in the same message ask for anything else you need that they didn't say (what it's about, the style or \
                 genre, the mood, who sings). Keep it to a few quick questions.",
            ),
            Some(chat::SongAsk::Unclear) => about.push_str(
                "\n\nThe user asked you to write a song: reply in the chat with its words (a title, then each section labelled, \
                 like [Verse 1] and [Chorus]); ask first only if what it's about isn't clear. A song they can hear needs a music \
                 model from Studio › Models, which isn't installed: say so in one short sentence after the words.",
            ),
            None => {}
        }
        if let Some(note) = &people_note {
            about.push_str(note);
            notes.push((&["ask_person", "work_with_person"], note.clone()));
        }
        if let Some(note) = &library_note {
            about.push_str(note);
            notes.push((&["search_documents"], note.clone()));
        }
        if chats_note {
            let text = if tools::chats::is_asked(&self.chat_id) {
                "\n\nYou can look through the user's other chats with search_chats and read_chat."
            } else {
                "\n\nThe user's other chats on this PC work with you: look through them with search_chats and read_chat, and give another \
                 chat's assistant a piece of work or a question with message_chat (or start a new chat for it). It works there with \
                 the same model, one at a time, and its reply comes back to you. Use it when the user asks, or to split up a big job."
            };
            about.push_str(text);
            notes.push((&["search_chats", "read_chat", "message_chat"], text.to_string()));
        } else if self.use_tools && !incognito {
            // Chats don't work together: other chats' notes aren't this chat's work.
            about.push_str(
                "\n\nThis chat works on its own. Notes other chats left for picking up their work (HANDOFF.md, next steps, \
                 session notes) aren't for you: don't open or continue them unless the user asks you to pick up where another \
                 chat left off.",
            );
        }
        if !linked.is_empty() {
            let names: Vec<String> = linked.iter().map(|c| format!("“{}” (id {})", c.title, c.id)).collect();
            let text = format!(
                "\n\nThe user linked this chat with {}: you work together. Read what one has done with read_chat, and ask or tell it \
                 something with message_chat (its reply comes back to you) when its work matters to yours or the user asks. Say what you \
                 learned from it.",
                names.join(", ")
            );
            about.push_str(&text);
            notes.push((&["read_chat", "message_chat"], text));
        }
        if self.use_tools {
            accounts_part = tools::app_note(&self.state, &self.cipher);
            about.push_str(&accounts_part);
            about.push_str(&tool_instructions(&sandbox, self.mode));
            self.add_live_data(&sandbox, &mut about, &mut tools, build_ask || !small);
            if tools.as_ref().and_then(Value::as_array).is_some_and(|t| t.iter().any(|d| d.pointer("/function/name").and_then(Value::as_str) == Some("calculate"))) {
                about.push_str(
                    "\n\nFor any arithmetic, percentages, money, dates or counting, call calculate (or run_code for longer work) and use its exact result; \
                     never work numbers out in your head.",
                );
            }
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
        if small && !lean {
            self.fit_small(&recent_asks.join("\n"), &mut about, &mut tools, &notes).await;
        }
        // An online model: what it receives is listed, and in privacy mode
        // the person shares it, leaves out the parts about them, or doesn't.
        if self.ep.cloud.is_some() {
            if let Some(stopped) = self.cloud_gate(&request, &mut about, &memory_part, &accounts_part).await? {
                return Ok(stopped);
            }
        }
        let project_id = project.as_ref().map(|p| p.id.clone());
        let system_of = |about: &str| if about.trim().is_empty() { self.base.clone() } else { format!("{}\n\n{}", self.base, about.trim_start()) };
        let mut system = system_of(&about);
        let checkpoint_dir = self.state.paths.data.join("checkpoints");

        let mut result = TurnResult { last: None, context: ContextInfo::default(), tps: None, cancelled: false };
        let max_steps = adv.max_steps();
        // While it's getting somewhere (a file written or a command run in
        // the last few steps), 10 more steps at a time, up to four times as
        // many: a 20B model adding a feature stopped at 30, one run from
        // done. Not past a limit set in Advanced.
        let most_steps = if adv.guardrails.max_steps.is_some() { max_steps } else { max_steps * 4 };
        let mut limit = max_steps;
        let mut progressed_at: Option<usize> = None;
        // Calls already made this turn, to catch a model asking for the same thing again.
        let mut done_calls: Vec<String> = Vec::new();
        let mut repeats = 0;
        // Files written this turn, and the follow-ups asked for the pieces they need.
        let mut written: Vec<std::path::PathBuf> = Vec::new();
        let mut follow_up: Option<chat::ForcedLookup> = None;
        let mut follow_ups = 0;
        // A follow-up the engine didn't answer with a call: no more this turn.
        let mut follow_ups_failed = false;
        // The person (or a rule they saved) said no to a call this turn.
        let mut declined = false;
        // A site it built: previewed since the last change? What the last
        // preview found wrong, and how many times it was pushed to fix it.
        let mut previewed = false;
        let mut previews = 0;
        let mut problems_left: Option<String> = None;
        let mut fix_rounds = 0;
        // A picture, clip or song made this turn (it's in the chat already).
        let mut made_media = false;
        // Calls that failed this turn and why: the same call again won't do better.
        let mut failed_calls: Vec<(String, String)> = Vec::new();
        let mut failed_repeats = 0;
        // Told once this turn to carry out steps it only explained.
        let mut told_to_act = false;
        // It stopped short (saying what it does next, all thinking and no
        // step, or with the last command failing): told to go on, up to four
        // times a turn.
        let mut go_on: Option<&str> = None;
        let mut midway = 0;
        // The last command it ran failed (and nothing ran since), or the
        // last file it changed doesn't parse.
        let mut failing_run = false;
        let mut broken_file = false;
        // A site it built was checked against what was asked.
        let mut reviewed_asks = false;
        // Told once to step back from a call it kept repeating.
        let mut stepped_back = false;
        // Unfinished lines ("TODO") it wrote this turn, by file.
        let mut stubs: Vec<(std::path::PathBuf, String)> = Vec::new();
        // It changed, ran, made or sent something this turn (not only looked).
        let mut did_something = false;
        // What came before this request was compacted partway through it.
        let mut compacted_mid = false;
        for step in 0..most_steps {
            if step >= limit {
                break;
            }
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
            // Nearly full partway through a task: the conversation before
            // this request is compacted (summarized), keeping the request and
            // the steps taken for it word for word, so the reply has room to
            // finish. Once a task.
            if step > 0 && !compacted_mid && !incognito && self.nearly_full() {
                compacted_mid = true;
                if let Some((old, summary)) = self.compact_before_turn().await {
                    about = compact::with_summary(&about, old.as_deref(), &summary);
                }
            }
            let (history, _) = compact::after_last(db::messages(&self.state.db.lock().unwrap(), &self.cipher, &self.chat_id));
            // The model can't see pictures: the picture reader describes the
            // newest attached ones (once each; the words are kept with the picture).
            let attached: Vec<String> = history
                .iter()
                .filter(|m| m.role == "user")
                .filter_map(|m| m.meta.as_ref()?.get("images")?.as_array().cloned())
                .flatten()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
            let described = if !self.vision && !attached.is_empty() && !crate::vision::readers(&self.state).is_empty() {
                crate::vision::describe_attached(&self.state, &self.cipher, &attached, || {
                    self.emit("chat:status", json!({ "chat_id": self.chat_id, "status": "reading_pictures" }));
                })
                .await
            } else {
                Vec::new()
            };
            // When it doesn't all fit, files it already wrote aren't sent again
            // (it can read them back); when it does, it keeps seeing them, to edit them exactly.
            let (mut kept, mut info) = chat::fit_history(&self.ep, &self.base, &about, tools.as_ref(), &history).await?;
            let room = self.ep.ctx.saturating_sub(info.system_tokens + info.profile_tokens + info.tools_tokens + info.reply_reserve);
            if info.history_tokens > room || info.dropped_messages > 0 {
                (kept, info) = chat::fit_history(&self.ep, &self.base, &about, tools.as_ref(), &chat::without_written(&history)).await?;
            }
            // Outputs removed to make room can be fetched again: the repeat
            // guard lets those lookups through.
            if kept.iter().any(chat::is_trimmed) || info.dropped_messages > 0 {
                done_calls.retain(|k| !tools::find(k.split('{').next().unwrap_or_default()).is_some_and(|t| t.risk == Risk::Read));
            }
            self.state.contexts.lock().unwrap().insert(self.chat_id.clone(), info.clone());
            self.emit("chat:context", json!({ "chat_id": self.chat_id, "context": info }));

            // One message for the step, whatever is tried again in it: what it
            // wrote stays on screen (a new id cleared it, answer and thinking).
            let message_id = uuid::Uuid::new_v4().to_string();
            // Set when the model stalled: one more try that can only be a lookup.
            let mut forced: Option<(Endpoint, chat::ForcedLookup)> = None;
            let mut first = None;
            // A page it wrote needs a file it didn't write: this step writes it.
            if let Some(f) = follow_up.take() {
                let mut ep = self.ep.clone();
                ep.extra["response_format"] = f.response_format.clone();
                ep.extra["max_tokens"] = json!((self.ep.ctx / 2).clamp(1024, 8192));
                forced = Some((ep, f));
                first = Some(chat::Finished::default());
            }
            // Set when its reply only explained what to do: the same step again, told to do it.
            let mut carry_on: Option<&str> = go_on.take();
            // That reply's words and thinking: they stay, ahead of what it writes next.
            let mut aside = (String::new(), String::new());
            let finished = loop {
                self.emit("chat:start", json!({ "chat_id": self.chat_id, "message_id": message_id }));
                let mut messages = chat::api_messages(&system, &memory::with_recall(&kept, &self.turn_id, &recall_note));
                if let Some(n) = carry_on {
                    messages.push(json!({ "role": "user", "content": n }));
                }
                let see = |id: &str| crate::media::image_data_url(&self.state, &self.cipher, id).ok();
                let see: Option<&dyn Fn(&str) -> Option<String>> = if self.vision { Some(&see) } else { None };
                let can_edit = tools.as_ref().and_then(Value::as_array).is_some_and(|t| t.iter().any(|d| d.pointer("/function/name").and_then(Value::as_str) == Some("edit_image")));
                // Songs attached (to work on with create_music) aren't pictures:
                // left out of those, and named in the message instead.
                let songs: Vec<(usize, String, String)> = {
                    let conn = self.state.db.lock().unwrap();
                    kept.iter()
                        .enumerate()
                        .filter(|(_, m)| m.role == "user")
                        .flat_map(|(i, m)| m.meta.as_ref().and_then(|v| v.get("images")).and_then(Value::as_array).cloned().unwrap_or_default().into_iter().map(move |v| (i, v)))
                        .filter_map(|(i, v)| {
                            let id = v.as_str()?.to_string();
                            let item = crate::media::store::get(&conn, &self.cipher, &id).filter(|it| it.kind == "audio")?;
                            let name = if item.prompt.trim().is_empty() { String::new() } else { format!(" “{}”", item.prompt.trim()) };
                            Some((i, id.clone(), format!("[The user attached a song{name} (id {id}, {:.0} s). To work on it, call create_music with song \"{id}\" and the change they ask for.]", item.seconds)))
                        })
                        .collect()
                };
                if songs.is_empty() {
                    chat::attach_images(&mut messages, &kept, see, can_edit, &described);
                } else {
                    let song_ids: Vec<&str> = songs.iter().map(|(_, id, _)| id.as_str()).collect();
                    let pictures: Vec<Message> = kept
                        .iter()
                        .map(|m| {
                            let mut m = m.clone();
                            if let Some(list) = m.meta.as_mut().and_then(|v| v.get_mut("images")).and_then(Value::as_array_mut) {
                                list.retain(|v| v.as_str().is_none_or(|id| !song_ids.contains(&id)));
                            }
                            m
                        })
                        .collect();
                    chat::attach_images(&mut messages, &pictures, see, can_edit, &described);
                    for (i, _, note) in &songs {
                        if let Some(slot) = messages.get_mut(i + 1) {
                            chat::add_text(slot, note);
                        }
                    }
                }
                if let (0, Some(n)) = (step, song_nudge) {
                    if let Some(last) = messages.iter_mut().rev().find(|m| m["role"] == "user") {
                        chat::add_text(last, &format!("\n\n{n}"));
                    }
                }
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
                // What the user has seen of this reply, kept if it fails part-way.
                let mut shown = (String::new(), String::new());
                let result = chat::stream(ep, messages, offer, &self.cancel, |d| {
                    tell_read();
                    if quiet {
                        return;
                    }
                    let (content, thinking) = match d {
                        Delta::Content(t) => (Some(t), None),
                        Delta::Thinking(t) => (None, Some(t)),
                    };
                    shown.0.push_str(content.as_deref().unwrap_or_default());
                    shown.1.push_str(thinking.as_deref().unwrap_or_default());
                    self.emit("chat:delta", json!({ "chat_id": self.chat_id, "message_id": message_id, "content": content, "thinking": thinking }));
                })
                .await;
                if result.as_ref().is_ok_and(|r| !r.cancelled) {
                    tell_read();
                }
                if let Some((_, f)) = &forced {
                    // Its call, or (if the engine or model let us down) the first reply.
                    let first = first.take().expect("the first reply is kept for a forced try");
                    // A follow-up (nothing said before it) that didn't come back as
                    // a call isn't tried again this turn; words it wrote are its reply.
                    let follow_up_try = first.content.trim().is_empty() && first.tool_calls.is_empty();
                    break match result {
                        Ok(mut r) => match chat::forced_call(&r.content, &f.names) {
                            Some(mut call) => {
                                call.id = format!("call_{}", uuid::Uuid::new_v4().simple());
                                r.tool_calls = vec![call];
                                // The reply set aside (the user watched it written) stays, with the call.
                                r.content = first.content;
                                r.thinking = chat::joined(&first.thinking, &r.thinking);
                                r
                            }
                            None if follow_up_try => {
                                follow_ups_failed = true;
                                // A call cut off half-way is JSON, not words for the user.
                                if r.content.trim_start().starts_with('{') {
                                    r.content.clear();
                                }
                                r
                            }
                            None => first,
                        },
                        Err(_) => {
                            follow_ups_failed |= follow_up_try;
                            first
                        }
                    };
                }
                let finished = match result {
                    Ok(f) => f,
                    // All thinking, cut off before it did anything (asked
                    // again too): in the middle of work it's told to take the
                    // next step, and what it thought stays on screen.
                    Err(e) if e == chat::THOUGHT_ONLY && tools.is_some() && midway < 4 && step < limit - 1 && !self.cancel.load(Ordering::Relaxed) => {
                        midway += 1;
                        carry_on = Some(chat::THINK_LESS);
                        self.emit("chat:delta", json!({ "chat_id": self.chat_id, "message_id": message_id, "content": null, "thinking": (!shown.1.trim().is_empty()).then_some("\n\n") }));
                        aside = (chat::joined(&aside.0, &shown.0), chat::joined(&aside.1, &shown.1));
                        continue;
                    }
                    // A call the engine couldn't read (gpt-oss writing a long
                    // file left a quote unescaped, and llama-server answered
                    // 500): told to send it again, valid, split if it's long.
                    Err(e) if e.contains("Failed to parse tool call arguments") && tools.is_some() && midway < 4 && step < limit - 1 && !self.cancel.load(Ordering::Relaxed) => {
                        midway += 1;
                        carry_on = Some(chat::RESEND_CALL);
                        aside = (chat::joined(&aside.0, &shown.0), chat::joined(&aside.1, &shown.1));
                        continue;
                    }
                    // What it wrote before the error stays in the chat.
                    Err(e) => {
                        let (content, thinking) = (chat::joined(&aside.0, &shown.0), chat::joined(&aside.1, &shown.1));
                        if !content.trim().is_empty() || !thinking.trim().is_empty() {
                            self.save(&Message {
                                id: message_id.clone(),
                                chat_id: self.chat_id.clone(),
                                role: "assistant".into(),
                                content,
                                thinking: (!thinking.trim().is_empty()).then_some(thinking),
                                created_at: now(),
                                ..Default::default()
                            })?;
                        }
                        return Err(e);
                    }
                };
                // A model that asks the user for details about their own mail
                // or calendar, or says it can't access them, instead of looking
                // gets one more try that can only be a lookup. In these tries the
                // reply stays (on screen, and in the message with the call).
                let asked = history.iter().rev().find(|m| m.role == "user").map(|m| m.content.as_str()).unwrap_or("");
                let recent: Vec<&str> = history.iter().rev().filter(|m| m.role == "user").take(3).map(|m| m.content.as_str()).collect();
                let offered: Vec<String> = tools.as_ref().and_then(Value::as_array).into_iter().flatten().filter_map(|t| t.pointer("/function/name").and_then(Value::as_str).map(str::to_string)).collect();
                // Asked to make something and pasted it (or steps) instead: one
                // more try that can only make it on the PC.
                if step == 0 && !edits_text && tools.is_some() && self.ep.cloud.is_none() && self.mode != tools::Mode::Plan && finished.tool_calls.is_empty() && !finished.cancelled && chat::builds_in_chat(&recent, &finished.content, &offered) {
                    if let Some(f) = tools.as_ref().and_then(chat::forced_build) {
                        let mut ep = self.ep.clone();
                        ep.extra["response_format"] = f.response_format.clone();
                        ep.extra["max_tokens"] = json!((self.ep.ctx / 2).clamp(1024, 8192));
                        forced = Some((ep, f));
                        first = Some(finished);
                        continue;
                    }
                }
                // Asked for a picture and answered without making one ("I can't
                // generate images"): one more try that can only make it.
                // A change asked for with a picture attached goes to edit_image
                // (also when it says it can't change pictures).
                let has_pictures = history.iter().rev().find(|m| m.role == "user").and_then(|m| m.meta.as_ref()?.get("images")?.as_array().map(|a| !a.is_empty())).unwrap_or(false);
                let change = has_pictures && chat::asks_picture_change(asked);
                if step == 0 && !edits_text && tools.is_some() && self.ep.cloud.is_none() && finished.tool_calls.is_empty() && !finished.cancelled && (chat::asks_for_picture(asked) || (change && chat::denies_pictures(&finished.content))) {
                    if let Some(f) = tools.as_ref().and_then(|t| chat::forced_picture(t, change)) {
                        let mut ep = self.ep.clone();
                        ep.extra["response_format"] = f.response_format.clone();
                        ep.extra["max_tokens"] = json!(600);
                        forced = Some((ep, f));
                        first = Some(finished);
                        continue;
                    }
                }
                // A sum answered from its head: one more try that works it out
                // with calculate.
                if step == 0 && tools.is_some() && self.ep.cloud.is_none() && finished.tool_calls.is_empty() && !finished.cancelled && chat::asks_math(asked) {
                    if let Some(f) = tools.as_ref().and_then(chat::forced_calc) {
                        let mut ep = self.ep.clone();
                        ep.extra["response_format"] = f.response_format.clone();
                        ep.extra["max_tokens"] = json!(300);
                        forced = Some((ep, f));
                        first = Some(finished);
                        continue;
                    }
                }
                // Doesn't know what (or who) the user means: it looks it up on
                // the web, or asks to when the web is off.
                if step == 0 && tools.is_some() && self.ep.cloud.is_none() && finished.tool_calls.is_empty() && !finished.cancelled && chat::doesnt_know(&finished.content) {
                    if let Some(f) = tools.as_ref().and_then(chat::forced_web) {
                        let mut ep = self.ep.clone();
                        ep.extra["response_format"] = f.response_format.clone();
                        ep.extra["max_tokens"] = json!(400);
                        forced = Some((ep, f));
                        first = Some(finished);
                        continue;
                    }
                }
                // Asked to do something, and at any step it explained the steps
                // or asked "should I?" instead of doing them (often right after
                // fetching a page of instructions): the reply stays and it's
                // told to carry them out, once a turn, unless it already changed
                // or ran something this turn (then the reply is its summary).
                let acted = kept.iter().rev().take_while(|m| m.role != "user").any(|m| m.tool_calls.iter().flatten().any(|c| tools::find(&c.name).is_some_and(|t| matches!(t.risk, Risk::Write | Risk::Execute))));
                if !told_to_act && !acted && !edits_text && tools.is_some() && self.mode != tools::Mode::Plan && forced.is_none() && finished.tool_calls.is_empty() && !finished.cancelled && chat::asks_to_act(asked) && chat::explains_instead(&finished.content) {
                    told_to_act = true;
                    carry_on = Some(chat::CARRY_ON);
                    let gap = |t: &str| (!t.trim().is_empty()).then_some("\n\n");
                    self.emit("chat:delta", json!({ "chat_id": self.chat_id, "message_id": message_id, "content": gap(&finished.content), "thinking": gap(&finished.thinking) }));
                    aside = (chat::joined(&aside.0, &finished.content), chat::joined(&aside.1, &finished.thinking));
                    continue;
                }
                if step == 0 && !edits_text && tools.is_some() && self.ep.cloud.is_none() && finished.tool_calls.is_empty() && !finished.cancelled && chat::stalls(asked, &finished.content, &offered) {
                    let looks_only = |n: &str| n != "open_page" && n != "preview_site" && tools::find(n).is_some_and(|t| t.risk == Risk::Read);
                    if let Some(f) = tools.as_ref().and_then(|t| chat::forced_lookup(t, asked, looks_only)) {
                        let mut ep = self.ep.clone();
                        ep.extra["response_format"] = f.response_format.clone();
                        ep.extra["max_tokens"] = json!(400);
                        forced = Some((ep, f));
                        first = Some(finished);
                        continue;
                    }
                }
                break finished;
            };
            let mut finished = finished;
            finished.content = chat::joined(&aside.0, &finished.content);
            finished.thinking = chat::joined(&aside.1, &finished.thinking);

            let calls = finished.tool_calls.clone();
            // "I can't generate images" just after making one: what's true
            // follows it (what it wrote stays).
            let content = if calls.is_empty() && made_media && chat::denies_pictures(&finished.content) {
                chat::joined(&finished.content, "It's ready: it's shown above, with a button to save it.")
            } else {
                finished.content
            };
            let message = Message {
                id: message_id,
                chat_id: self.chat_id.clone(),
                role: "assistant".into(),
                content,
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
                // It stopped, but a page it wrote uses files it didn't write
                // (small models write index.html and leave out styles.css):
                // one more step writes each, up to a few.
                if follow_ups < 12 && !follow_ups_failed && self.ep.cloud.is_none() && step < limit - 1 {
                    let offers = |name: &str| tools.as_ref().and_then(Value::as_array).is_some_and(|t| t.iter().any(|d| d.pointer("/function/name").and_then(Value::as_str) == Some(name)));
                    let can_write = offers("write_file");
                    let next = chat::missing_pieces(&written)
                        .into_iter()
                        .next()
                        .and_then(|(page, missing)| tools.as_ref().and_then(|t| chat::forced_write(t, &page, &missing)))
                        // Asked to make something, a folder to make it in, and
                        // it stopped without writing anything ("Great, let's
                        // proceed"): the next step writes the first file.
                        .or_else(|| (build_ask && can_write && written.is_empty()).then(|| tools.as_ref().and_then(chat::forced_build)).flatten())
                        // A stylesheet and script but no page to load them.
                        .or_else(|| {
                            let (dir, links) = chat::page_missing(&written)?;
                            tools.as_ref().and_then(|t| chat::forced_page(t, &dir.join("index.html"), &links))
                        })
                        // It ended by pasting a fixed file into its reply: save it (then preview, below).
                        .or_else(|| {
                            let said = result.last.as_ref().map(|m| m.content.as_str()).unwrap_or_default();
                            if !chat::pasted_code(said) || problems_left.is_none() {
                                return None;
                            }
                            let file = chat::named_file(said, &written)?;
                            tools.as_ref().and_then(|t| chat::forced_save(t, &PathBuf::from(sandbox.display(&file))))
                        })
                        // It wrote a site and stopped: open it beside the chat and
                        // check it, then (up to twice) make it fix what the check found.
                        .or_else(|| {
                            let folder = chat::site_folder(&written)?;
                            if !offers("preview_site") || previews >= 6 {
                                return None;
                            }
                            if !previewed {
                                return tools.as_ref().and_then(|t| chat::forced_preview(t, &sandbox.display(&folder)));
                            }
                            let problems = problems_left.as_ref()?;
                            if fix_rounds >= 3 {
                                return None;
                            }
                            fix_rounds += 1;
                            tools.as_ref().and_then(|t| chat::forced_fix(t, problems))
                        });
                    if let Some(f) = next {
                        follow_ups += 1;
                        follow_up = Some(f);
                        self.emit("agent:step", json!({ "chat_id": self.chat_id }));
                        continue;
                    }
                }
                // It stopped partway: the last command it ran (asked to get
                // something done) failed, or its last words say what it does
                // next ("Let's update the uses.") and it didn't do it.
                let said = result.last.as_ref().map(|m| m.content.as_str()).unwrap_or_default();
                // A site it built that checks out: once, it goes through what
                // was asked, item by item, against the files (a 20B model's
                // budget app had Edit and Delete buttons that did nothing).
                // Other work that took a while (files changed or commands run
                // over several steps) the same way: the 20B model added a
                // command without the test it was asked for.
                let built = build_ask && !written.is_empty() && previewed && problems_left.is_none();
                let worked = progressed_at.is_some() && step >= 4 && chat::asks_to_act(&request);
                // Unfinished lines it wrote that are still there.
                let unfinished = stubs.iter().any(|(p, l)| std::fs::read_to_string(p).is_ok_and(|t| t.lines().any(|x| x.trim() == l)));
                // Asked to change something in its folders, and nothing's
                // changed: gpt-oss 20B wrote out a plan for a new command and
                // stopped there.
                let unchanged = !sandbox.is_empty() && !did_something && chat::asks_to_change(&request) && !said.trim_end().ends_with('?');
                let short = if said.trim().is_empty() && step > 0 && !result.cancelled {
                    // Nothing at all after its lookups (an empty reply).
                    Some(chat::ANSWER_NOW)
                } else if declined {
                    // Told why it stopped: pushing it on repeated the same
                    // answer four times after a refused write.
                    None
                } else if (failing_run || broken_file) && chat::asks_to_act(&request) {
                    Some(if broken_file { chat::FIX_BROKEN } else { chat::FIX_FAILING })
                } else if unchanged {
                    Some(chat::CHANGE_NOW)
                } else if unfinished && chat::asks_to_act(&request) {
                    Some(chat::FINISH_STUBS)
                } else if (built || worked) && !reviewed_asks {
                    reviewed_asks = true;
                    Some(if built { chat::CHECK_ASKED } else { chat::CHECK_WORK })
                } else if chat::stops_midway(said) {
                    Some(chat::GO_ON)
                } else {
                    None
                };
                if let Some(note) = short.filter(|_| midway < 4 && tools.is_some() && self.mode != tools::Mode::Plan && step < limit - 1) {
                    midway += 1;
                    go_on = Some(note);
                    self.emit("agent:step", json!({ "chat_id": self.chat_id }));
                    continue;
                }
                // Something was added while it wrote this: it answers that too.
                if crate::inbox::waiting(&self.chat_id) && step < limit - 1 {
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
            // Lookups run ahead together, by index (see below).
            let mut ready: HashMap<usize, Outcome> = HashMap::new();
            for (i, call) in calls.iter().enumerate() {
                // A run of lookups starting here: run them together now; the
                // body below takes each result in turn, as if it had run it.
                if !ready.contains_key(&i) && !self.cancel.load(Ordering::Relaxed) {
                    let mut seen: Vec<String> = Vec::new();
                    let ok: Vec<bool> = calls
                        .iter()
                        .enumerate()
                        .map(|(j, c)| {
                            let key = call_key(c);
                            let fine = j >= i
                                && tools::parallel_safe(&c.name)
                                && !extras.mcp.contains_key(&c.name)
                                && !done_calls.contains(&key)
                                && !failed_calls.iter().any(|(k, _)| *k == key)
                                && !seen.contains(&key)
                                && !self.might_ask(c, &args_of(c), &sandbox, project_id.as_deref(), &extras);
                            if fine {
                                seen.push(key);
                            }
                            fine
                        })
                        .collect();
                    let end = if ok[i] { run_end(&ok, i) } else { i };
                    if end - i >= 2 {
                        let started = std::time::Instant::now();
                        // Four at a time; `join_all` keeps the order they came in.
                        let mut outcomes: Vec<Outcome> = Vec::with_capacity(end - i);
                        for group in calls[i..end].chunks(4) {
                            let running: Vec<_> = group.iter().map(|c| self.execute(c, &sandbox, &recorder, project_id.as_deref(), &extras)).collect();
                            outcomes.extend(futures_util::future::join_all(running).await);
                        }
                        crate::applog::line("agent", &format!("Ran {} lookups together in {} ms", end - i, started.elapsed().as_millis()));
                        ready.extend((i..end).zip(outcomes));
                    }
                }
                let key = call_key(call);
                // What a page or a running command shows changes between
                // identical calls: those are never repeats.
                let changes_by_itself = call.name.starts_with("browser_") || matches!(call.name.as_str(), "command_output" | "wait");
                let repeat = !changes_by_itself && done_calls.contains(&key) && tools::find(&call.name).is_some_and(|t| t.risk == Risk::Read);
                // A lookup that failed once (often the network) gets one more try.
                let network = is_network_tool(&call.name);
                let fails = failed_calls.iter().filter(|(k, _)| *k == key).count();
                let failed_before = if network && fails < 2 { None } else { failed_calls.iter().find(|(k, _)| *k == key).map(|(_, why)| why.clone()) };
                let mut outcome = if self.cancel.load(Ordering::Relaxed) {
                    Outcome::denied(call.name.clone()).with_text("Stopped by the user before this ran.")
                } else if let Some(why) = failed_before {
                    failed_repeats += 1;
                    Outcome::error(
                        tools::failed_title(&call.name, &args_of(call)),
                        format!("This exact call already failed: {why} Don't send it again unchanged: fix what the error says, or use another tool."),
                    )
                } else if made_media && matches!(call.name.as_str(), "create_image" | "create_video" | "create_music") && !chat::wants_several(&request) {
                    // Small models went on making the same picture again and again.
                    Outcome::ok(
                        "You already made it in this reply, and it's shown in the chat with a button to save it. Don't make another: tell the user in a sentence what you made.",
                        "Didn't make it again",
                        "text",
                        None,
                    )
                } else if repeat && call.name == "preview_site" && result.last.as_ref().is_some_and(|m| chat::pasted_code(&m.content)) {
                    // It wrote the fix into its reply, not the file, and previews the
                    // unchanged site again (the live test's 14B): the next step saves it.
                    let file = result.last.as_ref().and_then(|m| chat::named_file(&m.content, &written));
                    if follow_ups < 12 {
                        if let Some(f) = file.as_deref().and_then(|p| tools.as_ref().and_then(|t| chat::forced_save(t, &PathBuf::from(sandbox.display(p))))) {
                            follow_ups += 1;
                            follow_up = Some(f);
                        }
                    }
                    Outcome::ok(
                        "The site hasn't changed since the last preview: the code in your reply was never saved to the file. Save it with write_file (the whole file), then preview again.",
                        format!("Skipped a repeat of {}", call.name),
                        "text",
                        None,
                    )
                } else if repeat {
                    repeats += 1;
                    // It worked the answer out while thinking, asked again for
                    // what it already had, and then answered with none of it:
                    // its thinking comes back with the reminder, to answer from.
                    let thought = result.last.as_ref().and_then(|m| m.thinking.as_deref()).filter(|t| t.trim().len() > 200);
                    let from_thought = thought.map(|t| {
                        format!(
                            " Your thinking just before this call is below: answer from it, and put what you found in your answer (names, links, numbers, steps).\n\n{}",
                            chat::thought_so_far(t, self.ep.ctx as usize)
                        )
                    });
                    Outcome::ok(
                        format!(
                            "You already ran {} with these arguments; its result is above. Answer the user with it now, or try different arguments.{}",
                            call.name,
                            from_thought.unwrap_or_default()
                        ),
                        format!("Skipped a repeat of {}", call.name),
                        "text",
                        None,
                    )
                } else {
                    done_calls.push(key.clone());
                    let o = match ready.remove(&i) {
                        Some(o) => o,
                        None => self.execute(call, &sandbox, &recorder, project_id.as_deref(), &extras).await,
                    };
                    if call.name == "write_file" && o.meta["status"] != "error" {
                        if let Some(f) = o.meta["file"].as_str() {
                            written.push(std::path::PathBuf::from(f));
                        }
                    }
                    if o.meta["status"] == "error" {
                        crate::applog::line("agent", &format!("The {} tool failed in a chat (model {})", call.name, self.ep.cloud.as_ref().map_or("on this PC", |_| "in the cloud")));
                        let why = o.meta["detail"].as_str().map(str::to_string).unwrap_or_else(|| o.text.clone());
                        failed_calls.push((key, why));
                    } else if tools::find(&call.name).is_some_and(|t| matches!(t.risk, Risk::Write | Risk::Execute | Risk::Memory)) || changes_by_itself {
                        // Something changed (a file written, a command run): a call
                        // that failed before (previewing a page that wasn't there
                        // yet) may work now.
                        failed_calls.clear();
                        failed_repeats = 0;
                        if tools::find(&call.name).is_some_and(|t| matches!(t.risk, Risk::Write | Risk::Execute)) {
                            progressed_at = Some(step);
                        }
                        // Reading a file or previewing the site again shows the change.
                        done_calls.retain(|k| !tools::find(k.split('{').next().unwrap_or_default()).is_some_and(|t| t.risk == Risk::Read));
                    }
                    o
                };
                // The last command failed: not done until one runs right. A
                // file it left that doesn't parse counts the same, until a
                // change to it parses.
                if call.name == "run_command" {
                    failing_run = outcome.meta["status"] == "error";
                }
                // Something done (not only looked at) this turn.
                if outcome.meta["status"] == "ok" && tools::find(&call.name).is_some_and(|t| t.risk != Risk::Read) {
                    did_something = true;
                }
                if matches!(call.name.as_str(), "write_file" | "edit_file") && outcome.meta["status"] != "error" {
                    broken_file = outcome.meta["broken"] == true;
                    // Unfinished lines it added ("TODO"), looked for again when it stops.
                    if let (Some(path), Some(lines)) = (outcome.meta["unfinished"]["path"].as_str(), outcome.meta["unfinished"]["lines"].as_array()) {
                        stubs.extend(lines.iter().filter_map(Value::as_str).map(|l| (std::path::PathBuf::from(path), l.to_string())));
                    }
                }
                // Declined: nothing more is pushed on them this turn (a declined
                // file write came back up to 12 times).
                if outcome.meta["status"] == "denied" {
                    follow_ups_failed = true;
                    declined = true;
                    follow_up = None;
                }
                // A change to the site since it was last previewed; what a preview found.
                if matches!(call.name.as_str(), "write_file" | "edit_file") && outcome.meta["status"] != "error" {
                    previewed = false;
                }
                // Questions asked before a picture: nothing was made yet.
                if matches!(call.name.as_str(), "create_image" | "edit_image" | "create_video" | "create_music") && outcome.meta["status"] == "ok" && outcome.meta["questions"].is_null() {
                    made_media = true;
                }
                // Asked to preview before writing anything (the live test's 14B
                // did, three times): the next step writes the first file.
                if call.name == "preview_site" && outcome.meta["status"] != "ok" && written.is_empty() && build_ask && follow_ups < 12 {
                    if let Some(f) = tools.as_ref().and_then(chat::forced_build) {
                        follow_ups += 1;
                        follow_up = Some(f);
                    }
                }
                if call.name == "preview_site" && (outcome.meta["preview_site"].is_string() || outcome.meta["preview_port"].is_u64()) {
                    previewed = true;
                    previews += 1;
                    let found = (outcome.meta["preview_problems"].as_u64().unwrap_or(0) > 0)
                        .then(|| outcome.meta["detail"].as_str().unwrap_or_default().lines().map(|l| format!("- {l}")).collect::<Vec<_>>().join("\n"));
                    // The same problems as last time: patching isn't working.
                    if found.is_some() && found == problems_left {
                        outcome.text.push_str(
                            "\n\nThese are the same problems as the last preview: the change didn't fix them. Read the whole file with read_file, \
                             then send it back fixed and complete with write_file (define everything before it's used), instead of another small edit.",
                        );
                    }
                    problems_left = found;
                }
                // The user let it use the web: the web tools, from the next step on.
                if outcome.meta["web_granted"] == true {
                    if let Some(Value::Array(defs)) = tools.as_mut() {
                        defs.retain(|d| d.pointer("/function/name").and_then(Value::as_str) != Some("request_web"));
                        for name in ["weather", "web_search", "fetch_page", "wikipedia", "research", "market_quote", "find_places"] {
                            if let Some(d) = tools::find(name) {
                                defs.push(json!({ "type": "function", "function": { "name": d.name, "description": d.description, "parameters": (d.params)() } }));
                            }
                        }
                    }
                    if self.add_live_data(&sandbox, &mut about, &mut tools, build_ask || !small) {
                        system = system_of(&about);
                    }
                }
                // The user shared a folder: the file tools work there from the next step on.
                if outcome.meta["folder_granted"] == true {
                    let (folders, general) = {
                        let conn = self.state.db.lock().unwrap();
                        db::chat(&conn, &self.cipher, &self.chat_id).map(|c| (projects::chat_folders(&conn, &c), projects::general_folders(&conn, &c))).unwrap_or_default()
                    };
                    let before = tool_instructions(&sandbox, self.mode);
                    sandbox = Sandbox::new(&folders).with_home().with_general(&general);
                    if self.ep.cloud.is_some() {
                        sandbox = sandbox.for_online();
                    }
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
                    if self.add_live_data(&sandbox, &mut about, &mut tools, build_ask || !small) {
                        system = system_of(&about);
                    }
                }
                let outcome = fit_result(outcome, &call.name, self.ep.ctx);
                self.save_result(call, outcome)?;
                self.emit("agent:step", json!({ "chat_id": self.chat_id }));
            }
            // The same failing call again and again: once, it's told to step
            // back and do it another way (gpt-oss 20B sent one edit 30 times
            // over); the next time, stop and say what went wrong.
            if failed_repeats >= 2 && !stepped_back && step < limit - 1 {
                stepped_back = true;
                failed_repeats = 0;
                go_on = Some(chat::STEP_BACK);
                continue;
            }
            if failed_repeats >= 2 {
                let why = failed_calls.last().map(|(_, w)| w.clone()).unwrap_or_default();
                let online = failed_calls.last().is_some_and(|(k, _)| is_network_tool(k.split('{').next().unwrap_or_default()));
                let note = Message {
                    id: uuid::Uuid::new_v4().to_string(),
                    chat_id: self.chat_id.clone(),
                    role: "assistant".into(),
                    content: if online {
                        format!("I couldn't get that from the internet, even after trying again, so I stopped. The error was: {why}\n\nCheck that this PC is online (and that the site is up), then ask again.")
                    } else {
                        format!(
                            "I kept trying the same thing and it kept failing, so I stopped. The error was: {why}\n\nA larger model usually gets this right: pick one from the model menu (one with 7B or more), then ask again."
                        )
                    },
                    created_at: now(),
                    ..Default::default()
                };
                self.save(&note)?;
                result.last = Some(note);
                break;
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
            if step == limit - 1 && limit < most_steps && progressed_at.is_some_and(|at| at + 5 >= step) {
                limit = (limit + 10).min(most_steps);
                crate::applog::line("agent", &format!("Still making progress after {} steps: going on, up to {limit}", step + 1));
            } else if step == limit - 1 {
                let note = Message {
                    id: uuid::Uuid::new_v4().to_string(),
                    chat_id: self.chat_id.clone(),
                    role: "assistant".into(),
                    content: format!("I've taken {limit} steps on this, so I'm pausing here. Say \"continue\" if you'd like me to keep going."),
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

    /// Agent actions go in the activity log, encrypted (they name files).
    fn log(&self, summary: &str) {
        let conn = self.state.db.lock().unwrap();
        // An incognito chat leaves no lines in Activity.
        if !db::chat_incognito(&conn, &self.chat_id) {
            db::log_action_enc(&conn, &self.cipher, "agent", summary);
        }
    }
}

/// A call's arguments as JSON (empty when they don't parse).
fn args_of(call: &ToolCall) -> Value {
    serde_json::from_str(&call.arguments).unwrap_or_default()
}

/// Tools that come from outside the built-in set, for one turn.
#[derive(Default)]
struct Extras {
    mcp: std::collections::HashMap<String, OfferedTool>,
    skills: Vec<Skill>,
    problems: Vec<String>,
    allowances: Vec<allowances::Allowance>,
}

impl Outcome {
    fn with_text(mut self, text: &str) -> Outcome {
        self.text = text.to_string();
        self
    }
}

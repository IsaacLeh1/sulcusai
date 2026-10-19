// SPDX-License-Identifier: AGPL-3.0-only
//! Questions before a picture is made. A model reads the request (and, for a
//! change, looks at the picture) and asks the person only what would really
//! change the result: which of two women "her" is, what a sign should say,
//! what "better" means, what "my dog" looks like. Most requests are clear and
//! get none. The Studio shows the questions above the box; a chat's assistant
//! asks them and calls the tool again with the answers worked in.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use rusqlite::Connection;
use serde_json::{json, Value};

use super::{prompts, MediaModel};
use crate::db::Message;
use crate::vision::Helper;
use crate::{db, AppState, AppStateRef};

/// At most this many questions, each under `MAX_WORDS` words.
const MAX_QUESTIONS: usize = 3;
const MAX_WORDS: usize = 25;

/// The stored setting: chats ask before a picture when its request leaves
/// something open (the Studio's "Ask me when something's unclear" sets it).
const ASK_KEY: &str = "ask_picture_questions";

/// Whether chats ask picture questions (on unless turned off).
pub fn enabled(conn: &Connection) -> bool {
    db::get(conn, ASK_KEY).unwrap_or(true)
}

/// The jobs questions are asked for: new pictures and changes from words.
fn asks_about(op: &str) -> bool {
    matches!(op, "generate" | "edit")
}

const NEW_PICTURE: &str = "You read a request for a new picture before it's made and decide whether it can be done well as it stands. \
     Ask only when the answer would really change the picture and no sensible choice covers it: the words for a sign, poster, card, label \
     or cover when none are given; a particular person, pet or thing of theirs (\"my dog\", \"me\", \"our house\") with no picture of it \
     and no description of how it looks; a request too vague to start from (\"something cool\"). Most requests are clear: then the list \
     is empty. Never ask about style, colors, light, framing, the background or other details a sensible choice covers, and never about \
     anything the request already says. At most 3 questions, each short and plain, about one thing, asked of the person. \
     Reply with only JSON: {\"questions\": [...]}.";

const CHANGE: &str = "You look at someone's picture and the change they asked for, before it's made, and decide whether it can be done \
     well as it stands. Ask only when the answer would really change the result and no sensible choice covers it: which one they mean \
     when the picture has several that fit (\"change her shirt\" with two women in it); the words to write when they ask for text \
     without giving it; what \"better\", \"nicer\" or \"fix it\" should mean; a particular person, pet or thing of theirs to add \
     (\"put my dog in it\") with no picture or description of it. Most changes are clear: then the list is empty. Never ask about style, \
     colors, light or other details a sensible choice covers, and never about anything the request already says. At most 3 questions, \
     each short and plain, about one thing, asked of the person. Reply with only JSON: {\"questions\": [...]}.";

const UNSEEN: &str = " You can't see their picture: ask only about what the words leave open, never about what's in the picture.";

/// Worked examples, most of them clear (as most requests are).
const NEW_DEMOS: &[(&str, &[&str])] = &[
    ("a golden retriever puppy playing in autumn leaves", &[]),
    ("a poster for our bake sale", &["What should the poster say, such as the name, date and place?"]),
    ("a cozy reading nook by a big window on a rainy day", &[]),
    ("my dog wearing a space suit", &["What does your dog look like: the breed, colors and size?"]),
];
const CHANGE_DEMOS: &[(&str, &[&str])] = &[("make it night", &[]), ("make it better", &["Better in what way: brighter colors, sharper, a different background, or something else?"])];

fn ask_text(op: &str, prompt: &str) -> String {
    if op == "edit" {
        format!("The change they asked for: {}", prompt.trim())
    } else {
        format!("The picture they asked for: {}", prompt.trim())
    }
}

fn schema() -> Value {
    json!({ "type": "object", "properties": { "questions": { "type": "array", "items": { "type": "string" }, "maxItems": MAX_QUESTIONS } }, "required": ["questions"] })
}

/// The messages that ask: instructions, the worked examples, then the request
/// (with the picture when the model sees it).
fn messages(op: &str, prompt: &str, picture: Option<&image::DynamicImage>, sees: bool) -> Result<Vec<Value>, String> {
    let system = match op {
        "edit" if sees && picture.is_some() => CHANGE.to_string(),
        "edit" => format!("{CHANGE}{UNSEEN}"),
        _ => NEW_PICTURE.to_string(),
    };
    let mut out = vec![json!({ "role": "system", "content": system })];
    let demos = if op == "edit" { CHANGE_DEMOS } else { NEW_DEMOS };
    for (ask, questions) in demos {
        out.push(json!({ "role": "user", "content": ask_text(op, ask) }));
        out.push(json!({ "role": "assistant", "content": json!({ "questions": questions }).to_string() }));
    }
    let text = ask_text(op, prompt);
    out.push(match picture.filter(|_| sees) {
        Some(img) => json!({ "role": "user", "content": [
            { "type": "text", "text": text },
            { "type": "image_url", "image_url": { "url": crate::vision::data_url(img)? } },
        ] }),
        None => json!({ "role": "user", "content": text }),
    });
    Ok(out)
}

/// The model that reads the words only: the picture model's prompt writer,
/// or the chat model when there's no picture model on this PC.
async fn reads_words(state: &Arc<AppState>, m: Option<&MediaModel>) -> Result<Helper, String> {
    match m {
        Some(m) => prompts::writer(state, m).await,
        None => Helper::chat_model(state, false).await,
    }
}

/// The questions worth asking before this picture is made or this change is
/// done; empty when the request is clear (the usual case), when nothing here
/// can answer, or while another job has the graphics card.
pub async fn questions(state: &Arc<AppState>, op: &str, prompt: &str, source: Option<&image::DynamicImage>, m: Option<&MediaModel>) -> Vec<String> {
    let prompt = prompt.trim();
    if !asks_about(op) || prompt.is_empty() {
        return Vec::new();
    }
    // Another model on a card busy making something could break that job.
    if super::any_job() {
        crate::applog::line("media", "Didn't look for questions about the request: another job is using the graphics card");
        return Vec::new();
    }
    let picture = source.filter(|_| op == "edit");
    let helper = match picture {
        Some(_) => match prompts::reader(state, m).await {
            Ok(h) => Ok(h),
            Err(_) => reads_words(state, m).await,
        },
        None => reads_words(state, m).await,
    };
    let h = match helper {
        Ok(h) => h,
        Err(e) => {
            crate::applog::line("media", &format!("Couldn't look for questions about the request: {e}"));
            return Vec::new();
        }
    };
    let said = match messages(op, prompt, picture, h.sees) {
        Ok(msgs) => h.ask_json(msgs, schema(), 300, 0.0).await,
        Err(e) => Err(e),
    };
    let name = h.name.clone();
    h.done(state).await;
    match said {
        Ok(v) => {
            let q = keep_questions(&v);
            if !q.is_empty() {
                crate::applog::line("media", &format!("{name} had {} question(s) before the {}", q.len(), if op == "edit" { "change" } else { "picture" }));
            }
            q
        }
        Err(e) => {
            crate::applog::line("media", &format!("Couldn't look for questions about the request: {e}"));
            Vec::new()
        }
    }
}

/// A list marker a model put before a question ("1.", "2)", "-", "•").
fn without_marker(s: &str) -> &str {
    let s = s.trim_start();
    for m in ["-", "*", "•"] {
        if let Some(rest) = s.strip_prefix(m) {
            return rest.trim_start();
        }
    }
    let digits = s.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 {
        let rest = &s[digits..];
        if let Some(rest) = rest.strip_prefix('.').or_else(|| rest.strip_prefix(')')) {
            if rest.starts_with(' ') {
                return rest.trim_start();
            }
        }
    }
    s
}

/// The questions in a model's answer worth showing: each trimmed onto one
/// line, a real question (words after its last question mark dropped), under
/// 25 words, not a repeat; at most 3.
fn keep_questions(v: &Value) -> Vec<String> {
    let list = v.get("questions").unwrap_or(v).as_array().cloned().unwrap_or_default();
    let mut out: Vec<String> = Vec::new();
    for q in list.iter().filter_map(Value::as_str) {
        let q = without_marker(q).split_whitespace().collect::<Vec<_>>().join(" ");
        let q = q.trim_matches(|c: char| c == '"' || c == '\'' || c.is_whitespace());
        let Some(end) = q.rfind('?') else { continue };
        let q = q[..=end].trim();
        if q.len() < 3 || q.split_whitespace().count() >= MAX_WORDS || out.iter().any(|o| o.eq_ignore_ascii_case(q)) {
            continue;
        }
        out.push(q.to_string());
        if out.len() == MAX_QUESTIONS {
            break;
        }
    }
    out
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The request with the person's answers folded in: "…. Answers to questions
/// asked: Which woman? The one on the left." Unanswered questions are left out.
pub fn with_answers(prompt: &str, qa: &[(String, String)]) -> String {
    let prompt = prompt.trim();
    let answered: Vec<String> = qa
        .iter()
        .map(|(q, a)| (one_line(q), one_line(a)))
        .filter(|(q, a)| !q.is_empty() && !a.is_empty())
        .map(|(q, a)| if a.ends_with(['.', '!', '?']) { format!("{q} {a}") } else { format!("{q} {a}.") })
        .collect();
    if answered.is_empty() {
        return prompt.to_string();
    }
    let stop = if prompt.is_empty() || prompt.ends_with(['.', '!', '?']) { "" } else { "." };
    format!("{prompt}{stop} Answers to questions asked: {}", answered.join(" ")).trim_start().to_string()
}

/// The request as the questions see it, noting example pictures it came with
/// (so "my dog" isn't asked about when a picture of the dog is there).
pub fn noting_examples(prompt: &str, examples: usize) -> String {
    match examples {
        0 => prompt.trim().to_string(),
        1 => format!("{} (They gave an example picture for it to follow, which may show the person, pet or thing they mean.)", prompt.trim()),
        n => format!("{} (They gave {n} example pictures for it to follow, which may show the people, pets or things they mean.)", prompt.trim()),
    }
}

// ---------- chats: never twice in a row ----------

/// Questions a chat's tool asked, and the person's message they were asked after.
#[derive(Debug, Clone, PartialEq)]
pub struct Asked {
    pub after_user: String,
    pub questions: Vec<String>,
}

fn asked_in_chats() -> &'static Mutex<HashMap<String, Asked>> {
    static A: OnceLock<Mutex<HashMap<String, Asked>>> = OnceLock::new();
    A.get_or_init(Default::default)
}

pub fn last_asked(chat_id: &str) -> Option<Asked> {
    asked_in_chats().lock().unwrap().get(chat_id).cloned()
}

pub fn remember_asked(chat_id: &str, asked: Asked) {
    asked_in_chats().lock().unwrap().insert(chat_id.to_string(), asked);
}

pub fn forget_asked(chat_id: &str) {
    asked_in_chats().lock().unwrap().remove(chat_id);
}

/// Lowercase words only, for finding a question in what the assistant said.
fn plain(s: &str) -> String {
    let kept: String = s.chars().map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { ' ' }).collect();
    one_line(&kept)
}

/// Whether this chat just asked picture questions, so it doesn't ask again:
/// in this same turn (the assistant called the tool again instead of asking),
/// in the turn before (the person's newest message is the first since), or,
/// when nothing was kept (after a restart), when the assistant's last message
/// before the person's newest one asked one of these questions.
pub fn asked_already(history: &[Message], last: Option<&Asked>, questions: &[String]) -> bool {
    let Some(newest) = history.iter().rposition(|m| m.role == "user") else { return false };
    if let Some(a) = last {
        if history[newest].id == a.after_user {
            return true;
        }
        if let Some(i) = history.iter().position(|m| m.id == a.after_user) {
            if history[i + 1..].iter().filter(|m| m.role == "user").count() == 1 {
                return true;
            }
        }
    }
    let Some(said) = history[..newest].iter().rev().find(|m| m.role == "assistant" && !m.content.trim().is_empty()).map(|m| plain(&m.content)) else {
        return false;
    };
    let kept = last.map(|a| a.questions.as_slice()).unwrap_or_default();
    questions.iter().chain(kept).map(|q| plain(q)).any(|q| q.split(' ').count() >= 3 && said.contains(&q))
}

// ---------- commands ----------

/// The Studio's questions before a new picture or a change ("generate" or
/// "edit"): `source` is the gallery picture to change, `examples` how many
/// example pictures the new one follows.
#[tauri::command]
pub async fn media_questions(state: AppStateRef<'_>, op: String, prompt: String, source: Option<String>, model_id: Option<String>, examples: Option<u32>) -> Result<Vec<String>, String> {
    let c = state.cipher()?;
    let m = super::choose(&state, &op, model_id.as_deref(), source.is_some()).ok();
    let picture = source.and_then(|id| super::store::read(&state.paths, &c, &id).ok()).and_then(|b| super::imaging::decode(&b).ok());
    let prompt = noting_examples(&prompt, examples.unwrap_or(0) as usize);
    Ok(questions(state.inner(), &op, &prompt, picture.as_ref(), m.as_ref()).await)
}

/// The request with the answers to its questions folded in.
#[tauri::command]
pub fn media_with_answers(prompt: String, answers: Vec<(String, String)>) -> String {
    with_answers(&prompt, &answers)
}

/// Turns asking before a picture on or off for chats (the Studio's checkbox).
#[tauri::command(async)]
pub fn media_set_ask_questions(state: AppStateRef, on: bool) -> Result<(), String> {
    db::set(&state.db.lock().unwrap(), ASK_KEY, &on)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn qs(v: Value) -> Vec<String> {
        keep_questions(&v)
    }

    #[test]
    fn only_real_short_questions_are_kept() {
        assert_eq!(qs(json!({ "questions": [] })), Vec::<String>::new(), "a clear request asks nothing");
        assert_eq!(qs(json!({ "questions": ["  Which woman: the one on the left or the right?  "] })), vec!["Which woman: the one on the left or the right?"]);
        assert_eq!(
            qs(json!({ "questions": ["", "   ", "The picture is clear.", "What should the sign say?"] })),
            vec!["What should the sign say?"],
            "empty answers and statements are dropped"
        );
        let long = format!("{}?", vec!["word"; 30].join(" "));
        assert!(qs(json!({ "questions": [long] })).is_empty(), "25 words or more is too long");
        assert_eq!(qs(json!({ "questions": ["A?", "B or c?", "Which one?", "What color?", "How many?"] })).len(), 3, "at most 3");
        assert_eq!(qs(json!({ "questions": ["Which one?", "which one?"] })), vec!["Which one?"], "no repeats");
    }

    #[test]
    fn questions_are_tidied() {
        assert_eq!(qs(json!({ "questions": ["1. Which woman?", "2) What should it say?", "- Better how?", "• Which dog?"] })), vec!["Which woman?", "What should it say?", "Better how?"]);
        assert_eq!(qs(json!({ "questions": ["3 candles or 5?"] })), vec!["3 candles or 5?"], "a number that starts the question stays");
        assert_eq!(qs(json!({ "questions": ["What should the sign say? For example, a shop name."] })), vec!["What should the sign say?"]);
        assert_eq!(qs(json!({ "questions": ["\"Which\n  woman?\""] })), vec!["Which woman?"]);
        assert_eq!(qs(json!(["What should it say?"])), vec!["What should it say?"], "a bare list is read too");
        assert!(qs(json!({ "questions": [1, null, { "q": "x?" }] })).is_empty());
        assert!(qs(json!("Which one?")).is_empty());
    }

    #[test]
    fn answers_are_folded_into_the_request() {
        let qa = |pairs: &[(&str, &str)]| pairs.iter().map(|(q, a)| (q.to_string(), a.to_string())).collect::<Vec<_>>();
        assert_eq!(
            with_answers("change her shirt to red", &qa(&[("Which woman?", "The one on the left")])),
            "change her shirt to red. Answers to questions asked: Which woman? The one on the left."
        );
        assert_eq!(
            with_answers("A bake sale poster.", &qa(&[("What should it say?", "Bake Sale, Saturday 10am!"), ("Which colors?", "  "), ("Where is it?", "the school\ngym")])),
            "A bake sale poster. Answers to questions asked: What should it say? Bake Sale, Saturday 10am! Where is it? the school gym."
        );
        assert_eq!(with_answers("  make it better ", &qa(&[("Better how?", "")])), "make it better", "nothing answered: the request as it was");
        assert_eq!(with_answers("make it better", &[]), "make it better");
    }

    #[test]
    fn example_pictures_are_noted() {
        assert_eq!(noting_examples(" my dog in space ", 0), "my dog in space");
        assert!(noting_examples("my dog in space", 1).contains("an example picture"));
        assert!(noting_examples("my dog in space", 3).contains("3 example pictures"));
    }

    #[test]
    fn only_new_pictures_and_changes_are_asked_about() {
        assert!(asks_about("generate") && asks_about("edit"));
        for op in ["restyle", "upscale", "remove_background", "fill", "extend", "video", "music", "narrate", "tune"] {
            assert!(!asks_about(op), "{op}");
        }
    }

    #[test]
    fn the_request_and_picture_reach_the_model() {
        let img = image::DynamicImage::new_rgb8(16, 16);
        let msgs = messages("edit", "change her shirt", Some(&img), true).unwrap();
        assert_eq!(msgs.len(), 2 + 2 * CHANGE_DEMOS.len());
        assert!(!msgs[0]["content"].as_str().unwrap().contains("can't see"));
        let last = msgs.last().unwrap();
        assert_eq!(last["content"][0]["text"], "The change they asked for: change her shirt");
        assert!(last["content"][1]["image_url"]["url"].as_str().unwrap().starts_with("data:image/jpeg;base64,"));
        let blind = messages("edit", "change her shirt", Some(&img), false).unwrap();
        assert!(blind[0]["content"].as_str().unwrap().contains("can't see their picture"));
        assert_eq!(blind.last().unwrap()["content"], "The change they asked for: change her shirt");
        let new = messages("generate", "a sign for my shop", None, false).unwrap();
        assert_eq!(new.len(), 2 + 2 * NEW_DEMOS.len());
        assert_eq!(new.last().unwrap()["content"], "The picture they asked for: a sign for my shop");
        // The demos answer in the same JSON the schema asks for.
        for m in new.iter().filter(|m| m["role"] == "assistant") {
            let v: Value = serde_json::from_str(m["content"].as_str().unwrap()).unwrap();
            assert_eq!(keep_questions(&v).len(), v["questions"].as_array().unwrap().len());
        }
    }

    fn msg(id: &str, role: &str, content: &str) -> Message {
        Message { id: id.into(), role: role.into(), content: content.into(), ..Default::default() }
    }

    #[test]
    fn a_chat_never_asks_twice_in_a_row() {
        let q = vec!["What should the poster say?".to_string()];
        let asked = Asked { after_user: "u1".into(), questions: q.clone() };
        // The first time: nothing asked yet.
        let first = [msg("u1", "user", "make a poster for our bake sale")];
        assert!(!asked_already(&first, None, &q));
        // The assistant called the tool again in the same turn instead of asking.
        let same_turn = [msg("u1", "user", "make a poster for our bake sale"), msg("a1", "assistant", ""), msg("t1", "tool", "Not made yet")];
        assert!(asked_already(&same_turn, Some(&asked), &q));
        // The person answered: their first message since.
        let answered = [
            msg("u1", "user", "make a poster for our bake sale"),
            msg("a1", "assistant", "Sure. Quick question: what should it say?"),
            msg("u2", "user", "Bake Sale Saturday 10am"),
        ];
        assert!(asked_already(&answered, Some(&asked), &q));
        // A later request, two messages on: it may ask again.
        let later = [
            msg("u1", "user", "make a poster for our bake sale"),
            msg("a1", "assistant", "What should the poster say?"),
            msg("u2", "user", "never mind"),
            msg("a2", "assistant", "Okay."),
            msg("u3", "user", "make a sign for my shop"),
        ];
        assert!(!asked_already(&later, Some(&asked), &["What should the sign say?".to_string()]));
        // After a restart nothing is kept: the assistant's last message shows it asked.
        let restarted = [msg("u1", "user", "make a poster"), msg("a1", "assistant", "Before I make it: What should the poster say?"), msg("u2", "user", "Bake sale")];
        assert!(asked_already(&restarted, None, &q));
        let other = [msg("u1", "user", "make a poster"), msg("a1", "assistant", "Here's a cat."), msg("u2", "user", "now a poster")];
        assert!(!asked_already(&other, None, &q));
        assert!(!asked_already(&[], Some(&asked), &q), "no message from the person");
    }

    #[test]
    fn asked_questions_are_kept_per_chat() {
        let a = Asked { after_user: "u1".into(), questions: vec!["Which one?".into()] };
        remember_asked("clarify-test-chat", a.clone());
        assert_eq!(last_asked("clarify-test-chat"), Some(a));
        assert_eq!(last_asked("clarify-other-chat"), None);
        forget_asked("clarify-test-chat");
        assert_eq!(last_asked("clarify-test-chat"), None);
    }
}

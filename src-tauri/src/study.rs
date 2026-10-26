// SPDX-License-Identifier: AGPL-3.0-only
//! Study material from what the person is learning: flashcards (practiced
//! with spaced repetition), quizzes (with why each answer is right) and mind
//! maps, written by the chat model from their notes, files or documents in
//! the library. Podcasts and explainer videos about the same material are
//! made in the Studio (`media::podcast`, `media::explainer`).
//!
//! Long material is read in parts that fit the model, each giving its share
//! of the cards or questions, so even a small model covers a whole book
//! chapter. Sets are kept encrypted like everything else.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::Emitter;

use crate::{AppState, AppStateRef};

/// What to make.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Cards,
    Quiz,
    Map,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Card {
    pub front: String,
    pub back: String,
    /// Spaced repetition: its box (0 = new or missed; each box waits longer)
    /// and when it's next due (ms).
    #[serde(rename = "box")]
    pub level: u8,
    pub due: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Question {
    pub question: String,
    pub choices: Vec<String>,
    /// The right choice (from 0).
    pub answer: usize,
    /// Why it's right.
    pub why: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Node {
    pub label: String,
    pub children: Vec<Node>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StudySet {
    pub id: String,
    pub title: String,
    pub kind: Option<Kind>,
    pub created_at: i64,
    /// What it was made from, in a few words ("Biology notes.pdf and 2 more").
    pub from: String,
    pub cards: Vec<Card>,
    pub quiz: Vec<Question>,
    pub map: Option<Node>,
    /// A quiz's best score (0 to 1) and how many times it was taken.
    pub best: Option<f32>,
    pub taken: u32,
}

/// What to make it from.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct MakeRequest {
    pub kind: Option<Kind>,
    /// Notes or a topic typed or pasted in.
    pub text: String,
    /// Files on this PC.
    pub files: Vec<String>,
    /// Files in the documents library (ids).
    pub docs: Vec<String>,
    /// How many cards or questions (a mind map ignores it).
    pub count: Option<usize>,
    pub language: Option<String>,
    /// The chat model that writes it (None: the loaded or default one).
    pub writer: Option<String>,
}

// ---------- kept ----------

fn put(conn: &rusqlite::Connection, c: &crate::crypto::Cipher, s: &StudySet) -> Result<(), String> {
    let data = c.encrypt(&serde_json::to_string(s).map_err(|e| e.to_string())?);
    conn.execute(
        "INSERT INTO study_sets (id, data, created_at) VALUES (?1, ?2, ?3) ON CONFLICT(id) DO UPDATE SET data = excluded.data",
        rusqlite::params![s.id, data, s.created_at],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn all(conn: &rusqlite::Connection, c: &crate::crypto::Cipher) -> Vec<StudySet> {
    let Ok(mut st) = conn.prepare("SELECT data FROM study_sets ORDER BY created_at DESC") else { return Vec::new() };
    st.query_map([], |r| r.get::<_, String>(0))
        .map(|rows| rows.flatten().filter_map(|d| c.decrypt(&d).ok()).filter_map(|j| serde_json::from_str(&j).ok()).collect())
        .unwrap_or_default()
}

fn get(conn: &rusqlite::Connection, c: &crate::crypto::Cipher, id: &str) -> Option<StudySet> {
    let d: String = conn.query_row("SELECT data FROM study_sets WHERE id = ?1", [id], |r| r.get(0)).ok()?;
    serde_json::from_str(&c.decrypt(&d).ok()?).ok()
}

// ---------- writing ----------

/// The first JSON value in a reply (models wrap it in words or fences).
fn json_in(reply: &str) -> Option<Value> {
    let start = reply.find(['{', '['])?;
    let end = reply.rfind(['}', ']'])?;
    serde_json::from_str(reply.get(start..=end)?).ok()
}

/// The material cut into parts of at most `room` characters at paragraph ends.
pub fn parts_of(text: &str, room: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for para in text.split("\n\n") {
        if cur.chars().count() + para.chars().count() > room && !cur.trim().is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        // A paragraph bigger than a part is cut where it must be.
        let mut rest = para;
        while rest.chars().count() > room {
            let at = rest.char_indices().nth(room).map(|(i, _)| i).unwrap_or(rest.len());
            out.push(rest[..at].to_string());
            rest = &rest[at..];
        }
        cur.push_str(rest);
        cur.push_str("\n\n");
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out.into_iter().map(|p| p.trim().to_string()).filter(|p| p.chars().count() >= 40).collect()
}

/// How many of `total` items each part gives: by its length, at least one.
pub fn shares(lengths: &[usize], total: usize) -> Vec<usize> {
    let sum: usize = lengths.iter().sum::<usize>().max(1);
    let mut out: Vec<usize> = lengths.iter().map(|l| ((total * l) as f64 / sum as f64).round().max(1.0) as usize).collect();
    // Never far past what was asked for.
    while out.iter().sum::<usize>() > total.max(lengths.len()) {
        if let Some(i) = (0..out.len()).max_by_key(|&i| out[i]) {
            out[i] -= 1;
        }
    }
    out
}

fn language_note(code: Option<&str>) -> String {
    code.and_then(|c| crate::translate::LANGUAGES.iter().find(|(k, _)| k.eq_ignore_ascii_case(c)).map(|(_, n)| *n)).map(|n| format!(" Write in {n}.")).unwrap_or_default()
}

/// Cards from a reply: `[{"front": …, "back": …}]`.
pub fn cards_from(reply: &str) -> Vec<Card> {
    let v = json_in(reply).unwrap_or(Value::Null);
    let list = v.get("cards").cloned().unwrap_or(v);
    list.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|c| {
                    let front = c.get("front").or_else(|| c.get("q")).and_then(Value::as_str)?.trim().to_string();
                    let back = c.get("back").or_else(|| c.get("a")).and_then(Value::as_str)?.trim().to_string();
                    (!front.is_empty() && !back.is_empty()).then_some(Card { front, back, ..Default::default() })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Questions from a reply: `[{"question", "choices": [4], "answer": n, "why"}]`,
/// the right answer given as its number or its words.
pub fn questions_from(reply: &str) -> Vec<Question> {
    let v = json_in(reply).unwrap_or(Value::Null);
    let list = v.get("questions").cloned().unwrap_or(v);
    list.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|q| {
                    let question = q.get("question").and_then(Value::as_str)?.trim().to_string();
                    let choices: Vec<String> = q.get("choices")?.as_array()?.iter().filter_map(|c| c.as_str().map(|s| s.trim().to_string())).filter(|s| !s.is_empty()).collect();
                    if question.is_empty() || !(2..=6).contains(&choices.len()) {
                        return None;
                    }
                    let answer = match q.get("answer") {
                        Some(Value::Number(n)) => n.as_u64().map(|n| n as usize),
                        Some(Value::String(s)) => choices.iter().position(|c| c.eq_ignore_ascii_case(s.trim())).or_else(|| {
                            // "B" or "b)" for the second.
                            let first = s.trim().chars().next()?.to_ascii_uppercase();
                            first.is_ascii_uppercase().then(|| (first as u8 - b'A') as usize).filter(|i| *i < choices.len() && s.trim().len() <= 3)
                        }),
                        _ => None,
                    }
                    .filter(|i| *i < choices.len())?;
                    let why = q.get("why").or_else(|| q.get("explanation")).and_then(Value::as_str).unwrap_or("").trim().to_string();
                    Some(Question { question, choices, answer, why })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A mind map from a reply: `{"label", "children": [...]}`, at most four levels.
pub fn map_from(reply: &str) -> Option<Node> {
    fn node(v: &Value, depth: usize) -> Option<Node> {
        let label = v.get("label").or_else(|| v.get("topic")).or_else(|| v.get("name")).and_then(Value::as_str)?.trim().to_string();
        if label.is_empty() {
            return None;
        }
        let children = if depth < 4 { v.get("children").and_then(Value::as_array).map(|a| a.iter().filter_map(|c| node(c, depth + 1)).take(12).collect()).unwrap_or_default() } else { Vec::new() };
        Some(Node { label: label.chars().take(120).collect(), children })
    }
    node(&json_in(reply)?, 1)
}

/// The material: the typed words, the files' text and the library files' passages.
async fn material(state: &Arc<AppState>, req: &MakeRequest) -> Result<(String, String), String> {
    let mut parts = Vec::new();
    let mut names = Vec::new();
    if !req.text.trim().is_empty() {
        parts.push(req.text.trim().to_string());
        names.push("your notes".to_string());
    }
    for f in req.files.iter().take(20) {
        let path = std::path::PathBuf::from(f);
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let text = tokio::task::spawn_blocking(move || crate::docs::read_all(&path)).await.map_err(|e| e.to_string())?.map_err(|e| format!("{name}: {e}"))?;
        parts.push(format!("# {name}\n\n{text}"));
        names.push(name);
    }
    for id in req.docs.iter().take(20) {
        if let Some((name, text)) = crate::doclib::file_text(state, id) {
            parts.push(format!("# {name}\n\n{text}"));
            names.push(name);
        }
    }
    let from = match names.len() {
        0 => return Err("Add something to study: type or paste it, or pick files.".into()),
        1 => names[0].clone(),
        n => format!("{} and {} more", names[0], n - 1),
    };
    Ok((parts.join("\n\n"), from))
}

fn say(state: &AppState, stage: &str, fraction: f64) {
    if let Some(app) = state.app.get() {
        let _ = app.emit("study:progress", json!({ "stage": stage, "fraction": fraction }));
    }
}

/// Makes a set: flashcards, a quiz or a mind map.
pub async fn make(state: &Arc<AppState>, req: &MakeRequest) -> Result<StudySet, String> {
    let kind = req.kind.ok_or("Pick what to make.")?;
    let (text, from) = material(state, req).await?;
    say(state, "Getting the writer ready", 0.02);
    let (ep, model, _hold) = crate::endpoint_for(state, req.writer.as_deref()).await?;
    if ep.cloud.is_some() {
        if let Ok(c) = state.cipher() {
            let me = crate::outgoing::context(state, &c).me;
            let r = crate::outgoing::review_cloud(&model.name, &text.chars().take(600).collect::<String>(), "", "", "", &[], 0, false, &me);
            crate::outgoing::record(state, &c, "", "cloud_chat", &r, true);
        }
    }
    // A part fits with the instructions and the reply (about 3 characters a token).
    let room = ((ep.ctx as usize).saturating_sub(3_000) * 3 / 2).clamp(3_000, 40_000);
    let parts = parts_of(&text, room);
    if parts.is_empty() {
        return Err("There's too little to study in it.".into());
    }
    let lang = language_note(req.language.as_deref());
    let mut set = StudySet { id: uuid::Uuid::new_v4().simple().to_string(), kind: Some(kind), created_at: crate::db::now_ms(), from, ..Default::default() };
    let n = parts.len();
    match kind {
        Kind::Cards | Kind::Quiz => {
            let total = req.count.unwrap_or(if kind == Kind::Cards { 20 } else { 10 }).clamp(3, 100);
            let each = shares(&parts.iter().map(|p| p.chars().count()).collect::<Vec<_>>(), total);
            for (i, (part, want)) in parts.iter().zip(each).enumerate() {
                say(state, &format!("Writing from part {} of {n}", i + 1), 0.05 + 0.9 * i as f64 / n as f64);
                let ask = if kind == Kind::Cards {
                    format!(
                        "Make {want} flashcards for studying the material below. Each card asks one clear thing on the front (a term, a question, a cause, a date) and answers it briefly on the back. Cover the most important ideas, not trivia. Reply with only JSON: [{{\"front\": \"…\", \"back\": \"…\"}}].{lang}\n\nMaterial:\n{part}"
                    )
                } else {
                    format!(
                        "Write {want} multiple-choice questions that test understanding of the material below. Each has 4 choices, exactly one right, the wrong ones plausible; mix recall with why and how questions. Reply with only JSON: [{{\"question\": \"…\", \"choices\": [\"…\", \"…\", \"…\", \"…\"], \"answer\": the right choice's number from 0, \"why\": \"one sentence on why it's right\"}}].{lang}\n\nMaterial:\n{part}"
                    )
                };
                let msgs = vec![json!({"role": "system", "content": "You write study material from a person's own notes. Use only what the material says."}), json!({"role": "user", "content": ask})];
                let reply = crate::chat::complete(&ep, msgs, json!({"temperature": 0.4}), (want as u32 * 120 + 400).min(8_000)).await?;
                if kind == Kind::Cards {
                    set.cards.extend(cards_from(&reply));
                } else {
                    set.quiz.extend(questions_from(&reply));
                }
            }
            // The same card or question twice (from overlapping parts) is kept once.
            let mut seen = std::collections::HashSet::new();
            set.cards.retain(|c| seen.insert(c.front.to_lowercase()));
            set.quiz.retain(|q| seen.insert(q.question.to_lowercase()));
            if set.cards.is_empty() && set.quiz.is_empty() {
                return Err(format!("{} didn't write any. Try again, or pick another model to write them.", model.name));
            }
        }
        Kind::Map => {
            // Each part's outline, then one map of them all.
            let mut outlines = Vec::new();
            for (i, part) in parts.iter().enumerate() {
                if n == 1 {
                    outlines.push(part.clone());
                    break;
                }
                say(state, &format!("Reading part {} of {n}", i + 1), 0.05 + 0.6 * i as f64 / n as f64);
                let msgs = vec![json!({"role": "user", "content": format!("List the main ideas of this material as a short outline (headings and sub-points, a few words each).{lang}\n\n{part}")})];
                outlines.push(crate::chat::complete(&ep, msgs, json!({"temperature": 0.2}), 800).await?);
            }
            say(state, "Drawing the map", 0.7);
            let ask = format!(
                "Make a mind map of the material below: one central topic, 3 to 7 main branches, each with a few short sub-points (a few words each, up to 3 levels deep). Reply with only JSON: {{\"label\": \"the central topic\", \"children\": [{{\"label\": \"…\", \"children\": [...]}}]}}.{lang}\n\n{}",
                clip(&outlines.join("\n\n"), room)
            );
            let reply = crate::chat::complete(&ep, vec![json!({"role": "user", "content": ask})], json!({"temperature": 0.3}), 2_000).await?;
            set.map = Some(map_from(&reply).ok_or_else(|| format!("{} didn't draw a map. Try again, or pick another model.", model.name))?);
        }
    }
    set.title = match kind {
        Kind::Map => set.map.as_ref().map(|m| m.label.clone()).unwrap_or_default(),
        _ => set.from.clone(),
    };
    say(state, "Done", 1.0);
    state.log("study", &format!("Made {} from {} part(s) with {}", match kind { Kind::Cards => format!("{} flashcards", set.cards.len()), Kind::Quiz => format!("a {}-question quiz", set.quiz.len()), Kind::Map => "a mind map".into() }, n, model.name));
    Ok(set)
}

fn clip(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// Days a card waits in each box before it's due again.
const WAIT_DAYS: [i64; 6] = [0, 1, 3, 7, 16, 35];

/// A card answered: knew it moves it up a box (due later), missed it starts again.
pub fn answered(card: &mut Card, knew: bool, now: i64) {
    card.level = if knew { (card.level + 1).min(WAIT_DAYS.len() as u8 - 1) } else { 0 };
    card.due = now + WAIT_DAYS[card.level as usize] * 86_400_000 + if knew { 0 } else { 60_000 };
}

// ---------- commands ----------

#[tauri::command]
pub fn study_sets(state: AppStateRef<'_>) -> Result<Vec<StudySet>, String> {
    let c = state.work_cipher()?;
    Ok(all(&state.db.lock().unwrap(), &c))
}

/// Makes a set and keeps it. "study:progress" events say how far it is.
#[tauri::command]
pub async fn study_make(state: AppStateRef<'_>, request: MakeRequest) -> Result<StudySet, String> {
    crate::features::require(&state, crate::features::Feature::Documents)?;
    let state = state.inner().clone();
    let set = make(&state, &request).await?;
    let c = state.work_cipher()?;
    put(&state.db.lock().unwrap(), &c, &set)?;
    Ok(set)
}

#[tauri::command]
pub fn study_delete(state: AppStateRef<'_>, id: String) -> Result<(), String> {
    state.db.lock().unwrap().execute("DELETE FROM study_sets WHERE id = ?1", [&id]).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn study_rename(state: AppStateRef<'_>, id: String, title: String) -> Result<StudySet, String> {
    let c = state.work_cipher()?;
    let conn = state.db.lock().unwrap();
    let mut s = get(&conn, &c, &id).ok_or("That set is no longer here.")?;
    s.title = title.trim().chars().take(120).collect();
    put(&conn, &c, &s)?;
    Ok(s)
}

/// A flashcard answered (`knew`), for spaced repetition.
#[tauri::command]
pub fn study_card(state: AppStateRef<'_>, id: String, card: usize, knew: bool) -> Result<StudySet, String> {
    let c = state.work_cipher()?;
    let conn = state.db.lock().unwrap();
    let mut s = get(&conn, &c, &id).ok_or("That set is no longer here.")?;
    let k = s.cards.get_mut(card).ok_or("No such card.")?;
    answered(k, knew, crate::db::now_ms());
    put(&conn, &c, &s)?;
    Ok(s)
}

/// A quiz finished with `score` (0 to 1).
#[tauri::command]
pub fn study_quiz_done(state: AppStateRef<'_>, id: String, score: f32) -> Result<StudySet, String> {
    let c = state.work_cipher()?;
    let conn = state.db.lock().unwrap();
    let mut s = get(&conn, &c, &id).ok_or("That set is no longer here.")?;
    s.taken += 1;
    s.best = Some(s.best.unwrap_or(0.0).max(score.clamp(0.0, 1.0)));
    put(&conn, &c, &s)?;
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies_become_cards_questions_and_maps() {
        let cards = cards_from("Here you go:\n```json\n[{\"front\": \"Mitochondria\", \"back\": \"Make the cell's energy (ATP)\"}, {\"front\": \"\", \"back\": \"x\"}]\n```");
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].front, "Mitochondria");
        let q = questions_from(r#"{"questions": [
            {"question": "What makes ATP?", "choices": ["Nucleus", "Mitochondria", "Ribosome", "Wall"], "answer": 1, "why": "It's where respiration happens."},
            {"question": "Plants make food by?", "choices": ["Photosynthesis", "Osmosis", "Diffusion", "Mitosis"], "answer": "Photosynthesis"},
            {"question": "Letter answer", "choices": ["a", "b", "c", "d"], "answer": "C)"},
            {"question": "Bad", "choices": ["only one"], "answer": 0}
        ]}"#);
        assert_eq!(q.len(), 3, "{q:?}");
        assert_eq!(q[0].answer, 1);
        assert_eq!(q[1].answer, 0);
        assert_eq!(q[2].answer, 2);
        let m = map_from(r#"{"topic": "Cells", "children": [{"label": "Parts", "children": [{"label": "Nucleus"}]}, {"label": "Energy"}]}"#).unwrap();
        assert_eq!(m.label, "Cells");
        assert_eq!(m.children.len(), 2);
        assert_eq!(m.children[0].children[0].label, "Nucleus");
    }

    #[test]
    fn long_material_is_shared_out_in_parts() {
        let text = (0..30).map(|i| format!("Paragraph {i}. {}", "word ".repeat(40))).collect::<Vec<_>>().join("\n\n");
        let parts = parts_of(&text, 2_000);
        assert!(parts.len() >= 3, "{}", parts.len());
        assert!(parts.iter().all(|p| p.chars().count() <= 2_000));
        let each = shares(&parts.iter().map(|p| p.chars().count()).collect::<Vec<_>>(), 20);
        assert!(each.iter().all(|n| *n >= 1));
        assert!(each.iter().sum::<usize>() <= 20.max(parts.len()));
        assert_eq!(shares(&[100, 100, 100], 2), vec![1, 1, 1], "every part gives at least one");
    }

    #[test]
    fn cards_wait_longer_each_time_they_are_known() {
        let mut c = Card::default();
        answered(&mut c, true, 0);
        assert_eq!((c.level, c.due), (1, 86_400_000));
        answered(&mut c, true, 0);
        assert_eq!(c.level, 2);
        answered(&mut c, false, 0);
        assert_eq!(c.level, 0, "missed: back to the start");
        assert!(c.due > 0 && c.due < 86_400_000, "seen again soon");
    }
}

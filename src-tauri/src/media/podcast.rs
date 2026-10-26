// SPDX-License-Identifier: AGPL-3.0-only
//! Podcasts, and readings with several voices.
//!
//! - A podcast ("podcast"): the chat model writes a conversation between the
//!   people the person picked, about what they gave it (their words, files,
//!   and what a web search found when they asked for one). It's planned
//!   first, then written part by part, so even a small model stays lively
//!   and near the length asked for. Each line is then read in its speaker's
//!   voice.
//! - A reading with several voices ("dialogue"): the person's own script, a
//!   "Name: words" line for each turn, read the same way.
//!
//! Voices are any narration has: Windows', the natural voices, and the
//! person's recorded or made voices (each read in one go, so its model loads
//! once). Lines are levelled, spaced like people talking (a short "Right."
//! comes in just before the other finishes), set a little apart left and
//! right, and kept as one MP3 with the script as its words.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{own_voice, stems, store, MediaItem, Request, RunCtx};
use crate::{download, AppState};

/// The most people in one podcast or reading.
pub const MAX_SPEAKERS: usize = 4;
/// Everything is mixed at this rate.
const RATE: u32 = 44_100;
/// Words a minute at a podcast's pace.
const WPM: f32 = 150.0;
/// A podcast's length, in minutes, when none is asked for, and its longest.
const DEFAULT_MINUTES: f32 = 5.0;
const MAX_MINUTES: f32 = 30.0;

/// Who talks and, for a podcast, what to write.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Spec {
    pub speakers: Vec<Speaker>,
    /// About how long a podcast runs (1 to 30 minutes).
    pub minutes: Option<f32>,
    /// "conversation", "interview", "debate", "lesson" or "story".
    pub style: Option<String>,
    /// Look the topic up on the web first.
    pub research: bool,
    /// Files to talk about, read as text.
    pub files: Vec<String>,
    /// The chat model that writes it (None: the loaded or default one).
    pub writer: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct Speaker {
    pub name: String,
    /// "own:<id>", "supertonic:F1", "system:<id>"; None: one picked for them.
    pub voice: Option<String>,
    /// Who they are in it ("the host", "a chef who grew up in Naples").
    pub about: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    /// Index into the speakers.
    pub speaker: usize,
    pub text: String,
}

// ---------- scripts ----------

/// What's read of a line: no directions in brackets or between asterisks
/// ("[laughs]", "*pauses*"), no Markdown marks.
fn spoken(text: &str) -> String {
    let mut out = String::new();
    let mut skip: Option<char> = None;
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match skip {
            Some(end) if c == end => skip = None,
            Some(_) => {}
            None if c == '[' => skip = Some(']'),
            // "*laughs*" is a direction; a lone asterisk is dropped.
            None if c == '*' && chars[i + 1..].contains(&'*') => skip = Some('*'),
            None if matches!(c, '*' | '#' | '`' | '_' | '~') => {}
            None => out.push(c),
        }
        i += 1;
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The name a line starts with ("**Ana:** Hi" → "ana"), lowercased.
fn name_of(line: &str) -> Option<(String, &str)> {
    let (head, rest) = line.split_once(':')?;
    let name = head.trim().trim_matches(|c: char| matches!(c, '*' | '_' | '-' | '>' | '#') || c.is_whitespace()).to_lowercase();
    let ok = !name.is_empty() && name.chars().count() <= 32 && name.split_whitespace().count() <= 4 && !name.contains("http");
    ok.then_some((name, rest))
}

/// A script's title (a first "# Title" or "Title: …" line) and its turns:
/// each "Name: words" line starts one; a line without a known name carries
/// on the turn before.
pub fn parse(script: &str, speakers: &[Speaker]) -> (Option<String>, Vec<Turn>) {
    let names: Vec<String> = speakers.iter().map(|s| s.name.trim().to_lowercase()).collect();
    let mut title = None;
    let mut turns: Vec<Turn> = Vec::new();
    for raw in script.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('#') {
            if turns.is_empty() && title.is_none() {
                title = Some(line.trim_start_matches('#').trim().to_string()).filter(|t| !t.is_empty());
            }
            continue;
        }
        let named = name_of(line);
        if let Some((n, rest)) = &named {
            if n == "title" && turns.is_empty() && title.is_none() {
                title = Some(spoken(rest)).filter(|t| !t.is_empty());
                continue;
            }
        }
        let (who, said) = match named.as_ref().and_then(|(n, rest)| names.iter().position(|x| x == n).map(|i| (i, *rest))) {
            Some((i, rest)) => (Some(i), spoken(rest)),
            None => (None, spoken(line)),
        };
        if said.is_empty() {
            continue;
        }
        match (who, turns.last_mut()) {
            (Some(i), _) => turns.push(Turn { speaker: i, text: said }),
            (None, Some(t)) => {
                t.text.push(' ');
                t.text.push_str(&said);
            }
            (None, None) => turns.push(Turn { speaker: 0, text: said }),
        }
    }
    (title, turns)
}

/// The people a script names ("Name:" at the start of lines), in order of
/// first line, at most four: for a script read without speakers set up.
pub fn names_in(script: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in script.lines().map(str::trim).filter(|l| !l.starts_with('#')) {
        let Some((head, rest)) = line.split_once(':') else { continue };
        let name = head.trim().trim_matches(|c: char| matches!(c, '*' | '_' | '-' | '>') || c.is_whitespace());
        let fits = !name.is_empty() && name.chars().count() <= 32 && name.split_whitespace().count() <= 3 && name.chars().next().is_some_and(char::is_alphabetic);
        if fits && !rest.trim().is_empty() && !name.eq_ignore_ascii_case("title") && !out.iter().any(|n| n.eq_ignore_ascii_case(name)) {
            out.push(name.to_string());
            if out.len() == MAX_SPEAKERS {
                break;
            }
        }
    }
    out
}

/// The speakers to read with: the ones set up, else the script's names.
pub fn speakers_for(spec: &Spec, script: &str) -> Vec<Speaker> {
    let set: Vec<Speaker> = spec.speakers.iter().filter(|s| !s.name.trim().is_empty()).take(MAX_SPEAKERS).cloned().collect();
    if !set.is_empty() {
        return set;
    }
    let names = names_in(script);
    if names.is_empty() {
        return vec![Speaker { name: "Narrator".into(), ..Default::default() }];
    }
    names.into_iter().map(|name| Speaker { name, ..Default::default() }).collect()
}

/// Gives everyone without a voice one of their own: natural voices that
/// speak the language first (taking turns between women's and men's), else
/// Windows' voices; someone already picked isn't picked twice.
fn fill_voices(speakers: &mut [Speaker], voices: &[crate::tts::Voice], language: Option<&str>) {
    let lang = language.map(|l| l.split(['-', '_']).next().unwrap_or("").to_lowercase()).filter(|l| !l.is_empty()).unwrap_or_else(|| "en".into());
    let speaks = |v: &crate::tts::Voice| v.languages.iter().any(|l| *l == lang) || v.language.to_lowercase().starts_with(&lang);
    let mut free: Vec<&crate::tts::Voice> = voices.iter().filter(|v| v.engine == "natural" && speaks(v)).collect();
    // Alternate women's and men's voices, so two people sound different.
    let (mut women, mut men): (Vec<&crate::tts::Voice>, Vec<&crate::tts::Voice>) = free.drain(..).partition(|v| v.gender.eq_ignore_ascii_case("female"));
    let mut natural = Vec::new();
    while !women.is_empty() || !men.is_empty() {
        if !women.is_empty() {
            natural.push(women.remove(0));
        }
        if !men.is_empty() {
            natural.push(men.remove(0));
        }
    }
    let system: Vec<&crate::tts::Voice> = voices.iter().filter(|v| v.engine == "system" && speaks(v)).collect();
    let mut taken: HashSet<String> = speakers.iter().filter_map(|s| s.voice.clone()).collect();
    for s in speakers.iter_mut().filter(|s| s.voice.is_none()) {
        let pick = natural.iter().chain(system.iter()).find(|v| !taken.contains(&v.id)).map(|v| v.id.clone());
        if let Some(id) = &pick {
            taken.insert(id.clone());
        }
        s.voice = pick;
    }
}

// ---------- writing ----------

/// The name of a language for the writer ("de" → "German").
fn language_name(code: Option<&str>) -> Option<&'static str> {
    let code = code?.trim();
    crate::translate::LANGUAGES.iter().find(|(c, _)| c.eq_ignore_ascii_case(code) || c.split('-').next() == Some(code)).map(|(_, n)| *n)
}

fn style_words(style: Option<&str>) -> &'static str {
    match style.unwrap_or("conversation") {
        "interview" => "an interview: one host asks, the guests answer and tell stories",
        "debate" => "a friendly debate: they take different sides, push back, and end on what they agree on",
        "lesson" => "a lesson for students: they explain step by step, check understanding, and recap the key points",
        "story" => "a story told together: they narrate and act it out, with a beginning, middle and end",
        _ => "a relaxed conversation between friends who know the subject",
    }
}

/// The first JSON value in a reply (models wrap it in words or fences).
fn json_in(reply: &str) -> Option<Value> {
    let start = reply.find(['{', '['])?;
    let end = reply.rfind(['}', ']'])?;
    serde_json::from_str(reply.get(start..=end)?).ok()
}

/// Up to `max` characters of the text, cut at a line.
fn clipped(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max).collect();
    let at = cut.rfind('\n').filter(|i| *i > max / 2).unwrap_or(cut.len());
    format!("{}\n[…]", &cut[..at])
}

/// Notes from the web on the topic: searches the writer suggests, and the
/// lines that bear on them from the top pages. Each search and page is
/// listed under "What left this PC".
async fn research(state: &Arc<AppState>, ep: &crate::engine::Endpoint, topic: &str, chat_id: &str, cancel: &AtomicBool, progress: &(dyn Fn(&str, f64) + Sync)) -> String {
    progress("Choosing what to look up", 0.04);
    let ask = vec![
        json!({"role": "system", "content": "You suggest web searches. Reply with only a JSON array of 3 short search queries."}),
        json!({"role": "user", "content": format!("Topic for a podcast episode:\n{}\n\nThree web searches that would find the key facts:", clipped(topic, 1500))}),
    ];
    let mut queries: Vec<String> = crate::chat::complete(ep, ask, json!({"temperature": 0.2}), 200)
        .await
        .ok()
        .and_then(|r| json_in(&r))
        .and_then(|v| v.as_array().cloned())
        .map(|a| a.iter().filter_map(|q| q.as_str().map(|s| s.trim().chars().take(120).collect::<String>())).filter(|q| !q.is_empty()).take(3).collect())
        .unwrap_or_default();
    if queries.is_empty() {
        queries.push(topic.split_whitespace().take(12).collect::<Vec<_>>().join(" "));
    }
    let Ok(c) = state.work_cipher() else { return String::new() };
    let prepared = crate::web::prepare(&state.db.lock().unwrap(), &c, chat_id);
    let (client, settings, key) = match prepared {
        Ok(p) => p,
        Err(e) => {
            crate::applog::line("media", &format!("The podcast couldn't look things up: {e}"));
            return String::new();
        }
    };
    let cx = crate::outgoing::context(state, &c);
    let mut notes = String::new();
    let mut seen = HashSet::new();
    let mut read = 0;
    for (n, q) in queries.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        progress(&format!("Looking up “{q}”"), 0.05 + 0.1 * n as f64 / queries.len() as f64);
        let args = json!({ "query": q });
        if let Some(r) = crate::outgoing::review("web_search", &args, &cx) {
            // Something secret in a search the writer made up: not sent.
            if r.level >= 5 {
                crate::outgoing::record(state, &c, chat_id, "web_search", &r, false);
                continue;
            }
            crate::outgoing::record(state, &c, chat_id, "web_search", &r, true);
        }
        let hits = match crate::web::search(&settings, key.clone(), &client, q, 5, None).await {
            Ok((h, _)) => h,
            Err(e) => {
                crate::applog::line("media", &format!("A podcast search didn't work: {e}"));
                continue;
            }
        };
        for h in hits.iter().take(2) {
            if read >= 5 || !seen.insert(h.url.clone()) {
                continue;
            }
            let args = json!({ "url": h.url });
            if let Some(r) = crate::outgoing::review("fetch_page", &args, &cx) {
                crate::outgoing::record(state, &c, chat_id, "fetch_page", &r, true);
            }
            match crate::web::fetch(&client, &h.url).await {
                Ok(p) => {
                    let lines = crate::web::excerpts(&p.text, q, 1500);
                    if !lines.trim().is_empty() {
                        notes.push_str(&format!("From “{}” ({}):\n{}\n\n", p.title.trim(), p.url, lines.trim()));
                        read += 1;
                    }
                }
                Err(_) if !h.snippet.is_empty() => notes.push_str(&format!("From “{}” ({}):\n{}\n\n", h.title.trim(), h.url, h.snippet.trim())),
                Err(_) => {}
            }
        }
    }
    state.log("media", &format!("A podcast looked up {} search(es) and read {read} page(s)", queries.len()));
    notes
}

/// What the podcast is about: the person's words, their files as text, and
/// what the web said, cut to fit the writer.
async fn material(state: &Arc<AppState>, req: &Request, ep: &crate::engine::Endpoint, cancel: &AtomicBool, progress: &(dyn Fn(&str, f64) + Sync)) -> String {
    let spec = req.talk.clone().unwrap_or_default();
    // Room for the material: what the model holds, less the instructions and
    // the part it writes (about 3 characters a token).
    let room = ((ep.ctx as usize).saturating_sub(3_500) * 3).clamp(4_000, 60_000);
    let mut parts = Vec::new();
    for f in spec.files.iter().take(10) {
        let path = std::path::PathBuf::from(f);
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        match tokio::task::spawn_blocking(move || crate::docs::read_text(&path)).await {
            Ok(Ok(text)) if !text.trim().is_empty() => parts.push(format!("From the file “{name}”:\n{}", text.trim())),
            Ok(Err(e)) => crate::applog::line("media", &format!("A podcast file couldn't be read ({name}): {e}")),
            _ => {}
        }
    }
    if spec.research {
        let found = research(state, ep, &req.prompt, req.chat_id.as_deref().unwrap_or(""), cancel, progress).await;
        if !found.is_empty() {
            parts.push(format!("Found on the web:\n{found}"));
        }
    }
    let each = room / parts.len().max(1);
    parts.iter().map(|p| clipped(p, each)).collect::<Vec<_>>().join("\n\n")
}

/// Who's in it, for the writer: "Ana (the host)".
fn cast(speakers: &[Speaker]) -> String {
    speakers
        .iter()
        .map(|s| match s.about.as_deref().map(str::trim).filter(|a| !a.is_empty()) {
            Some(a) => format!("- {}: {a}", s.name.trim()),
            None => format!("- {}", s.name.trim()),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Writes a podcast: "# Title", then "Name: words" lines.
pub async fn write(state: &Arc<AppState>, req: &Request, cancel: &AtomicBool, progress: &(dyn Fn(&str, f64) + Sync)) -> Result<String, String> {
    progress("Getting the writer ready", 0.02);
    let spec = req.talk.clone().unwrap_or_default();
    let (ep, model, _hold) = crate::endpoint_for(state, spec.writer.as_deref()).await?;
    if ep.cloud.is_some() {
        if let Ok(c) = state.cipher() {
            let me = crate::outgoing::context(state, &c).me;
            let r = crate::outgoing::review_cloud(&model.name, &req.prompt, "", "", "", &[], 0, false, &me);
            crate::outgoing::record(state, &c, req.chat_id.as_deref().unwrap_or(""), "cloud_chat", &r, true);
        }
    }
    write_with(state, req, &ep, &model.name, cancel, progress).await
}

/// `write` with the model at `ep` (`name`, for messages).
pub(crate) async fn write_with(state: &Arc<AppState>, req: &Request, ep: &crate::engine::Endpoint, name: &str, cancel: &AtomicBool, progress: &(dyn Fn(&str, f64) + Sync)) -> Result<String, String> {
    let spec = req.talk.clone().unwrap_or_default();
    let speakers = speakers_for(&spec, "");
    let notes = material(state, req, ep, cancel, progress).await;
    if cancel.load(Ordering::Relaxed) {
        return Err(download::CANCELLED.into());
    }
    let minutes = spec.minutes.unwrap_or(DEFAULT_MINUTES).clamp(1.0, MAX_MINUTES);
    let parts = (minutes / 2.5).ceil().clamp(1.0, 12.0) as usize;
    let words_each = (minutes * WPM / parts as f32).round() as usize;
    let language = language_name(req.language.as_deref()).map(|l| format!(" Write it in {l}.")).unwrap_or_default();
    let names: Vec<String> = speakers.iter().map(|s| s.name.trim().to_string()).collect();
    let about = format!(
        "Topic and what the person asked for:\n{}\n\n{}",
        clipped(req.prompt.trim(), 4_000),
        if notes.is_empty() { String::new() } else { format!("Material to draw on (use only facts from here or that you're sure of; don't invent numbers or quotes):\n{notes}") }
    );

    // The plan: a title and what each part covers.
    progress("Planning the episode", 0.17);
    let plan_ask = vec![
        json!({"role": "system", "content": "You plan podcast episodes. Reply with only JSON."}),
        json!({"role": "user", "content": format!(
            "{about}\n\nPeople talking:\n{}\n\nPlan an episode in the form of {}. Reply with JSON: {{\"title\": \"a short catchy episode title\", \"parts\": [{parts} short strings, each saying what that part covers, in order]}}.{language}",
            cast(&speakers), style_words(spec.style.as_deref())
        )}),
    ];
    let plan = crate::chat::complete(ep, plan_ask, json!({"temperature": 0.5}), 600).await?;
    let plan = json_in(&plan).unwrap_or(Value::Null);
    let title = plan.get("title").and_then(Value::as_str).map(|t| spoken(t)).filter(|t| !t.is_empty()).unwrap_or_else(|| req.prompt.split_whitespace().take(8).collect::<Vec<_>>().join(" "));
    let mut outline: Vec<String> = plan.get("parts").and_then(Value::as_array).map(|a| a.iter().filter_map(|p| p.as_str().map(str::to_string)).collect()).unwrap_or_default();
    outline.truncate(parts);
    while outline.len() < parts {
        outline.push(if outline.is_empty() { "Introduce the topic".into() } else { "Go deeper into the topic".into() });
    }

    // Each part in turn, carrying on from the last lines.
    let mut lines: Vec<String> = Vec::new();
    for (i, what) in outline.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(download::CANCELLED.into());
        }
        progress(&format!("Writing part {} of {parts}", i + 1), 0.2 + 0.18 * i as f64 / parts as f64);
        let place = match (i, parts) {
            (_, 1) => "This is the whole episode: open with a warm welcome that says who's talking and what it's about, and end by wrapping up and saying goodbye.".to_string(),
            (0, _) => "This is the opening: start with a warm welcome that says who's talking and what the episode is about.".to_string(),
            (i, n) if i + 1 == n => "This is the last part: wrap up the main points and say goodbye to the listeners.".to_string(),
            _ => "This is a middle part: carry on naturally from the lines before, without greeting again.".to_string(),
        };
        let before = if lines.is_empty() { String::new() } else { format!("\n\nThe last lines so far:\n{}", lines[lines.len().saturating_sub(6)..].join("\n")) };
        let ask = vec![
            json!({"role": "system", "content": format!(
                "You write podcast scripts that sound like real people talking: short turns, back and forth, questions, reactions, a little humor, plain spoken words. Each line is \"Name: what they say\", using only these names: {}. No stage directions, no sound or music cues, no Markdown, no narration outside the lines.",
                names.join(", ")
            )}),
            json!({"role": "user", "content": format!(
                "{about}\n\nPeople talking:\n{}\n\nThe episode, “{title}”, is {}. The plan: {}.\n\nWrite part {} of {parts}, about: {what}. {place} About {words_each} words. Mix longer explanations with quick reactions (\"Right.\", \"Wait, really?\").{language}{before}",
                cast(&speakers), style_words(spec.style.as_deref()), outline.join(" / "), i + 1
            )}),
        ];
        let room = ((words_each as f32 * 2.2) as u32 + 300).min(4_000);
        let mut got = Vec::new();
        for attempt in 0..2 {
            let reply = crate::chat::complete(ep, ask.clone(), json!({"temperature": if attempt == 0 { 0.85 } else { 0.6 }}), room).await?;
            let (_, turns) = parse(&reply, &speakers);
            got = turns.into_iter().map(|t| format!("{}: {}", names[t.speaker], t.text)).collect();
            if got.len() >= 2 {
                break;
            }
        }
        lines.extend(got);
    }
    if lines.len() < 2 {
        return Err(format!("{name} didn't write a conversation. Try again, or pick another model to write it."));
    }
    state.log("media", &format!("Wrote a podcast script ({} lines, {parts} parts) with {name}", lines.len()));
    Ok(format!("# {title}\n\n{}", lines.join("\n")))
}

// ---------- reading ----------

/// A short reaction ("Right.", "Mm-hm."), which comes in just before the
/// other person finishes.
fn is_reaction(text: &str) -> bool {
    text.split_whitespace().count() <= 3 && text.chars().count() <= 24
}

/// Without the quiet before and after (keeping 40 ms).
fn trimmed(v: &[f32]) -> &[f32] {
    let keep = (RATE as f32 * 0.04) as usize;
    let first = v.iter().position(|s| s.abs() > 0.01);
    let last = v.iter().rposition(|s| s.abs() > 0.01);
    match (first, last) {
        (Some(a), Some(b)) => &v[a.saturating_sub(keep)..(b + keep).min(v.len())],
        _ => &[],
    }
}

/// As loud as the others: its speech brought to the same level.
fn levelled(v: &[f32]) -> Vec<f32> {
    let frame = (RATE / 20) as usize;
    let loud: Vec<f32> = v.chunks(frame).map(crate::audio::rms).filter(|r| *r > 0.01).collect();
    if loud.is_empty() {
        return v.to_vec();
    }
    let level = (loud.iter().map(|r| r * r).sum::<f32>() / loud.len() as f32).sqrt();
    let gain = (0.1 / level).clamp(0.25, 4.0);
    v.iter().map(|s| s * gain).collect()
}

/// Where each speaker sits, left (-) to right (+).
fn pans(n: usize) -> Vec<f32> {
    match n {
        0 | 1 => vec![0.0],
        2 => vec![-0.2, 0.2],
        3 => vec![-0.25, 0.0, 0.25],
        _ => vec![-0.3, -0.1, 0.1, 0.3],
    }
}

/// The turns one after another like people talking, each levelled and at
/// its speaker's place: (left, right) at 44.1 kHz.
pub fn mix(turns: &[Turn], voiced: &[Vec<f32>], speakers: usize) -> (Vec<f32>, Vec<f32>) {
    let places = pans(speakers);
    let lead = (RATE as f32 * 0.3) as usize;
    let (mut l, mut r) = (vec![0f32; lead], vec![0f32; lead]);
    let mut end = lead;
    let mut prev: Option<&Turn> = None;
    for (t, v) in turns.iter().zip(voiced) {
        let v = levelled(trimmed(v));
        if v.is_empty() {
            continue;
        }
        let gap: f32 = match prev {
            None => 0.0,
            Some(p) if p.speaker == t.speaker => 0.45,
            Some(_) if is_reaction(&t.text) => -0.2,
            Some(p) if p.text.trim_end().ends_with('?') => 0.2,
            Some(_) => 0.32,
        };
        let start = (end as i64 + (gap * RATE as f32) as i64).max(0) as usize;
        let need = start + v.len();
        if l.len() < need {
            l.resize(need, 0.0);
            r.resize(need, 0.0);
        }
        let pan = places.get(t.speaker).copied().unwrap_or(0.0);
        let (gl, gr) = ((1.0 - pan).min(1.0), (1.0 + pan).min(1.0));
        for (k, s) in v.iter().enumerate() {
            l[start + k] += s * gl;
            r[start + k] += s * gr;
        }
        // A reaction under the other's last words doesn't move the next line earlier.
        end = end.max(need);
        prev = Some(t);
    }
    let tail = (RATE as f32 * 0.6) as usize;
    l.resize(end + tail, 0.0);
    r.resize(end + tail, 0.0);
    let peak = l.iter().chain(r.iter()).fold(0f32, |m, s| m.max(s.abs()));
    if peak > 0.97 {
        let g = 0.97 / peak;
        l.iter_mut().chain(r.iter_mut()).for_each(|s| *s *= g);
    }
    (l, r)
}

/// Reads a podcast's script (written before, in `req.lyrics`) or a reading's
/// (`req.prompt`) with each speaker's voice, and keeps it.
pub(super) async fn read(ctx: &RunCtx<'_>, work: &Path) -> Result<Vec<MediaItem>, String> {
    let req = ctx.req;
    let spec = req.talk.clone().unwrap_or_default();
    let script = if req.op == "podcast" { req.lyrics.clone().ok_or("The podcast's script wasn't written.")? } else { req.prompt.clone() };
    let mut speakers = speakers_for(&spec, &script);
    let (title, turns) = parse(&script, &speakers);
    if turns.is_empty() {
        return Err("Write who says what, one line each: “Name: what they say”.".into());
    }
    let language = req.language.clone().filter(|l| l != "auto");
    if speakers.iter().any(|s| s.voice.is_none()) {
        let voices = tokio::task::spawn_blocking(crate::tts::voices).await.map_err(|e| e.to_string())?.unwrap_or_default();
        fill_voices(&mut speakers, &voices, language.as_deref());
    }
    let total = turns.len();
    let mut voiced: Vec<Vec<f32>> = vec![Vec::new(); total];
    let mut done = 0usize;
    let reading = |done: usize, stage: &str, f: f64| (ctx.progress)(stage, 0.4 + 0.55 * ((done as f64 + f) / total as f64).clamp(0.0, 1.0));

    // Recorded and made voices: each reads all its lines in one go.
    let mut own: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, t) in turns.iter().enumerate() {
        if let Some(id) = speakers[t.speaker].voice.as_deref().and_then(|v| v.strip_prefix("own:")) {
            own.entry(id.to_string()).or_default().push(i);
        }
    }
    for (id, which) in &own {
        let passages: Vec<String> = which.iter().map(|i| turns[*i].text.clone()).collect();
        let n = which.len() as f64;
        let at = done;
        let progress = |stage: &str, f: f64| reading(at, stage, f * n);
        let (parts, rate) = own_voice::speak_each(ctx.state, id, &passages, language.as_deref(), ctx.cancel, &progress).await?;
        for (i, p) in which.iter().zip(parts) {
            voiced[*i] = stems::resample(&p, rate, RATE);
        }
        done += which.len();
    }
    // The others, line by line.
    for (i, t) in turns.iter().enumerate() {
        if own.values().any(|w| w.contains(&i)) {
            continue;
        }
        if ctx.cancel.load(Ordering::Relaxed) {
            return Err(download::CANCELLED.into());
        }
        reading(done, &format!("Reading line {} of {total}", done + 1), 0.0);
        let (text, voice, lang) = (t.text.clone(), speakers[t.speaker].voice.clone(), language.clone());
        let speech = tokio::task::spawn_blocking(move || crate::tts::synthesize(&text, voice.as_deref(), lang.as_deref(), 1.0)).await.map_err(|e| e.to_string())??;
        let f: Vec<f32> = speech.samples.iter().map(|s| *s as f32 / 32768.0).collect();
        voiced[i] = stems::resample(&f, speech.rate, RATE);
        done += 1;
    }
    (ctx.progress)("Putting it together", 0.96);
    let (l, r) = mix(&turns, &voiced, speakers.len());
    let seconds = l.len() as f64 / RATE as f64;
    let (bytes, mime) = encode(&l, &r, work)?;
    // The words, as read: who said what.
    let words = turns.iter().map(|t| format!("{}: {}", speakers[t.speaker].name.trim(), t.text)).collect::<Vec<_>>().join("\n");
    let c = ctx.state.work_cipher()?;
    let item = MediaItem {
        kind: "audio".into(),
        op: req.op.clone(),
        prompt: req.prompt.trim().chars().take(4_000).collect(),
        title: title.or_else(|| (req.op == "podcast").then(|| req.prompt.split_whitespace().take(8).collect::<Vec<_>>().join(" "))),
        mime: mime.into(),
        seconds,
        lyrics: Some(words),
        chat_id: req.chat_id.clone(),
        ..Default::default()
    };
    ctx.state.log("media", &format!("Read {} with {} voice(s): {total} lines, {}", if req.op == "podcast" { "a podcast" } else { "a script" }, speakers.len(), super::clock(seconds)));
    Ok(vec![store::save(&ctx.state.db.lock().unwrap(), &ctx.state.paths, &c, item, &bytes, None)?])
}

/// The sound as an MP3 (WAV where there's no MP3 encoder).
#[cfg(windows)]
fn encode(l: &[f32], r: &[f32], work: &Path) -> Result<(Vec<u8>, &'static str), String> {
    let pcm: Vec<i16> = l.iter().zip(r).flat_map(|(a, b)| [*a, *b]).map(|s| (s.clamp(-1.0, 1.0) * 32767.0) as i16).collect();
    let path = work.join("talk.mp3");
    super::mp4::mp3(&path, &pcm, RATE)?;
    Ok((std::fs::read(&path).map_err(|e| e.to_string())?, "audio/mpeg"))
}

#[cfg(not(windows))]
fn encode(l: &[f32], r: &[f32], _: &Path) -> Result<(Vec<u8>, &'static str), String> {
    Ok((own_voice::wav_stereo(l, r, RATE), "audio/wav"))
}

/// Checks a podcast or reading before it's queued.
pub fn check(state: &AppState, req: &Request) -> Result<(), String> {
    let spec = req.talk.clone().unwrap_or_default();
    if spec.speakers.len() > MAX_SPEAKERS {
        return Err(format!("Up to {MAX_SPEAKERS} people can talk in one."));
    }
    let mut names = HashSet::new();
    for s in &spec.speakers {
        let n = s.name.trim().to_lowercase();
        if n.is_empty() {
            return Err("Give everyone a name.".into());
        }
        if !names.insert(n) {
            return Err(format!("Two people are called {}: give them different names.", s.name.trim()));
        }
        if let Some(id) = s.voice.as_deref().and_then(|v| v.strip_prefix("own:")) {
            let v = own_voice::get(&state.db.lock().unwrap(), &state.work_cipher()?, id).ok_or("A voice picked is no longer here.")?;
            if !v.can_speak() {
                return Err(format!("Read at least three lines for {} in Studio › Your voice first.", v.name));
            }
        }
    }
    if req.op == "podcast" {
        if spec.speakers.is_empty() {
            return Err("Add who talks in it.".into());
        }
        if req.prompt.trim().is_empty() && spec.files.is_empty() {
            return Err("Say what the podcast is about, or add files for it to talk about.".into());
        }
    } else if req.prompt.trim().is_empty() {
        return Err("Write the script: one line for each turn, “Name: what they say”.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn people(names: &[&str]) -> Vec<Speaker> {
        names.iter().map(|n| Speaker { name: n.to_string(), ..Default::default() }).collect()
    }

    #[test]
    fn scripts_become_turns() {
        let script = "# Coffee, explained\n\n**Ana:** Welcome back! Today: coffee.\nBen: Mm-hm.\nIt's my favourite. [laughs]\nANA: *sips* So where does it come from?\n\nTitle: not a title here\n";
        let (title, turns) = parse(script, &people(&["Ana", "Ben"]));
        assert_eq!(title.as_deref(), Some("Coffee, explained"));
        assert_eq!(turns.len(), 3, "{turns:?}");
        assert_eq!(turns[0], Turn { speaker: 0, text: "Welcome back! Today: coffee.".into() });
        assert_eq!(turns[1], Turn { speaker: 1, text: "Mm-hm. It's my favourite.".into() }, "a line without a name carries on the turn, directions dropped");
        assert_eq!(turns[2].speaker, 0);
        assert_eq!(turns[2].text, "So where does it come from? Title: not a title here");
        let (title, _) = parse("Title: The Sea\nAna: Hi", &people(&["Ana"]));
        assert_eq!(title.as_deref(), Some("The Sea"));
    }

    #[test]
    fn a_script_names_its_people() {
        assert_eq!(names_in("Title: x\nAna: Hi\n**Ben Lee:** Hey\nAna: again\nnote: 3:30 pm\nDr. Who: yes"), vec!["Ana", "Ben Lee", "note", "Dr. Who"]);
        let spec = Spec::default();
        assert_eq!(speakers_for(&spec, "no names here").len(), 1);
        let spec = Spec { speakers: people(&["Kim", ""]), ..Default::default() };
        assert_eq!(speakers_for(&spec, "Ana: hi"), people(&["Kim"]));
    }

    #[test]
    fn everyone_gets_a_different_voice() {
        let v = |id: &str, gender: &str, engine: &'static str| crate::tts::Voice { id: id.into(), name: id.into(), language: "en-US".into(), gender: gender.into(), engine, languages: vec!["en".into()] };
        let voices = vec![v("supertonic:F1", "female", "natural"), v("supertonic:F2", "female", "natural"), v("supertonic:M1", "male", "natural"), v("system:a", "male", "system")];
        let mut s = people(&["A", "B", "C"]);
        s[1].voice = Some("supertonic:F1".into());
        fill_voices(&mut s, &voices, Some("en"));
        assert_eq!(s[0].voice.as_deref(), Some("supertonic:M1"), "F1 is taken; a man's voice comes next");
        assert_eq!(s[2].voice.as_deref(), Some("supertonic:F2"));
    }

    #[test]
    fn turns_are_spaced_like_people_talking() {
        let tone = |s: f32| (0..(RATE as f32 * s) as usize).map(|i| (i as f32 * 0.05).sin() * 0.2).collect::<Vec<f32>>();
        let turns = vec![Turn { speaker: 0, text: "This is a long thought about coffee.".into() }, Turn { speaker: 1, text: "Right.".into() }, Turn { speaker: 1, text: "And another thing about it.".into() }];
        let voiced = vec![tone(2.0), tone(0.5), tone(1.0)];
        let (l, r) = mix(&turns, &voiced, 2);
        assert_eq!(l.len(), r.len());
        let secs = l.len() as f32 / RATE as f32;
        // 0.3 lead + 2.0 + (0.5 - 0.2 overlap) + 0.45 + 1.0 + 0.6 tail, give or take the trimming.
        assert!((secs - 4.65).abs() < 0.15, "{secs}");
        // The first speaker is on the left, the second on the right.
        let at = |v: &[f32], s: f32| crate::audio::rms(&v[(RATE as f32 * s) as usize..(RATE as f32 * (s + 0.2)) as usize]);
        assert!(at(&l, 1.0) > at(&r, 1.0));
        assert!(at(&r, 3.5) > at(&l, 3.5));
        assert!(l.iter().chain(r.iter()).all(|s| s.abs() <= 0.98));
    }

    #[test]
    fn directions_are_not_read() {
        assert_eq!(spoken("Well *laughs* that's [pause] true_ish"), "Well that's trueish");
        assert_eq!(spoken("5 * 3 = 15"), "5 3 = 15");
        assert!(is_reaction("Wait, really?"));
        assert!(!is_reaction("That's a really good point about it."));
    }
}

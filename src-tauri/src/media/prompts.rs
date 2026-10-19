// SPDX-License-Identifier: AGPL-3.0-only
//! The prompt writer: turns what someone asked for into the description a
//! picture or video model follows best, built the way DSPy builds programs.
//!
//! - A **signature** (`Ask` → `Written`): what goes in (the request, the look,
//!   what example pictures show and what to take from them, the shape, notes
//!   such as "desktop wallpaper") and what comes out (the prompt, and a
//!   negative prompt for the models that take one).
//! - A **program** per model family (`Program`): instructions plus worked
//!   examples (demos). The bundled ones are in `catalog/prompt_programs.json`;
//!   `tune` replaces a family's with one compiled on this PC.
//! - A **metric** (`judge`): the picture reader checks a made picture against
//!   a checklist of what was asked for, item by item.
//! - An **optimizer** (`tune`): for a set of requests it writes several
//!   prompts, makes each picture, scores it, and keeps the best (request →
//!   prompt) pairs as the family's demos (bootstrapped few-shot). It also asks
//!   for better instructions from the checklist items that failed most, and
//!   keeps them when they score higher.
//!
//! The writer is the picture model's own language model where it has one
//! (FLUX.2: Qwen3, Z-Image: Qwen3 4B, Qwen-Image: Qwen3-VL 8B), so the model
//! that writes the description is the kind that reads it.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{Kind, MediaModel};
use crate::paths::Paths;
use crate::vision::Helper;
use crate::AppState;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Demo {
    pub ask: String,
    pub prompt: String,
    #[serde(default)]
    pub negative: String,
    /// How the picture made from it scored (tuned demos).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tuned {
    /// When, and with which model.
    pub at: i64,
    pub model_id: String,
    /// Average checklist score with the bundled program, then with this one.
    pub baseline: f32,
    pub score: f32,
    pub pictures: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Program {
    pub family: String,
    pub instructions: String,
    /// What to avoid, for models that take a negative prompt (empty: none).
    #[serde(default)]
    pub negative: String,
    #[serde(default)]
    pub max_words: u32,
    #[serde(default)]
    pub demos: Vec<Demo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tuned: Option<Tuned>,
}

const BUNDLED: &str = include_str!("../../../catalog/prompt_programs.json");

/// The family a picture or video model belongs to, which decides how its prompts are written.
pub fn family(m: &MediaModel) -> &'static str {
    // A model from Hugging Face: the family of the one it's built on (one
    // installed before that was kept names it in its description).
    let id = match (&m.built_on, &m.from_hub) {
        (Some(b), _) => b.to_lowercase(),
        (None, Some(_)) => m.description.split("built on ").nth(1).unwrap_or(&m.id).to_lowercase(),
        _ => m.id.to_lowercase(),
    };
    let hub = m.hub.as_ref().map(|h| h.name.to_lowercase()).unwrap_or_default();
    let any = |words: &[&str]| words.iter().any(|w| id.contains(w) || hub.contains(w));
    if m.kind == Kind::Video {
        "wan"
    } else if any(&["flux"]) {
        "flux2"
    } else if any(&["qwen-image", "qwen image"]) {
        "qwen-image"
    } else if any(&["z-image", "z_image"]) {
        "z-image"
    } else if any(&["1.5", "sd15", "sd-1-5", "realistic-vision"]) {
        "sd15"
    } else if any(&["xl", "sdxl", "realvis"]) {
        "sdxl"
    } else {
        "general"
    }
}

fn tuned_path(paths: &Paths, family: &str) -> PathBuf {
    paths.data.join("prompt_programs").join(format!("{family}.json"))
}

/// The bundled program for a family.
pub fn bundled(family: &str) -> Program {
    let all: serde_json::Map<String, Value> = serde_json::from_str(BUNDLED).unwrap_or_default();
    all.get(family).or_else(|| all.get("general")).and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or(Program {
        family: family.into(),
        instructions: "You write prompts for a picture model: one clear, concrete description of what was asked for.".into(),
        negative: String::new(),
        max_words: 120,
        demos: Vec::new(),
        tuned: None,
    })
}

/// A family's program: the one tuned on this PC, else the bundled one.
pub fn program(paths: &Paths, family: &str) -> Program {
    std::fs::read_to_string(tuned_path(paths, family)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_else(|| bundled(family))
}

fn save(paths: &Paths, p: &Program) -> Result<(), String> {
    let path = tuned_path(paths, &p.family);
    std::fs::create_dir_all(path.parent().unwrap_or(&paths.data)).map_err(|e| e.to_string())?;
    std::fs::write(path, serde_json::to_string_pretty(p).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

/// Goes back to the bundled program for a family.
pub fn forget_tuning(paths: &Paths, family: &str) {
    let _ = std::fs::remove_file(tuned_path(paths, family));
}

/// The signature's inputs.
#[derive(Debug, Clone, Default)]
pub struct Ask {
    pub request: String,
    /// A look id from `STYLES`, or empty/"auto".
    pub look: String,
    /// "landscape (wider than tall)" and the like.
    pub shape: String,
    /// What each example picture shows (from the picture reader).
    pub examples: Vec<String>,
    /// What to take from them: "style", "subject" or "layout".
    pub example_use: String,
    /// Anything else the prompt must serve ("desktop wallpaper…").
    pub notes: String,
    /// Example pictures go to the model itself as references (image 1, 2…).
    pub refs_to_model: usize,
}

impl Ask {
    fn render(&self) -> String {
        let mut s = format!("What they want: {}", self.request.trim());
        let look = match self.look.as_str() {
            "" | "auto" => "photo unless they named one".to_string(),
            other => super::STYLES.iter().find(|(id, _)| *id == other).map_or(other.replace('_', " "), |(id, words)| format!("{} ({words})", id.replace('_', " "))),
        };
        s.push_str(&format!("\nLook: {look}"));
        if !self.shape.is_empty() {
            s.push_str(&format!("\nShape: {}", self.shape));
        }
        if !self.examples.is_empty() {
            let list = self.examples.iter().enumerate().map(|(i, e)| format!("{}) {e}", i + 1)).collect::<Vec<_>>().join(" ");
            let use_for = match self.example_use.as_str() {
                "subject" => "the subject only: put it into the scene asked for",
                "layout" => "the layout and composition only",
                _ => "the style and look only (medium, colors, light, mood); what's in the picture comes from the request, not from them",
            };
            s.push_str(&format!("\nExample pictures: {list} (use them for: {use_for})"));
            if self.refs_to_model > 0 {
                s.push_str(&format!("\nThe picture model also gets the example pictures themselves as image 1 to image {}.", self.refs_to_model));
            }
        }
        if !self.notes.is_empty() {
            s.push_str(&format!("\nNotes: {}", self.notes.trim()));
        }
        s
    }
}

/// The signature's outputs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Written {
    pub prompt: String,
    pub negative: String,
}

fn schema() -> Value {
    json!({ "type": "object", "properties": { "prompt": { "type": "string" }, "negative": { "type": "string" } }, "required": ["prompt", "negative"] })
}

/// The messages that run the program on one request: instructions, the demos as worked turns, then the request.
pub fn messages(p: &Program, ask: &Ask) -> Vec<Value> {
    let negative_rule = if p.negative.is_empty() {
        "This model doesn't use a negative prompt: leave \"negative\" empty and describe only what should be there (\"without X\" tends to bring X)."
    } else {
        "Also write \"negative\": a comma-separated list of what to avoid for this request (start from the usual list and add anything specific)."
    };
    let max = if p.max_words > 0 { p.max_words } else { 120 };
    let mut out = vec![json!({ "role": "system", "content": format!(
        "{}\n\nKeep counts exact: \"a keeper\" is one person, so say \"a single\" or \"one\" when it matters, and add no people or animals that weren't asked for.\n\nReply with only JSON: {{\"prompt\": \"…\", \"negative\": \"…\"}}. {negative_rule} The prompt is at most {max} words.",
        p.instructions
    ) })];
    for d in p.demos.iter().take(4) {
        out.push(json!({ "role": "user", "content": d.ask }));
        out.push(json!({ "role": "assistant", "content": json!({ "prompt": d.prompt, "negative": d.negative }).to_string() }));
    }
    out.push(json!({ "role": "user", "content": ask.render() }));
    out
}

fn cut_words(s: &str, max: u32) -> String {
    let words: Vec<&str> = s.split_whitespace().collect();
    if max == 0 || words.len() <= (max as usize) * 13 / 10 {
        return words.join(" ");
    }
    words[..(max as usize) * 13 / 10].join(" ")
}

/// Runs the program: the prompt (and negative) for one request.
pub async fn write(h: &Helper, p: &Program, ask: &Ask, temperature: f32) -> Result<Written, String> {
    let v = h.ask_json(messages(p, ask), schema(), 900, temperature).await?;
    let prompt = v["prompt"].as_str().map(str::trim).unwrap_or_default();
    if prompt.split_whitespace().count() < 3 {
        return Err("The prompt writer gave back an empty description.".into());
    }
    let negative = v["negative"].as_str().map(str::trim).unwrap_or_default();
    Ok(Written {
        prompt: cut_words(prompt, p.max_words),
        negative: if p.negative.is_empty() { String::new() } else if negative.is_empty() { p.negative.clone() } else { negative.to_string() },
    })
}

/// Rewrites a prompt whose picture missed things the request asked for.
pub async fn revise(h: &Helper, p: &Program, ask: &Ask, before: &Written, missing: &[String]) -> Result<Written, String> {
    let mut msgs = messages(p, ask);
    msgs.push(json!({ "role": "assistant", "content": json!({ "prompt": before.prompt, "negative": before.negative }).to_string() }));
    msgs.push(json!({ "role": "user", "content": format!(
        "The picture made from that prompt was missing: {}. Rewrite the prompt so each of these comes through clearly: name each one early and \
         plainly, with its count, color and position, and drop minor details that compete with them. Reply with only the JSON.",
        missing.join("; ")
    ) }));
    let v = h.ask_json(msgs, schema(), 900, 0.4).await?;
    let prompt = v["prompt"].as_str().map(str::trim).unwrap_or_default();
    if prompt.split_whitespace().count() < 3 {
        return Ok(before.clone());
    }
    Ok(Written { prompt: cut_words(prompt, p.max_words), negative: v["negative"].as_str().map(str::trim).filter(|n| !n.is_empty()).map_or(before.negative.clone(), str::to_string) })
}

/// The things a request asks to see, as short checkable items ("3 red apples",
/// the text "OPEN" on the sign, "sunset light"), at most 10.
pub async fn checklist(h: &Helper, request: &str) -> Vec<String> {
    let msgs = vec![
        json!({ "role": "system", "content": "You turn a request for a picture into a checklist of what must be visible in it. Each item is short, concrete and checkable by looking: an object or person with its key color, count or position, an action, exact text in quotes, the setting, the light or the style. Only what the request itself says, nothing added or inferred: a short request gets 2 to 5 items, a detailed one up to 10. Include a count when one is given or implied (\"a dog\" is one dog). Most important first." }),
        json!({ "role": "user", "content": request }),
    ];
    let schema = json!({ "type": "object", "properties": { "items": { "type": "array", "items": { "type": "string" }, "maxItems": 10 } }, "required": ["items"] });
    match h.ask_json(msgs, schema, 400, 0.1).await {
        Ok(v) => v["items"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).take(10).map(str::to_string).collect(),
        Err(_) => Vec::new(),
    }
}

/// How well a picture shows its checklist: the share of items shown, and the ones missing.
#[derive(Debug, Clone, Default)]
pub struct Verdict {
    pub score: f32,
    pub missing: Vec<String>,
}

/// The metric: the picture reader checks each item.
pub async fn judge(reader: &Helper, img: &image::DynamicImage, items: &[String]) -> Result<Verdict, String> {
    if items.is_empty() {
        return Ok(Verdict { score: 1.0, missing: Vec::new() });
    }
    if !reader.sees {
        return Err(format!("{} can't see pictures.", reader.name));
    }
    let list = items.iter().enumerate().map(|(i, x)| format!("{}. {x}", i + 1)).collect::<Vec<_>>().join("\n");
    let msgs = vec![
        json!({ "role": "system", "content": "You check pictures strictly against a list. An item counts as shown only if it is clearly visible as described (right count, color, text spelled exactly, position). When unsure, it isn't shown." }),
        json!({ "role": "user", "content": [
            { "type": "text", "text": format!("Does the picture show each item?\n{list}\nReply with JSON: {{\"shown\": [true or false for each item, in order]}}.") },
            { "type": "image_url", "image_url": { "url": crate::vision::data_url(img)? } },
        ] }),
    ];
    let schema = json!({ "type": "object", "properties": { "shown": { "type": "array", "items": { "type": "boolean" } } }, "required": ["shown"] });
    let v = reader.ask_json(msgs, schema, 300, 0.0).await?;
    let shown: Vec<bool> = v["shown"].as_array().into_iter().flatten().map(|b| b.as_bool().unwrap_or(false)).collect();
    let missing: Vec<String> = items.iter().enumerate().filter(|(i, _)| !shown.get(*i).copied().unwrap_or(false)).map(|(_, x)| x.clone()).collect();
    Ok(Verdict { score: 1.0 - missing.len() as f32 / items.len() as f32, missing })
}

/// An edit, planned by a model that sees the picture: the steps for the
/// edit model, what to check afterwards, and the planner's own list of
/// changes (to show it when planning again).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EditPlan {
    pub steps: Vec<String>,
    pub checklist: Vec<String>,
    pub changes: Value,
    /// The main word of each thing that moves ("barbell"): there must be one of each after.
    pub moved: Vec<String>,
    /// How many people the edit takes away.
    pub people_gone: usize,
}

/// Words for a person, to count the people an edit takes away.
const PEOPLE: &[&str] = &["adult", "woman", "man", "person", "girl", "boy", "child", "baby", "toddler", "kid", "lady", "guy", "people", "mother", "father", "mom", "dad", "parent", "teenager", "men", "women", "player", "worker"];

/// What the planner knows about FLUX.2 klein's edits. Asked to have a child
/// lift the barbell in front of her overhead, it drew a second barbell
/// above her and left the first on the floor, with fewer plates or another
/// kind. Asked to plan steps itself, the planner (Qwen3.5 9B) wrote the
/// request back as one step; asked to sort changes into kinds, it called
/// the lift a pose. So it answers two plain questions: what will no longer
/// be where it is now, and how the changed part looks after. The steps are
/// built from that: whatever leaves goes first, then the finished scene.
pub(crate) const EDIT_PLANNER: &str = "You plan a change to a picture for an edit model. The edit model redraws the picture from a plain description. \
But asked to move something, or to have someone lift or pick it up, it draws a second copy and leaves the first where it was, and it \
redraws moved things smaller or with different parts. So you answer two questions, and the instructions are built from your answers.\n\n\
Look at the picture.\n\
- \"leaving\": every person or thing that will no longer be where it is now. That is whatever the request removes, and also whatever \
it has someone lift, pick up, hold up, carry or move: it leaves its place now and is drawn again where it goes. For each: \"what\", \
the thing alone described exactly as it looks (size, colors, its parts and how many of each, any writing on it), not what it rests \
on or what's near it; \"where_now\"; and \"left_behind\": what shows in that place once it's gone (the floor, wall or room behind it).\n\
- \"result\": the changed part of the finished picture in one or two sentences: who and what, where, and the pose (where the arms, \
hands and legs are), with anything from \"leaving\" that ends up somewhere else described exactly as in \"what\". Empty when the \
request only removes things.\n\
- \"checklist\": 3 to 8 short things to check by looking at the finished picture, including what must be gone (\"no …\").\n\
Name people and things by how they look, never \"she\", \"he\" or \"it\". Only what the request asks for: add no ideas of your own. \
Reply with only JSON.";

/// A worked example for the planner (a request and its plan, without the picture).
fn plan_demo() -> [Value; 2] {
    [
        json!({ "role": "user", "content": "The request: have the man pick up the guitar and play it, and take the dog out. \
            (The picture: a man with a grey beard on a wooden stool; a brown acoustic guitar with a black strap leaning on the white wall to the right; a small white dog lying on a striped rug.)" }),
        json!({ "role": "assistant", "content": json!({
            "leaving": [
                { "what": "the small white dog", "where_now": "lying on the striped rug", "left_behind": "the striped rug" },
                { "what": "the brown acoustic guitar with a black strap", "where_now": "leaning on the white wall to the right", "left_behind": "the plain white wall and the wooden floor" }
            ],
            "result": "The man with the grey beard sits on the wooden stool playing the brown acoustic guitar with the black strap, his left hand on its neck and his right hand over the strings.",
            "checklist": ["no dog in the picture", "no guitar left leaning on the wall", "the man holds the guitar and plays it", "the guitar is brown with a black strap"]
        }).to_string() }),
    ]
}

fn plan_schema() -> Value {
    let text = json!({ "type": "string" });
    json!({ "type": "object", "properties": {
        "leaving": { "type": "array", "maxItems": 4, "items": { "type": "object", "properties": { "what": text, "where_now": text, "left_behind": text },
            "required": ["what", "where_now", "left_behind"] } },
        "result": text,
        "checklist": { "type": "array", "items": { "type": "string" }, "maxItems": 8 } }, "required": ["leaving", "result", "checklist"] })
}

/// "adult woman" for "the adult woman": a thing's description without its article.
fn bare(what: &str) -> &str {
    let low = what.to_lowercase();
    ["the ", "a ", "an "].iter().find(|a| low.starts_with(*a)).and_then(|a| what.get(a.len()..)).unwrap_or(what)
}

/// The main word of a description: "barbell" for "the long silver barbell with three black plates".
fn head_word(what: &str) -> String {
    let low = bare(what).to_lowercase();
    let cut = [" with ", " in ", " on ", " wearing ", " holding ", " that ", " which ", ",", " of "].iter().filter_map(|w| low.find(w)).min().unwrap_or(low.len());
    low[..cut].split_whitespace().last().unwrap_or_default().trim_matches(|c: char| !c.is_alphanumeric()).to_string()
}

/// The thing a request has someone lift, pick up, hold up, carry or move,
/// by its main word ("barbell" in "she is lifting the barbell overhead").
pub fn moved_thing(request: &str) -> Option<String> {
    let low = request.to_lowercase();
    let words: Vec<&str> = low.split(|c: char| !c.is_alphanumeric() && c != '\'').filter(|w| !w.is_empty()).collect();
    const VERBS: &[&str] = &["lift", "lifts", "lifting", "lifted", "pick", "picks", "picking", "hold", "holds", "holding", "carry", "carries", "carrying", "raise", "raises", "raising", "move", "moves", "moving", "grab", "grabs", "grabbing", "hoist", "hoisting", "press", "pressing"];
    const SKIP: &[&str] = &["up", "the", "her", "his", "their", "its", "a", "an", "that", "this", "it", "them"];
    let at = words.iter().position(|w| VERBS.contains(w))?;
    let thing = words[at + 1..].iter().take(4).find(|w| !SKIP.contains(w) && w.len() > 2)?;
    // "lifting her arms" moves nothing.
    (!["arm", "arms", "hand", "hands", "head", "leg", "legs", "foot", "feet"].contains(thing)).then(|| thing.to_string())
}

/// A description kept short: its first sentence, at most `words` words,
/// with "A …"/"The …" in lower case so it reads inside a sentence.
fn short(s: &str, words: usize) -> String {
    let s = s.trim();
    let first = [". ", "; ", "\n"].iter().filter_map(|d| s.find(d)).min().map_or(s, |i| &s[..i]);
    let mut out = first.split_whitespace().take(words).collect::<Vec<_>>().join(" ");
    out = out.trim_end_matches(['.', ',', ';', ':', ' ']).to_string();
    for (big, small) in [("A ", "a "), ("An ", "an "), ("The ", "the ")] {
        if let Some(rest) = out.strip_prefix(big) {
            out = format!("{small}{rest}");
        }
    }
    out
}

/// The plan built from the planner's answers: a step for each thing leaving
/// its place (saying what shows there instead), then one with how the
/// changed part looks after, each ending by keeping the rest. The checklist starts with what must be gone. None when nothing
/// usable came back.
pub fn parse_plan(v: &Value) -> Option<EditPlan> {
    let text = |c: &Value, k: &str, words: usize| short(c[k].as_str().unwrap_or_default(), words);
    let result = cut_words(v["result"].as_str().unwrap_or_default().trim().trim_end_matches(['.', ' ']), 80);
    let result_low = result.to_lowercase();
    let (mut gone, mut checks, mut moved, mut people_gone) = (Vec::new(), Vec::new(), Vec::new(), 0);
    // Where things were taken from, so the planner's own "no …" items about them aren't checked twice.
    let mut emptied: Vec<String> = Vec::new();
    for c in v["leaving"].as_array().into_iter().flatten().take(4) {
        let (what, now, left) = (text(c, "what", 30), text(c, "where_now", 15), text(c, "left_behind", 15));
        if what.is_empty() {
            continue;
        }
        let place = if now.is_empty() { String::new() } else { format!(" {now}") };
        let fill = if left.is_empty() { "fill the space so it matches its surroundings".to_string() } else { format!("show {left} there instead") };
        gone.push(format!("Remove {what}{place}, and {fill}."));
        if !now.is_empty() {
            emptied.push(now.to_lowercase());
        }
        // Drawn again somewhere else: it mustn't stay here too.
        let head = head_word(&what);
        let moves = head.len() > 2 && result_low.contains(&head);
        if moves {
            moved.push(head.clone());
        } else if PEOPLE.contains(&head.as_str()) {
            people_gone += 1;
        }
        checks.push(if moves { format!("no {} left{place}", bare(&what)) } else { format!("no {}{place}", bare(&what)) });
    }
    let then = if result.split_whitespace().count() >= 3 { vec![format!("{result}.")] } else { Vec::new() };
    let keep = "Keep everything else as it is.";
    // One removal per step: given two in one instruction, klein made the
    // first and ignored the second.
    let steps: Vec<String> = gone.into_iter().map(|g| vec![g]).chain([then]).filter(|s| !s.is_empty()).map(|s| cut_words(&format!("{} {keep}", s.join(" ")), 140)).collect();
    if steps.is_empty() {
        return None;
    }
    for x in v["checklist"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::trim) {
        let low = x.to_lowercase();
        let again = checks.iter().any(|c: &String| c.eq_ignore_ascii_case(x)) || (low.starts_with("no ") && emptied.iter().any(|p| low.ends_with(p.as_str())));
        if x.split_whitespace().count() >= 2 && !again {
            checks.push(x.to_string());
        }
    }
    checks.truncate(8);
    Some(EditPlan { steps, checklist: checks, changes: v.clone(), moved, people_gone })
}

/// When the request moves something the planner didn't list as leaving its
/// place, the reader is asked where that thing is now, and it's added.
async fn add_moved_thing(h: &Helper, img: &image::DynamicImage, request: &str, mut v: Value) -> Value {
    let Some(thing) = moved_thing(request) else { return v };
    let listed = v["leaving"].as_array().into_iter().flatten().any(|c| c["what"].as_str().is_some_and(|w| w.to_lowercase().contains(&thing)));
    if listed {
        return v;
    }
    let Ok(url) = crate::vision::data_url(img) else { return v };
    let msgs = vec![
        json!({ "role": "system", "content": "You describe things in a picture exactly as they look. Reply with only JSON." }),
        json!({ "role": "user", "content": [
            { "type": "text", "text": format!(
                "Is there a {thing} in this picture? If there is: \"what\": the {thing} itself exactly as it looks (size, colors, its parts and how \
                 many of each, any writing on it), in one sentence of at most 25 words, nothing about people or other things; \"where_now\": where \
                 it is, in a few words; \"left_behind\": what would show in that place if it were taken away, in a few words (the floor, wall or room behind it)."
            ) },
            { "type": "image_url", "image_url": { "url": url } },
        ] }),
    ];
    let text = |max: u32| json!({ "type": "string", "maxLength": max });
    let schema = json!({ "type": "object", "properties": { "there": { "type": "boolean" }, "what": text(200), "where_now": text(100), "left_behind": text(100) }, "required": ["there", "what", "where_now", "left_behind"] });
    let Ok(a) = h.ask_json(msgs, schema, 400, 0.1).await else { return v };
    if a["there"].as_bool() != Some(true) || a["what"].as_str().is_none_or(|w| w.trim().is_empty()) {
        return v;
    }
    let looks = short(a["what"].as_str().unwrap_or_default(), 30);
    let what = if looks.to_lowercase().contains(&thing) { looks.clone() } else { format!("the {thing}, {looks}") };
    // The result describes it the planner's way; this says how it really looks.
    let result = v["result"].as_str().unwrap_or_default().trim().trim_end_matches('.').to_string();
    if !result.is_empty() {
        v["result"] = json!(format!("{result}. The {thing} looks exactly as before: {looks}."));
    }
    let entry = json!({ "what": what, "where_now": short(a["where_now"].as_str().unwrap_or_default(), 15), "left_behind": short(a["left_behind"].as_str().unwrap_or_default(), 15) });
    match v["leaving"].as_array_mut() {
        Some(list) => list.push(entry),
        None => v["leaving"] = json!([entry]),
    }
    v
}

/// Plans an edit, looking at the picture.
pub async fn plan_edit(h: &Helper, img: &image::DynamicImage, request: &str) -> Result<EditPlan, String> {
    if !h.sees {
        return Err(format!("{} can't see pictures.", h.name));
    }
    let mut msgs = vec![json!({ "role": "system", "content": EDIT_PLANNER })];
    msgs.extend(plan_demo());
    msgs.push(json!({ "role": "user", "content": [
        { "type": "text", "text": format!("The request: {}", request.trim()) },
        { "type": "image_url", "image_url": { "url": crate::vision::data_url(img)? } },
    ] }));
    let v = h.ask_json(msgs, plan_schema(), 1200, 0.2).await?;
    let v = add_moved_thing(h, img, request, v).await;
    parse_plan(&v).ok_or_else(|| "The planner gave back no changes.".to_string())
}

/// Plans an edit again after its result fell short: the planner sees the
/// picture before, the result, and what was wrong with it. The checklist
/// stays the first one, so both results are scored the same way.
pub async fn replan_edit(h: &Helper, before: &image::DynamicImage, after: &image::DynamicImage, request: &str, plan: &EditPlan, wrong: &[String]) -> Result<EditPlan, String> {
    let mut msgs = vec![json!({ "role": "system", "content": EDIT_PLANNER })];
    msgs.extend(plan_demo());
    msgs.push(json!({ "role": "user", "content": [
        { "type": "text", "text": format!("The request: {}", request.trim()) },
        { "type": "image_url", "image_url": { "url": crate::vision::data_url(before)? } },
    ] }));
    msgs.push(json!({ "role": "assistant", "content": plan.changes.to_string() }));
    msgs.push(json!({ "role": "user", "content": [
        { "type": "text", "text": format!(
            "The edit model made this picture from that, and it fell short: {}. List the changes again so each of these comes out right: \
             describe more exactly what must go and what shows there instead, where each thing ends up, and how it looks. Reply with only the JSON.",
            wrong.join("; ")
        ) },
        { "type": "image_url", "image_url": { "url": crate::vision::data_url(after)? } },
    ] }));
    let v = h.ask_json(msgs, plan_schema(), 1200, 0.4).await?;
    let v = add_moved_thing(h, before, request, v).await;
    let again = parse_plan(&v).ok_or_else(|| "The planner gave back no changes.".to_string())?;
    Ok(EditPlan { checklist: plan.checklist.clone(), ..again })
}

/// A checklist item as a yes-or-no question about the finished picture, and
/// the answer it should get: "no barbell left on the floor" asks whether
/// there's a barbell on the floor (it should be no).
pub fn item_question(item: &str) -> (String, bool) {
    let t = item.trim().trim_end_matches('.');
    match t.get(..3).filter(|p| p.eq_ignore_ascii_case("no ")) {
        Some(_) => (format!("Is there any {} in this picture?", t[3..].trim()), false),
        None => (format!("Is this true of this picture: \"{t}\"?"), true),
    }
}

/// One question about one picture, answered in JSON. The picture comes
/// first, so the engine reuses it from one question to the next.
async fn ask_about(reader: &Helper, url: &str, question: &str, schema: Value) -> Result<Value, String> {
    let msgs = vec![
        json!({ "role": "system", "content": "You look at a picture and answer a question about it strictly and literally. When unsure, answer no. Reply with only JSON." }),
        json!({ "role": "user", "content": [
            { "type": "image_url", "image_url": { "url": url } },
            { "type": "text", "text": question },
        ] }),
    ];
    reader.ask_json(msgs, schema, 60, 0.0).await
}

fn count_schema() -> Value {
    json!({ "type": "object", "properties": { "count": { "type": "integer", "minimum": 0, "maximum": 30 } }, "required": ["count"] })
}

/// The metric for an edit. Asked to compare two pictures in one go, the
/// reader (Qwen3.5 9B) passed an edit that left the barbell on the floor
/// and added two people. So it answers one plain question at a time about
/// the finished picture: each checklist item, how many of each moved thing
/// there are (one, not two), and how many people there are against the
/// picture before (none added).
pub async fn judge_edit(reader: &Helper, before: &image::DynamicImage, after: &image::DynamicImage, plan: &EditPlan) -> Result<Verdict, String> {
    if !reader.sees {
        return Err(format!("{} can't see pictures.", reader.name));
    }
    let after_url = crate::vision::data_url(after)?;
    let yes_no = json!({ "type": "object", "properties": { "answer": { "type": "string", "enum": ["yes", "no"] } }, "required": ["answer"] });
    let (mut missing, mut total) = (Vec::new(), 0usize);
    for item in &plan.checklist {
        let (q, want) = item_question(item);
        let v = ask_about(reader, &after_url, &format!("{q} Answer yes or no."), yes_no.clone()).await?;
        total += 1;
        if (v["answer"].as_str() == Some("yes")) != want {
            missing.push(item.clone());
        }
    }
    for thing in &plan.moved {
        let v = ask_about(reader, &after_url, &format!("How many of these can you see in this picture: {thing}? Count each one, even one partly hidden."), count_schema()).await?;
        total += 1;
        let n = v["count"].as_u64().unwrap_or(0);
        if n > 1 {
            missing.push(format!("only one {thing}, not {n}"));
        }
    }
    let people = "How many people can you see in this picture? Count every person, even one shown only in part (just legs, arms or a hand).";
    let before_url = crate::vision::data_url(before)?;
    let n0 = ask_about(reader, &before_url, people, count_schema()).await?["count"].as_u64().unwrap_or(0);
    let n1 = ask_about(reader, &after_url, people, count_schema()).await?["count"].as_u64().unwrap_or(0);
    total += 1;
    let expected = n0.saturating_sub(plan.people_gone as u64);
    if n1 > expected {
        missing.push(format!("no people added ({n1} people where there should be {expected})"));
    }
    Ok(Verdict { score: 1.0 - missing.len() as f32 / total.max(1) as f32, missing })
}

/// The language model that writes prompts for `m`: its own (the text reader
/// it ships with), else a vision model on this PC, else the chat model.
pub async fn writer(state: &Arc<AppState>, m: &MediaModel) -> Result<Helper, String> {
    if let Some(llm) = super::installed_file(state, m, "llm") {
        let vision = super::installed_file(state, m, "llm_vision");
        return Helper::start(state, &format!("{}'s language model", m.name), &llm, vision.as_deref()).await;
    }
    if let Some(r) = crate::vision::readers(state).into_iter().next() {
        return Helper::start(state, &r.name, &r.model, Some(&r.mmproj)).await;
    }
    Helper::chat_model(state, false).await
}

/// A model that can look at pictures (to read examples and check results):
/// the picture model's own when it sees, else the picture reader.
pub async fn reader(state: &Arc<AppState>, m: Option<&MediaModel>) -> Result<Helper, String> {
    if let Some(m) = m {
        if let (Some(llm), Some(vision)) = (super::installed_file(state, m, "llm"), super::installed_file(state, m, "llm_vision")) {
            return Helper::start(state, &format!("{}'s language model", m.name), &llm, Some(&vision)).await;
        }
    }
    if let Some(r) = crate::vision::readers(state).into_iter().next() {
        return Helper::start(state, &r.name, &r.model, Some(&r.mmproj)).await;
    }
    if crate::vision::chat_sees(state, None) {
        return Helper::chat_model(state, true).await;
    }
    Err(crate::vision::NO_READER.into())
}

/// What a job's description became before the picture model ran.
pub struct Prepared {
    /// The writer's description (None: the request's own is used).
    pub written: Option<Written>,
    /// What to check the picture for (empty: no check).
    pub checklist: Vec<String>,
    pub ask: Ask,
    pub program: Program,
}

/// "landscape (wider than tall)" for a request's shape or size.
fn shape_words(req: &super::Request) -> String {
    let (width, height) = match &req.wallpaper {
        Some(w) => (Some(w.width), Some(w.height)),
        None => (req.width, req.height),
    };
    let named = match (width, height) {
        (Some(w), Some(h)) if w * 10 > h * 18 => "wide (about twice as wide as tall)",
        (Some(w), Some(h)) if w > h => "landscape (wider than tall)",
        (Some(w), Some(h)) if h > w => "portrait (taller than wide)",
        (Some(_), Some(_)) => "square",
        _ => match req.shape.as_deref().unwrap_or("square") {
            "landscape" => "landscape (wider than tall)",
            "portrait" => "portrait (taller than wide)",
            "wide" => "wide (about twice as wide as tall)",
            "tall" => "tall (about twice as tall as wide)",
            _ => "square",
        },
    };
    named.to_string()
}

/// Before a new picture or clip: reads the example pictures, writes the
/// description for this model, and lists what to check. None when there's
/// nothing to do (no writing asked for, nothing to check).
pub async fn prepare(state: &Arc<AppState>, req: &super::Request, m: &MediaModel, progress: &(dyn Fn(&str, f64) + Sync)) -> Result<Option<Prepared>, String> {
    let fam = family(m);
    let picture = req.op == "generate";
    let write_it = req.write_prompt.unwrap_or(false) || !req.examples.is_empty() || matches!(fam, "sd15" | "sdxl");
    // Checking needs something that sees pictures.
    let check_it = picture && req.check.unwrap_or(false) && (crate::vision::can_read(state, None) || (super::installed_file(state, m, "llm_vision").is_some()));
    if !write_it && !check_it {
        return Ok(None);
    }
    let c = state.work_cipher()?;
    let examples: Vec<image::DynamicImage> = req
        .examples
        .iter()
        .take(4)
        .filter_map(|id| super::store::read(&state.paths, &c, id).ok())
        .filter_map(|b| super::imaging::decode(&b).ok())
        .collect();
    let mut words = Vec::new();
    if !examples.is_empty() {
        progress("Looking at your example pictures", 0.0);
        match reader(state, Some(m)).await {
            Ok(r) => {
                for img in &examples {
                    if let Ok(d) = crate::vision::describe_with(&r, img, crate::vision::as_example(req.example_use.as_deref().unwrap_or("style")), 260).await {
                        words.push(d);
                    }
                }
                r.done(state).await;
            }
            Err(e) => crate::applog::line("media", &format!("Couldn't read the example pictures: {e}")),
        }
    }
    progress("Writing the description for the picture model", 0.0);
    let program = program(&state.paths, fam);
    let ask = Ask {
        request: req.prompt.clone(),
        look: req.style.clone().unwrap_or_default(),
        shape: shape_words(req),
        examples: words,
        example_use: req.example_use.clone().unwrap_or_else(|| "style".into()),
        notes: [req.notes.clone().unwrap_or_default(), req.wallpaper.as_ref().map(|w| super::wallpaper::notes(w, req.op == "video")).unwrap_or_default()].join(" ").trim().to_string(),
        refs_to_model: if picture && m.can("edit") && req.example_use.as_deref() != Some("layout") { examples.len() } else { 0 },
    };
    let w = writer(state, m).await?;
    let written = if write_it {
        match write(&w, &program, &ask, 0.4).await {
            Ok(x) => Some(x),
            Err(e) => {
                crate::applog::line("media", &format!("The prompt writer failed ({e}); using the description as written"));
                None
            }
        }
    } else {
        None
    };
    let checklist = if check_it { checklist(&w, &req.prompt).await } else { Vec::new() };
    w.done(state).await;
    if let Some(x) = &written {
        crate::applog::line("media", &format!("Wrote a {fam} description of {} words; {} things to check", x.prompt.split_whitespace().count(), checklist.len()));
    }
    Ok(Some(Prepared { written, checklist, ask, program }))
}

/// Requests the optimizer practices on: varied, each with things that are
/// easy to get wrong (counts, positions, text, light).
pub(crate) const PRACTICE: &[(&str, &str, &str)] = &[
    ("three red apples and one green pear in a wooden bowl on a white kitchen table, morning light from the left", "photo", "landscape (wider than tall)"),
    ("a bakery storefront with a striped awning and a sign that says \"SUNRISE BAKERY\"", "photo", "landscape (wider than tall)"),
    ("an astronaut riding a horse across a desert at sunset, two moons in the sky", "cinematic", "landscape (wider than tall)"),
    ("a cozy cabin interior with a fireplace on the left, a sleeping dog on a rug, snow falling outside the window", "photo", "landscape (wider than tall)"),
    ("a girl in a yellow raincoat holding a blue umbrella, splashing in a puddle, city street", "cartoon", "portrait (taller than wide)"),
    ("a chessboard mid-game with a white queen and a black knight in front, shallow depth of field", "photo", "square"),
    ("a futuristic city skyline at night with flying cars and a giant hologram of a whale", "digital_art", "landscape (wider than tall)"),
    ("a birthday cake with five lit candles and the words \"HAPPY 5TH\" in blue icing", "photo", "square"),
];

#[derive(Debug, Clone, Serialize)]
pub struct TuneReport {
    pub family: String,
    pub baseline: f32,
    pub score: f32,
    pub pictures: usize,
    pub demos: usize,
    /// The instructions were replaced by better-scoring ones.
    pub new_instructions: bool,
}

/// One practice picture, made without keeping it in the gallery.
pub type Render<'a> = &'a (dyn Fn(String, String, u64) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<image::DynamicImage, String>> + Send + 'a>> + Sync);

/// The optimizer. Writes `tries` prompts per practice request (the first at
/// low temperature with the current program: the baseline), makes each
/// picture with `render`, scores it against the request's checklist, and
/// saves the family's program with the best-scoring pairs as its demos.
/// Then it asks for better instructions from what failed most, scores those
/// on a few requests, and keeps them if they do better.
pub async fn tune(state: &Arc<AppState>, m: &MediaModel, tries: usize, render: Render<'_>, progress: &(dyn Fn(&str, f64) + Sync), cancel: &AtomicBool) -> Result<TuneReport, String> {
    if m.kind != Kind::Image {
        return Err("Prompt tuning works with picture models.".into());
    }
    let fam = family(m);
    let mut prog = program(&state.paths, fam);
    let base = bundled(fam);
    let tries = tries.clamp(1, 4);
    let cancelled = || cancel.load(Ordering::Relaxed);
    let stop = || -> Result<TuneReport, String> { Err(crate::download::CANCELLED.into()) };

    // 1. Checklists and candidate prompts, with the writer loaded once.
    progress("Writing practice prompts", 0.02);
    let w = writer(state, m).await?;
    let mut cases: Vec<(Ask, Vec<String>, Vec<Written>)> = Vec::new();
    for (i, (request, look, shape)) in PRACTICE.iter().enumerate() {
        if cancelled() {
            w.done(state).await;
            return stop();
        }
        let ask = Ask { request: request.to_string(), look: look.to_string(), shape: shape.to_string(), ..Default::default() };
        let items = checklist(&w, request).await;
        let mut written = Vec::new();
        for t in 0..tries {
            // The first is the current program at its steadiest: the baseline.
            let program = if t == 0 { &base } else { &prog };
            if let Ok(x) = write(&w, program, &ask, if t == 0 { 0.2 } else { 0.9 }).await {
                written.push(x);
            }
        }
        cases.push((ask, items, written));
        progress("Writing practice prompts", 0.02 + 0.15 * (i + 1) as f64 / PRACTICE.len() as f64);
    }
    w.done(state).await;

    // 2. The pictures.
    let total: usize = cases.iter().map(|c| c.2.len()).sum();
    let mut pictures: Vec<Vec<image::DynamicImage>> = Vec::new();
    let mut made = 0;
    for (ask, _, written) in &cases {
        let mut row = Vec::new();
        for (k, x) in written.iter().enumerate() {
            if cancelled() {
                return stop();
            }
            progress(&format!("Making practice picture {} of {total}", made + 1), 0.17 + 0.6 * made as f64 / total.max(1) as f64);
            let seed = 1000 + k as u64;
            match render(x.prompt.clone(), x.negative.clone(), seed).await {
                Ok(img) => row.push(img),
                Err(e) if e == crate::download::CANCELLED => return stop(),
                Err(e) => {
                    crate::applog::line("media", &format!("A practice picture failed: {e}"));
                    row.push(image::DynamicImage::new_rgb8(8, 8));
                }
            }
            made += 1;
        }
        let _ = ask;
        pictures.push(row);
    }

    // 3. The scores.
    progress("Checking the practice pictures", 0.8);
    let r = reader(state, Some(m)).await?;
    let mut scored: Vec<(usize, usize, f32, Vec<String>)> = Vec::new();
    for (ci, (_, items, written)) in cases.iter().enumerate() {
        for (k, _) in written.iter().enumerate() {
            if cancelled() {
                r.done(state).await;
                return stop();
            }
            let img = &pictures[ci][k];
            let v = if img.width() < 64 { Verdict { score: 0.0, missing: items.clone() } } else { judge(&r, img, items).await.unwrap_or_default() };
            scored.push((ci, k, v.score, v.missing));
        }
    }
    r.done(state).await;

    let baseline = {
        let firsts: Vec<f32> = scored.iter().filter(|s| s.1 == 0).map(|s| s.2).collect();
        firsts.iter().sum::<f32>() / firsts.len().max(1) as f32
    };
    // The best prompt per request, then the best requests as demos.
    let mut best: Vec<(usize, usize, f32)> = Vec::new();
    for ci in 0..cases.len() {
        if let Some(b) = scored.iter().filter(|s| s.0 == ci).max_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal)) {
            best.push((b.0, b.1, b.2));
        }
    }
    let score = best.iter().map(|b| b.2).sum::<f32>() / best.len().max(1) as f32;
    best.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
    let mut demos: Vec<Demo> = best
        .iter()
        .filter(|b| b.2 >= 0.6)
        .take(4)
        .map(|b| {
            let (ask, _, written) = &cases[b.0];
            Demo { ask: ask.render(), prompt: written[b.1].prompt.clone(), negative: written[b.1].negative.clone(), score: Some(b.2) }
        })
        .collect();
    // Too few good ones: keep the bundled examples alongside.
    for d in &base.demos {
        if demos.len() >= 4 {
            break;
        }
        demos.push(d.clone());
    }

    // 4. Better instructions, from what was missed most.
    let mut missed: Vec<String> = scored.iter().flat_map(|s| s.3.clone()).collect();
    missed.sort();
    missed.dedup();
    let mut new_instructions = false;
    if !missed.is_empty() && !cancelled() {
        progress("Looking for better instructions", 0.9);
        let w = writer(state, m).await?;
        let msgs = vec![
            json!({ "role": "system", "content": "You improve the instructions a prompt writer follows for a picture model. Keep what works; add short, specific rules that would have prevented the misses. Same length or a little longer, plain sentences." }),
            json!({ "role": "user", "content": format!(
                "Current instructions:\n{}\n\nPictures made from prompts written this way missed: {}.\nReply with JSON: {{\"instructions\": \"…\"}}.",
                prog.instructions,
                missed.iter().take(20).cloned().collect::<Vec<_>>().join("; ")
            ) }),
        ];
        let schema = json!({ "type": "object", "properties": { "instructions": { "type": "string" } }, "required": ["instructions"] });
        let proposal = w.ask_json(msgs, schema, 700, 0.5).await.ok().and_then(|v| v["instructions"].as_str().map(str::to_string)).filter(|s| s.split_whitespace().count() > 30);
        // Try them on the three requests that did worst.
        let mut worst: Vec<usize> = (0..cases.len()).collect();
        worst.sort_by(|a, b| {
            let s = |ci: usize| best.iter().find(|x| x.0 == ci).map_or(0.0, |x| x.2);
            s(*a).partial_cmp(&s(*b)).unwrap_or(std::cmp::Ordering::Equal)
        });
        worst.truncate(3);
        let mut trial: Vec<(usize, Written)> = Vec::new();
        if let Some(text) = &proposal {
            let candidate = Program { instructions: text.clone(), demos: demos.clone(), ..prog.clone() };
            for ci in &worst {
                if let Ok(x) = write(&w, &candidate, &cases[*ci].0, 0.3).await {
                    trial.push((*ci, x));
                }
            }
        }
        w.done(state).await;
        if let (Some(text), false) = (&proposal, trial.is_empty()) {
            let mut imgs = Vec::new();
            for (ci, x) in &trial {
                if cancelled() {
                    return stop();
                }
                if let Ok(img) = render(x.prompt.clone(), x.negative.clone(), 1000).await {
                    imgs.push((*ci, img));
                }
            }
            let r = reader(state, Some(m)).await?;
            let mut new_total = 0.0;
            let mut old_total = 0.0;
            for (ci, img) in &imgs {
                new_total += judge(&r, img, &cases[*ci].1).await.map_or(0.0, |v| v.score);
                old_total += best.iter().find(|x| x.0 == *ci).map_or(0.0, |x| x.2);
            }
            r.done(state).await;
            if !imgs.is_empty() && new_total > old_total {
                prog.instructions = text.clone();
                new_instructions = true;
            }
        }
    }

    prog.demos = demos;
    prog.tuned = Some(Tuned { at: crate::db::now_ms(), model_id: m.id.clone(), baseline, score, pictures: made });
    save(&state.paths, &prog)?;
    progress("Done", 1.0);
    crate::applog::line("media", &format!("Tuned the {fam} prompt writer with {}: checklist score {:.0}% → {:.0}% over {made} pictures", m.name, baseline * 100.0, score * 100.0));
    Ok(TuneReport { family: fam.into(), baseline, score, pictures: made, demos: prog.demos.len(), new_instructions })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_family_has_a_bundled_program() {
        for fam in ["flux2", "qwen-image", "z-image", "sdxl", "sd15", "wan", "general"] {
            let p = bundled(fam);
            assert_eq!(p.family, fam);
            assert!(p.instructions.len() > 100, "{fam}");
            assert!(p.max_words > 0, "{fam}");
        }
        assert!(bundled("sd15").negative.contains("deformed"));
        assert!(bundled("flux2").negative.is_empty(), "FLUX.2 runs at cfg 1: no negative prompt");
        assert_eq!(bundled("nonsense").family, "general");
    }

    #[test]
    fn the_signature_renders_what_the_writer_needs() {
        let ask = Ask {
            request: "my dog as a knight".into(),
            look: "oil_painting".into(),
            shape: "square".into(),
            examples: vec!["a golden retriever on grass".into()],
            example_use: "subject".into(),
            notes: "desktop wallpaper".into(),
            refs_to_model: 1,
        };
        let r = ask.render();
        assert!(r.contains("What they want: my dog as a knight"));
        assert!(r.contains("Look: oil painting (oil painting on canvas"));
        assert!(r.contains("1) a golden retriever on grass (use them for: the subject only"));
        let style = Ask { example_use: "style".into(), ..ask.clone() }.render();
        assert!(style.contains("not from them"), "a style example lends its look, not its content");
        assert!(r.contains("image 1 to image 1"));
        assert!(r.contains("Notes: desktop wallpaper"));
        let msgs = messages(&bundled("flux2"), &ask);
        assert_eq!(msgs.len(), 2 + 2 * bundled("flux2").demos.len().min(4));
        assert!(msgs[0]["content"].as_str().unwrap().contains("leave \"negative\" empty"));
        assert!(messages(&bundled("sd15"), &ask)[0]["content"].as_str().unwrap().contains("Also write \"negative\""));
    }

    #[test]
    fn a_move_is_planned_as_taken_away_then_put_in_its_new_place() {
        let v = json!({
            "leaving": [
                { "what": "the woman in a black sweatshirt", "where_now": "standing behind the girl", "left_behind": "the weight bench and the black floor mats" },
                { "what": "the long silver barbell with three black plates on each end", "where_now": "on the floor in front of the girl", "left_behind": "the black rubber floor mat" },
                { "what": "", "where_now": "x", "left_behind": "y" }
            ],
            "result": "The little girl in the red dress kneels and holds the long silver barbell with three black plates on each end above her head, arms straight up.",
            "checklist": ["No barbell left on the floor in front of the girl", "both arms straight up", "x"]
        });
        let p = parse_plan(&v).unwrap();
        assert_eq!(p.steps.len(), 3, "each thing that goes on its own, then how the changed part looks");
        assert_eq!(p.steps[0], "Remove the woman in a black sweatshirt standing behind the girl, and show the weight bench and the black floor mats there instead. Keep everything else as it is.");
        assert_eq!(p.steps[1], "Remove the long silver barbell with three black plates on each end on the floor in front of the girl, and show the black rubber floor mat there instead. Keep everything else as it is.");
        assert!(p.steps[2].starts_with("The little girl in the red dress kneels and holds the long silver barbell"));
        assert!(p.steps[2].ends_with("arms straight up. Keep everything else as it is."));
        assert_eq!(
            p.checklist,
            vec!["no woman in a black sweatshirt standing behind the girl", "no long silver barbell with three black plates on each end left on the floor in front of the girl", "both arms straight up"],
            "what must be gone comes first, once; the barbell is drawn again, so it mustn't be *left* there"
        );
        // Only taking things away: one step.
        let v = json!({ "leaving": [{ "what": "the red car", "where_now": "in the driveway", "left_behind": "the grey driveway" }], "result": "", "checklist": [] });
        assert_eq!(parse_plan(&v).unwrap().steps, vec!["Remove the red car in the driveway, and show the grey driveway there instead. Keep everything else as it is."]);
        assert!(parse_plan(&json!({ "leaving": [], "result": "", "checklist": ["a b"] })).is_none());
        assert_eq!(bare("The red cup"), "red cup");
        assert_eq!(bare("Anna"), "Anna");
        assert_eq!(head_word("the long silver barbell with three black plates"), "barbell");
        assert_eq!(head_word("an adult woman, standing"), "woman");
    }

    #[test]
    fn what_a_request_moves_is_found_by_its_verb() {
        let ask = "Change the baby's pose so she is lifting the barbell overhead in a clean motion, with her arms extended upward.";
        assert_eq!(moved_thing(ask).as_deref(), Some("barbell"));
        assert_eq!(moved_thing("have him pick up the red guitar").as_deref(), Some("red"), "the first word after the verb; the reader is asked about it");
        assert_eq!(moved_thing("make her raise her arms").as_deref(), None, "arms aren't things in the picture to move");
        assert_eq!(moved_thing("make it night").as_deref(), None);
    }

    #[test]
    fn checks_become_plain_questions_about_the_finished_picture() {
        assert_eq!(item_question("No barbell left on the floor in front of the girl"), ("Is there any barbell left on the floor in front of the girl in this picture?".into(), false));
        assert_eq!(item_question("both arms straight up."), ("Is this true of this picture: \"both arms straight up\"?".into(), true));
        assert_eq!(item_question("north light").1, true, "only \"no \" with its space is an absence");
        // The plan knows what moves (one of each after) and how many people go.
        let v = json!({
            "leaving": [
                { "what": "the adult woman in black", "where_now": "behind the girl", "left_behind": "the bench" },
                { "what": "the silver barbell with black plates", "where_now": "on the floor", "left_behind": "the mat" }
            ],
            "result": "The girl holds the silver barbell above her head.",
            "checklist": []
        });
        let p = parse_plan(&v).unwrap();
        assert_eq!(p.moved, vec!["barbell"]);
        assert_eq!(p.people_gone, 1);
    }

    #[test]
    fn long_prompts_are_cut_to_the_family_limit() {
        let long = vec!["word"; 200].join(" ");
        assert_eq!(cut_words(&long, 45).split_whitespace().count(), 58);
        assert_eq!(cut_words("a red apple", 45), "a red apple");
    }
}

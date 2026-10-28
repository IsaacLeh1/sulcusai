// SPDX-License-Identifier: AGPL-3.0-only
//! Reading what the person asked and what the model answered: pictures,
//! songs, names, building, stalling, and the notes that push it on.

use super::*;

pub(super) static ABOUT_THEIR_THINGS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b(e-?mails?|mail|inbox|messages?|calendar|events?|schedule|meetings?|appointments?|party|birthday|today|tonight|tomorrow|this week|next week|notes?|tasks?|to-?dos?|reminders?|weather|forecast|near me|nearby|nearest|closest|around here)\b").unwrap()
});
// "I don't have access to your messages": wrong whenever tools were offered,
// whatever the question.
pub(super) static NO_ACCESS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)(don'?t|do not|can'?t|cannot) (have )?(direct |any |real-time |personal )?access|(unable|not able) to (access|read|check|see)|i (can'?t|cannot) (read|check|see) your|i don'?t have (any )?(information|details) about|without you providing|i don'?t have any (email|calendar|accounts?)").unwrap()
});
pub(super) static STALLING: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)i'?ll need to|i need to|i will need|i would need|please (provide|confirm|specify)|(could|can) you (please )?(provide|confirm|specify|tell me|share|give me)|(any|more|specific) (details|criteria|information)|function call|<tool|(i'?ll|i will|let me) (search|look|check)|(don'?t|do not|can'?t|cannot) (have )?access|(unable|not able) to (access|read|check|see)|i (can'?t|cannot) (read|check|see)").unwrap()
});

// "I'm not familiar with that", "as of my knowledge cutoff": it doesn't know
// what was asked about, which a web search would tell it.
pub(super) static NOT_KNOWN: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b(i'?m|i am) (not |un)familiar\b|\bnot (familiar|aware) with\b|\b(i'?m|i am) not (sure|certain) (what|who|which) .{0,40}\b(is|are|was|were|refers? to|means?)\b|\bi (don'?t|do not) (know|recognize|recognise) (what|who) .{0,40}\b(is|are|was|were|refers? to|means?)\b|\bi (don'?t|do not) (know|recognize|recognise) (of |anything about )|\bi (don'?t|do not) have (any )?(information|details|knowledge) (on|about|of) (that|this|it|the)\b|\b(as of|beyond|after|past) my (last )?(knowledge|training) (cutoff|cut-off|date|update)|\bmy (knowledge|training data) (only goes|doesn'?t|does not|ends|is limited)|\bi couldn'?t find (any )?information|\b(could|can) you (clarify|explain|tell me) (what|who) .{0,40}\b(is|are|means?)\b").unwrap()
});

/// The reply says it doesn't know what (or who) the user means.
pub fn doesnt_know(reply: &str) -> bool {
    let reply = reply.trim().replace(['\u{2019}', '\u{2018}'], "'");
    let opening: String = reply.chars().take(600).collect();
    NOT_KNOWN.is_match(&opening)
}

// A sum to work out ("what's 37*48", "15% of 2,345", "1250 divided by 7",
// "how many days until Christmas"), which small models get wrong in their heads.
// Dates (10/17, 2026-10-17) and sizes (3x3) aren't sums.
pub(super) static MATH_ASK: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(
        r"(?i)\d\s*[*×÷^]\s*\(?\d|\d\s+[-+/]\s+\(?\d|\d\s*\+\s*\d|\b\d[\d,.]*\s*(plus|minus|times|divided by|multiplied by|to the power of)\s+\d|\b\d+(\.\d+)?\s*(%|percent)\s+of\s+\$?\d|\b(square root|cube root|sqrt) of\s+\d|\bhow many (days|weeks|months|hours|minutes|seconds) (until|till|between|since|from|are (there )?in|left)\b|\b(compound interest|monthly payment|mortgage payment|loan payment)\b|\btip on \$?\d|\bsplit \$?\d[\d,.]* (\w+ )?(ways|between)\b|\bsolve\b[^.?!\n]*=|\b(integral|integrate|derivative|differentiate)\b|\b[a-z]\s*\^\s*\d",
    )
    .unwrap()
});

/// The message asks for something to be worked out (and isn't a request to build or write code).
pub fn asks_math(text: &str) -> bool {
    let t: String = text.chars().take(600).collect();
    MATH_ASK.is_match(&t) && !t.contains("```") && !is_build_ask(&[t.as_str()])
}

// Asking for a picture to be made.
pub(super) static PICTURE_ASK:std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b(make|create|draw|generate|paint|render|design|show me|give me|i want|can you (make|draw|create))\b[\s\S]{0,60}\b(picture|image|photo|drawing|painting|illustration|artwork|art|wallpaper|logo|portrait|sketch|pixel art|poster)\b|\b(picture|image|photo|drawing|painting) of\b|\b(draw|paint|sketch|illustrate)\s+(me\s+)?(a|an|the|some|my|our)\b").unwrap()
});
// Saying it can't make pictures (often right after making one).
pub(super) static NO_PICTURES: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b(can'?t|cannot|can not|unable to|not able to|don'?t have the ability to)\b[^.!?\n]{0,30}\b(generate|create|make|draw|produce|display|show|edit|modify|change|alter)\b[^.!?\n]{0,20}\b(images?|pictures?|photos?|drawings?|art)").unwrap()
});

/// More than one picture (or clip, or song) asked for in one message.
pub fn wants_several(text: &str) -> bool {
    static SEVERAL: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)\b(pictures|images|photos|drawings|versions|variations|options|several|a few|some|both|each|every|all of|series|set of|[2-9]|two|three|four|five)\b").unwrap()
    });
    SEVERAL.is_match(text)
}

/// The person asked for a picture to be made.
pub fn asks_for_picture(text: &str) -> bool {
    PICTURE_ASK.is_match(&own_words(text))
}

/// The part of a message that's the person's own words rather than text they
/// pasted to be worked on: all of a short message, else its first and last
/// lines (where "Refine these lyrics:" or "Make them rhyme" go), section tags
/// left out. Lyrics full of imagery ("paint the sky", "picture of us") read as
/// a picture request, and "write… the page" as a site to build.
pub fn own_words(text: &str) -> String {
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty() && !(l.starts_with('[') && l.ends_with(']'))).collect();
    if lines.len() <= 3 {
        return lines.join("\n");
    }
    format!("{}\n{}", lines[0], lines[lines.len() - 1])
}

pub(super) static EDIT_WORDS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b(refine|rewrite|re-write|revise|edit|improve|polish|tweak|proofread|shorten|tighten|reword|rephrase|clean (it|them|this|these) up|fix (the|my|these|this|its|their|the \w+ of)\s*\w*\s*(rhythm|rhymes?|flow|lines?|lyrics|words|grammar|meter|wording|verse|chorus)|make (it|them|this|these|the \w+)(\s+\w+)?\s+(better|flow|rhyme|catchier|smoother|stronger|tighter|shorter|longer|sadder|happier|deeper|darker)|rhyme better|feedback on|critique|help me (with|improve|finish|fix) (my|these|this|the)\b)").unwrap()
});
// Lyrics, a poem, a text: what the edit is about.
pub(super) static WORDS_OBJECT: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b(lyrics?|lines?|verses?|chorus(es)?|bridge|hook|poems?|poetry|words|text|rhymes?|stanzas?|sentences?|paragraphs?|essay|story)\b").unwrap()
});
// Asking to hear it, see it, or keep it in a file, beside the edit.
pub(super) static MORE_THAN_WORDS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b(sing|sung|record|produce|compose|melody|beat|instrumental|audio|mp3|music|listen|hear|picture|image|photo|draw|illustrate|artwork|cover art|album cover|video|file|folder|document|desktop|save)\b|\.[a-z0-9]{2,4}\b|\b(make|turn|create|generate)\b[^.?!\n]{0,30}\b(song|track|tune)\b").unwrap()
});

/// The person wants their own words edited (lyrics, a poem, a text pasted in
/// or written earlier in the chat): refine, rewrite, make them rhyme, fix the
/// rhythm. Answered in words: not a song, a picture or a file (each happened).
/// Not when the same ask wants it sung, drawn or saved too.
pub fn edits_text(text: &str) -> bool {
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    // The lines that ask: in a long message, its first two and last two.
    let ends: Vec<&str> = if lines.len() <= 4 { lines.clone() } else { [&lines[..2], &lines[lines.len() - 2..]].concat() };
    let asking: Vec<&str> = ends.into_iter().filter(|l| EDIT_WORDS.is_match(l)).collect();
    if asking.is_empty() || asking.iter().any(|l| MORE_THAN_WORDS.is_match(l)) {
        return false;
    }
    let pasted = lines.len() >= 3 || text.contains('[') || text.contains('"') || text.contains('\u{201c}');
    pasted || asking.iter().any(|l| WORDS_OBJECT.is_match(l))
}

// Asked to write a song's words: "write me a song", "can you write a rap
// about my dog", "write lyrics for a love song".
pub(super) static WRITE_SONG: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b(write|writing|wright|pen|come up with)\b[^.?!\n]{0,40}\b(songs?|lyrics?|raps?|verses?|chorus(es)?|hooks?|jingles?|ballads?|anthems?|poems?)\b").unwrap()
});

pub(super) static ONLY_WORDS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"(?i)\b(lyrics?|words|poems?|verses?)\b").unwrap());

/// Asked to write a song: just its words ("write lyrics for…"), or not said
/// whether they want the words or the song to hear ("write me a song").
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SongAsk {
    Words,
    Unclear,
}

/// The person asked for a song written. Not the song made (that's "make me
/// a song", or asking to hear it): asked to write one, the song itself was
/// made too, which went past what they asked. Words asked for are written;
/// otherwise they're asked which they want first.
pub fn writes_song(text: &str) -> Option<SongAsk> {
    if !WRITE_SONG.is_match(text) || MORE_THAN_WORDS.is_match(text) {
        return None;
    }
    Some(if ONLY_WORDS.is_match(text) { SongAsk::Words } else { SongAsk::Unclear })
}

/// `writes_song` for a request and the person's asks before it (newest
/// first): answering "just the lyrics" to the question a song ask brought is
/// words too.
pub fn writes_song_in(request: &str, earlier: &[&str]) -> Option<SongAsk> {
    writes_song(request).or_else(|| {
        let answered = earlier.iter().take(2).any(|a| writes_song(a) == Some(SongAsk::Unclear));
        (answered && ONLY_WORDS.is_match(request) && !MORE_THAN_WORDS.is_match(request) && !COMPLETE_SONG.is_match(request)).then_some(SongAsk::Words)
    })
}

// "The complete song", "the whole thing", "both": the song made, not only its words.
pub(super) static COMPLETE_SONG: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"(?i)\b(complete|whole|full|finished|both)\b").unwrap());

/// The reply says it can't make pictures.
pub fn denies_pictures(reply: &str) -> bool {
    NO_PICTURES.is_match(&reply.replace(['\u{2019}', '\u{2018}'], "'"))
}

// A change to an attached picture ("add a hat", "make me a cartoon", "remove the car").
pub(super) static PICTURE_CHANGE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b(add|put|give)\s+(a|an|some|the|me|him|her|them|us|it)\b|\b(remove|erase|replace)\s+(the|my|his|her|their|that|this)\b|\b(make|turn)\s+(it|this|me|him|her|them|us|the (photo|picture|image|background|sky))\b|\bchange\s+(the|my|his|her|their)\s+\w+|\b(restyle|edit (this|the|my) (photo|picture|image)|in the style of|as an? (cartoon|painting|drawing|anime))\b").unwrap()
});

/// The person asked for a change to a picture (with one attached).
pub fn asks_picture_change(text: &str) -> bool {
    PICTURE_CHANGE.is_match(text)
}

/// A named thing in a picture request (a character, a game, a brand, a
/// person, a place) and what the model says it looks like.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct Named {
    pub name: String,
    #[serde(default)]
    pub known: bool,
    #[serde(default)]
    pub looks: String,
}

impl Named {
    /// Sure of its look, with enough words to draw it.
    pub fn described(&self) -> bool {
        self.known && self.looks.split_whitespace().count() >= 4
    }
}

/// Asks the model which things in a picture request have a particular look
/// that picture models won't know by name, and what each looks like.
pub fn names_check(asked: &str, prompt: &str) -> (String, Value) {
    let ask = format!(
        "A picture is about to be made.\nWhat the user asked: \u{ab}{asked}\u{bb}\nThe description for the picture model: \u{ab}{prompt}\u{bb}\n\n\
         Picture models don't know names: they only draw what the words describe. List each specific named thing above that \
         has its own particular look: a character, a game, film, show or book, a brand, product or logo, a real person, a famous \
         place, building or artwork. Leave out ordinary things (a dog, a castle, a sword, a sunset). For each one: known is true \
         only if you are sure exactly what it looks like, and looks is that look in one line (colors, clothes, hair, shapes, \
         distinctive marks), or empty if you aren't sure. An empty list is fine. Reply with only the JSON."
    );
    let schema = json!({ "type": "object", "properties": { "names": { "type": "array", "maxItems": 6, "items": {
        "type": "object",
        "properties": { "name": { "type": "string" }, "known": { "type": "boolean" }, "looks": { "type": "string" } },
        "required": ["name", "known", "looks"] } } }, "required": ["names"] });
    (ask, json!({ "type": "json_schema", "json_schema": { "name": "names", "schema": schema } }))
}

/// The named things from a names check's reply (empty if it isn't JSON).
pub fn names_of(reply: &str) -> Vec<Named> {
    #[derive(serde::Deserialize)]
    struct Reply {
        #[serde(default)]
        names: Vec<Named>,
    }
    let all: Vec<Named> = serde_json::from_str::<Reply>(reply.trim()).map(|r| r.names).unwrap_or_default();
    let mut names: Vec<Named> = Vec::new();
    for n in all {
        if !n.name.trim().is_empty() && !names.iter().any(|k| k.name.trim().eq_ignore_ascii_case(n.name.trim())) {
            names.push(n);
        }
    }
    names.truncate(6);
    names
}

/// Only real names: capitalized as written in what the user asked or in the
/// description, and not an everyday word. (Asked about "put him at a party",
/// a small model listed "him", and the look it found on the web was HIM the
/// cartoon villain's: lobster claws and red skin on the user's grandfather.)
pub fn real_names(names: Vec<Named>, asked: &str, prompt: &str) -> Vec<Named> {
    const COMMON: &[&str] = &[
        "him", "her", "he", "she", "it", "they", "them", "his", "hers", "their", "me", "my", "you", "your", "we", "us", "our", "i",
        "man", "woman", "boy", "girl", "person", "people", "guy", "kid", "child", "baby", "dog", "cat", "picture", "photo", "image",
        "the man", "the woman", "old man", "old woman", "the picture", "this", "that", "someone", "everyone",
    ];
    names
        .into_iter()
        .filter(|n| {
            let name = n.name.trim();
            let lower = name.to_lowercase();
            name.chars().count() >= 2
                && !COMMON.contains(&lower.as_str())
                && name.chars().any(char::is_uppercase)
                && (asked.contains(name) || prompt.contains(name))
        })
        .collect()
}

/// Asks the model to describe a thing's look from web search results.
pub fn looks_from(name: &str, found: &str) -> (String, Value) {
    let ask = format!(
        "{found}\n\nFrom the search results above, describe how {name} looks, in one line for a picture model: colors, clothes, \
         hair, shapes and distinctive marks. Only what the results say or clearly show; empty if they say nothing about its look. \
         Reply with only the JSON."
    );
    let schema = json!({ "type": "object", "properties": { "looks": { "type": "string" } }, "required": ["looks"] });
    (ask, json!({ "type": "json_schema", "json_schema": { "name": "looks", "schema": schema } }))
}

/// The look from a `looks_from` reply.
pub fn looks_of(reply: &str) -> String {
    serde_json::from_str::<Value>(reply.trim()).ok().and_then(|v| v["looks"].as_str().map(|s| s.trim().to_string())).unwrap_or_default()
}

/// The picture description with each named thing's look spelled out after
/// it, since picture models draw words, not names.
pub fn with_looks(prompt: &str, names: &[Named]) -> String {
    let lower = prompt.to_lowercase();
    let added: Vec<String> = names
        .iter()
        .filter(|n| n.described())
        // Already described in the prompt itself.
        .filter(|n| !lower.contains(&n.looks.to_lowercase()))
        .map(|n| format!("{} looks like this: {}", n.name.trim(), n.looks.trim().trim_end_matches('.')))
        .collect();
    if added.is_empty() {
        return prompt.to_string();
    }
    let p = prompt.trim().trim_end_matches('.');
    format!("{p}. {}.", added.join(". "))
}

/// The reply without its code (fenced blocks and `inline code`).
pub(super) fn without_code(reply: &str) -> String {
    let mut out = String::with_capacity(reply.len());
    for (i, part) in reply.split("```").enumerate() {
        if i % 2 == 0 {
            for (j, bit) in part.split('`').enumerate() {
                if j % 2 == 0 {
                    out.push_str(bit);
                }
            }
        }
    }
    out
}

/// A small model asked the user for details or permission about their own
/// mail, calendar, notes, tasks or weather instead of using a tool to look.
pub fn stalls(user: &str, reply: &str, offered: &[String]) -> bool {
    let reply = reply.trim().replace(['\u{2019}', '\u{2018}'], "'");
    if reply.is_empty() {
        return false;
    }
    // Telling the user to "use email_search" instead of using it. Not in
    // code it wrote (a function of the same name) or as a call there ("name(").
    let prose = without_code(&reply);
    if offered.iter().any(|n| n.contains('_') && prose.match_indices(n.as_str()).any(|(i, _)| !prose[i + n.len()..].starts_with('('))) {
        return true;
    }
    // Saying it has no access is wrong however long the reply goes on.
    let opening: String = reply.chars().take(600).collect();
    // Ending on what it's about to look up, without looking ("Here are the
    // searches I'll run: … Let's execute these searches", the live test's 14B).
    let n = reply.chars().count();
    let ending: String = reply.chars().skip(n.saturating_sub(250)).collect();
    NO_ACCESS.is_match(&opening) || (n < 800 && ABOUT_THEIR_THINGS.is_match(user) && STALLING.is_match(&reply)) || (n < 1500 && ANNOUNCES.is_match(&ending))
}

pub(super) static ANNOUNCES: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b(i'?ll|i will|let me|let'?s|let us|i'?m going to|i am going to)\s+(now\s+)?(perform|run|do|execute|conduct|start|carry out|search|look|check|research|find|gather|fetch|browse)\b[^\n]{0,80}\b(search(es)?|research|look ?ups?|the web|online|sources|information|results|data)\b[^\n]{0,40}[.:!]?\s*$").unwrap()
});

// Asking for something to be made: a site, an app, a script, files.
pub(super) static BUILD_ASK: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b(make|create|build|write|generate|code|program|develop|scaffold|set up|save|put)\b[\s\S]{0,80}\b(files?|folders?|website|web ?site|web ?page|page|site|app|application|script|program|game|project|component|extension|bot|server|api|tool|calculator|html|css|javascript|python|react|template|readme|\.[a-z]{1,4}\b)").unwrap()
});
// Steps for the person to follow instead of doing it.
pub(super) static HANDS_OVER: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b(create (a|an|the) (new )?(file|folder|directory)|save (this|it|the (code|file)) as|copy (and paste |the following|this)|paste (this|it|the following)|open (your|a) (terminal|editor|text editor|command prompt)|step 1\b|here'?s (the|a|some) (code|html|script))").unwrap()
});

// "Do it for me" after asking for something to be made.
pub(super) static DO_IT: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b(do (it|this|that|it all|all (of )?(it|this|that))|try again|go ahead|build it|make it|run it|on my (pc|computer|laptop))\b").unwrap()
});
// Saying it can't make or run things, which it can here.
pub(super) static CANT_DO: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)\bas an ai\b|\bi (can't|cannot|can not|am unable to|'m unable to|am not able to|don't have the ability to|do not have the ability to) (directly )?(execute|run|create|make|build|access|write|set up|save)\b|\b(can't|cannot) (create|write|save) files\b").unwrap()
});
// Asking for a document (Word, PDF, Excel, PowerPoint, Markdown), which create_document makes.
pub(super) static DOCUMENT_ASK: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b(docx?|word (doc|document|file)|pdf|excel|spreadsheet|xlsx|powerpoint|slides?|deck|pptx|report|resume|cv|letter|memo|markdown)\b|\.md\b").unwrap()
});

/// The person asked for something to be made: in their newest message, or
/// earlier with the newest saying "do it" ("do it for me", "try again").
/// `recent_user` is their last few messages, newest first.
pub fn is_build_ask(recent_user: &[&str]) -> bool {
    let Some((newest, earlier)) = recent_user.split_first() else { return false };
    // Their own words, not text pasted in ("write… on the same page" in lyrics).
    let build = |m: &str| BUILD_ASK.is_match(&own_words(m));
    build(newest) || (DO_IT.is_match(&own_words(newest)) && earlier.iter().any(|m| build(m)))
}

/// Asking for a document rather than a site, an app or code.
pub fn wants_a_document(text: &str) -> bool {
    DOCUMENT_ASK.is_match(text)
}

/// A short reply that only asks for details ("What kind of features do you
/// need?") instead of making a first version.
pub(super) fn only_asks(reply: &str) -> bool {
    static ASKS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)\b(what (kind|sort|type)s? of|which (features|kind|type|style|framework|language|pages)|what (features|would you like|do you (want|need|have in mind))|do you (want|need|have|prefer)|would you like|(could|can) you (tell|share|provide|describe|give)|let me know)\b").unwrap()
    });
    let r = reply.trim();
    r.chars().count() < 600 && r.contains('?') && (ASKS.is_match(r) || STALLING.is_match(r))
}

/// A request to make something answered with code, steps, "I can't" or
/// questions in the chat, though tools to make it on the PC were offered.
/// Said to a model that explained steps instead of carrying them out.
pub const CARRY_ON: &str = "Don't explain the steps or ask whether to go ahead: you already have permission. Carry them out now with \
     your tools, one after another, then say briefly what you did and what happened.";

pub const GO_ON: &str = "You stopped partway: you said what you'd do next but didn't do it. Do it now with your tools and keep going \
     until the work is done (run the tests or the check again if there are any), then say briefly what you did.";

/// Told when its reply was all thinking, cut off before it did anything.
pub const THINK_LESS: &str = "Your last reply ran out of room while you were still thinking, so nothing was done. Don't work it all out \
     again: take the next step now with one tool call (or, if the work is done, say so briefly).";

/// Told when it stops while the last command it ran failed.
pub const FIX_FAILING: &str = "The last command you ran failed, and you stopped there. Read what it printed, fix the cause in the files, \
     and run it again; keep going until it works, then say briefly what you did.";

/// Told once when it keeps sending a call that fails.
pub const STEP_BACK: &str = "You've sent the same call more than once and it fails every time, so it won't work this way. Step back: \
     read the whole file once, decide what it should say, and send the complete fixed file with write_file (or go on to the next part of \
     the work).";

/// Told when its reply after looking things up was empty.
pub const ANSWER_NOW: &str = "Your last reply was empty. Answer the user now from what you found (or take the next step with a tool).";

/// Told when it was asked to change something and stopped having changed nothing.
pub const CHANGE_NOW: &str = "You haven't changed anything yet, and the user asked you to. Do it now with your tools (write or edit the files, \
     then run the tests or the check if there are any), and keep going until it's done; then say briefly what you did.";

/// The person asked for something in their files to be changed or made.
pub fn asks_to_change(user: &str) -> bool {
    static CHANGE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)\b(add|fix|implement|change|update|build|create|write|make|refactor|rename|remove|finish|extend)\b").unwrap()
    });
    CHANGE.is_match(user)
}

/// Told when the engine couldn't read its last tool call.
pub const RESEND_CALL: &str = "Your last tool call couldn't be read: its arguments weren't valid JSON (a quote or a line break inside a \
     string wasn't escaped). Send it again as valid JSON. For a long file, write the first half with write_file, then add the rest with \
     edit_file and an empty old_text (that adds it at the end).";

/// Told when it stops with unfinished parts it wrote ("TODO") still there.
pub const FINISH_STUBS: &str = "Some of what you wrote is still unfinished (the note after your change listed the lines, like a TODO). \
     Finish those parts now if they're part of what was asked, check them, then say briefly what's done.";

/// Told when it stops with a file it changed that doesn't parse.
pub const FIX_BROKEN: &str = "A file you changed doesn't parse now (the warning after your edit said where). Read it, fix it, run the \
     tests or the command again, and keep going until it works, then say briefly what you did.";

/// Told once, when a site it built checks out, before it finishes.
pub const CHECK_ASKED: &str = "Before you finish: go through everything the user asked for, one thing at a time (each feature, button and \
     page), and check the files really do it; read them back if you need to. A button that does nothing, or a library that's used but \
     never loaded, counts as missing. Fix or add what's missing, check the site again, then say briefly what's done.";

/// Told once, after longer work on files or commands, before it finishes.
pub const CHECK_WORK: &str = "Before you finish: go through what the user asked for, one thing at a time (each change, feature and test \
     they asked for), and check it's really done; read the files back if you need to, and run the tests or the command again if there \
     are any. Do what's missing, then say briefly what's done.";

/// What `stream` says when a reply was all thinking, even when asked again.
pub const THOUGHT_ONLY: &str = "The model spent its whole reply thinking and never answered. Try again, or turn its thinking off in Advanced settings.";

/// A reply that ends by saying what it does next ("Need to adjust the
/// names. Let's update the uses.") instead of finishing: a 20B model adding
/// a feature stopped like that with a test still failing.
pub fn stops_midway(reply: &str) -> bool {
    static NEXT: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)\b(let's|let me|i'll|i will|i'm going to|i am going to|we'll|we will|we need to|i need to|need to|next,? (i|we)|now (i|we)|then (i|we))\b").unwrap()
    });
    static NOT: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)\b(let me know|if you('d| would)? (like|want)|feel free|happy to|you('ll| will)? need to|you need to|would you like)\b").unwrap()
    });
    let prose = without_code(reply);
    let t = prose.trim();
    // A finished answer ends by asking the user something; a summary is
    // long (the 20B model's unfinished notes ran to 620 characters).
    if t.is_empty() || t.chars().count() > 1_500 || t.ends_with('?') {
        return false;
    }
    // Its last two sentences.
    let sentences: Vec<&str> = t.split_inclusive(['.', '!', '\n']).map(str::trim).filter(|s| !s.is_empty()).collect();
    let tail = sentences[sentences.len().saturating_sub(2)..].join(" ");
    NEXT.is_match(&tail) && !NOT.is_match(&tail)
}

/// The person asked for something to be done (not explained): apply, run,
/// install, follow, set up, fix, make…
pub fn asks_to_act(user: &str) -> bool {
    static ACT: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)\b(apply|applies|do (it|this|that|them|these|the steps)|run|execute|install|set (it |this |them )?up|setup|follow|carry (it |them )?out|implement|fix|change|update|configure|move|copy|rename|delete|download|open|launch|start|make|create|build|find (and|then)|go ahead)\b").unwrap()
    });
    ACT.is_match(user)
}

/// A reply that tells the user how to do it (numbered steps, "you'll need
/// to", "run the following") or asks whether to go ahead, instead of doing it.
pub fn explains_instead(reply: &str) -> bool {
    static NUMBERED: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"(?m)^\s*(\*\*)?(1[.)]|step 1\b)").unwrap());
    static TELLS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)\b(you('ll| will)? (need|have) to|you (can|should|could) (run|open|create|install|copy|paste|type|edit|add|change|go|click|navigate|download|use)|run the following|follow these steps|here('s| is| are) (how|the steps)|steps to|open (a |your )?(terminal|command prompt|powershell|file explorer)|(should|shall|may|can) i (go ahead|proceed|start|run|do|apply|continue)|would you like me to|do you want me to|want me to (go ahead|proceed|run|do|apply)|let me know (if|when) you('d| would)? (like|want))\b").unwrap()
    });
    let prose = without_code(reply);
    let numbered_steps = NUMBERED.is_match(&prose) && prose.lines().filter(|l| l.trim_start().starts_with("2.") || l.trim_start().starts_with("2)")).count() > 0;
    numbered_steps || TELLS.is_match(&prose) || HANDS_OVER.is_match(&prose)
}

pub fn builds_in_chat(recent_user: &[&str], reply: &str, offered: &[String]) -> bool {
    let can_make = offered.iter().any(|n| n == "write_file" || n == "request_folder");
    let code_lines = reply.split("```").skip(1).step_by(2).map(|block| block.lines().count()).sum::<usize>();
    can_make && is_build_ask(recent_user) && (code_lines >= 4 || HANDS_OVER.is_match(reply) || CANT_DO.is_match(reply) || only_asks(reply))
}

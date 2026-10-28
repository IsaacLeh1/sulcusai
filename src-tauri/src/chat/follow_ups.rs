// SPDX-License-Identifier: AGPL-3.0-only
//! One-step follow-ups the loop forces: a lookup, a missing file, a
//! preview, a fix, a calculation.

use super::*;

/// For a model that doesn't know what it was asked about: one call that looks
/// it up on the web (or asks to, when the web is off for this chat).
pub fn forced_web(tools: &Value) -> Option<ForcedLookup> {
    let name_of = |t: &Value| t.pointer("/function/name").and_then(Value::as_str).unwrap_or("").to_string();
    let all = tools.as_array()?;
    let pick = if all.iter().any(|t| name_of(t) == "web_search") { "web_search" } else if all.iter().any(|t| name_of(t) == "request_web") { "request_web" } else { return None };
    let def = all.iter().find(|t| name_of(t) == pick)?;
    let schema = json!({ "type": "object", "properties": { "name": { "const": pick }, "arguments": def["function"]["parameters"] }, "required": ["name", "arguments"] });
    let what = if pick == "web_search" {
        "search the web for it now with web_search (a short query naming it)"
    } else {
        "ask to look it up with request_web (the reason names what you'd look up)"
    };
    Some(ForcedLookup {
        names: vec![pick.into()],
        nudge: format!("(Don't guess and don't ask me to explain it: {what}. Reply with only one tool call as JSON, {{\"name\": \"{pick}\", \"arguments\": {{...}}}}.)"),
        response_format: json!({ "type": "json_schema", "json_schema": { "name": "tool_call", "schema": schema } }),
    })
}

/// For a model that answered a sum from its head: one call that works it out.
pub fn forced_calc(tools: &Value) -> Option<ForcedLookup> {
    let def = tools.as_array()?.iter().find(|t| t.pointer("/function/name").and_then(Value::as_str) == Some("calculate"))?;
    let schema = json!({ "type": "object", "properties": { "name": { "const": "calculate" }, "arguments": def["function"]["parameters"] }, "required": ["name", "arguments"] });
    Some(ForcedLookup {
        names: vec!["calculate".into()],
        nudge: "(Don't work it out in your head: call calculate with the math from my message as one expression, e.g. \"37*48\", \"15% of 2345\", \
                an equation as it is (\"x^2 - 5x + 6 = 0\"), \"integral of x^2 from 0 to 3\", \"derivative of x^3 at 2\" or \
                \"daysBetween('2026-10-17', '2026-12-25')\". Reply with only one tool call as JSON, {\"name\": \"calculate\", \"arguments\": {\"expression\": \"...\"}}.)"
            .into(),
        response_format: json!({ "type": "json_schema", "json_schema": { "name": "tool_call", "schema": schema } }),
    })
}

/// For a model that answered a picture request without making one: one call
/// that makes it, or, for a change to an attached picture, that changes it.
pub fn forced_picture(tools: &Value, change: bool) -> Option<ForcedLookup> {
    let find = |name: &str| tools.as_array()?.iter().find(|t| t.pointer("/function/name").and_then(Value::as_str) == Some(name)).cloned();
    if change {
        if let Some(def) = find("edit_image") {
            let schema = json!({ "type": "object", "properties": { "name": { "const": "edit_image" }, "arguments": def["function"]["parameters"] }, "required": ["name", "arguments"] });
            return Some(ForcedLookup {
                names: vec!["edit_image".into()],
                nudge: "(You can change pictures on this PC: change the picture I attached now with edit_image, with the change as the instruction. Reply with only one tool call as JSON, {\"name\": \"edit_image\", \"arguments\": {...}}.)".into(),
                response_format: json!({ "type": "json_schema", "json_schema": { "name": "tool_call", "schema": schema } }),
            });
        }
    }
    let def = find("create_image")?;
    let schema = json!({ "type": "object", "properties": { "name": { "const": "create_image" }, "arguments": def["function"]["parameters"] }, "required": ["name", "arguments"] });
    Some(ForcedLookup {
        names: vec!["create_image".into()],
        nudge: "(You can make pictures on this PC: make it now with create_image, with a vivid description of what I asked for (spell out how anything named looks). Reply with only one tool call as JSON, {\"name\": \"create_image\", \"arguments\": {...}}.)".into(),
        response_format: json!({ "type": "json_schema", "json_schema": { "name": "tool_call", "schema": schema } }),
    })
}

/// Files a page it wrote uses but that aren't there yet: (the page, the
/// missing file's full path). Only stylesheets and scripts, which it can write.
pub fn missing_pieces(written: &[std::path::PathBuf]) -> Vec<(String, std::path::PathBuf)> {
    static REF: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r#"(?i)(?:href|src)\s*=\s*["']([^"'#?]+\.(?:css|js|mjs))["']"#).unwrap());
    let mut out: Vec<(String, std::path::PathBuf)> = Vec::new();
    for page in written.iter().filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("html") || e.eq_ignore_ascii_case("htm"))) {
        let Ok(text) = std::fs::read_to_string(page) else { continue };
        let Some(dir) = page.parent() else { continue };
        for c in REF.captures_iter(&text) {
            let r = &c[1];
            if r.contains("://") || r.starts_with("//") || r.starts_with('/') || r.contains("..") {
                continue;
            }
            let full = dir.join(r);
            if !full.exists() && !out.iter().any(|(_, p)| *p == full) {
                out.push((page.file_name().unwrap_or_default().to_string_lossy().into_owned(), full));
            }
        }
    }
    out
}

/// One call that writes a file a page needs (its path fixed in the schema).
pub fn forced_write(tools: &Value, page: &str, missing: &std::path::Path) -> Option<ForcedLookup> {
    let def = tools.as_array()?.iter().find(|t| t.pointer("/function/name").and_then(Value::as_str) == Some("write_file"))?;
    let path = missing.display().to_string();
    let mut params = def["function"]["parameters"].clone();
    params["properties"]["path"] = json!({ "type": "string", "const": path });
    let schema = json!({ "type": "object", "properties": { "name": { "const": "write_file" }, "arguments": params }, "required": ["name", "arguments"] });
    let name = missing.file_name().unwrap_or_default().to_string_lossy().into_owned();
    Some(ForcedLookup {
        names: vec!["write_file".into()],
        nudge: format!(
            "({page} uses {name}, which doesn't exist yet, so the page won't work. Write it now with write_file (path {path:?}): its full, working contents to go with {page}. Reply with only one tool call as JSON, {{\"name\": \"write_file\", \"arguments\": {{...}}}}.)"
        ),
        response_format: json!({ "type": "json_schema", "json_schema": { "name": "tool_call", "schema": schema } }),
    })
}

/// The reply has a long block of code in it (more than 12 lines).
pub fn pasted_code(reply: &str) -> bool {
    static FENCE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"(?s)```[^\n]*\n(.*?)(```|$)").unwrap());
    FENCE.captures_iter(reply).any(|c| c[1].lines().count() > 12)
}

/// The file a reply is about, among those written this turn: the one it
/// names ("update `app.js`"), else the newest script, else the newest file.
pub fn named_file(reply: &str, written: &[std::path::PathBuf]) -> Option<std::path::PathBuf> {
    let low = reply.to_lowercase();
    written
        .iter()
        .rev()
        .find(|p| p.file_name().is_some_and(|n| low.contains(&n.to_string_lossy().to_lowercase())))
        .or_else(|| written.iter().rev().find(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("js"))))
        .or_else(|| written.last())
        .cloned()
}

/// For a model that wrote a fix into its reply and previewed the unchanged
/// site again: one call that saves the file, whole.
pub fn forced_save(tools: &Value, file: &std::path::Path) -> Option<ForcedLookup> {
    let mut f = forced_write(tools, "", file)?;
    let name = file.file_name().unwrap_or_default().to_string_lossy().into_owned();
    f.nudge = format!(
        "(The {name} you wrote in your reply was never saved, so the site didn't change. Save it now with write_file (path {:?}): the whole file, with every fix and every feature. Reply with only one tool call as JSON, {{\"name\": \"write_file\", \"arguments\": {{...}}}}.)",
        file.display().to_string()
    );
    Some(f)
}

/// For a model that wrote a site and stopped: one call that previews it (the
/// folder fixed in the schema), which opens it beside the chat and checks it.
pub fn forced_preview(tools: &Value, folder: &str) -> Option<ForcedLookup> {
    let def = tools.as_array()?.iter().find(|t| t.pointer("/function/name").and_then(Value::as_str) == Some("preview_site"))?;
    let mut params = def["function"]["parameters"].clone();
    params["properties"] = json!({ "path": { "type": "string", "const": folder } });
    params["required"] = json!(["path"]);
    let schema = json!({ "type": "object", "properties": { "name": { "const": "preview_site" }, "arguments": params }, "required": ["name", "arguments"] });
    Some(ForcedLookup {
        names: vec!["preview_site".into()],
        nudge: format!(
            "(Before you finish, open the site and check that it works: call preview_site with path {folder:?}. Reply with only one tool call as JSON, {{\"name\": \"preview_site\", \"arguments\": {{\"path\": {folder:?}}}}}.)"
        ),
        response_format: json!({ "type": "json_schema", "json_schema": { "name": "tool_call", "schema": schema } }),
    })
}

/// For a model that stopped with problems the preview found: one call that
/// starts fixing them (it carries on from there by itself).
pub fn forced_fix(tools: &Value, problems: &str) -> Option<ForcedLookup> {
    let name_of = |t: &Value| t.pointer("/function/name").and_then(Value::as_str).unwrap_or("").to_string();
    let list: Vec<&Value> = tools.as_array()?.iter().filter(|t| matches!(name_of(t).as_str(), "edit_file" | "write_file" | "read_file")).collect();
    if list.is_empty() {
        return None;
    }
    let names: Vec<String> = list.iter().map(|t| name_of(t)).collect();
    let schema = json!({ "oneOf": list.iter().map(|t| json!({
        "type": "object",
        "properties": { "name": { "const": name_of(t) }, "arguments": t["function"]["parameters"] },
        "required": ["name", "arguments"],
    })).collect::<Vec<_>>() });
    Some(ForcedLookup {
        names,
        nudge: format!(
            "(The site isn't finished: the preview found these problems.\n{problems}\nDon't stop or tell me it's done: fix them now. \
             Read the file first if you need to, then change it. Reply with only one tool call as JSON, {{\"name\": ..., \"arguments\": {{...}}}}.)"
        ),
        response_format: json!({ "type": "json_schema", "json_schema": { "name": "tool_call", "schema": schema } }),
    })
}

/// A site's stylesheet or script written but no page to load them: the
/// folder its index.html belongs in, and the files it should link.
pub fn page_missing(written: &[std::path::PathBuf]) -> Option<(std::path::PathBuf, Vec<String>)> {
    let ext = |p: &std::path::PathBuf| p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if written.iter().any(|p| matches!(ext(p).as_str(), "html" | "htm")) {
        return None;
    }
    let parts: Vec<&std::path::PathBuf> = written.iter().filter(|p| matches!(ext(p).as_str(), "css" | "js" | "mjs")).collect();
    let first = parts.first()?;
    // A script in js/ or css/ belongs to the folder above.
    let mut dir = first.parent()?.to_path_buf();
    if dir.file_name().is_some_and(|n| matches!(n.to_string_lossy().to_ascii_lowercase().as_str(), "js" | "css" | "scripts" | "styles" | "assets")) {
        dir = dir.parent()?.to_path_buf();
    }
    let index = dir.join("index.html");
    if index.exists() {
        return None;
    }
    let links = parts.iter().filter_map(|p| p.strip_prefix(&dir).ok()).map(|p| p.to_string_lossy().replace('\\', "/")).collect();
    Some((dir, links))
}

/// One call that writes the page a site's stylesheet and script were made for.
pub fn forced_page(tools: &Value, index: &std::path::Path, links: &[String]) -> Option<ForcedLookup> {
    let mut f = forced_write(tools, "index.html", index)?;
    f.nudge = format!(
        "(The site has {} but no page: write index.html now with write_file (path {:?}), the whole page, linking {} and with every part the user asked for. Reply with only one tool call as JSON, {{\"name\": \"write_file\", \"arguments\": {{...}}}}.)",
        links.join(" and "),
        index.display().to_string(),
        links.join(" and ")
    );
    Some(f)
}

/// The folder of the newest web page written (where a site's index.html is).
pub fn site_folder(written: &[std::path::PathBuf]) -> Option<std::path::PathBuf> {
    written
        .iter()
        .rev()
        .find(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("html") || e.eq_ignore_ascii_case("htm")))
        .and_then(|p| p.parent().map(std::path::Path::to_path_buf))
}

/// For a model that pasted what it was asked to make: one call that makes it
/// on the PC (the first file, or asking for a folder to put it in).
pub fn forced_build(tools: &Value) -> Option<ForcedLookup> {
    let name_of = |t: &Value| t.pointer("/function/name").and_then(Value::as_str).unwrap_or("").to_string();
    let all = tools.as_array()?;
    let has = |n: &str| all.iter().any(|t| name_of(t) == n);
    // A file, not just a folder (a small model would stop there).
    let pick: &[&str] = if has("write_file") { &["write_file"] } else if has("request_folder") { &["request_folder"] } else { return None };
    let list: Vec<&Value> = all.iter().filter(|t| pick.contains(&name_of(t).as_str())).collect();
    let names: Vec<String> = list.iter().map(|t| name_of(t)).collect();
    let schema = json!({ "oneOf": list.iter().map(|t| json!({
        "type": "object",
        "properties": { "name": { "const": name_of(t) }, "arguments": t["function"]["parameters"] },
        "required": ["name", "arguments"],
    })).collect::<Vec<_>>() });
    let what = if names[0] == "write_file" {
        "Make a sensible first version on my PC now, choosing the details yourself: write the first file with write_file \
         (its full, working contents, not a sketch; for a website, one index.html with its CSS and JavaScript inside is fine). \
         You'll write any other files next."
    } else {
        "Make it on my PC: first ask for a folder to work in with request_folder."
    };
    Some(ForcedLookup {
        names,
        nudge: format!("(Don't paste the code or give me steps to follow. {what} Reply with only one tool call as JSON, {{\"name\": ..., \"arguments\": {{...}}}}.)"),
        response_format: json!({ "type": "json_schema", "json_schema": { "name": "tool_call", "schema": schema } }),
    })
}

/// For a model that stalled: its look-up tools (most relevant to the question
/// first), what to tell it, and a JSON schema that makes the engine accept
/// nothing but one call to one of them. (`tool_choice: "required"` isn't
/// enforced by llama.cpp for every model; a response schema always is.)
pub struct ForcedLookup {
    pub names: Vec<String>,
    pub nudge: String,
    pub response_format: Value,
}

pub fn forced_lookup(tools: &Value, asked: &str, looks_only: impl Fn(&str) -> bool) -> Option<ForcedLookup> {
    let q = asked.to_lowercase();
    let first: &[(&str, &[&str])] = &[
        ("calendar_events", &["calendar", "event", "party", "meeting", "appointment", "schedule", "when is", "today", "tomorrow", "birthday"]),
        ("email_search", &["email", "mail", "inbox", "message", "sent", "ask", "wrote", "said", "reply"]),
        ("search_notes", &["note"]),
        ("list_tasks", &["task", "to-do", "todo", "remind"]),
        ("weather", &["weather", "forecast", "rain", "temperature"]),
        ("find_places", &["near me", "nearby", "nearest", "closest", "around here", "near "]),
        ("search_meetings", &["meeting notes", "transcript"]),
        ("market_quote", &["stock", "share price", "price of", "crypto", "bitcoin", "exchange rate", "currency"]),
        ("research", &["research", "compare", "better", "best", "versus", " vs", "pros and cons"]),
        ("wikipedia", &["who is", "who was", "what is", "tell me about"]),
    ];
    let rank = |name: &str| first.iter().position(|(n, words)| *n == name && words.iter().any(|w| q.contains(w))).unwrap_or(usize::MAX);
    let name_of = |t: &Value| t.pointer("/function/name").and_then(Value::as_str).unwrap_or("").to_string();
    let mut list: Vec<&Value> = tools.as_array()?.iter().filter(|t| looks_only(&name_of(t))).collect();
    if list.is_empty() {
        return None;
    }
    list.sort_by_key(|t| rank(&name_of(t)));
    // Some tools fit the question: offer only those, so it can't wander off.
    if rank(&name_of(list[0])) != usize::MAX {
        list.retain(|t| rank(&name_of(t)) != usize::MAX);
    }
    let names: Vec<String> = list.iter().map(|t| name_of(t)).collect();
    let listing: Vec<String> = list.iter().map(|t| format!("- {}: {}", name_of(t), t["function"]["description"].as_str().unwrap_or(""))).collect();
    let schema = json!({ "oneOf": list.iter().map(|t| json!({
        "type": "object",
        "properties": { "name": { "const": name_of(t) }, "arguments": t["function"]["parameters"] },
        "required": ["name", "arguments"],
    })).collect::<Vec<_>>() });
    Some(ForcedLookup {
        names,
        nudge: format!(
            "(Don't ask me for details or confirmation, and don't say you can't access it: look it up now. Reply with only one tool call as JSON, {{\"name\": ..., \"arguments\": {{...}}}}, using one of these tools:\n{})",
            listing.join("\n")
        ),
        response_format: json!({ "type": "json_schema", "json_schema": { "name": "tool_call", "schema": schema } }),
    })
}

/// The call a forced lookup replied with.
pub fn forced_call(content: &str, names: &[String]) -> Option<ToolCall> {
    let v: Value = serde_json::from_str(content.trim()).ok()?;
    call_from_value(&v, names)
}

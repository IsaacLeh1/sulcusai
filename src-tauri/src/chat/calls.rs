// SPDX-License-Identifier: AGPL-3.0-only
//! Tool calls a model wrote as text (gpt-oss's Harmony, JSON in the
//! reply) read back into real calls.

use super::*;

pub(super) fn offered_names(tools: Option<&Value>) -> Vec<String> {
    tools.and_then(Value::as_array).into_iter().flatten().filter_map(|t| t.pointer("/function/name").and_then(Value::as_str).map(str::to_string)).collect()
}

/// gpt-oss's built-in tools, as SulcusAI's: it was trained to search and
/// open pages with `browser` and to run code with `python`, and calls them
/// even when they aren't offered.
pub(crate) fn harmony_alias(name: &str, args: &Value) -> Option<(String, Value)> {
    let s = |k: &str| args.get(k).and_then(Value::as_str).map(str::to_string);
    match name {
        "browser.search" | "browser_search" => Some(("web_search".into(), json!({ "query": s("query")? }))),
        "browser.open" | "browser_open_url" => Some(("fetch_page".into(), json!({ "url": s("url").or_else(|| s("id").filter(|i| i.starts_with("http")))? }))),
        "python" => Some(("run_code".into(), json!({ "code": s("code").or_else(|| s("input"))? }))),
        "container.exec" => {
            let cmd = args.get("cmd").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" ")).or_else(|| s("cmd"))?;
            Some(("run_command".into(), json!({ "command": cmd })))
        }
        _ => None,
    }
}

/// The JSON object starting at `text`'s first `{`, with braces inside
/// strings ignored; and where it ends.
pub(super) fn json_object_at(text: &str) -> Option<(usize, usize)> {
    let start = text.find('{')?;
    let (mut depth, mut in_str, mut esc) = (0usize, false, false);
    for (i, c) in text[start..].char_indices() {
        match c {
            _ if esc => esc = false,
            '\\' if in_str => esc = true,
            '"' => in_str = !in_str,
            '{' if !in_str => depth += 1,
            '}' if !in_str => {
                depth -= 1;
                if depth == 0 {
                    return Some((start, start + i + 1));
                }
            }
            _ => {}
        }
    }
    None
}

pub(super) static HARMONY_CALL: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"to=(?:functions\.)?([A-Za-z_][\w.\-]*)").unwrap());
pub(super) static HARMONY_TOKEN: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"<\|(?:start|end|message|channel|constrain|call|return)\|>").unwrap());

/// Tool calls written in gpt-oss's Harmony format as plain text
/// (`<|channel|>commentary to=functions.find_files <|constrain|>json<|message|>{…}`,
/// or the same with the special tokens left out), which the engine leaves
/// in the reply when it doesn't parse them: run them, and keep the rest of
/// the text without Harmony's markers.
pub(crate) fn harmony_calls(text: &str, offered: &[String]) -> (Vec<ToolCall>, String) {
    if !text.contains("to=") {
        return (Vec::new(), text.to_string());
    }
    let mut calls = Vec::new();
    let mut rest = String::new();
    let mut at = 0;
    while let Some(m) = HARMONY_CALL.captures(&text[at..]) {
        let whole = m.get(0).unwrap();
        let name = m[1].trim_end_matches('.').to_string();
        let after = at + whole.end();
        let Some((s, e)) = json_object_at(&text[after..]) else { break };
        // Only the channel and format words may sit between the name and its arguments.
        let between = HARMONY_TOKEN.replace_all(&text[after..after + s], " ");
        if between.split_whitespace().any(|w| !matches!(w, "commentary" | "json" | "<|constrain|>json" | "code")) {
            rest.push_str(&text[at..after]);
            at = after;
            continue;
        }
        let raw = &text[after + s..after + e];
        let args: Value = serde_json::from_str(raw).unwrap_or(Value::Null);
        let (name, args) = if offered.iter().any(|o| *o == name) {
            (name, args)
        } else if let Some((n, a)) = harmony_alias(&name, &args).filter(|(n, _)| offered.iter().any(|o| o == n)) {
            (n, a)
        } else {
            rest.push_str(&text[at..after + e]);
            at = after + e;
            continue;
        };
        // The channel word just before the call goes with it.
        let before = text[at..at + whole.start()].trim_end();
        rest.push_str(before.strip_suffix("commentary").unwrap_or(before));
        rest.push(' ');
        calls.push(ToolCall { id: String::new(), name, arguments: if args.is_null() { raw.to_string() } else { args.to_string() } });
        at = after + e;
    }
    rest.push_str(&text[at..]);
    let rest = HARMONY_TOKEN.replace_all(&rest, " ");
    let rest = rest.replace("assistantcommentary", " ").replace("assistantanalysis", " ").replace("assistantfinal", " ");
    // Line breaks kept (lists, code blocks); only the gaps the tokens left are tidied.
    let mut tidy = String::new();
    let mut blank = 0;
    for line in rest.lines().map(str::trim_end) {
        blank = if line.trim().is_empty() { blank + 1 } else { 0 };
        if blank < 2 {
            tidy.push_str(line);
            tidy.push('\n');
        }
    }
    (calls, tidy.trim().to_string())
}

/// Small models sometimes write a tool call into their thinking or their
/// text instead of the tool-call channel; run it rather than lose it.
pub(crate) fn rescue_tool_calls(out: &mut Finished, tools: Option<&Value>) {
    if out.tool_calls.is_empty() && !out.cancelled {
        let offered = offered_names(tools);
        // gpt-oss's own format, left as text by the engine.
        if !offered.is_empty() {
            let (calls, rest) = harmony_calls(&out.content, &offered);
            if !calls.is_empty() {
                out.content = rest;
                out.tool_calls = calls;
                return;
            }
            let (calls, _) = harmony_calls(&out.thinking, &offered);
            if !calls.is_empty() && out.content.trim().is_empty() {
                out.tool_calls = calls;
                return;
            }
        }
        if !offered.is_empty() {
            let (calls, rest) = salvage(&out.content, &offered);
            if !calls.is_empty() {
                out.content = rest;
                out.tool_calls = calls;
            } else if out.content.trim().is_empty() {
                out.tool_calls = salvage(&out.thinking, &offered).0;
            }
        }
    }
}

// What's left around a call once its JSON is taken out: the tags models wrap
// calls in (<tool_call>, Qwen's own; <tools>, which coder models copy from
// their template; <tool>, <function_call>) or an empty ```json fence.
pub(super) static EMPTY_WRAPPER: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?s)<(?:tool_call|tools?|function_call)>\s*</(?:tool_call|tools?|function_call)>|```(?:json|tool_call)?\s*```").unwrap()
});

/// The first tool call written into text, for tools that were offered:
/// `{"name": ..., "arguments": {...}}` anywhere in it, wrapped in a tag or a
/// code fence or not. Returns it and the words before it. Only the first:
/// what a model writes after a call it can't see the result of is made up
/// (it "imagines" the result and goes on calling tools with invented details).
pub(crate) fn salvage(text: &str, offered: &[String]) -> (Vec<ToolCall>, String) {
    let mut i = 0;
    while let Some(off) = text[i..].find('{') {
        let start = i + off;
        let mut stream = serde_json::Deserializer::from_str(&text[start..]).into_iter::<Value>();
        if let Some(Ok(v)) = stream.next() {
            if let Some(call) = call_from_value(&v, offered) {
                let before = EMPTY_WRAPPER.replace_all(&text[..start], "").to_string();
                let before = before.trim_end().trim_end_matches(|c| c == '<' || c == '`').trim_end_matches("<tool_call>").trim_end_matches("<tools>").trim_end_matches("```json").trim();
                return (vec![call], before.to_string());
            }
        }
        i = start + 1;
    }
    (Vec::new(), text.trim().to_string())
}

#[cfg(test)]
pub(super) fn salvage_tool_calls(text: &str, offered: &[String]) -> Vec<ToolCall> {
    salvage(text, offered).0
}

pub(super) fn call_from_value(v: &Value, offered: &[String]) -> Option<ToolCall> {
    let name = v["name"].as_str()?.to_string();
    if !offered.contains(&name) {
        return None;
    }
    let args = if v["arguments"].is_null() { &v["parameters"] } else { &v["arguments"] };
    let arguments = match args {
        Value::String(s) => s.clone(),
        Value::Null => "{}".into(),
        other => other.to_string(),
    };
    Some(ToolCall { id: String::new(), name, arguments })
}

// SPDX-License-Identifier: AGPL-3.0-only
//! preview_site: serves a site the assistant built (or shows a dev server it
//! started), opens it in the chat's Preview panel, and checks it in a hidden
//! window so the model can fix what doesn't work.

use serde_json::{json, Value};
use tauri::Emitter;

use super::{Ctx, Outcome};

pub fn preview_site_params() -> Value {
    json!({ "type": "object", "properties": {
        "path": { "type": "string", "description": "The site's folder or its start page in a shared folder, e.g. stock-site or stock-site/index.html." },
        "port": { "type": "integer", "description": "Instead of path: the port of a dev server you started with run_command (background: true), e.g. 5173." } } })
}

/// The page to start on in a folder: index.html, or its only page.
fn start_page(dir: &std::path::Path) -> Option<String> {
    if dir.join("index.html").is_file() {
        return Some("index.html".into());
    }
    let mut pages: Vec<String> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.to_ascii_lowercase().ends_with(".html") || n.to_ascii_lowercase().ends_with(".htm"))
        .collect();
    pages.sort();
    pages.into_iter().next()
}

fn show(ctx: &Ctx<'_>, payload: Value) {
    if let Some(app) = ctx.app {
        let _ = app.emit("chat:preview", payload);
    }
}

pub async fn run(args: &Value, ctx: &Ctx<'_>) -> Outcome {
    let chat_id = ctx.memory.chat_id.to_string();
    // The site may reach the internet only when this chat may.
    let web = crate::browser::allowed_for_chat(&ctx.memory.db.lock().unwrap(), &chat_id);
    if let Some(port) = args.get("port").and_then(Value::as_u64).filter(|p| *p > 0 && *p < 65536) {
        let port = port as u16;
        if !crate::ports::previewable(port) {
            return Outcome::error(format!("Couldn't preview port {port}"), format!("Port {port} isn't a server you started. Start it with run_command (background: true), or give the site's folder as path."));
        }
        let url = format!("http://localhost:{port}/");
        let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(8)).no_proxy().build().unwrap_or_default();
        let loaded = match client.get(&url).send().await {
            Ok(r) if r.status().is_success() => Ok(r.text().await.unwrap_or_default().len()),
            Ok(r) => Err(format!("it answered with {}", r.status())),
            Err(e) => Err(format!("it didn't answer ({e})")),
        };
        show(ctx, json!({ "chat_id": chat_id, "url": url, "port": port }));
        let mut o = match loaded {
            Ok(bytes) => Outcome::ok(
                format!("The site is open in the Preview panel beside the chat ({url}); the user can open it in their browser from there. Its page loaded ({bytes} bytes). Read the server's output with command_output to see any errors."),
                format!("Previewed the site on port {port}"),
                "text",
                Some(url.clone()),
            ),
            Err(why) => Outcome::error(format!("Couldn't preview port {port}"), format!("The panel shows {url}, but {why}. Check the server's output with command_output and fix it.")),
        };
        o.meta["preview_url"] = json!(url);
        o.meta["preview_port"] = json!(port);
        return o;
    }
    let target = match args.get("path").and_then(Value::as_str).map(str::trim).filter(|p| !p.is_empty()) {
        Some(t) => t.to_string(),
        None => return Outcome::error("Preview a site", "Give the site's folder as path (or a dev server's port)."),
    };
    let path = match ctx.sandbox.resolve(&target) {
        Ok(p) => p,
        Err(e) => return Outcome::error(format!("Preview {target}"), e),
    };
    let (root, start) = if path.is_dir() {
        match start_page(&path) {
            Some(s) => (path.clone(), s),
            None => return Outcome::error(format!("Preview {target}"), format!("There's no index.html in {}: write it first.", ctx.sandbox.display(&path))),
        }
    } else if path.is_file() && path.extension().is_some_and(|e| e.eq_ignore_ascii_case("html") || e.eq_ignore_ascii_case("htm")) {
        let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        (path.parent().map(std::path::Path::to_path_buf).unwrap_or_default(), name)
    } else if path.exists() {
        return Outcome::error(format!("Preview {target}"), "Give the site's folder or an .html page.");
    } else {
        return Outcome::error(format!("Preview {target}"), format!("{} doesn't exist.", ctx.sandbox.display(&path)));
    };
    let shown = ctx.sandbox.display(&root);
    let served = match crate::preview::serve(&chat_id, &root, &start, web).await {
        Ok(s) => s,
        Err(e) => return Outcome::error(format!("Preview {shown}"), e),
    };
    show(ctx, json!({ "chat_id": chat_id, "url": served.url, "site": served.id, "root": served.root, "start": start }));
    let checked = match ctx.app {
        Some(app) => crate::preview::check(app, &served.id).await,
        None => Err("there's no window to check it in here".into()),
    };
    let (problems, summary) = match &checked {
        Ok(reports) => crate::preview::describe(reports, web),
        Err(e) => (Vec::new(), format!("It couldn't be checked ({e}).")),
    };
    let offline = if web { "" } else { " This chat is offline, so the site can't reach the internet: anything live has to be simulated (and labelled so) or the web turned on with request_web." };
    let text = if problems.is_empty() {
        format!(
            "The site is open in the Preview panel beside the chat ({}); the user can open it in their browser from there. Check: {summary} No problems found.{offline} \
             Tell the user in a few sentences what you built and how to use it.",
            served.url
        )
    } else {
        format!(
            "The site is open in the Preview panel beside the chat ({}). Check: {summary}{offline}\nProblems to fix:\n{}\n\
             Fix every one of these now with edit_file or write_file (read the file first), then call preview_site again to check.",
            served.url,
            problems.iter().map(|p| format!("- {p}")).collect::<Vec<_>>().join("\n")
        )
    };
    let detail = if problems.is_empty() { summary.clone() } else { problems.join("\n") };
    let mut o = match problems.len() {
        0 => Outcome::ok(text, format!("Previewed {shown}"), "text", Some(detail)),
        1 => Outcome::ok(text, format!("Previewed {shown}: 1 problem to fix"), "text", Some(detail)),
        n => Outcome::ok(text, format!("Previewed {shown}: {n} problems to fix"), "text", Some(detail)),
    };
    o.meta["preview_url"] = json!(served.url);
    o.meta["preview_site"] = json!(served.id);
    o.meta["preview_root"] = json!(served.root);
    o.meta["preview_start"] = json!(start);
    o.meta["preview_problems"] = json!(problems.len());
    o.meta["preview_checked"] = json!(checked.is_ok());
    o
}

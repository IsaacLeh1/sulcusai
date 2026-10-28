// SPDX-License-Identifier: AGPL-3.0-only
//! Asking the person for something mid-turn: a key, the web, a folder,
//! another chat, a cloud model, or sharing what's personal.

use super::*;

impl Turn {
    /// A site needs a data source that asks for a key: the user is told what
    /// it connects to and what it needs, and gives the key in a popup (or
    /// keeps one saved earlier). SulcusAI keeps it and adds it to this chat's
    /// sites' requests to that host; the model never sees it.
    pub(super) async fn request_api_key(&self, call: &ToolCall) -> Outcome {
        let args: Value = serde_json::from_str(&call.arguments).unwrap_or_default();
        let text = |k: &str| args.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
        let Some(host) = text("address").as_deref().and_then(crate::sitekeys::host_of) else {
            return Outcome::error("Ask for a key", "Give the API's public address (address), e.g. https://finnhub.io/api/v1/.");
        };
        let platform: String = text("platform").unwrap_or_else(|| host.clone()).chars().take(60).collect();
        let reason: String = text("reason").unwrap_or_else(|| "this site".into()).chars().take(120).collect();
        // The free sources under Live data need no key (the live test's 14B asked for a Yahoo one).
        if crate::sitekeys::keyless(&host) {
            return Outcome::ok(
                format!(
                    "{platform} ({host}) needs no API key: fetch it through fetch('/__sulcus/fetch?url=' + encodeURIComponent(address)) as it is. \
                     Go on now: write the site's files with write_file, index.html first."
                ),
                format!("{platform} needs no key"),
                "text",
                None,
            );
        }
        let mut param = text("key_param").unwrap_or_default();
        let mut header = text("key_header").unwrap_or_default();
        // Both given (copied from the examples): the parameter, the usual place.
        if !param.is_empty() && !header.is_empty() {
            if crate::sitekeys::param_ok(&param) {
                header.clear();
            } else {
                param.clear();
            }
        }
        let place_ok = match (param.is_empty(), header.is_empty()) {
            (false, true) => crate::sitekeys::param_ok(&param),
            (true, false) => crate::sitekeys::header_ok(&header),
            _ => false,
        };
        if !place_ok {
            return Outcome::error(
                format!("Ask for a {platform} key"),
                "Say where the key goes: key_param (the query parameter, e.g. token) or key_header (e.g. X-API-Key), one of them.",
            );
        }
        if !crate::web::allowed_for_chat(&self.state.db.lock().unwrap(), &self.chat_id) {
            return Outcome::error(format!("Ask for a {platform} key"), "This chat is offline, so a site can't use the key. Ask to turn the web on with request_web first.");
        }
        let signup = text("signup").filter(|s| s.starts_with("https://") && reqwest::Url::parse(s).is_ok());
        let saved = crate::sitekeys::saved_for(&self.state.db.lock().unwrap(), &self.cipher, &host).is_some();
        crate::sitekeys::offer(&call.id, crate::sitekeys::Ask { platform: platform.clone(), host: host.clone(), reason: reason.clone(), signup, saved });
        // The popup (KeyPermission) shows the platform, what it's for, where to get a key, and a box for it.
        let preview = Preview { title: format!("Connect {platform} for: {reason}"), kind: "key", detail: Some(reason.clone()), note: Some(host.clone()) };
        let decision = self.ask(call, Risk::Read, preview).await;
        let typed = crate::sitekeys::taken(&call.id);
        if decision == Decision::Deny || (typed.is_none() && !saved) {
            self.log(&format!("Didn't connect {platform} (asked for: {reason})"));
            return Outcome::ok(
                format!(
                    "The user didn't connect {platform}. Don't ask for the key in the chat or put one in the files. Use a source that \
                     needs no key instead (see Live data), or, if there's none for this, show realistic simulated data labelled \"simulated\", \
                     and say in a sentence that connecting {platform} would make it live."
                ),
                format!("{platform} not connected"),
                "text",
                None,
            );
        }
        let given = crate::sitekeys::SiteKey { host: host.clone(), platform: platform.clone(), param: param.clone(), header: header.clone(), key: String::new(), chats: Vec::new() };
        {
            let conn = self.state.db.lock().unwrap();
            if let Err(e) = crate::sitekeys::approve(&conn, &self.cipher, &self.chat_id, given, typed) {
                return Outcome::error(format!("Couldn't connect {platform}"), e);
            }
            db::log_action(&conn, "privacy", &format!("Let this chat's sites use an API key for {host} (the assistant asked)"));
        }
        let place = if param.is_empty() { format!("in the {header} header") } else { format!("as the {param} parameter") };
        let mut o = Outcome::ok(
            format!(
                "The user connected {platform}. SulcusAI adds their key {place} to every request this chat's sites make to {host} through \
                 /__sulcus/fetch. In the JavaScript, leave the key out entirely and fetch like this: \
                 fetch('/__sulcus/fetch?url=' + encodeURIComponent('https://{host}/...')).then(r => r.json()). Never write the key, \
                 a stand-in for it or a {} into the files.",
                if param.is_empty() { "key header".to_string() } else { format!("{param}= parameter") }
            ),
            format!("Connected {platform}"),
            "text",
            None,
        );
        o.meta["key_granted"] = json!(host);
        o
    }

    /// Web access is off and the model needs it: the user is asked in a
    /// popup. A yes turns web on for this chat and the turn carries on with
    /// the web tools; a no lets the model answer without them.
    pub(super) async fn request_web(&self, call: &ToolCall) -> Outcome {
        let args: Value = serde_json::from_str(&call.arguments).unwrap_or_default();
        let reason = args.get("reason").and_then(Value::as_str).map(str::trim).filter(|r| !r.is_empty()).unwrap_or("this request").to_string();
        // The person has allowed fetching web pages, so this chat goes online
        // without a popup (asking every time left tasks waiting). Still asked:
        // with SulcusAI set to Offline (going online overrides their own
        // choice), and in Plan mode. Fully offline, this tool isn't offered.
        let offline = self.state.settings().connectivity == crate::net::Connectivity::Offline;
        if self.mode == tools::Mode::Plan || offline {
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
            "Web access is on for this chat now. Go ahead: use web_search, fetch_page, wikipedia, research, market_quote, find_places or weather to finish what they asked.",
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
    pub(super) fn message_chat<'a>(&'a self, call: &'a ToolCall) -> std::pin::Pin<Box<dyn std::future::Future<Output = Outcome> + Send + 'a>> {
        Box::pin(self.ask_chat(call))
    }

    pub(super) async fn ask_chat(&self, call: &ToolCall) -> Outcome {
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
    pub(super) async fn request_folder(&self, call: &ToolCall, sandbox: &Sandbox) -> Outcome {
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
            crate::workspace::shared_everywhere(&call.id);
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
        // This chat's, unless the person let every chat use it.
        let scope = (!crate::workspace::shared_everywhere(&call.id)).then_some(self.chat_id.as_str());
        let shown = match crate::workspace::share_folder(&self.state, &chosen, true, true, scope) {
            Ok(s) => s,
            Err(e) => return Outcome::error("Couldn't share the folder", format!("{e} Tell the user, and ask them to share one with the folder button under the message box.")),
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

    /// An online model: what this turn sends it goes on the "What left this
    /// PC" list. In privacy mode, from the level set, the person sees it
    /// first: once a chat, and again only when a turn is more personal than
    /// what they shared before. They may leave out the parts about them
    /// (profile, memories, connected accounts), taken out of `about` here.
    /// Some(result) when they said no: the turn ends with a note saying so.
    pub(super) async fn cloud_gate(&self, request: &str, about: &mut String, memories: &str, accounts: &str) -> Result<Option<TurnResult>, String> {
        let Some(target) = self.ep.cloud.clone() else { return Ok(None) };
        let privacy = crate::outgoing::mode(&self.state.db.lock().unwrap());
        let (history, _) = compact::after_last(db::messages(&self.state.db.lock().unwrap(), &self.cipher, &self.chat_id));
        let earlier: Vec<String> = history
            .iter()
            .filter(|m| m.id != self.turn_id && (m.role == "user" || m.role == "assistant") && !m.content.trim().is_empty())
            .map(|m| m.content.clone())
            .collect();
        let pictures: usize = history.iter().filter_map(|m| m.meta.as_ref()?.get("images")?.as_array().map(Vec::len)).sum();
        let me = crate::outgoing::context(&self.state, &self.cipher).me;
        let r = crate::outgoing::review_cloud(&target.provider, request, &self.about, memories, accounts, &earlier, pictures, target.redact, &me);
        let mut sent = r.clone();
        if privacy.on && r.level >= privacy.ask_from && r.level > self.state.approvals.cloud_ok(&self.chat_id) {
            let call = ToolCall { id: format!("{}_cloud", self.turn_id), name: "cloud_chat".into(), arguments: "{}".into() };
            match self.share(&call, &r).await {
                Ok(edits) => {
                    for e in edits.iter().filter(|e| e.value.is_none()) {
                        let part = match e.key.as_str() {
                            "profile" => self.about.as_str(),
                            "memories" => memories,
                            "accounts" => accounts,
                            _ => "",
                        };
                        if !part.is_empty() {
                            *about = about.replacen(part, "", 1);
                        }
                        sent.items.retain(|i| i.key.as_deref() != Some(e.key.as_str()));
                    }
                    sent.level = sent.items.iter().map(|i| i.level).max().unwrap_or(r.level);
                    self.state.approvals.allow_cloud(&self.chat_id, r.level);
                }
                Err(_) => {
                    let note = Message {
                        id: uuid::Uuid::new_v4().to_string(),
                        chat_id: self.chat_id.clone(),
                        role: "assistant".into(),
                        content: format!(
                            "Not sent: you chose not to share this with {}. To keep this chat on your PC, pick a model that runs here in the model menu.",
                            target.provider
                        ),
                        created_at: now(),
                        ..Default::default()
                    };
                    self.save(&note)?;
                    return Ok(Some(TurnResult { last: Some(note), context: ContextInfo::default(), tps: None, cancelled: false }));
                }
            }
        }
        crate::outgoing::record(&self.state, &self.cipher, &self.chat_id, "cloud_chat", &sent, true);
        Ok(None)
    }

    /// Privacy mode: shows the person what a call would send, to whom, and
    /// how personal it is, and asks. Their changes (reworded or left-out
    /// parts) come back. Refused when they say no, or when nobody's there
    /// to ask (a scheduled task).
    pub(super) async fn share(&self, call: &ToolCall, r: &crate::outgoing::Review) -> Result<Vec<crate::outgoing::Edit>, Outcome> {
        let title = format!("Send to {}", r.to);
        if self.unattended {
            crate::outgoing::record(&self.state, &self.cipher, &self.chat_id, &call.name, r, false);
            self.log(&format!("Privacy mode: didn't send to {} in a scheduled task (level {} of 5, nobody there to approve it)", r.to, r.level));
            return Err(Outcome::error(
                title,
                format!(
                    "Privacy mode is on, and this would send level {} information ({}) to {}. Nobody is here to approve it, so it wasn't sent. Carry on without it.",
                    r.level,
                    r.level_name.to_lowercase(),
                    r.to
                ),
            ));
        }
        let preview = Preview { title: format!("Send to {}?", r.to), kind: "privacy", detail: serde_json::to_string(r).ok(), note: None };
        match self.ask(call, Risk::Read, preview).await {
            Decision::Deny => {
                crate::outgoing::record(&self.state, &self.cipher, &self.chat_id, &call.name, r, false);
                self.log(&format!("Privacy mode: chose not to send to {}", r.to));
                Err(Outcome {
                    text: "The user chose not to share this (privacy mode). Don't send it another way: carry on without it, or ask them what they'd be happy to share.".into(),
                    meta: json!({ "title": title, "status": "denied", "kind": "text" }),
                })
            }
            Decision::Allow | Decision::AllowForChat => Ok(crate::outgoing::take_edits(&call.id)),
        }
    }
}

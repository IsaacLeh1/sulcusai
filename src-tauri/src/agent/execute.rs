// SPDX-License-Identifier: AGPL-3.0-only
//! Running one tool call: the checks and questions before it, then the tool.

use super::*;

const RULE_REFUSED: &str = "A rule the user saved doesn't allow this, so it didn't run. Tell the user what you were going to do, and that they can change the rule in Settings › Permissions. Don't try another way around it.";

impl Turn {
    pub(super) async fn execute(&self, call: &ToolCall, sandbox: &Sandbox, recorder: &Recorder<'_>, project_id: Option<&str>, extras: &Extras) -> Outcome {
        if let Some(t) = extras.mcp.get(&call.name) {
            let rule = allowances::decide(&extras.allowances, project_id, &call.name, &args_of(call), sandbox);
            return self.execute_connector(call, t, rule).await;
        }
        if call.name == "request_web" {
            return self.request_web(call).await;
        }
        if call.name == "request_folder" {
            return self.request_folder(call, sandbox).await;
        }
        if call.name == "request_api_key" {
            return self.request_api_key(call).await;
        }
        if call.name == "message_chat" {
            return self.message_chat(call).await;
        }
        if call.name == "load_skill" {
            let args: Value = serde_json::from_str(&call.arguments).unwrap_or_default();
            let wanted = args.get("name").and_then(Value::as_str).unwrap_or("");
            let file = args.get("file").and_then(Value::as_str).filter(|f| !f.trim().is_empty());
            return match (extras.skills.iter().find(|s| s.name.eq_ignore_ascii_case(wanted)), file) {
                (Some(s), None) => Outcome::ok(connectors::skill_text(s), format!("Loaded the {} skill", s.name), "text", None),
                (Some(s), Some(f)) => match s.read_file(f) {
                    Ok(text) => Outcome::ok(text, format!("Read {f} from the {} skill", s.name), "text", None),
                    Err(e) => Outcome::error(format!("Couldn't read {f}"), e),
                },
                (None, _) => Outcome::error("Couldn't load a skill", format!("There's no skill called {wanted}.")),
            };
        }
        let Some(def) = tools::find(&call.name) else {
            return Outcome::error(call.name.clone(), format!("There is no tool called {}. Call one of the tools you were given, by its exact name.", call.name));
        };
        let mut args: Value = match serde_json::from_str(if call.arguments.trim().is_empty() { "{}" } else { &call.arguments }) {
            Ok(v) => v,
            Err(e) => return Outcome::error(format!("Couldn't use {}", call.name), format!("The arguments weren't valid JSON ({e}). Try again.")),
        };
        if let Err(e) = tools::check_args(def, &mut args) {
            return Outcome::error(tools::failed_title(&call.name, &args), format!("{e} Fix the arguments and call it again."));
        }
        // A Deny holds in every mode, Bypass too.
        let mut rule = allowances::decide(&extras.allowances, project_id, &call.name, &args, sandbox);
        if rule == Some(Effect::Deny) {
            let title = tools::failed_title(&call.name, &args);
            self.log(&format!("A saved rule refused: {title}"));
            return Outcome::denied(title).with_text(RULE_REFUSED);
        }
        // A plugin's check before the tool: it can stop the call, have the
        // person asked, or let it run like a saved Allow (never past what
        // always asks).
        let mut check_asks = None;
        if extras.checks_at(plugin_checks::Event::BeforeTool, Some(&call.name)) {
            let folder = sandbox.roots().iter().find(|r| !sandbox.is_general(r)).or(sandbox.roots().first()).cloned();
            let moment = plugin_checks::Moment { event: plugin_checks::Event::BeforeTool, chat_id: &self.chat_id, tool: Some(&call.name), arguments: Some(&args), result: None, folder: folder.as_deref(), mode: self.mode };
            let a = plugin_checks::run(&self.state, &extras.checks, &moment, sandbox).await;
            if let Some(why) = a.block {
                self.log(&why);
                return Outcome::denied(tools::failed_title(&call.name, &args)).with_text(&format!("{why} Tell the user, and don't try another way around it."));
            }
            check_asks = a.ask;
            if a.allow && rule.is_none() {
                rule = Some(Effect::Allow);
            }
        }
        // Clicking or typing in the browser: refuse private fields, and ask
        // first when it submits, buys, sends or deletes.
        let mut risk = def.risk;
        let mut browser_preview = None;
        if matches!(call.name.as_str(), "browser_click" | "browser_type") {
            match tools::browser::assess(self.state.app.get(), &self.chat_id, &call.name, &args).await {
                Ok(None) => {}
                Ok(Some(p)) => {
                    risk = Risk::Submit;
                    browser_preview = Some(p);
                }
                Err(e) => return Outcome::error(tools::failed_title(&call.name, &args), e),
            }
        } else if matches!(call.name.as_str(), "program_click" | "program_type" | "program_key") {
            // Another program: a press, Enter or keys that send, buy, delete or close ask every time.
            match tools::programs::assess(&self.chat_id, &call.name, &args).await {
                Ok(None) => {}
                Ok(Some(p)) => {
                    risk = Risk::Submit;
                    browser_preview = Some(p);
                }
                Err(e) => return Outcome::error(tools::failed_title(&call.name, &args), e),
            }
        } else if matches!(call.name.as_str(), "email_send" | "calendar_create_event") {
            // Sending mail and inviting people always ask; a plain event follows the mode.
            match tools::comms::assess(&call.name, &args, &self.state, &self.cipher) {
                Ok((r, p)) => {
                    risk = r;
                    browser_preview = Some(p);
                }
                Err(e) => return Outcome::error(tools::failed_title(&call.name, &args), e),
            }
        }
        // A command that sends something off the PC (an upload, a post, a
        // push, a copy to another machine, a request to the local network):
        // asked in every mode, Bypass included, never "for the whole chat".
        // Models here may read anything; nothing leaves without a yes, and a
        // secret leaving gets a warning.
        let egress = tools::egress_call(&call.name, &args, sandbox);
        if let Some(e) = &egress {
            risk = Risk::Submit;
            browser_preview = Some(tools::egress_preview(e, args.get("command").and_then(Value::as_str).unwrap_or_default()));
        }
        // Privacy mode: what this call sends off the PC, who gets it, and how
        // personal it is (1-5). Shown on the approval card when the call asks
        // anyway; otherwise asked about below, from the level set.
        // Reviewed whether or not the mode is on: every call that goes out is
        // listed under "What left this PC".
        let privacy = crate::outgoing::mode(&self.state.db.lock().unwrap());
        let cx = if crate::outgoing::is_external(&call.name, &args) { Some(crate::outgoing::context(&self.state, &self.cipher)) } else { None };
        let mut review = cx.as_ref().and_then(|cx| crate::outgoing::review(&call.name, &args, cx));
        // A picture or video an online model would make (nothing here can).
        if review.is_none() && matches!(call.name.as_str(), "create_image" | "edit_image" | "create_video") {
            let me = crate::outgoing::context(&self.state, &self.cipher).me;
            review = tools::media::cloud_review(&self.state, &call.name, &args, &me);
        }
        // An event written to an account's calendar goes to its company.
        if review.is_none() && call.name == "calendar_create_event" {
            let me = crate::outgoing::context(&self.state, &self.cipher).me;
            review = tools::comms::calendar_review(&self.state, &self.cipher, &args, &me);
        }
        let mut asked = false;
        // "Allow for this chat" (or a saved Allow) never covers a delete or
        // another call that asks even in Auto: each one asks.
        let allowed = allowances::allowed(self.state.approvals.allowed(&self.chat_id, risk), rule, tools::destructive_call(&call.name, &args));
        let adv = crate::advanced::get(&self.state);
        if adv.forbids(risk) {
            return Outcome::error(
                tools::failed_title(&call.name, &args),
                "This kind of action is turned off in Settings → Advanced. Tell the user, and don't try another way around it.",
            );
        }
        let mut permission = tools::permission(self.mode, risk, allowed);
        // Auto: the person has already allowed changes and commands, so
        // they run on their own; only a destructive one asks (deleting, or a
        // command the guard flags). Asking before every step left tasks
        // unfinished. What the guard refuses never runs at all.
        if self.mode == tools::Mode::Auto && permission == Permission::Ask && matches!(risk, tools::Risk::Write | tools::Risk::Execute) && !tools::destructive_call(&call.name, &args) {
            permission = Permission::Run;
        }
        // A saved Allow is the person's exception to "ask before every …".
        if permission == Permission::Run && adv.must_ask(risk) && rule != Some(Effect::Allow) {
            permission = Permission::Ask;
        }
        // Deleting a chat always asks, in every mode (Bypass too).
        if call.name == "delete_chat" && permission == Permission::Run {
            permission = Permission::Ask;
        }
        // A timed action the user approved when they set it up: what it was
        // approved for runs, and nothing else that changes or sends.
        if let Some(approved) = &self.pre_approved {
            if !matches!(risk, Risk::Read | Risk::Memory) {
                if !approved.iter().any(|t| *t == call.name) {
                    return Outcome::error(
                        tools::failed_title(&call.name, &args),
                        format!("This task was only approved to use {} when it was set up, so it can't do this. Do what it says with those, or say what else it needs.", approved.join(", ")),
                    );
                }
                permission = Permission::Run;
            }
        }
        // A secret in what goes out (a key in a search or a web address, a
        // password in an email or a message, a key file a command sends)
        // always asks, whatever the mode and with privacy mode on or off.
        let secret_out = review.as_ref().is_some_and(|r| r.level >= 5) || egress.as_ref().is_some_and(|e| e.secret.is_some());
        if secret_out && permission == Permission::Run {
            permission = Permission::Ask;
        }
        // A saved Ask (or a plugin's check asking) asks in every mode, even
        // when the chat allowed the kind.
        if (rule == Some(Effect::Ask) || check_asks.is_some()) && permission == Permission::Run {
            permission = Permission::Ask;
        }
        match permission {
            Permission::Refuse => {
                return Outcome::error(tools::failed_title(&call.name, &args), "Plan mode can't change anything. Put this step in your plan instead.");
            }
            Permission::Ask => {
                // A preview that fails (say, edit text not found) goes straight back to the model.
                let mut preview = match if let Some(p) = browser_preview.take() {
                    Ok(p)
                } else if call.name == "update_note" {
                    tools::notes_preview(&call.name, &args, &self.state.db, &self.cipher)
                } else if call.name == "delete_chat" {
                    tools::chats::delete_preview(&args, &self.state.db, &self.cipher, &self.chat_id)
                } else {
                    tools::preview(&call.name, &args, sandbox)
                } {
                    Ok(p) => p,
                    Err(e) => return Outcome::error(tools::failed_title(&call.name, &args), e),
                };
                if let Some(r) = review.as_ref().filter(|_| privacy.on) {
                    let line = crate::outgoing::summary(r);
                    preview.note = Some(match preview.note.take() {
                        Some(n) => format!("{n}\n{line}"),
                        None => line,
                    });
                    asked = true;
                }
                // A secret named on the card, plainly (the command card says so already).
                if let Some(r) = review.as_ref().filter(|r| r.level >= 5 && egress.is_none()) {
                    let what = r.found.iter().find(|f| f.level >= 5).map(|f| f.what.clone()).unwrap_or_else(|| "a secret".into());
                    preview.title = format!("Warning: this would send {what} to {}", r.to);
                    preview.kind = "text";
                    preview.detail = Some(r.items.iter().map(|i| format!("{}: {}", i.label, i.value)).collect::<Vec<_>>().join("\n"));
                    asked = true;
                }
                let why = match (&check_asks, rule) {
                    (Some(c), _) => Some(c.clone()),
                    (None, Some(Effect::Ask)) => Some("Asking because of a rule you saved in Settings › Permissions.".to_string()),
                    _ => None,
                };
                if let Some(line) = why {
                    preview.note = Some(match preview.note.take() {
                        Some(n) => format!("{n}\n{line}"),
                        None => line,
                    });
                }
                // "Always allow…" only where a saved Allow would skip this question next time.
                let once = risk == Risk::Submit || tools::destructive_call(&call.name, &args);
                let offer = (!once && rule.is_none() && check_asks.is_none()).then(|| allowances::suggest(&call.name, &args, sandbox, project_id));
                let title = preview.title.clone();
                match self.ask_offering(call, risk, preview, offer).await {
                    Decision::Deny => {
                        self.log(&format!("Declined: {title}"));
                        return Outcome::denied(title);
                    }
                    Decision::Allow | Decision::AllowForChat => {}
                }
            }
            Permission::Run => {}
        }
        let mut args = args;
        let mut shared_note = None;
        if let Some(r) = review {
            if privacy.on && !asked && r.level >= privacy.ask_from && r.level > self.state.approvals.shared_up_to(&self.chat_id) {
                match self.share(call, &r).await {
                    Ok(edits) if edits.is_empty() => {}
                    Ok(edits) => {
                        args = crate::outgoing::apply(&args, &r, &edits);
                        shared_note = Some(crate::outgoing::changed(&edits));
                    }
                    Err(refused) => return refused,
                }
            }
            // As it really goes, after any changes.
            let sent = cx.as_ref().and_then(|cx| crate::outgoing::review(&call.name, &args, cx)).unwrap_or(r);
            // (A chat's online picture job is listed here, not again by the job.)
            crate::outgoing::record(&self.state, &self.cipher, &self.chat_id, &call.name, &sent, true);
            if privacy.on {
                self.log(&format!("Sent to {} (privacy level {} of 5)", sent.to, sent.level));
            }
        }
        // Chats not working together: another chat's handoff note needs the person's word.
        if let Some(refused) = self.handoff_gate(call, &args, sandbox).await {
            return refused;
        }
        let ctx = tools::Ctx {
            sandbox,
            recorder,
            cancel: &self.cancel,
            job: self.state.job(),
            memory: tools::MemoryCtx { db: &self.state.db, cipher: &self.cipher, chat_id: &self.chat_id, project_id },
            app: self.state.app.get(),
            state: &self.state,
        };
        // A picture of something named (a character, a game, a brand): its look
        // is spelled out, looked up first if the model isn't sure of it.
        let (args, picture_note, web_granted) = if matches!(call.name.as_str(), "create_image" | "edit_image" | "create_video") {
            self.picture_names(call, &args, &ctx).await
        } else {
            (args, None, false)
        };
        self.emit("agent:tool_start", json!({ "chat_id": self.chat_id, "call_id": call.id, "tool": call.name }));
        let mut outcome = if call.name == "delegate" {
            match tools::arg_str(&args, "task") {
                Ok(task) => self.run_helper(&call.id, task, &ctx).await,
                Err(e) => Outcome::error(tools::failed_title(&call.name, &args), e),
            }
        } else if call.name == "research" && args.get("depth").and_then(Value::as_str) == Some("deep") {
            self.deep_research(call, &args).await
        } else if call.name == "use_screen" {
            match tools::arg_str(&args, "task") {
                Ok(task) => self.use_screen(call, task).await,
                Err(e) => Outcome::error(tools::failed_title(&call.name, &args), e),
            }
        } else {
            tools::run(&call.name, &args, &ctx).await
        };
        if let Some(note) = picture_note {
            outcome.text = format!("{}\n\n(Before it was made: {note}.)", outcome.text);
        }
        if let Some(note) = shared_note {
            outcome.text = format!("{}\n\n({note})", outcome.text);
        }
        if web_granted {
            outcome.meta["web_granted"] = json!(true);
        }
        if let Some(title) = outcome.meta.get("title").and_then(Value::as_str) {
            self.log(title);
        }
        // A chat on an online model: what a tool read goes to it next. A key
        // or password in it (a file, a command's output) asks first; denied,
        // the model gets a note instead of the text.
        if let Some(cloud) = self.ep.cloud.as_ref().filter(|_| matches!(call.name.as_str(), "read_file" | "run_command" | "search_files" | "read_document" | "list_dir")) {
            if crate::practice::yourcode::SECRET_TEXT.is_match(&outcome.text) {
                let preview = Preview {
                    title: format!("Warning: this would send a key or password to {}", cloud.provider),
                    kind: "text",
                    detail: Some(format!("What {} found holds what looks like a key or password. It would go to {}, the online model this chat uses.", call.name, cloud.provider)),
                    note: Some("Deny to leave it out; the model is told it was left out.".into()),
                };
                if self.ask(call, Risk::Submit, preview).await == Decision::Deny {
                    self.log("Left a key or password out of what went to the online model");
                    outcome.text = "[Left out: it holds a key or password, and the user chose not to send it to the online model.]".into();
                }
            }
        }
        outcome
    }

    /// One quiet request whose reply must match a JSON schema (not shown in
    /// the chat). None if the engine fails or it's stopped.
    pub(super) async fn quiet_json(&self, ask: String, format: Value, max_tokens: u32) -> Option<String> {
        let mut ep = self.ep.clone();
        ep.extra["response_format"] = format;
        ep.extra["max_tokens"] = json!(max_tokens);
        let messages = vec![json!({ "role": "system", "content": self.base }), json!({ "role": "user", "content": ask })];
        chat::stream(&ep, messages, None, &self.cancel, |_| {}).await.ok().filter(|f| !f.cancelled).map(|f| f.content)
    }

    /// Before a picture or clip of named things: which have a look the
    /// picture model won't know by name, and what each looks like. One the
    /// model isn't sure of is looked up on the web (asking first when the web
    /// is off for this chat). Returns the arguments with the looks spelled
    /// out, a note for the model, and whether the user turned the web on.
    pub(super) async fn picture_names(&self, call: &ToolCall, args: &Value, ctx: &tools::Ctx<'_>) -> (Value, Option<String>, bool) {
        let unchanged = (args.clone(), None, false);
        // Cloud models write full descriptions; the schema trick is for local engines.
        if self.ep.cloud.is_some() {
            return unchanged;
        }
        let field = match call.name.as_str() {
            "create_image" | "create_video" => "prompt",
            "edit_image" if matches!(args.get("action").and_then(Value::as_str).unwrap_or("edit"), "edit" | "restyle") => "instruction",
            _ => return unchanged,
        };
        let Some(prompt) = args.get(field).and_then(Value::as_str).map(str::trim).filter(|p| !p.is_empty()).map(str::to_string) else {
            return unchanged;
        };
        let asked = db::messages(&self.state.db.lock().unwrap(), &self.cipher, &self.chat_id)
            .iter()
            .rev()
            .find(|m| m.role == "user")
            .map(|m| m.content.clone())
            .unwrap_or_default();
        self.emit("chat:status", json!({ "chat_id": self.chat_id, "status": "looking_up" }));
        let (ask, format) = chat::names_check(&asked, &prompt);
        let mut names = match self.quiet_json(ask, format, 500).await {
            Some(reply) => chat::real_names(chat::names_of(&reply), &asked, &prompt),
            None => Vec::new(),
        };
        if names.is_empty() {
            self.emit("chat:status", json!({ "chat_id": self.chat_id, "status": "idle" }));
            return unchanged;
        }
        let mut notes: Vec<String> = Vec::new();
        let mut web_granted = false;
        let unsure: Vec<usize> = names.iter().enumerate().filter(|(_, n)| !n.described()).map(|(i, _)| i).take(3).collect();
        // With the web already on, the first named things are looked up even when
        // the model says it knows them: its memory of a look is often muddled
        // ("green tunic, blue tunic, red tunic"). With it off, it asks only when unsure.
        let free = crate::web::allowed_for_chat(&self.state.db.lock().unwrap(), &self.chat_id);
        let targets: Vec<usize> = if free { (0..names.len()).take(2).chain(unsure.iter().copied()).fold(Vec::new(), |mut v, i| { if !v.contains(&i) { v.push(i); } v }) } else { unsure.clone() };
        if !targets.is_empty() {
            let list = unsure.iter().map(|&i| names[i].name.trim().to_string()).collect::<Vec<_>>().join(", ");
            let mut can_search = free;
            if !can_search && !unsure.is_empty() && !crate::offline::blocks(crate::offline::Keep::Web) {
                let looks = if unsure.len() == 1 { "looks" } else { "look" };
                let ask_call = ToolCall {
                    id: format!("{}_web", call.id),
                    name: "request_web".into(),
                    arguments: json!({ "reason": format!("what {list} {looks} like, for the picture") }).to_string(),
                };
                web_granted = self.request_web(&ask_call).await.meta["web_granted"] == true;
                can_search = web_granted;
            }
            if can_search {
                for &i in &targets {
                    if self.cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    let name = names[i].name.trim().to_string();
                    let mut search = json!({ "query": format!("{name} appearance"), "count": 4 });
                    // Privacy mode covers these lookups too.
                    let privacy = crate::outgoing::mode(&self.state.db.lock().unwrap());
                    let cx = crate::outgoing::context(&self.state, &self.cipher);
                    if let Some(r) = crate::outgoing::review("web_search", &search, &cx) {
                        if privacy.on && r.level >= privacy.ask_from && r.level > self.state.approvals.shared_up_to(&self.chat_id) {
                            let look = ToolCall { id: format!("{}_look{i}", call.id), name: "web_search".into(), arguments: search.to_string() };
                            match self.share(&look, &r).await {
                                Ok(edits) => search = crate::outgoing::apply(&search, &r, &edits),
                                Err(_) => continue,
                            }
                        }
                        if let Some(sent) = crate::outgoing::review("web_search", &search, &cx) {
                            crate::outgoing::record(&self.state, &self.cipher, &self.chat_id, "web_search", &sent, true);
                        }
                    }
                    self.emit("chat:status", json!({ "chat_id": self.chat_id, "status": "looking_up", "detail": name }));
                    let found = tools::run("web_search", &search, ctx).await;
                    if found.meta["status"] != "ok" {
                        continue;
                    }
                    let text: String = found.text.chars().take((self.ep.ctx as usize / 3) * 3).collect();
                    let (ask, format) = chat::looks_from(&name, &text);
                    let looks = self.quiet_json(ask, format, 300).await.map(|r| chat::looks_of(&r)).unwrap_or_default();
                    if looks.split_whitespace().count() >= 4 {
                        names[i].known = true;
                        names[i].looks = looks;
                        notes.push(format!("looked up what {name} looks like on the web and described it in the picture's description"));
                        self.log(&format!("Looked up what {name} looks like, for a picture"));
                    }
                }
            }
            let still: Vec<String> = names.iter().filter(|n| !n.described()).map(|n| n.name.trim().to_string()).collect();
            if !still.is_empty() {
                notes.push(format!(
                    "it wasn't possible to find out what {} {} like, so the result may not match; tell the user that in a sentence",
                    still.join(", "),
                    if still.len() == 1 { "looks" } else { "look" }
                ));
            }
        }
        self.emit("chat:status", json!({ "chat_id": self.chat_id, "status": "idle" }));
        let mut args = args.clone();
        args[field] = json!(chat::with_looks(&prompt, &names));
        (args, (!notes.is_empty()).then(|| notes.join("; ")), web_granted)
    }

    /// A helper agent: fresh context, read-only tools, one task, a short
    /// report back. Its steps show inside the delegate card, not the chat.
    pub(super) async fn run_helper(&self, call_id: &str, task: &str, ctx: &tools::Ctx<'_>) -> Outcome {
        let short: String = task.chars().take(60).collect();
        let title = format!("Helper: {short}");
        let defs = tools::helper_definitions(!ctx.sandbox.is_empty());
        let folders: Vec<String> = ctx
            .sandbox
            .roots()
            .iter()
            .map(|r| if ctx.sandbox.is_general(r) { format!("{}/ (another project: only if the task is about it)", ctx.sandbox.display(r)) } else { format!("{}/", ctx.sandbox.display(r)) })
            .collect();
        let system = format!(
            "{}\n\nYou are a helper agent working for the main assistant. Do only the task below, using the read-only tools \
             on these folders: {}. Write paths starting with the folder name. Then reply with a concise, factual report of \
             what you found, citing file paths and line numbers. You can't change anything.",
            self.base,
            if folders.is_empty() { "(none shared)".to_string() } else { folders.join(", ") }
        );
        let msg = |role: &str, content: String| Message {
            id: uuid::Uuid::new_v4().to_string(),
            role: role.into(),
            content,
            ..Default::default()
        };
        let mut history = vec![msg("user", task.to_string())];
        let mut steps: Vec<String> = Vec::new();
        for _ in 0..crate::advanced::get(&self.state).helper_steps() {
            if self.cancel.load(Ordering::Relaxed) {
                return Outcome::error(title, "Stopped by the user.");
            }
            let kept = match chat::fit_history(&self.ep, &system, "", Some(&defs), &history).await {
                Ok((kept, _)) => kept,
                Err(e) => return Outcome::error(title, e),
            };
            let finished = match chat::stream(&self.ep, chat::api_messages(&system, &kept), Some(&defs), &self.cancel, |_| {}).await {
                Ok(f) => f,
                Err(e) => return Outcome::error(title, e),
            };
            let calls = finished.tool_calls.clone();
            let mut assistant = msg("assistant", finished.content.clone());
            assistant.tool_calls = (!calls.is_empty()).then(|| calls.clone());
            history.push(assistant);
            if calls.is_empty() {
                let detail = if steps.is_empty() { None } else { Some(steps.iter().map(|s| format!("• {s}")).collect::<Vec<_>>().join("\n")) };
                let report = if finished.content.trim().is_empty() { "The helper finished without a report.".to_string() } else { finished.content };
                return Outcome::ok(report, title, "text", detail);
            }
            for c in &calls {
                let args: Value = serde_json::from_str(if c.arguments.trim().is_empty() { "{}" } else { &c.arguments }).unwrap_or_default();
                let read_only = tools::find(&c.name).is_some_and(|d| d.risk == Risk::Read) && c.name != "delegate";
                // Privacy mode: helpers work on this PC only (nobody sees their calls to approve them).
                let sends = crate::outgoing::is_external(&c.name, &args) && crate::outgoing::mode(&self.state.db.lock().unwrap()).on;
                let out = if sends {
                    Outcome::error(tools::failed_title(&c.name, &args), "Privacy mode is on, so helpers can't send anything off this PC. Say what you'd look up; the main chat can do it with the person's OK.")
                } else if read_only {
                    tools::run(&c.name, &args, ctx).await
                } else {
                    Outcome::error(tools::failed_title(&c.name, &args), "Helpers can only look, not change anything.")
                };
                let step = out.meta.get("title").and_then(Value::as_str).unwrap_or(&c.name).to_string();
                self.emit("agent:helper", json!({ "chat_id": self.chat_id, "call_id": call_id, "step": step }));
                steps.push(step);
                let mut result = msg("tool", out.text);
                result.tool_call_id = Some(c.id.clone());
                history.push(result);
            }
        }
        Outcome::ok(
            format!("The helper ran out of steps. What it looked at:\n{}", steps.join("\n")),
            title,
            "text",
            Some(steps.join("\n")),
        )
    }

    /// A connector tool: the user's per-tool setting plus the run mode decide.
    pub(super) async fn execute_connector(&self, call: &ToolCall, t: &OfferedTool, rule: Option<Effect>) -> Outcome {
        let title = format!("{} ({})", t.tool, t.connector_name);
        let args: Value = match serde_json::from_str(if call.arguments.trim().is_empty() { "{}" } else { &call.arguments }) {
            Ok(v) => v,
            Err(e) => return Outcome::error(format!("Couldn't use {title}"), format!("The arguments weren't valid JSON ({e}).")),
        };
        if rule == Some(Effect::Deny) {
            self.log(&format!("A saved rule refused: {title}"));
            return Outcome::denied(format!("Use {title}")).with_text(RULE_REFUSED);
        }
        let mut permission = match (self.mode, t.mode) {
            (_, ToolMode::Off) => Permission::Refuse,
            (Mode::Plan, _) if !t.read_only => Permission::Refuse,
            // What a connector on the web is given leaves the PC: each call
            // asks, in every mode, unless the person allowed that tool by
            // name (on the connector's page, or in a saved rule).
            (_, ToolMode::Allow) if t.web.is_some() => Permission::Run,
            _ if t.web.is_some() && rule == Some(Effect::Allow) => Permission::Run,
            _ if t.web.is_some() => Permission::Ask,
            (Mode::Plan, _) | (Mode::Bypass, _) | (Mode::Auto, ToolMode::Allow) => Permission::Run,
            (Mode::Auto, _) if self.state.approvals.allowed(&self.chat_id, Risk::Connector) || rule == Some(Effect::Allow) => Permission::Run,
            (Mode::Auto, ToolMode::Ask) => Permission::Ask,
        };
        if rule == Some(Effect::Ask) && permission == Permission::Run {
            permission = Permission::Ask;
        }
        // Privacy mode: a connector may pass what it's given on to an online service.
        let privacy = crate::outgoing::mode(&self.state.db.lock().unwrap());
        let all = crate::outgoing::review_connector(&t.connector_name, &args, &crate::outgoing::context(&self.state, &self.cipher).me);
        let review = privacy.on.then(|| all.clone());
        match permission {
            Permission::Refuse => return Outcome::error(format!("Couldn't use {title}"), "This connector tool isn't allowed in this mode."),
            Permission::Ask => {
                let note = match &t.web {
                    Some(host) => format!("This sends what's shown to {host}. To stop being asked for this tool, allow it on the connector's page."),
                    None => "Connectors are programs on this PC; what this does is up to the connector.".to_string(),
                };
                let preview = Preview {
                    title: format!("Use {} from the {} connector", t.tool, t.connector_name),
                    kind: "text",
                    detail: Some(serde_json::to_string_pretty(&args).unwrap_or_default()),
                    note: Some(match &review {
                        Some(r) => format!("{note}\n{}", crate::outgoing::summary(r)),
                        None => note,
                    }),
                };
                // On the web each call is asked about on its own, never for the whole chat.
                let risk = if t.web.is_some() { Risk::Submit } else { Risk::Connector };
                if self.ask(call, risk, preview).await == Decision::Deny {
                    self.log(&format!("Declined: {title}"));
                    return Outcome::denied(format!("Use {title}"));
                }
            }
            Permission::Run => {
                if let Some(r) = review.filter(|r| r.level >= privacy.ask_from && r.level > self.state.approvals.shared_up_to(&self.chat_id)) {
                    if let Err(refused) = self.share(call, &r).await {
                        return refused;
                    }
                }
            }
        }
        self.emit("agent:tool_start", json!({ "chat_id": self.chat_id, "call_id": call.id, "tool": call.name }));
        match &t.web {
            Some(host) => self.log(&format!("Sent a request to connector {} ({host}): {}", t.connector_name, t.tool)),
            None => self.log(&format!("Used {title}")),
        }
        crate::outgoing::record(&self.state, &self.cipher, &self.chat_id, &call.name, &all, true);
        match connectors::call(&self.state, &t.connector_id, &t.tool, args).await {
            Ok((false, text)) => Outcome::ok(text.clone(), format!("Used {title}"), "text", Some(text)),
            Ok((true, text)) => Outcome::error(format!("{title} reported a problem"), text),
            Err(e) => Outcome::error(format!("Couldn't use {title}"), e),
        }
    }

    /// use_screen: the screen agent's loop (`screen_agent`), each step shown
    /// in the chat as it's done, and a step that sends, buys or deletes put
    /// to the person on a card of its own.
    pub(super) async fn use_screen(&self, call: &ToolCall, task: &str) -> Outcome {
        let chat_model = db::chat(&self.state.db.lock().unwrap(), &self.cipher, &self.chat_id).and_then(|c| c.model_id);
        let n = std::sync::atomic::AtomicU32::new(0);
        let ask = |title: String, detail: String| {
            let k = n.fetch_add(1, Ordering::Relaxed);
            let step_call = ToolCall { id: format!("{}-step{k}", call.id), name: call.name.clone(), arguments: call.arguments.clone() };
            async move {
                let note = Some("Asking because this step could send, buy, delete or close something.".to_string());
                matches!(self.ask(&step_call, Risk::Submit, Preview { title, kind: "text", detail: Some(detail), note }).await, Decision::Allow | Decision::AllowForChat)
            }
        };
        let step = |line: &str| self.emit("agent:helper", json!({ "chat_id": self.chat_id, "call_id": call.id, "step": line }));
        let title = format!("Used the screen: {}", task.chars().take(60).collect::<String>());
        match crate::screen_agent::run(&self.state, &self.chat_id, chat_model.as_deref(), task, &self.cancel, ask, step).await {
            Ok(text) => Outcome::ok(text, title, "text", None),
            Err(e) => Outcome::error("Couldn't use the screen", e),
        }
    }

    /// research in depth, with this chat's model planning and writing, and
    /// each step shown under the tool as it goes.
    pub(super) async fn deep_research(&self, call: &ToolCall, args: &Value) -> Outcome {
        let prepared = {
            let conn = self.state.db.lock().unwrap();
            crate::web::prepare(&conn, &self.cipher, &self.chat_id)
        };
        let (client, settings, key) = match prepared {
            Ok(x) => x,
            Err(e) => return Outcome::error("Web access is off", e),
        };
        let w = tools::lookup::Web { client, settings, key, app: self.state.app.get().cloned() };
        let project = db::chat(&self.state.db.lock().unwrap(), &self.cipher, &self.chat_id).and_then(|c| c.project_id);
        let memory = tools::MemoryCtx { db: &self.state.db, cipher: &self.cipher, chat_id: &self.chat_id, project_id: project.as_deref() };
        let step = |line: &str| self.emit("agent:helper", json!({ "chat_id": self.chat_id, "call_id": call.id, "step": line }));
        tools::lookup::deep(args, &w, &memory, &self.ep, &self.cancel, &step).await
    }

    pub(super) async fn ask(&self, call: &ToolCall, risk: Risk, preview: Preview) -> Decision {
        self.ask_offering(call, risk, preview, None).await
    }

    pub(super) async fn ask_offering(&self, call: &ToolCall, risk: Risk, preview: Preview, rule: Option<allowances::Suggestion>) -> Decision {
        if self.unattended {
            self.log(&format!("Declined {} in a scheduled task: nobody was there to approve it", call.name));
            return Decision::Deny;
        }
        let (tx, mut rx) = oneshot::channel();
        let args: Value = serde_json::from_str(&call.arguments).unwrap_or_default();
        let once = risk == Risk::Submit || tools::destructive_call(&call.name, &args);
        let pending = PendingApproval { chat_id: self.chat_id.clone(), call_id: call.id.clone(), tool: call.name.clone(), risk, preview, once, rule: rule.filter(|_| !once) };
        self.state.approvals.waiting.lock().unwrap().insert(call.id.clone(), (pending.clone(), tx));
        self.emit("agent:approval", serde_json::to_value(&pending).unwrap_or_default());
        let decision = loop {
            tokio::select! {
                d = &mut rx => break d.unwrap_or(Decision::Deny),
                _ = tokio::time::sleep(Duration::from_millis(250)) => {
                    if self.cancel.load(Ordering::Relaxed) {
                        self.state.approvals.waiting.lock().unwrap().remove(&call.id);
                        break Decision::Deny;
                    }
                }
            }
        };
        self.emit("agent:approval_done", json!({ "chat_id": self.chat_id, "call_id": call.id }));
        decision
    }

    /// Whether running this lookup could stop to ask the person: a page or
    /// search going off the PC is reviewed (and asks in privacy mode),
    /// another chat's handoff note asks, a saved rule can ask, and
    /// Settings › Advanced can ask before every read. Those run on their
    /// own, never in a group.
    pub(super) fn might_ask(&self, call: &ToolCall, args: &Value, sandbox: &Sandbox, project_id: Option<&str>, extras: &Extras) -> bool {
        if crate::outgoing::is_external(&call.name, args) || tools::egress_call(&call.name, args, sandbox).is_some() {
            return true;
        }
        if crate::advanced::get(&self.state).must_ask(Risk::Read) {
            return true;
        }
        if allowances::decide(&extras.allowances, project_id, &call.name, args, sandbox) == Some(Effect::Ask) || extras.checks_at(plugin_checks::Event::BeforeTool, Some(&call.name)) {
            return true;
        }
        call.name == "read_file"
            && args
                .get("path")
                .and_then(Value::as_str)
                .and_then(|p| sandbox.resolve(p).ok())
                .is_some_and(|p| crate::handoff_guard::guarded(&self.state, &self.cipher, &self.chat_id, &p).is_some())
    }

    /// Reading another chat's handoff note (read_file, or a command naming
    /// one) while chats don't work together: allowed when the person's own
    /// message asks to pick up where another chat left off, else only from a
    /// card they answer (in every mode, Bypass too).
    pub(super) async fn handoff_gate(&self, call: &ToolCall, args: &Value, sandbox: &Sandbox) -> Option<Outcome> {
        let (path, from) = match call.name.as_str() {
            "read_file" => {
                let path = sandbox.resolve(args.get("path")?.as_str()?).ok()?;
                let from = crate::handoff_guard::guarded(&self.state, &self.cipher, &self.chat_id, &path)?;
                (Some(path), from)
            }
            "run_command" => {
                let command = args.get("command")?.as_str()?;
                if !crate::handoff_guard::command_names_one(command) || crate::features::is_on(&self.state.db.lock().unwrap(), Feature::ChatTeam) {
                    return None;
                }
                (None, "another chat".to_string())
            }
            _ => return None,
        };
        let asked = {
            let conn = self.state.db.lock().unwrap();
            db::messages(&conn, &self.cipher, &self.chat_id).into_iter().rev().find(|m| m.role == "user").map(|m| m.content).unwrap_or_default()
        };
        let name = path.as_ref().map_or_else(|| "a handoff note".to_string(), |p| p.file_name().unwrap_or_default().to_string_lossy().into_owned());
        if !crate::handoff_guard::asked_to_pick_up(&asked) {
            let preview = Preview {
                title: format!("Let this chat read {name}, a handoff from {from}?"),
                kind: "handoff",
                detail: Some(format!(
                    "{name} is a note {from} left for picking up its work. Chats don't share work while “Chats work together” is off. Allow it only if you want this chat to continue where that one left off."
                )),
                note: None,
            };
            if self.ask(call, Risk::Read, preview).await == Decision::Deny {
                self.log(&format!("Kept {name} (another chat's handoff) from this chat"));
                return Some(Outcome::denied(format!("Read {name}")).with_text(
                    "That's a handoff note another chat left, and the user didn't let this chat pick it up. Don't read it another way (search, commands); carry on with what they asked here.",
                ));
            }
        }
        if let Some(p) = &path {
            crate::handoff_guard::allow(&self.state, &self.cipher, &self.chat_id, p);
        }
        None
    }
}

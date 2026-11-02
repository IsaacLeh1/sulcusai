// SPDX-License-Identifier: AGPL-3.0-only
//! One turn of a chat: the model chosen and loaded, the reply streamed,
//! memories and thinking kept, and work waiting its turn on the card.

use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) async fn run_turn_with(
    app: &AppHandle,
    state: &Arc<AppState>,
    chat_id: String,
    text: String,
    images: Vec<String>,
    tee: Option<Tee>,
    asking: Option<Asking>,
    again: Option<Again>,
) -> Result<Option<Message>, String> {
    let mut text = text.trim().to_string();
    if text.is_empty() && images.is_empty() {
        return Err("Type a message first.".into());
    }
    if text.is_empty() {
        text = if images.len() == 1 { "What's in this picture?".into() } else { "What's in these pictures?".into() };
    }
    let cipher = state.cipher()?;
    let Some((cancel, _guard)) = claim(&state.generations, &chat_id) else {
        return Err("Still answering the last message.".into());
    };
    let _live = LiveGuard(chat_id.clone());
    sync::people::turn_changed(state, &chat_id, true);
    let _shared = SharedTurn(state.clone(), chat_id.clone());
    // Messages typed meanwhile: added to this reply, or queued for after it.
    let inbox = inbox::Opened::new(app, &chat_id);
    let asked_in = chat_id.clone();
    let _asked = asking.as_ref().map(|_| tools::chats::Asked::new(&chat_id));

    let mut turn_id = uuid::Uuid::new_v4().to_string();
    // Asking again: what followed is kept as a version first. Another reply
    // answers the message that's there; an edit is sent as a new one.
    let mut answer_again = false;
    if let Some(again) = &again {
        {
            let conn = state.db.lock().unwrap();
            let all = db::messages(&conn, &cipher, &chat_id);
            let (id, edit) = match again {
                Again::Reply(id) => (id, false),
                Again::Edit(id) => (id, true),
            };
            let at = all.iter().position(|m| m.id == *id).ok_or("That message isn't in this chat any more.")?;
            let anchor = if !edit { id.clone() } else if at == 0 { String::new() } else { all[at - 1].id.clone() };
            versions::set_aside(&conn, &cipher, &chat_id, &anchor)?;
            if !edit {
                turn_id = id.clone();
                answer_again = true;
            }
        }
        state.contexts.lock().unwrap().remove(&chat_id);
        app.emit("chat:changed", json!({ "chat_id": chat_id })).ok();
    }
    let (chat, profile) = {
        let conn = state.db.lock().unwrap();
        let chat = db::chat(&conn, &cipher, &chat_id).ok_or("That chat no longer exists.")?;
        if chat.model_id.is_none() {
            if let Some(m) = db::settings(&conn).default_model {
                db::set_chat_model(&conn, &chat_id, &m)?;
            }
        }
        (chat, db::profile(&conn, &cipher))
    };
    let mode = tools::Mode::parse(&chat.mode);
    // Pictures in this message or earlier ones: start the model with its image encoder.
    let pictures = !images.is_empty()
        || db::messages(&state.db.lock().unwrap(), &cipher, &chat_id).iter().any(|m| m.meta.as_ref().is_some_and(|v| v.get("images").is_some()));
    // Its turn for the graphics card (`lineup`), held while it replies.
    let mut _line: Option<lineup::Turn> = None;
    let (ep, use_tools, vision, answering) = match &asking {
        Some(a) => (a.ep.clone(), a.tools, a.vision, None),
        None => {
            // Auto: a message the chat's model isn't the best installed one
            // for goes to the one that is, for this message only (`router`).
            // Not with pictures: the other model may not see them.
            let routed = if pictures { None } else { router::route(state, chat.model_id.as_deref(), &text) };
            if let Some(p) = &routed {
                state.log("chat", &format!("Auto: {} answers this message (better at {})", p.name, p.task.word()));
                app.emit("chat:routed", json!({ "chat_id": chat_id, "model_id": p.model_id, "name": p.name, "task": p.task.word() })).ok();
            }
            let model_id = routed.map(|p| p.model_id).or_else(|| chat.model_id.clone());
            let local = model_id.clone().or_else(|| state.settings().default_model).filter(|id| !cloud::is_cloud(id) && !sync::compute::is_compute(id));
            if let Some(id) = local {
                let entry = lineup::Entry::chat(&chat_id, &chat.title, &id, chat_card_need(state, &id));
                _line = Some(
                    lineup::wait(state, entry, cancel.clone(), |place| {
                        app.emit("chat:status", json!({ "chat_id": chat_id, "status": "waiting_card", "place": place })).ok();
                    })
                    .await?,
                );
            }
            let (ep, spec, _) = llm_endpoint(state, model_id, pictures, chat_waiting(app, &chat_id)).await?;
            let vision = if ep.cloud.is_some() { spec.vision.is_some() } else { state.engine.lock().await.sees() };
            (ep, spec.tools, vision, Some(spec.id))
        }
    };

    let today = chrono::Local::now().format("%A, %B %-d, %Y").to_string();
    let (base, about) = chat::system_prompt(&profile, &today);
    let base = format!("{}\n\n{}", advanced::get(state).base_prompt(base), chat::runs_where(&ep));
    // The answering model's own instructions (its settings in Models).
    let base = match &answering {
        Some(id) => presets::with_instructions(base, &presets::get(&state.db.lock().unwrap(), id)),
        None => base,
    };
    // A chat started with one of the person's own assistants.
    let own = assistants::of_chat(&state.db.lock().unwrap(), &cipher, &chat_id);
    let base = match own {
        Some(a) => assistants::with_instructions(base, &a),
        None => base,
    };

    // Nearly full: compact it (summarize the earlier part and keep going
    // here), or continue in a fresh chat, as set in Advanced.
    let last_context = state.contexts.lock().unwrap().get(&chat_id).cloned().or_else(|| estimate_context(state, &chat_id));
    let mut chat_id = chat_id;
    let long = advanced::get(state).long_chat();
    if let advanced::LongChat::Compact(at) = long {
        if !chat.incognito && handoff::needed(last_context.as_ref(), Some(at)) {
            app.emit("chat:status", json!({ "chat_id": chat_id, "status": "compacting" })).ok();
            let history = db::messages(&state.db.lock().unwrap(), &cipher, &chat_id);
            let covered = compact::pending(&history);
            if let Ok(summary) = compact::summarize(&ep, &base, history, &cancel).await {
                compact::save(&state.db.lock().unwrap(), &cipher, &chat_id, &summary, covered, true)?;
                state.contexts.lock().unwrap().remove(&chat_id);
                state.log("chat", "Compacted a long chat (it was nearly full)");
                app.emit("chat:compacted", json!({ "chat_id": chat_id, "auto": true })).ok();
            }
        }
    }
    if let advanced::LongChat::NewChat(at) = long {
        if !chat.incognito && handoff::needed(last_context.as_ref(), Some(at)) {
            app.emit("chat:status", json!({ "chat_id": chat_id, "status": "handoff" })).ok();
            let history = db::messages(&state.db.lock().unwrap(), &cipher, &chat_id);
            let summary = handoff::summarize(&ep, &base, &history, &cancel).await?;
            let new = handoff::continue_in_new_chat(&state.db.lock().unwrap(), &cipher, &chat, &summary)?;
            state.log("chat", "A long chat continued in a new chat with a summary");
            app.emit("chat:handoff", json!({ "from": chat_id, "to": new.id })).ok();
            chat_id = new.id;
        }
    }

    // What a cloud model's replies cost counts toward this chat.
    if let Some(t) = &ep.cloud {
        t.for_chat(&chat_id);
    }
    let first_message;
    if answer_again {
        first_message = false;
    } else {
        let conn = state.db.lock().unwrap();
        // An edited first message keeps the chat's name.
        let first = db::message_count(&conn, &chat_id) == 0 && again.is_none();
        first_message = first;
        db::add_message(&conn, &cipher, &Message {
            id: turn_id.clone(),
            chat_id: chat_id.clone(),
            role: "user".into(),
            content: text.clone(),
            created_at: db::now_ms(),
            meta: match &asking {
                Some(a) => Some(json!({ "from_chat": a.from.0, "from_title": a.from.1 })),
                None => (!images.is_empty()).then(|| json!({ "images": images })),
            },
            ..Default::default()
        })?;
        // A chat started by another chat keeps the title it was given.
        if first && asking.is_none() {
            db::set_chat_title(&conn, &cipher, &chat_id, &chat::title_from(&text))?;
        }
    }
    // Shows at once in a window on this chat, even when the message came
    // from the queue, another chat or the phone rather than that window.
    app.emit("chat:changed", json!({ "chat_id": chat_id })).ok();
    inbox::sent(app, &asked_in);

    // After the first reply, the model names the chat from the request.
    let name_it = (first_message && !chat.incognito && asking.is_none()).then(|| (ep.clone(), text.clone()));
    // Its thinking, in short, once it has answered: on this PC only (online
    // it would be another paid request).
    let sum_up = ep.cloud.is_none().then(|| (ep.clone(), turn_id.clone()));
    let emitter = app.clone();
    let watched = state.clone();
    let turn = agent::Turn {
        emit: Arc::new(move |event: &str, payload: serde_json::Value| {
            if let Some(t) = &tee {
                t(event, &payload);
            }
            track_live(event, &payload);
            sync::people::chat_event(&watched, event, &payload);
            emitter.emit(event, payload).ok();
        }),
        state: state.clone(),
        chat_id: chat_id.clone(),
        turn_id,
        cipher,
        ep,
        mode,
        use_tools,
        vision,
        unattended: false,
        pre_approved: None,
        base,
        about,
        cancel,
    };
    let result = turn.run().await;
    let (last, context, tps, cancelled) = match result {
        Ok(r) => (r.last, r.context, r.tps, r.cancelled),
        Err(e) => {
            app.emit("chat:done", json!({ "chat_id": chat_id, "error": e })).ok();
            return Err(e);
        }
    };
    state.contexts.lock().unwrap().insert(chat_id.clone(), context.clone());
    // Kept, so the meter has it when the chat is opened after a restart (and
    // the instructions' share, for chats it works out from their messages).
    {
        let conn = state.db.lock().unwrap();
        let _ = db::set(&conn, &format!("context:{chat_id}"), &context);
        let _ = db::set(&conn, "context:last", &context);
    }
    if let Some((ep, request)) = name_it.filter(|_| !cancelled) {
        let (state, app, chat_id) = (state.clone(), app.clone(), chat_id.clone());
        tauri::async_runtime::spawn(async move {
            if let Some(title) = chat::summary_title(&ep, &request).await {
                if let Ok(c) = state.cipher() {
                    if db::set_chat_title(&state.db.lock().unwrap(), &c, &chat_id, &title).is_ok() {
                        app.emit("chat:titled", json!({ "chat_id": chat_id, "title": title })).ok();
                    }
                }
            }
        });
    }
    if let Some((ep, asked)) = sum_up.filter(|_| !cancelled) {
        let (state, app, chat_id) = (state.clone(), app.clone(), chat_id.clone());
        tauri::async_runtime::spawn(async move {
            sum_up_thinking(&state, &app, &ep, &chat_id, &asked).await;
            remember_from_reply(&state, &app, &ep, &chat_id, &asked).await;
        });
    }
    app.emit("chat:done", json!({ "chat_id": chat_id, "message": last, "tps": tps, "context": context, "cancelled": cancelled }))
        .ok();
    inbox.finish(state, cancelled);
    Ok(last)
}

/// Lasting facts from the reply to `asked`, saved without being asked when
/// the model finds any (Memory: "Remember things from chats by itself"):
/// not when it saved one with remember this turn, in incognito chats, or
/// with memory off. A project chat's facts about the project stay in it.
pub(super) async fn remember_from_reply(state: &AppState, app: &AppHandle, ep: &engine::Endpoint, chat_id: &str, asked: &str) {
    let Ok(c) = state.cipher() else { return };
    if !state.settings().memory_enabled {
        return;
    }
    let (chat, messages, known) = {
        let conn = state.db.lock().unwrap();
        if !memory::auto_on(&conn) || !features::is_on(&conn, features::Feature::Memory) {
            return;
        }
        let Some(chat) = db::chat(&conn, &c, chat_id) else { return };
        (chat, db::messages(&conn, &c, chat_id), memory::list(&conn, &c, None))
    };
    let turn: Vec<&Message> = messages.iter().skip_while(|m| m.id != asked).collect();
    let Some(user) = turn.first().filter(|m| m.role == "user") else { return };
    if chat.incognito || turn.iter().any(|m| m.tool_calls.iter().flatten().any(|t| t.name == "remember")) {
        return;
    }
    let reply = turn.iter().filter(|m| m.role == "assistant").map(|m| m.content.as_str()).collect::<Vec<_>>().join("\n");
    let Ok(raw) = chat::complete(ep, memory::auto_prompt(&known, &user.content, &reply), json!({ "response_format": memory::auto_format() }), 300).await else { return };
    let mut saved = Vec::new();
    {
        let conn = state.db.lock().unwrap();
        for (fact, about_user) in memory::auto_facts(&raw) {
            let project = if about_user { None } else { chat.project_id.as_deref() };
            if let Ok((m, true)) = memory::add(&conn, &c, &fact, project, Some(chat_id)) {
                saved.push(m.content);
            }
        }
    }
    if !saved.is_empty() {
        state.log("chat", &format!("Remembered {} thing(s) from a chat by itself", saved.len()));
        app.emit("memory:auto", json!({ "chat_id": chat_id, "saved": saved })).ok();
    }
}

/// The thinking of the reply to `asked` (the user's message), every step of
/// it, in a few short points: kept with the answer (the reply's last
/// message), shown between the whole thought process and the answer.
pub(super) async fn sum_up_thinking(state: &AppState, app: &AppHandle, ep: &engine::Endpoint, chat_id: &str, asked: &str) {
    let Ok(c) = state.cipher() else { return };
    let reply: Vec<Message> = db::messages(&state.db.lock().unwrap(), &c, chat_id).into_iter().skip_while(|m| m.id != asked).skip(1).filter(|m| m.role == "assistant").collect();
    let thinking = reply.iter().filter_map(|m| m.thinking.as_deref()).fold(String::new(), |all, t| chat::joined(&all, t));
    // A short think is its own summary.
    let Some(last) = reply.last().filter(|_| thinking.trim().chars().count() >= 400) else { return };
    let Some(summary) = chat::thought_summary(ep, &thinking).await else { return };
    let mut meta = last.meta.clone().filter(|v| v.is_object()).unwrap_or_else(|| json!({}));
    meta["thought_summary"] = json!(summary);
    if db::set_message_meta(&state.db.lock().unwrap(), &c, &last.id, &meta).is_ok() {
        app.emit("chat:thought", json!({ "chat_id": chat_id })).ok();
    }
}

/// A model that didn't load: why, said plainly (what the engine's log
/// shows, else what the model's size says), and the whole story in
/// crashes.log with what each line of the engine's log means.
pub(super) fn load_failed(state: &AppState, spec: &ModelSpec, quant: &str, ctx: u32, layers: u32, want: memcheck::Need, e: String) -> String {
    if !e.starts_with(engine::STOPPED_LOADING) {
        crate::applog::line("error", &format!("{} didn't load: {e}", spec.name));
        return e;
    }
    let log = crate::applog::tail(&state.paths.engine_log(), 64 * 1024);
    let last: Vec<&str> = log.lines().rev().take(60).collect::<Vec<_>>().into_iter().rev().collect();
    let last = last.join("\n");
    let gb = |b: u64| format!("{:.1} GB", b as f64 / 1e9);
    let budget = state.budget();
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    let (ram_total, ram_free) = (sys.total_memory(), sys.available_memory());
    let size = spec.variant(quant).map_or(0, |v| v.size);
    let reason = match crate::applog::diagnose(&last) {
        Some(m) => m.to_string(),
        // Nothing in the log says: what the size says.
        None if size > ram_total + budget.vram => format!(
            "It's too large for this PC: the model alone is {}, and this PC has {} of memory and {} on the graphics card. Pick a smaller model or a smaller version of it (a lower quant).",
            gb(size), gb(ram_total), gb(budget.vram)
        ),
        None if want.ram > ram_free => format!(
            "This PC didn't have enough free memory: loading it this way needs about {} and {} was free. Close other apps, choose a smaller context window, or a smaller version of the model.",
            gb(want.ram), gb(ram_free)
        ),
        None if budget.vram > 0 && want.vram > budget.vram => format!(
            "It's too large for the graphics card as set up: it needed about {} there and the card has {}. Lower the GPU limit or the context window at the top of the chat, or pick a smaller version.",
            gb(want.vram), gb(budget.vram)
        ),
        None => "The engine closed without saying why. The usual causes are memory running out as it loaded (close other apps, or pick a smaller version), a graphics driver problem (update the driver), or an antivirus stopping the engine. Settings, Logs, crashes.log has the details.".to_string(),
    };
    crate::applog::crash(
        &format!("{} didn't load: the engine stopped while loading it", spec.name),
        &format!(
            "Why: {reason}\n\nThe model: {} ({quant}), {} file, {ctx}-token context window, {layers} layers planned on the graphics card.\n\
             This PC: {} memory ({} free), {} usable on the graphics card ({} card{}).\nPlanned use: about {} of memory and {} on the card.\n\n\
             The engine's last lines (engine.log), with what the known ones mean:\n{}",
            spec.name,
            gb(size),
            gb(ram_total),
            gb(ram_free),
            gb(budget.vram),
            budget.cards,
            if budget.cards == 1 { "" } else { "s" },
            gb(want.ram),
            gb(want.vram),
            crate::applog::annotated(&last)
        ),
    );
    format!("{} couldn't load: the engine stopped while loading it. {reason}", spec.name)
}

/// The graphics memory a reply on this model takes (its weights and its
/// window, as planned), for its place in the card's line; 0 when unknown.
pub(super) fn chat_card_need(state: &AppState, model_id: &str) -> u64 {
    let id = finetune::parse_ref(model_id).and_then(|a| finetune::adapter(state, a)).map_or(model_id.to_string(), |a| a.base);
    let Some(spec) = state.model_spec(&id) else { return 0 };
    let quant = db::installed_model(&state.db.lock().unwrap(), &id).map(|m| m.quant).unwrap_or_else(|| spec.variants[0].quant.clone());
    let Some(v) = spec.variant(&quant) else { return 0 };
    let ctx = plan_with(state, &spec, &quant, 0, true).ctx;
    v.size + catalog::kv_cache_bytes(&spec.arch, ctx) + (700 << 20)
}

/// A chat's status while its model is on the way: loading, or its place in
/// line behind other chats using another model.
pub(crate) fn chat_waiting<'a>(app: &'a AppHandle, chat_id: &'a str) -> impl FnMut(engine::Wait) + 'a {
    move |w| {
        let status = match w {
            engine::Wait::Loading => json!({ "chat_id": chat_id, "status": "loading" }),
            engine::Wait::Line(place, other) => json!({ "chat_id": chat_id, "status": "queued", "place": place, "detail": other }),
        };
        app.emit("chat:status", status).ok();
    }
}

/// How long a request for another model waits for the replies using the
/// loaded one to finish (an agent task can take minutes).
pub(super) const WAIT_FOR_REPLIES: std::time::Duration = std::time::Duration::from_secs(180);

/// How long a request waits in line for another model (the chats ahead of
/// it can each run a long agent task).
pub(super) const WAIT_IN_LINE: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// The stop flags of the replies running now.
pub(super) fn replies(state: &AppState) -> Vec<Arc<AtomicBool>> {
    state.generations.lock().unwrap().values().cloned().collect()
}

/// Starts (or reuses) the engine for a model: the given one, else the
/// default. Returns the endpoint and the model. Another model isn't stopped
/// while replies are using it: one model runs at a time, so this waits in
/// line (`on_wait` hears its place) for them, then loads it.
pub(crate) async fn llm_endpoint(
    state: &Arc<AppState>,
    model_id: Option<String>,
    pictures: bool,
    on_wait: impl FnMut(engine::Wait),
) -> Result<(engine::Endpoint, ModelSpec, InstalledModel), String> {
    // Replies running before this request that weren't given a model yet:
    // this request's own, when a chat makes it.
    let before = engine::USERS.lock().unwrap().unplaced(&replies(state));
    let task = tokio::task::try_id();
    let id = {
        let conn = state.db.lock().unwrap();
        let default = db::settings(&conn).default_model;
        // A chat whose model was removed (by hand or by the cleanup) uses the default.
        let gone = |id: &String| !cloud::is_cloud(id) && !sync::compute::is_compute(id) && finetune::parse_ref(id).is_none() && steer::parse_ref(id).is_none() && db::installed_model(&conn, id).is_none();
        let chosen = match model_id {
            Some(id) if gone(&id) && default.as_ref().is_some_and(|d| !gone(d)) => default.clone(),
            other => other,
        };
        chosen.or(default).ok_or("Install a model first: open Models and pick one.")?
    };
    if cloud::is_cloud(&id) {
        *state.engine_used.lock().unwrap() = std::time::Instant::now();
        let found = cloud::endpoint(state, &id, &state.cipher()?)?;
        engine::USERS.lock().unwrap().place(&replies(state), &before, "", task);
        return Ok(found);
    }
    // A model on the compute PC: its turn is taken there.
    if sync::compute::is_compute(&id) {
        let found = sync::compute::endpoint(state, &id).await?;
        engine::USERS.lock().unwrap().place(&replies(state), &before, "", task);
        return Ok(found);
    }
    if finetune::training() {
        return Err("A model is being taught right now and has the graphics card. Chats on this PC's models work again when it's done (or stop it in Settings → Advanced). Cloud models still work.".into());
    }
    if uncensor::running() {
        return Err("An uncensored copy of a model is being made and has the graphics card. Chats on this PC's models work again when it's done (or stop it on the Models page). Cloud models still work.".into());
    }
    // A taught model: its base model with the adapter on top.
    let taught = match finetune::parse_ref(&id) {
        Some(aid) => Some(finetune::adapter(state, aid).ok_or("That taught model was removed. Pick another one.")?),
        None => None,
    };
    // A steered model: its model with a direction from the Lens added.
    let direction = match steer::parse_ref(&id) {
        Some(sid) => Some(steer::get(&state.db.lock().unwrap(), &state.work_cipher()?, sid).ok_or("That steering direction was removed. Pick another model.")?),
        None => None,
    };
    let steering = direction.as_ref().map(|d| d.steer(&state.paths));
    let base_id = taught.as_ref().map_or_else(|| direction.as_ref().map_or(id.clone(), |d| d.model_id.clone()), |a| a.base.clone());
    let installed = db::installed_model(&state.db.lock().unwrap(), &base_id).ok_or("This chat's model isn't installed anymore. Pick another one.")?;
    cleanup::touch_model(state, &installed.model_id);
    let mut spec = state.model_spec(&installed.model_id).ok_or("This model is no longer available. Pick another one.")?;
    if let Some(a) = &taught {
        spec.name = format!("{} + {}", spec.name, a.name);
    }
    if let Some(d) = &direction {
        spec.name = format!("{}, steered: {} ({:+.1})", spec.name, d.name, d.strength);
    }
    let lora = taught.as_ref().map(|a| std::path::PathBuf::from(&a.path));
    recheck_if_card_missing(state).await;
    let limits = state.limits();
    let backend = engine::backend_for(&state.budget())?;
    let exe = match engine::installed_server(&state.paths, &state.catalog.engine, backend) {
        Some(exe) => exe,
        // A model found on this PC before the engine was ever downloaded.
        None => {
            let asset = state.catalog.engine.assets.get(backend).map(|a| a.url.clone()).unwrap_or_default();
            state.log("network", &format!("Downloading the llama.cpp engine ({backend}) from {}", host_of(&asset)));
            let app = state.app.get().cloned();
            let progress = |received: u64, total: u64| {
                if let Some(app) = &app {
                    app.emit("engine:download", json!({ "received": received, "total": total })).ok();
                }
            };
            let exe = engine::ensure_installed(&state.paths, &state.catalog.engine, backend, &AtomicBool::new(false), progress).await;
            if let Some(app) = &app {
                app.emit("engine:download", json!({ "done": true })).ok();
            }
            exe?
        }
    };
    *state.engine_used.lock().unwrap() = std::time::Instant::now();
    let model_path = model_file(&state.paths, &installed);
    // A damaged or unfinished file is said plainly, not as the engine's log.
    if let Some(why) = model_file_problem(&model_path, installed.size) {
        state.log("model", &format!("{}'s model file is unusable ({why}): {}", spec.name, model_path.display()));
        return Err(format!("{}'s model file is damaged: {why}. Open Models and press Download again on it.", spec.name));
    }
    let encoder = if pictures { ensure_vision(state, &spec).await? } else { vision_file(&state.paths, &spec) };
    // Said once each time it changes.
    let mut on_wait = on_wait;
    let mut told: Option<engine::Wait> = None;
    let mut tell = move |w: engine::Wait| {
        if told.as_ref() != Some(&w) {
            told = Some(w.clone());
            on_wait(w);
        }
    };
    let mut ticket: Option<engine::Ticket> = None;
    let asked = std::time::Instant::now();
    let mut ep = loop {
        let mut engine = state.engine.lock().await;
        // Keep the image encoder once it's loaded, so chats don't flip it on and off.
        let keep = engine.endpoint_steered(&installed.model_id, lora.as_deref(), steering.as_ref()).is_some() && engine.sees();
        let mmproj = encoder.as_deref().filter(|_| pictures || keep);
        let encoder_size = mmproj.and_then(|p| std::fs::metadata(p).ok()).map_or(0, |m| m.len());
        let plan = launch_plan(state, &spec, &installed.quant, encoder_size);
        let launch = LaunchSpec {
            exe: &exe,
            model_path: &model_path,
            model_id: &installed.model_id,
            quant: &installed.quant,
            ctx: plan.ctx,
            gpu_layers: plan.gpu_layers,
            threads: plan.threads,
            low_priority: limits.low_priority,
            log: &state.paths.engine_log(),
            mmproj,
            batch: plan.batch,
            lora: lora.as_deref(),
            steer: steering.as_ref(),
        };
        // Other replies still using the model loaded now: it isn't stopped
        // under them (they'd be cut off mid-reply).
        let loaded = engine.current();
        let held = loaded.as_ref().map_or(0, |(_, ep)| engine::held(&ep.key));
        let busy = held + loaded.as_ref().map_or(0, |(_, ep)| engine::USERS.lock().unwrap().others(&replies(state), &before, &ep.key, task));
        let was = loaded.as_ref().map(|(_, ep)| ep.key.clone());
        if let Some(ep) = engine.endpoint_steered(&installed.model_id, lora.as_deref(), steering.as_ref()) {
            // Same model: new limits (mode, heat) apply between replies.
            if busy > 0 || engine.matches_tuned(&launch, plan.tuning) {
                break ep;
            }
        } else if busy > 0 || ticket.as_ref().map_or(engine::waiting() > 0, |t| t.place() > 1) {
            // Replies on the model running now, or others who asked for
            // another model first: this waits its turn in line.
            drop(engine);
            if before.iter().any(|f| f.load(Ordering::Relaxed)) {
                return Err("Stopped before the model loaded.".into());
            }
            let other = loaded.map(|(id, _)| state.model_spec(&id).map_or(id, |s| s.name)).unwrap_or_default();
            if asked.elapsed() >= WAIT_IN_LINE {
                if held == busy && busy > 0 {
                    return Err(format!("A background job (the mail report, meeting notes or a translation) is still using {other}. Try again when it's done."));
                }
                return Err(format!("This waited 30 minutes in line while other chats answered with {other}. Try again when they're done, or stop them first."));
            }
            let place = ticket.get_or_insert_with(engine::Ticket::new).place();
            tell(engine::Wait::Line(place, other));
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            continue;
        }
        // Room for it right now (a game may be holding the memory)? The
        // engine isn't held while the person decides.
        let replacing = engine.loaded().map(|(pid, id, quant, ctx, layers)| {
            (pid, state.model_spec(&id).map(|s| memcheck::need(&s, &quant, ctx, layers, 0, false)).unwrap_or_default())
        });
        drop(engine);
        let want = memcheck::need(&spec, &installed.quant, plan.ctx, plan.gpu_layers, encoder_size, plan.tuning.kv_q8);
        // Room on the card for it beside the model loaded now: that one
        // stays loaded, so going back to it needs no loading.
        let keep = replacing.is_some() && perf::settings(&state.db.lock().unwrap()).keep_two && memcheck::room_beside(want).await;
        if !keep {
            memcheck::before_loading(state, &spec, want, replacing).await?;
        }
        let mut engine = state.engine.lock().await;
        engine.keep_next(keep);
        // Another model was loaded, or a reply started, meanwhile (the
        // question can wait minutes): look again before stopping it.
        let now = engine.current().map(|(_, ep)| ep.key);
        if now != was || now.is_some_and(|key| engine::held(&key) + engine::USERS.lock().unwrap().others(&replies(state), &before, &key, task) > 0) {
            continue;
        }
        tell(engine::Wait::Loading);
        let ep = match engine.ensure_tuned(launch, plan.tuning, state.job()).await {
            Ok(ep) => ep,
            Err(e) => return Err(load_failed(state, &spec, &installed.quant, plan.ctx, plan.gpu_layers, want, e)),
        };
        engine.asked_as(&id);
        break ep;
    };
    engine::USERS.lock().unwrap().place(&replies(state), &before, &ep.key, task);
    watch_work(state, &ep);
    let advanced = advanced::get(state);
    ep.extra = advanced.request_extra();
    presets::apply_sampling(&mut ep.extra, &presets::get(&state.db.lock().unwrap(), &id));
    // Thinking counts against the reply's allowance: a long think would
    // otherwise use it all and leave an empty reply.
    if thinks(&spec, advanced.processing.thinking) && ep.extra.get("max_tokens").is_none() {
        ep.extra["max_tokens"] = json!((ep.ctx / 2).clamp(4096, 16_384));
    }
    // gpt-oss (Harmony) reads its reasoning effort from the request: never
    // sent before, so its template's default applied whatever was asked.
    if harmony(&spec) {
        let effort = advanced.reasoning_effort();
        ep.extra["reasoning_effort"] = json!(effort);
        if !ep.extra["chat_template_kwargs"].is_object() {
            ep.extra["chat_template_kwargs"] = json!({});
        }
        ep.extra["chat_template_kwargs"]["reasoning_effort"] = json!(effort);
    }
    Ok((ep, spec, installed))
}

/// gpt-oss, or a model built on it (huihui-ai's abliterated one): it speaks
/// OpenAI's Harmony format, with channels and `to=functions.x` tool calls.
pub(crate) fn harmony(spec: &ModelSpec) -> bool {
    let s = format!("{} {} {}", spec.id, spec.name, spec.source).to_lowercase();
    s.contains("gpt-oss") || s.contains("gpt_oss") || s.contains("gptoss")
}

/// The model thinks before it answers (Qwen3, gpt-oss, DeepSeek R1, QwQ),
/// as set in Advanced or, by default, as the model does.
pub(crate) fn thinks(spec: &ModelSpec, setting: advanced::Thinking) -> bool {
    match setting {
        advanced::Thinking::Off => false,
        advanced::Thinking::On => true,
        advanced::Thinking::Auto => {
            let name = format!("{} {}", spec.id, spec.name).to_lowercase();
            // Qwen3's own models think unless they're an instruct-only version.
            let hybrid = name.contains("qwen3") && !["instruct", "2507", "coder", "-vl", " vl"].iter().any(|w| name.contains(w));
            spec.tags.iter().any(|t| t == "thinking") || hybrid || ["thinking", "reasoning", "qwq", "-r1", "gpt-oss", "magistral"].iter().any(|w| name.contains(w))
        }
    }
}

/// The loaded model `watch_work` watches (by its key).
pub(super) static WATCHING: Mutex<Option<String>> = Mutex::new(None);

/// While a loaded model runs, checks every 15 seconds whether it's working
/// (a reply, a translation, meeting notes, a program using the local API)
/// and counts that as use, so the idle unload stops only a model nobody is
/// using: a long job asks for the model once, then keeps it busy for many
/// minutes.
pub(super) fn watch_work(state: &Arc<AppState>, ep: &engine::Endpoint) {
    {
        let mut w = WATCHING.lock().unwrap();
        if w.as_deref() == Some(ep.key.as_str()) {
            return;
        }
        *w = Some(ep.key.clone());
    }
    let (state, ep) = (state.clone(), ep.clone());
    tauri::async_runtime::spawn(async move {
        let client = net::local_client();
        let mine = || WATCHING.lock().unwrap().as_deref() == Some(ep.key.as_str());
        while mine() {
            tokio::time::sleep(std::time::Duration::from_secs(15)).await;
            let asked = client.get(ep.url("/slots")).bearer_auth(&ep.key).timeout(std::time::Duration::from_secs(5)).send().await;
            let working = match asked.and_then(|r| r.error_for_status()) {
                Ok(r) => r.json::<serde_json::Value>().await.ok().and_then(|v| v.as_array().map(|slots| slots.iter().any(|s| s["is_processing"] == true))),
                // Too busy to answer at once: it's working.
                Err(e) if e.is_timeout() => Some(true),
                Err(_) => None,
            };
            match working {
                Some(true) => *state.engine_used.lock().unwrap() = std::time::Instant::now(),
                Some(false) => {}
                // Not answering: stopped, unless it's starting again under
                // the same key (new limits) or the engine is busy loading.
                None => {
                    let still = match state.engine.try_lock() {
                        Ok(mut e) => e.current().is_some_and(|(_, now)| now.key == ep.key),
                        Err(_) => true,
                    };
                    if !still {
                        break;
                    }
                }
            }
        }
        let mut w = WATCHING.lock().unwrap();
        if w.as_deref() == Some(ep.key.as_str()) {
            *w = None;
        }
    });
}

/// For background work (meeting notes, translation): the model already
/// loaded if there is one, so a chat's model isn't swapped out mid-reply.
/// The model stays theirs (not stopped or relaunched by a chat) while the
/// returned hold lives.
pub(crate) async fn background_endpoint(state: &Arc<AppState>) -> Result<(engine::Endpoint, ModelSpec, engine::Hold), String> {
    // Use the model as it is loaded: relaunching it with new limits here
    // could cut off a chat that is mid-reply.
    let loaded = {
        let mut engine = state.engine.lock().await;
        // Held before the engine is let go, so a chat can't swap it in between.
        engine.current().and_then(|(id, ep)| Some((state.model_spec(&id)?, engine::Hold::new(&ep.key), ep)))
    };
    if let Some((spec, hold, ep)) = loaded {
        *state.engine_used.lock().unwrap() = std::time::Instant::now();
        watch_work(state, &ep);
        return Ok((ep, spec, hold));
    }
    let (ep, spec, _) = llm_endpoint(state, None, false, |_| {}).await?;
    let hold = engine::Hold::new(&ep.key);
    Ok((ep, spec, hold))
}

/// The chat model for work the person started outside a chat (a podcast's
/// script, a code review): the one they picked, loaded and held, else the
/// one already loaded or the default (`background_endpoint`).
pub(crate) async fn endpoint_for(state: &Arc<AppState>, model_id: Option<&str>) -> Result<(engine::Endpoint, ModelSpec, engine::Hold), String> {
    match model_id.filter(|m| !m.is_empty()) {
        None => background_endpoint(state).await,
        Some(id) => {
            let (ep, spec, _) = llm_endpoint(state, Some(id.to_string()), false, |_| {}).await?;
            let hold = engine::Hold::new(&ep.key);
            Ok((ep, spec, hold))
        }
    }
}

// SPDX-License-Identifier: AGPL-3.0-only
//! Getting a turn ready: room in the model's memory, a cool card, tools
//! that fit a small memory, and live data for sites.

use super::*;
use super::instructions::LIVE_DATA;

impl Turn {
    /// The window was nearly full at the last step (as Advanced sets it for
    /// compacting), or messages had to be left out to fit.
    pub(super) fn nearly_full(&self) -> bool {
        let crate::advanced::LongChat::Compact(at) = crate::advanced::get(&self.state).long_chat() else { return false };
        self.state.contexts.lock().unwrap().get(&self.chat_id).is_some_and(|i| i.used_fraction() >= at || i.dropped_messages > 0)
    }

    /// Compacts the conversation before the request being worked on (its
    /// messages and steps stay as they are): the new summary, or None when
    /// there's nothing before it to compact or it couldn't be written.
    pub(super) async fn compact_before_turn(&self) -> Option<(Option<String>, String)> {
        let all = db::messages(&self.state.db.lock().unwrap(), &self.cipher, &self.chat_id);
        let turn = all.iter().position(|m| m.id == self.turn_id)?;
        let before = all[..turn].to_vec();
        let (_, old) = compact::after_last(before.clone());
        let covered = compact::pending(&before);
        if covered == 0 {
            return None;
        }
        self.emit("chat:status", json!({ "chat_id": self.chat_id, "status": "compacting" }));
        let summary = match compact::summarize(&self.ep, &self.base, before, &self.cancel).await {
            Ok(s) => s,
            Err(e) => {
                crate::applog::line("chat", &format!("Couldn't compact a long chat partway through a reply: {e}"));
                return None;
            }
        };
        compact::save_at(&self.state.db.lock().unwrap(), &self.cipher, &self.chat_id, &summary, covered, true, all[turn].created_at - 1).ok()?;
        self.state.contexts.lock().unwrap().remove(&self.chat_id);
        crate::applog::line("chat", "Compacted a long chat partway through a reply (it was nearly full)");
        self.emit("chat:compacted", json!({ "chat_id": self.chat_id, "auto": true }));
        Some((old, summary))
    }

    /// Before each step: wait out the heat guard while the PC is over its
    /// limit, and rest briefly while adaptive cooling has stepped down. A hot
    /// processor while the model runs on it, on a PC with a graphics card,
    /// comes with the reason ("the GPU limit is Off").
    pub(super) async fn rest_if_hot(&self) {
        let limits = self.state.limits();
        crate::perf::cool_down(&limits, |what| {
            let note = if what.starts_with("processor") { crate::placement::processor_note(&self.state) } else { None };
            let detail = match note {
                Some(n) => format!("{what}; {n}"),
                None => what.to_string(),
            };
            self.emit("chat:status", json!({ "chat_id": self.chat_id, "status": "cooling", "detail": detail }));
        })
        .await;
        let pause = crate::perf::step_pause(&self.state.heat.get());
        if !pause.is_zero() && !self.cancel.load(Ordering::Relaxed) {
            tokio::time::sleep(pause).await;
        }
    }

    /// A small memory (8K) can't take every tool and note at once: the tools
    /// this request doesn't need are left out, group by group (tools::SPARE),
    /// with their notes, until the instructions and tools leave room for the
    /// chat and the reply. Sizes are counted by the engine once, then scaled.
    pub(super) async fn fit_small(&self, asked: &str, about: &mut String, offered: &mut Option<Value>, notes: &[(&[&str], String)]) {
        let Some(Value::Array(defs)) = offered.as_mut() else { return };
        let json_len = Value::Array(defs.clone()).to_string().len().max(1);
        let (Ok(base), Ok(said), Ok(listed)) = (
            chat::count_tokens(&self.ep, &self.base).await,
            chat::count_tokens(&self.ep, about).await,
            chat::count_tokens(&self.ep, &Value::Array(defs.clone()).to_string()).await,
        ) else {
            return;
        };
        let reply = (self.ep.ctx / 4).clamp(256, 4096);
        // Room for the conversation itself.
        let chat_room = (self.ep.ctx / 4).max(768);
        let limit = self.ep.ctx.saturating_sub(reply + chat_room) as f64;
        let mut total = (base + said + listed) as f64;
        if total <= limit {
            return;
        }
        let per_tool_byte = listed as f64 / json_len as f64;
        let per_note_byte = said as f64 / about.len().max(1) as f64;
        let name = |d: &Value| d.pointer("/function/name").and_then(Value::as_str).unwrap_or_default().to_string();
        let mut left_out: Vec<String> = Vec::new();
        for (group, needed) in tools::SPARE.iter() {
            if total <= limit {
                break;
            }
            if needed.is_match(asked) {
                continue;
            }
            defs.retain(|d| {
                let n = name(d);
                if tools::in_group(&n, group) {
                    total -= d.to_string().len() as f64 * per_tool_byte;
                    left_out.push(n);
                    false
                } else {
                    true
                }
            });
            for (tools_of, note) in notes {
                if !defs.iter().any(|d| tools_of.contains(&name(d).as_str())) && about.contains(note.as_str()) {
                    *about = about.replacen(note.as_str(), "", 1);
                    total -= note.len() as f64 * per_note_byte;
                }
            }
        }
        if !left_out.is_empty() {
            crate::applog::line("agent", &format!("Small memory ({}): left out {} tools this request doesn't need", self.ep.ctx, left_out.len()));
        }
    }

    /// Building with the web on: how sites get live data, and a way to ask
    /// for a key. Whether it added them (once a turn). `wanted` is false for a
    /// small memory when nothing is being built (the note would crowd it).
    pub(super) fn add_live_data(&self, sandbox: &Sandbox, about: &mut String, offered: &mut Option<Value>, wanted: bool) -> bool {
        if !wanted || !self.use_tools || sandbox.is_empty() || self.mode == tools::Mode::Plan || about.contains(LIVE_DATA) {
            return false;
        }
        if !crate::web::allowed_for_chat(&self.state.db.lock().unwrap(), &self.chat_id) {
            return false;
        }
        about.push_str(LIVE_DATA);
        if let (Some(Value::Array(defs)), Some(d)) = (offered.as_mut(), tools::find("request_api_key")) {
            if !defs.iter().any(|x| x.pointer("/function/name").and_then(Value::as_str) == Some(d.name)) {
                defs.push(json!({ "type": "function", "function": { "name": d.name, "description": d.description, "parameters": (d.params)() } }));
            }
        }
        true
    }
}

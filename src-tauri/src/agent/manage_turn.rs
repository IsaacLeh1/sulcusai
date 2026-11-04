//! Where "Manage itself" meets a turn: kept apart so an unmanaged chat
//! runs the loop exactly as before.

use super::manager::{self, Budget, BudgetState, ManagerKind, Repeats, Report, Settings, StepState, Verdict};
use super::steps::now;
use super::Turn;
use crate::db::{self, Message};
use serde_json::json;

pub(super) struct Managed {
    pub settings: Settings,
    pub budget: Budget,
    pub repeats: Repeats,
    /// Set when the same action failed too often; the loop stops at the next step.
    pub escalate: Option<String>,
    pub warned: bool,
    /// Commands run this turn, for the plan's `run` checks.
    pub ran_ok: Vec<String>,
    pub ran_failed: Vec<String>,
    /// Handed to the managing model once already this turn.
    pub stepped_up: bool,
    /// The plan's checks all passed this turn (the evidence), or what went wrong.
    pub report: Option<Report>,
}

impl Turn {
    pub(super) fn manager_start(&mut self, about: &mut String) -> Option<Managed> {
        let data = self.state.paths.data.clone();
        let settings = manager::load(&data, &self.chat_id);
        if !settings.on {
            return None;
        }
        about.push_str(manager::prompt_section());
        about.extend(manager::take_redo(&data, &self.chat_id));
        about.extend(manager::take_answer(&data, &self.chat_id));
        let mut plan = manager::load_plan(&data, &self.chat_id);
        let share = self.state.contexts.lock().unwrap().get(&self.chat_id).map_or(0.0, |i| i.used_fraction() as f32);
        if manager::should_clear(&settings, plan.as_ref(), share) {
            self.clear_to_handoff(about, plan.as_ref());
            // A finished plan belongs to the job before; this request starts a new one.
            if plan.as_ref().is_some_and(|p| p.steps.iter().all(|s| s.state == StepState::Done)) {
                let _ = std::fs::remove_file(manager::folder(&data, &self.chat_id).join("plan.json"));
                plan = None;
            }
        }
        if let Some(plan) = plan.as_ref().filter(|p| p.steps.iter().any(|s| s.state != StepState::Done)) {
            if let Some(h) = manager::Handoff::from_notes(&data, &self.chat_id, "none") {
                about.push_str(&format!("\n\nYour working notes (resume from these; open only the files they name):\n{}", h.text(&data, &self.chat_id)));
            }
            let step = plan.steps.iter().find(|s| s.state != StepState::Done);
            if let Some(size) = step.and_then(|s| s.size) {
                self.fit_to_step(size, step.is_some_and(|s| s.state == StepState::Stuck));
            }
        }
        Some(Managed { budget: Budget::new(&settings), settings, repeats: Repeats::default(), escalate: None, warned: false, ran_ok: Vec::new(), ran_failed: Vec::new(), stepped_up: false, report: None })
    }

    /// Effort for local models, model for cloud ones; the person's own choices win.
    fn fit_to_step(&mut self, size: manager::Size, failed_before: bool) {
        let person_set = crate::advanced::get(&self.state).processing.reasoning_effort.is_some_and(|e| !e.trim().is_empty());
        if !person_set && self.ep.extra.get("reasoning_effort").is_some() {
            let effort = manager::pick_effort(size, failed_before);
            self.ep.extra["reasoning_effort"] = json!(effort);
            if let Some(k) = self.ep.extra.get_mut("chat_template_kwargs").and_then(|v| v.as_object_mut()) {
                k.insert("reasoning_effort".into(), json!(effort));
            }
        }
        let Some(target) = self.ep.cloud.clone() else { return };
        // Only a chat that kept the provider's model it was given; a model the person picked for it stays.
        let models = {
            let conn = self.state.db.lock().unwrap();
            let s = crate::cloud::settings(&conn);
            crate::cloud::provider_of(&s, &target.provider).map(|p| p.models.iter().map(|m| (m.id.clone(), m.price_in)).collect::<Vec<_>>())
        };
        let Some(models) = models else { return };
        let list = manager::by_cost(&models);
        if let Some(pick) = manager::pick_model(size, failed_before, &list).filter(|p| *p != target.model.id) {
            self.switch_model(&crate::cloud::model_ref(&target.provider, pick));
        }
    }

    fn switch_model(&mut self, id: &str) -> bool {
        match crate::cloud::endpoint(&self.state, id, &self.cipher) {
            Ok((mut ep, _, _)) => {
                ep.extra = std::mem::take(&mut self.ep.extra);
                if let Some(t) = ep.cloud.as_ref() {
                    t.for_chat(&self.chat_id);
                }
                self.ep = ep;
                crate::applog::line("agent", "Manage itself picked another model from the same provider for this step");
                true
            }
            Err(e) => {
                crate::applog::line("agent", &format!("Manage itself couldn't switch models: {e}"));
                false
            }
        }
    }

    /// Clean break: what came before this request is replaced by the
    /// handoff from the working notes, with no model call to summarize it.
    fn clear_to_handoff(&self, about: &mut String, plan: Option<&manager::Plan>) {
        let data = &self.state.paths.data;
        let Some(h) = plan.and_then(|_| manager::Handoff::from_notes(data, &self.chat_id, "none")).filter(|h| h.is_complete() || !h.state.trim().is_empty()) else { return };
        let conn = self.state.db.lock().unwrap();
        let all = db::messages(&conn, &self.cipher, &self.chat_id);
        let Some(turn) = all.iter().position(|m| m.id == self.turn_id) else { return };
        let before = all[..turn].to_vec();
        let covered = crate::compact::pending(&before);
        if covered == 0 {
            return;
        }
        let (_, old) = crate::compact::after_last(before);
        let text = h.text(data, &self.chat_id);
        if crate::compact::save_at(&conn, &self.cipher, &self.chat_id, &text, covered, true, all[turn].created_at - 1).is_ok() {
            *about = crate::compact::with_summary(about, old.as_deref(), &text);
            self.state.contexts.lock().unwrap().remove(&self.chat_id);
            crate::applog::line("agent", "Manage itself cleared a chat to its working notes at a clean break");
            self.emit("chat:compacted", json!({ "chat_id": self.chat_id, "auto": true }));
        }
    }

    /// Notes and the plan come out of the reply into their files; the person sees the rest.
    pub(super) fn manager_reply(&self, managed: &Option<Managed>, content: String) -> String {
        if managed.is_none() {
            return content;
        }
        let data = &self.state.paths.data;
        if let Some(plan) = manager::parse_plan(&content) {
            if let Err(e) = manager::save_plan(data, &self.chat_id, &plan) {
                crate::applog::line("agent", &format!("Couldn't keep a chat's plan: {e}"));
            }
            if let Some(ManagerKind::Chat(to)) = managed.as_ref().map(|m| &m.settings.manager) {
                let from = db::chat(&self.state.db.lock().unwrap(), &self.cipher, &self.chat_id).map(|c| c.title).unwrap_or_default();
                self.post_to(to, manager::plan_for_manager(&plan, &from), json!({ "report_from": self.chat_id, "plan_for": self.chat_id }));
            }
        }
        let (notes, clean) = manager::take_notes(&content);
        if notes.is_empty() {
            return content;
        }
        if let Err(e) = manager::write_notes(data, &self.chat_id, &notes) {
            crate::applog::line("agent", &format!("Couldn't keep a chat's working notes: {e}"));
        }
        clean
    }

    /// The reply would end the turn: the plan's checks are run by the app,
    /// and a step that isn't proven sends it back (None: let it end).
    pub(super) fn manager_done_checks(&self, managed: &mut Option<Managed>, folder: Option<&std::path::Path>) -> Option<String> {
        let m = managed.as_mut()?;
        let data = &self.state.paths.data;
        let mut plan = manager::load_plan(data, &self.chat_id)?;
        let root = folder?;
        let verdict = manager::verify(&mut plan, root, &m.ran_ok, &m.ran_failed);
        let _ = manager::save_plan(data, &self.chat_id, &plan);
        match verdict {
            Verdict::AllDone(evidence) => {
                m.report = Some(Report::Finished { evidence });
                None
            }
            Verdict::Back(_, why) => {
                m.report = Some(Report::Problem { what: why.clone() });
                Some(why)
            }
        }
    }

    /// A stop note when the budget is spent or an action keeps failing; a
    /// wrap-up hint for the model at 80%. Repeated failure goes to a
    /// managing model first, once, when one is set.
    pub(super) fn manager_step(&mut self, managed: &mut Option<Managed>, hints: &mut Vec<String>) -> Option<Message> {
        let m = managed.as_mut()?;
        let text = if let Some(t) = m.escalate.take() {
            if let (ManagerKind::Model(id), false) = (m.settings.manager.clone(), m.stepped_up) {
                m.stepped_up = true;
                m.repeats.progressed();
                if self.switch_model(&id) {
                    hints.push("A stronger model takes over this step because the same action kept failing. Try a different way.".into());
                    return None;
                }
            }
            m.report = Some(Report::Problem { what: t.clone() });
            t
        } else {
            match m.budget.check() {
                BudgetState::Fine => return None,
                BudgetState::Warn => {
                    if !m.warned {
                        m.warned = true;
                        hints.extend(manager::budget_note(BudgetState::Warn, &m.settings));
                    }
                    return None;
                }
                BudgetState::Spent => manager::budget_note(BudgetState::Spent, &m.settings)?,
            }
        };
        Some(Message { id: uuid::Uuid::new_v4().to_string(), chat_id: self.chat_id.clone(), role: "assistant".into(), content: text, created_at: now(), ..Default::default() })
    }

    /// The turn's one report to a managing chat: finished with evidence, or a real problem.
    pub(super) fn manager_end(&self, managed: &Option<Managed>) {
        let Some(m) = managed.as_ref() else { return };
        let (ManagerKind::Chat(to), Some(report)) = (&m.settings.manager, &m.report) else { return };
        let from = db::chat(&self.state.db.lock().unwrap(), &self.cipher, &self.chat_id).map(|c| c.title).unwrap_or_default();
        self.post_to(to, report.text(&from), json!({ "report_from": self.chat_id }));
    }

    fn post_to(&self, to: &str, content: String, meta: serde_json::Value) {
        let conn = self.state.db.lock().unwrap();
        let note = Message { id: uuid::Uuid::new_v4().to_string(), chat_id: to.to_string(), role: "assistant".into(), content, created_at: now(), meta: Some(meta), ..Default::default() };
        if db::chat(&conn, &self.cipher, to).is_some() && db::add_message(&conn, &self.cipher, &note).is_ok() {
            drop(conn);
            self.emit("agent:step", json!({ "chat_id": to }));
        }
    }
}

pub(super) fn failed(managed: &mut Option<Managed>, tool: &str, key: &str, why: &str) {
    if let Some(m) = managed.as_mut() {
        m.ran_failed.push(key.to_string());
        if m.repeats.failed(&manager::action_key(tool, key), m.settings.repeat_limit) {
            m.escalate = Some(manager::repeat_note(tool, why, m.settings.repeat_limit));
        }
    }
}

pub(super) fn progressed(managed: &mut Option<Managed>, key: &str) {
    if let Some(m) = managed.as_mut() {
        m.ran_ok.push(key.to_string());
        m.repeats.progressed();
    }
}

pub(super) fn used(managed: &mut Option<Managed>, tokens: Option<u64>) {
    if let (Some(m), Some(t)) = (managed.as_mut(), tokens) {
        m.budget.add_tokens(t);
    }
}

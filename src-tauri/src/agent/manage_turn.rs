//! Where "Manage itself" meets a turn: kept apart so an unmanaged chat
//! runs the loop exactly as before.

use super::manager::{self, Budget, BudgetState, Repeats, Settings, StepState};
use super::Turn;
use super::steps::now;
use crate::db::Message;

pub(super) struct Managed {
    pub settings: Settings,
    pub budget: Budget,
    pub repeats: Repeats,
    /// Set when the same action failed too often; the loop stops at the next step.
    pub escalate: Option<String>,
    pub warned: bool,
}

impl Turn {
    pub(super) fn manager_start(&mut self, about: &mut String) -> Option<Managed> {
        let data = self.state.paths.data.clone();
        let settings = manager::load(&data, &self.chat_id);
        if !settings.on {
            return None;
        }
        about.push_str(manager::prompt_section());
        let plan = manager::load_plan(&data, &self.chat_id);
        if let Some(plan) = plan.as_ref().filter(|p| p.steps.iter().any(|s| s.state != StepState::Done)) {
            if let Some(h) = manager::Handoff::from_notes(&data, &self.chat_id, "none") {
                about.push_str(&format!("\n\nYour working notes (resume from these; open only the files they name):\n{}", h.text(&data, &self.chat_id)));
            }
            // Effort for the step it's on, unless the person set one in Advanced.
            let step = plan.steps.iter().find(|s| s.state != StepState::Done);
            let person_set = crate::advanced::get(&self.state).processing.reasoning_effort.is_some_and(|e| !e.trim().is_empty());
            if let (Some(size), false) = (step.and_then(|s| s.size), person_set) {
                if self.ep.extra.get("reasoning_effort").is_some() {
                    let effort = manager::pick_effort(size, step.is_some_and(|s| s.state == StepState::Stuck));
                    self.ep.extra["reasoning_effort"] = serde_json::json!(effort);
                    if let Some(k) = self.ep.extra.get_mut("chat_template_kwargs").and_then(|v| v.as_object_mut()) {
                        k.insert("reasoning_effort".into(), serde_json::json!(effort));
                    }
                }
            }
        }
        Some(Managed { budget: Budget::new(&settings), settings, repeats: Repeats::default(), escalate: None, warned: false })
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

    /// A stop note when the budget is spent or an action keeps failing; a
    /// wrap-up hint for the model at 80%.
    pub(super) fn manager_step(&self, managed: &mut Option<Managed>, hints: &mut Vec<String>) -> Option<Message> {
        let m = managed.as_mut()?;
        let text = if let Some(t) = m.escalate.take() {
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
}

pub(super) fn failed(managed: &mut Option<Managed>, tool: &str, key: &str, why: &str) {
    if let Some(m) = managed.as_mut() {
        if m.repeats.failed(&manager::action_key(tool, key), m.settings.repeat_limit) {
            m.escalate = Some(manager::repeat_note(tool, why, m.settings.repeat_limit));
        }
    }
}

pub(super) fn progressed(managed: &mut Option<Managed>) {
    if let Some(m) = managed.as_mut() {
        m.repeats.progressed();
    }
}

pub(super) fn used(managed: &mut Option<Managed>, tokens: Option<u64>) {
    if let (Some(m), Some(t)) = (managed.as_mut(), tokens) {
        m.budget.add_tokens(t);
    }
}

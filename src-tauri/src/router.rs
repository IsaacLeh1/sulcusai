// SPDX-License-Identifier: AGPL-3.0-only
//! Auto: a message is answered by the installed model best at its kind of
//! task (code, writing, translation) when one is clearly better at it than
//! the chat's own model and runs well on this PC. Small models are trained
//! for different things: a coding model writes better code than a general
//! one of its size. The model already loaded is kept when it's nearly as
//! good, since loading another costs seconds; a chat whose model the person
//! picked keeps it.

use std::collections::HashSet;

use crate::catalog::{self, ModelSpec};
use crate::AppState;

/// What a message mostly asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Task {
    Coding,
    Writing,
    Languages,
    General,
}

impl Task {
    /// The rating a model gets for it (General: none, so no switch).
    fn rating(self, m: &ModelSpec) -> Option<u8> {
        let r = &m.ratings;
        match self {
            Task::Coding => Some(r.coding),
            Task::Writing => Some(r.writing),
            Task::Languages => Some(r.languages),
            Task::General => None,
        }
    }

    /// For the chat's note: "it's better at {word}".
    pub fn word(self) -> &'static str {
        match self {
            Task::Coding => "code",
            Task::Writing => "writing",
            Task::Languages => "languages",
            Task::General => "this",
        }
    }
}

/// The kind of task a message is. Code: a code block, a file name with a
/// code extension, an error's trace, or two words about programming;
/// writing and translation by their words. Anything else is general.
pub fn classify(text: &str) -> Task {
    let low = text.to_lowercase();
    let words: HashSet<&str> = low.split(|c: char| !c.is_alphanumeric() && c != '+' && c != '#').filter(|w| !w.is_empty()).collect();
    let has = |list: &[&str]| list.iter().filter(|w| words.contains(*w)).count();
    let strong = low.contains("```")
        || low.contains("traceback (most recent call last)")
        || low.contains("stack trace")
        || low.contains("error[e")
        || low.contains("exception:")
        || low.split_whitespace().any(|w| {
            let w = w.trim_matches(|c: char| !c.is_alphanumeric() && c != '.' && c != '_');
            w.rsplit_once('.').is_some_and(|(stem, ext)| {
                !stem.is_empty() && ["py", "rs", "js", "ts", "tsx", "jsx", "java", "cs", "cpp", "go", "rb", "php", "swift", "kt", "sql", "sh", "ps1", "html", "css", "json", "yaml", "toml", "lua", "dart"].contains(&ext)
            })
        });
    let code = has(&[
        "code", "coding", "program", "programming", "script", "function", "bug", "debug", "compile", "compiler", "refactor", "regex", "sql", "api", "python", "javascript",
        "typescript", "rust", "java", "c++", "c#", "golang", "html", "css", "react", "repo", "repository", "git", "terminal", "powershell", "bash", "class", "variable",
        "loop", "array", "syntax", "algorithm", "database", "endpoint", "backend", "frontend", "npm", "pip", "cargo", "dependency", "unittest", "test",
    ]);
    if strong || code >= 2 {
        return Task::Coding;
    }
    if has(&["translate", "translation", "translating"]) > 0 || low.contains("in spanish") || low.contains("in french") || low.contains("in german") || low.contains("in japanese") || low.contains("in chinese") {
        return Task::Languages;
    }
    if has(&["essay", "poem", "story", "novel", "blog", "proofread", "rewrite", "paragraph", "letter", "speech", "lyrics", "chapter", "article", "copywriting", "tagline"]) > 0 {
        return Task::Writing;
    }
    Task::General
}

/// The model to answer with instead of `current`, and why.
#[derive(Debug, Clone, PartialEq)]
pub struct Pick {
    pub model_id: String,
    pub name: String,
    pub task: Task,
}

/// Points out of 10 a model must beat the chat's own by for the task.
const BETTER_BY: u8 = 2;

/// Among `candidates` (installed models that run well here), the one for
/// `task` instead of `current`: clearly better at it, and the loaded one
/// when it's within a point of the best. None: keep `current`.
pub fn choose(task: Task, current: &ModelSpec, candidates: &[&ModelSpec], loaded: Option<&str>) -> Option<Pick> {
    let mine = task.rating(current)?;
    // Unrated (from another app or Hugging Face): the person's choice stands.
    if current.ratings.overall == 0 {
        return None;
    }
    let best = candidates.iter().filter(|m| m.id != current.id).max_by_key(|m| (task.rating(m).unwrap_or(0), m.ratings.overall))?;
    let top = task.rating(best)?;
    if top < mine + BETTER_BY {
        return None;
    }
    let pick = candidates
        .iter()
        .find(|m| Some(m.id.as_str()) == loaded && m.id != current.id && task.rating(m).unwrap_or(0) + 1 >= top && task.rating(m).unwrap_or(0) >= mine + BETTER_BY)
        .unwrap_or(best);
    Some(Pick { model_id: pick.id.clone(), name: pick.name.clone(), task })
}

/// Auto for one message of a chat on the default model (`Settings::auto_model`):
/// `chat_model` is the chat's own (None: the default). A model that uses
/// tools hands over only to another that does.
pub fn route(state: &AppState, chat_model: Option<&str>, text: &str) -> Option<Pick> {
    let settings = state.settings();
    let chat_model = chat_model.or(settings.default_model.as_deref());
    if !settings.auto_model || chat_model.is_none() || chat_model != settings.default_model.as_deref() {
        return None;
    }
    let task = classify(text);
    if task == Task::General {
        return None;
    }
    let current = state.catalog.model(chat_model?)?;
    let budget = state.budget();
    let installed = crate::db::installed_models(&state.db.lock().unwrap());
    let runs_well = |m: &ModelSpec, quant: &str| {
        m.variant(quant).is_some_and(|v| {
            let f = catalog::fit_variant(m, v, crate::TOOL_CTX, &budget);
            f.runnable() && f.est_tps >= ROUTE_MIN_TPS
        })
    };
    let candidates: Vec<&ModelSpec> = installed
        .iter()
        .filter_map(|i| state.catalog.model(&i.model_id).filter(|m| runs_well(m, &i.quant)))
        .filter(|m| !current.tools || m.tools)
        .collect();
    let loaded = state.engine.try_lock().ok().and_then(|mut e| e.current().map(|(id, _)| id));
    choose(task, current, &candidates, loaded.as_deref())
}

/// Words a second a model must write here to be picked for a message.
const ROUTE_MIN_TPS: f64 = 10.0;

/// The installed model best at `task` that runs well on this PC (for work
/// the person starts outside a chat, such as a code review): (id, name).
/// Ties go to the better model overall. None: nothing installed runs well.
pub fn best_for(state: &AppState, task: Task) -> Option<(String, String)> {
    let budget = state.budget();
    let installed = crate::db::installed_models(&state.db.lock().unwrap());
    installed
        .iter()
        .filter_map(|i| {
            let m = state.catalog.model(&i.model_id)?;
            let v = m.variant(&i.quant)?;
            let f = catalog::fit_variant(m, v, crate::TOOL_CTX, &budget);
            (f.runnable() && f.est_tps >= ROUTE_MIN_TPS).then_some(m)
        })
        .max_by_key(|m| (task.rating(m).unwrap_or(m.ratings.overall), m.ratings.overall))
        .map(|m| (m.id.clone(), m.name.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_sorted_by_task() {
        assert_eq!(classify("Why does this crash?\n```python\nprint(x[3])\n```"), Task::Coding);
        assert_eq!(classify("fix the bug in main.rs"), Task::Coding);
        assert_eq!(classify("write a python script that renames my photos"), Task::Coding);
        assert_eq!(classify("Traceback (most recent call last): File \"a.py\""), Task::Coding);
        assert_eq!(classify("translate this into Spanish: good morning"), Task::Languages);
        assert_eq!(classify("write a short poem about the sea"), Task::Writing);
        assert_eq!(classify("what's the capital of Peru?"), Task::General);
        assert_eq!(classify("I'm happy with the app"), Task::General, "one programming-ish word isn't enough");
        assert_eq!(classify("the best budget app?"), Task::General);
    }

    #[test]
    fn a_clearly_better_specialist_answers() {
        let c = catalog::Catalog::bundled();
        let m = |id: &str| c.model(id).unwrap();
        let general = m("qwen3.5-9b");
        let coder = m("ornith-1.5-9b");
        let writer = m("gemma-4-12b");
        let all = [general, coder, writer];
        // Code from a small general model: the coding model, clearly better (9 against 6).
        let pick = choose(Task::Coding, m("qwen3.5-4b"), &all, None).unwrap();
        assert_eq!(pick.model_id, "ornith-1.5-9b", "{pick:?}");
        // Not clearly better: the chat's own model stays.
        assert_eq!(choose(Task::Coding, general, &all, None), None, "Ornith 9 against Qwen3.5 9B's 8");
        // Writing: Gemma, better at it by 2.
        assert_eq!(choose(Task::Writing, general, &all, None).map(|p| p.model_id).as_deref(), Some("gemma-4-12b"));
        // General messages never switch.
        assert_eq!(choose(Task::General, general, &all, None), None);
        // The loaded model, when it's within a point of the best and clearly better.
        let strong = m("ornith-1.5-35b-a3b");
        let pick = choose(Task::Coding, m("qwen3.5-4b"), &[coder, strong], Some("ornith-1.5-9b")).unwrap();
        assert_eq!(pick.model_id, "ornith-1.5-9b", "no reload for one point");
    }
}

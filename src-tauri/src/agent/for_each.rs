// SPDX-License-Identifier: AGPL-3.0-only
//! One instruction over a list: each item (a file, a row, a name) gets its
//! own helper with a fresh context, so item 40 is done as carefully as item
//! 1, and the reports come back together. Helpers only read; what to change
//! from the results is up to the main chat.

use serde_json::Value;

use super::Turn;
use crate::tools::{self, Outcome};

/// Items one call may cover; more is a sign the list should be split.
pub(super) const MAX_ITEMS: usize = 30;
/// Each helper's report as it goes back to the main chat.
const REPORT_CHARS: usize = 1_500;

pub(super) fn items_of(args: &Value) -> Vec<String> {
    args.get("items")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)).collect())
        .unwrap_or_default()
}

pub(super) fn task_for(instruction: &str, item: &str) -> String {
    format!("{instruction}\n\nDo this for one item only: {item}\nReport what you found for it, briefly.")
}

impl Turn {
    pub(super) async fn run_for_each(&self, call_id: &str, args: &Value, ctx: &tools::Ctx<'_>) -> Outcome {
        let title = "Did it for each item".to_string();
        let instruction = match tools::arg_str(args, "instruction") {
            Ok(i) => i.to_string(),
            Err(e) => return Outcome::error(title, e),
        };
        let items = items_of(args);
        if items.is_empty() {
            return Outcome::error(title, "Give the items (items), for example the file names from list_files.");
        }
        if items.len() > MAX_ITEMS {
            return Outcome::error(title, format!("That's {} items; do at most {MAX_ITEMS} at a time and call again for the rest.", items.len()));
        }
        let mut reports = Vec::new();
        let mut failed = 0;
        for item in &items {
            if self.cancel.load(std::sync::atomic::Ordering::Relaxed) {
                reports.push(format!("### {item}\nStopped by the user before this one."));
                break;
            }
            let out = self.run_helper(call_id, &task_for(&instruction, item), ctx).await;
            if out.meta.get("status").and_then(Value::as_str) == Some("error") {
                failed += 1;
            }
            let text: String = out.text.chars().take(REPORT_CHARS).collect();
            reports.push(format!("### {item}\n{}", text.trim()));
        }
        let summary = if failed == 0 { format!("Did it for {} items", reports.len()) } else { format!("Did it for {} items ({failed} couldn't finish)", reports.len()) };
        Outcome::ok(reports.join("\n\n"), summary, "text", None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn items_are_read_and_each_task_names_one() {
        assert_eq!(items_of(&json!({ "items": ["a.txt", " ", "b.txt", 3] })), vec!["a.txt", "b.txt"]);
        assert!(items_of(&json!({})).is_empty());
        let t = task_for("Summarize it in one line", "notes/a.txt");
        assert!(t.starts_with("Summarize it in one line") && t.contains("one item only: notes/a.txt"));
    }
}

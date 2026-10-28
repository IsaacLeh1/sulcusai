// SPDX-License-Identifier: AGPL-3.0-only
//! Small helpers for the loop's steps: results cut to size, and calls told apart.

use super::*;

pub(super) fn now() -> i64 {
    db::now_ms()
}

/// A tool result cut to about a third of the model's memory, so one long
/// email, page or file can't crowd out the conversation (small models have
/// 8K tokens or less; about 3 characters a token, to be safe), and to the
/// tool's own limit. A command keeps its end as well as its start: that's
/// where errors are.
pub(super) fn fit_result(mut outcome: Outcome, name: &str, ctx: u32) -> Outcome {
    let share = (ctx as usize / 3) * 3;
    let max = if share == 0 { tools::result_limit(name) } else { share.min(tools::result_limit(name)) };
    let total = outcome.text.chars().count();
    if total <= max {
        return outcome;
    }
    let left = total - max;
    outcome.text = if matches!(name, "run_command" | "command_output") && max > 4_000 {
        let head: String = outcome.text.chars().take(2_000).collect();
        let tail: String = outcome.text.chars().skip(total - (max - 2_000)).collect();
        format!("{head}\n\n[Cut here: {left} more characters of output weren't shown, to fit your memory. The end follows.]\n\n{tail}")
    } else {
        let kept: String = outcome.text.chars().take(max).collect();
        let next = if name == "read_file" { "Read on with start_line if you need the rest." } else { "Answer from what's above." };
        format!("{kept}\n\n[Cut here: {left} more characters weren't shown, to fit your memory. {next}]")
    };
    outcome
}

/// Tools that reach the internet: a failure is often the network, worth one retry.
pub(super) fn is_network_tool(name: &str) -> bool {
    matches!(name, "web_search" | "fetch_page" | "wikipedia" | "research" | "market_quote" | "find_places" | "weather" | "news")
}

/// The end of the run of calls that can go together starting at `from`
/// (exclusive): consecutive calls that are each `ok`. A run of one isn't
/// worth it, so the caller only acts when the run is 2 or longer.
pub(super) fn run_end(ok: &[bool], from: usize) -> usize {
    ok[from..].iter().position(|b| !b).map_or(ok.len(), |n| from + n)
}

/// A tool call's name and arguments, with the arguments' formatting evened out.
pub(super) fn call_key(call: &ToolCall) -> String {
    let args = serde_json::from_str::<Value>(&call.arguments).map(|v| v.to_string()).unwrap_or_else(|_| call.arguments.clone());
    format!("{}{}", call.name, args)
}

/// A call's arguments as JSON (empty when they don't parse).
pub(super) fn args_of(call: &ToolCall) -> Value {
    serde_json::from_str(&call.arguments).unwrap_or_default()
}

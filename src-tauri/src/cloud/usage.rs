// SPDX-License-Identifier: AGPL-3.0-only
//! Cloud usage and cost by month, provider and project, for Settings and a
//! CSV export (what a company's finance team asks for).

use std::collections::BTreeMap;

use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::Spend;
use crate::db;

const KEY: &str = "cloud_usage";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct Slot {
    pub month: String,
    pub provider: String,
    /// The project's id, empty for chats outside projects.
    pub project: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Row {
    pub month: String,
    pub provider: String,
    pub project: String,
    pub requests: u64,
    pub input_tokens: u64,
    pub cached_tokens: u64,
    pub output_tokens: u64,
    pub usd: f64,
}

fn load(conn: &Connection) -> Vec<(Slot, Spend)> {
    db::get(conn, KEY).unwrap_or_default()
}

fn project_of(conn: &Connection, chat: &str) -> String {
    conn.query_row("SELECT project_id FROM chats WHERE id = ?1", [chat], |r| r.get::<_, Option<String>>(0)).optional().ok().flatten().flatten().unwrap_or_default()
}

/// Adds one reply's usage to its month, provider and project.
pub fn add(conn: &Connection, provider: &str, chat: Option<&str>, delta: &Spend) {
    let slot = Slot { month: super::this_month(), provider: provider.to_string(), project: chat.map(|c| project_of(conn, c)).unwrap_or_default() };
    let mut all = load(conn);
    let i = match all.iter().position(|(s, _)| *s == slot) {
        Some(i) => i,
        None => {
            all.push((slot.clone(), Spend { month: slot.month.clone(), ..Default::default() }));
            all.len() - 1
        }
    };
    let s = &mut all[i].1;
    s.usd += delta.usd;
    s.input_tokens += delta.input_tokens;
    s.cached_tokens += delta.cached_tokens;
    s.output_tokens += delta.output_tokens;
    s.requests += delta.requests;
    let _ = db::set(conn, KEY, &all);
}

/// Every month's rows, newest first, with project names.
pub fn rows(conn: &Connection) -> Vec<Row> {
    let names: BTreeMap<String, String> = conn
        .prepare("SELECT id, name FROM projects")
        .and_then(|mut st| st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).map(|it| it.filter_map(Result::ok).collect()))
        .unwrap_or_default();
    let mut all = load(conn);
    all.sort_by(|a, b| b.0.month.cmp(&a.0.month).then(a.0.provider.cmp(&b.0.provider)).then(a.0.project.cmp(&b.0.project)));
    all.into_iter()
        .map(|(slot, s)| Row {
            project: if slot.project.is_empty() { String::new() } else { names.get(&slot.project).cloned().unwrap_or_else(|| "(removed project)".into()) },
            month: slot.month,
            provider: slot.provider,
            requests: s.requests,
            input_tokens: s.input_tokens,
            cached_tokens: s.cached_tokens,
            output_tokens: s.output_tokens,
            usd: s.usd,
        })
        .collect()
}

fn cell(s: &str) -> String {
    // A leading = + - @ would run as a formula in a spreadsheet.
    let s = if s.starts_with(['=', '+', '-', '@']) { format!("'{s}") } else { s.to_string() };
    if s.contains([',', '"', '\n', '\r']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s }
}

pub fn csv(rows: &[Row]) -> String {
    let mut out = String::from("month,provider,project,requests,input_tokens,cached_tokens,output_tokens,usd\r\n");
    for r in rows {
        out += &format!("{},{},{},{},{},{},{},{:.4}\r\n", cell(&r.month), cell(&r.provider), cell(&r.project), r.requests, r.input_tokens, r.cached_tokens, r.output_tokens, r.usd);
    }
    out
}

#[tauri::command(async)]
pub fn cloud_usage(state: crate::AppStateRef) -> Vec<Row> {
    rows(&state.db.lock().unwrap())
}

/// Saves the usage table as CSV where the user picked in the save dialog.
#[tauri::command(async)]
pub fn cloud_usage_csv(state: crate::AppStateRef, path: String) -> Result<(), String> {
    if !path.to_lowercase().ends_with(".csv") {
        return Err("Save it as a .csv file.".into());
    }
    let text = csv(&rows(&state.db.lock().unwrap()));
    std::fs::write(&path, text).map_err(|e| format!("Couldn't save it: {e}"))?;
    state.log("files", "Saved the cloud usage as CSV");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_adds_up_per_provider_and_project() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::init_for_test(&conn);
        conn.execute("INSERT INTO projects (id, name, instructions, created_at, updated_at) VALUES ('p1', 'Launch, phase 2', '', 0, 0)", []).unwrap();
        conn.execute("INSERT INTO chats (id, title, created_at, updated_at, project_id) VALUES ('c1', 't', 0, 0, 'p1')", []).unwrap();
        let one = Spend { usd: 0.5, input_tokens: 100, output_tokens: 10, requests: 1, ..Default::default() };
        add(&conn, "Azure OpenAI", Some("c1"), &one);
        add(&conn, "Azure OpenAI", Some("c1"), &one);
        add(&conn, "AWS Bedrock", None, &one);
        let r = rows(&conn);
        assert_eq!(r.len(), 2);
        let azure = r.iter().find(|x| x.provider == "Azure OpenAI").unwrap();
        assert_eq!((azure.requests, azure.input_tokens, azure.project.as_str()), (2, 200, "Launch, phase 2"));
        let text = csv(&r);
        assert!(text.starts_with("month,provider,project,"));
        assert!(text.contains(",\"Launch, phase 2\",2,200,0,20,1.0000\r\n"), "{text}");
        assert_eq!(cell("=HYPERLINK(1)"), "'=HYPERLINK(1)");
    }
}

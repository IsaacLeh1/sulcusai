// SPDX-License-Identifier: AGPL-3.0-only
//! Home inventory and forms, both kept in the encrypted records store.
//! Inventory items add up per room for an insurance list; a form is a set of
//! fields checked before it is saved, so a filled form is never half valid.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::records::Record;

/// Cleans an inventory item: a name is required, the value is a number.
pub fn clean_item(data: &Value) -> Result<Value, String> {
    let o = data.as_object().ok_or("An item is a set of named fields.")?;
    let name = o.get("name").and_then(Value::as_str).map(str::trim).unwrap_or("");
    if name.is_empty() {
        return Err("An item needs a name.".into());
    }
    let mut out = o.clone();
    out.insert("name".into(), Value::from(name));
    if let Some(v) = o.get("value") {
        let n = match v {
            Value::Number(n) => n.as_f64(),
            Value::String(s) if s.trim().is_empty() => None,
            Value::String(s) => Some(s.trim().trim_start_matches('$').replace(',', "").parse::<f64>().map_err(|_| "The value must be a number.")?),
            Value::Null => None,
            _ => return Err("The value must be a number.".into()),
        };
        match n {
            Some(n) if n < 0.0 => return Err("The value can't be below zero.".into()),
            Some(n) => out.insert("value".into(), Value::from(n)),
            None => out.remove("value"),
        };
    }
    Ok(Value::Object(out))
}

#[derive(Serialize, Debug, PartialEq)]
pub struct RoomTotal {
    pub room: String,
    pub items: usize,
    pub value: f64,
}

pub fn totals(items: &[Record]) -> Vec<RoomTotal> {
    let mut out: Vec<RoomTotal> = vec![];
    for r in items {
        let room = r.data.get("room").and_then(Value::as_str).filter(|s| !s.trim().is_empty()).unwrap_or("No room").to_string();
        let v = r.data.get("value").and_then(Value::as_f64).unwrap_or(0.0);
        match out.iter_mut().find(|t| t.room == room) {
            Some(t) => {
                t.items += 1;
                t.value += v;
            }
            None => out.push(RoomTotal { room, items: 1, value: v }),
        }
    }
    out.sort_by(|a, b| a.room.cmp(&b.room));
    out
}

fn csv_cell(s: &str) -> String {
    // A leading = + - @ would run as a formula in a spreadsheet.
    let s = if s.starts_with(['=', '+', '-', '@']) { format!("'{s}") } else { s.to_string() };
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s
    }
}

/// The insurance list as CSV.
pub fn to_csv(items: &[Record]) -> String {
    let cols = ["name", "room", "value", "purchased", "serial", "notes"];
    let mut out = String::from("Name,Room,Value,Purchased,Serial,Notes\n");
    for r in items {
        let row: Vec<String> = cols
            .iter()
            .map(|c| match r.data.get(*c) {
                Some(Value::String(s)) => csv_cell(s),
                Some(Value::Number(n)) => n.to_string(),
                _ => String::new(),
            })
            .collect();
        out.push_str(&row.join(","));
        out.push('\n');
    }
    out
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Field {
    pub name: String,
    pub label: String,
    /// "text", "number", "date" (YYYY-MM-DD), "email" or "choice".
    pub kind: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub choices: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct FormTemplate {
    pub title: String,
    pub fields: Vec<Field>,
}

/// Checks filled values against the template; every problem is listed at once.
pub fn check_form(t: &FormTemplate, values: &Map<String, Value>) -> Result<Value, Vec<String>> {
    let mut errs = vec![];
    let mut out = Map::new();
    for f in &t.fields {
        let s = match values.get(&f.name) {
            Some(Value::String(s)) => s.trim().to_string(),
            Some(Value::Null) | None => String::new(),
            Some(other) => other.to_string(),
        };
        if s.is_empty() {
            if f.required {
                errs.push(format!("{} is required.", f.label));
            }
            continue;
        }
        let ok = match f.kind.as_str() {
            "number" => s.parse::<f64>().is_ok(),
            "date" => chrono::NaiveDate::parse_from_str(&s, "%Y-%m-%d").is_ok(),
            "email" => {
                let mut p = s.splitn(2, '@');
                matches!((p.next(), p.next()), (Some(a), Some(b)) if !a.is_empty() && b.contains('.') && !b.starts_with('.') && !b.ends_with('.'))
            }
            "choice" => f.choices.iter().any(|c| c == &s),
            _ => true,
        };
        if !ok {
            errs.push(format!("{} doesn't look right.", f.label));
            continue;
        }
        out.insert(f.name.clone(), if f.kind == "number" { Value::from(s.parse::<f64>().unwrap()) } else { Value::from(s) });
    }
    if errs.is_empty() {
        Ok(serde_json::json!({ "title": t.title, "values": out }))
    } else {
        Err(errs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rec(data: Value) -> Record {
        Record { id: "x".into(), kind: "inventory".into(), created: 0, updated: 0, data }
    }

    #[test]
    fn items_are_cleaned_and_totalled() {
        assert!(clean_item(&json!({"name": " "})).is_err());
        assert!(clean_item(&json!({"name": "TV", "value": "abc"})).is_err());
        assert!(clean_item(&json!({"name": "TV", "value": -1})).is_err());
        let tv = clean_item(&json!({"name": " TV ", "value": "$1,099.50", "room": "Living"})).unwrap();
        assert_eq!(tv["value"], 1099.5);
        assert_eq!(tv["name"], "TV");
        let items = vec![rec(tv), rec(json!({"name": "Rug", "value": 100.0, "room": "Living"})), rec(json!({"name": "Box"}))];
        let t = totals(&items);
        assert_eq!(t[0], RoomTotal { room: "Living".into(), items: 2, value: 1199.5 });
        assert_eq!(t[1].room, "No room");
        let csv = to_csv(&[rec(json!({"name": "=cmd()", "notes": "a, \"b\""}))]);
        assert!(csv.contains("'=cmd()"));
        assert!(csv.contains("\"a, \"\"b\"\"\""));
    }

    #[test]
    fn forms_list_every_problem() {
        let f = |n: &str, l: &str, k: &str, req: bool, ch: &[&str]| Field { name: n.into(), label: l.into(), kind: k.into(), required: req, choices: ch.iter().map(|s| s.to_string()).collect() };
        let t = FormTemplate {
            title: "Sign-up".into(),
            fields: vec![f("n", "Name", "text", true, &[]), f("e", "Email", "email", false, &[]), f("d", "Date", "date", false, &[]), f("s", "Size", "choice", false, &["S", "M"]), f("a", "Age", "number", false, &[])],
        };
        let bad = json!({"e": "x@y", "d": "2026-13-01", "s": "XL", "a": "old"});
        assert_eq!(check_form(&t, bad.as_object().unwrap()).unwrap_err().len(), 5);
        let good = json!({"n": "Sam", "e": "sam@example.com", "d": "2026-11-04", "s": "M", "a": "30"});
        assert_eq!(check_form(&t, good.as_object().unwrap()).unwrap()["values"]["a"], 30.0);
    }
}

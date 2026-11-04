// SPDX-License-Identifier: AGPL-3.0-only
//! Pictures in a batch from a spreadsheet saved as CSV: one picture per row.
//! A "prompt" column is needed; "negative", "seed", "width", "height",
//! "count" and "name" are used when present. Other columns fill {column}
//! placeholders in the prompt, so one template can make many pictures.

use std::collections::HashMap;

use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Item {
    pub row: usize,
    pub name: String,
    pub prompt: String,
    pub negative: String,
    pub seed: Option<i64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

/// A CSV line split into fields, with quoted fields and "" for a quote.
fn fields(line: &str, sep: char) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                chars.next();
                out.last_mut().unwrap().push('"');
            }
            '"' => quoted = !quoted,
            c if c == sep && !quoted => out.push(String::new()),
            c => out.last_mut().unwrap().push(c),
        }
    }
    out.into_iter().map(|f| f.trim().to_string()).collect()
}

/// Records, joining lines when a quoted field holds a line break.
fn records(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for line in text.trim_start_matches('\u{feff}').lines() {
        if !cur.is_empty() {
            cur.push('\n');
        }
        cur.push_str(line);
        if cur.matches('"').count() % 2 == 0 {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out.retain(|r| !r.trim().is_empty());
    out
}

pub const MAX: usize = 500;

pub fn parse(text: &str) -> Result<Vec<Item>, String> {
    let recs = records(text);
    let head = recs.first().ok_or("The spreadsheet is empty.")?;
    // Spreadsheets in many countries save with semicolons.
    let sep = if head.matches(';').count() > head.matches(',').count() { ';' } else { ',' };
    let cols: Vec<String> = fields(head, sep).into_iter().map(|c| c.to_lowercase()).collect();
    let at = |n: &str| cols.iter().position(|c| c == n);
    let p = at("prompt").ok_or("The first row needs a column named \"prompt\".")?;
    let mut out = Vec::new();
    for (i, rec) in recs.iter().enumerate().skip(1) {
        let f = fields(rec, sep);
        let get = |n: &str| at(n).and_then(|k| f.get(k)).map(String::as_str).unwrap_or("");
        let vals: HashMap<&str, &str> = cols.iter().map(String::as_str).zip(f.iter().map(String::as_str)).collect();
        let mut prompt = f.get(p).cloned().unwrap_or_default();
        for (k, v) in &vals {
            prompt = prompt.replace(&format!("{{{k}}}"), v);
        }
        if prompt.trim().is_empty() {
            continue;
        }
        let count = get("count").parse::<usize>().unwrap_or(1).clamp(1, 20);
        let seed = get("seed").parse::<i64>().ok();
        for c in 0..count {
            let base = if get("name").is_empty() { format!("row-{}", i + 1) } else { get("name").to_string() };
            out.push(Item {
                row: i + 1,
                name: if count > 1 { format!("{base}-{}", c + 1) } else { base },
                prompt: prompt.clone(),
                negative: get("negative").to_string(),
                seed: seed.map(|s| s + c as i64),
                width: get("width").parse().ok(),
                height: get("height").parse().ok(),
            });
            if out.len() > MAX {
                return Err(format!("At most {MAX} pictures in one batch."));
            }
        }
    }
    if out.is_empty() {
        return Err("No rows with a prompt.".into());
    }
    Ok(out)
}

/// A file name with only safe characters, unique within the batch.
pub fn file_names(items: &[Item]) -> Vec<String> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    items
        .iter()
        .map(|it| {
            let mut n: String = it.name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).take(60).collect();
            if n.trim_matches('-').is_empty() {
                n = format!("row-{}", it.row);
            }
            let k = seen.entry(n.clone()).or_default();
            *k += 1;
            if *k > 1 { format!("{n}-{k}.png") } else { format!("{n}.png") }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn template_and_columns() {
        let csv = "prompt,animal,seed,count,name\n\"a {animal}, watercolor\",fox,10,2,fox\nplain cat,,,,";
        let items = parse(csv).unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].prompt, "a fox, watercolor");
        assert_eq!((items[0].seed, items[1].seed), (Some(10), Some(11)));
        assert_eq!(items[1].name, "fox-2");
        assert_eq!(items[2].name, "row-3");
    }
    #[test]
    fn semicolons_quotes_and_breaks() {
        let csv = "\u{feff}Prompt;Width\n\"say \"\"hi\"\"\nthere\";640";
        let items = parse(csv).unwrap();
        assert_eq!(items[0].prompt, "say \"hi\"\nthere");
        assert_eq!(items[0].width, Some(640));
    }
    #[test]
    fn needs_prompt_column() {
        assert!(parse("text\nhello").is_err());
    }
    #[test]
    fn names_unique_and_safe() {
        let items = parse("prompt,name\na,x/y\nb,x/y\nc,\"..\"").unwrap();
        assert_eq!(file_names(&items), vec!["x-y.png", "x-y-2.png", "row-4.png"]);
    }
}

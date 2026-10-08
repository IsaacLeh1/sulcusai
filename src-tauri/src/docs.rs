// SPDX-License-Identifier: AGPL-3.0-only
//! Documents without Office: Word, Excel (working formulas and charts),
//! PowerPoint, PDF and Markdown, written by local libraries; and any of
//! those (plus CSV and text) read back as text.
//!
//! - Word and PDF are written from Markdown (headings, paragraphs, bold,
//!   italic, code, bullet and numbered lists, tables).
//! - Excel takes sheets of rows: text, numbers, and formulas starting with =.
//! - PowerPoint takes slides (title, bullets), built on a blank 16:9
//!   template made from python-pptx's default (MIT).
//! - PDF uses the built-in Helvetica and Courier fonts (Windows-1252 text).

use std::io::{Cursor, Read, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Read-back text kept, in characters.
pub const MAX_READ: usize = 40_000;

// ---------- Markdown ----------

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Heading(u8, Vec<Span>),
    Para(Vec<Span>),
    Bullet(u8, Vec<Span>),
    Numbered(u8, u32, Vec<Span>),
    Code(String),
    Table(Vec<Vec<String>>),
    Rule,
}

/// Inline **bold**, *italic*, `code` and [links](url).
pub fn spans(text: &str) -> Vec<Span> {
    let link = regex::Regex::new(r"\[([^\]]+)\]\(([^)\s]+)\)").unwrap();
    let text = link.replace_all(text, "$1 ($2)");
    let mut out: Vec<Span> = Vec::new();
    let (mut bold, mut italic, mut code) = (false, false, false);
    let mut cur = String::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    let flush = |cur: &mut String, out: &mut Vec<Span>, b: bool, it: bool, c: bool| {
        if !cur.is_empty() {
            out.push(Span { text: std::mem::take(cur), bold: b, italic: it, code: c });
        }
    };
    while i < chars.len() {
        let ch = chars[i];
        if ch == '`' {
            flush(&mut cur, &mut out, bold, italic, code);
            code = !code;
        } else if code {
            cur.push(ch);
        } else if ch == '*' && chars.get(i + 1) == Some(&'*') || ch == '_' && chars.get(i + 1) == Some(&'_') {
            flush(&mut cur, &mut out, bold, italic, code);
            bold = !bold;
            i += 1;
        } else if ch == '*' || ch == '_' && (i == 0 || !chars[i - 1].is_alphanumeric() || chars.get(i + 1).is_none_or(|n| !n.is_alphanumeric())) {
            flush(&mut cur, &mut out, bold, italic, code);
            italic = !italic;
        } else if ch == '\\' && i + 1 < chars.len() {
            cur.push(chars[i + 1]);
            i += 1;
        } else {
            cur.push(ch);
        }
        i += 1;
    }
    flush(&mut cur, &mut out, bold, italic, code);
    out
}

pub fn plain(spans: &[Span]) -> String {
    spans.iter().map(|s| s.text.as_str()).collect()
}

pub fn parse_markdown(md: &str) -> Vec<Block> {
    let heading = regex::Regex::new(r"^(#{1,6})\s+(.*)$").unwrap();
    let bullet = regex::Regex::new(r"^(\s*)[-*+]\s+(.*)$").unwrap();
    let numbered = regex::Regex::new(r"^(\s*)(\d+)[.)]\s+(.*)$").unwrap();
    let mut out = Vec::new();
    let mut para: Vec<String> = Vec::new();
    let mut table: Vec<Vec<String>> = Vec::new();
    let mut code: Option<String> = None;
    let end_para = |para: &mut Vec<String>, out: &mut Vec<Block>| {
        if !para.is_empty() {
            out.push(Block::Para(spans(&para.join(" "))));
            para.clear();
        }
    };
    let end_table = |table: &mut Vec<Vec<String>>, out: &mut Vec<Block>| {
        if !table.is_empty() {
            out.push(Block::Table(std::mem::take(table)));
        }
    };
    for raw in md.replace("\r\n", "\n").split('\n') {
        let line = raw.trim_end();
        if let Some(c) = code.as_mut() {
            if line.trim_start().starts_with("```") {
                out.push(Block::Code(std::mem::take(c).trim_end_matches('\n').to_string()));
                code = None;
            } else {
                c.push_str(line);
                c.push('\n');
            }
            continue;
        }
        if line.trim_start().starts_with("```") {
            end_para(&mut para, &mut out);
            end_table(&mut table, &mut out);
            code = Some(String::new());
            continue;
        }
        let t = line.trim();
        if t.starts_with('|') {
            end_para(&mut para, &mut out);
            let cells: Vec<String> = t.trim_matches('|').split('|').map(|c| c.trim().to_string()).collect();
            // The |---|---| line under the header.
            if !cells.iter().all(|c| !c.is_empty() && c.chars().all(|ch| matches!(ch, '-' | ':' | ' '))) {
                table.push(cells);
            }
            continue;
        }
        end_table(&mut table, &mut out);
        if t.is_empty() {
            end_para(&mut para, &mut out);
        } else if matches!(t, "---" | "***" | "___") {
            end_para(&mut para, &mut out);
            out.push(Block::Rule);
        } else if let Some(c) = heading.captures(line) {
            end_para(&mut para, &mut out);
            out.push(Block::Heading(c[1].len() as u8, spans(c[2].trim())));
        } else if let Some(c) = bullet.captures(line) {
            end_para(&mut para, &mut out);
            out.push(Block::Bullet((c[1].len() / 2).min(4) as u8, spans(&c[2])));
        } else if let Some(c) = numbered.captures(line) {
            end_para(&mut para, &mut out);
            out.push(Block::Numbered((c[1].len() / 2).min(4) as u8, c[2].parse().unwrap_or(1), spans(&c[3])));
        } else {
            para.push(t.to_string());
        }
    }
    if let Some(c) = code {
        out.push(Block::Code(c));
    }
    end_para(&mut para, &mut out);
    end_table(&mut table, &mut out);
    out
}

// ---------- Word ----------

pub fn write_docx(path: &Path, md: &str) -> Result<(), String> {
    use docx_rs::*;
    let run_of = |s: &Span| {
        let mut r = Run::new().add_text(&s.text);
        if s.bold {
            r = r.bold();
        }
        if s.italic {
            r = r.italic();
        }
        if s.code {
            r = r.fonts(RunFonts::new().ascii("Consolas").hi_ansi("Consolas"));
        }
        r
    };
    let para_of = |sp: &[Span]| sp.iter().fold(Paragraph::new(), |p, s| p.add_run(run_of(s)));
    let level = |n: usize, fmt: &str, text: &str| {
        Level::new(n, Start::new(1), NumberFormat::new(fmt), LevelText::new(text), LevelJc::new("left"))
            .indent(Some(720 * (n as i32 + 1)), Some(SpecialIndentType::Hanging(360)), None, None)
    };
    let bullets = (0..5).fold(AbstractNumbering::new(1), |a, n| a.add_level(level(n, "bullet", ["•", "◦", "▪", "•", "◦"][n])));
    let numbers = (0..5).fold(AbstractNumbering::new(2), |a, n| a.add_level(level(n, ["decimal", "lowerLetter", "lowerRoman", "decimal", "lowerLetter"][n], &format!("%{}.", n + 1))));
    let mut doc = Docx::new()
        .add_style(Style::new("Heading1", StyleType::Paragraph).name("Heading 1").size(36).bold().color("1F3864"))
        .add_style(Style::new("Heading2", StyleType::Paragraph).name("Heading 2").size(30).bold().color("2F5496"))
        .add_style(Style::new("Heading3", StyleType::Paragraph).name("Heading 3").size(26).bold().color("2F5496"))
        .add_abstract_numbering(bullets)
        .add_abstract_numbering(numbers)
        .add_numbering(Numbering::new(1, 1));
    // Each numbered list restarts at 1.
    let mut next_numbering = 2;
    let mut in_numbered = false;
    for b in parse_markdown(md) {
        let numbered_now = matches!(b, Block::Numbered(..));
        if numbered_now && !in_numbered {
            next_numbering += 1;
            doc = doc.add_numbering(Numbering::new(next_numbering, 2));
        }
        in_numbered = numbered_now;
        doc = match b {
            Block::Heading(n, sp) => doc.add_paragraph(para_of(&sp).style(&format!("Heading{}", n.min(3)))),
            Block::Para(sp) => doc.add_paragraph(para_of(&sp)),
            Block::Bullet(l, sp) => doc.add_paragraph(para_of(&sp).numbering(NumberingId::new(1), IndentLevel::new(l as usize))),
            Block::Numbered(l, _, sp) => doc.add_paragraph(para_of(&sp).numbering(NumberingId::new(next_numbering), IndentLevel::new(l as usize))),
            Block::Code(text) => text.lines().fold(doc, |d, line| {
                d.add_paragraph(Paragraph::new().add_run(Run::new().add_text(line).fonts(RunFonts::new().ascii("Consolas").hi_ansi("Consolas")).size(19)))
            }),
            Block::Rule => doc.add_paragraph(Paragraph::new().add_run(Run::new().add_text("—".repeat(30)))),
            Block::Table(rows) => {
                let rows: Vec<TableRow> = rows
                    .iter()
                    .enumerate()
                    .map(|(i, r)| {
                        TableRow::new(
                            r.iter()
                                .map(|c| {
                                    let sp = spans(c);
                                    let p = sp.iter().fold(Paragraph::new(), |p, s| {
                                        let r = run_of(s);
                                        p.add_run(if i == 0 { r.bold() } else { r })
                                    });
                                    TableCell::new().add_paragraph(p)
                                })
                                .collect(),
                        )
                    })
                    .collect();
                doc.add_table(Table::new(rows))
            }
        };
    }
    let mut buf = Cursor::new(Vec::new());
    doc.pack(&mut buf).map_err(|e| e.to_string())?;
    std::fs::write(path, buf.into_inner()).map_err(|e| e.to_string())
}

// ---------- Excel ----------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Sheet {
    pub name: String,
    /// Rows of cells: text, numbers, or formulas starting with "=".
    pub rows: Vec<serde_json::Value>,
    pub charts: Vec<ChartSpec>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ChartSpec {
    /// column, bar, line, pie, area or scatter.
    #[serde(rename = "type")]
    pub kind: String,
    pub title: String,
    /// The labels, e.g. "A2:A6".
    pub categories: String,
    /// One range per series, e.g. ["B2:B6", "C2:C6"].
    pub values: Vec<String>,
    /// Series names, in the same order.
    pub names: Vec<String>,
    /// Where it goes, e.g. "E2" (default: right of the data).
    pub at: String,
}

/// "B12" → (row 11, column 1).
pub fn cell_ref(a1: &str) -> Option<(u32, u16)> {
    let a1 = a1.trim().replace('$', "");
    let split = a1.find(|c: char| c.is_ascii_digit())?;
    let (letters, digits) = a1.split_at(split);
    if letters.is_empty() {
        return None;
    }
    let col = letters.to_ascii_uppercase().bytes().try_fold(0u32, |acc, b| b.is_ascii_uppercase().then(|| acc * 26 + (b - b'A' + 1) as u32))?;
    let row: u32 = digits.parse().ok()?;
    (row >= 1 && col >= 1).then(|| (row - 1, (col - 1) as u16))
}

fn absolute(sheet: &str, range: &str) -> Result<String, String> {
    let (a, b) = range.split_once(':').unwrap_or((range, range));
    let (r1, c1) = cell_ref(a).ok_or_else(|| format!("“{range}” isn't a cell range like B2:B6."))?;
    let (r2, c2) = cell_ref(b).ok_or_else(|| format!("“{range}” isn't a cell range like B2:B6."))?;
    let col = |c: u16| {
        let mut n = c as u32 + 1;
        let mut s = String::new();
        while n > 0 {
            let m = (n - 1) % 26;
            s.insert(0, (b'A' + m as u8) as char);
            n = (n - 1) / 26;
        }
        s
    };
    Ok(format!("'{}'!${}${}:${}${}", sheet.replace('\'', "''"), col(c1), r1 + 1, col(c2), r2 + 1))
}

/// A row as cells: a list, an object holding one ({"cells": [...]}), or a
/// line of text split on tabs or commas.
pub fn row_cells(row: &serde_json::Value) -> Vec<serde_json::Value> {
    match row {
        serde_json::Value::Array(a) => a.clone(),
        serde_json::Value::Object(o) => ["cells", "values", "row", "data"]
            .iter()
            .find_map(|k| o.get(*k).and_then(|v| v.as_array()).cloned())
            .unwrap_or_else(|| o.values().cloned().collect()),
        serde_json::Value::String(t) => sheets_from_text(t).pop().and_then(|s| s.rows.into_iter().next()).map(|r| row_cells(&r)).unwrap_or_default(),
        other => vec![other.clone()],
    }
}

/// Formulas that name things instead of cells ("=SUM(Rent, Food)") show
/// #NAME? in Excel. Returns the unknown names.
pub fn unknown_names(formula: &str) -> Vec<String> {
    let cell = regex::Regex::new(r"^\$?[A-Za-z]{1,3}\$?\d+$").unwrap();
    let ident = regex::Regex::new(r#""[^"]*"|'[^']*'!|[A-Za-z_][A-Za-z0-9_.]*(\s*[(!])?"#).unwrap();
    let mut out = Vec::new();
    for m in ident.find_iter(formula.trim_start_matches('=')) {
        let t = m.as_str();
        if t.starts_with('"') || t.starts_with('\'') || t.ends_with('(') || t.ends_with('!') || cell.is_match(t) {
            continue;
        }
        let up = t.to_ascii_uppercase();
        // Whole columns and rows (A:A) and the two constants are fine.
        if matches!(up.as_str(), "TRUE" | "FALSE") || up.chars().all(|c| c.is_ascii_uppercase()) && up.len() <= 3 && formula.contains(&format!("{t}:")) {
            continue;
        }
        out.push(t.to_string());
    }
    out
}

/// The value of simple totals (SUM, AVERAGE, MIN, MAX, COUNT of cells,
/// ranges and numbers), so the file shows numbers before Excel recalculates.
fn simple_result(formula: &str, grid: &[Vec<Option<f64>>]) -> Option<f64> {
    let re = regex::Regex::new(r"(?i)^=\s*(SUM|AVERAGE|MIN|MAX|COUNT)\((.*)\)\s*$").unwrap();
    let c = re.captures(formula)?;
    let mut vals = Vec::new();
    for arg in c[2].split(',').map(str::trim).filter(|a| !a.is_empty()) {
        if let Ok(n) = arg.parse::<f64>() {
            vals.push(n);
            continue;
        }
        let (a, b) = arg.split_once(':').unwrap_or((arg, arg));
        let ((r1, c1), (r2, c2)) = (cell_ref(a)?, cell_ref(b)?);
        for r in r1.min(r2)..=r1.max(r2) {
            for col in c1.min(c2)..=c1.max(c2) {
                if let Some(Some(v)) = grid.get(r as usize).and_then(|row| row.get(col as usize)) {
                    vals.push(*v);
                }
            }
        }
    }
    let n = vals.len() as f64;
    match c[1].to_ascii_uppercase().as_str() {
        "SUM" => Some(vals.iter().sum()),
        "AVERAGE" if n > 0.0 => Some(vals.iter().sum::<f64>() / n),
        "MIN" => vals.iter().cloned().reduce(f64::min).or(Some(0.0)),
        "MAX" => vals.iter().cloned().reduce(f64::max).or(Some(0.0)),
        "COUNT" => Some(n),
        _ => None,
    }
}

fn number_of(v: &serde_json::Value) -> Option<f64> {
    match v {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) if !s.starts_with('=') => s.trim().parse().ok(),
        _ => None,
    }
}

pub fn write_xlsx(path: &Path, sheets: &[Sheet]) -> Result<(), String> {
    for s in sheets {
        for row in &s.rows {
            for c in row_cells(row) {
                if let Some(f) = c.as_str().filter(|f| f.starts_with('=')) {
                    let names = unknown_names(f);
                    if !names.is_empty() {
                        return Err(format!("The formula {f} names {} instead of cells. Use cell references such as =SUM(B2:B4).", names.join(", ")));
                    }
                }
            }
        }
    }
    use rust_xlsxwriter::{Chart, ChartType, Format, Formula, Workbook};
    if sheets.is_empty() {
        return Err("Give at least one sheet with rows.".into());
    }
    let mut wb = Workbook::new();
    let header = Format::new().set_bold();
    for (i, s) in sheets.iter().enumerate() {
        let name = if s.name.trim().is_empty() { format!("Sheet{}", i + 1) } else { s.name.trim().chars().take(31).collect() };
        let ws = wb.add_worksheet();
        ws.set_name(&name).map_err(|e| e.to_string())?;
        // Numbers by position, with simple totals filled in as they're found.
        let mut grid: Vec<Vec<Option<f64>>> = s.rows.iter().map(|r| row_cells(r).iter().map(number_of).collect()).collect();
        let mut results: std::collections::HashMap<(usize, usize), f64> = std::collections::HashMap::new();
        for (r, row) in s.rows.iter().enumerate() {
            for (c, v) in row_cells(row).iter().enumerate() {
                if let Some(x) = v.as_str().filter(|f| f.starts_with('=')).and_then(|f| simple_result(f, &grid)) {
                    grid[r][c] = Some(x);
                    results.insert((r, c), x);
                }
            }
        }
        for (r, row) in s.rows.iter().enumerate() {
            for (c, v) in row_cells(row).iter().enumerate() {
                let (r, c) = (r as u32, c as u16);
                let fmt = (r == 0).then_some(&header);
                let res = match v {
                    serde_json::Value::Number(n) => match fmt {
                        Some(f) => ws.write_number_with_format(r, c, n.as_f64().unwrap_or(0.0), f).map(|_| ()),
                        None => ws.write_number(r, c, n.as_f64().unwrap_or(0.0)).map(|_| ()),
                    },
                    serde_json::Value::Bool(b) => ws.write_boolean(r, c, *b).map(|_| ()),
                    serde_json::Value::Null => Ok(()),
                    other => {
                        let text = other.as_str().map(str::to_string).unwrap_or_else(|| other.to_string());
                        if let Some(f) = text.strip_prefix('=').filter(|f| !f.is_empty()) {
                            let mut formula = Formula::new(format!("={f}"));
                            if let Some(x) = results.get(&(r as usize, c as usize)) {
                                formula = formula.set_result(x.to_string());
                            }
                            ws.write_formula(r, c, formula).map(|_| ())
                        } else if let Some(n) = text.trim().parse::<f64>().ok().filter(|_| r > 0 && !text.trim().is_empty()) {
                            ws.write_number(r, c, n).map(|_| ())
                        } else {
                            match fmt {
                                Some(f) => ws.write_string_with_format(r, c, &text, f).map(|_| ()),
                                None => ws.write_string(r, c, &text).map(|_| ()),
                            }
                        }
                    }
                };
                res.map_err(|e| e.to_string())?;
            }
        }
        ws.autofit();
        let width = s.rows.iter().map(|r| row_cells(r).len()).max().unwrap_or(0) as u16;
        for (k, ch) in s.charts.iter().enumerate() {
            let kind = match ch.kind.to_lowercase().as_str() {
                "bar" => ChartType::Bar,
                "line" => ChartType::Line,
                "pie" => ChartType::Pie,
                "area" => ChartType::Area,
                "scatter" => ChartType::Scatter,
                "doughnut" => ChartType::Doughnut,
                _ => ChartType::Column,
            };
            let mut chart = Chart::new(kind);
            if ch.values.is_empty() {
                return Err("A chart needs values, e.g. \"values\": [\"B2:B6\"].".into());
            }
            for (j, v) in ch.values.iter().enumerate() {
                let series = chart.add_series();
                series.set_values(absolute(&name, v)?.as_str());
                if !ch.categories.trim().is_empty() {
                    series.set_categories(absolute(&name, &ch.categories)?.as_str());
                }
                if let Some(n) = ch.names.get(j) {
                    series.set_name(n.as_str());
                }
            }
            if !ch.title.is_empty() {
                chart.title().set_name(&ch.title);
            }
            let (row, col) = cell_ref(&ch.at).unwrap_or((1 + 16 * k as u32, width + 1));
            ws.insert_chart(row, col, &chart).map_err(|e| e.to_string())?;
        }
    }
    wb.save(path).map_err(|e| format!("Couldn't save the spreadsheet: {e}"))
}

/// Sheets from text: Markdown tables (one sheet each), or lines of cells
/// separated by tabs, commas or | (what small models tend to write).
pub fn sheets_from_text(text: &str) -> Vec<Sheet> {
    // JSON: a list of rows, or a list of records (their keys become the header).
    if let Ok(serde_json::Value::Array(items)) = serde_json::from_str::<serde_json::Value>(text.trim()) {
        let nested = |v: &serde_json::Value| v.as_object().is_some_and(|o| o.values().any(|x| x.is_array() || x.is_object()));
        if items.iter().any(nested) {
            return Vec::new();
        }
        if let Some(serde_json::Value::Object(first)) = items.first() {
            let keys: Vec<String> = first.keys().cloned().collect();
            let mut rows = vec![serde_json::Value::Array(keys.iter().map(|k| serde_json::json!(k)).collect())];
            rows.extend(items.iter().filter_map(|i| i.as_object()).map(|o| serde_json::Value::Array(keys.iter().map(|k| o.get(k).cloned().unwrap_or(serde_json::Value::Null)).collect())));
            return vec![Sheet { name: "Sheet1".into(), rows, charts: vec![] }];
        }
        if items.iter().all(|i| i.is_array()) && !items.is_empty() {
            return vec![Sheet { name: "Sheet1".into(), rows: items, charts: vec![] }];
        }
    }
    let cell = |c: &str| -> serde_json::Value {
        let c = c.trim();
        match c.replace(',', "").parse::<f64>() {
            Ok(n) if !c.is_empty() && !c.starts_with('=') => serde_json::json!(n),
            _ => serde_json::json!(c),
        }
    };
    let tables: Vec<Vec<Vec<String>>> = parse_markdown(text).into_iter().filter_map(|b| if let Block::Table(t) = b { Some(t) } else { None }).collect();
    if !tables.is_empty() {
        return tables
            .into_iter()
            .enumerate()
            .map(|(i, t)| Sheet { name: format!("Sheet{}", i + 1), rows: t.iter().map(|r| serde_json::Value::Array(r.iter().map(|c| cell(c)).collect())).collect(), charts: vec![] })
            .collect();
    }
    // Lines, without list brackets ("[Rent, 1200]") or bullets.
    let lines: Vec<&str> = text
        .lines()
        .map(|l| l.trim().trim_start_matches(['-', '*']).trim().trim_start_matches('[').trim_end_matches([']', ',']).trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();
    let sep = ['\t', '|', ';', ','].into_iter().find(|d| lines.iter().filter(|l| l.contains(*d)).count() * 2 >= lines.len().max(1)).unwrap_or('\t');
    let rows: Vec<serde_json::Value> = lines.iter().map(|l| serde_json::Value::Array(l.trim_matches('|').split(sep).map(|c| cell(c)).collect())).collect();
    if rows.is_empty() {
        return Vec::new();
    }
    vec![Sheet { name: "Sheet1".into(), rows, charts: vec![] }]
}

// ---------- PowerPoint ----------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Slide {
    pub title: String,
    /// A title slide's second line.
    pub subtitle: String,
    /// Points; two leading spaces per level of indent.
    pub bullets: Vec<String>,
}

const TEMPLATE: &[u8] = include_bytes!("../assets/blank.pptx");

/// Slides from Markdown: each heading starts a slide; its bullets and
/// lines become points. A slide with only a line under its heading is a
/// title slide with that line as its subtitle.
pub fn slides_from_markdown(md: &str) -> Vec<Slide> {
    let mut out: Vec<Slide> = Vec::new();
    for b in parse_markdown(md) {
        match b {
            Block::Heading(_, sp) => out.push(Slide { title: plain(&sp), ..Default::default() }),
            Block::Bullet(l, sp) | Block::Numbered(l, _, sp) => {
                if out.is_empty() {
                    out.push(Slide::default());
                }
                out.last_mut().unwrap().bullets.push(format!("{}{}", "  ".repeat(l as usize), plain(&sp)));
            }
            Block::Para(sp) => {
                if out.is_empty() {
                    out.push(Slide::default());
                }
                let s = out.last_mut().unwrap();
                if s.bullets.is_empty() && s.subtitle.is_empty() {
                    s.subtitle = plain(&sp);
                } else {
                    s.bullets.push(plain(&sp));
                }
            }
            _ => {}
        }
    }
    // A subtitle on a slide that also has points reads as a point.
    for s in &mut out {
        if !s.bullets.is_empty() && !s.subtitle.is_empty() {
            s.bullets.insert(0, std::mem::take(&mut s.subtitle));
        }
    }
    out
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

const NS: &str = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#;

fn shape(id: u32, name: &str, ph: &str, paras: &str) -> String {
    format!(
        r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="{name}"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr>{ph}</p:nvPr></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/>{paras}</p:txBody></p:sp>"#
    )
}

fn para(text: &str, level: usize) -> String {
    let lvl = if level > 0 { format!(r#"<a:pPr lvl="{level}"/>"#) } else { String::new() };
    format!(r#"<a:p>{lvl}<a:r><a:rPr lang="en-US" dirty="0"/><a:t>{}</a:t></a:r></a:p>"#, esc(text))
}

/// The slide's XML and which layout it uses (1 = title slide, 2 = title and content).
fn slide_xml(s: &Slide) -> (String, u32) {
    let title_slide = s.bullets.is_empty();
    let shapes = if title_slide {
        let sub = if s.subtitle.is_empty() { "<a:p><a:endParaRPr lang=\"en-US\"/></a:p>".to_string() } else { para(&s.subtitle, 0) };
        shape(2, "Title 1", r#"<p:ph type="ctrTitle"/>"#, &para(&s.title, 0)) + &shape(3, "Subtitle 2", r#"<p:ph type="subTitle" idx="1"/>"#, &sub)
    } else {
        let body: String = s
            .bullets
            .iter()
            .map(|b| {
                let indent = b.len() - b.trim_start().len();
                let text = b.trim_start().trim_start_matches(['-', '*', '•']).trim_start();
                para(text, (indent / 2).min(4))
            })
            .collect();
        shape(2, "Title 1", r#"<p:ph type="title"/>"#, &para(&s.title, 0)) + &shape(3, "Content Placeholder 2", r#"<p:ph idx="1"/>"#, &body)
    };
    (
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:sld {NS}><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>{shapes}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>"#
        ),
        if title_slide { 1 } else { 2 },
    )
}

pub fn write_pptx(path: &Path, slides: &[Slide]) -> Result<(), String> {
    use zip::write::SimpleFileOptions;
    if slides.is_empty() {
        return Err("Give at least one slide.".into());
    }
    let mut tpl = zip::ZipArchive::new(Cursor::new(TEMPLATE)).map_err(|e| e.to_string())?;
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let ids: Vec<String> = (0..slides.len()).map(|i| format!("rIdS{}", i + 1)).collect();
    for i in 0..tpl.len() {
        let mut f = tpl.by_index(i).map_err(|e| e.to_string())?;
        let name = f.name().to_string();
        let mut data = Vec::new();
        f.read_to_end(&mut data).map_err(|e| e.to_string())?;
        let text = || String::from_utf8_lossy(&data).to_string();
        let data = match name.as_str() {
            "[Content_Types].xml" => {
                let overrides: String = (1..=slides.len())
                    .map(|n| format!(r#"<Override PartName="/ppt/slides/slide{n}.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>"#))
                    .collect();
                text().replace("</Types>", &format!("{overrides}</Types>")).into_bytes()
            }
            "ppt/_rels/presentation.xml.rels" => {
                let rels: String = ids
                    .iter()
                    .enumerate()
                    .map(|(i, id)| format!(r#"<Relationship Id="{id}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide{}.xml"/>"#, i + 1))
                    .collect();
                text().replace("</Relationships>", &format!("{rels}</Relationships>")).into_bytes()
            }
            "ppt/presentation.xml" => {
                let list: String = ids.iter().enumerate().map(|(i, id)| format!(r#"<p:sldId id="{}" r:id="{id}"/>"#, 256 + i)).collect();
                text().replace("</p:sldMasterIdLst>", &format!("</p:sldMasterIdLst><p:sldIdLst>{list}</p:sldIdLst>")).into_bytes()
            }
            _ => data,
        };
        out.start_file(name, opts).map_err(|e| e.to_string())?;
        out.write_all(&data).map_err(|e| e.to_string())?;
    }
    for (i, s) in slides.iter().enumerate() {
        let (xml, layout) = slide_xml(s);
        out.start_file(format!("ppt/slides/slide{}.xml", i + 1), opts).map_err(|e| e.to_string())?;
        out.write_all(xml.as_bytes()).map_err(|e| e.to_string())?;
        out.start_file(format!("ppt/slides/_rels/slide{}.xml.rels", i + 1), opts).map_err(|e| e.to_string())?;
        let rels = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout" Target="../slideLayouts/slideLayout{layout}.xml"/></Relationships>"#
        );
        out.write_all(rels.as_bytes()).map_err(|e| e.to_string())?;
    }
    let bytes = out.finish().map_err(|e| e.to_string())?.into_inner();
    std::fs::write(path, bytes).map_err(|e| e.to_string())
}

// ---------- PDF ----------

/// Helvetica and Helvetica-Bold advance widths for ' ' through '~' (per 1000).
const HELV: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, 1015, 667,
    667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 278, 278, 278, 469, 556, 333, 556, 556, 500,
    556, 556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584,
];
const HELV_BOLD: [u16; 95] = [
    278, 333, 474, 556, 556, 889, 722, 238, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 333, 333, 584, 584, 584, 611, 975, 722,
    722, 722, 722, 667, 611, 778, 722, 278, 556, 722, 611, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 333, 278, 333, 584, 556, 333, 556, 611, 556,
    611, 556, 333, 611, 611, 278, 278, 556, 278, 889, 611, 611, 611, 611, 389, 556, 333, 611, 556, 778, 556, 556, 500, 389, 280, 389, 584,
];

#[derive(Clone, Copy, PartialEq)]
enum Font {
    Regular,
    Bold,
    Italic,
    Mono,
}

impl Font {
    fn of(s: &Span) -> Font {
        if s.code {
            Font::Mono
        } else if s.bold {
            Font::Bold
        } else if s.italic {
            Font::Italic
        } else {
            Font::Regular
        }
    }
    fn name(self) -> &'static str {
        match self {
            Font::Regular => "F1",
            Font::Bold => "F2",
            Font::Italic => "F3",
            Font::Mono => "F4",
        }
    }
    fn width(self, b: u8) -> f32 {
        match self {
            Font::Mono => 600.0,
            _ if (32..=126).contains(&b) => (if self == Font::Bold { HELV_BOLD } else { HELV })[(b - 32) as usize] as f32,
            _ => 556.0,
        }
    }
}

/// Text as Windows-1252 bytes (what the built-in fonts use).
fn win1252(s: &str) -> Vec<u8> {
    s.chars()
        .map(|c| match c {
            '\u{20}'..='\u{7e}' | '\u{a0}'..='\u{ff}' => c as u32 as u8,
            '€' => 0x80,
            '‚' => 0x82,
            '…' => 0x85,
            '‘' => 0x91,
            '’' => 0x92,
            '“' => 0x93,
            '”' => 0x94,
            '•' | '◦' | '▪' => 0x95,
            '–' => 0x96,
            '—' => 0x97,
            '™' => 0x99,
            '\t' => b' ',
            _ => b'?',
        })
        .collect()
}

struct PdfWriter {
    pages: Vec<Vec<u8>>,
    cur: Vec<u8>,
    y: f32,
}

const PAGE_W: f32 = 612.0;
const PAGE_H: f32 = 792.0;
const MARGIN: f32 = 64.0;

impl PdfWriter {
    fn new() -> Self {
        PdfWriter { pages: Vec::new(), cur: Vec::new(), y: PAGE_H - MARGIN }
    }

    fn need(&mut self, h: f32) {
        if self.y - h < MARGIN {
            self.pages.push(std::mem::take(&mut self.cur));
            self.y = PAGE_H - MARGIN;
        }
    }

    fn text(&mut self, x: f32, y: f32, font: Font, size: f32, bytes: &[u8]) {
        let mut s = Vec::new();
        for &b in bytes {
            if matches!(b, b'(' | b')' | b'\\') {
                s.push(b'\\');
            }
            s.push(b);
        }
        self.cur.extend_from_slice(format!("BT /{} {size} Tf {x:.2} {y:.2} Td (", font.name()).as_bytes());
        self.cur.extend_from_slice(&s);
        self.cur.extend_from_slice(b") Tj ET\n");
    }

    /// Lays out spans in lines, wrapping between words.
    fn flow(&mut self, spans: &[Span], size: f32, indent: f32, prefix: Option<&str>, force: Option<Font>) {
        let max = PAGE_W - 2.0 * MARGIN - indent;
        let line_h = size * 1.4;
        // Words with their fonts; spaces stay attached to the word before.
        let mut words: Vec<(Font, Vec<u8>)> = Vec::new();
        for s in spans {
            let f = force.unwrap_or(Font::of(s));
            let bytes = win1252(&s.text);
            let mut start = 0;
            for (i, &b) in bytes.iter().enumerate() {
                if b == b' ' {
                    words.push((f, bytes[start..=i].to_vec()));
                    start = i + 1;
                }
            }
            if start < bytes.len() {
                words.push((f, bytes[start..].to_vec()));
            }
        }
        let width = |f: Font, w: &[u8]| w.iter().map(|&b| f.width(b)).sum::<f32>() * size / 1000.0;
        let mut lines: Vec<Vec<(Font, Vec<u8>)>> = vec![Vec::new()];
        let mut x = 0.0;
        for (f, w) in words {
            let ww = width(f, w.trim_ascii_end());
            if x > 0.0 && x + ww > max {
                lines.push(Vec::new());
                x = 0.0;
            }
            x += width(f, &w);
            lines.last_mut().unwrap().push((f, w));
        }
        for (i, line) in lines.iter().enumerate() {
            self.need(line_h);
            self.y -= line_h;
            if i == 0 {
                if let Some(p) = prefix {
                    self.text(MARGIN + indent - 14.0, self.y, Font::Regular, size, &win1252(p));
                }
            }
            let mut x = MARGIN + indent;
            for (f, w) in line {
                self.text(x, self.y, *f, size, w);
                x += width(*f, w);
            }
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if !self.cur.is_empty() || self.pages.is_empty() {
            self.pages.push(std::mem::take(&mut self.cur));
        }
        let n = self.pages.len();
        let mut objs: Vec<Vec<u8>> = Vec::new();
        objs.push(b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
        let kids: String = (0..n).map(|i| format!("{} 0 R ", 7 + 2 * i)).collect();
        objs.push(format!("<< /Type /Pages /Kids [{kids}] /Count {n} >>").into_bytes());
        for base in ["Helvetica", "Helvetica-Bold", "Helvetica-Oblique", "Courier"] {
            objs.push(format!("<< /Type /Font /Subtype /Type1 /BaseFont /{base} /Encoding /WinAnsiEncoding >>").into_bytes());
        }
        for (i, content) in self.pages.iter().enumerate() {
            objs.push(
                format!(
                    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {PAGE_W} {PAGE_H}] /Resources << /Font << /F1 3 0 R /F2 4 0 R /F3 5 0 R /F4 6 0 R >> >> /Contents {} 0 R >>",
                    8 + 2 * i
                )
                .into_bytes(),
            );
            let mut stream = format!("<< /Length {} >>\nstream\n", content.len()).into_bytes();
            stream.extend_from_slice(content);
            stream.extend_from_slice(b"\nendstream");
            objs.push(stream);
        }
        let mut out = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            out.extend_from_slice(o);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for off in offsets {
            out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
        out
    }
}

pub fn write_pdf(path: &Path, md: &str) -> Result<(), String> {
    let mut w = PdfWriter::new();
    let mut counters = [0u32; 5];
    for b in parse_markdown(md) {
        if !matches!(b, Block::Numbered(..)) {
            counters = [0; 5];
        }
        match b {
            Block::Heading(n, sp) => {
                let size = match n {
                    1 => 20.0,
                    2 => 16.0,
                    _ => 13.0,
                };
                w.y -= size * 0.5;
                w.flow(&sp, size, 0.0, None, Some(Font::Bold));
                w.y -= 4.0;
            }
            Block::Para(sp) => {
                w.flow(&sp, 11.0, 0.0, None, None);
                w.y -= 6.0;
            }
            Block::Bullet(l, sp) => w.flow(&sp, 11.0, 18.0 + 18.0 * l as f32, Some("•"), None),
            Block::Numbered(l, _, sp) => {
                let l = l as usize;
                counters[l] += 1;
                for c in counters.iter_mut().skip(l + 1) {
                    *c = 0;
                }
                let label = format!("{}.", counters[l]);
                w.flow(&sp, 11.0, 22.0 + 18.0 * l as f32, Some(&label), None);
            }
            Block::Code(text) => {
                for line in text.lines() {
                    w.flow(&[Span { text: line.to_string(), code: true, ..Default::default() }], 9.5, 8.0, None, None);
                }
                w.y -= 6.0;
            }
            Block::Rule => {
                w.need(12.0);
                w.y -= 8.0;
                let y = w.y;
                w.cur.extend_from_slice(format!("0.7 G {MARGIN} {y:.2} m {:.2} {y:.2} l S 0 G\n", PAGE_W - MARGIN).as_bytes());
                w.y -= 6.0;
            }
            Block::Table(rows) => {
                for (i, r) in rows.iter().enumerate() {
                    let line: Vec<Span> = r
                        .iter()
                        .enumerate()
                        .flat_map(|(j, c)| {
                            let mut sp = spans(c);
                            if i == 0 {
                                sp.iter_mut().for_each(|s| s.bold = true);
                            }
                            if j > 0 {
                                sp.insert(0, Span { text: "  |  ".into(), ..Default::default() });
                            }
                            sp
                        })
                        .collect();
                    w.flow(&line, 10.0, 0.0, None, None);
                }
                w.y -= 6.0;
            }
        }
    }
    std::fs::write(path, w.finish()).map_err(|e| e.to_string())
}

// ---------- opening ----------

/// Opens a document the assistant made, in its usual app. Only files in
/// shared folders, and only document types (never programs or scripts).
#[tauri::command]
pub fn open_document(state: crate::AppStateRef, path: String) -> Result<(), String> {
    state.cipher()?;
    let p = std::path::PathBuf::from(&path).canonicalize().map_err(|_| "That file isn't there anymore.".to_string())?;
    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if !matches!(ext.as_str(), "docx" | "xlsx" | "pptx" | "pdf" | "md" | "csv" | "txt") {
        return Err("Only documents can be opened from here.".into());
    }
    let shared: Vec<std::path::PathBuf> = {
        let conn = state.db.lock().unwrap();
        let mut all = crate::db::folders(&conn);
        if let Ok(mut stmt) = conn.prepare("SELECT path FROM project_folders") {
            all.extend(stmt.query_map([], |r| r.get::<_, String>(0)).map(|rows| rows.filter_map(Result::ok).collect::<Vec<_>>()).unwrap_or_default());
        }
        all.into_iter().filter_map(|f| std::path::PathBuf::from(f).canonicalize().ok()).collect()
    };
    if !shared.iter().any(|root| p.starts_with(root)) {
        return Err("That file isn't in a shared folder.".into());
    }
    std::process::Command::new("explorer.exe").arg(&p).spawn().map_err(|e| e.to_string())?;
    Ok(())
}

// ---------- reading ----------

fn zip_part(path: &Path, name: &str) -> Result<String, String> {
    let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut z = zip::ZipArchive::new(f).map_err(|_| "This file isn't a valid Office document.".to_string())?;
    let mut part = z.by_name(name).map_err(|_| format!("This file has no {name}."))?;
    let mut s = String::new();
    part.read_to_string(&mut s).map_err(|e| e.to_string())?;
    Ok(s)
}

/// Text from Office XML: paragraphs on their own lines, table cells split by |.
fn office_text(xml: &str) -> String {
    use quick_xml::events::Event as X;
    let mut r = quick_xml::Reader::from_str(xml);
    let mut out = String::new();
    let mut in_text = false;
    loop {
        match r.read_event() {
            Ok(X::Start(e)) => {
                let n = e.name();
                let n = n.as_ref();
                in_text = n == b"w:t" || n == b"a:t";
            }
            Ok(X::Empty(e)) => {
                let n = e.name();
                match n.as_ref() {
                    b"w:tab" => out.push('\t'),
                    b"w:br" | b"a:br" => out.push('\n'),
                    b"w:pStyle" => {
                        if let Some(v) = e.try_get_attribute("w:val").ok().flatten() {
                            let v = String::from_utf8_lossy(&v.value).to_string();
                            if let Some(level) = v.strip_prefix("Heading").and_then(|l| l.parse::<usize>().ok()) {
                                out.push_str(&"#".repeat(level.min(6)));
                                out.push(' ');
                            } else if v == "Title" {
                                out.push_str("# ");
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(X::Text(t)) if in_text => out.push_str(&t.decode().map(|s| s.to_string()).unwrap_or_default()),
            Ok(X::GeneralRef(g)) if in_text => out.push_str(match g.decode().unwrap_or_default().as_ref() {
                "amp" => "&",
                "lt" => "<",
                "gt" => ">",
                "quot" => "\"",
                "apos" => "'",
                _ => "",
            }),
            Ok(X::End(e)) => {
                match e.name().as_ref() {
                    b"w:t" | b"a:t" => in_text = false,
                    b"w:p" | b"a:p" => out.push('\n'),
                    b"w:tc" => {
                        if out.ends_with('\n') {
                            out.pop();
                        }
                        out.push_str(" | ");
                    }
                    b"w:tr" => out.push('\n'),
                    _ => {}
                }
            }
            Ok(X::Eof) | Err(_) => break,
            _ => {}
        }
    }
    out
}

fn sheets_text(path: &Path) -> Result<String, String> {
    use calamine::{open_workbook_auto, Data, Reader};
    let mut wb = open_workbook_auto(path).map_err(|e| format!("Couldn't open the spreadsheet: {e}"))?;
    let mut out = String::new();
    for name in wb.sheet_names() {
        let Ok(range) = wb.worksheet_range(&name) else { continue };
        out.push_str(&format!("## Sheet: {name}\n"));
        for row in range.rows().take(300) {
            let cells: Vec<String> = row
                .iter()
                .take(40)
                .map(|c| match c {
                    Data::Empty => String::new(),
                    Data::Float(f) if f.fract() == 0.0 && f.abs() < 1e15 => format!("{}", *f as i64),
                    other => other.to_string(),
                })
                .collect();
            out.push_str(&format!("| {} |\n", cells.join(" | ")));
        }
        if range.height() > 300 {
            out.push_str(&format!("[{} more rows]\n", range.height() - 300));
        }
        if let Ok(f) = wb.worksheet_formula(&name) {
            let (r0, c0) = f.start().unwrap_or((0, 0));
            let formulas: Vec<String> = f
                .cells()
                .filter(|(_, _, v)| !v.is_empty())
                .take(100)
                .map(|(r, c, v)| {
                    let col = (c as u32 + c0) as u16;
                    let a1 = absolute("x", &format!("{}{}", col_name(col), r as u32 + r0 + 1)).unwrap_or_default();
                    let short = a1.rsplit('!').next().unwrap_or("").replace('$', "");
                    format!("{} = {v}", short.split(':').next().unwrap_or(""))
                })
                .collect();
            if !formulas.is_empty() {
                out.push_str(&format!("Formulas: {}\n", formulas.join("; ")));
            }
        }
        out.push('\n');
    }
    Ok(out)
}

fn col_name(c: u16) -> String {
    let mut n = c as u32 + 1;
    let mut s = String::new();
    while n > 0 {
        let m = (n - 1) % 26;
        s.insert(0, (b'A' + m as u8) as char);
        n = (n - 1) / 26;
    }
    s
}

/// A document's text, for reading or summarizing.
pub fn read_text(path: &Path) -> Result<String, String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let text = match ext.as_str() {
        "docx" => office_text(&zip_part(path, "word/document.xml")?),
        "pptx" => {
            let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
            let z = zip::ZipArchive::new(f).map_err(|_| "This file isn't a valid PowerPoint file.".to_string())?;
            let mut names: Vec<(u32, String)> = z
                .file_names()
                .filter_map(|n| n.strip_prefix("ppt/slides/slide").and_then(|r| r.strip_suffix(".xml")).and_then(|k| k.parse().ok()).map(|k| (k, n.to_string())))
                .collect();
            names.sort();
            let mut out = String::new();
            for (k, n) in names {
                out.push_str(&format!("## Slide {k}\n{}\n", office_text(&zip_part(path, &n)?).trim()));
            }
            out
        }
        "xlsx" | "xlsm" | "xls" | "xlsb" | "ods" => sheets_text(path)?,
        "pdf" => {
            let p = path.to_path_buf();
            std::panic::catch_unwind(move || pdf_extract::extract_text(&p))
                .map_err(|_| "Couldn't read this PDF.".to_string())?
                .map_err(|e| format!("Couldn't read this PDF: {e}"))?
        }
        "txt" | "md" | "markdown" | "csv" | "tsv" | "json" | "log" | "html" | "htm" | "xml" | "rtf" => {
            String::from_utf8_lossy(&std::fs::read(path).map_err(|e| e.to_string())?).to_string()
        }
        other => return Err(format!("I can't read .{other} files as documents.")),
    };
    let text = text.replace("\r\n", "\n");
    let cut: String = text.chars().take(MAX_READ).collect();
    Ok(if text.chars().count() > MAX_READ { format!("{cut}\n[The document goes on.]") } else { cut })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MD: &str = "# Quarterly report\n\nRevenue grew **12%** this quarter, led by *new* customers.\n\n## Highlights\n\n- Launched the `v2` API\n- Hired 3 people\n  - two engineers\n\n1. Plan\n2. Build\n\n| Region | Sales |\n|---|---|\n| West | 120 |\n| East | 95 |\n\n```\nlet x = 1;\n```\n\n---\n\nSee [the site](https://example.com).\n";

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("sulcusai-docs-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d.join(name)
    }

    /// Writes one of each into SULCUSAI_SAMPLES for checking with other readers.
    #[test]
    #[ignore]
    fn write_samples() {
        let dir = std::path::PathBuf::from(std::env::var("SULCUSAI_SAMPLES").unwrap());
        std::fs::create_dir_all(&dir).unwrap();
        write_docx(&dir.join("report.docx"), MD).unwrap();
        write_pdf(&dir.join("report.pdf"), MD).unwrap();
        let sheet: Sheet = serde_json::from_value(serde_json::json!({
            "name": "Budget", "rows": [["Month", "Spend", "Plan"], ["Jan", 120, 100], ["Feb", 95.5, 110], ["Total", "=SUM(B2:B3)", "=SUM(C2:C3)"]],
            "charts": [{ "type": "column", "title": "Spend vs plan", "categories": "A2:A3", "values": ["B2:B3", "C2:C3"], "names": ["Spend", "Plan"] }]
        })).unwrap();
        write_xlsx(&dir.join("budget.xlsx"), &[sheet]).unwrap();
        write_pptx(&dir.join("deck.pptx"), &[
            Slide { title: "Q4 plan".into(), subtitle: "Team offsite".into(), bullets: vec![] },
            Slide { title: "Goals".into(), subtitle: String::new(), bullets: vec!["Grow revenue".into(), "  in the West".into(), "Ship v2 & docs".into()] },
        ]).unwrap();
    }

    #[test]
    fn markdown_is_read() {
        let b = parse_markdown(MD);
        assert!(matches!(&b[0], Block::Heading(1, s) if plain(s) == "Quarterly report"));
        let Block::Para(p) = &b[1] else { panic!("{:?}", b[1]) };
        assert!(p.iter().any(|s| s.bold && s.text == "12%") && p.iter().any(|s| s.italic && s.text == "new"));
        assert!(b.iter().any(|x| matches!(x, Block::Bullet(1, s) if plain(s) == "two engineers")));
        assert!(b.iter().any(|x| matches!(x, Block::Numbered(0, 2, s) if plain(s) == "Build")));
        assert!(b.iter().any(|x| matches!(x, Block::Table(rows) if rows.len() == 3 && rows[1] == vec!["West", "120"])));
        assert!(b.iter().any(|x| matches!(x, Block::Code(c) if c == "let x = 1;")));
        assert!(b.iter().any(|x| matches!(x, Block::Para(s) if plain(s) == "See the site (https://example.com).")));
        assert_eq!(spans("snake_case_name").len(), 1, "underscores inside words aren't italics");
    }

    #[test]
    fn word_documents_round_trip() {
        let p = tmp("report.docx");
        write_docx(&p, MD).unwrap();
        let text = read_text(&p).unwrap();
        assert!(text.contains("# Quarterly report") && text.contains("## Highlights"), "{text}");
        assert!(text.contains("Revenue grew 12% this quarter") && text.contains("Launched the v2 API"));
        assert!(text.contains("West | 120"), "{text}");
    }

    #[test]
    fn spreadsheets_keep_formulas_and_charts() {
        let p = tmp("budget.xlsx");
        let sheet: Sheet = serde_json::from_value(serde_json::json!({
            "name": "Budget",
            "rows": [["Month", "Spend"], ["Jan", 120], ["Feb", "95.5"], ["Total", "=SUM(B2:B3)"]],
            "charts": [{ "type": "column", "title": "Spend", "categories": "A2:A3", "values": ["B2:B3"], "names": ["Spend"] }]
        }))
        .unwrap();
        write_xlsx(&p, &[sheet]).unwrap();
        let text = read_text(&p).unwrap();
        assert!(text.contains("## Sheet: Budget") && text.contains("| Jan | 120 |") && text.contains("| Feb | 95.5 |"), "{text}");
        assert!(text.contains("B4 = SUM(B2:B3)"), "{text}");
        assert!(text.contains("| Total | 215.5 |"), "the total is filled in: {text}");
        let z = zip::ZipArchive::new(std::fs::File::open(&p).unwrap()).unwrap();
        assert!(z.file_names().any(|n| n.starts_with("xl/charts/chart")), "has a chart");
        assert_eq!(cell_ref("B12"), Some((11, 1)));
        assert_eq!(cell_ref("AA1"), Some((0, 26)));
        assert_eq!(absolute("My Sheet", "B2:B6").unwrap(), "'My Sheet'!$B$2:$B$6");
    }

    #[test]
    fn slides_are_built_on_the_template() {
        let p = tmp("deck.pptx");
        let slides = vec![
            Slide { title: "Q4 plan".into(), subtitle: "Team offsite".into(), bullets: vec![] },
            Slide { title: "Goals".into(), subtitle: String::new(), bullets: vec!["Grow revenue".into(), "  in the West".into(), "Ship v2 & docs".into()] },
        ];
        write_pptx(&p, &slides).unwrap();
        let text = read_text(&p).unwrap();
        assert!(text.contains("## Slide 1\nQ4 plan\nTeam offsite") && text.contains("## Slide 2\nGoals\nGrow revenue\nin the West\nShip v2 & docs"), "{text}");
    }

    #[test]
    fn small_model_shapes_are_understood() {
        let tsv = sheets_from_text("Item\tCost\nRent\t1200\nFood\t400\nTotal\t=SUM(B2:B3)");
        assert_eq!(tsv[0].rows.len(), 4);
        assert_eq!(tsv[0].rows[1][1], serde_json::json!(1200.0));
        assert_eq!(tsv[0].rows[3][1], serde_json::json!("=SUM(B2:B3)"));
        let md = sheets_from_text("| Item | Cost |\n|---|---|\n| Rent | 1,200 |");
        assert_eq!(md[0].rows[1][1], serde_json::json!(1200.0));
        let csv = sheets_from_text("Name,Score\nAna,9");
        assert_eq!(csv[0].rows[1], serde_json::json!(["Ana", 9.0]));
        let records = sheets_from_text(r#"[{"item": "Rent", "cost": 1200}, {"item": "Food", "cost": 400}]"#);
        assert_eq!(records[0].rows, vec![serde_json::json!(["item", "cost"]), serde_json::json!(["Rent", 1200]), serde_json::json!(["Food", 400])]);
        assert!(sheets_from_text(r#"[{"name": "Item", "rows": [{"cells": ["Rent"]}]}]"#).is_empty(), "nested JSON is refused, not guessed");
        assert_eq!(unknown_names("=SUM(Rent, Food, Travel)"), vec!["Rent", "Food", "Travel"]);
        assert!(unknown_names("=SUM(B2:B4)*2+IF(C1>0,\"yes\",'Other sheet'!A1)").is_empty());
        assert!(unknown_names("=SUM(A:A)+Sheet2!B3+TRUE").is_empty());
        let bracketed = sheets_from_text("[[Item, Cost]\n[Rent, 1200]\n[Total, =SUM(B2:B2)]]");
        assert_eq!(bracketed[0].rows[2], serde_json::json!(["Total", "=SUM(B2:B2)"]));
        assert_eq!(row_cells(&serde_json::json!({ "cells": ["a", 1] })), vec![serde_json::json!("a"), serde_json::json!(1)]);
        let deck = slides_from_markdown("# Budget 2027\nTeam offsite\n\n## Costs\n- Rent: 1200\n- Food\n  - groceries\n\n## Next steps\n- Cut travel\n- Review in March");
        assert_eq!(deck.len(), 3);
        assert_eq!((deck[0].title.as_str(), deck[0].subtitle.as_str(), deck[0].bullets.len()), ("Budget 2027", "Team offsite", 0));
        assert_eq!(deck[1].bullets, vec!["Rent: 1200", "Food", "  groceries"]);
        assert_eq!(deck[2].bullets, vec!["Cut travel", "Review in March"]);
    }

    #[test]
    fn pdfs_are_written_and_read() {
        let p = tmp("memo.pdf");
        let long = "word ".repeat(2000);
        write_pdf(&p, &format!("{MD}\n\n{long}\n\nCafé – “quoted” (done)")).unwrap();
        let bytes = std::fs::read(&p).unwrap();
        assert!(bytes.starts_with(b"%PDF-1.4") && bytes.ends_with(b"%%EOF\n"));
        let text = read_text(&p).unwrap();
        assert!(text.contains("Quarterly report") && text.contains("Revenue grew") && text.contains("(done)"), "{}", &text[..text.len().min(500)]);
        assert!(text.contains("Café"), "Windows-1252 accents survive");
        let pages = String::from_utf8_lossy(&bytes).matches("/Type /Page ").count();
        assert!(pages >= 2, "long text flows onto more pages ({pages})");
    }
}

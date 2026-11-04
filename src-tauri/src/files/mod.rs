// SPDX-License-Identifier: AGPL-3.0-only
//! Files and documents: the Find index, the encrypted records store (home
//! inventory, forms, scans), PDF citations and comparison, and spreadsheet
//! analysis with an approved plan.

pub mod find;
pub mod inventory;
pub mod pdf;
pub mod pdf_forms;
pub mod records;
pub mod scan;
pub mod sheets;

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde_json::Value;

use crate::AppStateRef;

static FIND: OnceLock<Mutex<find::FindIndex>> = OnceLock::new();
static RECORDS: OnceLock<Mutex<records::Records>> = OnceLock::new();

fn with_find<T>(data: &Path, f: impl FnOnce(&find::FindIndex) -> T) -> Result<T, String> {
    if FIND.get().is_none() {
        let idx = find::FindIndex::open(&data.join("find.db")).map_err(|e| e.to_string())?;
        let _ = FIND.set(Mutex::new(idx));
    }
    Ok(f(&FIND.get().unwrap().lock().unwrap()))
}

fn with_records<T>(state: &crate::AppState, f: impl FnOnce(&records::Records, &crate::crypto::Cipher) -> Result<T, String>) -> Result<T, String> {
    let cipher = state.vault.lock().unwrap().cipher().ok_or("Unlock SulcusAI first.")?;
    if RECORDS.get().is_none() {
        let r = records::Records::open(&state.paths.data.join("records.db")).map_err(|e| e.to_string())?;
        let _ = RECORDS.set(Mutex::new(r));
    }
    f(&RECORDS.get().unwrap().lock().unwrap(), &cipher)
}

/// The Media index's transcripts, searchable in Find too.
pub fn media_transcript(data: &Path, path: &str, text: &str) {
    let _ = with_find(data, |i| i.set_transcript(path, text));
}

pub fn media_transcript_removed(data: &Path, path: &str) {
    let _ = with_find(data, |i| i.clear_transcript(path));
}

/// Search words from a launcher or chat (other parts of the app call this).
pub fn search(data: &Path, q: &str, limit: usize) -> Vec<find::Hit> {
    search_kind(data, q, None, limit)
}

pub fn search_kind(data: &Path, q: &str, kind: Option<&str>, limit: usize) -> Vec<find::Hit> {
    with_find(data, |i| i.search(q, kind, limit)).unwrap_or_default()
}

#[tauri::command(async)]
pub fn find_folders(state: AppStateRef<'_>) -> Result<Vec<String>, String> {
    with_find(&state.paths.data, |i| i.roots())
}

#[tauri::command(async)]
pub fn find_add_folder(state: AppStateRef<'_>, path: String) -> Result<find::IndexReport, String> {
    with_find(&state.paths.data, |i| i.add_root(Path::new(&path)))?
}

#[tauri::command(async)]
pub fn find_remove_folder(state: AppStateRef<'_>, path: String) -> Result<(), String> {
    with_find(&state.paths.data, |i| i.remove_root(&path))?
}

#[tauri::command(async)]
pub fn find_refresh(state: AppStateRef<'_>) -> Result<find::IndexReport, String> {
    with_find(&state.paths.data, |i| i.refresh())?
}

#[tauri::command(async)]
pub fn find_search(state: AppStateRef<'_>, query: String, kind: Option<String>) -> Result<Vec<find::Hit>, String> {
    with_find(&state.paths.data, |i| i.search(&query, kind.as_deref(), 100))
}

#[tauri::command(async)]
pub fn find_duplicates(state: AppStateRef<'_>) -> Result<Vec<find::DupeGroup>, String> {
    with_find(&state.paths.data, |i| i.duplicates(1024))
}

#[tauri::command(async)]
pub fn find_set_caption(state: AppStateRef<'_>, path: String, caption: String) -> Result<bool, String> {
    with_find(&state.paths.data, |i| i.set_caption(&path, &caption))?
}

#[tauri::command(async)]
pub fn tidy_plan(path: String) -> Result<Vec<find::Move>, String> {
    find::tidy_plan(Path::new(&path))
}

#[tauri::command(async)]
pub fn tidy_apply(moves: Vec<find::Move>) -> (usize, Vec<String>) {
    find::apply_tidy(&moves)
}

#[tauri::command(async)]
pub fn pdf_passages(path: String, question: String) -> Result<Vec<pdf::Passage>, String> {
    Ok(pdf::passages(&crate::docs::pdf_pages(&PathBuf::from(path))?, &question, 8))
}

#[tauri::command(async)]
pub fn pdf_compare(old: String, new: String) -> Result<Vec<pdf::PageChange>, String> {
    Ok(pdf::compare(&crate::docs::pdf_pages(&PathBuf::from(old))?, &crate::docs::pdf_pages(&PathBuf::from(new))?))
}

#[tauri::command(async)]
pub fn pdf_scanned_pages(path: String) -> Result<Vec<usize>, String> {
    Ok(pdf::scanned_pages(&crate::docs::pdf_pages(&PathBuf::from(path))?))
}

/// The first sheet (or the named one) as headers and text rows.
fn read_sheet(path: &Path, sheet: Option<&str>) -> Result<(Vec<String>, Vec<Vec<String>>), String> {
    use calamine::{open_workbook_auto, Reader};
    let mut wb = open_workbook_auto(path).map_err(|e| format!("Couldn't open the spreadsheet: {e}"))?;
    let name = match sheet {
        Some(s) => s.to_string(),
        None => wb.sheet_names().first().cloned().ok_or("The spreadsheet has no sheets.")?,
    };
    let range = wb.worksheet_range(&name).map_err(|e| e.to_string())?;
    let mut rows = range.rows().map(|r| r.iter().map(|c| c.to_string()).collect::<Vec<String>>());
    let headers = rows.next().ok_or("The sheet is empty.")?;
    Ok((headers, rows.collect()))
}

#[tauri::command(async)]
pub fn sheet_plan(path: String, sheet: Option<String>, steps: Vec<sheets::Step>) -> Result<Vec<String>, String> {
    let (h, _) = read_sheet(Path::new(&path), sheet.as_deref())?;
    sheets::check_plan(&h, &steps)
}

/// Runs an approved plan and saves the results next to the original as a
/// new file, so the original is never changed.
#[tauri::command(async)]
pub fn sheet_run(path: String, sheet: Option<String>, steps: Vec<sheets::Step>) -> Result<String, String> {
    let src = Path::new(&path);
    let (h, rows) = read_sheet(src, sheet.as_deref())?;
    let results = sheets::run(&h, &rows, &steps)?;
    let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("sheet");
    let mut out = src.with_file_name(format!("{stem} analysis.xlsx"));
    let mut n = 2;
    while out.exists() {
        out = src.with_file_name(format!("{stem} analysis ({n}).xlsx"));
        n += 1;
    }
    sheets::write_results(&out, &results)?;
    Ok(out.display().to_string())
}

/// A redacted copy beside the original ("<name> redacted.pdf"), rebuilt from
/// the text so the hidden words are not in the file at all.
#[tauri::command(async)]
pub fn pdf_redact(path: String, terms: Vec<String>, patterns: bool) -> Result<(String, usize), String> {
    let src = PathBuf::from(&path);
    let (pages, n) = pdf::redact(&crate::docs::pdf_pages(&src)?, &terms, patterns);
    let md: String = pages.iter().enumerate().map(|(i, p)| format!("## Page {}\n\n{}\n\n", i + 1, p.trim())).collect();
    let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("document");
    let out = src.with_file_name(format!("{stem} redacted.pdf"));
    crate::docs::write_pdf(&out, &md)?;
    Ok((out.display().to_string(), n))
}

#[tauri::command(async)]
pub fn records_list(state: AppStateRef<'_>, kind: String, query: Option<String>) -> Result<Vec<records::Record>, String> {
    with_records(&state, |r, c| match query.as_deref() {
        Some(q) if !q.trim().is_empty() => r.search(c, &kind, q),
        _ => r.list(c, &kind),
    })
}

#[tauri::command(async)]
pub fn records_delete(state: AppStateRef<'_>, id: String) -> Result<bool, String> {
    with_records(&state, |r, _| r.delete(&id))
}

#[tauri::command(async)]
pub fn inventory_save(state: AppStateRef<'_>, id: Option<String>, item: Value) -> Result<records::Record, String> {
    let item = inventory::clean_item(&item)?;
    with_records(&state, |r, c| r.put(c, "inventory", id.as_deref(), &item))
}

#[tauri::command(async)]
pub fn inventory_totals(state: AppStateRef<'_>) -> Result<Vec<inventory::RoomTotal>, String> {
    with_records(&state, |r, c| Ok(inventory::totals(&r.list(c, "inventory")?)))
}

#[tauri::command(async)]
pub fn inventory_export(state: AppStateRef<'_>, path: String) -> Result<(), String> {
    let csv = with_records(&state, |r, c| Ok(inventory::to_csv(&r.list(c, "inventory")?)))?;
    std::fs::write(path, csv).map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub fn form_save(state: AppStateRef<'_>, id: Option<String>, template: inventory::FormTemplate, values: serde_json::Map<String, Value>) -> Result<records::Record, String> {
    let filled = inventory::check_form(&template, &values).map_err(|e| e.join(" "))?;
    with_records(&state, |r, c| r.put(c, "form", id.as_deref(), &filled))
}

#[tauri::command(async)]
pub fn pdf_form_fields(path: String) -> Result<Vec<pdf_forms::FormField>, String> {
    pdf_forms::fields(Path::new(&path))
}

/// Fills a PDF form into "<name> filled.pdf" beside it.
#[tauri::command(async)]
pub fn pdf_form_fill(path: String, values: std::collections::HashMap<String, String>) -> Result<String, String> {
    let src = PathBuf::from(&path);
    let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("form");
    let out = src.with_file_name(format!("{stem} filled.pdf"));
    pdf_forms::fill(&src, &values, &out)?;
    Ok(out.display().to_string())
}

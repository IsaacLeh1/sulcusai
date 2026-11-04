// SPDX-License-Identifier: AGPL-3.0-only
//! PDF reading with page citations: the passages an answer is built from,
//! each with the page it came from, and a page-by-page comparison of two
//! versions of a document.

use serde::Serialize;

#[derive(Serialize, Debug, PartialEq)]
pub struct Passage {
    /// 1-based, as printed in a viewer.
    pub page: usize,
    pub text: String,
    pub score: usize,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct PageChange {
    pub page: usize,
    pub removed: Vec<String>,
    pub added: Vec<String>,
}

fn words(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() > 2).map(str::to_lowercase).collect()
}

/// The passages (paragraph-sized pieces) that share the most words with the
/// question, best first, each with its page. A chat answer quotes these and
/// names the pages, so the person can check them.
pub fn passages(pages: &[String], question: &str, limit: usize) -> Vec<Passage> {
    let q: std::collections::HashSet<String> = words(question).into_iter().collect();
    if q.is_empty() {
        return vec![];
    }
    let mut out = vec![];
    for (i, page) in pages.iter().enumerate() {
        for para in page.split("\n\n").map(str::trim).filter(|p| !p.is_empty()) {
            let w = words(para);
            let hits: std::collections::HashSet<&String> = w.iter().filter(|x| q.contains(*x)).collect();
            if !hits.is_empty() {
                out.push(Passage { page: i + 1, text: para.chars().take(1200).collect(), score: hits.len() });
            }
        }
    }
    out.sort_by(|a, b| b.score.cmp(&a.score).then(a.page.cmp(&b.page)));
    out.truncate(limit);
    out
}

/// The passages as a block for the model, each marked with its page.
pub fn cited_context(found: &[Passage]) -> String {
    found.iter().map(|p| format!("[page {}] {}", p.page, p.text)).collect::<Vec<_>>().join("\n\n")
}

/// Lines removed and added on each page, comparing page n with page n.
pub fn compare(old: &[String], new: &[String]) -> Vec<PageChange> {
    let n = old.len().max(new.len());
    let mut out = vec![];
    for i in 0..n {
        let a = old.get(i).map(String::as_str).unwrap_or("");
        let b = new.get(i).map(String::as_str).unwrap_or("");
        let diff = similar::TextDiff::from_lines(a, b);
        let (mut removed, mut added) = (vec![], vec![]);
        for ch in diff.iter_all_changes() {
            let line = ch.value().trim().to_string();
            if line.is_empty() {
                continue;
            }
            match ch.tag() {
                similar::ChangeTag::Delete => removed.push(line),
                similar::ChangeTag::Insert => added.push(line),
                similar::ChangeTag::Equal => {}
            }
        }
        if !removed.is_empty() || !added.is_empty() {
            out.push(PageChange { page: i + 1, removed, added });
        }
    }
    out
}

/// Pages with almost no text are most likely scanned pictures; these are
/// the ones a scan reader has to make searchable.
pub fn scanned_pages(pages: &[String]) -> Vec<usize> {
    pages.iter().enumerate().filter(|(_, p)| p.chars().filter(|c| c.is_alphanumeric()).count() < 20).map(|(i, _)| i + 1).collect()
}

/// Replaces each term (any case) and, when asked, email addresses and long
/// digit runs (account, card and phone numbers) with blocks. The redacted
/// copy is written from this text, so the hidden words are gone from the
/// file, not just covered.
pub fn redact(pages: &[String], terms: &[String], patterns: bool) -> (Vec<String>, usize) {
    let mut res: Vec<regex::Regex> = terms
        .iter()
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .filter_map(|t| regex::RegexBuilder::new(&regex::escape(t)).case_insensitive(true).build().ok())
        .collect();
    if patterns {
        res.push(regex::Regex::new(r"[\w.+-]+@[\w-]+(\.[\w-]+)+").unwrap());
        res.push(regex::Regex::new(r"\+?\d[\d\s().-]{6,}\d").unwrap());
    }
    let mut count = 0;
    let mut out = vec![];
    for p in pages {
        let mut t = p.clone();
        for re in &res {
            count += re.find_iter(&t).count();
            t = re.replace_all(&t, |c: &regex::Captures| "\u{2588}".repeat(c[0].chars().count().min(12))).into_owned();
        }
        out.push(t);
    }
    (out, count)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pages(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn passages_carry_their_page() {
        let p = pages(&["Intro text.\n\nThe warranty lasts two years.", "Returns within thirty days.\n\nWarranty claims need the receipt."]);
        let found = passages(&p, "How long is the warranty?", 5);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].page, 1);
        assert!(cited_context(&found).contains("[page 2] Warranty claims"));
        assert!(passages(&p, "?", 5).is_empty());
    }

    #[test]
    fn compare_names_the_page() {
        let old = pages(&["a\nrent is 900\n", "same\n"]);
        let new = pages(&["a\nrent is 950\n", "same\n", "new page\n"]);
        let c = compare(&old, &new);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].page, 1);
        assert_eq!(c[0].removed, vec!["rent is 900"]);
        assert_eq!(c[0].added, vec!["rent is 950"]);
        assert_eq!(c[1].page, 3);
    }

    #[test]
    fn redaction_removes_the_words() {
        let (out, n) = redact(&pages(&["Call Sam at +1 (801) 555-0199 or sam@x.org.", "SAM paid"]), &["sam".into()], true);
        assert!(n >= 4);
        assert!(!out.join(" ").to_lowercase().contains("sam"));
        assert!(!out[0].contains("555"));
    }

    #[test]
    fn empty_pages_look_scanned() {
        assert_eq!(scanned_pages(&pages(&["plenty of words on this page here", " \n ", "x"])), vec![2, 3]);
    }
}

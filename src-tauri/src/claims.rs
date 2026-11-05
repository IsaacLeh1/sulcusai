// SPDX-License-Identifier: AGPL-3.0-only
//! A check after the answer: when it was built on tool results that were
//! partial, does it say so, and does it lean on pages that weren't read?
//! Plain text matching, no model call, so it costs nothing on a clean read.

/// One partial result in the turn: the tool, and its text (which carries
/// the coverage lines naming the pages that weren't read).
pub struct Partial<'a> {
    pub tool: &'a str,
    pub text: &'a str,
}

/// Words that show the answer owns up to a gap.
fn admits_gap(answer: &str) -> bool {
    let a = answer.to_lowercase();
    [
        "couldn't read", "could not read", "wasn't read", "weren't read", "not read", "couldn't see", "could not see", "missing", "partial",
        "unable to read", "didn't read", "cut off", "not included", "wasn't able", "unreadable", "not shown", "couldn't get",
    ]
    .iter()
    .any(|w| a.contains(w))
}

/// Page numbers the coverage lines flag ("- page 4: ...").
fn flagged_pages(text: &str) -> Vec<u32> {
    let mut out: Vec<u32> = text
        .lines()
        .filter_map(|l| l.trim().strip_prefix("- page "))
        .filter_map(|r| r.split(':').next()?.trim().parse().ok())
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Pages the answer cites ("page 4", "p. 4", "pages 3 and 4" counts 3).
fn cited_pages(answer: &str) -> Vec<u32> {
    let a = answer.to_lowercase();
    let mut out = Vec::new();
    for key in ["page ", "pages ", "p. ", "pp. "] {
        for (i, _) in a.match_indices(key) {
            let n: String = a[i + key.len()..].chars().take_while(char::is_ascii_digit).collect();
            if let Ok(n) = n.parse() {
                out.push(n);
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// What the answer may get wrong because of the gaps, in plain words; empty
/// when there's nothing to flag.
pub fn check(answer: &str, partials: &[Partial]) -> Vec<String> {
    if partials.is_empty() || answer.trim().is_empty() {
        return Vec::new();
    }
    let mut flags = Vec::new();
    let flagged: Vec<u32> = partials.iter().flat_map(|p| flagged_pages(p.text)).collect();
    let leaned: Vec<u32> = cited_pages(answer).into_iter().filter(|p| flagged.contains(p)).collect();
    if !leaned.is_empty() {
        let list: Vec<String> = leaned.iter().map(u32::to_string).collect();
        flags.push(format!("It cites page(s) {} that the reading couldn't fully turn into text (equations, figures or scans).", list.join(", ")));
    }
    if !admits_gap(answer) {
        let mut tools: Vec<&str> = partials.iter().map(|p| p.tool).collect();
        tools.dedup();
        flags.push(format!("It rests on partial results ({}) and doesn't say what was missing.", tools.join(", ")));
    }
    flags
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "text\n[Coverage: PARTIAL. Not everything:\n- page 4: 3 picture(s)\n- page 7: equations set in math fonts]";

    #[test]
    fn a_clean_turn_has_nothing_to_flag() {
        assert!(check("The answer is 4.", &[]).is_empty());
    }

    #[test]
    fn an_answer_on_unread_pages_is_flagged() {
        let p = [Partial { tool: "read_document", text: DOC }];
        let f = check("As shown on page 4, the mean is 3.2.", &p);
        assert_eq!(f.len(), 2, "{f:?}");
        assert!(f[0].contains("page(s) 4") && f[1].contains("read_document"));
    }

    #[test]
    fn saying_what_was_missing_is_enough() {
        let p = [Partial { tool: "read_document", text: DOC }];
        assert!(check("I couldn't read the equations on page 7, so this leaves them out.", &p).iter().all(|f| !f.contains("doesn't say")));
        assert_eq!(flagged_pages(DOC), vec![4, 7]);
        assert_eq!(cited_pages("see p. 3 and pages 7"), vec![3, 7]);
    }
}

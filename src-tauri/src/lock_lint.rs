// SPDX-License-Identifier: AGPL-3.0-only
//! A check on this crate's own source for one way to freeze the app.
//!
//! A `x.lock().unwrap()` inside an expression makes a guard that lives until
//! the end of the whole statement (a struct literal, a call's arguments, a
//! `match`). A second lock of the same mutex in that statement waits on the
//! first forever: 0.8.0 froze the window that way on the Studio page. The
//! same goes for the database lock next to a helper that locks it itself.

use std::path::Path;

/// Splits Rust source into statements: at `;`, and at braces that open or
/// close a block. A brace after a type name (`Name {`) is a struct literal,
/// which stays part of its statement. Comments and string contents are dropped.
fn statements(src: &str) -> Vec<(usize, String)> {
    let chars: Vec<char> = src.chars().collect();
    let (mut out, mut buf, mut line, mut start) = (Vec::new(), String::new(), 1, 1);
    let mut i = 0;
    let flush = |out: &mut Vec<(usize, String)>, buf: &mut String, start: usize| {
        if !buf.trim().is_empty() {
            out.push((start, std::mem::take(buf)));
        }
        buf.clear();
    };
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c == '\n' {
            line += 1;
        }
        // Comments.
        if c == '/' && next == Some('/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && next == Some('*') {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                line += (chars[i] == '\n') as usize;
                i += 1;
            }
            i += 2;
            continue;
        }
        // Raw strings: r"…", r#"…"#.
        if c == 'r' && (next == Some('"') || next == Some('#')) && (i == 0 || !chars[i - 1].is_alphanumeric()) {
            let mut j = i + 1;
            let mut hashes = 0;
            while j < chars.len() && chars[j] == '#' {
                hashes += 1;
                j += 1;
            }
            if j < chars.len() && chars[j] == '"' {
                j += 1;
                loop {
                    if j >= chars.len() {
                        break;
                    }
                    if chars[j] == '"' && chars[j + 1..].iter().take(hashes).filter(|&&h| h == '#').count() == hashes {
                        j += 1 + hashes;
                        break;
                    }
                    line += (chars[j] == '\n') as usize;
                    j += 1;
                }
                buf.push_str("\"\"");
                i = j;
                continue;
            }
        }
        // Strings and the character literals that would confuse the split.
        if c == '"' {
            let mut j = i + 1;
            while j < chars.len() && chars[j] != '"' {
                if chars[j] == '\\' {
                    j += 1;
                }
                line += (chars.get(j) == Some(&'\n')) as usize;
                j += 1;
            }
            buf.push_str("\"\"");
            i = j + 1;
            continue;
        }
        if c == '\'' && chars.get(i + 2) == Some(&'\'') {
            buf.push_str("' '");
            i += 3;
            continue;
        }
        match c {
            ';' => {
                flush(&mut out, &mut buf, start);
                start = line;
            }
            '{' => {
                let head = buf.trim_end();
                let literal = head.rsplit(|ch: char| !(ch.is_alphanumeric() || ch == '_' || ch == ':')).next().is_some_and(|w| {
                    w.rsplit("::").next().is_some_and(|n| n.chars().next().is_some_and(char::is_uppercase))
                });
                if literal {
                    buf.push(c);
                } else {
                    flush(&mut out, &mut buf, start);
                    start = line;
                }
            }
            '}' => {
                buf.push(c);
                flush(&mut out, &mut buf, start);
                start = line;
            }
            _ => buf.push(c),
        }
        i += 1;
    }
    flush(&mut out, &mut buf, start);
    out
}

/// What in one statement would lock the same mutex twice.
fn double_locks(statement: &str) -> Option<String> {
    let mut seen: Vec<&str> = Vec::new();
    let mut rest = statement;
    while let Some(at) = rest.find(".lock().unwrap()") {
        let before = &rest[..at];
        let name = before.rsplit(|ch: char| !(ch.is_alphanumeric() || ch == '_')).next().unwrap_or("");
        if seen.contains(&name) {
            return Some(format!("`{name}` is locked twice"));
        }
        seen.push(name);
        rest = &rest[at + 1..];
    }
    let helper = ["state.log(", "self.log(", "state.settings()", "self.settings()", "keep_working(&"];
    if seen.contains(&"db") {
        if let Some(h) = helper.iter().find(|h| statement.contains(**h)) {
            return Some(format!("the database is locked next to `{h}`, which locks it too"));
        }
    }
    None
}

fn check_dir(dir: &Path, problems: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            check_dir(&path, problems);
        } else if path.extension().is_some_and(|e| e == "rs") && path.file_name().is_some_and(|n| n != "lock_lint.rs") {
            let src = std::fs::read_to_string(&path).unwrap();
            for (line, st) in statements(&src) {
                if let Some(why) = double_locks(&st) {
                    problems.push(format!("{}:{line}: {why}", path.display()));
                }
            }
        }
    }
}

#[test]
fn no_statement_locks_a_mutex_twice() {
    let mut problems = Vec::new();
    check_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut problems);
    assert!(problems.is_empty(), "these would freeze the app:\n{}", problems.join("\n"));
}

#[test]
fn the_check_finds_the_studio_freeze() {
    let old = r#"
        fn view(state: &AppState) -> MediaView {
            let c = state.cipher();
            MediaView {
                cloud: crate::cloud::media_choices(&state.db.lock().unwrap()),
                gallery: c.map(|_| store::count(&state.db.lock().unwrap())).unwrap_or(0),
            }
        }
    "#;
    let found: Vec<_> = statements(old).into_iter().filter_map(|(_, s)| double_locks(&s)).collect();
    assert_eq!(found, vec!["`db` is locked twice".to_string()]);
    // Separate statements are fine, and so are strings that mention a lock.
    let fine = r#"
        let a = f(&state.db.lock().unwrap());
        let b = g(&state.db.lock().unwrap(), "x.db.lock().unwrap()");
        if let Some(x) = y { state.log("a", "b"); }
    "#;
    assert!(statements(fine).iter().all(|(_, s)| double_locks(s).is_none()));
    let helper = "db::set(&state.db.lock().unwrap(), K, &state.settings())";
    assert!(double_locks(helper).is_some());
}

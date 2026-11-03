// SPDX-License-Identifier: AGPL-3.0-only
//! Changes made here (read, archived, trashed), sent to the mail server, and syncing every account.

use super::*;

// ---------- changes made here, sent to the mail server ----------

/// Something done to messages in SulcusAI that the mail server should know:
/// read state, archiving (out of the inbox) and moving to the trash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Seen,
    Unseen,
    Archive,
    Trash,
}

impl Change {
    pub fn parse(s: &str) -> Option<Change> {
        Some(match s {
            "seen" => Change::Seen,
            "unseen" => Change::Unseen,
            "archive" => Change::Archive,
            "trash" => Change::Trash,
            _ => return None,
        })
    }
    fn code(self) -> &'static str {
        match self {
            Change::Seen => "seen",
            Change::Unseen => "unseen",
            Change::Archive => "archive",
            Change::Trash => "trash",
        }
    }
}

/// Where archived mail is kept on this PC (it isn't synced from the server).
pub const ARCHIVE: &str = "Archive";

/// Applies a change on this PC now and queues it for the server (sent by
/// `flush_pending` right away when online, else at the next sync).
pub fn change(conn: &Connection, ids: &[String], what: Change) -> Result<(), String> {
    for id in ids {
        let row: Option<(String, String, u32)> = conn
            .query_row("SELECT account_id, folder, uid FROM mail_messages WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .optional()
            .map_err(|e| e.to_string())?;
        let Some((account, folder, uid)) = row else { continue };
        match what {
            Change::Seen => {
                conn.execute("UPDATE mail_messages SET seen = 1 WHERE id = ?1", [id]).map_err(|e| e.to_string())?;
            }
            // Unread again: out of Marked read, back in the inbox.
            Change::Unseen => {
                conn.execute("UPDATE mail_messages SET seen = 0, marked_at = NULL WHERE id = ?1", [id]).map_err(|e| e.to_string())?;
            }
            Change::Archive => {
                if folder == ARCHIVE {
                    continue;
                }
                // Kept here under Archive (All mail shows it); if that clashes, the copy goes.
                if conn.execute("UPDATE mail_messages SET folder = ?2, seen = 1 WHERE id = ?1", params![id, ARCHIVE]).is_err() {
                    conn.execute("DELETE FROM mail_messages WHERE id = ?1", [id]).map_err(|e| e.to_string())?;
                }
            }
            Change::Trash => {
                conn.execute("DELETE FROM mail_triage WHERE id = ?1", [id]).ok();
                conn.execute("DELETE FROM mail_messages WHERE id = ?1", [id]).map_err(|e| e.to_string())?;
            }
        }
        // A copy kept under Archive has no place on the server we track.
        if folder != ARCHIVE {
            conn.execute(
                "INSERT INTO mail_pending (account_id, folder, uid, op, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![account, folder, uid, what.code(), db::now_ms()],
            )
            .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// The mark-read button: read here and on the server, and listed under
/// Marked read (newest mark first) instead of the inbox, until it's unread
/// again.
pub fn mark_read(conn: &Connection, ids: &[String]) -> Result<(), String> {
    change(conn, ids, Change::Seen)?;
    let now = db::now_ms();
    for (i, id) in ids.iter().enumerate() {
        conn.execute("UPDATE mail_messages SET marked_at = ?2 WHERE id = ?1", params![id, now + i as i64]).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// A special folder by its IMAP SPECIAL-USE flag, else by a usual name.
pub(super) async fn find_special(s: &mut Session, want: &str) -> Option<String> {
    use async_imap::types::NameAttribute;
    let names: Vec<_> = s.list(None, Some("*")).await.ok()?.try_collect().await.ok()?;
    let flagged = |n: &async_imap::types::Name| {
        n.attributes().iter().any(|a| match want {
            "all" => matches!(a, NameAttribute::All),
            "archive" => matches!(a, NameAttribute::Archive),
            "trash" => matches!(a, NameAttribute::Trash),
            _ => false,
        })
    };
    let usual: &[&str] = match want {
        "all" => &["[gmail]/all mail", "[google mail]/all mail", "all mail"],
        "archive" => &["archive", "archives"],
        "trash" => &["trash", "deleted items", "deleted messages", "[gmail]/trash", "[gmail]/bin", "[google mail]/trash", "[google mail]/bin"],
        _ => &[],
    };
    names
        .iter()
        .find(|n| flagged(n))
        .or_else(|| names.iter().find(|n| usual.contains(&n.name().to_ascii_lowercase().as_str())))
        .map(|n| n.name().to_string())
}

/// Moves messages; servers without MOVE get copy + delete-from-here.
pub(super) async fn move_to(s: &mut Session, set: &str, target: &str) -> Result<(), String> {
    if s.uid_mv(set, target).await.is_ok() {
        return Ok(());
    }
    s.uid_copy(set, target).await.map_err(|e| e.to_string())?;
    let _: Vec<_> = s.uid_store(set, "+FLAGS.SILENT (\\Deleted)").await.map_err(|e| e.to_string())?.try_collect().await.map_err(|e| e.to_string())?;
    let _: Vec<_> = s.uid_expunge(set).await.map_err(|e| e.to_string())?.try_collect().await.map_err(|e| e.to_string())?;
    Ok(())
}

/// Sends one account's queued changes over an open session.
pub(super) async fn push_pending(state: &AppState, s: &mut Session, id: &str, cfg: &AccountConfig) -> Result<(), String> {
    let rows: Vec<(i64, String, u32, String, i64)> = {
        let conn = state.db.lock().unwrap();
        let mut stmt = conn.prepare("SELECT rowid, folder, uid, op, created_at FROM mail_pending WHERE account_id = ?1 ORDER BY rowid").map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .collect();
        rows
    };
    if rows.is_empty() {
        return Ok(());
    }
    // Gmail archives by leaving the inbox for All Mail.
    let gmail = cfg.imap_host.to_ascii_lowercase().contains("gmail") || cfg.imap_host.to_ascii_lowercase().contains("googlemail");
    // Where archived and trashed mail goes, looked up before any folder is open.
    let archive = if rows.iter().any(|r| r.3 == "archive") { find_special(s, if gmail { "all" } else { "archive" }).await } else { None };
    let trash = if rows.iter().any(|r| r.3 == "trash") { find_special(s, "trash").await } else { None };
    // Runs of the same folder and change, in the order they were made.
    let mut i = 0;
    while i < rows.len() {
        let (folder, op) = (rows[i].1.clone(), rows[i].3.clone());
        let mut j = i;
        while j < rows.len() && rows[j].1 == folder && rows[j].3 == op {
            j += 1;
        }
        let group = &rows[i..j];
        let set = group.iter().map(|r| r.2.to_string()).collect::<Vec<_>>().join(",");
        let result: Result<(), String> = async {
            s.select(&folder).await.map_err(|e| format!("Couldn't open {folder}: {e}"))?;
            match op.as_str() {
                "seen" | "unseen" => {
                    let sign = if op == "seen" { "+" } else { "-" };
                    let _: Vec<_> = s.uid_store(&set, format!("{sign}FLAGS.SILENT (\\Seen)")).await.map_err(|e| e.to_string())?.try_collect().await.map_err(|e| e.to_string())?;
                }
                "archive" => {
                    if let Some(target) = archive.as_deref().filter(|t| *t != folder) {
                        move_to(s, &set, target).await?;
                    }
                }
                // Never deleted outright: without a trash folder it stays put.
                "trash" => {
                    if let Some(target) = trash.as_deref().filter(|t| *t != folder) {
                        move_to(s, &set, target).await?;
                    }
                }
                _ => {}
            }
            Ok(())
        }
        .await;
        let conn = state.db.lock().unwrap();
        if let Err(e) = &result {
            db::log_action(&conn, "email", &format!("Couldn't update the mail server ({op} in {folder}, will try again): {e}"));
        }
        let week_ago = db::now_ms() - 7 * 86_400_000;
        for r in group {
            // Done, or failing for a week (the message is gone): drop it.
            if result.is_ok() || r.4 < week_ago {
                conn.execute("DELETE FROM mail_pending WHERE rowid = ?1", [r.0]).ok();
            }
        }
        drop(conn);
        i = j;
    }
    Ok(())
}

/// Read state and inbox membership of mail already here, as the server has
/// them now (read on the phone, archived on the web…).
pub(super) async fn pull_state(state: &AppState, s: &mut Session, id: &str) -> Result<(), String> {
    let local: Vec<(String, u32, bool)> = {
        let conn = state.db.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT id, uid, seen FROM mail_messages WHERE account_id = ?1 AND folder = 'INBOX' ORDER BY uid DESC LIMIT 500")
            .map_err(|e| e.to_string())?;
        let rows = stmt.query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0))).map_err(|e| e.to_string())?.filter_map(Result::ok).collect();
        rows
    };
    if local.is_empty() {
        return Ok(());
    }
    let pending = |conn: &rusqlite::Connection| -> std::collections::HashSet<u32> {
        conn.prepare("SELECT uid FROM mail_pending WHERE account_id = ?1 AND folder = 'INBOX'")
            .and_then(|mut st| st.query_map([id], |r| r.get(0)).map(|rows| rows.filter_map(Result::ok).collect()))
            .unwrap_or_default()
    };
    // Changes waiting to go up before the fetch too: one sent (and its row
    // removed) while the server's old flags were coming back isn't undone.
    let waiting_before = pending(&state.db.lock().unwrap());
    s.examine("INBOX").await.map_err(|e| e.to_string())?;
    let mut flags: HashMap<u32, bool> = HashMap::new();
    for chunk in local.chunks(200) {
        let set = chunk.iter().map(|(_, u, _)| u.to_string()).collect::<Vec<_>>().join(",");
        let fetched: Vec<_> = s.uid_fetch(&set, "(UID FLAGS)").await.map_err(|e| e.to_string())?.try_collect().await.map_err(|e| e.to_string())?;
        for f in fetched {
            if let Some(uid) = f.uid {
                flags.insert(uid, f.flags().any(|fl| matches!(fl, async_imap::types::Flag::Seen)));
            }
        }
    }
    let conn = state.db.lock().unwrap();
    let waiting = pending(&conn);
    for (mid, uid, seen) in local {
        if waiting.contains(&uid) || waiting_before.contains(&uid) {
            continue;
        }
        match flags.get(&uid) {
            // Unread again there: out of Marked read here too.
            Some(&now) if now != seen => {
                conn.execute("UPDATE mail_messages SET seen = ?2, marked_at = CASE WHEN ?2 = 0 THEN NULL ELSE marked_at END WHERE id = ?1", params![mid, now as i64]).ok();
            }
            Some(_) => {}
            // Gone from the inbox there (archived or deleted elsewhere).
            None => {
                if conn.execute("UPDATE mail_messages SET folder = ?2 WHERE id = ?1", params![mid, ARCHIVE]).is_err() {
                    conn.execute("DELETE FROM mail_messages WHERE id = ?1", [&mid]).ok();
                }
            }
        }
    }
    Ok(())
}

/// Sends queued changes for every account that has some.
pub async fn flush_pending(state: &AppState, c: &Cipher) -> Result<(), String> {
    web_allowed(state)?;
    let ids: Vec<String> = {
        let conn = state.db.lock().unwrap();
        let mut stmt = conn.prepare("SELECT DISTINCT account_id FROM mail_pending").map_err(|e| e.to_string())?;
        let ids = stmt.query_map([], |r| r.get(0)).map_err(|e| e.to_string())?.filter_map(Result::ok).collect();
        ids
    };
    for id in ids {
        let Some(mut cfg) = account(&state.db.lock().unwrap(), c, &id) else {
            state.db.lock().unwrap().execute("DELETE FROM mail_pending WHERE account_id = ?1", [&id]).ok();
            continue;
        };
        refresh(state, c, Some(&id), &mut cfg).await?;
        let mut s = imap_login(&cfg).await?;
        let r = push_pending(state, &mut s, &id, &cfg).await;
        let _ = s.logout().await;
        r?;
    }
    Ok(())
}

/// Changes made within this long of each other go to the server together
/// (marking read one message after another, quickly).
pub(super) const FLUSH_QUIET_MS: i64 = 1500;
pub(super) static FLUSH_ASKED: AtomicU64 = AtomicU64::new(0);
pub(super) static FLUSH_LAST: AtomicI64 = AtomicI64::new(0);
pub(super) static FLUSHING: AtomicBool = AtomicBool::new(false);

/// Sends queued changes in the background, if web access is on: once the
/// changes pause for a moment, and one sign-in at a time.
pub fn flush_soon(state: Arc<AppState>) {
    if web_allowed(&state).is_err() {
        return;
    }
    FLUSH_LAST.store(db::now_ms(), Ordering::SeqCst);
    FLUSH_ASKED.fetch_add(1, Ordering::SeqCst);
    if FLUSHING.swap(true, Ordering::SeqCst) {
        // The one running sends this too.
        return;
    }
    tauri::async_runtime::spawn(async move {
        loop {
            loop {
                let quiet = db::now_ms() - FLUSH_LAST.load(Ordering::SeqCst);
                if quiet >= FLUSH_QUIET_MS {
                    break;
                }
                tokio::time::sleep(Duration::from_millis((FLUSH_QUIET_MS - quiet) as u64)).await;
            }
            let asked = FLUSH_ASKED.load(Ordering::SeqCst);
            if let Ok(c) = state.work_cipher() {
                let _ = flush_pending(&state, &c).await;
            }
            // More changes came while sending: send those too.
            if FLUSH_ASKED.load(Ordering::SeqCst) != asked {
                continue;
            }
            FLUSHING.store(false, Ordering::SeqCst);
            // One asked just now saw this running and left it to this one.
            if FLUSH_ASKED.load(Ordering::SeqCst) == asked || FLUSHING.swap(true, Ordering::SeqCst) {
                break;
            }
        }
    });
}

pub async fn sync_all(state: &AppState, c: &Cipher) -> Result<usize, String> {
    let ids: Vec<String> = accounts(&state.db.lock().unwrap(), c).into_iter().map(|(id, _, _)| id).collect();
    let mut total = 0;
    let mut errors = Vec::new();
    for id in ids {
        match sync_account(state, c, &id).await {
            Ok(n) => total += n,
            Err(e) => errors.push(e),
        }
    }
    if total == 0 && !errors.is_empty() {
        return Err(errors.join(" "));
    }
    Ok(total)
}

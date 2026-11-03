//! The structured export formats: KeePass XML, Bitwarden JSON and
//! 1Password's 1PUX. CSV exports from all of these go through `import_csv`.

use std::io::Read;

use serde_json::Value;
use zeroize::Zeroize;

use super::import::{parse_time, Imported, Source, MAX_IMPORT_BYTES};
use super::store::Entry;
use super::VaultError;

/// Plaintext passwords sit in the parsed tree until it's dropped; wipe them
/// first. serde_json's depth limit keeps this recursion shallow.
fn wipe_value(v: &mut Value) {
    match v {
        Value::String(s) => s.zeroize(),
        Value::Array(a) => a.iter_mut().for_each(wipe_value),
        Value::Object(o) => o.values_mut().for_each(wipe_value),
        _ => {}
    }
}

fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

pub(crate) fn bitwarden_json(text: &str) -> Result<Imported, VaultError> {
    let mut root: Value = serde_json::from_str(text).map_err(|_| VaultError::BadImport("this JSON file can't be read".into()))?;
    let result = bitwarden_items(&root);
    wipe_value(&mut root);
    result
}

fn bitwarden_items(root: &Value) -> Result<Imported, VaultError> {
    if root.get("encrypted").and_then(Value::as_bool) == Some(true) || root.get("passwordProtected").and_then(Value::as_bool) == Some(true) {
        return Err(VaultError::BadImport(
            "this Bitwarden export is encrypted: export again choosing the plain .json format, import that, then delete it".into(),
        ));
    }
    let Some(items) = root.get("items").and_then(Value::as_array) else {
        return Err(VaultError::BadImport("this JSON file isn't a Bitwarden export".into()));
    };
    let mut out = Imported { source: Some(Source::Bitwarden), ..Default::default() };
    for item in items {
        let login = item.get("login");
        let deleted = item.get("deletedDate").is_some_and(|d| !d.is_null());
        let is_login = item.get("type").and_then(Value::as_i64) == Some(1);
        let password = login.map(|l| s(l, "password")).unwrap_or("");
        if !is_login || deleted || password.is_empty() {
            out.skipped += 1;
            continue;
        }
        let login = login.unwrap_or(&Value::Null);
        let uri = login
            .get("uris")
            .and_then(Value::as_array)
            .and_then(|u| u.iter().map(|x| s(x, "uri")).find(|x| !x.is_empty()))
            .unwrap_or("");
        let mut e = Entry::login(uri.trim(), s(login, "username").trim(), password);
        e.name = s(item, "name").trim().to_string();
        e.notes = s(item, "notes").to_string();
        e.created_ms = parse_time(s(item, "creationDate")).unwrap_or(0);
        e.changed_ms = parse_time(s(login, "passwordRevisionDate")).or_else(|| parse_time(s(item, "revisionDate"))).unwrap_or(0);
        out.entries.push(e);
    }
    Ok(out)
}

pub(crate) fn one_password_1pux(bytes: &[u8]) -> Result<Imported, VaultError> {
    let bad = || VaultError::BadImport("this file isn't a 1Password .1pux export".into());
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|_| bad())?;
    let mut data = Vec::new();
    {
        let file = zip.by_name("export.data").map_err(|_| bad())?;
        // The size in the zip header can lie; the cap holds either way.
        file.take(MAX_IMPORT_BYTES as u64 + 1).read_to_end(&mut data).map_err(|_| bad())?;
    }
    if data.len() > MAX_IMPORT_BYTES {
        data.zeroize();
        return Err(VaultError::BadImport("this export is too large".into()));
    }
    let parsed = serde_json::from_slice::<Value>(&data);
    data.zeroize();
    let mut root = parsed.map_err(|_| bad())?;
    let result = one_password_items(&root);
    wipe_value(&mut root);
    result
}

fn one_password_items(root: &Value) -> Result<Imported, VaultError> {
    let Some(accounts) = root.get("accounts").and_then(Value::as_array) else {
        return Err(VaultError::BadImport("this file isn't a 1Password .1pux export".into()));
    };
    let mut out = Imported { source: Some(Source::OnePassword), ..Default::default() };
    let items = accounts
        .iter()
        .filter_map(|a| a.get("vaults").and_then(Value::as_array))
        .flatten()
        .filter_map(|v| v.get("items").and_then(Value::as_array))
        .flatten();
    for item in items {
        // Login ("001") and Password ("005") items hold passwords; the rest
        // are cards, notes and documents.
        let category = s(item, "categoryUuid");
        let details = item.get("details").unwrap_or(&Value::Null);
        let overview = item.get("overview").unwrap_or(&Value::Null);
        let fields: &[Value] = details.get("loginFields").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]);
        let field = |designation: &str| fields.iter().find(|f| s(f, "designation") == designation).map(|f| s(f, "value")).unwrap_or("");
        let mut password = field("password");
        if password.is_empty() {
            password = s(details, "password");
        }
        if !matches!(category, "001" | "005") || s(item, "state") == "trashed" || password.is_empty() {
            out.skipped += 1;
            continue;
        }
        let mut url = s(overview, "url");
        if url.is_empty() {
            url = overview.get("urls").and_then(Value::as_array).and_then(|u| u.iter().map(|x| s(x, "url")).find(|x| !x.is_empty())).unwrap_or("");
        }
        let mut e = Entry::login(url.trim(), field("username").trim(), password);
        e.name = s(overview, "title").trim().to_string();
        e.notes = s(details, "notesPlain").to_string();
        let secs = |k: &str| item.get(k).and_then(Value::as_i64).filter(|n| *n > 0 && *n < 100_000_000_000).map(|n| n * 1000);
        e.created_ms = secs("createdAt").unwrap_or(0);
        e.changed_ms = secs("updatedAt").unwrap_or(0);
        out.entries.push(e);
    }
    Ok(out)
}

/// KeePass's own time format inside KDBX 4: base64 of seconds since
/// 0001-01-01, little-endian. XML exports use RFC 3339 instead.
fn keepass_time(text: &str) -> Option<i64> {
    if let Some(t) = parse_time(text) {
        return Some(t);
    }
    use base64::Engine;
    let raw = base64::engine::general_purpose::STANDARD.decode(text.trim()).ok()?;
    let arr: [u8; 8] = raw.as_slice().try_into().ok()?;
    const YEAR_ONE_TO_1970: i64 = 62_135_596_800;
    i64::from_le_bytes(arr).checked_sub(YEAR_ONE_TO_1970)?.checked_mul(1000)
}

const MAX_XML_DEPTH: usize = 256;

#[derive(Default)]
struct KpEntry {
    fields: Vec<(String, String)>,
    created: String,
    changed: String,
}

impl Drop for KpEntry {
    fn drop(&mut self) {
        for (_, v) in self.fields.iter_mut() {
            v.zeroize();
        }
    }
}

pub(crate) fn keepass_xml(text: &str) -> Result<Imported, VaultError> {
    use quick_xml::events::Event as X;
    let mut reader = quick_xml::Reader::from_str(text);
    let mut stack: Vec<String> = Vec::new();
    let mut groups: Vec<String> = Vec::new();
    let mut recycle = String::new();
    let mut cur: Option<KpEntry> = None;
    let mut key = String::new();
    let mut value = String::new();
    let mut seen_root = false;
    let mut out = Imported { source: Some(Source::KeePass), ..Default::default() };

    let in_history = |stack: &[String]| stack.iter().any(|n| n == "History");
    loop {
        let text_piece: Option<String> = match reader.read_event() {
            Ok(X::Start(e)) => {
                // Real exports nest a few dozen levels at most; the cap keeps
                // the per-element checks below cheap on a hostile file.
                if stack.len() >= MAX_XML_DEPTH {
                    return Err(VaultError::BadImport("this XML file is nested too deeply to be a KeePass export".into()));
                }
                let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                match name.as_str() {
                    "KeePassFile" => seen_root = true,
                    "Group" => groups.push(String::new()),
                    "Entry" if !in_history(&stack) => cur = Some(KpEntry::default()),
                    "String" => {
                        key.clear();
                        value.zeroize();
                    }
                    _ => {}
                }
                stack.push(name);
                None
            }
            Ok(X::End(_)) => {
                let name = stack.pop().unwrap_or_default();
                match name.as_str() {
                    "Group" => {
                        groups.pop();
                    }
                    "String" if !in_history(&stack) => {
                        if let Some(c) = cur.as_mut() {
                            c.fields.push((std::mem::take(&mut key), std::mem::take(&mut value)));
                        }
                    }
                    "Entry" if !in_history(&stack) => {
                        if let Some(c) = cur.take() {
                            let binned = !recycle.is_empty() && groups.iter().any(|g| *g == recycle);
                            finish_keepass(c, binned, &mut out);
                        }
                    }
                    _ => {}
                }
                None
            }
            Ok(X::Text(t)) => Some(t.decode().map(|c| c.into_owned()).unwrap_or_default()),
            Ok(X::CData(t)) => Some(String::from_utf8_lossy(&t).into_owned()),
            Ok(X::GeneralRef(r)) => Some(match r.decode().unwrap_or_default().as_ref() {
                "amp" => "&".into(),
                "lt" => "<".into(),
                "gt" => ">".into(),
                "quot" => "\"".into(),
                "apos" => "'".into(),
                other => other
                    .strip_prefix("#x")
                    .and_then(|h| u32::from_str_radix(h, 16).ok())
                    .or_else(|| other.strip_prefix('#').and_then(|d| d.parse().ok()))
                    .and_then(char::from_u32)
                    .map(String::from)
                    .unwrap_or_default(),
            }),
            Ok(X::Eof) | Err(_) => break,
            _ => None,
        };
        let Some(mut piece) = text_piece else { continue };
        let top = stack.last().map(String::as_str).unwrap_or("");
        let parent = stack.len().checked_sub(2).map(|i| stack[i].as_str()).unwrap_or("");
        let grand = stack.len().checked_sub(3).map(|i| stack[i].as_str()).unwrap_or("");
        match (top, parent) {
            ("Key", "String") => key.push_str(&piece),
            ("Value", "String") => value.push_str(&piece),
            ("UUID", "Group") => {
                if let Some(g) = groups.last_mut() {
                    g.push_str(piece.trim());
                }
            }
            ("RecycleBinUUID", "Meta") => recycle.push_str(piece.trim()),
            ("CreationTime", "Times") | ("LastModificationTime", "Times") if grand == "Entry" && !in_history(&stack) => {
                if let Some(c) = cur.as_mut() {
                    if top == "CreationTime" {
                        c.created.push_str(&piece);
                    } else {
                        c.changed.push_str(&piece);
                    }
                }
            }
            _ => {}
        }
        piece.zeroize();
    }
    value.zeroize();
    if !seen_root {
        return Err(VaultError::BadImport("this XML file isn't a KeePass export".into()));
    }
    Ok(out)
}

fn finish_keepass(c: KpEntry, binned: bool, out: &mut Imported) {
    let get = |k: &str| c.fields.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str()).unwrap_or("");
    let password = get("Password");
    if binned || password.is_empty() {
        out.skipped += 1;
        return;
    }
    let mut e = Entry::login(get("URL").trim(), get("UserName").trim(), password);
    e.name = get("Title").trim().to_string();
    e.notes = get("Notes").to_string();
    e.created_ms = keepass_time(&c.created).unwrap_or(0);
    e.changed_ms = keepass_time(&c.changed).unwrap_or(0);
    out.entries.push(e);
}

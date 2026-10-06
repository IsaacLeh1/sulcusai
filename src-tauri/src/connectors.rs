// SPDX-License-Identifier: AGPL-3.0-only
//! Connectors (local MCP servers) and plugins (skills plus connectors in a
//! folder). Settings are stored encrypted, since connector settings often
//! hold API keys.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::crypto::Cipher;
use crate::db;
use crate::mcp::{self, McpTool, ServerSpec};
use crate::{AppState, AppStateRef};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolMode {
    /// Runs without asking (plan mode still only runs read-only tools).
    Allow,
    /// Asks first in Auto mode.
    Ask,
    /// Never offered to the model.
    Off,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Connector {
    pub id: String,
    pub name: String,
    pub spec: ServerSpec,
    pub enabled: bool,
    /// Set when a plugin installed it; removed with the plugin.
    #[serde(default)]
    pub plugin: Option<String>,
    /// Tools seen the last time it connected.
    #[serde(default)]
    pub tools: Vec<McpTool>,
    #[serde(default)]
    pub tool_modes: HashMap<String, ToolMode>,
}

impl Connector {
    pub fn mode(&self, tool: &McpTool) -> ToolMode {
        self.tool_modes.get(&tool.name).copied().unwrap_or(if tool.read_only { ToolMode::Allow } else { ToolMode::Ask })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plugin {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    pub enabled: bool,
    pub dir: String,
    #[serde(default)]
    pub skills: Vec<Skill>,
    #[serde(default)]
    pub connectors: Vec<String>,
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn load<T: for<'de> Deserialize<'de>>(conn: &Connection, c: &Cipher, table: &str) -> Vec<T> {
    let Ok(mut stmt) = conn.prepare(&format!("SELECT data FROM {table} ORDER BY created_at")) else { return Vec::new() };
    stmt.query_map([], |r| r.get::<_, String>(0))
        .map(|rows| rows.filter_map(Result::ok).filter_map(|d| c.decrypt(&d).ok().and_then(|j| serde_json::from_str(&j).ok())).collect())
        .unwrap_or_default()
}

pub(crate) fn store<T: Serialize>(conn: &Connection, c: &Cipher, table: &str, id: &str, value: &T) -> Result<(), String> {
    let data = c.encrypt(&serde_json::to_string(value).map_err(err)?);
    conn.execute(
        &format!("INSERT INTO {table} (id, data, created_at) VALUES (?1, ?2, ?3) ON CONFLICT(id) DO UPDATE SET data = excluded.data"),
        params![id, data, db::now_ms()],
    )
    .map_err(err)?;
    Ok(())
}

pub fn connectors(conn: &Connection, c: &Cipher) -> Vec<Connector> {
    load(conn, c, "connectors")
}

pub fn plugins(conn: &Connection, c: &Cipher) -> Vec<Plugin> {
    load(conn, c, "plugins")
}

/// Skills from enabled plugins.
pub fn active_skills(conn: &Connection, c: &Cipher) -> Vec<Skill> {
    plugins(conn, c).into_iter().filter(|p| p.enabled).flat_map(|p| p.skills).collect()
}

/// Skills this short go into the prompt whole; small models often skip
/// loading a skill and guess from its summary.
const INLINE_SKILL_CHARS: usize = 1_200;
const INLINE_BUDGET: usize = 4_000;

/// Splits skills into those shown in full and those loaded on demand.
pub fn split_skills(skills: &[Skill]) -> (Vec<&Skill>, Vec<&Skill>) {
    let mut budget = INLINE_BUDGET;
    let mut inline = Vec::new();
    let mut on_demand = Vec::new();
    for k in skills {
        let len = k.body.chars().count();
        if len <= INLINE_SKILL_CHARS && len <= budget {
            budget -= len;
            inline.push(k);
        } else {
            on_demand.push(k);
        }
    }
    (inline, on_demand)
}

pub fn skills_prompt(skills: &[Skill]) -> String {
    if skills.is_empty() {
        return String::new();
    }
    let (inline, on_demand) = split_skills(skills);
    let mut s = String::from("\n\n## Skills\nWhen the user's request matches one of these skills, or names it, follow that skill's instructions exactly.");
    for k in &inline {
        s.push_str(&format!("\n\n### Skill: {}\n{}\nInstructions:\n{}", k.name, k.description, k.body));
    }
    if !on_demand.is_empty() {
        s.push_str("\n\nThese longer skills are summarized here; call load_skill with the name to read the full instructions before using one:");
        for k in &on_demand {
            s.push_str(&format!("\n- {}: {}", k.name, k.description));
        }
    }
    s
}

/// A connector tool the model can call this turn.
#[derive(Debug, Clone)]
pub struct OfferedTool {
    pub connector_id: String,
    pub connector_name: String,
    pub tool: String,
    pub mode: ToolMode,
    pub read_only: bool,
}

/// Starts enabled connectors as needed and returns their tool definitions
/// for the model, plus a lookup from model-facing name to connector tool.
/// Connectors that fail to start are reported, not fatal.
pub async fn offer(state: &AppState, c: &Cipher) -> (Vec<Value>, HashMap<String, OfferedTool>, Vec<String>) {
    let list: Vec<Connector> = connectors(&state.db.lock().unwrap(), c).into_iter().filter(|x| x.enabled).collect();
    let mut defs = Vec::new();
    let mut map = HashMap::new();
    let mut problems = Vec::new();
    for mut conn in list {
        match ensure_running(state, &conn).await {
            Ok(tools) => {
                if tools != conn.tools {
                    conn.tools = tools;
                    let _ = store(&state.db.lock().unwrap(), c, "connectors", &conn.id.clone(), &conn);
                }
            }
            Err(e) => {
                problems.push(format!("{}: {e}", conn.name));
                continue;
            }
        }
        for t in &conn.tools {
            let mode = conn.mode(t);
            if mode == ToolMode::Off {
                continue;
            }
            let name = mcp::tool_name(&conn.name, &t.name);
            defs.push(json!({ "type": "function", "function": {
                "name": name,
                "description": format!("[{} connector] {}", conn.name, t.description),
                "parameters": t.input_schema,
            } }));
            map.insert(name, OfferedTool { connector_id: conn.id.clone(), connector_name: conn.name.clone(), tool: t.name.clone(), mode, read_only: t.read_only });
        }
    }
    (defs, map, problems)
}

/// Makes sure the connector's server is running; returns its tools.
pub async fn ensure_running(state: &AppState, conn: &Connector) -> Result<Vec<McpTool>, String> {
    let mut clients = state.mcp.lock().await;
    if let Some(client) = clients.get_mut(&conn.id) {
        if client.alive() {
            return client.list_tools().await;
        }
        clients.remove(&conn.id);
    }
    let mut client = mcp::Client::start(&conn.spec, state.job()).await?;
    let tools = client.list_tools().await?;
    clients.insert(conn.id.clone(), client);
    Ok(tools)
}

pub async fn call(state: &AppState, connector_id: &str, tool: &str, args: Value) -> Result<(bool, String), String> {
    let mut clients = state.mcp.lock().await;
    let client = clients.get_mut(connector_id).ok_or("The connector isn't running.")?;
    client.call(tool, args).await
}

async fn stop(state: &AppState, id: &str) {
    if let Some(client) = state.mcp.lock().await.remove(id) {
        client.stop().await;
    }
}

// ---------- plugins on disk ----------

#[derive(Debug, Deserialize)]
struct Manifest {
    name: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    skills: Vec<String>,
    #[serde(default, rename = "mcpServers")]
    mcp_servers: HashMap<String, ServerSpec>,
}

#[derive(Debug, Serialize)]
pub struct PluginPreview {
    pub name: String,
    pub version: String,
    pub description: String,
    pub skills: Vec<String>,
    /// Programs it would run: "name: command args".
    pub programs: Vec<String>,
}

fn parse_skill(text: &str, fallback: &str) -> Skill {
    // Optional front matter: --- name: x / description: y ---
    let mut name = fallback.to_string();
    let mut description = String::new();
    let mut body = text;
    if let Some(rest) = text.strip_prefix("---") {
        if let Some(end) = rest.find("\n---") {
            for line in rest[..end].lines() {
                if let Some(v) = line.strip_prefix("name:") {
                    name = v.trim().trim_matches('"').to_string();
                } else if let Some(v) = line.strip_prefix("description:") {
                    description = v.trim().trim_matches('"').to_string();
                }
            }
            body = rest[end + 4..].trim_start_matches(['-', '\r', '\n']);
        }
    }
    if description.is_empty() {
        description = body.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with('#')).unwrap_or("").chars().take(160).collect();
    }
    Skill { name, description, body: body.trim().to_string() }
}

fn read_manifest(dir: &Path) -> Result<Manifest, String> {
    let text = std::fs::read_to_string(dir.join("plugin.json")).map_err(|_| "That folder has no plugin.json.".to_string())?;
    let m: Manifest = serde_json::from_str(&text).map_err(|e| format!("plugin.json isn't valid: {e}"))?;
    if m.name.trim().is_empty() {
        return Err("plugin.json needs a name.".into());
    }
    Ok(m)
}

fn plugin_id(name: &str) -> String {
    name.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect::<String>().trim_matches('-').to_string()
}

/// Paths in a plugin's server settings may use ${PLUGIN_DIR}.
fn expand(spec: &ServerSpec, dir: &Path) -> ServerSpec {
    let d = dir.display().to_string();
    let x = |s: &String| s.replace("${PLUGIN_DIR}", &d);
    ServerSpec {
        command: x(&spec.command),
        args: spec.args.iter().map(x).collect(),
        env: spec.env.iter().map(|(k, v)| (k.clone(), x(v))).collect(),
        cwd: Some(spec.cwd.as_ref().map(x).unwrap_or_else(|| d.clone())),
    }
}

fn skill_files(dir: &Path, m: &Manifest) -> Result<Vec<PathBuf>, String> {
    m.skills
        .iter()
        .map(|rel| {
            let p = dir.join(rel);
            let canon = dunce::canonicalize(&p).map_err(|_| format!("The skill file {rel} is missing."))?;
            if !canon.starts_with(dunce::canonicalize(dir).map_err(err)?) {
                return Err(format!("The skill file {rel} is outside the plugin folder."));
            }
            Ok(canon)
        })
        .collect()
}

fn copy_dir(from: &Path, to: &Path, budget: &mut u64) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(err)?;
    for entry in std::fs::read_dir(from).map_err(err)?.flatten() {
        let ft = entry.file_type().map_err(err)?;
        if ft.is_symlink() {
            continue;
        }
        let dest = to.join(entry.file_name());
        if ft.is_dir() {
            if entry.file_name() == ".git" || entry.file_name() == "node_modules" {
                continue;
            }
            copy_dir(&entry.path(), &dest, budget)?;
        } else {
            let len = entry.metadata().map_err(err)?.len();
            if len > *budget {
                return Err("The plugin folder is too big (over 50 MB).".into());
            }
            *budget -= len;
            std::fs::copy(entry.path(), dest).map_err(err)?;
        }
    }
    Ok(())
}

// ---------- commands ----------

#[derive(Debug, Deserialize)]
pub struct ConnectorInput {
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

#[tauri::command]
pub fn list_connectors(state: AppStateRef) -> Result<Vec<Connector>, String> {
    let c = state.cipher()?;
    Ok(connectors(&state.db.lock().unwrap(), &c))
}

#[tauri::command]
pub async fn save_connector(state: AppStateRef<'_>, input: ConnectorInput) -> Result<Connector, String> {
    let c = state.cipher()?;
    let name = input.name.trim();
    if name.is_empty() || input.command.trim().is_empty() {
        return Err("A connector needs a name and a command.".into());
    }
    let existing = connectors(&state.db.lock().unwrap(), &c);
    if existing.iter().any(|x| x.name.eq_ignore_ascii_case(name) && x.id != input.id) {
        return Err("Another connector already has that name.".into());
    }
    let mut conn = existing.into_iter().find(|x| x.id == input.id).unwrap_or(Connector {
        id: uuid::Uuid::new_v4().to_string(),
        name: String::new(),
        spec: ServerSpec::default(),
        enabled: true,
        plugin: None,
        tools: Vec::new(),
        tool_modes: HashMap::new(),
    });
    conn.name = name.to_string();
    conn.spec = ServerSpec { command: input.command.trim().to_string(), args: input.args, env: input.env, cwd: conn.spec.cwd.clone() };
    stop(&state, &conn.id).await;
    // Connect once now so the user sees its tools (or what went wrong).
    conn.tools = ensure_running(&state, &conn).await?;
    let db_conn = state.db.lock().unwrap();
    store(&db_conn, &c, "connectors", &conn.id, &conn)?;
    db::log_action(&db_conn, "privacy", "Added or changed a local connector (it runs a program on this PC)");
    Ok(conn)
}

#[tauri::command]
pub async fn delete_connector(state: AppStateRef<'_>, id: String) -> Result<(), String> {
    state.cipher()?;
    stop(&state, &id).await;
    let conn = state.db.lock().unwrap();
    conn.execute("DELETE FROM connectors WHERE id = ?1", [&id]).map_err(err)?;
    db::log_action(&conn, "privacy", "Removed a connector");
    Ok(())
}

#[tauri::command]
pub async fn set_connector_enabled(state: AppStateRef<'_>, id: String, enabled: bool) -> Result<(), String> {
    let c = state.cipher()?;
    let mut conn = connectors(&state.db.lock().unwrap(), &c).into_iter().find(|x| x.id == id).ok_or("That connector no longer exists.")?;
    conn.enabled = enabled;
    if !enabled {
        stop(&state, &id).await;
    }
    store(&state.db.lock().unwrap(), &c, "connectors", &id, &conn)
}

#[tauri::command]
pub fn set_tool_mode(state: AppStateRef, id: String, tool: String, mode: ToolMode) -> Result<(), String> {
    let c = state.cipher()?;
    let conn_db = state.db.lock().unwrap();
    let mut conn = connectors(&conn_db, &c).into_iter().find(|x| x.id == id).ok_or("That connector no longer exists.")?;
    conn.tool_modes.insert(tool, mode);
    store(&conn_db, &c, "connectors", &id, &conn)
}

#[tauri::command]
pub fn list_plugins(state: AppStateRef) -> Result<Vec<Plugin>, String> {
    let c = state.cipher()?;
    Ok(plugins(&state.db.lock().unwrap(), &c))
}

/// What installing a plugin folder would do, for the confirmation dialog.
#[tauri::command]
pub fn inspect_plugin(state: AppStateRef, path: String) -> Result<PluginPreview, String> {
    state.cipher()?;
    let dir = PathBuf::from(&path);
    let m = read_manifest(&dir)?;
    skill_files(&dir, &m)?;
    let mut programs: Vec<String> = m
        .mcp_servers
        .iter()
        .map(|(n, s)| {
            let s = expand(s, &dir);
            format!("{n}: {} {}", s.command, s.args.join(" ")).trim_end().to_string()
        })
        .collect();
    programs.sort();
    Ok(PluginPreview { name: m.name, version: m.version, description: m.description, skills: m.skills, programs })
}

#[tauri::command]
pub async fn install_plugin(state: AppStateRef<'_>, path: String) -> Result<Plugin, String> {
    let c = state.cipher()?;
    let src = PathBuf::from(&path);
    let m = read_manifest(&src)?;
    let id = plugin_id(&m.name);
    if id.is_empty() {
        return Err("The plugin's name needs letters or numbers.".into());
    }
    let dest = state.paths.data.join("plugins").join(&id);
    if dest.exists() {
        return Err("A plugin with that name is already installed. Remove it first to reinstall.".into());
    }
    let mut budget = 50 * 1024 * 1024;
    copy_dir(&src, &dest, &mut budget)?;

    let skills = skill_files(&dest, &m)?
        .iter()
        .map(|f| {
            let fallback = f.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            parse_skill(&std::fs::read_to_string(f).unwrap_or_default(), &fallback)
        })
        .collect();
    let mut connector_ids = Vec::new();
    for (name, spec) in &m.mcp_servers {
        let conn = Connector {
            id: uuid::Uuid::new_v4().to_string(),
            name: format!("{}-{name}", id),
            spec: expand(spec, &dest),
            enabled: true,
            plugin: Some(id.clone()),
            tools: Vec::new(),
            tool_modes: HashMap::new(),
        };
        store(&state.db.lock().unwrap(), &c, "connectors", &conn.id, &conn)?;
        connector_ids.push(conn.id);
    }
    let plugin = Plugin {
        id: id.clone(),
        name: m.name,
        version: m.version,
        description: m.description,
        enabled: true,
        dir: dest.display().to_string(),
        skills,
        connectors: connector_ids,
    };
    let conn = state.db.lock().unwrap();
    store(&conn, &c, "plugins", &id, &plugin)?;
    db::log_action(&conn, "privacy", &format!("Installed the plugin {}", plugin.name));
    Ok(plugin)
}

#[tauri::command]
pub async fn set_plugin_enabled(state: AppStateRef<'_>, id: String, enabled: bool) -> Result<(), String> {
    let c = state.cipher()?;
    let mut p = plugins(&state.db.lock().unwrap(), &c).into_iter().find(|x| x.id == id).ok_or("That plugin no longer exists.")?;
    p.enabled = enabled;
    for cid in &p.connectors {
        set_connector_enabled(state.clone(), cid.clone(), enabled).await.ok();
    }
    store(&state.db.lock().unwrap(), &c, "plugins", &id, &p)
}

#[tauri::command]
pub async fn remove_plugin(state: AppStateRef<'_>, id: String) -> Result<(), String> {
    let c = state.cipher()?;
    let p = plugins(&state.db.lock().unwrap(), &c).into_iter().find(|x| x.id == id).ok_or("That plugin no longer exists.")?;
    for cid in &p.connectors {
        stop(&state, cid).await;
    }
    {
        let conn = state.db.lock().unwrap();
        for cid in &p.connectors {
            conn.execute("DELETE FROM connectors WHERE id = ?1", [cid]).map_err(err)?;
        }
        conn.execute("DELETE FROM plugins WHERE id = ?1", [&id]).map_err(err)?;
        db::log_action(&conn, "privacy", &format!("Removed the plugin {}", p.name));
    }
    let dir = PathBuf::from(&p.dir);
    let plugins_root = state.paths.data.join("plugins");
    if dir.starts_with(&plugins_root) && dir != plugins_root {
        // Files the app copied in itself; safe to delete.
        std::fs::remove_dir_all(&dir).ok();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skills_read_front_matter_or_fall_back() {
        let s = parse_skill("---\nname: tidy-notes\ndescription: Clean up messy notes\n---\n# Steps\nDo this.", "file");
        assert_eq!(s.name, "tidy-notes");
        assert_eq!(s.description, "Clean up messy notes");
        assert!(s.body.starts_with("# Steps"));
        let s = parse_skill("# Title\nFirst real line.\nMore.", "my-skill");
        assert_eq!(s.name, "my-skill");
        assert_eq!(s.description, "First real line.");
    }

    #[test]
    fn short_skills_are_inlined_and_long_ones_loaded_on_demand() {
        let short = Skill { name: "short".into(), description: "d".into(), body: "Do X.".into() };
        let long = Skill { name: "long".into(), description: "d".into(), body: "y".repeat(INLINE_SKILL_CHARS + 1) };
        let (inline, later) = split_skills(&[short.clone(), long.clone()]);
        assert_eq!(inline.len(), 1);
        assert_eq!(later[0].name, "long");
        let p = skills_prompt(&[short, long]);
        assert!(p.contains("Do X.") && p.contains("load_skill") && !p.contains(&"y".repeat(50)));
    }

    #[test]
    fn plugin_paths_expand_and_ids_are_safe() {
        let spec = ServerSpec { command: "python".into(), args: vec!["${PLUGIN_DIR}/server.py".into()], ..Default::default() };
        let x = expand(&spec, Path::new("C:/plugins/demo"));
        assert_eq!(x.args[0], "C:/plugins/demo/server.py");
        assert_eq!(x.cwd.as_deref(), Some("C:/plugins/demo"));
        assert_eq!(plugin_id("My Cool Plugin!"), "my-cool-plugin");
    }

    #[test]
    fn read_only_tools_default_to_allow_and_others_to_ask() {
        let mut c = Connector { id: "c".into(), name: "n".into(), spec: ServerSpec::default(), enabled: true, plugin: None, tools: vec![], tool_modes: HashMap::new() };
        let ro = McpTool { name: "look".into(), description: String::new(), input_schema: json!({}), read_only: true };
        let rw = McpTool { name: "send".into(), description: String::new(), input_schema: json!({}), read_only: false };
        assert_eq!(c.mode(&ro), ToolMode::Allow);
        assert_eq!(c.mode(&rw), ToolMode::Ask);
        c.tool_modes.insert("send".into(), ToolMode::Off);
        assert_eq!(c.mode(&rw), ToolMode::Off);
    }

    #[test]
    fn skill_files_must_stay_inside_the_plugin() {
        let d = std::env::temp_dir().join(format!("sulcusai-plug-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(d.join("p/skills")).unwrap();
        std::fs::write(d.join("p/skills/a.md"), "x").unwrap();
        std::fs::write(d.join("outside.md"), "x").unwrap();
        let m = Manifest { name: "p".into(), version: String::new(), description: String::new(), skills: vec!["skills/a.md".into()], mcp_servers: HashMap::new() };
        assert!(skill_files(&d.join("p"), &m).is_ok());
        let bad = Manifest { skills: vec!["../outside.md".into()], ..m };
        assert!(skill_files(&d.join("p"), &bad).is_err());
    }
}

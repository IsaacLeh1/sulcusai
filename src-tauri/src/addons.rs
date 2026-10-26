// SPDX-License-Identifier: AGPL-3.0-only
//! One-click add-ons: well-known MCP servers the assistant can use, set up
//! without installing anything by hand. Those written for Node.js or Python
//! run on a Node.js and a uv that SulcusAI downloads once into its own
//! folder (nothing goes on the system or its PATH); online services
//! (Notion, Linear, GitHub…) are reached through `mcp-remote`, which signs
//! in through the browser. Each becomes a connector, so its tools ask first
//! like any other (Settings › Connectors).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::Emitter;

use crate::mcp::ServerSpec;
use crate::{engine, AppState, AppStateRef};

/// The runtimes add-ons run on (Windows x64).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Runtimes {
    pub node: crate::catalog::Asset,
    pub uv: crate::catalog::Asset,
}

/// How an add-on runs.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Run {
    /// An npm package, run with npx.
    Node { package: &'static str },
    /// A Python package, run with uvx.
    Uv { package: &'static str },
    /// An online service's MCP address, through mcp-remote.
    Remote { url: &'static str },
}

/// Something the person fills in when adding it.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Input {
    pub id: &'static str,
    pub label: &'static str,
    /// "folder", "file", "secret" or "text".
    pub kind: &'static str,
    pub help: &'static str,
    /// Given to the server as this environment variable (else as an argument).
    pub env: Option<&'static str>,
    /// The argument's flag ("--db-path"); none: the value alone.
    pub flag: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Addon {
    pub id: &'static str,
    pub name: &'static str,
    pub by: &'static str,
    pub category: &'static str,
    /// What it lets the assistant do.
    pub about: &'static str,
    pub run: Run,
    /// Arguments always given.
    pub args: &'static [&'static str],
    pub inputs: &'static [Input],
    pub link: &'static str,
}

const FOLDER: Input = Input { id: "folder", label: "Folder it may use", kind: "folder", help: "Only this folder (and what's in it) can be read or changed.", env: None, flag: None };

pub const ADDONS: &[Addon] = &[
    Addon { id: "filesystem", name: "Files", by: "Model Context Protocol", category: "On this PC", about: "Read, search, write and move files in one folder you pick.", run: Run::Node { package: "@modelcontextprotocol/server-filesystem" }, args: &[], inputs: &[FOLDER], link: "https://github.com/modelcontextprotocol/servers/tree/main/src/filesystem" },
    Addon { id: "git", name: "Git", by: "Model Context Protocol", category: "Code", about: "Look through a Git repository's history, changes and branches.", run: Run::Uv { package: "mcp-server-git" }, args: &[], inputs: &[Input { id: "repo", label: "The repository's folder", kind: "folder", help: "A folder with a .git folder in it.", env: None, flag: Some("--repository") }], link: "https://github.com/modelcontextprotocol/servers/tree/main/src/git" },
    Addon { id: "fetch", name: "Fetch web pages", by: "Model Context Protocol", category: "Web", about: "Read any web page as text, in pieces for long ones.", run: Run::Uv { package: "mcp-server-fetch" }, args: &[], inputs: &[], link: "https://github.com/modelcontextprotocol/servers/tree/main/src/fetch" },
    Addon { id: "memory", name: "Knowledge graph memory", by: "Model Context Protocol", category: "On this PC", about: "Keeps facts about people, places and projects as a graph the assistant can add to and search.", run: Run::Node { package: "@modelcontextprotocol/server-memory" }, args: &[], inputs: &[], link: "https://github.com/modelcontextprotocol/servers/tree/main/src/memory" },
    Addon { id: "thinking", name: "Step-by-step thinking", by: "Model Context Protocol", category: "On this PC", about: "Gives the assistant a scratchpad to plan a hard problem in steps and revise them.", run: Run::Node { package: "@modelcontextprotocol/server-sequential-thinking" }, args: &[], inputs: &[], link: "https://github.com/modelcontextprotocol/servers/tree/main/src/sequentialthinking" },
    Addon { id: "time", name: "Time zones", by: "Model Context Protocol", category: "On this PC", about: "The time anywhere, and converting between time zones.", run: Run::Uv { package: "mcp-server-time" }, args: &[], inputs: &[], link: "https://github.com/modelcontextprotocol/servers/tree/main/src/time" },
    Addon { id: "sqlite", name: "SQLite database", by: "Model Context Protocol", category: "Code", about: "Ask questions of a SQLite database file and change it with SQL.", run: Run::Uv { package: "mcp-server-sqlite" }, args: &[], inputs: &[Input { id: "db", label: "The database file", kind: "file", help: "A .db or .sqlite file.", env: None, flag: Some("--db-path") }], link: "https://github.com/modelcontextprotocol/servers-archived/tree/main/src/sqlite" },
    Addon { id: "playwright", name: "Browser automation (Playwright)", by: "Microsoft", category: "Web", about: "Drives a browser of its own: opens pages, clicks, fills forms and reads them.", run: Run::Node { package: "@playwright/mcp@latest" }, args: &["--browser", "msedge"], inputs: &[], link: "https://github.com/microsoft/playwright-mcp" },
    Addon { id: "context7", name: "Up-to-date code docs (Context7)", by: "Upstash", category: "Code", about: "Current documentation and examples for programming libraries, so code isn't written for old versions.", run: Run::Node { package: "@upstash/context7-mcp" }, args: &[], inputs: &[], link: "https://github.com/upstash/context7" },
    Addon { id: "brave", name: "Brave Search", by: "Brave", category: "Web", about: "Web, news and image search through Brave's own index.", run: Run::Node { package: "@brave/brave-search-mcp-server" }, args: &[], inputs: &[Input { id: "key", label: "Brave Search API key", kind: "secret", help: "Free from api-dashboard.search.brave.com.", env: Some("BRAVE_API_KEY"), flag: None }], link: "https://github.com/brave/brave-search-mcp-server" },
    Addon { id: "github", name: "GitHub", by: "GitHub", category: "Online services", about: "Issues, pull requests, code search and repositories on GitHub.", run: Run::Remote { url: "https://api.githubcopilot.com/mcp/" }, args: &[], inputs: &[Input { id: "token", label: "GitHub personal access token", kind: "secret", help: "From github.com › Settings › Developer settings › Personal access tokens.", env: Some("AUTH_TOKEN"), flag: None }], link: "https://github.com/github/github-mcp-server" },
    Addon { id: "notion", name: "Notion", by: "Notion", category: "Online services", about: "Search, read and write your Notion pages and databases. You sign in to Notion in your browser.", run: Run::Remote { url: "https://mcp.notion.com/mcp" }, args: &[], inputs: &[], link: "https://developers.notion.com/docs/mcp" },
    Addon { id: "linear", name: "Linear", by: "Linear", category: "Online services", about: "Find, create and update Linear issues and projects. You sign in to Linear in your browser.", run: Run::Remote { url: "https://mcp.linear.app/mcp" }, args: &[], inputs: &[], link: "https://linear.app/docs/mcp" },
    Addon { id: "atlassian", name: "Jira and Confluence", by: "Atlassian", category: "Online services", about: "Search and update Jira issues and Confluence pages. You sign in to Atlassian in your browser.", run: Run::Remote { url: "https://mcp.atlassian.com/v2/mcp" }, args: &[], inputs: &[], link: "https://github.com/atlassian/atlassian-mcp-server" },
    Addon { id: "sentry", name: "Sentry", by: "Sentry", category: "Online services", about: "Look into errors and performance problems your apps report to Sentry. You sign in in your browser.", run: Run::Remote { url: "https://mcp.sentry.dev/mcp" }, args: &[], inputs: &[], link: "https://docs.sentry.io/product/sentry-mcp/" },
    Addon { id: "huggingface", name: "Hugging Face", by: "Hugging Face", category: "Online services", about: "Search models, datasets, papers and Spaces on Hugging Face.", run: Run::Remote { url: "https://huggingface.co/mcp" }, args: &[], inputs: &[], link: "https://huggingface.co/settings/mcp" },
];

pub fn find(id: &str) -> Option<&'static Addon> {
    ADDONS.iter().find(|a| a.id == id)
}

/// The connector id an add-on is kept under.
pub fn connector_id(id: &str) -> String {
    format!("addon-{id}")
}

// ---------- runtimes ----------

fn say(state: &AppState, stage: &str, fraction: f64) {
    if let Some(app) = state.app.get() {
        let _ = app.emit("addons:progress", json!({ "stage": stage, "fraction": fraction }));
    }
}

/// Node.js, downloaded once into SulcusAI's engines folder: its node.exe.
async fn node(state: &Arc<AppState>) -> Result<PathBuf, String> {
    let rt = state.catalog.addons.clone().ok_or("Add-ons aren't available on this system yet.")?;
    let st = state.clone();
    engine::ensure_unpacked(&state.paths, "addons-node", &rt.node, if cfg!(windows) { "node.exe" } else { "node" }, &AtomicBool::new(false), move |r, t| say(&st, "Getting Node.js (once)", r as f64 / t.max(1) as f64)).await
}

/// uv, downloaded once: its uvx.exe.
async fn uvx(state: &Arc<AppState>) -> Result<PathBuf, String> {
    let rt = state.catalog.addons.clone().ok_or("Add-ons aren't available on this system yet.")?;
    let st = state.clone();
    engine::ensure_unpacked(&state.paths, "addons-uv", &rt.uv, if cfg!(windows) { "uvx.exe" } else { "uvx" }, &AtomicBool::new(false), move |r, t| say(&st, "Getting uv (once)", r as f64 / t.max(1) as f64)).await
}

/// The environment add-ons run with: their caches in SulcusAI's folder, and
/// the runtime first on the PATH (npx starts packages with `node`).
fn env_for(state: &AppState, bin: &std::path::Path) -> HashMap<String, String> {
    let cache = state.paths.engines.join("addons-cache");
    let path = format!("{};{}", bin.display(), std::env::var("PATH").unwrap_or_default());
    HashMap::from([
        ("PATH".to_string(), path),
        ("npm_config_cache".to_string(), cache.join("npm").display().to_string()),
        ("npm_config_update_notifier".to_string(), "false".to_string()),
        ("UV_CACHE_DIR".to_string(), cache.join("uv").display().to_string()),
        ("UV_PYTHON_INSTALL_DIR".to_string(), cache.join("python").display().to_string()),
        ("UV_PYTHON_PREFERENCE".to_string(), "only-managed".to_string()),
        ("UV_TOOL_DIR".to_string(), cache.join("uv-tools").display().to_string()),
    ])
}

/// How to start an add-on with the person's values.
pub fn spec_for(addon: &Addon, values: &HashMap<String, String>, node: Option<&std::path::Path>, uvx: Option<&std::path::Path>, env: HashMap<String, String>) -> Result<ServerSpec, String> {
    let mut args: Vec<String> = Vec::new();
    let mut env = env;
    let (command, lead): (String, Vec<String>) = match addon.run {
        Run::Node { package } | Run::Remote { url: package } => {
            let node = node.ok_or("Node.js isn't here.")?;
            let npx = node.parent().unwrap_or(node).join("node_modules").join("npm").join("bin").join("npx-cli.js");
            let lead = match addon.run {
                Run::Remote { url } => vec![npx.display().to_string(), "-y".into(), "mcp-remote".into(), url.to_string()],
                _ => vec![npx.display().to_string(), "-y".into(), package.to_string()],
            };
            (node.display().to_string(), lead)
        }
        Run::Uv { package } => (uvx.ok_or("uv isn't here.")?.display().to_string(), vec![package.to_string()]),
    };
    args.extend(lead);
    args.extend(addon.args.iter().map(|a| a.to_string()));
    for input in addon.inputs {
        let value = values.get(input.id).map(|v| v.trim().to_string()).filter(|v| !v.is_empty()).ok_or_else(|| format!("Fill in “{}”.", input.label))?;
        if matches!(input.kind, "folder" | "file") && !std::path::Path::new(&value).exists() {
            return Err(format!("{value} isn't there."));
        }
        match input.env {
            Some(var) => {
                env.insert(var.to_string(), value);
                // A service's token goes to it as a header (mcp-remote reads it from the environment).
                if matches!(addon.run, Run::Remote { .. }) {
                    args.extend(["--header".to_string(), format!("Authorization:Bearer ${{{var}}}")]);
                }
            }
            None => {
                if let Some(f) = input.flag {
                    args.push(f.to_string());
                }
                args.push(value);
            }
        }
    }
    Ok(ServerSpec { command, args, env, cwd: None })
}

/// Fetches an add-on's package before it first starts, so starting it
/// doesn't wait on the download (and its first connection doesn't time out).
async fn warm(addon: &Addon, spec: &ServerSpec) -> Result<(), String> {
    // npm installs the package and runs nothing ("node -e 0"); uvx the same with Python.
    let npm = || spec.args[0].replace("npx-cli.js", "npm-cli.js");
    let args: Vec<String> = match addon.run {
        Run::Node { package } => vec![npm(), "exec".into(), "--yes".into(), format!("--package={package}"), "--".into(), "node".into(), "-e".into(), "0".into()],
        Run::Remote { .. } => vec![npm(), "exec".into(), "--yes".into(), "--package=mcp-remote".into(), "--".into(), "node".into(), "-e".into(), "0".into()],
        Run::Uv { package } => vec!["--from".into(), package.into(), "python".into(), "-c".into(), "0".into()],
    };
    let mut cmd = tokio::process::Command::new(&spec.command);
    cmd.args(&args).envs(&spec.env).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    let out = tokio::time::timeout(std::time::Duration::from_secs(600), cmd.output()).await.map_err(|_| "Downloading it took more than 10 minutes.".to_string())?.map_err(|e| e.to_string())?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let last: Vec<&str> = err.lines().filter(|l| !l.trim().is_empty()).rev().take(3).collect();
        return Err(format!("It didn't download: {}", last.into_iter().rev().collect::<Vec<_>>().join(" ")));
    }
    Ok(())
}

// ---------- commands ----------

#[derive(Serialize)]
pub struct AddonView {
    #[serde(flatten)]
    addon: Addon,
    added: bool,
}

#[tauri::command]
pub fn addons_list(state: AppStateRef<'_>) -> Result<Vec<AddonView>, String> {
    let c = state.cipher()?;
    let have: Vec<String> = crate::connectors::connectors(&state.db.lock().unwrap(), &c).into_iter().map(|x| x.id).collect();
    Ok(ADDONS.iter().map(|a| AddonView { addon: *a, added: have.contains(&connector_id(a.id)) }).collect())
}

/// Adds an add-on: its runtime and package are downloaded the first time
/// ("addons:progress" says what's happening), then it's started once to
/// see its tools, and kept as a connector.
#[tauri::command]
pub async fn addon_add(state: AppStateRef<'_>, id: String, values: HashMap<String, String>) -> Result<crate::connectors::Connector, String> {
    crate::features::require(&state, crate::features::Feature::Connectors)?;
    let addon = find(&id).ok_or("Unknown add-on.")?;
    if !cfg!(windows) {
        return Err("One-click add-ons are Windows-only for now: add it as a connector with its own command.".into());
    }
    let state = state.inner().clone();
    let (node_exe, uvx_exe) = match addon.run {
        Run::Uv { .. } => (None, Some(uvx(&state).await?)),
        _ => (Some(node(&state).await?), None),
    };
    let bin = node_exe.as_deref().or(uvx_exe.as_deref()).and_then(|p| p.parent()).map(|p| p.to_path_buf()).unwrap_or_default();
    let spec = spec_for(addon, &values, node_exe.as_deref(), uvx_exe.as_deref(), env_for(&state, &bin))?;
    say(&state, &format!("Downloading {} (the first time)", addon.name), -1.0);
    warm(addon, &spec).await?;
    say(&state, &format!("Starting {}", addon.name), -1.0);
    if matches!(addon.run, Run::Remote { .. }) && addon.inputs.is_empty() {
        say(&state, &format!("Sign in to {} in the browser window that opens", addon.by), -1.0);
    }
    let saved = crate::connectors::save(&state, &connector_id(addon.id), addon.name, spec).await;
    say(&state, "", 1.0);
    let saved = saved?;
    state.log("connectors", &format!("Added the {} add-on ({} tools)", addon.name, saved.tools.len()));
    Ok(saved)
}

/// Starts an add-on the way `addon_add` does and lists its tools (for tests).
#[cfg(test)]
pub async fn tools_for_test(state: &Arc<AppState>, id: &str, values: &HashMap<String, String>) -> Result<Vec<String>, String> {
    let addon = find(id).ok_or("Unknown add-on.")?;
    let (node_exe, uvx_exe) = match addon.run {
        Run::Uv { .. } => (None, Some(uvx(state).await?)),
        _ => (Some(node(state).await?), None),
    };
    let bin = node_exe.as_deref().or(uvx_exe.as_deref()).and_then(|p| p.parent()).map(|p| p.to_path_buf()).unwrap_or_default();
    let spec = spec_for(addon, values, node_exe.as_deref(), uvx_exe.as_deref(), env_for(state, &bin))?;
    warm(addon, &spec).await?;
    let mut client = crate::mcp::Client::start(&spec, None).await?;
    let tools = client.list_tools().await?;
    client.stop().await;
    Ok(tools.into_iter().map(|t| t.name).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two add-ons started for real: Node.js and uv are downloaded once,
    /// each package fetched, started, and its tools listed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore]
    async fn e2e_add_ons_start_with_their_tools() {
        let state = crate::e2e::temp_state(None);
        for id in ["memory", "time"] {
            let started = std::time::Instant::now();
            let tools = tools_for_test(&state, id, &HashMap::new()).await.unwrap();
            println!("{id}: {} tools in {:.0} s: {:?}", tools.len(), started.elapsed().as_secs_f64(), tools);
            assert!(!tools.is_empty());
        }
    }

    #[test]
    fn add_ons_start_the_way_their_makers_say() {
        let node = std::path::Path::new(r"C:\x\node.exe");
        let dir = std::env::temp_dir();
        let values = HashMap::from([("folder".to_string(), dir.display().to_string())]);
        let s = spec_for(find("filesystem").unwrap(), &values, Some(node), None, HashMap::new()).unwrap();
        assert_eq!(s.command, node.display().to_string());
        assert!(s.args[0].ends_with("npx-cli.js"));
        assert_eq!(&s.args[1..3], ["-y", "@modelcontextprotocol/server-filesystem"]);
        assert_eq!(s.args.last().unwrap(), &dir.display().to_string());
        assert!(spec_for(find("filesystem").unwrap(), &HashMap::new(), Some(node), None, HashMap::new()).is_err(), "the folder is needed");
        let gh = spec_for(find("github").unwrap(), &HashMap::from([("token".to_string(), "ghp_x".to_string())]), Some(node), None, HashMap::new()).unwrap();
        assert!(gh.args.contains(&"mcp-remote".to_string()) && gh.args.contains(&"https://api.githubcopilot.com/mcp/".to_string()));
        assert_eq!(gh.env.get("AUTH_TOKEN").map(String::as_str), Some("ghp_x"));
        assert!(gh.args.contains(&"Authorization:Bearer ${AUTH_TOKEN}".to_string()), "{:?}", gh.args);
        let uv = std::path::Path::new(r"C:\x\uvx.exe");
        let git = spec_for(find("git").unwrap(), &HashMap::from([("repo".to_string(), dir.display().to_string())]), None, Some(uv), HashMap::new()).unwrap();
        assert_eq!(git.args, vec!["mcp-server-git".to_string(), "--repository".into(), dir.display().to_string()]);
        let ids: std::collections::HashSet<&str> = ADDONS.iter().map(|a| a.id).collect();
        assert_eq!(ids.len(), ADDONS.len(), "ids are unique");
    }
}

// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api, errorText, type Connector, type Plugin, type PluginPreview, type ToolMode } from "../api";
import { Modal } from "../components/Modal";
import { Addons } from "../components/Addons";
import type { PushToast } from "../components/Toasts";
import { t, tx } from "../i18n";

const MODES: { id: ToolMode; label: string }[] = [
  { id: "allow", label: tx("Allow") },
  { id: "ask", label: tx("Ask") },
  { id: "off", label: tx("Off") },
];

/** The host a connector's requests go to, when it isn't on this PC. */
export function webHost(c: Connector): string | null {
  if (!c.spec.url) return null;
  try {
    const host = new URL(c.spec.url).hostname.toLowerCase();
    return ["localhost", "127.0.0.1", "[::1]", "::1"].includes(host) ? null : host;
  } catch {
    return null;
  }
}

/** Mirrors `Connector::mode`: on the web even a read-only tool asks. */
export function modeOf(c: Connector, tool: string, readOnly: boolean): ToolMode {
  return c.tool_modes[tool] ?? (readOnly && !webHost(c) ? "allow" : "ask");
}

export function ConnectorsView({ toast }: { toast: PushToast }) {
  const [connectors, setConnectors] = useState<Connector[] | null>(null);
  const [plugins, setPlugins] = useState<Plugin[]>([]);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [editing, setEditing] = useState<Connector | "new" | null>(null);
  const [preview, setPreview] = useState<{ path: string; info: PluginPreview } | null>(null);
  const [checksOk, setChecksOk] = useState(false);
  const [checkProblems, setCheckProblems] = useState<Record<string, string>>({});

  const load = () => {
    api.connectors().then(setConnectors).catch((e) => toast(errorText(e), "error"));
    api.plugins().then(setPlugins).catch(() => {});
    api.pluginCheckProblems().then(setCheckProblems).catch(() => {});
  };
  useEffect(load, []); // eslint-disable-line react-hooks/exhaustive-deps

  const choosePlugin = async () => {
    const path = await open({ directory: true, multiple: false, title: t("Choose a plugin or skill folder (with plugin.json or SKILL.md)") });
    if (typeof path !== "string") return;
    try {
      setChecksOk(false);
      setPreview({ path, info: await api.inspectPlugin(path) });
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  const own = (connectors ?? []).filter((c) => !c.plugin);
  return (
    <div className="page">
      <div className="narrow">
        <header className="page-head">
          <div>
            <h1>{t("Connectors & plugins")}</h1>
            <p className="muted">
              {t("Connectors give the assistant extra tools through small programs on this PC (MCP servers). Plugins bundle skills (instructions it can follow) and connectors.")}
            </p>
          </div>
        </header>
        <p className="callout small">
          {t("Connectors are programs that run on this PC with your permissions. SulcusAI decides when the assistant may use their tools, but not what a connector does by itself, so only add ones you trust.")}
        </p>

        <Addons toast={toast} onAdded={load} />

        <section className="card">
          <div className="section-head">
            <h2>{t("Local connectors")}</h2>
            <button className="btn small" onClick={() => setEditing("new")}>{t("+ Add connector")}</button>
          </div>
          {connectors !== null && own.length === 0 && <p className="muted small">{t("None yet.")}</p>}
          {own.map((c) => (
            <ConnectorRow
              key={c.id}
              c={c}
              open={expanded === c.id}
              onToggleOpen={() => setExpanded(expanded === c.id ? null : c.id)}
              onEdit={() => setEditing(c)}
              onChanged={load}
              toast={toast}
            />
          ))}
        </section>

        <section className="card">
          <div className="section-head">
            <h2>{t("Plugins")}</h2>
            <button className="btn small" onClick={choosePlugin}>{t("+ Install from folder…")}</button>
          </div>
          {plugins.length === 0 && <p className="muted small">{t("None installed.")}</p>}
          <p className="muted small">{t("Skills made for Claude (a folder with SKILL.md, or a folder of them) and Claude Code plugins install as they are: their skills, the files those skills use, and their connectors.")}</p>
          {plugins.map((p) => (
            <div key={p.id} className="plugin">
              <div className="setting-row first">
                <div>
                  <strong>{p.name}</strong> {p.version && <span className="muted small">v{p.version}</span>}
                  <span className="small muted block">{p.description}</span>
                  {p.skills.length > 0 && <span className="small block">{t("Skills: {names}", { names: p.skills.map((s) => s.name).join(", ") })}</span>}
                  {p.checks.length > 0 && <span className="small block">{p.checks.length === 1 ? t("Runs 1 check during replies") : t("Runs {n} checks during replies", { n: p.checks.length })}</span>}
                  {checkProblems[p.name] && <span className="small block warn-text">{checkProblems[p.name]}</span>}
                </div>
                <div className="row">
                  <label className="switch" title={p.enabled ? t("On") : t("Off")}>
                    <input
                      type="checkbox"
                      checked={p.enabled}
                      onChange={async (e) => {
                        await api.setPluginEnabled(p.id, e.target.checked);
                        load();
                      }}
                      aria-label={t("{name} on or off", { name: p.name })}
                    />
                    <span />
                  </label>
                  <button
                    className="btn ghost danger small"
                    onClick={async () => {
                      await api.removePlugin(p.id);
                      load();
                    }}
                  >
                    {t("Remove")}
                  </button>
                </div>
              </div>
              {(connectors ?? [])
                .filter((c) => c.plugin === p.id)
                .map((c) => (
                  <ConnectorRow key={c.id} c={c} open={expanded === c.id} onToggleOpen={() => setExpanded(expanded === c.id ? null : c.id)} onChanged={load} toast={toast} />
                ))}
            </div>
          ))}
        </section>
      </div>

      {editing && (
        <ConnectorEditor
          initial={editing === "new" ? null : editing}
          onClose={() => setEditing(null)}
          onSaved={(c) => {
            setEditing(null);
            setExpanded(c.id);
            load();
            toast(c.tools.length === 1 ? t("Connected. 1 tool available.") : t("Connected. {n} tools available.", { n: c.tools.length }), "success");
          }}
          toast={toast}
        />
      )}
      {preview && (
        <Modal title={t("Install {name}?", { name: preview.info.name })} onClose={() => setPreview(null)}>
          {preview.info.description && <p>{preview.info.description}</p>}
          {preview.info.skills.length > 0 && (
            <>
              <strong className="small">{t("Skills")}</strong>
              <ul className="small">{preview.info.skills.map((s) => <li key={s}>{s}</li>)}</ul>
            </>
          )}
          {preview.info.programs.length > 0 ? (
            <>
              <strong className="small">{t("Programs it will run on this PC")}</strong>
              <pre className="tool-output boxed">{preview.info.programs.join("\n")}</pre>
            </>
          ) : (
            <p className="small muted">{t("It doesn't run any programs.")}</p>
          )}
          {preview.info.checks.length > 0 && (
            <>
              <strong className="small">{t("This plugin runs these commands on your PC")}</strong>
              <p className="small muted">{t("At these points of each reply, with what the assistant is doing. A command can stop a step, have you asked first, or tell the assistant something. They don't run in Plan mode or in incognito chats.")}</p>
              <pre className="tool-output boxed">{preview.info.checks.join("\n")}</pre>
              <label className="check small">
                <input type="checkbox" checked={checksOk} onChange={(e) => setChecksOk(e.target.checked)} />
                <span>{t("I trust these commands to run on this PC")}</span>
              </label>
            </>
          )}
          <div className="modal-actions">
            <button className="btn" onClick={() => setPreview(null)}>{t("Cancel")}</button>
            <button
              className="btn primary"
              disabled={preview.info.checks.length > 0 && !checksOk}
              onClick={async () => {
                try {
                  await api.installPlugin(preview.path, checksOk);
                  setPreview(null);
                  load();
                  toast(t("Plugin installed."), "success");
                } catch (e) {
                  toast(errorText(e), "error");
                }
              }}
            >
              {t("Install")}
            </button>
          </div>
        </Modal>
      )}
    </div>
  );
}

function ConnectorRow({ c, open, onToggleOpen, onEdit, onChanged, toast }: { c: Connector; open: boolean; onToggleOpen: () => void; onEdit?: () => void; onChanged: () => void; toast: PushToast }) {
  const host = webHost(c);
  const [signing, setSigning] = useState(false);
  const sign = async (into: boolean) => {
    setSigning(true);
    try {
      await (into ? api.connectorSignIn(c.id) : api.connectorSignOut(c.id));
      onChanged();
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setSigning(false);
    }
  };
  return (
    <div className="connector">
      <div className="setting-row">
        <button className="tool-head" onClick={onToggleOpen} aria-expanded={open}>
          <span>
            <strong>{c.name}</strong>
            <span className="small muted block">
              {c.tools.length === 1 ? t("1 tool") : t("{n} tools", { n: c.tools.length })} · <code>{c.spec.url ? c.spec.url : `${c.spec.command} ${c.spec.args.join(" ")}`}</code>
            </span>
          </span>
          <span className="chev" aria-hidden>{open ? "▾" : "▸"}</span>
        </button>
        <div className="row">
          {onEdit && <button className="btn ghost small" onClick={onEdit}>{t("Edit")}</button>}
          <label className="switch" title={c.enabled ? t("On") : t("Off")}>
            <input
              type="checkbox"
              checked={c.enabled}
              onChange={async (e) => {
                await api.setConnectorEnabled(c.id, e.target.checked);
                onChanged();
              }}
              aria-label={t("{name} on or off", { name: c.name })}
            />
            <span />
          </label>
          {onEdit && (
            <button
              className="btn ghost danger small"
              onClick={async () => {
                await api.deleteConnector(c.id);
                onChanged();
              }}
            >
              {t("Remove")}
            </button>
          )}
        </div>
      </div>
      {c.spec.url && (
        <div className="row wrap small">
          {c.auth ? (
            <>
              <span className="muted">{c.auth.account ? t("Signed in as {account}", { account: c.auth.account }) : t("Signed in")}</span>
              <button className="btn ghost small" disabled={signing} onClick={() => sign(false)}>
                {t("Sign out")}
              </button>
            </>
          ) : (
            <button className="btn small" disabled={signing} onClick={() => sign(true)} title={t("Opens the connector's sign-in page in your browser")}>
              {signing ? t("Waiting for the browser…") : t("Sign in to {name}", { name: c.name })}
            </button>
          )}
          {host && <span className="muted">{t("Each call asks first, since what it's given goes to {host}. Allow a tool to stop being asked.", { host })}</span>}
        </div>
      )}
      {open && (
        <ul className="tool-modes">
          {c.tools.length === 0 && <li className="muted small">{t("No tools seen yet. Turn it on and start a chat, or edit and save it to connect.")}</li>}
          {c.tools.map((tool) => (
            <li key={tool.name}>
              <span>
                <strong className="small">{tool.name}</strong>
                {tool.read_only && <span className="tag">{t("read-only")}</span>}
                <span className="small muted block">{tool.description}</span>
              </span>
              <span className="mode-switch small-switch">
                {MODES.map((m) => (
                  <button
                    key={m.id}
                    title={host && m.id === "allow" ? t("Runs without asking: what it's given goes to {host}", { host }) : undefined}
                    className={`mode ${modeOf(c, tool.name, tool.read_only) === m.id ? "active" : ""}`}
                    onClick={async () => {
                      try {
                        await api.setToolMode(c.id, tool.name, m.id);
                        onChanged();
                      } catch (e) {
                        toast(errorText(e), "error");
                      }
                    }}
                  >
                    {t(m.label)}
                  </button>
                ))}
              </span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

function ConnectorEditor({ initial, onClose, onSaved, toast }: { initial: Connector | null; onClose: () => void; onSaved: (c: Connector) => void; toast: PushToast }) {
  const [name, setName] = useState(initial?.name ?? "");
  const [command, setCommand] = useState(initial?.spec.command ?? "");
  const [args, setArgs] = useState((initial?.spec.args ?? []).join("\n"));
  const [env, setEnv] = useState(Object.entries(initial?.spec.env ?? {}).map(([k, v]) => `${k}=${v}`).join("\n"));
  // A program on this PC, or a server at an address.
  const [remote, setRemote] = useState(!!initial?.spec.url);
  const [url, setUrl] = useState(initial?.spec.url ?? "");
  const [headers, setHeaders] = useState(Object.entries(initial?.spec.headers ?? {}).map(([k, v]) => `${k}: ${v}`).join("\n"));
  const [saving, setSaving] = useState(false);

  const save = async () => {
    setSaving(true);
    try {
      const headerMap: Record<string, string> = {};
      for (const line of headers.split("\n").map((l) => l.trim()).filter(Boolean)) {
        const i = line.indexOf(":");
        if (i > 0) headerMap[line.slice(0, i).trim()] = line.slice(i + 1).trim();
      }
      const envMap: Record<string, string> = {};
      for (const line of env.split("\n").map((l) => l.trim()).filter(Boolean)) {
        const i = line.indexOf("=");
        if (i > 0) envMap[line.slice(0, i).trim()] = line.slice(i + 1);
      }
      const c = await api.saveConnector(
        remote
          ? { id: initial?.id ?? "", name, command: "", args: [], env: {}, url: url.trim(), headers: headerMap }
          : { id: initial?.id ?? "", name, command, args: args.split("\n").map((a) => a.trim()).filter(Boolean), env: envMap },
      );
      onSaved(c);
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setSaving(false);
    }
  };

  return (
    <Modal title={initial ? t("Edit {name}", { name: initial.name }) : t("Add a connector")} onClose={onClose}>
      <form
        className="form"
        onSubmit={(e) => {
          e.preventDefault();
          save();
        }}
      >
        <label>
          {t("Name")}
          <input className="input" value={name} onChange={(e) => setName(e.target.value)} placeholder={t("files, calendar, notes…")} aria-label={t("files, calendar, notes…")} maxLength={40} autoFocus />
        </label>
        <div className="filters" role="radiogroup" aria-label={t("Where the connector is")}>
          <button type="button" role="radio" aria-checked={!remote} className={`chip ${!remote ? "active" : ""}`} onClick={() => setRemote(false)}>
            {t("A program on this PC")}
          </button>
          <button type="button" role="radio" aria-checked={remote} className={`chip ${remote ? "active" : ""}`} onClick={() => setRemote(true)}>
            {t("A server at an address")}
          </button>
        </div>
        {remote ? (
          <>
            <label>
              {t("Address")}
              <input className="input" value={url} onChange={(e) => setUrl(e.target.value)} placeholder="https://example.com/mcp" aria-label={t("Address")} />
            </label>
            <label>
              {t("Headers")} <span className="muted small">{t("(Name: value per line, such as a key; stored encrypted)")}</span>
              <textarea className="input mono" rows={2} value={headers} onChange={(e) => setHeaders(e.target.value)} placeholder="Authorization: Bearer …" />
            </label>
            <p className="muted small">{t("Saving connects once to list its tools. Its tools send what the assistant gives them to that address, so it needs web access (Local AI + Web or Cloud). Plain http works only for servers on this PC.")}</p>
          </>
        ) : (
          <>
            <label>
              {t("Command")}
              <input className="input" value={command} onChange={(e) => setCommand(e.target.value)} placeholder={t("npx, uvx, python, or a program path")} aria-label={t("npx, uvx, python, or a program path")} />
            </label>
            <label>
              {t("Arguments")} <span className="muted small">{t("(one per line)")}</span>
              <textarea className="input mono" rows={3} value={args} onChange={(e) => setArgs(e.target.value)} placeholder={"-y\n@modelcontextprotocol/server-everything"} />
            </label>
            <label>
              {t("Environment variables")} <span className="muted small">{t("(KEY=value per line; stored encrypted)")}</span>
              <textarea className="input mono" rows={2} value={env} onChange={(e) => setEnv(e.target.value)} />
            </label>
            <p className="muted small">{t("Saving starts the program once to check that it works and to list its tools.")}</p>
          </>
        )}
        <div className="modal-actions">
          <button type="button" className="btn" onClick={onClose}>{t("Cancel")}</button>
          <button type="submit" className="btn primary" disabled={saving || !name.trim() || (remote ? !url.trim() : !command.trim())}>
            {saving ? t("Connecting…") : t("Save and connect")}
          </button>
        </div>
      </form>
    </Modal>
  );
}

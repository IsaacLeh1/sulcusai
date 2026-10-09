// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api, errorText, type Connector, type Plugin, type PluginPreview, type ToolMode } from "../api";
import { Modal } from "../components/Modal";
import type { PushToast } from "../components/Toasts";
import { t, tx } from "../i18n";

const MODES: { id: ToolMode; label: string }[] = [
  { id: "allow", label: tx("Allow") },
  { id: "ask", label: tx("Ask") },
  { id: "off", label: tx("Off") },
];

function modeOf(c: Connector, tool: string, readOnly: boolean): ToolMode {
  return c.tool_modes[tool] ?? (readOnly ? "allow" : "ask");
}

export function ConnectorsView({ toast }: { toast: PushToast }) {
  const [connectors, setConnectors] = useState<Connector[] | null>(null);
  const [plugins, setPlugins] = useState<Plugin[]>([]);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [editing, setEditing] = useState<Connector | "new" | null>(null);
  const [preview, setPreview] = useState<{ path: string; info: PluginPreview } | null>(null);

  const load = () => {
    api.connectors().then(setConnectors).catch((e) => toast(errorText(e), "error"));
    api.plugins().then(setPlugins).catch(() => {});
  };
  useEffect(load, []); // eslint-disable-line react-hooks/exhaustive-deps

  const choosePlugin = async () => {
    const path = await open({ directory: true, multiple: false, title: t("Choose a plugin folder (it contains plugin.json)") });
    if (typeof path !== "string") return;
    try {
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
              {t("Connectors give the assistant extra tools through MCP servers running on this PC. Plugins bundle skills (instructions it can follow) and connectors.")}
            </p>
          </div>
        </header>
        <p className="callout small">
          {t("Connectors are programs that run on this PC with your permissions. SulcusAI decides when the assistant may use their tools, but not what a connector does by itself, so only add ones you trust. Internet-based connectors arrive with the web features.")}
        </p>

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
          {plugins.map((p) => (
            <div key={p.id} className="plugin">
              <div className="setting-row first">
                <div>
                  <strong>{p.name}</strong> {p.version && <span className="muted small">v{p.version}</span>}
                  <span className="small muted block">{p.description}</span>
                  {p.skills.length > 0 && <span className="small block">{t("Skills: {names}", { names: p.skills.map((s) => s.name).join(", ") })}</span>}
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
          <div className="modal-actions">
            <button className="btn" onClick={() => setPreview(null)}>{t("Cancel")}</button>
            <button
              className="btn primary"
              onClick={async () => {
                try {
                  await api.installPlugin(preview.path);
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
  return (
    <div className="connector">
      <div className="setting-row">
        <button className="tool-head" onClick={onToggleOpen} aria-expanded={open}>
          <span aria-hidden>🔌</span>
          <span>
            <strong>{c.name}</strong>
            <span className="small muted block">
              {c.tools.length === 1 ? t("1 tool") : t("{n} tools", { n: c.tools.length })} · <code>{c.spec.command} {c.spec.args.join(" ")}</code>
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
  const [saving, setSaving] = useState(false);

  const save = async () => {
    setSaving(true);
    try {
      const envMap: Record<string, string> = {};
      for (const line of env.split("\n").map((l) => l.trim()).filter(Boolean)) {
        const i = line.indexOf("=");
        if (i > 0) envMap[line.slice(0, i).trim()] = line.slice(i + 1);
      }
      const c = await api.saveConnector({
        id: initial?.id ?? "",
        name,
        command,
        args: args.split("\n").map((a) => a.trim()).filter(Boolean),
        env: envMap,
      });
      onSaved(c);
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setSaving(false);
    }
  };

  return (
    <Modal title={initial ? t("Edit {name}", { name: initial.name }) : t("Add a local connector")} onClose={onClose}>
      <form
        className="form"
        onSubmit={(e) => {
          e.preventDefault();
          save();
        }}
      >
        <label>
          {t("Name")}
          <input className="input" value={name} onChange={(e) => setName(e.target.value)} placeholder={t("files, calendar, notes…")} maxLength={40} autoFocus />
        </label>
        <label>
          {t("Command")}
          <input className="input" value={command} onChange={(e) => setCommand(e.target.value)} placeholder={t("npx, uvx, python, or a program path")} />
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
        <div className="modal-actions">
          <button type="button" className="btn" onClick={onClose}>{t("Cancel")}</button>
          <button type="submit" className="btn primary" disabled={saving || !name.trim() || !command.trim()}>
            {saving ? t("Connecting…") : t("Save and connect")}
          </button>
        </div>
      </form>
    </Modal>
  );
}

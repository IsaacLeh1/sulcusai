// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { api, errorText, type Allowance, type Project } from "../api";
import { t, tx } from "../i18n";
import type { PushToast } from "./Toasts";

const EFFECTS: { id: Allowance["effect"]; label: string }[] = [
  { id: "allow", label: tx("Allow") },
  { id: "ask", label: tx("Ask first") },
  { id: "deny", label: tx("Never") },
];

const COMMON_TOOLS = ["run_command", "read_file", "write_file", "edit_file", "delete_path", "move_path", "list_dir", "fetch_page", "browser_*", "program_*", "email_send"];

type Draft = { tool: string; pattern: string; effect: Allowance["effect"]; project: string | null };

/** One rule being written: the tool, what it acts on, and (in Settings) the
 * effect and where it holds. The approval card fixes the tool and effect. */
export function RuleForm({
  initial,
  projects,
  fixed,
  onSaved,
  onCancel,
}: {
  initial: Draft;
  projects: Project[];
  /** From the approval card: the tool and "Allow" can't change, and the scope is this project or everywhere. */
  fixed?: boolean;
  onSaved: (a: Allowance) => void;
  onCancel?: () => void;
}) {
  const [d, setD] = useState<Draft>(initial);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const save = async () => {
    setBusy(true);
    setError(null);
    try {
      onSaved(await api.saveAllowance({ tool: d.tool.trim(), pattern: d.pattern.trim() || null, effect: d.effect, project: d.project }));
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };
  const ownProject = initial.project ? projects.find((p) => p.id === initial.project) : undefined;
  return (
    <div className="rule-form">
      {fixed ? (
        <p className="small">
          {t("Always allow {tool}", { tool: d.tool })}
        </p>
      ) : (
        <label className="adv-field">
          <span>{t("Tool")}</span>
          <input className="input" list="rule-tools" value={d.tool} placeholder="run_command" onChange={(e) => setD({ ...d, tool: e.target.value })} />
          <datalist id="rule-tools">
            {COMMON_TOOLS.map((name) => (
              <option key={name} value={name} />
            ))}
          </datalist>
        </label>
      )}
      <label className="adv-field">
        <span>{t("What it acts on")}</span>
        <input className="input" value={d.pattern} placeholder={t("Empty: every use of the tool")} onChange={(e) => setD({ ...d, pattern: e.target.value })} />
        <span className="muted small">{t("A command, a file or folder, or a web address. In paths * is anything within one folder and ** any folders; elsewhere * is anything.")}</span>
      </label>
      {!fixed && (
        <label className="adv-field">
          <span>{t("Then")}</span>
          <select className="input" value={d.effect} onChange={(e) => setD({ ...d, effect: e.target.value as Allowance["effect"] })}>
            {EFFECTS.map((x) => (
              <option key={x.id} value={x.id}>
                {t(x.label)}
              </option>
            ))}
          </select>
        </label>
      )}
      {fixed ? (
        ownProject && (
          <label className="check">
            <input type="checkbox" checked={d.project !== null} onChange={(e) => setD({ ...d, project: e.target.checked ? ownProject.id : null })} />
            <span>{t("Only in the {project} project", { project: ownProject.name })}</span>
          </label>
        )
      ) : (
        <label className="adv-field">
          <span>{t("Where")}</span>
          <select className="input" value={d.project ?? ""} onChange={(e) => setD({ ...d, project: e.target.value || null })}>
            <option value="">{t("Every chat")}</option>
            {projects.map((p) => (
              <option key={p.id} value={p.id}>
                {t("Chats in {project}", { project: p.name })}
              </option>
            ))}
          </select>
        </label>
      )}
      {error && <p className="warn-text small">{error}</p>}
      <div className="row">
        <button className="btn primary small" disabled={busy || !d.tool.trim()} onClick={save}>
          {fixed ? t("Save and allow") : t("Add rule")}
        </button>
        {onCancel && (
          <button className="btn ghost small" disabled={busy} onClick={onCancel}>
            {t("Cancel")}
          </button>
        )}
      </div>
    </div>
  );
}

/** Settings › Permissions: the saved rules, and adding one. */
export function PermissionRules({ toast }: { toast: PushToast }) {
  const [rules, setRules] = useState<Allowance[] | null>(null);
  const [projects, setProjects] = useState<Project[]>([]);
  const [adding, setAdding] = useState(false);
  const load = () => api.allowances().then(setRules).catch((e) => toast(errorText(e), "error"));
  useEffect(() => {
    load();
    api.projects().then(setProjects).catch(() => {});
  }, []);
  const remove = async (id: string) => {
    try {
      await api.removeAllowance(id);
      load();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const effect = (e: Allowance["effect"]) => t(EFFECTS.find((x) => x.id === e)?.label ?? e);
  const where = (p: string | null) => (p ? t("Chats in {project}", { project: projects.find((x) => x.id === p)?.name ?? p }) : t("Every chat"));
  return (
    <section className="card">
      <h2>{t("Permissions")}</h2>
      <p className="muted small">
        {t(
          "Rules for what the assistant may do without asking, in any run mode. Never wins over Ask first, and Ask first over Allow. Allow never skips the questions that always come up: sending, buying or deleting, anything leaving this PC, and commands that are always refused. A command is allowed only when every part of it is. Rules stay on this PC.",
        )}
      </p>
      {rules && rules.length === 0 && <p className="muted small">{t("No rules yet. “Always allow…” on an approval card adds one too.")}</p>}
      {rules && rules.length > 0 && (
        <ul className="teach-list small">
          {rules.map((r) => (
            <li key={r.id}>
              <span className="ellipsis">
                <strong>{effect(r.effect)}</strong> <code>{r.tool}</code> {r.pattern ? <code title={r.pattern}>{r.pattern}</code> : <span className="muted">{t("(every use)")}</span>}
                <span className="muted block">
                  {where(r.project)} · {new Date(r.created_at).toLocaleDateString()}
                </span>
              </span>
              <button className="btn ghost small" onClick={() => remove(r.id)}>
                {t("Delete")}
              </button>
            </li>
          ))}
        </ul>
      )}
      {adding ? (
        <RuleForm
          initial={{ tool: "", pattern: "", effect: "deny", project: null }}
          projects={projects}
          onSaved={() => {
            setAdding(false);
            load();
          }}
          onCancel={() => setAdding(false)}
        />
      ) : (
        <button className="btn small" onClick={() => setAdding(true)}>
          {t("Add rule")}
        </button>
      )}
    </section>
  );
}

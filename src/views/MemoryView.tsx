// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useMemo, useState } from "react";
import { api, errorText, on, type Memory, type Project, type Settings } from "../api";
import { Modal } from "../components/Modal";
import type { PushToast } from "../components/Toasts";
import { t } from "../i18n";

export function MemoryView({ settings, onSettings, toast }: { settings: Settings; onSettings: (s: Settings) => void; toast: PushToast }) {
  const [memories, setMemories] = useState<Memory[] | null>(null);
  const [projects, setProjects] = useState<Project[]>([]);
  const [query, setQuery] = useState("");
  const [editing, setEditing] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [adding, setAdding] = useState("");
  const [confirmClear, setConfirmClear] = useState(false);

  const load = () => {
    api.memories().then(setMemories).catch((e) => toast(errorText(e), "error"));
    api.projects().then(setProjects).catch(() => {});
  };
  useEffect(() => {
    load();
    const sub = on("sync:changed", () => load());
    return () => {
      sub.then((un) => un());
    };
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  const projectName = (id: string | null) => (id ? projects.find((p) => p.id === id)?.name ?? t("A deleted project") : null);
  const shown = useMemo(() => {
    const q = query.trim().toLowerCase();
    return (memories ?? []).filter((m) => !q || m.content.toLowerCase().includes(q));
  }, [memories, query]);

  const save = async (id: string) => {
    try {
      await api.updateMemory(id, draft);
      setEditing(null);
      load();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  return (
    <div className="page">
      <div className="narrow">
        <header className="page-head">
          <div>
            <h1>{t("Memory")}</h1>
            <p className="muted">{t("Things the assistant remembers across chats. It saves them when you share something lasting, or you can add your own. Stored encrypted on this PC.")}</p>
          </div>
        </header>

        <section className="card">
          <div className="setting-row first">
            <div>
              <strong>{t("Use memory")}</strong>
              <span className="small muted block">
                {settings.memory_enabled ? t("On: chats can recall and save memories.") : t("Off: chats neither recall nor save anything.")} {t("Incognito chats never do.")}
              </span>
            </div>
            <label className="switch">
              <input
                type="checkbox"
                checked={settings.memory_enabled}
                onChange={async (e) => onSettings(await api.setMemoryEnabled(e.target.checked))}
                aria-label={t("Use memory")}
              />
              <span />
            </label>
          </div>
        </section>

        <form
          className="memory-add"
          onSubmit={async (e) => {
            e.preventDefault();
            if (!adding.trim()) return;
            try {
              await api.addMemory(adding, null);
              setAdding("");
              load();
            } catch (err) {
              toast(errorText(err), "error");
            }
          }}
        >
          <input className="input" placeholder={t("Add something to remember, e.g. I prefer metric units")} value={adding} onChange={(e) => setAdding(e.target.value)} maxLength={500} />
          <button className="btn primary" type="submit" disabled={!adding.trim()}>{t("Add")}</button>
        </form>

        <div className="filters">
          <input className="input search" placeholder={t("Search memories")} value={query} onChange={(e) => setQuery(e.target.value)} aria-label={t("Search memories")} />
        </div>

        <section className="card">
          {memories === null && <p className="muted">{t("Loading…")}</p>}
          {memories !== null && shown.length === 0 && <p className="muted">{memories.length ? t("No memories match.") : t("Nothing remembered yet.")}</p>}
          <ul className="memory-list">
            {shown.map((m) => (
              <li key={m.id}>
                {editing === m.id ? (
                  <form
                    className="memory-edit"
                    onSubmit={(e) => {
                      e.preventDefault();
                      save(m.id);
                    }}
                  >
                    <input className="input" value={draft} onChange={(e) => setDraft(e.target.value)} autoFocus maxLength={500} />
                    <button className="btn primary small" type="submit">{t("Save")}</button>
                    <button className="btn ghost small" type="button" onClick={() => setEditing(null)}>{t("Cancel")}</button>
                  </form>
                ) : (
                  <>
                    <span className="memory-text">{m.content}</span>
                    <span className="tag">{projectName(m.project_id) ?? t("Everywhere")}</span>
                    <button
                      className="link"
                      onClick={() => {
                        setEditing(m.id);
                        setDraft(m.content);
                      }}
                    >
                      {t("Edit")}
                    </button>
                    <button
                      className="link danger"
                      onClick={async () => {
                        await api.deleteMemory(m.id);
                        load();
                      }}
                    >
                      {t("Delete")}
                    </button>
                  </>
                )}
              </li>
            ))}
          </ul>
        </section>
        {memories && memories.length > 0 && (
          <button className="btn ghost danger" onClick={() => setConfirmClear(true)}>{t("Delete all memories")}</button>
        )}
      </div>
      {confirmClear && (
        <Modal title={t("Delete all memories?")} onClose={() => setConfirmClear(false)}>
          <p>{t("The assistant will forget everything it remembered, in every project. This can't be undone.")}</p>
          <div className="modal-actions">
            <button className="btn" onClick={() => setConfirmClear(false)}>{t("Cancel")}</button>
            <button
              className="btn danger-fill"
              onClick={async () => {
                await api.clearMemories();
                setConfirmClear(false);
                load();
              }}
            >
              {t("Delete all")}
            </button>
          </div>
        </Modal>
      )}
    </div>
  );
}

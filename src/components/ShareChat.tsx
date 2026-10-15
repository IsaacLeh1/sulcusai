// SPDX-License-Identifier: AGPL-3.0-only
// Share a chat with people linked in People: they can watch it live, read-only.
import { useEffect, useState } from "react";
import { api, errorText, on, type SharingChoice } from "../api";
import { Modal } from "./Modal";
import type { PushToast } from "./Toasts";
import { t } from "../i18n";

export function ShareChat({ chatId, toast }: { chatId: string; toast: PushToast }) {
  const [list, setList] = useState<SharingChoice[] | null>(null);
  const [open, setOpen] = useState(false);
  const [picked, setPicked] = useState<Set<string>>(new Set());

  useEffect(() => {
    const load = () => api.peopleChatSharing(chatId).then(setList).catch(() => setList([]));
    load();
    const sub = on("people:changed", load);
    return () => {
      sub.then((un) => un());
    };
  }, [chatId]);
  if (!list || list.length === 0) return null;
  const shared = list.filter((p) => p.shared);

  const save = async () => {
    try {
      setList(await api.peopleShareChat(chatId, [...picked]));
      setOpen(false);
      toast(picked.size ? t("Shared. They can watch this chat while you're both connected.") : t("This chat isn't shared anymore."), "success");
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  return (
    <>
      <button
        className={`btn ghost small ${shared.length ? "shared-on" : ""}`}
        title={shared.length ? t("Shared with {names}", { names: shared.map((p) => p.name).join(", ") }) : t("Let people you're linked with watch this chat")}
        onClick={() => {
          setPicked(new Set(shared.map((p) => p.id)));
          setOpen(true);
        }}
      >
        👥 {shared.length ? t("Shared · {n}", { n: shared.length }) : t("Share")}
      </button>
      {open && (
        <Modal title={t("Share this chat")} onClose={() => setOpen(false)}>
          <p className="muted small">
            {t("They can open it in People and watch it live, read-only, while you're both connected: your messages, the replies as they're written, and the steps the assistant takes (not what its tools found). Nothing is kept on their PC.")}
          </p>
          <div className="share-list">
            {list.map((p) => (
              <label key={p.id} className="check">
                <input
                  type="checkbox"
                  checked={picked.has(p.id)}
                  onChange={(e) => {
                    const next = new Set(picked);
                    if (e.target.checked) next.add(p.id);
                    else next.delete(p.id);
                    setPicked(next);
                  }}
                />
                <span>
                  {p.name} <span className="muted small">{p.online ? t("connected") : t("not connected")}</span>
                </span>
              </label>
            ))}
          </div>
          <div className="adv-actions">
            <button className="btn primary" onClick={save}>
              {t("Save")}
            </button>
            <button className="btn ghost" onClick={() => setOpen(false)}>
              {t("Cancel")}
            </button>
          </div>
        </Modal>
      )}
    </>
  );
}

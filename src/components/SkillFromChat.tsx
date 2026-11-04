// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { errorText } from "../api";
import { t } from "../i18n";
import { Modal } from "./Modal";
import type { PushToast } from "./Toasts";

interface Skill {
  name: string;
  description: string;
  body: string;
  dir: string | null;
  files: string[];
}

/** Turns a chat into a skill: the model drafts it, the person edits and saves it. */
export function SkillFromChat({ chatId, onClose, toast }: { chatId: string; onClose: () => void; toast: PushToast }) {
  const [draft, setDraft] = useState<Skill | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    invoke<Skill>("skill_from_chat", { chatId })
      .then(setDraft)
      .catch((e) => setProblem(errorText(e)));
  }, [chatId]);

  const save = async () => {
    if (!draft) return;
    setSaving(true);
    try {
      await invoke("save_own_skill", { skill: draft });
      toast(t("Saved to My skills. The assistant uses it when a request matches."), "success");
      onClose();
    } catch (e) {
      setProblem(errorText(e));
      setSaving(false);
    }
  };

  return (
    <Modal title={t("Make a skill from this chat")} onClose={() => !saving && onClose()}>
      {!draft && !problem && <p className="muted">{t("Reading the chat and writing the skill…")}</p>}
      {problem && <p className="error-text">{problem}</p>}
      {draft && (
        <div className="form">
          <p className="small muted">{t("Check it before saving: this is what the assistant will follow next time.")}</p>
          <label>
            {t("Name")}
            <input className="input" value={draft.name} maxLength={60} onChange={(e) => setDraft({ ...draft, name: e.target.value })} />
          </label>
          <label>
            {t("When to use it")}
            <input className="input" value={draft.description} maxLength={240} onChange={(e) => setDraft({ ...draft, description: e.target.value })} />
          </label>
          <label>
            {t("Instructions")}
            <textarea className="input" rows={10} value={draft.body} onChange={(e) => setDraft({ ...draft, body: e.target.value })} />
          </label>
        </div>
      )}
      <div className="modal-actions">
        <button className="btn" onClick={onClose} disabled={saving}>{t("Cancel")}</button>
        <button className="btn primary" onClick={save} disabled={!draft || saving || !draft.name.trim() || !draft.body.trim()}>
          {t("Save skill")}
        </button>
      </div>
    </Modal>
  );
}

// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState, type KeyboardEvent } from "react";
import { api, errorText, type QuickSettings } from "../api";
import { t } from "../i18n";
import { ModelSelect } from "./ModelChoices";
import type { PushToast } from "./Toasts";

/** "ctrl+alt+KeyK" from a key press, or null for a modifier on its own. */
function combo(e: KeyboardEvent): string | null {
  if (["Control", "Alt", "Shift", "Meta", "OS"].includes(e.key)) return null;
  const mods = [e.ctrlKey && "ctrl", e.altKey && "alt", e.shiftKey && "shift", e.metaKey && "super"].filter(Boolean);
  return [...mods, e.code].join("+");
}

/** Dictating into any app: a hotkey starts listening, and stops it. */
export function DictateAnywhere({ toast }: { toast: PushToast }) {
  const [s, setS] = useState<QuickSettings | null>(null);
  useEffect(() => {
    api.quickSettings().then(setS).catch(() => {});
  }, []);
  if (!s) return null;
  return (
    <label className="check small quick-setup">
      <input
        type="checkbox"
        checked={!!s.dictate_anywhere}
        onChange={async (e) => {
          try {
            setS(await api.setDictateAnywhere(e.target.checked));
          } catch (err) {
            toast(errorText(err), "error");
          }
        }}
      />
      <span>
        {t("Dictate into any app with {keys}", { keys: s.dictate_hotkey ?? "Ctrl+Alt+D" })}
        <span className="muted block">{t("Press it in any program, speak, and what you say is typed where the cursor is. Press it again to stop. A strip at the top of the screen shows while it listens.")}</span>
      </span>
    </label>
  );
}

/** Quick ask's hotkey (pressed, not typed) and the model its chats use. */
export function QuickAskSetup({ toast }: { toast: PushToast }) {
  const [s, setS] = useState<QuickSettings | null>(null);
  const [recording, setRecording] = useState(false);
  useEffect(() => {
    api.quickSettings().then(setS).catch(() => {});
  }, []);
  if (!s) return null;
  return (
    <div className="quick-setup small">
      <div className="row wrap">
        <span>{t("Opens with")}</span>
        <button
          className={`btn small ${recording ? "primary" : ""}`}
          onClick={() => setRecording(true)}
          onBlur={() => setRecording(false)}
          onKeyDown={async (e) => {
            if (!recording) return;
            e.preventDefault();
            if (e.key === "Escape") return setRecording(false);
            const c = combo(e);
            if (!c) return;
            setRecording(false);
            try {
              setS(await api.setQuickHotkey(c));
            } catch (err) {
              toast(errorText(err), "error");
            }
          }}
          aria-label={t("Change the shortcut")}
        >
          {recording ? t("Press the keys…") : s.shown}
        </button>
        {!recording && <span className="muted">{t("Click it, then press the keys you want (Esc to keep this one).")}</span>}
      </div>
      <label className="row wrap">
        <span>{t("Answers with")}</span>
        <ModelSelect
          value={s.model}
          label={t("Answers with")}
          onChange={async (model) => {
            try {
              await api.setQuickModel(model);
              setS({ ...s, model });
            } catch (err) {
              toast(errorText(err), "error");
            }
          }}
        />
      </label>
      <p className="muted">{t("A small, fast model suits quick questions; it loads when you ask, as in a chat.")}</p>
    </div>
  );
}

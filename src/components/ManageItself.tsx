import { useEffect, useState } from "react";
import { api, errorText, type ManagerSettings } from "../api";
import { t } from "../i18n";

/** The per-chat "Manage itself" switch: budgets per job, a stop on repeated failures, a checked plan and working notes. */
export function ManageItself({ chatId, onError }: { chatId: string; onError: (text: string) => void }) {
  const [settings, setSettings] = useState<ManagerSettings | null>(null);
  useEffect(() => {
    let live = true;
    api.managerGet(chatId).then((s) => live && setSettings(s), () => live && setSettings(null));
    return () => {
      live = false;
    };
  }, [chatId]);
  if (!settings) return null;
  const toggle = async () => {
    const next = { ...settings, on: !settings.on };
    try {
      await api.managerSet(chatId, next);
      setSettings(next);
    } catch (e) {
      onError(errorText(e));
    }
  };
  return (
    <button
      className={`globe ${settings.on ? "on" : ""}`}
      onClick={toggle}
      aria-pressed={settings.on}
      title={t("Manage itself: a plan with checks, a budget per job, and a stop when the same step keeps failing")}
    >
      {settings.on ? t("Managing itself") : t("Manage itself")}
    </button>
  );
}

// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import type { Message } from "../api";
import { t } from "../i18n";
import { lowTokenSavings, lowTokensGet, lowTokensSet, type LowTokens } from "../lowTokens";

/** This chat's low token switch, with what it saved once there is something to compare. */
export function LowTokenChip({ chatId, messages }: { chatId: string; messages: Message[] }) {
  const [v, setV] = useState<LowTokens | null>(null);
  useEffect(() => {
    lowTokensGet(chatId).then(setV).catch(() => {});
  }, [chatId]);
  if (!v) return null;
  const flip = async () => {
    await lowTokensSet(chatId, !v.on).catch(() => {});
    setV(await lowTokensGet(chatId).catch(() => v));
  };
  const s = v.on ? lowTokenSavings(messages) : null;
  const detail = s
    ? s.saved !== null
      ? t("about {n}% fewer tokens a step", { n: Math.round(s.saved * 100) })
      : t("about {n} tokens a step", { n: s.low })
    : null;
  return (
    <button
      className={v.on ? "btn small" : "btn ghost small"}
      aria-pressed={v.on}
      onClick={flip}
      title={v.on ? t("Low token mode is on for this chat. Click to turn it off.") : t("Turn on low token mode for this chat")}
    >
      {v.on ? t("Low token mode") : t("Low tokens: off")}
      {detail && <span className="muted"> · {detail}</span>}
    </button>
  );
}

// SPDX-License-Identifier: AGPL-3.0-only
import type { ContextInfo } from "../api";
import { percent, tokens } from "../format";
import { t } from "../i18n";

/** Shows how much of the model's context window the next reply has to work with. */
export function ContextMeter({ info, fallbackCtx }: { info: ContextInfo | null; fallbackCtx: number | null }) {
  if (!info) {
    return (
      <div className="ctx" title={t("Fills in after your first message")}>
        <div className="ctx-bar" />
        <span className="ctx-label muted">{fallbackCtx ? t("0 / {n} tokens", { n: tokens(fallbackCtx) }) : t("Context")}</span>
      </div>
    );
  }
  // After a reply, the engine's own count (prompt + reply) is the accurate one.
  const used = info.last_total ?? info.system_tokens + info.profile_tokens + info.history_tokens;
  const parts = [
    { key: "sys", label: t("Instructions"), value: info.system_tokens },
    { key: "profile", label: t("Your profile"), value: info.profile_tokens },
    { key: "history", label: t("Conversation"), value: info.history_tokens },
    { key: "reserve", label: t("Room for the reply"), value: info.reply_reserve },
  ];
  const tip =
    parts.map((p) => t("{label}: {n}", { label: p.label, n: tokens(p.value) })).join("\n") +
    "\n" +
    t("Window: {n}", { n: tokens(info.ctx) }) +
    (info.dropped_messages > 0 ? "\n" + t("{n} older message(s) no longer fit and were left out.", { n: info.dropped_messages }) : "");

  return (
    <div className="ctx" title={tip} aria-label={t("Context window: {used} of {total} tokens used", { used: tokens(used), total: tokens(info.ctx) })}>
      <div className="ctx-bar">
        {parts.map((p) => (
          <span key={p.key} className={`ctx-seg ${p.key}`} style={{ width: `${percent(p.value, info.ctx)}%` }} />
        ))}
      </div>
      <span className="ctx-label">
        {tokens(used)} / {tokens(info.ctx)}
        {info.dropped_messages > 0 && <span className="warn"> · {t("{n} left out", { n: info.dropped_messages })}</span>}
      </span>
    </div>
  );
}

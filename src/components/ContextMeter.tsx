// SPDX-License-Identifier: AGPL-3.0-only
import type { ContextInfo } from "../api";
import { percent, tokens } from "../format";

/** Shows how much of the model's context window the next reply has to work with. */
export function ContextMeter({ info, fallbackCtx }: { info: ContextInfo | null; fallbackCtx: number | null }) {
  if (!info) {
    return (
      <div className="ctx" title="Fills in after your first message">
        <div className="ctx-bar" />
        <span className="ctx-label muted">{fallbackCtx ? `0 / ${tokens(fallbackCtx)} tokens` : "Context"}</span>
      </div>
    );
  }
  // After a reply, the engine's own count (prompt + reply) is the accurate one.
  const used = info.last_total ?? info.system_tokens + info.profile_tokens + info.history_tokens;
  const parts = [
    { key: "sys", label: "Instructions", value: info.system_tokens },
    { key: "profile", label: "Your profile", value: info.profile_tokens },
    { key: "history", label: "Conversation", value: info.history_tokens },
    { key: "reserve", label: "Room for the reply", value: info.reply_reserve },
  ];
  const tip =
    parts.map((p) => `${p.label}: ${tokens(p.value)}`).join("\n") +
    `\nWindow: ${tokens(info.ctx)}` +
    (info.dropped_messages > 0 ? `\n${info.dropped_messages} older message(s) no longer fit and were left out.` : "");

  return (
    <div className="ctx" title={tip} aria-label={`Context window: ${tokens(used)} of ${tokens(info.ctx)} tokens used`}>
      <div className="ctx-bar">
        {parts.map((p) => (
          <span key={p.key} className={`ctx-seg ${p.key}`} style={{ width: `${percent(p.value, info.ctx)}%` }} />
        ))}
      </div>
      <span className="ctx-label">
        {tokens(used)} / {tokens(info.ctx)}
        {info.dropped_messages > 0 && <span className="warn"> · {info.dropped_messages} left out</span>}
      </span>
    </div>
  );
}

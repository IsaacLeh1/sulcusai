// SPDX-License-Identifier: AGPL-3.0-only
import type { Message } from "../api";
import { t } from "../i18n";
import { chatTotal, useTokensShown } from "../tokenUsage";

/** The chat's tokens so far, summed over every reply and tool step. */
export function ChatTokens({ messages }: { messages: Message[] }) {
  const shown = useTokensShown();
  const total = chatTotal(messages);
  if (!shown || !total) return null;
  const n = (x: number) => x.toLocaleString();
  const about = total.estimate ? t("about") + " " : "";
  return (
    <span className="muted small" title={t("Tokens this chat has used: {in} sent to the model, {out} written by it", { in: n(total.in), out: n(total.out) })}>
      {t("{a}{n} tokens in this chat", { a: about, n: n(total.in + total.out) })}
    </span>
  );
}

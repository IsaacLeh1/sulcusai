// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { api, on, type ChatCloudSpend, type CloudChoice } from "../api";
import { t } from "../i18n";
import { priceText } from "./ModelChoices";

const money = (n: number) => (n < 0.01 && n > 0 ? `$${n.toFixed(4)}` : `$${n.toFixed(2)}`);

/** What a chat has cost on cloud models, this month's total against the
 * budget, and the model's prices: shown while the chat uses a cloud model
 * (or has cost something). */
export function CloudCost({ chatId, cloud }: { chatId: string; cloud: CloudChoice | null }) {
  const [s, setS] = useState<ChatCloudSpend | null>(null);
  useEffect(() => {
    let alive = true;
    const load = () =>
      api
        .chatCloudSpend(chatId)
        .then((x) => alive && setS(x))
        .catch(() => {});
    load();
    const sub = on("cloud:spent", (p) => (p.chat_id === chatId || p.chat_id === null) && load());
    return () => {
      alive = false;
      sub.then((un) => un());
    };
  }, [chatId]);
  if (!s || (!cloud && s.chat.requests === 0)) return null;
  const price = cloud ? priceText(cloud.price_in, cloud.price_out) : "";
  const unpriced = cloud && cloud.price_in == null && cloud.price_out == null;
  const near = s.budget != null && s.month.usd >= s.budget * 0.9;
  const title = [
    t("This chat: {usd} over {n} requests ({inTokens} tokens in, {outTokens} out)", {
      usd: money(s.chat.usd),
      n: s.chat.requests,
      inTokens: s.chat.input_tokens.toLocaleString(),
      outTokens: s.chat.output_tokens.toLocaleString(),
    }),
    s.budget != null
      ? t("This month on cloud models: {spent} of your {budget} budget ({left} left)", { spent: money(s.month.usd), budget: money(s.budget), left: money(Math.max(0, s.budget - s.month.usd)) })
      : t("This month on cloud models: {spent} (no budget set; set one in Settings → Cloud models)", { spent: money(s.month.usd) }),
    cloud && price ? `${cloud.name}: ${price}` : "",
    unpriced ? t("This model's prices aren't set, so its cost shows as $0. Add them in Settings → Cloud models.") : "",
    t("Estimates from the prices in Settings; your provider's bill is the final word."),
  ]
    .filter(Boolean)
    .join("\n");
  return (
    <span className={`small cloud-cost ${near ? "warn-text" : "muted"}`} title={title}>
      {t("{usd} this chat", { usd: money(s.chat.usd) })}
      {" · "}
      {s.budget != null ? t("{spent} of {budget} this month", { spent: money(s.month.usd), budget: money(s.budget) }) : t("{spent} this month", { spent: money(s.month.usd) })}
      {price ? ` · ${price}` : unpriced ? ` · ${t("prices not set")}` : ""}
    </span>
  );
}

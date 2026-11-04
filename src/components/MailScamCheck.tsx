// SPDX-License-Identifier: AGPL-3.0-only
// The scam check on an open message, shown only while its switch is on.
import { useEffect, useState } from "react";
import { errorText } from "../api";
import { writingApi, type ScamVerdict } from "../api/writing";
import type { PushToast } from "./Toasts";
import { t } from "../i18n";

export function MailScamCheck({ id, text, from, toast }: { id: string; text: string; from: string; toast: PushToast }) {
  const [on, setOn] = useState(false);
  const [v, setV] = useState<ScamVerdict | null>(null);
  useEffect(() => {
    writingApi.switches().then((s) => setOn(!!s.scam_check)).catch(() => {});
  }, []);
  useEffect(() => setV(null), [id]);
  if (!on) return null;
  const run = () => writingApi.scamCheck(text, from || null).then(setV).catch((e) => toast(errorText(e), "error"));
  const label = { no_signs: t("No common signs found. That doesn't prove it's safe."), careful: t("Be careful: some signs of a scam."), likely_scam: t("Likely a scam. Don't reply, pay or click.") };
  return (
    <>
      <button className="btn" onClick={run}>{t("Scam check")}</button>
      {v && (
        <div className={`scam-verdict ${v.level}`}>
          <strong>{label[v.level]}</strong>
          {v.signs.length > 0 && <ul>{v.signs.map((s, i) => <li key={i}>{s.why}</li>)}</ul>}
        </div>
      )}
    </>
  );
}

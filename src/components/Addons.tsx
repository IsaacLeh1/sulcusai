// SPDX-License-Identifier: AGPL-3.0-only
// One-click add-ons: well-known MCP servers, set up without installing
// anything by hand. Each becomes a connector, whose tools ask first.
import { useEffect, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { api, errorText, on, type AddonView } from "../api";
import { t } from "../i18n";
import { Busy, useSince } from "./Busy";
import { Modal } from "./Modal";
import type { PushToast } from "./Toasts";

export function Addons({ toast, onAdded }: { toast: PushToast; onAdded: () => void }) {
  const [list, setList] = useState<AddonView[]>([]);
  const [adding, setAdding] = useState<AddonView | null>(null);
  const [values, setValues] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState<{ stage: string; fraction: number } | null>(null);
  const since = useSince(!!busy);
  const load = () => api.addons().then(setList).catch(() => {});
  useEffect(() => {
    load();
    const sub = on("addons:progress", (p) => setBusy((b) => (b && p.stage ? p : b)));
    return () => {
      sub.then((un) => un());
    };
  }, []);

  const add = async (a: AddonView, given: Record<string, string>) => {
    setBusy({ stage: t("Starting"), fraction: -1 });
    try {
      const c = await api.addonAdd(a.id, given);
      toast(t("{name} is added with {n} tools. Its tools ask before they act.", { name: a.name, n: c.tools.length }), "success");
      setAdding(null);
      setValues({});
      load();
      onAdded();
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(null);
    }
  };

  const pick = async (id: string, kind: string) => {
    const p = await openDialog(kind === "folder" ? { directory: true, multiple: false } : { multiple: false });
    if (typeof p === "string") setValues((v) => ({ ...v, [id]: p }));
  };

  const categories = [...new Set(list.map((a) => a.category))];
  return (
    <section className="card addons">
      <div className="section-head">
        <h2>{t("Add-ons")}</h2>
      </div>
      <p className="small muted">
        {t("Popular tools the assistant can use, set up in one click. SulcusAI downloads what each needs into its own folder. Online ones sign you in through your browser.")}
      </p>
      {busy && !adding && <Busy stage={busy.stage} fraction={busy.fraction >= 0 && busy.fraction < 1 ? busy.fraction : null} since={since} />}
      {categories.map((cat) => (
        <div key={cat} className="addons-group">
          <h3 className="small">{t(cat)}</h3>
          <div className="addons-grid">
            {list.filter((a) => a.category === cat).map((a) => (
              <div key={a.id} className="addon">
                <div className="row">
                  <strong className="small">{t(a.name)}</strong>
                  <span className="spacer" />
                  {a.added ? (
                    <span className="small addon-added">{t("Added")}</span>
                  ) : (
                    <button className="btn small" disabled={!!busy} onClick={() => (a.inputs.length ? setAdding(a) : add(a, {}))}>{t("Add")}</button>
                  )}
                </div>
                <span className="small muted">{t(a.about)}</span>
                <span className="small faint">{t("By {by}", { by: a.by })}</span>
              </div>
            ))}
          </div>
        </div>
      ))}
      {adding && (
        <Modal title={t("Add {name}", { name: t(adding.name) })} onClose={() => !busy && setAdding(null)}>
          <div className="form">
            <p className="small muted">{t(adding.about)}</p>
            {adding.inputs.map((i) => (
              <label key={i.id}>
                {t(i.label)}
                <div className="row">
                  <input className="input grow" type={i.kind === "secret" ? "password" : "text"} value={values[i.id] ?? ""} onChange={(e) => setValues((v) => ({ ...v, [i.id]: e.target.value }))} autoComplete="off" />
                  {(i.kind === "folder" || i.kind === "file") && <button className="btn small" onClick={() => pick(i.id, i.kind)}>{t("Choose…")}</button>}
                </div>
                <span className="small muted">{t(i.help)}</span>
              </label>
            ))}
            {adding.inputs.some((i) => i.kind === "secret") && <p className="small muted">{t("Keys are kept encrypted on this PC and given only to this add-on.")}</p>}
            {busy ? (
              <Busy stage={busy.stage} fraction={busy.fraction >= 0 && busy.fraction < 1 ? busy.fraction : null} since={since} />
            ) : (
              <div className="row">
                <button className="link small" onClick={() => api.openWeb(adding.link)}>{t("About it")}</button>
                <span className="spacer" />
                <button className="btn" onClick={() => setAdding(null)}>{t("Cancel")}</button>
                <button className="btn primary" disabled={adding.inputs.some((i) => !values[i.id]?.trim())} onClick={() => add(adding, values)}>{t("Add")}</button>
              </div>
            )}
          </div>
        </Modal>
      )}
    </section>
  );
}

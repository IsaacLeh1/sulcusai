// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useMemo, useState } from "react";
import { api, errorText, type CloudModel, type CloudProvider, type CloudView } from "../api";
import { Modal } from "./Modal";
import type { PushToast } from "./Toasts";

const money = (n: number) => (n < 0.01 && n > 0 ? `$${n.toFixed(4)}` : `$${n.toFixed(2)}`);

function ModelPicker({ provider, onClose, onSaved, toast }: { provider: CloudProvider; onClose: () => void; onSaved: (v: CloudView) => void; toast: PushToast }) {
  const [all, setAll] = useState<CloudModel[] | null>(null);
  const [chosen, setChosen] = useState<Record<string, CloudModel>>(() => Object.fromEntries(provider.models.map((m) => [m.id, m])));
  const [filter, setFilter] = useState("");
  useEffect(() => {
    api
      .cloudModelsAvailable(provider.id)
      .then(setAll)
      .catch((e) => {
        toast(errorText(e), "error");
        onClose();
      });
  }, [provider.id, toast, onClose]);
  const shown = useMemo(() => (all ?? []).filter((m) => `${m.name} ${m.id}`.toLowerCase().includes(filter.toLowerCase())), [all, filter]);
  return (
    <Modal title={`Models from ${provider.name}`} onClose={onClose}>
      {!all ? (
        <p className="muted">Asking {provider.name} which models it offers…</p>
      ) : (
        <>
          <input className="input" placeholder="Filter" value={filter} onChange={(e) => setFilter(e.target.value)} autoFocus />
          <div className="cloud-pick">
            {shown.map((m) => (
              <label key={m.id} className="check">
                <input
                  type="checkbox"
                  checked={!!chosen[m.id]}
                  onChange={(e) =>
                    setChosen((c) => {
                      const next = { ...c };
                      if (e.target.checked) next[m.id] = c[m.id] ?? m;
                      else delete next[m.id];
                      return next;
                    })
                  }
                />
                <span>
                  {m.name} <span className="muted small">{m.id}{m.price_in !== null ? ` · $${m.price_in} / $${m.price_out} per M tokens` : ""}</span>
                </span>
              </label>
            ))}
            {shown.length === 0 && <p className="muted small">No models match.</p>}
          </div>
          <div className="modal-actions">
            <button className="btn" onClick={onClose}>Cancel</button>
            <button
              className="btn primary"
              onClick={async () => {
                try {
                  onSaved(await api.setCloudModels(provider.id, Object.values(chosen)));
                  onClose();
                } catch (e) {
                  toast(errorText(e), "error");
                }
              }}
            >
              Use {Object.keys(chosen).length} {Object.keys(chosen).length === 1 ? "model" : "models"}
            </button>
          </div>
        </>
      )}
    </Modal>
  );
}

function ProviderCard({ p, onChanged, toast }: { p: CloudProvider; onChanged: (v: CloudView) => void; toast: PushToast }) {
  const [key, setKey] = useState("");
  const [picking, setPicking] = useState(false);
  const run = async (f: () => Promise<CloudView>) => {
    try {
      onChanged(await f());
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const setPrice = (id: string, field: "price_in" | "price_out", v: string) =>
    run(() => api.setCloudModels(p.id, p.models.map((m) => (m.id === id ? { ...m, [field]: v === "" ? null : Number(v) } : m))));
  return (
    <div className="cloud-provider">
      <div className="adv-head">
        <label className="check">
          <input type="checkbox" checked={p.enabled} disabled={!p.has_key} onChange={(e) => run(() => api.setCloudProvider(p.id, e.target.checked, null))} />
          <strong>{p.name}</strong>
        </label>
        <button className="btn ghost small danger" onClick={() => run(() => api.removeCloudProvider(p.id))}>Remove</button>
      </div>
      <span className="muted small">{p.base_url}</span>
      <div className="row wrap">
        <input className="input" type="password" value={key} onChange={(e) => setKey(e.target.value)} placeholder={p.has_key ? "Key saved (encrypted). Paste a new one to replace it." : "API key"} aria-label={`${p.name} API key`} />
        <button
          className="btn small"
          disabled={!key.trim()}
          onClick={async () => {
            await run(() => api.setCloudProvider(p.id, true, key));
            setKey("");
          }}
        >
          Save key
        </button>
        {p.has_key && <button className="btn ghost small" onClick={() => run(() => api.setCloudProvider(p.id, false, ""))}>Remove key</button>}
        <button className="btn small" disabled={!p.has_key} onClick={() => setPicking(true)}>Choose models…</button>
      </div>
      {p.models.length > 0 && (
        <table className="cloud-models small">
          <thead>
            <tr>
              <th>Model</th>
              <th title="US dollars per million tokens, for the spend estimate">$ in / M</th>
              <th>$ out / M</th>
            </tr>
          </thead>
          <tbody>
            {p.models.map((m) => (
              <tr key={m.id}>
                <td>
                  ☁ {m.name}
                  {m.vision && <span className="muted"> · sees pictures</span>}
                </td>
                <td><input className="input" type="number" min={0} step={0.01} defaultValue={m.price_in ?? ""} onBlur={(e) => setPrice(m.id, "price_in", e.target.value)} aria-label={`${m.name} input price`} /></td>
                <td><input className="input" type="number" min={0} step={0.01} defaultValue={m.price_out ?? ""} onBlur={(e) => setPrice(m.id, "price_out", e.target.value)} aria-label={`${m.name} output price`} /></td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      {picking && <ModelPicker provider={p} onClose={() => setPicking(false)} onSaved={onChanged} toast={toast} />}
    </div>
  );
}

export function CloudSettings({ toast, cloudOn }: { toast: PushToast; cloudOn: boolean }) {
  const [v, setV] = useState<CloudView | null>(null);
  const [custom, setCustom] = useState<{ name: string; url: string } | null>(null);
  const [budget, setBudget] = useState("");
  useEffect(() => {
    api.cloudView().then((x) => {
      setV(x);
      setBudget(x.budget === null ? "" : String(x.budget));
    });
  }, []);
  if (!v) return null;
  const run = async (f: () => Promise<CloudView>) => {
    try {
      setV(await f());
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const used = v.budget ? Math.min(1, v.spend.usd / v.budget) : 0;

  return (
    <section className="card cloud-settings">
      <h2>☁ Cloud models</h2>
      <p className="muted small">
        Use models from providers you have an account with, with your own API key. They only work at the Cloud level, and what you send to them goes to that provider. Models on this PC stay the default.
        {!cloudOn && " You're not at the Cloud level now, so these models are switched off."}
      </p>

      <div className="cloud-spend">
        <div className="adv-head">
          <span className="small">
            This month: <strong>{money(v.spend.usd)}</strong>
            {v.budget ? ` of ${money(v.budget)}` : ""} · {v.spend.requests} requests · {(v.spend.input_tokens + v.spend.output_tokens).toLocaleString()} tokens
          </span>
        </div>
        {v.budget ? <div className={`progress ${used >= 0.9 ? "warn" : ""}`}><span style={{ width: `${used * 100}%` }} /></div> : null}
        <span className="muted small">An estimate from the prices below; your provider's bill is the final word.</span>
      </div>
      <div className="row wrap">
        <label className="adv-field">
          <span>Monthly budget (US$)</span>
          <input className="input" type="number" min={0} step={1} value={budget} placeholder="No limit" onChange={(e) => setBudget(e.target.value)} onBlur={() => run(() => api.setCloudOptions(budget === "" ? null : Number(budget), v.redact))} />
        </label>
      </div>
      <label className="check">
        <input type="checkbox" checked={v.redact} onChange={(e) => run(() => api.setCloudOptions(v.budget, e.target.checked))} />
        <span>
          Hide personal details from cloud models
          <span className="muted small block">Email addresses, phone numbers, card numbers and US Social Security numbers are replaced with placeholders like [email 1] before sending, and put back in the reply.</span>
        </span>
      </label>

      {v.providers.map((p) => (
        <ProviderCard key={p.id} p={p} onChanged={setV} toast={toast} />
      ))}

      <div className="row wrap">
        <span className="small">Add:</span>
        {v.presets.map((p) => (
          <button key={p.id} className="btn small" onClick={() => run(() => api.addCloudProvider(p.id, null, null))}>
            {p.name}
          </button>
        ))}
        <button className="btn ghost small" onClick={() => setCustom({ name: "", url: "" })}>Other (OpenAI-compatible)…</button>
      </div>
      {custom && (
        <div className="row wrap">
          <input className="input" placeholder="Name" value={custom.name} onChange={(e) => setCustom({ ...custom, name: e.target.value })} />
          <input className="input" placeholder="https://…/v1" value={custom.url} onChange={(e) => setCustom({ ...custom, url: e.target.value })} />
          <button
            className="btn small"
            disabled={!custom.url.trim()}
            onClick={async () => {
              await run(() => api.addCloudProvider(null, custom.name, custom.url));
              setCustom(null);
            }}
          >
            Add
          </button>
        </div>
      )}
    </section>
  );
}

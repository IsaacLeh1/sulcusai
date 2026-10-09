// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { api, errorText, type ApiServerView } from "../api";
import type { PushToast } from "./Toasts";

export function ApiServerSettings({ toast }: { toast: PushToast }) {
  const [v, setV] = useState<ApiServerView | null>(null);
  const [port, setPort] = useState("");
  const [show, setShow] = useState(false);
  useEffect(() => {
    api.apiServerView().then((x) => {
      setV(x);
      setPort(String(x.port));
    });
  }, []);
  if (!v) return null;

  const apply = async (enabled: boolean) => {
    try {
      const x = await api.setApiServer(enabled, Number(port) || v.port);
      setV(x);
      setPort(String(x.port));
      if (x.problem) toast(x.problem, "error");
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const copy = async (text: string, what: string) => {
    await navigator.clipboard.writeText(text);
    toast(`Copied the ${what}.`, "success");
  };
  const example = `curl ${v.url}/chat/completions \\
  -H "Authorization: Bearer ${show ? v.key : "YOUR_KEY"}" \\
  -H "Content-Type: application/json" \\
  -d '{"messages":[{"role":"user","content":"Hello"}]}'`;

  return (
    <section className="card api-server">
      <h2>Local API server</h2>
      <p className="muted small">
        Lets other programs on this PC (code editors, scripts, other chat apps) use your models through an OpenAI-compatible API. Only this PC can reach it, every request needs the key, and each one shows in Activity.
      </p>
      <label className="check">
        <input type="checkbox" checked={v.enabled} onChange={(e) => apply(e.target.checked)} />
        <span>
          <strong>Turn on the local API server</strong>
          {v.enabled && <span className={`small block ${v.running && !v.problem ? "ok-text" : "muted"}`}>{v.problem ?? (v.running ? `Running at ${v.url}` : "Starting…")}</span>}
        </span>
      </label>
      <div className="row wrap">
        <label className="adv-field">
          <span>Port</span>
          <input className="input" type="number" min={1024} max={65535} value={port} onChange={(e) => setPort(e.target.value)} />
        </label>
        {Number(port) !== v.port && (
          <button className="btn small" onClick={() => apply(v.enabled)}>
            Use port {port}
          </button>
        )}
      </div>
      <div className="adv-field">
        <span>Base URL</span>
        <div className="row">
          <code className="api-value">{v.url}</code>
          <button className="btn ghost small" onClick={() => copy(v.url, "address")}>Copy</button>
        </div>
      </div>
      <div className="adv-field">
        <span>API key</span>
        <div className="row wrap">
          <code className="api-value">{show ? v.key : "sk-sulcus-" + "•".repeat(16)}</code>
          <button className="btn ghost small" onClick={() => setShow(!show)}>{show ? "Hide" : "Show"}</button>
          <button className="btn ghost small" onClick={() => copy(v.key, "key")}>Copy</button>
          <button
            className="btn ghost small"
            onClick={async () => {
              try {
                setV(await api.newApiKey());
                toast("Made a new key. Programs using the old one need the new one.", "success");
              } catch (e) {
                toast(errorText(e), "error");
              }
            }}
          >
            New key
          </button>
        </div>
      </div>
      <details className="small">
        <summary>How to use it</summary>
        <p className="muted">
          Point any OpenAI-compatible program at the base URL with the key. <code>model</code> can be a model's name or id from <code>GET /v1/models</code>; leave it out to use your default model. Replies stream when <code>"stream": true</code>.
        </p>
        <pre className="api-example">{example}</pre>
      </details>
    </section>
  );
}

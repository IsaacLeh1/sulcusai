// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { api, errorText, type ApiServerView } from "../api";
import { t } from "../i18n";
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
  const copy = async (text: string, done: string) => {
    await navigator.clipboard.writeText(text);
    toast(done, "success");
  };
  const example = `curl ${v.url}/chat/completions \\
  -H "Authorization: Bearer ${show ? v.key : "YOUR_KEY"}" \\
  -H "Content-Type: application/json" \\
  -d '{"messages":[{"role":"user","content":"Hello"}]}'`;

  return (
    <section className="card api-server">
      <h2>{t("Local API server")}</h2>
      <p className="muted small">
        {t("Lets other programs on this PC (code editors, scripts, other chat apps) use your models through an OpenAI-compatible API. Only this PC can reach it, every request needs the key, and each one shows in Activity.")}
      </p>
      <label className="check">
        <input type="checkbox" checked={v.enabled} onChange={(e) => apply(e.target.checked)} />
        <span>
          <strong>{t("Turn on the local API server")}</strong>
          {v.enabled && <span className={`small block ${v.running && !v.problem ? "ok-text" : "muted"}`}>{v.problem ?? (v.running ? t("Running at {url}", { url: v.url }) : t("Starting…"))}</span>}
        </span>
      </label>
      <div className="row wrap">
        <label className="adv-field">
          <span>{t("Port")}</span>
          <input className="input" type="number" min={1024} max={65535} value={port} onChange={(e) => setPort(e.target.value)} />
        </label>
        {Number(port) !== v.port && (
          <button className="btn small" onClick={() => apply(v.enabled)}>
            {t("Use port {port}", { port })}
          </button>
        )}
      </div>
      <div className="adv-field">
        <span>{t("Base URL")}</span>
        <div className="row">
          <code className="api-value">{v.url}</code>
          <button className="btn ghost small" onClick={() => copy(v.url, t("Copied the address."))}>{t("Copy")}</button>
        </div>
      </div>
      <div className="adv-field">
        <span>{t("API key")}</span>
        <div className="row wrap">
          <code className="api-value">{show ? v.key : "sk-sulcus-" + "•".repeat(16)}</code>
          <button className="btn ghost small" onClick={() => setShow(!show)}>{show ? t("Hide") : t("Show")}</button>
          <button className="btn ghost small" onClick={() => copy(v.key, t("Copied the key."))}>{t("Copy")}</button>
          <button
            className="btn ghost small"
            onClick={async () => {
              try {
                setV(await api.newApiKey());
                toast(t("Made a new key. Programs using the old one need the new one."), "success");
              } catch (e) {
                toast(errorText(e), "error");
              }
            }}
          >
            {t("New key")}
          </button>
        </div>
      </div>
      <details className="small">
        <summary>{t("How to use it")}</summary>
        <p className="muted">
          {t("Point any OpenAI-compatible program at the base URL with the key.")} <code>model</code> {t("can be a model's name or id from")} <code>GET /v1/models</code>;{" "}
          {t("leave it out to use your default model.")} {t("Replies stream when")} <code>"stream": true</code>.
        </p>
        <pre className="api-example">{example}</pre>
        <p className="muted">
          {t("Programs made for Claude (like Claude Code) work too: it also answers Claude's API at the same address, with the key as x-api-key. A Claude model name means your default model here.")}{" "}
          {t("Programs that search by meaning can get text turned into numbers at")} <code>/v1/embeddings</code> {t("(once Documents has its model).")}
        </p>
        <p className="muted">
          {t("A reply in a set shape: send")} <code>response_format</code> {t("as OpenAI does (JSON, or JSON that fits a schema). It works with this PC's models and the cloud models turned on.")}
        </p>
      </details>
      <OllamaServer v={v} setV={setV} toast={toast} />
    </section>
  );
}

/** The Ollama-shaped server: programs made for Ollama use this PC's models as they are. */
function OllamaServer({ v, setV, toast }: { v: ApiServerView; setV: (v: ApiServerView) => void; toast: PushToast }) {
  const [port, setPort] = useState(String(v.ollama_port));
  const apply = async (enabled: boolean) => {
    try {
      const x = await api.setOllamaServer(enabled, Number(port) || v.ollama_port);
      setV(x);
      setPort(String(x.ollama_port));
      if (x.ollama_problem) toast(x.ollama_problem, "error");
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  return (
    <div className="ollama-server">
      <label className="check">
        <input type="checkbox" checked={v.ollama} onChange={(e) => apply(e.target.checked)} />
        <span>
          <strong>{t("Also answer programs made for Ollama")}</strong>
          <span className="small muted block">
            {t("Open WebUI, Continue, Obsidian plugins and other apps that work with Ollama can use the models on this PC, at Ollama's usual address. Like Ollama it needs no key, so it offers only this PC's own models: no cloud models and not the SulcusAI assistant. Web pages can't use it.")}
          </span>
          {v.ollama && (
            <span className={`small block ${v.ollama_running && !v.ollama_problem ? "ok-text" : "muted"}`}>
              {v.ollama_problem ?? (v.ollama_running ? t("Running at {url}", { url: v.ollama_url }) : t("Starting…"))}
            </span>
          )}
        </span>
      </label>
      <div className="row wrap">
        <label className="adv-field">
          <span>{t("Port")}</span>
          <input className="input" type="number" min={1024} max={65535} value={port} onChange={(e) => setPort(e.target.value)} />
        </label>
        {Number(port) !== v.ollama_port && (
          <button className="btn small" onClick={() => apply(v.ollama)}>
            {t("Use port {port}", { port })}
          </button>
        )}
      </div>
      <p className="muted small">{t("If Ollama itself is installed and running, it has this port: quit it, or pick another port here and in the program.")}</p>
    </div>
  );
}

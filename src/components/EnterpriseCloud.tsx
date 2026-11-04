// SPDX-License-Identifier: AGPL-3.0-only
// Company AI plans: the policy's locks, Azure/Bedrock/Vertex settings, gateway headers and CA,
// local or cloud routing per assistant or project, and usage by provider and project.
import { useEffect, useState } from "react";
import { save } from "@tauri-apps/plugin-dialog";
import { api, errorText, on, openInViewer, type CloudChoice, type CloudProvider, type CloudView } from "../api";
import { enterpriseApi, type DeviceCode, type EntAuth, type EntInput, type PolicyView, type Route, type RouteScope, type UsageRow } from "../api/enterprise";
import { t } from "../i18n";
import type { PushToast } from "./Toasts";

type Run = (f: () => Promise<CloudView>) => Promise<void>;

export function PolicyLocks({ policy }: { policy: PolicyView }) {
  if (!policy.active) return null;
  return (
    <div className="callout small">
      <strong>{t("Set by your organization")}</strong>
      <ul>
        {policy.locks.map((l) => (
          <li key={l.what}>
            {l.what} <span className="muted">{l.why}</span>
          </li>
        ))}
      </ul>
      <span className="muted">{t("From {path}. Only an administrator can change it.", { path: policy.path })}</span>
    </div>
  );
}

const AUTHS: Record<string, { id: EntAuth; label: string }[]> = {
  azure: [
    { id: "key", label: "API key" },
    { id: "entra", label: "Microsoft Entra ID" },
  ],
  bedrock: [
    { id: "key", label: "Access keys or a Bedrock API key" },
    { id: "profile", label: "AWS profile on this PC" },
  ],
  vertex: [
    { id: "key", label: "Service-account key (JSON)" },
    { id: "adc", label: "gcloud sign-in on this PC" },
  ],
};

function EntraSignIn({ p, run, toast }: { p: CloudProvider; run: Run; toast: PushToast }) {
  const [code, setCode] = useState<DeviceCode | null>(null);
  useEffect(() => {
    const a = on("cloud:signed_in", (e) => {
      if (e.id === p.id) setCode(null);
    });
    const b = on("cloud:sign_in_failed", (e) => {
      if (e.id !== p.id) return;
      setCode(null);
      toast(e.error, "error");
    });
    return () => {
      a.then((f) => f());
      b.then((f) => f());
    };
  }, [p.id, toast]);
  if (p.ent.signed_in)
    return (
      <div className="row wrap small">
        <span>{p.ent.account ? t("Signed in as {name}.", { name: p.ent.account }) : t("Signed in.")}</span>
        <button className="btn ghost small" onClick={() => run(() => enterpriseApi.entraSignOut<CloudView>(p.id))}>
          {t("Sign out")}
        </button>
      </div>
    );
  return code ? (
    <div className="small">
      <p>{t("Go to {url} and enter the code {code}. This window updates once you're signed in.", { url: code.verification_uri, code: code.user_code })}</p>
      <button className="btn small" onClick={() => openInViewer(code.verification_uri)}>
        {t("Open the sign-in page")}
      </button>
    </div>
  ) : (
    <button
      className="btn small"
      onClick={() =>
        enterpriseApi
          .entraStart(p.id)
          .then(setCode)
          .catch((e) => toast(errorText(e), "error"))
      }
    >
      {t("Sign in with Microsoft…")}
    </button>
  );
}

/** Azure, Bedrock and Vertex: where the resource is and how to sign in. */
export function EnterpriseFields({ p, run, toast }: { p: CloudProvider; run: Run; toast: PushToast }) {
  const [f, setF] = useState<EntInput>(() => ({ base_url: p.base_url, auth: p.ent.auth, api_version: p.ent.api_version, region: p.ent.region, project: p.ent.project, profile: p.ent.profile, tenant: p.ent.tenant, client_id: p.ent.client_id, authority: p.ent.authority }));
  const [model, setModel] = useState("");
  const auths = AUTHS[p.kind];
  if (!auths) return null;
  const field = (key: keyof EntInput, label: string, placeholder: string) => (
    <label className="adv-field">
      <span>{label}</span>
      <input className="input" value={(f[key] as string) ?? ""} placeholder={placeholder} spellCheck={false} onChange={(e) => setF({ ...f, [key]: e.target.value })} disabled={p.ent.locked} />
    </label>
  );
  const addModel = () => {
    const id = model.trim();
    if (!id || p.models.some((m) => m.id === id)) return;
    run(() => api.setCloudModels(p.id, [...p.models, { id, name: id, ctx: 128000, max_output: 8192, vision: true, price_in: null, price_out: null }])).then(() => setModel(""));
  };
  return (
    <div className="form small">
      {p.ent.problem && <p className="callout small">{p.ent.problem}</p>}
      <div className="row wrap">
        <label className="adv-field">
          <span>{t("Sign in with")}</span>
          <select className="input" value={f.auth} onChange={(e) => setF({ ...f, auth: e.target.value as EntAuth })} disabled={p.ent.locked}>
            {auths.map((a) => (
              <option key={a.id} value={a.id}>
                {t(a.label)}
              </option>
            ))}
          </select>
        </label>
        {p.kind === "azure" && field("base_url", t("Resource address"), "https://NAME.openai.azure.com")}
        {p.kind === "azure" && field("api_version", t("API version"), "2024-10-21")}
        {p.kind === "azure" && f.auth === "entra" && field("tenant", t("Directory (tenant) id"), "organizations")}
        {p.kind === "azure" && f.auth === "entra" && field("client_id", t("App (client) id from IT"), "00000000-0000-0000-0000-000000000000")}
        {p.kind === "bedrock" && field("region", t("AWS region"), "us-east-1")}
        {p.kind === "bedrock" && f.auth === "profile" && field("profile", t("Profile"), "default")}
        {p.kind === "vertex" && field("project", t("Google Cloud project id"), "my-project")}
        {p.kind === "vertex" && field("region", t("Location"), "global")}
      </div>
      <p className="muted small">
        {p.kind === "azure" && t("Each model is a deployment: its name goes under Models. A Foundry or v1 address ending in /openai/v1 needs no API version.")}
        {p.kind === "bedrock" && t("Paste keys as ACCESS_KEY_ID:SECRET (add :SESSION_TOKEN for temporary keys), or a Bedrock API key. Cross-region inference profiles (us., eu., global.) are listed with the models.")}
        {p.kind === "vertex" && t("Gemini models go through Vertex's OpenAI-compatible endpoint and Claude models through Anthropic's API on Vertex.")}
      </p>
      {!p.ent.locked && (
        <button className="btn small" onClick={() => run(() => enterpriseApi.setCloudEnterprise<CloudView>(p.id, f))}>
          {t("Save")}
        </button>
      )}
      {p.kind === "azure" && p.ent.auth === "entra" && <EntraSignIn p={p} run={run} toast={toast} />}
      <div className="row wrap">
        <input className="input" value={model} placeholder={p.kind === "azure" ? t("Deployment name") : t("Model id")} aria-label={t("Model id")} spellCheck={false} onChange={(e) => setModel(e.target.value)} onKeyDown={(e) => e.key === "Enter" && addModel()} />
        <button className="btn small" disabled={!model.trim()} onClick={addModel}>
          {t("Add model")}
        </button>
      </div>
    </div>
  );
}

/** Headers and a company CA for a gateway in front of a provider. */
export function GatewaySettings({ p, run }: { p: CloudProvider; run: Run }) {
  const [rows, setRows] = useState<[string, string][]>(() => p.ent.header_names.map((n) => [n, ""]));
  const [pem, setPem] = useState("");
  return (
    <details className="cloud-media small">
      <summary>
        {t("Gateway")} ({p.ent.header_names.length === 1 ? t("1 extra header") : t("{n} extra headers", { n: p.ent.header_names.length })}
        {p.ent.has_ca ? `, ${t("company certificate")}` : ""})
      </summary>
      <p className="muted">{t("For a company gateway in front of this provider: headers sent with every request (organization or project ids), kept encrypted, and a CA certificate trusted for this provider only.")}</p>
      {rows.map(([k, v], i) => (
        <div className="row wrap" key={i}>
          <input className="input" value={k} placeholder="OpenAI-Organization" aria-label={t("Header name")} spellCheck={false} onChange={(e) => setRows(rows.map((r, j) => (j === i ? [e.target.value, r[1]] : r)))} />
          <input className="input" value={v} placeholder={p.ent.header_names.includes(k) ? t("Saved (encrypted). Type to replace.") : t("Value")} aria-label={t("Header value")} spellCheck={false} onChange={(e) => setRows(rows.map((r, j) => (j === i ? [r[0], e.target.value] : r)))} />
          <button className="btn ghost small" onClick={() => setRows(rows.filter((_, j) => j !== i))}>
            {t("Remove")}
          </button>
        </div>
      ))}
      <div className="row wrap">
        <button className="btn ghost small" onClick={() => setRows([...rows, ["", ""]])}>
          {t("Add header")}
        </button>
        <button className="btn small" onClick={() => run(() => enterpriseApi.setCloudHeaders<CloudView>(p.id, rows.filter(([k]) => k.trim())))}>
          {t("Save headers")}
        </button>
      </div>
      {!p.ent.locked && (
        <div className="form">
          <textarea className="input" rows={3} value={pem} placeholder="-----BEGIN CERTIFICATE-----" aria-label={t("Company CA certificate")} spellCheck={false} onChange={(e) => setPem(e.target.value)} />
          <div className="row wrap">
            <button className="btn small" disabled={!pem.trim()} onClick={() => run(() => enterpriseApi.setCloudCa<CloudView>(p.id, pem)).then(() => setPem(""))}>
              {t("Trust this certificate")}
            </button>
            {p.ent.has_ca && (
              <button className="btn ghost small" onClick={() => run(() => enterpriseApi.setCloudCa<CloudView>(p.id, ""))}>
                {t("Remove the certificate")}
              </button>
            )}
          </div>
        </div>
      )}
    </details>
  );
}

const usd = (n: number) => `$${n.toFixed(n > 0 && n < 0.01 ? 4 : 2)}`;

export function CloudUsage({ toast }: { toast: PushToast }) {
  const [rows, setRows] = useState<UsageRow[] | null>(null);
  useEffect(() => {
    enterpriseApi.usage().then(setRows).catch(() => setRows([]));
  }, []);
  if (!rows || rows.length === 0) return null;
  const exportCsv = async () => {
    const path = await save({ defaultPath: "cloud-usage.csv", filters: [{ name: "CSV", extensions: ["csv"] }] });
    if (!path) return;
    try {
      await enterpriseApi.saveUsageCsv(path);
      toast(t("Saved."), "success");
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  return (
    <details className="cloud-media small">
      <summary>{t("Usage by provider and project")}</summary>
      <table className="table small">
        <thead>
          <tr>
            <th>{t("Month")}</th>
            <th>{t("Provider")}</th>
            <th>{t("Project")}</th>
            <th>{t("Requests")}</th>
            <th>{t("Tokens")}</th>
            <th>{t("Cost")}</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((r) => (
            <tr key={`${r.month}|${r.provider}|${r.project}`}>
              <td>{r.month}</td>
              <td>{r.provider}</td>
              <td>{r.project || t("No project")}</td>
              <td>{r.requests}</td>
              <td>{(r.input_tokens + r.output_tokens).toLocaleString()}</td>
              <td>{usd(r.usd)}</td>
            </tr>
          ))}
        </tbody>
      </table>
      <button className="btn small" onClick={exportCsv}>
        {t("Export CSV…")}
      </button>
    </details>
  );
}

/** Local or cloud per message, for one assistant or project. Off until turned on. */
export function RouteEditor({ scope, id, toast }: { scope: RouteScope; id: string; toast: PushToast }) {
  const [r, setR] = useState<Route | null>(null);
  const [cloud, setCloud] = useState<CloudChoice[]>([]);
  const [local, setLocal] = useState<{ id: string; name: string }[]>([]);
  useEffect(() => {
    enterpriseApi.route(scope, id).then(setR).catch(() => {});
    api.cloudChoices().then(setCloud).catch(() => {});
    api
      .catalog()
      .then((c) => setLocal(c.models.filter((m) => m.installed).map((m) => ({ id: m.id, name: m.name }))))
      .catch(() => {});
  }, [scope, id]);
  if (!r) return null;
  const change = (next: Route) =>
    enterpriseApi
      .setRoute(scope, id, next)
      .then(setR)
      .catch((e) => toast(errorText(e), "error"));
  return (
    <details className="cloud-media small">
      <summary>
        {t("Local or cloud")} ({r.on ? t("on") : t("off")})
      </summary>
      <label className="check">
        <input type="checkbox" checked={r.on} onChange={(e) => change({ ...r, on: e.target.checked })} />
        <span>
          {t("Choose per message")}
          <span className="muted small block">{t("Messages with personal details, private words or file names stay on a model on this PC. Long or hard ones may go to the cloud model below. The chat says which way each went.")}</span>
        </span>
      </label>
      {r.on && (
        <div className="row wrap">
          <label className="adv-field">
            <span>{t("Private messages")}</span>
            <select className="input" value={r.local_model ?? ""} onChange={(e) => change({ ...r, local_model: e.target.value || null })}>
              <option value="">{t("The chat's model on this PC")}</option>
              {local.map((m) => (
                <option key={m.id} value={m.id}>
                  {m.name}
                </option>
              ))}
            </select>
          </label>
          <label className="adv-field">
            <span>{t("Long or hard messages")}</span>
            <select className="input" value={r.cloud_model ?? ""} onChange={(e) => change({ ...r, cloud_model: e.target.value || null })}>
              <option value="">{t("Stay on this PC")}</option>
              {cloud.map((m) => (
                <option key={m.id} value={m.id}>
                  {m.name} ({m.provider})
                </option>
              ))}
            </select>
          </label>
          <label className="adv-field">
            <span>{t("Private words (comma-separated)")}</span>
            <input
              className="input"
              defaultValue={r.private_words.join(", ")}
              onBlur={(e) =>
                change({
                  ...r,
                  private_words: e.target.value
                    .split(",")
                    .map((w) => w.trim())
                    .filter(Boolean),
                })
              }
            />
          </label>
        </div>
      )}
    </details>
  );
}

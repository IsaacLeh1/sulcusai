// SPDX-License-Identifier: AGPL-3.0-only
// A pasted Hugging Face link: the model it points at, ready to install.
import { useState } from "react";
import { api, errorText, type HubLinkResult, type InstallProgress } from "../api";
import { bytes, percent, placementLabel, words } from "../format";
import { t } from "../i18n";
import { waitLabel } from "../media";
import type { PushToast } from "./Toasts";

const baseName = (path: string) => path.split("/").pop() ?? path;

/** One row to show for each file, whichever kind of model the link is. */
interface Row {
  file: string;
  modelId: string;
  quant: string;
  size: number;
  runs: boolean;
  what: string;
}

/** "Paste a Hugging Face link": works for chat models and for picture,
 * video and music models; shows the versions SulcusAI can use, the one the
 * link named first, and installs the one picked like any other model. */
export function HubLinkBox({
  progress,
  isInstalled,
  toast,
  onInstalled,
  placeholder,
}: {
  progress: Record<string, InstallProgress>;
  isInstalled: (id: string) => boolean;
  toast: PushToast;
  onInstalled: () => void;
  /** What the box asks for (a chat model's link, unless given). */
  placeholder?: string;
}) {
  const [link, setLink] = useState("");
  const [busy, setBusy] = useState(false);
  const [found, setFound] = useState<HubLinkResult | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [all, setAll] = useState(false);
  const installing = Object.keys(progress).length > 0;

  const look = async () => {
    if (!link.trim()) return;
    setBusy(true);
    setProblem(null);
    setFound(null);
    setAll(false);
    try {
      setFound(await api.hubLink(link));
    } catch (e) {
      setProblem(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  const rows: Row[] = !found
    ? []
    : found.kind === "chat"
      ? found.model.files.map((f) => ({
          file: f.file,
          modelId: f.model_id,
          quant: f.quant,
          size: f.size,
          runs: f.placement !== null,
          what: f.placement ? `${t(f.quality)} · ${placementLabel(f.placement)} · ${t("about {n} words a second", { n: words(f.est_tps) })}` : t("too much for this PC"),
        }))
      : found.model.files.map((f) => ({
          file: f.file,
          modelId: f.model_id,
          quant: f.quant,
          size: f.size,
          runs: f.runnable,
          what: f.runnable
            ? `${f.on_gpu ? t("Runs on your graphics card") : t("Runs on your processor")} · ${waitLabel(f.est_secs)}`
            : f.reason
              ? t("This PC can't run it: it {reason}.", { reason: f.reason })
              : t("too much for this PC"),
        }));
  const first = found ? (found.file ?? found.model.recommended) : null;
  const picked = rows.filter((r) => r.file === first);
  const shown = all ? rows : picked.length ? picked : rows.slice(-1);

  const install = async (r: Row) => {
    if (!found) return;
    const m = found.model;
    try {
      if (found.kind === "chat") {
        if (!r.runs && !window.confirm(t("{name} probably won't run on this PC: it needs more memory than it has. Install it anyway?", { name: `${m.name} ${r.quant}` }))) return;
        await api.hubInstall(m.id, r.file, !r.runs);
      } else {
        const ok = window.confirm(
          t("{name} is shared on Hugging Face by {author}{license}. Check what its license allows on the model's page before using what it makes for work. Install it?", {
            name: m.name,
            author: m.author,
            license: found.model.license ? t(" under the license “{license}”", { license: found.model.license }) : "",
          }),
        );
        if (!ok) return;
        await api.mediaHubInstall(m.id, r.file);
      }
      onInstalled();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  return (
    <div className="hub-link">
      <form
        className="row hub-search"
        onSubmit={(e) => {
          e.preventDefault();
          look();
        }}
      >
        <input
          className="input"
          value={link}
          onChange={(e) => setLink(e.target.value)}
          placeholder={placeholder ?? t("Paste a Hugging Face link, e.g. https://huggingface.co/unsloth/Qwen3-8B-GGUF")}
          aria-label={t("Hugging Face link")}
        />
        <button className="btn" type="submit" disabled={busy || !link.trim()}>
          {busy ? t("Looking…") : t("Get this model")}
        </button>
      </form>
      {problem && <p className="warn-text small">{problem}</p>}
      {found && (
        <article className="hub-model">
          <div className="hub-head">
            <div>
              <a href={found.model.url} target="_blank" rel="noreferrer"><strong>{found.model.name}</strong></a> <span className="muted small">{t("by {author}", { author: found.model.author })}</span>{" "}
              <span className="badge subtle">
                {found.kind === "chat"
                  ? t("Chat model")
                  : found.media === "video"
                    ? t("{family} video model", { family: found.model.family })
                    : found.media === "music"
                      ? t("{family} music model", { family: found.model.family })
                      : t("{family} picture model", { family: found.model.family })}
              </span>
            </div>
            <span className="muted small">
              {found.kind === "media" && found.model.license && `${found.model.license} · `}
              {found.kind === "chat"
                ? t("Installs under Models → Chat.")
                : t("Installs under Models → Pictures, video, music, and shows in the Studio.")}
            </span>
          </div>
          {found.kind === "chat" && found.found_for && (
            <p className="small muted">
              {t("{original} isn't a .gguf model file, the kind SulcusAI runs chat models from, so this is its most popular .gguf copy on Hugging Face.", { original: found.found_for })}
            </p>
          )}
          {shown.map((r) => {
            const p = progress[r.modelId];
            const pct = p && p.total > 0 ? percent(p.received, p.total) : null;
            return (
              <div key={r.file} className="hub-file">
                <div className="hub-file-what">
                  <strong>{r.quant}</strong>
                  {r.file === found.file && <span className="badge">{t("From your link")}</span>}
                  {r.file === found.model.recommended && r.file !== found.file && <span className="badge" title={t("The best fit for this PC")}>★ {t("Best fit")}</span>}
                  <span className="muted small ellipsis" title={r.file}>{baseName(r.file)}</span>
                  <span className={`small block ${r.runs ? "muted" : "warn-text"}`}>
                    {bytes(r.size)} · {r.what}
                  </span>
                </div>
                {p ? (
                  <span className="small muted">{pct !== null ? t("Downloading · {n}%", { n: Math.round(pct) }) : t("Setting up…")}</span>
                ) : isInstalled(r.modelId) ? (
                  <span className="badge subtle">{t("Installed")}</span>
                ) : (
                  <button className="btn small" disabled={installing || (found.kind === "media" && !r.runs)} title={installing ? t("One install at a time") : undefined} onClick={() => install(r)}>
                    {r.runs || found.kind === "media" ? t("Install") : t("Install anyway")}
                  </button>
                )}
              </div>
            );
          })}
          {rows.length > 1 && (
            <button className="link small" onClick={() => setAll(!all)}>
              {all ? t("Show only the best fit") : t("All {n} versions", { n: rows.length })}
            </button>
          )}
        </article>
      )}
    </div>
  );
}

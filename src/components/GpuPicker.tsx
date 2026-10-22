// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useEffect, useRef, useState } from "react";
import { api, errorText, on, type CardView, type EngineStatus, type GpuMenu, type PlaceReason } from "../api";
import { bytes } from "../format";
import { t } from "../i18n";
import { openSettings } from "./ConnectivityMenu";
import type { PushToast } from "./Toasts";

/** Status bar: the graphics card the chat model runs on, by name ("Processor
 * only" when it runs on none, "partly on the processor" when some of it
 * does). Clicking it lists every card found, with its memory and the one in
 * use: picking one makes it the main card (a loaded model starts again on
 * it), and "Scan for graphics cards" looks again. When the model runs on the
 * processor, the menu says why, with a fix where there is one. */
export function GpuPicker({ engine, onEngineChanged, toast }: { engine: EngineStatus | null; onEngineChanged: () => void; toast: PushToast }) {
  const [menu, setMenu] = useState<GpuMenu | null>(null);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState<"scan" | "pick" | null>(null);
  const ref = useRef<HTMLDivElement>(null);
  const load = useCallback(() => api.gpuMenu().then(setMenu).catch(() => {}), []);

  // Again whenever the model loads, unloads or moves.
  const loadedKey = `${engine?.model_id ?? ""}|${engine?.gpu_layers ?? ""}|${engine?.layers_on_card ?? ""}|${engine?.device ?? ""}`;
  useEffect(() => {
    load();
  }, [load, loadedKey]);
  useEffect(() => {
    const subs = [
      on("hardware:changed", () => load()),
      on("gpu:limit", () => load()),
      on("perf:heat", () => load()),
      on("engine:unloaded", () => load()),
      on("gpu:main", (p) => {
        load();
        if (p.reloaded !== undefined) onEngineChanged();
      }),
    ];
    window.addEventListener("perf-changed", load);
    // Plugged in or unplugged (Cool & quiet on battery): looked at again now
    // and then, not while the window is hidden.
    const timer = setInterval(() => !document.hidden && load(), 30_000);
    return () => {
      subs.forEach((s) => s.then((un) => un()));
      window.removeEventListener("perf-changed", load);
      clearInterval(timer);
    };
  }, [load, onEngineChanged]);

  useEffect(() => {
    if (!open) return;
    load();
    const close = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) setOpen(false);
    };
    const esc = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", esc);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", esc);
    };
  }, [open, load]);

  if (!menu) return null;
  const runs = menu.runs;
  const main = menu.cards.find((c) => c.main);
  const usable = menu.cards.filter((c) => c.kind === "card");
  const cardName = runs.device ?? main?.name;
  const onProcessor = runs.place === "processor" || (!runs.place && !main) || !cardName;
  const hasCard = menu.cards.some((c) => c.kind !== "built_in");
  const label = onProcessor ? t("Processor only") : cardName;

  const pick = async (name: string | null) => {
    setBusy("pick");
    try {
      setMenu(await api.setMainGpu(name));
      setOpen(false);
      const loaded = !!runs.place;
      toast(
        name
          ? loaded
            ? t("Models now run on {name}. The model is starting again on it.", { name })
            : t("Models now run on {name}.", { name })
          : t("Models now use your graphics cards together (those from the same maker as the biggest)."),
        "success",
      );
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(null);
    }
  };

  const scan = async () => {
    setBusy("scan");
    try {
      await api.refreshHardware();
      await load();
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(null);
    }
  };

  const fix = async (f: PlaceReason["fix"]) => {
    if (f === "recheck") return scan();
    setOpen(false);
    if (f === "perf_settings") openSettings("performance");
    else if (f === "advanced_settings") openSettings("advanced");
    else if (f === "gpu_full") {
      try {
        await api.setGpuLimit(100);
        toast(t("From the next reply: up to {n}% of the model on the graphics card, and the processor held back to match.", { n: 100 }), "success");
        load();
      } catch (e) {
        toast(errorText(e), "error");
      }
    }
  };

  return (
    <div className="gpu-picker" ref={ref}>
      <button
        className={`status-item perf-pill gpu-pick ${onProcessor && hasCard && runs.place ? "hot" : ""}`}
        onClick={() => setOpen((o) => !o)}
        aria-haspopup="menu"
        aria-expanded={open}
        title={t("The graphics card the AI models run on. Click to change it or scan for others.")}
      >
        <span className="gpu-pick-name">{label}</span>
        {runs.place === "partly" && <span className="muted">· {t("Partly on the processor")}</span>}
      </button>
      {open && (
        <div className="gpu-menu" role="menu" aria-label={t("Graphics card for AI models")}>
          <div className="gpu-menu-head">
            <strong>{t("Graphics card for AI models")}</strong>
            {runs.place && runs.reported && runs.layers_on_card !== null && runs.layers_total !== null && (
              <span className="small muted block">{t("Now: {n} of {total} layers on the graphics card.", { n: runs.layers_on_card, total: runs.layers_total })}</span>
            )}
          </div>
          {usable.length > 1 && (
            <button role="menuitemradio" aria-checked={!menu.chosen} className={`gpu-card ${!menu.chosen ? "active" : ""}`} disabled={busy !== null} onClick={() => pick(null)}>
              <strong>{t("Automatic")}</strong>
              <span className="small muted block">{t("Your cards together: a big model is spread over them")}</span>
            </button>
          )}
          {menu.cards.map((c) =>
            c.kind === "card" ? (
              <button
                key={`card-${c.name}`}
                role="menuitemradio"
                aria-checked={c.main}
                className={`gpu-card ${c.main ? "active" : ""}`}
                disabled={busy !== null}
                onClick={() => pick(c.name)}
              >
                <strong>{c.name}</strong>
                <span className="small muted block">
                  {t("{size} memory", { size: bytes(c.memory) })}
                  {c.main && ` · ${runs.place === "card" || runs.place === "partly" ? t("In use") : t("Main card")}`}
                </span>
              </button>
            ) : (
              <div key={`${c.kind}-${c.name}`} className="gpu-card off" role="menuitem" aria-disabled>
                <strong>{c.kind === "no_driver" && c.vendor ? t("{maker} graphics card ({name})", { maker: c.vendor, name: c.name }) : c.name}</strong>
                <span className={`small block ${c.kind === "no_driver" ? "warn-text" : "muted"}`}>{offText(c)}</span>
              </div>
            ),
          )}
          {menu.cards.length === 0 && <p className="small muted gpu-menu-note">{t("No graphics card was found. Models run on the processor.")}</p>}
          {runs.place && runs.place !== "card" && runs.reasons.length > 0 && (
            <div className="gpu-reasons">
              <strong className="small">{runs.place === "processor" ? t("Why the model runs on the processor") : t("Why part of the model runs on the processor")}</strong>
              <ul>
                {runs.reasons.map((r) => (
                  <li key={r.code} className="small">
                    {reasonText(r, runs.layers_total)}
                    {r.fix && (
                      <button className="link" disabled={busy !== null} onClick={() => fix(r.fix)}>
                        {fixLabel(r.fix)}
                      </button>
                    )}
                  </li>
                ))}
              </ul>
            </div>
          )}
          <div className="gpu-menu-foot">
            <button className="btn ghost small" disabled={busy !== null} onClick={scan}>
              {busy === "scan" ? t("Scanning…") : t("Scan for graphics cards")}
            </button>
          </div>
        </div>
      )}
    </div>
  );
}

/** A card models can't use, and what to do about it. */
function offText(c: CardView): string {
  if (c.kind === "built_in") return t("Built-in graphics: it shares system memory and isn't used for models.");
  if (c.code === 22) return t("Turned off in Device Manager. Turn it on there, then scan again.");
  if (c.code === 0 || c.code === 28 || c.code === null) return t("Found, but it has no driver, so models can't use it. Update its driver, then scan again.");
  return t("Found, but Windows stopped it (Device Manager code {code}). Update its driver, then scan again.", { code: c.code });
}

function fixLabel(f: NonNullable<PlaceReason["fix"]>): string {
  switch (f) {
    case "gpu_full":
      return t("Use the graphics card fully");
    case "recheck":
      return t("Scan for graphics cards");
    case "perf_settings":
      return t("Open Performance settings");
    case "advanced_settings":
      return t("Open Advanced settings");
  }
}

/** The window's wording for a reason (placement.rs `reasons`). */
function reasonText(r: PlaceReason, total: number | null): string {
  const card = r.card ?? t("The graphics card");
  const n = r.number ?? 0;
  // A reason this window has no wording for yet: the core's own.
  const fallback = r.message;
  switch (r.code) {
    case "no_driver": {
      const which = r.maker ? t("{maker} graphics card ({name})", { maker: r.maker, name: card }) : card;
      if (n === 22) return t("{card} is turned off in Device Manager, so models run on the processor. Turn it on there, then scan again.", { card: which });
      if (n === 0 || n === 28) return t("{card} has no driver, so models run on the processor. Update its driver from the card maker's site (or the laptop maker's), then scan again.", { card: which });
      return t("Windows stopped {card} (Device Manager code {code}), so models run on the processor. Update its driver, then scan again.", { card: which, code: n });
    }
    case "no_card":
      return r.card
        ? t("Only built-in graphics were found ({name}), and models don't use those. If this PC has a separate graphics card, it may be switched off (some laptops have an eco or built-in-only graphics mode) or missing its driver.", { name: r.card })
        : t("No graphics card was found. Models run on the processor.");
    case "engine_no_card":
      return t("The AI engine couldn't use {card}. Updating its driver from the card maker's site usually fixes this.", { card });
    case "gpu_layers":
      return t("Advanced settings put {n} of the model's layers on the graphics card.", { n });
    case "gpu_limit":
      return n === 0
        ? t("The GPU limit at the top of the chat is Off, so the model runs on the processor only.")
        : t("The GPU limit at the top of the chat is {n}%, so the rest runs on the processor.", { n });
    case "cool_on_battery":
      return t("Cool & quiet runs while the laptop is on battery and leaves part of the graphics card free.");
    case "cool_mode":
      return t("Cool & quiet mode leaves part of the graphics card free.");
    case "too_small":
      if (n === 0) return t("{card} is too small for this model. A smaller model, or keeping less of the chat in mind, fits it.", { card });
      if (total !== null && n >= total) return t("{card} holds every layer of this model but not all its experts; those wait in system memory. A smaller model fits it whole.", { card });
      return t("{card} holds {n} of this model's {total} layers; the rest runs on the processor. A smaller model, or keeping less of the chat in mind, fits it whole.", { card, n, total: total ?? "?" });
    case "window":
      return t("This chat keeps {k}K of conversation in mind, so {card} holds {n} of the model's {total} layers and the rest runs on the processor. A smaller memory setting (the meter at the top of the chat) fits it whole.", { k: r.ctx_k ?? "?", card, n, total: total ?? "?" });
    default:
      return fallback;
  }
}

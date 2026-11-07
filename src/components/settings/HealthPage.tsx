// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { api, errorText } from "../../api";
import { t } from "../../i18n";
import type { PushToast } from "../Toasts";

type State = "checking" | "ok" | "warn" | "bad" | "off";

interface Check {
  id: string;
  label: string;
  state: State;
  detail: string;
  /** What the Fix button does, when there is something to do. */
  fix?: { label: string; run: () => Promise<void> | void };
}

const GB = 1024 ** 3;

async function deviceCheck(kind: "audioinput" | "videoinput"): Promise<{ state: State; detail: string }> {
  try {
    const all = await navigator.mediaDevices.enumerateDevices();
    const found = all.filter((d) => d.kind === kind);
    if (found.length === 0) return { state: "bad", detail: kind === "audioinput" ? t("No microphone found. Plug one in or check Windows sound settings.") : t("No camera found.") };
    return { state: "ok", detail: t("{n} found.", { n: found.length }) };
  } catch {
    return { state: "warn", detail: t("Couldn't look. Windows may be blocking access.") };
  }
}

/** Plain checks of what the app depends on, each with a Fix where one exists. */
export function HealthPage({ toast, goTo }: { toast: PushToast; goTo: (section: string) => void }) {
  const [checks, setChecks] = useState<Check[]>([]);
  const [round, setRound] = useState(0);

  useEffect(() => {
    let live = true;
    const set = (c: Check) => live && setChecks((prev) => [...prev.filter((p) => p.id !== c.id), c]);
    const base = (id: string, label: string) => set({ id, label, state: "checking", detail: t("Checking...") });
    base("mic", t("Microphone"));
    base("camera", t("Camera"));
    base("network", t("Internet"));
    base("relay", t("Phone relay"));
    base("tor", t("Private connection (Tor)"));
    base("gpu", t("Graphics card"));
    base("disk", t("Disk space"));

    const askMic = {
      label: t("Allow microphone"),
      run: async () => {
        try {
          const s = await navigator.mediaDevices.getUserMedia({ audio: true });
          s.getTracks().forEach((tr) => tr.stop());
          setRound((r) => r + 1);
        } catch {
          toast(t("Windows turned it down. Open Windows Settings, Privacy, Microphone, and allow desktop apps."), "error");
        }
      },
    };
    deviceCheck("audioinput").then((r) => set({ id: "mic", label: t("Microphone"), ...r, fix: r.state === "ok" ? undefined : askMic }));
    deviceCheck("videoinput").then((r) => set({ id: "camera", label: t("Camera"), ...r }));

    const online = navigator.onLine;
    set({ id: "network", label: t("Internet"), state: online ? "ok" : "warn", detail: online ? t("This PC is online. The app goes online only if Connectivity allows it.") : t("This PC looks offline. Check Wi-Fi or the cable."), fix: online ? undefined : { label: t("Open Connectivity"), run: () => goTo("connectivity") } });

    api.phoneView().then(
      (v) => {
        if (!v.enabled) return set({ id: "relay", label: t("Phone relay"), state: "off", detail: t("Phone use is off."), fix: { label: t("Set up"), run: () => goTo("phone") } });
        if (!v.anywhere) return set({ id: "relay", label: t("Phone relay"), state: "off", detail: t("Phone works at home only.") });
        const ok = v.link.connected;
        set({ id: "relay", label: t("Phone relay"), state: ok ? "ok" : "bad", detail: ok ? t("Connected.") : v.link.problem ?? t("Not connected."), fix: ok ? undefined : { label: t("Turn off and on"), run: async () => { await api.setPhoneLink(false); await api.setPhoneLink(true); setRound((r) => r + 1); } } });
      },
      (e) => set({ id: "relay", label: t("Phone relay"), state: "warn", detail: errorText(e) }),
    );

    api.privacyView().then(
      (v) => {
        const state: State = v.state === "off" ? "off" : v.state === "ready" || v.state === "waiting" ? "ok" : v.state === "stuck" ? "bad" : "checking";
        set({ id: "tor", label: t("Private connection (Tor)"), state, detail: v.state === "off" ? t("Off.") : v.problem || t("{p}% started.", { p: v.progress }), fix: state === "bad" ? { label: t("Test again"), run: async () => { await api.testPrivacy().catch(() => {}); setRound((r) => r + 1); } } : undefined });
      },
      (e) => set({ id: "tor", label: t("Private connection (Tor)"), state: "warn", detail: errorText(e) }),
    );

    api.refreshHardware().then(
      (h) => {
        const card = h.gpus.find((g) => !g.integrated) ?? h.gpus[0];
        const broken = h.broken_cards.length > 0;
        set({ id: "gpu", label: t("Graphics card"), state: broken ? "bad" : card ? "ok" : "warn", detail: broken ? t("Windows lists a card that isn't working. Updating its driver usually fixes it.") : card ? t("{name}, {gb} GB.", { name: card.name, gb: Math.round(card.vram_bytes / GB) }) : t("No graphics card found; models run on the processor, more slowly."), fix: broken ? { label: t("Open Check this PC"), run: () => goTo("check") } : undefined });
        const freeGb = h.disk_free / GB;
        set({ id: "disk", label: t("Disk space"), state: freeGb < 10 ? "bad" : freeGb < 40 ? "warn" : "ok", detail: t("{free} GB free on {drive}.", { free: Math.round(freeGb), drive: h.models_drive }), fix: freeGb < 40 ? { label: t("Free up space"), run: () => goTo("cleanup") } : undefined });
      },
      (e) => {
        set({ id: "gpu", label: t("Graphics card"), state: "warn", detail: errorText(e) });
        set({ id: "disk", label: t("Disk space"), state: "warn", detail: errorText(e) });
      },
    );
    return () => {
      live = false;
    };
  }, [round]);

  const order = ["mic", "camera", "network", "relay", "tor", "gpu", "disk"];
  const sorted = [...checks].sort((a, b) => order.indexOf(a.id) - order.indexOf(b.id));
  const mark: Record<State, string> = { checking: t("Checking"), ok: t("Working"), warn: t("Look"), bad: t("Problem"), off: t("Off") };
  return (
    <section className="card health">
      <h2>{t("Health")}</h2>
      <p className="muted small">{t("Checks the things the app depends on. Press Fix where something needs a hand.")}</p>
      <ul className="health-list">
        {sorted.map((c) => (
          <li key={c.id} className={`health-row ${c.state}`}>
            <span className={`health-dot ${c.state}`} aria-hidden />
            <span className="grow">
              <strong>{c.label}</strong> <span className="badge subtle">{mark[c.state]}</span>
              <span className="muted small block">{c.detail}</span>
            </span>
            {c.fix && <button className="btn small" onClick={() => c.fix!.run()}>{c.fix.label}</button>}
          </li>
        ))}
      </ul>
      <button className="btn ghost small" onClick={() => setRound((r) => r + 1)}>{t("Check again")}</button>
    </section>
  );
}

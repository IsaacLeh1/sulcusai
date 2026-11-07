// SPDX-License-Identifier: AGPL-3.0-only
import { useState, type ReactNode } from "react";
import { api, errorText, type Connectivity, type EngineStatus, type ModelCard, type Settings } from "../api";
import { t } from "../i18n";
import { clock } from "../format";
import { ConnectivityMenu } from "../components/ConnectivityMenu";
import { GpuPill, PerfPill } from "../components/Performance";
import { PauseButton } from "../components/Quiet";
import { GpuPicker } from "../components/GpuPicker";
import { ModelPicker } from "../components/ModelPicker";
import { UpdatePill } from "../components/Updates";
import { PrivacyPill } from "../components/PrivacySettings";
import { WorkBar } from "../components/WorkBar";
import { LineupBar } from "../components/LineupBar";
import { NetworkStatus } from "../components/NetworkStatus";
import type { PushToast } from "../components/Toasts";
import type { View } from "../App";


export interface StatusBarProps {
  settings: Settings;
  setSettings: (s: Settings) => void;
  engine: EngineStatus | null;
  installed: ModelCard[];
  loadedName: string | undefined;
  keptName: string | null;
  defaultName: string | undefined;
  modelNow: { busy: boolean; idle_secs: number; unloads_in: number | null } | null;
  engineDl: { received: number; total: number } | null;
  liveMeeting: string | null;
  onEngineChanged: () => void;
  onChangeConnectivity: (level: Connectivity) => void;
  onPickDefault: (id: string) => void;
  setView: (v: View) => void;
  toast: PushToast;
}

/** The bar along the bottom: connectivity, the loaded model, the GPU, and the small pills. */
export function StatusBar(p: StatusBarProps) {
  const { settings, setSettings, engine, loadedName, keptName, defaultName, modelNow, engineDl, liveMeeting, onEngineChanged, toast, setView } = p;
  const [loadingNow, setLoadingNow] = useState(false);
  const picker = (label: ReactNode) => (
    <ModelPicker
      label={label}
      installed={p.installed}
      current={settings.default_model}
      onPick={p.onPickDefault}
      onModels={() => setView("models")}
      auto={settings.auto_model ?? true}
      onAuto={async (on) => {
        try {
          setSettings(await api.setAutoModel(on));
        } catch (e) {
          toast(errorText(e), "error");
        }
      }}
    />
  );
  return (
    <footer className="statusbar">
      <ConnectivityMenu level={settings.connectivity} onChange={p.onChangeConnectivity} onRefresh={() => api.settings().then(setSettings).catch(() => {})} />
      <span
        className="status-item"
        title={
          loadedName
            ? keptName
              ? t("The model in memory, with {other} still loaded beside it (switching back needs no loading)", { other: keptName })
              : t("The model in memory")
            : t("Models load when you send a message and unload after a while idle, to free memory")
        }
      >
        {engineDl ? (
          <>
            <span className="led busy" /> {t("Getting the AI engine (first time only)")}{engineDl.total ? ` · ${Math.round((engineDl.received / engineDl.total) * 100)}%` : "…"}
          </>
        ) : loadedName ? (
          <>
            <span className="led on" /> {picker(t("{name} loaded", { name: loadedName }))}
            {keptName && <span className="muted"> {t("+ {name}", { name: keptName })}</span>}
            {modelNow && (
              <span className="muted" title={t("A loaded model keeps memory until it's unloaded. When it unloads by itself is set in Settings › Performance.")}>
                {modelNow.busy
                  ? t("· working")
                  : `· ${t("idle {time}", { time: clock(modelNow.idle_secs) })}${modelNow.unloads_in == null ? ` · ${t("stays loaded")}` : ` · ${t("unloads in about {time}", { time: clock(modelNow.unloads_in) })}`}`}
              </span>
            )}
            <button className="link" onClick={async () => { await api.unload(); onEngineChanged(); }}>
              {t("Unload")}
            </button>
          </>
        ) : defaultName ? (
          <>
            <span className="led" /> {loadingNow ? t("Loading {name}…", { name: defaultName }) : picker(t("{name} · loads when you chat", { name: defaultName }))}
            {!loadingNow && (
              <button
                className="link"
                onClick={async () => {
                  setLoadingNow(true);
                  try {
                    await api.loadDefault();
                  } catch (e) {
                    toast(errorText(e), "error");
                  } finally {
                    setLoadingNow(false);
                    onEngineChanged();
                  }
                }}
              >
                {t("Load now")}
              </button>
            )}
          </>
        ) : (
          <>
            <span className="led" /> {t("No model installed")}
          </>
        )}
      </span>
      <GpuPicker engine={engine} onEngineChanged={onEngineChanged} toast={toast} />
      {liveMeeting && (
        <button className="status-item rec-pill" onClick={() => setView("meetings")} title={t("A meeting is being recorded")}>
          <span className="rec-dot on" /> {t("Recording meeting")}
        </button>
      )}
      <PerfPill toast={toast} />
      <PauseButton toast={toast} />
      <GpuPill onOpen={() => setView("settings")} />
      <WorkBar onOpen={(page) => setView(page)} />
      <LineupBar toast={toast} />
      <UpdatePill toast={toast} />
      <PrivacyPill onOpen={() => setView("settings")} />
      <NetworkStatus toast={toast} setView={setView} />
      <span className="spacer" />
      <span className="status-item muted">
        {t("Encrypted ·")}{" "}
        {settings.connectivity === "cloud" ? t("AI runs on this PC unless you pick a cloud model") : t("AI runs only on this PC")}
      </span>
    </footer>
  );
}

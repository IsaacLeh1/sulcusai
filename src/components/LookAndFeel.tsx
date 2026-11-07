// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { t } from "../i18n";
import { desktop } from "../ui/desktop";
import { DEFAULT_SHORTCUTS, setUiPrefs, TOP_TABS, useUiPrefs, type TopTab } from "../ui/prefs";
import { TAB_LABEL } from "./TopTabs";
import { SliderRow, ToggleRow } from "./UiKit";

const SHORTCUT_NAMES: Record<string, string> = {
  commandBar: "Command bar",
  newChat: "New chat",
  closeTab: "Close tab",
  nextTab: "Next tab",
  prevTab: "Previous tab",
  settings: "Settings",
  home: "Home",
};

/** Density, contrast, motion, sounds, which tabs show and the keyboard shortcuts. Ready to sit in Settings. */
export function LookAndFeel() {
  const p = useUiPrefs();
  const [startup, setStartup] = useState(false);
  const [startupError, setStartupError] = useState<string | null>(null);
  useEffect(() => {
    desktop.startsWithWindows().then(setStartup);
  }, []);
  const move = (x: string, by: number) => {
    const list = [...p.tabs];
    const i = list.indexOf(x);
    const j = i + by;
    if (i < 0 || j < 0 || j >= list.length) return;
    [list[i], list[j]] = [list[j], list[i]];
    setUiPrefs({ tabs: list });
  };
  return (
    <div className="ui-stack">
      <ToggleRow label={t("Compact")} help={t("Less room between rows, more on screen.")} checked={p.density === "compact"} onChange={(v) => setUiPrefs({ density: v ? "compact" : "comfortable" })} />
      <ToggleRow label={t("High contrast")} help={t("Stronger text and borders.")} checked={p.contrast} onChange={(v) => setUiPrefs({ contrast: v })} />
      <ToggleRow label={t("Colour-blind-safe colours")} help={t("Status shows by shape as well as colour, in blue and orange.")} checked={p.colorSafe} onChange={(v) => setUiPrefs({ colorSafe: v })} />
      <ToggleRow label={t("Reduce motion")} help={t("No sliding or fading. Also on when Windows asks for it.")} checked={p.reducedMotion} onChange={(v) => setUiPrefs({ reducedMotion: v })} />
      <ToggleRow label={t("Easier-to-read font")} help={t("Wider letters and lines, for dyslexia and low vision.")} checked={p.dyslexiaFont} onChange={(v) => setUiPrefs({ dyslexiaFont: v })} />
      <ToggleRow label={t("Calm mode")} help={t("Fewer badges and counts.")} checked={p.calm} onChange={(v) => setUiPrefs({ calm: v })} />
      <ToggleRow label={t("Sounds")} help={t("Short tones for done, errors, joins and mentions.")} checked={p.sounds} onChange={(v) => setUiPrefs({ sounds: v })} />
      {p.sounds && <SliderRow label={t("Volume")} value={Math.round(p.volume * 100)} min={0} max={100} reset={50} unit="%" onChange={(v) => setUiPrefs({ volume: v / 100 })} />}
      <ToggleRow label={t("Sidebar as icons")} checked={p.sidebarIcons} onChange={(v) => setUiPrefs({ sidebarIcons: v })} />
      <ToggleRow
        label={t("Start with Windows")}
        help={startupError ?? t("Waits in the tray, ready for scheduled tasks and messages.")}
        checked={startup}
        onChange={(v) =>
          desktop
            .startWithWindows(v)
            .then(() => (setStartup(v), setStartupError(null)))
            .catch((e) => setStartupError(String(e)))
        }
      />
      <ToggleRow label={t("Keep in the tray when closed")} help={t("Closing the window leaves SulcusAI running in the tray.")} checked={p.keepInTray} onChange={(v) => setUiPrefs({ keepInTray: v })} />
      <h4>{t("Tabs")}</h4>
      {p.tabs.map((x) => (
        <div key={x} className="ui-row">
          <label className="ui-grow">
            <input type="checkbox" checked={!p.hiddenTabs.includes(x)} disabled={x === "chat" || x === "settings"} title={t("Chat and Settings always show")} onChange={(e) => setUiPrefs({ hiddenTabs: e.target.checked ? p.hiddenTabs.filter((h) => h !== x) : [...p.hiddenTabs, x] })} />{" "}
            {(TOP_TABS as readonly string[]).includes(x) ? t(TAB_LABEL[x as TopTab]) : x}
          </label>
          <button className="icon-btn" aria-label={t("Move up")} onClick={() => move(x, -1)}>↑</button>
          <button className="icon-btn" aria-label={t("Move down")} onClick={() => move(x, 1)}>↓</button>
        </div>
      ))}
      <h4>{t("Keyboard shortcuts")}</h4>
      {Object.keys(DEFAULT_SHORTCUTS).map((k) => (
        <label key={k} className="ui-row">
          <span className="ui-grow">{t(SHORTCUT_NAMES[k] ?? k)}</span>
          <input
            className="ui-key-input"
            readOnly
            value={p.shortcuts[k] ?? ""}
            title={t("Click, then press the keys")}
            onKeyDown={(e) => {
              if (["Control", "Shift", "Alt", "Meta", "Tab"].includes(e.key)) return;
              e.preventDefault();
              const combo = [e.ctrlKey && "Ctrl", e.shiftKey && "Shift", e.altKey && "Alt", e.key.length === 1 ? e.key.toUpperCase() : e.key].filter(Boolean).join("+");
              setUiPrefs({ shortcuts: { ...p.shortcuts, [k]: combo } });
            }}
          />
          {p.shortcuts[k] !== DEFAULT_SHORTCUTS[k] && (
            <button className="link-btn small" onClick={() => setUiPrefs({ shortcuts: { ...p.shortcuts, [k]: DEFAULT_SHORTCUTS[k] } })}>
              {t("Reset")}
            </button>
          )}
        </label>
      ))}
    </div>
  );
}

// SPDX-License-Identifier: AGPL-3.0-only
// The phone as a remote for the PC's call (phone/controls.rs): mute,
// deafen, switch voice channel and leave, while the PC runs a game, say.
// Shown on the phone only; the Social view mounts it where calls appear.
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { errorText } from "../api";
import { t } from "../i18n";
import { onPhone } from "./env";

type Call = { call: string; group: string; channel: string; people: [string, string][] };
type State = { call: Call | null; muted: boolean; deafened: boolean };
export type VoiceChannel = { group: string; channel: string; name: string };

export function PhoneRemote({ channels = [] }: { channels?: VoiceChannel[] }) {
  const [s, setS] = useState<State | null>(null);
  const [error, setError] = useState("");
  const load = () =>
    invoke<State>("remote_controls")
      .then(setS)
      .catch((e) => setError(errorText(e)));
  useEffect(() => {
    if (!onPhone()) return;
    load();
    const id = window.setInterval(load, 4000);
    return () => window.clearInterval(id);
  }, []);
  if (!onPhone() || !s?.call) return null;
  const run = (cmd: string, args: Record<string, unknown> = {}) =>
    invoke(cmd, args)
      .then(() => window.setTimeout(load, 600))
      .catch((e) => setError(errorText(e)));
  return (
    <section className="card">
      <h3>{t("The PC's call")}</h3>
      <p className="muted small">{t("{n} in the call", { n: s.call.people.length })}</p>
      <div className="row wrap">
        <button className={`btn${s.muted ? " primary" : ""}`} onClick={() => run("remote_mute", { on: !s.muted })}>
          {s.muted ? t("Unmute the PC") : t("Mute the PC")}
        </button>
        <button className={`btn${s.deafened ? " primary" : ""}`} onClick={() => run("remote_deafen", { on: !s.deafened })}>
          {s.deafened ? t("Undeafen") : t("Deafen")}
        </button>
        <button className="btn ghost" onClick={() => run("remote_leave")}>{t("Leave")}</button>
      </div>
      {channels.length > 0 && (
        <label className="stack">
          <span className="small">{t("Switch voice channel")}</span>
          <select
            value={`${s.call.group}/${s.call.channel}`}
            onChange={(e) => {
              const [group, channel] = e.target.value.split("/");
              run("remote_switch_channel", { group, channel });
            }}
          >
            {channels.map((c) => (
              <option key={`${c.group}/${c.channel}`} value={`${c.group}/${c.channel}`}>
                {c.name}
              </option>
            ))}
          </select>
        </label>
      )}
      {error && <p className="small error">{error}</p>}
    </section>
  );
}

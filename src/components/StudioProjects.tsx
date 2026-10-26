// SPDX-License-Identifier: AGPL-3.0-only
// The song studio's projects, in the gallery: open one, start an empty one, delete.
import { useEffect, useState } from "react";
import { api, errorText, type StudioProject } from "../api";
import { t } from "../i18n";
import { newTrack, type Project } from "../studio/model";
import type { PushToast } from "./Toasts";

export function StudioProjects({ onOpen, toast }: { onOpen: (p: Project) => void; toast: PushToast }) {
  const [list, setList] = useState<StudioProject[] | null>(null);
  const [deleting, setDeleting] = useState<string | null>(null);
  const load = () => api.studioProjects().then(setList).catch((e) => toast(errorText(e), "error"));
  useEffect(() => {
    load();
  }, []); // eslint-disable-line react-hooks/exhaustive-deps
  const empty = async () => {
    const now = Date.now();
    const p: Project = { id: `pj${now.toString(36)}`, name: t("New project"), bpm: 120, beats: 4, tracks: [newTrack(t("Audio"), "audio", 0), newTrack(t("Keys"), "midi", 1)], loop: null, created_at: now, updated_at: now };
    try {
      await api.studioProjectSave(p);
      onOpen(p);
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  if (!list) return null;
  return (
    <div className="playlists">
      <div className="row wrap">
        <button className="btn small primary" onClick={empty}>{t("New empty project")}</button>
        <span className="small muted">{t("Or open a song's page and press Open in the song studio.")}</span>
      </div>
      {!list.length && <p className="muted small">{t("No song studio projects yet.")}</p>}
      <ul className="plain-list">
        {list.map((p) => (
          <li key={p.id} className="row">
            <button className="link" onClick={() => onOpen(p)}>{p.name}</button>
            <span className="small muted">{t("{n} tracks · {date}", { n: p.tracks.length, date: new Date(p.updated_at).toLocaleString() })}</span>
            <span className="spacer" />
            {deleting === p.id ? (
              <>
                <button className="btn small" onClick={() => setDeleting(null)}>{t("Cancel")}</button>
                <button className="btn small danger-fill" onClick={() => api.studioProjectDelete(p.id).then(() => { setDeleting(null); load(); }).catch((e) => toast(errorText(e), "error"))}>{t("Delete")}</button>
              </>
            ) : (
              <button className="btn ghost danger small" onClick={() => setDeleting(p.id)}>{t("Delete")}</button>
            )}
          </li>
        ))}
      </ul>
    </div>
  );
}

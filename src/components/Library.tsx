// SPDX-License-Identifier: AGPL-3.0-only
// The song library beyond the gallery: playlists, and playing songs one
// after another.
import { useCallback, useEffect, useRef, useState } from "react";
import { api, errorText, type MediaItem, type Playlist } from "../api";
import { clock } from "../format";
import { t } from "../i18n";
import { fileUrl } from "../media";
import type { PushToast } from "./Toasts";

export function usePlaylists(): [Playlist[], () => void] {
  const [lists, setLists] = useState<Playlist[]>([]);
  const load = useCallback(() => {
    api.musicPlaylists().then(setLists).catch(() => {});
  }, []);
  useEffect(load, [load]);
  return [lists, load];
}

/** "Add to a playlist": an existing one, or a new one named here. */
export function AddToPlaylist({ item, toast }: { item: MediaItem; toast: PushToast }) {
  const [lists, reload] = usePlaylists();
  const [naming, setNaming] = useState<string | null>(null);
  const add = async (p: Pick<Playlist, "id" | "name" | "songs">) => {
    try {
      const saved = await api.musicPlaylistSave({ ...p, songs: [...p.songs.filter((s) => s !== item.id), item.id] });
      toast(t("Added to {name}.", { name: saved.name }), "success");
      setNaming(null);
      reload();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  if (naming !== null)
    return (
      <div className="row small">
        <input className="input" value={naming} onChange={(e) => setNaming(e.target.value)} onKeyDown={(e) => e.key === "Enter" && naming.trim() && add({ id: "", name: naming, songs: [] })} placeholder={t("Playlist name")} aria-label={t("Playlist name")} autoFocus />
        <button className="btn small primary" disabled={!naming.trim()} onClick={() => add({ id: "", name: naming, songs: [] })}>{t("Add")}</button>
        <button className="btn small" onClick={() => setNaming(null)}>{t("Cancel")}</button>
      </div>
    );
  return (
    <select
      value=""
      aria-label={t("Add to a playlist")}
      onChange={(e) => {
        const v = e.target.value;
        if (v === "+") setNaming("");
        else {
          const p = lists.find((l) => l.id === v);
          if (p) add(p);
        }
      }}
    >
      <option value="">{t("Add to a playlist…")}</option>
      {lists.map((l) => (
        <option key={l.id} value={l.id} disabled={l.songs.includes(item.id)}>
          {l.songs.includes(item.id) ? t("{name} (in it)", { name: l.name }) : l.name}
        </option>
      ))}
      <option value="+">{t("New playlist…")}</option>
    </select>
  );
}

/** Songs played one after another: next on its own when one ends. */
export function QueuePlayer({ songs, start = 0, onClose }: { songs: MediaItem[]; start?: number; onClose: () => void }) {
  const [at, setAt] = useState(start);
  const [src, setSrc] = useState<string | null>(null);
  const audio = useRef<HTMLAudioElement>(null);
  const song = songs[at];
  useEffect(() => setAt(start), [songs, start]);
  useEffect(() => {
    let live = true;
    if (song) fileUrl(song).then((u) => live && setSrc(u)).catch(() => live && setSrc(null));
    return () => {
      live = false;
    };
  }, [song?.id]); // eslint-disable-line react-hooks/exhaustive-deps
  if (!song) return null;
  return (
    <div className="queue-player">
      <div className="row">
        <strong className="ellipsis">{song.title || song.prompt || t("A song")}</strong>
        <span className="small muted">{t("{n} of {total}", { n: at + 1, total: songs.length })}</span>
        <span className="spacer" />
        <button className="btn small" disabled={at === 0} onClick={() => setAt(at - 1)} aria-label={t("Previous")}>{t("Previous")}</button>
        <button className="btn small" disabled={at >= songs.length - 1} onClick={() => setAt(at + 1)} aria-label={t("Next")}>{t("Next")}</button>
        <button className="icon-btn" aria-label={t("Stop playing")} onClick={onClose}>✕</button>
      </div>
      {src && <audio ref={audio} src={src} controls autoPlay controlsList="nodownload" onEnded={() => at < songs.length - 1 && setAt(at + 1)} />}
    </div>
  );
}

/** The playlists: pick one to see, reorder, play or shuffle its songs. */
export function Playlists({ toast, onOpen, onInspire }: { toast: PushToast; onOpen: (i: MediaItem) => void; onInspire: (ids: string[]) => void }) {
  const [lists, reload] = usePlaylists();
  const [openId, setOpenId] = useState<string | null>(null);
  const [songs, setSongs] = useState<MediaItem[]>([]);
  const [playing, setPlaying] = useState<{ songs: MediaItem[]; start: number } | null>(null);
  const [renaming, setRenaming] = useState<string | null>(null);
  const [confirm, setConfirm] = useState(false);
  const open = lists.find((l) => l.id === openId) ?? null;
  useEffect(() => {
    if (!open) return setSongs([]);
    let live = true;
    // A song that's gone from the gallery is left out.
    Promise.all(open.songs.map((id) => api.mediaGet(id).catch(() => null))).then((all) => live && setSongs(all.filter((s): s is MediaItem => !!s)));
    return () => {
      live = false;
    };
  }, [open?.id, open?.updated_at]); // eslint-disable-line react-hooks/exhaustive-deps
  const save = async (p: Pick<Playlist, "id" | "name" | "songs">) => {
    try {
      await api.musicPlaylistSave(p);
      reload();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const move = (i: number, by: number) => {
    if (!open) return;
    const ids = songs.map((s) => s.id);
    const j = i + by;
    if (j < 0 || j >= ids.length) return;
    [ids[i], ids[j]] = [ids[j], ids[i]];
    save({ ...open, songs: ids });
  };
  const total = songs.reduce((n, s) => n + (s.seconds || 0), 0);
  if (!lists.length) return <p className="muted small">{t("No playlists yet. On a song's page, use Add to a playlist.")}</p>;
  return (
    <div className="playlists">
      <div className="filters tight wrap">
        {lists.map((l) => (
          <button key={l.id} className={`chip ${openId === l.id ? "active" : ""}`} onClick={() => setOpenId(openId === l.id ? null : l.id)}>
            {l.name} <span className="muted">· {l.songs.length}</span>
          </button>
        ))}
      </div>
      {open && (
        <div className="playlist">
          <div className="row wrap">
            {renaming !== null ? (
              <>
                <input className="input" value={renaming} onChange={(e) => setRenaming(e.target.value)} aria-label={t("Playlist name")} autoFocus onKeyDown={(e) => e.key === "Enter" && save({ ...open, name: renaming }).then(() => setRenaming(null))} />
                <button className="btn small primary" onClick={() => save({ ...open, name: renaming }).then(() => setRenaming(null))}>{t("Save")}</button>
              </>
            ) : (
              <>
                <strong>{open.name}</strong>
                <span className="small muted">{songs.length === 1 ? t("1 song · {time}", { time: clock(total) }) : t("{n} songs · {time}", { n: songs.length, time: clock(total) })}</span>
                <button className="link small" onClick={() => setRenaming(open.name)}>{t("Rename")}</button>
              </>
            )}
            <span className="spacer" />
            <button className="btn small primary" disabled={!songs.length} onClick={() => setPlaying({ songs, start: 0 })}>{t("Play all")}</button>
            <button className="btn small" disabled={songs.length < 2} onClick={() => setPlaying({ songs: shuffled(songs), start: 0 })}>{t("Shuffle")}</button>
            <button className="btn small" disabled={!songs.length} onClick={() => onInspire(songs.map((s) => s.id))} title={t("The style, mood, instruments and voice these songs share, filled in for a new song")}>
              {t("A new song like these")}
            </button>
            <button className="btn ghost danger small" onClick={() => setConfirm(true)}>{t("Delete playlist")}</button>
          </div>
          {confirm && (
            <div className="callout small row">
              {t("Delete the playlist {name}? Its songs stay in the gallery.", { name: open.name })}
              <span className="spacer" />
              <button className="btn small" onClick={() => setConfirm(false)}>{t("Cancel")}</button>
              <button
                className="btn small danger-fill"
                onClick={() =>
                  api
                    .musicPlaylistDelete(open.id)
                    .then(() => {
                      setConfirm(false);
                      setOpenId(null);
                      reload();
                    })
                    .catch((e) => toast(errorText(e), "error"))
                }
              >
                {t("Delete")}
              </button>
            </div>
          )}
          {playing && <QueuePlayer songs={playing.songs} start={playing.start} onClose={() => setPlaying(null)} />}
          <ol className="playlist-songs">
            {songs.map((s, i) => (
              <li key={s.id} className="row">
                <button className="link ellipsis" onClick={() => onOpen(s)}>{s.title || s.prompt || t("A song")}</button>
                <span className="small muted">{clock(s.seconds)}</span>
                <span className="spacer" />
                <button className="btn ghost small" onClick={() => setPlaying({ songs, start: i })}>{t("Play")}</button>
                <button className="btn ghost small" disabled={i === 0} onClick={() => move(i, -1)} aria-label={t("Move up")}>↑</button>
                <button className="btn ghost small" disabled={i === songs.length - 1} onClick={() => move(i, 1)} aria-label={t("Move down")}>↓</button>
                <button className="btn ghost small" onClick={() => save({ ...open, songs: open.songs.filter((x) => x !== s.id) })}>{t("Remove")}</button>
              </li>
            ))}
          </ol>
        </div>
      )}
    </div>
  );
}

/** A fair shuffle (Fisher-Yates). */
function shuffled<T>(list: T[]): T[] {
  const out = [...list];
  for (let i = out.length - 1; i > 0; i--) {
    const j = Math.floor(Math.random() * (i + 1));
    [out[i], out[j]] = [out[j], out[i]];
  }
  return out;
}

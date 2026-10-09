// SPDX-License-Identifier: AGPL-3.0-only
// Meetings: record a call or an in-person meeting, follow the live
// transcript, and get notes. Past meetings are searchable here.
import { useCallback, useEffect, useRef, useState } from "react";
import { save } from "@tauri-apps/plugin-dialog";
import { api, errorText, on, type Language, type Meeting, type MeetingDetail, type MeetingHit, type Segment } from "../api";
import { clock, duration } from "../format";
import { t, tx } from "../i18n";
import { Modal } from "../components/Modal";
import type { PushToast } from "../components/Toasts";

interface Props {
  live: string | null;
  onLiveChanged: (id: string | null) => void;
  onOpenChat: (id: string) => void;
  onGoModels: () => void;
  toast: PushToast;
}

const STATUS_LABEL: Record<string, string> = {
  recording: tx("Recording"),
  transcribing: tx("Finishing the transcript"),
  summarizing: tx("Writing notes"),
  done: "",
  failed: tx("Didn't finish"),
};

/** "You", a name you gave, "Speaker 2", or "Others". */
export function who(s: Segment, names: Record<string, string>): string {
  if (s.speaker === "you") return t("You");
  if (s.voice !== null) return names[String(s.voice)] ?? t("Speaker {n}", { n: s.voice });
  return t("Others");
}

function when(ms: number): string {
  return new Date(ms).toLocaleString(undefined, { weekday: "short", month: "short", day: "numeric", hour: "numeric", minute: "2-digit" });
}

export function MeetingsView({ live, onLiveChanged, onOpenChat, onGoModels, toast }: Props) {
  const [meetings, setMeetings] = useState<Meeting[] | null>(null);
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<MeetingHit[] | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [starting, setStarting] = useState(false);
  const [canTranscribe, setCanTranscribe] = useState<boolean | null>(null);
  const [finishing, setFinishing] = useState<string | null>(null);

  const refresh = useCallback(() => {
    api.meetings().then(setMeetings).catch((e) => toast(errorText(e), "error"));
  }, [toast]);

  useEffect(() => {
    refresh();
    api.speechView().then((v) => setCanTranscribe(v.models.some((m) => m.installed))).catch(() => setCanTranscribe(false));
    const sub = on("meeting", (e) => {
      if (e.kind === "done" || e.kind === "error" || (e.kind === "state" && e.status !== "loading")) refresh();
    });
    return () => {
      sub.then((un) => un());
    };
  }, [refresh]);

  useEffect(() => {
    const q = query.trim();
    if (!q) {
      setHits(null);
      return;
    }
    const timer = window.setTimeout(() => api.searchMeetings(q).then(setHits).catch(() => {}), 250);
    return () => window.clearTimeout(timer);
  }, [query]);

  if (live) {
    return (
      <div className="page">
        <LiveMeeting
          id={live}
          onEnded={(id) => {
            setFinishing(id);
            onLiveChanged(null);
          }}
          toast={toast}
        />
      </div>
    );
  }
  if (selected || finishing) {
    const id = (selected ?? finishing)!;
    return (
      <div className="page">
        <MeetingPage
          key={id}
          id={id}
          onBack={() => {
            setSelected(null);
            setFinishing(null);
            refresh();
          }}
          onOpenChat={onOpenChat}
          toast={toast}
        />
      </div>
    );
  }

  const list: { meeting: Meeting; lines: string[] }[] = hits ?? (meetings ?? []).map((m) => ({ meeting: m, lines: [] }));
  return (
    <div className="page">
      <div className="narrow">
        <header className="page-head">
          <div>
            <h1>{t("Meetings")}</h1>
            <p className="muted">
              {t("Records your microphone and your computer's sound, so it hears everyone on Teams, Meet, Zoom or in the room, with no bot joining. You get a transcript and notes, all kept on this PC.")}
            </p>
          </div>
          <button className="btn primary" onClick={() => setStarting(true)} disabled={!canTranscribe}>
            {t("● Begin meeting")}
          </button>
        </header>

        {canTranscribe === false && (
          <div className="callout">
            {t("Meetings need a speech model to write the transcript.")}{" "}
            <button className="link" onClick={onGoModels}>{t("Install one from Models › Speech")}</button>
          </div>
        )}

        {(meetings?.length ?? 0) > 0 && (
          <input className="input search" placeholder={t("Search meetings: a topic, a name, a decision…")} value={query} onChange={(e) => setQuery(e.target.value)} aria-label={t("Search meetings")} />
        )}

        {meetings && meetings.length === 0 && canTranscribe && <p className="muted">{t("No meetings yet. Click Begin meeting when your next call starts.")}</p>}
        {hits && hits.length === 0 && <p className="muted">{t("No meetings mention that.")}</p>}

        <div className="meeting-list">
          {list.map(({ meeting: m, lines }) => (
            <button key={m.id} className="card meeting-item" onClick={() => setSelected(m.id)}>
              <div className="row">
                <strong className="ellipsis">{m.title}</strong>
                <span className="spacer" />
                {STATUS_LABEL[m.status] && <span className={`badge ${m.status === "failed" ? "warn" : ""}`}>{t(STATUS_LABEL[m.status])}</span>}
              </div>
              <span className="muted small">
                {when(m.started_at)}
                {m.ended_at && ` · ${duration(m.ended_at - m.started_at)}`}
                {m.notes && m.notes.action_items.length > 0 && ` · ${t("{n} open action items", { n: m.notes.action_items.filter((a) => !a.done).length })}`}
              </span>
              {m.notes?.summary && !lines.length && <span className="small meeting-summary">{m.notes.summary}</span>}
              {lines.map((l, i) => (
                <span key={i} className="small muted block ellipsis">{l}</span>
              ))}
            </button>
          ))}
        </div>
      </div>
      {starting && (
        <StartDialog
          onClose={() => setStarting(false)}
          onStarted={(m) => {
            setStarting(false);
            onLiveChanged(m.id);
          }}
          toast={toast}
        />
      )}
    </div>
  );
}

const ANNOUNCEMENT = tx("Heads up: I'm recording this meeting on my computer to take notes. The recording stays private on my PC.");

function StartDialog({ onClose, onStarted, toast }: { onClose: () => void; onStarted: (m: Meeting) => void; toast: PushToast }) {
  const [title, setTitle] = useState("");
  const [mic, setMic] = useState(true);
  const [system, setSystem] = useState(true);
  const [translate, setTranslate] = useState("");
  const [languages, setLanguages] = useState<Language[]>([]);
  const [busy, setBusy] = useState(false);
  const [labels, setLabels] = useState<boolean | null>(null);
  useEffect(() => {
    api.languages().then(setLanguages).catch(() => {});
    api.speakerModel().then((m) => setLabels(m.installed)).catch(() => {});
  }, []);

  const start = async () => {
    setBusy(true);
    try {
      onStarted(await api.startMeeting({ title: title.trim() || null, mic, system, translate_to: translate || null }));
    } catch (e) {
      toast(errorText(e), "error");
      setBusy(false);
    }
  };

  return (
    <Modal title={t("Begin meeting")} onClose={onClose}>
      <div className="form">
        <label>
          {t("Name")} <span className="muted small">{t("(optional; it's named from the notes otherwise)")}</span>
          <input className="input" value={title} onChange={(e) => setTitle(e.target.value)} placeholder={t("Weekly team sync")} maxLength={120} autoFocus />
        </label>
        <label className="check">
          <input type="checkbox" checked={mic} onChange={(e) => setMic(e.target.checked)} />
          <span>{t("My microphone")} <span className="muted small">{t("(you, and anyone in the room)")}</span></span>
        </label>
        <label className="check">
          <input type="checkbox" checked={system} onChange={(e) => setSystem(e.target.checked)} />
          <span>{t("My computer's sound")} <span className="muted small">{t("(the other people in a call)")}</span></span>
        </label>
        {system && labels === false && (
          <p className="muted small">{t("Tip: install Speaker labels in Models › Speech to tell the other people apart.")}</p>
        )}
        <label>
          {t("Live translated captions")}
          <select value={translate} onChange={(e) => setTranslate(e.target.value)}>
            <option value="">{t("Off")}</option>
            {languages.map((l) => (
              <option key={l.code} value={l.code}>{t("Into {language}", { language: l.name })}</option>
            ))}
          </select>
        </label>
        <div className="callout small">
          <strong>{t("Tell people you're recording.")}</strong> {t("Recording laws differ, and in some places everyone must agree to be recorded.")}
          <div className="row">
            <button
              className="btn small"
              onClick={() => navigator.clipboard.writeText(t(ANNOUNCEMENT)).then(() => toast(t("Copied. Paste it into the meeting chat."), "success"))}
            >
              {t("Copy an announcement")}
            </button>
            <span className="muted small ellipsis" title={t(ANNOUNCEMENT)}>“{t(ANNOUNCEMENT)}”</span>
          </div>
        </div>
      </div>
      <div className="modal-actions">
        <button className="btn" onClick={onClose}>{t("Cancel")}</button>
        <button className="btn primary" onClick={start} disabled={busy || (!mic && !system)}>
          {busy ? t("Starting…") : t("● Start recording")}
        </button>
      </div>
    </Modal>
  );
}

function Meter({ label, level }: { label: string; level: number | null }) {
  if (level === null) return null;
  const pct = Math.min(100, Math.sqrt(level) * 220);
  return (
    <div className="meter" title={t("{label} level", { label })}>
      <span className="small">{label}</span>
      <div className="meter-bar"><span style={{ width: `${pct}%` }} /></div>
    </div>
  );
}

function SegmentLine({ s, names, onPlay, onName }: { s: Segment; names: Record<string, string>; onPlay?: (s: Segment) => void; onName?: (voice: number) => void }) {
  return (
    <div className={`segment ${s.speaker}`}>
      {onPlay ? (
        <button className="seg-time link" onClick={() => onPlay(s)} title={t("Play this part")}>▶ {clock(s.start)}</button>
      ) : (
        <span className="seg-time muted">{clock(s.start)}</span>
      )}
      {onName && s.voice !== null ? (
        <button className={`seg-speaker link voice-${(s.voice - 1) % 6}`} onClick={() => onName(s.voice!)} title={t("Name this person")}>
          {who(s, names)}
        </button>
      ) : (
        <span className={`seg-speaker ${s.voice !== null ? `voice-${(s.voice - 1) % 6}` : ""}`}>{who(s, names)}</span>
      )}
      <span className="seg-text">
        {s.text}
        {s.translation && <span className="seg-translation">{s.translation}</span>}
      </span>
    </div>
  );
}

function LiveMeeting({ id, onEnded, toast }: { id: string; onEnded: (id: string) => void; toast: PushToast }) {
  const [meeting, setMeeting] = useState<MeetingDetail | null>(null);
  const [segments, setSegments] = useState<Segment[]>([]);
  const [status, setStatus] = useState<string>("loading");
  const [levels, setLevels] = useState<{ you: number | null; others: number | null }>({ you: null, others: null });
  const [now, setNow] = useState(Date.now());
  const [stopping, setStopping] = useState(false);
  const scroller = useRef<HTMLDivElement>(null);
  // Callbacks from the parent change every render; events shouldn't resubscribe.
  const onEndedRef = useRef(onEnded);
  onEndedRef.current = onEnded;
  const toastRef = useRef(toast);
  toastRef.current = toast;

  useEffect(() => {
    api.meeting(id).then((m) => {
      setMeeting(m);
      setSegments(m.segments);
      if (m.status !== "recording") setStatus(m.status);
    });
    const tick = window.setInterval(() => setNow(Date.now()), 1000);
    const sub = on("meeting", (e) => {
      if (e.meeting_id !== id) return;
      switch (e.kind) {
        case "state":
          setStatus(e.status);
          break;
        case "level":
          setLevels({ you: e.you, others: e.others });
          break;
        case "segment":
          setSegments((all) => [...all, e.segment].sort((a, b) => a.start - b.start));
          break;
        case "removed":
          setSegments((all) => all.filter((s) => s.id !== e.segment_id));
          break;
        case "translation":
          setSegments((all) => all.map((s) => (s.id === e.segment_id ? { ...s, translation: e.text } : s)));
          break;
        case "relabeled":
          api.meeting(id).then((m) => setSegments(m.segments));
          break;
        case "warning":
          toastRef.current(e.message, "error");
          break;
        case "done":
          onEndedRef.current(id);
          break;
        case "error":
          toastRef.current(e.error, "error");
          onEndedRef.current(id);
          break;
      }
    });
    return () => {
      window.clearInterval(tick);
      sub.then((un) => un());
    };
  }, [id]);

  useEffect(() => {
    const el = scroller.current;
    if (el && el.scrollHeight - el.scrollTop - el.clientHeight < 160) el.scrollTop = el.scrollHeight;
  }, [segments]);

  const recording = status === "recording" || status === "loading";
  const elapsed = meeting ? (now - meeting.started_at) / 1000 : 0;
  return (
    <div className="live-meeting">
      <header className="live-head">
        <span className={`rec-dot ${recording ? "on" : ""}`} aria-hidden />
        <div className="live-title">
          <h1 className="ellipsis">{meeting?.title ?? t("Meeting")}</h1>
          <span className="muted small">
            {status === "loading" && t("Getting speech recognition ready…")}
            {status === "recording" && t("Recording · {time}", { time: clock(elapsed) })}
            {status === "transcribing" && t("Writing down the last few sentences…")}
            {status === "summarizing" && t("Writing notes with your local model…")}
          </span>
        </div>
        <span className="spacer" />
        <Meter label={t("You")} level={levels.you} />
        <Meter label={t("Others")} level={levels.others} />
        {recording && (
          <button
            className="btn danger-fill"
            disabled={stopping}
            onClick={async () => {
              setStopping(true);
              await api.stopMeeting();
            }}
          >
            {stopping ? t("Ending…") : t("■ End meeting")}
          </button>
        )}
      </header>
      <div className="transcript live" ref={scroller}>
        {segments.length === 0 && (
          <p className="muted small">
            {recording ? t("The transcript appears here a few seconds after people speak.") : t("Nothing was transcribed.")}
          </p>
        )}
        {segments.map((s) => (
          <SegmentLine key={s.id} s={s} names={meeting?.speaker_names ?? {}} />
        ))}
      </div>
      <p className="muted small">{t("Lines appear in batches at pauses in the conversation; long speeches take a little longer.")}</p>
    </div>
  );
}

function List({ title, items }: { title: string; items: string[] }) {
  if (!items.length) return null;
  return (
    <section>
      <h3>{title}</h3>
      <ul>{items.map((t, i) => <li key={i}>{t}</li>)}</ul>
    </section>
  );
}

function MeetingPage({ id, onBack, onOpenChat, toast }: { id: string; onBack: () => void; onOpenChat: (id: string) => void; toast: PushToast }) {
  const [m, setM] = useState<MeetingDetail | null>(null);
  const [renaming, setRenaming] = useState(false);
  const [title, setTitle] = useState("");
  const [confirm, setConfirm] = useState<"meeting" | "audio" | null>(null);
  const [naming, setNaming] = useState<{ voice: number; name: string } | null>(null);
  const audio = useRef<HTMLAudioElement>(null);
  const url = useRef<string | null>(null);

  const load = useCallback(() => api.meeting(id).then(setM).catch((e) => toast(errorText(e), "error")), [id, toast]);
  useEffect(() => {
    load();
    const sub = on("meeting", (e) => {
      if (e.meeting_id === id && (e.kind === "done" || e.kind === "error" || e.kind === "state")) load();
    });
    return () => {
      sub.then((un) => un());
      if (url.current) URL.revokeObjectURL(url.current);
    };
  }, [id, load]);

  if (!m) return <p className="muted">{t("Loading…")}</p>;
  const n = m.notes;
  const busy = m.status === "summarizing" || m.status === "transcribing";

  const play = async (s: Segment) => {
    try {
      const buf = await api.meetingClip(m.id, Math.max(0, s.start - 0.3), s.end + 0.5);
      if (url.current) URL.revokeObjectURL(url.current);
      url.current = URL.createObjectURL(new Blob([buf], { type: "audio/wav" }));
      if (audio.current) {
        audio.current.src = url.current;
        await audio.current.play();
      }
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  const exportNotes = async () => {
    const path = await save({ defaultPath: `${m.title.replace(/[\\/:*?"<>|]/g, "-")}.md`, filters: [{ name: "Markdown", extensions: ["md"] }] });
    if (!path) return;
    try {
      await api.exportMeeting(m.id, path, true);
      toast(t("Saved the notes and transcript."), "success");
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  return (
    <div className="narrow meeting-page">
      <button className="link" onClick={onBack}>{t("← All meetings")}</button>
      <header className="page-head">
        <div>
          {renaming ? (
            <form
              className="row"
              onSubmit={async (e) => {
                e.preventDefault();
                try {
                  await api.renameMeeting(m.id, title);
                  setRenaming(false);
                  load();
                } catch (err) {
                  toast(errorText(err), "error");
                }
              }}
            >
              <input className="input" value={title} onChange={(e) => setTitle(e.target.value)} autoFocus maxLength={120} aria-label={t("Meeting name")} />
              <button className="btn small primary" type="submit">{t("Save")}</button>
            </form>
          ) : (
            <h1>
              {m.title}{" "}
              <button className="link small" onClick={() => { setTitle(m.title); setRenaming(true); }}>{t("Rename")}</button>
            </h1>
          )}
          <p className="muted small">
            {when(m.started_at)}
            {m.ended_at && ` · ${duration(m.ended_at - m.started_at)}`}
            {m.translate_to && ` · ${t("captions translated")}`}
          </p>
        </div>
      </header>

      <div className="row wrap">
        <button className="btn primary" onClick={async () => onOpenChat(await api.askAboutMeeting(m.id))} disabled={busy}>
          {t("Ask about this meeting")}
        </button>
        <button className="btn" onClick={exportNotes}>{t("Export…")}</button>
        <button
          className="btn"
          disabled={busy || m.segments.length === 0}
          onClick={() => api.rewriteNotes(m.id).then(load).catch((e) => toast(errorText(e), "error"))}
        >
          {t("Write notes again")}
        </button>
        <span className="spacer" />
        {m.has_audio && <button className="btn ghost small" onClick={() => setConfirm("audio")}>{t("Delete recording")}</button>}
        <button className="btn ghost small danger" onClick={() => setConfirm("meeting")}>{t("Delete")}</button>
      </div>

      {busy && <p className="callout small">{t("Writing notes with your local model…")}</p>}
      {m.error && <div className="error-box">{m.error}</div>}

      {n && (
        <section className="card notes">
          <h2>{t("Summary")}</h2>
          <p>{n.summary}</p>
          <List title={t("Most important")} items={n.most_important} />
          {n.action_items.length > 0 && (
            <section>
              <div className="row">
                <h3>{t("Action items")}</h3>
                <span className="spacer" />
                <button
                  className="btn ghost small"
                  onClick={async () => {
                    try {
                      const added = await api.tasksFromMeeting(m.id, n.action_items.map((_, i) => i));
                      toast(added ? t("Added {n} to Tasks.", { n: added }) : t("They're already in Tasks."), "success");
                    } catch (e) {
                      toast(errorText(e), "error");
                    }
                  }}
                >
                  {t("+ Add to Tasks")}
                </button>
              </div>
              <ul className="actions">
                {n.action_items.map((a, i) => (
                  <li key={i}>
                    <label className="check">
                      <input
                        type="checkbox"
                        checked={a.done}
                        onChange={async (e) => {
                          await api.setActionDone(m.id, i, e.target.checked);
                          load();
                        }}
                      />
                      <span className={a.done ? "done" : ""}>
                        {a.task}
                        {(a.owner || a.due) && (
                          <span className="muted small"> — {[a.owner, a.due && t("due {date}", { date: a.due })].filter(Boolean).join(", ")}</span>
                        )}
                      </span>
                    </label>
                  </li>
                ))}
              </ul>
            </section>
          )}
          <List title={t("Decisions")} items={n.decisions} />
          <List title={t("Key points")} items={n.key_points} />
          <List title={t("Topics discussed")} items={n.topics} />
        </section>
      )}

      <section className="card">
        <h2>{t("Transcript")}</h2>
        {(m.has_audio || m.segments.some((s) => s.voice !== null)) && (
          <p className="muted small">
            {m.has_audio && `${t("Click a time to hear that part.")} `}
            {m.segments.some((s) => s.voice !== null) && t("Click a speaker to give them a name.")}
          </p>
        )}
        <audio ref={audio} hidden />
        <div className="transcript">
          {m.segments.length === 0 && <p className="muted small">{t("Nothing was transcribed.")}</p>}
          {m.segments.map((s) => (
            <SegmentLine
              key={s.id}
              s={s}
              names={m.speaker_names}
              onPlay={m.has_audio ? play : undefined}
              onName={(voice) => setNaming({ voice, name: m.speaker_names[String(voice)] ?? "" })}
            />
          ))}
        </div>
      </section>

      {naming && (
        <Modal title={t("Who is Speaker {n}?", { n: naming.voice })} onClose={() => setNaming(null)}>
          <form
            className="form"
            onSubmit={async (e) => {
              e.preventDefault();
              try {
                await api.renameSpeaker(m.id, naming.voice, naming.name);
                setNaming(null);
                load();
              } catch (err) {
                toast(errorText(err), "error");
              }
            }}
          >
            <input className="input" value={naming.name} onChange={(e) => setNaming({ ...naming, name: e.target.value })} placeholder={t("Their name")} autoFocus maxLength={60} aria-label={t("Speaker name")} />
            <p className="muted small">{t("Every line by this speaker shows the name. Use “Write notes again” so the notes use it too.")}</p>
            <div className="modal-actions">
              <button type="button" className="btn" onClick={() => setNaming(null)}>{t("Cancel")}</button>
              <button type="submit" className="btn primary">{t("Save")}</button>
            </div>
          </form>
        </Modal>
      )}
      {confirm && (
        <Modal title={confirm === "meeting" ? t("Delete this meeting?") : t("Delete the recording?")} onClose={() => setConfirm(null)}>
          <p>
            {confirm === "meeting"
              ? t("“{title}”, its transcript, notes and recording will be permanently deleted from this PC.", { title: m.title })
              : t("The audio is deleted; the transcript and notes stay.")}
          </p>
          <div className="modal-actions">
            <button className="btn" onClick={() => setConfirm(null)}>{t("Cancel")}</button>
            <button
              className="btn danger-fill"
              onClick={async () => {
                try {
                  if (confirm === "meeting") {
                    await api.deleteMeeting(m.id);
                    setConfirm(null);
                    onBack();
                  } else {
                    await api.deleteMeetingAudio(m.id);
                    setConfirm(null);
                    load();
                  }
                } catch (e) {
                  toast(errorText(e), "error");
                }
              }}
            >
              {t("Delete")}
            </button>
          </div>
        </Modal>
      )}
    </div>
  );
}

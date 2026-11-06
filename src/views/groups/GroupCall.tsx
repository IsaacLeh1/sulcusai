// SPDX-License-Identifier: AGPL-3.0-only
// A call in a group's voice channel. The window records in short pieces and
// the core seals and passes them on; pieces from others play through
// MediaSource. Screen share uses the system picker, which captures a window
// from outside it (nothing is loaded into a game).
import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { errorText } from "../../api";
import { t } from "../../i18n";
import type { PushToast } from "../../components/Toasts";
import { social, type CallPiece, type CallView, type ChannelView, type StreamKind } from "./socialApi";

const SLICE_MS = 250;
const TYPES: Record<StreamKind, string> = { voice: "audio/webm;codecs=opus", camera: "video/webm;codecs=vp8", screen: "video/webm;codecs=vp8" };

function toB64(buf: ArrayBuffer): string {
  const bytes = new Uint8Array(buf);
  let s = "";
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(s);
}

function fromB64(s: string): Uint8Array {
  const bin = atob(s);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

/** Plays one person's stream as its pieces arrive, in order. */
class Player {
  el: HTMLMediaElement;
  private ms = new MediaSource();
  private sb: SourceBuffer | null = null;
  private queue: Uint8Array[] = [];
  private started = false;
  constructor(kind: StreamKind) {
    this.el = document.createElement(kind === "voice" ? "audio" : "video");
    this.el.autoplay = true;
    this.el.src = URL.createObjectURL(this.ms);
    this.ms.addEventListener("sourceopen", () => {
      this.sb = this.ms.addSourceBuffer(TYPES[kind]);
      this.sb.mode = "sequence";
      this.sb.addEventListener("updateend", () => this.pump());
      this.pump();
    });
  }
  push(data: Uint8Array, first: boolean) {
    // Pieces before the first one can't be decoded on their own.
    if (!this.started && !first) return;
    this.started = true;
    this.queue.push(data);
    // Keep latency low: a long backlog is dropped, not played late.
    if (this.queue.length > 20) this.queue.splice(1, this.queue.length - 5);
    this.pump();
  }
  private pump() {
    if (!this.sb || this.sb.updating || this.queue.length === 0) return;
    try {
      this.sb.appendBuffer(this.queue.shift()! as BufferSource);
    } catch {
      this.queue = [];
    }
  }
  close() {
    this.el.pause();
    URL.revokeObjectURL(this.el.src);
  }
}

export function GroupCall({
  group,
  channel,
  myName,
  onClose,
  toast,
  muted = false,
  deafened = false,
  onPeople,
  me,
  big = false,
  voice,
}: {
  group: string;
  channel: string;
  myName: string;
  onClose: () => void;
  toast: PushToast;
  muted?: boolean;
  deafened?: boolean;
  onPeople?: (people: CallPerson[]) => void;
  /** This PC's member id, for its own tile. */
  me: string;
  /** The channel's own view: everyone's tile, video and screens. */
  big?: boolean;
  /** The voice channel: its bitrate, and whether this user may speak there. */
  voice?: ChannelView;
}) {
  const canSpeak = voice?.can_speak !== false;
  const bitrate = (voice?.bitrate || 48) * 1000;
  const deaf = useRef(deafened);
  const [call, setCall] = useState<CallView | null>(null);
  const [sending, setSending] = useState<Record<StreamKind, boolean>>({ voice: false, camera: false, screen: false });
  const recorders = useRef<Partial<Record<StreamKind, { rec: MediaRecorder; stream: MediaStream }>>>({});
  const players = useRef<Map<string, Player>>(new Map());
  const stage = useRef<HTMLDivElement>(null);
  const meters = useSpeaking();
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, []);
  const joinedAt = useRef(Date.now());

  useEffect(() => {
    let alive = true;
    // The mic opens while the join goes out, instead of after it.
    const mic = !muted && !deafened && canSpeak ? navigator.mediaDevices.getUserMedia({ audio: { echoCancellation: true, noiseSuppression: true } }).catch(() => null) : Promise.resolve(null);
    social
      .callJoin(group, channel, myName)
      .then(() => {
        social
          .callView()
          .then((v) => alive && v && setCall(v))
          .catch(() => {});
        return mic.then((stream) => {
          if (!alive) return stream?.getTracks().forEach((tr) => tr.stop());
          if (stream) record("voice", stream);
        });
      })
      .catch((e) => {
        toast(errorText(e));
        onClose();
      });
    const un = [
      listen<CallView>("social:call", (e) => e.payload.group === group && setCall(e.payload)),
      listen<CallPiece>("social:call_piece", (e) => {
        const p = e.payload;
        const key = `${p.from}:${p.kind}`;
        let pl = players.current.get(key);
        if (!pl) {
          pl = new Player(p.kind);
          pl.el.muted = deaf.current;
          players.current.set(key, pl);
          place(p.from, p.kind, pl.el);
          if (p.kind === "voice") meters.watchElement(p.from, pl.el);
        }
        pl.push(fromB64(p.data), p.first);
      }),
    ];
    const recs = recorders.current;
    const pls = players.current;
    return () => {
      alive = false;
      mic.then((s) => s && !Object.values(recorders.current).some((r) => r?.stream === s) && s.getTracks().forEach((tr) => tr.stop()));
      un.forEach((u) => u.then((f) => f()));
      Object.values(recs).forEach((r) => {
        r?.rec.stop();
        r?.stream.getTracks().forEach((tr) => tr.stop());
      });
      pls.forEach((p) => {
        p.close();
        p.el.remove();
      });
      social.callLeave().catch(() => {});
    };
    // Joining once per call; mute changes are handled below.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [group, channel, myName, onClose, toast]);
  // The owner's answer normally comes as an event; this catches one that was missed.
  useEffect(() => {
    if (call) return;
    const id = window.setInterval(() => {
      social
        .callView()
        .then((v) => v && v.group === group && setCall(v))
        .catch(() => {});
    }, 1000);
    return () => window.clearInterval(id);
  }, [call, group]);
  const people: CallPerson[] = (call ? call.people : [[me, myName] as [string, string]]).map(([id, name]) => ({
    id,
    name,
    since: call?.since?.[id] ?? (id === me ? joinedAt.current : now),
    speaking: meters.speaking.has(id),
  }));
  const peopleKey = people.map((p) => `${p.id}:${p.name}:${p.speaking}:${p.since}`).join("|");
  useEffect(() => {
    onPeople?.(people);
    // Only when someone joins, leaves or starts or stops speaking.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [peopleKey, onPeople]);
  // Video goes into the sender's tile once it exists; voice plays from a hidden box.
  const place = (who: string, kind: StreamKind, el: HTMLMediaElement) => {
    const box = kind === "voice" ? stage.current?.querySelector(".call-audio") : stage.current?.querySelector(`[data-who="${CSS.escape(who)}"]`);
    (box ?? stage.current)?.appendChild(el);
  };
  useEffect(() => {
    players.current.forEach((pl, key) => {
      const [who, kind] = key.split(":") as [string, StreamKind];
      if (kind !== "voice" && !pl.el.parentElement?.matches(`[data-who="${CSS.escape(who)}"]`)) place(who, kind, pl.el);
    });
  });

  useEffect(() => {
    deaf.current = deafened;
    players.current.forEach((p) => (p.el.muted = deafened));
  }, [deafened]);
  useEffect(() => {
    if (muted || deafened) stop("voice");
  }, [muted, deafened]);

  const stop = (kind: StreamKind) => {
    const r = recorders.current[kind];
    r?.rec.stop();
    r?.stream.getTracks().forEach((tr) => tr.stop());
    delete recorders.current[kind];
    setSending((s) => ({ ...s, [kind]: false }));
  };

  const start = async (kind: StreamKind) => {
    try {
      const stream =
        kind === "voice"
          ? await navigator.mediaDevices.getUserMedia({ audio: { echoCancellation: true, noiseSuppression: true } })
          : kind === "camera"
            ? await navigator.mediaDevices.getUserMedia({ video: { width: 640, height: 360 } })
            : await navigator.mediaDevices.getDisplayMedia({ video: { frameRate: 30 }, audio: false });
      record(kind, stream);
    } catch (e) {
      toast(errorText(e));
    }
  };

  function record(kind: StreamKind, stream: MediaStream) {
      const rec = new MediaRecorder(stream, { mimeType: TYPES[kind], videoBitsPerSecond: kind === "screen" ? 2_000_000 : 600_000, audioBitsPerSecond: bitrate });
      let first = true;
      rec.ondataavailable = async (e) => {
        if (e.data.size === 0) return;
        const isFirst = first;
        first = false;
        social.callPiece(kind, isFirst, toB64(await e.data.arrayBuffer())).catch(() => {});
      };
      stream.getTracks().forEach((tr) => (tr.onended = () => stop(kind)));
      rec.start(SLICE_MS);
      recorders.current[kind] = { rec, stream };
      if (kind === "voice") meters.watchStream(me, stream);
      setSending((s) => ({ ...s, [kind]: true }));
  }

  const toggle = (kind: StreamKind) => (sending[kind] ? stop(kind) : start(kind));

  return (
    <div className={big ? "group-call big" : "group-call"}>
      <div className="call-bar">
        <span className={call ? "call-state on" : "call-state"}>{call ? t("Voice connected") : t("Connecting...")}</span>
        {!big && <span className="small muted ellipsis">{people.map((p) => p.name).join(", ")}</span>}
        <span className="call-tools">
          <button className={sending.voice ? "active" : ""} disabled={!canSpeak} title={canSpeak ? undefined : t("You can listen here but not speak.")} onClick={() => toggle("voice")}>
            {sending.voice ? t("Mute") : t("Talk")}
          </button>
          <button className={sending.camera ? "active" : ""} onClick={() => toggle("camera")}>
            {sending.camera ? t("Stop camera") : t("Camera")}
          </button>
          <button className={sending.screen ? "active" : ""} onClick={() => toggle("screen")} title={t("For games, share the game's window in borderless mode; nothing is loaded into the game.")}>
            {sending.screen ? t("Stop sharing") : t("Share screen")}
          </button>
          <button className="danger" onClick={onClose}>
            {t("Leave call")}
          </button>
        </span>
      </div>
      <div className="call-stage" ref={stage}>
        <div className="call-audio" hidden />
        {people.map((p) => (
          <div key={p.id} className={p.speaking ? "call-tile speaking" : "call-tile"}>
            <div className="call-media" data-who={p.id} />
            <span className="call-avatar" aria-hidden>
              {p.name.slice(0, 1).toUpperCase()}
            </span>
            <span className="call-name">
              {p.name} <span className="call-time">{clock(now - p.since)}</span>
            </span>
          </div>
        ))}
      </div>
    </div>
  );
}

export type CallPerson = { id: string; name: string; since: number; speaking: boolean };

/** How long someone has been in the call, as m:ss or h:mm:ss. */
export function clock(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const ss = String(s % 60).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${ss}` : `${m}:${ss}`;
}

/** Who is speaking: the loudness of each voice, checked ten times a second. */
function useSpeaking() {
  const ctx = useRef<AudioContext | null>(null);
  const nodes = useRef<Map<string, AnalyserNode>>(new Map());
  const [speaking, setSpeaking] = useState<Set<string>>(new Set());
  const audio = () => (ctx.current ??= new AudioContext());
  const analyser = (who: string, src: AudioNode, out: boolean) => {
    const an = audio().createAnalyser();
    an.fftSize = 512;
    src.connect(an);
    if (out) an.connect(audio().destination);
    nodes.current.set(who, an);
  };
  useEffect(() => {
    const buf = new Uint8Array(512);
    const id = window.setInterval(() => {
      const on = new Set<string>();
      nodes.current.forEach((an, who) => {
        an.getByteTimeDomainData(buf);
        let sum = 0;
        for (const v of buf) sum += ((v - 128) / 128) ** 2;
        if (Math.sqrt(sum / buf.length) > 0.03) on.add(who);
      });
      setSpeaking((old) => (old.size === on.size && [...on].every((x) => old.has(x)) ? old : on));
    }, 100);
    return () => {
      window.clearInterval(id);
      ctx.current?.close().catch(() => {});
    };
  }, []);
  return {
    speaking,
    // Another person's voice: it now plays through the meter.
    watchElement: (who: string, el: HTMLMediaElement) => analyser(who, audio().createMediaElementSource(el), true),
    // This PC's own mic: measured, not played back.
    watchStream: (who: string, stream: MediaStream) => analyser(who, audio().createMediaStreamSource(stream), false),
  };
}

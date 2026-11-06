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
import { social, type CallPiece, type CallView, type StreamKind } from "./socialApi";

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
}: {
  group: string;
  channel: string;
  myName: string;
  onClose: () => void;
  toast: PushToast;
  muted?: boolean;
  deafened?: boolean;
  onPeople?: (names: string[]) => void;
}) {
  const deaf = useRef(deafened);
  const [call, setCall] = useState<CallView | null>(null);
  const [sending, setSending] = useState<Record<StreamKind, boolean>>({ voice: false, camera: false, screen: false });
  const recorders = useRef<Partial<Record<StreamKind, { rec: MediaRecorder; stream: MediaStream }>>>({});
  const players = useRef<Map<string, Player>>(new Map());
  const stage = useRef<HTMLDivElement>(null);

  useEffect(() => {
    let alive = true;
    // The mic opens while the join goes out, instead of after it.
    const mic = !muted && !deafened ? navigator.mediaDevices.getUserMedia({ audio: { echoCancellation: true, noiseSuppression: true } }).catch(() => null) : Promise.resolve(null);
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
          stage.current?.appendChild(pl.el);
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
  const names = call ?call.people.map(([, n]) => n) : [myName];
  const namesKey = names.join("|");
  useEffect(() => {
    onPeople?.(namesKey.split("|"));
  }, [namesKey, onPeople]);

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
      const rec = new MediaRecorder(stream, { mimeType: TYPES[kind], videoBitsPerSecond: kind === "screen" ? 2_000_000 : 600_000, audioBitsPerSecond: 48_000 });
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
      setSending((s) => ({ ...s, [kind]: true }));
  }

  const toggle = (kind: StreamKind) => (sending[kind] ? stop(kind) : start(kind));

  return (
    <div className="group-call card">
      <div className="row">
        <span className={call ? "call-state on" : "call-state"}>{call ? t("Voice connected") : t("Connecting...")}</span>
        <span className="small muted">{names.join(", ")}</span>
      </div>
      <div className="row">
        <button className={sending.voice ? "active" : ""} onClick={() => toggle("voice")}>
          {sending.voice ? t("Mute") : t("Talk")}
        </button>
        <button className={sending.camera ? "active" : ""} onClick={() => toggle("camera")}>
          {sending.camera ? t("Stop camera") : t("Camera")}
        </button>
        <button className={sending.screen ? "active" : ""} onClick={() => toggle("screen")}>
          {sending.screen ? t("Stop sharing") : t("Share a window or screen")}
        </button>
        <button className="danger" onClick={onClose}>
          {t("Leave call")}
        </button>
      </div>
      <p className="small muted">{t("For games, share the game's window in borderless mode; nothing is loaded into the game.")}</p>
      <div className="group-call-stage" ref={stage} />
    </div>
  );
}

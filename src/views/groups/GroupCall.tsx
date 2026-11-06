// SPDX-License-Identifier: AGPL-3.0-only
// A call in a group's voice channel. The window records in short pieces and
// the core seals and passes them on; pieces from others play through
// MediaSource. Screen share uses the system picker, which captures a window
// from outside it (nothing is loaded into a game).
import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { useDismiss } from "../../useDismiss";
import { cameraConstraint, devices, DEVICES_EVENT, DevicesDialog, micConstraint, setVolume, volumes } from "./Devices";
import { listen } from "@tauri-apps/api/event";
import { errorText } from "../../api";
import { t } from "../../i18n";
import type { PushToast } from "../../components/Toasts";
import { CameraIcon, CogIcon, HangUpIcon, MicIcon, ScreenIcon } from "./SocialIcons";
import { social, type CallPiece, type CallView, type ChannelView, type StreamKind } from "./socialApi";

const SLICE_MS = 250;
const TYPES: Record<StreamKind, string> = { voice: "audio/webm;codecs=opus", camera: "video/webm;codecs=vp8", screen: "video/webm;codecs=vp8" };
/** "screen1".."screen3" are more windows or screens from the same person. */
const typeOf = (kind: string) => TYPES[kind.startsWith("screen") ? "screen" : (kind as StreamKind)] ?? TYPES.screen;

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
  constructor(kind: string) {
    this.el = document.createElement(kind === "voice" ? "audio" : "video");
    this.el.autoplay = true;
    this.el.src = URL.createObjectURL(this.ms);
    this.ms.addEventListener("sourceopen", () => {
      this.sb = this.ms.addSourceBuffer(typeOf(kind));
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
  hd = false,
  onMuted,
  moderator = false,
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
  /** Extras: screens shared at up to 1080p and 60 frames a second. */
  hd?: boolean;
  /** The shared mute (the user bar shows the same). */
  onMuted?: (m: boolean) => void;
  /** This user may mute, deafen or remove others in the call. */
  moderator?: boolean;
}) {
  const canSpeak = voice?.can_speak !== false;
  const bitrate = (voice?.bitrate || 48) * 1000;
  const deaf = useRef(deafened);
  const [call, setCall] = useState<CallView | null>(null);
  // What this PC sends: voice, camera, and up to four windows or screens ("screen", "screen1".."screen3").
  const [sending, setSending] = useState<Record<string, boolean>>({});
  const recorders = useRef<Record<string, { rec: MediaRecorder; stream: MediaStream; preview?: HTMLVideoElement }>>({});
  const players = useRef<Map<string, Player>>(new Map());
  const lastPiece = useRef<Map<string, number>>(new Map());
  const [, setVideoCount] = useState(0);
  const stage = useRef<HTMLDivElement>(null);
  const meters = useSpeaking();
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, []);
  const joinedAt = useRef(Date.now());
  const [focus, setFocus] = useState<string | null>(null);
  // Remote control: whom this user asked or controls, and a request to this PC.
  const [asking, setAsking] = useState<string | null>(null);
  const [control, setControl] = useState<string | null>(null);
  const [asked, setAsked] = useState<{ by: string; name: string } | null>(null);
  const [deviceDialog, setDeviceDialog] = useState(false);

  useEffect(() => {
    let alive = true;
    // The mic opens while the join goes out, instead of after it.
    const mic = !muted && !deafened && canSpeak ? navigator.mediaDevices.getUserMedia({ audio: micConstraint() }).catch(() => null) : Promise.resolve(null);
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
          place(key, pl.el);
          if (p.kind === "voice") meters.watchElement(p.from, pl.el);
          else setVideoCount((n) => n + 1);
        }
        lastPiece.current.set(key, Date.now());
        pl.push(fromB64(p.data), p.first);
      }),
      listen<{ by: string; name: string }>("social:rc_ask", (e) => setAsked(e.payload)),
      listen<{ by: string; name: string; ok: boolean; why: string }>("social:rc_answer", (e) => {
        setAsking(null);
        if (e.payload.ok) {
          setControl(e.payload.by);
          toast(t("You're controlling {name}'s PC. They can stop it at any time.", { name: e.payload.name }));
        } else {
          setControl((c) => (c === e.payload.by ? null : c));
          if (e.payload.why) toast(e.payload.why);
        }
      }),
      listen<{ by: string }>("social:rc_end", (e) => setControl((c) => (c === e.payload.by ? null : c))),
      listen("social:call_removed", () => {
        toast(t("A moderator removed you from the call."));
        onClose();
      }),
    ];
    const recs = recorders.current;
    const pls = players.current;
    return () => {
      alive = false;
      mic.then((s) => s && !Object.values(recorders.current).some((r) => r?.stream === s) && s.getTracks().forEach((tr) => tr.stop()));
      un.forEach((u) => u.then((f) => f()));
      Object.values(recs).forEach((r) => {
        r.rec.stop();
        r.stream.getTracks().forEach((tr) => tr.stop());
      });
      pls.forEach((p) => {
        p.close();
        p.el.remove();
      });
      social.rcEnd(null).catch(() => {});
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
  // A moderator muted or deafened this user in the call.
  const modMuted = !!call?.muted?.includes(me);
  const modDeaf = !!call?.deafened?.includes(me);
  useEffect(() => {
    if (modMuted) stop("voice");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [modMuted]);
  // A new microphone, camera or speakers picked in Voice and video: used at once.
  useEffect(() => {
    let lastMic = devices().mic;
    let lastCam = devices().camera;
    const changed = () => {
      const d = devices();
      if (d.mic !== lastMic && recorders.current.voice) {
        stop("voice");
        start("voice");
      }
      if (d.camera !== lastCam && recorders.current.camera) {
        stop("camera");
        start("camera");
      }
      lastMic = d.mic;
      lastCam = d.camera;
      meters.refresh();
    };
    window.addEventListener(DEVICES_EVENT, changed);
    return () => window.removeEventListener(DEVICES_EVENT, changed);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  const [menuFor, setMenuFor] = useState<{ id: string; x: number; y: number } | null>(null);
  const nameOf = (id: string) => people.find((p) => p.id === id)?.name ?? t("Someone");

  // Every video shown: others' (while pieces keep coming) and this PC's own.
  const videos: string[] = [
    ...[...players.current.keys()].filter((k) => !k.endsWith(":voice") && now - (lastPiece.current.get(k) ?? 0) < 4000 && people.some((p) => k.startsWith(p.id + ":"))),
    ...Object.keys(sending)
      .filter((k) => k !== "voice" && sending[k])
      .map((k) => `${me}:${k}`),
  ];
  const label = (key: string) => {
    const [who, kind] = splitKey(key);
    const n = kind === "screen" ? 1 : Number(kind.slice(6)) + 1;
    return kind === "camera" ? nameOf(who) : videos.filter((v) => v.startsWith(who + ":screen")).length > 1 ? t("{name}'s screen {n}", { name: nameOf(who), n }) : t("{name}'s screen", { name: nameOf(who) });
  };
  // Each video sits in its own tile once that tile exists; voice plays from a hidden box.
  const place = (key: string, el: HTMLMediaElement) => {
    const box = key.endsWith(":voice") ? stage.current?.querySelector(".call-audio") : stage.current?.querySelector(`[data-stream="${CSS.escape(key)}"]`);
    (box ?? stage.current?.querySelector(".call-audio"))?.appendChild(el);
  };
  useEffect(() => {
    players.current.forEach((pl, key) => {
      if (!key.endsWith(":voice") && !pl.el.parentElement?.matches(`[data-stream="${CSS.escape(key)}"]`)) place(key, pl.el);
    });
    Object.entries(recorders.current).forEach(([kind, r]) => {
      if (r.preview && !r.preview.parentElement?.matches(`[data-stream="${CSS.escape(`${me}:${kind}`)}"]`)) place(`${me}:${kind}`, r.preview);
    });
  });
  useEffect(() => {
    if (focus && !videos.includes(focus)) setFocus(null);
  }, [focus, videos]);

  useEffect(() => {
    deaf.current = deafened;
    players.current.forEach((p) => (p.el.muted = deafened));
  }, [deafened]);
  // Muted or deafened anywhere stops the mic; unmuting anywhere opens it again.
  const wasOff = useRef(muted || deafened);
  useEffect(() => {
    const off = muted || deafened;
    if (off) stop("voice");
    else if (wasOff.current && canSpeak && !recorders.current.voice) start("voice");
    wasOff.current = off;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [muted, deafened]);

  const stop = (kind: string) => {
    const r = recorders.current[kind];
    r?.rec.stop();
    r?.stream.getTracks().forEach((tr) => tr.stop());
    r?.preview?.remove();
    delete recorders.current[kind];
    setSending((s) => ({ ...s, [kind]: false }));
    // Remote control works on a shared screen: it ends with the last one.
    if (kind.startsWith("screen") && !Object.keys(recorders.current).some((k) => k.startsWith("screen"))) social.rcEnd(null).catch(() => {});
  };

  const start = async (kind: string) => {
    try {
      const stream =
        kind === "voice"
          ? await navigator.mediaDevices.getUserMedia({ audio: micConstraint() })
          : kind === "camera"
            ? await navigator.mediaDevices.getUserMedia({ video: cameraConstraint() })
            : await navigator.mediaDevices.getDisplayMedia({ video: hd ? { frameRate: 60, width: { ideal: 1920 }, height: { ideal: 1080 } } : { frameRate: 30 }, audio: false });
      record(kind, stream);
    } catch (e) {
      toast(errorText(e));
    }
  };

  // Another window or screen, up to four at once.
  const shareAnother = () => {
    const free = ["screen", "screen1", "screen2", "screen3"].find((k) => !recorders.current[k]);
    if (free) start(free);
    else toast(t("Four windows or screens at most at once."));
  };

  function record(kind: string, stream: MediaStream) {
    const isScreen = kind.startsWith("screen");
    const rec = new MediaRecorder(stream, { mimeType: typeOf(kind), videoBitsPerSecond: isScreen ? (hd ? 6_000_000 : 2_000_000) : 600_000, audioBitsPerSecond: bitrate });
    let first = true;
    rec.ondataavailable = async (e) => {
      if (e.data.size === 0) return;
      const isFirst = first;
      first = false;
      social.callPiece(kind, isFirst, toB64(await e.data.arrayBuffer())).catch(() => {});
    };
    stream.getTracks().forEach((tr) => (tr.onended = () => stop(kind)));
    rec.start(SLICE_MS);
    let preview: HTMLVideoElement | undefined;
    if (kind !== "voice") {
      // What this PC shares, shown in its own tile.
      preview = document.createElement("video");
      preview.muted = true;
      preview.autoplay = true;
      preview.srcObject = stream;
    }
    recorders.current[kind] = { rec, stream, preview };
    if (kind === "voice") meters.watchStream(me, stream);
    setSending((s) => ({ ...s, [kind]: true }));
  }

  const toggle = (kind: string) => (sending[kind] ? stop(kind) : start(kind));
  const sharingScreens = Object.keys(sending).filter((k) => k.startsWith("screen") && sending[k]);

  // This PC was asked for control: allowed only by typing the asker's name,
  // and only while a whole screen is shared (that's where their input lands).
  const allow = async (typed: string) => {
    const monitor = sharingScreens.map((k) => recorders.current[k]?.stream.getVideoTracks()[0]).find((tr) => (tr?.getSettings() as { displaySurface?: string }).displaySurface === "monitor");
    if (!monitor) {
      setAsked(null);
      await social.rcEnd(null, t("They need to share their whole screen (not a window) before they can allow control.")).catch(() => {});
      return toast(t("Share your whole screen (not a window) first, then they can ask again."));
    }
    const s = monitor.getSettings();
    try {
      await social.rcAccept(typed, Math.round(s.width ?? 0), Math.round(s.height ?? 0));
      setAsked(null);
    } catch (e) {
      setAsked(null);
      toast(errorText(e));
    }
  };

  // The controller's mouse and keys on the tile they control.
  const lastMove = useRef(0);
  const sendInput = (input: RcInput) => control && social.rcInput(control, input).catch(() => {});
  const pointAt = (e: React.PointerEvent | React.WheelEvent, video: HTMLVideoElement | null) => {
    if (!video || !video.videoWidth) return null;
    const r = video.getBoundingClientRect();
    const k = Math.min(r.width / video.videoWidth, r.height / video.videoHeight);
    const w = video.videoWidth * k;
    const h = video.videoHeight * k;
    const x = (e.clientX - r.left - (r.width - w) / 2) / w;
    const y = (e.clientY - r.top - (r.height - h) / 2) / h;
    return x < 0 || y < 0 || x > 1 || y > 1 ? null : { x, y };
  };
  const controlHandlers = (key: string) => {
    const [who, kind] = splitKey(key);
    if (control !== who || !kind.startsWith("screen")) return {};
    const video = () => stage.current?.querySelector<HTMLVideoElement>(`[data-stream="${CSS.escape(key)}"] video`) ?? null;
    return {
      tabIndex: 0,
      onPointerMove: (e: React.PointerEvent) => {
        if (Date.now() - lastMove.current < 33) return;
        lastMove.current = Date.now();
        const p = pointAt(e, video());
        if (p) sendInput({ do: "move", ...p });
      },
      onPointerDown: (e: React.PointerEvent) => {
        e.preventDefault();
        (e.currentTarget as HTMLElement).focus();
        const p = pointAt(e, video());
        if (p) sendInput({ do: "move", ...p });
        sendInput({ do: "click", button: e.button, down: true });
      },
      onPointerUp: (e: React.PointerEvent) => sendInput({ do: "click", button: e.button, down: false }),
      onContextMenu: (e: React.MouseEvent) => e.preventDefault(),
      onWheel: (e: React.WheelEvent) => sendInput({ do: "scroll", dy: e.deltaY }),
      onKeyDown: (e: React.KeyboardEvent) => {
        e.preventDefault();
        sendInput({ do: "key", code: e.code, down: true });
      },
      onKeyUp: (e: React.KeyboardEvent) => {
        e.preventDefault();
        sendInput({ do: "key", code: e.code, down: false });
      },
    };
  };

  const tiles = videos.length > 0 && focus ? [focus, ...videos.filter((v) => v !== focus)] : videos;
  return (
    <div className={big ? "group-call big" : "group-call"}>
      <div className="call-bar">
        <span className={call ? "call-state on" : "call-state"}>{call ? t("Voice connected") : t("Connecting...")}</span>
        {!big && <span className="small muted ellipsis">{people.map((p) => p.name).join(", ")}</span>}
        {(modMuted || modDeaf) && <span className="call-control-note">{modMuted && modDeaf ? t("A moderator muted and deafened you") : modMuted ? t("A moderator muted you") : t("A moderator deafened you")}</span>}
        {control && (
          <span className="call-control-note">
            {t("Controlling {name}'s PC", { name: nameOf(control) })}
            <button
              className="link"
              onClick={() => {
                social.rcEnd(control).catch(() => {});
                setControl(null);
              }}
            >
              {t("Stop")}
            </button>
          </span>
        )}
        <span className="call-tools">
          <button
            className={sending.voice ? "call-btn" : "call-btn off"}
            disabled={!canSpeak || modMuted}
            title={modMuted ? t("A moderator muted you in this call.") : !canSpeak ? t("You can listen here but not speak.") : sending.voice ? t("Mute") : t("Unmute")}
            aria-label={sending.voice ? t("Mute") : t("Unmute")}
            onClick={() => (sending.voice ? (onMuted ? onMuted(true) : stop("voice")) : muted && onMuted ? onMuted(false) : start("voice"))}
          >
            <MicIcon off={!sending.voice} />
          </button>
          <button className={sending.camera ? "call-btn on" : "call-btn"} onClick={() => toggle("camera")} title={sending.camera ? t("Stop camera") : t("Camera")} aria-label={sending.camera ? t("Stop camera") : t("Camera")}>
            <CameraIcon />
          </button>
          <button
            className={sharingScreens.length > 0 ? "call-btn on" : "call-btn"}
            onClick={shareAnother}
            title={sharingScreens.length > 0 ? t("Share another window or screen") : t("Share a screen, a window or a browser tab. For games, share the game's window in borderless mode; nothing is loaded into the game.")}
            aria-label={t("Share screen")}
          >
            <ScreenIcon />
          </button>
          {sharingScreens.length > 0 && (
            <button className="call-btn" onClick={() => sharingScreens.forEach(stop)} title={t("Stop sharing everything")} aria-label={t("Stop sharing")}>
              <StopShareIcon />
            </button>
          )}
          <button className="call-btn" onClick={() => setDeviceDialog(true)} title={t("Voice and video settings")} aria-label={t("Voice and video settings")}>
            <CogIcon />
          </button>
          <button className="call-btn hang" onClick={onClose} title={t("Leave call")} aria-label={t("Leave call")}>
            <HangUpIcon />
          </button>
        </span>
      </div>
      <div className={focus ? "call-stage focused" : "call-stage"} ref={stage}>
        <div className="call-audio" hidden />
        {tiles.map((key) => {
          const [who, kind] = splitKey(key);
          const mine = who === me;
          const screenTile = kind.startsWith("screen");
          return (
            <div key={key} className={`call-tile video${focus === key ? " focus" : ""}${control === who && screenTile ? " controlling" : ""}`} onDoubleClick={(e) => fullScreen(e.currentTarget)}>
              <div className="call-media" data-stream={key} {...controlHandlers(key)} onClick={control === who && screenTile ? undefined : () => setFocus((f) => (f === key ? null : key))} />
              <span className="call-name">{mine && screenTile ? t("Your screen") : label(key)}</span>
              <span className="call-tile-tools">
                {mine && screenTile && (
                  <button onClick={() => stop(kind)} title={t("Stop sharing this")}>
                    {t("Stop")}
                  </button>
                )}
                {!mine && screenTile && control !== who && (
                  <button
                    disabled={asking === who}
                    onClick={() => {
                      setAsking(who);
                      social.rcAsk(who).catch((e) => {
                        setAsking(null);
                        toast(errorText(e));
                      });
                    }}
                    title={t("Ask {name} to let you control their PC", { name: nameOf(who) })}
                  >
                    {asking === who ? t("Asking...") : t("Remote control")}
                  </button>
                )}
                <button onClick={() => setFocus((f) => (f === key ? null : key))} title={focus === key ? t("Back to all") : t("Focus on this")}>
                  {focus === key ? t("Back to all") : t("Focus")}
                </button>
                <button onClick={(e) => fullScreen(e.currentTarget.closest(".call-tile") as HTMLElement)} title={t("Full screen (Esc to leave)")} aria-label={t("Full screen")}>
                  <FullIcon />
                </button>
              </span>
            </div>
          );
        })}
        {people.map((p) => (
          <div
            key={p.id}
            className={p.speaking ? "call-tile person speaking" : "call-tile person"}
            onClick={(e) => setMenuFor({ id: p.id, x: e.clientX, y: e.clientY })}
            onContextMenu={(e) => {
              e.preventDefault();
              setMenuFor({ id: p.id, x: e.clientX, y: e.clientY });
            }}
            title={p.id === me ? undefined : t("Volume and more for {name}", { name: p.name })}
          >
            <span className="call-avatar" aria-hidden>
              {p.name.slice(0, 1).toUpperCase()}
            </span>
            <span className="call-name">
              {p.name} <span className="call-time">{clock(now - p.since)}</span>
              {call?.muted?.includes(p.id) && <span className="call-flag"> {t("muted")}</span>}
              {call?.deafened?.includes(p.id) && <span className="call-flag"> {t("deafened")}</span>}
              {(volumes()[p.id] ?? 1) === 0 && p.id !== me && <span className="call-flag"> {t("muted for you")}</span>}
            </span>
          </div>
        ))}
      </div>
      {menuFor && (
        <PersonMenu
          id={menuFor.id}
          name={nameOf(menuFor.id)}
          x={menuFor.x}
          y={menuFor.y}
          self={menuFor.id === me}
          moderator={moderator}
          muted={!!call?.muted?.includes(menuFor.id)}
          deafened={!!call?.deafened?.includes(menuFor.id)}
          onVolume={(v) => {
            setVolume(menuFor.id, v);
            meters.refresh();
          }}
          onMod={(action, on) => social.callMod(menuFor.id, action, on).catch((e) => toast(errorText(e)))}
          onClose={() => setMenuFor(null)}
        />
      )}
      {deviceDialog && <DevicesDialog onClose={() => setDeviceDialog(false)} />}
      {asked &&
        createPortal(
          <AllowControl
            name={asked.name}
            onAllow={allow}
            onDecline={() => {
              setAsked(null);
              social.rcEnd(null, t("They declined.")).catch(() => {});
            }}
          />,
          document.body,
        )}
    </div>
  );
}

type RcInput = { do: "move"; x: number; y: number } | { do: "click"; button: number; down: boolean } | { do: "key"; code: string; down: boolean } | { do: "scroll"; dy: number };

const splitKey = (key: string): [string, string] => {
  const i = key.lastIndexOf(":");
  return [key.slice(0, i), key.slice(i + 1)];
};

function fullScreen(el: HTMLElement | null) {
  if (!el) return;
  if (document.fullscreenElement) document.exitFullscreen().catch(() => {});
  else el.requestFullscreen().catch(() => {});
}

/** Asked for control: allowed only by typing the asker's name. */
function AllowControl({ name, onAllow, onDecline }: { name: string; onAllow: (typed: string) => void; onDecline: () => void }) {
  const [typed, setTyped] = useState("");
  const box = useRef<HTMLDivElement>(null);
  useDismiss(true, onDecline, box);
  return (
    <div className="social-modal" role="alertdialog" aria-label={t("Remote control request")}>
      <div className="social-modal-box" ref={box}>
        <h3>{t("{name} wants to control your PC", { name })}</h3>
        <p className="small">{t("They'll be able to move your mouse and type on the screen you're sharing, as if they were at your PC. Only allow people you trust.")}</p>
        <p className="small muted">{t("A red border shows while they're in control. Stop it at any time with the Stop bar at the top of the screen or Ctrl+Alt+Backspace.")}</p>
        <label>
          {t("To allow it, type their name: {name}", { name })}
          <input autoFocus value={typed} onChange={(e) => setTyped(e.target.value)} onKeyDown={(e) => e.key === "Enter" && typed.trim().toLowerCase() === name.trim().toLowerCase() && onAllow(typed)} />
        </label>
        <div className="row">
          <button className="primary" disabled={typed.trim().toLowerCase() !== name.trim().toLowerCase()} onClick={() => onAllow(typed)}>
            {t("Allow control")}
          </button>
          <button onClick={onDecline}>{t("Decline")}</button>
        </div>
      </div>
    </div>
  );
}

function FullIcon() {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden>
      <path d="M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5" />
    </svg>
  );
}

function StopShareIcon() {
  return (
    <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden>
      <rect x="2" y="3" width="20" height="14" rx="2" />
      <path d="M8 21h8M12 17v4M9 7l6 6M15 7l-6 6" />
    </svg>
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
  const gains = useRef<Map<string, GainNode>>(new Map());
  const [speaking, setSpeaking] = useState<Set<string>>(new Set());
  const audio = () => {
    if (!ctx.current) {
      const c = new AudioContext() as AudioContext & { setSinkId?: (id: string) => Promise<void> };
      // The speakers picked in Voice and video.
      if (devices().speaker) c.setSinkId?.(devices().speaker).catch(() => {});
      ctx.current = c;
    }
    return ctx.current;
  };
  const analyser = (who: string, src: AudioNode, out: boolean) => {
    const an = audio().createAnalyser();
    an.fftSize = 512;
    src.connect(an);
    if (out) {
      const g = audio().createGain();
      g.gain.value = volumes()[who] ?? 1;
      src.connect(g).connect(audio().destination);
      gains.current.set(who, g);
    }
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
    // Volumes and speakers picked since.
    refresh: () => {
      const v = volumes();
      gains.current.forEach((g, who) => (g.gain.value = v[who] ?? 1));
      const sink = devices().speaker;
      const c = ctx.current as (AudioContext & { setSinkId?: (id: string) => Promise<void> }) | null;
      c?.setSinkId?.(sink).catch(() => {});
    },
    // Another person's voice: it now plays through the meter.
    watchElement: (who: string, el: HTMLMediaElement) => analyser(who, audio().createMediaElementSource(el), true),
    // This PC's own mic: measured, not played back.
    watchStream: (who: string, stream: MediaStream) => analyser(who, audio().createMediaStreamSource(stream), false),
  };
}

/** A person in the call: their volume for you, mute for you, and a moderator's mute, deafen and remove. */
function PersonMenu({
  id,
  name,
  x,
  y,
  self,
  moderator,
  muted,
  deafened,
  onVolume,
  onMod,
  onClose,
}: {
  id: string;
  name: string;
  x: number;
  y: number;
  self: boolean;
  moderator: boolean;
  muted: boolean;
  deafened: boolean;
  onVolume: (v: number) => void;
  onMod: (action: "mute" | "deafen" | "remove", on: boolean) => void;
  onClose: () => void;
}) {
  const box = useRef<HTMLDivElement>(null);
  useDismiss(true, onClose, box);
  const [v, setV] = useState(volumes()[id] ?? 1);
  const [before, setBefore] = useState(v > 0 ? v : 1);
  return createPortal(
    <div ref={box} className="social-msg-menu person-menu" role="menu" style={{ left: Math.min(x, window.innerWidth - 260), top: Math.min(y, window.innerHeight - 260) }}>
      <strong className="pad-s">{name}</strong>
      {!self && (
        <>
          <label className="person-volume">
            <span className="small">{t("Volume for you: {n}%", { n: Math.round(v * 100) })}</span>
            <input
              type="range"
              min={0}
              max={200}
              value={Math.round(v * 100)}
              onChange={(e) => {
                const n = Number(e.target.value) / 100;
                setV(n);
                if (n > 0) setBefore(n);
                onVolume(n);
              }}
            />
          </label>
          <button
            role="menuitemcheckbox"
            aria-checked={v === 0}
            onClick={() => {
              const n = v === 0 ? before : 0;
              setV(n);
              onVolume(n);
            }}
          >
            {v === 0 ? t("Unmute for you") : t("Mute for you")}
          </button>
        </>
      )}
      {moderator && (
        <>
          <hr />
          <button role="menuitem" onClick={() => (onMod("mute", !muted), onClose())}>
            {muted ? t("Unmute in the call") : t("Mute in the call")}
          </button>
          <button role="menuitem" onClick={() => (onMod("deafen", !deafened), onClose())}>
            {deafened ? t("Undeafen in the call") : t("Deafen in the call")}
          </button>
          {!self && (
            <button role="menuitem" className="danger" onClick={() => (onMod("remove", true), onClose())}>
              {t("Remove from the call")}
            </button>
          )}
        </>
      )}
    </div>,
    document.body,
  );
}

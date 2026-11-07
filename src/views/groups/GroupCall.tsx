// SPDX-License-Identifier: AGPL-3.0-only
// A call in a group's voice channel. The window records in short pieces and
// the core seals and passes them on; pieces from others play through
// MediaSource. Screen share uses the system picker, which captures a window
// from outside it (nothing is loaded into a game).
import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { useDismiss } from "../../useDismiss";
import "./call.css";
import { cameraConstraint, devices, DEVICES_EVENT, DevicesDialog, setVolume, volumes } from "./Devices";
import { changerOn, keyMatches, MicChain, micConstraints, personNoise, PRESETS, setPersonNoise, voiceSettings } from "./voice";
import { useVoiceSettings, VoicePanel } from "./VoicePanel";
import { CAMERA_EVENT, cameraFx, cameraSettings, hasRoom, nativeBlur, needsFx } from "./camera";
import { colorFor, cropStream, decodeSignal, DrawLayer, encodeSignal, quality, RegionPicker, type Arrival, type Region, type Signal, type Stroke } from "./callExtras";
import { listen } from "@tauri-apps/api/event";
import { errorText } from "../../api";
import { t } from "../../i18n";
import type { PushToast } from "../../components/Toasts";
import { CameraIcon, CogIcon, HangUpIcon, MicIcon, ScreenIcon } from "./SocialIcons";
import { social, type CallPiece, type CallView, type ChannelView, type StreamKind } from "./socialApi";

const SLICE_MS = 250;
const TYPES: Record<StreamKind | "sound", string> = { voice: "audio/webm;codecs=opus", sound: "audio/webm;codecs=opus", camera: "video/webm;codecs=vp8", screen: "video/webm;codecs=vp8" };
/** "screen1".."screen3" are more windows or screens from the same person. */
const typeOf = (kind: string) => TYPES[kind.startsWith("screen") ? "screen" : (kind as StreamKind)] ?? TYPES.screen;
const isAudio = (kind: string) => kind === "voice" || kind === "sound" || kind === "whisper";

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
  /** A failed append leaves the buffer unusable; the next fresh start replaces the player. */
  broken = false;
  constructor(kind: string) {
    this.el = document.createElement(isAudio(kind) ? "audio" : "video");
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
      this.broken = true;
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
  const [vs] = useVoiceSettings();
  const preset = PRESETS[vs.preset];
  // The channel's bitrate, or more for Music; adaptive lowers it from there.
  const maxKbps = Math.max(voice?.bitrate || 48, preset.bitrate);
  const [kbps, setKbps] = useState(maxKbps);
  const kbpsRef = useRef(kbps);
  kbpsRef.current = kbps;
  const deaf = useRef(deafened);
  const [call, setCall] = useState<CallView | null>(null);
  // What this PC sends: voice, camera, and up to four windows or screens ("screen", "screen1".."screen3").
  const [sending, setSending] = useState<Record<string, boolean>>({});
  const recorders = useRef<Record<string, { rec: MediaRecorder; stream: MediaStream; preview?: HTMLVideoElement; cleanup?: () => void; source?: MediaStream }>>({});
  const chain = useRef<MicChain | null>(null);
  const players = useRef<Map<string, Player>>(new Map());
  const lastPiece = useRef<Map<string, number>>(new Map());
  const arrivals = useRef<Map<string, Arrival>>(new Map());
  const sendDelays = useRef<number[]>([]);
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
  const [layout, setLayout] = useState<"grid" | "speaker">("grid");
  // Remote control: whom this user asked or controls, and a request to this PC.
  const [asking, setAsking] = useState<string | null>(null);
  const [control, setControl] = useState<string | null>(null);
  const [asked, setAsked] = useState<{ by: string; name: string } | null>(null);
  const [deviceDialog, setDeviceDialog] = useState(false);
  const [reconnecting, setReconnecting] = useState(false);
  const [pttHeld, setPttHeld] = useState(false);
  const [talkingMuted, setTalkingMuted] = useState(false);
  // Who is recording and whose voice is changed, as their windows said.
  const [recBy, setRecBy] = useState<Set<string>>(new Set());
  const [changedBy, setChangedBy] = useState<Set<string>>(new Set());
  const [recorder, setRecorder] = useState<MediaRecorder | null>(null);
  const [strokes, setStrokes] = useState<Record<string, Stroke[]>>({});
  const [drawOn, setDrawOn] = useState<string | null>(null);
  const [picking, setPicking] = useState<string | null>(null);
  // Lower quality asked for by viewers, per stream of this PC: viewer -> low.
  const wantLow = useRef<Record<string, Record<string, boolean>>>({});
  const [viewQuality, setViewQuality] = useState<Record<string, boolean>>({});
  // While whispering, the mic goes only to these people and the rest hear nothing.
  const [whisperTo, setWhisperTo] = useState<string[] | null>(null);
  const whisperRef = useRef<string[] | null>(null);
  whisperRef.current = whisperTo;

  const signal = (s: Signal) => {
    social.callPiece("signal", false, toB64(encodeSignal(s).buffer as ArrayBuffer)).catch(() => {});
  };

  // A player that can't continue, or one whose sender started over, is replaced.
  const drop = (key: string) => {
    const pl = players.current.get(key);
    if (!pl) return;
    pl.close();
    pl.el.remove();
    players.current.delete(key);
    lastPiece.current.delete(key);
    const [who, kind] = splitKey(key);
    if (kind === "voice") meters.forget(who);
  };

  const onSignal = (from: string, s: Signal) => {
    if (s.t === "rec") setRecBy((o) => withOrWithout(o, from, s.on));
    else if (s.t === "changed") setChangedBy((o) => withOrWithout(o, from, s.on));
    else if (s.t === "draw") setStrokes((o) => ({ ...o, [s.key]: [...(o[s.key] ?? []).slice(-60), { pts: s.pts.slice(0, 2000), color: s.color, at: Date.now() }] }));
    else if (s.t === "clear") setStrokes((o) => ({ ...o, [s.key]: [] }));
    else if (s.t === "quality" && s.to === me) {
      wantLow.current[s.kind] = { ...wantLow.current[s.kind], [from]: s.low };
      if (recorders.current[s.kind]) restartRec(s.kind);
    }
  };

  useEffect(() => {
    let alive = true;
    // The mic opens while the join goes out, instead of after it.
    const mic = !muted && !deafened && canSpeak ? MicChain.open(voiceSettings()).catch(() => null) : Promise.resolve(null);
    social
      .callJoin(group, channel, myName)
      .then(() => {
        social
          .callView()
          .then((v) => alive && v && setCall(v))
          .catch(() => {});
        return mic.then((c) => {
          if (!alive) return c?.close();
          if (c) {
            chain.current = c;
            record("voice", c.stream);
          } else if (!muted && !deafened && canSpeak) toast(t("The microphone didn't open. Check that SulcusAI may use it in Windows privacy settings, then unmute to try again."));
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
        if (p.kind === "signal") {
          const s = decodeSignal(fromB64(p.data));
          if (s) onSignal(p.from, s);
          return;
        }
        const key = `${p.from}:${p.kind}`;
        const at = Date.now();
        const a = arrivals.current.get(key) ?? { times: [], last: 0 };
        a.times.push(at);
        if (a.times.length > 40) a.times.shift();
        a.last = at;
        arrivals.current.set(key, a);
        let pl = players.current.get(key);
        // A fresh start (a rejoin, a new recorder) can't follow the old stream.
        if (pl && (p.first || pl.broken)) {
          if (!p.first) return;
          drop(key);
          pl = undefined;
        }
        if (!pl) {
          if (!p.first) return;
          pl = new Player(p.kind);
          pl.el.muted = deaf.current;
          players.current.set(key, pl);
          place(key, pl.el);
          if (p.kind === "voice") meters.watchElement(p.from, pl.el);
          else if (p.kind !== "sound") setVideoCount((n) => n + 1);
        }
        lastPiece.current.set(key, at);
        pl.push(fromB64(p.data), p.first);
      }),
      listen<boolean>("social:ptt", (e) => setPttHeld(e.payload)),
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
      mic.then((c) => c && c !== chain.current && c.close());
      un.forEach((u) => u.then((f) => f()));
      Object.values(recs).forEach((r) => {
        r.rec.stop();
        r.stream.getTracks().forEach((tr) => tr.stop());
        r.source?.getTracks().forEach((tr) => tr.stop());
        r.cleanup?.();
      });
      chain.current?.close();
      chain.current = null;
      pls.forEach((p) => {
        p.close();
        p.el.remove();
      });
      pls.clear();
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

  // Back after a network blip: if the host lost this PC, or nothing has
  // arrived from anyone for a while, join again and start every stream over.
  const lastRejoin = useRef(0);
  useEffect(() => {
    if (!call) return;
    let busy = false;
    const check = async () => {
      if (busy) return;
      busy = true;
      try {
        const v = await social.callView();
        const inIt = !!v && v.group === group && v.people.some(([id]) => id === me);
        const others = (v?.people.length ?? 1) > 1;
        const newest = Math.max(0, ...lastPiece.current.values());
        const silent = others && players.current.size > 0 && Date.now() - newest > 8000;
        if ((!inIt || silent) && Date.now() - lastRejoin.current > 15000) {
          lastRejoin.current = Date.now();
          setReconnecting(true);
          await social.callJoin(group, channel, myName);
          restartAll();
        } else if (inIt) setReconnecting(false);
      } catch {
        setReconnecting(true);
      } finally {
        busy = false;
      }
    };
    const id = window.setInterval(check, 4000);
    const online = () => {
      lastRejoin.current = 0;
      check();
    };
    window.addEventListener("online", online);
    return () => {
      window.clearInterval(id);
      window.removeEventListener("online", online);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [!!call, group, channel, myName, me]);

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
  // Someone left: their players go, so a rejoin starts clean. Someone came:
  // every stream of this PC starts over, so they get a fresh start to play from
  // even if the host's copy of it was lost.
  const ids = people.map((p) => p.id).join("|");
  const seenIds = useRef<Set<string>>(new Set());
  useEffect(() => {
    const here = new Set(people.map((p) => p.id));
    for (const key of [...players.current.keys()]) if (!here.has(splitKey(key)[0])) drop(key);
    for (const who of [...recBy]) if (!here.has(who)) setRecBy((o) => withOrWithout(o, who, false));
    const fresh = call && seenIds.current.size > 0 && [...here].some((id) => id !== me && !seenIds.current.has(id));
    if (call) seenIds.current = here;
    if (fresh) {
      const id = window.setTimeout(() => {
        restartAll();
        if (recorder) signal({ t: "rec", on: true });
        if (changerOn(vs) && vs.markChanged && recorders.current.voice) signal({ t: "changed", on: true });
      }, 400);
      return () => window.clearTimeout(id);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ids, !!call]);

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
    const cam = () => {
      if (recorders.current.camera) {
        stop("camera");
        start("camera");
      }
    };
    window.addEventListener(CAMERA_EVENT, cam);
    // A headset plugged in mid-call is taken at once when no mic was picked.
    const plugged = () => {
      if (!devices().mic && recorders.current.voice) {
        stop("voice");
        start("voice");
        toast(t("Switched to the microphone just plugged in."));
      }
    };
    navigator.mediaDevices.addEventListener("devicechange", plugged);
    return () => {
      window.removeEventListener(DEVICES_EVENT, changed);
      window.removeEventListener(CAMERA_EVENT, cam);
      navigator.mediaDevices.removeEventListener("devicechange", plugged);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Voice settings: most apply live; the preset and the browser's own processing reopen the mic.
  const opened = useRef({ preset: vs.preset, echo: vs.echo, agc: vs.agc });
  useEffect(() => {
    chain.current?.apply(vs);
    const o = opened.current;
    if ((o.preset !== vs.preset || o.echo !== vs.echo || o.agc !== vs.agc) && recorders.current.voice) {
      stop("voice");
      start("voice");
    }
    opened.current = { preset: vs.preset, echo: vs.echo, agc: vs.agc };
    meters.setSpatial(vs.spatial);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [vs]);
  useEffect(() => setKbps(maxKbps), [maxKbps]);

  // Push to talk: the system-wide key, and the window's own keys while it has focus.
  useEffect(() => {
    if (!vs.pushToTalk) return;
    social.callPtt(vs.pttKey).catch((e) => toast(errorText(e)));
    const down = (e: KeyboardEvent) => keyMatches(e, vs.pttKey) && setPttHeld(true);
    const up = (e: KeyboardEvent) => keyMatches(e, vs.pttKey) && setPttHeld(false);
    window.addEventListener("keydown", down);
    window.addEventListener("keyup", up);
    return () => {
      social.callPtt(null).catch(() => {});
      window.removeEventListener("keydown", down);
      window.removeEventListener("keyup", up);
      setPttHeld(false);
    };
  }, [vs.pushToTalk, vs.pttKey, toast]);
  const talking = !vs.pushToTalk || pttHeld;
  useEffect(() => chain.current?.setOpen(talking), [talking, sending.voice]);

  // The "voice changed" mark others see.
  const markOn = changerOn(vs) && vs.markChanged && !!sending.voice;
  useEffect(() => {
    if (call) signal({ t: "changed", on: markOn });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [markOn, !!call]);

  // Talking while muted: a short note, from a meter that never sends anything.
  useEffect(() => {
    if (!vs.muteReminder || !(muted || modMuted) || deafened) return;
    let stopped = false;
    let close = () => {};
    navigator.mediaDevices
      .getUserMedia({ audio: micConstraints(vs) })
      .then((s) => {
        if (stopped) return s.getTracks().forEach((tr) => tr.stop());
        const ctx = new AudioContext();
        const an = ctx.createAnalyser();
        ctx.createMediaStreamSource(s).connect(an);
        const buf = new Uint8Array(512);
        let loud = 0;
        const id = window.setInterval(() => {
          an.getByteTimeDomainData(buf);
          let sum = 0;
          for (const v of buf) sum += ((v - 128) / 128) ** 2;
          loud = Math.sqrt(sum / buf.length) > 0.05 ? loud + 1 : 0;
          if (loud === 4) {
            setTalkingMuted(true);
            window.setTimeout(() => setTalkingMuted(false), 3000);
          }
        }, 100);
        close = () => {
          window.clearInterval(id);
          s.getTracks().forEach((tr) => tr.stop());
          ctx.close().catch(() => {});
        };
      })
      .catch(() => {});
    return () => {
      stopped = true;
      close();
    };
  }, [vs, muted, modMuted, deafened]);

  // Adaptive bitrate: the time each piece takes to leave stands in for the connection.
  useEffect(() => {
    if (!vs.adaptive) return;
    const id = window.setInterval(() => {
      const d = sendDelays.current.splice(0);
      if (d.length < 4 || !recorders.current.voice) return;
      const avg = d.reduce((a, b) => a + b, 0) / d.length;
      const k = kbpsRef.current;
      const next = avg > 300 ? Math.max(16, Math.round(k * 0.7)) : avg < 60 ? Math.min(maxKbps, Math.round(k * 1.25)) : k;
      if (next !== k) {
        setKbps(next);
        kbpsRef.current = next;
        restartRec("voice");
      }
    }, 6000);
    return () => window.clearInterval(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [vs.adaptive, maxKbps]);

  // Shared screen sound is lowered while someone talks.
  useEffect(() => {
    const talk = [...meters.speaking].some((id) => id !== me);
    players.current.forEach((pl, key) => {
      if (splitKey(key)[1] === "sound") pl.el.volume = vs.ducking && talk ? 0.3 : 1;
    });
  }, [meters.speaking, vs.ducking, me]);

  // Others' strokes fade after a while.
  useEffect(() => {
    const id = window.setInterval(() => {
      setStrokes((o) => {
        const cut = Date.now() - 10000;
        let changed = false;
        const out: Record<string, Stroke[]> = {};
        for (const [k, list] of Object.entries(o)) {
          out[k] = list.filter((s) => s.at > cut);
          if (out[k].length !== list.length) changed = true;
        }
        return changed ? out : o;
      });
    }, 2000);
    return () => window.clearInterval(id);
  }, []);

  // People placed left to right as their tiles are.
  useEffect(() => {
    const others = people.filter((p) => p.id !== me);
    others.forEach((p, i) => meters.place(p.id, others.length < 2 ? 0 : -0.8 + (1.6 * i) / (others.length - 1)));
    const nr = personNoise();
    others.forEach((p) => meters.noise(p.id, !!nr[p.id]));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ids, vs]);

  const [menuFor, setMenuFor] = useState<{ id: string; x: number; y: number } | null>(null);
  const nameOf = (id: string) => people.find((p) => p.id === id)?.name ?? t("Someone");

  // Every video shown: others' (while pieces keep coming) and this PC's own.
  const videos: string[] = [
    ...[...players.current.keys()].filter((k) => !isAudio(splitKey(k)[1]) && now - (lastPiece.current.get(k) ?? 0) < 4000 && people.some((p) => k.startsWith(p.id + ":"))),
    ...Object.keys(sending)
      .filter((k) => !isAudio(k) && sending[k])
      .map((k) => `${me}:${k}`),
  ];
  const label = (key: string) => {
    const [who, kind] = splitKey(key);
    const n = kind === "screen" ? 1 : Number(kind.slice(6)) + 1;
    return kind === "camera" ? nameOf(who) : videos.filter((v) => v.startsWith(who + ":screen")).length > 1 ? t("{name}'s screen {n}", { name: nameOf(who), n }) : t("{name}'s screen", { name: nameOf(who) });
  };
  // Each video sits in its own tile once that tile exists; sound plays from a hidden box.
  const place = (key: string, el: HTMLMediaElement) => {
    const box = isAudio(splitKey(key)[1]) ? stage.current?.querySelector(".call-audio") : stage.current?.querySelector(`[data-stream="${CSS.escape(key)}"]`);
    (box ?? stage.current?.querySelector(".call-audio"))?.appendChild(el);
  };
  useEffect(() => {
    players.current.forEach((pl, key) => {
      if (!isAudio(splitKey(key)[1]) && !pl.el.parentElement?.matches(`[data-stream="${CSS.escape(key)}"]`)) place(key, pl.el);
    });
    Object.entries(recorders.current).forEach(([kind, r]) => {
      if (r.preview && !r.preview.parentElement?.matches(`[data-stream="${CSS.escape(`${me}:${kind}`)}"]`)) place(`${me}:${kind}`, r.preview);
    });
  });
  // Speaker view follows whoever talks, when they have a camera on.
  const speaker = [...meters.speaking].find((id) => id !== me && videos.includes(`${id}:camera`));
  const lastSpeaker = useRef<string | null>(null);
  if (speaker) lastSpeaker.current = `${speaker}:camera`;
  const shown = focus ?? (layout === "speaker" ? (lastSpeaker.current && videos.includes(lastSpeaker.current) ? lastSpeaker.current : (videos[0] ?? null)) : null);
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
    r?.source?.getTracks().forEach((tr) => tr.stop());
    r?.cleanup?.();
    r?.preview?.remove();
    delete recorders.current[kind];
    if (kind === "voice") {
      if (recorders.current.whisper) stop("whisper");
      setWhisperTo(null);
      chain.current?.close();
      chain.current = null;
    }
    // A shared screen's sound goes with the screen.
    if (kind === "screen" && recorders.current.sound) stop("sound");
    setSending((s) => ({ ...s, [kind]: false }));
    // Remote control works on a shared screen: it ends with the last one.
    if (kind.startsWith("screen") && !Object.keys(recorders.current).some((k) => k.startsWith("screen"))) social.rcEnd(null).catch(() => {});
  };

  const start = async (kind: string) => {
    try {
      if (kind === "voice") {
        const c = await MicChain.open(voiceSettings());
        chain.current?.close();
        chain.current = c;
        c.setOpen(!vs.pushToTalk || pttHeld);
        return record("voice", c.stream);
      }
      if (kind === "camera") {
        const raw = await navigator.mediaDevices.getUserMedia({ video: cameraConstraint() });
        const cs = cameraSettings();
        let fx = cs;
        // The camera's own blur costs nothing here, so it's used when there is one.
        if (cs.background === "blur" && (await nativeBlur(raw.getVideoTracks()[0], true))) fx = { ...cs, background: "off" };
        if (fx.background !== "off" && !hasRoom()) {
          toast(t("For a background effect, learn the empty room first in Voice and video settings, under Camera."));
          fx = { ...fx, background: "off" };
        }
        if (!needsFx(fx)) return record("camera", raw);
        const c = cameraFx(raw, fx);
        return record("camera", c.stream, { cleanup: c.stop, source: raw });
      }
      const stream =
        await navigator.mediaDevices.getDisplayMedia({
              video: { ...(hd ? { frameRate: 60, width: { ideal: 1920 }, height: { ideal: 1080 } } : { frameRate: 30 }), cursor: "always" } as MediaTrackConstraints,
              // The first share can carry its sound (a tab, or the whole system on Windows).
              audio: kind === "screen",
            });
      const sound = stream.getAudioTracks();
      if (sound.length) {
        sound.forEach((tr) => stream.removeTrack(tr));
        record("sound", new MediaStream(sound));
      }
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

  // Only part of a screen: the capture keeps running and a crop of it is sent.
  const shareRegion = (kind: string, r: Region) => {
    const cur = recorders.current[kind];
    if (!cur) return;
    const source = cur.source ?? cur.stream;
    cur.cleanup?.();
    const crop = cropStream(source, r, hd ? 60 : 30);
    cur.rec.stop();
    cur.preview?.remove();
    delete recorders.current[kind];
    record(kind, crop.stream, { cleanup: crop.stop, source });
  };

  function videoBits(kind: string) {
    const low = Object.values(wantLow.current[kind] ?? {}).some(Boolean);
    if (kind.startsWith("screen")) return low ? 800_000 : hd ? 6_000_000 : 2_000_000;
    return low ? 250_000 : 600_000;
  }

  // The same stream again with a new recorder: a fresh start, new bitrate.
  function restartRec(kind: string) {
    const r = recorders.current[kind];
    if (!r) return;
    r.rec.ondataavailable = null;
    r.rec.stop();
    delete recorders.current[kind];
    record(kind, r.stream, { cleanup: r.cleanup, source: r.source, preview: r.preview });
  }
  function restartAll() {
    Object.keys(recorders.current).forEach(restartRec);
  }

  function record(kind: string, stream: MediaStream, keep: { cleanup?: () => void; source?: MediaStream; preview?: HTMLVideoElement } = {}) {
    const isScreen = kind.startsWith("screen");
    const rec = new MediaRecorder(stream, { mimeType: typeOf(kind), videoBitsPerSecond: isScreen || kind === "camera" ? videoBits(kind) : undefined, audioBitsPerSecond: kind === "sound" ? 128_000 : kbpsRef.current * 1000 });
    let first = true;
    rec.ondataavailable = async (e) => {
      if (e.data.size === 0) return;
      const isFirst = first;
      first = false;
      if (kind === "voice" && whisperRef.current) return;
      const t0 = performance.now();
      social
        .callPiece(kind, isFirst, toB64(await e.data.arrayBuffer()), kind === "whisper" ? (whisperRef.current ?? []) : undefined)
        .then(() => kind === "voice" && sendDelays.current.push(performance.now() - t0))
        .catch(() => kind === "voice" && sendDelays.current.push(2000));
    };
    stream.getTracks().forEach((tr) => (tr.onended = () => stop(kind)));
    keep.source?.getVideoTracks().forEach((tr) => (tr.onended = () => stop(kind)));
    rec.start(SLICE_MS);
    let preview = keep.preview;
    if (!isAudio(kind) && !preview) {
      // What this PC shares, shown in its own tile.
      preview = document.createElement("video");
      preview.muted = true;
      preview.autoplay = true;
    }
    if (preview && preview.srcObject !== stream) preview.srcObject = stream;
    // Mirrored for you only, like a mirror; others see you the right way round.
    if (preview && kind === "camera") preview.style.transform = cameraSettings().mirror ? "scaleX(-1)" : "";
    recorders.current[kind] = { rec, stream, preview, cleanup: keep.cleanup, source: keep.source };
    if (kind === "voice") meters.watchStream(me, stream);
    if (kind === "whisper") return;
    setSending((s) => ({ ...s, [kind]: true }));
  }

  const whisper = (to: string[] | null) => {
    const r = recorders.current.whisper;
    if (r) {
      r.rec.ondataavailable = null;
      r.rec.stop();
      delete recorders.current.whisper;
    }
    setWhisperTo(to);
    whisperRef.current = to;
    const c = chain.current;
    // Its own stream with its own start, so only they get it and everyone else's voice player is untouched.
    if (to && c) record("whisper", c.stream);
    else if (to) toast(t("Unmute first to whisper."));
    // Back to everyone: a fresh start so their players pick it up again cleanly.
    if (!to) restartRec("voice");
  };

  const toggle = (kind: string) => (sending[kind] ? stop(kind) : start(kind));
  const sharingScreens = Object.keys(sending).filter((k) => k.startsWith("screen") && sending[k]);

  // Recording: everyone is told first and sees a red mark while it runs.
  const startRecording = () => {
    if (!window.confirm(t("Everyone in the call will be told you're recording and will see a red mark until you stop. Record?"))) return;
    signal({ t: "rec", on: true });
    const own = chain.current?.stream;
    const mix = meters.mix(own);
    const rec = new MediaRecorder(mix.stream, { mimeType: "audio/webm;codecs=opus" });
    const parts: Blob[] = [];
    rec.ondataavailable = (e) => e.data.size && parts.push(e.data);
    rec.onstop = () => {
      mix.stop();
      const url = URL.createObjectURL(new Blob(parts, { type: "audio/webm" }));
      const a = document.createElement("a");
      a.href = url;
      a.download = `call-${new Date().toISOString().slice(0, 16).replace(/[:T]/g, "-")}.webm`;
      a.click();
      window.setTimeout(() => URL.revokeObjectURL(url), 60_000);
    };
    rec.start(1000);
    setRecorder(rec);
    setRecBy((o) => withOrWithout(o, me, true));
  };
  const stopRecording = () => {
    if (recorder?.state !== "inactive") recorder?.stop();
    setRecorder(null);
    signal({ t: "rec", on: false });
    setRecBy((o) => withOrWithout(o, me, false));
  };
  // Leaving the call while recording still saves what was recorded.
  useEffect(() => () => void (recorder && recorder.state !== "inactive" && recorder.stop()), [recorder]);

  // This PC was asked for control: allowed only by typing the asker's name,
  // and only while a whole screen is shared (that's where their input lands).
  const allow = async (typed: string) => {
    const monitor = sharingScreens.map((k) => (recorders.current[k]?.source ?? recorders.current[k]?.stream)?.getVideoTracks()[0]).find((tr) => (tr?.getSettings() as { displaySurface?: string }).displaySurface === "monitor");
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

  const popOut = (key: string) => {
    const v = stage.current?.querySelector<HTMLVideoElement>(`[data-stream="${CSS.escape(key)}"] video`);
    v?.requestPictureInPicture?.().catch((e: unknown) => toast(errorText(e)));
  };
  const askQuality = (key: string, low: boolean) => {
    const [who, kind] = splitKey(key);
    setViewQuality((q) => ({ ...q, [key]: low }));
    signal({ t: "quality", to: who, kind, low });
  };

  const details = (id: string) => {
    const q = quality(arrivals.current.get(`${id}:voice`), now, SLICE_MS);
    const delay = sendDelays.current.length ? Math.round(sendDelays.current.reduce((a, b) => a + b, 0) / sendDelays.current.length) : null;
    return [
      t("Quality: {why}", { why: q.why }),
      t("Codec: Opus, {k} kbit/s from you", { k: kbps }),
      t("Pieces arriving late: {n}%", { n: Math.round(q.late * 100) }),
      t("Route: through the group owner's PC, sealed with the group key"),
      delay !== null ? t("Your send delay: {ms} ms", { ms: delay }) : "",
    ]
      .filter(Boolean)
      .join("\n");
  };

  const tiles = videos.length > 0 && shown ? [shown, ...videos.filter((v) => v !== shown)] : videos;
  const recording = recBy.size > 0;
  return (
    <div className={big ? "group-call big" : "group-call"}>
      <div className="call-bar">
        <span className={call && !reconnecting ? "call-state on" : "call-state"}>{reconnecting ? t("Reconnecting...") : call ? t("Voice connected") : t("Connecting...")}</span>
        {!big && <span className="small muted ellipsis">{people.map((p) => p.name).join(", ")}</span>}
        {recording && (
          <span className="call-rec" role="status">
            {t("Recording: {names}", { names: [...recBy].map(nameOf).join(", ") })}
          </span>
        )}
        {whisperTo && (
          <span className="call-control-note on">
            {t("Whispering to {names}", { names: whisperTo.map(nameOf).join(", ") })}
            <button className="link" onClick={() => whisper(null)}>
              {t("Stop")}
            </button>
          </span>
        )}
        {talkingMuted && <span className="call-control-note">{t("You're muted")}</span>}
        {vs.pushToTalk && !!sending.voice && <span className={pttHeld ? "call-control-note on" : "call-control-note"}>{pttHeld ? t("Talking") : t("Hold {key} to talk", { key: vs.pttKey })}</span>}
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
            title={sharingScreens.length > 0 ? t("Share another window or screen") : t("Share a screen, a window or a browser tab, with its sound if you tick it in the picker. For games, share the game's window in borderless mode; nothing is loaded into the game.")}
            aria-label={t("Share screen")}
          >
            <ScreenIcon />
          </button>
          {sharingScreens.length > 0 && (
            <button className="call-btn" onClick={() => sharingScreens.forEach(stop)} title={t("Stop sharing everything")} aria-label={t("Stop sharing")}>
              <StopShareIcon />
            </button>
          )}
          {big && (
            <button className="call-btn" onClick={() => setLayout((l) => (l === "grid" ? "speaker" : "grid"))} title={layout === "grid" ? t("Speaker view: whoever talks is shown large") : t("Grid view: everyone the same size")}>
              {layout === "grid" ? t("Speaker") : t("Grid")}
            </button>
          )}
          <button className={recorder ? "call-btn on rec" : "call-btn"} onClick={() => (recorder ? stopRecording() : startRecording())} title={recorder ? t("Stop recording and save") : t("Record the call (everyone is told)")}>
            {recorder ? t("Stop rec") : t("Rec")}
          </button>
          <button className="call-btn" onClick={() => setDeviceDialog(true)} title={t("Voice and video settings")} aria-label={t("Voice and video settings")}>
            <CogIcon />
          </button>
          <button className="call-btn hang" onClick={onClose} title={t("Leave call")} aria-label={t("Leave call")}>
            <HangUpIcon />
          </button>
        </span>
      </div>
      <div className={shown ? "call-stage focused" : "call-stage"} ref={stage}>
        <div className="call-audio" hidden />
        {tiles.map((key) => {
          const [who, kind] = splitKey(key);
          const mine = who === me;
          const screenTile = kind.startsWith("screen");
          const controlling = control === who && screenTile;
          return (
            <div key={key} className={`call-tile video${shown === key ? " focus" : ""}${controlling ? " controlling" : ""}`} onDoubleClick={(e) => fullScreen(e.currentTarget)}>
              <div className="call-media" data-stream={key} {...controlHandlers(key)} onClick={controlling || drawOn === key ? undefined : () => setFocus((f) => (f === key ? null : key))} />
              {screenTile && !controlling && (
                <DrawLayer
                  strokes={strokes[key] ?? []}
                  drawing={drawOn === key}
                  color={colorFor(me)}
                  onStroke={(pts) => {
                    const s = { pts, color: colorFor(me) };
                    setStrokes((o) => ({ ...o, [key]: [...(o[key] ?? []), { ...s, at: Date.now() }] }));
                    signal({ t: "draw", key, ...s });
                  }}
                />
              )}
              {picking === key && (
                <RegionPicker
                  onPick={(r) => {
                    setPicking(null);
                    shareRegion(kind, r);
                  }}
                  onCancel={() => setPicking(null)}
                />
              )}
              <span className="call-name">{mine && screenTile ? t("Your screen") : label(key)}</span>
              <span className="call-tile-tools">
                {mine && screenTile && (
                  <>
                    <button onClick={() => setPicking(key)} title={t("Share only part of this screen")}>
                      {t("Part")}
                    </button>
                    <button onClick={() => stop(kind)} title={t("Stop sharing this")}>
                      {t("Stop")}
                    </button>
                  </>
                )}
                {screenTile && !controlling && (
                  <>
                    <button className={drawOn === key ? "on" : ""} onClick={() => setDrawOn((d) => (d === key ? null : key))} title={t("Draw on this screen; everyone sees it")}>
                      {drawOn === key ? t("Done drawing") : t("Draw")}
                    </button>
                    {(strokes[key]?.length ?? 0) > 0 && (
                      <button
                        onClick={() => {
                          setStrokes((o) => ({ ...o, [key]: [] }));
                          signal({ t: "clear", key });
                        }}
                      >
                        {t("Clear")}
                      </button>
                    )}
                  </>
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
                {!mine && (
                  <button onClick={() => askQuality(key, !viewQuality[key])} title={t("Ask for a lighter stream, for a slow connection or to save power")}>
                    {viewQuality[key] ? t("Low quality") : t("Best quality")}
                  </button>
                )}
                <button onClick={() => setFocus((f) => (f === key ? null : key))} title={focus === key ? t("Unpin") : t("Pin this large")}>
                  {focus === key ? t("Unpin") : t("Pin")}
                </button>
                <button onClick={() => popOut(key)} title={t("Pop out into its own small window that stays on top")}>
                  {t("Pop out")}
                </button>
                <button onClick={(e) => fullScreen(e.currentTarget.closest(".call-tile") as HTMLElement)} title={t("Full screen (Esc to leave)")} aria-label={t("Full screen")}>
                  <FullIcon />
                </button>
              </span>
            </div>
          );
        })}
        {people.map((p) => {
          const q = p.id === me ? null : quality(arrivals.current.get(`${p.id}:voice`), now, SLICE_MS);
          return (
            <div
              key={p.id}
              className={`call-tile person${p.speaking ? " speaking" : ""}${p.id === me && sending.voice && !talking ? " held" : ""}`}
              onClick={(e) => setMenuFor({ id: p.id, x: e.clientX, y: e.clientY })}
              onContextMenu={(e) => {
                e.preventDefault();
                setMenuFor({ id: p.id, x: e.clientX, y: e.clientY });
              }}
              title={p.id === me ? (p.speaking ? t("You're being picked up") : undefined) : `${t("Volume and more for {name}", { name: p.name })}\n${details(p.id)}`}
            >
              <span className="call-avatar" aria-hidden>
                {p.name.slice(0, 1).toUpperCase()}
              </span>
              <span className="call-name">
                {q && q.level !== "none" && <span className={`call-quality ${q.level}`} aria-label={q.why} />}
                {p.name} <span className="call-time">{clock(now - p.since)}</span>
                {now - (lastPiece.current.get(`${p.id}:whisper`) ?? 0) < 2000 && <span className="call-flag"> {t("whispering to you")}</span>}
                {changedBy.has(p.id) && <span className="call-flag"> {t("voice changed")}</span>}
                {p.id === me && markOn && <span className="call-flag"> {t("voice changed")}</span>}
                {recBy.has(p.id) && <span className="call-flag rec"> {t("recording")}</span>}
                {call?.muted?.includes(p.id) && <span className="call-flag"> {t("muted")}</span>}
                {call?.deafened?.includes(p.id) && <span className="call-flag"> {t("deafened")}</span>}
                {(volumes()[p.id] ?? 1) === 0 && p.id !== me && <span className="call-flag"> {t("muted for you")}</span>}
              </span>
            </div>
          );
        })}
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
          whispering={!!whisperTo?.includes(menuFor.id)}
          onWhisper={() => {
            const id = menuFor.id;
            const cur = whisperTo ?? [];
            const next = cur.includes(id) ? cur.filter((x) => x !== id) : [...cur, id];
            whisper(next.length ? next : null);
          }}
          onNoise={(on) => {
            setPersonNoise(menuFor.id, on);
            meters.noise(menuFor.id, on);
          }}
          onMod={(action, on) => social.callMod(menuFor.id, action, on).catch((e) => toast(errorText(e)))}
          onClose={() => setMenuFor(null)}
        />
      )}
      {deviceDialog && <DevicesDialog onClose={() => setDeviceDialog(false)} extra={<VoicePanel />} />}
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

const withOrWithout = (s: Set<string>, id: string, on: boolean): Set<string> => {
  if (s.has(id) === on) return s;
  const out = new Set(s);
  if (on) out.add(id);
  else out.delete(id);
  return out;
};

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
  // Each other voice: source -> clean-up filters -> volume -> left/right -> speakers.
  const outs = useRef<Map<string, { src: AudioNode; hp: BiquadFilterNode; lp: BiquadFilterNode; gain: GainNode; pan: StereoPannerNode }>>(new Map());
  const spatial = useRef(false);
  const pans = useRef<Map<string, number>>(new Map());
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
  const forget = (who: string) => {
    nodes.current.get(who)?.disconnect();
    nodes.current.delete(who);
    const o = outs.current.get(who);
    if (o) {
      o.src.disconnect();
      o.hp.disconnect();
      o.lp.disconnect();
      o.gain.disconnect();
      o.pan.disconnect();
      outs.current.delete(who);
    }
  };
  const setNoise = (o: { hp: BiquadFilterNode; lp: BiquadFilterNode }, on: boolean) => {
    // Rumble and hiss cut away from the voice band; off, both are wide open.
    o.hp.frequency.value = on ? 150 : 10;
    o.lp.frequency.value = on ? 7000 : 20000;
  };
  const analyser = (who: string, src: AudioNode, out: boolean) => {
    forget(who);
    const c = audio();
    const an = c.createAnalyser();
    an.fftSize = 512;
    src.connect(an);
    if (out) {
      const hp = c.createBiquadFilter();
      hp.type = "highpass";
      const lp = c.createBiquadFilter();
      lp.type = "lowpass";
      const gain = c.createGain();
      gain.gain.value = volumes()[who] ?? 1;
      const pan = c.createStereoPanner();
      pan.pan.value = spatial.current ? (pans.current.get(who) ?? 0) : 0;
      const o = { src, hp, lp, gain, pan };
      setNoise(o, !!personNoise()[who]);
      src.connect(hp).connect(lp).connect(gain).connect(pan).connect(c.destination);
      outs.current.set(who, o);
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
      outs.current.forEach((o, who) => (o.gain.gain.value = v[who] ?? 1));
      const sink = devices().speaker;
      const c = ctx.current as (AudioContext & { setSinkId?: (id: string) => Promise<void> }) | null;
      c?.setSinkId?.(sink).catch(() => {});
    },
    forget,
    place: (who: string, pan: number) => {
      pans.current.set(who, pan);
      const o = outs.current.get(who);
      if (o) o.pan.pan.value = spatial.current ? pan : 0;
    },
    setSpatial: (on: boolean) => {
      spatial.current = on;
      outs.current.forEach((o, who) => (o.pan.pan.value = on ? (pans.current.get(who) ?? 0) : 0));
    },
    noise: (who: string, on: boolean) => {
      const o = outs.current.get(who);
      if (o) setNoise(o, on);
    },
    // Everyone as heard here plus this PC's own voice, for a recording.
    mix: (own?: MediaStream | null) => {
      const c = audio();
      const dest = c.createMediaStreamDestination();
      outs.current.forEach((o) => o.pan.connect(dest));
      const mine = own ? c.createMediaStreamSource(own) : null;
      mine?.connect(dest);
      return {
        stream: dest.stream,
        stop: () => {
          outs.current.forEach((o) => {
            try {
              o.pan.disconnect(dest);
            } catch {
              // Already gone with its person.
            }
          });
          mine?.disconnect();
        },
      };
    },
    // Another person's voice: it now plays through the meter.
    watchElement: (who: string, el: HTMLMediaElement) => analyser(who, audio().createMediaElementSource(el), true),
    // This PC's own mic after processing: what others actually get.
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
  whispering,
  onWhisper,
  onNoise,
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
  onNoise: (on: boolean) => void;
  whispering: boolean;
  onWhisper: () => void;
  onMod: (action: "mute" | "deafen" | "remove", on: boolean) => void;
  onClose: () => void;
}) {
  const box = useRef<HTMLDivElement>(null);
  useDismiss(true, onClose, box);
  const [v, setV] = useState(volumes()[id] ?? 1);
  const [before, setBefore] = useState(v > 0 ? v : 1);
  const [noise, setNoise] = useState(!!personNoise()[id]);
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
          <button
            role="menuitemcheckbox"
            aria-checked={noise}
            onClick={() => {
              setNoise(!noise);
              onNoise(!noise);
            }}
          >
            {noise ? t("Noise removal for them: on") : t("Noise removal for them: off")}
          </button>
          <button role="menuitemcheckbox" aria-checked={whispering} onClick={() => (onWhisper(), onClose())}>
            {whispering ? t("Stop whispering to them") : t("Whisper to them (only they hear you)")}
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

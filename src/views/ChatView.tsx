// SPDX-License-Identifier: AGPL-3.0-only
import { draftOf, setDraft } from "../drafts";
import { Fragment, lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState, type ClipboardEvent, type CSSProperties, type DragEvent, type KeyboardEvent, type ReactNode } from "react";
import { open as openFile } from "@tauri-apps/plugin-dialog";
import {
  api,
  errorText,
  on,
  openInViewer,
  toBase64,
  type Chat,
  type Connectivity,
  type FeatureId,
  type ContextInfo,
  type MediaItem,
  type Message,
  type ModelCard,
  type PendingApproval,
  type QueueItem,
  type RunMode,
  type CloudChoice,
  type LensDirection, type TaughtAdapter,
  type Versions,
} from "../api";
import { APP_NAME } from "../brand";
import { t, tx } from "../i18n";
import { ApprovalCard, FolderMenu, ToolCard } from "../components/AgentCards";
import { ContextMeter } from "../components/ContextMeter";
import { CloudCost } from "../components/CloudCost";
import { findCloud, priceText, useComputeModels } from "../components/ModelChoices";
import { GpuLimitMenu } from "../components/Performance";
import { ShareChat } from "../components/ShareChat";
import { JobCard, Lightbox, MediaInline, useMediaJobs } from "../components/MediaBits";
import { ChatFilesPanel } from "../components/ChatFiles";
import { ChatMemories, ChatNotes, LinkedChats } from "../components/ChatPanels";
import { PreviewPanel, latestPreview, usePreviewWidth, type PreviewTarget } from "../components/PreviewPanel";
import { BrowserPanel } from "./WebViewerView";
import { useMediaUrl } from "../media";
import { Markdown } from "../components/Markdown";
import { CopyButton } from "../components/CopyButton";
// It brings React's page renderer, which nothing else needs at startup.
const ExportChat = lazy(() => import("../components/ExportChat").then((m) => ({ default: m.ExportChat })));
import { AssistantStarters } from "../components/Assistants";
import { Modal } from "../components/Modal";
import type { PushToast } from "../components/Toasts";
import { MicButton, SpeakButton, VoicePanel } from "../components/Voice";
import { insertDictation, words } from "../format";
import { aiModeUrl, GoogleSetup } from "../components/GoogleSetup";
import { LookForModels } from "./ModelsView";
import { IncognitoIcon } from "../components/IncognitoIcon";
import { BackIcon, EditIcon, ForwardIcon, GlobeIcon, NewWindowIcon, PaperclipIcon, ReloadIcon, VoiceIcon } from "../components/Icons";
import { rememberSent, SmartInput, type SlashCommand } from "../components/SmartInput";
import { expandSlash, SavedPromptsManager, useSavedPrompts } from "../components/SavedPrompts";

interface Props {
  chat: Chat | null;
  projectName: string | null;
  onOpenChat: (id: string) => void;
  installed: ModelCard[];
  defaultModel: string | null;
  connectivity: Connectivity;
  onNewChat: () => void;
  onGoModels: () => void;
  onChanged: () => void;
  onDeleted: () => void;
  onToggleWeb: () => void;
  features: Set<FeatureId>;
  onGoFeatures: () => void;
  /** This chat's own browser, open beside it (and where it starts), or null. */
  browser?: { url: string | null; at: number } | null;
  onBrowser?: (open: boolean) => void;
  /** Opens this chat in a window of its own (none when it's in one). */
  onPopOut?: () => void;
  toast: PushToast;
}

interface Streaming {
  id: string;
  content: string;
  thinking: string;
}

type Status = "idle" | "loading" | "thinking" | "writing" | "working";

const MODES: { id: RunMode; label: string; help: string }[] = [
  { id: "plan", label: tx("Plan"), help: tx("Looks around and proposes a plan. Changes nothing until you approve.") },
  { id: "auto", label: tx("Auto"), help: tx("Changes files and runs commands on its own. Asks before deleting, before anything leaves this PC, and before commands that throw work away.") },
  { id: "bypass", label: tx("Bypass"), help: tx("Changes files and runs commands without asking, deletes included. Sending something off this PC still asks. Undo still works for file changes.") },
];

export function ChatView(props: Props) {
  const { chat, installed } = props;

  if (installed.length === 0) {
    return (
      <div className="empty">
        <img src="logo.svg" alt="" width={64} height={64} />
        <h1>{t("Welcome to {app}", { app: APP_NAME })}</h1>
        <p className="muted">{t("Install a model to start chatting. Everything runs privately on this PC.")}</p>
        <button className="btn primary" onClick={props.onGoModels}>
          {t("Choose a model")}
        </button>
        {/* The list refreshes itself when models are found. */}
        <LookForModels compact onRefresh={async () => {}} toast={props.toast} />
      </div>
    );
  }
  if (!chat) {
    return (
      <div className="empty">
        <img src="logo.svg" alt="" width={64} height={64} />
        <h1>{t("What can I help with?")}</h1>
        <p className="muted">{t("Start a new chat, or pick one from the sidebar.")}</p>
        <button className="btn primary" onClick={props.onNewChat}>
          {t("New chat")}
        </button>
      </div>
    );
  }
  return <Conversation key={chat.id} {...props} chat={chat} />;
}

function Conversation({ chat, projectName, onOpenChat, installed, defaultModel, connectivity, onChanged, onDeleted, onToggleWeb, features, onGoFeatures, onGoModels, browser, onBrowser, onPopOut, toast }: Props & { chat: Chat }) {
  const [messages, setMessages] = useState<Message[]>([]);
  const [streaming, setStreamingNow] = useState<Streaming | null>(null);
  // Tokens arrive dozens a second and the live reply is drawn as Markdown
  // (with maths): they're gathered and drawn at most every 80 ms, not each.
  const buffered = useRef<Streaming | null>(null);
  const flushTimer = useRef<number | undefined>(undefined);
  const setStreaming = useCallback((v: Streaming | null | ((s: Streaming | null) => Streaming | null)) => {
    buffered.current = null;
    window.clearTimeout(flushTimer.current);
    flushTimer.current = undefined;
    setStreamingNow(v);
  }, []);
  const addDelta = useCallback((id: string, content: string, thinking: string) => {
    const b = buffered.current;
    if (b && b.id === id) {
      b.content += content;
      b.thinking += thinking;
    } else {
      buffered.current = { id, content, thinking };
    }
    if (flushTimer.current !== undefined) return;
    flushTimer.current = window.setTimeout(() => {
      flushTimer.current = undefined;
      const p = buffered.current;
      buffered.current = null;
      if (p) setStreamingNow((s) => (s && s.id === p.id ? { ...s, content: s.content + p.content, thinking: s.thinking + p.thinking } : s));
    }, 80);
  }, []);
  useEffect(() => () => window.clearTimeout(flushTimer.current), []);
  const [status, setStatus] = useState<Status>("idle");
  // What screen readers hear: when a reply starts and when it's ready (not
  // every streamed word).
  const [announce, setAnnounce] = useState("");
  const lastStatus = useRef<Status>("idle");
  useEffect(() => {
    const prev = lastStatus.current;
    lastStatus.current = status;
    if (status === prev) return;
    if (status === "idle") setAnnounce(t("Reply ready."));
    else if (status === "loading") setAnnounce(t("Loading the model."));
    else if (prev === "idle" || prev === "loading") setAnnounce(t("Replying…"));
  }, [status]);
  /** What's too hot while the heat guard pauses between steps. */
  const [cooling, setCooling] = useState<string | null>(null);
  // A cloud provider was busy: when the next try goes, and which it is.
  const [retrying, setRetrying] = useState<{ provider: string; at: number; attempt: number; of: number } | null>(null);
  // Waiting in line for another model: one runs at a time.
  const [queued, setQueued] = useState<{ place: number; model: string; card?: boolean } | null>(null);
  // Before a picture of something named: checking (or looking up) its look.
  const [lookingUp, setLookingUp] = useState<string | null>(null);
  // The picture reader is describing attached pictures for a model that can't see them.
  const [readingPictures, setReadingPictures] = useState(false);
  const [helperSteps, setHelperSteps] = useState<Record<string, string[]>>({});
  const [handingOff, setHandingOff] = useState(false);
  const [compacting, setCompacting] = useState(false);
  // Auto handed the last message to another model (cleared when the next is sent).
  const [routed, setRouted] = useState<{ name: string; task: string } | null>(null);
  const [context, setContext] = useState<ContextInfo | null>(null);
  // What's typed and not sent stays with its chat (`drafts`).
  const [input, setInput] = useState(() => draftOf(chat.id));
  useEffect(() => setDraft(chat.id, input), [chat.id, input]);
  useEffect(() => {
    const loaded = () => setInput((v) => v || draftOf(chat.id));
    window.addEventListener("drafts:loaded", loaded);
    return () => window.removeEventListener("drafts:loaded", loaded);
  }, [chat.id]);
  const [error, setError] = useState<string | null>(null);
  const [tps, setTps] = useState<number | null>(null);
  const [pending, setPending] = useState<PendingApproval[]>([]);
  /** Messages waiting for the reply to finish (Ctrl+Enter). */
  const [queue, setQueue] = useState<QueueItem[]>([]);
  const [showFiles, setShowFiles] = useState(false);
  // A site the assistant built, shown beside the chat.
  const [preview, setPreview] = useState<PreviewTarget | null>(null);
  const [showPreview, setShowPreview] = useState(false);
  // This chat has its own browser (open, or minimized with its page kept).
  const [hasBrowser, setHasBrowser] = useState(false);
  useEffect(() => {
    api.viewerExists(chat.id).then(setHasBrowser).catch(() => {});
    const sub = on("viewer:dock", (p) => p.key === chat.id && setHasBrowser(true));
    return () => {
      sub.then((un) => un());
    };
  }, [chat.id]);
  const [previewStamp, setPreviewStamp] = useState(0);
  const [previewWidth, setPreviewWidth] = usePreviewWidth();
  // Sent while it worked, not read by the model yet: shown in the chat meanwhile.
  const [waiting, setWaiting] = useState<QueueItem[]>([]);
  /** The model has read the messages sent up to then (it started answering). */
  const [readUpTo, setReadUpTo] = useState(0);
  // Tools running now (lookups can run several at once).
  const [runningCalls, setRunningCalls] = useState<Set<string>>(() => new Set());
  const [undoable, setUndoable] = useState<Set<string>>(new Set());
  const [renaming, setRenaming] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [panel, setPanel] = useState<"memories" | "links" | "notes" | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [confirmBypass, setConfirmBypass] = useState(false);
  const [voice, setVoice] = useState(false);
  /** Pictures attached to the next message. */
  const [attached, setAttached] = useState<MediaItem[]>([]);
  const [lightbox, setLightbox] = useState<MediaItem | null>(null);
  const jobs = useMediaJobs().filter((j) => j.chat_id === chat.id);
  const scroller = useRef<HTMLDivElement>(null);
  // Whether the view follows new text: only while the person is at the
  // bottom. Scrolling up while a reply streams leaves them where they are.
  const following = useRef(true);
  const [awayFromEnd, setAwayFromEnd] = useState(false);
  const inputRef = useRef<HTMLTextAreaElement>(null);

  const modelId = chat.model_id ?? defaultModel;
  const model = installed.find((m) => m.id === modelId) ?? null;
  // Cloud models, from the providers turned on in Settings.
  const [cloudChoices, setCloudChoices] = useState<CloudChoice[]>([]);
  useEffect(() => {
    api.cloudChoices().then(setCloudChoices).catch(() => {});
  }, [connectivity]);
  // "Google" in the model list sets Gemini up the first time.
  const [googleSetup, setGoogleSetup] = useState(false);
  const hasGemini = cloudChoices.some((c) => c.id.startsWith("cloud:gemini:"));
  // Matched by model too: a chat synced from another PC names that PC's provider.
  const cloud = findCloud(cloudChoices, modelId);
  // Models on the linked compute PC.
  const compute = useComputeModels();
  const computeModel = compute.models.find((m) => m.id === modelId) ?? null;
  // Models taught on this PC (a model plus an adapter).
  const [taught, setTaught] = useState<TaughtAdapter[]>([]);
  useEffect(() => {
    api.finetuneView().then((v) => setTaught(v.adapters)).catch(() => {});
  }, []);
  const teach = taught.find((a) => `adapter:${a.id}` === modelId) ?? null;
  // Models steered in the Lens (a model plus a direction).
  const [steered, setSteered] = useState<LensDirection[]>([]);
  useEffect(() => {
    api.lensDirections().then(setSteered).catch(() => {});
  }, []);
  const steering = steered.find((d) => `steer:${d.id}` === modelId) ?? null;
  const steerBase = steering ? installed.find((m) => m.id === steering.model_id) ?? null : null;
  const steerLabel = (d: LensDirection) => `${installed.find((m) => m.id === d.model_id)?.name ?? d.model_id}, ${t("steered")}: ${d.name} (${d.strength > 0 ? "+" : ""}${d.strength.toFixed(1)})`;
  const cloudId = modelId?.startsWith("cloud:") ? modelId : null;
  const teachBase = teach ? installed.find((m) => m.id === teach.base) ?? null : null;
  const modelName = model?.name ?? (teach ? `${teach.base_name} + ${teach.name}` : null) ?? (steering ? steerLabel(steering) : null) ?? cloud?.name ?? (computeModel ? t("{name} on {pc}", { name: computeModel.name, pc: compute.pc }) : null);
  const canTools = model ? model.tools : teachBase ? teachBase.tools : steerBase ? steerBase.tools : computeModel ? computeModel.tools : !!cloud;
  const canSee = model ? !!model.vision : computeModel ? computeModel.vision : !!cloud?.vision;
  const webOn = connectivity !== "offline" || chat.web;
  // The Web search feature: search and reading pages in every chat, even Offline.
  const searchOn = webOn || features.has("web_search");
  const busy = status !== "idle" || compacting;
  // A reply is being written: what's typed now is added to it or queued.
  const replying = status !== "idle";
  // The suggested next message: once a reply is done and the box is empty,
  // the model that answered guesses what's asked next (Tab takes it).
  const [nextUp, setNextUp] = useState<string | null>(null);
  const wasReplying = useRef(false);
  const chatNow = useRef(chat.id);
  chatNow.current = chat.id;
  useEffect(() => {
    if (wasReplying.current && !replying && features.has("next_prompt") && !inputRef.current?.value) {
      const id = chat.id;
      api
        .typingNext(id)
        .then((s) => {
          if (s && chatNow.current === id && !inputRef.current?.value) setNextUp(s);
        })
        .catch(() => {});
    }
    wasReplying.current = replying;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [replying]);
  useEffect(() => setNextUp(null), [chat.id]);
  // Messages since the last summary: what Compact now would cover.
  const lastSummary = messages.map((m) => !!m.meta?.compacted).lastIndexOf(true);
  const canCompact = !busy && messages.slice(lastSummary + 1).some((m) => m.role === "user");

  const reload = useCallback(() => {
    api.messages(chat.id).then(setMessages);
    api.undoableTurns(chat.id).then((turns) => setUndoable(new Set(turns))).catch(() => {});
  }, [chat.id]);
  // Earlier versions after edited messages and other replies.
  const [versions, setVersions] = useState<Versions[]>([]);
  const [editing, setEditing] = useState<string | null>(null);
  useEffect(() => {
    api.versions(chat.id).then(setVersions).catch(() => setVersions([]));
  }, [chat.id, messages]);

  useEffect(() => {
    reload();
    api.context(chat.id).then(setContext);
    api.pendingApprovals(chat.id).then(setPending).catch(() => {});
    api.chatQueue(chat.id).then(setQueue).catch(() => {});
    api.chatInbox(chat.id).then(setWaiting).catch(() => setWaiting([]));
    // Back in a chat that's still answering: show where it is.
    api.live(chat.id).then((r) => {
      if (!r) return;
      setStatus(r.status);
      if (r.message_id) setStreaming({ id: r.message_id, content: r.content, thinking: r.thinking });
    }).catch(() => {});
    inputRef.current?.focus();
  }, [chat.id, reload]);

  // Messages written on another PC arrive by sync (not mid-reply).
  useEffect(() => {
    if (busy) return;
    const sub = on("sync:changed", () => reload());
    return () => {
      sub.then((un) => un());
    };
  }, [busy, reload]);

  useEffect(() => {
    const mine = <T extends { chat_id: string }>(fn: (p: T) => void) => (p: T) => p.chat_id === chat.id && fn(p);
    const subs = [
      on("chat:status", mine((p) => {
        setQueued(p.status === "queued" ? { place: p.place ?? 1, model: p.detail ?? "" } : p.status === "waiting_card" ? { place: p.place ?? 1, model: "", card: true } : null);
        if (p.status === "handoff") setHandingOff(true);
        else if (p.status === "compacting") setCompacting(true);
        else if (p.status === "retrying") setRetrying({ provider: p.provider ?? "", at: Date.now() + (p.seconds ?? 1) * 1000, attempt: p.attempt ?? 1, of: p.of ?? 1 });
        else if (p.status === "idle") {
          setCompacting(false);
          setRetrying(null);
          setLookingUp(null);
          setReadingPictures(false);
        } else if (p.status === "looking_up") {
          setLookingUp(p.detail ?? "");
          setStatus("working");
        } else if (p.status === "reading_pictures") {
          setReadingPictures(true);
          setStatus("working");
        } else if (p.status === "cooling") {
          setCooling(p.detail ?? t("the PC"));
          setStatus("working");
        } else setStatus("loading");
      })),
      on("agent:helper", mine((p) => setHelperSteps((all) => ({ ...all, [p.call_id]: [...(all[p.call_id] ?? []), p.step] })))),
      on("chat:compacted", mine(() => {
        setCompacting(false);
        reload();
        api.context(chat.id).then(setContext).catch(() => {});
      })),
      on("chat:context", mine((p) => setContext(p.context))),
      on("chat:routed", mine((p) => setRouted({ name: p.name, task: p.task }))),
      on("chat:queue", mine((p) => setQueue(p.queue))),
      on("chat:inbox", mine((p) => setWaiting(p.waiting))),
      on("chat:read", mine((p) => setReadUpTo((r) => Math.max(r, p.up_to)))),
      // The assistant previewed a site: it opens beside the chat (again, with its changes).
      on("chat:preview", mine((p) => {
        setPreview({ url: p.url, site: p.site, root: p.root, start: p.start, port: p.port });
        setShowPreview(true);
        setPreviewStamp((n) => n + 1);
      })),
      // Someone this chat is shared with wrote in it (or their model did).
      on("chat:changed", mine(() => reload())),
      on("chat:thought", mine(() => reload())),
      // Saved by itself after a reply: said once, with where to change it.
      on("memory:auto", mine((p) => toast(t("Remembered: {what}", { what: p.saved.join(" ") }), "success"))),
      on("chat:start", mine((p) => {
        setCooling(null);
        setRetrying(null);
        setQueued(null);
        setLookingUp(null);
        setReadingPictures(false);
        setStatus("thinking");
        // The same message tried again: what it wrote stays (and what was
        // still waiting to be drawn).
        const b = buffered.current?.id === p.message_id ? buffered.current : null;
        setStreaming((s) =>
          s && s.id === p.message_id ? { ...s, content: s.content + (b?.content ?? ""), thinking: s.thinking + (b?.thinking ?? "") } : { id: p.message_id, content: "", thinking: "" },
        );
      })),
      on("chat:delta", mine((p) => {
        setRetrying(null);
        if (p.content) setStatus("writing");
        addDelta(p.message_id, p.content ?? "", p.thinking ?? "");
      })),
      on("agent:step", mine(() => {
        setStreaming(null);
        setStatus("working");
        setRunningCalls(new Set());
        api.messages(chat.id).then(setMessages);
      })),
      on("agent:tool_start", mine((p) => setRunningCalls((all) => new Set(all).add(p.call_id)))),
      on("agent:approval", mine((p) => setPending((all) => [...all.filter((a) => a.call_id !== p.call_id), p]))),
      on("agent:approval_done", mine((p) => setPending((all) => all.filter((a) => a.call_id !== p.call_id)))),
      on("chat:done", mine((p) => {
        setQueued(null);
        if (p.context) setContext(p.context);
        if (p.tps !== undefined) setTps(p.tps ?? null);
        if (p.error) setError(errorText(p.error));
        setStreaming(null);
        setStatus("idle");
        setRunningCalls(new Set());
        setPending([]);
        reload();
      })),
    ];
    return () => subs.forEach((s) => s.then((un) => un()));
  }, [chat.id, reload]);

  // Keep the newest text in view while it streams, unless the person
  // scrolled up to read.
  // Where the view was last put or seen: only moving up from there means the
  // person scrolled (scroll events arrive late, after more text may have
  // landed, so a gap at the bottom alone proves nothing).
  const lastTop = useRef(0);
  const toEnd = () => {
    const el = scroller.current;
    if (!el) return;
    following.current = true;
    setAwayFromEnd(false);
    el.scrollTop = el.scrollHeight;
    lastTop.current = el.scrollTop;
  };
  const onScroll = () => {
    const el = scroller.current;
    if (!el) return;
    if (el.scrollHeight - el.scrollTop - el.clientHeight < 24) following.current = true;
    else if (el.scrollTop < lastTop.current - 2) following.current = false;
    lastTop.current = el.scrollTop;
    setAwayFromEnd(!following.current);
  };
  useEffect(() => {
    const el = scroller.current;
    if (el && following.current) {
      el.scrollTop = el.scrollHeight;
      lastTop.current = el.scrollTop;
    }
  }, [messages, streaming, pending]);
  // A newly opened chat starts at its end.
  useEffect(() => {
    following.current = true;
    setAwayFromEnd(false);
    setPreview(null);
    setShowPreview(false);
  }, [chat.id]);
  // The site this chat previewed last, until the assistant previews another.
  const lastPreview = useMemo(() => latestPreview(messages), [messages]);
  const previewTarget = preview ?? lastPreview;
  const openPreview = useCallback((p: PreviewTarget) => {
    setPreview(p);
    setShowPreview(true);
  }, []);

  const sendText = async (text: string, images: string[] = [], force = false) => {
    if ((!text && images.length === 0) || (busy && !force)) return;
    rememberSent(text);
    setNextUp(null);
    toEnd();
    setError(null);
    setRouted(null);
    setStatus("loading");
    const meta = images.length ? ({ images } as Message["meta"]) : undefined;
    setMessages((m) => [...m, { id: `pending-${Date.now()}`, chat_id: chat.id, role: "user", content: text, thinking: null, created_at: Date.now(), meta }]);
    try {
      await api.send(chat.id, text, images);
    } catch (e) {
      setError(errorText(e));
      setStreaming(null);
      setStatus("idle");
      reload();
    } finally {
      onChanged();
    }
  };

  // Dictated phrases land at the cursor.
  const dictate = useCallback((text: string) => {
    const el = inputRef.current;
    setInput((value) => {
      const start = el?.selectionStart ?? value.length;
      const end = el?.selectionEnd ?? value.length;
      const r = insertDictation(value, start, end, text);
      requestAnimationFrame(() => {
        el?.focus();
        el?.setSelectionRange(r.cursor, r.cursor);
      });
      return r.value;
    });
  }, []);

  // The assistant asked for web access: turn it on for this chat and carry on.
  const enableWeb = useCallback(async () => {
    await api.setChatWeb(chat.id, true);
    onChanged();
    sendText("Web access is on now. Please go ahead.");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [chat.id]);

  // Voice mode sends what it heard itself; show it like a typed message.
  const heard = useCallback((text: string) => {
    setError(null);
    setStatus("loading");
    setMessages((m) => [...m, { id: `pending-${Date.now()}`, chat_id: chat.id, role: "user", content: text, thinking: null, created_at: Date.now() }]);
  }, [chat.id]);

  // Compact now: the model summarizes the chat so far and works from that.
  const compactNow = async () => {
    setCompacting(true);
    setError(null);
    try {
      await api.compactChat(chat.id);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setCompacting(false);
      reload();
      api.context(chat.id).then(setContext).catch(() => {});
    }
  };

  // Typing "/": saved prompts (they fill the box), /compact, and editing them.
  const [prompts, setPrompts] = useSavedPrompts();
  const [managingPrompts, setManagingPrompts] = useState(false);
  const slashCommands: SlashCommand[] = [
    ...prompts.map((p) => ({
      name: p.name,
      hint: p.text.replace(/\s+/g, " ").slice(0, 80),
      pick: () => {
        setInput(p.text.replace("{input}", ""));
        requestAnimationFrame(() => inputRef.current?.focus());
      },
    })),
    { name: "compact", hint: t("Sum up the chat so far, so it has room to go on"), pick: () => (setInput(""), compactNow()) },
    { name: "prompts", hint: t("Add or change saved prompts"), pick: () => (setInput(""), setManagingPrompts(true)) },
  ];

  const send = () => {
    const text = input.trim();
    if ((!text && attached.length === 0) || busy) return;
    // Like Claude: /compact compacts instead of sending.
    if (text.toLowerCase() === "/compact") {
      setInput("");
      compactNow();
      return;
    }
    const images = attached.map((a) => a.id);
    setInput("");
    setAttached([]);
    // "/name more words": the saved prompt, with the words.
    sendText(expandSlash(text, prompts), images);
  };

  // While it writes: Enter adds the message to what it's working on (it
  // reads it at its next step); Ctrl+Enter sends it once the reply is done.
  const addWhileReplying = async (later: boolean) => {
    const text = input.trim();
    if (!text && attached.length === 0) return;
    const images = attached.map((a) => a.id);
    setInput("");
    setAttached([]);
    try {
      if (later) {
        setQueue(await api.chatQueueAdd(chat.id, text, images));
        return;
      }
      // In the chat at once; the backend's list replaces this one.
      const mine = { id: `pending-${Date.now()}`, text, images, at: Date.now() };
      setWaiting((w) => [...w, mine]);
      toEnd();
      if ((await api.chatAdd(chat.id, text, images)) === "idle") {
        // It finished just now: an ordinary message.
        setWaiting((w) => w.filter((x) => x.id !== mine.id));
        setTimeout(() => sendText(text, images, true), 300);
      }
    } catch (e) {
      setInput(text);
      toast(errorText(e), "error");
    }
  };

  const attachFile = async () => {
    const picked = await openFile({
      multiple: true,
      filters: [
        { name: t("Pictures and songs"), extensions: ["png", "jpg", "jpeg", "webp", "mp3", "wav", "m4a", "aac", "flac", "ogg", "wma"] },
        { name: t("Pictures"), extensions: ["png", "jpg", "jpeg", "webp"] },
        { name: t("Songs and sounds"), extensions: ["mp3", "wav", "m4a", "aac", "flac", "ogg", "wma"] },
      ],
    });
    const paths = Array.isArray(picked) ? picked : typeof picked === "string" ? [picked] : [];
    for (const p of paths) {
      try {
        const item = await api.mediaImport(p, chat.id);
        setAttached((a) => [...a, item]);
      } catch (e) {
        toast(errorText(e), "error");
      }
    }
  };

  // Pasting a picture (a screenshot, an image copied from a page) attaches it.
  const onPaste = async (e: ClipboardEvent<HTMLTextAreaElement>) => {
    const files = [...e.clipboardData.items].filter((i) => i.kind === "file" && i.type.startsWith("image/")).map((i) => i.getAsFile()).filter((f): f is File => !!f);
    if (files.length === 0) return;
    e.preventDefault();
    await attachPictures(files);
  };
  // Pictures dropped on the message box go with the message, as pasted ones do.
  const onDrop = async (e: DragEvent<HTMLDivElement>) => {
    if (!e.dataTransfer.types.includes("Files")) return;
    e.preventDefault();
    const all = [...e.dataTransfer.files];
    const pictures = all.filter((f) => f.type.startsWith("image/"));
    if (pictures.length < all.length) toast(t("Only pictures can be dropped here. For other files, give the chat a folder (the folder button) and ask about them."), "info");
    await attachPictures(pictures);
  };
  const attachPictures = async (files: File[]) => {
    for (const f of files) {
      try {
        const data = toBase64(new Uint8Array(await f.arrayBuffer()));
        const item = await api.mediaAdd({ data, mime: f.type, op: "paste", chatId: chat.id, hidden: true });
        setAttached((a) => [...a, item]);
      } catch (err) {
        toast(errorText(err), "error");
      }
    }
  };

  const onKey = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
      e.preventDefault();
      if (replying) addWhileReplying(e.ctrlKey || e.metaKey);
      else send();
    }
  };

  const switchModel = async (id: string) => {
    if (id === "google:setup") {
      setGoogleSetup(true);
      return;
    }
    try {
      await api.setChatModel(chat.id, id);
      setContext(null);
      onChanged();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  const setMode = async (mode: RunMode) => {
    if (mode === "bypass" && chat.mode !== "bypass") {
      setConfirmBypass(true);
      return;
    }
    await api.setChatMode(chat.id, mode);
    onChanged();
  };

  const runPlan = async () => {
    await api.setChatMode(chat.id, "auto");
    onChanged();
    sendText("Go ahead and carry out the plan.");
  };

  const undo = async (turnId: string) => {
    try {
      const notes = await api.undoTurn(chat.id, turnId);
      toast(notes.length ? t("Undone, with notes: {notes}", { notes: notes.join(" ") }) : t("Changes from that reply were undone."), notes.length ? "error" : "success");
      reload();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  // Asking again from a message: another reply, or the message edited.
  // What was there stays as a version to switch back to.
  const askAgain = async (run: () => Promise<unknown>) => {
    if (busy) return;
    setEditing(null);
    setNextUp(null);
    setError(null);
    setRouted(null);
    setStatus("loading");
    try {
      await run();
    } catch (e) {
      setError(errorText(e));
      setStreaming(null);
      setStatus("idle");
      reload();
    } finally {
      onChanged();
    }
  };
  const turns: TurnActions = {
    versions,
    editing,
    setEditing,
    again: (id) => askAgain(() => api.regenerate(chat.id, id)),
    edit: (id, text) => askAgain(() => api.editMessage(chat.id, id, text)),
    switchTo: (anchor, to) => api.switchVersion(chat.id, anchor, to).catch((e) => toast(errorText(e), "error")),
  };
  const items = useMemo(
    () => renderItems(messages, undoable, busy, runningCalls, undo, helperSteps, onOpenChat, features.has("read_aloud") ? toast : undefined, searchOn ? null : enableWeb, setLightbox, readUpTo, openPreview, toast, turns),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [messages, undoable, busy, runningCalls, helperSteps, features, searchOn, readUpTo, versions, editing],
  );
  const last = messages[messages.length - 1];
  const showRunPlan = chat.mode === "plan" && !busy && last?.role === "assistant" && !!last.content && !last.tool_calls?.length;

  // One panel beside the chat at a time: its browser, else the site preview.
  const previewShown = showPreview && !!previewTarget && !browser;
  const beside = previewShown || !!browser;
  return (
    <div className={`conversation ${beside ? "with-preview" : ""}`} style={beside ? ({ "--preview-w": `${previewWidth}%` } as CSSProperties) : undefined}>
      <header className="chat-header">
        <h1 className="ellipsis" title={chat.title}>{chat.title}</h1>
        <div className="mode-switch" role="radiogroup" aria-label={t("Run mode")}>
          {MODES.map((m) => (
            <button
              key={m.id}
              role="radio"
              aria-checked={chat.mode === m.id}
              className={`mode ${m.id} ${chat.mode === m.id ? "active" : ""}`}
              title={t(m.help)}
              onClick={() => setMode(m.id)}
              disabled={busy}
            >
              {t(m.label)}
            </button>
          ))}
        </div>
        {!cloudId && <GpuLimitMenu toast={toast} />}
        <span className="sr-only" role="status" aria-live="polite">{announce}</span>
        <div className="chat-actions">
          {previewTarget && (
            <button
              className={`btn ghost small ${previewShown ? "active" : ""}`}
              aria-pressed={previewShown}
              onClick={() => {
                if (browser) onBrowser?.(false);
                setShowPreview(!previewShown);
              }}
              title={t("The site built in this chat, running beside it")}
            >
              {t("Preview")}
            </button>
          )}
          {(hasBrowser || !!browser) && onBrowser && (
            <button className={`btn ghost small ${browser ? "active" : ""}`} aria-pressed={!!browser} onClick={() => onBrowser(!browser)} title={t("This chat's own browser, beside it: what the assistant browses shows here")}>
              {t("Browser")}
            </button>
          )}
          <button className={`btn ghost small ${showFiles ? "active" : ""}`} aria-pressed={showFiles} onClick={() => setShowFiles((v) => !v)} title={t("Pictures and files made in this chat")}>
            {t("Files")}
          </button>
          {onPopOut && !chat.incognito && (
            <button className="icon-btn" onClick={onPopOut} aria-label={t("Open this chat in a new window")} title={t("Open this chat in a new window, with its own model, beside the others")}>
              <NewWindowIcon />
            </button>
          )}
          {features.has("people") && !chat.incognito && <ShareChat chatId={chat.id} toast={toast} />}
          {!chat.incognito && (
            <>
              <button className="btn ghost small" onClick={() => setPanel("memories")} title={t("What the assistant remembered from this chat")}>{t("Memories")}</button>
              <button className="btn ghost small" onClick={() => setPanel("links")} title={t("Chats connected with this one to work together")}>{t("Connected chats")}</button>
              <button className="btn ghost small" onClick={() => setPanel("notes")} title={t("This chat's memory and plan")}>{t("Notes")}</button>
            </>
          )}
          {!chat.incognito && (
            <button className="btn ghost small" onClick={() => setExporting(true)} title={t("Save this chat as a file, or print it")} disabled={!messages.length}>
              {t("Save as")}
            </button>
          )}
          <button className="btn ghost small" onClick={() => setRenaming(true)}>{t("Rename")}</button>
          <button className="btn ghost small danger" onClick={() => setConfirmDelete(true)}>{t("Delete")}</button>
        </div>
      </header>

      {(chat.incognito || projectName || chat.parent_id) && (
        <div className={`chat-banner ${chat.incognito ? "incognito" : ""}`}>
          {chat.incognito ? (
            <>
              <IncognitoIcon /> {t("Incognito: this chat isn't saved and doesn't use or create memories. It's deleted when you leave it, with the pictures and songs made or pasted here (save any you want to keep).")}
            </>
          ) : projectName ? t("In project {name}", { name: projectName }) : null}
          {chat.parent_id && (
            <button className="link" onClick={() => onOpenChat(chat.parent_id!)}>
              {t("↩ Continues an earlier chat")}
            </button>
          )}
        </div>
      )}
      <div
        className="messages"
        ref={scroller}
        onScroll={onScroll}
        onWheel={(e) => {
          // Stop following at once, before the next bit of text arrives.
          if (e.deltaY < 0) following.current = false;
        }}
      >
        {messages.length === 0 && !streaming && (
          <div className="hello">
            <AssistantStarters
              chatId={chat.id}
              onPick={(s) => {
                setInput(s.endsWith(":") ? `${s} ` : s);
                requestAnimationFrame(() => inputRef.current?.focus());
              }}
            />
            <p className="muted">
              {cloud ? (
                <>
                  {t("You're chatting with")} <strong>{cloud.name}</strong>
                  {t(", which runs on {provider}'s servers. What you send goes to them.", { provider: cloud.provider })}
                </>
              ) : (
                <>
                  {t("You're chatting with")} <strong>{model?.name ?? t("a local model")}</strong>
                  {t(", running on this PC.")}
                </>
              )}
            </p>
            {!cloud && model && model.params_b > 0 && model.params_b < 3 && (
              <p className="muted small">
                {t("Small models like this one often miss questions about your email, calendar and notes. A model with 4B or more parameters works much better.")}{" "}
                <button className="link" onClick={onGoModels}>{t("Find one in Models")}</button>
              </p>
            )}
            {canTools && features.has("files") && <p className="muted small">{t("Share a folder with the folder button below and it can read and change files there, or run commands to build and test code.")}</p>}
            {!features.has("files") && features.size === 0 && (
              <p className="muted small">
                {t("Want dictation, voice chat, files or meetings?")} <button className="link" onClick={onGoFeatures}>{t("Add features")}</button>
              </p>
            )}
          </div>
        )}
        {items}
        {streaming && (
          <MessageView
            m={{ id: streaming.id, chat_id: chat.id, role: "assistant", content: streaming.content, thinking: streaming.thinking || null, created_at: 0 }}
            live={status}
          />
        )}
        {waiting.map((w) => (
          <MessageView
            key={w.id}
            m={{ id: w.id, chat_id: chat.id, role: "user", content: w.text || t("(a picture)"), thinking: null, created_at: w.at, meta: w.images.length ? ({ images: w.images } as Message["meta"]) : undefined }}
            waiting
          />
        ))}
        {/* Asking for the web, a folder or an API key is a popup (WebPermission, FolderPermission, KeyPermission), wherever the person is. */}
        {pending.filter((p) => p.tool !== "request_web" && p.tool !== "request_folder" && p.tool !== "request_api_key" && p.preview.kind !== "privacy").map((p) => (
          <ApprovalCard key={p.call_id} p={p} onAnswered={() => setPending((all) => all.filter((a) => a.call_id !== p.call_id))} />
        ))}
        {jobs.map((j) => (
          <div key={j.id} className="chat-media-job">
            <JobCard job={j} />
          </div>
        ))}
        {routed && (
          <p className="muted small pad">
            {routed.task === "code"
              ? t("Auto: {name} is answering this, it's better at code.", { name: routed.name })
              : routed.task === "writing"
                ? t("Auto: {name} is answering this, it's better at writing.", { name: routed.name })
                : routed.task === "languages"
                  ? t("Auto: {name} is answering this, it's better at languages.", { name: routed.name })
                  : t("Auto: {name} is answering this.", { name: routed.name })}
          </p>
        )}
        {handingOff && <p className="muted small pad"><span className="spinner small" aria-hidden />{t("This chat is nearly full. Summarizing it to continue in a new chat…")}</p>}
        {compacting && <p className="muted small pad"><span className="spinner small" aria-hidden />{t("Compacting: the model is summarizing the earlier part of this chat so it can keep going here…")}</p>}
        {retrying && <RetryLine {...retrying} />}
        {status === "loading" && !streaming && queued && (
          <p className="muted small pad queued">
            <span className="spinner small" aria-hidden />
            {queued.card
              ? t("Your message is number {place} in line for the graphics card. It starts when the work ahead of it is done, so nothing slows down on the processor. The line is at the bottom of the window.", { place: queued.place })
              : t("Your message is number {place} in line. {model} is answering in another chat, and one model runs at a time, so this starts when it's done.", { place: queued.place, model: queued.model || t("Another model") })}
          </p>
        )}
        {status === "loading" && !streaming && !queued && <p className="muted small pad"><span className="spinner small" aria-hidden />{cloud ? t("Connecting to {provider}…", { provider: cloud.provider }) : t("Loading {name} into memory… (the first time on a PC it also gets the AI engine, which can take a minute)", { name: model?.name ?? t("the model") })}</p>}
        {status === "working" && !streaming && pending.length === 0 && (
          <p className="muted small pad">
            <span className="spinner small" aria-hidden />
            {cooling
              ? t("Letting the PC cool down before the next step ({what})…", { what: cooling })
              : readingPictures
                ? t("The picture reader is looking at your picture, so the model knows what's in it…")
                : lookingUp
                ? t("Looking up what {what} looks like, so the picture matches…", { what: lookingUp })
                : lookingUp === ""
                  ? t("Checking the names in the picture's description…")
                  : t("Working…")}
          </p>
        )}
        {showRunPlan && (
          <div className="plan-actions">
            <button className="btn primary" onClick={runPlan}>{t("Run this plan")}</button>
            <span className="muted small">{t("Switches to Auto, so you'll still approve each change.")}</span>
          </div>
        )}
        {error && <div className="error-box">{error}</div>}
        {awayFromEnd && (
          <button className="jump-latest" onClick={toEnd} title={t("Back to the newest text")}>
            ↓ {streaming ? t("New text") : t("Latest")}
          </button>
        )}
      </div>

      <div className="composer-wrap">
        {voice && features.has("voice_chat") && <VoicePanel chatId={chat.id} onHeard={heard} onEnd={() => setVoice(false)} toast={toast} />}
        <div className="composer" onDragOver={(e) => e.dataTransfer.types.includes("Files") && e.preventDefault()} onDrop={onDrop}>
          {queue.length > 0 && (
            <div className="queue-chips" role="group" aria-label={t("Queued messages")}>
              <span className="muted small">{replying ? t("Sends when the reply is done:") : t("Waiting to send:")}</span>
              {queue.map((q) => (
                <span key={q.id} className="queue-chip" title={q.text}>
                  <span className="ellipsis">{q.text || t("(a picture)")}</span>
                  <button className="link" aria-label={t("Remove")} onClick={() => api.chatQueueRemove(chat.id, q.id).then(setQueue)}>
                    ×
                  </button>
                </span>
              ))}
              {!replying && (
                <button className="btn small" onClick={() => api.chatQueueSend(chat.id).catch((e) => toast(errorText(e), "error"))}>
                  {t("Send now")}
                </button>
              )}
            </div>
          )}
          {attached.length > 0 && (
            <div className="attach-chips">
              {attached.map((a) => (
                <AttachChip key={a.id} item={a} onRemove={() => setAttached((all) => all.filter((x) => x.id !== a.id))} />
              ))}
            </div>
          )}
          <SmartInput
            ref={inputRef}
            value={input}
            onValueChange={setInput}
            complete={features.has("autocomplete")}
            correct={features.has("autocorrect")}
            suggestion={nextUp}
            onSuggestionGone={() => setNextUp(null)}
            onKeyDown={onKey}
            onPaste={onPaste}
            commands={slashCommands}
            placeholder={
              replying
                ? t("Add to what it's working on…")
                : chat.mode === "plan"
                  ? t("Describe what you want; it will propose a plan first…")
                  : t("Message {name}…", { name: modelName ?? APP_NAME })
            }
            rows={Math.min(8, Math.max(1, input.split("\n").length))}
            aria-label={t("Message")}
          />
          <div className="composer-bar">
            {canTools && features.has("files") && <FolderMenu toast={toast} chatId={chat.id} />}
            <button className="globe" onClick={attachFile} title={canSee ? t("Attach a picture (or paste one) or a song") : t("Attach a picture or a song. This model can't see pictures; Qwen3 VL and Gemma 3 can.")}>
              <PaperclipIcon />
            </button>
            <button
              className={`globe ${webOn ? "on" : ""}`}
              onClick={onToggleWeb}
              disabled={connectivity !== "offline"}
              aria-pressed={webOn}
              title={
                connectivity !== "offline"
                  ? t("Web is on for every chat. Change it in the status bar.")
                  : webOn
                    ? t("Web is on for this chat (Ctrl+Shift+W)")
                    : t("Turn on web for this chat (Ctrl+Shift+W)")
              }
            >
              <GlobeIcon /> {webOn ? t("Web on") : t("Web off")}
            </button>
            <button
              className="globe"
              onClick={() => {
                const asked = input.trim() || [...messages].reverse().find((m) => m.role === "user")?.content.trim() || "";
                if (asked) openInViewer(aiModeUrl(asked));
                else toast(t("Type a question first."), "info");
              }}
              title={t("Opens Google's AI Mode with your message in the built-in browser. Its answer stays on Google's page.")}
            >
              {t("Ask Google")}
            </button>
            <select value={cloud?.id ?? modelId ?? ""} onChange={(e) => switchModel(e.target.value)} aria-label={t("Model for this chat")} disabled={busy}>
              <optgroup label={t("On this PC")}>
                {installed.map((m) => (
                  <option key={m.id} value={m.id}>
                    {m.name}
                  </option>
                ))}
              </optgroup>
              {taught.length > 0 && (
                <optgroup label={t("Taught on this PC")}>
                  {taught.map((a) => (
                    <option key={a.id} value={`adapter:${a.id}`}>
                      {a.base_name} + {a.name}
                    </option>
                  ))}
                </optgroup>
              )}
              {steered.some((d) => installed.some((m) => m.id === d.model_id)) && (
                <optgroup label={t("Steered in the Lens")}>
                  {steered
                    .filter((d) => installed.some((m) => m.id === d.model_id))
                    .map((d) => (
                      <option key={d.id} value={`steer:${d.id}`}>
                        {steerLabel(d)}
                      </option>
                    ))}
                </optgroup>
              )}
              {compute.models.length > 0 && (
                <optgroup label={t("On {pc} (your compute PC)", { pc: compute.pc })}>
                  {compute.models.map((m) => (
                    <option key={m.id} value={m.id}>
                      {m.name}
                    </option>
                  ))}
                </optgroup>
              )}
              <optgroup label={connectivity === "cloud" ? t("Cloud") : t("Cloud (needs the Cloud level)")}>
                {cloudChoices.map((c) => (
                  <option key={c.id} value={c.id} disabled={connectivity !== "cloud"}>
                    {c.name} · {c.provider}
                    {priceText(c.price_in, c.price_out) && ` · ${priceText(c.price_in, c.price_out)}`}
                  </option>
                ))}
                {!hasGemini && <option value="google:setup">{t("Google (Gemini): set it up…")}</option>}
                {cloudId && !cloud && <option value={cloudId} disabled>{t("(removed model)")}</option>}
              </optgroup>
            </select>
            {cloudId && <span className="badge cloud-badge" title={cloud ? t("Runs on {provider}'s servers", { provider: cloud.provider }) : t("A cloud model")}>{t("Cloud")}</span>}
            <CloudCost chatId={chat.id} cloud={cloud} />
            <span className="spacer" />
            {tps !== null && !busy && <span className="muted small">{t("about {n} words a second", { n: words(tps) })}</span>}
            {features.has("dictation") && <MicButton onText={dictate} toast={toast} disabled={voice} />}
            {features.has("voice_chat") && (
              <button
                type="button"
                className={`voice-btn ${voice ? "on" : ""}`}
                onClick={() => setVoice((v) => !v)}
                aria-pressed={voice}
                title={voice ? t("End voice chat") : t("Voice chat: talk, and hear the answers")}
                aria-label={voice ? t("End voice chat") : t("Start voice chat")}
              >
                <VoiceIcon />
              </button>
            )}
            {replying && (input.trim() || attached.length > 0) && (
              <>
                <button className="btn small" onClick={() => addWhileReplying(false)} title={t("It reads this at its next step (Enter)")}>
                  {t("Add now")}
                </button>
                <button className="btn small" onClick={() => addWhileReplying(true)} title={t("Sends this when the reply is done (Ctrl+Enter)")}>
                  {t("Queue")}
                </button>
              </>
            )}
            <ContextMeter info={context} fallbackCtx={model?.fit.ctx ?? null} modelId={modelId ?? null} canCompact={canCompact} onCompact={compactNow} />
            {busy ? (
              <button className="btn" onClick={() => api.stop(chat.id)}>
                {t("Stop")}
              </button>
            ) : (
              <button className="btn primary" onClick={send} disabled={!input.trim() && attached.length === 0}>
                {t("Send")}
              </button>
            )}
          </div>
        </div>
        {attached.length > 0 && model && !model.vision && !cloud && (
          <p className="small warn-text center">{t("{name} can't see pictures. Pick Qwen3 VL or Gemma 3 from the model menu (install them from Models).", { name: model.name })}</p>
        )}
        {model && model.tools && !cloud && features.has("files") && model.params_b > 0 && model.params_b < 3 && (
          <p className="muted small center">{t("{name} is very small: it often picks the wrong tool or explains instead of doing. For building things, pick a model with 7B or more from the menu.", { name: model.name })}</p>
        )}
        {model && !model.tools && !cloud && (
          <p className="muted small center">{t("{name} can chat but can't use files or commands. Qwen3 models can.", { name: model.name })}</p>
        )}
        {replying && <p className="muted small center">{t("While it works, Enter adds your message to what it's doing; Ctrl+Enter sends it when it's done.")}</p>}
        {webOn && <p className="muted small center">{t("Web access is on for this chat: it can search and read pages. Your messages and the AI stay on this PC.")}</p>}
        {!webOn && searchOn && <p className="muted small center">{t("Web search is on for every chat (Features): it can search and read pages. Your messages and the AI stay on this PC.")}</p>}
      </div>

      {previewShown && previewTarget && (
        <PreviewPanel chatId={chat.id} target={previewTarget} stamp={previewStamp} onWidth={setPreviewWidth} onClose={() => setShowPreview(false)} toast={toast} />
      )}
      {browser && onBrowser && (
        <BrowserPanel
          chatId={chat.id}
          start={browser}
          onWidth={setPreviewWidth}
          onMinimize={() => {
            setHasBrowser(true);
            onBrowser(false);
          }}
          onClose={() => {
            api.viewerClose(chat.id).catch(() => {});
            setHasBrowser(false);
            onBrowser(false);
          }}
          toast={toast}
        />
      )}
      {showFiles && <ChatFilesPanel chatId={chat.id} messages={messages} onOpenPicture={setLightbox} onClose={() => setShowFiles(false)} toast={toast} />}
      {lightbox && <Lightbox item={lightbox} toast={toast} onClose={() => setLightbox(null)} />}
      {confirmBypass && (
        <Modal title={t("Switch to Bypass mode?")} onClose={() => setConfirmBypass(false)}>
          <p>{t("In Bypass mode the assistant changes files and runs commands in your shared folders")} <strong>{t("without asking")}</strong>.</p>
          <ul>
            <li>{t("File changes can still be undone after each reply.")}</li>
            <li>{t("Commands can't be undone, so their effects stay.")}</li>
          </ul>
          <div className="modal-actions">
            <button className="btn" onClick={() => setConfirmBypass(false)}>{t("Stay in Auto")}</button>
            <button
              className="btn danger-fill"
              onClick={async () => {
                setConfirmBypass(false);
                await api.setChatMode(chat.id, "bypass");
                onChanged();
              }}
            >
              {t("Use Bypass")}
            </button>
          </div>
        </Modal>
      )}
      {panel === "memories" && <ChatMemories chat={chat} onClose={() => setPanel(null)} toast={toast} />}
      {panel === "links" && <LinkedChats chat={chat} onOpenChat={onOpenChat} onClose={() => setPanel(null)} toast={toast} />}
      {panel === "notes" && <ChatNotes chat={chat} onClose={() => setPanel(null)} toast={toast} />}
      {googleSetup && (
        <GoogleSetup
          toast={toast}
          onClose={() => setGoogleSetup(false)}
          onDone={(ref) => {
            setGoogleSetup(false);
            api.cloudChoices().then(setCloudChoices).catch(() => {});
            if (connectivity === "cloud") switchModel(ref);
            else toast(t("Gemini is set up. It answers at the Cloud level: change the level in the status bar to use it."), "info");
          }}
        />
      )}
      {exporting && (
        <Suspense fallback={null}>
          <ExportChat chat={chat} messages={messages} onClose={() => setExporting(false)} toast={toast} />
        </Suspense>
      )}
      {managingPrompts && <SavedPromptsManager prompts={prompts} onChange={setPrompts} onClose={() => setManagingPrompts(false)} toast={toast} />}
      {renaming && (
        <RenameDialog
          title={chat.title}
          onClose={() => setRenaming(false)}
          onSave={async (name) => {
            try {
              await api.renameChat(chat.id, name);
              setRenaming(false);
              onChanged();
            } catch (e) {
              toast(errorText(e), "error");
            }
          }}
        />
      )}
      {confirmDelete && (
        <Modal title={t("Delete this chat?")} onClose={() => setConfirmDelete(false)}>
          <p>{t("“{title}” and all its messages will be permanently deleted from this PC.", { title: chat.title })}</p>
          <div className="modal-actions">
            <button className="btn" onClick={() => setConfirmDelete(false)}>{t("Cancel")}</button>
            <button
              className="btn danger-fill"
              onClick={async () => {
                setConfirmDelete(false);
                try {
                  await api.deleteChat(chat.id);
                  onDeleted();
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

/** Messages as the reader sees them: tool results tucked into their calls,
 *  and an Undo button at the end of each turn that changed files. */
function renderItems(
  messages: Message[],
  undoable: Set<string>,
  busy: boolean,
  runningCalls: Set<string>,
  undo: (turnId: string) => void,
  helperSteps: Record<string, string[]>,
  openChat: (id: string) => void,
  toast?: PushToast,
  onEnableWeb?: (() => void) | null,
  openPicture?: (i: MediaItem) => void,
  readUpTo = 0,
  openPreview?: (p: PreviewTarget) => void,
  /** For saving pictures made in the chat (always on; `toast` above is read-aloud's). */
  saveToast?: PushToast,
  turns?: TurnActions,
): ReactNode[] {
  const results = new Map<string, Message>();
  // Where versions start (their first message), or, for one with nothing
  // in it yet, the message before.
  const startsAt = new Map<string, Versions>();
  const after = new Map<string, Versions>();
  for (const v of turns?.versions ?? []) (v.first ? startsAt : after).set(v.first ?? v.anchor, v);
  const switcher = (v: Versions) => (
    <VersionSwitch key={`v-${v.anchor}-${v.first}`} v={v} disabled={busy} onSwitch={(to) => turns?.switchTo(v.anchor, to)} />
  );
  // The reply that ends each turn gets "another reply".
  const lastReply = new Map<string, string>();
  {
    let from: string | null = null;
    for (const m of messages) {
      if (m.role === "user" && !m.meta?.added) from = !m.meta?.from_person && !m.meta?.from_chat && !m.id.startsWith("pending-") ? m.id : null;
      else if (m.role === "assistant" && from && m.content && !m.meta?.compacted) lastReply.set(from, m.id);
    }
  }
  const againFor = new Map([...lastReply].map(([user, reply]) => [reply, user]));
  const userTools = (m: Message) => {
    if (!turns || m.meta?.added || m.meta?.from_person || m.meta?.from_chat || m.id.startsWith("pending-")) return {};
    return {
      editing: turns.editing === m.id,
      onEdit: busy ? undefined : () => turns.setEditing(m.id),
      onEditDone: (text: string | null) => (text === null ? turns.setEditing(null) : turns.edit(m.id, text)),
    };
  };
  const out: ReactNode[] = [];
  if (after.has("")) out.push(switcher(after.get("")!));
  // "Sent" or "Read" under the newest message (and ones added mid-reply):
  // read once the model started answering after it.
  const lastUser = messages.map((m) => m.role === "user" && !m.meta?.from_person).lastIndexOf(true);
  const receipt = (m: Message, i: number): "sent" | "read" | undefined => {
    if (m.meta?.from_person || (i !== lastUser && !m.meta?.added)) return undefined;
    const answered = messages.slice(i + 1).some((x) => x.role === "assistant" || x.role === "tool");
    return answered || (readUpTo > 0 && m.created_at <= readUpTo) ? "read" : "sent";
  };
  for (const m of messages) if (m.role === "tool" && m.tool_call_id) results.set(m.tool_call_id, m);

  let turnId: string | null = null;
  const closeTurn = (key: string, isLastTurn: boolean) => {
    if (turnId && undoable.has(turnId) && !(busy && isLastTurn)) {
      const id = turnId;
      out.push(
        <div key={`undo-${key}`} className="undo-row">
          <button className="btn ghost small" onClick={() => undo(id)}>{t("↶ Undo file changes from this reply")}</button>
        </div>,
      );
    }
  };
  messages.forEach((m, i) => {
    if (startsAt.has(m.id)) out.push(switcher(startsAt.get(m.id)!));
    if (m.role === "user" && m.meta?.added) {
      // Added mid-reply: still the same turn (and the same Undo).
      out.push(<MessageView key={m.id} m={m} onPicture={openPicture} onOpenChat={openChat} receipt={receipt(m, i)} />);
    } else if (m.role === "user") {
      closeTurn(m.id, false);
      turnId = m.id;
      out.push(<MessageView key={m.id} m={m} onPicture={openPicture} onOpenChat={openChat} receipt={receipt(m, i)} {...userTools(m)} />);
    } else if (m.role === "assistant" && m.meta?.compacted) {
      out.push(<CompactedNote key={m.id} m={m} />);
    } else if (m.role === "assistant") {
      const again = againFor.get(m.id);
      out.push(
        <Fragment key={m.id}>
          {(m.content || m.thinking) && <MessageView m={m} toast={toast} onAgain={again && turns && !busy ? () => turns.again(again) : undefined} />}
          {m.meta?.handoff_to && (
            <div className="undo-row">
              <button className="btn small" onClick={() => openChat(m.meta!.handoff_to!)}>{t("Open the continued chat →")}</button>
            </div>
          )}
          {m.tool_calls?.map((c) => {
            const r = results.get(c.id);
            if (c.name === "request_web" && r?.meta?.web_request) {
              return <WebRequestCard key={c.id} title={r.meta.title} onEnable={onEnableWeb ?? null} />;
            }
            return (
              <Fragment key={c.id}>
                <ToolCard call={c} meta={r?.meta} output={r?.content} running={runningCalls.has(c.id)} liveSteps={helperSteps[c.id]} onOpenChat={openChat} onPreview={openPreview} toast={saveToast} />
                {r?.meta?.media && r.meta.media.length > 0 && <MediaInline items={r.meta.media} onOpen={openPicture} toast={saveToast} />}
              </Fragment>
            );
          })}
        </Fragment>,
      );
    }
    if (after.has(m.id)) out.push(switcher(after.get(m.id)!));
    if (i === messages.length - 1) closeTurn("end", true);
  });
  return out;
}

/** What a chat's messages can do about asking again (`renderItems`). */
interface TurnActions {
  versions: Versions[];
  editing: string | null;
  setEditing: (id: string | null) => void;
  again: (userMessageId: string) => void;
  edit: (userMessageId: string, text: string) => void;
  switchTo: (anchor: string, to: number) => void;
}

/** "‹ 2 of 3 ›": which version of the chat from here on is shown. */
function VersionSwitch({ v, disabled, onSwitch }: { v: Versions; disabled: boolean; onSwitch: (to: number) => void }) {
  return (
    <div className="version-switch" role="group" aria-label={t("Versions from here on")}>
      <button className="icon-btn" disabled={disabled || v.index === 0} onClick={() => onSwitch(v.index - 1)} title={t("Earlier version")} aria-label={t("Earlier version")}>
        <BackIcon />
      </button>
      <span className="small muted">{t("Version {n} of {count}", { n: v.index + 1, count: v.count })}</span>
      <button className="icon-btn" disabled={disabled || v.index >= v.count - 1} onClick={() => onSwitch(v.index + 1)} title={t("Later version")} aria-label={t("Later version")}>
        <ForwardIcon />
      </button>
    </div>
  );
}

/** A message of the person's being changed, in place. */
function EditBox({ text, onDone }: { text: string; onDone: (text: string | null) => void }) {
  const [value, setValue] = useState(text);
  const send = () => value.trim() && onDone(value.trim());
  return (
    <div className="edit-box">
      <textarea
        className="input"
        value={value}
        onChange={(e) => setValue(e.target.value)}
        autoFocus
        rows={Math.min(12, Math.max(2, value.split("\n").length))}
        aria-label={t("Your message")}
        onKeyDown={(e) => {
          if (e.key === "Enter" && !e.shiftKey) {
            e.preventDefault();
            send();
          } else if (e.key === "Escape") onDone(null);
        }}
      />
      <div className="edit-box-actions">
        <span className="small muted">{t("The old message and its reply are kept: switch back with the arrows.")}</span>
        <button className="btn small" onClick={() => onDone(null)}>{t("Cancel")}</button>
        <button className="btn primary small" disabled={!value.trim() || value.trim() === text.trim()} onClick={send}>{t("Send")}</button>
      </div>
    </div>
  );
}

/** Where a chat was compacted: the model works from this summary instead
 * of the messages above it (they're still here to read). */
function CompactedNote({ m }: { m: Message }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="compacted-note">
      <div className="compacted-line">
        <span>
          {m.meta?.auto ? t("This chat was getting full, so it compacted itself.") : t("Compacted.")}{" "}
          {t("The model now works from a summary of the messages above.")}
        </span>
        <button className="link small" onClick={() => setOpen((o) => !o)}>
          {open ? t("Hide the summary") : t("Show the summary")}
        </button>
      </div>
      {open && (
        <div className="compacted-summary">
          <Markdown text={m.content} />
        </div>
      )}
    </div>
  );
}

// Thought processes the person folded. The rest stay open: the live reply
// is drawn again as the saved one, and folding by itself (or cutting the
// text to a scrolling box) looked like the model deleting what it wrote.
const thoughtFolded = new Set<string>();

function MessageView({
  m,
  live,
  toast,
  onPicture,
  onOpenChat,
  receipt,
  waiting,
  editing,
  onEdit,
  onEditDone,
  onAgain,
}: {
  m: Message;
  live?: Status;
  toast?: PushToast;
  onPicture?: (i: MediaItem) => void;
  onOpenChat?: (id: string) => void;
  receipt?: "sent" | "read";
  waiting?: boolean;
  editing?: boolean;
  onEdit?: () => void;
  onEditDone?: (text: string | null) => void;
  onAgain?: () => void;
}) {
  const thinkingNow = live === "thinking" && !m.content;
  const summary = m.role === "assistant" ? m.meta?.thought_summary : undefined;
  return (
    <div className={`msg ${m.role}${waiting ? " waiting" : ""}`}>
      {m.role === "user" && m.meta?.images && m.meta.images.length > 0 && (
        <div className="msg-images">
          {m.meta.images.map((id) => (
            <UserImage key={id} id={id} onOpen={onPicture} />
          ))}
        </div>
      )}
      {m.thinking && (
        <details className="thinking" open={!thoughtFolded.has(m.id)} onToggle={(e) => (e.currentTarget.open ? thoughtFolded.delete(m.id) : thoughtFolded.add(m.id))}>
          <summary>{thinkingNow ? t("Thinking…") : t("Thought process")}</summary>
          <div className="thinking-text">{m.thinking}</div>
        </details>
      )}
      {summary && (
        <div className="thought-summary">
          <div className="thought-summary-head">{t("In short")}</div>
          <Markdown text={summary} />
        </div>
      )}
      {m.role === "assistant" && m.content && (m.thinking || summary) && <div className="answer-head">{t("Answer")}</div>}
      {m.role === "user" ? (
        editing && onEditDone ? <EditBox text={m.content} onDone={onEditDone} /> : <div className="bubble">{m.content}</div>
      ) : m.content ? (
        <Markdown text={m.content} />
      ) : null}
      {waiting && (
        <span className="added-note" role="status" title={t("It reads this at its next step, or right after the current reply")}>
          {t("Sent while it worked · waiting for the model to read it")}
        </span>
      )}
      {m.role === "user" && (m.meta?.added || receipt) && (
        <span className={`added-note ${receipt === "read" ? "read" : ""}`} title={receipt === "read" ? t("The model has read this") : t("The model hasn't read this yet")}>
          {[m.meta?.added ? t("Added while it worked") : null, receipt === "read" ? "✓✓ " + t("Read") : receipt === "sent" ? "✓ " + t("Sent") : null].filter(Boolean).join(" · ")}
        </span>
      )}
      {m.meta?.from_person && (
        <span className="added-note">
          {m.role === "user" ? t("From {name}", { name: m.meta.from_name ?? "" }) : t("{name}'s assistant", { name: m.meta.from_name ?? "" }) + (m.meta.model ? " · " + m.meta.model : "")}
        </span>
      )}
      {m.role === "user" && m.meta?.from_chat && (
        <span className="added-note">
          {t("From the assistant in")}{" "}
          <button className="link" onClick={() => onOpenChat?.(m.meta!.from_chat!)}>
            “{m.meta.from_title ?? t("another chat")}”
          </button>
        </span>
      )}
      {live && !m.content && !m.thinking && <span className="typing" aria-label={t("Writing")} />}
      {m.content && !live && !editing && (m.role === "assistant" || m.role === "user") && (
        <div className="msg-tools">
          <CopyButton text={m.content} />
          {onEdit && (
            <button className="icon-btn speak-btn" onClick={onEdit} title={t("Edit and send again")} aria-label={t("Edit and send again")}>
              <EditIcon />
            </button>
          )}
          {onAgain && (
            <button className="icon-btn speak-btn" onClick={onAgain} title={t("Write another reply")} aria-label={t("Write another reply")}>
              <ReloadIcon />
            </button>
          )}
          {toast && m.role === "assistant" && <SpeakButton text={m.content} toast={toast} />}
        </div>
      )}
    </div>
  );
}

/** A picture the user attached, shown above their message. */
function UserImage({ id, onOpen }: { id: string; onOpen?: (i: MediaItem) => void }) {
  // A song attached to work on: heard here, not shown as a picture.
  const [song, setSong] = useState<MediaItem | null>(null);
  useEffect(() => {
    let live = true;
    api.mediaGet(id).then((i) => live && i.kind === "audio" && setSong(i)).catch(() => {});
    return () => {
      live = false;
    };
  }, [id]);
  const src = useMediaUrl(song ? null : { id, mime: "image/jpeg", kind: "image" }, true);
  if (song) return <UserSong item={song} />;
  if (!src) return null;
  // A button, so the keyboard reaches it too.
  return (
    <button className="msg-image-btn" onClick={() => api.mediaGet(id).then((i) => onOpen?.(i))} title={t("Open")}>
      <img src={src} alt={t("Attached picture")} />
    </button>
  );
}

/** A song the user attached, with a player. */
function UserSong({ item }: { item: MediaItem }) {
  const src = useMediaUrl(item);
  return (
    <div className="user-song">
      <span className="small">{item.prompt || t("A song")}</span>
      {src && <audio controls controlsList="nodownload" src={src} />}
    </div>
  );
}

function AttachChip({ item, onRemove }: { item: MediaItem; onRemove: () => void }) {
  const src = useMediaUrl(item.kind === "image" ? item : null, true);
  return (
    <span className={`attach-chip ${item.kind === "audio" ? "song" : ""}`}>
      {item.kind === "audio" ? <span className="small">{item.prompt || t("A song")}</span> : src ? <img src={src} alt={t("Attached picture")} /> : <span className="media-inline-ph">…</span>}
      <button onClick={onRemove} aria-label={item.kind === "audio" ? t("Remove song") : t("Remove picture")}>✕</button>
    </span>
  );
}

function WebRequestCard({ title, onEnable }: { title: string; onEnable: (() => void) | null }) {
  const reason = title.replace(/^Needs web access: /, "");
  return (
    <div className="web-request" role="note">
      <GlobeIcon className="web-request-icon" />
      <div>
        <strong>{t("This needs web access")}</strong>
        <span className="small muted block">{t("To look up: {reason}. Only the search and the pages it opens go online; the AI stays on this PC.", { reason })}</span>
      </div>
      {onEnable ? (
        <button className="btn primary small" onClick={onEnable}>{t("Turn on web for this chat")}</button>
      ) : (
        <span className="small muted">{t("Web is on")}</span>
      )}
    </div>
  );
}

function RenameDialog({ title, onClose, onSave }: { title: string; onClose: () => void; onSave: (t: string) => void }) {
  const [value, setValue] = useState(title);
  return (
    <Modal title={t("Rename chat")} onClose={onClose}>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          onSave(value);
        }}
      >
        <input className="input" value={value} onChange={(e) => setValue(e.target.value)} autoFocus maxLength={80} aria-label={t("Chat name")} />
        <div className="modal-actions">
          <button type="button" className="btn" onClick={onClose}>{t("Cancel")}</button>
          <button type="submit" className="btn primary" disabled={!value.trim()}>{t("Save")}</button>
        </div>
      </form>
    </Modal>
  );
}

/** A busy cloud provider's next try, counted down. */
function RetryLine({ provider, at, attempt, of }: { provider: string; at: number; attempt: number; of: number }) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 250);
    return () => clearInterval(timer);
  }, []);
  const seconds = Math.max(0, Math.ceil((at - now) / 1000));
  return (
    <p className="muted small pad">
      <span className="spinner small" aria-hidden />
      {t("{provider} is busy. Trying again in {seconds} s ({attempt} of {of})", { provider, seconds, attempt, of })}
    </p>
  );
}

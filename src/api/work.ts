// SPDX-License-Identifier: AGPL-3.0-only
// Types for projects, connectors, tools, voice, meetings, notes, mail and the gallery.
import { type License } from "./system";
import { type Composition, type MadeStats, type MusicSpec, type SongMap, type SongReview } from "./media";

export interface Project {
  id: string;
  name: string;
  instructions: string;
  folders: string[];
  created_at: number;
  updated_at: number;
}

export type Cadence =
  | { kind: "once"; at: number }
  | { kind: "hourly"; minute: number }
  | { kind: "daily"; time: string }
  | { kind: "weekdays"; time: string }
  | { kind: "weekly"; weekday: number; time: string }
  /** The chosen days of the week (0 = Monday), at each of the times ("08:00"). */
  | { kind: "days"; weekdays: number[]; times: string[] }
  /** The chosen dates ("2026-10-24"), at each of the times. */
  | { kind: "dates"; dates: string[]; times: string[] }
  /** New mail, from someone (part of a name or address) and with words in the subject, when given. */
  | { kind: "on_mail"; from: string; subject: string }
  /** A calendar event about to start, this many minutes before; only titles with these words, when given. */
  | { kind: "on_event"; minutes: number; title: string }
  /** A file appearing in a folder; only names with this in them, when given. */
  | { kind: "on_file"; folder: string; name: string }
  /** Each time SulcusAI starts. */
  | { kind: "on_start" }
  /** Another task finishing without an error (a workflow step). */
  | { kind: "after_task"; task: string }
  /** A meeting's notes being written; only titles with these words, when given. */
  | { kind: "on_meeting"; title: string }
  /** Tasks by email: mail from your own address, to it, with this secret phrase; its text is the task. */
  | { kind: "by_mail"; phrase: string }
  /** A web page's text changing, looked at every `minutes`. */
  | { kind: "on_page"; url: string; minutes: number };

export interface Schedule {
  id: string;
  name: string;
  prompt: string;
  cadence: Cadence;
  model_id: string | null;
  project_id: string | null;
  allow_changes: boolean;
  /** What it may do by itself, approved when it was set up (message_person, email_send, …). */
  actions: string[];
  enabled: boolean;
  last_run: number | null;
  next_run: number | null;
  last_chat: string | null;
  /** When a trigger was set up (only what happens after counts). */
  armed?: number | null;
  /** What a trigger already ran for. */
  handled?: string[];
  /** Why the last run failed (cleared when one works). */
  last_error?: string | null;
}

export interface ScheduleView extends Schedule {
  when: string;
}

export type ToolMode = "allow" | "ask" | "off";

export interface McpTool {
  name: string;
  description: string;
  inputSchema: unknown;
  read_only: boolean;
}

export interface Connector {
  id: string;
  name: string;
  spec: { command: string; args: string[]; env: Record<string, string>; cwd: string | null; url?: string; headers?: Record<string, string> };
  enabled: boolean;
  plugin: string | null;
  tools: McpTool[];
  tool_modes: Record<string, ToolMode>;
  /** Signed in to a connector at an address (who, and where; never the tokens). */
  auth?: { account: string; issuer: string } | null;
}

export interface Plugin {
  id: string;
  name: string;
  version: string;
  description: string;
  enabled: boolean;
  dir: string;
  skills: { name: string; description: string }[];
  connectors: string[];
  /** Commands it runs at points of a reply, agreed to when it was installed. */
  checks: { event: "before_tool" | "after_tool" | "message_sent" | "reply_done"; tools: string[]; command: string; timeout: number }[];
}

export interface PluginPreview {
  name: string;
  version: string;
  description: string;
  skills: string[];
  programs: string[];
  /** Commands its checks would run on this PC, "when: command". */
  checks: string[];
}

export interface Memory {
  id: string;
  content: string;
  project_id: string | null;
  /** The chat it was saved from (null: added on the Memory page). */
  source_chat: string | null;
  created_at: number;
  updated_at: number;
  /** One of the memory groups (memory.rs CATEGORIES): chosen by the person, else from its words. */
  category: string;
  /** Health, beliefs, politics, sexuality, money or legal matters. */
  sensitive: boolean;
}


export interface QuickSettings {
  hotkey: string;
  /** As people write it ("Ctrl+Alt+Space"). */
  shown: string;
  /** The model its chats use (null: the default model). */
  model: string | null;
  /** Dictating into any app with its own hotkey. */
  dictate_anywhere?: boolean;
  dictate_hotkey?: string;
}

/** Text selected in the app in front when Quick ask opened. */
export interface QuickSelection {
  text: string;
  /** The answer can be pasted in its place (not in a terminal). */
  editable: boolean;
  app: string;
}

export interface ToolCall {
  id: string;
  name: string;
  arguments: string;
}

/** A site served on this PC for the Preview panel. */
export interface SitePreview {
  id: string;
  url: string;
  root: string;
  web: boolean;
}

/** A chart the assistant drew (run_code's chart(), market_quote's price history). */
export interface ChartSpec {
  type: "bar" | "line" | "area" | "pie" | "scatter";
  title: string;
  x_label: string;
  y_label: string;
  labels: string[];
  series: { name: string; data?: (number | null)[]; points?: [number, number][] }[];
}

/** A reply's token use as saved with it: from the provider, or estimated. */
export interface Usage {
  in: number | null;
  out: number;
  thinking: number | null;
  cache_read: number | null;
  source: "provider" | "estimate";
  /** Model steps summed into this figure (a reply that called tools). */
  steps?: number;
  /** The model that wrote it (older replies have none). */
  model?: string;
}

export interface ToolMeta {
  usage?: Usage;
  /** Made in low token mode. */
  low_tokens?: boolean;
  title: string;
  status: "ok" | "error" | "denied";
  kind: "diff" | "command" | "text";
  detail?: string | null;
  /** Charts to show with the result. */
  charts?: ChartSpec[];
  /** Where it came from: web pages, or library files ("doc:<id>"). */
  sources?: { title: string; url: string; snippet?: string }[];
  tool?: string;
  /** On the note in a chat that continued elsewhere. */
  handoff_to?: string;
  /** On the first message of a continued chat. */
  handoff_from?: string;
  /** A summary left by compacting; the model works from it instead of the messages before it. */
  compacted?: boolean;
  /** On a reply's last message: all its thinking in a few short points (added after it answered). */
  thought_summary?: string;
  /** A user message added while the reply was being written. */
  added?: boolean;
  /** In a chat this user shares: written by that person (or their model). */
  from_person?: string;
  from_name?: string;
  model?: string;
  /** A message from the assistant of another chat (it asked this one). */
  from_chat?: string;
  from_title?: string;
  /** What the answer may get wrong because a tool result was partial. */
  claim_check?: string[];
  /** A step about another chat: open it from the step. */
  open_chat?: string;
  open_title?: string;
  /** How many messages the summary covers, and whether the chat compacted itself. */
  messages?: number;
  auto?: boolean;
  /** A document the assistant made (it can be opened). */
  file?: string;
  /** A server the assistant started on this PC: its port, for Preview. */
  preview_port?: number;
  /** A site the assistant previewed: where it is served, its folder and page, and what its check found. */
  preview_url?: string;
  preview_site?: string;
  preview_root?: string;
  preview_start?: string;
  preview_problems?: number;
  /** The assistant asked the user to turn on web access. */
  web_request?: boolean;
  /** Pictures, clips or sounds the assistant made. */
  media?: MediaItem[];
  /** User messages: attached pictures (gallery ids). */
  images?: string[];
}

/** A message waiting for a reply to finish. */
export interface QueueItem {
  id: string;
  text: string;
  images: string[];
  at: number;
}

export interface Preview {
  title: string;
  /** "privacy": detail is a ShareReview (JSON) of what a call would send. */
  kind: "diff" | "command" | "text" | "folder" | "key" | "privacy";
  detail: string | null;
  note: string | null;
}

/** Privacy mode: one part of what a call would send off this PC. */
export interface ShareItem {
  /** Where it is in the call ("query", "searches.1"); null when it goes along on its own. */
  key: string | null;
  label: string;
  value: string;
  level: number;
  editable: boolean;
  removable: boolean;
}

/** Privacy mode: what a call would send, who gets it, what they could do with it, how personal (1-5). */
export interface ShareReview {
  /** "tool" for a tool call; "cloud" for a turn going to an online model (asked once a chat). */
  kind?: string;
  level: number;
  level_name: string;
  to: string;
  items: ShareItem[];
  could: string[];
  found: { level: number; what: string }[];
  note: string | null;
}

/** One thing sent off this PC ("What left this PC"). */
export interface SentEntry {
  /** Its own id, to delete it alone. */
  id: string;
  at: number;
  chat_id: string;
  /** The tool, or "cloud_chat" for a turn sent to an online model. */
  tool: string;
  to: string;
  level: number;
  what: string;
  /** False when the person chose not to send it. */
  sent: boolean;
  /** What the person had last asked in that chat (kept from 0.9.40 on). */
  asked?: string | null;
  /** Everything it sent, each with how personal it was. */
  items?: { label: string; value: string; level: number }[];
  could?: string[];
  found?: { level: number; what: string }[];
  note?: string | null;
}

export interface PrivacyMode {
  on: boolean;
  /** Asks before sending anything at this level or above (1-5). */
  ask_from: number;
}

/** A site the assistant is building needs an API key for a data source. */
export interface SiteKeyAsk {
  platform: string;
  host: string;
  reason: string;
  /** Where to get a key. */
  signup: string | null;
  /** A key for this host is saved already. */
  saved: boolean;
}

/** A saved site key, as Settings lists it (never the key itself). */
export interface SavedSiteKey {
  host: string;
  platform: string;
  chats: number;
}

export interface PendingApproval {
  chat_id: string;
  call_id: string;
  tool: string;
  risk: "read" | "write" | "execute" | "connector" | "programs" | "memory" | "submit";
  preview: Preview;
  /** Only this one call can be allowed (a delete, a send): no "for this chat". */
  once?: boolean;
  /** What "Always allow…" would save, for the person to change first. */
  rule?: { tool: string; pattern: string | null; project: string | null } | null;
}

/** What going back to a message would take away. */
export interface GoBack {
  messages: number;
  changes: number;
}

/** A saved rule: a tool (or `prefix*`), what it acts on, and whether it's allowed, asked about or refused. */
export interface Allowance {
  id: string;
  tool: string;
  pattern: string | null;
  effect: "allow" | "ask" | "deny";
  /** Only in this project's chats; null for every chat. */
  project: string | null;
  created_at: number;
}

/** Whether the assistant may use anything on the Desktop. */
export interface DesktopAccess {
  on: boolean;
  path: string | null;
}

export interface Folder {
  path: string;
  name: string;
  exists: boolean;
  /** Who can use it: this chat only, the chat's project, or every chat. */
  scope: "chat" | "project" | "all";
}

export interface Message {
  id: string;
  chat_id: string;
  role: "user" | "assistant" | "tool";
  content: string;
  thinking: string | null;
  created_at: number;
  tool_calls?: ToolCall[];
  tool_call_id?: string;
  meta?: ToolMeta;
}

/** A chat with every word searched for. */
export interface ChatHit {
  chat_id: string;
  title: string;
  /** Where in it the words are (none when only the name has them). */
  snippet: string | null;
  updated_at: number;
}

/** Other versions of a chat after one point (an edited message, another reply). */
export interface Versions {
  /** The message just before them ("" for the chat's start). */
  anchor: string;
  /** The first message of the version shown, if it has one yet. */
  first: string | null;
  index: number;
  count: number;
}

export interface ContextInfo {
  ctx: number;
  system_tokens: number;
  profile_tokens: number;
  tools_tokens: number;
  history_tokens: number;
  reply_reserve: number;
  dropped_messages: number;
  last_total: number | null;
  /** Worked out from the saved messages, not counted by the engine. */
  estimated?: boolean;
  /** The conversation by kind (tokens). */
  history?: { yours: number; replies: number; tool_results: number; summary: number };
}

export interface EngineStatus {
  model_id: string | null;
  quant: string | null;
  ctx: number | null;
  /** Layers planned for the graphics card. */
  gpu_layers: number | null;
  /** As the engine reported them once loaded (null: not known). */
  layers_on_card: number | null;
  layers_total: number | null;
  device: string | null;
  /** The model used before, still loaded beside it. */
  kept?: string | null;
}

/** Why some or all of the model runs on the processor (placement.rs `reasons`). */
export interface PlaceReason {
  code: "no_card" | "no_driver" | "engine_no_card" | "gpu_layers" | "gpu_limit" | "cool_on_battery" | "cool_mode" | "too_small" | "window";
  /** In English, for when the window has no wording for the code. */
  message: string;
  fix: "gpu_full" | "recheck" | "perf_settings" | "advanced_settings" | null;
  card: string | null;
  maker: string | null;
  number: number | null;
  /** "window": how much the chat keeps in mind, in thousands of tokens. */
  ctx_k?: number | null;
}

export interface WhereItRuns {
  /** null while no model is loaded. */
  place: "card" | "partly" | "processor" | null;
  model_id: string | null;
  layers_on_card: number | null;
  layers_total: number | null;
  /** The engine's own report, not just the plan. */
  reported: boolean;
  device: string | null;
  reasons: PlaceReason[];
}

export interface CardView {
  name: string;
  vendor: string;
  memory: number;
  kind: "card" | "built_in" | "no_driver";
  /** The card models use. */
  main: boolean;
  code: number | null;
}

export interface GpuMenu {
  cards: CardView[];
  /** The card picked by name (null: automatic). */
  chosen: string | null;
  runs: WhereItRuns;
}

export interface AppInfo {
  name: string;
  version: string;
  data_dir: string;
}

export interface SecurityStatus {
  locked: boolean;
  lock_enabled: boolean;
  hello_enabled: boolean;
  hello_available: boolean;
  auto_lock_minutes: number;
  /** Locking lets work in progress carry on. */
  keep_working?: boolean;
}

export interface Action {
  id: number;
  at: number;
  category: "model" | "network" | "privacy" | "security" | "chat" | string;
  summary: string;
}

export interface SpeechFit {
  speed: number;
  needed_bytes: number;
  runnable: boolean;
  disk_ok: boolean;
}

export interface InstalledSpeech {
  model_id: string;
  path: string;
  size: number;
  installed_at: number;
  speed: number | null;
}

export interface SpeechCard {
  id: string;
  name: string;
  publisher: string;
  description: string;
  license: License;
  languages: number;
  size: number;
  fit: SpeechFit;
  installed: InstalledSpeech | null;
  recommended: boolean;
}

export interface SpeechView {
  models: SpeechCard[];
  hidden: number;
  active: string | null;
  live: string | null;
  recommended_live: string | null;
  threads: number;
  installing: string[];
}

export interface VoiceSettings {
  speech_model: string | null;
  live_model: string | null;
  mic: string | null;
  voice_processing: boolean;
  language: string;
  voice: string | null;
  rate: number;
  keep_meeting_audio: boolean;
  /** Names and terms dictation should spell this way. */
  words?: string[];
  /** Phrases that dictate something else. */
  shortcuts?: { say: string; write: string }[];
  /** Fillers and stutters left out of dictated text. */
  tidy_dictation?: boolean;
}

export interface AudioDevice {
  id: string;
  name: string;
  default: boolean;
}

export interface Voice {
  id: string;
  name: string;
  language: string;
  gender: string;
  engine: string;
}

export type Speaker = "you" | "others";

export interface ActionItem {
  task: string;
  owner: string;
  due: string;
  done: boolean;
}

export interface Notes {
  title: string;
  summary: string;
  topics: string[];
  key_points: string[];
  most_important: string[];
  decisions: string[];
  action_items: ActionItem[];
}

export type MeetingStatus = "recording" | "transcribing" | "summarizing" | "done" | "failed";

export interface Meeting {
  id: string;
  started_at: number;
  ended_at: number | null;
  status: MeetingStatus;
  title: string;
  titled_by_user: boolean;
  notes: Notes | null;
  translate_to: string | null;
  has_audio: boolean;
  error: string | null;
  speaker_names: Record<string, string>;
  /** What the user typed during the meeting, merged into the notes. */
  typed?: string;
}

export interface Segment {
  id: number;
  speaker: Speaker;
  voice: number | null;
  start: number;
  end: number;
  text: string;
  translation: string | null;
}

export interface MeetingDetail extends Meeting {
  segments: Segment[];
}

export interface MeetingHit {
  meeting: Meeting;
  lines: string[];
}

export interface VoicePack {
  id: string;
  name: string;
  publisher: string;
  description: string;
  license: License;
  languages: string[];
  styles: { id: string; name: string; gender: string }[];
  size: number;
  installed: boolean;
}

export interface SpeakerModel {
  id: string;
  name: string;
  publisher: string;
  description: string;
  license: License;
  size: number;
  installed: boolean;
  download: number;
}

export type FeatureId =
  | "dictation"
  | "voice_chat"
  | "read_aloud"
  | "meetings"
  | "translate"
  | "notes"
  | "tasks"
  | "web_search"
  | "browser"
  | "email"
  | "calendar"
  | "documents"
  | "quick_ask"
  | "clipboard"
  | "look_ahead"
  | "call_reminder"
  | "browser_control"
  | "programs"
  | "images"
  | "video"
  | "music"
  | "files"
  | "memory"
  | "projects"
  | "scheduled"
  | "connectors"
  | "people"
  | "chat_team"
  | "sessions"
  | "library"
  | "autocomplete"
  | "autocorrect"
  | "next_prompt";

/** Autocomplete's material: the person's own words (with counts) and requests, newest first. */
export interface Vocabulary {
  words: [string, number][];
  prompts: string[];
}

export interface FeatureView {
  id: FeatureId;
  enabled: boolean;
  need: { model_id: string; model_name: string; size: number; met: boolean } | null;
}

export interface NoteBody {
  title: string;
  body: string;
  folder: string;
  tags: string[];
}

export interface Note extends NoteBody {
  id: string;
  pinned: boolean;
  created_at: number;
  updated_at: number;
}

export interface Task {
  id: string;
  title: string;
  notes: string;
  subtasks: { title: string; done: boolean }[];
  source: { kind: string; id: string; label: string } | null;
  due: number | null;
  due_has_time: boolean;
  priority: number;
  remind_at: number | null;
  done_at: number | null;
  created_at: number;
}

export interface BridgeStatus {
  installed: boolean;
  connected: boolean;
  version: string | null;
  extension_dir: string;
  extension_id: string;
  pair_code: string | null;
}

export type MainBrowser = { kind: "builtin" } | { kind: "system" } | { kind: "app"; name: string };

/** Where the web viewer goes, in the window's (CSS) pixels. */
export interface ViewerArea {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** What SulcusAI's own browser starts on and searches with. */
export type BrowserSearch = "duckduckgo" | "google" | "bing" | "brave";

export interface BrowsersView {
  current: MainBrowser;
  installed: { name: string }[];
  search: BrowserSearch;
}

export type MailSecurity = "tls" | "starttls" | "plain";

export interface MailConfig {
  name: string;
  email: string;
  username: string;
  password: string;
  imap_host: string;
  imap_port: number;
  imap_security: MailSecurity;
  smtp_host: string;
  smtp_port: number;
  smtp_security: MailSecurity;
}

export interface MailAccount {
  id: string;
  name: string;
  email: string;
  imap_host: string;
  smtp_host: string;
  synced_at: number | null;
}

export interface MailItem {
  id: string;
  account_id: string;
  folder: string;
  date: number;
  seen: boolean;
  from: string;
  from_name: string;
  to: string[];
  cc: string[];
  subject: string;
  snippet: string;
  body: string;
  attachments: string[];
  partial: boolean;
  /** When the user marked it read with the button (it's under Marked read). */
  marked_at: number | null;
  /** When the user last opened it here. */
  opened_at: number | null;
  /** List-Unsubscribe targets (mailto: and https:). */
  unsubscribe: string[];
  one_click: boolean;
  list_id: string;
  list_name: string;
  /** A calendar invitation in it (METHOD:REQUEST). */
  invite: MailInvite | null;
}

export type PartStat = "ACCEPTED" | "TENTATIVE" | "DECLINED";

/** A calendar invitation in an email. */
export interface MailInvite {
  method: string;
  uid: string;
  sequence: number;
  title: string;
  start: number;
  end: number;
  all_day: boolean;
  location: string;
  notes: string;
  organizer: string;
  organizer_name: string;
  attendees: { email: string; name: string; partstat: string }[];
  recurring: boolean;
  /** What the user answered here. */
  answer: { partstat: PartStat; at: number; sent: boolean; event_id: string | null } | null;
}

export interface InviteOutcome {
  answer: NonNullable<MailInvite["answer"]>;
  /** The event on the calendar: added now, or found there. */
  event: CalEvent | null;
  /** It was on the calendar already. */
  already: boolean;
  /** The answer couldn't be sent (the rest was done). */
  send_error: string;
}

/** What a bulk action on a report group would act on. */
export interface BulkPlan {
  action: "mark_read" | "archive" | "unsubscribe";
  ids: string[];
  lists: { key: string; name: string }[];
  count: number;
  /** More than five: ask once first. */
  ask: boolean;
}

export type MailCategory = "needs reply" | "to-do" | "work" | "personal" | "bills" | "travel" | "updates" | "newsletters" | "promotions" | "social";

/** How to leave a mailing list. */
export interface Unsub {
  key: string;
  method: "one_click" | "email" | "link" | "none";
  /** For "email": where the unsubscribe email goes, and its subject. */
  email_to: string;
  email_subject: string;
  unsubscribed: { at: number; method: string } | null;
}

/** A list or bulk sender in the inbox. */
export interface Subscription extends Unsub {
  account_id: string;
  name: string;
  address: string;
  list_id: string;
  count: number;
  latest: number;
  /** Its mail still in the inbox. */
  inbox_ids: string[];
  /** When one of its emails was last opened here. */
  opened_at: number | null;
  /** Never opened: how many of its latest emails went unopened (else 0). */
  unopened: number;
}

export interface UnsubscribeResult {
  key: string;
  method: "one_click" | "email" | "link";
  at: number;
  /** For "link": the page to open in the browser. */
  url: string;
  /** For "link" after the one-click request failed. */
  problem: string;
  to: string;
  subject: string;
}

/** An event read out of an email, for the calendar form. */
export interface EventDraft {
  title: string;
  /** YYYY-MM-DD */
  date: string;
  /** HH:MM, or "" when not known. */
  start: string;
  end: string;
  all_day: boolean;
  location: string;
  notes: string;
  source: "ai" | "text" | "none";
}

/** One message in the AI's sorting report. */
export interface TriageItem {
  id: string;
  account_id: string;
  from: string;
  from_name: string;
  subject: string;
  snippet: string;
  date: number;
  seen: boolean;
  important: boolean;
  marked_read: boolean;
  reason: string;
  next: "reply" | "calendar" | "pay" | "review" | "read later" | "none";
  category: MailCategory;
  /** Needs a reply from a person, with none sent after a day. */
  missed: boolean;
  unsubscribe: Unsub | null;
  /** Still in the inbox. */
  in_inbox: boolean;
  /** A task to reply to it is open. */
  reminder: boolean;
}

export interface TriageReport {
  run_at: number | null;
  model: string;
  looked_at: number;
  items: TriageItem[];
  /** New mail is sorted as it arrives. */
  auto: boolean;
  /** Missed replies a sort finds get a reminder by themselves. */
  remind_missed: boolean;
}

export interface MailDraft {
  to: string[];
  cc: string[];
  subject: string;
  body: string;
  reply_to: string | null;
}

export interface CalInfo {
  href: string;
  name: string;
  color: string;
}

export interface CalAccount {
  id: string;
  name: string;
  url: string;
  calendars: CalInfo[];
  synced_at: number | null;
}

export interface CalEvent {
  id: string;
  account_id: string | null;
  calendar: string | null;
  start: number;
  end: number;
  all_day: boolean;
  title: string;
  location: string;
  notes: string;
  attendees: string[];
  recurring: boolean;
}

export interface NewEvent {
  title: string;
  start: number;
  end: number;
  all_day: boolean;
  location: string;
  notes: string;
  attendees: string[];
  calendar: string | null;
}

export type PerfMode = "away" | "cool" | "balanced" | "turbo";

export interface PerfSettings {
  mode: PerfMode;
  turbo: { threads: number; gpu_percent: number; ram_gb: number } | null;
  cool_on_battery: boolean;
  heat_guard: boolean;
  adaptive: boolean;
  gpu_limit?: number;
  /** Unload an idle chat model after this many minutes (0 = never); null for the mode's own. */
  unload_after_mins?: number | null;
  /** Free it right after each reply (a few seconds after last use). */
  unload_right_away?: boolean;
  /** Switching models, the one used before stays loaded while the card has room for both. */
  keep_two?: boolean;
  /** Out of the way by itself while a game or any other app fills the screen. */
  away_full_screen?: boolean;
  /** Out of the way by itself on battery. */
  away_on_battery?: boolean;
}

export interface PauseSettings {
  keep_phone: boolean;
  shortcuts: boolean;
  tasks_after_quit: boolean;
}

export interface PauseView {
  paused: boolean;
  settings: PauseSettings;
  hotkey: string;
  next_task: number | null;
}

export interface ZeroProc {
  pid: number;
  name: string;
  cpu: number;
  memory_mb: number;
  written_kb: number;
  card_mb: number;
  connections: number;
}

export interface ZeroView {
  paused: boolean;
  seconds: number;
  procs: ZeroProc[];
  total: ZeroProc;
  still_on: string[];
  at_zero: boolean;
}

export interface PerfLimits {
  mode: PerfMode;
  on_battery: boolean;
  threads: number;
  vram_bytes: number;
  ram_bytes: number;
  low_priority: boolean;
  idle_unload_mins: number;
  max_ctx: number;
  gpu_temp_limit: number;
  cpu_temp_limit: number;
  heat_level: number;
  gpu_share: number;
}

export interface Temps {
  gpu: number | null;
  cpu: number | null;
}

export interface HeatState {
  gpu_level: number;
  cpu_level: number;
  temps: Temps;
}

export interface PerfView {
  settings: PerfSettings;
  ceiling: { threads: number; gpu_percent: number; ram_gb: number; has_gpu: boolean };
  base: PerfLimits;
  limits: PerfLimits;
  heat: HeatState;
  temps: Temps;
  has_battery: boolean;
  cores: number;
  ram_total_gb: number;
  vram_gb: number;
}

export type SearchProvider = "duckduckgo" | "brave" | "searxng";

export interface WebSettings {
  provider: SearchProvider;
  searxng_url: string;
  /** DuckDuckGo only, even when it turns searches away. */
  no_fallback?: boolean;
  has_brave_key?: boolean;
}

export interface Language {
  code: string;
  name: string;
}

export type OAuthProvider = "microsoft" | "google";

export interface OAuthClients {
  microsoft: string;
  google: string;
  google_secret: string;
}

/** The person's own registrations, and which providers work without one. */
export interface LiveReply {
  message_id: string | null;
  content: string;
  thinking: string;
  status: "loading" | "thinking" | "writing" | "working";
}

export interface OAuthClientsView {
  own: OAuthClients;
  built_in_microsoft: boolean;
  built_in_google: boolean;
}

export type MediaKind = "image" | "video" | "music" | "upscale" | "background";

export interface MediaFit {
  runnable: boolean;
  on_gpu: boolean;
  est_secs: number;
  needed_bytes: number;
  disk_ok: boolean;
  reason: string | null;
}

export interface MediaModelCard {
  id: string;
  name: string;
  publisher: string;
  source: string;
  description: string;
  license: License;
  kind: MediaKind;
  can: string[];
  files: { role: string; file: string; size: number }[];
  quality: number;
  /** What it's good at, and less good at, for choosing between models. */
  strengths?: string[];
  weaknesses?: string[];
  defaults: { steps: number; width: number; height: number; fps: number; seconds: number; max_frames?: number };
  /** Installed from Hugging Face: its repository there ("owner/name"). */
  from_hub?: string | null;
  fit: MediaFit;
  installed: boolean;
}

export interface MediaItem {
  id: string;
  kind: "image" | "video" | "audio";
  op: string;
  prompt: string;
  /** A song's name: its hook, or what the person called it. */
  title?: string | null;
  model_id: string | null;
  mime: string;
  width: number;
  height: number;
  seconds: number;
  size: number;
  created_at: number;
  parent: string | null;
  chat_id: string | null;
  seed: number | null;
  lyrics: string | null;
  favorite: boolean;
  /** Marked as not liked (a song's thumbs down). */
  disliked?: boolean;
  hidden: boolean;
  /** Videos: a still frame is saved as the thumbnail. */
  poster: boolean;
  /** What making it took (older items have none). */
  stats: MadeStats | null;
  /** What was asked for that the check didn't find in it. */
  missing?: string[];
  /** Something else to know about it ("the whole scene was redrawn"). */
  note?: string | null;
  /** What the listener heard in a song. */
  review?: SongReview | null;
  /** How a song is built, for the song view. */
  map?: SongMap | null;
  /** A song written as notes in Compose; the file is its demo. */
  composition?: Composition | null;
  /** A new song's settings, to make it again (Reuse these settings). */
  music?: MusicSpec | null;
}

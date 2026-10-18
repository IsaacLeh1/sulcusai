// SPDX-License-Identifier: AGPL-3.0-only
// Typed wrappers around the Rust commands in src-tauri/src/lib.rs.
import { t } from "./i18n";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type Connectivity = "offline" | "web" | "cloud";
export type Placement = "gpu" | "split" | "cpu";

export interface Gpu {
  name: string;
  vendor: string;
  vram_bytes: number;
  integrated: boolean;
}

export interface Hardware {
  os: string;
  cpu_name: string;
  cpu_threads: number;
  cpu_cores: number | null;
  avx2: boolean;
  avx512: boolean;
  ram_total: number;
  ram_available: number;
  gpus: Gpu[];
  models_drive: string;
  disk_free: number;
  disk_total: number;
  on_battery: boolean | null;
}

export interface Budget {
  vram: number;
  ram: number;
  disk: number;
}

export interface License {
  name: string;
  url: string;
  commercial: boolean;
  note?: string | null;
}

export interface Variant {
  quant: string;
  quality: string;
  file: string;
  size: number;
}

export interface VariantFit {
  quant: string;
  placement: Placement | null;
  needed_bytes: number;
  est_tps: number;
  gpu_layers: number;
  disk_ok: boolean;
}

export interface InstalledModel {
  model_id: string;
  quant: string;
  path: string;
  size: number;
  installed_at: number;
  /** Writing speed measured on this PC (tokens a second). */
  tps: number | null;
  /** Reading speed (prompt tokens a second). */
  prompt_tps: number | null;
}

export interface Speed {
  tps: number;
  prompt_tps: number | null;
}

export interface Ratings {
  /** 0-100, least to most capable. */
  overall: number;
  coding: number;
  writing: number;
  research: number;
  agents: number;
  languages: number;
}

export interface TaughtAdapter {
  id: string;
  name: string;
  base: string;
  base_name: string;
  path: string;
  source: string;
  examples: number;
  epochs: number;
  created: number;
  final_loss: number | null;
}

export interface FinetuneJob {
  id: string;
  name: string;
  base_name: string;
  status: "preparing" | "downloading" | "training" | "saving" | "done" | "failed" | "cancelled";
  fraction: number;
  epoch: number;
  epochs: number;
  loss: number | null;
  eta: string | null;
  error: string | null;
}

export interface FinetuneView {
  bases: { id: string; name: string; supported: boolean }[];
  adapters: TaughtAdapter[];
  job: FinetuneJob | null;
  trainer_installed: boolean;
  trainer_size: number;
}

export interface UpdateAvailable {
  version: string;
  notes: string;
  date: string | null;
}

export interface UpdateView {
  current: string;
  auto: boolean;
  /** Installs updates without asking, when nothing is running. */
  install: boolean;
  last_check: number;
  available: UpdateAvailable | null;
}

export interface PrivacySettings {
  enabled: boolean;
  via: "tor" | "proxy";
  /** The user's own proxy (host:port, socks5://… or http://…). */
  proxy: string;
  web: boolean;
  updates: boolean;
  mail: boolean;
  models: boolean;
}

export interface PrivacyView {
  settings: PrivacySettings;
  /** "waiting": Tor connects the first time something goes through it. */
  state: "off" | "waiting" | "starting" | "ready" | "stuck";
  /** Tor's start-up progress, 0-100. */
  progress: number;
  problem: string;
}

export interface PrivacyTest {
  ip: string;
  /** Where sites guess that address is ("Frankfurt, Hesse, DE"); may be empty. */
  place: string;
}

export interface StorageView {
  models_dir: string;
  default_dir: string;
  used: number;
  drives: { mount: string; free: number; total: number }[];
  moving: boolean;
  error: string | null;
}

export interface PhoneView {
  enabled: boolean;
  running: boolean;
  /** The page's address on this network, e.g. "http://192.168.1.42:47818". */
  address: string | null;
  problem: string | null;
  phones: { id: string; name: string; added: number; last_seen: number | null; has_password: boolean }[];
  /** Paired phones keep working while SulcusAI is locked. */
  while_locked: boolean;
  /** "Keep working while locked" (Settings → Security), which that needs. */
  keep_working: boolean;
}

export interface NewPhone {
  link: string;
  /** SVG of the QR code. */
  qr: string;
  view: PhoneView;
}

export interface PeerModels {
  peer_id: string;
  peer: string;
  /** Models that PC has and this one doesn't (and that fit here). */
  models: { model_id: string; quant: string; name: string; files: [string, number][] }[];
}

export interface SyncPeer {
  id: string;
  name: string;
  last_sync: number | null;
  /** Heard from on this network in the last two minutes. */
  online: boolean;
  syncing: boolean;
  error: string | null;
}

/** How syncing with one paired PC went, from "Sync now". */
export interface SyncOutcome {
  name: string;
  ok: boolean;
  /** Empty when it worked. */
  text: string;
  changes: number;
}

export interface SyncReport {
  view: SyncView;
  results: SyncOutcome[];
}

export interface SyncView {
  enabled: boolean;
  name: string;
  listening: boolean;
  port: number | null;
  /** This PC's address on the local network, for typing on the other PC. */
  address: string | null;
  peers: SyncPeer[];
  /** PCs found on this network that aren't paired (showing a code or not). */
  nearby: { id: string; name: string; pairing: boolean }[];
  /** The code this PC is showing, like "K7Q2-9XMB", and its seconds left. */
  code: string | null;
  code_left: number;
  /** A relay for PCs on other networks (used only at the Cloud level). */
  relay_url: string | null;
  relay_active: boolean;
}

/** A message in People (between people; never sent to a model). */
export interface PersonMessage {
  id: string;
  from_me: boolean;
  at: number;
  text: string;
  file: { name: string; size: number } | null;
  /** For messages from here: they have it. */
  delivered: boolean;
  /** For messages from here: they've read it (they send read receipts). */
  read: boolean;
}

/** A chat someone shares with this user (seen live while connected). */
export interface SharedChat {
  id: string;
  title: string;
  /** The model that writes its replies (on their PC). */
  model: string;
  /** A reply is being written. */
  busy: boolean;
  updated_at: number;
}

export interface SharedMsg {
  id: string;
  /** A tool the assistant used shows as "step", with its title only. */
  role: "user" | "assistant" | "step" | "note";
  text: string;
  at: number;
  /** Written by someone it's shared with (their name), or by their model. */
  by: string;
  /** For an answer from someone else's model: which. */
  model: string;
  /** This user wrote it (or this user's model did). */
  mine: boolean;
}

/** For the sidebar: a chat someone shares with this user. */
export interface SharedHere {
  contact_id: string;
  contact_name: string;
  /** Connected now, so it can be opened. */
  online: boolean;
  chat: SharedChat;
}

export interface SharedLive {
  status: "loading" | "thinking" | "writing" | "working" | string;
  content: string;
}

export interface SharedView {
  contact_id: string;
  chat: SharedChat;
  messages: SharedMsg[];
  live: SharedLive | null;
}

/** What someone's assistant may do here. */
export interface PersonAllow {
  /** "ask": approve each request; "auto": answer on its own; "off": turn them away. */
  mode: "ask" | "auto" | "off";
  notes: boolean;
  calendar: boolean;
  mail: boolean;
  folders: string[];
}

/** A question between assistants, or a task they work out together. */
export interface PersonAsk {
  id: string;
  contact_id: string;
  /** This user's assistant asked theirs. */
  from_me: boolean;
  kind: "ask" | "task";
  at: number;
  /** The question, or the task's goal. */
  question: string;
  answer: string | null;
  status: "sent" | "approval" | "working" | "answered" | "declined" | "failed";
  steps: string[];
  thread: { from_me: boolean; text: string; at: number }[];
}

/** A request waiting for this user to allow it. */
export interface PersonRequest {
  id: string;
  contact_id: string;
  name: string;
  kind: "ask" | "task";
  question: string;
  at: number;
  /** What their assistant would be able to use here. */
  uses: string[];
}

export interface SharingChoice {
  id: string;
  name: string;
  shared: boolean;
  online: boolean;
}

export interface PersonContact {
  id: string;
  name: string;
  /** Connected right now. */
  online: boolean;
  last_seen: number | null;
  address: string | null;
  unread: number;
  last: PersonMessage | null;
  /** Why the last call from here didn't get through. */
  error: string | null;
  /** Chats they share with this user (while connected). */
  shared: SharedChat[];
  /** Chats this user shares with them. */
  sharing: { chat_id: string; title: string }[];
  /** What their assistant may do here. */
  allow: PersonAllow;
  /** Not connected directly but reachable through a relay: whose. */
  relay: "mine" | "theirs" | null;
  /** The last exchange through the relay worked. */
  via_relay: boolean;
  /** This user lets them use their relay. */
  offer_relay: boolean;
  /** This user tells them when they've read their messages. */
  read_receipts: boolean;
}

/** A code for linking: it doesn't expire; it works for up to `max` people. */
export interface PersonCode {
  id: string;
  /** "K7Q2-9XMB". */
  code: string;
  label: string;
  max: number;
  /** Who linked with it. */
  used: { id: string; name: string; at: number; linked: boolean; blocked: boolean }[];
  /** Wrong codes tried against it (it stops after 10). */
  wrong: number;
  /** Taking links now. */
  open: boolean;
  /** Why not: used up, turned off, or too many wrong tries. */
  why: string | null;
  created_at: number;
}

/** Whether people can reach SulcusAI on each network (Windows Firewall). */
export interface PeopleFirewall {
  checked: boolean;
  networks: { name: string; kind: string; allowed: boolean }[];
}

export interface PeopleView {
  /** How others see this user. */
  name: string;
  /** This PC takes calls (others can reach it). */
  listening: boolean;
  /** Where others can reach this PC, by network ("Wi-Fi", "Radmin VPN"); `adapter` is Windows' name for it. */
  addresses: { address: string; network: string; adapter: string }[];
  /** This user's codes for linking, and who linked with each. */
  codes: PersonCode[];
  /** People who can't link again. */
  blocked: { id: string; name: string; at: number }[];
  contacts: PersonContact[];
  /** Apps on this network showing a code. */
  nearby: { id: string; name: string }[];
  unread: number;
  /** A relay is set (Settings → Your devices). */
  relay_set: boolean;
  /** The app is at the Cloud level (needed for relays). */
  cloud: boolean;
}

export interface BackupInfo {
  created: number;
  app_version: string;
  files: number;
  bytes: number;
}

export interface CloudModel {
  id: string;
  name: string;
  ctx: number;
  max_output: number;
  vision: boolean;
  /** US dollars per million tokens, if known. */
  price_in: number | null;
  price_out: number | null;
}

export interface CloudProvider {
  id: string;
  name: string;
  kind: "openai" | "anthropic";
  base_url: string;
  enabled: boolean;
  has_key: boolean;
  models: CloudModel[];
  media: CloudMedia[];
  media_suggestions: CloudMedia[];
}

export interface CloudView {
  providers: CloudProvider[];
  presets: CloudProvider[];
  budget: number | null;
  redact: boolean;
  spend: { month: string; usd: number; input_tokens: number; output_tokens: number; requests: number };
}

/** A cloud model people can pick for a chat. */
export interface CloudChoice {
  id: string;
  name: string;
  provider: string;
  vision: boolean;
}

export interface ApiServerView {
  enabled: boolean;
  port: number;
  key: string;
  url: string;
  running: boolean;
  problem: string | null;
}

export interface Advanced {
  enabled: boolean;
  sampling: { temperature: number | null; top_p: number | null; top_k: number | null; min_p: number | null; repeat_penalty: number | null; presence_penalty: number | null; frequency_penalty: number | null; seed: number | null };
  engine: { ctx: number | null; gpu_layers: number | null; threads: number | null; batch: number | null };
  guardrails: { instructions: string | null; extra: string; never: string[]; always_ask: boolean; max_steps: number | null };
  processing: { thinking: "auto" | "on" | "off"; handoff_at: number | null; handoff_off: boolean; new_chat?: boolean; helper_steps: number | null };
}

export interface HubFile {
  /** The file's path in the repository. */
  file: string;
  /** Memory it needs while running (estimated). */
  needed: number;
  /** The id it installs as. */
  model_id: string;
  quant: string;
  quality: string;
  size: number;
  placement: Placement | null;
  est_tps: number;
}

export interface HubModel {
  /** "owner/name" on Hugging Face. */
  id: string;
  name: string;
  author: string;
  downloads: number;
  likes: number;
  params_b: number | null;
  updated: string | null;
  vision: boolean;
  url: string;
  /** Only the files this PC can run, smallest first. */
  files: HubFile[];
  recommended: string | null;
}

/** Chats and models the automatic cleanup never removes. */
export interface KeepLocks {
  chats: string[];
  models: string[];
}

/** Something the automatic cleanup would remove (or did). */
export interface CleanItem {
  kind: "chat" | "model" | "media";
  id: string;
  name: string;
  last_used: number;
  size: number;
}

export interface CleanView {
  settings: { chat_days: number | null; model_days: number | null; last_run: number; last_result: string | null };
  locks: KeepLocks;
  would_remove: CleanItem[];
  next_run: number | null;
}

export interface CacheItem {
  id: string;
  name: string;
  what: string;
  size: number;
}

/** A chat folder in the sidebar. */
export interface FolderView {
  id: string;
  name: string;
  chats: string[];
  locked: boolean;
  shared_with: { id: string; name: string }[];
  collapsed: boolean;
}

/** A pasted Hugging Face link, as the model it points at, and the file it named. */
export type HubLinkResult =
  | { kind: "chat"; model: HubModel; file: string | null; /** The model the link named, when this is a GGUF copy of it (it had none). */ found_for?: string | null }
  | { kind: "media"; model: HubMediaModel; media: "image" | "video" | "music"; file: string | null };

export interface HubResults {
  models: HubModel[];
  /** Results left out because no file of theirs runs on this PC. */
  hidden: number;
}

/** What the speed test is doing (or what the last one left). */
export interface SpeedStatus {
  running: boolean;
  /** Testing every installed model. */
  all: boolean;
  model_id: string | null;
  done: number;
  total: number;
  /** Models whose test didn't finish, with why. */
  errors: Record<string, string>;
}

/** A download that was paused. */
export interface PausedDownload {
  model_id: string;
  quant: string;
  name: string;
  received: number;
  total: number;
  at: number;
}

/** A context window size and how the model would run with it. */
export interface WindowOption {
  ctx: number;
  placement: Placement | null;
  est_tps: number;
  needed: number;
}

export interface ContextOptions {
  /** What the person chose (null: automatic). */
  chosen: number | null;
  /** The automatic size for this model on this PC. */
  auto: number;
  /** The largest the model supports. */
  max: number;
  options: WindowOption[];
}

export interface FolderImport {
  /** Names of the models added. */
  added: string[];
  /** Model files that weren't added; `reason` is a whole sentence. */
  skipped: { name: string; reason: string }[];
}

export interface FoundModels {
  /** Names of the models added. */
  added: string[];
  /** Models seen that can't be used, with why. */
  skipped: { name: string; reason: string }[];
  /** The apps whose folders exist on this PC. */
  looked_in: string[];
}

export interface ModelCard {
  id: string;
  /** As shown: the user's name for it when they renamed it. */
  name: string;
  /** The model's own name, when the user renamed it. */
  renamed_from: string | null;
  /** Some version runs on this PC (memory, speed and disk). */
  runnable: boolean;
  /** The best model for this PC: the most capable one that runs comfortably (on the graphics card, when there is one). */
  best: boolean;
  publisher: string;
  source: string;
  description: string;
  license: License;
  tags: string[];
  params_b: number;
  default_ctx: number;
  tools: boolean;
  variants: Variant[];
  ratings: Ratings;
  /** The model can see pictures (it has an image encoder). */
  vision?: { file: string; size: number } | null;
  fit: { ctx: number; variants: VariantFit[]; recommended: string | null };
  installed: InstalledModel | null;
  /** A version whose file is already on this PC, so installing it skips the download. */
  on_disk?: string | null;
  /** Downloaded by another app (Ollama, LM Studio…) and used where it is. */
  local?: { app: string; path: string } | null;
}

export interface Hints {
  hidden: number;
  ram: { extra_ram_gb: number; unlocks: number }[];
  disk_blocked: number;
  disk_needed_bytes: number;
}

export interface CatalogView {
  hardware: Hardware;
  budget: Budget;
  backend: string | null;
  models: ModelCard[];
  hints: Hints;
  installing: string[];
}

export interface Settings {
  connectivity: Connectivity;
  default_model: string | null;
  onboarded: boolean;
  auto_lock_minutes: number;
  memory_enabled: boolean;
}

export interface Profile {
  name: string;
  about: string;
  preferences: string;
  /** Where they are, e.g. "Orem, Utah". */
  location?: string;
  lat?: number | null;
  lon?: number | null;
}

export type RunMode = "plan" | "auto" | "bypass";

export interface Chat {
  id: string;
  title: string;
  model_id: string | null;
  web: boolean;
  created_at: number;
  updated_at: number;
  mode: RunMode;
  project_id: string | null;
  incognito: boolean;
  parent_id: string | null;
  /** Nothing sent yet: kept out of the sidebar until the first message. */
  empty?: boolean;
}

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
  | { kind: "weekly"; weekday: number; time: string };

export interface Schedule {
  id: string;
  name: string;
  prompt: string;
  cadence: Cadence;
  model_id: string | null;
  project_id: string | null;
  allow_changes: boolean;
  enabled: boolean;
  last_run: number | null;
  next_run: number | null;
  last_chat: string | null;
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
  spec: { command: string; args: string[]; env: Record<string, string>; cwd: string | null };
  enabled: boolean;
  plugin: string | null;
  tools: McpTool[];
  tool_modes: Record<string, ToolMode>;
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
}

export interface PluginPreview {
  name: string;
  version: string;
  description: string;
  skills: string[];
  programs: string[];
}

export interface Memory {
  id: string;
  content: string;
  project_id: string | null;
  created_at: number;
  updated_at: number;
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

export interface ToolMeta {
  title: string;
  status: "ok" | "error" | "denied";
  kind: "diff" | "command" | "text";
  detail?: string | null;
  /** Charts to show with the result. */
  charts?: ChartSpec[];
  tool?: string;
  /** On the note in a chat that continued elsewhere. */
  handoff_to?: string;
  /** On the first message of a continued chat. */
  handoff_from?: string;
  /** A summary left by compacting; the model works from it instead of the messages before it. */
  compacted?: boolean;
  /** A user message added while the reply was being written. */
  added?: boolean;
  /** In a chat this user shares: written by that person (or their model). */
  from_person?: string;
  from_name?: string;
  model?: string;
  /** A message from the assistant of another chat (it asked this one). */
  from_chat?: string;
  from_title?: string;
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
  kind: "diff" | "command" | "text" | "folder" | "key";
  detail: string | null;
  note: string | null;
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
  risk: "read" | "write" | "execute" | "connector" | "memory" | "submit";
  preview: Preview;
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

export interface ContextInfo {
  ctx: number;
  system_tokens: number;
  profile_tokens: number;
  tools_tokens: number;
  history_tokens: number;
  reply_reserve: number;
  dropped_messages: number;
  last_total: number | null;
}

export interface EngineStatus {
  model_id: string | null;
  quant: string | null;
  ctx: number | null;
  gpu_layers: number | null;
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
  | "browser_control"
  | "images"
  | "video"
  | "music"
  | "files"
  | "memory"
  | "projects"
  | "scheduled"
  | "connectors"
  | "people"
  | "chat_team";

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
}

export type MainBrowser = { kind: "builtin" } | { kind: "system" } | { kind: "app"; name: string };

export interface BrowsersView {
  current: MainBrowser;
  installed: { name: string }[];
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
}

/** One message in the AI's sorting report. */
export interface TriageItem {
  id: string;
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
}

export interface TriageReport {
  run_at: number | null;
  model: string;
  looked_at: number;
  items: TriageItem[];
  /** New mail is sorted as it arrives. */
  auto: boolean;
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

export type PerfMode = "cool" | "balanced" | "turbo";

export interface PerfSettings {
  mode: PerfMode;
  turbo: { threads: number; gpu_percent: number; ram_gb: number } | null;
  cool_on_battery: boolean;
  heat_guard: boolean;
  adaptive: boolean;
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
  defaults: { steps: number; width: number; height: number; fps: number; seconds: number };
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
  hidden: boolean;
  /** Videos: a still frame is saved as the thumbnail. */
  poster: boolean;
  /** What making it took (older items have none). */
  stats: MadeStats | null;
}

/** How long making an item took, and what it used. */
export interface MadeStats {
  secs: number;
  on: "graphics card" | "processor" | "cloud" | string;
  /** Which one: the card's or processor's name, or the cloud model. */
  device: string | null;
  steps: number | null;
  /** About how many tokens the description was. */
  prompt_tokens: number | null;
  /** Music: audio tokens written (5 a second). */
  audio_tokens: number | null;
  /** Video: frames made. */
  frames: number | null;
  gpu_peak_temp: number | null;
  gpu_avg_usage: number | null;
  gpu_peak_usage: number | null;
  cpu_avg_usage: number | null;
  cpu_peak_usage: number | null;
}

export interface MediaJob {
  id: string;
  op: string;
  prompt: string;
  model_id: string | null;
  status: "queued" | "running";
  stage: string;
  fraction: number;
  eta_secs: number | null;
  est_secs: number;
  chat_id: string | null;
  /** What's making it while it runs, and which one (the card's or processor's name, or the cloud model). */
  on: "graphics card" | "processor" | "cloud" | null;
  device: string | null;
  /** The graphics card now: percent busy and °C. */
  gpu_usage: number | null;
  gpu_temp: number | null;
  /** The processor now: percent busy. */
  cpu_usage: number | null;
}

export type CloudMediaKind = "image" | "video" | "speech";

/** A cloud picture, video or voice model (Settings → Cloud models). */
export interface CloudMedia {
  id: string;
  name: string;
  kind: CloudMediaKind;
  /** $ per picture, per second of video, or per million characters read. */
  price: number | null;
}

/** One usable now (Cloud level, provider on, key set); id is "cloud:provider:model". */
export interface CloudMediaChoice extends CloudMedia {
  provider: string;
}

export interface MediaView {
  models: MediaModelCard[];
  cloud: CloudMediaChoice[];
  hidden: number;
  video_note: string | null;
  installing: string[];
  jobs: MediaJob[];
  gallery: number;
  /** The model used unless a job names one, per kind. */
  defaults: MediaDefaults;
}

export interface MediaDefaults {
  image: string | null;
  video: string | null;
  music: string | null;
}

/** One file of a Hugging Face repository the Studio can install. */
export interface HubMediaFile {
  file: string;
  model_id: string;
  quant: string;
  size: number;
  /** What's left to download (shared files may be here already). */
  download: number;
  runnable: boolean;
  on_gpu: boolean;
  est_secs: number;
  reason: string | null;
}

export interface HubMediaModel {
  id: string;
  name: string;
  author: string;
  downloads: number;
  likes: number;
  updated: string | null;
  url: string;
  /** "SDXL", "Z-Image", and the catalog model it builds on. */
  family: string;
  base_model: string;
  license: string | null;
  files: HubMediaFile[];
  recommended: string | null;
}

export interface HubMediaResults {
  models: HubMediaModel[];
  hidden: number;
}

export type MediaOp = "generate" | "edit" | "fill" | "extend" | "restyle" | "upscale" | "remove_background" | "video" | "music" | "sound" | "narrate";

export interface DevModeView {
  on: boolean;
  since: number | null;
  reports: number;
  last_sent: number | null;
}

export interface LogsView {
  folder: string;
  data_folder: string;
  files: { name: string; size: number; modified: number }[];
}

export interface HandoffsView {
  /** "Chats work together" is on: chats share freely. */
  together: boolean;
  notes: { key: string; path: string; by: string | null; at: number; allowed: string[] }[];
}

export interface PortRow {
  port: number;
  addresses: string[];
  local_only: boolean;
  pid: number;
  program: string;
  /** The assistant's server it belongs to. */
  server: string | null;
  /** Part of Windows: not stopped from here. */
  system: boolean;
}

export interface ServerInfo {
  id: string;
  command: string;
  folder: string;
  running: boolean;
  url: string | null;
}

export interface PortsView {
  ports: PortRow[];
  servers: ServerInfo[];
  supported: boolean;
}

export interface MediaRequest {
  op: MediaOp;
  model_id?: string | null;
  prompt: string;
  negative?: string | null;
  source?: string | null;
  /** PNG, base64. */
  mask?: string | null;
  shape?: "square" | "portrait" | "landscape" | "wide" | "tall";
  count?: number;
  seed?: number | null;
  extend?: [number, number, number, number];
  seconds?: number;
  lyrics?: string | null;
  instrumental?: boolean;
  voice?: string | null;
  language?: string | null;
  chat_id?: string | null;
  /** New pictures: a look from the Studio's list ("auto" by default). */
  style?: string | null;
}

/** Base64 of bytes (for sending files to the core). */
export function toBase64(bytes: Uint8Array): string {
  let s = "";
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(s);
}

/** The core's error text while app lock is engaged. */
export const LOCKED = "locked";

export const api = {
  appInfo: () => invoke<AppInfo>("app_info"),
  advancedView: () => invoke<{ settings: Advanced; locked: boolean }>("advanced_view"),
  setAdvanced: (settings: Advanced, pin: string | null) => invoke<void>("set_advanced", { settings, pin }),
  setAdvancedPin: (pin: string | null, newPin: string) => invoke<void>("set_advanced_pin", { pin, newPin }),
  checkAdvancedPin: (pin: string) => invoke<void>("check_advanced_pin", { pin }),
  apiServerView: () => invoke<ApiServerView>("api_server_view"),
  cloudView: () => invoke<CloudView>("cloud_view"),
  finetuneView: () => invoke<FinetuneView>("finetune_view"),
  startFinetune: (name: string, base: string, source: { kind: "chats"; chat_ids: string[] } | { kind: "file"; path: string }, epochs: number, strength: string) =>
    invoke<void>("start_finetune", { name, base, source, epochs, strength }),
  cancelFinetune: () => invoke<void>("cancel_finetune"),
  removeAdapter: (id: string) => invoke<void>("remove_adapter", { id }),
  updateView: () => invoke<UpdateView>("update_view"),
  checkForUpdate: () => invoke<UpdateAvailable | null>("check_for_update"),
  setUpdateAuto: (auto: boolean) => invoke<void>("set_update_auto", { auto }),
  setUpdateInstall: (install: boolean) => invoke<void>("set_update_install", { install }),
  installUpdate: () => invoke<void>("install_update"),
  privacyView: () => invoke<PrivacyView>("privacy_view"),
  setPrivacy: (settings: PrivacySettings) => invoke<void>("set_privacy", { settings }),
  testPrivacy: () => invoke<PrivacyTest>("test_privacy"),
  makeBackup: (path: string, password: string) => invoke<{ files: number; bytes: number }>("make_backup", { path, password }),
  backupInfo: (path: string) => invoke<BackupInfo>("backup_info", { path }),
  restoreBackup: (path: string, password: string) => invoke<void>("restore_backup", { path, password }),
  exportMarkdown: (dir: string) => invoke<string>("export_markdown_cmd", { dir }),
  storageView: () => invoke<StorageView>("storage_view"),
  moveModels: (dest: string) => invoke<void>("move_models", { dest }),
  phoneView: () => invoke<PhoneView>("phone_view"),
  setPhone: (enabled: boolean) => invoke<PhoneView>("set_phone", { enabled }),
  addPhone: (name: string, password: string) => invoke<NewPhone>("add_phone", { name, password }),
  removePhone: (id: string) => invoke<PhoneView>("remove_phone", { id }),
  setPhoneWhileLocked: (on: boolean) => invoke<PhoneView>("set_phone_while_locked", { on }),
  syncView: () => invoke<SyncView>("sync_view"),
  setSync: (enabled: boolean, name: string | null) => invoke<SyncView>("set_sync", { enabled, name }),
  startPairing: () => invoke<SyncView>("start_pairing"),
  cancelPairing: () => invoke<SyncView>("cancel_pairing"),
  pairDevice: (code: string, target: string | null) => invoke<SyncView>("pair_device", { code, target }),
  removeDevice: (id: string) => invoke<SyncView>("remove_device", { id }),
  syncNow: () => invoke<SyncReport>("sync_now"),
  setRelay: (url: string | null) => invoke<SyncView>("set_relay", { url }),
  peerModels: () => invoke<PeerModels[]>("peer_models"),
  copyModelFrom: (peerId: string, modelId: string) => invoke<void>("copy_model_from", { peerId, modelId }),
  peopleView: () => invoke<PeopleView>("people_view"),
  peopleUnread: () => invoke<number>("people_unread"),
  setPeopleName: (name: string) => invoke<PeopleView>("set_people_name", { name }),
  /** A new code, good for `max` people. */
  peopleNewCode: (max: number, label: string | null) => invoke<PeopleView>("people_new_code", { max, label }),
  peopleSetCode: (id: string, change: { max?: number; label?: string; on?: boolean }) =>
    invoke<PeopleView>("people_set_code", { id, max: change.max ?? null, label: change.label ?? null, on: change.on ?? null }),
  peopleDeleteCode: (id: string) => invoke<PeopleView>("people_delete_code", { id }),
  /** Blocks someone (removing them), or unblocks them. */
  peopleBlock: (id: string, name: string | null, block: boolean) => invoke<PeopleView>("people_block", { id, name, block }),
  peopleFirewall: () => invoke<PeopleFirewall>("people_firewall"),
  /** Lets people reach SulcusAI on every network; Windows asks for permission. */
  peopleAllowFirewall: () => invoke<PeopleFirewall>("people_allow_firewall"),
  /** Links with whoever shows `code`; returns their name. */
  peopleConnect: (code: string, target: string | null) => invoke<string>("people_connect", { code, target }),
  peopleRemove: (id: string) => invoke<PeopleView>("people_remove", { id }),
  peopleSetAddress: (id: string, address: string) => invoke<void>("people_set_address", { id, address }),
  /** A conversation; marks what they sent as read. */
  peopleMessages: (id: string) => invoke<PersonMessage[]>("people_messages", { id }),
  peopleSend: (id: string, text: string, file: string | null) => invoke<PersonMessage>("people_send", { id, text, file }),
  peopleSaveFile: (id: string, message: string, to: string) => invoke<void>("people_save_file", { id, message, to }),
  peopleChatSharing: (chatId: string) => invoke<SharingChoice[]>("people_chat_sharing", { chatId }),
  /** Who may watch a chat (an empty list stops sharing it). */
  peopleShareChat: (chatId: string, withIds: string[]) => invoke<SharingChoice[]>("people_share_chat", { chatId, with: withIds }),
  /** Opens a chat someone shares; changes follow as people:snapshot and people:live. */
  peopleWatch: (contactId: string, chatId: string) => invoke<SharedView>("people_watch", { contactId, chatId }),
  peopleUnwatch: (contactId: string, chatId: string) => invoke<void>("people_unwatch", { contactId, chatId }),
  /** Chats others share with this user (for the sidebar). */
  peopleSharedChats: () => invoke<SharedHere[]>("people_shared_chats"),
  /** Writes in a chat someone shares; this user's model answers it (people:post_live). */
  peopleSharedPost: (contactId: string, chatId: string, text: string) => invoke<void>("people_shared_post", { contactId, chatId, text }),
  peopleSharedStop: (contactId: string, chatId: string) => invoke<void>("people_shared_stop", { contactId, chatId }),
  peopleRequests: () => invoke<PersonRequest[]>("people_requests"),
  /** "assistant": let this user's assistant answer; "mine": answer with `text`; "decline". */
  peopleReply: (id: string, choice: "assistant" | "mine" | "decline", text: string | null, always: boolean) =>
    invoke<void>("people_reply", { id, choice, text, always }),
  peopleStop: (id: string) => invoke<void>("people_stop", { id }),
  peopleSetAllow: (id: string, allow: PersonAllow) => invoke<void>("people_set_allow", { id, allow }),
  peopleAsks: (id: string) => invoke<PersonAsk[]>("people_asks", { id }),
  /** Lets someone reach this app through this user's relay. */
  peopleSetRelayOffer: (id: string, on: boolean) => invoke<void>("people_set_relay_offer", { id, on }),
  peopleSetReadReceipts: (id: string, on: boolean) => invoke<void>("people_set_read_receipts", { id, on }),
  addCloudProvider: (preset: string | null, name: string | null, baseUrl: string | null) => invoke<CloudView>("add_cloud_provider", { preset, name, baseUrl }),
  removeCloudProvider: (id: string) => invoke<CloudView>("remove_cloud_provider", { id }),
  setCloudProvider: (id: string, enabled: boolean, key: string | null) => invoke<CloudView>("set_cloud_provider", { id, enabled, key }),
  cloudModelsAvailable: (id: string) => invoke<CloudModel[]>("cloud_models_available", { id }),
  setCloudMedia: (id: string, media: CloudMedia[]) => invoke<CloudView>("set_cloud_media", { id, media }),
  setCloudModels: (id: string, models: CloudModel[]) => invoke<CloudView>("set_cloud_models", { id, models }),
  setCloudOptions: (budget: number | null, redact: boolean) => invoke<CloudView>("set_cloud_options", { budget, redact }),
  cloudChoices: () => invoke<CloudChoice[]>("cloud_choices"),
  setApiServer: (enabled: boolean, port: number) => invoke<ApiServerView>("set_api_server", { enabled, port }),
  newApiKey: () => invoke<ApiServerView>("new_api_key"),
  importModelFile: (path: string) => invoke<string>("import_model_file", { path }),
  importModelFolder: (path: string) => invoke<FolderImport>("import_model_folder", { path }),
  renameModel: (modelId: string, name: string) => invoke<void>("rename_model", { modelId, name }),
  /** Summarizes the chat so far; the model continues from the summary. */
  compactChat: (chatId: string) => invoke<void>("compact_chat", { chatId }),
  contextOptions: (modelId: string | null) => invoke<ContextOptions>("context_options", { modelId }),
  /** null: automatic. */
  setContextWindow: (size: number | null) => invoke<void>("set_context_window", { size }),
  hubSearch: (query: string, sort: "downloads" | "trending" | "newest", all = false) => invoke<HubResults>("hub_search", { query, sort, all }),
  /** Starts installing one file; returns the model id its progress is reported under. */
  hubInstall: (repo: string, file: string, force = false) => invoke<string>("hub_install", { repo, file, force }),
  hubLink: (link: string) => invoke<HubLinkResult>("hub_link", { link }),
  measureModelSpeed: (modelId: string) => invoke<Speed>("measure_model_speed", { modelId }),
  // Tests every installed chat model; progress comes as models:speed.
  speedTestStatus: () => invoke<SpeedStatus>("speed_test_status"),
  measureAllSpeeds: () => invoke<number>("measure_all_speeds"),
  refreshHardware: () => invoke<Hardware>("refresh_hardware"),
  /** The models this PC can run, or with `all` every one. */
  catalog: (all = false) => invoke<CatalogView>("catalog_view", { all }),
  /** `force`: install even though this PC probably can't run it (the person was warned). */
  install: (modelId: string, quant: string, force = false) => invoke<void>("install_model", { modelId, quant, force }),
  pauseInstall: (modelId: string) => invoke<void>("pause_install", { modelId }),
  resumeInstall: (modelId: string) => invoke<void>("resume_install", { modelId }),
  downloadsView: () => invoke<{ paused: PausedDownload[] }>("downloads_view"),
  /** Adds models already on this PC (an earlier install or another app). `again` also brings back ones removed earlier. */
  findModels: (again = false) => invoke<FoundModels>("find_models", { again }),
  cancelInstall: (modelId: string) => invoke<void>("cancel_install", { modelId }),
  /** Removes a chat model; `deleteFile` also sends another app's (or a folder's) file to the Recycle Bin. Returns the bytes freed. */
  removeModel: (modelId: string, deleteFile = false) => invoke<number>("remove_model", { modelId, deleteFile }),
  keepLocks: () => invoke<KeepLocks>("keep_locks"),
  setKept: (kind: "chat" | "model", id: string, keep: boolean) => invoke<KeepLocks>("set_kept", { kind, id, keep }),
  cleanView: () => invoke<CleanView>("clean_view"),
  setAutoClean: (chatDays: number | null, modelDays: number | null) => invoke<CleanView>("set_auto_clean", { chatDays, modelDays }),
  cleanNow: () => invoke<CleanView>("clean_now"),
  cacheView: () => invoke<CacheItem[]>("cache_view"),
  clearCache: (ids: string[]) => invoke<{ freed: number; skipped: number }>("clear_cache", { ids }),
  foldersView: () => invoke<FolderView[]>("folders_view"),
  createFolder: (name: string, chatId: string | null = null) => invoke<FolderView[]>("create_folder", { name, chatId }),
  renameFolder: (id: string, name: string) => invoke<FolderView[]>("rename_folder", { id, name }),
  setFolderFlags: (id: string, flags: { locked?: boolean; collapsed?: boolean }) => invoke<FolderView[]>("set_folder_flags", { id, locked: flags.locked ?? null, collapsed: flags.collapsed ?? null }),
  deleteFolder: (id: string) => invoke<FolderView[]>("delete_folder", { id }),
  moveToFolder: (chatId: string, folderId: string | null) => invoke<FolderView[]>("move_to_folder", { chatId, folderId }),
  folderSharing: (id: string) => invoke<{ id: string; name: string; shared: boolean }[]>("folder_sharing", { id }),
  shareFolder: (id: string, withIds: string[]) => invoke<{ id: string; name: string; shared: boolean }[]>("share_folder", { id, with: withIds }),
  settings: () => invoke<Settings>("get_settings"),
  offlineView: () => invoke<OfflineView>("offline_view"),
  /** To the developer's inbox: only this text (plus a reply address and the version, if given). */
  sendFeedback: (kind: "bug" | "idea", text: string, contact: string | null, version: boolean) => invoke<void>("send_feedback", { kind, text, contact, version }),
  crashReportsView: () => invoke<CrashView>("crash_reports_view"),
  setCrashReports: (on: boolean) => invoke<CrashView>("set_crash_reports", { on }),
  dismissCrashReports: () => invoke<void>("dismiss_crash_reports"),
  sendCrashReport: (id: string) => invoke<void>("send_crash_report", { id }),
  crashPing: () => invoke<void>("crash_ping"),
  reportWindowError: (message: string, stack: string | null) => invoke<void>("report_window_error", { message, stack }),
  /** Fully offline on (with what to keep) or off. */
  setFullyOffline: (on: boolean, keep: OfflineKeep[]) => invoke<OfflineView>("set_fully_offline", { on, keep }),
  setConnectivity: (level: Connectivity) => invoke<Settings>("set_connectivity", { level }),
  setDefaultModel: (modelId: string) => invoke<Settings>("set_default_model", { modelId }),
  profile: () => invoke<Profile>("get_profile"),
  setProfile: (profile: Profile) => invoke<void>("set_profile", { profile }),
  chats: () => invoke<Chat[]>("list_chats"),
  createChat: () => invoke<Chat>("create_chat"),
  deleteChat: (chatId: string) => invoke<void>("delete_chat", { chatId }),
  renameChat: (chatId: string, title: string) => invoke<void>("rename_chat", { chatId, title }),
  setChatWeb: (chatId: string, web: boolean) => invoke<void>("set_chat_web", { chatId, web }),
  setChatModel: (chatId: string, modelId: string) => invoke<void>("set_chat_model", { chatId, modelId }),
  messages: (chatId: string) => invoke<Message[]>("get_messages", { chatId }),
  context: (chatId: string) => invoke<ContextInfo | null>("get_context", { chatId }),
  send: (chatId: string, text: string, images?: string[]) => invoke<Message | null>("send_message", { chatId, text, images: images ?? null }),
  /** While a reply is written: the assistant reads it at its next step ("idle": no reply running, send it). */
  chatAdd: (chatId: string, text: string, images?: string[]) => invoke<"added" | "idle">("chat_add", { chatId, text, images: images ?? null }),
  /** While a reply is written: sends it when the reply is done. */
  chatQueueAdd: (chatId: string, text: string, images?: string[]) => invoke<QueueItem[]>("chat_queue_add", { chatId, text, images: images ?? null }),
  chatQueue: (chatId: string) => invoke<QueueItem[]>("chat_queue", { chatId }),
  /** Messages the running reply hasn't read yet. */
  chatInbox: (chatId: string) => invoke<QueueItem[]>("chat_inbox", { chatId }),
  chatQueueRemove: (chatId: string, id: string) => invoke<QueueItem[]>("chat_queue_remove", { chatId, id }),
  chatQueueSend: (chatId: string) => invoke<void>("chat_queue_send", { chatId }),
  stop: (chatId: string) => invoke<void>("stop_generation", { chatId }),
  // The reply a chat is in the middle of (it keeps going while the user is elsewhere).
  live: (chatId: string) => invoke<LiveReply | null>("chat_live", { chatId }),
  engineStatus: () => invoke<EngineStatus>("engine_status"),
  unload: () => invoke<void>("unload_model"),
  /** Loads the default model now (it otherwise loads with the first message). */
  loadDefault: () => invoke<void>("load_default_model"),
  finishOnboarding: () => invoke<Settings>("finish_onboarding"),
  security: () => invoke<SecurityStatus>("security_status"),
  enableLock: (pin: string) => invoke<string>("enable_lock", { pin }),
  changePin: (oldPin: string, newPin: string) => invoke<void>("change_pin", { oldPin, newPin }),
  resetPin: (newPin: string) => invoke<string>("reset_pin", { newPin }),
  disableLock: (pin: string) => invoke<void>("disable_lock", { pin }),
  unlock: (method: "pin" | "recovery", secret: string) => invoke<void>("unlock", { method, secret }),
  unlockWithHello: () => invoke<void>("unlock_with_hello"),
  lockNow: () => invoke<void>("lock_now"),
  setKeepWorking: (on: boolean) => invoke<void>("set_keep_working", { on }),
  setHello: (enabled: boolean) => invoke<void>("set_hello", { enabled }),
  setAutoLock: (minutes: number) => invoke<void>("set_auto_lock", { minutes }),
  /** The activity log, newest first; `since` in ms limits it to a period. */
  actions: (limit?: number, since?: number) => invoke<Action[]>("list_actions", { limit, since: since ?? null }),
  clearActions: () => invoke<void>("clear_actions"),
  folders: () => invoke<Folder[]>("list_folders"),
  addFolder: (path: string, allowSystem = false) => invoke<void>("add_folder", { path, allowSystem }),
  /** The warning to confirm before sharing this folder, if it needs one. */
  folderWarning: (path: string) => invoke<string | null>("folder_warning", { path }),
  removeFolder: (path: string) => invoke<void>("remove_folder", { path }),
  /** Picks another folder for the assistant's folder request. */
  chooseFolder: (callId: string, path: string) => invoke<void>("choose_folder", { callId, path }),
  saveChart: (path: string, svg: string) => invoke<void>("save_chart", { path, svg }),
  siteKeyAsk: (callId: string) => invoke<SiteKeyAsk>("site_key_ask", { callId }),
  siteKeyGive: (callId: string, key: string) => invoke<void>("site_key_give", { callId, key }),
  siteKeys: () => invoke<SavedSiteKey[]>("site_keys"),
  siteKeyForget: (host: string) => invoke<void>("site_key_forget", { host }),
  desktopAccess: () => invoke<DesktopAccess>("desktop_access"),
  setDesktopAccess: (on: boolean) => invoke<DesktopAccess>("set_desktop_access", { on }),
  setChatMode: (chatId: string, mode: RunMode) => invoke<void>("set_chat_mode", { chatId, mode }),
  answerApproval: (callId: string, decision: "allow" | "always" | "deny") => invoke<void>("answer_approval", { callId, decision }),
  pendingApprovals: (chatId: string) => invoke<PendingApproval[]>("pending_approvals", { chatId }),
  undoableTurns: (chatId: string) => invoke<string[]>("undoable_turns", { chatId }),
  undoTurn: (chatId: string, turnId: string) => invoke<string[]>("undo_turn", { chatId, turnId }),
  projects: () => invoke<Project[]>("list_projects"),
  createProject: (name: string) => invoke<Project>("create_project", { name }),
  updateProject: (id: string, name: string, instructions: string) => invoke<void>("update_project", { id, name, instructions }),
  deleteProject: (id: string) => invoke<void>("delete_project", { id }),
  addProjectFolder: (id: string, path: string, allowSystem = false) => invoke<void>("add_project_folder", { id, path, allowSystem }),
  removeProjectFolder: (id: string, path: string) => invoke<void>("remove_project_folder", { id, path }),
  createChatIn: (projectId: string | null, incognito: boolean) => invoke<Chat>("create_chat_in", { projectId, incognito }),
  leaveIncognito: (keep: string | null) => invoke<void>("leave_incognito", { keep }),
  setChatProject: (chatId: string, projectId: string | null) => invoke<void>("set_chat_project", { chatId, projectId }),
  memories: () => invoke<Memory[]>("list_memories"),
  addMemory: (content: string, projectId: string | null) => invoke<Memory>("add_memory", { content, projectId }),
  updateMemory: (id: string, content: string) => invoke<void>("update_memory", { id, content }),
  deleteMemory: (id: string) => invoke<void>("delete_memory", { id }),
  clearMemories: () => invoke<void>("clear_memories"),
  setMemoryEnabled: (enabled: boolean) => invoke<Settings>("set_memory_enabled", { enabled }),
  schedules: () => invoke<ScheduleView[]>("list_schedules"),
  saveSchedule: (schedule: Schedule) => invoke<ScheduleView>("save_schedule", { schedule }),
  deleteSchedule: (id: string) => invoke<void>("delete_schedule", { id }),
  runScheduleNow: (id: string) => invoke<string>("run_schedule_now", { id }),
  connectors: () => invoke<Connector[]>("list_connectors"),
  saveConnector: (input: { id: string; name: string; command: string; args: string[]; env: Record<string, string> }) =>
    invoke<Connector>("save_connector", { input }),
  deleteConnector: (id: string) => invoke<void>("delete_connector", { id }),
  setConnectorEnabled: (id: string, enabled: boolean) => invoke<void>("set_connector_enabled", { id, enabled }),
  setToolMode: (id: string, tool: string, mode: ToolMode) => invoke<void>("set_tool_mode", { id, tool, mode }),
  plugins: () => invoke<Plugin[]>("list_plugins"),
  inspectPlugin: (path: string) => invoke<PluginPreview>("inspect_plugin", { path }),
  installPlugin: (path: string) => invoke<Plugin>("install_plugin", { path }),
  setPluginEnabled: (id: string, enabled: boolean) => invoke<void>("set_plugin_enabled", { id, enabled }),
  removePlugin: (id: string) => invoke<void>("remove_plugin", { id }),
  speechView: () => invoke<SpeechView>("speech_view"),
  installSpeech: (modelId: string) => invoke<void>("install_speech_model", { modelId }),
  removeSpeech: (modelId: string) => invoke<void>("remove_speech_model", { modelId }),
  voiceSettings: () => invoke<VoiceSettings>("get_voice_settings"),
  setVoiceSettings: (settings: VoiceSettings) => invoke<VoiceSettings>("set_voice_settings", { settings }),
  audioDevices: () => invoke<{ inputs: AudioDevice[]; outputs: AudioDevice[] }>("audio_devices"),
  voices: () => invoke<Voice[]>("list_voices"),
  speak: (text: string, language?: string | null) => invoke<void>("speak", { text, language: language ?? null }),
  stopSpeaking: () => invoke<void>("stop_speaking"),
  startDictation: () => invoke<string>("start_dictation"),
  stopDictation: (id: string) => invoke<void>("stop_dictation", { id }),
  startVoice: (chatId: string) => invoke<void>("start_voice", { chatId }),
  stopVoice: (chatId: string) => invoke<void>("stop_voice", { chatId }),
  interruptVoice: (chatId: string) => invoke<void>("interrupt_voice", { chatId }),
  startMeeting: (options: { title: string | null; mic: boolean; system: boolean; translate_to: string | null }) =>
    invoke<Meeting>("start_meeting", { options }),
  stopMeeting: () => invoke<void>("stop_meeting"),
  liveMeeting: () => invoke<string | null>("live_meeting"),
  meetings: () => invoke<Meeting[]>("list_meetings"),
  meeting: (id: string) => invoke<MeetingDetail>("get_meeting", { id }),
  renameMeeting: (id: string, title: string) => invoke<void>("rename_meeting", { id, title }),
  setActionDone: (id: string, index: number, done: boolean) => invoke<void>("set_action_done", { id, index, done }),
  deleteMeeting: (id: string) => invoke<void>("delete_meeting", { id }),
  deleteMeetingAudio: (id: string) => invoke<void>("delete_meeting_audio", { id }),
  rewriteNotes: (id: string) => invoke<void>("rewrite_notes", { id }),
  meetingClip: (id: string, start: number, end: number) => invoke<ArrayBuffer>("meeting_clip", { id, start, end }),
  exportMeeting: (id: string, path: string, transcript: boolean) => invoke<void>("export_meeting", { id, path, transcript }),
  askAboutMeeting: (id: string) => invoke<string>("ask_about_meeting", { id }),
  searchMeetings: (query: string) => invoke<MeetingHit[]>("search_meetings", { query }),
  translate: (text: string, to: string, detect: boolean) => invoke<{ text: string; detected: string | null }>("translate", { text, to, detect }),
  languages: () => invoke<Language[]>("translation_languages"),
  translateFile: (path: string, to: string) => invoke<string>("translate_file", { path, to }),
  voicePacks: () => invoke<VoicePack[]>("voice_packs"),
  installVoicePack: (packId: string) => invoke<void>("install_voice_pack", { packId }),
  removeVoicePack: (packId: string) => invoke<void>("remove_voice_pack", { packId }),
  speakerModel: () => invoke<SpeakerModel>("speaker_model"),
  installSpeakerModel: () => invoke<void>("install_speaker_model"),
  removeSpeakerModel: () => invoke<void>("remove_speaker_model"),
  renameSpeaker: (id: string, voice: number, name: string) => invoke<void>("rename_speaker", { id, voice, name }),
  features: () => invoke<FeatureView[]>("features_view"),
  setFeature: (feature: FeatureId, on: boolean) => invoke<FeatureId[]>("set_feature", { feature, on }),
  installFeature: (feature: FeatureId) => invoke<void>("install_feature", { feature }),
  notes: () => invoke<Note[]>("notes"),
  saveNote: (id: string | null, note: NoteBody) => invoke<Note>("save_note_cmd", { id, note }),
  pinNote: (id: string, pinned: boolean) => invoke<void>("pin_note", { id, pinned }),
  deleteNote: (id: string) => invoke<void>("delete_note", { id }),
  tasks: () => invoke<Task[]>("tasks"),
  saveTask: (task: Task) => invoke<Task>("save_task_cmd", { task }),
  deleteTask: (id: string) => invoke<void>("delete_task", { id }),
  parseDue: (text: string) => invoke<{ due: number; has_time: boolean } | null>("parse_due_text", { text }),
  tasksFromMeeting: (meetingId: string, indexes: number[]) => invoke<number>("tasks_from_meeting", { meetingId, indexes }),
  webSettings: () => invoke<WebSettings>("get_web_settings"),
  openBrowser: () => invoke<void>("open_browser"),
  browsersView: () => invoke<BrowsersView>("browsers_view"),
  detectLocation: () => invoke<{ label: string; lat: number; lon: number }>("detect_location"),
  bridgeStatus: () => invoke<BridgeStatus>("bridge_status"),
  showExtensionFolder: () => invoke<void>("show_extension_folder"),
  setMainBrowser: (choice: MainBrowser) => invoke<void>("set_main_browser", { choice }),
  /** Opens a web address (or just the browser) in the user's main browser. */
  openWeb: (url: string | null) => invoke<void>("open_web", { url }),
  openDocument: (path: string) => invoke<void>("open_document", { path }),
  /** Handoff notes chats left, and which chats may read them. */
  handoffsView: () => invoke<HandoffsView>("handoffs_view"),
  handoffRevoke: (key: string) => invoke<HandoffsView>("handoff_revoke", { key }),
  handoffForget: (key: string) => invoke<HandoffsView>("handoff_forget", { key }),
  /** Developer mode: reports to the developer from this PC (only after a yes). */
  devModeView: () => invoke<DevModeView>("dev_mode_view"),
  setDevMode: (on: boolean, understood: boolean) => invoke<DevModeView>("set_dev_mode", { on, understood }),
  sendDevReport: () => invoke<DevModeView>("send_dev_report"),
  /** Settings → Logs. */
  logsView: () => invoke<LogsView>("logs_view"),
  readLog: (name: string, lines?: number) => invoke<string>("read_log", { name, lines: lines ?? null }),
  /** Opens the logs folder, or with `data` the app's data folder, in File Explorer. */
  openAppFolder: (data: boolean) => invoke<void>("open_app_folder", { data }),
  clearLogs: () => invoke<LogsView>("clear_logs"),
  /** Servers and ports: what's listening on this PC. */
  portsView: () => invoke<PortsView>("ports_view"),
  stopPortProgram: (pid: number) => invoke<PortsView>("stop_port_program", { pid }),
  serverStop: (id: string) => invoke<PortsView>("server_stop", { id }),
  serverStart: (id: string) => invoke<PortsView>("server_start", { id }),
  /** Opens a port on this PC in SulcusAI's browser. */
  previewPort: (port: number) => invoke<void>("preview_port", { port }),
  previewOpen: (chatId: string, root: string, start: string | null) => invoke<SitePreview>("preview_open", { chatId, root, start }),
  previewStop: (id: string) => invoke<void>("preview_stop", { id }),
  previewOpenExternal: (url: string) => invoke<void>("preview_open_external", { url }),
  /** Shows a file the assistant made in File Explorer (nothing is run). */
  showInFolder: (path: string) => invoke<void>("show_in_folder", { path }),
  quickClipboard: () => invoke<string | null>("quick_clipboard"),
  quickHide: () => invoke<void>("quick_hide"),
  quickResize: (height: number) => invoke<void>("quick_resize", { height }),
  quickOpenInApp: (chatId: string | null) => invoke<void>("quick_open_in_app", { chatId }),
  quickScreenshot: () => invoke<MediaItem>("quick_screenshot"),
  mediaView: () => invoke<MediaView>("media_view"),
  installMedia: (modelId: string) => invoke<void>("install_media_model", { modelId }),
  mediaHubSearch: (kind: "image" | "video" | "music", query: string, sort: "downloads" | "trending" | "newest", all = false) =>
    invoke<HubMediaResults>("media_hub_search", { kind, query, sort, all }),
  mediaHubInstall: (repo: string, file: string) => invoke<string>("media_hub_install", { repo, file }),
  mediaSetDefault: (kind: "image" | "video" | "music", modelId: string | null) => invoke<MediaDefaults>("media_set_default", { kind, modelId }),
  removeMedia: (modelId: string) => invoke<number>("remove_media_model", { modelId }),
  mediaStart: (request: MediaRequest) => invoke<string>("media_start", { request }),
  mediaCancel: (jobId: string) => invoke<void>("media_cancel", { jobId }),
  mediaJobs: () => invoke<MediaJob[]>("media_jobs"),
  mediaList: (kind: string | null, before: number | null = null, limit = 60) => invoke<MediaItem[]>("media_list", { kind, before, limit }),
  /** A chat's pictures, video and music: made there or attached. */
  mediaInChat: (chatId: string) => invoke<MediaItem[]>("media_in_chat", { chatId }),
  /** A fuller description from a short idea, written by the chat model. */
  improveMediaPrompt: (prompt: string, kind: string) => invoke<string>("improve_media_prompt", { prompt, kind }),
  mediaGet: (id: string) => invoke<MediaItem>("media_get", { id }),
  mediaFile: (id: string) => invoke<ArrayBuffer>("media_file", { id }),
  mediaThumb: (id: string) => invoke<ArrayBuffer>("media_thumb", { id }),
  mediaSetPoster: (id: string, data: string) => invoke<MediaItem>("media_set_poster", { id, data }),
  mediaDelete: (id: string) => invoke<void>("media_delete", { id }),
  mediaFavorite: (id: string, on: boolean) => invoke<MediaItem>("media_favorite", { id, on }),
  mediaExport: (id: string, path: string) => invoke<void>("media_export", { id, path }),
  mediaImport: (path: string, chatId: string | null = null) => invoke<MediaItem>("media_import", { path, chatId }),
  mediaAdd: (input: { data: string; mime: string; op: string; prompt?: string | null; parent?: string | null; chatId?: string | null; hidden?: boolean; seconds?: number | null }) =>
    invoke<MediaItem>("media_add", { data: input.data, mime: input.mime, op: input.op, prompt: input.prompt ?? null, parent: input.parent ?? null, chatId: input.chatId ?? null, hidden: input.hidden ?? null, seconds: input.seconds ?? null }),
  mailPreset: (email: string) => invoke<{ config: MailConfig; note: string | null }>("mail_preset", { email }),
  mailAccounts: () => invoke<MailAccount[]>("mail_accounts"),
  addMailAccount: (config: MailConfig) => invoke<string>("add_mail_account", { config }),
  addMailAccountOAuth: (provider: OAuthProvider, name: string | null) => invoke<string>("add_mail_account_oauth", { provider, name, hint: null }),
  addCalendarAccountOAuth: (provider: OAuthProvider) => invoke<string>("add_calendar_account_oauth", { provider, hint: null }),
  oauthClients: () => invoke<OAuthClientsView>("oauth_clients"),
  setOauthClients: (ids: OAuthClients) => invoke<void>("set_oauth_clients", { ids }),
  removeMailAccount: (id: string) => invoke<void>("remove_mail_account", { id }),
  syncMail: () => invoke<number>("sync_mail"),
  mailList: (query: string) => invoke<MailItem[]>("mail_list", { query, limit: 300 }),
  mailGet: (id: string) => invoke<MailItem>("mail_get", { id }),
  mailSetSeen: (id: string, seen: boolean) => invoke<void>("mail_set_seen", { id, seen }),
  mailTriage: () => invoke<TriageReport>("mail_triage"),
  mailTriageReport: () => invoke<TriageReport>("mail_triage_report"),
  mailTriageUndo: (ids: string[]) => invoke<void>("mail_triage_undo", { ids }),
  setMailAutoSort: (on: boolean) => invoke<void>("set_mail_auto_sort", { on }),
  mailAction: (ids: string[], action: "seen" | "unseen" | "archive" | "trash") => invoke<void>("mail_action", { ids, action }),
  setUiLanguage: (code: string) => invoke<void>("set_ui_language", { code }),
  sendMail: (accountId: string, draft: MailDraft) => invoke<void>("send_mail", { accountId, draft }),
  calendarPresets: () => invoke<{ name: string; url: string; note: string }[]>("calendar_presets"),
  calendarAccounts: () => invoke<CalAccount[]>("calendar_accounts"),
  addCalendarAccount: (account: { name: string; url: string; username: string; password: string; calendars: CalInfo[] }) => invoke<string>("add_calendar_account", { account }),
  removeCalendarAccount: (id: string) => invoke<void>("remove_calendar_account", { id }),
  syncCalendars: () => invoke<number>("sync_calendars"),
  calendarEvents: (from: number, to: number) => invoke<CalEvent[]>("calendar_events", { from, to }),
  createEvent: (event: NewEvent) => invoke<CalEvent>("create_event", { event }),
  deleteEvent: (id: string) => invoke<void>("delete_event", { id }),
  perfView: () => invoke<PerfView>("perf_view"),
  gpuNow: () => invoke<GpuNow>("gpu_now"),
  /** How much of the model may run on the graphics card (percent). */
  gpuLimit: () => invoke<GpuLimit>("gpu_limit"),
  setGpuLimit: (percent: number) => invoke<GpuLimit>("set_gpu_limit", { percent }),
  answerMemoryWarning: (id: string, go: boolean) => invoke<void>("answer_memory_warning", { id, go }),
  setPerf: (settings: PerfSettings) => invoke<PerfLimits>("set_perf", { settings }),
  setWebSettings: (settings: WebSettings, braveKey: string | null) => invoke<void>("set_web_settings", { settings, braveKey }),
};

export type DictationEvent =
  | { id: string; state: "loading" | "listening" | "hearing" | "done" }
  | { id: string; state: "level"; level: number }
  | { id: string; state: "text"; text: string }
  | { id: string; state: "error"; error: string };

export type VoiceEvent =
  | { chat_id: string; state: "loading" | "listening" | "hearing" | "thinking" | "speaking" | "interrupted" | "ended" }
  | { chat_id: string; state: "level"; level: number; speaking: boolean }
  | { chat_id: string; state: "heard"; text: string }
  | { chat_id: string; state: "error"; error: string };

export type MeetingEvent =
  | { meeting_id: string; kind: "state"; status: MeetingStatus | "loading" }
  | { meeting_id: string; kind: "level"; you: number | null; others: number | null }
  | { meeting_id: string; kind: "segment"; segment: Segment }
  | { meeting_id: string; kind: "removed"; segment_id: number }
  | { meeting_id: string; kind: "translation"; segment_id: number; text: string }
  | { meeting_id: string; kind: "warning"; message: string }
  | { meeting_id: string; kind: "relabeled" }
  | { meeting_id: string; kind: "done" }
  | { meeting_id: string; kind: "error"; error: string };

/** The graphics card right now (bottom bar). */
/** A crash report kept on this PC, shown before anything is sent. */
export interface CrashReport {
  id: string;
  kind: "core" | "window" | "closed";
  at: number;
  text: string;
}

export interface CrashView {
  on: boolean;
  pending: CrashReport[];
}

export type OfflineKeep = "updates" | "downloads" | "web" | "local_network" | "connectors";

/** Fully offline: on or off, and what may still use the network. */
export interface OfflineView {
  on: boolean;
  items: { id: OfflineKeep; name: string; detail: string; in_use: boolean; kept: boolean }[];
}

export interface GpuLimit {
  percent: number;
  has_gpu: boolean;
}

/** Before loading a model: the PC is short of memory right now. */
export interface MemoryWarning {
  id: string;
  model: string;
  ram_need: number;
  ram_free: number;
  ram_total: number;
  vram_need: number;
  vram_free: number | null;
  vram_total: number | null;
  short_ram: boolean;
  short_vram: boolean;
  apps: { name: string; bytes: number }[];
}

export interface GpuNow {
  name: string | null;
  /** Percent busy. */
  usage: number | null;
  /** °C; only NVIDIA cards report it. */
  temp: number | null;
  mem_used: number | null;
  mem_total: number | null;
}

export interface InstallProgress {
  model_id: string;
  /** "loading" and "benchmark": the model is tried out on this PC (its speed is measured). */
  phase: "engine" | "verify" | "download" | "loading" | "benchmark" | "vision";
  received: number;
  total: number;
}

export interface InstallFinished {
  model_id: string;
  /** Set for speech models, which aren't in the chat catalog. */
  name?: string;
  ok: boolean;
  cancelled?: boolean;
  /** Paused: what's downloaded is kept, to resume. */
  paused?: boolean;
  error?: string;
}

export interface ChatEvents {
  "chat:status": { chat_id: string; status: "loading" | "handoff" | "cooling" | "compacting" | "looking_up" | "idle"; detail?: string };
  "chat:compacted": { chat_id: string; auto: boolean };
  "media:progress": MediaJob;
  "media:done": { job_id: string; ok: boolean; items?: MediaItem[]; cancelled?: boolean; error?: string };
  "chat:context": { chat_id: string; context: ContextInfo };
  "chat:start": { chat_id: string; message_id: string };
  "chat:delta": { chat_id: string; message_id: string; content: string | null; thinking: string | null };
  "chat:done": { chat_id: string; message?: Message | null; tps?: number | null; context?: ContextInfo; cancelled?: boolean; error?: string };
  "agent:step": { chat_id: string };
  "agent:tool_start": { chat_id: string; call_id: string; tool: string };
  "agent:approval": PendingApproval;
  "agent:approval_done": { chat_id: string; call_id: string };
  "agent:helper": { chat_id: string; call_id: string; step: string };
  "chat:handoff": { from: string; to: string };
  "chat:titled": { chat_id: string; title: string };
  "chat:queue": { chat_id: string; queue: QueueItem[] };
  "chat:inbox": { chat_id: string; waiting: QueueItem[] };
  /** The model has read this chat's messages up to then (it started answering). */
  "chat:read": { chat_id: string; up_to: number };
  "gpu:limit": { percent: number };
  "offline:changed": { on: boolean };
  "engine:memory": MemoryWarning;
  "engine:memory_done": { id: string };
  "chat:web": { chat_id: string; web: boolean };
  /** The assistant opened a site in the chat's Preview panel. */
  "chat:preview": { chat_id: string; url: string; site?: string; root?: string; start?: string; port?: number };
  "app:navigate": { page: string };
  "mail:triage": { done: number; total: number };
  "mail:triaged": Record<string, never>;
  "schedule:ran": { id: string; name: string; chat_id?: string; error?: string };
  "install:progress": InstallProgress;
  "install:finished": InstallFinished;
  "downloads:changed": Record<string, never>;
  "security:locked": null;
  "features:changed": Record<string, never>;
  "folders:changed": Record<string, never>;
  "models:found": { names: string[] };
  "task:reminder": { id: string; title: string };
  "perf:heat": HeatState;
  "mail:synced": { new: number };
  "quick:shown": Record<string, never>;
  "bridge:status": { connected: boolean };
  "quick:open-chat": { chat_id: string };
  "quick:hotkey_taken": { hotkey: string };
  "calendar:synced": Record<string, never>;
  "engine:unloaded": null;
  "engine:download": { received?: number; total?: number; done?: boolean };
  "update:available": UpdateAvailable;
  "finetune:progress": FinetuneJob;
  "sync:status": null;
  "models:moving": { done?: number; total?: number; finished?: boolean; error?: string };
  /** The speed test's status, plus the model just tested (tps or error), and finished at the end. */
  "models:speed": SpeedStatus & { tested?: string; tps?: number; prompt_tps?: number | null; error?: string; finished?: boolean };
  "sync:changed": null;
  "people:changed": null;
  "people:message": { contact_id: string };
  "people:shared": { contact_id: string };
  "people:ask": { contact_id: string; id: string };
  /** Linking, step by step: find, reach, check, save. */
  "people:connecting": { step: "find" | "reach" | "check" | "save" };
  /** Someone tried one of this user's codes. */
  "people:attempt": { invite_id: string; name: string; result: "checking" | "wrong" | "linked" | "refused"; at: number };
  "people:snapshot": { contact_id: string; chat_id: string; view: SharedView };
  "people:live": { contact_id: string; chat_id: string; live: SharedLive | null };
  /** This user's model answering in a chat someone shares. */
  "people:post_live": { contact_id: string; chat_id: string; status: "loading" | "writing" | "done"; delta: string | null };
  /** Someone wrote in a chat this user shares (or their model did). */
  "chat:changed": { chat_id: string };
  "update:progress": { received: number; total: number | null };
  "update:installing": Record<string, never>;
  "privacy:status": null;
  dictation: DictationEvent;
  voice: VoiceEvent;
  meeting: MeetingEvent;
}

export function on<K extends keyof ChatEvents>(name: K, handler: (payload: ChatEvents[K]) => void): Promise<UnlistenFn> {
  return listen<ChatEvents[K]>(name, (e) => handler(e.payload));
}

/** An error for people to read, in the window's language when the message
    has a translation (the core's messages are looked up as written). */
export function errorText(e: unknown): string {
  const text = typeof e === "string" ? e : e instanceof Error ? e.message : String(e);
  return t(text);
}

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
  tps: number | null;
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
  last_check: number;
  available: UpdateAvailable | null;
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
  phones: { id: string; name: string; added: number; last_seen: number | null }[];
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
  processing: { thinking: "auto" | "on" | "off"; handoff_at: number | null; handoff_off: boolean; helper_steps: number | null };
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
  name: string;
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

export interface ToolMeta {
  title: string;
  status: "ok" | "error" | "denied";
  kind: "diff" | "command" | "text";
  detail?: string | null;
  tool?: string;
  /** On the note in a chat that continued elsewhere. */
  handoff_to?: string;
  /** On the first message of a continued chat. */
  handoff_from?: string;
  /** A document the assistant made (it can be opened). */
  file?: string;
  /** The assistant asked the user to turn on web access. */
  web_request?: boolean;
  /** Pictures, clips or sounds the assistant made. */
  media?: MediaItem[];
  /** User messages: attached pictures (gallery ids). */
  images?: string[];
}

export interface Preview {
  title: string;
  kind: "diff" | "command" | "text";
  detail: string | null;
  note: string | null;
}

export interface PendingApproval {
  chat_id: string;
  call_id: string;
  tool: string;
  risk: "read" | "write" | "execute" | "connector" | "memory" | "submit";
  preview: Preview;
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
  | "connectors";

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
}

export type MediaOp = "generate" | "edit" | "fill" | "extend" | "restyle" | "upscale" | "remove_background" | "video" | "music" | "sound" | "narrate";

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
  installUpdate: () => invoke<void>("install_update"),
  makeBackup: (path: string, password: string) => invoke<{ files: number; bytes: number }>("make_backup", { path, password }),
  backupInfo: (path: string) => invoke<BackupInfo>("backup_info", { path }),
  restoreBackup: (path: string, password: string) => invoke<void>("restore_backup", { path, password }),
  exportMarkdown: (dir: string) => invoke<string>("export_markdown_cmd", { dir }),
  storageView: () => invoke<StorageView>("storage_view"),
  moveModels: (dest: string) => invoke<void>("move_models", { dest }),
  phoneView: () => invoke<PhoneView>("phone_view"),
  setPhone: (enabled: boolean) => invoke<PhoneView>("set_phone", { enabled }),
  addPhone: (name: string) => invoke<NewPhone>("add_phone", { name }),
  removePhone: (id: string) => invoke<PhoneView>("remove_phone", { id }),
  syncView: () => invoke<SyncView>("sync_view"),
  setSync: (enabled: boolean, name: string | null) => invoke<SyncView>("set_sync", { enabled, name }),
  startPairing: () => invoke<SyncView>("start_pairing"),
  cancelPairing: () => invoke<SyncView>("cancel_pairing"),
  pairDevice: (code: string, target: string | null) => invoke<SyncView>("pair_device", { code, target }),
  removeDevice: (id: string) => invoke<SyncView>("remove_device", { id }),
  syncNow: () => invoke<SyncView>("sync_now"),
  peerModels: () => invoke<PeerModels[]>("peer_models"),
  copyModelFrom: (peerId: string, modelId: string) => invoke<void>("copy_model_from", { peerId, modelId }),
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
  measureModelSpeed: (modelId: string) => invoke<number>("measure_model_speed", { modelId }),
  refreshHardware: () => invoke<Hardware>("refresh_hardware"),
  catalog: () => invoke<CatalogView>("catalog_view"),
  install: (modelId: string, quant: string) => invoke<void>("install_model", { modelId, quant }),
  /** Adds models already on this PC (an earlier install or another app). `again` also brings back ones removed earlier. */
  findModels: (again = false) => invoke<FoundModels>("find_models", { again }),
  cancelInstall: (modelId: string) => invoke<void>("cancel_install", { modelId }),
  removeModel: (modelId: string) => invoke<void>("remove_model", { modelId }),
  settings: () => invoke<Settings>("get_settings"),
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
  stop: (chatId: string) => invoke<void>("stop_generation", { chatId }),
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
  actions: (limit?: number) => invoke<Action[]>("list_actions", { limit }),
  clearActions: () => invoke<void>("clear_actions"),
  folders: () => invoke<Folder[]>("list_folders"),
  addFolder: (path: string) => invoke<void>("add_folder", { path }),
  removeFolder: (path: string) => invoke<void>("remove_folder", { path }),
  setChatMode: (chatId: string, mode: RunMode) => invoke<void>("set_chat_mode", { chatId, mode }),
  answerApproval: (callId: string, decision: "allow" | "always" | "deny") => invoke<void>("answer_approval", { callId, decision }),
  pendingApprovals: (chatId: string) => invoke<PendingApproval[]>("pending_approvals", { chatId }),
  undoableTurns: (chatId: string) => invoke<string[]>("undoable_turns", { chatId }),
  undoTurn: (chatId: string, turnId: string) => invoke<string[]>("undo_turn", { chatId, turnId }),
  projects: () => invoke<Project[]>("list_projects"),
  createProject: (name: string) => invoke<Project>("create_project", { name }),
  updateProject: (id: string, name: string, instructions: string) => invoke<void>("update_project", { id, name, instructions }),
  deleteProject: (id: string) => invoke<void>("delete_project", { id }),
  addProjectFolder: (id: string, path: string) => invoke<void>("add_project_folder", { id, path }),
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
  quickClipboard: () => invoke<string | null>("quick_clipboard"),
  quickHide: () => invoke<void>("quick_hide"),
  quickResize: (height: number) => invoke<void>("quick_resize", { height }),
  quickOpenInApp: (chatId: string | null) => invoke<void>("quick_open_in_app", { chatId }),
  quickScreenshot: () => invoke<MediaItem>("quick_screenshot"),
  mediaView: () => invoke<MediaView>("media_view"),
  installMedia: (modelId: string) => invoke<void>("install_media_model", { modelId }),
  removeMedia: (modelId: string) => invoke<void>("remove_media_model", { modelId }),
  mediaStart: (request: MediaRequest) => invoke<string>("media_start", { request }),
  mediaCancel: (jobId: string) => invoke<void>("media_cancel", { jobId }),
  mediaJobs: () => invoke<MediaJob[]>("media_jobs"),
  mediaList: (kind: string | null, before: number | null = null, limit = 60) => invoke<MediaItem[]>("media_list", { kind, before, limit }),
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
  oauthClients: () => invoke<OAuthClients>("oauth_clients"),
  setOauthClients: (ids: OAuthClients) => invoke<void>("set_oauth_clients", { ids }),
  removeMailAccount: (id: string) => invoke<void>("remove_mail_account", { id }),
  syncMail: () => invoke<number>("sync_mail"),
  mailList: (query: string) => invoke<MailItem[]>("mail_list", { query, limit: 300 }),
  mailGet: (id: string) => invoke<MailItem>("mail_get", { id }),
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

export interface InstallProgress {
  model_id: string;
  phase: "engine" | "verify" | "download" | "benchmark" | "vision";
  received: number;
  total: number;
}

export interface InstallFinished {
  model_id: string;
  /** Set for speech models, which aren't in the chat catalog. */
  name?: string;
  ok: boolean;
  cancelled?: boolean;
  error?: string;
}

export interface ChatEvents {
  "chat:status": { chat_id: string; status: "loading" | "handoff" | "cooling"; detail?: string };
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
  "schedule:ran": { id: string; name: string; chat_id?: string; error?: string };
  "install:progress": InstallProgress;
  "install:finished": InstallFinished;
  "security:locked": null;
  "features:changed": Record<string, never>;
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
  "sync:changed": null;
  "update:progress": { received: number; total: number | null };
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

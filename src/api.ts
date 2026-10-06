// SPDX-License-Identifier: AGPL-3.0-only
// Typed wrappers around the Rust commands in src-tauri/src/lib.rs.
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
  fit: { ctx: number; variants: VariantFit[]; recommended: string | null };
  installed: InstalledModel | null;
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
  risk: "read" | "write" | "execute" | "connector";
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
}

export interface Segment {
  id: number;
  speaker: Speaker;
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

export interface Language {
  code: string;
  name: string;
}

/** The core's error text while app lock is engaged. */
export const LOCKED = "locked";

export const api = {
  appInfo: () => invoke<AppInfo>("app_info"),
  refreshHardware: () => invoke<Hardware>("refresh_hardware"),
  catalog: () => invoke<CatalogView>("catalog_view"),
  install: (modelId: string, quant: string) => invoke<void>("install_model", { modelId, quant }),
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
  send: (chatId: string, text: string) => invoke<Message | null>("send_message", { chatId, text }),
  stop: (chatId: string) => invoke<void>("stop_generation", { chatId }),
  engineStatus: () => invoke<EngineStatus>("engine_status"),
  unload: () => invoke<void>("unload_model"),
  finishOnboarding: () => invoke<Settings>("finish_onboarding"),
  security: () => invoke<SecurityStatus>("security_status"),
  enableLock: (pin: string) => invoke<string>("enable_lock", { pin }),
  changePin: (oldPin: string, newPin: string) => invoke<void>("change_pin", { oldPin, newPin }),
  resetPin: (newPin: string) => invoke<string>("reset_pin", { newPin }),
  disableLock: (pin: string) => invoke<void>("disable_lock", { pin }),
  unlock: (method: "pin" | "recovery", secret: string) => invoke<void>("unlock", { method, secret }),
  unlockWithHello: () => invoke<void>("unlock_with_hello"),
  lockNow: () => invoke<void>("lock_now"),
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
  | { meeting_id: string; kind: "done" }
  | { meeting_id: string; kind: "error"; error: string };

export interface InstallProgress {
  model_id: string;
  phase: "engine" | "verify" | "download" | "benchmark";
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
  "chat:status": { chat_id: string; status: "loading" | "handoff" };
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
  "schedule:ran": { id: string; name: string; chat_id?: string; error?: string };
  "install:progress": InstallProgress;
  "install:finished": InstallFinished;
  "security:locked": null;
  dictation: DictationEvent;
  voice: VoiceEvent;
  meeting: MeetingEvent;
}

export function on<K extends keyof ChatEvents>(name: K, handler: (payload: ChatEvents[K]) => void): Promise<UnlistenFn> {
  return listen<ChatEvents[K]>(name, (e) => handler(e.payload));
}

export function errorText(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  return String(e);
}

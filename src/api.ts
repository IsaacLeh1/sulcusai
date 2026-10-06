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
  risk: "read" | "write" | "execute";
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
};

export interface InstallProgress {
  model_id: string;
  phase: "engine" | "verify" | "download" | "benchmark";
  received: number;
  total: number;
}

export interface InstallFinished {
  model_id: string;
  ok: boolean;
  cancelled?: boolean;
  error?: string;
}

export interface ChatEvents {
  "chat:status": { chat_id: string; status: "loading" };
  "chat:context": { chat_id: string; context: ContextInfo };
  "chat:start": { chat_id: string; message_id: string };
  "chat:delta": { chat_id: string; message_id: string; content: string | null; thinking: string | null };
  "chat:done": { chat_id: string; message?: Message | null; tps?: number | null; context?: ContextInfo; cancelled?: boolean; error?: string };
  "agent:step": { chat_id: string };
  "agent:tool_start": { chat_id: string; call_id: string; tool: string };
  "agent:approval": PendingApproval;
  "agent:approval_done": { chat_id: string; call_id: string };
  "install:progress": InstallProgress;
  "install:finished": InstallFinished;
  "security:locked": null;
}

export function on<K extends keyof ChatEvents>(name: K, handler: (payload: ChatEvents[K]) => void): Promise<UnlistenFn> {
  return listen<ChatEvents[K]>(name, (e) => handler(e.payload));
}

export function errorText(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  return String(e);
}

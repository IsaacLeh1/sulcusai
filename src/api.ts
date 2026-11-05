// SPDX-License-Identifier: AGPL-3.0-only
// Typed wrappers around the Rust commands in src-tauri/src/lib.rs.
import { t } from "./i18n";
import type { FindHit } from "./api/files";
import { invoke } from "@tauri-apps/api/core";
import { emit, listen, type UnlistenFn } from "@tauri-apps/api/event";
import { type Advanced, type ApiServerView, type Assistant, type BackupInfo, type CacheItem, type CatalogView, type Chat, type ChatCloudSpend, type CleanView, type CloudChoice, type CloudModel, type CloudView, type Connectivity, type ContextOptions, type FinetuneJob, type FinetuneView, type FolderImport, type FolderView, type FoundModels, type Gpu, type Hardware, type HubLinkResult, type HubPurpose, type HubResults, type KeepLocks, type Lend, type LendOffer, type LendRuns, type ModelPreset, type NewPhone, type PausedDownload, type PcStatus, type PeerModels, type PeopleFirewall, type PeopleView, type PersonAllow, type PersonAsk, type PersonMessage, type PersonRequest, type PhoneView, type PracticeSkill, type PrivacySettings, type PrivacyTest, type PrivacyView, type Profile, type RunMode, type SavedPrompt, type Settings, type SharedHere, type SharedLive, type SharedView, type SharingChoice, type SidebarLayout, type Speed, type SpeedStatus, type StorageView, type SyncReport, type SyncView, type UncensorJob, type UncensorLook, type UpdateAvailable, type UpdateView, type PeerUpdates } from "./api/system";
import { type Action, type Allowance, type AppInfo, type AudioDevice, type BridgeStatus, type BrowserSearch, type BrowsersView, type BulkPlan, type CalAccount, type CalEvent, type CalInfo, type ChatHit, type Connector, type ContextInfo, type DesktopAccess, type EngineStatus, type EventDraft, type FeatureId, type FeatureView, type Folder, type GoBack, type GpuMenu, type HeatState, type InviteOutcome, type Language, type LiveReply, type MailAccount, type MailConfig, type MailDraft, type MailItem, type MainBrowser, type MediaItem, type Meeting, type MeetingDetail, type MeetingHit, type MeetingStatus, type Memory, type Message, type NewEvent, type Note, type NoteBody, type OAuthClients, type OAuthClientsView, type OAuthProvider, type PauseSettings, type PauseView, type PendingApproval, type PerfLimits, type PerfMode, type PerfSettings, type PerfView, type Plugin, type PluginPreview, type PrivacyMode, type Project, type QueueItem, type QuickSelection, type QuickSettings, type SavedSiteKey, type Schedule, type ScheduleView, type SecurityStatus, type Segment, type SentEntry, type SiteKeyAsk, type SitePreview, type SpeakerModel, type SpeechView, type Subscription, type Task, type ToolMode, type TriageReport, type UnsubscribeResult, type Versions, type ViewerArea, type Vocabulary, type Voice, type VoicePack, type VoiceSettings, type WebSettings, type ZeroView } from "./api/work";
import { type AddonView, type ArtistStyle, type CheckedRecording, type CloudMedia, type CodeReport, type ComposeAsk, type Composition, type ComputeSettings, type ComputeStatus, type CustomModel, type CustomModelsView, type DevModeView, type DocFile, type DocHit, type DocProgress, type HandoffsView, type HubMediaResults, type LibraryView, type LogsView, type MediaDefaults, type MediaJob, type MediaRequest, type MediaView, type ModelTuning, type MusicSpec, type OptimizeReport, type OwnVoicesView, type PcCheckReport, type Persona, type Plan, type Playlist, type PortsView, type PromptStatus, type ReviewSetup, type Screen, type SectionOp, type SongMap, type StudioAction, type StudioProject, type StudyRequest, type StudySet, type TalkWays, type Taste, type WallpaperSet } from "./api/media";
// Types only in these three. A function in one was bundled with the one view
// that used it, the bundle then loaded in a loop, and the window stayed blank.
export * from "./api/system";
export * from "./api/work";
export * from "./api/media";

/** The card models use: the one picked, else the dedicated card with the most memory (hardware.rs `main_of`). */
export function mainGpu(hw: Hardware): Gpu | undefined {
  const dedicated = hw.gpus.filter((g) => !g.integrated);
  return dedicated.find((g) => g.name === hw.main) ?? [...dedicated].sort((a, b) => b.vram_bytes - a.vram_bytes)[0];
}

/** Base64 of bytes (for sending files to the core). */
export function toBase64(bytes: Uint8Array): string {
  let s = "";
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(s);
}

export function fromBase64(text: string): Uint8Array {
  const s = atob(text);
  const out = new Uint8Array(s.length);
  for (let i = 0; i < s.length; i++) out[i] = s.charCodeAt(i);
  return out;
}

/** A Google Drive or OneDrive the assistant may read (cloudfiles.rs). */
export interface CloudAccount {
  id: string;
  provider: "google" | "microsoft";
  email: string;
}

/** Waking the assistant by saying a phrase (wake.rs). */
export interface WakeView {
  on: boolean;
  phrase: string;
  /** Listening for it now. */
  listening: boolean;
  problem: string | null;
}

/** Texting the assistant from Discord (discord.rs). */
export interface DiscordView {
  connected: boolean;
  bot: string | null;
  /** Adds the bot to one of the person's servers, so they can message it. */
  invite: string | null;
  linked_to: string | null;
  code: string | null;
  tasks: boolean;
  problem: string | null;
}

/** Texting the assistant from Matrix (matrix.rs). */
export interface MatrixView {
  connected: boolean;
  /** The assistant's own Matrix ID. */
  account: string | null;
  /** The Matrix ID it answers, once linked. */
  linked_to: string | null;
  code: string | null;
  tasks: boolean;
  problem: string | null;
}

/** Texting the assistant from Telegram (telegram.rs). */
export interface TelegramView {
  connected: boolean;
  /** The bot's @name. */
  bot: string | null;
  /** The account it answers (its first name), once linked. */
  linked_to: string | null;
  /** The code to send the bot, until an account has. */
  code: string | null;
  /** Scheduled tasks' results are sent too. */
  tasks: boolean;
  problem: string | null;
}

/** The core's error text while app lock is engaged. */
export const LOCKED = "locked";

/** Another machine running llama.cpp's rpc-server: models are split across it too. */
export interface LinkedMachine {
  addr: string;
  vram_bytes: number;
  bw_gbs: number;
}

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
  /** `folders`: where the person's code is, for the "project" skill. */
  startPractice: (name: string, base: string, skills: PracticeSkill[], epochs: number, strength: string, teacher: string | null, folders: string[] | null) =>
    invoke<void>("start_practice", { name, base, skills, epochs, strength, teacher, folders }),
  cancelFinetune: () => invoke<void>("cancel_finetune"),
  removeAdapter: (id: string) => invoke<void>("remove_adapter", { id }),
  /** `original`: a pasted Hugging Face link; null looks for the one the model was made from. */
  uncensorLook: (modelId: string, original: string | null) => invoke<UncensorLook>("uncensor_look", { modelId, original }),
  startUncensor: (modelId: string, original: string, thorough: boolean, pin: string | null) => invoke<void>("start_uncensor", { modelId, original, thorough, pin }),
  cancelUncensor: () => invoke<void>("cancel_uncensor"),
  uncensorJob: () => invoke<UncensorJob | null>("uncensor_job"),
  /** Deletes what stopped runs left; the bytes freed. */
  uncensorClear: () => invoke<number>("uncensor_clear"),
  updateView: () => invoke<UpdateView>("update_view"),
  checkForUpdate: () => invoke<UpdateAvailable | null>("check_for_update"),
  setUpdateAuto: (auto: boolean) => invoke<void>("set_update_auto", { auto }),
  setUpdateInstall: (install: boolean) => invoke<void>("set_update_install", { install }),
  installUpdate: () => invoke<void>("install_update"),
  peerUpdatesView: () => invoke<PeerUpdates>("peer_updates_view"),
  setPeerUpdates: (s: PeerUpdates) => invoke<void>("set_peer_updates", { s }),
  /** Fetches, checks and runs the update another PC holds; the app closes. */
  installUpdateFromPeer: (peer: string) => invoke<string>("install_update_from_peer", { peer }),
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
  setPhoneLink: (anywhere: boolean) => invoke<PhoneView>("set_phone_link", { anywhere }),
  setPhoneWorker: (url: string) => invoke<PhoneView>("set_phone_worker", { url }),
  setPhoneOwnRelay: (on: boolean, address: string, page: string) => invoke<PhoneView>("set_phone_own_relay", { on, address, page }),
  pcStatus: () => invoke<PcStatus>("pc_status"),
  stopAll: () => invoke<number>("stop_all"),
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
  /** People in a window of its own (brought forward if it's open). */
  openPeopleWindow: () => invoke<void>("open_people_window"),
  /** A chat in a window of its own (brought forward when it's open). */
  openChatWindow: (chatId: string) => invoke<void>("open_chat_window", { chatId }),
  /** The Studio in a new window of its own. */
  openStudioWindow: () => invoke<void>("open_studio_window"),
  /** A song studio project in a window of its own; with `onlyIfOpen`, only brought forward (true when it was open). */
  openSongStudioWindow: (projectId: string, onlyIfOpen = false) => invoke<boolean>("open_song_studio_window", { projectId, onlyIfOpen }),
  /** The main window brought forward on a page. */
  showMainPage: (page: string) => invoke<void>("show_main_page", { page }),
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
  peopleEdit: (id: string, message: string, text: string) => invoke<void>("people_edit", { id, message, text }),
  peopleDelete: (id: string, message: string) => invoke<void>("people_delete", { id, message }),
  /** Asks their assistant directly ("Also ask {name}'s assistant"). */
  peopleAsk: (id: string, question: string) => invoke<PersonAsk>("people_ask", { id, question }),
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
  peopleSharedPost: (contactId: string, chatId: string, text: string) => invoke<void>("people_shared_post", { contactId, chatId, text, model: null }),
  /** The model that answers what this user writes in someone's shared chat (null: the default). */
  peopleSharedModel: (contactId: string, chatId: string) => invoke<string | null>("people_shared_model", { contactId, chatId }),
  peopleSetSharedModel: (contactId: string, chatId: string, model: string | null) => invoke<void>("people_set_shared_model", { contactId, chatId, model }),
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
  /** Names someone for this user only; "" goes back to their own name. */
  peopleRename: (id: string, name: string) => invoke<void>("people_rename", { id, name }),
  /** What someone used this PC for. */
  peopleRuns: (contactId: string) => invoke<LendRuns>("people_runs", { contactId }),
  peopleSetLend: (contactId: string, lend: Lend) => invoke<void>("people_set_lend", { contactId, lend }),
  /** The models someone lends this user (while connected). */
  peopleLent: (contactId: string) => invoke<LendOffer>("people_lent", { contactId }),
  addCloudProvider: (preset: string | null, name: string | null, baseUrl: string | null) => invoke<CloudView>("add_cloud_provider", { preset, name, baseUrl }),
  removeCloudProvider: (id: string) => invoke<CloudView>("remove_cloud_provider", { id }),
  setCloudProvider: (id: string, enabled: boolean, key: string | null) => invoke<CloudView>("set_cloud_provider", { id, enabled, key }),
  cloudModelsAvailable: (id: string) => invoke<CloudModel[]>("cloud_models_available", { id }),
  setCloudMedia: (id: string, media: CloudMedia[]) => invoke<CloudView>("set_cloud_media", { id, media }),
  setCloudModels: (id: string, models: CloudModel[]) => invoke<CloudView>("set_cloud_models", { id, models }),
  setCloudOptions: (budget: number | null, redact: boolean) => invoke<CloudView>("set_cloud_options", { budget, redact }),
  /** Where a provider's chats go: this user's other PCs with the same key, and people. */
  setCloudSharing: (id: string, sharePcs: boolean, sharePeople: string[]) => invoke<CloudView>("set_cloud_sharing", { id, sharePcs, sharePeople }),
  /** Sends one provider's requests through a proxy ("" for directly). */
  setCloudProxy: (id: string, proxy: string) => invoke<CloudView>("set_cloud_proxy", { id, proxy }),
  cloudChoices: () => invoke<CloudChoice[]>("cloud_choices"),
  sidebarLayout: () => invoke<SidebarLayout>("sidebar_layout"),
  /** A chat's notes: its memory and plan (Markdown, "- [ ] step" lines). */
  chatNotes: (chatId: string) => invoke<string>("chat_notes", { chatId }),
  setChatNotes: (chatId: string, text: string) => invoke<string>("set_chat_notes", { chatId, text }),
  setSidebarLayout: (layout: SidebarLayout) => invoke<SidebarLayout>("set_sidebar_layout", { layout }),
  /** Opens a server in the built-in browser to sign in; its session is used once signed in (`cloud:signed_in`). */
  cloudSignIn: (id: string) => invoke<string>("cloud_sign_in", { id }),
  chatCloudSpend: (chatId: string) => invoke<ChatCloudSpend>("chat_cloud_spend", { chatId }),
  setApiServer: (enabled: boolean, port: number) => invoke<ApiServerView>("set_api_server", { enabled, port }),
  newApiKey: () => invoke<ApiServerView>("new_api_key"),
  setOllamaServer: (enabled: boolean, port: number) => invoke<ApiServerView>("set_ollama_server", { enabled, port }),
  setOllamaHome: (enabled: boolean) => invoke<ApiServerView>("set_ollama_home", { enabled }),
  importModelFile: (path: string) => invoke<string>("import_model_file", { path }),
  importModelFolder: (path: string) => invoke<FolderImport>("import_model_folder", { path }),
  renameModel: (modelId: string, name: string) => invoke<void>("rename_model", { modelId, name }),
  /** Summarizes the chat so far; the model continues from the summary. */
  compactChat: (chatId: string) => invoke<void>("compact_chat", { chatId }),
  contextOptions: (modelId: string | null) => invoke<ContextOptions>("context_options", { modelId }),
  /** null: automatic. */
  setContextWindow: (size: number | null) => invoke<void>("set_context_window", { size }),
  hubSearch: (query: string, sort: "downloads" | "trending" | "newest", all = false, purpose: HubPurpose = "") => invoke<HubResults>("hub_search", { query, sort, all, purpose }),
  /** Starts installing one file; returns the model id its progress is reported under. */
  hubInstall: (repo: string, file: string, force = false) => invoke<string>("hub_install", { repo, file, force }),
  hubLink: (link: string) => invoke<HubLinkResult>("hub_link", { link }),
  /** Tries a few setups on this PC's graphics card and keeps the fastest. */
  optimizeModel: (modelId: string) => invoke<OptimizeReport>("optimize_model", { modelId }),
  modelTunings: () => invoke<Record<string, ModelTuning>>("model_tunings"),
  forgetOptimization: (modelId: string) => invoke<void>("forget_optimization", { modelId }),
  measureModelSpeed: (modelId: string) => invoke<Speed>("measure_model_speed", { modelId }),
  // Tests every installed chat model; progress comes as models:speed.
  speedTestStatus: () => invoke<SpeedStatus>("speed_test_status"),
  measureAllSpeeds: () => invoke<number>("measure_all_speeds"),
  refreshHardware: () => invoke<Hardware>("refresh_hardware"),
  /** The models this PC can run, or with `all` every one. */
  catalog: (all = false) => invoke<CatalogView>("catalog_view", { all }),
  /** `force`: install even though this PC probably can't run it (the person was warned). */
  install: (modelId: string, quant: string, force = false) => invoke<void>("install_model", { modelId, quant, force }),
  /** Downloads a damaged model's file again. */
  repairModel: (modelId: string) => invoke<void>("repair_model", { modelId }),
  fasterModel: (modelId: string) => invoke<void>("faster_model", { modelId }),
  /** A ChatGPT or Claude export brought in (import.rs). */
  importChats: (path: string) => invoke<{ from: string; chats: number; messages: number; memories: number; already: number }>("import_chats", { path }),
  importMemories: (text: string) => invoke<number>("import_memories", { text }),
  mediaSaveCaptions: (id: string, path: string) => invoke<void>("media_save_captions", { id, path }),
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
  crashPing: (visible = true) => invoke<void>("crash_ping", { visible }),
  windowRest: () => invoke<void>("window_rest"),
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
  managerGet: (chatId: string) => invoke<ManagerSettings>("manager_get", { chatId }),
  managerSet: (chatId: string, settings: ManagerSettings) => invoke<void>("manager_set", { chatId, settings }),
  managerPlan: (chatId: string) => invoke<ManagerPlan>("manager_plan", { chatId }),
  managerAnswer: (chatId: string, answer: string) => invoke<string>("manager_answer", { chatId, answer }),
  managerRedo: (chatId: string, decision: string, choice: string) => invoke<void>("manager_redo", { chatId, decision, choice }),
  setChatModel: (chatId: string, modelId: string) => invoke<void>("set_chat_model", { chatId, modelId }),
  messages: (chatId: string) => invoke<Message[]>("get_messages", { chatId }),
  context: (chatId: string) => invoke<ContextInfo | null>("get_context", { chatId }),
  send: (chatId: string, text: string, images?: string[]) => invoke<Message | null>("send_message", { chatId, text, images: images ?? null }),
  /** Another reply to the person's message; the one it had is kept as a version. */
  regenerate: (chatId: string, messageId: string) => invoke<Message | null>("chat_regenerate", { chatId, messageId }),
  /** The person's message sent again as `text`; the old one and what followed are kept as a version. */
  editMessage: (chatId: string, messageId: string, text: string) => invoke<Message | null>("chat_edit", { chatId, messageId, text }),
  versions: (chatId: string) => invoke<Versions[]>("chat_versions", { chatId }),
  searchChats: (query: string) => invoke<ChatHit[]>("search_chats", { query }),
  assistants: () => invoke<Assistant[]>("assistants"),
  saveAssistant: (assistant: Assistant) => invoke<Assistant[]>("save_assistant", { assistant }),
  deleteAssistant: (id: string) => invoke<Assistant[]>("delete_assistant", { id }),
  chatWithAssistant: (id: string) => invoke<Chat>("chat_with_assistant", { id }),
  chatAssistant: (chatId: string) => invoke<Assistant | null>("chat_assistant", { chatId }),
  savedPrompts: () => invoke<SavedPrompt[]>("saved_prompts"),
  savePrompt: (id: string, name: string, text: string) => invoke<SavedPrompt[]>("save_prompt", { id, name, text }),
  deletePrompt: (id: string) => invoke<SavedPrompt[]>("delete_prompt", { id }),
  modelPreset: (modelId: string) => invoke<ModelPreset>("model_preset", { modelId }),
  compareModels: (run: string, models: string[], prompt: string) => invoke<void>("compare_models", { run, models, prompt }),
  compareStop: (run: string) => invoke<void>("compare_stop", { run }),
  compareMerge: (prompt: string, a: string, b: string) => invoke<string>("compare_merge", { prompt, a, b }),
  setModelPreset: (modelId: string, preset: ModelPreset) => invoke<void>("set_model_preset", { modelId, preset }),
  /** Saves a chat as .md (written by the core) or .html (`html`, the page the window made). */
  exportChat: (chatId: string, path: string, html?: string) => invoke<void>("export_chat", { chatId, path, html: html ?? null }),
  switchVersion: (chatId: string, anchor: string, to: number) => invoke<void>("chat_switch_version", { chatId, anchor, to }),
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
  /** The strip shown while the assistant works another program. */
  programsBanner: () => invoke<{ chat_id: string; what: string } | null>("programs_banner"),
  programsBannerStop: () => invoke<void>("programs_banner_stop"),
  lensWords: (modelId: string, prompt: string, given: string) => invoke<LensWords>("lens_words", { modelId, prompt, given, top: 5, most: 300 }),
  lensLayers: (modelId: string, text: string) => invoke<LensLayers>("lens_layers", { modelId, text }),
  lensEdit: (modelId: string, edits: LensEdit[]) => invoke<string>("lens_edit", { modelId, edits }),
  lensDirections: () => invoke<LensDirection[]>("lens_directions"),
  lensMakeDirection: (modelId: string, name: string, toward: string, away: string) => invoke<LensDirection>("lens_make_direction", { modelId, name, toward, away }),
  lensSetDirection: (id: string, strength: number, from?: number, to?: number) => invoke<LensDirection>("lens_set_direction", { id, strength, from: from ?? null, to: to ?? null }),
  lensDeleteDirection: (id: string) => invoke<void>("lens_delete_direction", { id }),
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
  protectChat: (chatId: string) => invoke<void>("protect_chat", { chatId }),
  openProtectedChat: (chatId: string, pin: string, unprotect = false) => invoke<void>("open_protected_chat", { chatId, pin, unprotect }),
  openProtectedChatHello: (chatId: string, unprotect = false) => invoke<void>("open_protected_chat_hello", { chatId, unprotect }),
  shutProtectedChat: (chatId: string) => invoke<void>("shut_protected_chat", { chatId }),
  cloudFiles: () => invoke<CloudAccount[]>("cloud_files"),
  cloudFilesConnect: (provider: "google" | "microsoft") => invoke<CloudAccount[]>("cloud_files_connect", { provider }),
  cloudFilesRemove: (id: string) => invoke<CloudAccount[]>("cloud_files_remove", { id }),
  wakeWord: () => invoke<WakeView>("wake_word"),
  wakeForTasks: () => invoke<boolean>("wake_for_tasks"),
  setWakeForTasks: (on: boolean) => invoke<boolean>("set_wake_for_tasks", { on }),
  liveVoiceStart: (voiceId: string, semitones: number) => invoke<void>("live_voice_start", { voiceId, semitones }),
  liveVoiceStop: () => invoke<void>("live_voice_stop"),
  setWakeWord: (on: boolean, phrase: string) => invoke<WakeView>("set_wake_word", { on, phrase }),
  telegramView: () => invoke<TelegramView>("telegram_view"),
  telegramConnect: (token: string) => invoke<TelegramView>("telegram_connect", { token }),
  telegramSetTasks: (on: boolean) => invoke<TelegramView>("telegram_set_tasks", { on }),
  telegramDisconnect: () => invoke<void>("telegram_disconnect"),
  matrixView: () => invoke<MatrixView>("matrix_view"),
  matrixConnect: (homeserver: string, token: string) => invoke<MatrixView>("matrix_connect", { homeserver, token }),
  matrixSetTasks: (on: boolean) => invoke<MatrixView>("matrix_set_tasks", { on }),
  matrixDisconnect: () => invoke<void>("matrix_disconnect"),
  discordView: () => invoke<DiscordView>("discord_view"),
  discordConnect: (token: string) => invoke<DiscordView>("discord_connect", { token }),
  discordSetTasks: (on: boolean) => invoke<DiscordView>("discord_set_tasks", { on }),
  discordDisconnect: () => invoke<void>("discord_disconnect"),
  /** The activity log, newest first; `since` in ms limits it to a period. */
  actions: (limit?: number, since?: number) => invoke<Action[]>("list_actions", { limit, since: since ?? null }),
  clearActions: () => invoke<void>("clear_actions"),
  /** The folders a chat can use, or with no chat, the ones every chat can. */
  folders: (chatId: string | null = null) => invoke<Folder[]>("list_folders", { chatId }),
  /** Shares a folder in one chat, or with no chat, in every chat. */
  addFolder: (path: string, allowSystem = false, chatId: string | null = null) => invoke<void>("add_folder", { path, allowSystem, chatId }),
  /** Lets every chat use a folder shared in this chat, or keeps one to this chat. */
  setFolderEveryChat: (path: string, chatId: string, on: boolean) => invoke<void>("set_folder_every_chat", { path, chatId, on }),
  /** The assistant's folder request: share it with every chat, not only the asking one. */
  folderForEveryChat: (callId: string, on: boolean) => invoke<void>("folder_for_every_chat", { callId, on }),
  /** The warning to confirm before sharing this folder, if it needs one. */
  folderWarning: (path: string) => invoke<string | null>("folder_warning", { path }),
  /** Stops sharing a folder: in this chat when it was shared there, else in every chat. */
  removeFolder: (path: string, chatId: string | null = null) => invoke<void>("remove_folder", { path, chatId }),
  /** Picks another folder for the assistant's folder request. */
  chooseFolder: (callId: string, path: string) => invoke<void>("choose_folder", { callId, path }),
  saveChart: (path: string, svg: string) => invoke<void>("save_chart", { path, svg }),
  /** Chats hidden from the sidebar: own chats by id, shared ones as "contact:chat". */
  hiddenChats: () => invoke<string[]>("hidden_chats"),
  setChatHidden: (key: string, hidden: boolean) => invoke<string[]>("set_chat_hidden", { key, hidden }),
  siteKeyAsk: (callId: string) => invoke<SiteKeyAsk>("site_key_ask", { callId }),
  siteKeyGive: (callId: string, key: string) => invoke<void>("site_key_give", { callId, key }),
  privacyMode: () => invoke<PrivacyMode>("privacy_mode"),
  setPrivacyMode: (mode: PrivacyMode) => invoke<void>("set_privacy_mode", { mode }),
  /** The privacy popup's changes: a new value, or null to leave a part out. */
  privacyShare: (callId: string, edits: { key: string; value: string | null }[]) => invoke<void>("privacy_share", { callId, edits }),
  privacySent: () => invoke<SentEntry[]>("privacy_sent"),
  privacySentClear: () => invoke<void>("privacy_sent_clear"),
  /** Takes chosen items out of "What left this PC". */
  privacySentDelete: (ids: string[]) => invoke<void>("privacy_sent_delete", { ids }),
  siteKeys: () => invoke<SavedSiteKey[]>("site_keys"),
  siteKeyForget: (host: string) => invoke<void>("site_key_forget", { host }),
  desktopAccess: () => invoke<DesktopAccess>("desktop_access"),
  setDesktopAccess: (on: boolean) => invoke<DesktopAccess>("set_desktop_access", { on }),
  setChatMode: (chatId: string, mode: RunMode) => invoke<void>("set_chat_mode", { chatId, mode }),
  answerApproval: (callId: string, decision: "allow" | "always" | "deny") => invoke<void>("answer_approval", { callId, decision }),
  pendingApprovals: (chatId: string) => invoke<PendingApproval[]>("pending_approvals", { chatId }),
  /** Every approval waiting for an answer, in all chats. */
  allPendingApprovals: () => invoke<PendingApproval[]>("all_pending_approvals"),
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
  setMemoryCategory: (id: string, category: string) => invoke<void>("set_memory_category", { id, category }),
  memorySensitive: () => invoke<boolean>("memory_sensitive"),
  setMemorySensitive: (on: boolean) => invoke<void>("set_memory_sensitive", { on }),
  /** `chatId`: added from a chat, so it shows with that chat's memories. */
  addMemory: (content: string, projectId: string | null, chatId: string | null = null) => invoke<Memory>("add_memory", { content, projectId, chatId }),
  updateMemory: (id: string, content: string) => invoke<void>("update_memory", { id, content }),
  /** Where a memory applies: everywhere (null) or one project. */
  setMemoryScope: (id: string, projectId: string | null) => invoke<void>("set_memory_scope", { id, projectId }),
  memoryAuto: () => invoke<boolean>("memory_auto"),
  setMemoryAuto: (on: boolean) => invoke<void>("set_memory_auto", { on }),
  /** Chats linked with this one to work together. */
  linkedChats: (chatId: string) => invoke<Chat[]>("linked_chats", { chatId }),
  linkChats: (a: string, b: string) => invoke<void>("link_chats", { a, b }),
  unlinkChats: (a: string, b: string) => invoke<void>("unlink_chats", { a, b }),
  quickSettings: () => invoke<QuickSettings>("quick_settings"),
  setQuickHotkey: (hotkey: string) => invoke<QuickSettings>("set_quick_hotkey", { hotkey }),
  setQuickModel: (modelId: string | null) => invoke<void>("set_quick_model", { modelId }),
  setDictateAnywhere: (on: boolean) => invoke<QuickSettings>("set_dictate_anywhere", { on }),
  quickSelection: () => invoke<QuickSelection | null>("quick_selection"),
  quickReplace: (text: string) => invoke<void>("quick_replace", { text }),
  /** A new chat for the Quick ask box, on its model. */
  quickChat: () => invoke<Chat>("quick_chat"),
  deleteMemory: (id: string) => invoke<void>("delete_memory", { id }),
  clearMemories: () => invoke<void>("clear_memories"),
  setMemoryEnabled: (enabled: boolean) => invoke<Settings>("set_memory_enabled", { enabled }),
  setAutoModel: (on: boolean) => invoke<Settings>("set_auto_model", { on }),
  schedules: () => invoke<ScheduleView[]>("list_schedules"),
  saveSchedule: (schedule: Schedule) => invoke<ScheduleView>("save_schedule", { schedule }),
  deleteSchedule: (id: string) => invoke<void>("delete_schedule", { id }),
  runScheduleNow: (id: string) => invoke<string>("run_schedule_now", { id }),
  connectors: () => invoke<Connector[]>("list_connectors"),
  saveConnector: (input: { id: string; name: string; command: string; args: string[]; env: Record<string, string>; url?: string; headers?: Record<string, string> }) =>
    invoke<Connector>("save_connector", { input }),
  deleteConnector: (id: string) => invoke<void>("delete_connector", { id }),
  setConnectorEnabled: (id: string, enabled: boolean) => invoke<void>("set_connector_enabled", { id, enabled }),
  setToolMode: (id: string, tool: string, mode: ToolMode) => invoke<void>("set_tool_mode", { id, tool, mode }),
  goBackPreview: (chatId: string, messageId: string) => invoke<GoBack>("go_back_preview_cmd", { chatId, messageId }),
  goBack: (chatId: string, messageId: string, files: boolean) => invoke<{ text: string; notes: string[] }>("go_back_cmd", { chatId, messageId, files }),
  connectorSignIn: (id: string) => invoke<Connector>("connector_sign_in", { id }),
  connectorSignOut: (id: string) => invoke<Connector>("connector_sign_out", { id }),
  plugins: () => invoke<Plugin[]>("list_plugins"),
  inspectPlugin: (path: string) => invoke<PluginPreview>("inspect_plugin", { path }),
  installPlugin: (path: string, allowChecks = false) => invoke<Plugin>("install_plugin", { path, allowChecks }),
  pluginCheckProblems: () => invoke<Record<string, string>>("plugin_check_problems"),
  setPluginEnabled: (id: string, enabled: boolean) => invoke<void>("set_plugin_enabled", { id, enabled }),
  removePlugin: (id: string) => invoke<void>("remove_plugin", { id }),
  allowances: () => invoke<Allowance[]>("list_allowances"),
  saveAllowance: (allowance: Omit<Allowance, "id" | "created_at"> & { id?: string }) => invoke<Allowance>("save_allowance", { allowance }),
  removeAllowance: (id: string) => invoke<void>("remove_allowance", { id }),
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
  typingVocabulary: () => invoke<Vocabulary>("typing_vocabulary"),
  /** Windows' spell checker's sure fix for a finished word, or null. */
  typingFix: (word: string, lang: string, known: boolean) => invoke<string | null>("typing_fix", { word, lang, known }),
  typingCanCorrect: (lang: string) => invoke<boolean>("typing_can_correct", { lang }),
  /** The message the person will most likely send next in a chat (the local model's guess), or null. */
  typingNext: (chatId: string) => invoke<string | null>("typing_next", { chatId }),
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
  pairExtension: (code: string) => invoke<void>("bridge_pair", { code }),
  browserSites: () => invoke<string[]>("browser_sites"),
  removeBrowserSite: (host: string) => invoke<void>("browser_site_remove", { host }),
  setMainBrowser: (choice: MainBrowser) => invoke<void>("set_main_browser", { choice }),
  setBrowserSearch: (search: BrowserSearch) => invoke<void>("set_browser_search", { search }),
  /** Opens a web address (or just the browser) in the user's main browser. */
  openWeb: (url: string | null) => invoke<void>("open_web", { url }),
  /** Opens a web address in a browser outside SulcusAI (Chrome unless another was picked). */
  openOutside: (url: string) => invoke<void>("open_outside", { url }),
  /** The web viewer over `area` (window pixels), at `url` when given. */
  /** Viewer commands take a chat's id (its own viewer), or null for the Web page's. */
  viewerShow: (key: string | null, url: string | null, area: ViewerArea) => invoke<void>("viewer_show", { key, url, area }),
  viewerArea: (key: string | null, area: ViewerArea) => invoke<void>("viewer_area", { key, area }),
  viewerHide: (key: string | null) => invoke<void>("viewer_hide", { key }),
  /** "back", "forward", "reload", or what was typed in the address bar. */
  viewerGo: (key: string | null, to: string) => invoke<void>("viewer_go", { key, to }),
  viewerUrl: (key: string | null) => invoke<string | null>("viewer_url", { key }),
  viewerExists: (key: string | null) => invoke<boolean>("viewer_exists", { key }),
  viewerClose: (key: string | null) => invoke<void>("viewer_close", { key }),
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
  /** The known problems in a log's last lines, and what each means. */
  logMeanings: (name: string, lines?: number) => invoke<{ line: string; meaning: string }[]>("log_meanings", { name, lines }),
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
  quickFind: (q: string) => invoke<FindHit[]>("quick_find", { q }),
  quickOpen: (path: string, reveal: boolean) => invoke<void>("quick_open", { path, reveal }),
  quickHide: () => invoke<void>("quick_hide"),
  quickResize: (height: number) => invoke<void>("quick_resize", { height }),
  quickOpenInApp: (chatId: string | null) => invoke<void>("quick_open_in_app", { chatId }),
  quickScreenshot: () => invoke<MediaItem>("quick_screenshot"),
  /** The Studio's models; with all, also the ones this PC can't run (with why). */
  mediaView: (all = false) => invoke<MediaView>("media_view", { all }),
  installMedia: (modelId: string, anyway?: boolean) => invoke<void>("install_media_model", { modelId, anyway: anyway ?? null }),
  mediaHubSearch: (kind: "image" | "video" | "music", query: string, sort: "downloads" | "trending" | "newest", all = false) =>
    invoke<HubMediaResults>("media_hub_search", { kind, query, sort, all }),
  mediaHubInstall: (repo: string, file: string, anyway?: boolean) => invoke<string>("media_hub_install", { repo, file, anyway: anyway ?? null }),
  mediaSetDefault: (kind: "image" | "video" | "music", modelId: string | null) => invoke<MediaDefaults>("media_set_default", { kind, modelId }),
  removeMedia: (modelId: string) => invoke<number>("remove_media_model", { modelId }),
  mediaStart: (request: MediaRequest) => invoke<string>("media_start", { request }),
  mediaCancel: (jobId: string) => invoke<void>("media_cancel", { jobId }),
  mediaPromptStatus: (modelId: string) => invoke<PromptStatus>("media_prompt_status", { modelId }),
  mediaForgetTuning: (modelId: string) => invoke<void>("media_forget_tuning", { modelId }),
  mediaSetLearning: (on: boolean) => invoke<void>("media_set_learning", { on }),
  /** Questions worth asking before a new picture or a change (usually none). */
  mediaQuestions: (op: "generate" | "edit", prompt: string, source: string | null, modelId: string | null, examples = 0) =>
    invoke<string[]>("media_questions", { op, prompt, source, modelId, examples }),
  /** The request with the answers to its questions folded in. */
  mediaWithAnswers: (prompt: string, answers: [string, string][]) => invoke<string>("media_with_answers", { prompt, answers }),
  /** Whether chats ask before a picture too (the Studio's checkbox). */
  mediaSetAskQuestions: (on: boolean) => invoke<void>("media_set_ask_questions", { on }),
  wallpaperScreens: () => invoke<Screen[]>("wallpaper_screens"),
  wallpaperSet: (id: string, x: number | null, y: number | null) => invoke<WallpaperSet>("wallpaper_set", { id, x, y }),
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
  /** A thumbs down (or taking it back). */
  mediaDislike: (id: string, on: boolean) => invoke<MediaItem>("media_dislike", { id, on }),
  /** Names a song (or anything in the gallery); empty goes back to none. */
  mediaSetTitle: (id: string, title: string) => invoke<MediaItem>("media_set_title", { id, title }),  mediaExport: (id: string, path: string) => invoke<void>("media_export", { id, path }),
  mediaImport: (path: string, chatId: string | null = null) => invoke<MediaItem>("media_import", { path, chatId }),
  musicWriteLyrics: (prompt: string, music: MusicSpec | null, seconds: number | null, language: string | null) =>
    invoke<string>("music_write_lyrics", { prompt, music, seconds, language }),
  /** Some lines of the lyrics written again by the chat model, changed as `how` says; returns those lines. */
  musicRewriteLines: (whole: string, part: string, how: string, language: string | null) =>
    invoke<string>("music_rewrite_lines", { whole, part, how, language }),
  musicArtistStyle: (name: string) => invoke<ArtistStyle>("music_artist_style", { name }),
  musicPersonas: () => invoke<Persona[]>("music_personas"),
  musicPlaylists: () => invoke<Playlist[]>("music_playlists"),
  /** Saves a playlist (a new one when id is empty): its name and songs in order. */
  musicPlaylistSave: (playlist: Pick<Playlist, "id" | "name" | "songs">) => invoke<Playlist>("music_playlist_save", { playlist }),
  musicPlaylistDelete: (id: string) => invoke<void>("music_playlist_delete", { id }),
  /** What the person likes in songs, from their liked, made and "not for me" songs. */
  musicTaste: (ids?: string[]) => invoke<Taste>("music_taste", { ids: ids ?? null }),
  musicCustomModels: () => invoke<CustomModelsView>("music_custom_models"),
  mediaTalkWays: () => invoke<TalkWays>("media_talk_ways"),
  /** The Studio's projects, the last changed first. */
  studioProjects: () => invoke<StudioProject[]>("studio_projects"),
  studioProjectSave: (project: StudioProject) => invoke<void>("studio_project_save", { project }),
  studioProjectDelete: (id: string) => invoke<void>("studio_project_delete", { id }),
  /** Starts recording from the microphone (its level comes as "studio:level"). */
  studioRecordStart: () => invoke<void>("studio_record_start"),
  /** Stops recording: the take, kept out of the gallery. */
  studioRecordStop: () => invoke<MediaItem>("studio_record_stop"),
  studioRecordCancel: () => invoke<void>("studio_record_cancel"),
  /** The song studio's chat bar: a request turned into actions by the chat model. */
  studioAsk: (request: string, summary: string) => invoke<StudioAction[]>("studio_ask", { request, summary }),
  /** A MIDI file the person picked, as base64. */
  studioReadMidi: (path: string) => invoke<string>("studio_read_midi", { path }),
  /** A sound file the person picked (for a wavetable), as base64. */
  studioReadSound: (path: string) => invoke<string>("studio_read_sound", { path }),
  /** What the song studio keeps by name: the synth's sounds, or effects built from words or saved. */
  studioLibrary: (kind: "synth" | "effects") => invoke<unknown[]>("studio_library", { kind }),
  studioLibrarySave: (kind: "synth" | "effects", item: unknown) => invoke<void>("studio_library_save", { kind, item }),
  studioLibraryDelete: (kind: "synth" | "effects", name: string) => invoke<void>("studio_library_delete", { kind, name }),
  /** A MIDI file the window wrote, saved where the person picked (a .mid path). */
  studioSaveMidi: (path: string, data: string) => invoke<void>("studio_save_midi", { path, data }),
  /** A sound's notes (Basic Pitch), or with `drums` its kick, snare and hat hits. */
  studioMidi: (id: string, drums: boolean) => invoke<{ pitch: number; start: number; end: number; velocity: number }[]>("studio_midi", { id, drums }),
  /** Forgets a custom model and deletes it from this PC. */
  musicCustomModelDelete: (id: string) => invoke<void>("music_custom_model_delete", { id }),
  /** Saves an artist (a new one when id is empty). */
  musicPersonaSave: (persona: Persona) => invoke<Persona>("music_persona_save", { persona }),
  musicPersonaDelete: (id: string) => invoke<void>("music_persona_delete", { id }),
  ownVoices: () => invoke<OwnVoicesView>("own_voices"),
  /** `agreed`: it's the person's own voice, or its owner said yes. */
  ownVoiceCreate: (name: string, agreed: boolean) => invoke<OwnVoicesView>("own_voice_create", { name, agreed }),
  ownVoiceRename: (id: string, name: string) => invoke<OwnVoicesView>("own_voice_rename", { id, name }),
  ownVoiceDelete: (id: string) => invoke<OwnVoicesView>("own_voice_delete", { id }),
  /** Starts recording a line ("own_voice:level" while it records, "own_voice:full" at its longest). */
  ownVoiceRecord: (voice: string, line: string) => invoke<void>("own_voice_record", { voice, line }),
  /** Keeps the take; `words`: the line as shown, used when speech recognition isn't installed. */
  ownVoiceStop: (words: string) => invoke<OwnVoicesView>("own_voice_stop", { words }),
  ownVoiceCancel: () => invoke<void>("own_voice_cancel"),
  /** A take as WAV, base64. */
  ownVoiceTake: (voice: string, line: string) => invoke<string>("own_voice_take", { voice, line }),
  ownVoiceTakeDelete: (voice: string, line: string) => invoke<OwnVoicesView>("own_voice_take_delete", { voice, line }),
  /** Adds recordings the person has (songs they sang, or speaking): the clearest 15 s of each, a song's voice taken out of its music. */
  ownVoiceUpload: (voice: string, paths: string[], singing: boolean) => invoke<OwnVoicesView & { problems: string[] }>("own_voice_upload", { voice, paths, singing }),
  /** A sentence read in the voice, to hear it (WAV, base64); the first time downloads the voice engine. */
  ownVoiceTry: (voice: string, text: string, language: string | null) => invoke<string>("own_voice_try", { voice, text, language }),
  /** Three voices made from a description, each reading the same sample (WAV, base64). */
  ownVoiceDesign: (description: string, language: string | null) => invoke<{ wav: string; seconds: number }[]>("own_voice_design", { description, language }),
  ownVoiceDesignCancel: () => invoke<void>("own_voice_design_cancel"),
  /** Rates recordings for a voice made from them alone ("own_voice:adding" while it reads them); nothing is kept yet. */
  ownVoiceCheck: (paths: string[], singing: boolean) => invoke<CheckedRecording[]>("own_voice_check", { paths, singing }),
  /** A new voice from checked recordings (their tokens). */
  /** What was kept of a checked recording (WAV, base64). */
  ownVoiceCheckedTake: (token: string) => invoke<string>("own_voice_checked_take", { token }),
  ownVoiceCreateFrom: (name: string, agreed: boolean, tokens: string[]) => invoke<OwnVoicesView>("own_voice_create_from", { name, agreed, tokens }),
  // The documents library.
  docsLibrary: () => invoke<LibraryView>("docs_library"),
  docsAdd: (paths: string[]) => invoke<LibraryView>("docs_add", { paths }),
  docsRemove: (id: string) => invoke<LibraryView>("docs_remove", { id }),
  docsUpdate: () => invoke<LibraryView>("docs_update"),
  docsStop: () => invoke<void>("docs_stop"),
  docsFiles: (source: string) => invoke<DocFile[]>("docs_files", { source }),
  docsSearch: (query: string, limit?: number) => invoke<DocHit[]>("docs_search", { query, limit: limit ?? null }),
  /** Opens a library file in its app. */
  docsOpen: (id: string) => invoke<void>("docs_open", { id }),
  // The compute PC.
  computeSettings: () => invoke<{ settings: ComputeSettings; peers: [string, string][] }>("compute_settings"),
  computeSet: (settings: ComputeSettings) => invoke<void>("compute_set", { settings }),
  computeStatus: () => invoke<ComputeStatus>("compute_status"),
  computeControl: (action: string, value: string | null) => invoke<string>("compute_control", { action, value }),
  // Code reviews ("review:progress" while one runs).
  reviewSetup: () => invoke<ReviewSetup>("review_setup"),
  reviews: () => invoke<CodeReport[]>("reviews"),
  reviewStart: (request: { source: string; checks: string[]; notes: string; model: string | null }) => invoke<CodeReport>("review_start", { request }),
  reviewStop: () => invoke<void>("review_stop"),
  reviewDelete: (id: string) => invoke<void>("review_delete", { id }),
  reviewSave: (id: string, path: string) => invoke<void>("review_save", { id, path }),
  // One-click add-ons ("addons:progress" while one is set up).
  addons: () => invoke<AddonView[]>("addons_list"),
  addonAdd: (id: string, values: Record<string, string>) => invoke<Connector>("addon_add", { id, values }),
  // Check this PC ("pccheck:step" as it goes).
  pcCheckStart: () => invoke<PcCheckReport>("pc_check_start"),
  pcCheckStop: () => invoke<void>("pc_check_stop"),
  pcCheckReport: () => invoke<PcCheckReport | null>("pc_check_report"),
  // Study sets ("study:progress" while one is made).
  studySets: () => invoke<StudySet[]>("study_sets"),
  studyMake: (request: StudyRequest) => invoke<StudySet>("study_make", { request }),
  studyDelete: (id: string) => invoke<void>("study_delete", { id }),
  studyRename: (id: string, title: string) => invoke<StudySet>("study_rename", { id, title }),
  /** A flashcard answered: knew it, or not. */
  studyCard: (id: string, card: number, knew: boolean) => invoke<StudySet>("study_card", { id, card, knew }),
  studyQuizDone: (id: string, score: number) => invoke<StudySet>("study_quiz_done", { id, score }),
  /** Keeps one of the voices just made (`pick` from 0) under `name`. */
  ownVoiceDesignKeep: (name: string, pick: number) => invoke<OwnVoicesView>("own_voice_design_keep", { name, pick }),
  mediaAdd: (input: { data: string; mime: string; op: string; prompt?: string | null; parent?: string | null; chatId?: string | null; hidden?: boolean; seconds?: number | null; lyrics?: string | null; note?: string | null }) =>
    invoke<MediaItem>("media_add", { data: input.data, mime: input.mime, op: input.op, prompt: input.prompt ?? null, parent: input.parent ?? null, chatId: input.chatId ?? null, hidden: input.hidden ?? null, seconds: input.seconds ?? null, lyrics: input.lyrics ?? null, note: input.note ?? null }),
  /** A song's notes as a MIDI file, saved at `path`: `map` as edited, else the song's own. */
  mediaSongMidi: (id: string, path: string, map?: SongMap) => invoke<void>("media_song_midi", { id, path, map: map ?? null }),
  /** Keeps notes edited in the song view with the song. */
  mediaSongMapSave: (id: string, map: SongMap) => invoke<MediaItem>("media_song_map_save", { id, map }),
  /** The sampled instruments (a General MIDI sound set), downloaded the first time. */
  composeSounds: () => invoke<ArrayBuffer>("compose_sounds"),
  composeWrite: (ask: ComposeAsk) => invoke<Composition>("compose_write", { ask }),
  composeArrange: (plan: Plan) => invoke<Composition>("compose_arrange", { plan }),
  /** A section changed (in words, by the chat model, when `change` is given), written again, added, copied or deleted. */
  composeSection: (composition: Composition, index: number, op: SectionOp, change?: string) =>
    invoke<Composition>("compose_section", { composition, index, op, change: change ?? null }),
  composeSave: (id: string | null, composition: Composition, audio: string, seconds: number) => invoke<MediaItem>("compose_save", { id, composition, audio, seconds }),
  /** Saved at `path`; without one, its bytes (for the player). */
  composeMidi: (composition: Composition, path: string | null) => invoke<ArrayBuffer>("compose_midi", { composition, path }),
  /** Made real with the song options and words given (else the ones it was written with). */
  composeMakeReal: (id: string, o: { strength?: number; modelId?: string | null; song?: MusicSpec | null; lyrics?: string | null; language?: string | null }) =>
    invoke<string>("compose_make_real", { id, strength: o.strength ?? null, modelId: o.modelId ?? null, chatId: null, song: o.song ?? null, lyrics: o.lyrics ?? null, language: o.language ?? null }),
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
  /** "mark_read" is the mark-read button: read, and listed under Marked read. */
  mailAction: (ids: string[], action: "seen" | "unseen" | "archive" | "trash" | "mark_read") => invoke<void>("mail_action", { ids, action }),
  mailSubscriptions: () => invoke<Subscription[]>("mail_subscriptions"),
  mailUnsubscribe: (key: string) => invoke<UnsubscribeResult>("mail_unsubscribe", { key }),
  mailEventDraft: (id: string) => invoke<EventDraft>("mail_event_draft", { id }),
  /** The AI's draft of a reply (only the body; nothing is sent). */
  mailDraftReply: (id: string) => invoke<string>("mail_draft_reply", { id }),
  /** A task to reply, due tomorrow at 9:00 with a reminder. */
  mailRemindMissed: (id: string) => invoke<{ added: boolean }>("mail_remind_missed", { id }),
  setMailRemindMissed: (on: boolean) => invoke<void>("set_mail_remind_missed", { on }),
  mailBulkPlan: (ids: string[], action: BulkPlan["action"]) => invoke<BulkPlan>("mail_bulk_plan", { ids, action }),
  /** The invitation's event if it's on a calendar here already. */
  mailInviteStatus: (id: string) => invoke<CalEvent | null>("mail_invite_status", { id }),
  /** Accept, Maybe or Decline; the reply is sent only with sendReply. */
  mailInviteRespond: (id: string, answer: "accept" | "maybe" | "decline", calendar: string | null, sendReply: boolean) =>
    invoke<InviteOutcome>("mail_invite_respond", { id, answer, calendar, sendReply }),
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
  /** The status bar's graphics card menu: every card found, the one in use, where the model runs and why. */
  gpuMenu: () => invoke<GpuMenu>("gpu_menu"),
  /** Makes a card the one models run on (null: automatic); a loaded model starts again on it. */
  setMainGpu: (name: string | null) => invoke<GpuMenu>("set_main_gpu", { name }),
  /** Machines lending their graphics memory over the network (llama.cpp rpc-server); none until added. */
  linkedMachines: () => invoke<LinkedMachine[]>("linked_machines"),
  setLinkedMachines: (machines: LinkedMachine[]) => invoke<LinkedMachine[]>("set_linked_machines", { machines }),
  setGpuLimit: (percent: number) => invoke<GpuLimit>("set_gpu_limit", { percent }),
  answerMemoryWarning: (id: string, go: boolean) => invoke<void>("answer_memory_warning", { id, go }),
  cardAsks: () => invoke<CardAsk[]>("card_asks"),
  cardAskAnswer: (id: string, answer: "processor" | "always" | "free" | "restart" | "stop") => invoke<void>("card_ask_answer", { id, answer }),
  processorFallback: () => invoke<ProcessorFallback>("processor_fallback"),
  setProcessorFallback: (value: ProcessorFallback) => invoke<void>("set_processor_fallback", { value }),
  driverCheck: () => invoke<DriverNote[]>("driver_check"),
  loras: () => invoke<{ loras: Lora[]; families: [string, string][] }>("loras"),
  loraAdd: (path: string) => invoke<Lora>("lora_add", { path }),
  loraSave: (lora: Lora) => invoke<Lora[]>("lora_save", { lora }),
  loraDelete: (id: string) => invoke<Lora[]>("lora_delete", { id }),
  modelIdle: () => invoke<{ busy: boolean; idle_secs: number; unloads_in: number | null }>("model_idle"),
  goBackTo: (version: string) => invoke<void>("go_back_to", { version }),
  draftsKeep: () => invoke<boolean>("drafts_keep"),
  setDraftsKeep: (on: boolean) => invoke<void>("set_drafts_keep", { on }),
  drafts: () => invoke<Record<string, string>>("drafts"),
  draftSet: (chatId: string, text: string) => invoke<void>("draft_set", { chatId, text }),
  mediaSetCover: (id: string, path: string) => invoke<MediaItem>("media_set_cover", { id, path }),
  lineupList: () => invoke<LineupEntry[]>("lineup_list"),
  lineupMove: (id: string, to: number) => invoke<void>("lineup_move", { id, to }),
  lineupStartNowOptions: (id: string) => invoke<StartNow>("lineup_start_now_options", { id }),
  lineupStartNow: (id: string, how: "processor" | "stop") => invoke<void>("lineup_start_now", { id, how }),
  lineupRemove: (id: string) => invoke<void>("lineup_remove", { id }),
  driverPutOff: (key: string, need: string) => invoke<void>("driver_put_off", { key, need }),
  setPerf: (settings: PerfSettings) => invoke<PerfLimits>("set_perf", { settings }),
  pauseView: () => invoke<PauseView>("pause_view"),
  setPauseSettings: (settings: PauseSettings) => invoke<PauseView>("set_pause_settings", { settings }),
  /** Fails with "busy:<what's running>" unless `force`, which stops it. */
  pauseNow: (force = false) => invoke<void>("pause_now", { force }),
  resumeNow: () => invoke<void>("resume_now"),
  quitNow: () => invoke<void>("quit_now"),
  zeroView: () => invoke<ZeroView>("zero_view"),
  setWebSettings: (settings: WebSettings, braveKey: string | null) => invoke<void>("set_web_settings", { settings, braveKey }),
};

export type DictationEvent =
  | { id: string; state: "loading" | "listening" | "hearing" | "done" | "quiet" }
  | { id: string; state: "level"; level: number }
  | { id: string; state: "text"; text: string }
  | { id: string; state: "error"; error: string };

export type VoiceEvent =
  | { chat_id: string; state: "loading" | "listening" | "hearing" | "thinking" | "speaking" | "interrupted" | "ended" }
  | { chat_id: string; state: "level"; level: number; speaking: boolean }
  | { chat_id: string; state: "heard"; text: string }
  | { chat_id: string; state: "replied"; text: string }
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

/** Work meant for the graphics card that can't go there: why, and what's possible. */
export interface CardAsk {
  id: string;
  job: string | null;
  what: string;
  why: string;
  /** A chat model holds the card and nothing uses it: unloading may make room. */
  free: boolean;
  /** Starting SulcusAI again lets it use the card. */
  restart: boolean;
}

/** A chat reply or Studio job in the line for the graphics cards (`lineup.rs`). */
export interface LineupEntry {
  id: string;
  kind: "chat" | "studio";
  title: string;
  what: string;
  chat_id: string | null;
  model: string | null;
  need: number;
  second_ok: boolean;
  processor_ok: boolean;
  running: boolean;
  /** Where it runs: a card, or null for the processor (once running). */
  card: "main" | "second" | null;
  /** Part of a running reply (a picture or song it makes). */
  inside: string | null;
  queued_at: number;
  started_at: number | null;
}

/** What starting a waiting entry now would take. */
export interface StartNow {
  processor: boolean;
  in_the_way: LineupEntry[];
}

/** A steering direction made in the Lens (steer.rs): added to the model's
 * layers while it answers; "steer:<id>" in a chat's model menu. */
export interface LensDirection {
  id: string;
  name: string;
  model_id: string;
  toward: string;
  away: string;
  /** Negative: the other way. */
  strength: number;
  from: number;
  to: number;
  layers: number;
  pairs: number;
  created_at: number;
}

/** The Lens (lens.rs): a model on this PC seen from inside. */
export interface LensAlt { text: string; prob: number }
export interface LensPiece { text: string; prob: number; alts: LensAlt[] }
export interface LensWords { model: string; given: string; pieces: LensPiece[] }
export interface LensNeuron { layer: number; index: number; energy: number; ratio: number }
export interface LensLayers { layers: number; parts: string[]; energy: number[][]; usual: number[][]; neurons: LensNeuron[]; editable: string | null }
export interface LensEdit { layer: number; index: number; scale: number }

/** A video's edits (media/mod.rs `VideoEdit`). */
export interface VideoEdit {
  trim?: [number, number] | null;
  join?: string[];
  audio?: string | null;
  audio_file?: string | null;
  audio_mode?: "replace" | "mix";
  volume?: number;
  mute?: boolean;
  /** Stretches kept when editing by transcript. */
  keep?: Array<[number, number]>;
  clean_speech?: "gentle" | "strong" | null;
}

/** One of the person's own LoRAs for pictures (media/loras.rs). */
export interface Lora {
  id: string;
  name: string;
  family: string;
  trigger: string;
  weight: number;
  size: number;
}

/** A graphics driver that needs updating (checked when SulcusAI starts). */
export interface DriverNote {
  card: string;
  vendor: string;
  version: string | null;
  date: string | null;
  need: "missing" | "too_old" | "old";
  why: string;
  link: string;
  key: string;
}

/** When the graphics card can't take work: ask, always use the processor, or stop it. */
export type ProcessorFallback = "ask" | "always" | "never";

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
  /** Percent busy (every app's use). */
  usage: number | null;
  /** °C; only NVIDIA cards report it, and only while SulcusAI is working. */
  temp: number | null;
  mem_used: number | null;
  mem_total: number | null;
  /** SulcusAI's own share. */
  ours: number | null;
  /** SulcusAI is working now (a reply, a picture, listening). */
  working: boolean;
}

export interface InstallProgress {
  model_id: string;
  /** "loading" and "benchmark": the model is tried out on this PC (its speed is measured). */
  phase: "engine" | "verify" | "download" | "loading" | "benchmark" | "vision" | "optimizing" | "waiting";
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
  /** "queued": waiting in line for another model (`place`: 1 is next; `detail`: the model running now). */
  "chat:status": {
    chat_id: string;
    status: "loading" | "queued" | "waiting_card" | "handoff" | "cooling" | "compacting" | "looking_up" | "reading_pictures" | "retrying" | "idle";
    detail?: string;
    place?: number;
    /** Trying a busy cloud provider again: which, in how long, and which try of how many. */
    provider?: string;
    seconds?: number;
    attempt?: number;
    of?: number;
  };
  /** Auto handed this message to another installed model, better at its task. */
  "chat:routed": { chat_id: string; model_id: string; name: string; task: "code" | "writing" | "languages" | "this" | "private" | "files" | "long" | "hard"; cloud?: boolean };
  "chat:compacted": { chat_id: string; auto: boolean };
  "media:progress": MediaJob;
  "media:done": { job_id: string; ok: boolean; items?: MediaItem[]; cancelled?: boolean; error?: string };
  "compose:getting": { fraction: number };
  "media:tuned": { family: string; baseline: number; score: number; pictures: number; demos: number; new_instructions: boolean };
  "media:coached": { before: number; after: number; examples: number; kept: boolean };
  "media:trained": CustomModel;
  "media:lora_trained": Lora;
  "studio:level": { level: number; seconds: number };
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
  "wake:heard": Record<string, never>;
  live_voice: { state: "loading" | "on" | "level" | "error" | "stopped"; level?: number; took_ms?: number; behind?: boolean; error?: string };
  /** The assistant deleted a chat (the user approved it). */
  "chat:deleted": { chat_id: string };
  /** The assistant changed a chat's notes. */
  "chat:notes": { chat_id: string };
  /** A reply's thinking was summed up (its last message has `thought_summary`). */
  "chat:thought": { chat_id: string };
  /** Lasting facts the assistant saved from a chat by itself. */
  "memory:auto": { chat_id: string; saved: string[] };
  /** A link to open in the web viewer (the main browser is the viewer). */
  "viewer:open": { url: string | null };
  /** A cloud model's reply was counted toward a chat's (and the month's) spend. */
  "cloud:spent": { chat_id: string | null };
  /** Signed in to a server in the built-in browser; its session is saved. */
  "cloud:signed_in": { id: string; name: string; account?: string };
  "cloud:sign_in_failed": { id: string; error: string };
  /** `key`: the viewer's chat id, or "main" for the Web page's. */
  "viewer:page": { key: string; url: string; loading: boolean };
  "viewer:title": { key: string; title: string };
  /** A chat's assistant opened a page in its chat's viewer: show it beside the chat. */
  "viewer:dock": { key: string };
  "chat:queue": { chat_id: string; queue: QueueItem[] };
  "chat:inbox": { chat_id: string; waiting: QueueItem[] };
  /** The model has read this chat's messages up to then (it started answering). */
  "chat:read": { chat_id: string; up_to: number };
  "gpu:limit": { percent: number };
  /** A card was picked for models; `reloaded` once the loaded model started again on it. */
  "gpu:main": { reloading?: boolean; reloaded?: boolean };
  /** The PC's graphics cards changed (a driver installed, a card switched on or off). */
  "hardware:changed": null;
  "offline:changed": { on: boolean };
  "engine:memory": MemoryWarning;
  "engine:memory_done": { id: string };
  "card:ask": CardAsk;
  "card:ask_done": { id: string };
  "lineup:changed": LineupEntry[];
  "chat:web": { chat_id: string; web: boolean };
  /** The assistant opened a site in the chat's Preview panel. */
  "chat:preview": { chat_id: string; url: string; site?: string; root?: string; start?: string; port?: number };
  "app:navigate": { page: string };
  "mail:triage": { done: number; total: number };
  "mail:triaged": Record<string, never>;
  "schedule:ran": { id: string; name: string; chat_id?: string; error?: string };
  "speak:sentence": { index: number; count: number; text: string };
  "speak:done": Record<string, never>;
  "install:progress": InstallProgress;
  /** The Lens finding a steering direction (steer.rs). */
  "lens:steer": { stage: string; fraction: number };
  "install:finished": InstallFinished;
  "downloads:changed": Record<string, never>;
  "security:locked": null;
  "features:changed": Record<string, never>;
  "folders:changed": Record<string, never>;
  "models:found": { names: string[] };
  "task:reminder": { id: string; title: string };
  "perf:heat": HeatState;
  "perf:mode": { mode: PerfMode };
  "pause:changed": { paused: boolean };
  /** Pausing from the tray or a hotkey found work running: the window asks. */
  "pause:asks": { running: string };
  "zero:open": Record<string, never>;
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
  "uncensor:progress": UncensorJob;
  "sync:status": null;
  "models:moving": { done?: number; total?: number; finished?: boolean; error?: string };
  /** The speed test's status, plus the model just tested (tps or error), and finished at the end. */
  "models:speed": SpeedStatus & { tested?: string; tps?: number; prompt_tps?: number | null; error?: string; finished?: boolean };
  "models:optimize": { model_id: string; step?: number; of?: number; label?: string; finished?: boolean; report?: OptimizeReport; error?: string };
  "sync:changed": null;
  "people:changed": null;
  "people:message": { contact_id: string };
  /** A voice line being recorded: its loudness (0 to 1) and seconds so far. */
  "own_voice:level": { voice: string; line: string; level: number; seconds: number };
  /** The line reached its longest, and recording should stop. */
  "own_voice:full": { voice: string; line: string };
  /** Adding recordings: which file (n of `of`) and what it's doing. */
  "own_voice:adding": { voice: string; file: string; step: "reading" | "separating" | "choosing" | "getting_speakers" | "speakers" | "hearing" | "getting_words"; n: number; of: number };
  /** The documents library reading and indexing: what it's on. */
  "docs:progress": DocProgress;
  /** A code review: what it's checking and how far (0 to 1). */
  "review:progress": { stage: string; fraction: number };
  /** An add-on being set up: what it's doing and how far (0 to 1; -1: unknown). */
  "addons:progress": { stage: string; fraction: number };
  /** Check this PC, after each change. */
  "pccheck:step": PcCheckReport;
  /** A study set being made: what it's doing and how far (0 to 1). */
  "study:progress": { stage: string; fraction: number };
  /** The library changed (indexing finished). */
  "docs:changed": Record<string, never>;
  /** Hearing a sentence in a voice: downloading the engine or model the first time (fraction 0 to 1), then reading (fraction -1). */
  "own_voice:trying": { stage: string; fraction: number; downloading: boolean };
  /** Making voices from a description: what it's doing and how far (0 to 1). */
  "own_voice:designing": { stage: string; fraction: number };
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
  /** What someone lends this user changed. */
  "people:lends": { contact_id: string };
  /** A big file or folder going or arriving in parts: how far it's got. */
  "people:file": { contact_id: string; id: string; got: number; size: number };
  /** Their PC didn't answer a message here with the model they lend. */
  "people:lend_refused": { contact_id: string; chat_id: string; why: string };
  /** Someone used this PC (a lent answer or a question to this user's assistant). */
  "people:runs": { contact_id: string };
  /** Someone wrote in a chat this user shares (or their model did). */
  "chat:changed": { chat_id: string };
  "update:progress": { received: number; total: number | null };
  "update:installing": Record<string, never>;
  "privacy:status": null;
  /** The address shield's switches, rotation or problems changed. */
  "shield:changed": null;
  dictation: DictationEvent;
  voice: VoiceEvent;
  meeting: MeetingEvent;
  /** Comparing models: one of them started, wrote more, or finished. */
  "compare:start": { run: string; index: number };
  "compare:delta": { run: string; index: number; content?: string; thinking?: string };
  "compare:done": { run: string; index: number; loading?: number; answering?: number; tps?: number | null; tokens?: number | null; error?: string };
}

export function on<K extends keyof ChatEvents>(name: K, handler: (payload: ChatEvents[K]) => void): Promise<UnlistenFn> {
  return listen<ChatEvents[K]>(name, (e) => handler(e.payload));
}

/** Opens a page in the web viewer, inside the window (whatever the main browser is). */
export function openInViewer(url: string): Promise<void> {
  return emit("viewer:open", { url });
}

/** An error for people to read, in the window's language when the message
    has a translation (the core's messages are looked up as written). */
export function errorText(e: unknown): string {
  const text = typeof e === "string" ? e : e instanceof Error ? e.message : String(e);
  return t(text);
}

/** "Manage itself" for one chat; `manager` is who manages it. */
export type ManagerSettings = {
  on: boolean;
  token_budget: number;
  minutes_budget: number;
  repeat_limit: number;
  clear_at: number;
  manager: { kind: "itself" } | { kind: "chat" | "model"; id: string };
  /** Tokens across managed chats per day / week; 0 is no cap. */
  day_tokens: number;
  week_tokens: number;
  /** Work start to finish. */
  through: boolean;
  nudge_minutes: number;
};

export type ManagerStep = { text: string; size: "lookup" | "edit" | "feature" | "hard" | null; state: "waiting" | "working" | "done" | "stuck"; checks: unknown[] };
export type ManagerPlan = { plan: { steps: ManagerStep[] } | null; decisions: string[]; state: string };

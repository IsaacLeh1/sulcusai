// SPDX-License-Identifier: AGPL-3.0-only
// Types for the PC, its models, people, sync, cloud models and chats.
import { type CloudMedia, type HubMediaModel } from "./media";
import { type GpuNow } from "../api";

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
  /** Cards Windows lists that aren't working (no driver, stopped, turned off). */
  broken_cards: BrokenCard[];
  models_drive: string;
  disk_free: number;
  disk_total: number;
  on_battery: boolean | null;
  /** The card picked for models, by name (null: the one with the most memory). */
  main: string | null;
}

export interface BrokenCard {
  name: string;
  vendor: string;
  /** Device Manager's problem code: 0 none (Windows' stand-in driver), 22 turned off, 28 no driver, 43 stopped. */
  code: number;
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
  /** Taught by practice: test tasks right before and after, in all and skill by skill. */
  scores: { before: number; after: number; out_of: number; skills?: SkillScore[]; second?: SecondTest | null } | null;
  /** Practice with a teacher: the model that answered the tasks it missed. */
  teacher?: string | null;
}

export interface FinetuneJob {
  id: string;
  name: string;
  base_name: string;
  status: "preparing" | "downloading" | "training" | "saving" | "done" | "failed" | "cancelled" | "testing" | "practising" | "teacher" | "checking" | "not_better";
  fraction: number;
  epoch: number;
  epochs: number;
  loss: number | null;
  eta: string | null;
  error: string | null;
  /** Practice: tasks done of the stage's total, right answers kept (the teacher's, in its turn), test scores. */
  done: number;
  total: number;
  kept: number;
  before: number | null;
  after: number | null;
  /** Practice with a teacher: its name. */
  teacher: string | null;
  /** Practice learns each skill on its own first: the skills being learnt or tested now, and which try of how many. */
  part: PracticeSkill[];
  part_no: number;
  parts: number;
  /** Practice, at the end: the test skill by skill, for what was kept (or the best try). */
  skills: SkillScore[];
  /** Practice: the second test, of new tasks, of the one it would keep. */
  second: SecondTest | null;
  /** Training doesn't fit the card's free memory, or has slowed down as if it didn't. */
  low_memory: boolean;
}

/** An uncensored copy being made (Advanced mode, with Heretic). */
export interface UncensorJob {
  /** The model being copied. */
  model_id: string;
  name: string;
  status: "setting_up" | "downloading" | "loading" | "trying" | "saving" | "converting" | "done" | "failed" | "cancelled";
  fraction: number;
  trial: number;
  trials: number;
  /** The original's refusals of the test requests, and how many there are. */
  base_refusals: number | null;
  of: number;
  /** The best try so far within the drift allowed (at the end, the one kept). */
  refusals: number | null;
  kl: number | null;
  /** Loaded in 4 bits to fit the graphics card. */
  four_bit: boolean;
  seconds_left: number | null;
  error: string | null;
  /** The copy, once it's in. */
  copy_id: string | null;
}

/** What making an uncensored copy of a model would take. */
export interface UncensorLook {
  /** The original on Hugging Face ("owner/name"), as found or pasted. */
  original: string | null;
  /** Why it can't be made from that original (a whole sentence). */
  problem: string | null;
  download: number;
  /** Disk space the run needs at its fullest, and what's free there. */
  disk: number;
  free: number | null;
  four_bit: boolean;
  /** What setting Heretic up the first time downloads (0 once set up). */
  setup: number;
  /** About how long, in seconds. */
  quick: number;
  thorough: number;
  /** The finished copy's size, about. */
  size: number;
  /** What stopped runs left on the disk. */
  leftover: number;
  job: UncensorJob | null;
}

/** Practice's second test, of new tasks, that confirms what's kept: right before and after. */
export interface SecondTest {
  before: number;
  after: number;
  out_of: number;
}

/** One skill's test tasks right before and after practice; `learnt`: the adapter learnt from this skill. */
export interface SkillScore {
  skill: PracticeSkill;
  before: number;
  after: number;
  out_of: number;
  learnt: boolean;
}

export type PracticeSkill = "code" | "writing" | "research" | "math" | "careful" | "time" | "format" | "tools" | "project";

export interface FinetuneView {
  bases: { id: string; name: string; supported: boolean; conversations: boolean; practice: boolean; tools: boolean }[];
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
  /** "getting": downloading Tor, the first time. */
  state: "off" | "waiting" | "getting" | "starting" | "ready" | "stuck";
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
  /** "Reach this phone from anywhere", through SulcusAI's relay. */
  anywhere: boolean;
  link: { connected: boolean; problem: string | null; address: string | null };
}

/** How the PC is doing, for the phone's PC tab. */
export interface PcStatus {
  pc: string;
  cpu: number;
  cpu_temp: number | null;
  ram_used: number;
  ram_total: number;
  gpu: GpuNow;
  gpu_temp: number | null;
  /** Adaptive cooling's level now (0: not cooling, 3: the most). */
  cooling: number;
  model: string | null;
  jobs: { kind: "chat" | "media" | "install" | "training"; title: string; detail: string; fraction: number | null }[];
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
  /** `folder`: a folder, zipped (saved by unpacking it); `got`: still
   * arriving (or going) in parts, the bytes so far; `failed`: it couldn't arrive. */
  file: { name: string; size: number; folder?: boolean; got?: number | null; failed?: boolean } | null;
  /** For messages from here: they have it. */
  delivered: boolean;
  /** For messages from here: they've read it (they send read receipts). */
  read: boolean;
  /** Its writer changed it after sending. */
  edited: boolean;
  /** Its writer deleted it. */
  deleted: boolean;
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
  status: "loading" | "queued" | "thinking" | "writing" | "working" | string;
  content: string;
  /** "queued": its place in line on their PC (1 is next). */
  place?: number;
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
  /** It may check whether this user is free. */
  calendar: boolean;
  /** "yes_no": only whether they're free when asked; "times": their free times too. */
  free: "yes_no" | "times";
  /** It may add events to this user's calendar when asked. */
  calendar_add: boolean;
  mail: boolean;
  folders: string[];
  /** The model that answers (null: the default model). */
  model: string | null;
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
  /** Connected through SulcusAI's relay (the built-in pipe), not directly. */
  via_pipe: boolean;
  via_relay: boolean;
  /** This user lets them use their relay. */
  offer_relay: boolean;
  /** This user tells them when they've read their messages. */
  read_receipts: boolean;
  /** What they may use of this PC's models in chats shared with them. */
  lend: Lend;
  /** The name they gave themselves, when this user renamed them. */
  their_name: string | null;
}

/** What someone may use of this PC's models, and how much (null: no limit). */
export interface Lend {
  on: boolean;
  /** Model ids they may pick; empty: this user's default model. */
  models: string[];
  answers_per_day: number | null;
  tokens_per_day: number | null;
  usd_per_month: number | null;
  /** 1 is Monday, 7 Sunday; empty: every day. */
  days: number[];
  /** "HH:MM"; both empty: any time. */
  from: string;
  until: string;
  max_minutes: number | null;
}

/** A model someone lends this user (`id` empty: their default). */
export interface LentModel {
  id: string;
  name: string;
  cloud: boolean;
}

export interface LendOffer {
  models: LentModel[];
  limits: Lend;
}

export interface LendTotals {
  answers: number;
  tokens: number;
  usd: number;
  ms: number;
  refused: number;
}

/** One use of this PC by someone. */
export interface LendRun {
  at: number;
  /** "chat": an answer in a shared chat; "question": their assistant asked this one. */
  kind: "chat" | "question";
  chat_id: string | null;
  chat_title: string | null;
  model: string;
  ms: number;
  tokens_in: number;
  tokens_out: number;
  usd: number;
  /** "done", "stopped", "refused: why" or "failed: why". */
  outcome: string;
}

export interface LendRuns {
  today: LendTotals;
  month: LendTotals;
  all: LendTotals;
  runs: LendRun[];
  busy: boolean;
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
  /** Its chats go to this user's other PCs that use the same key. */
  share_pcs: boolean;
  /** People (linked in People) its chats are shared with. */
  share_people: string[];
  /** Its requests go through a proxy of their own (a VPN's local proxy). */
  has_proxy: boolean;
}

/** Cloud spending: a month's, or a chat's (all its months). */
export interface CloudSpend {
  month: string;
  usd: number;
  input_tokens: number;
  /** Of the input, how much came from the provider's prompt cache. */
  cached_tokens?: number;
  output_tokens: number;
  requests: number;
}

export interface CloudView {
  providers: CloudProvider[];
  presets: CloudProvider[];
  budget: number | null;
  redact: boolean;
  spend: CloudSpend;
}

/** A cloud model people can pick for a chat. */
/** The order the user dragged the sidebar's pages and chats into (empty: the usual order). */
export interface SidebarLayout {
  nav: string[];
  chats: string[];
  /** Pages in the folding "Collapse" group. */
  collapsed?: string[];
  collapse_open?: boolean;
  /** The whole sidebar folded away. */
  hidden?: boolean;
}

export interface CloudChoice {
  id: string;
  name: string;
  provider: string;
  vision: boolean;
  /** US dollars per million tokens in and out, if known. */
  price_in: number | null;
  price_out: number | null;
}

/** What a chat has cost on cloud models, this month's total, and the budget. */
export interface ChatCloudSpend {
  chat: CloudSpend;
  month: CloudSpend;
  budget: number | null;
}

export interface ApiServerView {
  enabled: boolean;
  port: number;
  key: string;
  url: string;
  running: boolean;
  problem: string | null;
  /** The Ollama-shaped server, for programs made for Ollama. */
  ollama: boolean;
  ollama_port: number;
  ollama_url: string;
  ollama_running: boolean;
  ollama_problem: string | null;
}

export interface Advanced {
  enabled: boolean;
  sampling: { temperature: number | null; top_p: number | null; top_k: number | null; min_p: number | null; repeat_penalty: number | null; presence_penalty: number | null; frequency_penalty: number | null; seed: number | null };
  engine: { ctx: number | null; gpu_layers: number | null; threads: number | null; batch: number | null; kv_cache?: string | null; flash_attn?: boolean | null };
  guardrails: { instructions: string | null; extra: string; never: string[]; always_ask: boolean; max_steps: number | null; max_minutes?: number | null; no_auto_review?: boolean };
  processing: { thinking: "auto" | "on" | "off"; handoff_at: number | null; handoff_off: boolean; new_chat?: boolean; helper_steps: number | null };
}

/** An assistant of the person's own, to start chats with. */
export interface Assistant {
  id: string;
  name: string;
  description: string;
  instructions: string;
  model: string | null;
  starters: string[];
  /** A character's lorebook (kept as is when the assistant is edited). */
  lore?: { keys: string[]; content: string }[];
}

/** Text typed in a chat as /name. */
export interface SavedPrompt {
  id: string;
  name: string;
  text: string;
}

/** One model's own settings (empty fields: the app's, or Advanced's). */
export interface ModelPreset {
  instructions: string;
  sampling: Advanced["sampling"];
  ctx: number | null;
  kv_cache: string | null;
  flash_attn: boolean | null;
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
  /** It would take most of the memory this PC has for models. */
  heavy: boolean;
  /** 3 bits a weight or fewer: runs, but answers are noticeably worse. */
  squeezed: boolean;
}

/** What a model is for, to narrow a Hugging Face search ("" for any). */
export type HubPurpose = "" | "coding" | "writing" | "reasoning" | "vision" | "uncensored" | "translation";

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
  /** The most tokens of context it was made for. */
  context: number;
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
  /** Tokens a second at the start of a chat, and once the window is full. */
  est_tps: number;
  full_tps: number;
  /** Seconds to read a full window, at this PC's measured reading speed. */
  read_full_secs: number | null;
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
  /** The model uses tools: under 16K a task of a few steps runs out of room. */
  tools: boolean;
  /** A window set in Advanced, which wins over these. */
  advanced: number | null;
  /** The speeds are this PC's measured ones (else estimates). */
  measured: boolean;
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
  /** An installed model whose file is damaged or unfinished, and how. */
  damaged?: string | null;
  /** A faster file of this model (with multi-token-prediction layers) is out: its download size. */
  faster?: number | null;
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
  /** Auto: a message on the default model goes to the installed model clearly better at it. */
  auto_model: boolean;
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
  /** Opens only with the app lock's PIN or Windows Hello. */
  protected?: boolean;
  /** Protected and not opened yet: its title is left out. */
  shut?: boolean;
}

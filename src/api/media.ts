// SPDX-License-Identifier: AGPL-3.0-only
// Types for songs, Compose, own voices, the compute PC, add-ons, study, documents and Studio.
import { type PcStatus } from "./system";
import { type MediaKind, type MediaModelCard } from "./work";
import { type VideoEdit } from "../api";

/** A section of a composition's plan. */
export interface PlanSection {
  name: string;
  bars: number;
  /** A chord a bar ("Am", "F", "C/E"); fewer repeat. */
  chords: string[];
  drums: "none" | "light" | "steady" | "busy" | "half-time";
  bass: "none" | "roots" | "pulse" | "walking" | "syncopated";
  keys: "none" | "held" | "arpeggio" | "strummed" | "stabs";
  melody: boolean;
  lines: string[];
  energy: number;
  take: number;
}

export interface Plan {
  title: string;
  /** Quarter notes a minute, in any time signature. */
  bpm: number;
  beats: number;
  unit: number;
  key: string;
  style: string;
  instruments: { chords: string; melody: string; bass: string; drums: string };
  sections: PlanSection[];
  seed: number;
  description: string;
  voice: string;
  era: string;
  /** Every option picked in the Music tab: what Make it real asks for. */
  song?: MusicSpec | null;
  language?: string | null;
}

/** A note of a composition, in beats (the time signature's unit) from the start. */
export interface ComposeNote {
  beat: number;
  len: number;
  pitch: number;
  vel: number;
}

export interface Composition {
  plan: Plan;
  parts: { name: "melody" | "chords" | "bass" | "drums" | string; instrument: string; notes: ComposeNote[] }[];
}

/** What Compose is asked for. */
export interface ComposeAsk {
  description: string;
  beats: number;
  unit: number;
  bpm?: number | null;
  key?: string | null;
  style?: string | null;
  sections: number;
  vocals: boolean;
  voice: string;
  era: string;
  seed?: number;
  /** Every option picked in the Music tab. */
  song?: MusicSpec | null;
  /** Their own lyrics: the sections and lines the plan is built around. */
  lyrics?: string | null;
  language?: string | null;
}

/** A change to one section of a composition. */
export type SectionOp = { do: "set"; section: PlanSection } | { do: "again" } | { do: "add"; section: PlanSection } | { do: "duplicate" } | { do: "delete" };

/** A note: start and end (seconds), MIDI pitch and loudness. */
export interface SongNote {
  start: number;
  end: number;
  pitch: number;
  velocity: number;
}

/** How a song is built: its sections and its parts' notes. */
export interface SongMap {
  length: number;
  sections: { name: string; start: number; end: number; lines: string[] }[];
  parts: { name: "vocals" | "other" | "bass" | "drums" | string; notes: SongNote[] }[];
}

/** A sung line, and how well it came through. */
export interface LineHeard {
  section: string;
  text: string;
  /** The share of its words heard (0 to 1). */
  heard: number;
  /** Where it's sung, in seconds. */
  at: [number, number] | null;
}

/** What the listener heard in a song. */
export interface SongReview {
  heard: number;
  words: number;
  lines: LineHeard[];
  /** Stretches whose words weren't clear (seconds): what a remake makes again. */
  unclear: [number, number][];
  notes: string[];
  score: number;
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
  /** Memory in use now, percent: the PC's, and the graphics card's. */
  ram_usage?: number | null;
  gpu_mem?: number | null;
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
  /** Models left out by kind and why: more memory, a bigger graphics card, or more free disk space. */
  hidden_by_kind?: { kind: MediaKind; why: "memory" | "card" | "disk"; count: number }[];
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

export type MediaOp = "generate" | "edit" | "fill" | "extend" | "restyle" | "upscale" | "remove_background" | "video" | "music" | "sound" | "narrate" | "podcast" | "dialogue" | "explainer" | "tune" | "coach" | "listen" | "remake" | "map" | "revoice" | "cover_art" | "stems" | "dry" | "song_video" | "train" | "trainer" | "studio_part" | "video_edit" | "video_sound" | "dub" | "speak" | "talk" | "captions" | "train_picture" | "lipsync";

/** A line of the voice script (the window holds its words). */
export interface OwnVoiceLine {
  id: string;
  kind: "speak" | "sing";
  min_s: number;
  max_s: number;
}

export interface OwnVoiceTake {
  line: string;
  seconds: number;
  /** What was said, as speech recognition heard it. */
  words: string;
  /** "quiet", "loud" (it distorted) or "noisy". */
  notes: string[];
  at: number;
  /** A recording the person added ("added-<n>"): singing or speaking. */
  kind?: "speak" | "sing" | null;
  /** The added file's name. */
  file?: string;
  /** How much of what was kept is voice (0 to 1); the clearest is used first. */
  score?: number;
}

/** A recorded voice: narration and songs in it. */
export interface OwnVoice {
  id: string;
  name: string;
  takes: OwnVoiceTake[];
  created_at: number;
  can_speak: boolean;
  can_sing: boolean;
  /** A voice made from a description: the description. */
  designed?: string | null;
}

export interface OwnVoicesView {
  voices: OwnVoice[];
  lines: OwnVoiceLine[];
  /** What narration and singing download the first time (bytes). */
  speak_size: number;
  sing_size: number;
  /** What making voices from words downloads the first time (0: not here). */
  design_size: number;
  /** Seconds of clear speech a voice from recordings needs, and that make it sound most like the person. */
  speech_enough: number;
  speech_best: number;
}

/** A recording checked for a voice made from recordings alone. */
export interface CheckedRecording {
  /** To keep it when creating the voice (null: it can't be used). */
  token: string | null;
  file: string;
  kind: "speak" | "sing";
  /** Seconds kept: its clearest stretch, up to 15. */
  seconds: number;
  /** How much of what's kept is voice (0 to 1). */
  clear: number;
  /** How well it can stand for the voice (0 to 1). */
  quality: number;
  notes: string[];
  words: string;
  problem: string | null;
  /** Others talk in it too: only whoever talks most was kept. */
  several: boolean;
}

/** A folder or file in the documents library. */
export interface DocSource {
  id: string;
  path: string;
  folder: boolean;
  added_at: number;
  scanned_at: number | null;
  name: string;
  files: number;
  passages: number;
  problems: number;
  /** Files found by their words only (read before the library's model was here). */
  words_only: number;
}

/** A file in the library and what happened when it was read. */
export interface DocFile {
  id: string;
  source_id: string;
  path: string;
  name: string;
  passages: number;
  embedded: boolean;
  indexed_at: number;
  error: string | null;
}

export interface DocProgress {
  running: boolean;
  stage: string;
  done: number;
  total: number;
  file: string | null;
}

export interface LibraryView {
  sources: DocSource[];
  files: number;
  passages: number;
  progress: DocProgress | null;
  model_ready: boolean;
  model_size: number;
  /** File kinds it reads. */
  kinds: string[];
}

/** The linked compute PC's health and what it has. */
export interface ComputeStatus {
  name: string;
  version: string;
  pc: PcStatus;
  drives: { name: string; mount: string; total: number; free: number; models: boolean }[];
  uptime_s: number;
  app_uptime_s: number;
  paused: boolean;
  models: { id: string; name: string; ctx: number; tools: boolean; vision: boolean }[];
  default_model: string | null;
  media: string[];
  perf: "cool" | "balanced" | "turbo";
}

export interface ComputeSettings {
  /** The paired PC doing the AI work. */
  peer: string | null;
  /** Pictures, video, songs and narration are made there. */
  media: boolean;
}

/** A problem a code review found. */
export interface CodeFinding {
  check: string;
  severity: "high" | "medium" | "low";
  file: string;
  line: number | null;
  title: string;
  why: string;
  fix: string;
}

export interface CodeReport {
  id: string;
  created_at: number;
  source: string;
  checks: string[];
  model: string;
  files: number;
  lines: number;
  findings: CodeFinding[];
  /** Findings dropped when checked again. */
  dropped: number;
  summary: string;
  took_ms: number;
  skipped: number;
}

export interface ReviewSetup {
  /** The best installed model for code: [id, name]. */
  best: [string, string] | null;
  /** Checks: [id, what it means]. */
  checks: [string, string][];
}

/** A one-click add-on (a well-known MCP server). */
export interface AddonView {
  id: string;
  name: string;
  by: string;
  category: string;
  about: string;
  inputs: { id: string; label: string; kind: "folder" | "file" | "secret" | "text"; help: string }[];
  link: string;
  added: boolean;
}

/** A step of Check this PC. */
export interface PcCheckStep {
  id: string;
  title: string;
  status: "waiting" | "running" | "ok" | "warn" | "failed" | "skipped";
  /** What it found, or why it failed or was skipped. */
  detail: string;
  /** What it's doing now, while it runs. */
  doing: string;
  ms: number;
}

export interface PcCheckReport {
  running: boolean;
  started_at: number | null;
  took_ms: number;
  steps: PcCheckStep[];
}

/** A flashcard, with its spaced-repetition box and when it's next due (ms). */
export interface StudyCard {
  front: string;
  back: string;
  box: number;
  due: number;
}

export interface StudyQuestion {
  question: string;
  choices: string[];
  /** The right choice, from 0. */
  answer: number;
  why: string;
}

export interface MapNode {
  label: string;
  children: MapNode[];
}

/** Flashcards, a quiz or a mind map made from what the person is learning. */
export interface StudySet {
  id: string;
  title: string;
  kind: "cards" | "quiz" | "map";
  created_at: number;
  /** What it was made from ("Biology notes.pdf and 2 more"). */
  from: string;
  cards: StudyCard[];
  quiz: StudyQuestion[];
  map: MapNode | null;
  /** A quiz's best score (0 to 1) and how often it was taken. */
  best: number | null;
  taken: number;
}

export interface StudyRequest {
  kind: "cards" | "quiz" | "map";
  text: string;
  files: string[];
  /** Library file ids. */
  docs: string[];
  count?: number | null;
  language?: string | null;
  writer?: string | null;
}

/** A passage found for a question. */
export interface DocHit {
  file_id: string;
  name: string;
  path: string;
  place: string | null;
  text: string;
  score: number;
}

/** Someone talking in a podcast or a reading with several voices. */
export interface TalkSpeaker {
  name: string;
  /** "own:<id>", a natural or Windows voice id; null: one is picked for them. */
  voice?: string | null;
  /** Who they are in it ("the host"). */
  about?: string | null;
}

/** A podcast ("podcast") or a reading with several voices ("dialogue"). */
export interface TalkSpec {
  speakers: TalkSpeaker[];
  minutes?: number | null;
  style?: "conversation" | "interview" | "debate" | "lesson" | "story" | null;
  /** Look the topic up on the web first. */
  research?: boolean;
  /** Files to talk about. */
  files?: string[];
  /** The chat model that writes it (null: the default). */
  writer?: string | null;
}

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
  /** What it is ("A model (the AI engine)"), when known. */
  what?: string | null;
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

/** A song as shaped in the Studio or a chat (media/mod.rs `MusicSpec`). */
export interface MusicSpec {
  genre?: string | null;
  mood?: string | null;
  instruments?: string[];
  voice?: string | null;
  beat?: string | null;
  bpm?: number | null;
  key?: string | null;
  time_signature?: string | null;
  /** Work on a song the person has (`audio`). */
  task?: "cover" | "repaint" | "extend" | "add" | "extract" | "complete" | null;
  audio?: string | null;
  reference?: string | null;
  region?: [number, number] | null;
  extend_by?: number | null;
  track?: string | null;
  strength?: number | null;
  /** How the lead sings ("breathy", "raspy"…). */
  vocal_style?: string | null;
  /** Backing harmony singers. */
  harmony_voice?: "female" | "male" | "mixed" | "choir" | null;
  /** An instrument playing a harmony line. */
  harmony_instrument?: string | null;
  harmony_where?: "chorus" | "throughout" | "ending" | null;
  harmony_style?: "thirds" | "octave" | "stacked" | "call" | "oohs" | null;
  /** Layered on afterwards by ACE-Step 1.5 Base (fuller, slower). */
  harmony_layer?: boolean;
  lead_instrument?: string | null;
  /** Sections in order ("Intro", "Verse", "Chorus"…). */
  sections?: string[];
  rhyme?: "aabb" | "abab" | "abcb" | "aaaa" | "free" | null;
  energy?: "steady" | "builds" | "big-chorus" | "quiet" | null;
  ending?: "fade" | "big-finish" | "hard-stop" | null;
  production?: string | null;
  /** The decade the song is from: "1950s" to "2020s". */
  era?: string | null;
  /** The length follows the song: a length named in the description, the lyrics, the sections, or the lyrics written for it. */
  auto_length?: boolean;
  /** Sung in a recorded voice (its id): made by a singer of the same sex, then sung again in it, in its range. */
  own_voice?: string | null;
  /** Of the versions made, keep only the one whose words were heard best. */
  best_of?: boolean;
  /** Each song gets cover art from the picture model, made next. */
  cover_art?: boolean;
  /** A custom model trained on the person's songs (its id): made on ACE-Step 1.5 Base with it. */
  custom?: string | null;
  /** How strongly the custom model is followed (1 as trained). */
  custom_scale?: number | null;
  /** How closely the description is followed, 0 to 1 (Suno's Style influence; null: the model's own). */
  influence?: number | null;
  /** How far from the usual, 0 to 1 (Suno's Weirdness; null: in between). */
  weirdness?: number | null;
  /** Styles, instruments or sounds to leave out, comma separated. */
  exclude?: string | null;
  /** The song's name (else its hook names it). */
  title?: string | null;
  /** A saved artist (id or name): their voice and sound for this song. */
  persona?: string | null;
  /** The singer's voice in words (a saved artist's). */
  voice_detail?: string | null;
  /** The feeling in the words and the voice. */
  emotion?: string | null;
  vocal_effects?: VocalEffect[];
  /** An artist whose style it follows (the name goes to the lyrics writer only). */
  sound_like?: string | null;
  /** Their sound in plain words, for the music model. */
  artist_sound?: string | null;
  artist_writing?: string | null;
}

/** A way of singing on part of a song: on is "all", "verse", "pre-chorus", "chorus", "bridge" or "final-chorus". */
export interface VocalEffect {
  how: string;
  on: string;
}

/** A saved artist (media/persona.rs): the song their voice follows, and their sound. */
/** One thing the chat bar asks the song studio to do (`studio::ask_messages`). */
export type StudioAction = { do: string } & Record<string, unknown>;

/** A Studio project as the window keeps it (`src/studio/model.ts`). */
export type StudioProject = import("../studio/model").Project;

/** What a training job is given. */
export interface TrainSpec {
  name: string;
  description: string;
  /** Song files on this PC. */
  files: string[];
  /** Songs from the gallery. */
  songs: string[];
  /** A recorded voice whose singing is learned too. */
  voice?: string | null;
}

/** A model trained on the person's songs. */
export interface CustomModel {
  id: string;
  name: string;
  trigger: string;
  description: string;
  songs: number;
  epochs: number;
  minutes: number;
  created_at: number;
}

/** The ways a picture can be made to talk here (media_talk_ways). */
export interface TalkWays {
  /** Quick (Ditto) is available here. */
  quick: boolean;
  /** No NVIDIA card: Quick runs on the processor, much slower. */
  processor: boolean;
  /** Ditto is set up (else its first run downloads `setup_bytes`). */
  ready: boolean;
  setup_bytes: number;
  /** Detailed (Wan 2.2 S2V) is installed. */
  detailed: boolean;
}

export interface CustomModelsView {
  models: CustomModel[];
  /** Training can run on this system. */
  available: boolean;
  nvidia: boolean;
  /** The trainer is set up (else its first run downloads `setup_bytes`). */
  ready: boolean;
  setup_bytes: number;
  /** ACE-Step 1.5 Base, which custom models play on, is installed. */
  base_installed: boolean;
  /** Recorded voices (Your voice) and how many singing takes each has. */
  voices?: { id: string; name: string; sung: number }[];
}

/** What the person likes in songs (My taste), from their own. */
export interface Taste {
  genres: string[];
  moods: string[];
  instruments: string[];
  voice: string | null;
  era: string | null;
  production: string | null;
  /** In songs marked "not for me", never in liked ones. */
  avoid: string[];
  songs: number;
}

/** A named list of songs from the gallery, in playing order. */
export interface Playlist {
  id: string;
  name: string;
  songs: string[];
  created_at: number;
  updated_at: number;
}

export interface Persona {
  id: string;
  name: string;
  /** Their voice in the music model's words, in every one of their songs. */
  voice_words: string;
  /** Their first song: its seed starts their later songs; followed when follow_voice_song. */
  voice_song: string | null;
  seed: number | null;
  follow_voice_song: boolean;
  sound: MusicSpec;
  language: string | null;
  created_at: number;
  updated_at: number;
}

/** An artist's style as the chat model describes it (media/artist.rs). */
export interface ArtistStyle {
  genre: string;
  voice: string;
  vocal: string;
  instruments: string[];
  production: string;
  mood: string;
  bpm: number | null;
  writing: string;
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
  width?: number;
  height?: number;
  steps?: number;
  extend?: [number, number, number, number];
  seconds?: number;
  lyrics?: string | null;
  instrumental?: boolean;
  /** Made from the song view: the new version's notes and sections are read too. */
  then_map?: boolean | null;
  /** A custom model to train on the person's songs (op "train"). */
  train?: TrainSpec | null;
  /** A picture LoRA to train on the person's photos (op "train_picture"). */
  picture_train?: { name: string; kind: "person" | "thing" | "style"; pictures: string[] } | null;
  /** Music: style, instruments, tempo, key, and work on a song. */
  music?: MusicSpec | null;
  voice?: string | null;
  language?: string | null;
  chat_id?: string | null;
  /** A podcast or a reading with several voices: who talks, and what to write about. */
  talk?: TalkSpec | null;
  /** New pictures: a look from the Studio's list ("auto" by default). */
  style?: string | null;
  /** Example pictures (gallery ids, at most 4) the new one should follow. */
  examples?: string[];
  /** What to take from them. */
  example_use?: "style" | "subject" | "layout";
  /** The person's own LoRAs, with how strongly each is used. */
  loras?: [string, number][];
  /** A picture (gallery id) whose people's pose the new one takes. */
  pose?: string | null;
  /** A video's camera move (`media::CAMERA`), added to its description. */
  camera?: string | null;
  /** The picture (gallery id) a video ends on; it starts on `source`. */
  end?: string | null;
  /** What to do to a video (video_edit). */
  edit?: VideoEdit | null;
  /** What a picture says or sings (speak): a gallery sound, a file, or words read aloud. */
  sound?: string | null;
  sound_file?: string | null;
  say?: string | null;
  /** The prompt writer rewrites the description for this model. */
  write_prompt?: boolean;
  /** The picture reader checks the result and it's made again if things are missing. */
  check?: boolean;
  notes?: string | null;
  /** A desktop wallpaper at this screen size. */
  wallpaper?: WallpaperSpec | null;
  /** Seamless tiles and sketch or depth guides. */
  picture_extra?: import("./create").PictureExtra | null;
}

/** A chat model's settings measured on this PC ("Optimize for this PC"). */
export interface ModelTuning {
  quant: string;
  ctx: number;
  room: number;
  gpu_layers: number;
  kv_q8: boolean;
  tps: number;
  prompt_tps: number | null;
  before_tps: number;
  all_layers: number;
  at: number;
  smaller_window?: { ctx: number; tps: number } | null;
}

export interface OptimizeReport {
  model_id: string;
  ctx: number;
  before_tps: number;
  tps: number;
  gpu_layers: number;
  all_layers: number;
  kv_q8: boolean;
  /** A smaller window at which the whole model fits the card. */
  smaller_window: { ctx: number; tps: number } | null;
}

export interface WallpaperSpec {
  width: number;
  height: number;
  oled: boolean;
}

export interface Screen {
  name: string;
  width: number;
  height: number;
  x: number;
  y: number;
  primary: boolean;
}

export interface WallpaperSet {
  path: string;
  how: "windows" | "lively" | "saved";
}

/** What the prompt writer knows about a picture model. */
export interface PromptStatus {
  family: string;
  tuned: { at: number; model_id: string; baseline: number; score: number; pictures: number } | null;
  /** The model brings its own language model to write with. */
  own_writer: boolean;
  /** What reads pictures here (checking, example pictures), if anything. */
  reader: string | null;
  /** The model's own writer, trained on this PC on its best practice
   * attempts (kept only when it scored higher than before). */
  coach: { at: number; model_id: string; writer: string; before: number; after: number; examples: number } | null;
  /** What the writer and edit planner learned from checked pictures and
   * edits: examples that worked, rules about the model, and how first tries
   * score (the first five against the last ten, once there are eight). */
  learned: {
    pictures: number;
    edits: number;
    rules: number;
    jobs: number;
    first: number | null;
    now: number | null;
    edit_first: number | null;
    edit_now: number | null;
  };
  /** Learning from what's made is on. */
  learning: boolean;
}

// SPDX-License-Identifier: AGPL-3.0-only
import { tx } from "../../i18n";

export type CategoryId =
  | "account"
  | "security"
  | "network"
  | "phone"
  | "models"
  | "voice"
  | "appearance"
  | "connections"
  | "files"
  | "pc"
  | "about";

export interface Category {
  id: CategoryId;
  label: string;
  /** One sentence on the home screen, so people can pick without opening each. */
  sentence: string;
}

export const CATEGORIES: Category[] = [
  { id: "account", label: tx("Account"), sentence: tx("Your name, what the assistant knows about you, language and chats.") },
  { id: "security", label: tx("Security"), sentence: tx("Locking the app, the vault, permissions and what may leave this PC.") },
  { id: "network", label: tx("Network"), sentence: tx("Going online, web search, the private connection and your other devices.") },
  { id: "phone", label: tx("Phone"), sentence: tx("Use the app from your phone, at home or anywhere, and chat apps like Telegram.") },
  { id: "models", label: tx("Models"), sentence: tx("Cloud models and API keys, where models are saved and how hard the PC works.") },
  { id: "voice", label: tx("Voice"), sentence: tx("Dictation, talking with the assistant, reading aloud and the wake word.") },
  { id: "appearance", label: tx("Appearance"), sentence: tx("Theme, layout and widgets.") },
  { id: "connections", label: tx("Accounts and mail"), sentence: tx("Microsoft and Google sign-in, Drive and OneDrive, mail and market data.") },
  { id: "files", label: tx("Files and backup"), sentence: tx("Shared folders, backups, your data and bringing things in from other apps.") },
  { id: "pc", label: tx("This PC"), sentence: tx("Checks, ports, cleanup, logs and options for experienced users.") },
  { id: "about", label: tx("About"), sentence: tx("Version, updates, crash reports and licenses.") },
];

/** Which category page shows each existing section (by its `settings-<id>` anchor).
 * Sections in "advanced" pages are folded under Advanced on their page. */
export const SECTION_CATEGORY: Record<string, { cat: CategoryId; advanced?: boolean }> = {
  appearance: { cat: "appearance" },
  language: { cat: "account" },
  chats: { cat: "account" },
  "about-you": { cat: "account" },
  security: { cat: "security" },
  permissions: { cat: "security" },
  "privacy-mode": { cat: "security" },
  sent: { cat: "security" },
  folders: { cat: "files" },
  "low-tokens": { cat: "models", advanced: true },
  handoffs: { cat: "files", advanced: true },
  voice: { cat: "voice" },
  performance: { cat: "models" },
  pause: { cat: "models" },
  check: { cat: "pc" },
  servers: { cat: "pc", advanced: true },
  storage: { cat: "models" },
  cleanup: { cat: "pc" },
  "sign-in": { cat: "connections" },
  connectivity: { cat: "network" },
  private: { cat: "network" },
  network: { cat: "network", advanced: true },
  devices: { cat: "network" },
  phone: { cat: "phone" },
  backup: { cat: "files" },
  "your-data": { cat: "files" },
  cloud: { cat: "models" },
  market: { cat: "connections", advanced: true },
  api: { cat: "pc", advanced: true },
  advanced: { cat: "pc", advanced: true },
  logs: { cat: "pc", advanced: true },
  about: { cat: "about" },
  "update-notes": { cat: "about" },
};

export function categoryOf(sectionId: string): CategoryId {
  return SECTION_CATEGORY[sectionId]?.cat ?? "pc";
}

/** "What's this?" for each section: plain words, shown under the section heading. */
export const WHATS_THIS: Record<string, string> = {
  appearance: tx("How the app looks. Nothing here changes what it does."),
  language: tx("The language of buttons and menus. The assistant always answers in the language you write in."),
  chats: tx("Small conveniences for chats, like keeping unsent drafts."),
  "about-you": tx("A short note sent with every chat so the assistant knows who you are. Encrypted on this PC."),
  security: tx("A PIN or Windows Hello to open the app, and the vault for passwords and keys."),
  permissions: tx("What the assistant may do without asking you first, like reading files or running programs."),
  "privacy-mode": tx("Hides personal details before anything goes to a website or cloud model."),
  sent: tx("A list of everything that left this PC, so you can check."),
  folders: tx("Folders the assistant may read and write. It can't see anything else."),
  voice: tx("Speaking to the assistant and having it speak back. Everything stays on this PC."),
  performance: tx("How hard the PC works: which graphics card, how many models at once, and keeping it cool."),
  pause: tx("Quiet times when the app doesn't start big jobs."),
  check: tx("A quick look at what this PC can run well."),
  storage: tx("The folder where downloaded models are kept, and how much space they use."),
  cleanup: tx("Free up space by removing caches and old files."),
  "sign-in": tx("Connect a Microsoft or Google account for mail, calendar and files. You sign in on their page, not here."),
  connectivity: tx("Whether the app may go online at all. Offline is the default."),
  private: tx("Send web traffic through Tor so sites don't see where you are."),
  devices: tx("Keep chats and settings in step between your own PCs."),
  phone: tx("Open the app from your phone's browser. The PC still does all the work."),
  backup: tx("Save everything to one encrypted file, and bring it back later."),
  "your-data": tx("See, export or delete what the app keeps."),
  cloud: tx("Use online models like Claude, GPT or Gemini with your own API key. Off unless you add one."),
  network: tx("Join private networks with your other PCs and friends' PCs."),
  about: tx("Version, updates and licenses."),
};

export interface GuideStep {
  text: string;
  /** A page to open in the browser for this step. */
  link?: string;
  /** The exact value to paste, shown with a Copy button. */
  value?: string;
}

export interface Guide {
  id: string;
  title: string;
  /** Which page the guide belongs to, and which section it finishes in. */
  cat: CategoryId;
  section: string;
  why: string;
  steps: GuideStep[];
}

export const GUIDES: Guide[] = [
  {
    id: "phone-anywhere",
    title: tx("Use your phone away from home"),
    cat: "phone",
    section: "phone",
    why: tx("Your phone reaches this PC through a small relay. The built-in one needs no account. Follow this only if you want your own relay on Cloudflare."),
    steps: [
      { text: tx("First try the easy way: in Phone below, turn on Reach this phone from anywhere and pair your phone. If that works, you are done.") },
      { text: tx("For your own relay, make a free Cloudflare account and confirm your email."), link: "https://dash.cloudflare.com/sign-up" },
      { text: tx("Install Node.js (the LTS version) if you don't have it, then restart this PC's terminal."), link: "https://nodejs.org/en/download" },
      { text: tx("Open a terminal in the relay folder of the SulcusAI source (download it from GitHub), then run this and sign in when the browser opens:"), value: "npx wrangler login" },
      { text: tx("Deploy the relay:"), value: "npx wrangler deploy" },
      { text: tx("The terminal prints an address like https://sulcusai-relay.yourname.workers.dev. Copy only that address: no path after it, no slash at the end.") },
      { text: tx("In Phone below, turn on Use my own Cloudflare worker, paste the address into the box and press Use this worker. It must start with https://.") },
      { text: tx("Pair the phone again so it learns the new relay.") },
    ],
  },
  {
    id: "sync-relay",
    title: tx("Sync your PCs when away from home"),
    cat: "network",
    section: "devices",
    why: tx("Your PCs exchange sealed messages through a relay you own. The relay can't read them."),
    steps: [
      { text: tx("Deploy the relay to Cloudflare as in Use your phone away from home (steps 2 to 5).") },
      { text: tx("Make up a long random secret, then store it on Cloudflare:"), value: "npx wrangler secret put RELAY_SECRET" },
      { text: tx("In Your devices below, under Sync when away from home, paste your address with /k/ and the secret after it:"), value: "https://sulcusai-relay.yourname.workers.dev/k/YOUR-SECRET" },
      { text: tx("Do the same on your other PC with the same address.") },
    ],
  },
  {
    id: "oracle-relay",
    title: tx("Host your own relay on Oracle (free)"),
    cat: "network",
    section: "network",
    why: tx("For people who want the relay on a server they control. Oracle's Always Free plan is enough. This switch is off by default."),
    steps: [
      { text: tx("Make an Oracle Cloud account. It asks for a card to check you are a person; Always Free resources are not charged."), link: "https://signup.cloud.oracle.com/" },
      { text: tx("In the Oracle console, create a compute instance with an Always Free shape (Ampere A1) and Ubuntu.") },
      { text: tx("Under the instance's subnet, add an ingress rule for TCP port 443.") },
      { text: tx("Point a domain name at the instance's public address, so the phone gets a valid certificate.") },
      { text: tx("Install the Sulcus Relay on the instance as its README says, then turn on Use my own relay in Phone and paste the https:// address.") },
    ],
  },
  {
    id: "key-anthropic",
    title: tx("Get an Anthropic (Claude) API key"),
    cat: "models",
    section: "cloud",
    why: tx("Lets you use Claude models from this app. Anthropic charges per use; set a budget in Cloud models."),
    steps: [
      { text: tx("Sign up or sign in at the Anthropic Console."), link: "https://console.anthropic.com/" },
      { text: tx("Add credit under Billing (a small amount is fine to start).") },
      { text: tx("Open API keys, press Create key, name it SulcusAI and copy the key. It starts with sk-ant-. It is shown once.") },
      { text: tx("In Cloud models below, pick Anthropic, paste the key and save. It is stored encrypted on this PC.") },
    ],
  },
  {
    id: "key-openai",
    title: tx("Get an OpenAI API key"),
    cat: "models",
    section: "cloud",
    why: tx("Lets you use GPT models from this app. OpenAI charges per use."),
    steps: [
      { text: tx("Sign in to the OpenAI platform."), link: "https://platform.openai.com/" },
      { text: tx("Add a payment method under Billing.") },
      { text: tx("Open API keys, press Create new secret key and copy it. It starts with sk-."), link: "https://platform.openai.com/api-keys" },
      { text: tx("In Cloud models below, pick OpenAI, paste the key and save.") },
    ],
  },
  {
    id: "key-google",
    title: tx("Get a Google Gemini API key"),
    cat: "models",
    section: "cloud",
    why: tx("Lets you use Gemini models. Google has a free tier with limits."),
    steps: [
      { text: tx("Open Google AI Studio and sign in with a Google account."), link: "https://aistudio.google.com/apikey" },
      { text: tx("Press Create API key and copy it. It starts with AIza.") },
      { text: tx("In Cloud models below, pick Google, paste the key and save.") },
    ],
  },
  {
    id: "key-openrouter",
    title: tx("Get an OpenRouter API key"),
    cat: "models",
    section: "cloud",
    why: tx("One key for many providers' models."),
    steps: [
      { text: tx("Sign in at OpenRouter and add credit."), link: "https://openrouter.ai/" },
      { text: tx("Open Keys, create a key and copy it. It starts with sk-or-."), link: "https://openrouter.ai/keys" },
      { text: tx("In Cloud models below, pick OpenRouter, paste the key and save.") },
    ],
  },
  {
    id: "key-brave",
    title: tx("Get a Brave Search API key"),
    cat: "network",
    section: "connectivity",
    why: tx("Only needed if DuckDuckGo limits your searches."),
    steps: [
      { text: tx("Sign up for the Brave Search API and pick the free plan."), link: "https://brave.com/search/api/" },
      { text: tx("In the dashboard, open API Keys and copy the key.") },
      { text: tx("In Connectivity below, set Search with to Brave Search, paste the key and press Save key.") },
    ],
  },
  {
    id: "mail",
    title: tx("Connect your mail"),
    cat: "connections",
    section: "sign-in",
    why: tx("The assistant can read, sort and draft mail. Nothing is sent without your OK."),
    steps: [
      { text: tx("Turn on Email in Features if it is off.") },
      { text: tx("For Outlook, Hotmail or a work account, press Sign in with Microsoft below. For Gmail, press Sign in with Google. Your browser opens their sign-in page.") },
      { text: tx("Sign in there and allow access. You type your password on their page, never in this app.") },
      { text: tx("Come back here: the account shows as connected. Other mail providers: add them in Mail with the IMAP address and an app password from your provider.") },
    ],
  },
  {
    id: "drive",
    title: tx("Connect Google Drive or OneDrive"),
    cat: "connections",
    section: "sign-in",
    why: tx("Lets the assistant find and read your cloud files when you ask."),
    steps: [
      { text: tx("Under Cloud files below, press Connect next to Google Drive or OneDrive.") },
      { text: tx("Sign in on the page that opens and allow access to files.") },
      { text: tx("Come back here; the account shows as connected. Disconnect any time.") },
    ],
  },
  {
    id: "telegram",
    title: tx("Talk to the assistant on Telegram"),
    cat: "phone",
    section: "phone",
    why: tx("Your own Telegram bot passes messages to this PC. Only you can use it."),
    steps: [
      { text: tx("In Telegram, open a chat with @BotFather."), link: "https://t.me/BotFather" },
      { text: tx("Send this and follow its questions (a name, then a username ending in bot):"), value: "/newbot" },
      { text: tx("BotFather replies with a token like 123456789:ABC... Copy the whole token.") },
      { text: tx("In Telegram below, paste the token and save.") },
      { text: tx("Open your new bot in Telegram and send it any message. Approve it here when asked, so only you can use it.") },
    ],
  },
  {
    id: "discord",
    title: tx("Talk to the assistant on Discord"),
    cat: "phone",
    section: "phone",
    why: tx("Your own Discord bot passes messages to this PC."),
    steps: [
      { text: tx("Open the Discord Developer Portal and press New Application."), link: "https://discord.com/developers/applications" },
      { text: tx("Open Bot, press Reset Token and copy the token. Turn on Message Content Intent.") },
      { text: tx("In Discord below, paste the token and save, then invite the bot with the link shown.") },
    ],
  },
];

export function guidesFor(cat: CategoryId): Guide[] {
  return GUIDES.filter((g) => g.cat === cat);
}

// SPDX-License-Identifier: AGPL-3.0-only
// Markdown in chat messages, as plain data: the view turns pieces into
// elements, so nothing a sender writes becomes HTML.

export type Inline =
  | { k: "text"; v: string }
  | { k: "bold" | "italic" | "strike" | "underline" | "spoiler"; c: Inline[] }
  | { k: "code"; v: string }
  | { k: "link"; href: string; v: string }
  | { k: "mention"; v: string };

export type Block = { k: "para"; c: Inline[] } | { k: "quote"; c: Inline[] } | { k: "code"; lang: string; v: string } | { k: "heading"; level: number; c: Inline[] } | { k: "list"; c: Inline[] };

const RULES: [RegExp, (m: RegExpExecArray) => Inline][] = [
  [/^`([^`\n]+)`/, (m) => ({ k: "code", v: m[1] })],
  [/^\*\*(.+?)\*\*/s, (m) => ({ k: "bold", c: inline(m[1]) })],
  [/^__(.+?)__/s, (m) => ({ k: "underline", c: inline(m[1]) })],
  [/^~~(.+?)~~/s, (m) => ({ k: "strike", c: inline(m[1]) })],
  [/^\|\|(.+?)\|\|/s, (m) => ({ k: "spoiler", c: inline(m[1]) })],
  [/^\*(?!\s)(.+?)\*/s, (m) => ({ k: "italic", c: inline(m[1]) })],
  [/^_(?!\s)(.+?)_(?![a-z0-9])/is, (m) => ({ k: "italic", c: inline(m[1]) })],
  [/^\[([^\]\n]+)\]\((https?:\/\/[^\s)]+)\)/, (m) => ({ k: "link", href: m[2], v: m[1] })],
  [/^<?(https?:\/\/[^\s<>]+[^\s<>.,;:!?'")\]])>?/, (m) => ({ k: "link", href: m[1], v: m[1] })],
  [/^@([\p{L}\p{N}_.-]+)/u, (m) => ({ k: "mention", v: m[1] })],
];

/** Inline markdown in one piece of text. */
export function inline(text: string): Inline[] {
  const out: Inline[] = [];
  let plain = "";
  let i = 0;
  while (i < text.length) {
    const rest = text.slice(i);
    if (rest[0] === "\\" && rest.length > 1 && /[\\`*_~|>#[\]()@-]/.test(rest[1])) {
      plain += rest[1];
      i += 2;
      continue;
    }
    // A rule only starts at a word edge, so snake_case stays as written.
    const edge = i === 0 || !/[\p{L}\p{N}]/u.test(text[i - 1]);
    let hit = false;
    if (edge || rest.startsWith("`") || rest.startsWith("**") || rest.startsWith("http")) {
      for (const [re, make] of RULES) {
        const m = re.exec(rest);
        if (m) {
          if (plain) out.push({ k: "text", v: plain });
          plain = "";
          out.push(make(m));
          i += m[0].length;
          hit = true;
          break;
        }
      }
    }
    if (!hit) {
      plain += text[i];
      i++;
    }
  }
  if (plain) out.push({ k: "text", v: plain });
  return out;
}

/** A whole message: code blocks, quotes, headings and list lines, then inline markdown. */
export function blocks(text: string): Block[] {
  const out: Block[] = [];
  const parts = text.split(/```/);
  parts.forEach((part, n) => {
    if (n % 2 === 1) {
      const nl = part.indexOf("\n");
      const lang = nl > 0 && /^[a-z0-9+#-]{1,20}$/i.test(part.slice(0, nl)) ? part.slice(0, nl) : "";
      out.push({ k: "code", lang, v: (lang ? part.slice(nl + 1) : part).replace(/^\n|\n$/g, "") });
      return;
    }
    let para: string[] = [];
    const flush = () => {
      if (para.length) out.push({ k: "para", c: inline(para.join("\n")) });
      para = [];
    };
    for (const line of part.split("\n")) {
      const quote = /^>\s?(.*)$/.exec(line);
      const head = /^(#{1,3})\s+(.+)$/.exec(line);
      const item = /^\s*[-*]\s+(.+)$/.exec(line);
      if (quote) {
        flush();
        out.push({ k: "quote", c: inline(quote[1]) });
      } else if (head) {
        flush();
        out.push({ k: "heading", level: head[1].length, c: inline(head[2]) });
      } else if (item) {
        flush();
        out.push({ k: "list", c: inline(item[1]) });
      } else para.push(line);
    }
    flush();
  });
  return out.filter((b) => b.k !== "para" || b.c.length > 0);
}

/** Every web address in a message, in order, once each. */
export function linksIn(text: string): string[] {
  const found: string[] = [];
  const walk = (xs: Inline[]) =>
    xs.forEach((x) => {
      if (x.k === "link" && !found.includes(x.href)) found.push(x.href);
      if ("c" in x) walk(x.c);
    });
  blocks(text).forEach((b) => "c" in b && walk(b.c));
  return found;
}

export type LinkKind = { k: "image" | "video" | "audio"; src: string } | { k: "youtube"; id: string } | { k: "player"; site: string; src: string } | { k: "page" };

/** What a link points at, from its address alone. */
export function linkKind(href: string): LinkKind {
  let u: URL;
  try {
    u = new URL(href);
  } catch {
    return { k: "page" };
  }
  const path = u.pathname.toLowerCase();
  if (/\.(png|jpe?g|gif|webp|avif)$/.test(path)) return { k: "image", src: href };
  if (/\.(mp4|webm|mov|m4v)$/.test(path)) return { k: "video", src: href };
  if (/\.(mp3|wav|ogg|m4a|flac|opus)$/.test(path)) return { k: "audio", src: href };
  const host = u.hostname.replace(/^www\.|^m\./, "");
  const yt =
    host === "youtu.be"
      ? u.pathname.replace(/^\/+|\/+$/g, "").split("/")[0]
      : host === "youtube.com"
      ? (u.searchParams.get("v") ?? /^\/(?:shorts|embed|live)\/([\w-]+)/.exec(u.pathname)?.[1] ?? "")
      : "";
  if (/^[\w-]{11}$/.test(yt)) return { k: "youtube", id: yt };
  // Other video sites, played by their own embed player.
  const id = (re: RegExp) => re.exec(u.pathname)?.[1];
  const vimeo = host === "vimeo.com" ? id(/^\/(\d+)/) : undefined;
  if (vimeo) return { k: "player", site: "Vimeo", src: `https://player.vimeo.com/video/${vimeo}` };
  const dm = host === "dailymotion.com" ? id(/^\/video\/([\w]+)/) : host === "dai.ly" ? id(/^\/([\w]+)/) : undefined;
  if (dm) return { k: "player", site: "Dailymotion", src: `https://www.dailymotion.com/embed/video/${dm}` };
  const st = host === "streamable.com" ? id(/^\/([\w]+)$/) : undefined;
  if (st) return { k: "player", site: "Streamable", src: `https://streamable.com/e/${st}` };
  const loom = host === "loom.com" ? id(/^\/share\/([\w]+)/) : undefined;
  if (loom) return { k: "player", site: "Loom", src: `https://www.loom.com/embed/${loom}` };
  const tw = host === "twitch.tv" ? id(/^\/videos\/(\d+)/) : undefined;
  if (tw) return { k: "player", site: "Twitch", src: `https://player.twitch.tv/?video=${tw}&parent=${location.hostname}&autoplay=true` };
  const twc = host === "clips.twitch.tv" ? id(/^\/([\w-]+)/) : undefined;
  if (twc) return { k: "player", site: "Twitch", src: `https://clips.twitch.tv/embed?clip=${twc}&parent=${location.hostname}` };
  return { k: "page" };
}

/** Slash commands typed in the message box. Returns the text to send, or null to send nothing. */
export function slash(text: string): { text: string } | { gif: string } | { error: string } | null {
  const m = /^\/(\w+)\s*([\s\S]*)$/.exec(text);
  if (!m) return { text };
  const [, cmd, arg] = m;
  switch (cmd.toLowerCase()) {
    case "shrug":
      return { text: `${arg} ¯\\\\\\_(ツ)\\_/¯`.trim() };
    case "tableflip":
      return { text: `${arg} (╯°□°)╯︵ ┻━┻`.trim() };
    case "unflip":
      return { text: `${arg} ┬─┬ノ( º _ ºノ)`.trim() };
    case "me":
      return arg ? { text: `_${arg}_` } : null;
    case "spoiler":
      return arg ? { text: `||${arg}||` } : null;
    case "gif":
      return { gif: arg };
    case "poll": {
      const parts = arg.split("|").map((x) => x.trim()).filter(Boolean);
      if (parts.length < 3) return { error: "Write /poll Question | first choice | second choice (up to 10 choices)." };
      return { text: makePoll(parts[0], parts.slice(1, 11)) };
    }
    default:
      return { error: `There's no /${cmd} command. Try /shrug, /me, /spoiler, /poll, /tableflip, /unflip or /gif.` };
  }
}

export const SLASH_COMMANDS: [string, string][] = [
  ["/shrug", "Adds a shrug to your message"],
  ["/me", "Writes your message as an action"],
  ["/spoiler", "Hides your message until it's clicked"],
  ["/tableflip", "Flips a table"],
  ["/unflip", "Puts the table back"],
  ["/gif", "Searches GIFs (when GIF search is on)"],
  ["/poll", "Asks a question with choices: /poll Question | one | two"],
];

// A poll is an ordinary message in a fixed shape, so older copies still show
// it as readable text; votes are reactions named "vote:N".
const POLL_HEAD = "Poll: ";

export function makePoll(question: string, choices: string[]): string {
  return POLL_HEAD + question + "
" + choices.map((c, i) => `${i + 1}. ${c}`).join("
");
}

export function parsePoll(text: string): { question: string; choices: string[] } | null {
  if (!text.startsWith(POLL_HEAD)) return null;
  const [first, ...rest] = text.slice(POLL_HEAD.length).split("
");
  const choices = rest.map((l, i) => (l.startsWith(`${i + 1}. `) ? l.slice(`${i + 1}. `.length) : null));
  if (choices.length < 2 || choices.some((c) => c === null)) return null;
  return { question: first, choices: choices as string[] };
}

export const isVote = (reaction: string) => reaction.startsWith("vote:");

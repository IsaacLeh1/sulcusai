// SPDX-License-Identifier: AGPL-3.0-only
// Emoji people send as content (never used as app icons). Each entry is the
// character and the words it's found by.

export type EmojiGroup = { name: string; items: [string, string][] };

const raw: [string, string][] = [
  ["Smileys", "😀 grin smile happy|😃 smile happy|😄 smile laugh|😁 grin teeth|😆 laugh squint|😅 sweat laugh relief|🤣 rofl rolling laugh|😂 joy tears laugh|🙂 slight smile|🙃 upside down|😉 wink|😊 blush smile|😇 halo innocent angel|🥰 love hearts|😍 heart eyes love|🤩 star struck|😘 kiss|😋 yum tasty|😛 tongue|😜 wink tongue|🤪 zany crazy|🤑 money|🤗 hug|🤭 oops giggle|🤫 shush quiet|🤔 think hmm|🤐 zip mouth|🤨 raised eyebrow|😐 neutral|😑 expressionless|😶 no mouth|😏 smirk|😒 unamused|🙄 eye roll|😬 grimace|😌 relieved|😔 pensive|😪 sleepy|🤤 drool|😴 sleep|😷 mask sick|🤒 thermometer ill|🤕 hurt bandage|🤢 nausea|🤮 vomit|🥵 hot|🥶 cold|🥴 woozy|😵 dizzy|🤯 mind blown|🤠 cowboy|🥳 party celebrate|😎 cool sunglasses|🤓 nerd|🧐 monocle|😕 confused|😟 worried|🙁 frown|😮 open mouth wow|😯 hushed|😲 astonished|😳 flushed|🥺 pleading|😦 frowning|😨 fearful|😰 anxious|😥 sad relieved|😢 cry|😭 sob|😱 scream|😖 confounded|😣 persevere|😞 disappointed|😓 sweat|😩 weary|😫 tired|🥱 yawn|😤 triumph huff|😡 angry pout|😠 angry|🤬 swear cursing|😈 devil smile|👿 imp|💀 skull dead|💩 poop|🤡 clown|👻 ghost|👽 alien|🤖 robot"],
  ["Gestures", "👍 thumbs up yes like +1|👎 thumbs down no|👌 ok|✌️ peace victory|🤞 fingers crossed luck|🤟 love you|🤘 rock|🤙 call me|👈 left|👉 right|👆 up|👇 down|☝️ point up|👋 wave hello bye|🤚 raised back|✋ hand stop high five|🖖 vulcan|👏 clap|🙌 raise hands hooray|👐 open hands|🤲 palms|🤝 handshake deal|🙏 pray thanks please|✍️ write|💪 muscle strong|🫶 heart hands|👀 eyes look|🧠 brain|👂 ear|👃 nose|👅 tongue|👄 lips"],
  ["Hearts", "❤️ heart love red|🧡 orange heart|💛 yellow heart|💚 green heart|💙 blue heart|💜 purple heart|🖤 black heart|🤍 white heart|🤎 brown heart|💔 broken heart|❣️ heart exclamation|💕 two hearts|💞 revolving hearts|💓 beating heart|💗 growing heart|💖 sparkling heart|💘 cupid|💝 heart gift"],
  ["Animals and nature", "🐶 dog|🐱 cat|🐭 mouse|🐹 hamster|🐰 rabbit bunny|🦊 fox|🐻 bear|🐼 panda|🐨 koala|🐯 tiger|🦁 lion|🐮 cow|🐷 pig|🐸 frog|🐵 monkey|🐔 chicken|🐧 penguin|🐦 bird|🦆 duck|🦅 eagle|🦉 owl|🐺 wolf|🐴 horse|🦄 unicorn|🐝 bee|🐛 bug|🦋 butterfly|🐌 snail|🐢 turtle|🐍 snake|🐙 octopus|🐬 dolphin|🐳 whale|🦈 shark|🌵 cactus|🌲 tree|🌴 palm|🍀 clover luck|🍁 maple leaf|🌸 blossom flower|🌹 rose|🌻 sunflower|🌞 sun|🌙 moon|⭐ star|🌈 rainbow|⚡ lightning|❄️ snow|🔥 fire lit|💧 drop water|🌊 wave ocean"],
  ["Food", "🍎 apple|🍌 banana|🍇 grapes|🍓 strawberry|🍉 watermelon|🍑 peach|🍍 pineapple|🥑 avocado|🥕 carrot|🌽 corn|🍞 bread|🧀 cheese|🍔 burger|🍟 fries|🍕 pizza|🌭 hot dog|🌮 taco|🍣 sushi|🍜 noodles ramen|🍝 spaghetti|🍰 cake|🎂 birthday cake|🍪 cookie|🍩 donut|🍫 chocolate|🍿 popcorn|☕ coffee|🍵 tea|🍺 beer|🍷 wine|🥤 drink|🧋 bubble tea"],
  ["Activities", "⚽ soccer football|🏀 basketball|🏈 american football|⚾ baseball|🎾 tennis|🏐 volleyball|🎱 pool|🏓 ping pong|🎮 game controller gaming|🕹️ joystick|🎲 dice|♟️ chess|🎯 target bullseye|🎳 bowling|🎸 guitar|🎹 piano|🥁 drum|🎤 microphone sing|🎧 headphones music|🎬 movie clapper|🎨 art palette|🏆 trophy win|🥇 gold medal first|🎉 party tada celebrate|🎊 confetti|🎁 gift present|🎈 balloon"],
  ["Objects and symbols", "💻 laptop computer|🖥️ desktop|📱 phone|⌨️ keyboard|🖱️ mouse|💾 save|📷 camera|📹 video camera|📺 tv|💡 idea bulb|🔒 lock|🔑 key|🔨 hammer|🛠️ tools|⚙️ gear|📌 pin|📎 paperclip|✏️ pencil|📝 memo note|📅 calendar|📈 chart up|📉 chart down|💰 money bag|💸 money fly|⏰ alarm clock|⌛ hourglass|🚀 rocket|✈️ plane|🚗 car|🏠 house home|✅ check done yes|❌ cross no|❓ question|❗ exclamation|⚠️ warning|💯 hundred|🆗 ok button|🆕 new|💤 zzz sleep|💬 speech chat|💭 thought|✨ sparkles|🎵 note music|➕ plus|➖ minus|🔁 repeat|👑 crown"],
];

export const EMOJI: EmojiGroup[] = raw.map(([name, list]) => ({
  name,
  items: list.split("|").map((e) => {
    const i = e.indexOf(" ");
    return [e.slice(0, i), e.slice(i + 1)] as [string, string];
  }),
}));

/** Emoji whose words start with every word of `query`. */
export function searchEmoji(query: string): string[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return [];
  return EMOJI.flatMap((g) => g.items)
    .filter(([, k]) => {
      const keys = k.split(" ");
      return words.every((w) => keys.some((x) => x.startsWith(w)));
    })
    .map(([e]) => e);
}

/** `:name:` written in a message becomes the emoji whose first word is that name. */
export function shortcodes(text: string): string {
  return text.replace(/:([a-z0-9_+-]{2,30}):/g, (all, name: string) => {
    const words = name.replace(/_/g, " ");
    for (const g of EMOJI) for (const [e, k] of g.items) if (k === words || k.startsWith(words + " ")) return e;
    return all;
  });
}

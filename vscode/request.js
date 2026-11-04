// SPDX-License-Identifier: AGPL-3.0-only
// Request building kept apart from the editor so it can be tested without VS Code.

const PROMPTS = {
  explain: "Explain what this code does, briefly.",
  tests: "Write unit tests for this code in the same language and test style.",
};

/** Only the loopback address: the key must never leave this PC. */
function endpoint(port) {
  const p = Number(port);
  if (!Number.isInteger(p) || p < 1 || p > 65535) throw new Error("The port must be a number from 1 to 65535.");
  return `http://127.0.0.1:${p}/v1/chat/completions`;
}

function body({ kind, question, code, language, fileName, model }) {
  const ask = kind === "ask" ? String(question || "").trim() : PROMPTS[kind];
  if (!ask) throw new Error("Nothing to ask.");
  const where = fileName ? ` from ${fileName}` : "";
  return {
    model: model || "default",
    stream: false,
    messages: [{ role: "user", content: `${ask}\n\nCode${where}:\n\`\`\`${language || ""}\n${code}\n\`\`\`` }],
  };
}

function answer(json) {
  const text = json && json.choices && json.choices[0] && json.choices[0].message && json.choices[0].message.content;
  if (typeof text !== "string") throw new Error("SulcusAI sent an answer this extension can't read.");
  return text;
}

module.exports = { endpoint, body, answer };

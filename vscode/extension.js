// SPDX-License-Identifier: AGPL-3.0-only
const vscode = require("vscode");
const { endpoint, body, answer } = require("./request");

const KEY = "sulcusai.apiKey";

async function run(context, kind) {
  const editor = vscode.window.activeTextEditor;
  if (!editor) return;
  const code = editor.document.getText(editor.selection.isEmpty ? undefined : editor.selection);
  let question;
  if (kind === "ask") {
    question = await vscode.window.showInputBox({ prompt: "Ask SulcusAI about this code" });
    if (!question) return;
  }
  const key = await context.secrets.get(KEY);
  if (!key) {
    vscode.window.showWarningMessage("Set the key from SulcusAI's Settings, Local API first (SulcusAI: Set the local API key).");
    return;
  }
  const cfg = vscode.workspace.getConfiguration("sulcusai");
  await vscode.window.withProgress({ location: vscode.ProgressLocation.Notification, title: "SulcusAI is thinking" }, async () => {
    try {
      const res = await fetch(endpoint(cfg.get("port")), {
        method: "POST",
        headers: { "Content-Type": "application/json", Authorization: `Bearer ${key}` },
        body: JSON.stringify(body({ kind, question, code, language: editor.document.languageId, fileName: vscode.workspace.asRelativePath(editor.document.uri), model: cfg.get("model") })),
      });
      if (!res.ok) throw new Error(res.status === 401 ? "The key was refused." : `SulcusAI answered ${res.status}.`);
      const doc = await vscode.workspace.openTextDocument({ language: "markdown", content: answer(await res.json()) });
      await vscode.window.showTextDocument(doc, vscode.ViewColumn.Beside);
    } catch (e) {
      vscode.window.showErrorMessage(e.cause ? "SulcusAI isn't reachable. Turn on the local API in its Settings." : e.message);
    }
  });
}

function activate(context) {
  for (const kind of ["ask", "explain", "tests"]) {
    context.subscriptions.push(vscode.commands.registerCommand(`sulcusai.${kind}`, () => run(context, kind)));
  }
  context.subscriptions.push(
    vscode.commands.registerCommand("sulcusai.setKey", async () => {
      const k = await vscode.window.showInputBox({ prompt: "SulcusAI local API key", password: true });
      if (k) await context.secrets.store(KEY, k.trim());
    }),
  );
}

module.exports = { activate, deactivate() {} };

// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api, errorText, type Folder, type PendingApproval, type ToolCall, type ToolMeta } from "../api";
import type { PushToast } from "./Toasts";

const TOOL_ICONS: Record<string, string> = {
  list_dir: "📂",
  read_file: "📄",
  find_files: "🔎",
  search_files: "🔎",
  write_file: "✏️",
  edit_file: "✏️",
  move_path: "↪️",
  make_folder: "📁",
  delete_path: "🗑️",
  run_command: "⌨️",
  remember: "🧠",
  search_memory: "🧠",
  delegate: "🤖",
};

/** A unified diff with added/removed lines colored. */
export function Diff({ text }: { text: string }) {
  return (
    <pre className="diff">
      {text.split("\n").map((line, i) => {
        const cls = line.startsWith("+++") || line.startsWith("---") ? "meta" : line.startsWith("+") ? "add" : line.startsWith("-") ? "del" : line.startsWith("@@") ? "hunk" : "";
        return (
          <span key={i} className={cls}>
            {line}
            {"\n"}
          </span>
        );
      })}
    </pre>
  );
}

function Detail({ kind, detail }: { kind: string; detail: string }) {
  if (kind === "diff") return <Diff text={detail} />;
  return <pre className="tool-output">{detail}</pre>;
}

/** One tool call and (once it has run) its result. */
export function ToolCard({ call, meta, output, running, liveSteps }: { call: ToolCall; meta?: ToolMeta; output?: string; running: boolean; liveSteps?: string[] }) {
  const [open, setOpen] = useState(false);
  const icon = call.name.startsWith("mcp__") ? "🔌" : TOOL_ICONS[call.name] ?? "🔧";
  const status = running ? "running" : meta?.status ?? "waiting";
  const title = meta?.title ?? describe(call);
  const detail = meta?.detail ?? (meta?.kind === "text" ? output : null);
  const expandable = !!detail || (!!output && call.name !== "read_file");
  return (
    <div className={`tool-card ${status}`}>
      <button className="tool-head" onClick={() => expandable && setOpen((o) => !o)} aria-expanded={open} disabled={!expandable}>
        <span aria-hidden>{icon}</span>
        <span className="tool-title ellipsis">{title}</span>
        <span className={`tool-status ${status}`}>
          {status === "running" ? "Running…" : status === "error" ? "Failed" : status === "denied" ? "Declined" : status === "waiting" ? "Waiting" : ""}
        </span>
        {expandable && <span className="chev" aria-hidden>{open ? "▾" : "▸"}</span>}
      </button>
      {running && liveSteps && liveSteps.length > 0 && (
        <ul className="helper-steps">
          {liveSteps.map((st, i) => (
            <li key={i}>{st}</li>
          ))}
        </ul>
      )}
      {open && call.name === "delegate" && output && <pre className="tool-output">{output}</pre>}
      {open && (detail ? <Detail kind={meta?.kind ?? "text"} detail={detail} /> : output && call.name !== "delegate" ? <pre className="tool-output">{output}</pre> : null)}
    </div>
  );
}

function describe(call: ToolCall): string {
  try {
    const a = JSON.parse(call.arguments || "{}");
    const target = a.path ?? a.pattern ?? a.command ?? a.from ?? "";
    return `${call.name.replace(/_/g, " ")} ${target}`.trim();
  } catch {
    return call.name.replace(/_/g, " ");
  }
}

/** Asks before a change or command runs. */
export function ApprovalCard({ p, onAnswered }: { p: PendingApproval; onAnswered: () => void }) {
  const [busy, setBusy] = useState(false);
  const answer = async (d: "allow" | "always" | "deny") => {
    setBusy(true);
    try {
      await api.answerApproval(p.call_id, d);
    } finally {
      onAnswered();
    }
  };
  const kindLabel = p.risk === "execute" ? "commands" : p.risk === "connector" ? "connector actions" : "file changes";
  return (
    <div className="approval" role="alert">
      <div className="approval-head">
        <span aria-hidden>{p.risk === "connector" ? "🔌" : TOOL_ICONS[p.tool] ?? "🔧"}</span>
        <strong>{p.preview.title}</strong>
      </div>
      {p.preview.detail && <Detail kind={p.preview.kind} detail={p.preview.detail} />}
      {p.preview.note && <p className="muted small">{p.preview.note}</p>}
      <div className="approval-actions">
        <button className="btn primary" disabled={busy} onClick={() => answer("allow")} autoFocus>
          Allow
        </button>
        <button className="btn" disabled={busy} onClick={() => answer("always")} title={`Don't ask again for ${kindLabel} in this chat`}>
          Allow {kindLabel} for this chat
        </button>
        <span className="spacer" />
        <button className="btn ghost danger" disabled={busy} onClick={() => answer("deny")}>
          Deny
        </button>
      </div>
    </div>
  );
}

/** Composer button: which folders the assistant may use. */
export function FolderMenu({ toast }: { toast: PushToast }) {
  const [openMenu, setOpenMenu] = useState(false);
  const [folders, setFolders] = useState<Folder[]>([]);
  const ref = useRef<HTMLDivElement>(null);
  const load = () => api.folders().then(setFolders).catch(() => {});

  useEffect(() => {
    load();
  }, []);
  useEffect(() => {
    if (!openMenu) return;
    const close = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) setOpenMenu(false);
    };
    window.addEventListener("mousedown", close);
    return () => window.removeEventListener("mousedown", close);
  }, [openMenu]);

  const add = async () => {
    const picked = await open({ directory: true, multiple: false, title: "Share a folder with SulcusAI" });
    if (typeof picked !== "string") return;
    try {
      await api.addFolder(picked);
      await load();
      toast("Folder shared. The assistant can now work in it.", "success");
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  return (
    <div className="folder-menu" ref={ref}>
      <button className={`globe ${folders.length ? "on" : ""}`} onClick={() => setOpenMenu((o) => !o)} title="Folders the assistant can use">
        📁 {folders.length ? `${folders.length} folder${folders.length === 1 ? "" : "s"}` : "No folders"}
      </button>
      {openMenu && (
        <div className="folder-pop">
          <p className="small muted">The assistant can read and change files only in these folders.</p>
          {folders.length === 0 && <p className="small">None yet.</p>}
          <ul>
            {folders.map((f) => (
              <li key={f.path}>
                <span className="ellipsis" title={f.path}>
                  📁 {f.name} {!f.exists && <span className="warn small">(missing)</span>}
                </span>
                <button
                  className="link"
                  onClick={async () => {
                    await api.removeFolder(f.path);
                    load();
                  }}
                >
                  Remove
                </button>
              </li>
            ))}
          </ul>
          <button className="btn small" onClick={add}>
            + Share a folder
          </button>
        </div>
      )}
    </div>
  );
}

/** Settings → Shared folders. */
export function FoldersSection({ toast }: { toast: PushToast }) {
  const [folders, setFolders] = useState<Folder[]>([]);
  const load = () => api.folders().then(setFolders).catch(() => {});
  useEffect(() => {
    load();
  }, []);
  const add = async () => {
    const picked = await open({ directory: true, multiple: false, title: "Share a folder with SulcusAI" });
    if (typeof picked !== "string") return;
    try {
      await api.addFolder(picked);
      load();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  return (
    <section className="card">
      <h2>Shared folders</h2>
      <p className="muted small">
        The assistant can read, change and run commands only inside these folders. Changes it makes can be undone from the chat;
        deleted files go to the Recycle Bin.
      </p>
      {folders.length === 0 && <p className="small">No folders shared yet.</p>}
      {folders.map((f) => (
        <div key={f.path} className="setting-row">
          <div>
            <strong>📁 {f.name}</strong>
            <span className="small muted block ellipsis" title={f.path}>
              {f.path}
              {!f.exists && " (missing)"}
            </span>
          </div>
          <button
            className="btn ghost danger"
            onClick={async () => {
              await api.removeFolder(f.path);
              load();
            }}
          >
            Stop sharing
          </button>
        </div>
      ))}
      <div>
        <button className="btn" onClick={add}>+ Share a folder</button>
      </div>
    </section>
  );
}

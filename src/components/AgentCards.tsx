// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api, errorText, type Folder, type PendingApproval, type ToolCall, type ToolMeta } from "../api";
import type { PushToast } from "./Toasts";
import { t } from "../i18n";

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
  create_document: "📄",
  read_document: "📄",
  browser_open: "🧭",
  browser_read: "🧭",
  browser_click: "👆",
  browser_type: "⌨️",
  browser_back: "↩️",
  create_image: "🎨",
  edit_image: "🖌",
  create_video: "🎬",
  create_music: "🎵",
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
          {status === "running" ? t("Running…") : status === "error" ? t("Failed") : status === "denied" ? t("Declined") : status === "waiting" ? t("Waiting") : ""}
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
      {meta?.file && status === "ok" && (
        <div className="tool-actions">
          <button className="btn small" onClick={() => api.openDocument(meta.file!).catch(() => {})}>{t("Open")}</button>
        </div>
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
  const always =
    p.risk === "execute"
      ? { title: t("Don't ask again for commands in this chat"), label: t("Allow commands for this chat") }
      : p.risk === "connector"
        ? { title: t("Don't ask again for connector actions in this chat"), label: t("Allow connector actions for this chat") }
        : { title: t("Don't ask again for file changes in this chat"), label: t("Allow file changes for this chat") };
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
          {t("Allow")}
        </button>
        {p.risk !== "submit" && (
          <button className="btn" disabled={busy} onClick={() => answer("always")} title={always.title}>
            {always.label}
          </button>
        )}
        <span className="spacer" />
        <button className="btn ghost danger" disabled={busy} onClick={() => answer("deny")}>
          {t("Deny")}
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
    const picked = await open({ directory: true, multiple: false, title: t("Share a folder with SulcusAI") });
    if (typeof picked !== "string") return;
    try {
      await api.addFolder(picked);
      await load();
      toast(t("Folder shared. The assistant can now work in it."), "success");
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  return (
    <div className="folder-menu" ref={ref}>
      <button className={`globe ${folders.length ? "on" : ""}`} onClick={() => setOpenMenu((o) => !o)} title={t("Folders the assistant can use")}>
        📁 {folders.length ? (folders.length === 1 ? t("1 folder") : t("{n} folders", { n: folders.length })) : t("No folders")}
      </button>
      {openMenu && (
        <div className="folder-pop">
          <p className="small muted">{t("The assistant can read and change files only in these folders.")}</p>
          {folders.length === 0 && <p className="small">{t("None yet.")}</p>}
          <ul>
            {folders.map((f) => (
              <li key={f.path}>
                <span className="ellipsis" title={f.path}>
                  📁 {f.name} {!f.exists && <span className="warn small">{t("(missing)")}</span>}
                </span>
                <button
                  className="link"
                  onClick={async () => {
                    await api.removeFolder(f.path);
                    load();
                  }}
                >
                  {t("Remove")}
                </button>
              </li>
            ))}
          </ul>
          <button className="btn small" onClick={add}>
            {t("+ Share a folder")}
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
    const picked = await open({ directory: true, multiple: false, title: t("Share a folder with SulcusAI") });
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
      <h2>{t("Shared folders")}</h2>
      <p className="muted small">
        {t("The assistant can read, change and run commands only inside these folders. Changes it makes can be undone from the chat; deleted files go to the Recycle Bin.")}
      </p>
      {folders.length === 0 && <p className="small">{t("No folders shared yet.")}</p>}
      {folders.map((f) => (
        <div key={f.path} className="setting-row">
          <div>
            <strong>📁 {f.name}</strong>
            <span className="small muted block ellipsis" title={f.path}>
              {f.path}
              {!f.exists && ` ${t("(missing)")}`}
            </span>
          </div>
          <button
            className="btn ghost danger"
            onClick={async () => {
              await api.removeFolder(f.path);
              load();
            }}
          >
            {t("Stop sharing")}
          </button>
        </div>
      ))}
      <div>
        <button className="btn" onClick={add}>{t("+ Share a folder")}</button>
      </div>
    </section>
  );
}

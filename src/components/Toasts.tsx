// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useMemo, useRef, useState } from "react";
import { t } from "../i18n";

export type ToastKind = "info" | "success" | "error";
export interface Toast {
  id: number;
  text: string;
  kind: ToastKind;
}
export type PushToast = (text: string, kind?: ToastKind) => void;

export function useToasts() {
  const [list, setList] = useState<Toast[]>([]);
  const next = useRef(1);
  const dismiss = useCallback((id: number) => setList((l) => l.filter((t) => t.id !== id)), []);
  const push = useCallback<PushToast>(
    (text, kind = "info") => {
      const id = next.current++;
      setList((l) => [...l, { id, text, kind }]);
      // Errors stay until dismissed; everything else fades out.
      if (kind !== "error") setTimeout(() => dismiss(id), 5000);
    },
    [dismiss],
  );
  return useMemo(() => ({ list, push, dismiss }), [list, push, dismiss]);
}

export function Toasts({ toasts, onDismiss }: { toasts: Toast[]; onDismiss: (id: number) => void }) {
  return (
    <div className="toasts" role="status" aria-live="polite">
      {toasts.map((item) => (
        <div key={item.id} className={`toast ${item.kind}`}>
          <span>{item.text}</span>
          <button className="icon-btn" aria-label={t("Dismiss")} onClick={() => onDismiss(item.id)}>
            ×
          </button>
        </div>
      ))}
    </div>
  );
}

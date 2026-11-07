// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useId, useRef, useState, type ReactNode } from "react";

const FOCUSABLE = 'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

/** Open dialogs, newest last: Escape and Tab belong to the top one. */
const stack: HTMLElement[] = [];

export function Modal({ title, onClose, children }: { title: string; onClose: () => void; children: ReactNode }) {
  const ref = useRef<HTMLDivElement>(null);
  const titleId = useId();
  const close = useRef(onClose);
  close.current = onClose;
  // Whatever had focus when the dialog opened (read while rendering: a
  // field inside with autoFocus takes focus before effects run).
  const [opener] = useState(() => document.activeElement as HTMLElement | null);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    stack.push(el);
    // Focus moves into the dialog (an autofocused field keeps it).
    if (!el.contains(document.activeElement)) (el.querySelector<HTMLElement>(FOCUSABLE) ?? el).focus();
    const onKey = (e: KeyboardEvent) => {
      if (stack[stack.length - 1] !== el) return;
      if (e.key === "Escape") {
        e.stopPropagation();
        close.current();
      } else if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
        // Enter does the dialog's one main thing, unless a field or button wants it.
        const at = document.activeElement;
        if (at instanceof HTMLTextAreaElement || at instanceof HTMLButtonElement || at instanceof HTMLSelectElement || at instanceof HTMLAnchorElement) return;
        if (at instanceof HTMLElement && at.isContentEditable) return;
        const main = el.querySelector<HTMLButtonElement>(".modal-actions button.primary:not([disabled]), .modal-actions button.danger-fill:not([disabled]), .ui-dialog-foot button.primary:not([disabled]), .ui-dialog-foot button.danger:not([disabled])");
        // A form handles its own Enter.
        if (main && !(at instanceof HTMLInputElement && at.form)) {
          e.preventDefault();
          main.click();
        }
      } else if (e.key === "Tab") {
        // Keep Tab inside the dialog.
        const items = [...el.querySelectorAll<HTMLElement>(FOCUSABLE)].filter((x) => x.offsetParent !== null);
        if (items.length === 0) return;
        const first = items[0];
        const last = items[items.length - 1];
        if (e.shiftKey && document.activeElement === first) {
          e.preventDefault();
          last.focus();
        } else if (!e.shiftKey && document.activeElement === last) {
          e.preventDefault();
          first.focus();
        }
      }
    };
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKey);
      stack.splice(stack.indexOf(el), 1);
      // Back to where the person was.
      if (opener && document.contains(opener)) opener.focus();
    };
  }, [opener]);

  return (
    <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal" role="dialog" aria-modal="true" aria-labelledby={titleId} ref={ref} tabIndex={-1}>
        <h2 id={titleId}>{title}</h2>
        {children}
      </div>
    </div>
  );
}

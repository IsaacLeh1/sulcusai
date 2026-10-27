// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useRef, useState } from "react";
import { t } from "../i18n";
import { CheckIcon, CopyIcon } from "./Icons";

/** A small button that copies `text` (or what `get` gives when clicked) and
 *  shows a tick for a moment. */
export function CopyButton({ text, get, label }: { text?: string; get?: () => string; label?: string }) {
  const [copied, setCopied] = useState(false);
  const timer = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(timer.current), []);
  const name = copied ? t("Copied") : label ?? t("Copy");
  return (
    <button
      className="icon-btn speak-btn"
      title={name}
      aria-label={name}
      onClick={() =>
        navigator.clipboard
          .writeText(get ? get() : text ?? "")
          .then(() => {
            setCopied(true);
            window.clearTimeout(timer.current);
            timer.current = window.setTimeout(() => setCopied(false), 1500);
          })
          .catch(() => {})
      }
    >
      {copied ? <CheckIcon /> : <CopyIcon />}
    </button>
  );
}

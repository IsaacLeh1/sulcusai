// SPDX-License-Identifier: AGPL-3.0-only
// Line icons for the mail screen, drawn in the text's colour (in place of
// emoji, which look different on every system).
import type { ReactNode } from "react";

export type MailIconName =
  | "inbox"
  | "unread"
  | "marked"
  | "sent"
  | "all"
  | "subscriptions"
  | "report"
  | "compose"
  | "search"
  | "archive"
  | "trash"
  | "clip"
  | "calendar";

const PATHS: Record<MailIconName, ReactNode> = {
  inbox: (
    <>
      <path d="M3 13h5l1.5 2.5h5L16 13h5" />
      <path d="M5.5 5h13L21 13v5.5a1.5 1.5 0 0 1-1.5 1.5h-15A1.5 1.5 0 0 1 3 18.5V13z" />
    </>
  ),
  unread: (
    <>
      <rect x="3" y="5.5" width="18" height="13" rx="2" />
      <path d="m3.5 7 8.5 6 8.5-6" />
    </>
  ),
  marked: (
    <>
      <circle cx="12" cy="12" r="8.5" />
      <path d="m8.2 12.2 2.6 2.6 5-5.2" />
    </>
  ),
  sent: (
    <>
      <path d="M20.5 3.5 10.5 13.5" />
      <path d="M20.5 3.5 14.2 20.5l-3.7-7-7-3.7z" />
    </>
  ),
  all: (
    <>
      <rect x="3.5" y="8.5" width="17" height="11.5" rx="1.8" />
      <path d="M6 5.5h12M8.5 2.5h7" />
    </>
  ),
  subscriptions: (
    <>
      <path d="M4.5 4.5h12v14a1.5 1.5 0 0 0 1.5 1.5H6a1.5 1.5 0 0 1-1.5-1.5z" />
      <path d="M16.5 8.5h3v10a1.5 1.5 0 0 1-3 0" />
      <path d="M7.5 8h6M7.5 11.5h6M7.5 15h4" />
    </>
  ),
  report: (
    <>
      <path d="M11 3.5c.6 3.9 1.9 5.2 5.8 5.8-3.9.6-5.2 1.9-5.8 5.8-.6-3.9-1.9-5.2-5.8-5.8 3.9-.6 5.2-1.9 5.8-5.8z" />
      <path d="M18 14.5c.3 1.6.8 2.1 2.4 2.4-1.6.3-2.1.8-2.4 2.4-.3-1.6-.8-2.1-2.4-2.4 1.6-.3 2.1-.8 2.4-2.4z" />
    </>
  ),
  compose: (
    <>
      <path d="M4 20h4L19 9a2.1 2.1 0 0 0-3-3L5 17z" />
      <path d="m14 7 3 3" />
    </>
  ),
  search: (
    <>
      <circle cx="10.5" cy="10.5" r="6" />
      <path d="m15 15 5 5" />
    </>
  ),
  archive: (
    <>
      <rect x="3" y="4" width="18" height="4.5" rx="1" />
      <path d="M5 8.5V19a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1V8.5" />
      <path d="M10 12.5h4" />
    </>
  ),
  trash: (
    <>
      <path d="M4 7h16" />
      <path d="M9.5 7V4.5h5V7" />
      <path d="m6.5 7 1 12.5a1 1 0 0 0 1 .9h7a1 1 0 0 0 1-.9l1-12.5" />
    </>
  ),
  clip: <path d="m20 11.5-7.8 7.8a4.6 4.6 0 0 1-6.5-6.5l8.4-8.4a3 3 0 0 1 4.3 4.3l-8.4 8.4a1.5 1.5 0 0 1-2.1-2.1l7.6-7.6" />,
  calendar: (
    <>
      <rect x="3.5" y="5" width="17" height="15" rx="2" />
      <path d="M3.5 9.5h17M8 3v4M16 3v4" />
    </>
  ),
};

/** A mail icon. Decorative unless `title` is given (then it's read out). */
export function MailIcon({ name, size = 18, title }: { name: MailIconName; size?: number | string; title?: string }) {
  return (
    <svg
      className="mail-icon"
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.8}
      strokeLinecap="round"
      strokeLinejoin="round"
      role={title ? "img" : undefined}
      aria-hidden={title ? undefined : true}
      aria-label={title}
    >
      {title && <title>{title}</title>}
      {PATHS[name]}
    </svg>
  );
}

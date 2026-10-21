// SPDX-License-Identifier: AGPL-3.0-only
// Small line icons for buttons and markers with no room for a word. Each is
// drawn on a 24×24 grid in the text's colour, about the size of a letter.
// Without a title an icon is hidden from screen readers (its button or the
// text beside it says what it means); with one, that's its name and tooltip.
import type { ReactNode } from "react";

export interface IconProps {
  size?: string | number;
  title?: string;
  className?: string;
}

function Icon({ size = "1em", title, className, children }: IconProps & { children: ReactNode }) {
  return (
    <svg
      className={className ? `icon ${className}` : "icon"}
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={2}
      strokeLinecap="round"
      strokeLinejoin="round"
      role={title ? "img" : undefined}
      aria-hidden={title ? undefined : true}
      aria-label={title}
    >
      {title && <title>{title}</title>}
      {children}
    </svg>
  );
}

/** A padlock; `open` draws it unlocked. */
export function LockIcon({ open = false, ...p }: IconProps & { open?: boolean }) {
  return (
    <Icon {...p}>
      <rect x="5" y="11" width="14" height="10" rx="2" />
      <path d={open ? "M8 11V7a4 4 0 0 1 7.7-1.5" : "M8 11V7a4 4 0 0 1 8 0v4"} />
    </Icon>
  );
}

/** The web. */
export function GlobeIcon(p: IconProps) {
  return (
    <Icon {...p}>
      <circle cx="12" cy="12" r="9" />
      <path d="M3 12h18" />
      <path d="M12 3c2.4 2.5 3.7 5.5 3.7 9s-1.3 6.5-3.7 9c-2.4-2.5-3.7-5.5-3.7-9s1.3-6.5 3.7-9z" />
    </Icon>
  );
}

/** Three lines: opens the menu (on a phone). */
export function MenuIcon(p: IconProps) {
  return (
    <Icon {...p}>
      <path d="M4 7h16M4 12h16M4 17h16" />
    </Icon>
  );
}

/** A computer screen: the PC (on a phone). */
export function PcIcon(p: IconProps) {
  return (
    <Icon {...p}>
      <rect x="3" y="4" width="18" height="12" rx="2" />
      <path d="M8 20h8M12 16v4" />
    </Icon>
  );
}

export function FolderIcon(p: IconProps) {
  return (
    <Icon {...p}>
      <path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
    </Icon>
  );
}

/** Two people: sharing with others. */
export function PeopleIcon(p: IconProps) {
  return (
    <Icon {...p}>
      <circle cx="9" cy="8" r="3.5" />
      <path d="M2.5 20a6.5 6.5 0 0 1 13 0" />
      <path d="M16 4.6a3.5 3.5 0 0 1 0 6.8" />
      <path d="M18 14.2a6.5 6.5 0 0 1 3.5 5.8" />
    </Icon>
  );
}

/** A book: a project. */
export function BookIcon(p: IconProps) {
  return (
    <Icon {...p}>
      <path d="M5 4.5a2 2 0 0 1 2-2h12V17H7a2 2 0 0 0-2 2z" />
      <path d="M5 19a2 2 0 0 0 2 2h12v-4" />
    </Icon>
  );
}

/** A push pin: pinned to the top. */
export function PinIcon(p: IconProps) {
  return (
    <Icon {...p}>
      <path d="M9 3h6l-1 6 3 4H7l3-4z" />
      <path d="M12 13v8" />
    </Icon>
  );
}

export function WarningIcon(p: IconProps) {
  return (
    <Icon {...p}>
      <path d="M10.3 3.9 2.4 18a2 2 0 0 0 1.7 3h15.8a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z" />
      <path d="M12 9v4" />
      <path d="M12 17h.01" />
    </Icon>
  );
}

/** Attaching a file or picture. */
export function PaperclipIcon(p: IconProps) {
  return (
    <Icon {...p}>
      <path d="M21 11.5 12.5 20a5 5 0 0 1-7-7l8.5-8.5a3.3 3.3 0 0 1 4.7 4.7l-8.5 8.5a1.7 1.7 0 0 1-2.4-2.4l7.7-7.7" />
    </Icon>
  );
}

/** Dictation. */
export function MicIcon(p: IconProps) {
  return (
    <Icon {...p}>
      <rect x="9" y="3" width="6" height="11" rx="3" />
      <path d="M5 11a7 7 0 0 0 14 0" />
      <path d="M12 18v3" />
    </Icon>
  );
}

/** Reading aloud. */
export function SpeakerIcon(p: IconProps) {
  return (
    <Icon {...p}>
      <path d="M4 9.5h3.5L12 5v14l-4.5-4.5H4z" />
      <path d="M16 9a4 4 0 0 1 0 6" />
      <path d="M18.8 6.2a8 8 0 0 1 0 11.6" />
    </Icon>
  );
}

/** Stopping something that plays. */
export function StopIcon(p: IconProps) {
  return (
    <Icon {...p}>
      <rect x="6" y="6" width="12" height="12" rx="2" fill="currentColor" />
    </Icon>
  );
}

/** A sound wave: voice chat. */
export function VoiceIcon(p: IconProps) {
  return (
    <Icon {...p}>
      <path d="M4 10v4M8 6v12M12 3v18M16 7v10M20 10v4" />
    </Icon>
  );
}

/** A screenshot. */
export function CameraIcon(p: IconProps) {
  return (
    <Icon {...p}>
      <path d="M4 7h3l2-3h6l2 3h3a1 1 0 0 1 1 1v11a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V8a1 1 0 0 1 1-1z" />
      <circle cx="12" cy="13" r="4" />
    </Icon>
  );
}

/** Settings. */
export function GearIcon(p: IconProps) {
  return (
    <Icon {...p}>
      <circle cx="12" cy="12" r="6" />
      <circle cx="12" cy="12" r="2.2" />
      <path strokeWidth={3} strokeLinecap="butt" d="M12 2.5V6M12 18v3.5M2.5 12H6M18 12h3.5M5.3 5.3l2.5 2.5M16.2 16.2l2.5 2.5M5.3 18.7l2.5-2.5M16.2 7.8l2.5-2.5" />
    </Icon>
  );
}

/** A reminder. */
export function BellIcon(p: IconProps) {
  return (
    <Icon {...p}>
      <path d="M6 16v-5a6 6 0 0 1 12 0v5l1.5 2h-15z" />
      <path d="M10.3 21a2 2 0 0 0 3.4 0" />
    </Icon>
  );
}

/** Light look. */
export function SunIcon(p: IconProps) {
  return (
    <Icon {...p}>
      <circle cx="12" cy="12" r="4" />
      <path d="M12 2v2M12 20v2M2 12h2M20 12h2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4" />
    </Icon>
  );
}

/** Dark look. */
export function MoonIcon(p: IconProps) {
  return (
    <Icon {...p}>
      <path d="M20 14A8 8 0 1 1 10 4a6 6 0 0 0 10 10z" />
    </Icon>
  );
}

/** Following Windows' look: half light, half dark. */
export function HalfCircleIcon(p: IconProps) {
  return (
    <Icon {...p}>
      <circle cx="12" cy="12" r="8" />
      <path d="M12 4a8 8 0 0 0 0 16z" fill="currentColor" />
    </Icon>
  );
}

/** True black. */
export function FullCircleIcon(p: IconProps) {
  return (
    <Icon {...p}>
      <circle cx="12" cy="12" r="8" fill="currentColor" />
    </Icon>
  );
}

// SPDX-License-Identifier: AGPL-3.0-only
// Drawn icons for Social's buttons (the app uses drawings, not emoji).
type P = { size?: number };

const Svg = ({ size = 20, children }: P & { children: React.ReactNode }) => (
  <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
    {children}
  </svg>
);

export const PlusCircleIcon = (p: P) => (
  <Svg {...p}>
    <circle cx="12" cy="12" r="10" fill="currentColor" stroke="none" />
    <path d="M12 7v10M7 12h10" stroke="var(--social-chat, #fff)" strokeWidth="2.4" />
  </Svg>
);
export const GifIcon = (p: P) => (
  <Svg {...p}>
    <rect x="2" y="5" width="20" height="14" rx="3" />
    <path d="M10 10H8a1 1 0 00-1 1v2a1 1 0 001 1h2v-2H9M13 10v4M16 14v-4h3M16 12h2" strokeWidth="1.6" />
  </Svg>
);
export const StickerIcon = (p: P) => (
  <Svg {...p}>
    <path d="M14 3H6a3 3 0 00-3 3v12a3 3 0 003 3h8l7-7V6a3 3 0 00-3-3z" />
    <path d="M14 21v-4a3 3 0 013-3h4" />
  </Svg>
);
export const SmileIcon = (p: P) => (
  <Svg {...p}>
    <circle cx="12" cy="12" r="10" />
    <path d="M8 14s1.5 2 4 2 4-2 4-2" />
    <path d="M9 9h.01M15 9h.01" strokeWidth="3" />
  </Svg>
);
export const PinIcon = (p: P) => (
  <Svg {...p}>
    <path d="M12 17v5M9 3h6l-1 6 3 3v2H7v-2l3-3z" />
  </Svg>
);
export const MembersIcon = (p: P) => (
  <Svg {...p}>
    <circle cx="9" cy="8" r="3.5" />
    <path d="M2 20c0-3.5 3-6 7-6s7 2.5 7 6" />
    <path d="M16 4.5a3.5 3.5 0 010 7M18 14c2.5.6 4 2.8 4 6" />
  </Svg>
);
export const SearchIcon = (p: P) => (
  <Svg {...p}>
    <circle cx="11" cy="11" r="7" />
    <path d="M20 20l-3.5-3.5" />
  </Svg>
);
export const SparkIcon = (p: P) => (
  <Svg {...p}>
    <path d="M12 3l1.8 5.2L19 10l-5.2 1.8L12 17l-1.8-5.2L5 10l5.2-1.8zM19 16l.8 2.2L22 19l-2.2.8L19 22l-.8-2.2L16 19l2.2-.8z" />
  </Svg>
);
export const UnreadIcon = (p: P) => (
  <Svg {...p}>
    <path d="M4 6h16M4 12h10M4 18h7" />
    <circle cx="19" cy="17" r="3" fill="currentColor" stroke="none" />
  </Svg>
);
export const MicIcon = ({ off, ...p }: P & { off?: boolean }) => (
  <Svg {...p}>
    <rect x="9" y="2" width="6" height="12" rx="3" />
    <path d="M5 10a7 7 0 0014 0M12 17v4M8 21h8" />
    {off && <path d="M3 3l18 18" stroke="var(--danger)" />}
  </Svg>
);
export const HeadphonesIcon = ({ off, ...p }: P & { off?: boolean }) => (
  <Svg {...p}>
    <path d="M3 18v-6a9 9 0 0118 0v6" />
    <path d="M21 19a2 2 0 01-2 2h-1v-6h3zM3 19a2 2 0 002 2h1v-6H3z" />
    {off && <path d="M3 3l18 18" stroke="var(--danger)" />}
  </Svg>
);
export const CogIcon = (p: P) => (
  <Svg {...p}>
    <circle cx="12" cy="12" r="3" />
    <path d="M19.4 15a1.7 1.7 0 00.3 1.8l.1.1a2 2 0 11-2.8 2.8l-.1-.1a1.7 1.7 0 00-1.8-.3 1.7 1.7 0 00-1 1.5V21a2 2 0 11-4 0v-.1a1.7 1.7 0 00-1.1-1.5 1.7 1.7 0 00-1.8.3l-.1.1a2 2 0 11-2.8-2.8l.1-.1a1.7 1.7 0 00.3-1.8 1.7 1.7 0 00-1.5-1H3a2 2 0 110-4h.1a1.7 1.7 0 001.5-1.1 1.7 1.7 0 00-.3-1.8l-.1-.1a2 2 0 112.8-2.8l.1.1a1.7 1.7 0 001.8.3H9a1.7 1.7 0 001-1.5V3a2 2 0 114 0v.1a1.7 1.7 0 001 1.5 1.7 1.7 0 001.8-.3l.1-.1a2 2 0 112.8 2.8l-.1.1a1.7 1.7 0 00-.3 1.8V9a1.7 1.7 0 001.5 1H21a2 2 0 110 4h-.1a1.7 1.7 0 00-1.5 1z" />
  </Svg>
);
export const CameraIcon = (p: P) => (
  <Svg {...p}>
    <rect x="2" y="6" width="14" height="12" rx="2" />
    <path d="M16 10l6-3v10l-6-3z" />
  </Svg>
);
export const ScreenIcon = (p: P) => (
  <Svg {...p}>
    <rect x="2" y="3" width="20" height="14" rx="2" />
    <path d="M8 21h8M12 17v4M12 7v6M9 10l3-3 3 3" />
  </Svg>
);
export const MicRecordIcon = (p: P) => <MicIcon {...p} />;

export function HangUpIcon() {
  return (
    <svg width="18" height="18" viewBox="0 0 24 24" aria-hidden>
      <path fill="currentColor" d="M12 9c-1.6 0-3.2.3-4.6.8v3.1c0 .4-.2.7-.6.9-1 .5-1.9 1.1-2.6 1.8-.2.2-.4.3-.7.3s-.5-.1-.7-.3L.3 13.1c-.2-.2-.3-.4-.3-.7s.1-.5.3-.7C3.3 8.9 7.4 7 12 7s8.7 1.9 11.7 4.7c.2.2.3.4.3.7s-.1.5-.3.7l-2.5 2.5c-.2.2-.4.3-.7.3s-.5-.1-.7-.3c-.8-.7-1.7-1.3-2.6-1.8-.3-.2-.6-.5-.6-.9V9.8C15.2 9.3 13.6 9 12 9z" />
    </svg>
  );
}

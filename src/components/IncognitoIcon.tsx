// SPDX-License-Identifier: AGPL-3.0-only
/** Incognito: a brimmed hat over a pair of round glasses, in the text's colour. */
export function IncognitoIcon({ size = "1.15em", title }: { size?: string | number; title?: string }) {
  return (
    <svg
      className="incognito-icon"
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="currentColor"
      role={title ? "img" : undefined}
      aria-hidden={title ? undefined : true}
      aria-label={title}
    >
      {title && <title>{title}</title>}
      {/* Crown, dipped in the middle, and its brim. */}
      <path d="M6.6 10.2 8.1 4.9c.3-1 1.3-1.5 2.2-1.1l1.7.7 1.7-.7c.9-.4 1.9.1 2.2 1.1l1.5 5.3z" />
      <rect x="1.5" y="10.6" width="21" height="2.3" rx="1.15" />
      {/* Glasses. */}
      <g fill="none" stroke="currentColor" strokeWidth="1.9">
        <circle cx="7.2" cy="17.4" r="2.9" />
        <circle cx="16.8" cy="17.4" r="2.9" />
        <path d="M10.1 17.1c1.2-.9 2.6-.9 3.8 0" strokeLinecap="round" />
      </g>
    </svg>
  );
}

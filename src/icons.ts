// Theme-aware inline SVG icons. All use stroke="currentColor" / fill="currentColor"
// so they follow the surrounding text color in both light and dark themes —
// unlike emoji, which render with fixed multicolor glyphs that clash with the
// chrome. Each export is an SVG string sized to a 16px box.

const svg = (body: string, opts: { fill?: boolean } = {}): string =>
  `<svg viewBox="0 0 24 24" width="16" height="16" fill="${
    opts.fill ? "currentColor" : "none"
  }" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${body}</svg>`;

// Padlock — closed (secure). Hollow outline, tinted by currentColor.
export const lockClosed = svg(
  `<rect x="4" y="11" width="16" height="10" rx="2"/><path d="M8 11V7a4 4 0 0 1 8 0v4"/>`
);
// Padlock — open (insecure http).
export const lockOpen = svg(
  `<rect x="4" y="11" width="16" height="10" rx="2"/><path d="M8 11V7a4 4 0 0 1 7.5-2"/>`
);
// Pin — outline (unpinned).
export const pin = svg(
  `<path d="M12 17v5"/><path d="M9 3h6l-1 6 3 3H7l3-3-1-6z"/>`
);
// Pin — filled (pinned/active).
export const pinFilled = svg(
  `<path d="M12 17v5" stroke="currentColor"/><path d="M9 3h6l-1 6 3 3H7l3-3-1-6z"/>`,
  { fill: true }
);
// Tab / window.
export const tab = svg(`<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M3 9h18"/>`);
// Bookmark / star.
export const bookmark = svg(
  `<path d="M12 3l2.9 5.9 6.1.9-4.5 4.4 1 6.1L12 17.8 6.5 20.3l1-6.1L3 9.8l6.1-.9L12 3z"/>`
);
// History / clock.
export const history = svg(`<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/>`);
// Search / magnifier.
export const search = svg(`<circle cx="11" cy="11" r="7"/><path d="M21 21l-4.3-4.3"/>`);
// Muted speaker.
export const muted = svg(
  `<path d="M11 5 6 9H3v6h3l5 4V5z"/><path d="M22 9l-6 6"/><path d="M16 9l6 6"/>`
);
// Globe (generic page fallback favicon).
export const globe = svg(`<circle cx="12" cy="12" r="9"/><path d="M3 12h18"/><path d="M12 3a15 15 0 0 1 0 18a15 15 0 0 1 0-18z"/>`);

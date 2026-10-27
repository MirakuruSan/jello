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
// Bookmark — filled (page is bookmarked).
export const bookmarkFilled = svg(
  `<path d="M12 3l2.9 5.9 6.1.9-4.5 4.4 1 6.1L12 17.8 6.5 20.3l1-6.1L3 9.8l6.1-.9L12 3z"/>`,
  { fill: true }
);
// Navigation.
export const back = svg(`<path d="M15 5l-7 7 7 7"/>`);
export const forward = svg(`<path d="M9 5l7 7-7 7"/>`);
export const reload = svg(`<path d="M20 12a8 8 0 1 1-2.34-5.66"/><path d="M20 4v5h-5"/>`);
export const chevronUp = svg(`<path d="M6 15l6-6 6 6"/>`);
export const chevronDown = svg(`<path d="M6 9l6 6 6-6"/>`);
// Actions.
export const plus = svg(`<path d="M12 5v14"/><path d="M5 12h14"/>`);
export const more = svg(
  `<circle cx="5" cy="12" r="1.2" fill="currentColor"/><circle cx="12" cy="12" r="1.2" fill="currentColor"/><circle cx="19" cy="12" r="1.2" fill="currentColor"/>`
);
export const close = svg(`<path d="M6 6l12 12"/><path d="M18 6L6 18"/>`);
// Picture-in-picture: a small window inset in a frame.
export const pip = svg(`<rect x="3" y="5" width="18" height="14" rx="2"/><rect x="12" y="12" width="7" height="5" rx="1" fill="currentColor" stroke="none"/>`);
export const download = svg(`<path d="M12 4v11"/><path d="M7 10l5 5 5-5"/><path d="M5 20h14"/>`);
// Window controls (drawn lighter, like the OS caption buttons).
export const minimize = svg(`<path d="M5 12h14"/>`);
export const maximize = svg(`<rect x="5" y="5" width="14" height="14" rx="2"/>`);
// Settings: a gear.
export const settings = svg(
  `<circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3h0a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8v0a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z"/>`
);
// Reopen closed tab: a curved arrow back.
export const reopen = svg(`<path d="M9 14L4 9l5-5"/><path d="M4 9h10.5a5.5 5.5 0 0 1 0 11H11"/>`);
// Screenshot: a camera.
export const camera = svg(`<path d="M3 8a2 2 0 0 1 2-2h2l2-2h6l2 2h2a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/><circle cx="12" cy="13" r="3.5"/>`);

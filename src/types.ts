// Shared IPC types (TS mirror of src-tauri/src/ipc_types.rs)
// Field names must be byte-identical (camelCase) per SPEC §B.4

export interface Tab {
  id: number;
  windowId: number;
  url: string;
  title: string | null;
  faviconId: number | null;
  pinned: boolean;
  muted: boolean;
  orderKey: string;
  scrollY: number;
  lastActive: number | null;
  createdAt: number;
}

export interface Extension {
  id: string;
  version: string;
  name: string;
  enabled: boolean;
}

export type TabState = "live" | "suspended" | "cold";

export interface HitRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface PaletteItem {
  id: string;
  itemType: string; // "tab" | "history" | "bookmark" | "search"
  title: string;
  url: string;
  matchedRanges: [number, number][];
}

export interface PaletteResults {
  openTabs: PaletteItem[];
  history: PaletteItem[];
  bookmarks: PaletteItem[];
}

export interface MonitorCaptureInfo {
  index: number;
  name: string;
  x: number;
  y: number;
  width: number;
  height: number;
  scaleFactor: number;
  imagePath: string;
}

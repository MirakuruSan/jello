// Lightweight overlay context menu (Phase 5). Pure DOM — no COM. The menu div
// carries `.interactive` so the overlay's hit-rect region includes it (clicks
// land instead of passing through to the page); opening/closing dispatches
// `overlay:layout` so main.ts recomputes the region.

export interface MenuItem {
  label: string;
  onClick: () => void;
  disabled?: boolean;
  danger?: boolean;
}
export type MenuEntry = MenuItem | "separator";

let current: HTMLElement | null = null;
let teardown: (() => void) | null = null;

export function closeContextMenu(): void {
  if (teardown) {
    teardown();
    teardown = null;
  }
  if (current) {
    current.remove();
    current = null;
    window.dispatchEvent(new Event("overlay:layout"));
  }
}

export function showContextMenu(x: number, y: number, entries: MenuEntry[]): void {
  closeContextMenu();
  const menu = document.createElement("div");
  menu.className = "context-menu interactive";

  for (const entry of entries) {
    if (entry === "separator") {
      const sep = document.createElement("div");
      sep.className = "context-menu-sep";
      menu.appendChild(sep);
      continue;
    }
    const item = document.createElement("button");
    item.className = "context-menu-item" + (entry.danger ? " danger" : "");
    item.textContent = entry.label;
    if (entry.disabled) {
      item.disabled = true;
    } else {
      item.addEventListener("click", (ev) => {
        ev.stopPropagation();
        closeContextMenu();
        entry.onClick();
      });
    }
    menu.appendChild(item);
  }

  document.body.appendChild(menu);
  current = menu;

  // Clamp inside the viewport.
  const r = menu.getBoundingClientRect();
  const left = Math.max(4, Math.min(x, window.innerWidth - r.width - 8));
  const top = Math.max(4, Math.min(y, window.innerHeight - r.height - 8));
  menu.style.left = `${left}px`;
  menu.style.top = `${top}px`;

  // Region must include the menu now that it exists and is positioned.
  window.dispatchEvent(new Event("overlay:layout"));

  // Dismiss on outside-click / Escape / scroll. Capture phase so it wins.
  const onDown = (ev: MouseEvent) => {
    if (current && !current.contains(ev.target as Node)) closeContextMenu();
  };
  const onKey = (ev: KeyboardEvent) => {
    if (ev.key === "Escape") closeContextMenu();
  };
  const onScroll = () => closeContextMenu();
  // Defer attaching so the opening right-click doesn't immediately dismiss.
  setTimeout(() => {
    document.addEventListener("mousedown", onDown, true);
    document.addEventListener("keydown", onKey, true);
    window.addEventListener("scroll", onScroll, true);
  }, 0);
  teardown = () => {
    document.removeEventListener("mousedown", onDown, true);
    document.removeEventListener("keydown", onKey, true);
    window.removeEventListener("scroll", onScroll, true);
  };
}

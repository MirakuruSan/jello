// Overlay menus: the main menu and the right-click menus on chrome (tab rows,
// address pill). Pure DOM — no COM. The menu carries `.interactive` so the
// overlay's hit-rect region includes it (clicks land instead of passing
// through to the page); opening/closing dispatches `overlay:layout` so
// main.ts recomputes the region.

export interface MenuItem {
  label: string;
  onClick: () => void;
  disabled?: boolean;
  danger?: boolean;
  /** Inline SVG string (see icons.ts). */
  icon?: string;
  /** Shortcut hint shown on the right, e.g. "Ctrl+T". */
  shortcut?: string;
  /** Shows a check mark; for toggles. */
  checked?: boolean;
}

/** A row the caller builds itself (e.g. the zoom stepper). */
export interface MenuCustom {
  render: () => HTMLElement;
}

export type MenuEntry = MenuItem | MenuCustom | "separator";

let current: HTMLElement | null = null;
let teardown: (() => void) | null = null;
/// The button that opened the current menu: pressing it again must toggle the
/// menu closed, not count as an outside click that closes and then reopens it.
let anchorEl: HTMLElement | null = null;

export function closeContextMenu(): void {
  if (teardown) {
    teardown();
    teardown = null;
  }
  anchorEl = null;
  if (current) {
    current.remove();
    current = null;
    window.dispatchEvent(new Event("overlay:layout"));
  }
}

export function isMenuOpen(): boolean {
  return current !== null;
}

function buildItem(entry: MenuItem): HTMLButtonElement {
  const item = document.createElement("button");
  item.className = "context-menu-item" + (entry.danger ? " danger" : "");
  item.setAttribute("role", entry.checked === undefined ? "menuitem" : "menuitemcheckbox");
  if (entry.checked !== undefined) item.setAttribute("aria-checked", String(entry.checked));

  const icon = document.createElement("span");
  icon.className = "menu-icon";
  if (entry.icon) icon.innerHTML = entry.icon;
  const label = document.createElement("span");
  label.className = "menu-label";
  label.textContent = entry.label;
  const hint = document.createElement("span");
  hint.className = "menu-hint";
  if (entry.checked) hint.classList.add("checked");
  hint.textContent = entry.shortcut ?? "";
  item.append(icon, label, hint);

  if (entry.disabled) {
    item.disabled = true;
  } else {
    item.addEventListener("click", (ev) => {
      ev.stopPropagation();
      closeContextMenu();
      entry.onClick();
    });
  }
  return item;
}

/** Open a menu at (x, y), clamped inside the viewport. */
export function showContextMenu(x: number, y: number, entries: MenuEntry[]): void {
  anchorEl = null;
  open(entries, (r) => ({
    left: Math.max(4, Math.min(x, window.innerWidth - r.width - 8)),
    top: Math.max(4, Math.min(y, window.innerHeight - r.height - 8)),
  }));
}

/** Open a menu below `anchor`, right-aligned to it, and focus the first item.
 *  It never moves up over its anchor; in a short window it scrolls instead. */
export function showMenuAt(anchor: HTMLElement, entries: MenuEntry[]): void {
  const a = anchor.getBoundingClientRect();
  anchorEl = anchor;
  open(
    entries,
    (r) => ({
      left: Math.max(8, Math.min(a.right - r.width, window.innerWidth - r.width - 8)),
      top: a.bottom + 8,
    }),
    true
  );
}

function open(
  entries: MenuEntry[],
  place: (r: DOMRect) => { left: number; top: number },
  focusFirst = false
): void {
  const keepAnchor = anchorEl;
  closeContextMenu();
  anchorEl = keepAnchor;
  const menu = document.createElement("div");
  menu.className = "context-menu interactive";
  menu.setAttribute("role", "menu");
  for (const entry of entries) {
    if (entry === "separator") {
      const sep = document.createElement("div");
      sep.className = "context-menu-sep";
      sep.setAttribute("role", "separator");
      menu.appendChild(sep);
    } else if ("render" in entry) {
      menu.appendChild(entry.render());
    } else {
      menu.appendChild(buildItem(entry));
    }
  }
  document.body.appendChild(menu);
  current = menu;
  const { left, top } = place(menu.getBoundingClientRect());
  menu.style.left = `${left}px`;
  menu.style.top = `${top}px`;
  const room = window.innerHeight - top - 8;
  if (menu.getBoundingClientRect().height > room) {
    menu.style.maxHeight = `${room}px`;
    menu.style.overflowY = "auto";
  }
  // Region must include the menu now that it exists and is positioned.
  window.dispatchEvent(new Event("overlay:layout"));

  const items = () =>
    Array.from(menu.querySelectorAll<HTMLButtonElement>("button:not(:disabled)"));
  if (focusFirst) items()[0]?.focus();

  // Dismiss on outside-click / Escape / scroll; arrows move between items.
  // Capture phase so the menu wins over page-level shortcuts.
  const onDown = (ev: MouseEvent) => {
    const target = ev.target as Node;
    if (anchorEl?.contains(target)) return;
    if (current && !current.contains(target)) closeContextMenu();
  };
  const onKey = (ev: KeyboardEvent) => {
    if (ev.key === "Escape") {
      ev.preventDefault();
      ev.stopPropagation();
      closeContextMenu();
      return;
    }
    const list = items();
    if (!list.length) return;
    const at = list.indexOf(document.activeElement as HTMLButtonElement);
    let next = -1;
    if (ev.key === "ArrowDown") next = at < 0 ? 0 : (at + 1) % list.length;
    else if (ev.key === "ArrowUp") next = at < 0 ? list.length - 1 : (at - 1 + list.length) % list.length;
    else if (ev.key === "Home") next = 0;
    else if (ev.key === "End") next = list.length - 1;
    if (next >= 0) {
      ev.preventDefault();
      ev.stopPropagation();
      list[next].focus();
    }
  };
  // Scrolling the page dismisses the menu; scrolling inside a tall menu doesn't.
  const onScroll = (ev: Event) => {
    if (current && ev.target instanceof Node && current.contains(ev.target)) return;
    closeContextMenu();
  };
  // Defer attaching so the opening click doesn't immediately dismiss.
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

// Element picker. Hovering outlines the element under the pointer; a click
// picks it and offers a hiding filter. Resolves with the selector to save to
// My filters (the element is hidden right away), or null when Esc or Cancel
// ends picking without changes.
(() => new Promise((resolve) => {
  const root = document.createElement("div");
  root.style.cssText = "all:initial;position:fixed;inset:0;z-index:2147483647;pointer-events:none";
  const shadow = root.attachShadow({ mode: "closed" });
  shadow.innerHTML = `<style>
    .box { position: fixed; border: 2px solid #4f8cff; background: rgba(79,140,255,.18); border-radius: 3px; pointer-events: none; display: none; }
    .bar { position: fixed; left: 50%; bottom: 16px; transform: translateX(-50%); display: flex; gap: 8px; align-items: center;
           max-width: calc(100vw - 32px); padding: 8px 10px; border-radius: 10px; background: #1d1f24; color: #e8eaed;
           font: 13px/1.3 system-ui, sans-serif; box-shadow: 0 6px 24px rgba(0,0,0,.35); pointer-events: auto; }
    .bar input { flex: 1; min-width: 220px; padding: 5px 7px; border-radius: 6px; border: 1px solid #3a3d44; background: #111317; color: inherit; font: 12px ui-monospace, monospace; }
    .bar button { padding: 5px 10px; border-radius: 6px; border: 1px solid #3a3d44; background: #2a2d34; color: inherit; font: inherit; cursor: pointer; }
    .bar button.primary { background: #3b6fe0; border-color: #3b6fe0; }
    .count { color: #9aa0a6; white-space: nowrap; }
  </style>
  <div class="box"></div>
  <div class="bar"><span class="hint">Click an element to hide it · Esc to cancel</span></div>`;
  const box = shadow.querySelector(".box");
  const bar = shadow.querySelector(".bar");
  document.documentElement.appendChild(root);

  // A selector for `el`: its id when unique, else tag and classes, prefixed
  // with ancestors until it no longer matches unrelated elements (5 levels).
  const part = (el) => {
    if (el.id && document.querySelectorAll("#" + CSS.escape(el.id)).length === 1) return "#" + CSS.escape(el.id);
    let s = el.tagName.toLowerCase();
    for (const c of el.classList) if (/^[A-Za-z][\w-]*$/.test(c)) s += "." + CSS.escape(c);
    return s;
  };
  const selectorFor = (el) => {
    let sel = part(el);
    let node = el;
    for (let depth = 0; depth < 5 && !sel.startsWith("#") && document.querySelectorAll(sel).length > 1 && node.parentElement && node.parentElement !== document.body; depth++) {
      node = node.parentElement;
      sel = part(node) + " > " + sel;
      if (sel.startsWith("#")) break;
    }
    return sel;
  };

  let hovered = null;
  let picked = null;
  const outline = (el) => {
    if (!el) {
      box.style.display = "none";
      return;
    }
    const r = el.getBoundingClientRect();
    Object.assign(box.style, { display: "block", left: r.left + "px", top: r.top + "px", width: r.width + "px", height: r.height + "px" });
  };
  const finish = (selector = null) => {
    document.removeEventListener("mousemove", onMove, true);
    document.removeEventListener("click", onClick, true);
    document.removeEventListener("keydown", onKey, true);
    root.remove();
    resolve(selector);
  };
  const onMove = (e) => {
    if (picked) return;
    const el = document.elementFromPoint(e.clientX, e.clientY);
    if (el && el !== hovered && el !== root && el !== document.documentElement && el !== document.body) {
      hovered = el;
      outline(el);
    }
  };
  const showChoice = (el) => {
    picked = el;
    outline(el);
    bar.innerHTML = "";
    const input = document.createElement("input");
    input.value = selectorFor(el);
    const count = document.createElement("span");
    count.className = "count";
    const recount = () => {
      try {
        const n = document.querySelectorAll(input.value).length;
        count.textContent = n === 1 ? "1 element" : n + " elements";
      } catch (err) {
        count.textContent = "invalid selector";
      }
    };
    input.addEventListener("input", recount);
    const button = (label, cls, fn) => {
      const b = document.createElement("button");
      b.textContent = label;
      if (cls) b.className = cls;
      b.addEventListener("click", (e) => {
        e.stopPropagation();
        fn();
      });
      return b;
    };
    const wider = button("Wider", "", () => {
      if (picked.parentElement && picked.parentElement !== document.body) showChoice(picked.parentElement);
    });
    const hide = button("Hide", "primary", () => {
      const selector = input.value.trim();
      try {
        document.querySelector(selector);
      } catch (err) {
        return;
      }
      document.querySelectorAll(selector).forEach((el) => el.style.setProperty("display", "none", "important"));
      finish(selector);
    });
    bar.append(input, count, wider, hide, button("Cancel", "", () => finish()));
    recount();
    input.focus();
  };
  const onClick = (e) => {
    if (e.composedPath().includes(root)) return;
    e.preventDefault();
    e.stopImmediatePropagation();
    if (!picked && hovered) showChoice(hovered);
  };
  const onKey = (e) => {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopImmediatePropagation();
      finish();
    } else if (e.key === "Enter" && picked) {
      const hideButton = bar.querySelector("button.primary");
      if (hideButton) hideButton.click();
    }
  };
  document.addEventListener("mousemove", onMove, true);
  document.addEventListener("click", onClick, true);
  document.addEventListener("keydown", onKey, true);
}))()

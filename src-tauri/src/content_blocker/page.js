// Cosmetic filtering for one document, injected at document creation with a
// config built by the host for the document's hostname:
//   host, top        which document this copy is for
//   key              per-session name the host channel is handed over under
//   css              hostname-specific selectors to hide
//   procedural       JSON-encoded procedural/action filters
//   generic          look up generic class/id rules as the DOM grows
(function (cfg) {
  if (location.hostname !== cfg.host || (window === window.top) !== cfg.top) return;

  // The host channel left by the document-start script (bootstrap.js). Calls
  // go to Jello directly, not over the network.
  const channel = window[cfg.key];
  if (channel !== undefined) delete window[cfg.key];
  const parse = JSON.parse;
  const stringify = JSON.stringify;
  const lookup = (data) =>
    channel
      ? Promise.resolve(channel.generic(location.href, stringify(data)))
          .then((text) => parse(text))
          .catch(() => [])
      : Promise.resolve([]);

  const sheet = new CSSStyleSheet();
  const hidden = new Set();
  const adopt = () => {
    if (!document.adoptedStyleSheets.includes(sheet)) {
      document.adoptedStyleSheets = [...document.adoptedStyleSheets, sheet];
    }
  };
  const addRule = (rule) => {
    try {
      sheet.insertRule(rule, sheet.cssRules.length);
    } catch (e) {}
  };
  const hide = (selectors) => {
    for (const s of selectors) {
      if (hidden.has(s)) continue;
      hidden.add(s);
      addRule(s + "{display:none!important}");
    }
    adopt();
  };
  // Bulk load: one parse, and an invalid selector only drops its own rule.
  sheet.replaceSync(cfg.css.map((s) => s + "{display:none!important}").join("\n"));
  cfg.css.forEach((s) => hidden.add(s));
  adopt();
  // A page that replaces document.adoptedStyleSheets drops the sheet.
  document.addEventListener("DOMContentLoaded", adopt, { once: true });
  window.addEventListener("load", adopt, { once: true });

  const observers = [];
  const onMutations = (fn) => observers.push(fn);
  const startObserver = () => {
    if (!observers.length) return;
    new MutationObserver((records) => observers.forEach((fn) => fn(records))).observe(document, {
      childList: true,
      subtree: true,
      attributes: true,
      attributeFilter: ["class", "id"],
    });
  };

  // Generic rules (`##.ad-banner`) are too many to inject everywhere, so the
  // document reports the classes and ids it uses and the host answers with
  // the generic selectors that apply to them. Budget: each added subtree is
  // scanned once, reports are batched every 30 ms (at most 1,000 names per
  // request), and at most 50,000 distinct class/id names are reported per
  // document.
  if (cfg.generic && channel) {
    const seen = new Set();
    const classes = new Set();
    const ids = new Set();
    let timer = 0;
    const flush = () => {
      timer = 0;
      if (!classes.size && !ids.size) return;
      const c = [...classes].slice(0, 1000);
      const i = [...ids].slice(0, 1000 - c.length);
      c.forEach((n) => classes.delete(n));
      i.forEach((n) => ids.delete(n));
      lookup({ c, i }).then((selectors) => Array.isArray(selectors) && selectors.length && hide(selectors));
      if (classes.size || ids.size) timer = setTimeout(flush, 30);
    };
    const note = (el) => {
      if (seen.size > 50000) return;
      if (el.id && !seen.has("#" + el.id)) {
        seen.add("#" + el.id);
        ids.add(el.id);
      }
      const list = el.classList;
      if (list) {
        for (const c of list) {
          if (!seen.has("." + c)) {
            seen.add("." + c);
            classes.add(c);
          }
        }
      }
    };
    const scan = (root) => {
      note(root);
      for (const el of root.querySelectorAll("[id],[class]")) note(el);
      if (!timer && (classes.size || ids.size)) timer = setTimeout(flush, 30);
    };
    onMutations((records) => {
      for (const r of records) {
        if (r.type === "attributes") note(r.target);
        else for (const n of r.addedNodes) if (n.nodeType === 1) scan(n);
      }
      if (!timer && (classes.size || ids.size)) timer = setTimeout(flush, 30);
    });
    const first = () => document.documentElement && scan(document.documentElement);
    if (document.documentElement) first();
    document.addEventListener("DOMContentLoaded", first, { once: true });
  }

  // Procedural and action filters: `:has-text()`, `:upward()`, `:xpath()`,
  // `:matches-css()`, `:remove()`, `:style()`, … applied by walking the DOM,
  // since CSS cannot express them. `:style()` on a plain selector becomes a
  // stylesheet rule, so it also covers elements added later. Budget: one pass
  // over the page's filters per DOM-change burst, at most every 100 ms.
  const procedural = [];
  for (const raw of cfg.procedural) {
    let f;
    try {
      f = JSON.parse(raw);
    } catch (e) {
      continue;
    }
    const plain = f.selector.length === 1 && f.selector[0].type === "css-selector";
    if (plain && f.action && f.action.type === "style") {
      const decls = f.action.arg
        .split(";")
        .map((d) => d.trim())
        .filter(Boolean)
        .map((d) => (/!important\s*$/.test(d) ? d : d + " !important"))
        .join(";");
      addRule(f.selector[0].arg + "{" + decls + "}");
      continue;
    }
    if (plain && f.action && f.action.type === "remove") hide([f.selector[0].arg]);
    procedural.push(f);
  }
  adopt();
  if (procedural.length) {
    const toRegex = (arg) => {
      const m = /^\/(.+)\/([imsu]*)$/.exec(arg);
      if (m) {
        try {
          return new RegExp(m[1], m[2]);
        } catch (e) {
          return null;
        }
      }
      return { test: (s) => s.includes(arg) };
    };
    // A `/regex/` argument, or a literal that must match exactly.
    const exactOrRegex = (arg) => (/^\/.+\/[imsu]*$/.test(arg) ? toRegex(arg) : { test: (s) => s === arg });
    // `name="value"`, `name=/re/`, `prop: value` or `"name"` (an attribute or
    // a CSS property and an optional value).
    const pairArg = (arg) => {
      const m = /^\s*(?:"([^"]+)"|([^=:\s]+))\s*(?:[=:]\s*(.+?))?\s*$/.exec(arg);
      if (!m) return [exactOrRegex(arg.trim()), null];
      const value = m[3] === undefined ? null : m[3].replace(/^"(.*)"$/, "$1");
      return [exactOrRegex(m[1] || m[2]), value === null ? null : exactOrRegex(value)];
    };
    // Selectors after the first are relative to the nodes matched so far
    // (` > .x`, ` .x`, ` + .x`, ` ~ .x`).
    const query = (nodes, sel) => {
      const out = new Set();
      const trimmed = sel.trim();
      const sibling = trimmed[0] === "+" || trimmed[0] === "~" ? trimmed[0] : "";
      const rest = sibling ? trimmed.slice(1).trim() : "";
      for (const n of nodes) {
        try {
          if (n === document) {
            document.querySelectorAll(sel).forEach((e) => out.add(e));
          } else if (sibling) {
            for (let s = n.nextElementSibling; s; s = s.nextElementSibling) {
              if (s.matches(rest)) out.add(s);
              if (sibling === "+") break;
            }
          } else {
            n.querySelectorAll(":scope " + trimmed).forEach((e) => out.add(e));
          }
        } catch (e) {}
      }
      return [...out];
    };
    const ops = {
      "css-selector": query,
      "has-text": (nodes, arg) => {
        const re = toRegex(arg);
        return re ? nodes.filter((n) => re.test(n.textContent || "")) : [];
      },
      "min-text-length": (nodes, arg) => nodes.filter((n) => (n.textContent || "").length >= Number(arg)),
      "matches-attr": (nodes, arg) => {
        const [nameRe, valueRe] = pairArg(arg);
        return nodes.filter((n) =>
          [...(n.attributes || [])].some((a) => nameRe && nameRe.test(a.name) && (!valueRe || valueRe.test(a.value)))
        );
      },
      "matches-path": (nodes, arg) => {
        const re = toRegex(arg);
        return re && re.test(location.pathname + location.search) ? nodes : [];
      },
      upward: (nodes, arg) => {
        const out = new Set();
        const n = Number(arg);
        for (let node of nodes) {
          if (Number.isInteger(n) && n > 0) {
            for (let i = 0; i < n && node; i++) node = node.parentElement;
          } else {
            node = node.parentElement && node.parentElement.closest(arg);
          }
          if (node) out.add(node);
        }
        return [...out];
      },
      xpath: (nodes, arg) => {
        const out = new Set();
        for (const n of nodes) {
          try {
            const r = document.evaluate(arg, n, null, XPathResult.ORDERED_NODE_SNAPSHOT_TYPE, null);
            for (let i = 0; i < r.snapshotLength; i++) out.add(r.snapshotItem(i));
          } catch (e) {}
        }
        return [...out].filter((x) => x.nodeType === 1);
      },
    };
    // `prop: value`, the value literal or `/regex/`.
    const cssMatcher = (pseudo) => (nodes, arg) => {
      const m = /^\s*([\w-]+)\s*:\s*(.+?)\s*$/.exec(arg);
      if (!m) return [];
      const valueRe = exactOrRegex(m[2]);
      return nodes.filter((n) => valueRe.test(getComputedStyle(n, pseudo).getPropertyValue(m[1]).trim()));
    };
    ops["matches-css"] = cssMatcher(null);
    ops["matches-css-before"] = cssMatcher("::before");
    ops["matches-css-after"] = cssMatcher("::after");

    const apply = (filter) => {
      let nodes = [document];
      for (const op of filter.selector) {
        const fn = ops[op.type];
        nodes = fn ? fn(nodes, op.arg) : [];
        if (!nodes.length) return;
      }
      const action = filter.action || { type: "hide" };
      for (const n of nodes) {
        if (n === document) continue;
        switch (action.type) {
          case "remove":
            n.remove();
            break;
          case "style":
            n.style.cssText += ";" + action.arg;
            break;
          case "remove-attr":
            n.removeAttribute(action.arg);
            break;
          case "remove-class":
            n.classList.remove(action.arg);
            break;
          default:
            n.style.setProperty("display", "none", "important");
        }
      }
    };
    let pending = 0;
    const run = () => {
      pending = 0;
      procedural.forEach(apply);
    };
    const later = () => {
      if (!pending) pending = setTimeout(run, 100);
    };
    document.addEventListener("DOMContentLoaded", run, { once: true });
    window.addEventListener("load", run, { once: true });
    onMutations(later);
  }

  startObserver();
})

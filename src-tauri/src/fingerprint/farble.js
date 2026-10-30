// Fingerprinting protection. Runs before page scripts in every document of a
// content tab and, through the DevTools protocol, in every worker.
//
// Values that would identify the device are replaced with common ones (the
// GPU, WebGPU adapter details, screen position, battery) or given small,
// deterministic noise (canvas, WebGL and audio readback, text metrics,
// hardware concurrency and memory, media device ids). The noise is seeded by
// a random secret that lives for one browser session and by the top-level
// site (registrable domain), so every frame and worker of a site sees the
// same values for the whole session, while another site, or the same site in
// the next session, sees different ones and cannot link the two.
//
// cfg: secret    random per-session string
//      suffixes  public suffix rules, "\n"-separated with "\n" at both ends
//      allow     sites the user exempted (breakage)
//      marker    query flag that marks shared worker scripts (documents)
//      top       top-level origin, for workers (documents read their own)
(function (cfg) {
  "use strict";
  const G = globalThis;
  const isWindow = typeof G.Window === "function" && G instanceof G.Window;

  // Registrable domain of a hostname by the public suffix rules: the suffix
  // plus one label. IP addresses and unlisted single labels stay as they are.
  const registrable = (host) => {
    host = String(host).toLowerCase().replace(/\.$/, "");
    if (!host || host.includes(":") || /^[0-9.]+$/.test(host)) return host;
    const labels = host.split(".");
    const has = (rule) => cfg.suffixes.indexOf("\n" + rule + "\n") !== -1;
    let suffix = 1;
    for (let i = 0; i < labels.length; i++) {
      const name = labels.slice(i).join(".");
      if (has("!" + name)) {
        suffix = labels.length - i - 1;
        break;
      }
      if (has(name) || (i + 1 < labels.length && has("*." + labels.slice(i + 1).join(".")))) {
        suffix = labels.length - i;
        break;
      }
    }
    return suffix >= labels.length ? host : labels.slice(labels.length - suffix - 1).join(".");
  };

  let top = cfg.top;
  if (top === undefined) {
    try {
      const ancestors = G.location.ancestorOrigins;
      top = ancestors && ancestors.length ? ancestors[ancestors.length - 1] : G.location.origin;
    } catch (e) {
      top = "null";
    }
  }
  let site = "";
  try {
    const url = new URL(top);
    if (url.protocol === "https:" || url.protocol === "http:") site = registrable(url.hostname);
  } catch (e) {}
  // A context whose top-level site is unknown (opaque origins) gets values
  // of its own, which nothing else can share.
  if (!site) site = "opaque " + Math.random();
  if (cfg.allow.includes(site)) return;

  const hash = (text) => {
    let h1 = 0xdeadbeef ^ text.length;
    let h2 = 0x41c6ce57 ^ text.length;
    for (let i = 0; i < text.length; i++) {
      const c = text.charCodeAt(i);
      h1 = Math.imul(h1 ^ c, 2654435761);
      h2 = Math.imul(h2 ^ c, 1597334677);
    }
    h1 = Math.imul(h1 ^ (h1 >>> 16), 2246822507) ^ Math.imul(h2 ^ (h2 >>> 13), 3266489909);
    h2 = Math.imul(h2 ^ (h2 >>> 16), 2246822507) ^ Math.imul(h1 ^ (h1 >>> 13), 3266489909);
    return (h1 ^ h2) >>> 0;
  };
  const mix = (a, b) => {
    let h = Math.imul(a ^ b, 0x9e3779b1);
    h ^= h >>> 15;
    h = Math.imul(h, 0x85ebca77);
    h ^= h >>> 13;
    return h >>> 0;
  };
  // An independent seed per purpose.
  const seed = (purpose) => hash(purpose + "\n" + cfg.secret + "\n" + site);

  // Replacements look like the built-ins they replace: same name, length and
  // property attributes, and Function.prototype.toString shows native code.
  const originals = new WeakMap();
  const define = (target, name, descriptor) => {
    try {
      Object.defineProperty(target, name, descriptor);
    } catch (e) {}
  };
  const method = (proto, name, make) => {
    const d = proto && Object.getOwnPropertyDescriptor(proto, name);
    if (!d || typeof d.value !== "function") return;
    const original = d.value;
    const impl = make(original);
    const replacement = {
      [name]() {
        return impl.apply(this, arguments);
      },
    }[name];
    define(replacement, "length", { value: original.length, configurable: true });
    originals.set(replacement, original);
    define(proto, name, { ...d, value: replacement });
  };
  const getter = (target, name, make) => {
    const d = target && Object.getOwnPropertyDescriptor(target, name);
    if (!d || typeof d.get !== "function") return;
    const original = d.get;
    const impl = make(original);
    const replacement = Object.getOwnPropertyDescriptor(
      {
        get [name]() {
          return impl.call(this);
        },
      },
      name
    ).get;
    originals.set(replacement, original);
    define(target, name, { ...d, get: replacement });
  };
  method(Function.prototype, "toString", (original) =>
    function () {
      const o = originals.get(this);
      return original.call(o === undefined ? this : o);
    }
  );

  // Pixel readback: on a sparse, per-site pattern of fully opaque pixels, the
  // lowest bit of one color channel is set to a per-site value. The pattern
  // is fixed to canvas coordinates so every way of reading a canvas agrees,
  // and setting (not flipping) the bit gives the same result if a context is
  // prepared twice. Reads under 256 pixels (hit testing, color picking) are
  // left exact.
  const STEP = 16;
  const MIN_PIXELS = 256;
  const pixelSeed = seed("pixels");
  const noise = (data, width, x0, y0) => {
    if (!width) return;
    const height = Math.floor(data.length / 4 / width);
    for (let row = 0; row < height; row++) {
      const y = y0 + row;
      const offset = mix(pixelSeed, y) % STEP;
      let x = x0 + ((((offset - x0) % STEP) + STEP) % STEP);
      for (; x < x0 + width; x += STEP) {
        const i = (row * width + (x - x0)) * 4;
        if (data[i + 3] !== 255) continue;
        const h = mix(pixelSeed ^ 0x5bd1e995, ((y & 0xffff) << 16) ^ (x & 0xffff));
        const c = i + (h % 3);
        data[c] = (data[c] & 0xfe) | ((h >>> 8) & 1);
      }
    }
  };

  const ctx2d = [G.CanvasRenderingContext2D, G.OffscreenCanvasRenderingContext2D].filter(Boolean);
  const getImageData = ctx2d.length ? ctx2d[0].prototype.getImageData : null;
  for (const C of ctx2d) {
    method(C.prototype, "getImageData", (original) =>
      function (sx, sy, sw, sh) {
        const image = original.apply(this, arguments);
        if (image.width * image.height >= MIN_PIXELS) {
          noise(image.data, image.width, Math.floor(Math.min(sx, sx + sw)) || 0, Math.floor(Math.min(sy, sy + sh)) || 0);
        }
        return image;
      }
    );
  }

  // Encoders read a noised copy, so the canvas itself is left untouched.
  const noisedCopy = (canvas, makeCanvas) => {
    const w = canvas.width;
    const h = canvas.height;
    if (!w || !h) return null;
    const copy = makeCanvas(w, h);
    const ctx = copy.getContext("2d");
    ctx.drawImage(canvas, 0, 0);
    const image = getImageData.call(ctx, 0, 0, w, h);
    noise(image.data, w, 0, 0);
    ctx.putImageData(image, 0, 0);
    return copy;
  };
  if (isWindow && G.HTMLCanvasElement) {
    const createElement = Document.prototype.createElement;
    const makeCanvas = (doc) => (w, h) => {
      const c = createElement.call(doc, "canvas");
      c.width = w;
      c.height = h;
      return c;
    };
    for (const name of ["toDataURL", "toBlob"]) {
      method(G.HTMLCanvasElement.prototype, name, (original) =>
        function () {
          try {
            const copy = noisedCopy(this, makeCanvas(this.ownerDocument || document));
            if (copy) return original.apply(copy, arguments);
          } catch (e) {}
          return original.apply(this, arguments);
        }
      );
    }
  }
  if (G.OffscreenCanvas) {
    method(G.OffscreenCanvas.prototype, "convertToBlob", (original) =>
      function () {
        try {
          const copy = noisedCopy(this, (w, h) => new G.OffscreenCanvas(w, h));
          if (copy) return original.apply(copy, arguments);
        } catch (e) {}
        return original.apply(this, arguments);
      }
    );
  }

  // WebGL: one generic GPU for everyone, and noised readback.
  const GPU_VENDOR = "Google Inc. (Intel)";
  const GPU_RENDERER = "ANGLE (Intel, Intel(R) UHD Graphics 620 Direct3D11 vs_5_0 ps_5_0, D3D11)";
  for (const C of [G.WebGLRenderingContext, G.WebGL2RenderingContext].filter(Boolean)) {
    method(C.prototype, "getParameter", (original) =>
      function (pname) {
        const value = original.apply(this, arguments);
        if (typeof value === "string") {
          if (pname === 0x9245) return GPU_VENDOR; // UNMASKED_VENDOR_WEBGL
          if (pname === 0x9246) return GPU_RENDERER; // UNMASKED_RENDERER_WEBGL
        }
        return value;
      }
    );
    method(C.prototype, "readPixels", (original) =>
      function (x, y, width, height, format, type, pixels, offset) {
        const result = original.apply(this, arguments);
        if (width * height >= MIN_PIXELS && format === 0x1908 && type === 0x1401 && ArrayBuffer.isView(pixels) && pixels.BYTES_PER_ELEMENT === 1) {
          const start = Number.isInteger(offset) ? offset : 0;
          noise(new Uint8Array(pixels.buffer, pixels.byteOffset + start, width * height * 4), width, x, y);
        }
        return result;
      }
    );
  }

  // WebGPU: the adapter does not describe the GPU.
  if (G.GPUAdapterInfo) {
    for (const name of ["vendor", "architecture", "device", "description"]) {
      getter(G.GPUAdapterInfo.prototype, name, () => () => "");
    }
  }

  // Text metrics: a tiny per-site factor for each font and text, far below a
  // pixel, so comparing widths no longer reveals which fonts are installed.
  if (G.TextMetrics) {
    const textSeed = seed("text");
    const factors = new WeakMap();
    for (const C of ctx2d) {
      method(C.prototype, "measureText", (original) =>
        function (text) {
          const metrics = original.apply(this, arguments);
          const h = mix(textSeed, hash(String(this.font) + "\n" + String(text)));
          factors.set(metrics, 1 + ((h % 2001) - 1000) * 1e-8);
          return metrics;
        }
      );
    }
    for (const name of [
      "width",
      "actualBoundingBoxLeft",
      "actualBoundingBoxRight",
      "actualBoundingBoxAscent",
      "actualBoundingBoxDescent",
      "fontBoundingBoxAscent",
      "fontBoundingBoxDescent",
    ]) {
      getter(G.TextMetrics.prototype, name, (original) =>
        function () {
          const value = original.call(this);
          const f = factors.get(this);
          return f === undefined ? value : value * f;
        }
      );
    }
  }

  // Layout geometry: element, range and SVG measurements reflect the fonts
  // installed and how they render. Each value is moved by at most a
  // millionth of itself, decided by the value and the site, so equal values
  // stay equal and layout code is unaffected.
  const geometrySeed = seed("geometry");
  const nudge = (v) => (v ? v * (1 + ((mix(geometrySeed, hash(String(v))) % 2001) - 1000) * 1e-9) : v);
  const nudgeRect = (r) => {
    if (r) {
      const x = r.x;
      const y = r.y;
      const width = r.width;
      const height = r.height;
      r.x = nudge(x);
      r.y = nudge(y);
      r.width = nudge(width);
      r.height = nudge(height);
    }
    return r;
  };
  const nudgeList = (list) => {
    for (let i = 0; i < list.length; i++) nudgeRect(list[i]);
    return list;
  };
  for (const proto of [G.Element && G.Element.prototype, G.Range && G.Range.prototype]) {
    method(proto, "getBoundingClientRect", (original) =>
      function () {
        return nudgeRect(original.apply(this, arguments));
      }
    );
    method(proto, "getClientRects", (original) =>
      function () {
        return nudgeList(original.apply(this, arguments));
      }
    );
  }
  if (G.SVGGraphicsElement) {
    method(G.SVGGraphicsElement.prototype, "getBBox", (original) =>
      function () {
        return nudgeRect(original.apply(this, arguments));
      }
    );
  }
  if (G.SVGTextContentElement) {
    const T = G.SVGTextContentElement.prototype;
    for (const name of ["getComputedTextLength", "getSubStringLength"]) {
      method(T, name, (original) =>
        function () {
          return nudge(original.apply(this, arguments));
        }
      );
    }
    method(T, "getExtentOfChar", (original) =>
      function () {
        return nudgeRect(original.apply(this, arguments));
      }
    );
    for (const name of ["getStartPositionOfChar", "getEndPositionOfChar"]) {
      method(T, name, (original) =>
        function () {
          const p = original.apply(this, arguments);
          if (p) {
            const x = p.x;
            const y = p.y;
            p.x = nudge(x);
            p.y = nudge(y);
          }
          return p;
        }
      );
    }
  }

  // Local fonts through FontFace: only the fonts Windows installs by
  // default, so fonts added by other software cannot be told apart.
  if (G.FontFace) {
    const defaults = new Set(
      [
        "Arial", "Arial Black", "Bahnschrift", "Calibri", "Calibri Light", "Cambria", "Cambria Math", "Candara",
        "Comic Sans MS", "Consolas", "Constantia", "Corbel", "Courier New", "Ebrima", "Franklin Gothic Medium",
        "Gabriola", "Gadugi", "Georgia", "HoloLens MDL2 Assets", "Impact", "Ink Free", "Javanese Text",
        "Leelawadee UI", "Leelawadee UI Semilight", "Lucida Console", "Lucida Sans Unicode", "Malgun Gothic",
        "Malgun Gothic Semilight", "Marlett", "Microsoft Himalaya", "Microsoft JhengHei", "Microsoft JhengHei UI",
        "Microsoft New Tai Lue", "Microsoft PhagsPa", "Microsoft Sans Serif", "Microsoft Tai Le", "Microsoft YaHei",
        "Microsoft YaHei UI", "Microsoft Yi Baiti", "MingLiU-ExtB", "Mongolian Baiti", "MS Gothic", "MS PGothic",
        "MS UI Gothic", "MV Boli", "Myanmar Text", "Nirmala UI", "Nirmala UI Semilight", "Palatino Linotype",
        "Segoe Fluent Icons", "Segoe MDL2 Assets", "Segoe Print", "Segoe Script", "Segoe UI", "Segoe UI Black",
        "Segoe UI Emoji", "Segoe UI Historic", "Segoe UI Light", "Segoe UI Semibold", "Segoe UI Semilight",
        "Segoe UI Symbol", "Segoe UI Variable", "SimSun", "NSimSun", "Sitka", "Sylfaen", "Symbol", "Tahoma",
        "Times New Roman", "Trebuchet MS", "Verdana", "Webdings", "Wingdings", "Yu Gothic", "Yu Gothic UI",
      ].map((f) => f.toLowerCase())
    );
    const missing = 'local("f' + seed("fonts").toString(36) + '")';
    const keep = (source) =>
      typeof source === "string"
        ? source.replace(/local\(\s*(["']?)([^"')]+)\1\s*\)/gi, (all, quote, name) =>
            defaults.has(name.trim().toLowerCase()) ? all : missing
          )
        : source;
    const Original = G.FontFace;
    const Face = function FontFace(family, source) {
      if (!new.target) return Original.apply(this, arguments);
      const args = Array.prototype.slice.call(arguments);
      args[1] = keep(source);
      return Reflect.construct(Original, args, new.target);
    };
    define(Face, "length", { value: Original.length, configurable: true });
    define(Face, "prototype", { value: Original.prototype });
    define(Original.prototype, "constructor", { value: Face, writable: true, configurable: true });
    Object.setPrototypeOf(Face, Object.getPrototypeOf(Original));
    originals.set(Face, Original);
    define(G, "FontFace", { ...Object.getOwnPropertyDescriptor(G, "FontFace"), value: Face });
  }

  // Audio: rendered samples are scaled by a per-site factor within a few
  // millionths of 1, inaudible but enough to change any hash of them.
  if (G.AudioBuffer) {
    const s = seed("audio");
    const factor = 1 + (20 + (s % 80)) * 1e-8 * (s & 1 ? 1 : -1);
    const done = new WeakMap();
    let getChannelData = null;
    method(G.AudioBuffer.prototype, "getChannelData", (original) =>
      (getChannelData = function (channel) {
        const data = original.apply(this, arguments);
        let channels = done.get(this);
        if (!channels) done.set(this, (channels = new Set()));
        if (!channels.has(channel)) {
          channels.add(channel);
          for (let i = 0; i < data.length; i++) data[i] *= factor;
        }
        return data;
      })
    );
    method(G.AudioBuffer.prototype, "copyFromChannel", (original) =>
      function (destination, channel) {
        if (getChannelData) {
          try {
            getChannelData.call(this, channel);
          } catch (e) {}
        }
        return original.apply(this, arguments);
      }
    );
  }
  if (G.AnalyserNode) {
    const s = seed("analyser");
    const factor = 1 + (20 + (s % 80)) * 1e-8 * (s & 1 ? 1 : -1);
    for (const name of ["getFloatFrequencyData", "getFloatTimeDomainData"]) {
      method(G.AnalyserNode.prototype, name, (original) =>
        function (array) {
          const result = original.apply(this, arguments);
          if (array && array.length) for (let i = 0; i < array.length; i++) array[i] *= factor;
          return result;
        }
      );
    }
  }

  // Navigator: hardware figures from common values (never above the real
  // ones), one language, and no WebView2 brand.
  const navigators = [G.Navigator, G.WorkerNavigator].filter(Boolean).map((N) => N.prototype);
  const cores = [4, 8][seed("cores") & 1];
  const memory = [4, 8][seed("memory") & 1];
  for (const proto of navigators) {
    getter(proto, "hardwareConcurrency", (original) =>
      function () {
        const real = original.call(this);
        return real > cores ? cores : real;
      }
    );
    getter(proto, "deviceMemory", (original) =>
      function () {
        const real = original.call(this);
        return real > memory ? memory : real;
      }
    );
    let languages = null;
    getter(proto, "languages", (original) =>
      function () {
        const real = original.call(this);
        if (!languages || languages[0] !== real[0]) languages = Object.freeze(real.slice(0, 1));
        return languages;
      }
    );
  }
  if (G.NavigatorUAData) {
    const isEmbedderBrand = (b) => b && /webview/i.test(b.brand);
    getter(G.NavigatorUAData.prototype, "brands", (original) =>
      function () {
        return Object.freeze(original.call(this).filter((b) => !isEmbedderBrand(b)));
      }
    );
    method(G.NavigatorUAData.prototype, "getHighEntropyValues", (original) =>
      function () {
        return original.apply(this, arguments).then((values) => {
          for (const key of ["brands", "fullVersionList"]) {
            if (Array.isArray(values[key])) values[key] = values[key].filter((b) => !isEmbedderBrand(b));
          }
          return values;
        });
      }
    );
    method(G.NavigatorUAData.prototype, "toJSON", (original) =>
      function () {
        const values = original.apply(this, arguments);
        if (values && Array.isArray(values.brands)) values.brands = values.brands.filter((b) => !isEmbedderBrand(b));
        return values;
      }
    );
  }

  // Battery: the state of a desktop on mains power.
  if (G.BatteryManager) {
    const fixed = { charging: true, chargingTime: 0, dischargingTime: Infinity, level: 1 };
    for (const name of Object.keys(fixed)) getter(G.BatteryManager.prototype, name, () => () => fixed[name]);
  }

  // Keyboard layout is not exposed.
  if (G.Keyboard) {
    method(G.Keyboard.prototype, "getLayoutMap", () => () =>
      Promise.reject(new DOMException("Keyboard layout is not available.", "NotAllowedError"))
    );
  }

  // Media device ids: stable per site for the session only.
  if (G.MediaDeviceInfo) {
    const s = seed("devices");
    for (const name of ["deviceId", "groupId"]) {
      getter(G.MediaDeviceInfo.prototype, name, (original) =>
        function () {
          const real = original.call(this);
          if (!real) return real;
          let out = "";
          for (let i = 0; i < 8; i++) out += mix(s + i, hash(real)).toString(16).padStart(8, "0");
          return out;
        }
      );
    }
  }

  // Shared workers are prepared by Jello when their script loads; the
  // per-session marker added to their URL tells it which request that is.
  // Jello answers the marked request itself and loads the original URL, so
  // the server never sees the marker.
  if (isWindow && G.SharedWorker && cfg.marker) {
    const Original = G.SharedWorker;
    const Shared = function SharedWorker(url) {
      if (!new.target) return Original.apply(this, arguments);
      const args = Array.prototype.slice.call(arguments);
      try {
        const target = new URL(url, document.baseURI);
        if (target.protocol === "https:" || target.protocol === "http:") {
          target.search += (target.search ? "&" : "?") + cfg.marker;
          args[0] = target.href;
        }
      } catch (e) {}
      return Reflect.construct(Original, args, new.target);
    };
    define(Shared, "length", { value: Original.length, configurable: true });
    define(Shared, "prototype", { value: Original.prototype });
    define(Original.prototype, "constructor", { value: Shared, writable: true, configurable: true });
    Object.setPrototypeOf(Shared, Object.getPrototypeOf(Original));
    originals.set(Shared, Original);
    define(G, "SharedWorker", { ...Object.getOwnPropertyDescriptor(G, "SharedWorker"), value: Shared });
  }

  if (isWindow) {
    // Screen and window placement: no multi-monitor layout, no taskbar size,
    // and the window frame of an ordinary browser window.
    if (G.Screen) {
      const S = G.Screen.prototype;
      const width = Object.getOwnPropertyDescriptor(S, "width");
      const height = Object.getOwnPropertyDescriptor(S, "height");
      if (width && height) {
        getter(S, "availWidth", () => function () {
          return width.get.call(this);
        });
        getter(S, "availHeight", () => function () {
          return height.get.call(this);
        });
      }
      getter(S, "availLeft", () => () => 0);
      getter(S, "availTop", () => () => 0);
      getter(S, "isExtended", () => () => false);
    }
    for (const name of ["screenX", "screenY", "screenLeft", "screenTop"]) getter(G, name, () => () => 0);
    const inner = (name) => Object.getOwnPropertyDescriptor(G, name);
    const innerWidth = inner("innerWidth");
    const innerHeight = inner("innerHeight");
    if (innerWidth && innerHeight) {
      getter(G, "outerWidth", () => function () {
        return innerWidth.get.call(G);
      });
      getter(G, "outerHeight", () => function () {
        const h = innerHeight.get.call(G) + 85;
        const screenHeight = G.screen ? G.screen.height : h;
        return h > screenHeight ? screenHeight : h;
      });
    }
  }
})

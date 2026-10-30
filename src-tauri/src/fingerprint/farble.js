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
// cfg: secret          random per-session string (documents), or
//      key             the site's key derived from it (blob shared workers)
//      suffixes        public suffix rules, "\n"-separated with "\n" at both ends
//      allow           sites the user exempted (breakage)
//      hiddenFamilies  installed font families outside the Windows defaults
//      hiddenNames     full and PostScript names of those fonts
//      marker          query flag that marks shared worker scripts (documents)
//      top             top-level origin, for workers (documents read their own)
(function farble(cfg) {
  "use strict";
  const G = globalThis;
  const isWindow = typeof G.Window === "function" && G instanceof G.Window;
  // This script's own text, to prepare blob shared workers with it.
  const source = Function.prototype.toString.call(farble);

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
  // A site's key comes from the session secret; contexts a page could read
  // the script of get only their site's key. Seeds per purpose come from it.
  const keyFor = (name) => [0, 1, 2, 3].map((i) => hash(i + "\n" + cfg.secret + "\n" + name).toString(16)).join("");
  const siteKey = cfg.key !== undefined ? cfg.key : keyFor(site);
  const seed = (purpose) => hash(purpose + "\n" + siteKey);

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
  const setter = (target, name, make) => {
    const d = target && Object.getOwnPropertyDescriptor(target, name);
    if (!d || typeof d.set !== "function") return;
    const original = d.set;
    const impl = make(original);
    const replacement = Object.getOwnPropertyDescriptor(
      {
        set [name](value) {
          impl.call(this, value);
        },
      },
      name
    ).set;
    originals.set(replacement, original);
    define(target, name, { ...d, set: replacement });
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

  // WebGL: one generic GPU for everyone, capabilities no higher than those
  // of mainstream Direct3D 11 graphics, and noised readback. Reporting less
  // than the hardware offers is safe (pages use less); it is never more.
  const GPU_VENDOR = "Google Inc. (Intel)";
  const GPU_RENDERER = "ANGLE (Intel, Intel(R) UHD Graphics 620 Direct3D11 vs_5_0 ps_5_0, D3D11)";
  const LIMITS = new Map([
    [0x0d33, 16384], // MAX_TEXTURE_SIZE
    [0x851c, 16384], // MAX_CUBE_MAP_TEXTURE_SIZE
    [0x84e8, 16384], // MAX_RENDERBUFFER_SIZE
    [0x0d50, 4], // SUBPIXEL_BITS
    [0x80a9, 4], // SAMPLES
    [0x8869, 16], // MAX_VERTEX_ATTRIBS
    [0x8dfb, 4095], // MAX_VERTEX_UNIFORM_VECTORS
    [0x8dfc, 30], // MAX_VARYING_VECTORS
    [0x8dfd, 1024], // MAX_FRAGMENT_UNIFORM_VECTORS
    [0x8b4d, 32], // MAX_COMBINED_TEXTURE_IMAGE_UNITS
    [0x8b4c, 16], // MAX_VERTEX_TEXTURE_IMAGE_UNITS
    [0x8872, 16], // MAX_TEXTURE_IMAGE_UNITS
    [0x84ff, 16], // MAX_TEXTURE_MAX_ANISOTROPY_EXT
    [0x8073, 2048], // MAX_3D_TEXTURE_SIZE
    [0x88ff, 2048], // MAX_ARRAY_TEXTURE_LAYERS
    [0x80e8, 2147483647], // MAX_ELEMENTS_VERTICES
    [0x80e9, 2147483647], // MAX_ELEMENTS_INDICES
    [0x8d6b, 4294967294], // MAX_ELEMENT_INDEX
    [0x84fd, 2], // MAX_TEXTURE_LOD_BIAS
    [0x8824, 8], // MAX_DRAW_BUFFERS
    [0x8cdf, 8], // MAX_COLOR_ATTACHMENTS
    [0x8d57, 4], // MAX_SAMPLES
    [0x8b49, 4096], // MAX_FRAGMENT_UNIFORM_COMPONENTS
    [0x8b4a, 16380], // MAX_VERTEX_UNIFORM_COMPONENTS
    [0x8905, 7], // MAX_PROGRAM_TEXEL_OFFSET
    [0x8b4b, 120], // MAX_VARYING_COMPONENTS
    [0x9122, 120], // MAX_VERTEX_OUTPUT_COMPONENTS
    [0x9125, 120], // MAX_FRAGMENT_INPUT_COMPONENTS
    [0x8c80, 4], // MAX_TRANSFORM_FEEDBACK_SEPARATE_COMPONENTS
    [0x8c8a, 120], // MAX_TRANSFORM_FEEDBACK_INTERLEAVED_COMPONENTS
    [0x8c8b, 4], // MAX_TRANSFORM_FEEDBACK_SEPARATE_ATTRIBS
    [0x8a2b, 12], // MAX_VERTEX_UNIFORM_BLOCKS
    [0x8a2d, 12], // MAX_FRAGMENT_UNIFORM_BLOCKS
    [0x8a2e, 24], // MAX_COMBINED_UNIFORM_BLOCKS
    [0x8a2f, 24], // MAX_UNIFORM_BUFFER_BINDINGS
    [0x8a30, 65536], // MAX_UNIFORM_BLOCK_SIZE
    [0x8a31, 212988], // MAX_COMBINED_VERTEX_UNIFORM_COMPONENTS
    [0x8a33, 200704], // MAX_COMBINED_FRAGMENT_UNIFORM_COMPONENTS
    [0x9111, 0], // MAX_SERVER_WAIT_TIMEOUT
    [0x9247, 0], // MAX_CLIENT_WAIT_TIMEOUT_WEBGL
  ]);
  // Lower bounds a page relies on: report at least these.
  const FLOORS = new Map([
    [0x8904, -8], // MIN_PROGRAM_TEXEL_OFFSET
    [0x8a34, 256], // UNIFORM_BUFFER_OFFSET_ALIGNMENT
  ]);
  // [min, max] ranges and the maximum viewport.
  const RANGES = new Map([
    [0x846d, [1, 1024]], // ALIASED_POINT_SIZE_RANGE
    [0x846e, [1, 1]], // ALIASED_LINE_WIDTH_RANGE
  ]);
  const VIEWPORT = 0x0d3a; // MAX_VIEWPORT_DIMS
  const EXTENSIONS = new Set(
    [
      "ANGLE_instanced_arrays", "EXT_blend_minmax", "EXT_color_buffer_float", "EXT_color_buffer_half_float",
      "EXT_float_blend", "EXT_frag_depth", "EXT_shader_texture_lod", "EXT_sRGB", "EXT_texture_compression_bptc",
      "EXT_texture_compression_rgtc", "EXT_texture_filter_anisotropic", "KHR_parallel_shader_compile",
      "OES_draw_buffers_indexed", "OES_element_index_uint", "OES_fbo_render_mipmap", "OES_standard_derivatives",
      "OES_texture_float", "OES_texture_float_linear", "OES_texture_half_float", "OES_texture_half_float_linear",
      "OES_vertex_array_object", "WEBGL_color_buffer_float", "WEBGL_compressed_texture_s3tc",
      "WEBGL_compressed_texture_s3tc_srgb", "WEBGL_debug_renderer_info", "WEBGL_depth_texture",
      "WEBGL_draw_buffers", "WEBGL_lose_context", "WEBGL_multi_draw",
    ].map((e) => e.toLowerCase())
  );
  for (const C of [G.WebGLRenderingContext, G.WebGL2RenderingContext].filter(Boolean)) {
    method(C.prototype, "getParameter", (original) =>
      function (pname) {
        const value = original.apply(this, arguments);
        if (typeof value === "string") {
          if (pname === 0x9245) return GPU_VENDOR; // UNMASKED_VENDOR_WEBGL
          if (pname === 0x9246) return GPU_RENDERER; // UNMASKED_RENDERER_WEBGL
        }
        if (typeof value === "number") {
          if (LIMITS.has(pname)) return Math.min(value, LIMITS.get(pname));
          if (FLOORS.has(pname)) return Math.max(value, FLOORS.get(pname));
        }
        if (value && value.length === 2 && (RANGES.has(pname) || pname === VIEWPORT)) {
          const out = new value.constructor(2);
          if (pname === VIEWPORT) {
            out[0] = Math.min(value[0], 32767);
            out[1] = Math.min(value[1], 32767);
          } else {
            const [low, high] = RANGES.get(pname);
            out[0] = Math.max(value[0], low);
            out[1] = Math.min(value[1], high);
          }
          return out;
        }
        return value;
      }
    );
    method(C.prototype, "getSupportedExtensions", (original) =>
      function () {
        const list = original.apply(this, arguments);
        return list && list.filter((e) => EXTENSIONS.has(e.toLowerCase()));
      }
    );
    method(C.prototype, "getExtension", (original) =>
      function (name) {
        return EXTENSIONS.has(String(name).toLowerCase()) ? original.apply(this, arguments) : null;
      }
    );
    method(C.prototype, "getInternalformatParameter", (original) =>
      function (target, internalformat, pname) {
        const value = original.apply(this, arguments);
        // SAMPLES: the sample counts a format supports, within MAX_SAMPLES.
        return pname === 0x80a9 && value && value.filter ? value.filter((n) => n <= LIMITS.get(0x8d57)) : value;
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

  // Installed fonts: pages see the fonts every Windows install has and their
  // own web fonts, not fonts other software added. Each hidden family gets a
  // font face covering only U+0000, so text in that family falls through to
  // the next family in the list, exactly as if it were not installed. This
  // holds wherever a page names a font (style sheets, style attributes,
  // canvas). The faces are loaded now, before page scripts, and are left out
  // of everything the page can read from its font set.
  const hiddenFamilies = cfg.hiddenFamilies || [];
  const hiddenNames = new Set([...hiddenFamilies, ...(cfg.hiddenNames || [])].map((f) => f.toLowerCase()));
  const fontSet = isWindow ? G.document && G.document.fonts : G.fonts;
  if (G.FontFace && G.FontFaceSet && fontSet && hiddenFamilies.length) {
    const S = G.FontFaceSet.prototype;
    const values = S.values;
    const entries = S.entries;
    const remove = S.delete;
    const ours = new WeakSet();
    const OriginalFace = G.FontFace;
    let installed = false;
    const install = () => {
      if (installed) return;
      installed = true;
      for (const family of hiddenFamilies) {
        try {
          const face = new OriginalFace(family, 'local("Arial")', { unicodeRange: "U+0" });
          ours.add(face);
          S.add.call(fontSet, face);
          face.load().catch(() => {});
        } catch (e) {}
      }
    };
    // A worker draws text only through OffscreenCanvas, so its faces are
    // added when it first creates a canvas context. Creating font faces
    // while the DevTools protocol holds a new worker at start deadlocks it.
    if (isWindow) install();
    else if (G.OffscreenCanvas) {
      method(G.OffscreenCanvas.prototype, "getContext", (original) =>
        function () {
          install();
          return original.apply(this, arguments);
        }
      );
    }
    const visible = (set) => Array.from(values.call(set)).filter((f) => !ours.has(f));
    const pageLoading = (set) => visible(set).some((f) => f.status === "loading");
    // Loading events that only concern these faces end here, before any
    // listener of the page (this one is registered first).
    for (const type of ["loading", "loadingdone", "loadingerror"]) {
      fontSet.addEventListener(
        type,
        (e) => {
          if (Array.from(e.fontfaces).every((f) => ours.has(f)) && !pageLoading(fontSet)) e.stopImmediatePropagation();
        },
        true
      );
    }
    if (G.FontFaceSetLoadEvent) {
      getter(G.FontFaceSetLoadEvent.prototype, "fontfaces", (original) =>
        function () {
          return Object.freeze(Array.from(original.call(this)).filter((f) => !ours.has(f)));
        }
      );
    }
    const iterate = (original) =>
      function () {
        return original.call(new G.FontFaceSet(visible(this)));
      };
    method(S, "values", iterate);
    // keys and @@iterator are the same function as values on a set.
    for (const alias of ["keys", Symbol.iterator]) {
      const d = Object.getOwnPropertyDescriptor(S, alias);
      if (d && d.value === values) define(S, alias, { ...d, value: S.values });
      else method(S, alias, iterate);
    }
    method(S, "entries", () =>
      function () {
        return entries.call(new G.FontFaceSet(visible(this)));
      }
    );
    method(S, "forEach", () =>
      function (callback, thisArg) {
        for (const face of visible(this)) callback.call(thisArg, face, face, this);
      }
    );
    method(S, "has", (original) =>
      function (face) {
        return !ours.has(face) && original.apply(this, arguments);
      }
    );
    method(S, "delete", (original) =>
      function (face) {
        return !ours.has(face) && original.apply(this, arguments);
      }
    );
    method(S, "clear", () =>
      function () {
        for (const face of visible(this)) remove.call(this, face);
      }
    );
    method(S, "load", (original) =>
      function () {
        return original.apply(this, arguments).then((faces) => faces.filter((f) => !ours.has(f)));
      }
    );
    getter(S, "size", () =>
      function () {
        return visible(this).length;
      }
    );
    getter(S, "status", (original) =>
      function () {
        const status = original.call(this);
        return status === "loading" && !pageLoading(this) ? "loaded" : status;
      }
    );
  }
  if (isWindow && typeof G.queryLocalFonts === "function") {
    method(G, "queryLocalFonts", () => () =>
      Promise.reject(new DOMException("Local fonts are not available.", "NotAllowedError"))
    );
  }

  // Local fonts named in font sources, `local(...)`: the hidden ones are not
  // found, whether the source comes from a FontFace or a style sheet.
  const missing = 'local("f' + seed("fonts").toString(36) + '")';
  const keep = (source) =>
    typeof source === "string" && /local\s*\(/i.test(source)
      ? source.replace(/local\(\s*(["']?)([^"')]+)\1\s*\)/gi, (all, quote, name) =>
          hiddenNames.has(name.trim().toLowerCase()) ? missing : all
        )
      : source;
  if (G.FontFace) {
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

  // Style sheets: an @font-face rule can name a local font too. Rule text a
  // script hands to the CSS object model is cleaned on the way in, style
  // elements and markup a script inserts as they arrive, the page's own
  // style elements as the parser adds them, and linked sheets once loaded.
  // Sheets from other sites cannot be read and stay as they are.
  if (isWindow && hiddenNames.size && G.CSSStyleSheet && G.CSSStyleDeclaration && G.document) {
    const D = G.document;
    const Decl = G.CSSStyleDeclaration.prototype;
    const getProperty = Decl.getPropertyValue;
    const setProperty = Decl.setProperty;
    const done = new WeakSet();
    const fixRule = (rule) => {
      if (G.CSSFontFaceRule && rule instanceof G.CSSFontFaceRule) {
        const src = getProperty.call(rule.style, "src");
        const kept = keep(src);
        if (kept !== src) setProperty.call(rule.style, "src", kept);
      } else if (G.CSSImportRule && rule instanceof G.CSSImportRule) {
        if (rule.styleSheet) fix(rule.styleSheet, false);
      } else if (rule.cssRules) {
        for (let i = 0; i < rule.cssRules.length; i++) fixRule(rule.cssRules[i]);
      }
    };
    const fix = (sheet, again) => {
      if (!sheet || (done.has(sheet) && !again)) return;
      done.add(sheet);
      // A style element's text tells cheaply whether its rules need a look
      // (rules a script inserts were cleaned on the way in).
      const owner = sheet.ownerNode;
      if (owner && owner.localName === "style" && !/local\s*\(|@import/i.test(owner.textContent)) return;
      let rules;
      try {
        rules = sheet.cssRules;
      } catch (e) {
        return;
      }
      for (let i = 0; i < rules.length; i++) fixRule(rules[i]);
    };
    const scan = (root) => {
      const sheets = root && root.styleSheets;
      if (sheets) for (let i = 0; i < sheets.length; i++) fix(sheets[i], false);
    };
    const scanAround = (node) => {
      const root = node && node.isConnected ? node.getRootNode() : null;
      if (root) scan(root);
    };
    const isStyle = (node) => !!node && node.localName === "style";

    for (const proto of [G.CSSStyleSheet.prototype, G.CSSGroupingRule && G.CSSGroupingRule.prototype]) {
      for (const name of ["insertRule", "addRule"]) {
        method(proto, name, (original) =>
          function () {
            const args = Array.prototype.slice.call(arguments);
            args[name === "addRule" ? 1 : 0] = keep(args[name === "addRule" ? 1 : 0]);
            return original.apply(this, args);
          }
        );
      }
    }
    for (const name of ["replace", "replaceSync"]) {
      method(G.CSSStyleSheet.prototype, name, (original) =>
        function (text) {
          const args = Array.prototype.slice.call(arguments);
          args[0] = keep(text);
          return original.apply(this, args);
        }
      );
    }
    method(Decl, "setProperty", (original) =>
      function (name) {
        if (String(name).toLowerCase() !== "src") return original.apply(this, arguments);
        const args = Array.prototype.slice.call(arguments);
        args[1] = keep(args[1]);
        return original.apply(this, args);
      }
    );
    setter(Decl, "cssText", (original) =>
      function (value) {
        original.call(this, keep(value));
      }
    );

    // A style element added, or its text set, by a script: cleaned before
    // the script can measure anything with it. Markup with a font rule in
    // an element not yet in the document marks the element, and whatever
    // it is put into, until it arrives.
    const hasFace = (value) => /@font-face/i.test(String(value));
    const carrying = new WeakSet();
    const carries = (node) => isStyle(node) || (!!node && carrying.has(node));
    const arrived = (parent, node) => {
      if (node.isConnected) scanAround(node);
      else carrying.add(parent);
    };
    for (const name of ["appendChild", "insertBefore", "replaceChild"]) {
      method(G.Node.prototype, name, (original) =>
        function (node) {
          const result = original.apply(this, arguments);
          if (carries(node)) arrived(this, node);
          else if (isStyle(this)) scanAround(this);
          return result;
        }
      );
    }
    for (const name of ["append", "prepend"]) {
      method(G.Element.prototype, name, (original) =>
        function () {
          const result = original.apply(this, arguments);
          if (isStyle(this) || Array.prototype.some.call(arguments, carries)) arrived(this, this);
          return result;
        }
      );
    }
    setter(G.Node.prototype, "textContent", (original) =>
      function (value) {
        original.call(this, value);
        if (isStyle(this)) scanAround(this);
      }
    );
    setter(G.Element.prototype, "innerHTML", (original) =>
      function (value) {
        original.call(this, value);
        if (isStyle(this) || hasFace(value)) arrived(this, this);
      }
    );
    method(G.Element.prototype, "insertAdjacentHTML", (original) =>
      function (position, text) {
        const result = original.apply(this, arguments);
        if (hasFace(text)) arrived(this, this);
        return result;
      }
    );
    if (G.ShadowRoot) {
      setter(G.ShadowRoot.prototype, "innerHTML", (original) =>
        function (value) {
          original.call(this, value);
          if (hasFace(value)) scan(this);
        }
      );
    }

    // While the HTML parser builds the page, its style elements, before the
    // page renders or runs the next script; watching every DOM change for
    // the life of the page would slow down pages that change a lot. Linked
    // sheets (with their imports) once they have loaded.
    if (D.readyState === "loading") {
      const parsing = new G.MutationObserver(() => scan(D));
      parsing.observe(D, { childList: true, subtree: true });
      D.addEventListener(
        "DOMContentLoaded",
        () => {
          parsing.disconnect();
          scan(D);
        },
        { once: true }
      );
    }
    D.addEventListener(
      "load",
      (e) => {
        const t = e.target;
        if (t && (t.localName === "link" || t.localName === "style")) fix(t.sheet, true);
      },
      true
    );
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

  // Navigator: hardware figures picked per site (never above the real ones:
  // 2 to 8 cores, 2, 4 or 8 GB), one language, and no WebView2 brand.
  const navigators = [G.Navigator, G.WorkerNavigator].filter(Boolean).map((N) => N.prototype);
  const cores = 2 + (seed("cores") % 7);
  const memory = [2, 4, 8][seed("memory") % 3];
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

  // Shared workers. One from a network URL is prepared by Jello when its
  // script loads; the per-session marker added to the URL tells it which
  // request that is, and Jello loads the original URL, so the server never
  // sees the marker. One from a blob: URL is started from a wrapper blob
  // instead: this script with the site's key, then the original script. The
  // wrapper's URL is revoked once the worker has it, so the worker cannot
  // read it back; later connections from this document reuse it and reach
  // the same worker.
  if (isWindow && G.SharedWorker && cfg.marker) {
    const Original = G.SharedWorker;
    const createObjectURL = URL.createObjectURL;
    const revokeObjectURL = URL.revokeObjectURL;
    const blobs = new Map();
    method(URL, "createObjectURL", (original) =>
      function (object) {
        const url = original.apply(this, arguments);
        if (object instanceof G.Blob) blobs.set(url, object);
        return url;
      }
    );
    method(URL, "revokeObjectURL", (original) =>
      function (url) {
        blobs.delete(String(url));
        return original.apply(this, arguments);
      }
    );
    const wrappers = new Map();
    const wrapperFor = (url, options) => {
      const module = !!options && typeof options === "object" && options.type === "module";
      const name = typeof options === "string" ? options : (options && options.name) || "";
      const cacheKey = url + "\n" + name + "\n" + module;
      let wrapper = wrappers.get(cacheKey);
      if (wrapper) return [wrapper, false];
      const ownSite = registrable(G.location.hostname) || site;
      const workerCfg = {
        key: keyFor(ownSite),
        suffixes: cfg.suffixes,
        allow: cfg.allow,
        hiddenFamilies: cfg.hiddenFamilies,
        hiddenNames: cfg.hiddenNames,
        top: G.location.origin,
      };
      const original = blobs.get(url);
      const load = module ? "import " + JSON.stringify(url) + ";" : "importScripts(" + JSON.stringify(url) + ");";
      const parts = ["(" + source + ")(" + JSON.stringify(workerCfg) + ");\n", original || load];
      wrapper = createObjectURL.call(URL, new G.Blob(parts, { type: "text/javascript" }));
      wrappers.set(cacheKey, wrapper);
      return [wrapper, true];
    };
    const Shared = function SharedWorker(url) {
      if (!new.target) return Original.apply(this, arguments);
      const args = Array.prototype.slice.call(arguments);
      let created = null;
      try {
        const target = new URL(url, document.baseURI);
        if (target.protocol === "https:" || target.protocol === "http:") {
          target.search += (target.search ? "&" : "?") + cfg.marker;
          args[0] = target.href;
        } else if (target.protocol === "blob:") {
          const [wrapper, fresh] = wrapperFor(target.href, args[1]);
          args[0] = wrapper;
          if (fresh) created = wrapper;
        }
      } catch (e) {}
      try {
        return Reflect.construct(Original, args, new.target);
      } finally {
        if (created) revokeObjectURL.call(URL, created);
      }
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

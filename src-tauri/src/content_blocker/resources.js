// Builds adblock-rust resources from a uBlock Origin source tree: every
// scriptlet (`+js(...)` filters) and every redirect resource (`$redirect=`).
// uBlock Origin ships its scriptlets as ES modules, so they are loaded and
// serialized here rather than parsed as text. Returns a JSON array of
// { name, aliases, kind, content (base64), dependencies, permission }.
(async (base) => {
  const { builtinScriptlets } = await import(base + "js/resources/scriptlets.js");
  const redirects = (await import(base + "js/redirect-resources.js")).default;
  const toBase64 = (bytes) => {
    let bin = "";
    for (let i = 0; i < bytes.length; i += 0x8000) {
      bin += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
    }
    return btoa(bin);
  };
  const mimeOf = (name) =>
    ({
      css: "text/css",
      gif: "image/gif",
      html: "text/html",
      js: "application/javascript",
      json: "application/json",
      mp3: "audio/mp3",
      mp4: "video/mp4",
      png: "image/png",
      txt: "text/plain",
      xml: "text/xml",
    })[name.split(".").pop()] || "text/plain";

  const out = [];
  for (const s of builtinScriptlets) {
    out.push({
      name: s.name,
      aliases: s.aliases || [],
      // `.fn` entries are helpers that other scriptlets depend on.
      kind: { mime: s.name.endsWith(".fn") ? "fn/javascript" : "application/javascript" },
      content: toBase64(new TextEncoder().encode(s.fn.toString())),
      dependencies: s.dependencies || [],
      permission: s.requiresTrust ? 1 : 0,
    });
  }
  await Promise.all(
    [...redirects].map(async ([name, details]) => {
      const resp = await fetch(base + "web_accessible_resources/" + name);
      if (!resp.ok) return;
      out.push({
        name,
        aliases: details.alias ? [].concat(details.alias) : [],
        kind: { mime: mimeOf(name) },
        content: toBase64(new Uint8Array(await resp.arrayBuffer())),
      });
    })
  );
  return JSON.stringify(out);
})

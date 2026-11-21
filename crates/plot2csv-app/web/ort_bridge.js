// Bridge between the Rust app and onnxruntime-web, plus page helpers.
// Everything the page needs is embedded as gzip+base64 payload <script> tags.

async function wpPayload(id) {
  const el = document.getElementById("payload-" + id);
  if (!el) throw new Error("missing payload " + id);
  const res = await fetch("data:application/octet-stream;base64," + el.textContent.trim());
  let stream = res.body;
  if (el.dataset.gz === "1") stream = stream.pipeThrough(new DecompressionStream("gzip"));
  return new Uint8Array(await new Response(stream).arrayBuffer());
}
globalThis.wpPayload = wpPayload;

// Images pasted with Ctrl+V; drained by the app each frame.
globalThis.wpPasted = [];
document.addEventListener("paste", (e) => {
  const items = e.clipboardData ? e.clipboardData.items : [];
  for (const it of items) {
    if (it.kind === "file" && it.type.startsWith("image/")) {
      const f = it.getAsFile();
      if (f) f.arrayBuffer().then((b) => globalThis.wpPasted.push(new Uint8Array(b)));
    }
  }
});

(function () {
  let initPromise = null;
  let sessions = null;
  let metaText = null;

  async function init() {
    const ort = globalThis.ort;
    if (!ort) throw new Error("onnxruntime-web is not loaded");
    ort.env.wasm.numThreads = 1; // no cross-origin isolation under file://
    ort.env.wasm.proxy = false;
    ort.env.wasm.wasmBinary = await wpPayload("ort_wasm");
    const opts = { executionProviders: ["wasm"], graphOptimizationLevel: "all" };
    const [det, rec] = await Promise.all([wpPayload("det"), wpPayload("rec")]);
    sessions = {
      det: await ort.InferenceSession.create(det, opts),
      rec: await ort.InferenceSession.create(rec, opts),
    };
  }

  globalThis.wpOrt = {
    init() {
      if (!initPromise) initPromise = init();
      return initPromise;
    },
    async metaText() {
      if (metaText === null) metaText = new TextDecoder().decode(await wpPayload("meta"));
      return metaText;
    },
    async run(model, feeds, outputs) {
      await this.init();
      const ort = globalThis.ort;
      const f = {};
      for (const name of Object.keys(feeds)) {
        const t = feeds[name];
        f[name] = new ort.Tensor(t.type, t.data, t.dims);
      }
      const r = await sessions[model].run(f, outputs);
      return outputs.map((n) => ({ type: r[n].type, data: r[n].data, dims: Array.from(r[n].dims) }));
    },
  };
})();

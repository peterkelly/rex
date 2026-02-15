let rexWasm = null;
let rexWasmInit = null;
const rexAssetBaseUrl = new URL(".", import.meta.url);

function rexAssetUrl(path) {
  return new URL(path, rexAssetBaseUrl).toString();
}

async function ensureWasm() {
  if (rexWasm) return rexWasm;
  if (!rexWasmInit) {
    rexWasmInit = import(rexAssetUrl("rex_wasm.js")).then(async (m) => {
      await m.default();
      rexWasm = m;
      return m;
    });
  }
  return rexWasmInit;
}

self.onmessage = async (event) => {
  const msg = event.data ?? {};
  if (msg.type !== "run") return;
  const id = msg.id;
  try {
    const wasm = await ensureWasm();
    const output = wasm.evalToString(
      typeof msg.code === "string" ? msg.code : "",
      undefined
    );
    self.postMessage({ type: "result", id, ok: true, output });
  } catch (e) {
    self.postMessage({ type: "result", id, ok: false, error: String(e) });
  }
};

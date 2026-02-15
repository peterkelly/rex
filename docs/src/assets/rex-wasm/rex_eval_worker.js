let rexWasm = null;
let rexWasmInit = null;

async function ensureWasm() {
  if (rexWasm) return rexWasm;
  if (!rexWasmInit) {
    rexWasmInit = import("/assets/rex-wasm/rex_wasm.js").then(async (m) => {
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

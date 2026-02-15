let rexWasm = null;
let rexWasmInit = null;
let monacoInit = null;
let rexLanguageInit = false;
const rexEditors = new WeakMap();
const rexRuns = new WeakMap();
const rexInitialSource = new WeakMap();
const rexEditorNodes = new WeakMap();
let rexThemeObserver = null;

function installStyles() {
  if (document.getElementById("rex-repl-style")) return;
  const style = document.createElement("style");
  style.id = "rex-repl-style";
  style.textContent = `
    .rex-repl { margin: 1rem 0; background: transparent; border: none; padding: 0; --rex-code-bg: var(--quote-bg); }
    .rex-repl .rex-editor { width: 100%; min-height: 0; resize: vertical; overflow: auto; border: none; border-radius: 0; background: var(--rex-code-bg); }
    .rex-repl .rex-repl-actions { margin: 0.25rem 0 0; display: flex; gap: 0.4rem; align-items: center; justify-content: flex-end; }
    .rex-repl .rex-repl-actions button { cursor: pointer; margin: 0; padding: 2px 3px 0px 4px; font-size: 23px; border-style: solid; border-width: 1px; border-radius: 4px; border-color: var(--icons); background-color: var(--theme-popup-bg); color: var(--icons); transition: 100ms; transition-property: color,border-color,background-color; }
    .rex-repl .rex-repl-actions button:hover { color: var(--sidebar-active); border-color: var(--icons-hover); background-color: var(--theme-hover); }
    .rex-repl pre { margin: 0.5rem 0 0; padding: 0.5rem; white-space: pre-wrap; border: none; border-radius: 0; background: var(--rex-code-bg); }
    .rex-repl .monaco-editor,
    .rex-repl .monaco-editor .margin,
    .rex-repl .monaco-editor .monaco-editor-background { background: var(--rex-code-bg) !important; }
  `;
  document.head.appendChild(style);
}

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

async function ensureMonaco() {
  if (window.monaco) return window.monaco;
  if (!monacoInit) {
    monacoInit = new Promise((resolve, reject) => {
      const workerBase = "https://cdn.jsdelivr.net/npm/monaco-editor@0.52.2/min/";
      globalThis.MonacoEnvironment = {
        getWorkerUrl() {
          const src = `self.MonacoEnvironment={baseUrl:'${workerBase}'};importScripts('${workerBase}vs/base/worker/workerMain.js');`;
          return "data:text/javascript;charset=utf-8," + encodeURIComponent(src);
        }
      };

      const loader = document.createElement("script");
      loader.src = "https://cdn.jsdelivr.net/npm/monaco-editor@0.52.2/min/vs/loader.js";
      loader.async = true;
      loader.onload = () => {
        window.require.config({ paths: { vs: "https://cdn.jsdelivr.net/npm/monaco-editor@0.52.2/min/vs" } });
        window.require(["vs/editor/editor.main"], () => resolve(window.monaco), reject);
      };
      loader.onerror = () => reject(new Error("failed to load Monaco"));
      document.head.appendChild(loader);
    });
  }
  return monacoInit;
}

function lspKindToMonaco(kind, monaco) {
  const K = monaco.languages.CompletionItemKind;
  switch (kind) {
    case 3: return K.Function;
    case 6: return K.Variable;
    case 7: return K.Class;
    case 9: return K.Module;
    case 10: return K.Property;
    case 14: return K.Keyword;
    default: return K.Text;
  }
}

function lspSeverityToMonaco(severity, monaco) {
  const S = monaco.MarkerSeverity;
  switch (severity) {
    case 1: return S.Error;
    case 2: return S.Warning;
    case 3: return S.Info;
    case 4: return S.Hint;
    default: return S.Error;
  }
}

function hoverContentsToMarkdown(contents) {
  if (!contents) return null;
  if (typeof contents === "string") return contents;
  if (Array.isArray(contents)) {
    return contents
      .map((entry) => hoverContentsToMarkdown(entry))
      .filter((x) => typeof x === "string" && x.length > 0)
      .join("\n\n");
  }
  if (typeof contents === "object" && typeof contents.value === "string") {
    return contents.value;
  }
  if (typeof contents === "object" && typeof contents.language === "string" && typeof contents.value === "string") {
    return "```" + contents.language + "\n" + contents.value + "\n```";
  }
  return null;
}

function lspRangeToMonacoRange(range, monaco) {
  return new monaco.Range(
    range.start.line + 1,
    range.start.character + 1,
    range.end.line + 1,
    range.end.character + 1
  );
}

function decodeHex(hex) {
  let out = "";
  for (let i = 0; i + 1 < hex.length; i += 2) {
    const byte = Number.parseInt(hex.slice(i, i + 2), 16);
    out += String.fromCharCode(byte);
  }
  return out;
}

function cloneIconTemplate(id) {
  const tpl = document.getElementById(id);
  if (!tpl || !tpl.content || tpl.content.childElementCount === 0) return null;
  return tpl.content.firstElementChild.cloneNode(true);
}

function stopIconNode() {
  const span = document.createElement("span");
  span.className = "fa-svg";
  span.innerHTML = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 384 512"><path d="M64 96C64 78.3 78.3 64 96 64H288C305.7 64 320 78.3 320 96V416C320 433.7 305.7 448 288 448H96C78.3 448 64 433.7 64 416V96z"/></svg>';
  return span;
}

function setButtonIcon(button, kind, label) {
  button.textContent = "";
  let icon = null;
  if (kind === "play") icon = cloneIconTemplate("fa-play");
  if (kind === "reset") icon = cloneIconTemplate("fa-clock-rotate-left");
  if (kind === "stop") icon = stopIconNode();
  if (icon) {
    button.appendChild(icon);
  } else {
    button.textContent = label;
  }
  button.title = label;
  button.setAttribute("aria-label", label);
}

function resolveCodeBlockBackground() {
  const existing = document.querySelector("pre code.hljs");
  if (existing) {
    return getComputedStyle(existing).backgroundColor;
  }
  const pre = document.createElement("pre");
  const code = document.createElement("code");
  code.className = "hljs";
  pre.style.position = "absolute";
  pre.style.visibility = "hidden";
  pre.style.pointerEvents = "none";
  pre.appendChild(code);
  document.body.appendChild(pre);
  const bg = getComputedStyle(code).backgroundColor;
  pre.remove();
  return bg;
}

function applyReplBackground(root) {
  const bg = resolveCodeBlockBackground();
  if (bg) {
    root.style.setProperty("--rex-code-bg", bg);
  }
}

function installThemeWatcher() {
  if (rexThemeObserver) return;
  const applyAll = () => {
    document.querySelectorAll("[data-rex-repl]").forEach((root) => applyReplBackground(root));
  };
  rexThemeObserver = new MutationObserver(() => applyAll());
  rexThemeObserver.observe(document.documentElement, {
    attributes: true,
    attributeFilter: ["class", "data-theme", "style"]
  });
}

function fitEditorToContent(editor, editorNode) {
  const contentHeight = Math.max(80, Math.ceil(editor.getContentHeight()));
  editorNode.style.height = contentHeight + "px";
  editor.layout();
}

async function initRexLanguage(monaco, wasm) {
  if (rexLanguageInit) return;
  rexLanguageInit = true;

  monaco.languages.register({ id: "rex" });
  monaco.languages.setMonarchTokensProvider("rex", {
    keywords: ["declare", "import", "pub", "let", "in", "type", "match", "when", "if", "then", "else", "as", "for", "is", "fn"],
    operators: ["=", "->", "=>", "|", ":", ",", "."],
    tokenizer: {
      root: [
        [/[a-z_][A-Za-z0-9_]*/, { cases: { "@keywords": "keyword", "@default": "identifier" } }],
        [/[A-Z][A-Za-z0-9_]*/, "type.identifier"],
        [/-?\d+(\.\d+)?/, "number"],
        [/".*?"/, "string"],
        [/[{}()\[\]]/, "@brackets"],
        [/[=:,|.]/, "delimiter"],
        [/->|=>/, "operator"],
        [/--.*$/, "comment"]
      ]
    }
  });

  monaco.languages.registerCompletionItemProvider("rex", {
    triggerCharacters: [".", " "],
    provideCompletionItems(model, position) {
      try {
        const json = wasm.lspCompletionsToJson(
          model.getValue(),
          position.lineNumber - 1,
          position.column - 1
        );
        const items = JSON.parse(json);
        const suggestions = items.map((item) => ({
          label: item.label,
          kind: lspKindToMonaco(item.kind, monaco),
          insertText: item.insertText ?? item.label,
          detail: item.detail ?? undefined,
          documentation: item.documentation?.value ?? item.documentation ?? undefined
        }));
        return { suggestions };
      } catch (_) {
        return { suggestions: [] };
      }
    }
  });

  monaco.languages.registerHoverProvider("rex", {
    provideHover(model, position) {
      try {
        const json = wasm.lspHoverToJson(
          model.getValue(),
          position.lineNumber - 1,
          position.column - 1
        );
        const hover = JSON.parse(json);
        if (!hover) return null;
        const md = hoverContentsToMarkdown(hover.contents);
        if (!md) return null;
        const word = model.getWordAtPosition(position);
        const range = word
          ? new monaco.Range(position.lineNumber, word.startColumn, position.lineNumber, word.endColumn)
          : new monaco.Range(position.lineNumber, position.column, position.lineNumber, position.column + 1);
        return { range, contents: [{ value: md }] };
      } catch (_) {
        return null;
      }
    }
  });

  monaco.languages.registerDefinitionProvider("rex", {
    provideDefinition(model, position) {
      try {
        const json = wasm.lspGotoDefinitionToJson(
          model.getValue(),
          position.lineNumber - 1,
          position.column - 1
        );
        const location = JSON.parse(json);
        if (!location || !location.range) return null;
        return {
          uri: model.uri,
          range: new monaco.Range(
            location.range.start.line + 1,
            location.range.start.character + 1,
            location.range.end.line + 1,
            location.range.end.character + 1
          )
        };
      } catch (_) {
        return null;
      }
    }
  });

  monaco.languages.registerReferenceProvider("rex", {
    provideReferences(model, position, context) {
      try {
        const json = wasm.lspReferencesToJson(
          model.getValue(),
          position.lineNumber - 1,
          position.column - 1,
          !!context.includeDeclaration
        );
        const refs = JSON.parse(json);
        return refs.map((location) => ({
          uri: model.uri,
          range: lspRangeToMonacoRange(location.range, monaco)
        }));
      } catch (_) {
        return [];
      }
    }
  });

  monaco.languages.registerRenameProvider("rex", {
    resolveRenameLocation(model, position) {
      const word = model.getWordAtPosition(position);
      if (!word) return null;
      return {
        range: new monaco.Range(position.lineNumber, word.startColumn, position.lineNumber, word.endColumn),
        text: word.word
      };
    },
    provideRenameEdits(model, position, newName) {
      try {
        const json = wasm.lspRenameToJson(
          model.getValue(),
          position.lineNumber - 1,
          position.column - 1,
          newName
        );
        const edit = JSON.parse(json);
        if (!edit || !edit.changes) {
          return { edits: [] };
        }
        const key = "inmemory:///docs.rex";
        const sourceEdits = edit.changes[key] ?? [];
        const monacoEdits = sourceEdits.map((e) => ({
          resource: model.uri,
          edit: {
            range: lspRangeToMonacoRange(e.range, monaco),
            text: e.newText
          }
        }));
        return { edits: monacoEdits };
      } catch (_) {
        return { edits: [] };
      }
    }
  });

  monaco.languages.registerDocumentSymbolProvider("rex", {
    provideDocumentSymbols(model) {
      try {
        const json = wasm.lspDocumentSymbolsToJson(model.getValue());
        const symbols = JSON.parse(json);
        return symbols.map((symbol) => ({
          name: symbol.name,
          detail: symbol.detail ?? "",
          kind: symbol.kind,
          tags: [],
          containerName: "",
          range: lspRangeToMonacoRange(symbol.range, monaco),
          selectionRange: lspRangeToMonacoRange(symbol.selectionRange, monaco),
          children: (symbol.children ?? []).map((child) => ({
            name: child.name,
            detail: child.detail ?? "",
            kind: child.kind,
            tags: [],
            containerName: symbol.name,
            range: lspRangeToMonacoRange(child.range, monaco),
            selectionRange: lspRangeToMonacoRange(child.selectionRange, monaco)
          }))
        }));
      } catch (_) {
        return [];
      }
    }
  });

  monaco.languages.registerDocumentFormattingEditProvider("rex", {
    provideDocumentFormattingEdits(model) {
      try {
        const json = wasm.lspFormatToJson(model.getValue());
        const edits = JSON.parse(json);
        if (!edits) return [];
        return edits.map((edit) => ({
          range: lspRangeToMonacoRange(edit.range, monaco),
          text: edit.newText
        }));
      } catch (_) {
        return [];
      }
    }
  });
}

function bindDiagnostics(editor, monaco, wasm) {
  const model = editor.getModel();
  if (!model) return;

  let timer = null;
  const push = () => {
    try {
      const diagnostics = JSON.parse(wasm.lspDiagnosticsToJson(model.getValue()));
      const markers = diagnostics.map((diag) => ({
        startLineNumber: diag.range.start.line + 1,
        startColumn: diag.range.start.character + 1,
        endLineNumber: diag.range.end.line + 1,
        endColumn: diag.range.end.character + 1,
        message: diag.message,
        severity: lspSeverityToMonaco(diag.severity, monaco)
      }));
      monaco.editor.setModelMarkers(model, "rex-lsp", markers);
    } catch (_) {
      monaco.editor.setModelMarkers(model, "rex-lsp", []);
    }
  };

  push();
  editor.onDidChangeModelContent(() => {
    if (timer !== null) clearTimeout(timer);
    timer = setTimeout(push, 120);
  });
}

function createEvalWorker() {
  return new Worker("/assets/rex-wasm/rex_eval_worker.js", { type: "module" });
}

function setRunState(root, running) {
  const toggleButton = root.querySelector("[data-rex-run-toggle]");
  if (!toggleButton) return;
  if (running) {
    setButtonIcon(toggleButton, "stop", "Stop");
  } else {
    setButtonIcon(toggleButton, "play", "Run");
  }
  toggleButton.setAttribute("aria-pressed", running ? "true" : "false");
}

function resetRepl(root) {
  const editor = rexEditors.get(root);
  const original = rexInitialSource.get(root);
  if (!editor || typeof original !== "string") return;
  stopRepl(root);
  const model = editor.getModel();
  if (!model) return;
  model.setValue(original);
  const node = rexEditorNodes.get(root);
  if (node) fitEditorToContent(editor, node);
  const out = root.querySelector("[data-rex-output]");
  if (out) out.textContent = "";
}

function stopRepl(root, message) {
  const state = rexRuns.get(root);
  if (!state || !state.running) return;
  if (state.worker) {
    state.worker.terminate();
  }
  rexRuns.set(root, { worker: null, runId: state.runId, running: false });
  setRunState(root, false);
  if (message) {
    const out = root.querySelector("[data-rex-output]");
    if (out) out.textContent = message;
  }
}

async function runRepl(root) {
  const out = root.querySelector("[data-rex-output]");
  if (!out) return;
  out.hidden = false;
  const state = rexRuns.get(root);
  if (state?.running) return;
  out.textContent = "Running...";
  setRunState(root, true);

  const editor = rexEditors.get(root);
  const code = editor?.getModel()?.getValue() ?? "";
  const runId = (state?.runId ?? 0) + 1;
  const worker = createEvalWorker();
  rexRuns.set(root, { worker, runId, running: true });

  const finish = (text) => {
    const current = rexRuns.get(root);
    if (!current || current.runId !== runId) return;
    if (current.worker) {
      current.worker.terminate();
    }
    rexRuns.set(root, { worker: null, runId, running: false });
    setRunState(root, false);
    out.textContent = text;
  };

  worker.onmessage = (event) => {
    const msg = event.data ?? {};
    if (msg.type !== "result" || msg.id !== runId) return;
    if (msg.ok) {
      finish(String(msg.output ?? ""));
    } else {
      finish(String(msg.error ?? "Worker evaluation failed."));
    }
  };

  worker.onerror = (event) => {
    const msg = event && typeof event.message === "string"
      ? event.message
      : "Worker crashed.";
    finish(msg);
  };

  try {
    worker.postMessage({ type: "run", id: runId, code });
  } catch (e) {
    finish(String(e));
  }
}

async function initRepls() {
  installStyles();
  installThemeWatcher();
  const wasm = await ensureWasm();
  const monaco = await ensureMonaco();
  await initRexLanguage(monaco, wasm);

  document.querySelectorAll("[data-rex-repl]").forEach((root) => {
    if (root.dataset.rexInit === "1") return;
    root.dataset.rexInit = "1";
    const sourceHex = root.dataset.rexSourceHex ?? "";
    const source = decodeHex(sourceHex);

    const editorNode = document.createElement("div");
    editorNode.className = "rex-editor";
    const actions = document.createElement("div");
    actions.className = "rex-repl-actions";
    const toggleButton = document.createElement("button");
    toggleButton.type = "button";
    toggleButton.setAttribute("data-rex-run-toggle", "");
    setButtonIcon(toggleButton, "play", "Run");
    const resetButton = document.createElement("button");
    resetButton.type = "button";
    resetButton.setAttribute("data-rex-reset", "");
    setButtonIcon(resetButton, "reset", "Undo Changes");
    actions.appendChild(resetButton);
    actions.appendChild(toggleButton);
    const output = document.createElement("pre");
    output.setAttribute("data-rex-output", "");
    output.hidden = true;

    root.appendChild(editorNode);
    root.appendChild(actions);
    root.appendChild(output);

    const model = monaco.editor.createModel(source, "rex");
    const editor = monaco.editor.create(editorNode, {
      model,
      automaticLayout: true,
      minimap: { enabled: false },
      guides: {
        indentation: false,
        highlightActiveIndentation: false,
        bracketPairs: false,
        bracketPairsHorizontal: false,
        highlightActiveBracketPair: false
      },
      fontSize: 13,
      lineNumbers: "on",
      scrollBeyondLastLine: false
    });
    rexEditors.set(root, editor);
    rexEditorNodes.set(root, editorNode);
    rexInitialSource.set(root, source);
    rexRuns.set(root, { worker: null, runId: 0, running: false });
    applyReplBackground(root);
    fitEditorToContent(editor, editorNode);
    requestAnimationFrame(() => fitEditorToContent(editor, editorNode));
    bindDiagnostics(editor, monaco, wasm);
    setRunState(root, false);

    toggleButton.addEventListener("click", () => {
      const state = rexRuns.get(root);
      if (state?.running) {
        stopRepl(root, "Stopped.");
      } else {
        void runRepl(root);
      }
    });
    resetButton.addEventListener("click", () => {
      resetRepl(root);
    });
  });
}

if (document.readyState === "loading") {
  document.addEventListener("DOMContentLoaded", () => { void initRepls(); });
} else {
  void initRepls();
}

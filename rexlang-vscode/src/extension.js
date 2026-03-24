'use strict';

const fs = require('fs');
const path = require('path');
const cp = require('child_process');
const vscode = require('vscode');
const languageClient = require('vscode-languageclient/node');
const { LanguageClient } = languageClient;
const { buildFunctionCallTemplate } = require('./snippet_builder');

let client;
let queryOutputChannel;

function activate(context) {
  const outputChannel = vscode.window.createOutputChannel('Rex Language Server');
  const traceChannel = vscode.window.createOutputChannel('Rex Language Server Trace');
  queryOutputChannel = vscode.window.createOutputChannel('Rex Query Results');
  context.subscriptions.push(queryOutputChannel);

  const resolved = resolveServerCommand(context);
  if (!resolved) {
    outputChannel.appendLine('[rex] rexlang-lsp not found; syntax highlighting enabled, LSP disabled');
    vscode.window.showWarningMessage(
      'Rex: language server (rexlang-lsp) not found. Syntax highlighting works, but LSP features are disabled. ' +
        'Install rexlang-lsp (e.g. `cargo install --path rexlang-lsp`) or set `rex.serverPath`.'
    );
    return;
  }

  const { command, args } = resolved;
  outputChannel.appendLine(`[rex] server command = ${command} ${args.join(' ')}`);

  const serverOptions = {
    command,
    args
  };

  const clientOptions = {
    documentSelector: [{ scheme: 'file', language: 'rex' }],
    synchronize: {
      fileEvents: vscode.workspace.createFileSystemWatcher('**/*.rex')
    },
    outputChannel,
    traceOutputChannel: traceChannel
  };

  if (languageClient.RevealOutputChannelOn) {
    clientOptions.revealOutputChannelOn = languageClient.RevealOutputChannelOn.Error;
  }

  try {
    client = new LanguageClient(
      'rexLanguageServer',
      'Rex Language Server',
      serverOptions,
      clientOptions
    );

    if (client.onDidChangeState) {
      client.onDidChangeState((e) => {
        outputChannel.appendLine(`[rex] state: ${e.oldState} -> ${e.newState}`);
      });
    }

    const disposable = client.start();
    context.subscriptions.push(disposable);

    context.subscriptions.push(
      vscode.commands.registerCommand('rex.showExpectedTypeAtCursor', async () => {
        await runLspQueryAtCursor('expected type', async (uri, position) => {
          const out = await client.sendRequest('workspace/executeCommand', {
            command: 'rex.expectedTypeAt',
            arguments: [uri.toString(), position.line, position.character]
          });
          return out && typeof out.expectedType === 'string'
            ? out.expectedType
            : null;
        }, { showPanel: true });
      })
    );

    context.subscriptions.push(
      vscode.commands.registerCommand('rex.showFunctionsProducingExpectedTypeAtCursor', async () => {
        await runLspQueryAtCursor('functions producing expected type', async (uri, position) => {
          const out = await client.sendRequest('workspace/executeCommand', {
            command: 'rex.functionsProducingExpectedTypeAt',
            arguments: [uri.toString(), position.line, position.character]
          });
          if (!out || !Array.isArray(out.items)) {
            return null;
          }
          return out.items.join('\n');
        }, { showPanel: true });
      })
    );

    context.subscriptions.push(
      vscode.commands.registerCommand('rex.showFunctionsAcceptingInferredTypeAtCursor', async () => {
        await runLspQueryAtCursor('functions accepting inferred type', async (uri, position) => {
          const out = await client.sendRequest('workspace/executeCommand', {
            command: 'rex.functionsAcceptingInferredTypeAt',
            arguments: [uri.toString(), position.line, position.character]
          });
          if (!out || !Array.isArray(out.items)) {
            return null;
          }
          const header = typeof out.inferredType === 'string'
            ? `inferredType: ${out.inferredType}\n`
            : '';
          return `${header}${out.items.join('\n')}`.trim();
        }, { showPanel: true });
      })
    );

    context.subscriptions.push(
      vscode.commands.registerCommand('rex.showAdaptersFromInferredToExpectedAtCursor', async () => {
        await runLspQueryAtCursor('adapters from inferred to expected type', async (uri, position) => {
          const out = await client.sendRequest('workspace/executeCommand', {
            command: 'rex.adaptersFromInferredToExpectedAt',
            arguments: [uri.toString(), position.line, position.character]
          });
          if (!out || !Array.isArray(out.items)) {
            return null;
          }
          const lines = [];
          if (typeof out.inferredType === 'string') {
            lines.push(`inferredType: ${out.inferredType}`);
          }
          if (typeof out.expectedType === 'string') {
            lines.push(`expectedType: ${out.expectedType}`);
          }
          if (lines.length) {
            lines.push('');
          }
          lines.push(...out.items);
          return lines.join('\n');
        }, { showPanel: true });
      })
    );

    context.subscriptions.push(
      vscode.commands.registerCommand('rex.showFunctionsCompatibleWithInScopeValuesAtCursor', async () => {
        await runLspQueryAtCursor('functions compatible with in-scope values', async (uri, position) => {
          const out = await client.sendRequest('workspace/executeCommand', {
            command: 'rex.functionsCompatibleWithInScopeValuesAt',
            arguments: [uri.toString(), position.line, position.character]
          });
          if (!out || !Array.isArray(out.items)) {
            return null;
          }
          return out.items.join('\n');
        }, { showPanel: true });
      })
    );

    context.subscriptions.push(
      vscode.commands.registerCommand('rex.showHolesExpectedTypes', async () => {
        await runLspQueryForActiveDocument('holes expected types', async (uri) => {
          const out = await client.sendRequest('workspace/executeCommand', {
            command: 'rex.holesExpectedTypes',
            arguments: [uri.toString()]
          });
          if (!out || !Array.isArray(out.holes)) {
            return null;
          }
          return formatHolesExpectedTypes(out.holes);
        }, { showPanel: true });
      })
    );

    context.subscriptions.push(
      vscode.commands.registerCommand('rex.showSemanticLoopStepAtCursor', async () => {
        await runLspQueryAtCursor('semantic loop step', async (uri, position) => {
          const out = await client.sendRequest('workspace/executeCommand', {
            command: 'rex.semanticLoopStep',
            arguments: [uri.toString(), position.line, position.character]
          });
          return out ? formatSemanticLoopStep(out) : null;
        }, { showPanel: true });
      })
    );

    context.subscriptions.push(
      vscode.commands.registerCommand('rex.applySemanticLoopQuickFixAtCursor', async () => {
        await applySemanticLoopQuickFixAtCursor();
      })
    );

    context.subscriptions.push(
      vscode.commands.registerCommand('rex.applyBestSemanticLoopQuickFixesAtCursor', async () => {
        await applyBestSemanticLoopQuickFixesAtCursor();
      })
    );

    context.subscriptions.push(
      vscode.commands.registerCommand('rex.previewBestSemanticLoopQuickFixesAtCursor', async () => {
        await previewBestSemanticLoopQuickFixesAtCursor();
      })
    );

    context.subscriptions.push(
      vscode.commands.registerCommand('rex.insertFunctionFromCandidatesAtCursor', async () => {
        await insertFunctionFromCandidatesAtCursor();
      })
    );

    context.subscriptions.push(
      vscode.commands.registerCommand('rex.insertFunctionFromCandidatesWithTypeCommentAtCursor', async () => {
        await insertFunctionFromCandidatesAtCursor({ forceTypeComment: true });
      })
    );
  } catch (err) {
    outputChannel.appendLine(`[rex] failed to start language client: ${err}`);
    vscode.window.showErrorMessage(
      'Rex Language Server failed to start. See Output → "Rex Language Server" for details.'
    );
  }
}

async function runLspQueryAtCursor(label, queryFn, options = {}) {
  if (!client) {
    vscode.window.showWarningMessage(`Rex: language server is not running.`);
    return;
  }
  const editor = vscode.window.activeTextEditor;
  if (!editor || editor.document.languageId !== 'rex') {
    vscode.window.showInformationMessage('Rex: open a .rex file and place the cursor first.');
    return;
  }
  const position = editor.selection.active;
  try {
    const result = await queryFn(editor.document.uri, position);
    if (!result) {
      vscode.window.showInformationMessage(`Rex: no ${label} available at cursor.`);
      if (options.showPanel) {
        writeQueryPanel(label, editor.document.uri, position, '<none>');
      }
      return;
    }
    if (options.showPanel) {
      writeQueryPanel(label, editor.document.uri, position, result);
    }
    const short = result.length > 2000 ? `${result.slice(0, 2000)}\n...` : result;
    vscode.window.showInformationMessage(`Rex ${label}: ${short}`);
  } catch (err) {
    vscode.window.showErrorMessage(`Rex: failed to query ${label}: ${err}`);
  }
}

async function runLspQueryForActiveDocument(label, queryFn, options = {}) {
  if (!client) {
    vscode.window.showWarningMessage('Rex: language server is not running.');
    return;
  }
  const editor = vscode.window.activeTextEditor;
  if (!editor || editor.document.languageId !== 'rex') {
    vscode.window.showInformationMessage('Rex: open a .rex file first.');
    return;
  }
  const position = editor.selection.active;
  try {
    const result = await queryFn(editor.document.uri);
    if (!result) {
      vscode.window.showInformationMessage(`Rex: no ${label} available.`);
      if (options.showPanel) {
        writeQueryPanel(label, editor.document.uri, position, '<none>');
      }
      return;
    }
    if (options.showPanel) {
      writeQueryPanel(label, editor.document.uri, position, result);
    }
    const short = result.length > 2000 ? `${result.slice(0, 2000)}\n...` : result;
    vscode.window.showInformationMessage(`Rex ${label}: ${short}`);
  } catch (err) {
    vscode.window.showErrorMessage(`Rex: failed to query ${label}: ${err}`);
  }
}

function formatHolesExpectedTypes(holes) {
  if (!holes.length) {
    return '<none>';
  }
  const lines = [];
  for (const hole of holes) {
    const name = typeof hole.name === 'string' ? hole.name : '_';
    const line = Number.isInteger(hole.line) ? hole.line + 1 : '?';
    const character = Number.isInteger(hole.character) ? hole.character + 1 : '?';
    const expectedType = typeof hole.expectedType === 'string' ? hole.expectedType : '<unknown>';
    lines.push(`${name} @ ${line}:${character} : ${expectedType}`);
  }
  return lines.join('\n');
}

function formatSemanticLoopStep(step) {
  if (!step || typeof step !== 'object') {
    return null;
  }

  const expected = typeof step.expectedType === 'string' ? step.expectedType : '<none>';
  const localDiagnostics = Array.isArray(step.localDiagnostics) ? step.localDiagnostics : [];
  const quickFixTitles = Array.isArray(step.quickFixTitles) ? step.quickFixTitles : [];
  const quickFixes = Array.isArray(step.quickFixes) ? step.quickFixes : [];
  const functionCandidates = Array.isArray(step.functionCandidates) ? step.functionCandidates : [];
  const holeFillCandidates = Array.isArray(step.holeFillCandidates) ? step.holeFillCandidates : [];
  const inScopeValues = Array.isArray(step.inScopeValues) ? step.inScopeValues : [];
  const functionsAcceptingInferredType = Array.isArray(step.functionsAcceptingInferredType)
    ? step.functionsAcceptingInferredType
    : [];
  const adaptersFromInferredToExpectedType = Array.isArray(step.adaptersFromInferredToExpectedType)
    ? step.adaptersFromInferredToExpectedType
    : [];
  const functionsCompatibleWithInScopeValues = Array.isArray(step.functionsCompatibleWithInScopeValues)
    ? step.functionsCompatibleWithInScopeValues
    : [];

  const lines = [];
  lines.push(`expectedType: ${expected}`);
  lines.push(`localDiagnostics: ${localDiagnostics.length}`);
  lines.push(`quickFixes: ${quickFixes.length}`);
  lines.push(`functionCandidates: ${functionCandidates.length}`);
  lines.push(`holeFillCandidates: ${holeFillCandidates.length}`);
  lines.push(`inScopeValues: ${inScopeValues.length}`);
  lines.push(`functionsAcceptingInferredType: ${functionsAcceptingInferredType.length}`);
  lines.push(`adaptersFromInferredToExpectedType: ${adaptersFromInferredToExpectedType.length}`);
  lines.push(`functionsCompatibleWithInScopeValues: ${functionsCompatibleWithInScopeValues.length}`);

  if (quickFixes.length) {
    lines.push('');
    lines.push('quickFixes:');
    for (const fix of quickFixes) {
      const title = typeof fix.title === 'string' ? fix.title : '<unknown>';
      const id = typeof fix.id === 'string' ? fix.id : '';
      lines.push(id ? `- ${title} (${id})` : `- ${title}`);
    }
  } else if (quickFixTitles.length) {
    lines.push('');
    lines.push('quickFixTitles:');
    for (const title of quickFixTitles) {
      lines.push(`- ${title}`);
    }
  }

  if (holeFillCandidates.length) {
    lines.push('');
    lines.push('holeFillCandidates:');
    for (const candidate of holeFillCandidates.slice(0, 8)) {
      const name = typeof candidate.name === 'string' ? candidate.name : '<unknown>';
      const replacement = typeof candidate.replacement === 'string'
        ? candidate.replacement
        : '<unknown>';
      lines.push(`- ${name} => ${replacement}`);
    }
  }

  return lines.join('\n');
}

function writeQueryPanel(label, uri, position, result) {
  if (!queryOutputChannel) {
    return;
  }
  queryOutputChannel.appendLine('='.repeat(80));
  queryOutputChannel.appendLine(`Query: ${label}`);
  queryOutputChannel.appendLine(`File: ${uri.toString()}`);
  queryOutputChannel.appendLine(`Cursor: line ${position.line + 1}, column ${position.character + 1}`);
  queryOutputChannel.appendLine('-'.repeat(80));
  queryOutputChannel.appendLine(result);
  queryOutputChannel.appendLine('');
  queryOutputChannel.show(true);
}

function formatBulkSemanticLoopResult(out) {
  if (!out || typeof out !== 'object') {
    return '<none>';
  }
  const appliedCount = Number.isInteger(out.appliedCount) ? out.appliedCount : 0;
  const stoppedReason = typeof out.stoppedReason === 'string' ? out.stoppedReason : 'unknown';
  const stoppedReasonDetail = typeof out.stoppedReasonDetail === 'string'
    ? out.stoppedReasonDetail
    : '<none>';
  const strategy = typeof out.strategy === 'string' ? out.strategy : '<unknown>';
  const lastDiagnosticsDelta = Number.isInteger(out.lastDiagnosticsDelta)
    ? out.lastDiagnosticsDelta
    : 0;
  const noImprovementStreak = Number.isInteger(out.noImprovementStreak)
    ? out.noImprovementStreak
    : 0;
  const seenStatesCount = Number.isInteger(out.seenStatesCount)
    ? out.seenStatesCount
    : 0;
  const steps = Array.isArray(out.steps) ? out.steps : [];

  const lines = [
    `strategy: ${strategy}`,
    `appliedCount: ${appliedCount}`,
    `stepCount: ${steps.length}`,
    `stoppedReason: ${stoppedReason}`,
    `stoppedReasonDetail: ${stoppedReasonDetail}`,
    `lastDiagnosticsDelta: ${lastDiagnosticsDelta}`,
    `noImprovementStreak: ${noImprovementStreak}`,
    `seenStatesCount: ${seenStatesCount}`
  ];
  if (steps.length) {
    lines.push('');
    lines.push('steps:');
    for (const step of steps) {
      const index = Number.isInteger(step.index) ? step.index : '?';
      const quickFixTitle = step.quickFix && typeof step.quickFix.title === 'string'
        ? step.quickFix.title
        : '<unknown>';
      const beforeCount = Number.isInteger(step.diagnosticsBeforeCount)
        ? step.diagnosticsBeforeCount
        : '?';
      const afterCount = Number.isInteger(step.diagnosticsAfterCount)
        ? step.diagnosticsAfterCount
        : '?';
      const delta = Number.isInteger(step.diagnosticsDelta) ? step.diagnosticsDelta : '?';
      lines.push(
        `- [${index}] ${quickFixTitle} (before=${beforeCount}, after=${afterCount}, delta=${delta})`
      );
    }
  }
  return lines.join('\n');
}

function workspaceEditFromLspEdit(editObj) {
  if (!editObj || typeof editObj !== 'object') {
    return null;
  }
  const workspaceEdit = new vscode.WorkspaceEdit();
  const changes = editObj.changes && typeof editObj.changes === 'object'
    ? editObj.changes
    : null;
  if (!changes) {
    return null;
  }

  for (const [uriText, edits] of Object.entries(changes)) {
    if (!Array.isArray(edits)) {
      continue;
    }
    const uri = vscode.Uri.parse(uriText);
    for (const edit of edits) {
      const range = edit && edit.range ? edit.range : null;
      const newText = edit && typeof edit.newText === 'string' ? edit.newText : null;
      if (!range || newText === null) {
        continue;
      }
      const start = new vscode.Position(range.start.line, range.start.character);
      const end = new vscode.Position(range.end.line, range.end.character);
      workspaceEdit.replace(uri, new vscode.Range(start, end), newText);
    }
  }
  return workspaceEdit;
}

async function applySemanticLoopQuickFixAtCursor() {
  if (!client) {
    vscode.window.showWarningMessage('Rex: language server is not running.');
    return;
  }
  const editor = vscode.window.activeTextEditor;
  if (!editor || editor.document.languageId !== 'rex') {
    vscode.window.showInformationMessage('Rex: open a .rex file and place the cursor first.');
    return;
  }
  const uri = editor.document.uri;
  const position = editor.selection.active;

  try {
    const step = await client.sendRequest('workspace/executeCommand', {
      command: 'rex.semanticLoopStep',
      arguments: [uri.toString(), position.line, position.character]
    });
    const quickFixes = step && Array.isArray(step.quickFixes) ? step.quickFixes : [];
    if (quickFixes.length === 0) {
      vscode.window.showInformationMessage('Rex: no semantic loop quick-fixes at cursor.');
      return;
    }

    const picks = quickFixes.map((fix) => ({
      label: typeof fix.title === 'string' ? fix.title : '<unknown>',
      description: typeof fix.id === 'string' ? fix.id : '',
      detail: typeof fix.kind === 'string' ? fix.kind : '',
      id: typeof fix.id === 'string' ? fix.id : ''
    })).filter((item) => item.id);
    if (picks.length === 0) {
      vscode.window.showInformationMessage('Rex: no applicable semantic loop quick-fixes.');
      return;
    }

    const chosen = await vscode.window.showQuickPick(picks, {
      placeHolder: 'Select a semantic loop quick-fix to apply'
    });
    if (!chosen) {
      return;
    }

    const out = await client.sendRequest('workspace/executeCommand', {
      command: 'rex.semanticLoopApplyQuickFixAt',
      arguments: [uri.toString(), position.line, position.character, chosen.id]
    });
    const quickFix = out && out.quickFix ? out.quickFix : null;
    const workspaceEdit = workspaceEditFromLspEdit(quickFix && quickFix.edit ? quickFix.edit : null);
    if (!workspaceEdit) {
      vscode.window.showInformationMessage('Rex: selected quick-fix has no applicable edit.');
      return;
    }
    const applied = await vscode.workspace.applyEdit(workspaceEdit);
    if (!applied) {
      vscode.window.showErrorMessage('Rex: failed to apply semantic loop quick-fix edit.');
      return;
    }
    vscode.window.showInformationMessage(`Rex: applied quick-fix: ${chosen.label}`);
  } catch (err) {
    vscode.window.showErrorMessage(`Rex: failed to apply semantic loop quick-fix: ${err}`);
  }
}

async function applyBestSemanticLoopQuickFixesAtCursor() {
  if (!client) {
    vscode.window.showWarningMessage('Rex: language server is not running.');
    return;
  }
  const editor = vscode.window.activeTextEditor;
  if (!editor || editor.document.languageId !== 'rex') {
    vscode.window.showInformationMessage('Rex: open a .rex file and place the cursor first.');
    return;
  }
  const uri = editor.document.uri;
  const position = editor.selection.active;
  const config = vscode.workspace.getConfiguration('rex');
  const configuredMaxSteps = Number(config.get('semanticLoopBulkMaxSteps', 3));
  const maxSteps = Number.isFinite(configuredMaxSteps)
    ? Math.max(1, Math.min(20, Math.floor(configuredMaxSteps)))
    : 3;
  const configuredStrategy = String(config.get('semanticLoopBulkStrategy', 'conservative'));
  const strategy = configuredStrategy === 'aggressive' ? 'aggressive' : 'conservative';

  try {
    const out = await client.sendRequest('workspace/executeCommand', {
      command: 'rex.semanticLoopApplyBestQuickFixesAt',
      arguments: [uri.toString(), position.line, position.character, maxSteps, strategy, false]
    });
    if (!out || typeof out.updatedText !== 'string') {
      vscode.window.showInformationMessage('Rex: no bulk semantic quick-fix result available.');
      return;
    }

    const updatedText = out.updatedText;
    if (updatedText === editor.document.getText()) {
      vscode.window.showInformationMessage('Rex: no applicable bulk semantic quick-fixes.');
      return;
    }

    const fullRange = new vscode.Range(
      editor.document.positionAt(0),
      editor.document.positionAt(editor.document.getText().length)
    );
    const ok = await editor.edit((builder) => {
      builder.replace(fullRange, updatedText);
    });
    if (!ok) {
      vscode.window.showErrorMessage('Rex: failed to apply bulk semantic quick-fixes.');
      return;
    }
    const appliedCount = Number.isInteger(out.appliedCount) ? out.appliedCount : '?';
    const stoppedReason = typeof out.stoppedReason === 'string' ? out.stoppedReason : 'unknown';
    const stoppedReasonDetail = typeof out.stoppedReasonDetail === 'string'
      ? out.stoppedReasonDetail
      : '<none>';
    const stepCount = Array.isArray(out.steps) ? out.steps.length : '?';
    const usedStrategy = typeof out.strategy === 'string' ? out.strategy : strategy;
    const lastDiagnosticsDelta = Number.isInteger(out.lastDiagnosticsDelta)
      ? out.lastDiagnosticsDelta
      : '?';
    const noImprovementStreak = Number.isInteger(out.noImprovementStreak)
      ? out.noImprovementStreak
      : '?';
    const seenStatesCount = Number.isInteger(out.seenStatesCount)
      ? out.seenStatesCount
      : '?';
    writeQueryPanel(
      'semantic loop bulk apply',
      uri,
      position,
      formatBulkSemanticLoopResult(out)
    );
    vscode.window.showInformationMessage(
      `Rex: applied ${appliedCount} bulk semantic quick-fix(es) in ${stepCount} step(s); strategy=${usedStrategy}; stop=${stoppedReason}; detail=${stoppedReasonDetail}; delta=${lastDiagnosticsDelta}; streak=${noImprovementStreak}; states=${seenStatesCount}.`
    );
  } catch (err) {
    vscode.window.showErrorMessage(`Rex: failed to apply bulk semantic quick-fixes: ${err}`);
  }
}

async function previewBestSemanticLoopQuickFixesAtCursor() {
  if (!client) {
    vscode.window.showWarningMessage('Rex: language server is not running.');
    return;
  }
  const editor = vscode.window.activeTextEditor;
  if (!editor || editor.document.languageId !== 'rex') {
    vscode.window.showInformationMessage('Rex: open a .rex file and place the cursor first.');
    return;
  }
  const uri = editor.document.uri;
  const position = editor.selection.active;
  const config = vscode.workspace.getConfiguration('rex');
  const configuredMaxSteps = Number(config.get('semanticLoopBulkMaxSteps', 3));
  const maxSteps = Number.isFinite(configuredMaxSteps)
    ? Math.max(1, Math.min(20, Math.floor(configuredMaxSteps)))
    : 3;
  const configuredStrategy = String(config.get('semanticLoopBulkStrategy', 'conservative'));
  const strategy = configuredStrategy === 'aggressive' ? 'aggressive' : 'conservative';

  try {
    const out = await client.sendRequest('workspace/executeCommand', {
      command: 'rex.semanticLoopApplyBestQuickFixesAt',
      arguments: [uri.toString(), position.line, position.character, maxSteps, strategy, true]
    });
    if (!out || typeof out.updatedText !== 'string') {
      vscode.window.showInformationMessage('Rex: no bulk semantic quick-fix preview available.');
      return;
    }
    writeQueryPanel(
      'semantic loop bulk preview (dry run)',
      uri,
      position,
      formatBulkSemanticLoopResult(out)
    );
    const appliedCount = Number.isInteger(out.appliedCount) ? out.appliedCount : '?';
    const stoppedReason = typeof out.stoppedReason === 'string' ? out.stoppedReason : 'unknown';
    const stepCount = Array.isArray(out.steps) ? out.steps.length : '?';
    const usedStrategy = typeof out.strategy === 'string' ? out.strategy : strategy;
    vscode.window.showInformationMessage(
      `Rex: previewed ${appliedCount} bulk semantic quick-fix(es) in ${stepCount} step(s); strategy=${usedStrategy}; stop=${stoppedReason}.`
    );
  } catch (err) {
    vscode.window.showErrorMessage(`Rex: failed to preview bulk semantic quick-fixes: ${err}`);
  }
}

async function insertFunctionFromCandidatesAtCursor(options = {}) {
  if (!client) {
    vscode.window.showWarningMessage('Rex: language server is not running.');
    return;
  }
  const editor = vscode.window.activeTextEditor;
  if (!editor || editor.document.languageId !== 'rex') {
    vscode.window.showInformationMessage('Rex: open a .rex file and place the cursor first.');
    return;
  }

  const uri = editor.document.uri;
  const position = editor.selection.active;
  try {
    const out = await client.sendRequest('workspace/executeCommand', {
      command: 'rex.semanticLoopStep',
      arguments: [uri.toString(), position.line, position.character]
    });
    const items = out && Array.isArray(out.functionCandidates) ? out.functionCandidates : [];
    if (items.length === 0) {
      vscode.window.showInformationMessage('Rex: no compatible function candidates at cursor.');
      return;
    }
    const expectedType = out && typeof out.expectedType === 'string'
      ? out.expectedType
      : '';

    const picks = items.map((item) => {
      const sep = item.indexOf(' : ');
      const name = sep >= 0 ? item.slice(0, sep) : item;
      const type = sep >= 0 ? item.slice(sep + 3) : '';
      return {
        label: name,
        description: type,
        detail: item
      };
    });
    const chosen = await vscode.window.showQuickPick(picks, {
      placeHolder: expectedType
        ? `Select a function candidate to produce ${expectedType}`
        : 'Select a function candidate to insert'
    });
    if (!chosen) {
      return;
    }

    const snippet = buildFunctionCallSnippet(
      chosen.label,
      chosen.description || '',
      { forceTypeComment: !!options.forceTypeComment }
    );
    await editor.insertSnippet(snippet, position);
  } catch (err) {
    vscode.window.showErrorMessage(`Rex: failed to insert candidate function: ${err}`);
  }
}

function buildFunctionCallSnippet(name, typeSignature, options = {}) {
  const config = vscode.workspace.getConfiguration('rex');
  const includeTypeComment = !!options.forceTypeComment || !!config.get('insertCandidateTypeComment');
  return new vscode.SnippetString(
    buildFunctionCallTemplate(name, typeSignature, { includeTypeComment })
  );
}

function canExecute(commandPath) {
  try {
    const res = cp.spawnSync(commandPath, ['--version'], {
      encoding: 'utf8',
      stdio: 'ignore'
    });
    if (res.error) {
      return false;
    }
    return res.status === 0 || res.status === null;
  } catch (_) {
    return false;
  }
}

function resolveServerCommand(context) {
  const config = vscode.workspace.getConfiguration('rex');
  const configured = config.get('serverPath');
  if (configured && configured.trim()) {
    if (!fs.existsSync(configured)) {
      vscode.window.showErrorMessage(
        `rex.serverPath does not exist: ${configured}`
      );
      return null;
    }
    return { command: configured, args: [] };
  }

  const binName = process.platform === 'win32' ? 'rexlang-lsp.exe' : 'rexlang-lsp';

  // If you choose to ship a prebuilt server binary with the extension, put it here.
  const bundled = path.join(context.extensionPath, 'server', binName);
  if (fs.existsSync(bundled)) {
    return { command: bundled, args: [] };
  }

  // Development convenience: when running from the repo, prefer the workspace build output.
  if (context.extensionMode === vscode.ExtensionMode.Development) {
    const devPath = path.join(context.extensionPath, '..', 'target', 'debug', binName);
    if (fs.existsSync(devPath)) {
      return { command: devPath, args: [] };
    }
  }

  // Fall back to PATH lookup.
  if (canExecute(binName)) {
    return { command: binName, args: [] };
  }

  return null;
}

function deactivate() {
  if (!client) {
    return undefined;
  }
  return client.stop();
}

module.exports = {
  activate,
  deactivate
};

'use strict';

const fs = require('fs');
const path = require('path');
const cp = require('child_process');
const vscode = require('vscode');
const languageClient = require('vscode-languageclient/node');
const { LanguageClient } = languageClient;

let client;

function activate(context) {
  const outputChannel = vscode.window.createOutputChannel('Rex Language Server');
  const traceChannel = vscode.window.createOutputChannel('Rex Language Server Trace');

  const resolved = resolveServerCommand(context);
  if (!resolved) {
    outputChannel.appendLine('[rex] rex-lsp not found; syntax highlighting enabled, LSP disabled');
    vscode.window.showWarningMessage(
      'Rex: language server (rex-lsp) not found. Syntax highlighting works, but LSP features are disabled. ' +
        'Install rex-lsp (e.g. `cargo install --path rex-lsp`) or set `rex.serverPath`.'
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
  } catch (err) {
    outputChannel.appendLine(`[rex] failed to start language client: ${err}`);
    vscode.window.showErrorMessage(
      'Rex Language Server failed to start. See Output → "Rex Language Server" for details.'
    );
  }
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

  const binName = process.platform === 'win32' ? 'rex-lsp.exe' : 'rex-lsp';

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

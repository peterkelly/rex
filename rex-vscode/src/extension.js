'use strict';

const fs = require('fs');
const path = require('path');
const vscode = require('vscode');
const languageClient = require('vscode-languageclient/node');
const { LanguageClient } = languageClient;

let client;

function activate(context) {
  const outputChannel = vscode.window.createOutputChannel('Rex Language Server');
  const traceChannel = vscode.window.createOutputChannel('Rex Language Server Trace');

  const serverPath = resolveServerPath(context);
  outputChannel.appendLine(`[rex] serverPath = ${serverPath}`);

  const serverOptions = {
    command: serverPath,
    args: []
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

function resolveServerPath(context) {
  const config = vscode.workspace.getConfiguration('rex');
  const configured = config.get('serverPath');
  if (configured && configured.trim()) {
    if (!fs.existsSync(configured)) {
      vscode.window.showErrorMessage(
        `rex.serverPath does not exist: ${configured}`
      );
    }
    return configured;
  }

  const binName = process.platform === 'win32' ? 'rex-lsp.exe' : 'rex-lsp';
  const localPath = path.join(context.extensionPath, '..', 'target', 'debug', binName);
  if (fs.existsSync(localPath)) {
    return localPath;
  }

  return binName;
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

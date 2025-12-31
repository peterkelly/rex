'use strict';

const fs = require('fs');
const path = require('path');
const vscode = require('vscode');
const { LanguageClient } = require('vscode-languageclient/node');

let client;

function activate(context) {
  const serverPath = resolveServerPath(context);
  const serverOptions = {
    command: serverPath,
    args: []
  };

  const clientOptions = {
    documentSelector: [{ scheme: 'file', language: 'rex' }],
    synchronize: {
      fileEvents: vscode.workspace.createFileSystemWatcher('**/*.rex')
    }
  };

  client = new LanguageClient(
    'rexLanguageServer',
    'Rex Language Server',
    serverOptions,
    clientOptions
  );

  context.subscriptions.push(client.start());
}

function resolveServerPath(context) {
  const config = vscode.workspace.getConfiguration('rex');
  const configured = config.get('serverPath');
  if (configured && configured.trim()) {
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

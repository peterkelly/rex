'use strict';

const {
  createConnection,
  TextDocuments,
  ProposedFeatures,
  DiagnosticSeverity,
  MarkupKind
} = require('vscode-languageserver/node');
const { TextDocument } = require('vscode-languageserver-textdocument');

const connection = createConnection(ProposedFeatures.all);
const documents = new TextDocuments(TextDocument);
const maxDiagnostics = 50;

const keywordDocs = new Map([
  ['declare', 'Declares a forward signature for an externally-provided function.'],
  ['import', 'Imports a module into scope (e.g. `import foo.bar as Foo`).'],
  ['pub', 'Marks a top-level item as exported from its module.'],
  ['let', 'Introduces local bindings.'],
  ['in', 'Begins the expression body for a let binding.'],
  ['type', 'Declares a type or ADT.'],
  ['match', 'Starts a pattern match expression.'],
  ['when', 'Introduces a match arm.'],
  ['if', 'Conditional expression keyword.'],
  ['then', 'Conditional expression branch.'],
  ['else', 'Fallback branch of a conditional expression.'],
  ['as', 'Type ascription or aliasing keyword.'],
  ['for', 'List/dict comprehension keyword (when supported).'],
  ['is', 'Type assertion keyword.']
]);

const typeDocs = new Map([
  ['bool', 'Boolean type.'],
  ['string', 'UTF-8 string type.'],
  ['uuid', 'UUID type.'],
  ['datetime', 'Datetime type.'],
  ['u8', 'Unsigned 8-bit integer.'],
  ['u16', 'Unsigned 16-bit integer.'],
  ['u32', 'Unsigned 32-bit integer.'],
  ['u64', 'Unsigned 64-bit integer.'],
  ['i8', 'Signed 8-bit integer.'],
  ['i16', 'Signed 16-bit integer.'],
  ['i32', 'Signed 32-bit integer.'],
  ['i64', 'Signed 64-bit integer.'],
  ['f32', '32-bit float.'],
  ['f64', '64-bit float.'],
  ['List', 'List type constructor.'],
  ['Dict', 'Dictionary type constructor.'],
  ['Array', 'Array type constructor.'],
  ['Option', 'Optional type constructor.'],
  ['Result', 'Result type constructor.']
]);

const valueDocs = new Map([
  ['true', 'Boolean literal.'],
  ['false', 'Boolean literal.'],
  ['null', 'Null literal.'],
  ['Some', 'Option constructor.'],
  ['None', 'Option empty constructor.'],
  ['Ok', 'Result success constructor.'],
  ['Err', 'Result error constructor.']
]);

connection.onInitialize(() => {
  return {
    capabilities: {
      textDocumentSync: documents.syncKind,
      hoverProvider: true
    },
    serverInfo: {
      name: 'rex-lsp',
      version: '0.0.1'
    }
  };
});

connection.onHover((params) => {
  const document = documents.get(params.textDocument.uri);
  if (!document) {
    return null;
  }

  const word = getWordAtPosition(document, params.position);
  if (!word) {
    return null;
  }

  if (keywordDocs.has(word)) {
    return {
      contents: {
        kind: MarkupKind.Markdown,
        value: `**${word}** keyword\n\n${keywordDocs.get(word)}`
      }
    };
  }

  if (typeDocs.has(word)) {
    return {
      contents: {
        kind: MarkupKind.Markdown,
        value: `**${word}** type\n\n${typeDocs.get(word)}`
      }
    };
  }

  if (valueDocs.has(word)) {
    return {
      contents: {
        kind: MarkupKind.Markdown,
        value: `**${word}** value\n\n${valueDocs.get(word)}`
      }
    };
  }

  return null;
});

documents.onDidChangeContent((change) => {
  validateTextDocument(change.document);
});

documents.onDidClose((event) => {
  connection.sendDiagnostics({ uri: event.document.uri, diagnostics: [] });
});

function validateTextDocument(textDocument) {
  const text = textDocument.getText();
  const diagnostics = [];
  const stack = [];

  for (let i = 0; i < text.length - 1; i += 1) {
    const current = text[i];
    const next = text[i + 1];

    if (current === '{' && next === '-') {
      stack.push(i);
      i += 1;
      continue;
    }

    if (current === '-' && next === '}') {
      if (stack.length === 0) {
        diagnostics.push({
          severity: DiagnosticSeverity.Error,
          range: rangeFor(textDocument, i, i + 2),
          message: 'Unmatched block comment closer (-}).',
          source: 'rex-lsp'
        });
        if (diagnostics.length >= maxDiagnostics) {
          break;
        }
      } else {
        stack.pop();
      }
      i += 1;
    }
  }

  for (const start of stack) {
    diagnostics.push({
      severity: DiagnosticSeverity.Error,
      range: rangeFor(textDocument, start, start + 2),
      message: 'Unclosed block comment opener ({-).',
      source: 'rex-lsp'
    });
    if (diagnostics.length >= maxDiagnostics) {
      break;
    }
  }

  connection.sendDiagnostics({ uri: textDocument.uri, diagnostics });
}

function rangeFor(textDocument, startOffset, endOffset) {
  return {
    start: textDocument.positionAt(startOffset),
    end: textDocument.positionAt(endOffset)
  };
}

function getWordAtPosition(textDocument, position) {
  const text = textDocument.getText();
  const offset = textDocument.offsetAt(position);
  if (offset < 0 || offset >= text.length) {
    return null;
  }

  if (!isWordChar(text[offset])) {
    return null;
  }

  let start = offset;
  let end = offset + 1;

  while (start > 0 && isWordChar(text[start - 1])) {
    start -= 1;
  }

  while (end < text.length && isWordChar(text[end])) {
    end += 1;
  }

  return text.slice(start, end);
}

function isWordChar(ch) {
  return /[A-Za-z0-9_]/.test(ch);
}

documents.listen(connection);
connection.listen();

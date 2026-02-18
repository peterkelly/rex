'use strict';

function functionArityFromType(typeSignature) {
  if (!typeSignature || typeof typeSignature !== 'string') {
    return 0;
  }
  const normalized = normalizeTypeForArity(typeSignature);
  const parts = splitTopLevelArrows(normalized);
  return Math.max(parts.length - 1, 0);
}

function normalizeTypeForArity(typeSignature) {
  let out = stripTopLevelWhereClause(typeSignature.trim());
  out = stripOuterParens(out);
  return out;
}

function stripTopLevelWhereClause(typeSignature) {
  let depthParen = 0;
  let depthBrace = 0;
  let depthBracket = 0;

  for (let i = 0; i < typeSignature.length; i += 1) {
    const ch = typeSignature[i];
    if (ch === '(') {
      depthParen += 1;
      continue;
    }
    if (ch === ')') {
      depthParen = Math.max(depthParen - 1, 0);
      continue;
    }
    if (ch === '{') {
      depthBrace += 1;
      continue;
    }
    if (ch === '}') {
      depthBrace = Math.max(depthBrace - 1, 0);
      continue;
    }
    if (ch === '[') {
      depthBracket += 1;
      continue;
    }
    if (ch === ']') {
      depthBracket = Math.max(depthBracket - 1, 0);
      continue;
    }
    if (depthParen !== 0 || depthBrace !== 0 || depthBracket !== 0) {
      continue;
    }
    if (startsWithWord(typeSignature, i, 'where')) {
      return typeSignature.slice(0, i).trim();
    }
  }

  return typeSignature;
}

function startsWithWord(text, index, word) {
  if (index < 0 || index + word.length > text.length) {
    return false;
  }
  const segment = text.slice(index, index + word.length);
  if (segment !== word) {
    return false;
  }
  const before = index === 0 ? ' ' : text[index - 1];
  const after = index + word.length >= text.length ? ' ' : text[index + word.length];
  const isWordChar = (ch) => /[A-Za-z0-9_]/.test(ch);
  return !isWordChar(before) && !isWordChar(after);
}

function stripOuterParens(text) {
  let out = text.trim();
  while (out.startsWith('(') && out.endsWith(')')) {
    let depth = 0;
    let wrapsWhole = true;
    for (let i = 0; i < out.length; i += 1) {
      const ch = out[i];
      if (ch === '(') {
        depth += 1;
      } else if (ch === ')') {
        depth -= 1;
      }
      if (depth === 0 && i < out.length - 1) {
        wrapsWhole = false;
        break;
      }
    }
    if (!wrapsWhole || depth !== 0) {
      break;
    }
    out = out.slice(1, -1).trim();
  }
  return out;
}

function splitTopLevelArrows(typeSignature) {
  const parts = [];
  let depthParen = 0;
  let depthBrace = 0;
  let depthBracket = 0;
  let start = 0;

  for (let i = 0; i < typeSignature.length; i += 1) {
    const ch = typeSignature[i];
    if (ch === '(') {
      depthParen += 1;
      continue;
    }
    if (ch === ')') {
      depthParen = Math.max(depthParen - 1, 0);
      continue;
    }
    if (ch === '{') {
      depthBrace += 1;
      continue;
    }
    if (ch === '}') {
      depthBrace = Math.max(depthBrace - 1, 0);
      continue;
    }
    if (ch === '[') {
      depthBracket += 1;
      continue;
    }
    if (ch === ']') {
      depthBracket = Math.max(depthBracket - 1, 0);
      continue;
    }
    if (
      ch === '-' &&
      i + 1 < typeSignature.length &&
      typeSignature[i + 1] === '>' &&
      depthParen === 0 &&
      depthBrace === 0 &&
      depthBracket === 0
    ) {
      parts.push(typeSignature.slice(start, i).trim());
      start = i + 2;
      i += 1;
    }
  }

  parts.push(typeSignature.slice(start).trim());
  return parts.filter(Boolean);
}

module.exports = {
  functionArityFromType,
  splitTopLevelArrows,
  stripTopLevelWhereClause,
  stripOuterParens
};

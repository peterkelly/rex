'use strict';

const { functionArityFromType } = require('./arity');

function buildFunctionCallTemplate(name, typeSignature, options = {}) {
  const arity = functionArityFromType(typeSignature);
  let text = `${name}`;
  for (let i = 1; i <= arity; i += 1) {
    text += ` \${${i}:arg${i}}`;
  }
  if (options.includeTypeComment && typeSignature) {
    text += ` {- ${name} : ${typeSignature} -}`;
  }
  return text;
}

module.exports = {
  buildFunctionCallTemplate
};

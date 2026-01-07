from __future__ import annotations

import re

from pygments.lexer import RegexLexer, bygroups
from pygments.token import Comment, Keyword, Name, Number, Operator, Punctuation, String, Text, Whitespace


class RexLexer(RegexLexer):
    name = "Rex"
    aliases = ["rex"]
    filenames = ["*.rex"]

    _identifier = r"[A-Za-z_][A-Za-z0-9_]*"
    _operator = r"[+\-*/%=&|<>!:.^~]+"

    tokens = {
        "root": [
            (r"\s+", Whitespace),
            (r"--[^\n]*", Comment.Single),
            (r"\{\-", Comment.Multiline, "comment"),
            (r'"', String.Double, "string"),
            (r"\b(true|false)\b", Keyword.Constant),
            (r"\b\d+\.\d+\b", Number.Float),
            (r"\b\d+\b", Number.Integer),
            (
                r"\b(let|in|if|then|else|fn|type|class|instance|where)\b",
                Keyword,
            ),
            (r"\b(Default|Eq|Ord|Show|Functor|Applicative|Monad)\b", Name.Builtin),
            (r"(λ|\\\\)", Operator),
            (r"(→|->)", Operator),
            (r"\b[A-Z][A-Za-z0-9_]*\b", Name.Class),
            (rf"\b{_identifier}\b", Name),
            (rf"{_operator}", Operator),
            (r"[][(){}.,:;]", Punctuation),
            (r".", Text),
        ],
        "comment": [
            (r"[^\-\}]+", Comment.Multiline),
            (r"\-\}", Comment.Multiline, "#pop"),
            (r"\{\-", Comment.Multiline, "#push"),
            (r"[-}]", Comment.Multiline),
        ],
        "string": [
            (r'[^"\\]+', String.Double),
            (r"\\[\\\"nrt]", String.Escape),
            (r"\\x[0-9A-Fa-f]{2}", String.Escape),
            (r"\\u\{[0-9A-Fa-f]{1,6}\}", String.Escape),
            (r"\\.", String.Escape),
            (r'"', String.Double, "#pop"),
        ],
    }


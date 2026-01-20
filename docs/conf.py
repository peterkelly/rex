from __future__ import annotations

import os
import sys

project = "Rex"
author = "Rex contributors"

sys.path.insert(0, os.path.abspath("."))

extensions = [
    "myst_parser",
]

templates_path = ["_templates"]
exclude_patterns = ["_build", "Thumbs.db", ".DS_Store"]

root_doc = "index"

myst_heading_anchors = 3
myst_enable_extensions = [
    "colon_fence",
    "deflist",
    "fieldlist",
    "strikethrough",
    "tasklist",
]

html_theme = "shibuya"
html_static_path = ["_static"]

# Keep the Rex docs in Markdown without forcing hard line-wrapping.
myst_word_wrap = False

# Custom syntax highlighting for fenced code blocks like ```rex
from _pygments.rex_lexer import RexLexer  # noqa: E402

from sphinx.highlighting import lexers  # noqa: E402

lexers["rex"] = RexLexer()

# Rex Lexer (`rex-lexer`)

This crate tokenizes Rex source into a stream of `Token`s with precise `Span` information.

## Entry point

```rust
use rex_lexer::Token;

let tokens = Token::tokenize("let x = 1 in x").unwrap();
```

## Spans

`rex_lexer::span` provides `Position` and `Span` types used throughout the workspace for diagnostics
and editor tooling.


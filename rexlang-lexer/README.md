# Rex Lexer (`rexlang-lexer`)

This crate tokenizes Rex source into a stream of `Token`s with precise `Span` information.

## Entry point

```rust
use rex_lexer::Token;

fn main() -> Result<(), rex_lexer::LexicalError> {
    let tokens = Token::tokenize("let x = 1 in x")?;
    let _ = tokens;
    Ok(())
}
```

## Spans

`rex_lexer::span` provides `Position` and `Span` types used throughout the workspace for diagnostics
and editor tooling.

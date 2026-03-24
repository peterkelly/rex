# Rex Parser (`rexlang-parser`)

This crate parses token streams into the Rex AST (`rex-ast`), producing a `Program { decls, expr }`
or a list of parse errors with spans.

## Usage

```rust
use rex_lexer::Token;
use rex_parser::Parser;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let tokens = Token::tokenize("1 + 2")?;
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program(&mut GasMeter::default()).map_err(|errs| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, format!("parse error: {errs:?}"))
    })?;
    let _ = program;
    Ok(())
}
```

## Limits and metering

- `ParserLimits`: controls syntactic nesting limits
- `parse_program`: optional gas metering via `rexlang-util`

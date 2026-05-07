# Rex Parser (`rex-parser`)

This crate parses Rex source into the Rex AST (`rex-ast`), producing a `CompilationUnit { decls, body }`
or a list of parse errors with spans.

## Usage

```rust
use rex_parser::parse;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let program = parse("1 + 2").map_err(|errs| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, format!("parse error: {errs:?}"))
    })?;
    let _ = program;
    Ok(())
}
```

## Limits

- `ParserLimits`: controls syntactic nesting limits
- `parse`: parses a complete Rex program from source text
- `parse_with_limits`: parses with explicit syntactic nesting limits

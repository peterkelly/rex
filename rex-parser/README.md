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

- `parse`: parses a complete Rex program from source text
- Parsing enforces a fixed maximum AST nesting depth.

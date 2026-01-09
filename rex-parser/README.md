# Rex Parser (`rex-parser`)

This crate parses token streams into the Rex AST (`rex-ast`), producing a `Program { decls, expr }`
or a list of parse errors with spans.

## Usage

```rust
use rex_lexer::Token;
use rex_parser::Parser;

let tokens = Token::tokenize("1 + 2").unwrap();
let mut parser = Parser::new(tokens);
let program = parser.parse_program().unwrap();
```

## Limits and metering

- `ParserLimits`: controls syntactic nesting limits
- `parse_program_with_gas`: optional gas metering via `rex-gas`


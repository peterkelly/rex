# Debugging: CLI Tips and Common Errors

Rex is prepared and evaluated in stages:

1. Parsing
2. Import/declaration preparation
3. Type inference / checking
4. Evaluation

Most debugging is about figuring out *which stage* is failing and adding just enough information
to make the problem obvious.

## Useful CLI flags

Run a Rex file:

```sh
cargo run -p rex-cli --bin rex_cli -- path/to/file.rex
```

Run a file with JSON inputs for its entry point:

```sh
cargo run -p rex-cli --bin rex_cli -- path/to/program.rex --inputs path/to/inputs.json
```

The inputs file is a top-level JSON object. Each field name must match a
parameter of `main`; runnable files without `main` use their final expression
and have the empty input shape `{}`.

Inspect the entry point type metadata:

```sh
cargo run -p rex-cli --bin rex_cli -- path/to/program.rex --manifest
```

Run an inline snippet:

```sh
cargo run -p rex-cli --bin rex_cli -- -c 'let x = 1 in x + 2'
```

Show the parsed AST and exit:

```sh
cargo run -p rex-cli --bin rex_cli -- --emit-ast -c '1 + 2'
```

Show the entry point result type and exit:

```sh
cargo run -p rex-cli --bin rex_cli -- --emit-type -c 'map ((*) 2) [1, 2, 3]'
```

Print a string result without JSON quotes:

```sh
cargo run -p rex-cli --bin rex_cli -- --raw-output -c '"hello"'
```

## “Parse error”: start small

If you hit a parse error:

1. Reduce the program to the smallest failing snippet.
2. Add parentheses to disambiguate application vs infix operators.
3. Prefer multi-line `let`/`match` while debugging.

## “Missing typeclass impl”

This usually means you called a type-class method at a type that has no instance.

Typical fixes:

- use a different type (`List` vs `Option`, `Option` vs `Result`, …),
- add an instance (Section 2),
- add a type annotation so the intended instance is selected.

## “Ambiguous overload”

This happens when an overloaded *value* doesn’t have enough information to pick an instance.

Typical fixes:

- add a let-annotation: `let z: i32 = zero in z`
- add `is` ascription: `(zero) is i32` (if you prefer expression ascription style)
- use the value in a context that forces a type (e.g. `zero + 1`).

The exact defaulting rules are described in [Specification](../../SPEC.md).

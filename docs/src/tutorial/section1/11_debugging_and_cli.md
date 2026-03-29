# Debugging: CLI Tips and Common Errors

Rex is compiled and evaluated in stages:

1. Lexing
2. Parsing
3. Type inference / checking
4. Evaluation

Most debugging is about figuring out *which stage* is failing and adding just enough information
to make the problem obvious.

## Useful CLI flags

Run a file:

```sh
cargo run -p rexlang-cli -- run path/to/file.rex
```

Run an inline snippet:

```sh
cargo run -p rexlang-cli -- run -c 'let x = 1 in x + 2'
```

Show the parsed AST and exit:

```sh
cargo run -p rexlang-cli -- run --emit-ast -c '1 + 2'
```

Show the inferred type and exit:

```sh
cargo run -p rexlang-cli -- run --emit-type -c 'map ((*) 2) [1, 2, 3]'
```

## “Parse error”: start small

If you hit a parse error:

1. Reduce the program to the smallest failing snippet.
2. Add parentheses to disambiguate application vs infix operators.
3. Prefer layout-style `let`/`match` while debugging.

## “Missing typeclass impl”

This usually means you called a type-class method at a type that has no instance.

Typical fixes:

- use a different type (`List` vs `Array`, `Option` vs `Result`, …),
- add an instance (Section 2),
- add a type annotation so the intended instance is selected.

## “Ambiguous overload”

This happens when an overloaded *value* doesn’t have enough information to pick an instance.

Typical fixes:

- add a let-annotation: `let z: i32 = zero in z`
- add `is` ascription: `(zero) is i32` (if you prefer expression ascription style)
- use the value in a context that forces a type (e.g. `zero + 1`).

The exact defaulting rules are described in [Specification](../../SPEC.md).

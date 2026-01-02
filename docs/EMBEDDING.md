# Embedding Rex in Rust

Rex is designed as a small pipeline you can embed at whatever stage you need:

1. `rex-lexer`: source → `Tokens`
2. `rex-parser`: tokens → `Program { decls, expr }`
3. `rex-ts`: HM inference + type classes → `TypedExpr` (plus predicates/type)
4. `rex-engine`: evaluate a `TypedExpr` → `rex_engine::Value`

This document focuses on common embedding patterns.

## Evaluate Rex Code

```rust
use rex_engine::Engine;
use rex_lexer::Token;
use rex_parser::Parser;

let tokens = Token::tokenize("let x = 1 + 2 in x * 3")?;
let mut parser = Parser::new(tokens);
let program = parser.parse_program().map_err(|errs| format!("{errs:?}"))?;

let mut engine = Engine::with_prelude();
engine.inject_decls(&program.decls)?;
let value = engine.eval(program.expr.as_ref())?;
println!("{value}");
```

## Typecheck Without Evaluating

```rust
use rex_lexer::Token;
use rex_parser::Parser;
use rex_ts::TypeSystem;

let tokens = Token::tokenize("map (\\x -> x) [1, 2, 3]")?;
let mut parser = Parser::new(tokens);
let program = parser.parse_program().map_err(|errs| format!("{errs:?}"))?;

let mut ts = TypeSystem::with_prelude();
for decl in &program.decls {
    match decl {
        rex_ast::expr::Decl::Type(d) => ts.inject_type_decl(d)?,
        rex_ast::expr::Decl::Class(d) => ts.inject_class_decl(d)?,
        rex_ast::expr::Decl::Instance(d) => {
            ts.inject_instance_decl(d)?;
        }
        rex_ast::expr::Decl::Fn(d) => ts.inject_fn_decl(d)?,
    }
}

let (preds, ty) = ts.infer(program.expr.as_ref())?;
println!("type: {ty}");
if !preds.is_empty() {
    println!("constraints: {}", preds.iter().map(|p| format!(\"{} {}\", p.class, p.typ)).collect::<Vec<_>>().join(\", \"));
}
```

## Inject Native Values and Functions

`rex-engine` is the boundary where Rust provides implementations for Rex values.

```rust
use rex_engine::Engine;

let mut engine = Engine::with_prelude();
engine.inject_value("answer", 42i32)?;
engine.inject_fn1("inc", |x: i32| x + 1)?;
```

## Bridge Rust Types with `#[derive(Rex)]`

The derive:
- declares an ADT in the Rex type system
- injects runtime constructors (so Rex can *build* values)
- implements `FromValue`/`IntoValue` for converting Rust ↔ Rex

```rust
use rex_engine::{Engine, FromValue};
use rex_proc_macro::Rex;

#[derive(Rex, Debug, PartialEq)]
enum Maybe<T> {
    Just(T),
    Nothing,
}

let mut engine = Engine::with_prelude();
Maybe::<i32>::inject_rex(&mut engine)?;

let v = engine.eval(
    rex_parser::Parser::new(rex_lexer::Token::tokenize("Just 1")?)
        .parse_program()
        .unwrap()
        .expr
        .as_ref()
)?;
assert_eq!(Maybe::<i32>::from_value(&v, "v")?, Maybe::Just(1));
```

## Stack Size Entry Points

Some workloads (very deep nesting) can overflow the default thread stack. The project exposes
“large stack” entry points:

- `rex_parser::Parser::parse_program_with_stack_size`
- `rex_ts::TypeSystem::infer_with_stack_size`
- `rex_engine::Engine::eval_with_stack_size`


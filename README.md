# 🦖

Rex (short for Rush Expressions) is a strongly-typed domain-specific functional
programming language for defining complex workflows in the Rush platform.

It comes with a set of built-in functions (like `map`, `fold`, `zip` etc) but
all other functions are defined and implemented by the host VM. These host
functions are typically high-performance computing modules that are intended to
be dispatched to supercomputers.

Right now, all of that is managed by a proprietary VM implementation (known as
`tengu`), but we will slowly be moving more and more of that logic into Rex
itself (mostly so that local dev is easier).

## Example

```rex
-- Double everything in a list
map (λx → 2 * x) [1, 2, 3, 4]

-- Or, if you prefer currying
map ((*) 2) [1, 2, 3, 4]
```

## Crates

This repo is a Cargo workspace. The key crates are:

- `rex-lexer`: tokenization (+ spans)
- `rex-parser`: parser producing a `Program { decls, expr }`
- `rex-ts`: Hindley–Milner type inference + type classes + ADTs
- `rex-engine`: runtime evaluator + native-function injection, backed by `rex-ts`
- `rex-proc-macro`: `#[derive(Rex)]` for bridging Rust types ↔ Rex ADTs/values
- `rex`: CLI binary (`cargo run -p rex -- ...`)
- `rex-fuzz`: stdin-driven fuzz harness binaries
- `rex-util`: small shared helpers (e.g. module hashing, bundled stdlib sources)
- `rex-lsp` / `rex-vscode`: language tooling (LSP + VS Code extension)

## Docs

- [`docs/src/ARCHITECTURE.md`](docs/src/ARCHITECTURE.md): crate pipeline and design notes
- [`docs/src/EMBEDDING.md`](docs/src/EMBEDDING.md): embedding Rex in Rust (API patterns)
- [`docs/src/LANGUAGE.md`](docs/src/LANGUAGE.md): language notes and examples
- [`docs/src/SPEC.md`](docs/src/SPEC.md): locked semantics (record update, coherence, defaulting)
- [`docs/src/CONTRIBUTING.md`](docs/src/CONTRIBUTING.md): contributor workflow and repo policies
- Production note (untrusted code): [`docs/src/EMBEDDING.md`](docs/src/EMBEDDING.md) (“Running Untrusted Rex Code”)

Build the HTML docs (Sphinx + Shibuya):

```sh
python3 -m venv .venv
. .venv/bin/activate
pip install -r docs/requirements.txt
make -C docs html
open docs/_build/html/index.html
```

## CLI

Run a file:

```sh
cargo run -p rex -- run rex/examples/record_update.rex
```

Run the advanced module import example:

```sh
cargo run -p rex -- run rex/examples/modules_advanced/main.rex
```

Run inline code:

```sh
cargo run -p rex -- run -c 'map ((*) 2) [1, 2, 3]'
```

Other useful flags:

- `--emit-ast`: print parsed AST as JSON and exit
- `--emit-type` (alias: `--type`): print inferred type as JSON and exit
- `--stdin`: read a program from stdin
- `--stack-size-mb`: control the runner thread stack size
- `--max-nesting`: cap syntactic nesting depth during parsing
- `--no-max-nesting`: disable the parsing nesting cap
- `--gas`: total gas budget for parse/type/eval
- `--no-gas`: disable gas metering

## Embedding (Rust)

### Parse + Eval

```rust
use rex_engine::Engine;
use rex_lexer::Token;
use rex_parser::Parser;
use rex_util::{GasCosts, GasMeter};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let tokens = Token::tokenize(r#"let x = 1 + 2 in x * 3"#)?;
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program(&mut GasMeter::default()).map_err(|errs| format!("{errs:?}"))?;

    let mut engine = Engine::with_prelude(())?;
    engine.inject_decls(&program.decls)?;
    let mut gas = GasMeter::default();
    let value = engine
        .eval_with_gas(program.expr.as_ref(), &mut gas)
        .await?;
    println!("{value}");
    Ok(())
}
```

`Engine` is generic over host state (`Engine<State>`, where `State: Clone + Sync + 'static`).
Use `Engine::with_prelude(())?` when you do not need host state, or pass your own state value
to `Engine::new(state)` / `Engine::with_prelude(state)` and access it in injected natives via
the first callback parameter (`&State`) for `export` / `export_async`.

For host-provided module namespaces, prefer `Module` + `inject_module`:

```rust
use rex_engine::{Engine, Module};

let mut engine = Engine::with_prelude(())?;
engine.add_default_resolvers();

let mut math = Module::new("acme.math");
math.export("inc", |_state: &(), x: i32| -> i32 { x + 1 })?;
math.export_async("double_async", |_state: &(), x: i32| async move { x * 2 })?;
engine.inject_module(math)?;
```

For runtime-defined module signatures/implementations, use
`export_native` / `export_native_async` with an explicit `Scheme` + arity. Those
callbacks receive `&Engine<State>` so they can access `engine.state` and allocate on
`engine.heap()`.

For deeply nested programs, prefer the large-stack entrypoints:

- `rex_parser::Parser::parse_program_with_stack_size`
- `rex_ts::TypeSystem::infer_with_stack_size`

### Type Inference

```rust
use rex_lexer::Token;
use rex_parser::Parser;
use rex_ts::TypeSystem;

let tokens = Token::tokenize("map (\\x -> x) [1, 2, 3]")?;
let program = Parser::new(tokens)
    .parse_program_with_stack_size(rex_parser::DEFAULT_STACK_SIZE_BYTES)
    .map_err(|errs| format!("{errs:?}"))?;

let mut ts = TypeSystem::with_prelude()?;
// If you parsed type/class/instance/function decls, inject them before inference:
// for decl in &program.decls { ... }
let (_preds, ty) = ts.infer_with_stack_size(program.expr.as_ref(), rex_ts::DEFAULT_STACK_SIZE_BYTES)?;
println!("{ty}");
```

### Rust Types as Rex Types (`#[derive(Rex)]`)

Derive support lives in `rex-proc-macro`. The derive generates:

- an ADT declaration (`T::rex_adt_decl` returning `Result<AdtDecl, EngineError>`) + injection helper (`T::inject_rex`)
- `IntoValue` and `FromValue` to convert between Rust values and `rex_engine::Value`

```rust
use rex_engine::{Engine, FromValue};
use rex_proc_macro::Rex;
use rex_util::{GasCosts, GasMeter};

#[derive(Rex, Debug, PartialEq)]
struct Point {
    x: i32,
    y: i32,
}

let mut engine = Engine::with_prelude(())?;
Point::inject_rex(&mut engine)?;

let tokens = rex_lexer::Token::tokenize("Point { x = 1, y = 2 }")?;
let program = rex_parser::Parser::new(tokens)
    .parse_program(&mut GasMeter::default())
    .map_err(|errs| format!("{errs:?}"))?;
let mut gas = GasMeter::default();
let value = engine
    .eval_with_gas(program.expr.as_ref(), &mut gas)
    .await?;
let point = Point::from_value(&value, "point")?;
assert_eq!(point, Point { x: 1, y: 2 });
```

## Syntax Reference

- Programs are a single expression; whitespace (including newlines) is ignored outside of strings and comments. Block comments use `{- ... -}` and are stripped before parsing.
- Identifiers start with `_` or a letter and can contain numbers and underscores. Operators such as `+`, `-`, `*`, `/`, `==`, `<`, `>`, `++`, `&&`, `||`, and `.` are parsed as infix functions, so `(+) 2` and `(-) 4` work for partial application.
- Function application is left-associative: `f x y` is parsed as `(f x) y`. Parentheses control grouping when mixing applications and infix operators.
- Operator precedence (high to low): `.` > `* / %` > `+ - ++` > `== != < <= > >=` > `&&` > `||`.
- Core expression forms:
  - Literals: booleans, integers, floats, strings, UUIDs, datetimes.
  - Collections: tuples `(e1, e2)`, lists `[e1, e2]`, dictionaries `{ key = value }`.
  - Functions and control flow: lambdas `\x y -> body` (also accepts `λ` and `→`), let-in bindings `let x = e1, y = e2 in body`, and conditionals `if cond then a else b`.
- Top-level module imports:
  - `import foo.bar as Bar` (qualified access like `Bar.add`)
  - `import foo.bar (*)` (import all exported values)
  - `import foo.bar (x, y as z)` (import selected exports, with optional rename)
  - Local import paths resolve relative to the importing file; leading `super` segments walk up directories.

## Type Classes

Rex supports Haskell-style type classes with `class` and `instance`. Superclasses and instance contexts use `<=` (read it as “requires”).

```rex
class Default a
    default : a

class Ord a <= Eq a
    cmp : a -> a -> i32

instance Default (List a) <= Default a
    default = []
```

The `where` keyword is optional. A class/instance method block is recognized by layout:
- It must be indented more than the `class`/`instance` header.
- Class methods start with `name : ...` and instance methods start with `name = ...` (operator names are allowed for `name`).

Class methods are values: using `default` produces a `Default T` constraint, and evaluation resolves the right instance based on the inferred type `T`.

Prelude type classes and instances (including the methods for numeric operators and comparisons) live in `rex-ts/src/prelude_typeclasses.rex` and are injected by `TypeSystem::with_prelude()?`.

The prelude instances typically point at Rust-backed intrinsics with the `prim_` prefix (for example `prim_add`, `prim_zero`, `prim_eq`). Think of these as the Rex equivalent of GHC primops: the surface language stays “single source” (classes + instances are Rex), but the lowest-level implementations are still provided by the host.

## Let-in

We can assign variable names to expressions, allowing them to be re-used multiple times. This can help to simplify our Rex code. To create a variable, we need to use a let-in expression.

```rex
let 
    x = 1 + 2,
    y = 3
in
    x * y
```

It is important to note that variables are only accessible inside the let-in expression in which they are created. For example, the following code is not valid, and attempting to execute it will result in an error:

```rex
(let x = 1 + 2 in x * 3) * x
```

## Tuples, Lists, and Dictionaries

Rex supports the following collection types: tuples, lists, and dictionaries. Tuple are collections where each element can be a different type. Lists are collections where every element must be the same type. Dictionaries are collections that map an explicit name to an element, where each element can be a different type (you can think of them like "named tuples").

### Tuples

We create tuples using parentheses:

```rex
("this is a tuple", 420, true)
```

We can also use numeric projection to get specific elements from the tuple. For example:

```rex
let
    tuple = ("this is a ", 420, true)
in
    tuple.0 ++ "tuple"
```

will result in the value `"this is a tuple"` (we are using the `++` concatentation operator, which works on strings and lists).

### Lists

We create lists using brackets:

```rex
["this", "is", "a", "list", "of", "strings" ]
```

Lists can also be constructed explicitly using the prelude constructors:

```rex
Cons "this" (Cons "is" (Cons "a" Empty))
```

For lists, we can use the `get` function to get specific elements:

```rex
let
    list = ["this", "is", "a", "list", "of", "strings"]
in
    (get 0 list) ++ " " ++ (get 1 list) ++ "a string"
```

We can also use the `take` function to take a sub-list from the front of the list. For example:

```rex
let
    list = ["this", "is", "a", "list", "of", "strings"]
in
    take 3 list
```

will return `["this", "is", "a"]`. We can combine this with `skip` to take sub-lists from deeper in the list. For example:

```rex
let
    list = ["this", "is", "a", "list", "of", "strings"]
in
    take 2 (skip 2 list)
```

will return `["a", "list"]`.

### Dictionaries

We create dictionaries using braces:

```rex
{ key1 = "value1", key2 = 420, key3 = true }
```

## Lambda Functions

Rex allows you to define your own functions (also known as lambdas). These lambdas can accept any number of variables, and define an expression applied to those variables. You define a lambda by writing the `\` or `λ` characters, naming your variables, writing the `->` or `→` characters, and then writing the body of the lambda. Let's see an example:

```rex
(λ x y → x + y) 2 3
```

This defines a lambda that accepts 2 variables, `x` and `y`, that, when called, will add them together. We then immediately call this lambda using the values `2` and `3`. We can mix lambdas with let-in expressions to name our lambdas:

```rex
let
    quad_eq_pos = λ a b c →   (sqrt (b * b + 4 * a * c) - b) / (2 * a),
    quad_eq_neg = λ a b c → - (sqrt (b * b + 4 * a * c) + b) / (2 * a),
    a = 1,
    b = 0,
    c = -1
in
    (quad_eq_pos a b c, quad_eq_neg a b c)
```

This expression produces the solutions for the quadratic equation `x^2 - 1`.

## If-then-else

Sometimes you want to execute different code depending on a condition. This is done using the if-then-else construct. Consider the following expression:

```rex
λ x → if x >= 0 then "positive" else "negative"
```

This expression defines a lambda function that takes a number x as input and returns "positive" if x is greater than or equal to 0, and "negative" otherwise.

## Mapping

In most purely functional programming languages, mapping is a important technique for applying a function to every element in a list. Rex includes a built-in `map` function for doing this. Let's see it in action:

```rex
map (λ x → 2 * x) [0.5, 1.0, 1.5]
```

Running this expressions should return the list `[1.0, 2.0, 3.0]`. What's going on? Well, the first argument expected by `map` is a lambda function that accepts one argument and defines the transformation of that argument. In our example, the lambda `(λ x → 2 * x)` defines a multiplication by 2. The second argument expected by `map` is the list of values that we will apply this transformation to. In this case, we pass the list `[0.5, 1.0, 1.5]`. So the result is doubling every element in the list, resulting in `[1.0, 2.0, 3.0]`.

## Currying

Curring is a special technique for defining a function without explicitly creating a lambda. It is done by _partially_ calling a function. The result is yet another function that expects the remainder of the arguments. It is easiest to understand with an example:

```rex
let 
    triple = (*) 3
in
    (triple 3, triple 5, triple 11)
```

First, we define a new function called `triple` which is the result of _partially_ calling the multiplication `(*)` operator. Multiplication usually expects 2 arguments. So when we call it with only 1 argument, we get back a _new_ function that stores the first argument, and expects one more argument. Whatever argument it receives, it will multiple it with the first argument that was received. So in our example above, we would get the result `(9, 15, 33)`.

Another way to think about currying is that it's a short-hand for explicitly defining a lambda function:

```rex
let 
    triple = (λ x → 3 * x)
in
    (triple 3, triple 5, triple 11)
```

There is no difference between these two expressions. Both will result in `(9, 15, 33)`. Some people prefer `(*) 3` and some people prefer `λ x → 3 * x`. It is mostly a matter of taste. If we think that to our `map` example, we should simplify it:

```rex
map ((*) 2) [0.5, 1.0, 1.5]
```

This is shorter and -- for many people -- easier to read.

## Composition

Function composition is a more advanced technique that also allows us to simplify code and make it more readable. Put simply, you can think of function composition as creating a "pipeline" of function calls. Let's say we have 3 functions -- `f`, `g`, and `h` -- that we need to call one after the other: `f (g (h x))`. This works, but it is a little messy. Function composition allows us to re-write this as `(f . g . h) x`. This has far fewer parentheses and many people find it easier to read (especially in the functional programming community).

While this seems like a small optimization, it can be very helpful in siutations where `f`, `g`, and `h` have multiple arguments and we combined composition with currying.

```rex
(foo x y . bar a . baz t u v) my_value
```

More clearly says "apply foo and then bar and then baz" than:

```rex
foo x y (bar a (baz t u v my_value))
```

## Pattern Matching

`match` performs structural pattern matching without braces. It takes a scrutinee expression followed by one or more `when` arms:

```rex
match named
  when Ok x -> handle_ok x
  when Err e -> handle_err e
  when _ -> default
```

Supported patterns today:

- Wildcards: `_`
- Variables: `x`
- Named constructors with one or more subpatterns: `Ok x`, `Pair (Just a) (Just b)`, `Node left right`
- Lists with pattern elements: `[]`, `[x]`, `[x, y, z]`, `[head, _]` (any arity is allowed)
- Cons with pattern parts: `x:xs`, `_ : rest`, `(Cons h t) : xs`
- Dict key presence: `{foo, bar}` (keys are identifiers only)

Another example on lists:

```rex
match list
  when [] -> "empty"
  when [x] -> x
  when [x, y, z] -> z
  when x:xs -> xs
  when _ -> "catch-all"
```

## Record Field Projection

ADT variants can declare record fields with `{ field: Type }`. When a value is *definitely* that record variant, you can project fields with `x.field`.

Single-variant ADT:

```rex
type Boxed = Boxed { value: i32 }

let
  x = Boxed { value = 1 }
in
  x.value
```

Multi-variant ADT (projection is only valid in a branch that proves the constructor):

```rex
type MyADT = MyVariant1 { field1: i32 } | MyVariant2 i32

let
  x = MyVariant1 { field1 = 1 }
in
  match x
    when MyVariant1 { field1 } -> x.field1
    when MyVariant2 _ -> 0
```

## Record Update

When a value is *definitely* a record-carrying constructor, you can create a new value by updating fields:

```rex
type Boxed = Boxed { value: i32 }

let
  x = Boxed { value = 1 }
  y = { x with { value = 2 } }
in
  y.value
```

For multi-variant ADTs, record update follows the same “constructor must be known” rule as projection (use `match` to refine the variant first).

## Standard Library (Prelude)

Rex ships with a small prelude of common helpers. The type system constrains these using type classes where appropriate.

- Arithmetic: `+`, `-`, `*`, `/`, `negate`, `zero`, `one`
- Equality: `==`, `!=`
- Ordering: `<`, `<=`, `>`, `>=`
- Booleans: `&&`, `||`
- Collections (List/Array/Option/Result): `map`, `fold`, `foldl`, `foldr`, `filter`, `filter_map`, `bind`, `ap`, `sum`, `mean`, `count`, `take`, `skip`, `zip`, `unzip`, `min`, `max`, `or_else`
- Option/Result helpers: `is_some`, `is_none`, `is_ok`, `is_err`

## Contribute

See [`docs/src/ARCHITECTURE.md`](docs/src/ARCHITECTURE.md) for a high-level tour of the crates and the parsing → typing → evaluation pipeline.

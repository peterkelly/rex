# Mini Project: Validating and Transforming Records

This example combines records, `match`, and `Result`.

## Goal

Build a small “workflow” in Rex:

1. validate a `User`
2. transform it (birthday)
3. return either a useful error or the transformed user

This is a pattern you can scale up: each step is a function returning a `Result`, and you connect
steps with `bind`.

```rex,interactive
type User = User { name: string, age: i32 }

let
  validate = \u ->
    if u.age < 0
      then Err "age must be non-negative"
      else Ok u,
  birthday = \u -> { u with { age = u.age + 1 } }
in
  bind (\u -> Ok (birthday u)) (validate (User { name = "Ada", age = 36 }))
```

## Worked extensions

### Example: update `birthday` to change name too

Problem: increment age and append `"!"` to the user name in one transform.

```rex,interactive
type User = User { name: string, age: i32 }

let
  birthday = \u ->
    { u with
        { age = u.age + 1
        , name = u.name + "!"
        }
    }
in
  birthday (User { name = "Ada", age = 36 })
```

Why this works: one record update can set multiple fields at once.

### Example: reject empty names during validation

Problem: make validation fail when `name == ""`.

```rex,interactive
type User = User { name: string, age: i32 }

let
  validate = \u ->
    if u.age < 0 then Err "age must be non-negative" else
    if u.name == "" then Err "name must be non-empty" else
      Ok u
in
  ( validate (User { name = "Ada", age = 36 })
  , validate (User { name = "", age = 36 })
  )
```

Why this works: the second guard introduces an additional failure branch before success.

### Example: structured error ADT

Problem: replace free-form strings with typed error constructors.

```rex,interactive
type UserError = NegativeAge | EmptyName
type User = User { name: string, age: i32 }

let
  validate = \u ->
    if u.age < 0 then Err NegativeAge else
    if u.name == "" then Err EmptyName else
      Ok u
in
  validate (User { name = "", age = 36 })
```

Why this works: callers can pattern-match on error constructors without string parsing.

## A worked “structured error” version

Instead of strings, define an error ADT:

```rex,interactive
type UserError = NegativeAge | EmptyName
type User = User { name: string, age: i32 }

let
  validate = \u ->
    if u.age < 0 then Err NegativeAge else
    if u.name == "" then Err EmptyName else
      Ok u,
  birthday = \u -> { u with { age = u.age + 1 } },
  run = \u -> bind (\ok -> Ok (birthday ok)) (validate u)
in
  ( run (User { name = "Ada", age = 36 })
  , run (User { name = "", age = 36 })
  , run (User { name = "Ada", age = (0 - 1) })
  )
```

### What to notice

- `validate` returns early with the first error it finds.
- `run` uses `bind` to only call `birthday` when validation succeeded.

## Worked examples

### Example: add an upper-age validation rule

Problem: reject ages greater than `150`.

```rex,interactive
type UserError = NegativeAge | EmptyName | TooOld
type User = User { name: string, age: i32 }

let
  validate = \u ->
    if u.age < 0 then Err NegativeAge else
    if u.age > 150 then Err TooOld else
    if u.name == "" then Err EmptyName else
      Ok u
in
  ( validate (User { name = "Ada", age = 36 })
  , validate (User { name = "Ada", age = 200 })
  )
```

Why this works: the additional guard catches out-of-range ages before success.

### Example: chain a second transform step with `bind`

Problem: run two transforms (`birthday` then `normalize_name`) after validation.

```rex,interactive
type User = User { name: string, age: i32 }

let
  validate = \u -> if u.age < 0 then Err "negative-age" else Ok u,
  birthday = \u -> Ok ({ u with { age = u.age + 1 } }),
  normalize_name = \u -> Ok ({ u with { name = u.name + "!" } }),
  run = \u ->
    bind normalize_name
      (bind birthday
        (validate u))
in
  run (User { name = "Ada", age = 36 })
```

Why this works: each `bind` feeds a successful result into the next transform.

### Example: split validation into smaller validators

Problem: compose independent validators with `bind`.

```rex,interactive
type User = User { name: string, age: i32 }

let
  check_age = \u -> if u.age < 0 then Err "negative-age" else Ok u,
  check_name = \u -> if u.name == "" then Err "empty-name" else Ok u,
  validate = \u -> bind check_name (check_age u)
in
  ( validate (User { name = "Ada", age = 36 })
  , validate (User { name = "", age = 36 })
  )
```

Why this works: each validator has the same `User -> Result User e` shape, so they compose cleanly.

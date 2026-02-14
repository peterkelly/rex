# Mini Project: Validating and Transforming Records

This example combines records, `match`, and `Result`.

## Goal

Build a small “workflow” in Rex:

1. validate a `User`
2. transform it (birthday)
3. return either a useful error or the transformed user

This is a pattern you can scale up: each step is a function returning a `Result`, and you connect
steps with `bind`.

```rex
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

## Suggested extensions

1. Change `birthday` to also append `"!"` to the name.
2. Make `validate` also reject empty names.
3. Return a more structured error type by defining a custom ADT instead of using `string`.

## A worked “structured error” version

Instead of strings, define an error ADT:

```rex
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

## Exercises

1. Add another validation rule (e.g. age must be <= 150) with a new error constructor.
2. Add a second transform step and chain it with `bind`.
3. Split `validate` into multiple small validators and chain them using `bind`.

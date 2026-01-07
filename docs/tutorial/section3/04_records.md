# Example: Records and Updates

This example focuses on record-carrying ADTs, because that’s where you most often use:

- projection (`x.field`)
- update (`{ x with { field = ... } }`)

## Goal

Model a `User` record, read its fields, and produce an updated copy.

## Step 1: define and update

```rex
type User = User { name: string, age: i32 }

let
  u: User = User { name = "Ada", age = 36 },
  older = { u with { age = u.age + 1 } }
in
  (u.age, older.age)
```

## What to notice

- `User { ... }` constructs a record-carrying ADT value.
- `u.age` is field projection.
- `{ u with { age = ... } }` updates the record payload and re-wraps the constructor.

## Step 2: update multiple fields

```rex
type User = User { name: string, age: i32 }

let
  u: User = User { name = "Ada", age = 36 },
  updated =
    { u with
        { age = u.age + 1
        , name = u.name + "!"
        }
    }
in
  (u, updated)
```

## Step 3: why `match` sometimes matters

Projection/update is only allowed when a field is *definitely available* on the type. With a
multi-variant ADT, you often refine it with `match` first:

```rex
type Sum = A { x: i32 } | B { x: i32 }

let s: Sum = A { x = 1 } in
match s
  when A {x} -> { s with { x = x + 1 } }
  when B {x} -> { s with { x = x + 2 } }
```

## Exercises

1. Write `birthday : User -> User` and apply it twice.
2. Add a field `admin: bool` and write `promote : User -> User` that sets it to true.
3. Add a third constructor `C { x: i32 }` to `Sum` and update the match.

# Example: Records and Updates

This example focuses on record-carrying ADTs, because that’s where you most often use:

- projection (`x.field`)
- update (`{ x with { field = ... } }`)

## Goal

Model a `User` record, read its fields, and produce an updated copy.

## Step 1: define and update

```rex,interactive
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

```rex,interactive
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

```rex,interactive
type Sum = A { x: i32 } | B { x: i32 }

let s: Sum = A { x = 1 } in
match s
  when A {x} -> { s with { x = x + 1 } }
  when B {x} -> { s with { x = x + 2 } }
```

## Worked examples

### Example: `birthday` applied twice

Problem: define `birthday : User -> User` and apply it two times.

```rex,interactive
type User = User { name: string, age: i32 }

let
  birthday = \u -> { u with { age = u.age + 1 } },
  u0: User = User { name = "Ada", age = 36 },
  u2 = birthday (birthday u0)
in
  (u0.age, u2.age)
```

Why this works: each `birthday` call returns a new `User` with `age` incremented by one.

### Example: add `admin` and `promote`

Problem: add a boolean field and set it to `true`.

```rex,interactive
type User = User { name: string, age: i32, admin: bool }

let
  promote = \u -> { u with { admin = true } },
  u0: User = User { name = "Ada", age = 36, admin = false }
in
  promote u0
```

Why this works: record update changes only `admin`, preserving other fields.

### Example: add constructor `C` and update the match

Problem: extend `Sum` with `C { x: i32 }` and keep updates valid.

```rex,interactive
type Sum = A { x: i32 } | B { x: i32 } | C { x: i32 }

let s: Sum = C { x = 1 } in
match s
  when A {x} -> { s with { x = x + 1 } }
  when B {x} -> { s with { x = x + 2 } }
  when C {x} -> { s with { x = x + 3 } }
```

Why this works: each arm refines `s` to a definite constructor, so the update is type-safe.

# Records: Projection and Update

Records are key/value structures with named fields. Rex supports:

- field projection: `base.field`
- record update: `{ base with { field = expr } }`

At the value level, “record” literals are written like dicts:

```rex,interactive
{ x = 1, y = 2 }
```

At the type level, record types are written with `:`:

```rex,interactive
{ x: i32, y: i32 }
```

## Projection

```rex,interactive
let p = { x = 1, y = 2 } in p.x
```

Projection is accepted when the field is *definitely available* on the type (see [Specification](../../SPEC.md)).

> **Tip:** If you get a “field not definitely available” type error, it usually means the typechecker can’t
> prove which ADT variant you have. A `match` often fixes it.

## Update

```rex,interactive
let p = { x = 1, y = 2 } in
  { p with { x = p.x + 10 } }
```

Updates can set multiple fields at once:

```rex,interactive
let p = { x = 1, y = 2 } in
  { p with { x = 100, y = 200 } }
```

## Updating record-carrying ADT variants

This is a common pattern:

```rex,interactive
type Sum = A { x: i32 } | B { x: i32 }

let s: Sum = A { x = 1 } in
match s
  when A {x} -> { s with { x = x + 1 } }
  when B {x} -> { s with { x = x + 2 } }
```

The `match` arms refine which constructor `s` has, allowing the update to typecheck.

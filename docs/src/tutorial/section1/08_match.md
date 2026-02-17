# Pattern Matching with `match`

Use `match` to branch on the shape of values.

`match` is the “workhorse” control flow construct in Rex. You’ll use it for:

- consuming ADTs (`Option`, `Result`, your own types),
- splitting lists (`[]` vs `x::xs`),
- checking key presence in dicts (`{a, b}`),
- refining record-carrying variants so projection/update typecheck.

## Matching ADTs

```rex,interactive
type Maybe a = Just a | Nothing

let fromMaybe = \d m ->
  match m
    when Just x -> x
    when Nothing -> d
in
  fromMaybe 0 (Just 5)
```

Rex checks matches for exhaustiveness on ADTs and reports missing constructors.

### Inline match syntax

You’ll often see compact “inline” matches in examples:

```rex,interactive
match (Some 1)
  when Some x -> x
  when None -> 0
```

## Common patterns

Wildcards:

```rex,interactive
match [1, 2, 3]
  when Empty -> 0
  when Cons _ _ -> 1
```

List patterns:

```rex,interactive
match [1, 2]
  when [] -> 0
  when [x] -> x
  when [x, y] -> x + y
  when _ -> 0
```

Cons patterns:

```rex,interactive
match [1, 2, 3]
  when h::t -> h
  when [] -> 0
```

`h::t` is equivalent to `Cons h t`; both expression forms are valid.

Record patterns on record-carrying constructors:

```rex,interactive
type Point = Point { x: i32, y: i32 }

let p = Point { x = 1, y = 2 } in
match p
  when Point { x: x, y: y } -> x + y
```

Dict key presence patterns:

```rex,interactive
let d = ({ a = 1, b = 2 }) is Dict i32 in
match d
  when {a, b} -> a + b
  when {a} -> a
  when {} -> 0
```

## Arrow spelling

Arms can use `->` or `→`:

```rex,interactive
type Bit = T | F

let v = T in
match v
  when T → 1
  when F -> 0
```

## Ordering and fallbacks

Match arms are tried top-to-bottom. Put specific patterns first and broad patterns (like `_` or
`{}`) last.

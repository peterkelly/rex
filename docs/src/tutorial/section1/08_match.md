# Pattern Matching with `match`

Use `match` to branch on the shape of values.

`match` is the “workhorse” control flow construct in Rex. You’ll use it for:

- consuming ADTs (`Option`, `Result`, your own types),
- splitting lists (`[]` vs `x:xs`),
- checking key presence in dicts (`{a, b}`),
- refining record-carrying variants so projection/update typecheck.

## Matching ADTs

```rex
type Maybe a = Just a | Nothing

let fromMaybe = \d m ->
  match m
    when Just x -> x
    when Nothing -> d
```

Rex checks matches for exhaustiveness on ADTs and reports missing constructors.

### Inline match syntax

You’ll often see compact “inline” matches in examples:

```rex
match (Some 1)
  when Some x -> x
  when None -> 0
```

## Common patterns

Wildcards:

```rex
match [1, 2, 3]
  when Empty -> 0
  when Cons _ _ -> 1
```

List patterns:

```rex
match [1, 2]
  when [] -> 0
  when [x] -> x
  when [x, y] -> x + y
```

Cons patterns:

```rex
match [1, 2, 3]
  when h:t -> h
  when [] -> 0
```

Record patterns on record-carrying constructors:

```rex
type Point = Point { x: i32, y: i32 }

match Point { x = 1, y = 2 }
  when Point {x, y} -> x + y
```

Dict key presence patterns:

```rex
let d = ({ a = 1, b = 2 }) is Dict i32 in
match d
  when {a, b} -> a + b
  when {a} -> a
  when {} -> 0
```

## Arrow spelling

Arms can use `->` or `→`:

```rex
match true
  when true → 1
  when false -> 0
```

## Ordering and fallbacks

Match arms are tried top-to-bottom. Put specific patterns first and broad patterns (like `_` or
`{}`) last.

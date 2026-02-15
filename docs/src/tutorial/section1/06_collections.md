# Tuples, Lists, and Dictionaries

Rex supports several lightweight data shapes.

## Tuples

Tuples group fixed-position values:

```rex,interactive
(1, "hi", true)
```

Rex supports tuple patterns in `match` and `let`. For indexing, use numeric projection
like `.0` and `.1`.

### Indexing tuples with `.`

```rex,interactive
let t = (1, "hi", true) in t.1
```

## Lists

List literals use square brackets:

```rex,interactive
[1, 2, 3]
```

Under the hood, lists are a prelude ADT `List a` with constructors `Empty` and `Cons`.

```rex,interactive
match [1, 2, 3]
  when Empty -> 0
  when Cons h t -> h
```

### List patterns (sugar)

Rex also supports list-pattern sugar:

```rex,interactive
match [1, 2, 3]
  when [] -> 0
  when [x] -> x
  when x:xs -> x
```

## Dictionaries (records / dict values)

Dictionary literals use braces:

```rex,interactive
{ a = 1, b = 2 }
```

These are “record-like” values. Depending on context they may be treated as a record type
(`{ a: i32, b: i32 }`) or as a dictionary-like value; either way, you can project fields when the
field is known to exist:

```rex,interactive
type R = R { a: i32, b: i32 }

let r: R = R { a = 1, b = 2 } in r.a
```

### Forcing a dictionary type

If you want a polymorphic “dictionary” (instead of a specific record type), use type ascription
with `is`:

```rex,interactive
({ a = 1, b = 2 }) is Dict i32
```

### Matching dictionaries

Dictionary patterns check for key presence and bind those keys to variables:

```rex,interactive
let d = ({ a = 1, b = 2 }) is Dict i32 in
match d
  when {a, b} -> a + b
  when {a} -> a
  when {} -> 0
```

`{}` is useful as a fallback: it requires no keys, so it matches any dict.

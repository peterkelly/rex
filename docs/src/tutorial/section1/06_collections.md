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

You can construct cons cells either as `Cons h t` (normal constructor call style) or with `h::t` sugar.

```rex,interactive
let xs = 1::2::3::[] in xs
```

```rex,interactive
match [1, 2, 3] with {
  case Empty -> 0;
  case Cons h t -> h;
}
```

### List patterns (sugar)

Rex also supports list-pattern sugar:

```rex,interactive
match [1, 2, 3] with {
  case [] -> 0;
  case [x] -> x;
  case x::xs -> x;
}
```

## Lists At Host Boundaries

Rex exposes one ordered collection type: `List a`. User-written list literals,
list constructors, pattern matching, and Rust host `Vec<T>` values all use this
same type.

Internally, the runtime may store a list as linked `Cons` / `Empty` cells or as
a slice over contiguous heap data. Rex code does not need to choose or convert
between those representations.

```rex,interactive
let
  data = [1, 2, 3]
in
  match data with {
    case x::xs -> x;
    case [] -> -1;
  }
```

For embedders, a Rust function returning `Vec<i32>` is exposed in Rex as
returning `List i32`, and a Rust parameter of type `Vec<i32>` accepts any Rex
`List i32`.

## Dictionaries (records / dict values)

Dictionary literals use braces:

```rex,interactive
{ a = 1, b = 2 }
```

These are “record-like” values. Depending on context they may be treated as a record type
(`{ a: i32, b: i32 }`) or as a dictionary-like value; either way, you can project fields when the
field is known to exist:

```rex,interactive
type R = R { a: i32, b: i32 };

let r: R = R { a = 1, b = 2 } in r.a
```

### Forcing a dictionary type

If you want a polymorphic “dictionary” (instead of a specific record type), use type ascription
with `is`:

```rex,interactive
({ a = 1, b = 2 }) is Dict i32
```

`Dict a` has string keys and values of one uniform type `a`. Dictionary literals use identifier
keys, while functions such as `dict_insert` and `dict_from_entries` also accept arbitrary runtime
strings.

For a complete, function-by-function dictionary reference with signatures and runnable examples,
see [Dictionaries](06_dictionaries.md).

### Dictionary operations

Lookup is option-based, and updates return new dictionaries:

```rex,interactive
let
  d0 = dict_singleton "alpha" 1,
  d1 = dict_insert "beta" 2 d0,
  d2 = dict_update "alpha" (\old -> map ((+) 10) old) d1,
  d3 = dict_remove "beta" d2
in
  (dict_get "alpha" d3, dict_has "beta" d3)
```

`dict_keys`, `dict_values`, and `dict_entries` return lists in lexicographic key order.
`dict_from_entries` performs the inverse conversion; if a key occurs more than once, its last
entry wins.

The ordinary `map`, `filter`, and `filter_map` functions operate on dictionary values while
preserving their keys. When the key is also needed, use `dict_map` or `dict_filter`; their callbacks
receive a `(string, a)` tuple:

```rex,interactive
let
  d = (({ a = 1, b = 2 }) is Dict i32),
  renamed = dict_map
    (\entry -> match entry with {
      case (key, value) -> ("prefix_" + key, value * 10);
    })
    d,
  selected = dict_filter
    (\entry -> match entry with {
      case (key, value) -> key != "b" && value > 0;
    })
    d
in
  (renamed, selected)
```

`dict_map` may produce the same output key from multiple input entries. Results are applied in the
input dictionary's lexicographic key order, so the result produced for the latest input key wins.

### Matching dictionaries

Dictionary patterns check for key presence and bind those keys to variables:

```rex,interactive
let d = ({ a = 1, b = 2 }) is Dict i32 in
match d with {
  case {a, b} -> a + b;
  case {a} -> a;
  case {} -> 0;
}
```

`{}` is useful as a fallback: it requires no keys, so it matches any dict.

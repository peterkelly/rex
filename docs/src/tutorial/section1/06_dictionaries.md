# Dictionaries

A dictionary in Rex is an immutable mapping from `String` keys to values. Its type is `Dict a`, where
`a` is the type of every value in the dictionary:

- Keys are always `String`.
- All values in one dictionary have the same type.
- Operations return new dictionaries; they do not change their inputs.

For example, a `Dict i32` can contain any number of integer values, but it cannot mix integers,
strings, or other value types:

```rex,interactive
let scores = ({ alice = 10, bob = 12 }) is Dict i32 in scores
```

Dictionary literals use identifier-shaped keys. Functions such as `dict_singleton`, `dict_insert`,
and `dict_from_entries` accept arbitrary `String` values as keys, including strings containing spaces.

## Dictionary order

Rex stores dictionary entries in ascending lexicographic key order. This determines the order
returned by `dict_keys`, `dict_values`, and `dict_entries`. It also makes collision handling in
`dict_map` deterministic.

## Function argument order

Rex functions are curried. Dictionary operations conventionally take the dictionary as their last
argument, which makes partial application straightforward. For example:

```rex,interactive
let get_alice = dict_get "alice" in
get_alice (({ alice = 10, bob = 12 }) is Dict i32)
```

In the signatures below, `a` and `b` are type variables. Every occurrence of the same variable must
be the same type.

Every example below is a complete executable Rex expression. You can edit it in place and run it to
see the resulting value.

## Quick reference

| Function | Type |
|---|---|
| `dict_empty` | `Dict a` |
| `dict_singleton` | `String -> a -> Dict a` |
| `dict_get` | `String -> Dict a -> Option a` |
| `dict_has` | `String -> Dict a -> Bool` |
| `dict_insert` | `String -> a -> Dict a -> Dict a` |
| `dict_remove` | `String -> Dict a -> Dict a` |
| `dict_update` | `String -> (Option a -> Option a) -> Dict a -> Dict a` |
| `dict_is_empty` | `Dict a -> Bool` |
| `dict_keys` | `Dict a -> List String` |
| `dict_values` | `Dict a -> List a` |
| `dict_entries` | `Dict a -> List (String, a)` |
| `dict_from_entries` | `List (String, a) -> Dict a` |
| `dict_map` | `((String, a) -> (String, b)) -> Dict a -> Dict b` |
| `dict_filter` | `((String, a) -> Bool) -> Dict a -> Dict a` |
| `map` on `Dict` | `(a -> b) -> Dict a -> Dict b` |
| `filter` on `Dict` | `(a -> Bool) -> Dict a -> Dict a` |

## Construction

### `dict_empty`

Type: `dict_empty : Dict a`

`dict_empty` is an empty dictionary. Because it contains no values, its value type normally comes
from the surrounding context or an explicit annotation.

```rex,interactive
let numbers: Dict i32 = dict_empty in numbers
```

### `dict_singleton`

Type: `dict_singleton : String -> a -> Dict a`

`dict_singleton key value` constructs a dictionary containing exactly one entry.

```rex,interactive
dict_singleton "request id" "req-123"
```

### `dict_from_entries`

Type: `dict_from_entries : List (String, a) -> Dict a`

`dict_from_entries entries` constructs a dictionary from a list of key/value tuples. It processes
the list from first to last. If a key appears more than once, its last value wins. The resulting
dictionary is stored in lexicographic key order, not list order.

```rex,interactive
dict_from_entries
  [("beta", 2), ("alpha", 1), ("beta", 20)]
```

## Lookup and inspection

### `dict_get`

Type: `dict_get : String -> Dict a -> Option a`

`dict_get key dictionary` returns `Some value` when the key exists and `None` when it does not.
Lookup does not fail merely because a key is absent.

```rex,interactive
let scores = dict_from_entries [("alice", 10), ("bob", 12)] in
(dict_get "alice" scores, dict_get "carol" scores)
```

### `dict_has`

Type: `dict_has : String -> Dict a -> Bool`

`dict_has key dictionary` reports whether the key exists, without retrieving its value.

```rex,interactive
let scores = dict_from_entries [("alice", 10), ("bob", 12)] in
(dict_has "bob" scores, dict_has "carol" scores)
```

### `dict_is_empty`

Type: `dict_is_empty : Dict a -> Bool`

`dict_is_empty dictionary` is `true` only when the dictionary has no entries.

```rex,interactive
let empty: Dict String = dict_empty in
(dict_is_empty empty, dict_is_empty (dict_singleton "name" "Rex"))
```

### `dict_keys`

Type: `dict_keys : Dict a -> List String`

`dict_keys dictionary` returns all keys in ascending lexicographic order.

```rex,interactive
dict_keys (dict_from_entries [("z", 1), ("alpha", 2), ("middle", 3)])
```

### `dict_values`

Type: `dict_values : Dict a -> List a`

`dict_values dictionary` returns the values ordered by their corresponding keys. It does not sort
the values themselves.

```rex,interactive
dict_values (dict_from_entries [("z", 1), ("alpha", 2), ("middle", 3)])
```

The result is `[2, 3, 1]`, corresponding to keys `alpha`, `middle`, and `z`.

### `dict_entries`

Type: `dict_entries : Dict a -> List (String, a)`

`dict_entries dictionary` returns key/value tuples in ascending lexicographic key order.

```rex,interactive
dict_entries (dict_from_entries [("z", 1), ("alpha", 2), ("middle", 3)])
```

## Immutable updates

### `dict_insert`

Type: `dict_insert : String -> a -> Dict a -> Dict a`

`dict_insert key value dictionary` returns a new dictionary. It adds an absent key or replaces the
value of a present key. The input dictionary remains unchanged.

```rex,interactive
let
  original = dict_singleton "a" 1,
  added = dict_insert "b" 2 original,
  replaced = dict_insert "a" 99 original
in
  (original, added, replaced)
```

### `dict_remove`

Type: `dict_remove : String -> Dict a -> Dict a`

`dict_remove key dictionary` returns a new dictionary without that key. Removing an absent key
returns an equivalent dictionary and is not an error.

```rex,interactive
let original = dict_from_entries [("a", 1), ("b", 2)] in
(original, dict_remove "b" original, dict_remove "missing" original)
```

### `dict_update`

Type: `dict_update : String -> (Option a -> Option a) -> Dict a -> Dict a`

`dict_update key update dictionary` calls `update` with `Some current_value` when the key is present
or `None` when it is absent. The callback's result controls the new dictionary:

- `Some new_value` inserts or replaces the key.
- `None` removes the key.

```rex,interactive
let
  original = dict_singleton "visits" 2,
  incremented = dict_update
    "visits"
    (\current -> match current with {
      case Some count -> Some (count + 1);
      case None -> Some 1;
    })
    original,
  removed = dict_update "visits" (\_ -> None) incremented
in
  (original, incremented, removed)
```

## Transforming entries

The `dict_map` and `dict_filter` callbacks receive a two-element `(String, a)` tuple, so they can
inspect both the key and value. Use a tuple pattern in a `match` expression to name both parts.

Callback applications for different entries may run in parallel. Rex functions are pure, so this
does not change the result.

### `dict_map`

Type: `dict_map : ((String, a) -> (String, b)) -> Dict a -> Dict b`

`dict_map transform dictionary` transforms both keys and values. The callback must return a
`(String, b)` tuple, which becomes an entry in the result.

```rex,interactive
let
  input = (({ c = 3, a = 1, b = 2 }) is Dict i32),
  renamed = dict_map
    (\entry -> match entry with {
      case (key, value) -> ("item_" + key, value * 10);
    })
    input
in
  renamed
```

Multiple callbacks may return the same output key. Rex applies completed results in the input
dictionary's original lexicographic key order, regardless of callback completion order. Therefore,
the result from the latest input key wins:

```rex,interactive
dict_map
  (\entry -> match entry with {
    case (key, value) -> ("same", key + ":" + show value);
  })
  (({ c = 3, a = 1, b = 2 }) is Dict i32)
```

The input order is `a`, `b`, `c`, so the final dictionary contains `"c:3"` at key `same`.

### `dict_filter`

Type: `dict_filter : ((String, a) -> Bool) -> Dict a -> Dict a`

`dict_filter predicate dictionary` keeps each original entry for which the predicate returns
`true`. The predicate can inspect both key and value. Accepted entries keep their original keys and
values.

```rex,interactive
dict_filter
  (\entry -> match entry with {
    case (key, value) -> key != "draft" && value >= 2;
  })
  (({ draft = 10, first = 1, second = 2, third = 3 }) is Dict i32)
```

## Transforming values with typeclass functions

`Dict` implements `Functor` and `Filterable`. Their generic functions operate on values only; the
callback does not receive a key. These functions preserve every retained entry's original key.

Like the entry-aware functions, callback applications for different values may run in parallel.

### `map` on dictionaries

Type: `map : (a -> b) -> Dict a -> Dict b`

`map transform dictionary` transforms every value and preserves every key. Use `dict_map` instead
when the callback needs the key or must produce new keys.

```rex,interactive
map
  (\score -> "score=" + show score)
  (({ alice = 10, bob = 12 }) is Dict i32)
```

### `filter` on dictionaries

Type: `filter : (a -> Bool) -> Dict a -> Dict a`

`filter predicate dictionary` tests values only and preserves the keys of accepted values. Use
`dict_filter` when filtering depends on a key.

```rex,interactive
filter
  (\score -> score >= 10)
  (({ alice = 10, bob = 7, carol = 12 }) is Dict i32)
```

## Choosing the right transformation

| Need | Function |
|---|---|
| Transform values and preserve all keys | `map` |
| Keep entries based only on values | `filter` |
| Transform keys and/or inspect keys while mapping | `dict_map` |
| Keep entries based on keys and/or values | `dict_filter` |

For dictionary literal syntax and dictionary pattern matching, see [Collections](06_collections.md).

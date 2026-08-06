# Example: List Helpers

List-specific helpers complement the generic `map`, `filter`, folds, and `Sequence` operations.
They use `u64` positions and return `Option` when indexing or slicing can fail.

## Safe access

```rex,interactive
( list_get 1 [10, 20, 30]
, list_get 3 [10, 20, 30]
, list_slice 1 3 [10, 20, 30]
, list_slice 3 1 [10, 20, 30]
)
```

`list_get` uses a zero-based index. `list_slice` uses a half-open range and returns `None` when
either bound is invalid or the end precedes the start.

## Constructing and reshaping lists

```rex,interactive
( list_reverse [1, 2, 3]
, list_concat [[1, 2], [], [3, 4]]
, list_repeat 3 "rex"
)
```

`list_concat` removes exactly one layer of nesting. `list_repeat 0 value` returns an empty list.

## Searching

```rex,interactive
let even = \x -> x % 2 == 0 in
( list_any even [1, 3, 4, 5]
, list_all even [2, 4, 6]
, list_find even [1, 3, 4, 6]
, list_find_index even [1, 3, 4, 6]
, list_count even [1, 2, 3, 4, 5, 6]
)
```

The `any`, `all`, and find helpers inspect elements from left to right and stop when their answer is
known. The find helpers return the first match. On an empty list, `list_any` is `false`, `list_all`
is `true`, and both find helpers return `None`.

## Partitioning

```rex,interactive
list_partition (\x -> x < 0) [3, negate 1, 0, negate 4, 2]
```

The first output list contains matching elements and the second contains rejected elements. Both
retain their relative order from the input.

# Demo: Merge Sort

This demo implements merge sort, a divide-and-conquer algorithm that splits a list into halves, recursively sorts each half, and then merges the two sorted results. The implementation highlights a full pipeline of helper functions (`split`, `merge`, `compare`) and demonstrates how recursive decomposition can produce deterministic, stable ordering over immutable lists.

Related reading: [Merge sort](https://en.wikipedia.org/wiki/Merge_sort).

`compare_i32` converts primitive comparisons into an `Order` ADT, and `split_alt` peels off pairs to partition input into two sublists without mutation. `mergesort` handles the empty and singleton base cases, then recursively sorts both halves and combines them with `merge`, which pattern-matches on two lists and always emits the smaller head first.

```rex,interactive
type Order = Lt | Eq | Gt

fn compare_i32 : i32 -> i32 -> Order = \a b ->
  if a < b then Lt else if a == b then Eq else Gt

fn split_alt : List i32 -> (List i32, List i32) = \xs ->
  match xs
    when [] -> ([], [])
    when [x] -> ([x], [])
    when x::y::rest ->
      let (xs1, ys1) = split_alt rest in (Cons x xs1, Cons y ys1)

fn merge : List i32 -> List i32 -> List i32 = \xs ys ->
  match (xs, ys)
    when ([], _) -> ys
    when (_, []) -> xs
    when (x::xt, y::yt) ->
      match (compare_i32 x y)
        when Lt -> Cons x (merge xt ys)
        when Eq -> Cons x (Cons y (merge xt yt))
        when Gt -> Cons y (merge xs yt)

fn mergesort : List i32 -> List i32 = \xs ->
  match xs
    when [] -> []
    when [x] -> [x]
    when _ ->
      let (left, right) = split_alt xs in
      merge (mergesort left) (mergesort right)

let
  input = [9, 1, 7, 3, 2, 8, 6, 4, 5]
in
  mergesort input
```

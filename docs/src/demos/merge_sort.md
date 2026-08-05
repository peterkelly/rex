# Demo: Merge Sort

This demo implements merge sort, a divide-and-conquer algorithm that splits a list into halves, recursively sorts each half, and then merges the two sorted results. The implementation highlights recursive `split_alt` and `merge` helpers together with the prelude's `cmp` function, and demonstrates how recursive decomposition can produce deterministic, stable ordering over immutable lists.

Related reading: [Merge sort](https://en.wikipedia.org/wiki/Merge_sort).

`cmp` returns the prelude's `Ordering` ADT (`Less`, `Equal`, or `Greater`), and `split_alt` peels off pairs to partition input into two sublists without mutation. `mergesort` handles the empty and singleton base cases, then recursively sorts both halves and combines them with `merge`, which pattern-matches on two lists and emits the smaller head first. On equality, it takes from the left sublist first to keep the sort stable.

```rex,interactive
fn split_alt : List i32 -> (List i32, List i32) = \xs ->
  match xs with {
    case [] -> ([], []);
    case [x] -> ([x], []);
    case x::y::rest ->
      let (xs1, ys1) = split_alt rest in (Cons x xs1, Cons y ys1);
  };

fn merge : List i32 -> List i32 -> List i32 = \xs ys ->
  match (xs, ys) with {
    case ([], _) -> ys;
    case (_, []) -> xs;
    case (x::xt, y::yt) ->
      match (cmp x y) with {
        case Less -> Cons x (merge xt ys);
        case Equal -> Cons x (merge xt ys);
        case Greater -> Cons y (merge xs yt);
      };
  };

fn mergesort : List i32 -> List i32 = \xs ->
  match xs with {
    case [] -> [];
    case [x] -> [x];
    case _ ->
      let (left, right) = split_alt xs in
      merge (mergesort left) (mergesort right);
  };

let
  input = [9, 1, 7, 3, 2, 8, 6, 4, 5]
in
  mergesort input
```

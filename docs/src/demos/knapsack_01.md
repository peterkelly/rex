# Demo: 0/1 Knapsack

This demo solves the 0/1 knapsack optimization problem with dynamic programming: each item can be taken at most once, and the algorithm computes the best achievable value for every capacity from `0` to `max_cap`. The table is represented as immutable rows, and each new row is derived from the previous one by choosing between “take” and “skip” for the current item.

Related reading: [Knapsack problem](https://en.wikipedia.org/wiki/Knapsack_problem).

`zeros` initializes the base DP row, `build_row` computes one new row for a single item, and `go` folds this process across the full item list. For each capacity, `build_row` compares `without` (skip item) and `with_item` (take item plus best compatible remainder), then stores the maximum; `solve` returns the final cell at `max_cap`.

```rex,interactive
type Item = Item { w: i32, v: i32 }

fn nth : List i32 -> i32 -> i32 = \xs i ->
  match xs
    when [] -> 0
    when x:rest ->
      if i == 0 then x else nth rest (i - 1)

fn zeros : i32 -> i32 -> List i32 = \i max_cap ->
  if i > max_cap then [] else Cons 0 (zeros (i + 1) max_cap)

fn build_row : Item -> List i32 -> i32 -> i32 -> List i32 = \item prev cap max_cap ->
  if cap > max_cap then
    []
  else
    let
      without = nth prev cap,
      with_item =
        if item.w <= cap then
          item.v + nth prev (cap - item.w)
        else
          0,
      best = if without >= with_item then without else with_item
    in
      Cons best (build_row item prev (cap + 1) max_cap)

fn go : List Item -> List i32 -> i32 -> List i32 = \remaining row max_cap ->
  match remaining
    when [] -> row
    when item:rest ->
      let next = build_row item row 0 max_cap in
      go rest next max_cap

fn solve : List Item -> i32 -> i32 = \items max_cap ->
  nth (go items (zeros 0 max_cap) max_cap) max_cap

let
  items = [
    Item { w = 2, v = 3 },
    Item { w = 3, v = 4 },
    Item { w = 4, v = 5 },
    Item { w = 5, v = 8 }
  ],
  cap_5 = solve items 5,
  cap_8 = solve items 8
in
  (cap_5, cap_8)
```

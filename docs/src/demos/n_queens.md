# Demo: N-Queens Backtracking

This demo counts valid placements of queens on an `N x N` chessboard using backtracking with pruning. It places one queen per row, rejects positions that share a column or diagonal with prior queens, and recursively explores only safe continuations, which makes it a compact example of constraint search.

Related reading: [N-queens problem](https://en.wikipedia.org/wiki/Eight_queens_puzzle#The_n-queens_problem).

`is_safe` checks a candidate column against previously placed queens, carrying a diagonal distance counter so both diagonal directions can be tested with `abs_i32`. `try_cols` iterates candidate columns within a row and accumulates solution counts, while `count_from` advances to the next row after each safe placement; `solve` just seeds the recursion with row `0` and an empty placement list.

```rex,interactive
fn abs_i32 : i32 -> i32 = \x ->
  if x < 0 then 0 - x else x

fn is_safe : i32 -> List i32 -> i32 -> bool = \col placed dist ->
  match placed
    when [] -> true
    when c::rest ->
      if col == c then
        false
      else if abs_i32 (col - c) == dist then
        false
      else
        is_safe col rest (dist + 1)

fn count_from : i32 -> i32 -> List i32 -> i32 = \row n placed ->
  if row == n then
    1
  else
    try_cols row n placed 0

fn try_cols : i32 -> i32 -> List i32 -> i32 -> i32 = \row n placed col ->
  if col == n then
    0
  else
    let rest = try_cols row n placed (col + 1) in
    if is_safe col placed 1 then
      count_from (row + 1) n (Cons col placed) + rest
    else
      rest

fn solve : i32 -> i32 = \n ->
  count_from 0 n []

(solve 4, solve 5)
```

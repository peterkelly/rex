# Demo: Dijkstra Lite

This demo models a minimal shortest-path problem and applies the core relaxation idea behind Dijkstra-style algorithms: compare a direct edge with an indirect path through an intermediate node and keep the smaller distance. It uses a small `Dist` ADT (`Inf` or `Finite`) to make unreachable and reachable cases explicit and type-safe.

Related reading: [Dijkstra's algorithm](https://en.wikipedia.org/wiki/Dijkstra%27s_algorithm).

`add_weight` and `min_dist` isolate the distance algebra, so `shortest_a_to_b` can read clearly as “direct path versus via-`C` path, then pick minimum.” The sample graphs in the final `let` block exercise both outcomes: one where routing through `C` wins and one where the direct edge is best, with `as_i32` converting the ADT into plain output numbers for display.

```rex,interactive
type Dist = Inf | Finite i32
type Graph = Graph { ab: i32, ac: i32, cb: i32 }

fn add_weight : Dist -> i32 -> Dist = (\d w ->
  match d
    when Inf -> Inf
    when Finite x -> Finite (x + w)
)

fn min_dist : Dist -> Dist -> Dist = (\a b ->
  match (a, b)
    when (Inf, x) -> x
    when (x, Inf) -> x
    when (Finite x, Finite y) ->
      if x <= y then Finite x else Finite y
)

fn shortest_a_to_b : Graph -> Dist = (\g ->
  let
    direct = Finite g.ab,
    via_c = add_weight (Finite g.ac) g.cb
  in
    min_dist direct via_c
)

fn as_i32 : Dist -> i32 = (\d ->
  match d
    when Inf -> -1
    when Finite x -> x
)

let
  g1 = Graph { ab = 10, ac = 3, cb = 4 },
  g2 = Graph { ab = 5, ac = 9, cb = 9 },
  d1 = shortest_a_to_b g1,
  d2 = shortest_a_to_b g2
in
  (as_i32 d1, as_i32 d2)
```

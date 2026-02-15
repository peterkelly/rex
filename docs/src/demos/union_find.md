# Demo: Union-Find

This demo implements core union-find operations for tracking connected components in a small graph. It maintains parent links, finds set representatives, and merges sets with `union`, then checks connectivity; together these operations demonstrate how incremental edge additions can be answered with near-constant-time component queries.

Related reading: [Disjoint-set data structure](https://en.wikipedia.org/wiki/Disjoint-set_data_structure).

`get_parent` and `set_parent` provide indexed access over a fixed-size parent record, while `find` follows parent pointers recursively until it reaches a representative. `union` links one representative to another when sets differ, and `connected` compares representatives; the final block performs a few unions and returns both connectivity checks and representatives to show resulting components.

```rex,interactive
type UF = UF { p0: i32, p1: i32, p2: i32, p3: i32, p4: i32 }

fn get_parent : UF -> i32 -> i32 = \uf x ->
  if x == 0 then uf.p0
  else if x == 1 then uf.p1
  else if x == 2 then uf.p2
  else if x == 3 then uf.p3
  else uf.p4

fn set_parent : UF -> i32 -> i32 -> UF = \uf x p ->
  if x == 0 then { uf with { p0 = p } }
  else if x == 1 then { uf with { p1 = p } }
  else if x == 2 then { uf with { p2 = p } }
  else if x == 3 then { uf with { p3 = p } }
  else { uf with { p4 = p } }

fn find : UF -> i32 -> i32 = \uf x ->
  let px = get_parent uf x in
  if px == x then x else find uf px

fn union : UF -> i32 -> i32 -> UF = \uf a b ->
  let
    ra = find uf a,
    rb = find uf b
  in
    if ra == rb then uf else set_parent uf rb ra

fn connected : UF -> i32 -> i32 -> bool = \uf a b -> find uf a == find uf b

let
  uf0 = UF { p0 = 0, p1 = 1, p2 = 2, p3 = 3, p4 = 4 },
  uf1 = union uf0 0 1,
  uf2 = union uf1 1 2,
  uf3 = union uf2 3 4
in
  (
    connected uf3 0 2,
    connected uf3 0 4,
    find uf3 2,
    find uf3 4
  )
```

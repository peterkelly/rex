# Demo: Binary Search Tree

This demo builds and queries a binary search tree, where values smaller than a node go left and larger values go right. It shows insertion, membership testing, and size calculation over a custom recursive ADT, illustrating how ordered data structures can be expressed with pure pattern-matching functions.

Related reading: [Binary search tree](https://en.wikipedia.org/wiki/Binary_search_tree).

The `Tree` type has `Empty` and `Node` constructors, and each operation recursively follows tree structure. `insert` descends left or right based on key order and rebuilds the path back up, `contains` follows the same branching logic to test membership, and `size` traverses both subtrees to count nodes; the final `let` block builds an example tree and returns a tuple of summary queries.

```rex,interactive
type Tree = Empty | Node { key: i32, left: Tree, right: Tree }

fn insert : i32 -> Tree -> Tree = \k t ->
  match t
    when Empty -> Node { key = k, left = Empty, right = Empty }
    when Node {key, left, right} ->
      if k < key then
        Node { key = key, left = insert k left, right = right }
      else if k > key then
        Node { key = key, left = left, right = insert k right }
      else
        t

fn contains : i32 -> Tree -> bool = \k t ->
  match t
    when Empty -> false
    when Node {key, left, right} ->
      if k == key then true
      else if k < key then contains k left
      else contains k right

fn size : Tree -> i32 = \t ->
  match t
    when Empty -> 0
    when Node {left, right} -> 1 + size left + size right

let
  t0: Tree = Empty
in
  let
    t1 = insert 7 (insert 2 (insert 9 (insert 1 (insert 5 t0)))),
    t2 = insert 8 t1
  in
    (size t2, contains 5 t2, contains 4 t2)
```

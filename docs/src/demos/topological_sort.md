# Demo: Topological Sort (Kahn Style)

This demo computes a topological ordering of a directed acyclic graph using a Kahn-style process: repeatedly select nodes with zero in-degree, output them, and remove their outgoing edges. It demonstrates dependency resolution in graph form and returns an empty list when edges remain but no valid next node exists.

Related reading: [Topological sorting](https://en.wikipedia.org/wiki/Topological_sorting).

The helpers break the algorithm into pure list operations: `in_degree` counts incoming edges, `remove_outgoing` deletes edges from a chosen node, and `enqueue_zeros` updates the processing queue with newly unlocked nodes. `kahn` drives the main loop by consuming the queue and accumulating output order, with a final reversal because nodes are prepended during recursion.

```rex,interactive
type Node = A | B | C | D;
type Edge = Edge Node Node;

fn node_eq : Node -> Node -> Bool = \a b ->
  match (a, b) with {
    case (A, A) -> true;
    case (B, B) -> true;
    case (C, C) -> true;
    case (D, D) -> true;
    case _ -> false;
  };

fn contains : List Node -> Node -> Bool = \xs x ->
  match xs with {
    case [] -> false;
    case y::ys -> if node_eq y x then true else contains ys x;
  };

fn append : List Node -> List Node -> List Node = \xs ys ->
  match xs with {
    case [] -> ys;
    case h::t -> Cons h (append t ys);
  };

fn reverse_go : List Node -> List Node -> List Node = \rest acc ->
  match rest with {
    case [] -> acc;
    case h::t -> reverse_go t (Cons h acc);
  };

fn reverse : List Node -> List Node = \xs ->
  reverse_go xs [];

fn is_empty<a> : List a -> Bool = \xs ->
  match xs with {
    case [] -> true;
    case _::_ -> false;
  };

fn remove_outgoing : List Edge -> Node -> List Edge = \edges n ->
  match edges with {
    case [] -> [];
    case Edge from to::rest ->
      if node_eq from n then remove_outgoing rest n
      else Cons (Edge from to) (remove_outgoing rest n);
  };

fn in_degree : List Edge -> Node -> i32 = \edges n ->
  match edges with {
    case [] -> 0;
    case Edge from to::rest ->
      let tail = in_degree rest n in
      if node_eq to n then 1 + tail else tail;
  };

fn push_unique : List Node -> Node -> List Node = \queue n ->
  if contains queue n then queue else append queue [n];

fn enqueue_zeros : List Node -> List Node -> List Node -> List Edge -> List Node = \nodes queue seen edges ->
  match nodes with {
    case [] -> queue;
    case n::rest ->
      let queue1 =
        if contains seen n then
          queue
        else if in_degree edges n == 0 then
          push_unique queue n
        else
          queue
      in
        enqueue_zeros rest queue1 seen edges;
  };

fn kahn : List Node -> List Node -> List Node -> List Edge -> List Node -> List Node = \queue seen order edges nodes ->
  match queue with {
    case [] ->
      if is_empty edges then
        reverse order
      else
        [];
    case n::rest ->
      let
        edges1 = remove_outgoing edges n,
        seen1 = Cons n seen,
        queue1 = enqueue_zeros nodes rest seen1 edges1
      in
        kahn queue1 seen1 (Cons n order) edges1 nodes;
  };

let
  nodes = [A, B, C, D],
  edges = [Edge A B, Edge A C, Edge B D, Edge C D],
  initial = enqueue_zeros nodes [] [] edges
in
  kahn initial [] [] edges nodes
```

# Graphviz tools for Rex

The workflow host exposes semantic Graphviz rendering as `tools.graphviz`. Workflows construct a
completed `Graph` from Rex values and call `render`; DOT is a private compiler target and no
function accepts raw DOT source.

## Semantic model

- `Graph.nodes` is a `Dict NodeAttributes`. Each key is a globally unique node identifier and each
  value contains that node's final attributes.
- `Graph.edges` is a list of binary `Edge` values. Both endpoints must refer to declared nodes;
  missing nodes are rejected instead of being created implicitly.
- `Graph.node_defaults` and `Graph.edge_defaults` provide graph-wide shared styling. Rex record
  values and record updates provide more localized reuse without order-dependent DOT defaults.
- `Graph.attributes` contains final graph settings such as rank direction, margins, colors, and
  spline policy. The layout engine is selected exactly once by the `render` argument.
- `Endpoint` and `Port` attach edges to named label regions or standard compass positions.
- `TextLabel` and `HtmlLabel` distinguish ordinary text from Graphviz's HTML-like label language.
- `Subgraph` contains its own node dictionary, binary edge list, final attributes, and child arena
  references. `PlainSubgraph` represents a layout group and `Cluster` represents a visible cluster.
- Every node is defined in exactly one graph or subgraph dictionary. Node identifiers are global
  across the graph, matching Graphviz's node identity semantics.
- Attribute records have an `extra: Dict String` escape hatch for attributes supported by the
  packaged Graphviz installation but not modeled by a typed field. Typed/extra collisions are
  rejected.

`Graph`, `GraphAttributes`, `NodeAttributes`, `EdgeAttributes`, `Subgraph`, and the other
all-optional attribute records have registered defaults. Rex code can therefore supply only the
fields that differ from their semantic defaults.

The serializer emits a deterministic private DOT program in this order: graph attributes, node
and edge defaults, root nodes, root subgraphs, then root edges. Each subgraph similarly emits its
attributes, nodes, children, and edges. Rex code cannot depend on or manipulate this order.

## Subgraph arena

Recursive Rust/Rex ADT families are intentionally avoided, so `Graph.subgraphs` is an arena.
Each `Subgraph.subgraphs` entry names a child key in that arena. Entries that are not children of
another entry are emitted as roots. The validator rejects missing children, cycles, multiple
parents, duplicate node definitions, and excessive nesting.

## Rendering

```rex
import tools.graphviz as G;

fn endpoint (node: String) -> G.Endpoint = G.Endpoint {
    node = node,
    port = None
};

let
    empty_node = G.NodeAttributes {},
    graph = G.Graph {
        id = Some "workflow",
        nodes = dict_from_entries [
            ("prepare", empty_node),
            ("render", empty_node)
        ],
        edges = [
            G.Edge {
                from = endpoint "prepare",
                to = endpoint "render",
                attributes = G.EdgeAttributes {}
            }
        ]
    }
in
    G.render graph G.LayoutDot G.FormatSvg
```

`render` returns `Result RenderedGraph GraphvizError`. The artifact's `content` is a CAS hash.
`InvalidGraph` reports semantic validation failures. `ProcessFailed` reports Graphviz diagnostics,
including unsupported output plugins and invalid HTML-like labels. Storage and executor failures
remain Rex evaluation errors.

The module includes the packaged Graphviz layout engines and headless output formats. Unsupported
formats produce `ProcessFailed` rather than silently changing the requested representation.

Complete, typechecked workflows are in [`examples/graphviz`](../../../../examples/graphviz/README.md).

# Graphviz workflow examples

These programs build Graphviz inputs from typed Rex values. They never concatenate or submit raw
DOT source.

- `render_workflow.rex` composes a clustered directed graph from node dictionaries, binary edges,
  graph-level node defaults, final graph attributes, and a semantic subgraph arena entry.
- `render_ports.rex` connects HTML-table cells through typed named ports and compass points.
- `render_record_ports.rex` models a compiler pipeline with record-shaped nodes, named data and
  error ports, fan-in to diagnostics, and separate artifact outputs.
- `render_compass_ports.rex` models a branched workflow whose success, error, retry, audit, and
  metrics edges attach to deliberately selected node sides.
- `render_expression_ast.rex` tokenizes and parses an arithmetic expression, then dynamically
  builds a graph from the computed abstract syntax tree.

Run from the workspace root:

```sh
cargo run -p rex --bin rex -- --store-path ./store run \
  examples/graphviz/render_workflow.rex
```

Replace the final path with any of the other example filenames to run it.

The result contains a `content` hash. Export that hash with the workflow CLI's `store export`
command, or pass it directly to another CAS-aware stage.

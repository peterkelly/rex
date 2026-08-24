// Render a clustered workflow graph as an SVG in the content-addressable store.
//
// Run from the workspace root:
//   cargo run -p rex --bin rex -- --store-path ./store run \
//     examples/graphviz/render_workflow.rex
import tools.graphviz as G;

fn endpoint (node: String) -> G.Endpoint = G.Endpoint {
    node = node,
    port = None
};

fn edge (attributes: G.EdgeAttributes) -> (from: String) -> (to: String) -> G.Edge = G.Edge {
    from = endpoint from,
    to = endpoint to,
    attributes = attributes
};

let
    node_attributes = G.NodeAttributes {},
    edge_attributes = G.EdgeAttributes {},
    build_cluster = G.Subgraph {
        kind = G.Cluster,
        id = Some "build",
        attributes = G.SubgraphAttributes {
            label = Some (G.Label.Text "Build"),
            padding = Some 16.0
        },
        nodes = {
            parse = node_attributes,
            typecheck = node_attributes
        },
        edges = [edge edge_attributes "parse" "typecheck"]
    },
    graph = G.Graph {
        id = Some "rex_workflow",
        attributes = G.GraphAttributes {
            margin = Some (G.GraphMargin {
                horizontal = 0.25,
                vertical = None
            }),
            rank_direction = Some G.RankDirection.LeftToRight,
            ordering = Some G.EdgeOrdering.Outgoing,
            center = Some true,
            background_color = Some "white",
            splines = Some G.SplineMode.Spline
        },
        node_defaults = G.NodeAttributes {
            shape = Some G.NodeShape.Box,
            styles = Some [G.NodeStyle.Rounded, G.NodeStyle.Filled],
            fill_color = Some "lightgoldenrod1"
        },
        nodes = {
            evaluate = G.NodeAttributes {
                fill_color = Some "lightblue"
            }
        },
        edges = [
            edge G.EdgeAttributes {
                label = Some (G.Label.Text "typed program")
            } "typecheck" "evaluate"
        ],
        subgraphs = { build = build_cluster }
    }
in
    G.render graph G.LayoutEngine.Dot G.RenderFormat.Svg

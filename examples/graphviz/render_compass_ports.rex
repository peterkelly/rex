// Render a branched workflow where each node side has a distinct routing role.
//
// Run from the workspace root:
//   cargo run -p rex --bin rex -- --store-path ./store run \
//     examples/graphviz/render_compass_ports.rex
import tools.graphviz as G;

fn side (node: String) -> (compass: G.CompassPoint) -> G.Endpoint = G.Endpoint {
    node = node,
    port = Some (G.Port {
        name = None,
        compass = Some compass
    })
};

fn connection (attributes: G.EdgeAttributes)
    -> (from_node: String) -> (from_side: G.CompassPoint)
    -> (to_node: String) -> (to_side: G.CompassPoint) -> G.Edge = G.Edge {
        from = side from_node from_side,
        to = side to_node to_side,
        attributes = attributes
    };

let
    edge_attributes = G.EdgeAttributes {},
    normal_node = G.NodeAttributes {
        shape = Some G.NodeShape.Box,
        styles = Some [G.NodeStyle.Rounded, G.NodeStyle.Filled],
        fill_color = Some "aliceblue"
    },
    side_node = { normal_node with {
        fill_color = Some "ivory"
    } },
    error_node = { normal_node with {
        fill_color = Some "mistyrose",
        outline_color = Some "firebrick"
    } },
    success_edge = G.EdgeAttributes {
        colors = Some ["steelblue4"],
        styles = Some [G.EdgeStyle.Bold]
    },
    error_edge = G.EdgeAttributes {
        colors = Some ["firebrick"],
        styles = Some [G.EdgeStyle.Dashed]
    },
    metric_edge = G.EdgeAttributes {
        colors = Some ["darkgreen"],
        styles = Some [G.EdgeStyle.Dotted]
    },
    graph = G.Graph {
        id = Some "compass_ports",
        attributes = G.GraphAttributes {
            rank_direction = Some G.RankDirection.LeftToRight,
            splines = Some G.SplineMode.Polyline
        },
        nodes = {
            receive = normal_node,
            validate = normal_node,
            transform = normal_node,
            persist = normal_node,
            publish = normal_node,
            retry = side_node,
            audit = side_node,
            metrics = side_node,
            reject = error_node
        },
        edges = [
            connection success_edge "receive" G.CompassPoint.East "validate" G.CompassPoint.West,
            connection success_edge "validate" G.CompassPoint.East "transform" G.CompassPoint.West,
            connection success_edge "transform" G.CompassPoint.East "persist" G.CompassPoint.West,
            connection success_edge "persist" G.CompassPoint.East "publish" G.CompassPoint.West,
            connection error_edge "validate" G.CompassPoint.South "reject" G.CompassPoint.North,
            connection error_edge "transform" G.CompassPoint.South "retry" G.CompassPoint.North,
            connection edge_attributes "retry" G.CompassPoint.East "transform" G.CompassPoint.South,
            connection edge_attributes "persist" G.CompassPoint.South "audit" G.CompassPoint.North,
            connection metric_edge "receive" G.CompassPoint.North "metrics" G.CompassPoint.South,
            connection metric_edge "validate" G.CompassPoint.North "metrics" G.CompassPoint.South,
            connection metric_edge "transform" G.CompassPoint.North "metrics" G.CompassPoint.South,
            connection metric_edge "persist" G.CompassPoint.North "metrics" G.CompassPoint.South,
            connection metric_edge "publish" G.CompassPoint.North "metrics" G.CompassPoint.South
        ]
    }
in
    G.render graph G.LayoutEngine.Dot G.RenderFormat.Svg

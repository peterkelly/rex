// Render a branched workflow where each node side has a distinct routing role.
//
// Run from the workspace root:
//   cargo run -p rex-workflow -- --store-path ./store run \
//     rex-workflow/examples/graphviz/render_compass_ports.rex
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
        shape = Some G.ShapeBox,
        styles = Some [G.NodeRounded, G.NodeFilled],
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
        styles = Some [G.EdgeBold]
    },
    error_edge = G.EdgeAttributes {
        colors = Some ["firebrick"],
        styles = Some [G.EdgeDashed]
    },
    metric_edge = G.EdgeAttributes {
        colors = Some ["darkgreen"],
        styles = Some [G.EdgeDotted]
    },
    graph = G.Graph {
        id = Some "compass_ports",
        attributes = G.GraphAttributes {
            rank_direction = Some G.RankLeftToRight,
            splines = Some G.SplinesPolyline
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
            connection success_edge "receive" G.CompassEast "validate" G.CompassWest,
            connection success_edge "validate" G.CompassEast "transform" G.CompassWest,
            connection success_edge "transform" G.CompassEast "persist" G.CompassWest,
            connection success_edge "persist" G.CompassEast "publish" G.CompassWest,
            connection error_edge "validate" G.CompassSouth "reject" G.CompassNorth,
            connection error_edge "transform" G.CompassSouth "retry" G.CompassNorth,
            connection edge_attributes "retry" G.CompassEast "transform" G.CompassSouth,
            connection edge_attributes "persist" G.CompassSouth "audit" G.CompassNorth,
            connection metric_edge "receive" G.CompassNorth "metrics" G.CompassSouth,
            connection metric_edge "validate" G.CompassNorth "metrics" G.CompassSouth,
            connection metric_edge "transform" G.CompassNorth "metrics" G.CompassSouth,
            connection metric_edge "persist" G.CompassNorth "metrics" G.CompassSouth,
            connection metric_edge "publish" G.CompassNorth "metrics" G.CompassSouth
        ]
    }
in
    G.render graph G.LayoutDot G.FormatSvg

// Render two HTML-table nodes connected through named ports.
//
// Run from the workspace root:
//   cargo run -p rex-workflow -- --store-path ./store run \
//     rex-workflow/examples/graphviz/render_ports.rex
import tools.graphviz as G;

fn port (node: String) -> (name: String) -> (compass: G.CompassPoint) -> G.Endpoint = G.Endpoint {
    node = node,
    port = Some (G.Port {
        name = Some name,
        compass = Some compass
    })
};

let
    source_label = G.HtmlLabel "<TABLE BORDER=\"0\" CELLBORDER=\"1\" CELLSPACING=\"0\"><TR><TD PORT=\"input\">input</TD><TD PORT=\"output\">parse</TD></TR></TABLE>",
    target_label = G.HtmlLabel "<TABLE BORDER=\"0\" CELLBORDER=\"1\" CELLSPACING=\"0\"><TR><TD PORT=\"input\">check</TD><TD PORT=\"output\">output</TD></TR></TABLE>",
    graph = G.Graph {
        id = Some "ports",
        nodes = {
            parser = G.NodeAttributes {
                label = Some source_label,
                shape = Some G.ShapePlain
            },
            checker = G.NodeAttributes {
                label = Some target_label,
                shape = Some G.ShapePlain
            }
        },
        edges = [
            G.Edge {
                from = port "parser" "output" G.CompassEast,
                to = port "checker" "input" G.CompassWest,
                attributes = G.EdgeAttributes {
                    label = Some (G.TextLabel "typed AST")
                }
            }
        ]
    }
in
    G.render graph G.LayoutDot G.FormatSvg

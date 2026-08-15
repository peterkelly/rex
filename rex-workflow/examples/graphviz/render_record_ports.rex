// Render a compiler pipeline whose record fields act as named edge ports.
//
// Run from the workspace root:
//   cargo run -p rex-workflow -- --store-path ./store run \
//     rex-workflow/examples/graphviz/render_record_ports.rex
import tools.graphviz as G;

fn named_port (node: String) -> (name: String) -> G.Endpoint = G.Endpoint {
    node = node,
    port = Some (G.Port {
        name = Some name,
        compass = None
    })
};

fn record_node (label: String) -> G.NodeAttributes = G.NodeAttributes {
    label = Some (G.TextLabel label),
    shape = Some G.ShapeRecord
};

fn connection (attributes: G.EdgeAttributes)
    -> (from_node: String) -> (from_port: String)
    -> (to_node: String) -> (to_port: String) -> G.Edge = G.Edge {
        from = named_port from_node from_port,
        to = named_port to_node to_port,
        attributes = attributes
    };

let
    edge_attributes = G.EdgeAttributes {},
    diagnostic_edge = G.EdgeAttributes {
        colors = Some ["firebrick"],
        styles = Some [G.EdgeDashed]
    },
    graph = G.Graph {
        id = Some "record_ports",
        attributes = G.GraphAttributes {
            rank_direction = Some G.RankLeftToRight
        },
        nodes = {
            input = record_node "<source> source | <config> compiler config",
            lexer = record_node "<source> source | <tokens> tokens | <error> lexical error",
            parser = record_node "<tokens> tokens | <config> config | <ast> AST | <error> syntax error",
            checker = record_node "<ast> AST | <typed> typed AST | <error> type error",
            optimizer = record_node "<typed> typed AST | <optimized> optimized AST",
            codegen = record_node "<optimized> optimized AST | <binary> executable",
            diagnostics = record_node "<lex> lexer | <parse> parser | <type> checker | <report> report",
            output = record_node "<binary> artifact | <report> diagnostics"
        },
        edges = [
            connection edge_attributes "input" "source" "lexer" "source",
            connection edge_attributes "input" "config" "parser" "config",
            connection edge_attributes "lexer" "tokens" "parser" "tokens",
            connection edge_attributes "parser" "ast" "checker" "ast",
            connection edge_attributes "checker" "typed" "optimizer" "typed",
            connection edge_attributes "optimizer" "optimized" "codegen" "optimized",
            connection edge_attributes "codegen" "binary" "output" "binary",
            connection diagnostic_edge "lexer" "error" "diagnostics" "lex",
            connection diagnostic_edge "parser" "error" "diagnostics" "parse",
            connection diagnostic_edge "checker" "error" "diagnostics" "type",
            connection diagnostic_edge "diagnostics" "report" "output" "report"
        ]
    }
in
    G.render graph G.LayoutDot G.FormatSvg

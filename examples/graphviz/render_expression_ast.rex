// Parse an arithmetic expression and render its abstract syntax tree (AST).
//
// This example demonstrates how Rex can compute a graph from ordinary data instead of defining
// every Graphviz node and edge by hand. The input is the string:
//
//   12 + 3 * (8 - 2) / 2
//
// The program processes that string in three stages:
//
// 1. Tokenization
//
//    A token is one meaningful piece of the input. For example, `12` becomes
//    `IntegerToken 12`, `+` becomes `PlusToken`, and `(` becomes `LeftParenToken`.
//    Spaces are discarded because they do not affect this expression. Splitting text into tokens
//    keeps the parser from having to reason about individual characters and multi-digit integers
//    at the same time.
//
// 2. Parsing
//
//    Parsing turns the flat token list into a tree that records what the expression means. That
//    tree is called an abstract syntax tree, or AST. "Abstract" means that details needed only in
//    the original text, such as spaces and parentheses, are not stored as tree nodes. Parentheses
//    still affect the tree's shape.
//
//    Each integer becomes an `IntegerExpression` leaf. Each operator becomes a
//    `BinaryExpression` branch with a left operand and a right operand. The parser is divided into
//    three precedence levels:
//
//      - `parse_additive` handles `+` and `-`.
//      - `parse_multiplicative` handles `*` and `/` before addition and subtraction.
//      - `parse_primary` handles integers and parenthesized expressions.
//
//    Calling the higher-precedence parser from the lower-precedence parser is what makes `3 * 8`
//    stay together when it appears inside `12 + 3 * 8`. The `*_tail` functions repeatedly consume
//    operators of the same precedence, making subtraction and division associate from left to
//    right. For the sample input, the AST is equivalent to:
//
//                +
//               / \
//             12   /
//                 / \
//                *   2
//               / \
//              3   -
//                 / \
//                8   2
//
// 3. Graph construction
//
//    `graph_expression` recursively walks the AST. It creates one Graphviz node for each integer
//    or operator and creates two labeled edges for every binary operation: one to its left operand
//    and one to its right operand.
//
//    Node identifiers such as `node_0` are generated while walking the tree. `GraphFragment`
//    carries the next unused number along with the nodes and edges already created. Because these
//    identifiers and graph entries are computed at runtime, they cannot be written as a static Rex
//    dictionary literal. The final `dict_from_entries syntax_tree.nodes` call converts the generated
//    list of `(identifier, attributes)` pairs into the node dictionary required by `G.Graph`.
//
// `tokenize` and `parse_tokens` return `Result` values with readable errors. The sample expression
// is fixed and valid, so the final workflow uses `unwrap` before passing the computed AST to
// Graphviz.
//
// Run from the workspace root:
//   cargo run -p rex --bin rex -- --store-path ./store run \
//     examples/graphviz/render_expression_ast.rex
import tools.graphviz as G;

type Token =
    IntegerToken i32
    | PlusToken
    | MinusToken
    | TimesToken
    | DivideToken
    | LeftParenToken
    | RightParenToken;

type BinaryOperator =
    AddOperator
    | SubtractOperator
    | MultiplyOperator
    | DivideOperator;

type Expression =
    IntegerExpression i32
    | BinaryExpression BinaryOperator Expression Expression;

type ParseStep =
    Parsed Expression (List Token)
    | InvalidSyntax String;

type GraphFragment = GraphFragment {
    next_id: i32,
    root: String,
    nodes: List (String, G.NodeAttributes),
    edges: List G.Edge
};

fn is_digit (character: Char) -> Bool =
    character >= '0' && character <= '9';

fn is_whitespace (character: Char) -> Bool =
    character == ' ' || character == '\t' || character == '\n' || character == '\r';

fn take_digits (characters: List Char) -> (List Char, List Char) =
    match characters with {
        case [] -> ([], []);
        case character::rest ->
            if is_digit character then
                let (digits, remaining) = take_digits rest in
                    (character::digits, remaining)
            else
                ([], characters);
    };

fn prepend_token (token: Token) -> (result: Result (List Token) String)
    -> Result (List Token) String =
    match result with {
        case Ok tokens -> Ok (token::tokens);
        case Err message -> Err message;
    };

fn tokenize_characters (characters: List Char) -> Result (List Token) String =
    match characters with {
        case [] -> Ok [];
        case character::rest ->
            if is_whitespace character then
                tokenize_characters rest
            else if is_digit character then
                let
                    (digits, remaining) = take_digits characters,
                    text = chars_to_string digits,
                    number: Option i32 = parse text
                in
                    match number with {
                        case Some value ->
                            prepend_token (IntegerToken value) (tokenize_characters remaining);
                        case None -> Err ("invalid integer: " + text);
                    }
            else
                if character == '+' then
                    prepend_token PlusToken (tokenize_characters rest)
                else if character == '-' then
                    prepend_token MinusToken (tokenize_characters rest)
                else if character == '*' then
                    prepend_token TimesToken (tokenize_characters rest)
                else if character == '/' then
                    prepend_token DivideToken (tokenize_characters rest)
                else if character == '(' then
                    prepend_token LeftParenToken (tokenize_characters rest)
                else if character == ')' then
                    prepend_token RightParenToken (tokenize_characters rest)
                else
                    Err ("unexpected character: " + show character);
    };

fn tokenize (source: String) -> Result (List Token) String =
    tokenize_characters (string_to_chars source);

fn parse_expression (tokens: List Token) -> ParseStep =
    parse_additive tokens;

fn parse_additive (tokens: List Token) -> ParseStep =
    match parse_multiplicative tokens with {
        case InvalidSyntax message -> InvalidSyntax message;
        case Parsed first remaining -> parse_additive_tail first remaining;
    };

fn parse_additive_tail (left: Expression) -> (tokens: List Token) -> ParseStep =
    match tokens with {
        case PlusToken::rest ->
            match parse_multiplicative rest with {
                case InvalidSyntax message -> InvalidSyntax message;
                case Parsed right remaining ->
                    parse_additive_tail
                        (BinaryExpression AddOperator left right)
                        remaining;
            };
        case MinusToken::rest ->
            match parse_multiplicative rest with {
                case InvalidSyntax message -> InvalidSyntax message;
                case Parsed right remaining ->
                    parse_additive_tail
                        (BinaryExpression SubtractOperator left right)
                        remaining;
            };
        case _ -> Parsed left tokens;
    };

fn parse_multiplicative (tokens: List Token) -> ParseStep =
    match parse_primary tokens with {
        case InvalidSyntax message -> InvalidSyntax message;
        case Parsed first remaining -> parse_multiplicative_tail first remaining;
    };

fn parse_multiplicative_tail (left: Expression) -> (tokens: List Token) -> ParseStep =
    match tokens with {
        case TimesToken::rest ->
            match parse_primary rest with {
                case InvalidSyntax message -> InvalidSyntax message;
                case Parsed right remaining ->
                    parse_multiplicative_tail
                        (BinaryExpression MultiplyOperator left right)
                        remaining;
            };
        case DivideToken::rest ->
            match parse_primary rest with {
                case InvalidSyntax message -> InvalidSyntax message;
                case Parsed right remaining ->
                    parse_multiplicative_tail
                        (BinaryExpression DivideOperator left right)
                        remaining;
            };
        case _ -> Parsed left tokens;
    };

fn parse_primary (tokens: List Token) -> ParseStep =
    match tokens with {
        case [] -> InvalidSyntax "expected an integer or parenthesized expression";
        case IntegerToken value::rest -> Parsed (IntegerExpression value) rest;
        case LeftParenToken::rest ->
            match parse_expression rest with {
                case InvalidSyntax message -> InvalidSyntax message;
                case Parsed expression remaining ->
                    match remaining with {
                        case RightParenToken::after_paren -> Parsed expression after_paren;
                        case _ -> InvalidSyntax "expected ')'";
                    };
            };
        case _ -> InvalidSyntax "expected an integer or '('";
    };

fn parse_tokens (tokens: List Token) -> Result Expression String =
    match parse_expression tokens with {
        case InvalidSyntax message -> Err message;
        case Parsed expression remaining ->
            match remaining with {
                case [] -> Ok expression;
                case _ -> Err "unexpected token after expression";
            };
    };

fn endpoint (node: String) -> G.Endpoint = G.Endpoint {
    node = node,
    port = None
};

fn ast_edge (parent: String) -> (child: String) -> (label: String) -> G.Edge = G.Edge {
    from = endpoint parent,
    to = endpoint child,
    attributes = G.EdgeAttributes {
        label = Some (G.Label.Text label)
    }
};

fn operator_label (operator: BinaryOperator) -> String =
    match operator with {
        case AddOperator -> "+";
        case SubtractOperator -> "-";
        case MultiplyOperator -> "*";
        case DivideOperator -> "/";
    };

fn integer_node (value: i32) -> G.NodeAttributes = G.NodeAttributes {
    label = Some (G.Label.Text (show value)),
    shape = Some G.NodeShape.Box,
    fill_color = Some "lightgoldenrod1"
};

fn operator_node (operator: BinaryOperator) -> G.NodeAttributes = G.NodeAttributes {
    label = Some (G.Label.Text (operator_label operator)),
    shape = Some G.NodeShape.Circle,
    fill_color = Some "lightblue"
};

fn node_id (id: i32) -> String = "node_" + show id;

fn graph_expression (next_id: i32) -> (expression: Expression) -> GraphFragment =
    let root = node_id next_id in
    match expression with {
        case IntegerExpression value -> GraphFragment {
            next_id = next_id + 1,
            root = root,
            nodes = [(root, integer_node value)],
            edges = []
        };
        case BinaryExpression operator left right ->
            let
                left_graph = graph_expression (next_id + 1) left,
                right_graph = graph_expression left_graph.next_id right
            in
                GraphFragment {
                    next_id = right_graph.next_id,
                    root = root,
                    nodes =
                        (root, operator_node operator)
                        :: (left_graph.nodes + right_graph.nodes),
                    edges =
                        [ ast_edge root left_graph.root "left"
                        , ast_edge root right_graph.root "right"
                        ]
                        + left_graph.edges
                        + right_graph.edges
                };
    };

let
    source = "12 + 3 * (8 - 2) / 2",
    tokens = unwrap (tokenize source),
    expression = unwrap (parse_tokens tokens),
    syntax_tree = graph_expression 0 expression,
    graph = G.Graph {
        strict = true,
        id = Some "expression_ast",
        attributes = G.GraphAttributes {
            label = Some (G.Label.Text ("AST for: " + source)),
            background_color = Some "white",
            splines = Some G.SplineMode.Polyline
        },
        node_defaults = G.NodeAttributes {
            styles = Some [G.NodeStyle.Filled]
        },
        edge_defaults = G.EdgeAttributes {
            colors = Some ["slategray4"]
        },
        nodes = dict_from_entries syntax_tree.nodes,
        edges = syntax_tree.edges
    }
in
    G.render graph G.LayoutEngine.Dot G.RenderFormat.Svg

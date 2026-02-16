use rex::{Engine, GasMeter};

fn extract_first_interactive_rex(markdown: &str) -> String {
    let mut lines = markdown.lines();

    while let Some(line) = lines.next() {
        if line.trim() == "```rex,interactive" {
            let mut code = String::new();
            for code_line in &mut lines {
                if code_line.trim() == "```" {
                    return code;
                }
                code.push_str(code_line);
                code.push('\n');
            }
            panic!("unterminated rex,interactive fence");
        }
    }

    panic!("no rex,interactive fence found");
}

async fn assert_demo_ok(name: &str, markdown: &str) {
    let source = extract_first_interactive_rex(markdown);

    let mut engine = Engine::with_prelude(()).unwrap();
    let mut gas = GasMeter::default();
    engine
        .infer_snippet(&source, &mut gas)
        .unwrap_or_else(|err| panic!("{name}: infer error: {err}"));

    let mut gas = GasMeter::default();
    let (_value, ty) = engine
        .eval_snippet(&source, &mut gas)
        .await
        .unwrap_or_else(|err| panic!("{name}: eval error: {err}"));
    assert!(
        !ty.to_string().is_empty(),
        "{name}: eval returned an empty type"
    );
}

#[tokio::test]
async fn demo_factorial() {
    assert_demo_ok(
        "factorial",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../docs/src/demos/factorial.md"
        )),
    )
    .await;
}

#[tokio::test]
async fn demo_fibonacci() {
    assert_demo_ok(
        "fibonacci",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../docs/src/demos/fibonacci.md"
        )),
    )
    .await;
}

#[tokio::test]
async fn demo_merge_sort() {
    assert_demo_ok(
        "merge_sort",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../docs/src/demos/merge_sort.md"
        )),
    )
    .await;
}

#[tokio::test]
async fn demo_binary_search_tree() {
    assert_demo_ok(
        "binary_search_tree",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../docs/src/demos/binary_search_tree.md"
        )),
    )
    .await;
}

#[tokio::test]
async fn demo_expression_evaluator() {
    assert_demo_ok(
        "expression_evaluator",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../docs/src/demos/expression_evaluator.md"
        )),
    )
    .await;
}

#[tokio::test]
async fn demo_dijkstra_lite() {
    assert_demo_ok(
        "dijkstra_lite",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../docs/src/demos/dijkstra_lite.md"
        )),
    )
    .await;
}

#[tokio::test]
async fn demo_knapsack_01() {
    assert_demo_ok(
        "knapsack_01",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../docs/src/demos/knapsack_01.md"
        )),
    )
    .await;
}

#[tokio::test]
async fn demo_union_find() {
    assert_demo_ok(
        "union_find",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../docs/src/demos/union_find.md"
        )),
    )
    .await;
}

#[tokio::test]
async fn demo_prefix_parser() {
    assert_demo_ok(
        "prefix_parser",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../docs/src/demos/prefix_parser.md"
        )),
    )
    .await;
}

#[tokio::test]
async fn demo_topological_sort() {
    assert_demo_ok(
        "topological_sort",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../docs/src/demos/topological_sort.md"
        )),
    )
    .await;
}

#[tokio::test]
async fn demo_n_queens() {
    assert_demo_ok(
        "n_queens",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../docs/src/demos/n_queens.md"
        )),
    )
    .await;
}

use rex::{
    engine::{Engine, Handle, Heap, Value},
    typesystem::{BuiltinTypeId, Type},
};

const DEMO_STACK_SIZE_BYTES: usize = 16 * 1024 * 1024;

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

async fn eval_demo(name: &str, markdown: &str) -> (Heap, Handle, Type) {
    let source = extract_first_interactive_rex(markdown);
    let name = name.to_string();
    std::thread::Builder::new()
        .name(format!("demo_{name}"))
        .stack_size(DEMO_STACK_SIZE_BYTES)
        .spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async move {
                    let mut engine = Engine::with_prelude(()).unwrap();
                    engine
                        .infer_snippet(&source)
                        .unwrap_or_else(|err| panic!("{name}: infer error: {err}"));
                    let heap = engine.heap.clone();
                    let (value, ty) = engine
                        .into_evaluator()
                        .eval_snippet(&source)
                        .await
                        .unwrap_or_else(|err| panic!("{name}: eval error: {err}"));
                    (heap, value, ty)
                })
        })
        .unwrap()
        .join()
        .unwrap()
}

fn tuple_items(value: &Handle) -> Vec<Handle> {
    let Value::Tuple(items) = value.value().unwrap() else {
        panic!("expected tuple, got {}", value.type_name().unwrap());
    };
    items
}

fn list_elements(list: &Handle) -> Vec<Handle> {
    let mut out = Vec::new();
    let mut cur = list.clone();
    loop {
        match cur.value().unwrap() {
            Value::Adt(tag, _args) if tag.as_ref() == "Empty" => return out,
            Value::Adt(tag, args) if tag.as_ref() == "Cons" => {
                assert_eq!(args.len(), 2, "Cons must have exactly two fields");
                out.push(args[0].clone());
                cur = args[1].clone();
            }
            other => panic!("expected list, got {}", other.value_type_name()),
        }
    }
}

fn list_i32_values(handle: &Handle) -> Vec<i32> {
    let elems = list_elements(handle);
    elems
        .iter()
        .map(|p| p.to_rust::<i32>().unwrap())
        .collect::<Vec<_>>()
}

#[tokio::test]
async fn demo_factorial() {
    let (_heap, value, ty) = eval_demo(
        "factorial",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../docs/src/demos/factorial.md"
        )),
    )
    .await;
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    assert_eq!(value.to_rust::<i32>().unwrap(), 720);
}

#[tokio::test]
async fn demo_fibonacci() {
    let (_heap, value, ty) = eval_demo(
        "fibonacci",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../docs/src/demos/fibonacci.md"
        )),
    )
    .await;
    assert_eq!(ty, Type::list(Type::builtin(BuiltinTypeId::I32)));
    assert_eq!(
        list_i32_values(&value),
        vec![0, 1, 1, 2, 3, 5, 8, 13, 21, 34, 55]
    );
}

#[tokio::test]
async fn demo_merge_sort() {
    let (_heap, value, ty) = eval_demo(
        "merge_sort",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../docs/src/demos/merge_sort.md"
        )),
    )
    .await;
    assert_eq!(ty, Type::list(Type::builtin(BuiltinTypeId::I32)));
    assert_eq!(list_i32_values(&value), vec![1, 2, 3, 4, 5, 6, 7, 8, 9]);
}

#[tokio::test]
async fn demo_binary_search_tree() {
    let (_heap, value, ty) = eval_demo(
        "binary_search_tree",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../docs/src/demos/binary_search_tree.md"
        )),
    )
    .await;
    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::builtin(BuiltinTypeId::I32),
            Type::builtin(BuiltinTypeId::Bool),
            Type::builtin(BuiltinTypeId::Bool),
        ])
    );
    let items = tuple_items(&value);
    assert_eq!(items.len(), 3);
    assert_eq!(items[0].to_rust::<i32>().unwrap(), 6);
    assert!(items[1].to_rust::<bool>().unwrap());
    assert!(!items[2].to_rust::<bool>().unwrap());
}

#[tokio::test]
async fn demo_expression_evaluator() {
    let (_heap, value, ty) = eval_demo(
        "expression_evaluator",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../docs/src/demos/expression_evaluator.md"
        )),
    )
    .await;
    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::builtin(BuiltinTypeId::I32),
            Type::builtin(BuiltinTypeId::I32),
            Type::builtin(BuiltinTypeId::I32),
        ])
    );
    let items = tuple_items(&value);
    assert_eq!(items.len(), 3);
    assert_eq!(items[0].to_rust::<i32>().unwrap(), 14);
    assert_eq!(items[1].to_rust::<i32>().unwrap(), 3);
    assert_eq!(items[2].to_rust::<i32>().unwrap(), 14);
}

#[tokio::test]
async fn demo_dijkstra_lite() {
    let (_heap, value, ty) = eval_demo(
        "dijkstra_lite",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../docs/src/demos/dijkstra_lite.md"
        )),
    )
    .await;
    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::builtin(BuiltinTypeId::I32),
            Type::builtin(BuiltinTypeId::I32)
        ])
    );
    let items = tuple_items(&value);
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].to_rust::<i32>().unwrap(), 7);
    assert_eq!(items[1].to_rust::<i32>().unwrap(), 5);
}

#[tokio::test]
async fn demo_knapsack_01() {
    let (_heap, value, ty) = eval_demo(
        "knapsack_01",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../docs/src/demos/knapsack_01.md"
        )),
    )
    .await;
    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::builtin(BuiltinTypeId::I32),
            Type::builtin(BuiltinTypeId::I32)
        ])
    );
    let items = tuple_items(&value);
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].to_rust::<i32>().unwrap(), 8);
    assert_eq!(items[1].to_rust::<i32>().unwrap(), 12);
}

#[tokio::test]
async fn demo_union_find() {
    let (_heap, value, ty) = eval_demo(
        "union_find",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../docs/src/demos/union_find.md"
        )),
    )
    .await;
    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::builtin(BuiltinTypeId::Bool),
            Type::builtin(BuiltinTypeId::Bool),
            Type::builtin(BuiltinTypeId::I32),
            Type::builtin(BuiltinTypeId::I32),
        ])
    );
    let items = tuple_items(&value);
    assert_eq!(items.len(), 4);
    assert!(items[0].to_rust::<bool>().unwrap());
    assert!(!items[1].to_rust::<bool>().unwrap());
    assert_eq!(items[2].to_rust::<i32>().unwrap(), 0);
    assert_eq!(items[3].to_rust::<i32>().unwrap(), 3);
}

#[tokio::test]
async fn demo_prefix_parser() {
    let (_heap, value, ty) = eval_demo(
        "prefix_parser",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../docs/src/demos/prefix_parser.md"
        )),
    )
    .await;
    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::builtin(BuiltinTypeId::I32),
            Type::builtin(BuiltinTypeId::Bool),
            Type::builtin(BuiltinTypeId::I32),
            Type::builtin(BuiltinTypeId::Bool),
        ])
    );
    let items = tuple_items(&value);
    assert_eq!(items.len(), 4);
    assert_eq!(items[0].to_rust::<i32>().unwrap(), 14);
    assert!(items[1].to_rust::<bool>().unwrap());
    assert_eq!(items[2].to_rust::<i32>().unwrap(), 7);
    assert!(items[3].to_rust::<bool>().unwrap());
}

#[tokio::test]
async fn demo_topological_sort() {
    let (_heap, value, ty) = eval_demo(
        "topological_sort",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../docs/src/demos/topological_sort.md"
        )),
    )
    .await;
    let ty_str = ty.to_string();
    assert!(
        ty_str.starts_with("(List "),
        "topological_sort: expected list result type, got {ty_str}"
    );
    assert!(
        ty_str.ends_with(".Node)"),
        "topological_sort: expected element type ending in .Node, got {ty_str}"
    );
    let elems = list_elements(&value);
    assert_eq!(elems.len(), 4);
    for (idx, expected_tag) in ["A", "B", "C", "D"].iter().enumerate() {
        let Value::Adt(tag, args) = elems[idx].value().unwrap() else {
            panic!("expected ADT constructor");
        };
        assert_eq!(tag.as_ref(), *expected_tag, "unexpected constructor tag");
        assert!(args.is_empty(), "{expected_tag} should have no payload");
    }
}

#[test]
fn demo_n_queens() {
    let markdown = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../docs/src/demos/n_queens.md"
    ));
    let (_heap, value, ty) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(eval_demo("n_queens", markdown));
    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::builtin(BuiltinTypeId::I32),
            Type::builtin(BuiltinTypeId::I32)
        ])
    );
    let items = tuple_items(&value);
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].to_rust::<i32>().unwrap(), 2);
    assert_eq!(items[1].to_rust::<i32>().unwrap(), 10);
}

use rex::{Engine, GasMeter, Heap, Pointer, Type, Value};

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

async fn eval_demo(name: &str, markdown: &str) -> (Heap, Pointer, Type) {
    let source = extract_first_interactive_rex(markdown);

    let mut engine = Engine::with_prelude(()).unwrap();
    let mut gas = GasMeter::default();
    engine
        .infer_snippet(&source, &mut gas)
        .unwrap_or_else(|err| panic!("{name}: infer error: {err}"));

    let mut gas = GasMeter::default();
    let (value, ty) = engine
        .eval_snippet(&source, &mut gas)
        .await
        .unwrap_or_else(|err| panic!("{name}: eval error: {err}"));
    (engine.into_heap(), value, ty)
}

fn list_elements(heap: &rex::Heap, list: &Pointer) -> Vec<Pointer> {
    let mut out = Vec::new();
    let mut cur = *list;
    loop {
        let val = heap.get(&cur).unwrap();
        match val.as_ref() {
            Value::Adt(tag, args) if tag.as_ref() == "Empty" => return out,
            Value::Adt(tag, args) if tag.as_ref() == "Cons" => {
                assert_eq!(args.len(), 2, "Cons must have exactly two fields");
                out.push(args[0]);
                cur = args[1];
            }
            other => panic!("expected list, got {}", other.value_type_name()),
        }
    }
}

fn list_i32_values(heap: &rex::Heap, ptr: &Pointer) -> Vec<i32> {
    let elems = list_elements(heap, ptr);
    elems
        .iter()
        .map(|p| heap.pointer_as_i32(p).unwrap())
        .collect::<Vec<_>>()
}

#[tokio::test]
async fn demo_factorial() {
    let (heap, value, ty) = eval_demo(
        "factorial",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../docs/src/demos/factorial.md"
        )),
    )
    .await;
    assert_eq!(ty, Type::con("i32", 0));
    assert_eq!(heap.pointer_as_i32(&value).unwrap(), 720);
}

#[tokio::test]
async fn demo_fibonacci() {
    let (heap, value, ty) = eval_demo(
        "fibonacci",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../docs/src/demos/fibonacci.md"
        )),
    )
    .await;
    assert_eq!(ty, Type::list(Type::con("i32", 0)));
    assert_eq!(
        list_i32_values(&heap, &value),
        vec![0, 1, 1, 2, 3, 5, 8, 13, 21, 34, 55]
    );
}

#[tokio::test]
async fn demo_merge_sort() {
    let (heap, value, ty) = eval_demo(
        "merge_sort",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../docs/src/demos/merge_sort.md"
        )),
    )
    .await;
    assert_eq!(ty, Type::list(Type::con("i32", 0)));
    assert_eq!(
        list_i32_values(&heap, &value),
        vec![1, 2, 3, 4, 5, 6, 7, 8, 9]
    );
}

#[tokio::test]
async fn demo_binary_search_tree() {
    let (heap, value, ty) = eval_demo(
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
            Type::con("i32", 0),
            Type::con("bool", 0),
            Type::con("bool", 0),
        ])
    );
    let items = heap.pointer_as_tuple(&value).unwrap();
    assert_eq!(items.len(), 3);
    assert_eq!(heap.pointer_as_i32(&items[0]).unwrap(), 6);
    assert!(heap.pointer_as_bool(&items[1]).unwrap());
    assert!(!heap.pointer_as_bool(&items[2]).unwrap());
}

#[tokio::test]
async fn demo_expression_evaluator() {
    let (heap, value, ty) = eval_demo(
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
            Type::con("i32", 0),
            Type::con("i32", 0),
            Type::con("i32", 0),
        ])
    );
    let items = heap.pointer_as_tuple(&value).unwrap();
    assert_eq!(items.len(), 3);
    assert_eq!(heap.pointer_as_i32(&items[0]).unwrap(), 14);
    assert_eq!(heap.pointer_as_i32(&items[1]).unwrap(), 3);
    assert_eq!(heap.pointer_as_i32(&items[2]).unwrap(), 14);
}

#[tokio::test]
async fn demo_dijkstra_lite() {
    let (heap, value, ty) = eval_demo(
        "dijkstra_lite",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../docs/src/demos/dijkstra_lite.md"
        )),
    )
    .await;
    assert_eq!(
        ty,
        Type::tuple(vec![Type::con("i32", 0), Type::con("i32", 0)])
    );
    let items = heap.pointer_as_tuple(&value).unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(heap.pointer_as_i32(&items[0]).unwrap(), 7);
    assert_eq!(heap.pointer_as_i32(&items[1]).unwrap(), 5);
}

#[tokio::test]
async fn demo_knapsack_01() {
    let (heap, value, ty) = eval_demo(
        "knapsack_01",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../docs/src/demos/knapsack_01.md"
        )),
    )
    .await;
    assert_eq!(
        ty,
        Type::tuple(vec![Type::con("i32", 0), Type::con("i32", 0)])
    );
    let items = heap.pointer_as_tuple(&value).unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(heap.pointer_as_i32(&items[0]).unwrap(), 8);
    assert_eq!(heap.pointer_as_i32(&items[1]).unwrap(), 12);
}

#[tokio::test]
async fn demo_union_find() {
    let (heap, value, ty) = eval_demo(
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
            Type::con("bool", 0),
            Type::con("bool", 0),
            Type::con("i32", 0),
            Type::con("i32", 0),
        ])
    );
    let items = heap.pointer_as_tuple(&value).unwrap();
    assert_eq!(items.len(), 4);
    assert!(heap.pointer_as_bool(&items[0]).unwrap());
    assert!(!heap.pointer_as_bool(&items[1]).unwrap());
    assert_eq!(heap.pointer_as_i32(&items[2]).unwrap(), 0);
    assert_eq!(heap.pointer_as_i32(&items[3]).unwrap(), 3);
}

#[tokio::test]
async fn demo_prefix_parser() {
    let (heap, value, ty) = eval_demo(
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
            Type::con("i32", 0),
            Type::con("bool", 0),
            Type::con("i32", 0),
            Type::con("bool", 0),
        ])
    );
    let items = heap.pointer_as_tuple(&value).unwrap();
    assert_eq!(items.len(), 4);
    assert_eq!(heap.pointer_as_i32(&items[0]).unwrap(), 14);
    assert!(heap.pointer_as_bool(&items[1]).unwrap());
    assert_eq!(heap.pointer_as_i32(&items[2]).unwrap(), 7);
    assert!(heap.pointer_as_bool(&items[3]).unwrap());
}

#[tokio::test]
async fn demo_topological_sort() {
    let (heap, value, ty) = eval_demo(
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
    let elems = list_elements(&heap, &value);
    assert_eq!(elems.len(), 4);
    for (idx, expected_tag) in ["A", "B", "C", "D"].iter().enumerate() {
        let value = heap.get(&elems[idx]).unwrap();
        let Value::Adt(tag, args) = value.as_ref() else {
            panic!("expected ADT constructor");
        };
        assert!(
            tag.as_ref().ends_with(&format!(".{expected_tag}")),
            "expected constructor ending with .{expected_tag}, got {}",
            tag.as_ref()
        );
        assert!(args.is_empty(), "{expected_tag} should have no payload");
    }
}

#[tokio::test]
async fn demo_n_queens() {
    let (heap, value, ty) = eval_demo(
        "n_queens",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../docs/src/demos/n_queens.md"
        )),
    )
    .await;
    assert_eq!(
        ty,
        Type::tuple(vec![Type::con("i32", 0), Type::con("i32", 0)])
    );
    let items = heap.pointer_as_tuple(&value).unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(heap.pointer_as_i32(&items[0]).unwrap(), 2);
    assert_eq!(heap.pointer_as_i32(&items[1]).unwrap(), 10);
}

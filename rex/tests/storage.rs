use blake3::Hash;
use rex::{
    engine::{Builder, CompileOptions},
    json::{json_to_main_inputs, rex_to_json},
    modules::std::storage::{StateStore, storage_module},
    parser::parse as parse_rex,
    storage::{EntryKind, Store},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, str::FromStr};

#[derive(Clone)]
struct TestState(Store);

impl StateStore for TestState {
    fn store(&self) -> Option<&Store> {
        Some(&self.0)
    }
}

async fn eval_storage(source: &str, inputs: Option<Value>, store: Store) -> Value {
    let program = parse_rex(source).unwrap();
    let mut builder = Builder::with_prelude(TestState(store)).unwrap();
    builder
        .inject_module(storage_module::<TestState>().unwrap())
        .unwrap();
    let compiler = builder.build_compiler();
    let (compiled, evaluator) = compiler
        .compile_program(
            &program,
            CompileOptions::for_module("test.storage").unwrap(),
        )
        .await
        .unwrap();
    let result_type = compiled.result_type().clone();
    let type_system = evaluator.type_system();
    let inputs = json_to_main_inputs(
        inputs.unwrap_or_else(|| json!({})),
        compiled.main_signature(),
        type_system.as_ref(),
    )
    .unwrap();
    let value = evaluator.run(compiled, inputs).await.unwrap();
    rex_to_json(&value, &result_type, type_system.as_ref()).unwrap()
}

#[tokio::test]
async fn store_get_functions() {
    let store = Store::new_in_memory();
    let text_hash = store.put(b"hello").await.unwrap();
    let binary_hash = store.put([0, 1, 2, 3, 4, 5, 255]).await.unwrap();
    let inner_tree_hash = store
        .put_tree(BTreeMap::from_iter(
            vec![
                ("text.txt".to_string(), (EntryKind::Blob, text_hash)),
                ("binary.bin".to_string(), (EntryKind::Blob, binary_hash)),
            ]
            .into_iter(),
        ))
        .await
        .unwrap();

    let source = r#"
        import std.storage (*);

        fn main (root: Hash) -> Dict Entry =
            get_tree root;
    "#;

    let inputs = json!({
        "root": inner_tree_hash.to_hex().to_string(),
    });
    let result = eval_storage(source, Some(inputs), store.clone()).await;
    assert_eq!(
        result,
        json!({
            "binary.bin": {
                "hash": "0f01fd898c3fb65a7982c9c15dd284f8b22d1c1978dbfcc21d072dd1ddc1a085",
                "kind": "Blob",
                "size": 7
            },
            "text.txt": {
                "hash": "ea8f163db38682925e4491c5e58d4bb3506ef8c14eb78a86e908c5624a67200f",
                "kind": "Blob",
                "size": 5
            }
        })
    );

    let outer_tree_hash = store
        .put_tree(BTreeMap::from_iter(vec![(
            "inner".to_string(),
            (EntryKind::Tree, inner_tree_hash),
        )]))
        .await
        .unwrap();

    let inputs = json!({
        "root": outer_tree_hash.to_hex().to_string(),
    });
    let result = eval_storage(source, Some(inputs), store.clone()).await;
    assert_eq!(
        result,
        json!({
            "inner": {
                "hash": "4bdd4206ef1f4ef934c49b05654e69740ef0b3b359af356044544e4e0e18497f",
                "kind": "Tree",
                "size": 235
            }
        })
    );
}

#[tokio::test]
async fn store_put_functions() {
    let source = r#"
        import std.storage(*);

        let
            text_hash = put_string "hello",
            binary_hash = put_bytes [0, 1, 2, 3, 4, 5, 255],
            inner_dir_hash =
                put_tree
                    (dict_from_entries
                        [
                            ("text.txt", (Blob, text_hash)),
                            ("binary.bin", (Blob, binary_hash))
                        ]),
            outer_tree_hash =
                put_tree
                    (dict_from_entries
                        [
                            ("inner", (Tree, inner_dir_hash))
                        ])
        in
            outer_tree_hash
    "#;
    let store = Store::new_in_memory();
    let result = eval_storage(source, None, store.clone()).await;
    assert_eq!(
        result,
        "69907f3f9da275ec2c53991770806db62c1ccb9011bcc643467fd865314bb29f"
    );

    let outer_tree_hash = Hash::from_hex(result.as_str().unwrap()).unwrap();
    let outer_tree_content = store.get(outer_tree_hash).await.unwrap();
    let outer_tree_content: Value = serde_json::from_slice(&outer_tree_content).unwrap();

    assert_eq!(
        outer_tree_content,
        json!({
            "inner": {
                "hash": "4bdd4206ef1f4ef934c49b05654e69740ef0b3b359af356044544e4e0e18497f",
                "kind": "tree",
                "size": 235
            }
        })
    );

    let inner_tree_hash =
        Hash::from_hex("4bdd4206ef1f4ef934c49b05654e69740ef0b3b359af356044544e4e0e18497f").unwrap();
    let inner_tree_content = store.get(inner_tree_hash).await.unwrap();
    let inner_tree_json: Value = serde_json::from_slice(&inner_tree_content).unwrap();

    assert_eq!(
        inner_tree_json,
        json!({
            "binary.bin": {
                "hash": "0f01fd898c3fb65a7982c9c15dd284f8b22d1c1978dbfcc21d072dd1ddc1a085",
                "kind": "blob",
                "size": 7
            },
            "text.txt": {
                "hash": "ea8f163db38682925e4491c5e58d4bb3506ef8c14eb78a86e908c5624a67200f",
                "kind": "blob",
                "size": 5
            }
        })
    );

    let binary_size = store
        .size(
            Hash::from_str("0f01fd898c3fb65a7982c9c15dd284f8b22d1c1978dbfcc21d072dd1ddc1a085")
                .unwrap(),
        )
        .await
        .unwrap();
    let text_size = store
        .size(
            Hash::from_str("ea8f163db38682925e4491c5e58d4bb3506ef8c14eb78a86e908c5624a67200f")
                .unwrap(),
        )
        .await
        .unwrap();
    let inner_size = store
        .size(
            Hash::from_str("4bdd4206ef1f4ef934c49b05654e69740ef0b3b359af356044544e4e0e18497f")
                .unwrap(),
        )
        .await
        .unwrap();
    let inner_total_size = binary_size + text_size + inner_size;
    assert_eq!(inner_total_size, 235);
}

use blake3::Hash;
use rex::engine::{EngineError, Module};
use rex::storage::{Entry, EntryKind, Store};
use std::collections::BTreeMap;

/// Provides the content-addressed store used by the standard storage module.
pub trait StateStore {
    /// Return the configured store, or `None` when storage is unavailable.
    fn store(&self) -> Option<&Store>;
}

/// Build the standard storage module for a compatible host state.
pub fn storage_module<T>() -> Result<Module<T>, EngineError>
where
    T: StateStore + Clone + Send + Sync + 'static,
{
    storage_api::rex_module::<T>()
}

fn configured_store<T: StateStore>(state: &T) -> Result<&Store, EngineError> {
    state
        .store()
        .ok_or_else(|| EngineError::from("Storage not configured"))
}

/// Content-addressed storage for immutable blobs and directory trees.
///
/// Hashes returned by `put_string`, `put_bytes`, and `put_tree` can be passed to the corresponding
/// `get_*` functions and to workflow tools. Equal content produces the same hash. Trees map child
/// names to immutable blob or tree hashes and can be traversed recursively with `get_tree`.
#[rex::module(name = "std.storage")]
mod storage_api {
    use super::*;

    /// Load a UTF-8 string from content-addressed storage.
    ///
    /// `hash` must identify a stored blob whose bytes are valid UTF-8. Use `get_bytes` for arbitrary
    /// binary content.
    #[rex::export]
    async fn get_string<T>(state: T, hash: Hash) -> Result<String, EngineError>
    where
        T: StateStore,
    {
        let data = configured_store(&state)?
            .get(hash)
            .await
            .map_err(|x| format!("get: {}", x))?;
        let s = String::from_utf8(data).map_err(|_| "Invalid UTF-8 string".to_string())?;
        Ok(s)
    }

    /// Load the raw bytes of a blob from content-addressed storage.
    ///
    /// `hash` must identify a stored blob. Tree hashes should be read with `get_tree`.
    #[rex::export]
    async fn get_bytes<T>(state: T, hash: Hash) -> Result<Vec<u8>, EngineError>
    where
        T: StateStore,
    {
        let data = configured_store(&state)?
            .get(hash)
            .await
            .map_err(|x| format!("get: {}", x))?;
        Ok(data)
    }

    /// Load the immediate entries of a directory tree from content-addressed storage.
    ///
    /// `hash` must identify a stored tree. The returned dictionary maps each child name to its hash,
    /// kind, and byte size; nested trees can be traversed with additional `get_tree` calls.
    #[rex::export]
    async fn get_tree<T>(state: T, hash: Hash) -> Result<BTreeMap<String, Entry>, EngineError>
    where
        T: StateStore,
    {
        let entries = configured_store(&state)?
            .get_tree(hash)
            .await
            .map_err(|x| format!("get_tree: {}", x))?;
        Ok(entries)
    }

    /// Store a UTF-8 string as an immutable blob and return its content hash.
    ///
    /// Storing identical `data` returns the same hash.
    #[rex::export]
    async fn put_string<T>(state: T, data: String) -> Result<Hash, EngineError>
    where
        T: StateStore,
    {
        configured_store(&state)?
            .put(data.as_bytes())
            .await
            .map_err(|e| e.to_string().into())
    }

    /// Store arbitrary bytes as an immutable blob and return their content hash.
    ///
    /// Storing identical `data` returns the same hash.
    #[rex::export]
    async fn put_bytes<T>(state: T, data: Vec<u8>) -> Result<Hash, EngineError>
    where
        T: StateStore,
    {
        configured_store(&state)?
            .put(data)
            .await
            .map_err(|e| e.to_string().into())
    }

    /// Store an immutable directory tree and return its content hash.
    ///
    /// `entries` maps each child name to an `(EntryKind, Hash)` pair. Every referenced hash must
    /// already exist with the declared kind. The resulting tree is deterministic for the same map.
    #[rex::export]
    async fn put_tree<T>(
        state: T,
        entries: BTreeMap<String, (EntryKind, Hash)>,
    ) -> Result<Hash, EngineError>
    where
        T: StateStore,
    {
        configured_store(&state)?
            .put_tree(entries)
            .await
            .map_err(|e| e.to_string().into())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        struct UnconfiguredState;

        impl StateStore for UnconfiguredState {
            fn store(&self) -> Option<&Store> {
                None
            }
        }

        fn assert_storage_not_configured<T>(result: Result<T, EngineError>) {
            let Err(error) = result else {
                panic!("expected unconfigured storage to fail");
            };
            assert_eq!(error.to_string(), "Storage not configured");
        }

        #[tokio::test]
        async fn all_functions_reject_unconfigured_storage() {
            let hash = blake3::hash(b"missing");

            assert_storage_not_configured(get_string(UnconfiguredState, hash).await);
            assert_storage_not_configured(get_bytes(UnconfiguredState, hash).await);
            assert_storage_not_configured(get_tree(UnconfiguredState, hash).await);
            assert_storage_not_configured(put_string(UnconfiguredState, String::new()).await);
            assert_storage_not_configured(put_bytes(UnconfiguredState, Vec::new()).await);
            assert_storage_not_configured(put_tree(UnconfiguredState, BTreeMap::new()).await);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{StateStore, configured_store, storage_module};
    use crate::{run::eval_rex, state::State};
    use blake3::Hash;
    use rex::storage::{EntryKind, Store};
    use serde_json::{Value, json};
    use std::{collections::BTreeMap, str::FromStr};

    #[derive(Clone)]
    struct OptionalStoreState(Option<Store>);

    impl StateStore for OptionalStoreState {
        fn store(&self) -> Option<&Store> {
            self.0.as_ref()
        }
    }

    #[test]
    fn storage_module_supports_generic_optional_store_state() {
        let module = storage_module::<OptionalStoreState>().unwrap();
        assert_eq!(module.exports().len(), 6);

        let state = OptionalStoreState(None);
        let Err(error) = configured_store(&state) else {
            panic!("expected unconfigured storage to fail");
        };
        assert_eq!(error.to_string(), "Storage not configured");
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

        let state = State::local(store.clone());
        let inputs = json!({
            "root": inner_tree_hash.to_hex().to_string(),
        });
        let result = eval_rex(source, Some(inputs), state.clone()).await.unwrap();
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
        let result = eval_rex(source, Some(inputs), state).await.unwrap();
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
        let state = State::local(store.clone());
        let result = eval_rex(source, None, state).await.unwrap();
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
            Hash::from_hex("4bdd4206ef1f4ef934c49b05654e69740ef0b3b359af356044544e4e0e18497f")
                .unwrap();
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
            }
            )
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
}

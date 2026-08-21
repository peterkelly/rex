use crate::engine::{EngineError, Module};
use crate::storage::{Entry, EntryKind, Store};
use blake3::Hash;
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
    use crate::storage::Store;

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
}

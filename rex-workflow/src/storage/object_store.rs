use crate::storage::store::{StoreFuture, StoreImpl};
use blake3::Hash;
use object_store::{ObjectStore, ObjectStoreExt, PutPayload, path::Path};

/// A content-addressable store backed by an [`ObjectStore`].
///
/// The prefix is concatenated directly with the hexadecimal BLAKE3 hash to
/// determine an object's location. It may therefore be either a path prefix
/// such as `"rex/objects/"` or a filename prefix such as `"rex-"`.
pub struct ObjectStoreImpl<T> {
    store: T,
    prefix: String,
}

impl<T> ObjectStoreImpl<T> {
    pub fn new(store: T, prefix: impl Into<String>) -> Self {
        Self {
            store,
            prefix: prefix.into(),
        }
    }

    fn object_path(&self, hash: Hash) -> Path {
        Path::from(format!("{}{}", self.prefix, hash.to_hex()))
    }
}

impl<T: ObjectStore> StoreImpl for ObjectStoreImpl<T> {
    fn get(&self, hash: Hash) -> StoreFuture<'_, Vec<u8>> {
        Box::pin(async move {
            let result = self.store.get(&self.object_path(hash)).await?;
            Ok(result.bytes().await?.to_vec())
        })
    }

    fn size(&self, hash: Hash) -> StoreFuture<'_, u64> {
        Box::pin(async move { Ok(self.store.head(&self.object_path(hash)).await?.size) })
    }

    fn put<'a>(&'a self, data: &'a [u8]) -> StoreFuture<'a, Hash> {
        let hash = blake3::hash(data);
        let location = self.object_path(hash);
        let payload = PutPayload::from(data.to_vec());

        Box::pin(async move {
            // ObjectStoreExt::put is contractually atomic: readers see either the
            // previous complete object or this complete payload. Since the
            // location is content-addressed, concurrent writers use identical
            // payloads. Overwrite mode also works for stores such as HTTP/WebDAV
            // that do not implement conditional creates or atomic renames.
            self.store.put(&location, payload).await?;
            Ok(hash)
        })
    }
}

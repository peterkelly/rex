use crate::storage::{
    entry::{Entry, EntryKind},
    filesystem::FilesystemStoreImpl,
    memory::MemoryStoreImpl,
    object_store::ObjectStoreImpl,
};
use blake3::Hash;
use serde_json::error::Category;
use std::{
    collections::BTreeMap,
    future::Future,
    io::{Error, ErrorKind},
    path::PathBuf,
    pin::Pin,
    str::FromStr,
    sync::Arc,
};

pub enum StoreError {
    NotFound,
    Other(String),
}

pub type StoreFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, Error>> + Send + 'a>>;

pub trait StoreImpl: Send + Sync {
    fn get(&self, hash: Hash) -> StoreFuture<'_, Vec<u8>>;
    fn size(&self, hash: Hash) -> StoreFuture<'_, u64>;
    fn put<'a>(&'a self, data: &'a [u8]) -> StoreFuture<'a, Hash>;
}

/// Content-addressable storage for Rex
///
/// Because rex is based on functional programming and immutable data structures, we
/// expose a storage API based on content-addressable object storage. Every object
/// is identified by its BLAKE3 hash.
///
/// Hierarchy is represented using trees. These are dictionaries mapping filenames to entries,
/// where each entry records the kind, hash, and size. The kind is either `Blob` for opaque
/// binary objects (files) or `Tree` for trees. One can traverse through a tree structure by
/// starting at a given root hash, using get_tree to retrieve its contents, and recursing
/// through child entries whose kind is Tree.
///
/// The primary reason for using content-addressible storage is to give Rex a way of reading
/// and writing files that does not involve mutation. Calling one of the put methods does not
/// have any side effects if an object with the same content already exists, because the result
/// is a hash, and that's always going to be the same for a given piece of content.
///
/// Rex can create new files and directories (called blobs and trees for consistency with git),
/// it just can't modify existing ones. If modification-like behaviour is desired, a new blob
/// can be created with the updated content, and the tree structure containing it can be
/// recursively re-created using the new hash of the blob and the new hashes of inner trees.
#[derive(Clone)]
pub struct Store {
    inner: Arc<dyn StoreImpl>,
}

impl Store {
    pub fn new_with_filesystem(path: PathBuf) -> Store {
        Store::from_impl(FilesystemStoreImpl::new(path))
    }

    pub fn new_in_memory() -> Store {
        Store::from_impl(MemoryStoreImpl::new())
    }

    /// Create a store backed by an [`object_store::ObjectStore`].
    ///
    /// `prefix` is concatenated directly with each object's hexadecimal hash to
    /// form its location. For example, `"rex/objects/"` stores an object at
    /// `rex/objects/<hash>`.
    pub fn new_with_object_store<T>(store: T, prefix: impl Into<String>) -> Store
    where
        T: object_store::ObjectStore,
    {
        Store::from_impl(ObjectStoreImpl::new(store, prefix))
    }

    pub fn from_impl(store: impl StoreImpl + 'static) -> Store {
        Store {
            inner: Arc::new(store),
        }
    }

    pub async fn get(&self, hash: Hash) -> Result<Vec<u8>, Error> {
        self.inner.get(hash).await
    }

    pub async fn size(&self, hash: Hash) -> Result<u64, Error> {
        self.inner.size(hash).await
    }

    pub async fn put(&self, data: impl AsRef<[u8]>) -> Result<Hash, Error> {
        self.inner.put(data.as_ref()).await
    }

    pub async fn resolve_path(&self, path: impl AsRef<str>) -> Result<Hash, Error> {
        let components: Vec<String> = path.as_ref().split('/').map(|x| x.to_string()).collect();

        if components.is_empty() {
            return Err(Error::new(ErrorKind::NotFound, "Empty path"));
        }

        let mut hash = match blake3::Hash::from_str(&components[0]) {
            Ok(hash) => hash,
            Err(e) => {
                return Err(Error::new(
                    ErrorKind::NotFound,
                    format!("Invalid hash: {}", e),
                ));
            }
        };

        for component in components.iter().skip(1) {
            let dir_entries = self.get_tree(hash).await?;
            let Some(entry) = dir_entries.get(component) else {
                let full = format!("Path not found: {}", path.as_ref(),);
                return Err(Error::new(ErrorKind::NotFound, full));
            };
            hash = entry.hash;
        }
        Ok(hash)
    }

    pub async fn get_tree(&self, hash: Hash) -> Result<BTreeMap<String, Entry>, Error> {
        let data = self.get(hash).await?;
        match serde_json::from_slice::<BTreeMap<String, Entry>>(&data) {
            Ok(entries) => Ok(entries),
            Err(e) => match e.classify() {
                Category::Io => Err(e.into()),
                Category::Syntax | Category::Data | Category::Eof => Err(Error::new(
                    ErrorKind::NotADirectory,
                    format!("Not a tree: {}", hash),
                )),
            },
        }
    }

    pub async fn put_tree(
        &self,
        creations: BTreeMap<String, (EntryKind, Hash)>,
    ) -> Result<Hash, Error> {
        let mut entries: BTreeMap<String, Entry> = BTreeMap::new();
        for (name, (kind, hash)) in creations.into_iter() {
            let size = match kind {
                EntryKind::Tree => {
                    let data = self.get(hash).await?;
                    let mut total_size = data.len() as u64;
                    let entries: BTreeMap<String, Entry> = serde_json::from_slice(&data)?;
                    for entry in entries.values() {
                        total_size += entry.size;
                    }
                    total_size
                }
                EntryKind::Blob => self.size(hash).await?,
            };

            entries.insert(name, Entry { kind, hash, size });
        }

        // Convert to Value first to make sure fields are written alphabetically
        let value = serde_json::to_value(entries)?;
        let data = serde_json::to_vec(&value)?;
        let hash = self.put(data).await?;
        Ok(hash)
    }
}

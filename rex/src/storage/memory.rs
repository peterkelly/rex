use super::store::{StoreFuture, StoreImpl};
use blake3::Hash;
use std::{
    collections::BTreeMap,
    io::{Error, ErrorKind},
    sync::{Arc, Mutex},
};

#[derive(Default)]
pub struct MemoryStoreImpl {
    objects: Arc<Mutex<BTreeMap<[u8; 32], Vec<u8>>>>,
}

impl MemoryStoreImpl {
    pub fn new() -> Self {
        Self::default()
    }
}

impl StoreImpl for MemoryStoreImpl {
    fn get(&self, hash: Hash) -> StoreFuture<'_, Vec<u8>> {
        Box::pin(async move {
            let objects = self
                .objects
                .lock()
                .map_err(|_| Error::other("in-memory store lock poisoned"))?;
            match objects.get(hash.as_bytes()) {
                Some(data) => Ok(data.clone()),
                None => Err(Error::new(
                    ErrorKind::NotFound,
                    format!("Object not found: {}", hash),
                )),
            }
        })
    }

    fn put<'a>(&'a self, data: &'a [u8]) -> StoreFuture<'a, Hash> {
        let hash = blake3::hash(data);
        Box::pin(async move {
            let mut objects = self
                .objects
                .lock()
                .map_err(|_| Error::other("in-memory store lock poisoned"))?;
            objects.insert(*hash.as_bytes(), data.to_vec());
            Ok(hash)
        })
    }
}

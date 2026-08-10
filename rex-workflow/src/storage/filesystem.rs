use crate::storage::store::{StoreFuture, StoreImpl};
use blake3::Hash;
use std::{
    io::{Error, ErrorKind, Write},
    path::PathBuf,
};
use tempfile::Builder;

pub struct FilesystemStoreImpl {
    path: PathBuf,
}

impl FilesystemStoreImpl {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl StoreImpl for FilesystemStoreImpl {
    fn get(&self, hash: Hash) -> StoreFuture<'_, Vec<u8>> {
        Box::pin(async move {
            let mut object_path = self.path.clone();
            object_path.push(hash.to_hex().as_ref());
            match std::fs::read(object_path) {
                Ok(res) => Ok(res),
                Err(e) if e.kind() == ErrorKind::NotFound => Err(Error::new(
                    ErrorKind::NotFound,
                    format!("Object not found: {}", hash),
                )),
                Err(e) => Err(e),
            }
        })
    }

    fn size(&self, hash: Hash) -> StoreFuture<'_, u64> {
        Box::pin(async move {
            let mut object_path = self.path.clone();
            object_path.push(hash.to_hex().as_ref());
            let metadata = std::fs::metadata(object_path)?;
            Ok(metadata.len())
        })
    }

    fn put<'a>(&'a self, data: &'a [u8]) -> StoreFuture<'a, Hash> {
        Box::pin(async move {
            let hash = blake3::hash(data);
            let mut object_path = self.path.clone();
            object_path.push(hash.to_hex().as_ref());

            let mut temporary = Builder::new()
                .prefix(".put-")
                .tempfile_in(self.path.clone())?;
            temporary.write_all(data)?;
            temporary.as_file().sync_all()?;

            match temporary.persist_noclobber(&object_path) {
                Ok(_) => Ok(hash),
                Err(e) if e.error.kind() == ErrorKind::AlreadyExists => Ok(hash),
                Err(e) => Err(e.error),
            }
        })
    }
}

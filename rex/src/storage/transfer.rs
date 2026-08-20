//! Utilities for importing filesystem content into a [`Store`] and exporting
//! stored blobs and trees back to the filesystem.

use super::{entry::EntryKind, store::Store};
use blake3::Hash;
use std::{collections::BTreeMap, error::Error, path::Path};

/// Imports a regular file or directory tree from `path` into `store`.
///
/// Files become [`EntryKind::Blob`] objects. Directories are imported
/// recursively and become [`EntryKind::Tree`] objects. The returned tuple
/// contains the imported object's kind and content hash.
///
/// Symbolic links, special files, and directory entries with non-UTF-8 names
/// are rejected.
pub async fn import_path(
    store: &Store,
    path: &Path,
) -> Result<(EntryKind, Hash), Box<dyn Error + Send + Sync>> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            format!("path does not exist: `{}`", path.display())
        } else {
            format!("inspect path `{}`: {error}", path.display())
        }
    })?;
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        return Err(format!("cannot import symbolic link `{}`", path.display()).into());
    }

    if file_type.is_dir() {
        let mut entries = BTreeMap::new();
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let child_path = entry.path();
            let file_type = entry.file_type()?;
            if file_type.is_symlink() || (!file_type.is_file() && !file_type.is_dir()) {
                return Err(format!(
                    "cannot import non-file filesystem entry `{}`",
                    child_path.display()
                )
                .into());
            }
            let (kind, hash) = Box::pin(import_path(store, &child_path)).await?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|name| format!("filename is not UTF-8: {name:?}"))?;
            entries.insert(name, (kind, hash));
        }
        Ok((EntryKind::Tree, store.put_tree(entries).await?))
    } else if file_type.is_file() {
        Ok((EntryKind::Blob, store.put(std::fs::read(path)?).await?))
    } else {
        Err(format!(
            "cannot import non-file filesystem entry `{}`",
            path.display()
        )
        .into())
    }
}

/// Exports the stored tree identified by `hash` to `destination`.
///
/// The tree is written recursively, creating destination directories as
/// needed and overwriting files at paths represented by the tree. Entry names
/// that could escape the destination directory are rejected.
pub async fn export_tree(
    store: &Store,
    hash: Hash,
    destination: &Path,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    std::fs::create_dir_all(destination)?;
    for (name, entry) in store.get_tree(hash).await? {
        validate_tree_name(&name)?;
        let child_path = destination.join(name);
        match entry.kind {
            EntryKind::Blob => std::fs::write(child_path, store.get(entry.hash).await?)?,
            EntryKind::Tree => Box::pin(export_tree(store, entry.hash, &child_path)).await?,
        }
    }
    Ok(())
}

/// Exports the stored blob identified by `hash` to `destination`.
///
/// Missing parent directories are created, and an existing destination file
/// is overwritten.
pub async fn export_blob(
    store: &Store,
    hash: Hash,
    destination: &Path,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(destination, store.get(hash).await?)?;
    Ok(())
}

fn validate_tree_name(name: &str) -> Result<(), Box<dyn Error + Send + Sync>> {
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\']) {
        return Err(format!("invalid tree entry name `{name}`").into());
    }
    Ok(())
}

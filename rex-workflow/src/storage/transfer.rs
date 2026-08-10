use crate::storage::{entry::EntryKind, store::Store};
use blake3::Hash;
use std::{
    collections::BTreeMap,
    error::Error,
    path::{Path, PathBuf},
};

pub async fn import_path(
    store: &Store,
    path: &Path,
) -> Result<(EntryKind, Hash), Box<dyn Error + Send + Sync>> {
    if path.is_dir() {
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
    } else if path.is_file() {
        Ok((EntryKind::Blob, store.put(std::fs::read(path)?).await?))
    } else {
        Err(format!("path does not exist: `{}`", path.display()).into())
    }
}

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

pub fn regular_files(root: &Path) -> Result<Vec<PathBuf>, Box<dyn Error + Send + Sync>> {
    let mut files = Vec::new();
    collect_regular_files(root, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_regular_files(
    path: &Path,
    files: &mut Vec<PathBuf>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    if !path.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            return Err(format!(
                "tool output contains a symbolic link: `{}`",
                entry.path().display()
            )
            .into());
        }
        if file_type.is_dir() {
            collect_regular_files(&entry.path(), files)?;
        } else if file_type.is_file() {
            files.push(entry.path());
        } else {
            return Err(format!(
                "tool output contains a special file: `{}`",
                entry.path().display()
            )
            .into());
        }
    }
    Ok(())
}

fn validate_tree_name(name: &str) -> Result<(), Box<dyn Error + Send + Sync>> {
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\']) {
        return Err(format!("invalid tree entry name `{name}`").into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::store::Store;

    #[tokio::test]
    async fn directory_roundtrip_preserves_tree() {
        let source = tempfile::tempdir().unwrap();
        std::fs::create_dir(source.path().join("nested")).unwrap();
        std::fs::write(source.path().join("a.txt"), b"alpha").unwrap();
        std::fs::write(source.path().join("nested/b.bin"), [0_u8, 1, 2]).unwrap();

        let store = Store::new_in_memory();
        let (kind, hash) = import_path(&store, source.path()).await.unwrap();
        assert_eq!(kind, EntryKind::Tree);

        let destination = tempfile::tempdir().unwrap();
        export_tree(&store, hash, destination.path()).await.unwrap();
        assert_eq!(
            std::fs::read(destination.path().join("a.txt")).unwrap(),
            b"alpha"
        );
        assert_eq!(
            std::fs::read(destination.path().join("nested/b.bin")).unwrap(),
            [0_u8, 1, 2]
        );
    }
}

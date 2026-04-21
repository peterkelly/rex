use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ImportPathError {
    #[error("import path escapes filesystem root")]
    EscapesRoot,
}

/// Resolves a local Rex library import to a `.rex` path relative to `base_dir`.
/// Leading `super` segments walk up the directory tree.
pub fn resolve_local_import_path<S: AsRef<str>>(
    base_dir: &Path,
    segments: &[S],
) -> Result<Option<PathBuf>, ImportPathError> {
    if segments.is_empty() {
        return Ok(None);
    }

    let mut dir = base_dir.to_path_buf();
    let mut idx = 0usize;
    while idx < segments.len() && segments[idx].as_ref() == "super" {
        dir = dir
            .parent()
            .ok_or(ImportPathError::EscapesRoot)?
            .to_path_buf();
        idx += 1;
    }

    let mut path = dir;
    for seg in &segments[idx..segments.len().saturating_sub(1)] {
        path.push(seg.as_ref());
    }
    let Some(last) = segments.last().map(|s| s.as_ref()) else {
        return Ok(None);
    };
    path.push(format!("{last}.rex"));
    Ok(Some(path))
}

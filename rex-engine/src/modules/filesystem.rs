use std::path::PathBuf;
use std::sync::Arc;

use rex_util::{ImportPathError, resolve_local_import_path, sha256_hex};

use crate::{EngineError, ModuleError};

use super::{ModuleId, ResolveRequest, ResolvedModule, ResolverFn};

fn split_module_name_and_sha(module_name: String) -> (String, Option<String>) {
    match module_name.split_once('#') {
        Some((a, b)) if !b.is_empty() => (a.to_string(), Some(b.to_string())),
        _ => (module_name, None),
    }
}

fn resolve_rex_file(
    path: PathBuf,
    expected_sha: Option<String>,
    kind: &'static str,
) -> Result<Option<ResolvedModule>, EngineError> {
    let Ok(canon) = path.canonicalize() else {
        return Ok(None);
    };
    let bytes = match std::fs::read(&canon) {
        Ok(b) => b,
        Err(_) => return Ok(None),
    };
    let hash = sha256_hex(&bytes);
    if let Some(expected) = expected_sha {
        let expected = expected.to_ascii_lowercase();
        if !hash.starts_with(&expected) {
            return Err(ModuleError::ShaMismatchPath {
                kind,
                path: canon,
                expected,
                actual: hash,
            }
            .into());
        }
    }
    let source = String::from_utf8(bytes).map_err(|e| ModuleError::NotUtf8 {
        kind,
        path: canon.clone(),
        source: e,
    })?;
    Ok(Some(ResolvedModule {
        id: ModuleId::Local { path: canon },
        source,
    }))
}

pub fn default_local_resolver() -> ResolverFn {
    Arc::new(|req: ResolveRequest| {
        if req.module_name.starts_with("https://") {
            return Ok(None);
        }

        let (module_name, expected_sha) = split_module_name_and_sha(req.module_name);

        let base_dir = match req.importer {
            Some(ModuleId::Local { path }) => path.parent().map(|p| p.to_path_buf()),
            _ => std::env::current_dir().ok(),
        }
        .ok_or(ModuleError::NoBaseDirectory)?;

        let segs: Vec<&str> = module_name.split('.').collect();
        let path = match resolve_local_import_path(base_dir.as_path(), &segs) {
            Ok(Some(path)) => path,
            Ok(None) => return Ok(None),
            Err(ImportPathError::EscapesRoot) => return Err(ModuleError::ImportEscapesRoot.into()),
        };

        resolve_rex_file(path, expected_sha, "local")
    })
}

pub fn include_resolver(root: PathBuf) -> ResolverFn {
    Arc::new(move |req: ResolveRequest| {
        if req.module_name.starts_with("https://") {
            return Ok(None);
        }

        let (module_name, expected_sha) = split_module_name_and_sha(req.module_name);

        let segs: Vec<&str> = module_name.split('.').collect();
        if segs.is_empty() {
            return Ok(None);
        }
        let mut path = root.clone();
        for seg in &segs[..segs.len().saturating_sub(1)] {
            path.push(seg);
        }
        let last = segs.last().ok_or(ModuleError::EmptyModulePath)?;
        path.push(format!("{last}.rex"));

        resolve_rex_file(path, expected_sha, "include")
    })
}

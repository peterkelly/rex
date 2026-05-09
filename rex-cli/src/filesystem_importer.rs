use std::path::{Path, PathBuf};

use futures::future::BoxFuture;
use rex::engine::{
    EngineError, ImportRequest, Importer, ModuleError, ModuleId, ResolvedModule,
    ResolvedModuleContent,
};
use rex_util::{ImportPathError, resolve_local_import_path, sha256_hex};

#[derive(Clone, Debug, Default)]
pub struct FilesystemImporter {
    include_roots: Vec<PathBuf>,
}

impl FilesystemImporter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_include_root(&mut self, root: impl AsRef<Path>) -> Result<(), EngineError> {
        let root = root.as_ref();
        let canon = root
            .canonicalize()
            .map_err(|source| ModuleError::InvalidIncludeRoot {
                path: root.to_path_buf(),
                source,
            })?;
        self.include_roots.push(canon);
        Ok(())
    }
}

impl Importer for FilesystemImporter {
    fn import<'a>(
        &'a self,
        req: ImportRequest,
    ) -> BoxFuture<'a, Result<Option<ResolvedModule>, EngineError>> {
        Box::pin(async move {
            if req.module_name.starts_with("https://") {
                return Ok(None);
            }

            let (module_name, expected_sha) = split_module_name_and_sha(req.module_name);

            if module_name.ends_with(".rex") || Path::new(&module_name).components().count() > 1 {
                let path = PathBuf::from(&module_name);
                return resolve_rex_file(path, expected_sha, "local");
            }

            let segs: Vec<&str> = module_name.split('.').collect();

            let base_dir = match req.importer {
                Some(ModuleId::Local { path }) => path.parent().map(|p| p.to_path_buf()),
                _ => std::env::current_dir().ok(),
            };

            if let Some(base_dir) = base_dir {
                let path = match resolve_local_import_path(base_dir.as_path(), &segs) {
                    Ok(Some(path)) => path,
                    Ok(None) => return Ok(None),
                    Err(ImportPathError::EscapesRoot) => {
                        return Err(ModuleError::ImportEscapesRoot.into());
                    }
                };
                if let Some(module) = resolve_rex_file(path, expected_sha.clone(), "local")? {
                    return Ok(Some(module));
                }
            }

            for root in &self.include_roots {
                let Some(path) = include_path(root, &segs) else {
                    continue;
                };
                if let Some(module) = resolve_rex_file(path, expected_sha.clone(), "include")? {
                    return Ok(Some(module));
                }
            }

            Ok(None)
        })
    }
}

fn split_module_name_and_sha(module_name: String) -> (String, Option<String>) {
    match module_name.split_once('#') {
        Some((a, b)) if !b.is_empty() => (a.to_string(), Some(b.to_string())),
        _ => (module_name, None),
    }
}

fn include_path(root: &Path, segs: &[&str]) -> Option<PathBuf> {
    if segs.is_empty() {
        return None;
    }
    let mut path = root.to_path_buf();
    for seg in &segs[..segs.len().saturating_sub(1)] {
        path.push(seg);
    }
    let last = segs.last()?;
    path.push(format!("{last}.rex"));
    Some(path)
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
        Ok(bytes) => bytes,
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
    let source = String::from_utf8(bytes).map_err(|source| ModuleError::NotUtf8 {
        kind,
        path: canon.clone(),
        source,
    })?;
    Ok(Some(ResolvedModule {
        id: ModuleId::Local { path: canon },
        content: ResolvedModuleContent::Source(source),
    }))
}

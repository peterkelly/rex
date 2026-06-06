use std::path::PathBuf;

use futures::future::BoxFuture;
use rex::engine::{
    EngineError, ImportRequest, Importer, ModuleError, ModuleId, ResolvedModule,
    ResolvedModuleContent,
};
use rex_util::{ImportPathError, resolve_local_import_path, sha256_hex};

#[derive(Clone, Debug)]
pub struct FilesystemImporter {
    root: PathBuf,
}

impl FilesystemImporter {
    pub fn new() -> Self {
        Self {
            root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        }
    }

    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

impl Default for FilesystemImporter {
    fn default() -> Self {
        Self::new()
    }
}

impl Importer for FilesystemImporter {
    fn import<'a>(
        &'a self,
        req: ImportRequest,
    ) -> BoxFuture<'a, Result<Option<ResolvedModule>, EngineError>> {
        Box::pin(async move {
            let module_id = match req.importer.as_ref().and_then(ModuleId::parent) {
                Some(parent) => parent.join(&req.module_id),
                None => req.module_id.clone(),
            };
            let segs = module_id
                .segments()
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>();
            let path = match resolve_local_import_path(self.root.as_path(), &segs) {
                Ok(Some(path)) => path,
                Ok(None) => return Ok(None),
                Err(ImportPathError::EscapesRoot) => {
                    return Err(ModuleError::ImportEscapesRoot.into());
                }
            };
            if let Some(module) = resolve_rex_file(module_id, path, req.expected_sha, "local")? {
                return Ok(Some(module));
            }

            Ok(None)
        })
    }
}

fn resolve_rex_file(
    id: ModuleId,
    path: PathBuf,
    expected_sha: Option<String>,
    kind: &'static str,
) -> Result<Option<ResolvedModule>, EngineError> {
    let Ok(canon) = path.canonicalize() else {
        return Ok(None);
    };
    // println!("resolve_rex_file: path = {:?}, canon = {:?}", path, canon);
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
        id,
        content: ResolvedModuleContent::Source(source),
    }))
}

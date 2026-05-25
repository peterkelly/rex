use std::path::PathBuf;

use futures::future::BoxFuture;
use rex::engine::{
    EngineError, ImportRequest, Importer, ModuleError, ModuleId, ResolvedModule,
    ResolvedModuleContent,
};
use rex_util::{ImportPathError, resolve_local_import_path};

#[derive(Clone, Debug, Default)]
pub struct FilesystemImporter;

impl FilesystemImporter {
    pub fn new() -> Self {
        Self
    }
}

impl Importer for FilesystemImporter {
    fn import<'a>(
        &'a self,
        req: ImportRequest,
    ) -> BoxFuture<'a, Result<Option<ResolvedModule>, EngineError>> {
        Box::pin(async move {
            let segs: Vec<&str> = req.module_name.split('.').collect();

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
                if let Some(module) = resolve_rex_file(path, "local")? {
                    return Ok(Some(module));
                }
            }

            Ok(None)
        })
    }
}

fn resolve_rex_file(
    path: PathBuf,
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

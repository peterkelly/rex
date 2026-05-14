use futures::future::BoxFuture;
use rex_util::{sha256_hex, stdlib_source};

use crate::error::{EngineError, ModuleError};

use super::{ImportRequest, Importer, ModuleId, ResolvedModule, ResolvedModuleContent};

#[derive(Clone, Default)]
pub struct StdlibImporter;

impl Importer for StdlibImporter {
    fn import<'a>(
        &'a self,
        req: ImportRequest,
    ) -> BoxFuture<'a, Result<Option<ResolvedModule>, crate::EngineError>> {
        Box::pin(async move {
            let (base, expected_sha) = if let Some((a, b)) = req.module_name.split_once('#') {
                (a, Some(b))
            } else {
                (req.module_name.as_str(), None)
            };

            let Some(source) = stdlib_source(base) else {
                return Ok(None);
            };

            if let Some(expected) = expected_sha {
                let hash = sha256_hex(source.as_bytes());
                let expected = expected.to_ascii_lowercase();
                if !hash.starts_with(&expected) {
                    return Err(ModuleError::ShaMismatchStdlib {
                        module: base.to_string(),
                        expected,
                        actual: hash,
                    }
                    .into());
                }
            }

            Ok(Some(ResolvedModule {
                id: ModuleId::Virtual(base.to_string()),
                content: ResolvedModuleContent::Source(source.to_string()),
            }))
        })
    }
}

#[derive(Clone, Default)]
pub struct DenyImporter;

impl Importer for DenyImporter {
    fn import<'a>(
        &'a self,
        request: ImportRequest,
    ) -> BoxFuture<'a, Result<Option<ResolvedModule>, EngineError>> {
        Box::pin(async move {
            Err(ModuleError::ImportsDisabled {
                module_name: request.module_name,
            }
            .into())
        })
    }
}

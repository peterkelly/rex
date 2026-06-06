use futures::future::BoxFuture;
use rex_util::stdlib_source;

use crate::error::{EngineError, ModuleError};

use super::{ImportRequest, Importer, ResolvedModule, ResolvedModuleContent};

#[derive(Clone, Default)]
pub struct StdlibImporter;

impl<State> Importer<State> for StdlibImporter
where
    State: Clone + Send + Sync + 'static,
{
    fn import<'a>(
        &'a self,
        req: ImportRequest,
    ) -> BoxFuture<'a, Result<Option<ResolvedModule<State>>, crate::EngineError>> {
        Box::pin(async move {
            let base = req.module_id.to_string();

            let Some(source) = stdlib_source(&base) else {
                return Ok(None);
            };

            Ok(Some(ResolvedModule {
                id: req.module_id,
                content: ResolvedModuleContent::Source(source.to_string()),
            }))
        })
    }
}

#[derive(Clone, Default)]
pub struct DenyImporter;

impl<State> Importer<State> for DenyImporter
where
    State: Clone + Send + Sync + 'static,
{
    fn import<'a>(
        &'a self,
        request: ImportRequest,
    ) -> BoxFuture<'a, Result<Option<ResolvedModule<State>>, EngineError>> {
        Box::pin(async move {
            Err(ModuleError::ImportsDisabled {
                module_name: request.module_id.to_string(),
            }
            .into())
        })
    }
}

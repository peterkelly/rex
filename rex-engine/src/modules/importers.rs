use futures::future::BoxFuture;

use crate::error::{EngineError, ModuleError};

use super::{ImportRequest, Importer, ResolvedModule};

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

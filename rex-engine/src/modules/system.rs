use std::sync::Arc;

use futures::future::BoxFuture;

use crate::{EngineError, ModuleError};

use super::types::{ImportRequest, ResolvedModule};

pub trait Importer: Send + Sync {
    fn import<'a>(
        &'a self,
        request: ImportRequest,
    ) -> BoxFuture<'a, Result<Option<ResolvedModule>, EngineError>>;
}

#[derive(Clone)]
struct ImporterEntry {
    importer: Arc<dyn Importer>,
}

#[derive(Clone, Default)]
pub(crate) struct ImportChain {
    entries: Vec<ImporterEntry>,
}

impl ImportChain {
    pub(crate) fn with_importer(&self, importer: Arc<dyn Importer>) -> Self {
        let mut entries = self.entries.clone();
        entries.push(ImporterEntry { importer });
        Self { entries }
    }

    pub(crate) async fn import(&self, req: ImportRequest) -> Result<ResolvedModule, EngineError> {
        for entry in &self.entries {
            let resolved = entry
                .importer
                .import(ImportRequest {
                    module_name: req.module_name.clone(),
                    importer: req.importer.clone(),
                })
                .await?;
            match resolved {
                Some(resolved) => return Ok(resolved),
                None => continue,
            }
        }
        Err(ModuleError::NotFound {
            module_name: req.module_name,
        }
        .into())
    }
}

#[derive(Clone, Default)]
pub(crate) struct ModuleSystem {
    import_chain: ImportChain,
}

impl ModuleSystem {
    pub(crate) fn append_importer(
        &mut self,
        _name: impl Into<String>,
        importer: Arc<dyn Importer>,
    ) {
        self.import_chain.entries.push(ImporterEntry { importer });
    }

    pub(crate) fn prepend_importer(&mut self, importer: Arc<dyn Importer>) {
        self.import_chain
            .entries
            .insert(0, ImporterEntry { importer });
    }

    pub(crate) fn import_chain(&self) -> ImportChain {
        self.import_chain.clone()
    }
}

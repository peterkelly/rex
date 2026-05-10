use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;

use crate::{EngineError, ModuleError};

use super::types::{ImportRequest, ModuleId, ModuleInstance, ResolvedModule};

pub trait Importer: Send + Sync {
    fn import<'a>(
        &'a self,
        request: ImportRequest,
    ) -> BoxFuture<'a, Result<Option<ResolvedModule>, EngineError>>;
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

#[derive(Default)]
struct ModuleState {
    loaded: HashMap<ModuleId, ModuleInstance>,
    loading: HashSet<ModuleId>,
}

#[derive(Clone, Default)]
pub(crate) struct ModuleSystem {
    import_chain: ImportChain,
    state: Arc<Mutex<ModuleState>>,
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

    pub(crate) fn cached(&self, id: &ModuleId) -> Result<Option<ModuleInstance>, EngineError> {
        let state = self.state.lock().map_err(|_| ModuleError::StatePoisoned)?;
        Ok(state.loaded.get(id).cloned())
    }

    pub(crate) fn mark_loading(&self, id: &ModuleId) -> Result<(), EngineError> {
        let mut state = self.state.lock().map_err(|_| ModuleError::StatePoisoned)?;
        if state.loaded.contains_key(id) {
            return Ok(());
        }
        if state.loading.contains(id) {
            return Err(ModuleError::CyclicImport { id: id.clone() }.into());
        }
        state.loading.insert(id.clone());
        Ok(())
    }

    pub(crate) fn store_loaded(&self, inst: ModuleInstance) -> Result<(), EngineError> {
        let mut state = self.state.lock().map_err(|_| ModuleError::StatePoisoned)?;
        state.loading.remove(&inst.id);
        state.loaded.insert(inst.id.clone(), inst);
        Ok(())
    }

    pub(crate) fn invalidate(&self, id: &ModuleId) -> Result<(), EngineError> {
        let mut state = self.state.lock().map_err(|_| ModuleError::StatePoisoned)?;
        state.loading.remove(id);
        state.loaded.remove(id);
        Ok(())
    }
}

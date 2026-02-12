use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use crate::{EngineError, ModuleError};

use super::types::{ModuleId, ModuleInstance, ResolveRequest, ResolvedModule};

pub type ResolverFn =
    Arc<dyn Fn(ResolveRequest) -> Result<Option<ResolvedModule>, EngineError> + Send + Sync>;

#[derive(Clone)]
struct ResolverEntry {
    name: String,
    resolver: ResolverFn,
}

#[derive(Default)]
struct ModuleState<'h> {
    loaded: HashMap<ModuleId, ModuleInstance<'h>>,
    loading: HashSet<ModuleId>,
}

#[derive(Clone, Default)]
pub(crate) struct ModuleSystem<'h> {
    resolvers: Vec<ResolverEntry>,
    state: Arc<Mutex<ModuleState<'h>>>,
}

impl<'h> ModuleSystem<'h> {
    pub(crate) fn add_resolver(&mut self, name: impl Into<String>, resolver: ResolverFn) {
        self.resolvers.push(ResolverEntry {
            name: name.into(),
            resolver,
        });
    }

    pub(crate) fn resolve(&self, req: ResolveRequest) -> Result<ResolvedModule, EngineError> {
        for entry in &self.resolvers {
            tracing::trace!(resolver = %entry.name, module = %req.module_name, "trying module resolver");
            let resolved = (entry.resolver)(ResolveRequest {
                module_name: req.module_name.clone(),
                importer: req.importer.clone(),
            });
            match resolved? {
                Some(resolved) => return Ok(resolved),
                None => continue,
            }
        }
        Err(ModuleError::NotFound {
            module_name: req.module_name,
        }
        .into())
    }

    pub(crate) fn cached(&self, id: &ModuleId) -> Result<Option<ModuleInstance<'h>>, EngineError> {
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

    pub(crate) fn store_loaded(&self, inst: ModuleInstance<'h>) -> Result<(), EngineError> {
        let mut state = self.state.lock().map_err(|_| ModuleError::StatePoisoned)?;
        state.loading.remove(&inst.id);
        state.loaded.insert(inst.id.clone(), inst);
        Ok(())
    }
}

pub(crate) fn wrap_resolver<F>(f: F) -> ResolverFn
where
    F: Fn(ResolveRequest) -> Result<Option<ResolvedModule>, EngineError> + Send + Sync + 'static,
{
    Arc::new(f)
}

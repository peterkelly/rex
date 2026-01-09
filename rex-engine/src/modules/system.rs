use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use futures::FutureExt;
use futures::future::BoxFuture;

use crate::EngineError;

use super::types::{ModuleId, ModuleInstance, ResolveRequest, ResolvedModule};

pub type ResolverFuture = BoxFuture<'static, Result<Option<ResolvedModule>, EngineError>>;
pub type ResolverFn = Arc<dyn Fn(ResolveRequest) -> ResolverFuture + Send + Sync>;

#[derive(Clone)]
struct ResolverEntry {
    name: String,
    resolver: ResolverFn,
}

#[derive(Default)]
struct ModuleState {
    loaded: HashMap<ModuleId, ModuleInstance>,
    loading: HashSet<ModuleId>,
}

#[derive(Clone, Default)]
pub(crate) struct ModuleSystem {
    resolvers: Vec<ResolverEntry>,
    state: Arc<Mutex<ModuleState>>,
}

impl ModuleSystem {
    pub(crate) fn add_resolver(&mut self, name: impl Into<String>, resolver: ResolverFn) {
        self.resolvers.push(ResolverEntry {
            name: name.into(),
            resolver,
        });
    }

    pub(crate) fn resolve(&self, req: ResolveRequest) -> Result<ResolvedModule, EngineError> {
        for entry in &self.resolvers {
            tracing::trace!(resolver = %entry.name, module = %req.module_name, "trying module resolver");
            let fut = (entry.resolver)(ResolveRequest {
                module_name: req.module_name.clone(),
                importer: req.importer.clone(),
            });
            match futures::executor::block_on(fut)? {
                Some(resolved) => return Ok(resolved),
                None => continue,
            }
        }
        Err(EngineError::Module(format!(
            "module not found: {}",
            req.module_name
        )))
    }

    pub(crate) fn cached(&self, id: &ModuleId) -> Option<ModuleInstance> {
        self.state.lock().ok()?.loaded.get(id).cloned()
    }

    pub(crate) fn mark_loading(&self, id: &ModuleId) -> Result<(), EngineError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| EngineError::Internal("module state poisoned".into()))?;
        if state.loaded.contains_key(id) {
            return Ok(());
        }
        if state.loading.contains(id) {
            return Err(EngineError::Module(format!("cyclic module import: {id}")));
        }
        state.loading.insert(id.clone());
        Ok(())
    }

    pub(crate) fn store_loaded(&self, inst: ModuleInstance) -> Result<(), EngineError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| EngineError::Internal("module state poisoned".into()))?;
        state.loading.remove(&inst.id);
        state.loaded.insert(inst.id.clone(), inst);
        Ok(())
    }
}

pub(crate) fn wrap_resolver<F, Fut>(f: F) -> ResolverFn
where
    F: Fn(ResolveRequest) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<Option<ResolvedModule>, EngineError>> + Send + 'static,
{
    Arc::new(move |req| f(req).boxed())
}

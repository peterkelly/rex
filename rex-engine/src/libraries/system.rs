use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use crate::{EngineError, LibraryError};

use super::types::{LibraryId, LibraryInstance, ResolveRequest, ResolvedLibrary};

pub type ResolverFn =
    Arc<dyn Fn(ResolveRequest) -> Result<Option<ResolvedLibrary>, EngineError> + Send + Sync>;

#[derive(Clone)]
struct ResolverEntry {
    name: String,
    resolver: ResolverFn,
}

#[derive(Default)]
struct ModuleState {
    loaded: HashMap<LibraryId, LibraryInstance>,
    loading: HashSet<LibraryId>,
}

#[derive(Clone, Default)]
pub(crate) struct LibrarySystem {
    resolvers: Vec<ResolverEntry>,
    state: Arc<Mutex<ModuleState>>,
}

impl LibrarySystem {
    pub(crate) fn add_resolver(&mut self, name: impl Into<String>, resolver: ResolverFn) {
        self.resolvers.push(ResolverEntry {
            name: name.into(),
            resolver,
        });
    }

    pub(crate) fn resolve(&self, req: ResolveRequest) -> Result<ResolvedLibrary, EngineError> {
        for entry in &self.resolvers {
            tracing::trace!(resolver = %entry.name, library = %req.library_name, "trying library resolver");
            let resolved = (entry.resolver)(ResolveRequest {
                library_name: req.library_name.clone(),
                importer: req.importer.clone(),
            });
            match resolved? {
                Some(resolved) => return Ok(resolved),
                None => continue,
            }
        }
        Err(LibraryError::NotFound {
            library_name: req.library_name,
        }
        .into())
    }

    pub(crate) fn cached(&self, id: &LibraryId) -> Result<Option<LibraryInstance>, EngineError> {
        let state = self.state.lock().map_err(|_| LibraryError::StatePoisoned)?;
        Ok(state.loaded.get(id).cloned())
    }

    pub(crate) fn mark_loading(&self, id: &LibraryId) -> Result<(), EngineError> {
        let mut state = self.state.lock().map_err(|_| LibraryError::StatePoisoned)?;
        if state.loaded.contains_key(id) {
            return Ok(());
        }
        if state.loading.contains(id) {
            return Err(LibraryError::CyclicImport { id: id.clone() }.into());
        }
        state.loading.insert(id.clone());
        Ok(())
    }

    pub(crate) fn store_loaded(&self, inst: LibraryInstance) -> Result<(), EngineError> {
        let mut state = self.state.lock().map_err(|_| LibraryError::StatePoisoned)?;
        state.loading.remove(&inst.id);
        state.loaded.insert(inst.id.clone(), inst);
        Ok(())
    }

    pub(crate) fn invalidate(&self, id: &LibraryId) -> Result<(), EngineError> {
        let mut state = self.state.lock().map_err(|_| LibraryError::StatePoisoned)?;
        state.loading.remove(id);
        state.loaded.remove(id);
        Ok(())
    }
}

pub(crate) fn wrap_resolver<F>(f: F) -> ResolverFn
where
    F: Fn(ResolveRequest) -> Result<Option<ResolvedLibrary>, EngineError> + Send + Sync + 'static,
{
    Arc::new(f)
}

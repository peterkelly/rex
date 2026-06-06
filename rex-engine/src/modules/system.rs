use std::{collections::BTreeMap, sync::Arc};

use futures::future::BoxFuture;

use crate::{EngineError, ModuleError};

use super::{
    module_id::ModuleId,
    types::{ImportRequest, ResolvedModule},
};

pub trait Importer<State = ()>: Send + Sync
where
    State: Clone + Send + Sync + 'static,
{
    fn import<'a>(
        &'a self,
        request: ImportRequest,
    ) -> BoxFuture<'a, Result<Option<ResolvedModule<State>>, EngineError>>;
}

#[derive(Clone)]
struct ImporterEntry<State: Clone + Send + Sync + 'static> {
    importer: Arc<dyn Importer<State>>,
}

#[derive(Clone)]
pub(crate) struct ImportChain<State: Clone + Send + Sync + 'static = ()> {
    entries: Vec<ImporterEntry<State>>,
}

impl<State> Default for ImportChain<State>
where
    State: Clone + Send + Sync + 'static,
{
    fn default() -> Self {
        Self {
            entries: Vec::new(),
        }
    }
}

impl<State> ImportChain<State>
where
    State: Clone + Send + Sync + 'static,
{
    pub(crate) fn with_importer(&self, importer: Arc<dyn Importer<State>>) -> Self {
        let mut entries = self.entries.clone();
        entries.push(ImporterEntry { importer });
        Self { entries }
    }

    pub(crate) async fn import(
        &self,
        req: ImportRequest,
    ) -> Result<ResolvedModule<State>, EngineError> {
        for entry in &self.entries {
            let resolved = entry
                .importer
                .import(ImportRequest {
                    module_id: req.module_id.clone(),
                    importer: req.importer.clone(),
                })
                .await?;
            match resolved {
                Some(resolved) => return Ok(resolved),
                None => continue,
            }
        }
        Err(ModuleError::NotFound {
            module_name: req.module_id.to_string(),
        }
        .into())
    }
}

/// Per-compile importer result cache.
#[derive(Clone, Debug)]
pub(crate) struct ResolvedModuleCache<State: Clone + Send + Sync + 'static = ()> {
    requests: BTreeMap<ImportRequestKey, ModuleId>,
    modules: BTreeMap<ModuleId, ResolvedModule<State>>,
}

impl<State> Default for ResolvedModuleCache<State>
where
    State: Clone + Send + Sync + 'static,
{
    fn default() -> Self {
        Self {
            requests: BTreeMap::new(),
            modules: BTreeMap::new(),
        }
    }
}

impl<State> ResolvedModuleCache<State>
where
    State: Clone + Send + Sync + 'static,
{
    pub(crate) async fn import(
        &mut self,
        chain: &ImportChain<State>,
        request: ImportRequest,
    ) -> Result<ResolvedModule<State>, EngineError> {
        let key = ImportRequestKey::from_request(&request);
        if let Some(module_id) = self.requests.get(&key) {
            let resolved = self.modules.get(module_id).cloned().ok_or_else(|| {
                EngineError::Internal(format!(
                    "resolved module cache request pointed at missing module `{module_id}`"
                ))
            })?;
            return Ok(resolved);
        }

        if request.importer.is_none()
            && let Some(resolved) = self.modules.get(&request.module_id).cloned()
        {
            self.requests.insert(key, resolved.id.clone());
            return Ok(resolved);
        }

        let resolved = chain.import(request).await?;
        if let Some(cached) = self.modules.get(&resolved.id).cloned() {
            self.requests.insert(key, cached.id.clone());
            return Ok(cached);
        }

        self.requests.insert(key, resolved.id.clone());
        self.modules.insert(resolved.id.clone(), resolved.clone());
        Ok(resolved)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct ImportRequestKey {
    module_id: ModuleId,
    importer: Option<ModuleId>,
}

impl ImportRequestKey {
    fn from_request(request: &ImportRequest) -> Self {
        Self {
            module_id: request.module_id.clone(),
            importer: request.importer.clone(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct ModuleSystem<State: Clone + Send + Sync + 'static = ()> {
    import_chain: ImportChain<State>,
}

impl<State> Default for ModuleSystem<State>
where
    State: Clone + Send + Sync + 'static,
{
    fn default() -> Self {
        Self {
            import_chain: ImportChain::default(),
        }
    }
}

impl<State> ModuleSystem<State>
where
    State: Clone + Send + Sync + 'static,
{
    pub(crate) fn append_importer(
        &mut self,
        _name: impl Into<String>,
        importer: Arc<dyn Importer<State>>,
    ) {
        self.import_chain.entries.push(ImporterEntry { importer });
    }

    pub(crate) fn prepend_importer(&mut self, importer: Arc<dyn Importer<State>>) {
        self.import_chain
            .entries
            .insert(0, ImporterEntry { importer });
    }

    pub(crate) fn import_chain(&self) -> ImportChain<State> {
        self.import_chain.clone()
    }
}

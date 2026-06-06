use std::{collections::BTreeMap, sync::Arc};

use futures::future::BoxFuture;

use crate::{EngineError, ModuleError};

use super::{
    module_id::ModuleId,
    types::{ImportRequest, ResolvedModule},
};

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
                    module_id: req.module_id.clone(),
                    expected_sha: req.expected_sha.clone(),
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
///
/// `expected_sha` is deliberately not part of the request key because it
/// constrains module content, not module identity. Cache hits revalidate it
/// against the cached content hash instead of calling importers again.
#[derive(Clone, Debug, Default)]
pub(crate) struct ResolvedModuleCache {
    requests: BTreeMap<ImportRequestKey, ModuleId>,
    modules: BTreeMap<ModuleId, ResolvedModule>,
}

impl ResolvedModuleCache {
    pub(crate) async fn import(
        &mut self,
        chain: &ImportChain,
        request: ImportRequest,
    ) -> Result<ResolvedModule, EngineError> {
        let key = ImportRequestKey::from_request(&request);
        if let Some(module_id) = self.requests.get(&key) {
            let resolved = self.modules.get(module_id).cloned().ok_or_else(|| {
                EngineError::Internal(format!(
                    "resolved module cache request pointed at missing module `{module_id}`"
                ))
            })?;
            validate_expected_sha(&resolved, request.expected_sha.as_deref())?;
            return Ok(resolved);
        }

        if request.importer.is_none()
            && let Some(resolved) = self.modules.get(&request.module_id).cloned()
        {
            validate_expected_sha(&resolved, request.expected_sha.as_deref())?;
            self.requests.insert(key, resolved.id.clone());
            return Ok(resolved);
        }

        let expected_sha = request.expected_sha.clone();
        let resolved = chain.import(request).await?;
        if let Some(cached) = self.modules.get(&resolved.id).cloned() {
            validate_expected_sha(&cached, expected_sha.as_deref())?;
            self.requests.insert(key, cached.id.clone());
            return Ok(cached);
        }

        validate_expected_sha(&resolved, expected_sha.as_deref())?;
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

fn validate_expected_sha(
    resolved: &ResolvedModule,
    expected_sha: Option<&str>,
) -> Result<(), EngineError> {
    let Some(expected) = expected_sha else {
        return Ok(());
    };
    let Some(actual) = resolved.content_fingerprint() else {
        return Ok(());
    };
    let expected = expected.to_ascii_lowercase();
    if actual.starts_with(&expected) {
        return Ok(());
    }
    Err(ModuleError::ShaMismatchModule {
        module: resolved.id.clone(),
        expected,
        actual,
    }
    .into())
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

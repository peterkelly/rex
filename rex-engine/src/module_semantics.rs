//! Shared module/import semantics for the compiler and language tooling.
//!
//! This module exposes the compiler-owned import rules without exposing module
//! loading policy. Callers such as the LSP can resolve source through their own
//! snapshot-aware importer, then use these helpers for canonical names, export
//! tables, binding expansion, validation, and rewrite behavior.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use crate::{
    EngineError,
    builder::{engine::Engine, rewrite::load_module_types_from_resolved},
};
use rex_ast::{CompilationUnit, ImportDecl};

pub use crate::builder::rewrite::{
    ImportUseError, rewrite_import_uses, validate_import_uses, validate_import_uses_with_spans,
};
pub use crate::modules::types::{module_key_for_module, prefix_for_module, prefix_for_module_key};
pub use crate::modules::{
    ImportBindingPolicy, ImportBindings, ImportRequest, ModuleId, ResolvedModuleContent,
    add_import_bindings, alias_is_visible, collect_pattern_bindings, contains_import_alias,
    decl_type_names, decl_value_names, default_import_decl, exports_from_program, import_specifier,
    interface_decls_from_program, parse_program_from_source, program_from_resolved,
    qualified_alias_member,
};
use crate::modules::{Importer, ModuleExports};

#[derive(Clone, Debug)]
pub struct ToolingLoadedImport {
    pub module_id: ModuleId,
    pub exports: ModuleExports,
    pub program: CompilationUnit,
    pub source: Option<String>,
}

pub async fn load_import_for_tooling<State>(
    engine: &mut Engine<State>,
    import_decl: &ImportDecl,
    importer: Option<ModuleId>,
    extra_importer: Option<Arc<dyn Importer>>,
    loaded: &mut BTreeMap<ModuleId, ModuleExports>,
    loading: &mut BTreeSet<ModuleId>,
) -> Result<ToolingLoadedImport, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let chain = match extra_importer {
        Some(importer) => engine.modules.import_chain().with_importer(importer),
        None => engine.modules.import_chain(),
    };
    let spec = import_specifier(&import_decl.path);
    let resolved = chain
        .import(ImportRequest {
            module_name: spec,
            importer,
        })
        .await?;
    let program = program_from_resolved(&resolved)?;
    let source = match &resolved.content {
        ResolvedModuleContent::Source(source) => Some(source.clone()),
        ResolvedModuleContent::CompilationUnit(_) => None,
    };
    engine.refresh_if_stale(&resolved)?;
    let exports = if let Some(exports) = engine.module_exports_cache.get(&resolved.id).cloned() {
        engine.ensure_cycle_interfaces_published(&resolved.id)?;
        exports
    } else {
        load_module_types_from_resolved(engine, resolved.clone(), &chain, loaded, loading).await?
    };

    Ok(ToolingLoadedImport {
        module_id: resolved.id,
        exports,
        program,
        source,
    })
}

//! Shared module/import semantics for the compiler and language tooling.
//!
//! This module exposes the compiler-owned import rules without exposing module
//! loading policy. Callers such as the LSP can resolve source through their own
//! snapshot-aware importer, then use these helpers for canonical names, export
//! tables, binding expansion, validation, and rewrite behavior.

use std::sync::Arc;

use crate::{Builder, EngineError, builder::rewrite::load_module_types_from_resolved};
use rex_ast::ImportDecl;

pub use crate::builder::rewrite::{
    ImportUseError, ModuleLoadState, rewrite_import_uses, validate_import_uses,
    validate_import_uses_with_spans,
};
pub use crate::modules::types::{module_key_for_module, prefix_for_module, prefix_for_module_key};
use crate::modules::{CompilationPackage, Importer, ModuleExports};
pub use crate::modules::{
    ImportBindingPolicy, ImportBindings, ImportRequest, ModuleId, ResolvedModuleContent,
    add_import_bindings, alias_is_visible, collect_pattern_bindings, contains_import_alias,
    decl_type_names, decl_value_names, default_import_decl, exports_from_program, import_specifier,
    interface_decls_from_program, package_from_resolved, parse_program_from_source,
    qualified_alias_member,
};

#[derive(Clone, Debug)]
pub struct ToolingLoadedImport {
    pub module_id: ModuleId,
    pub exports: ModuleExports,
    pub package: CompilationPackage,
    pub source: Option<String>,
}

pub async fn load_import_for_tooling<State>(
    builder: &mut Builder<State>,
    import_decl: &ImportDecl,
    importer: Option<ModuleId>,
    extra_importer: Option<Arc<dyn Importer<State>>>,
    load_state: &mut ModuleLoadState<State>,
) -> Result<ToolingLoadedImport, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let chain = match extra_importer {
        Some(importer) => builder
            .module_loader
            .system
            .import_chain()
            .with_importer(importer),
        None => builder.module_loader.system.import_chain(),
    };
    let module_id = import_specifier(&import_decl.path)?;
    let resolved = load_state
        .import(
            &chain,
            ImportRequest {
                module_id,
                importer,
            },
        )
        .await?;
    let source = match &resolved.content {
        ResolvedModuleContent::Source(source) => Some(source.clone()),
        ResolvedModuleContent::CompilationPackage(_) | ResolvedModuleContent::Module(_) => None,
    };
    let exports = if let Some(exports) = builder
        .module_loader
        .module_exports_cache
        .get(&resolved.id)
        .cloned()
    {
        builder.ensure_cycle_interfaces_published(&resolved.id)?;
        exports
    } else {
        load_module_types_from_resolved(builder, resolved.clone(), &chain, load_state).await?
    };
    let package = match &resolved.content {
        ResolvedModuleContent::Source(_) | ResolvedModuleContent::CompilationPackage(_) => {
            package_from_resolved(&resolved)?
        }
        ResolvedModuleContent::Module(_) => {
            let module_name = resolved.id.to_string();
            let module = builder
                .module_loader
                .virtual_modules
                .get(&module_name)
                .ok_or_else(|| {
                    EngineError::Internal(format!(
                        "missing virtual module metadata for Rust module `{}`",
                        resolved.id
                    ))
                })?;
            module.package.clone()
        }
    };

    Ok(ToolingLoadedImport {
        module_id: resolved.id,
        exports,
        package,
        source,
    })
}

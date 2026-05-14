//! Module system: importers, loading, and import rewriting.

use std::collections::{BTreeMap, BTreeSet};

use rex_ast::{
    CompilationUnit, Decl, DeclareFnDecl, ImportClause, ImportDecl, ImportPath, NameRef, Pattern,
    Span, Symbol,
};
use rex_parser::parse as parse_rex;

use crate::{builder::qualify::collect_local_renames, error::EngineError};

pub(crate) mod importers;
pub(crate) mod module;
pub(crate) mod system;
pub(crate) mod types;

pub use importers::{DenyImporter, StdlibImporter};
pub use module::Module;
pub use system::Importer;
pub use types::virtual_export_name;
pub use types::{
    CanonicalSymbol, ImportRequest, ModuleExports, ModuleId, ModuleInstance, ModuleKey,
    ResolvedModule, ResolvedModuleContent, SymbolKind, VirtualModule,
};

pub(crate) use system::{ImportChain, ModuleSystem};
pub(crate) use types::{module_key_for_module, prefix_for_module};

pub const ROOT_MODULE_NAME: &str = "__root__";
pub const PRELUDE_MODULE_NAME: &str = "Prelude";

pub(crate) fn import_specifier(path: &ImportPath) -> String {
    match path {
        ImportPath::Local { segments, sha } => {
            let base = segments
                .iter()
                .map(|s| s.as_ref())
                .collect::<Vec<_>>()
                .join(".");
            if let Some(sha) = sha {
                format!("{base}#{sha}")
            } else {
                base
            }
        }
        ImportPath::Remote { url, sha } => {
            if let Some(sha) = sha {
                format!("{url}#{sha}")
            } else {
                url.clone()
            }
        }
    }
}

pub(crate) fn contains_import_alias(decls: &[Decl], alias: &Symbol) -> bool {
    decls.iter().any(|decl| match decl {
        Decl::Import(import_decl) => import_decl.alias == *alias,
        _ => false,
    })
}

pub(crate) fn default_import_decl(module_name: &str) -> ImportDecl {
    ImportDecl {
        span: Span::default(),
        is_pub: false,
        path: ImportPath::Local {
            segments: vec![Symbol::intern(module_name)],
            sha: None,
        },
        alias: Symbol::intern(module_name),
        clause: Some(ImportClause::All),
    }
}

#[derive(Default)]
pub(crate) struct ImportBindings {
    pub(crate) alias_exports: BTreeMap<Symbol, ModuleExports>,
    pub(crate) imported_values: BTreeMap<Symbol, CanonicalSymbol>,
    pub(crate) imported_types: BTreeMap<Symbol, CanonicalSymbol>,
    pub(crate) imported_classes: BTreeMap<Symbol, CanonicalSymbol>,
}

pub(crate) struct ImportBindingPolicy<'a> {
    pub(crate) forbidden_values: &'a BTreeSet<Symbol>,
    pub(crate) forbidden_types: &'a BTreeSet<Symbol>,
}

pub(crate) fn add_import_bindings(
    out: &mut ImportBindings,
    import: &ImportDecl,
    exports: &ModuleExports,
    policy: &ImportBindingPolicy<'_>,
) -> Result<(), EngineError> {
    let module_name = import.alias.clone();
    let mut bind_local_value =
        |local_name: Symbol, target: CanonicalSymbol| -> Result<(), EngineError> {
            if policy.forbidden_values.contains(&local_name) {
                return Err(crate::ModuleError::ImportNameConflictsWithLocal {
                    module: module_name.clone(),
                    name: local_name,
                }
                .into());
            }
            if out.imported_values.contains_key(&local_name) {
                return Err(crate::ModuleError::DuplicateImportedName { name: local_name }.into());
            }
            out.imported_values.insert(local_name, target);
            Ok(())
        };
    let mut bind_local_type =
        |local_name: Symbol, target: CanonicalSymbol| -> Result<(), EngineError> {
            if policy.forbidden_types.contains(&local_name) {
                return Err(crate::ModuleError::ImportNameConflictsWithLocal {
                    module: module_name.clone(),
                    name: local_name,
                }
                .into());
            }
            if out.imported_types.contains_key(&local_name) {
                return Err(crate::ModuleError::DuplicateImportedName { name: local_name }.into());
            }
            out.imported_types.insert(local_name, target);
            Ok(())
        };
    let mut bind_local_class =
        |local_name: Symbol, target: CanonicalSymbol| -> Result<(), EngineError> {
            if policy.forbidden_types.contains(&local_name) {
                return Err(crate::ModuleError::ImportNameConflictsWithLocal {
                    module: module_name.clone(),
                    name: local_name,
                }
                .into());
            }
            if out.imported_classes.contains_key(&local_name) {
                return Err(crate::ModuleError::DuplicateImportedName { name: local_name }.into());
            }
            out.imported_classes.insert(local_name, target);
            Ok(())
        };

    match &import.clause {
        None => {
            out.alias_exports
                .insert(import.alias.clone(), exports.clone());
            Ok(())
        }
        Some(ImportClause::All) => {
            for (export, target) in exports.values() {
                bind_local_value(export.clone(), target.clone())?;
            }
            for (export, target) in exports.types() {
                bind_local_type(export.clone(), target.clone())?;
            }
            for (export, target) in exports.classes() {
                bind_local_class(export.clone(), target.clone())?;
            }
            Ok(())
        }
        Some(ImportClause::Items(items)) => {
            for item in items {
                let mut found = false;
                let local_name = item.alias.clone().unwrap_or_else(|| item.name.clone());
                if let Some(target) = exports.value(&item.name) {
                    bind_local_value(local_name.clone(), target.clone())?;
                    found = true;
                }
                if let Some(target) = exports.typ(&item.name) {
                    bind_local_type(local_name.clone(), target.clone())?;
                    found = true;
                }
                if let Some(target) = exports.class(&item.name) {
                    bind_local_class(local_name.clone(), target.clone())?;
                    found = true;
                }
                if !found {
                    return Err(crate::ModuleError::MissingExport {
                        module: import.alias.clone(),
                        export: item.name.clone(),
                    }
                    .into());
                }
            }
            Ok(())
        }
    }
}

// FIXME: There are three copies of this function
pub(crate) fn collect_pattern_bindings(pat: &Pattern, out: &mut Vec<Symbol>) {
    match pat {
        Pattern::Wildcard(..) => {}
        Pattern::Var(v) => out.push(v.name.clone()),
        Pattern::Named(_, _, args) => {
            for arg in args {
                collect_pattern_bindings(arg, out);
            }
        }
        Pattern::Tuple(_, elems) | Pattern::List(_, elems) => {
            for elem in elems {
                collect_pattern_bindings(elem, out);
            }
        }
        Pattern::Cons(_, head, tail) => {
            collect_pattern_bindings(head, out);
            collect_pattern_bindings(tail, out);
        }
        Pattern::Dict(_, fields) => {
            for (_, pat) in fields {
                collect_pattern_bindings(pat, out);
            }
        }
    }
}

pub(crate) fn alias_is_visible(
    name: &Symbol,
    bound: &BTreeSet<Symbol>,
    shadowed_values: Option<&BTreeSet<Symbol>>,
) -> bool {
    if bound.contains(name) {
        return false;
    }
    match shadowed_values {
        None => true,
        Some(s) => !s.contains(name),
    }
}

// FIXME: There is another copy of this function in rex-lsp
pub(crate) fn qualified_alias_member(name: &NameRef) -> Option<(&Symbol, &Symbol)> {
    match name {
        NameRef::Qualified(_, segments) if segments.len() == 2 => {
            Some((&segments[0], &segments[1]))
        }
        _ => None,
    }
}

pub(crate) fn decl_value_names(decls: &[Decl]) -> BTreeSet<Symbol> {
    let mut out = BTreeSet::new();
    for decl in decls {
        match decl {
            Decl::Fn(fd) => {
                out.insert(fd.name.name.clone());
            }
            Decl::DeclareFn(df) => {
                out.insert(df.name.name.clone());
            }
            Decl::Type(td) => {
                for variant in &td.variants {
                    out.insert(variant.name.clone());
                }
            }
            Decl::Class(..) | Decl::Instance(..) | Decl::Import(..) => {}
        }
    }
    out
}

pub(crate) fn decl_type_names(decls: &[Decl]) -> BTreeSet<Symbol> {
    let mut out = BTreeSet::new();
    for decl in decls {
        match decl {
            Decl::Type(td) => {
                out.insert(td.name.clone());
            }
            Decl::Class(cd) => {
                out.insert(cd.name.clone());
            }
            Decl::Fn(..) | Decl::DeclareFn(..) | Decl::Instance(..) | Decl::Import(..) => {}
        }
    }
    out
}

pub(crate) fn interface_decls_from_program(compilation_unit: &CompilationUnit) -> Vec<Decl> {
    let mut out = Vec::new();
    for decl in &compilation_unit.decls {
        match decl {
            Decl::Fn(fd) if fd.is_pub => out.push(Decl::DeclareFn(DeclareFnDecl {
                span: fd.span,
                is_pub: fd.is_pub,
                name: fd.name.clone(),
                params: fd.params.clone(),
                ret: fd.ret.clone(),
                constraints: fd.constraints.clone(),
            })),
            Decl::Instance(..)
            | Decl::Import(..)
            | Decl::Fn(..)
            | Decl::DeclareFn(..)
            | Decl::Type(..)
            | Decl::Class(..) => {}
        }
    }
    out
}

pub(crate) fn exports_from_program(
    compilation_unit: &CompilationUnit,
    prefix: &str,
    module_id: &ModuleId,
) -> ModuleExports {
    let (value_renames, type_renames, class_renames) =
        collect_local_renames(compilation_unit, prefix);
    let module_key = module_key_for_module(module_id);

    let mut exports = ModuleExports::default();

    for decl in &compilation_unit.decls {
        match decl {
            Decl::Fn(fd) if fd.is_pub => {
                if let Some(internal) = value_renames.get(&fd.name.name) {
                    exports.insert_value(
                        fd.name.name.clone(),
                        CanonicalSymbol::from_symbol(
                            module_key,
                            SymbolKind::Value,
                            fd.name.name.clone(),
                            internal.clone(),
                        ),
                    );
                }
            }
            Decl::DeclareFn(df) if df.is_pub => {
                if let Some(internal) = value_renames.get(&df.name.name) {
                    exports.insert_value(
                        df.name.name.clone(),
                        CanonicalSymbol::from_symbol(
                            module_key,
                            SymbolKind::Value,
                            df.name.name.clone(),
                            internal.clone(),
                        ),
                    );
                }
            }
            Decl::Type(td) if td.is_pub => {
                if let Some(internal) = type_renames.get(&td.name) {
                    exports.insert_type(
                        td.name.clone(),
                        CanonicalSymbol::from_symbol(
                            module_key,
                            SymbolKind::Type,
                            td.name.clone(),
                            internal.clone(),
                        ),
                    );
                }
                for variant in &td.variants {
                    if let Some(internal) = value_renames.get(&variant.name) {
                        exports.insert_value(
                            variant.name.clone(),
                            CanonicalSymbol::from_symbol(
                                module_key,
                                SymbolKind::Value,
                                variant.name.clone(),
                                internal.clone(),
                            ),
                        );
                    }
                }
            }
            Decl::Class(cd) if cd.is_pub => {
                if let Some(internal) = class_renames.get(&cd.name) {
                    exports.insert_class(
                        cd.name.clone(),
                        CanonicalSymbol::from_symbol(
                            module_key,
                            SymbolKind::Class,
                            cd.name.clone(),
                            internal.clone(),
                        ),
                    );
                }
            }
            Decl::Instance(..)
            | Decl::Import(..)
            | Decl::Fn(..)
            | Decl::DeclareFn(..)
            | Decl::Type(..)
            | Decl::Class(..) => {}
        }
    }

    exports
}

pub(crate) fn parse_program_from_source(
    source: &str,
    context: Option<&ModuleId>,
) -> Result<CompilationUnit, EngineError> {
    let program = parse_rex(source).map_err(|errs| match context {
        Some(id) => EngineError::from(crate::ModuleError::ParseInModule {
            module: id.clone(),
            errors: errs,
        }),
        None => EngineError::from(crate::ModuleError::Parse { errors: errs }),
    })?;
    if let Some(module) = context
        && program.body.is_some()
    {
        return Err(crate::ModuleError::TopLevelExprInModule {
            module: module.clone(),
        }
        .into());
    }
    Ok(program)
}

pub(crate) fn program_from_resolved(
    resolved: &ResolvedModule,
) -> Result<CompilationUnit, EngineError> {
    match &resolved.content {
        ResolvedModuleContent::Source(source) => {
            parse_program_from_source(source, Some(&resolved.id))
        }
        ResolvedModuleContent::CompilationUnit(program) => {
            if program.body.is_some() {
                return Err(crate::ModuleError::TopLevelExprInModule {
                    module: resolved.id.clone(),
                }
                .into());
            }
            Ok(program.clone())
        }
    }
}

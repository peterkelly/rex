//! Module system: resolvers, loading, and import rewriting.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_recursion::async_recursion;
use rex_ast::expr::{
    Decl, DeclareFnDecl, Expr, FnDecl, ImportClause, ImportDecl, ImportPath, InstanceDecl, NameRef,
    Pattern, Program, Symbol, TypeConstraint, TypeDecl, TypeExpr, Var, intern,
};
use rex_lexer::Token;
use rex_parser::Parser as RexParser;
use rex_typesystem::types::{Predicate, Type};
use rex_util::{GasMeter, sha256_hex};
use uuid::Uuid;

use crate::{CompileError, Engine, EngineError};

#[cfg(not(target_arch = "wasm32"))]
mod filesystem;
mod module;
mod resolvers;
mod system;
mod types;

#[cfg(not(target_arch = "wasm32"))]
pub use filesystem::{default_local_resolver, include_resolver};
pub use module::Module;
#[cfg(all(not(target_arch = "wasm32"), feature = "github-imports"))]
pub use resolvers::default_github_resolver;
pub use resolvers::default_stdlib_resolver;
pub use system::ResolverFn;
pub use types::virtual_export_name;
pub use types::{
    CanonicalSymbol, ModuleExports, ModuleId, ModuleInstance, ModuleKey, ReplState, ResolveRequest,
    ResolvedModule, ResolvedModuleContent, SymbolKind, VirtualModule,
};

pub(crate) use system::ModuleSystem;
pub(crate) use types::{module_key_for_module, prefix_for_module};

use system::wrap_resolver;
use types::qualify;

fn import_specifier(path: &ImportPath) -> String {
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

fn spec_base_name(spec: &str) -> &str {
    spec.split_once('#').map_or(spec, |(base, _)| base)
}

fn contains_import_alias(decls: &[Decl], alias: &Symbol) -> bool {
    decls.iter().any(|decl| match decl {
        Decl::Import(import_decl) => import_decl.alias == *alias,
        _ => false,
    })
}

fn default_import_decl(module_name: &str) -> ImportDecl {
    ImportDecl {
        span: rex_lexer::span::Span::default(),
        is_pub: false,
        path: ImportPath::Local {
            segments: vec![intern(module_name)],
            sha: None,
        },
        alias: intern(module_name),
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
    pub(crate) existing_imported_values: Option<&'a BTreeSet<Symbol>>,
    pub(crate) existing_imported_types: Option<&'a BTreeSet<Symbol>>,
    pub(crate) existing_imported_classes: Option<&'a BTreeSet<Symbol>>,
}

fn add_import_bindings(
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
            if let Some(existing) = policy.existing_imported_values
                && existing.contains(&local_name)
            {
                return Err(crate::ModuleError::DuplicateImportedName { name: local_name }.into());
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
            if let Some(existing) = policy.existing_imported_types
                && existing.contains(&local_name)
            {
                return Err(crate::ModuleError::DuplicateImportedName { name: local_name }.into());
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
            if let Some(existing) = policy.existing_imported_classes
                && existing.contains(&local_name)
            {
                return Err(crate::ModuleError::DuplicateImportedName { name: local_name }.into());
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

fn collect_local_renames(
    program: &Program,
    prefix: &str,
) -> (
    BTreeMap<Symbol, Symbol>,
    BTreeMap<Symbol, Symbol>,
    BTreeMap<Symbol, Symbol>,
) {
    let mut values = BTreeMap::new();
    let mut types = BTreeMap::new();
    let mut classes = BTreeMap::new();

    for decl in &program.decls {
        match decl {
            Decl::Fn(fd) => {
                values.insert(fd.name.name.clone(), qualify(prefix, &fd.name.name));
            }
            Decl::DeclareFn(df) => {
                values.insert(df.name.name.clone(), qualify(prefix, &df.name.name));
            }
            Decl::Type(td) => {
                types.insert(td.name.clone(), qualify(prefix, &td.name));
                for variant in &td.variants {
                    values.insert(variant.name.clone(), qualify(prefix, &variant.name));
                }
            }
            Decl::Class(cd) => {
                classes.insert(cd.name.clone(), qualify(prefix, &cd.name));
            }
            Decl::Instance(..) | Decl::Import(..) => {}
        }
    }

    (values, types, classes)
}

fn collect_pattern_bindings(pat: &Pattern, out: &mut Vec<Symbol>) {
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

fn rename_type_expr(
    ty: &TypeExpr,
    type_renames: &BTreeMap<Symbol, Symbol>,
    class_renames: &BTreeMap<Symbol, Symbol>,
) -> TypeExpr {
    match ty {
        TypeExpr::Name(span, name) => {
            let name_sym = name.to_dotted_symbol();
            if let Some(new) = type_renames.get(&name_sym) {
                TypeExpr::Name(*span, NameRef::Unqualified(new.clone()))
            } else if let Some(new) = class_renames.get(&name_sym) {
                TypeExpr::Name(*span, NameRef::Unqualified(new.clone()))
            } else {
                TypeExpr::Name(*span, name.clone())
            }
        }
        TypeExpr::App(span, f, x) => TypeExpr::App(
            *span,
            Box::new(rename_type_expr(f, type_renames, class_renames)),
            Box::new(rename_type_expr(x, type_renames, class_renames)),
        ),
        TypeExpr::Fun(span, a, b) => TypeExpr::Fun(
            *span,
            Box::new(rename_type_expr(a, type_renames, class_renames)),
            Box::new(rename_type_expr(b, type_renames, class_renames)),
        ),
        TypeExpr::Tuple(span, elems) => TypeExpr::Tuple(
            *span,
            elems
                .iter()
                .map(|e| rename_type_expr(e, type_renames, class_renames))
                .collect(),
        ),
        TypeExpr::Record(span, fields) => TypeExpr::Record(
            *span,
            fields
                .iter()
                .map(|(name, ty)| {
                    (
                        name.clone(),
                        rename_type_expr(ty, type_renames, class_renames),
                    )
                })
                .collect(),
        ),
    }
}

fn rename_constraints(
    cs: &[TypeConstraint],
    type_renames: &BTreeMap<Symbol, Symbol>,
    class_renames: &BTreeMap<Symbol, Symbol>,
) -> Vec<TypeConstraint> {
    cs.iter()
        .map(|c| TypeConstraint {
            class: {
                let class_sym = c.class.to_dotted_symbol();
                class_renames
                    .get(&class_sym)
                    .cloned()
                    .map(NameRef::Unqualified)
                    .unwrap_or_else(|| c.class.clone())
            },
            typ: rename_type_expr(&c.typ, type_renames, class_renames),
        })
        .collect()
}

fn rename_pattern(pat: &Pattern, value_renames: &BTreeMap<Symbol, Symbol>) -> Pattern {
    match pat {
        Pattern::Wildcard(span) => Pattern::Wildcard(*span),
        Pattern::Var(v) => Pattern::Var(v.clone()),
        Pattern::Named(span, name, args) => Pattern::Named(
            *span,
            {
                let name_sym = name.to_dotted_symbol();
                value_renames
                    .get(&name_sym)
                    .cloned()
                    .map(NameRef::Unqualified)
                    .unwrap_or_else(|| name.clone())
            },
            args.iter()
                .map(|p| rename_pattern(p, value_renames))
                .collect(),
        ),
        Pattern::Tuple(span, elems) => Pattern::Tuple(
            *span,
            elems
                .iter()
                .map(|p| rename_pattern(p, value_renames))
                .collect(),
        ),
        Pattern::List(span, elems) => Pattern::List(
            *span,
            elems
                .iter()
                .map(|p| rename_pattern(p, value_renames))
                .collect(),
        ),
        Pattern::Cons(span, head, tail) => Pattern::Cons(
            *span,
            Box::new(rename_pattern(head, value_renames)),
            Box::new(rename_pattern(tail, value_renames)),
        ),
        Pattern::Dict(span, fields) => Pattern::Dict(
            *span,
            fields
                .iter()
                .map(|(name, p)| (name.clone(), rename_pattern(p, value_renames)))
                .collect(),
        ),
    }
}

fn rename_expr(
    expr: &Expr,
    bound: &mut BTreeSet<Symbol>,
    value_renames: &BTreeMap<Symbol, Symbol>,
    type_renames: &BTreeMap<Symbol, Symbol>,
    class_renames: &BTreeMap<Symbol, Symbol>,
) -> Expr {
    match expr {
        Expr::Bool(span, v) => Expr::Bool(*span, *v),
        Expr::Uint(span, v) => Expr::Uint(*span, *v),
        Expr::Int(span, v) => Expr::Int(*span, *v),
        Expr::Float(span, v) => Expr::Float(*span, *v),
        Expr::String(span, v) => Expr::String(*span, v.clone()),
        Expr::Uuid(span, v) => Expr::Uuid(*span, *v),
        Expr::DateTime(span, v) => Expr::DateTime(*span, *v),
        Expr::Hole(span) => Expr::Hole(*span),
        Expr::Tuple(span, elems) => Expr::Tuple(
            *span,
            elems
                .iter()
                .map(|e| {
                    Arc::new(rename_expr(
                        e,
                        bound,
                        value_renames,
                        type_renames,
                        class_renames,
                    ))
                })
                .collect(),
        ),
        Expr::List(span, elems) => Expr::List(
            *span,
            elems
                .iter()
                .map(|e| {
                    Arc::new(rename_expr(
                        e,
                        bound,
                        value_renames,
                        type_renames,
                        class_renames,
                    ))
                })
                .collect(),
        ),
        Expr::Dict(span, kvs) => Expr::Dict(
            *span,
            kvs.iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        Arc::new(rename_expr(
                            v,
                            bound,
                            value_renames,
                            type_renames,
                            class_renames,
                        )),
                    )
                })
                .collect(),
        ),
        Expr::RecordUpdate(span, base, updates) => Expr::RecordUpdate(
            *span,
            Arc::new(rename_expr(
                base,
                bound,
                value_renames,
                type_renames,
                class_renames,
            )),
            updates
                .iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        Arc::new(rename_expr(
                            v,
                            bound,
                            value_renames,
                            type_renames,
                            class_renames,
                        )),
                    )
                })
                .collect(),
        ),
        Expr::Var(v) => {
            if bound.contains(&v.name) {
                Expr::Var(v.clone())
            } else if let Some(new) = value_renames.get(&v.name) {
                Expr::Var(Var {
                    span: v.span,
                    name: new.clone(),
                })
            } else {
                Expr::Var(v.clone())
            }
        }
        Expr::App(span, f, x) => Expr::App(
            *span,
            Arc::new(rename_expr(
                f,
                bound,
                value_renames,
                type_renames,
                class_renames,
            )),
            Arc::new(rename_expr(
                x,
                bound,
                value_renames,
                type_renames,
                class_renames,
            )),
        ),
        Expr::Project(span, base, field) => Expr::Project(
            *span,
            Arc::new(rename_expr(
                base,
                bound,
                value_renames,
                type_renames,
                class_renames,
            )),
            field.clone(),
        ),
        Expr::Lam(span, scope, param, ann, constraints, body) => {
            bound.insert(param.name.clone());
            let out = Expr::Lam(
                *span,
                scope.clone(),
                param.clone(),
                ann.as_ref()
                    .map(|t| rename_type_expr(t, type_renames, class_renames)),
                rename_constraints(constraints, type_renames, class_renames),
                Arc::new(rename_expr(
                    body,
                    bound,
                    value_renames,
                    type_renames,
                    class_renames,
                )),
            );
            bound.remove(&param.name);
            out
        }
        Expr::Let(span, var, ann, val, body) => {
            let renamed_val = rename_expr(val, bound, value_renames, type_renames, class_renames);
            bound.insert(var.name.clone());
            let renamed_body = rename_expr(body, bound, value_renames, type_renames, class_renames);
            bound.remove(&var.name);
            Expr::Let(
                *span,
                var.clone(),
                ann.as_ref()
                    .map(|t| rename_type_expr(t, type_renames, class_renames)),
                Arc::new(renamed_val),
                Arc::new(renamed_body),
            )
        }
        Expr::LetRec(span, bindings, body) => {
            let names: Vec<Symbol> = bindings
                .iter()
                .map(|(var, _, _)| var.name.clone())
                .collect();
            for name in &names {
                bound.insert(name.clone());
            }
            let renamed_bindings = bindings
                .iter()
                .map(|(var, ann, def)| {
                    (
                        var.clone(),
                        ann.as_ref()
                            .map(|t| rename_type_expr(t, type_renames, class_renames)),
                        Arc::new(rename_expr(
                            def,
                            bound,
                            value_renames,
                            type_renames,
                            class_renames,
                        )),
                    )
                })
                .collect();
            let renamed_body = Arc::new(rename_expr(
                body,
                bound,
                value_renames,
                type_renames,
                class_renames,
            ));
            for name in &names {
                bound.remove(name);
            }
            Expr::LetRec(*span, renamed_bindings, renamed_body)
        }
        Expr::Ite(span, c, t, e) => Expr::Ite(
            *span,
            Arc::new(rename_expr(
                c,
                bound,
                value_renames,
                type_renames,
                class_renames,
            )),
            Arc::new(rename_expr(
                t,
                bound,
                value_renames,
                type_renames,
                class_renames,
            )),
            Arc::new(rename_expr(
                e,
                bound,
                value_renames,
                type_renames,
                class_renames,
            )),
        ),
        Expr::Match(span, scrutinee, arms) => {
            let scrutinee = Arc::new(rename_expr(
                scrutinee,
                bound,
                value_renames,
                type_renames,
                class_renames,
            ));
            let mut renamed_arms = Vec::new();
            for (pat, arm_expr) in arms {
                let pat_renamed = rename_pattern(pat, value_renames);
                let mut binds = Vec::new();
                collect_pattern_bindings(&pat_renamed, &mut binds);
                for b in &binds {
                    bound.insert(b.clone());
                }
                let arm_expr = Arc::new(rename_expr(
                    arm_expr,
                    bound,
                    value_renames,
                    type_renames,
                    class_renames,
                ));
                for b in &binds {
                    bound.remove(b);
                }
                renamed_arms.push((pat_renamed, arm_expr));
            }
            Expr::Match(*span, scrutinee, renamed_arms)
        }
        Expr::Ann(span, e, t) => Expr::Ann(
            *span,
            Arc::new(rename_expr(
                e,
                bound,
                value_renames,
                type_renames,
                class_renames,
            )),
            rename_type_expr(t, type_renames, class_renames),
        ),
    }
}

pub(crate) fn qualify_program(program: &Program, prefix: &str) -> Program {
    let (value_renames, type_renames, class_renames) = collect_local_renames(program, prefix);

    let decls = program
        .decls
        .iter()
        .filter_map(|decl| match decl {
            Decl::Import(..) => None,
            Decl::Type(td) => {
                let name = type_renames
                    .get(&td.name)
                    .cloned()
                    .unwrap_or_else(|| td.name.clone());
                let variants = td
                    .variants
                    .iter()
                    .map(|v| rex_ast::expr::TypeVariant {
                        name: value_renames
                            .get(&v.name)
                            .cloned()
                            .unwrap_or_else(|| v.name.clone()),
                        args: v
                            .args
                            .iter()
                            .map(|t| rename_type_expr(t, &type_renames, &class_renames))
                            .collect(),
                    })
                    .collect();
                Some(Decl::Type(TypeDecl {
                    span: td.span,
                    is_pub: td.is_pub,
                    name,
                    params: td.params.clone(),
                    variants,
                }))
            }
            Decl::Fn(fd) => {
                let name_sym = value_renames
                    .get(&fd.name.name)
                    .cloned()
                    .unwrap_or_else(|| fd.name.name.clone());
                let name = Var {
                    span: fd.name.span,
                    name: name_sym,
                };
                let params: Vec<(Var, TypeExpr)> = fd
                    .params
                    .iter()
                    .map(|(v, ann)| {
                        (
                            v.clone(),
                            rename_type_expr(ann, &type_renames, &class_renames),
                        )
                    })
                    .collect();
                let ret = rename_type_expr(&fd.ret, &type_renames, &class_renames);
                let constraints =
                    rename_constraints(&fd.constraints, &type_renames, &class_renames);
                let mut bound = BTreeSet::new();
                for (v, _) in &params {
                    bound.insert(v.name.clone());
                }
                let body = Arc::new(rename_expr(
                    fd.body.as_ref(),
                    &mut bound,
                    &value_renames,
                    &type_renames,
                    &class_renames,
                ));
                Some(Decl::Fn(FnDecl {
                    span: fd.span,
                    is_pub: fd.is_pub,
                    name,
                    params,
                    ret,
                    constraints,
                    body,
                }))
            }
            Decl::DeclareFn(df) => {
                let name_sym = value_renames
                    .get(&df.name.name)
                    .cloned()
                    .unwrap_or_else(|| df.name.name.clone());
                let name = Var {
                    span: df.name.span,
                    name: name_sym,
                };
                let params: Vec<(Var, TypeExpr)> = df
                    .params
                    .iter()
                    .map(|(v, ann)| {
                        (
                            v.clone(),
                            rename_type_expr(ann, &type_renames, &class_renames),
                        )
                    })
                    .collect();
                let ret = rename_type_expr(&df.ret, &type_renames, &class_renames);
                let constraints =
                    rename_constraints(&df.constraints, &type_renames, &class_renames);
                Some(Decl::DeclareFn(DeclareFnDecl {
                    span: df.span,
                    is_pub: df.is_pub,
                    name,
                    params,
                    ret,
                    constraints,
                }))
            }
            Decl::Class(cd) => {
                let name = class_renames
                    .get(&cd.name)
                    .cloned()
                    .unwrap_or_else(|| cd.name.clone());
                let supers = rename_constraints(&cd.supers, &type_renames, &class_renames);
                let methods = cd
                    .methods
                    .iter()
                    .map(|m| rex_ast::expr::ClassMethodSig {
                        name: m.name.clone(),
                        typ: rename_type_expr(&m.typ, &type_renames, &class_renames),
                    })
                    .collect();
                Some(Decl::Class(rex_ast::expr::ClassDecl {
                    span: cd.span,
                    is_pub: cd.is_pub,
                    name,
                    params: cd.params.clone(),
                    supers,
                    methods,
                }))
            }
            Decl::Instance(id) => {
                let class = class_renames
                    .get(&id.class)
                    .cloned()
                    .unwrap_or_else(|| id.class.clone());
                let head = rename_type_expr(&id.head, &type_renames, &class_renames);
                let context = rename_constraints(&id.context, &type_renames, &class_renames);
                let mut methods = Vec::new();
                for m in &id.methods {
                    let mut bound = BTreeSet::new();
                    let body = Arc::new(rename_expr(
                        m.body.as_ref(),
                        &mut bound,
                        &value_renames,
                        &type_renames,
                        &class_renames,
                    ));
                    methods.push(rex_ast::expr::InstanceMethodImpl {
                        name: m.name.clone(),
                        body,
                    });
                }
                Some(Decl::Instance(InstanceDecl {
                    span: id.span,
                    is_pub: id.is_pub,
                    class,
                    head,
                    context,
                    methods,
                }))
            }
        })
        .collect();

    let mut bound = BTreeSet::new();
    let expr = Arc::new(rename_expr(
        program.expr.as_ref(),
        &mut bound,
        &value_renames,
        &type_renames,
        &class_renames,
    ));

    Program { decls, expr }
}

fn alias_is_visible(
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

struct RewriteScope<'a> {
    aliases: &'a BTreeMap<Symbol, ModuleExports>,
    imported_values: &'a BTreeMap<Symbol, CanonicalSymbol>,
    imported_types: &'a BTreeMap<Symbol, CanonicalSymbol>,
    imported_classes: &'a BTreeMap<Symbol, CanonicalSymbol>,
    shadowed_types: Option<&'a BTreeSet<Symbol>>,
    shadowed_values: Option<&'a BTreeSet<Symbol>>,
}

fn rewrite_import_uses_expr(
    expr: &Expr,
    bound: &mut BTreeSet<Symbol>,
    scope: &RewriteScope<'_>,
) -> Expr {
    let rewrite_type = |ty: &TypeExpr, bound: &BTreeSet<Symbol>| {
        rewrite_import_uses_type_expr(
            ty,
            bound,
            scope.aliases,
            scope.imported_types,
            scope.shadowed_types,
            scope.shadowed_values,
        )
    };

    match expr {
        Expr::Bool(span, v) => Expr::Bool(*span, *v),
        Expr::Uint(span, v) => Expr::Uint(*span, *v),
        Expr::Int(span, v) => Expr::Int(*span, *v),
        Expr::Float(span, v) => Expr::Float(*span, *v),
        Expr::String(span, v) => Expr::String(*span, v.clone()),
        Expr::Uuid(span, v) => Expr::Uuid(*span, *v),
        Expr::DateTime(span, v) => Expr::DateTime(*span, *v),
        Expr::Hole(span) => Expr::Hole(*span),
        Expr::Project(span, base, field) => {
            if let Expr::Var(v) = base.as_ref()
                && alias_is_visible(&v.name, bound, scope.shadowed_values)
                && let Some(exports) = scope.aliases.get(&v.name)
                && let Some(internal) = exports.value(field)
            {
                return Expr::Var(Var {
                    span: *span,
                    name: internal.symbol().clone(),
                });
            }
            Expr::Project(
                *span,
                Arc::new(rewrite_import_uses_expr(base, bound, scope)),
                field.clone(),
            )
        }
        Expr::Var(v) => {
            if alias_is_visible(&v.name, bound, scope.shadowed_values)
                && let Some(internal) = scope.imported_values.get(&v.name)
            {
                Expr::Var(Var {
                    span: v.span,
                    name: internal.symbol().clone(),
                })
            } else {
                Expr::Var(v.clone())
            }
        }
        Expr::Lam(span, lam_scope, param, ann, constraints, body) => {
            let ann = ann.as_ref().map(|t| rewrite_type(t, bound));
            let constraints = constraints
                .iter()
                .map(|c| TypeConstraint {
                    class: rewrite_import_uses_class_name(
                        &c.class,
                        bound,
                        scope.aliases,
                        scope.imported_classes,
                        scope.shadowed_types,
                        scope.shadowed_values,
                    ),
                    typ: rewrite_type(&c.typ, bound),
                })
                .collect();
            bound.insert(param.name.clone());
            let out = Expr::Lam(
                *span,
                lam_scope.clone(),
                param.clone(),
                ann,
                constraints,
                Arc::new(rewrite_import_uses_expr(body, bound, scope)),
            );
            bound.remove(&param.name);
            out
        }
        Expr::Let(span, var, ann, val, body) => {
            let val = Arc::new(rewrite_import_uses_expr(val, bound, scope));
            bound.insert(var.name.clone());
            let body = Arc::new(rewrite_import_uses_expr(body, bound, scope));
            bound.remove(&var.name);
            Expr::Let(
                *span,
                var.clone(),
                ann.as_ref().map(|t| rewrite_type(t, bound)),
                val,
                body,
            )
        }
        Expr::LetRec(span, bindings, body) => {
            let anns: Vec<Option<TypeExpr>> = bindings
                .iter()
                .map(|(_, ann, _)| ann.as_ref().map(|t| rewrite_type(t, bound)))
                .collect();
            let names: Vec<Symbol> = bindings
                .iter()
                .map(|(var, _, _)| var.name.clone())
                .collect();
            for name in &names {
                bound.insert(name.clone());
            }
            let bindings = bindings
                .iter()
                .zip(anns)
                .map(|((var, _ann, def), ann)| {
                    (
                        var.clone(),
                        ann,
                        Arc::new(rewrite_import_uses_expr(def, bound, scope)),
                    )
                })
                .collect();
            let body = Arc::new(rewrite_import_uses_expr(body, bound, scope));
            for name in &names {
                bound.remove(name);
            }
            Expr::LetRec(*span, bindings, body)
        }
        Expr::Match(span, scrutinee, arms) => {
            let scrutinee = Arc::new(rewrite_import_uses_expr(scrutinee, bound, scope));
            let mut renamed_arms = Vec::new();
            for (pat, arm_expr) in arms {
                let pat = rewrite_import_uses_pattern(pat, scope.imported_values);
                let mut binds = Vec::new();
                collect_pattern_bindings(&pat, &mut binds);
                for b in &binds {
                    bound.insert(b.clone());
                }
                let arm_expr = Arc::new(rewrite_import_uses_expr(arm_expr, bound, scope));
                for b in &binds {
                    bound.remove(b);
                }
                renamed_arms.push((pat, arm_expr));
            }
            Expr::Match(*span, scrutinee, renamed_arms)
        }
        Expr::Tuple(span, elems) => Expr::Tuple(
            *span,
            elems
                .iter()
                .map(|e| Arc::new(rewrite_import_uses_expr(e, bound, scope)))
                .collect(),
        ),
        Expr::List(span, elems) => Expr::List(
            *span,
            elems
                .iter()
                .map(|e| Arc::new(rewrite_import_uses_expr(e, bound, scope)))
                .collect(),
        ),
        Expr::Dict(span, kvs) => Expr::Dict(
            *span,
            kvs.iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        Arc::new(rewrite_import_uses_expr(v, bound, scope)),
                    )
                })
                .collect(),
        ),
        Expr::RecordUpdate(span, base, updates) => Expr::RecordUpdate(
            *span,
            Arc::new(rewrite_import_uses_expr(base, bound, scope)),
            updates
                .iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        Arc::new(rewrite_import_uses_expr(v, bound, scope)),
                    )
                })
                .collect(),
        ),
        Expr::App(span, f, x) => Expr::App(
            *span,
            Arc::new(rewrite_import_uses_expr(f, bound, scope)),
            Arc::new(rewrite_import_uses_expr(x, bound, scope)),
        ),
        Expr::Ite(span, c, t, e) => Expr::Ite(
            *span,
            Arc::new(rewrite_import_uses_expr(c, bound, scope)),
            Arc::new(rewrite_import_uses_expr(t, bound, scope)),
            Arc::new(rewrite_import_uses_expr(e, bound, scope)),
        ),
        Expr::Ann(span, e, t) => Expr::Ann(
            *span,
            Arc::new(rewrite_import_uses_expr(e, bound, scope)),
            rewrite_type(t, bound),
        ),
    }
}

fn rewrite_import_uses_pattern(
    pat: &Pattern,
    imported_values: &BTreeMap<Symbol, CanonicalSymbol>,
) -> Pattern {
    match pat {
        Pattern::Wildcard(span) => Pattern::Wildcard(*span),
        Pattern::Var(v) => Pattern::Var(v.clone()),
        Pattern::Named(span, name, args) => {
            let name = imported_values
                .get(&name.to_dotted_symbol())
                .map(|c| NameRef::Unqualified(c.symbol().clone()))
                .unwrap_or_else(|| name.clone());
            let args = args
                .iter()
                .map(|p| rewrite_import_uses_pattern(p, imported_values))
                .collect();
            Pattern::Named(*span, name, args)
        }
        Pattern::Tuple(span, elems) => Pattern::Tuple(
            *span,
            elems
                .iter()
                .map(|p| rewrite_import_uses_pattern(p, imported_values))
                .collect(),
        ),
        Pattern::List(span, elems) => Pattern::List(
            *span,
            elems
                .iter()
                .map(|p| rewrite_import_uses_pattern(p, imported_values))
                .collect(),
        ),
        Pattern::Cons(span, head, tail) => Pattern::Cons(
            *span,
            Box::new(rewrite_import_uses_pattern(head, imported_values)),
            Box::new(rewrite_import_uses_pattern(tail, imported_values)),
        ),
        Pattern::Dict(span, fields) => Pattern::Dict(
            *span,
            fields
                .iter()
                .map(|(name, p)| {
                    (
                        name.clone(),
                        rewrite_import_uses_pattern(p, imported_values),
                    )
                })
                .collect(),
        ),
    }
}

fn rewrite_import_uses_class_name(
    class: &NameRef,
    bound: &BTreeSet<Symbol>,
    aliases: &BTreeMap<Symbol, ModuleExports>,
    imported_classes: &BTreeMap<Symbol, CanonicalSymbol>,
    shadowed_types: Option<&BTreeSet<Symbol>>,
    shadowed_values: Option<&BTreeSet<Symbol>>,
) -> NameRef {
    if let NameRef::Unqualified(name) = class {
        if shadowed_types.is_some_and(|shadowed| shadowed.contains(name)) {
            return class.clone();
        }
        if let Some(new) = imported_classes.get(name) {
            return NameRef::Unqualified(new.symbol().clone());
        }
        return class.clone();
    }
    let Some((alias_sym, member_sym)) = qualified_alias_member(class) else {
        return class.clone();
    };
    if !alias_is_visible(alias_sym, bound, shadowed_values) {
        return class.clone();
    }
    let Some(exports) = aliases.get(alias_sym) else {
        return class.clone();
    };
    exports
        .class(member_sym)
        .map(|s| s.symbol().clone())
        .map(NameRef::Unqualified)
        .unwrap_or_else(|| class.clone())
}

fn qualified_alias_member(name: &NameRef) -> Option<(&Symbol, &Symbol)> {
    match name {
        NameRef::Qualified(_, segments) if segments.len() == 2 => {
            Some((&segments[0], &segments[1]))
        }
        _ => None,
    }
}

fn rewrite_import_uses_type_expr(
    ty: &TypeExpr,
    bound: &BTreeSet<Symbol>,
    aliases: &BTreeMap<Symbol, ModuleExports>,
    imported_types: &BTreeMap<Symbol, CanonicalSymbol>,
    shadowed_types: Option<&BTreeSet<Symbol>>,
    shadowed_values: Option<&BTreeSet<Symbol>>,
) -> TypeExpr {
    match ty {
        TypeExpr::Name(span, name) => match name {
            NameRef::Unqualified(name) => {
                if shadowed_types.is_some_and(|shadowed| shadowed.contains(name)) {
                    return TypeExpr::Name(*span, NameRef::Unqualified(name.clone()));
                }
                if let Some(new) = imported_types.get(name) {
                    TypeExpr::Name(*span, NameRef::Unqualified(new.symbol().clone()))
                } else {
                    TypeExpr::Name(*span, NameRef::Unqualified(name.clone()))
                }
            }
            _ => {
                let Some((alias_sym, member_sym)) = qualified_alias_member(name) else {
                    return TypeExpr::Name(*span, name.clone());
                };
                if !alias_is_visible(alias_sym, bound, shadowed_values) {
                    return TypeExpr::Name(*span, name.clone());
                }
                let Some(exports) = aliases.get(alias_sym) else {
                    return TypeExpr::Name(*span, name.clone());
                };
                if let Some(new) = exports.typ(member_sym) {
                    TypeExpr::Name(*span, NameRef::Unqualified(new.symbol().clone()))
                } else if let Some(new) = exports.class(member_sym) {
                    TypeExpr::Name(*span, NameRef::Unqualified(new.symbol().clone()))
                } else {
                    TypeExpr::Name(*span, name.clone())
                }
            }
        },
        TypeExpr::App(span, f, x) => TypeExpr::App(
            *span,
            Box::new(rewrite_import_uses_type_expr(
                f,
                bound,
                aliases,
                imported_types,
                shadowed_types,
                shadowed_values,
            )),
            Box::new(rewrite_import_uses_type_expr(
                x,
                bound,
                aliases,
                imported_types,
                shadowed_types,
                shadowed_values,
            )),
        ),
        TypeExpr::Fun(span, a, b) => TypeExpr::Fun(
            *span,
            Box::new(rewrite_import_uses_type_expr(
                a,
                bound,
                aliases,
                imported_types,
                shadowed_types,
                shadowed_values,
            )),
            Box::new(rewrite_import_uses_type_expr(
                b,
                bound,
                aliases,
                imported_types,
                shadowed_types,
                shadowed_values,
            )),
        ),
        TypeExpr::Tuple(span, elems) => TypeExpr::Tuple(
            *span,
            elems
                .iter()
                .map(|e| {
                    rewrite_import_uses_type_expr(
                        e,
                        bound,
                        aliases,
                        imported_types,
                        shadowed_types,
                        shadowed_values,
                    )
                })
                .collect(),
        ),
        TypeExpr::Record(span, fields) => TypeExpr::Record(
            *span,
            fields
                .iter()
                .map(|(name, ty)| {
                    (
                        name.clone(),
                        rewrite_import_uses_type_expr(
                            ty,
                            bound,
                            aliases,
                            imported_types,
                            shadowed_types,
                            shadowed_values,
                        ),
                    )
                })
                .collect(),
        ),
    }
}

pub(crate) fn rewrite_import_uses(
    program: &Program,
    aliases: &BTreeMap<Symbol, ModuleExports>,
    imported_values: &BTreeMap<Symbol, CanonicalSymbol>,
    imported_types: &BTreeMap<Symbol, CanonicalSymbol>,
    imported_classes: &BTreeMap<Symbol, CanonicalSymbol>,
    shadowed_types: Option<&BTreeSet<Symbol>>,
    shadowed_values: Option<&BTreeSet<Symbol>>,
) -> Program {
    let scope = RewriteScope {
        aliases,
        imported_values,
        imported_types,
        imported_classes,
        shadowed_types,
        shadowed_values,
    };
    let decl_bound = BTreeSet::new();
    let decls = program
        .decls
        .iter()
        .map(|decl| match decl {
            Decl::Fn(fd) => {
                let mut bound: BTreeSet<Symbol> =
                    fd.params.iter().map(|(v, _)| v.name.clone()).collect();
                let body = Arc::new(rewrite_import_uses_expr(
                    fd.body.as_ref(),
                    &mut bound,
                    &scope,
                ));
                Decl::Fn(FnDecl {
                    span: fd.span,
                    is_pub: fd.is_pub,
                    name: fd.name.clone(),
                    params: fd
                        .params
                        .iter()
                        .map(|(v, t)| {
                            (
                                v.clone(),
                                rewrite_import_uses_type_expr(
                                    t,
                                    &decl_bound,
                                    aliases,
                                    imported_types,
                                    shadowed_types,
                                    shadowed_values,
                                ),
                            )
                        })
                        .collect(),
                    ret: rewrite_import_uses_type_expr(
                        &fd.ret,
                        &decl_bound,
                        aliases,
                        imported_types,
                        shadowed_types,
                        shadowed_values,
                    ),
                    constraints: fd
                        .constraints
                        .iter()
                        .map(|c| TypeConstraint {
                            class: rewrite_import_uses_class_name(
                                &c.class,
                                &decl_bound,
                                aliases,
                                imported_classes,
                                shadowed_types,
                                shadowed_values,
                            ),
                            typ: rewrite_import_uses_type_expr(
                                &c.typ,
                                &decl_bound,
                                aliases,
                                imported_types,
                                shadowed_types,
                                shadowed_values,
                            ),
                        })
                        .collect(),
                    body,
                })
            }
            Decl::DeclareFn(df) => Decl::DeclareFn(DeclareFnDecl {
                span: df.span,
                is_pub: df.is_pub,
                name: df.name.clone(),
                params: df
                    .params
                    .iter()
                    .map(|(v, t)| {
                        (
                            v.clone(),
                            rewrite_import_uses_type_expr(
                                t,
                                &decl_bound,
                                aliases,
                                imported_types,
                                shadowed_types,
                                shadowed_values,
                            ),
                        )
                    })
                    .collect(),
                ret: rewrite_import_uses_type_expr(
                    &df.ret,
                    &decl_bound,
                    aliases,
                    imported_types,
                    shadowed_types,
                    shadowed_values,
                ),
                constraints: df
                    .constraints
                    .iter()
                    .map(|c| TypeConstraint {
                        class: rewrite_import_uses_class_name(
                            &c.class,
                            &decl_bound,
                            aliases,
                            imported_classes,
                            shadowed_types,
                            shadowed_values,
                        ),
                        typ: rewrite_import_uses_type_expr(
                            &c.typ,
                            &decl_bound,
                            aliases,
                            imported_types,
                            shadowed_types,
                            shadowed_values,
                        ),
                    })
                    .collect(),
            }),
            Decl::Type(td) => Decl::Type(TypeDecl {
                span: td.span,
                is_pub: td.is_pub,
                name: td.name.clone(),
                params: td.params.clone(),
                variants: td
                    .variants
                    .iter()
                    .map(|v| rex_ast::expr::TypeVariant {
                        name: v.name.clone(),
                        args: v
                            .args
                            .iter()
                            .map(|t| {
                                rewrite_import_uses_type_expr(
                                    t,
                                    &decl_bound,
                                    aliases,
                                    imported_types,
                                    shadowed_types,
                                    shadowed_values,
                                )
                            })
                            .collect(),
                    })
                    .collect(),
            }),
            Decl::Class(cd) => Decl::Class(rex_ast::expr::ClassDecl {
                span: cd.span,
                is_pub: cd.is_pub,
                name: cd.name.clone(),
                params: cd.params.clone(),
                supers: cd
                    .supers
                    .iter()
                    .map(|c| TypeConstraint {
                        class: rewrite_import_uses_class_name(
                            &c.class,
                            &decl_bound,
                            aliases,
                            imported_classes,
                            shadowed_types,
                            shadowed_values,
                        ),
                        typ: rewrite_import_uses_type_expr(
                            &c.typ,
                            &decl_bound,
                            aliases,
                            imported_types,
                            shadowed_types,
                            shadowed_values,
                        ),
                    })
                    .collect(),
                methods: cd
                    .methods
                    .iter()
                    .map(|m| rex_ast::expr::ClassMethodSig {
                        name: m.name.clone(),
                        typ: rewrite_import_uses_type_expr(
                            &m.typ,
                            &decl_bound,
                            aliases,
                            imported_types,
                            shadowed_types,
                            shadowed_values,
                        ),
                    })
                    .collect(),
            }),
            Decl::Instance(inst) => {
                let methods = inst
                    .methods
                    .iter()
                    .map(|m| {
                        let mut bound = BTreeSet::new();
                        let body = Arc::new(rewrite_import_uses_expr(
                            m.body.as_ref(),
                            &mut bound,
                            &scope,
                        ));
                        rex_ast::expr::InstanceMethodImpl {
                            name: m.name.clone(),
                            body,
                        }
                    })
                    .collect();
                Decl::Instance(InstanceDecl {
                    span: inst.span,
                    is_pub: inst.is_pub,
                    class: rewrite_import_uses_class_name(
                        &NameRef::from_dotted(inst.class.as_ref()),
                        &decl_bound,
                        aliases,
                        imported_classes,
                        shadowed_types,
                        shadowed_values,
                    )
                    .to_dotted_symbol(),
                    head: rewrite_import_uses_type_expr(
                        &inst.head,
                        &decl_bound,
                        aliases,
                        imported_types,
                        shadowed_types,
                        shadowed_values,
                    ),
                    context: inst
                        .context
                        .iter()
                        .map(|c| TypeConstraint {
                            class: rewrite_import_uses_class_name(
                                &c.class,
                                &decl_bound,
                                aliases,
                                imported_classes,
                                shadowed_types,
                                shadowed_values,
                            ),
                            typ: rewrite_import_uses_type_expr(
                                &c.typ,
                                &decl_bound,
                                aliases,
                                imported_types,
                                shadowed_types,
                                shadowed_values,
                            ),
                        })
                        .collect(),
                    methods,
                })
            }
            other => other.clone(),
        })
        .collect();

    let mut bound = BTreeSet::new();
    let expr = Arc::new(rewrite_import_uses_expr(
        program.expr.as_ref(),
        &mut bound,
        &scope,
    ));
    Program { decls, expr }
}

fn validate_import_uses_expr(
    expr: &Expr,
    bound: &mut BTreeSet<Symbol>,
    aliases: &BTreeMap<Symbol, ModuleExports>,
    shadowed_values: Option<&BTreeSet<Symbol>>,
) -> Result<(), EngineError> {
    match expr {
        Expr::Project(_, base, field) => {
            if let Expr::Var(v) = base.as_ref()
                && alias_is_visible(&v.name, bound, shadowed_values)
                && let Some(exports) = aliases.get(&v.name)
                && exports.value(field).is_none()
            {
                return Err(crate::ModuleError::MissingExport {
                    module: v.name.clone(),
                    export: field.clone(),
                }
                .into());
            }
            validate_import_uses_expr(base, bound, aliases, shadowed_values)
        }
        Expr::Lam(_, _, param, ann, constraints, body) => {
            if let Some(ann) = ann {
                validate_import_uses_type_expr(ann, bound, aliases, shadowed_values)?;
            }
            for c in constraints {
                validate_import_uses_class_name(&c.class, bound, aliases, shadowed_values)?;
                validate_import_uses_type_expr(&c.typ, bound, aliases, shadowed_values)?;
            }
            bound.insert(param.name.clone());
            let res = validate_import_uses_expr(body, bound, aliases, shadowed_values);
            bound.remove(&param.name);
            res
        }
        Expr::Let(_, var, ann, val, body) => {
            if let Some(ann) = ann {
                validate_import_uses_type_expr(ann, bound, aliases, shadowed_values)?;
            }
            validate_import_uses_expr(val, bound, aliases, shadowed_values)?;
            bound.insert(var.name.clone());
            let res = validate_import_uses_expr(body, bound, aliases, shadowed_values);
            bound.remove(&var.name);
            res
        }
        Expr::LetRec(_, bindings, body) => {
            for (_, ann, _) in bindings {
                if let Some(ann) = ann {
                    validate_import_uses_type_expr(ann, bound, aliases, shadowed_values)?;
                }
            }
            let names: Vec<Symbol> = bindings
                .iter()
                .map(|(var, _, _)| var.name.clone())
                .collect();
            for name in &names {
                bound.insert(name.clone());
            }
            for (_, _ann, def) in bindings {
                validate_import_uses_expr(def, bound, aliases, shadowed_values)?;
            }
            let res = validate_import_uses_expr(body, bound, aliases, shadowed_values);
            for name in &names {
                bound.remove(name);
            }
            res
        }
        Expr::Match(_, scrutinee, arms) => {
            validate_import_uses_expr(scrutinee, bound, aliases, shadowed_values)?;
            for (pat, arm_expr) in arms {
                let mut binds = Vec::new();
                collect_pattern_bindings(pat, &mut binds);
                for b in &binds {
                    bound.insert(b.clone());
                }
                let res = validate_import_uses_expr(arm_expr, bound, aliases, shadowed_values);
                for b in &binds {
                    bound.remove(b);
                }
                res?;
            }
            Ok(())
        }
        Expr::Tuple(_, elems) | Expr::List(_, elems) => {
            for e in elems {
                validate_import_uses_expr(e, bound, aliases, shadowed_values)?;
            }
            Ok(())
        }
        Expr::Dict(_, kvs) => {
            for v in kvs.values() {
                validate_import_uses_expr(v, bound, aliases, shadowed_values)?;
            }
            Ok(())
        }
        Expr::RecordUpdate(_, base, updates) => {
            validate_import_uses_expr(base, bound, aliases, shadowed_values)?;
            for v in updates.values() {
                validate_import_uses_expr(v, bound, aliases, shadowed_values)?;
            }
            Ok(())
        }
        Expr::App(_, f, x) => {
            validate_import_uses_expr(f, bound, aliases, shadowed_values)?;
            validate_import_uses_expr(x, bound, aliases, shadowed_values)
        }
        Expr::Ite(_, c, t, e) => {
            validate_import_uses_expr(c, bound, aliases, shadowed_values)?;
            validate_import_uses_expr(t, bound, aliases, shadowed_values)?;
            validate_import_uses_expr(e, bound, aliases, shadowed_values)
        }
        Expr::Ann(_, e, t) => {
            validate_import_uses_expr(e, bound, aliases, shadowed_values)?;
            validate_import_uses_type_expr(t, bound, aliases, shadowed_values)
        }
        Expr::Var(..)
        | Expr::Bool(..)
        | Expr::Uint(..)
        | Expr::Int(..)
        | Expr::Float(..)
        | Expr::String(..)
        | Expr::Uuid(..)
        | Expr::DateTime(..)
        | Expr::Hole(..) => Ok(()),
    }
}

fn validate_import_uses_class_name(
    class: &NameRef,
    bound: &BTreeSet<Symbol>,
    aliases: &BTreeMap<Symbol, ModuleExports>,
    shadowed_values: Option<&BTreeSet<Symbol>>,
) -> Result<(), EngineError> {
    let Some((alias_sym, member_sym)) = qualified_alias_member(class) else {
        return Ok(());
    };
    if !alias_is_visible(alias_sym, bound, shadowed_values) {
        return Ok(());
    }
    let Some(exports) = aliases.get(alias_sym) else {
        return Ok(());
    };
    if exports.class(member_sym).is_some() {
        return Ok(());
    }
    Err(crate::ModuleError::MissingExport {
        module: alias_sym.clone(),
        export: member_sym.clone(),
    }
    .into())
}

fn validate_import_uses_type_expr(
    ty: &TypeExpr,
    bound: &BTreeSet<Symbol>,
    aliases: &BTreeMap<Symbol, ModuleExports>,
    shadowed_values: Option<&BTreeSet<Symbol>>,
) -> Result<(), EngineError> {
    match ty {
        TypeExpr::Name(_, name) => {
            let Some((alias_sym, member_sym)) = qualified_alias_member(name) else {
                return Ok(());
            };
            if !alias_is_visible(alias_sym, bound, shadowed_values) {
                return Ok(());
            }
            let Some(exports) = aliases.get(alias_sym) else {
                return Ok(());
            };
            if exports.typ(member_sym).is_some() || exports.class(member_sym).is_some() {
                Ok(())
            } else {
                Err(crate::ModuleError::MissingExport {
                    module: alias_sym.clone(),
                    export: member_sym.clone(),
                }
                .into())
            }
        }
        TypeExpr::App(_, f, x) => {
            validate_import_uses_type_expr(f, bound, aliases, shadowed_values)?;
            validate_import_uses_type_expr(x, bound, aliases, shadowed_values)
        }
        TypeExpr::Fun(_, a, b) => {
            validate_import_uses_type_expr(a, bound, aliases, shadowed_values)?;
            validate_import_uses_type_expr(b, bound, aliases, shadowed_values)
        }
        TypeExpr::Tuple(_, elems) => {
            for e in elems {
                validate_import_uses_type_expr(e, bound, aliases, shadowed_values)?;
            }
            Ok(())
        }
        TypeExpr::Record(_, fields) => {
            for (_, t) in fields {
                validate_import_uses_type_expr(t, bound, aliases, shadowed_values)?;
            }
            Ok(())
        }
    }
}

pub(crate) fn validate_import_uses(
    program: &Program,
    aliases: &BTreeMap<Symbol, ModuleExports>,
    shadowed_values: Option<&BTreeSet<Symbol>>,
) -> Result<(), EngineError> {
    for decl in &program.decls {
        match decl {
            Decl::Fn(fd) => {
                for (_, t) in &fd.params {
                    validate_import_uses_type_expr(t, &BTreeSet::new(), aliases, shadowed_values)?;
                }
                validate_import_uses_type_expr(
                    &fd.ret,
                    &BTreeSet::new(),
                    aliases,
                    shadowed_values,
                )?;
                for c in &fd.constraints {
                    validate_import_uses_class_name(
                        &c.class,
                        &BTreeSet::new(),
                        aliases,
                        shadowed_values,
                    )?;
                    validate_import_uses_type_expr(
                        &c.typ,
                        &BTreeSet::new(),
                        aliases,
                        shadowed_values,
                    )?;
                }
                let mut bound: BTreeSet<Symbol> =
                    fd.params.iter().map(|(v, _)| v.name.clone()).collect();
                validate_import_uses_expr(fd.body.as_ref(), &mut bound, aliases, shadowed_values)?;
            }
            Decl::DeclareFn(df) => {
                for (_, t) in &df.params {
                    validate_import_uses_type_expr(t, &BTreeSet::new(), aliases, shadowed_values)?;
                }
                validate_import_uses_type_expr(
                    &df.ret,
                    &BTreeSet::new(),
                    aliases,
                    shadowed_values,
                )?;
                for c in &df.constraints {
                    validate_import_uses_class_name(
                        &c.class,
                        &BTreeSet::new(),
                        aliases,
                        shadowed_values,
                    )?;
                    validate_import_uses_type_expr(
                        &c.typ,
                        &BTreeSet::new(),
                        aliases,
                        shadowed_values,
                    )?;
                }
            }
            Decl::Type(td) => {
                for v in &td.variants {
                    for t in &v.args {
                        validate_import_uses_type_expr(
                            t,
                            &BTreeSet::new(),
                            aliases,
                            shadowed_values,
                        )?;
                    }
                }
            }
            Decl::Class(cd) => {
                for c in &cd.supers {
                    validate_import_uses_class_name(
                        &c.class,
                        &BTreeSet::new(),
                        aliases,
                        shadowed_values,
                    )?;
                    validate_import_uses_type_expr(
                        &c.typ,
                        &BTreeSet::new(),
                        aliases,
                        shadowed_values,
                    )?;
                }
                for m in &cd.methods {
                    validate_import_uses_type_expr(
                        &m.typ,
                        &BTreeSet::new(),
                        aliases,
                        shadowed_values,
                    )?;
                }
            }
            Decl::Instance(inst) => {
                validate_import_uses_class_name(
                    &NameRef::from_dotted(inst.class.as_ref()),
                    &BTreeSet::new(),
                    aliases,
                    shadowed_values,
                )?;
                validate_import_uses_type_expr(
                    &inst.head,
                    &BTreeSet::new(),
                    aliases,
                    shadowed_values,
                )?;
                for c in &inst.context {
                    validate_import_uses_class_name(
                        &c.class,
                        &BTreeSet::new(),
                        aliases,
                        shadowed_values,
                    )?;
                    validate_import_uses_type_expr(
                        &c.typ,
                        &BTreeSet::new(),
                        aliases,
                        shadowed_values,
                    )?;
                }
                for m in &inst.methods {
                    let mut bound = BTreeSet::new();
                    validate_import_uses_expr(
                        m.body.as_ref(),
                        &mut bound,
                        aliases,
                        shadowed_values,
                    )?;
                }
            }
            Decl::Import(..) => {}
        }
    }
    let mut bound = BTreeSet::new();
    validate_import_uses_expr(program.expr.as_ref(), &mut bound, aliases, shadowed_values)
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

pub(crate) fn interface_decls_from_program(program: &Program) -> Vec<Decl> {
    let mut out = Vec::new();
    for decl in &program.decls {
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

fn graph_imports_for_program(program: &Program, default_imports: &[String]) -> Vec<ImportDecl> {
    let mut out = Vec::new();
    for decl in &program.decls {
        if let Decl::Import(import_decl) = decl {
            out.push(import_decl.clone());
        }
    }
    for module_name in default_imports {
        let alias = intern(module_name);
        if contains_import_alias(&program.decls, &alias) {
            continue;
        }
        out.push(default_import_decl(module_name));
    }
    out
}

fn tarjan_scc_module_ids(
    nodes: &[ModuleId],
    edges: &BTreeMap<ModuleId, Vec<ModuleId>>,
) -> Vec<Vec<ModuleId>> {
    // Tarjan's SCC algorithm (linear in |V| + |E|).
    //
    // References:
    // - Tarjan, R. E. (1972). "Depth-first search and linear graph algorithms."
    //   SIAM Journal on Computing, 1(2), 146-160.
    // - Cormen et al. (CLRS), 3rd ed., §22.5 "Strongly connected components".
    //
    // Why Tarjan here:
    // - We need explicit SCC groups to process module cycles as units.
    // - We want one DFS pass with low overhead because this runs in module loading paths.
    #[derive(Default)]
    struct TarjanState {
        index: usize,
        index_of: BTreeMap<ModuleId, usize>,
        lowlink: BTreeMap<ModuleId, usize>,
        stack: Vec<ModuleId>,
        on_stack: BTreeSet<ModuleId>,
        components: Vec<Vec<ModuleId>>,
    }

    fn strong_connect(
        v: &ModuleId,
        edges: &BTreeMap<ModuleId, Vec<ModuleId>>,
        st: &mut TarjanState,
    ) {
        st.index_of.insert(v.clone(), st.index);
        st.lowlink.insert(v.clone(), st.index);
        st.index += 1;

        st.stack.push(v.clone());
        st.on_stack.insert(v.clone());

        if let Some(neighbors) = edges.get(v) {
            for w in neighbors {
                if !st.index_of.contains_key(w) {
                    strong_connect(w, edges, st);
                    let lw = st.lowlink.get(w).copied();
                    if let (Some(lw), Some(lv)) = (lw, st.lowlink.get_mut(v)) {
                        *lv = (*lv).min(lw);
                    }
                } else if st.on_stack.contains(w) {
                    let iw = st.index_of.get(w).copied();
                    if let (Some(iw), Some(lv)) = (iw, st.lowlink.get_mut(v)) {
                        *lv = (*lv).min(iw);
                    }
                }
            }
        }

        // Root of an SCC when lowlink(v) == index(v): pop until we get v.
        let is_root = st.lowlink.get(v) == st.index_of.get(v);
        if is_root {
            let mut component = Vec::new();
            while let Some(w) = st.stack.pop() {
                st.on_stack.remove(&w);
                component.push(w.clone());
                if &w == v {
                    break;
                }
            }
            st.components.push(component);
        }
    }

    let mut st = TarjanState::default();
    for node in nodes {
        if !st.index_of.contains_key(node) {
            strong_connect(node, edges, &mut st);
        }
    }
    st.components
}

pub(crate) fn exports_from_program(
    program: &Program,
    prefix: &str,
    module_id: &ModuleId,
) -> ModuleExports {
    let (value_renames, type_renames, class_renames) = collect_local_renames(program, prefix);
    let module_key = module_key_for_module(module_id);

    let mut exports = ModuleExports::default();

    for decl in &program.decls {
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
    gas: Option<&mut GasMeter>,
) -> Result<Program, EngineError> {
    let tokens = Token::tokenize(source).map_err(|e| match context {
        Some(id) => EngineError::from(crate::ModuleError::LexInModule {
            module: id.clone(),
            source: e,
        }),
        None => EngineError::from(crate::ModuleError::Lex { source: e }),
    })?;
    let mut parser = RexParser::new(tokens);
    let program = match gas {
        Some(gas) => parser.parse_program(gas),
        None => parser.parse_program(&mut GasMeter::default()),
    }
    .map_err(|errs| match context {
        Some(id) => EngineError::from(crate::ModuleError::ParseInModule {
            module: id.clone(),
            errors: errs,
        }),
        None => EngineError::from(crate::ModuleError::Parse { errors: errs }),
    })?;
    if let Some(module) = context
        && !matches!(program.expr.as_ref(), Expr::Tuple(_, elems) if elems.is_empty())
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
    gas: &mut GasMeter,
) -> Result<Program, EngineError> {
    match &resolved.content {
        ResolvedModuleContent::Source(source) => {
            parse_program_from_source(source, Some(&resolved.id), Some(gas))
        }
        ResolvedModuleContent::Program(program) => {
            if !matches!(program.expr.as_ref(), Expr::Tuple(_, elems) if elems.is_empty()) {
                return Err(crate::ModuleError::TopLevelExprInModule {
                    module: resolved.id.clone(),
                }
                .into());
            }
            Ok(program.clone())
        }
    }
}

impl<State> Engine<State>
where
    State: Clone + Send + Sync + 'static,
{
    fn source_fingerprint(source: &str) -> String {
        sha256_hex(source.as_bytes())
    }

    fn content_fingerprint(resolved: &ResolvedModule) -> Option<String> {
        match &resolved.content {
            ResolvedModuleContent::Source(source) => Some(Self::source_fingerprint(source)),
            ResolvedModuleContent::Program(_) => None,
        }
    }

    fn refresh_if_stale(
        &mut self,
        resolved: &ResolvedModule,
    ) -> Result<Option<String>, EngineError> {
        let Some(next) = Self::content_fingerprint(resolved) else {
            return Ok(None);
        };
        if let Some(prev) = self.module_source_fingerprints.get(&resolved.id)
            && prev != &next
        {
            self.invalidate_module_caches(&resolved.id)?;
        }
        Ok(Some(next))
    }

    fn remove_type_level_symbols_for_module_interface(&mut self, decls: &[Decl]) {
        for decl in decls {
            match decl {
                Decl::Fn(fd) => {
                    self.type_system.env.remove(&fd.name.name);
                    self.type_system.declared_values.remove(&fd.name.name);
                }
                Decl::DeclareFn(df) => {
                    self.type_system.env.remove(&df.name.name);
                    self.type_system.declared_values.remove(&df.name.name);
                }
                Decl::Type(td) => {
                    self.type_system.adts.remove(&td.name);
                    for variant in &td.variants {
                        self.type_system.env.remove(&variant.name);
                        self.type_system.declared_values.remove(&variant.name);
                    }
                }
                Decl::Class(cd) => {
                    self.type_system.classes.classes.remove(&cd.name);
                    self.type_system.classes.instances.remove(&cd.name);
                    self.type_system.class_info.remove(&cd.name);
                    for method in &cd.methods {
                        self.type_system.env.remove(&method.name);
                        self.type_system.class_methods.remove(&method.name);
                    }
                }
                Decl::Import(..) | Decl::Instance(..) => {}
            }
        }
    }

    pub(crate) fn invalidate_module_caches(&mut self, id: &ModuleId) -> Result<(), EngineError> {
        if let Some(prev_interface) = self.module_interface_cache.get(id).cloned() {
            self.remove_type_level_symbols_for_module_interface(&prev_interface);
        }
        self.modules.invalidate(id)?;
        self.module_exports_cache.remove(id);
        self.module_interface_cache.remove(id);
        self.module_sources.remove(id);
        self.module_source_fingerprints.remove(id);
        self.published_cycle_interfaces.remove(id);
        Ok(())
    }

    fn load_module_types_via_scc(
        &mut self,
        root: ResolvedModule,
        gas: &mut GasMeter,
        loaded: &mut BTreeMap<ModuleId, ModuleExports>,
        loading: &mut BTreeSet<ModuleId>,
    ) -> Result<ModuleExports, EngineError> {
        #[derive(Clone)]
        struct PendingModule {
            resolved: ResolvedModule,
            program: Program,
            prefix: String,
        }

        if let Some(exports) = loaded.get(&root.id)
            && !loading.contains(&root.id)
        {
            return Ok(exports.clone());
        }

        let mut pending: BTreeMap<ModuleId, PendingModule> = BTreeMap::new();
        let mut edges: BTreeMap<ModuleId, Vec<ModuleId>> = BTreeMap::new();
        let mut stack = vec![root.clone()];

        while let Some(resolved) = stack.pop() {
            let fingerprint = self.refresh_if_stale(&resolved)?;
            if pending.contains_key(&resolved.id) {
                continue;
            }
            if loaded.contains_key(&resolved.id) && !loading.contains(&resolved.id) {
                continue;
            }

            let prefix = prefix_for_module(&resolved.id);
            let program = program_from_resolved(&resolved, &mut *gas)?;
            let exports = exports_from_program(&program, &prefix, &resolved.id);
            loaded.insert(resolved.id.clone(), exports);
            loading.insert(resolved.id.clone());
            if let ResolvedModuleContent::Source(source) = &resolved.content {
                self.module_sources
                    .insert(resolved.id.clone(), source.clone());
            }
            let qualified = qualify_program(&program, &prefix);
            let interfaces = interface_decls_from_program(&qualified);
            self.module_interface_cache
                .insert(resolved.id.clone(), interfaces);

            let imports = graph_imports_for_program(&program, self.default_imports());
            for import_decl in imports {
                let spec = import_specifier(&import_decl.path);
                if self.virtual_module_exports(spec_base_name(&spec)).is_some() {
                    continue;
                }
                let imported = self.modules.resolve(ResolveRequest {
                    module_name: spec,
                    importer: Some(resolved.id.clone()),
                })?;
                edges
                    .entry(resolved.id.clone())
                    .or_default()
                    .push(imported.id.clone());
                if (loading.contains(&imported.id) || !loaded.contains_key(&imported.id))
                    && !pending.contains_key(&imported.id)
                {
                    stack.push(imported);
                }
            }

            let module_id = resolved.id.clone();
            pending.insert(
                module_id.clone(),
                PendingModule {
                    resolved,
                    program,
                    prefix,
                },
            );
            if let Some(fingerprint) = fingerprint {
                self.module_source_fingerprints
                    .insert(module_id, fingerprint);
            }
        }

        if pending.is_empty() {
            return loaded.get(&root.id).cloned().ok_or_else(|| {
                EngineError::Internal("missing module exports after SCC load".into())
            });
        }

        let pending_ids: Vec<ModuleId> = pending.keys().cloned().collect();
        let sccs = tarjan_scc_module_ids(&pending_ids, &edges);

        // Tarjan yields SCCs in reverse topological order of the SCC DAG, so
        // dependencies are processed before dependents.
        for component in sccs {
            let has_cycle = component.len() > 1;
            if has_cycle {
                for module_id in &component {
                    self.ensure_cycle_interfaces_published(module_id)?;
                }
            }
            for module_id in &component {
                let node = pending
                    .get(module_id)
                    .ok_or_else(|| EngineError::Internal("missing pending module node".into()))?;
                let rewritten = self.rewrite_program_with_imports(
                    &node.program,
                    Some(node.resolved.id.clone()),
                    &node.prefix,
                    gas,
                    loaded,
                    loading,
                )?;
                self.inject_decls(&rewritten.decls)?;
            }
            for module_id in component {
                loading.remove(&module_id);
            }
        }

        loaded
            .get(&root.id)
            .cloned()
            .ok_or_else(|| EngineError::Internal("missing root exports after SCC load".into()))
    }

    fn ensure_cycle_interfaces_published(
        &mut self,
        module_id: &ModuleId,
    ) -> Result<(), EngineError> {
        if self.published_cycle_interfaces.contains(module_id) {
            return Ok(());
        }
        let Some(decls) = self.module_interface_cache.get(module_id).cloned() else {
            return Ok(());
        };
        self.inject_decls(&decls)?;
        self.publish_runtime_interfaces(&decls)?;
        self.published_cycle_interfaces.insert(module_id.clone());
        Ok(())
    }

    fn publish_runtime_interfaces(&mut self, decls: &[Decl]) -> Result<(), EngineError> {
        let mut signatures = Vec::new();
        for decl in decls {
            let Decl::DeclareFn(df) = decl else {
                continue;
            };
            signatures.push(df.clone());
        }
        self.publish_runtime_decl_interfaces(&signatures)
    }

    #[async_recursion]
    async fn resolve_module_exports_from_import_decl_async(
        &mut self,
        import_decl: &ImportDecl,
        importer: Option<ModuleId>,
        gas: &mut GasMeter,
    ) -> Result<ModuleExports, EngineError> {
        let spec = import_specifier(&import_decl.path);
        if let Some(exports) = self.virtual_module_exports(spec_base_name(&spec)) {
            return Ok(exports);
        }
        let imported = self.modules.resolve(ResolveRequest {
            module_name: spec,
            importer,
        })?;
        self.refresh_if_stale(&imported)?;
        if let Some(exports) = self.module_exports_cache.get(&imported.id).cloned() {
            self.ensure_cycle_interfaces_published(&imported.id)?;
            return Ok(exports);
        }
        let inst = self.load_module_from_resolved(imported, gas).await?;
        Ok(inst.exports)
    }

    async fn add_default_import_bindings(
        &mut self,
        bindings: &mut ImportBindings,
        decls: &[Decl],
        importer: Option<ModuleId>,
        policy: &ImportBindingPolicy<'_>,
        gas: &mut GasMeter,
    ) -> Result<(), EngineError> {
        let existing_value_names: BTreeSet<Symbol> =
            policy.existing_imported_values.cloned().unwrap_or_default();
        let default_imports = self.default_imports().to_vec();
        for module_name in default_imports {
            let alias = intern(&module_name);
            if contains_import_alias(decls, &alias) {
                continue;
            }
            let import_decl = default_import_decl(&module_name);
            let exports = self
                .resolve_module_exports_from_import_decl_async(&import_decl, importer.clone(), gas)
                .await?;
            for (local, target) in exports.values() {
                if !policy.forbidden_values.contains(local)
                    && !existing_value_names.contains(local)
                    && !bindings.imported_values.contains_key(local)
                {
                    bindings
                        .imported_values
                        .insert(local.clone(), target.clone());
                }
            }
            for (local, target) in exports.types() {
                if !policy.forbidden_types.contains(local)
                    && !policy
                        .existing_imported_types
                        .is_some_and(|names| names.contains(local))
                    && !bindings.imported_types.contains_key(local)
                {
                    bindings
                        .imported_types
                        .insert(local.clone(), target.clone());
                }
            }
            for (local, target) in exports.classes() {
                if !policy.forbidden_types.contains(local)
                    && !policy
                        .existing_imported_classes
                        .is_some_and(|names| names.contains(local))
                    && !bindings.imported_classes.contains_key(local)
                {
                    bindings
                        .imported_classes
                        .insert(local.clone(), target.clone());
                }
            }
        }
        Ok(())
    }

    pub fn add_resolver<F>(&mut self, name: impl Into<String>, f: F)
    where
        F: Fn(ResolveRequest) -> Result<Option<ResolvedModule>, EngineError>
            + Send
            + Sync
            + 'static,
    {
        self.modules.add_resolver(name, wrap_resolver(f));
    }

    pub fn add_default_resolvers(&mut self) {
        self.modules
            .add_resolver("stdlib", default_stdlib_resolver());

        #[cfg(not(target_arch = "wasm32"))]
        self.modules.add_resolver("local", default_local_resolver());

        #[cfg(all(not(target_arch = "wasm32"), feature = "github-imports"))]
        {
            self.modules
                .add_resolver("remote", default_github_resolver());
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn add_include_resolver(&mut self, root: impl AsRef<Path>) -> Result<(), EngineError> {
        let canon =
            root.as_ref()
                .canonicalize()
                .map_err(|e| crate::ModuleError::InvalidIncludeRoot {
                    path: root.as_ref().to_path_buf(),
                    source: e,
                })?;
        self.modules.add_resolver(
            format!("include:{}", canon.display()),
            include_resolver(canon),
        );
        Ok(())
    }

    #[cfg(target_arch = "wasm32")]
    pub fn add_include_resolver(&mut self, _root: impl AsRef<Path>) -> Result<(), EngineError> {
        Err(EngineError::UnsupportedExpr)
    }

    #[async_recursion]
    pub(crate) async fn import_bindings_for_decls(
        &mut self,
        decls: &[Decl],
        importer: Option<ModuleId>,
        policy: &ImportBindingPolicy<'_>,
        gas: &mut GasMeter,
    ) -> Result<ImportBindings, EngineError> {
        let mut bindings = ImportBindings::default();
        for decl in decls {
            let Decl::Import(import_decl) = decl else {
                continue;
            };
            let exports = self
                .resolve_module_exports_from_import_decl_async(import_decl, importer.clone(), gas)
                .await?;
            add_import_bindings(&mut bindings, import_decl, &exports, policy)?;
        }
        self.add_default_import_bindings(&mut bindings, decls, importer, policy, gas)
            .await?;
        Ok(bindings)
    }

    #[async_recursion]
    pub(crate) async fn load_module_from_resolved(
        &mut self,
        resolved: ResolvedModule,
        gas: &mut GasMeter,
    ) -> Result<ModuleInstance, EngineError> {
        let source_fingerprint = self.refresh_if_stale(&resolved)?;
        if let Some(inst) = self.modules.cached(&resolved.id)? {
            return Ok(inst);
        }

        self.modules.mark_loading(&resolved.id)?;

        let prefix = prefix_for_module(&resolved.id);
        let program = program_from_resolved(&resolved, gas)?;
        if let ResolvedModuleContent::Source(source) = &resolved.content {
            self.module_sources
                .insert(resolved.id.clone(), source.clone());
        }
        let exports = exports_from_program(&program, &prefix, &resolved.id);
        self.module_exports_cache
            .insert(resolved.id.clone(), exports.clone());
        let qualified = qualify_program(&program, &prefix);
        let interfaces = interface_decls_from_program(&qualified);
        self.module_interface_cache
            .insert(resolved.id.clone(), interfaces);

        // Resolve imports first so qualified names exist in the environment.
        let local_values = decl_value_names(&program.decls);
        let local_types = decl_type_names(&program.decls);
        let import_policy = ImportBindingPolicy {
            forbidden_values: &local_values,
            forbidden_types: &local_types,
            existing_imported_values: None,
            existing_imported_types: None,
            existing_imported_classes: None,
        };
        let import_bindings = self
            .import_bindings_for_decls(
                &program.decls,
                Some(resolved.id.clone()),
                &import_policy,
                gas,
            )
            .await?;

        // Qualify local names, then rewrite `alias.foo` uses into internal symbols.
        validate_import_uses(&qualified, &import_bindings.alias_exports, None)?;
        let rewritten = rewrite_import_uses(
            &qualified,
            &import_bindings.alias_exports,
            &import_bindings.imported_values,
            &import_bindings.imported_types,
            &import_bindings.imported_classes,
            Some(&local_types),
            None,
        );

        self.inject_decls(&rewritten.decls)?;
        let init_value = self.heap.alloc_tuple(vec![])?;
        let init_type = Type::tuple(vec![]);

        let inst = ModuleInstance {
            id: resolved.id.clone(),
            exports,
            init_value,
            init_type,
            source_fingerprint: source_fingerprint.clone(),
        };
        self.modules.store_loaded(inst.clone())?;
        if let Some(source_fingerprint) = source_fingerprint {
            self.module_source_fingerprints
                .insert(resolved.id.clone(), source_fingerprint);
        }
        Ok(inst)
    }

    fn load_module_types_from_resolved(
        &mut self,
        resolved: ResolvedModule,
        gas: &mut GasMeter,
        loaded: &mut BTreeMap<ModuleId, ModuleExports>,
        loading: &mut BTreeSet<ModuleId>,
    ) -> Result<ModuleExports, EngineError> {
        if let Some(exports) = loaded.get(&resolved.id) {
            return Ok(exports.clone());
        }

        if loading.contains(&resolved.id)
            && let Some(exports) = loaded.get(&resolved.id)
        {
            return Ok(exports.clone());
        }
        self.load_module_types_via_scc(resolved, gas, loaded, loading)
    }

    fn resolve_module_exports_for_rewrite(
        &mut self,
        import_decl: &ImportDecl,
        importer: Option<ModuleId>,
        gas: &mut GasMeter,
        loaded: &mut BTreeMap<ModuleId, ModuleExports>,
        loading: &mut BTreeSet<ModuleId>,
    ) -> Result<ModuleExports, EngineError> {
        let spec = import_specifier(&import_decl.path);
        if let Some(exports) = self.virtual_module_exports(spec_base_name(&spec)) {
            return Ok(exports);
        }
        let imported = self.modules.resolve(ResolveRequest {
            module_name: spec,
            importer,
        })?;
        self.refresh_if_stale(&imported)?;
        self.load_module_types_from_resolved(imported, gas, loaded, loading)
    }

    pub(crate) fn rewrite_program_with_imports(
        &mut self,
        program: &Program,
        importer: Option<ModuleId>,
        prefix: &str,
        gas: &mut GasMeter,
        loaded: &mut BTreeMap<ModuleId, ModuleExports>,
        loading: &mut BTreeSet<ModuleId>,
    ) -> Result<Program, EngineError> {
        let mut bindings = ImportBindings::default();
        let local_values = decl_value_names(&program.decls);
        let local_types = decl_type_names(&program.decls);
        let import_policy = ImportBindingPolicy {
            forbidden_values: &local_values,
            forbidden_types: &local_types,
            existing_imported_values: None,
            existing_imported_types: None,
            existing_imported_classes: None,
        };
        for decl in &program.decls {
            let Decl::Import(import_decl) = decl else {
                continue;
            };
            let exports = self.resolve_module_exports_for_rewrite(
                import_decl,
                importer.clone(),
                gas,
                loaded,
                loading,
            )?;
            add_import_bindings(&mut bindings, import_decl, &exports, &import_policy)?;
        }

        let default_imports = self.default_imports().to_vec();
        for module_name in default_imports {
            let alias = intern(&module_name);
            if contains_import_alias(&program.decls, &alias) {
                continue;
            }
            let import_decl = default_import_decl(&module_name);
            let exports = self.resolve_module_exports_for_rewrite(
                &import_decl,
                importer.clone(),
                gas,
                loaded,
                loading,
            )?;
            for (local, target) in exports.values() {
                if !local_values.contains(local) && !bindings.imported_values.contains_key(local) {
                    bindings
                        .imported_values
                        .insert(local.clone(), target.clone());
                }
            }
            for (local, target) in exports.types() {
                if !local_types.contains(local) && !bindings.imported_types.contains_key(local) {
                    bindings
                        .imported_types
                        .insert(local.clone(), target.clone());
                }
            }
            for (local, target) in exports.classes() {
                if !local_types.contains(local) && !bindings.imported_classes.contains_key(local) {
                    bindings
                        .imported_classes
                        .insert(local.clone(), target.clone());
                }
            }
        }

        let qualified = qualify_program(program, prefix);
        validate_import_uses(&qualified, &bindings.alias_exports, None)?;
        Ok(rewrite_import_uses(
            &qualified,
            &bindings.alias_exports,
            &bindings.imported_values,
            &bindings.imported_types,
            &bindings.imported_classes,
            Some(&local_types),
            None,
        ))
    }

    pub(crate) fn read_local_module_bytes(
        &self,
        path: &Path,
    ) -> Result<(ModuleId, Vec<u8>), EngineError> {
        let canon = path
            .canonicalize()
            .map_err(|e| crate::ModuleError::InvalidModulePath {
                path: path.to_path_buf(),
                source: e,
            })?;
        let bytes = std::fs::read(&canon).map_err(|e| crate::ModuleError::ReadFailed {
            path: canon.clone(),
            source: e,
        })?;
        Ok((ModuleId::Local { path: canon }, bytes))
    }

    pub(crate) fn decode_local_module_source(
        &self,
        id: &ModuleId,
        bytes: Vec<u8>,
    ) -> Result<String, EngineError> {
        let path = match id {
            ModuleId::Local { path, .. } => path.clone(),
            other => {
                return Err(EngineError::Internal(format!(
                    "decode_local_module_source called with non-local module id {other}"
                )));
            }
        };
        String::from_utf8(bytes).map_err(|e| {
            crate::ModuleError::NotUtf8 {
                kind: "local",
                path,
                source: e,
            }
            .into()
        })
    }

    pub fn infer_module_file(
        &mut self,
        path: impl AsRef<Path>,
        gas: &mut GasMeter,
    ) -> Result<(Vec<Predicate>, Type), CompileError> {
        let (id, bytes) = self.read_local_module_bytes(path.as_ref())?;
        let source = self.decode_local_module_source(&id, bytes)?;
        self.infer_module_source(
            ResolvedModule {
                id,
                content: ResolvedModuleContent::Source(source),
            },
            gas,
        )
        .map_err(CompileError::from)
    }

    fn infer_module_source(
        &mut self,
        resolved: ResolvedModule,
        gas: &mut GasMeter,
    ) -> Result<(Vec<Predicate>, Type), EngineError> {
        let mut loaded: BTreeMap<ModuleId, ModuleExports> = BTreeMap::new();
        let mut loading: BTreeSet<ModuleId> = BTreeSet::new();

        loading.insert(resolved.id.clone());

        let prefix = prefix_for_module(&resolved.id);
        let program = program_from_resolved(&resolved, &mut *gas)?;

        let rewritten = self.rewrite_program_with_imports(
            &program,
            Some(resolved.id.clone()),
            &prefix,
            gas,
            &mut loaded,
            &mut loading,
        )?;
        self.inject_decls(&rewritten.decls)?;

        let (preds, ty) = self.infer_type(rewritten.expr.as_ref(), gas)?;

        let exports = exports_from_program(&program, &prefix, &resolved.id);
        loaded.insert(resolved.id.clone(), exports);
        loading.remove(&resolved.id);

        Ok((preds, ty))
    }

    pub fn infer_snippet(
        &mut self,
        source: &str,
        gas: &mut GasMeter,
    ) -> Result<(Vec<Predicate>, Type), CompileError> {
        self.infer_snippet_with_gas_and_importer(source, gas, None)
            .map_err(CompileError::from)
    }

    pub fn infer_snippet_at(
        &mut self,
        source: &str,
        importer_path: impl AsRef<Path>,
        gas: &mut GasMeter,
    ) -> Result<(Vec<Predicate>, Type), CompileError> {
        let path = importer_path.as_ref().to_path_buf();
        self.infer_snippet_with_gas_and_importer(source, gas, Some(path))
            .map_err(CompileError::from)
    }

    fn infer_snippet_with_gas_and_importer(
        &mut self,
        source: &str,
        gas: &mut GasMeter,
        importer_path: Option<PathBuf>,
    ) -> Result<(Vec<Predicate>, Type), EngineError> {
        let program = parse_program_from_source(source, None, Some(&mut *gas))?;

        let importer = importer_path.map(|p| ModuleId::Local { path: p });

        let mut loaded: BTreeMap<ModuleId, ModuleExports> = BTreeMap::new();
        let mut loading: BTreeSet<ModuleId> = BTreeSet::new();

        let prefix = format!("@snippet{}", Uuid::new_v4());
        let rewritten = self.rewrite_program_with_imports(
            &program,
            importer,
            &prefix,
            gas,
            &mut loaded,
            &mut loading,
        )?;
        self.inject_decls(&rewritten.decls)?;
        self.infer_type(rewritten.expr.as_ref(), gas)
    }
}

//! Module system: resolvers, loading, and import rewriting.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_recursion::async_recursion;
use rex_ast::expr::{
    Decl, DeclareFnDecl, Expr, FnDecl, ImportDecl, ImportPath, InstanceDecl, Pattern, Program,
    Symbol, TypeConstraint, TypeDecl, TypeExpr, Var,
};
use rex_lexer::Token;
use rex_parser::Parser as RexParser;
use rex_ts::{Predicate, Type};
use rex_util::{GasMeter, sha256_hex};
use uuid::Uuid;

use crate::{Engine, EngineError, Pointer};

mod resolvers;
mod system;
mod types;

#[cfg(feature = "github-imports")]
pub use resolvers::default_github_resolver;
pub use resolvers::{default_local_resolver, default_stdlib_resolver, include_resolver};
pub use system::ResolverFn;
pub use types::virtual_export_name;
pub use types::{
    ModuleExports, ModuleId, ModuleInstance, ReplState, ResolveRequest, ResolvedModule,
};

pub(crate) use system::ModuleSystem;

use system::wrap_resolver;
use types::{prefix_for_module, qualify};

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

fn collect_local_renames(
    program: &Program,
    prefix: &str,
) -> (
    HashMap<Symbol, Symbol>,
    HashMap<Symbol, Symbol>,
    HashMap<Symbol, Symbol>,
) {
    let mut values = HashMap::new();
    let mut types = HashMap::new();
    let mut classes = HashMap::new();

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
    type_renames: &HashMap<Symbol, Symbol>,
    class_renames: &HashMap<Symbol, Symbol>,
) -> TypeExpr {
    match ty {
        TypeExpr::Name(span, name) => {
            if let Some(new) = type_renames.get(name) {
                TypeExpr::Name(*span, new.clone())
            } else if let Some(new) = class_renames.get(name) {
                TypeExpr::Name(*span, new.clone())
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
    type_renames: &HashMap<Symbol, Symbol>,
    class_renames: &HashMap<Symbol, Symbol>,
) -> Vec<TypeConstraint> {
    cs.iter()
        .map(|c| TypeConstraint {
            class: class_renames
                .get(&c.class)
                .cloned()
                .unwrap_or_else(|| c.class.clone()),
            typ: rename_type_expr(&c.typ, type_renames, class_renames),
        })
        .collect()
}

fn rename_pattern(pat: &Pattern, value_renames: &HashMap<Symbol, Symbol>) -> Pattern {
    match pat {
        Pattern::Wildcard(span) => Pattern::Wildcard(*span),
        Pattern::Var(v) => Pattern::Var(v.clone()),
        Pattern::Named(span, name, args) => Pattern::Named(
            *span,
            value_renames
                .get(name)
                .cloned()
                .unwrap_or_else(|| name.clone()),
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
    bound: &mut HashSet<Symbol>,
    value_renames: &HashMap<Symbol, Symbol>,
    type_renames: &HashMap<Symbol, Symbol>,
    class_renames: &HashMap<Symbol, Symbol>,
) -> Expr {
    match expr {
        Expr::Bool(span, v) => Expr::Bool(*span, *v),
        Expr::Uint(span, v) => Expr::Uint(*span, *v),
        Expr::Int(span, v) => Expr::Int(*span, *v),
        Expr::Float(span, v) => Expr::Float(*span, *v),
        Expr::String(span, v) => Expr::String(*span, v.clone()),
        Expr::Uuid(span, v) => Expr::Uuid(*span, *v),
        Expr::DateTime(span, v) => Expr::DateTime(*span, *v),
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

fn qualify_program(program: &Program, prefix: &str) -> Program {
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
                let mut bound = HashSet::new();
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
                    let mut bound = HashSet::new();
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

    let mut bound = HashSet::new();
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
    bound: &HashSet<Symbol>,
    shadowed_values: Option<&HashSet<Symbol>>,
) -> bool {
    if bound.contains(name) {
        return false;
    }
    match shadowed_values {
        None => true,
        Some(s) => !s.contains(name),
    }
}

fn rewrite_import_uses_expr(
    expr: &Expr,
    bound: &mut HashSet<Symbol>,
    aliases: &HashMap<Symbol, ModuleExports>,
    shadowed_values: Option<&HashSet<Symbol>>,
) -> Expr {
    match expr {
        Expr::Bool(span, v) => Expr::Bool(*span, *v),
        Expr::Uint(span, v) => Expr::Uint(*span, *v),
        Expr::Int(span, v) => Expr::Int(*span, *v),
        Expr::Float(span, v) => Expr::Float(*span, *v),
        Expr::String(span, v) => Expr::String(*span, v.clone()),
        Expr::Uuid(span, v) => Expr::Uuid(*span, *v),
        Expr::DateTime(span, v) => Expr::DateTime(*span, *v),
        Expr::Project(span, base, field) => {
            if let Expr::Var(v) = base.as_ref()
                && alias_is_visible(&v.name, bound, shadowed_values)
                && let Some(exports) = aliases.get(&v.name)
                && let Some(internal) = exports.values.get(field)
            {
                return Expr::Var(Var {
                    span: *span,
                    name: internal.clone(),
                });
            }
            Expr::Project(
                *span,
                Arc::new(rewrite_import_uses_expr(
                    base,
                    bound,
                    aliases,
                    shadowed_values,
                )),
                field.clone(),
            )
        }
        Expr::Var(v) => Expr::Var(v.clone()),
        Expr::Lam(span, scope, param, ann, constraints, body) => {
            bound.insert(param.name.clone());
            let out = Expr::Lam(
                *span,
                scope.clone(),
                param.clone(),
                ann.clone(),
                constraints.clone(),
                Arc::new(rewrite_import_uses_expr(
                    body,
                    bound,
                    aliases,
                    shadowed_values,
                )),
            );
            bound.remove(&param.name);
            out
        }
        Expr::Let(span, var, ann, val, body) => {
            let val = Arc::new(rewrite_import_uses_expr(
                val,
                bound,
                aliases,
                shadowed_values,
            ));
            bound.insert(var.name.clone());
            let body = Arc::new(rewrite_import_uses_expr(
                body,
                bound,
                aliases,
                shadowed_values,
            ));
            bound.remove(&var.name);
            Expr::Let(*span, var.clone(), ann.clone(), val, body)
        }
        Expr::LetRec(span, bindings, body) => {
            let names: Vec<Symbol> = bindings
                .iter()
                .map(|(var, _, _)| var.name.clone())
                .collect();
            for name in &names {
                bound.insert(name.clone());
            }
            let bindings = bindings
                .iter()
                .map(|(var, ann, def)| {
                    (
                        var.clone(),
                        ann.clone(),
                        Arc::new(rewrite_import_uses_expr(
                            def,
                            bound,
                            aliases,
                            shadowed_values,
                        )),
                    )
                })
                .collect();
            let body = Arc::new(rewrite_import_uses_expr(
                body,
                bound,
                aliases,
                shadowed_values,
            ));
            for name in &names {
                bound.remove(name);
            }
            Expr::LetRec(*span, bindings, body)
        }
        Expr::Match(span, scrutinee, arms) => {
            let scrutinee = Arc::new(rewrite_import_uses_expr(
                scrutinee,
                bound,
                aliases,
                shadowed_values,
            ));
            let mut renamed_arms = Vec::new();
            for (pat, arm_expr) in arms {
                let mut binds = Vec::new();
                collect_pattern_bindings(pat, &mut binds);
                for b in &binds {
                    bound.insert(b.clone());
                }
                let arm_expr = Arc::new(rewrite_import_uses_expr(
                    arm_expr,
                    bound,
                    aliases,
                    shadowed_values,
                ));
                for b in &binds {
                    bound.remove(b);
                }
                renamed_arms.push((pat.clone(), arm_expr));
            }
            Expr::Match(*span, scrutinee, renamed_arms)
        }
        Expr::Tuple(span, elems) => Expr::Tuple(
            *span,
            elems
                .iter()
                .map(|e| Arc::new(rewrite_import_uses_expr(e, bound, aliases, shadowed_values)))
                .collect(),
        ),
        Expr::List(span, elems) => Expr::List(
            *span,
            elems
                .iter()
                .map(|e| Arc::new(rewrite_import_uses_expr(e, bound, aliases, shadowed_values)))
                .collect(),
        ),
        Expr::Dict(span, kvs) => Expr::Dict(
            *span,
            kvs.iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        Arc::new(rewrite_import_uses_expr(v, bound, aliases, shadowed_values)),
                    )
                })
                .collect(),
        ),
        Expr::RecordUpdate(span, base, updates) => Expr::RecordUpdate(
            *span,
            Arc::new(rewrite_import_uses_expr(
                base,
                bound,
                aliases,
                shadowed_values,
            )),
            updates
                .iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        Arc::new(rewrite_import_uses_expr(v, bound, aliases, shadowed_values)),
                    )
                })
                .collect(),
        ),
        Expr::App(span, f, x) => Expr::App(
            *span,
            Arc::new(rewrite_import_uses_expr(f, bound, aliases, shadowed_values)),
            Arc::new(rewrite_import_uses_expr(x, bound, aliases, shadowed_values)),
        ),
        Expr::Ite(span, c, t, e) => Expr::Ite(
            *span,
            Arc::new(rewrite_import_uses_expr(c, bound, aliases, shadowed_values)),
            Arc::new(rewrite_import_uses_expr(t, bound, aliases, shadowed_values)),
            Arc::new(rewrite_import_uses_expr(e, bound, aliases, shadowed_values)),
        ),
        Expr::Ann(span, e, t) => Expr::Ann(
            *span,
            Arc::new(rewrite_import_uses_expr(e, bound, aliases, shadowed_values)),
            t.clone(),
        ),
    }
}

fn rewrite_import_uses(
    program: &Program,
    aliases: &HashMap<Symbol, ModuleExports>,
    shadowed_values: Option<&HashSet<Symbol>>,
) -> Program {
    let decls = program
        .decls
        .iter()
        .map(|decl| match decl {
            Decl::Fn(fd) => {
                let mut bound: HashSet<Symbol> =
                    fd.params.iter().map(|(v, _)| v.name.clone()).collect();
                let body = Arc::new(rewrite_import_uses_expr(
                    fd.body.as_ref(),
                    &mut bound,
                    aliases,
                    shadowed_values,
                ));
                Decl::Fn(FnDecl {
                    span: fd.span,
                    is_pub: fd.is_pub,
                    name: fd.name.clone(),
                    params: fd.params.clone(),
                    ret: fd.ret.clone(),
                    constraints: fd.constraints.clone(),
                    body,
                })
            }
            Decl::Instance(inst) => {
                let methods = inst
                    .methods
                    .iter()
                    .map(|m| {
                        let mut bound = HashSet::new();
                        let body = Arc::new(rewrite_import_uses_expr(
                            m.body.as_ref(),
                            &mut bound,
                            aliases,
                            shadowed_values,
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
                    class: inst.class.clone(),
                    head: inst.head.clone(),
                    context: inst.context.clone(),
                    methods,
                })
            }
            other => other.clone(),
        })
        .collect();

    let mut bound = HashSet::new();
    let expr = Arc::new(rewrite_import_uses_expr(
        program.expr.as_ref(),
        &mut bound,
        aliases,
        shadowed_values,
    ));
    Program { decls, expr }
}

fn validate_import_uses_expr(
    expr: &Expr,
    bound: &mut HashSet<Symbol>,
    aliases: &HashMap<Symbol, ModuleExports>,
    shadowed_values: Option<&HashSet<Symbol>>,
) -> Result<(), EngineError> {
    match expr {
        Expr::Project(_, base, field) => {
            if let Expr::Var(v) = base.as_ref()
                && alias_is_visible(&v.name, bound, shadowed_values)
                && let Some(exports) = aliases.get(&v.name)
                && !exports.values.contains_key(field)
            {
                return Err(crate::ModuleError::MissingExport {
                    module: v.name.clone(),
                    export: field.clone(),
                }
                .into());
            }
            validate_import_uses_expr(base, bound, aliases, shadowed_values)
        }
        Expr::Lam(_, _, param, _, _, body) => {
            bound.insert(param.name.clone());
            let res = validate_import_uses_expr(body, bound, aliases, shadowed_values);
            bound.remove(&param.name);
            res
        }
        Expr::Let(_, var, _, val, body) => {
            validate_import_uses_expr(val, bound, aliases, shadowed_values)?;
            bound.insert(var.name.clone());
            let res = validate_import_uses_expr(body, bound, aliases, shadowed_values);
            bound.remove(&var.name);
            res
        }
        Expr::LetRec(_, bindings, body) => {
            let names: Vec<Symbol> = bindings
                .iter()
                .map(|(var, _, _)| var.name.clone())
                .collect();
            for name in &names {
                bound.insert(name.clone());
            }
            for (_, _, def) in bindings {
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
        Expr::Ann(_, e, _) => validate_import_uses_expr(e, bound, aliases, shadowed_values),
        Expr::Var(..)
        | Expr::Bool(..)
        | Expr::Uint(..)
        | Expr::Int(..)
        | Expr::Float(..)
        | Expr::String(..)
        | Expr::Uuid(..)
        | Expr::DateTime(..) => Ok(()),
    }
}

fn validate_import_uses(
    program: &Program,
    aliases: &HashMap<Symbol, ModuleExports>,
    shadowed_values: Option<&HashSet<Symbol>>,
) -> Result<(), EngineError> {
    for decl in &program.decls {
        match decl {
            Decl::Fn(fd) => {
                let mut bound: HashSet<Symbol> =
                    fd.params.iter().map(|(v, _)| v.name.clone()).collect();
                validate_import_uses_expr(fd.body.as_ref(), &mut bound, aliases, shadowed_values)?;
            }
            Decl::Instance(inst) => {
                for m in &inst.methods {
                    let mut bound = HashSet::new();
                    validate_import_uses_expr(
                        m.body.as_ref(),
                        &mut bound,
                        aliases,
                        shadowed_values,
                    )?;
                }
            }
            _ => {}
        }
    }
    let mut bound = HashSet::new();
    validate_import_uses_expr(program.expr.as_ref(), &mut bound, aliases, shadowed_values)
}

fn decl_value_names(decls: &[Decl]) -> HashSet<Symbol> {
    let mut out = HashSet::new();
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

fn exports_from_program(program: &Program, prefix: &str) -> ModuleExports {
    let (value_renames, type_renames, class_renames) = collect_local_renames(program, prefix);

    let mut values = HashMap::new();
    let mut types = HashMap::new();
    let mut classes = HashMap::new();

    for decl in &program.decls {
        match decl {
            Decl::Fn(fd) if fd.is_pub => {
                if let Some(internal) = value_renames.get(&fd.name.name) {
                    values.insert(fd.name.name.clone(), internal.clone());
                }
            }
            Decl::DeclareFn(df) if df.is_pub => {
                if let Some(internal) = value_renames.get(&df.name.name) {
                    values.insert(df.name.name.clone(), internal.clone());
                }
            }
            Decl::Type(td) if td.is_pub => {
                if let Some(internal) = type_renames.get(&td.name) {
                    types.insert(td.name.clone(), internal.clone());
                }
                for variant in &td.variants {
                    if let Some(internal) = value_renames.get(&variant.name) {
                        values.insert(variant.name.clone(), internal.clone());
                    }
                }
            }
            Decl::Class(cd) if cd.is_pub => {
                if let Some(internal) = class_renames.get(&cd.name) {
                    classes.insert(cd.name.clone(), internal.clone());
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

    ModuleExports {
        values,
        types,
        classes,
    }
}

fn parse_program_from_source(
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
        Some(gas) => parser.parse_program_with_gas(gas),
        None => parser.parse_program(),
    }
    .map_err(|errs| match context {
        Some(id) => EngineError::from(crate::ModuleError::ParseInModule {
            module: id.clone(),
            errors: errs,
        }),
        None => EngineError::from(crate::ModuleError::Parse { errors: errs }),
    })?;
    Ok(program)
}

impl Engine {
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
        self.modules.add_resolver("local", default_local_resolver());
        #[cfg(feature = "github-imports")]
        {
            self.modules
                .add_resolver("remote", default_github_resolver());
        }
    }

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

    #[async_recursion]
    async fn alias_exports_for_decls_with_gas(
        &mut self,
        decls: &[Decl],
        importer: Option<ModuleId>,
        gas: &mut GasMeter,
    ) -> Result<HashMap<Symbol, ModuleExports>, EngineError> {
        let mut alias_exports = HashMap::new();
        for decl in decls {
            let Decl::Import(ImportDecl { path, alias, .. }) = decl else {
                continue;
            };
            let spec = import_specifier(path);
            let imported = self.modules.resolve(ResolveRequest {
                module_name: spec,
                importer: importer.clone(),
            })?;
            let inst = self
                .load_module_from_resolved_with_gas(imported, gas)
                .await?;
            alias_exports.insert(alias.clone(), inst.exports.clone());
        }
        Ok(alias_exports)
    }

    #[async_recursion]
    async fn load_module_from_resolved_with_gas(
        &mut self,
        resolved: ResolvedModule,
        gas: &mut GasMeter,
    ) -> Result<ModuleInstance, EngineError> {
        if let Some(inst) = self.modules.cached(&resolved.id)? {
            return Ok(inst);
        }

        self.modules.mark_loading(&resolved.id)?;

        let prefix = prefix_for_module(&resolved.id);
        let program = parse_program_from_source(&resolved.source, Some(&resolved.id), Some(gas))?;

        // Resolve imports first so qualified names exist in the environment.
        let alias_exports = self
            .alias_exports_for_decls_with_gas(&program.decls, Some(resolved.id.clone()), gas)
            .await?;

        // Qualify local names, then rewrite `alias.foo` uses into internal symbols.
        let qualified = qualify_program(&program, &prefix);
        validate_import_uses(&qualified, &alias_exports, None)?;
        let rewritten = rewrite_import_uses(&qualified, &alias_exports, None);

        self.inject_decls(&rewritten.decls)?;
        let init_value = self.eval_with_gas(rewritten.expr.as_ref(), gas).await?;

        let exports = exports_from_program(&program, &prefix);
        let inst = ModuleInstance {
            id: resolved.id.clone(),
            exports,
            init_value,
        };
        self.modules.store_loaded(inst.clone())?;
        Ok(inst)
    }

    fn load_module_types_from_resolved_with_gas(
        &mut self,
        resolved: ResolvedModule,
        gas: &mut GasMeter,
        loaded: &mut HashMap<ModuleId, ModuleExports>,
        loading: &mut HashSet<ModuleId>,
    ) -> Result<ModuleExports, EngineError> {
        if let Some(exports) = loaded.get(&resolved.id) {
            return Ok(exports.clone());
        }

        if loading.contains(&resolved.id) {
            return Err(crate::ModuleError::CyclicImport {
                id: resolved.id.clone(),
            }
            .into());
        }
        loading.insert(resolved.id.clone());

        let prefix = prefix_for_module(&resolved.id);
        let program =
            parse_program_from_source(&resolved.source, Some(&resolved.id), Some(&mut *gas))?;

        let rewritten = self.rewrite_program_with_imports(
            &program,
            Some(resolved.id.clone()),
            &prefix,
            gas,
            loaded,
            loading,
        )?;
        self.inject_decls(&rewritten.decls)?;

        let exports = exports_from_program(&program, &prefix);
        loaded.insert(resolved.id.clone(), exports.clone());
        loading.remove(&resolved.id);
        Ok(exports)
    }

    fn rewrite_program_with_imports(
        &mut self,
        program: &Program,
        importer: Option<ModuleId>,
        prefix: &str,
        gas: &mut GasMeter,
        loaded: &mut HashMap<ModuleId, ModuleExports>,
        loading: &mut HashSet<ModuleId>,
    ) -> Result<Program, EngineError> {
        let mut alias_exports: HashMap<Symbol, ModuleExports> = HashMap::new();
        for decl in &program.decls {
            let Decl::Import(ImportDecl { path, alias, .. }) = decl else {
                continue;
            };
            let spec = import_specifier(path);
            let imported = self.modules.resolve(ResolveRequest {
                module_name: spec,
                importer: importer.clone(),
            })?;
            let exports =
                self.load_module_types_from_resolved_with_gas(imported, gas, loaded, loading)?;
            alias_exports.insert(alias.clone(), exports);
        }

        let qualified = qualify_program(program, prefix);
        validate_import_uses(&qualified, &alias_exports, None)?;
        Ok(rewrite_import_uses(&qualified, &alias_exports, None))
    }

    fn read_local_module_bytes(&self, path: &Path) -> Result<(ModuleId, Vec<u8>), EngineError> {
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
        let hash = sha256_hex(&bytes);
        Ok((ModuleId::Local { path: canon, hash }, bytes))
    }

    fn decode_local_module_source(
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

    pub fn infer_module_file_with_gas(
        &mut self,
        path: impl AsRef<Path>,
        gas: &mut GasMeter,
    ) -> Result<(Vec<Predicate>, Type), EngineError> {
        let (id, bytes) = self.read_local_module_bytes(path.as_ref())?;
        let source = self.decode_local_module_source(&id, bytes)?;
        self.infer_module_source_with_gas(ResolvedModule { id, source }, gas)
    }

    fn infer_module_source_with_gas(
        &mut self,
        resolved: ResolvedModule,
        gas: &mut GasMeter,
    ) -> Result<(Vec<Predicate>, Type), EngineError> {
        let mut loaded: HashMap<ModuleId, ModuleExports> = HashMap::new();
        let mut loading: HashSet<ModuleId> = HashSet::new();

        loading.insert(resolved.id.clone());

        let prefix = prefix_for_module(&resolved.id);
        let program =
            parse_program_from_source(&resolved.source, Some(&resolved.id), Some(&mut *gas))?;

        let rewritten = self.rewrite_program_with_imports(
            &program,
            Some(resolved.id.clone()),
            &prefix,
            gas,
            &mut loaded,
            &mut loading,
        )?;
        self.inject_decls(&rewritten.decls)?;

        let (preds, ty) = self.infer_type_with_gas(rewritten.expr.as_ref(), gas)?;

        let exports = exports_from_program(&program, &prefix);
        loaded.insert(resolved.id.clone(), exports);
        loading.remove(&resolved.id);

        Ok((preds, ty))
    }

    pub fn infer_snippet_with_gas(
        &mut self,
        source: &str,
        gas: &mut GasMeter,
    ) -> Result<(Vec<Predicate>, Type), EngineError> {
        self.infer_snippet_with_gas_and_importer(source, gas, None)
    }

    pub fn infer_snippet_at_with_gas(
        &mut self,
        source: &str,
        importer_path: impl AsRef<Path>,
        gas: &mut GasMeter,
    ) -> Result<(Vec<Predicate>, Type), EngineError> {
        let path = importer_path.as_ref().to_path_buf();
        self.infer_snippet_with_gas_and_importer(source, gas, Some(path))
    }

    fn infer_snippet_with_gas_and_importer(
        &mut self,
        source: &str,
        gas: &mut GasMeter,
        importer_path: Option<PathBuf>,
    ) -> Result<(Vec<Predicate>, Type), EngineError> {
        let program = parse_program_from_source(source, None, Some(&mut *gas))?;

        let importer = importer_path.map(|p| ModuleId::Local {
            path: p,
            hash: "snippet".into(),
        });

        let mut loaded: HashMap<ModuleId, ModuleExports> = HashMap::new();
        let mut loading: HashSet<ModuleId> = HashSet::new();

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
        self.infer_type_with_gas(rewritten.expr.as_ref(), gas)
    }

    pub async fn eval_module_file_with_gas(
        &mut self,
        path: impl AsRef<Path>,
        gas: &mut GasMeter,
    ) -> Result<Pointer, EngineError> {
        let (id, bytes) = self.read_local_module_bytes(path.as_ref())?;
        if let Some(inst) = self.modules.cached(&id)? {
            return Ok(inst.init_value);
        }
        let source = self.decode_local_module_source(&id, bytes)?;
        let inst = self
            .load_module_from_resolved_with_gas(ResolvedModule { id, source }, gas)
            .await?;
        Ok(inst.init_value)
    }

    pub async fn eval_module_source_with_gas(
        &mut self,
        source: &str,
        gas: &mut GasMeter,
    ) -> Result<Pointer, EngineError> {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        source.hash(&mut hasher);
        let id = ModuleId::Virtual(format!("<inline:{:016x}>", hasher.finish()));
        if let Some(inst) = self.modules.cached(&id)? {
            return Ok(inst.init_value);
        }
        let inst = self
            .load_module_from_resolved_with_gas(
                ResolvedModule {
                    id,
                    source: source.to_string(),
                },
                gas,
            )
            .await?;
        Ok(inst.init_value)
    }

    pub async fn eval_snippet_with_gas(
        &mut self,
        source: &str,
        gas: &mut GasMeter,
    ) -> Result<Pointer, EngineError> {
        self.eval_snippet_with_gas_and_importer(source, gas, None)
            .await
    }

    pub async fn eval_repl_program_with_gas(
        &mut self,
        program: &Program,
        state: &mut ReplState,
        gas: &mut GasMeter,
    ) -> Result<Pointer, EngineError> {
        let importer = state.importer_path.as_ref().map(|p| ModuleId::Local {
            path: p.clone(),
            hash: "repl".into(),
        });

        let alias_exports = self
            .alias_exports_for_decls_with_gas(&program.decls, importer.clone(), gas)
            .await?;
        state.alias_exports.extend(alias_exports);

        let mut shadowed_values = state.defined_values.clone();
        shadowed_values.extend(decl_value_names(&program.decls));

        validate_import_uses(program, &state.alias_exports, Some(&shadowed_values))?;
        let rewritten = rewrite_import_uses(program, &state.alias_exports, Some(&shadowed_values));

        self.inject_decls(&rewritten.decls)?;
        state
            .defined_values
            .extend(decl_value_names(&program.decls));
        self.eval_with_gas(rewritten.expr.as_ref(), gas).await
    }

    /// Evaluate a non-module snippet (with gas), but use `importer_path` for resolving local-relative imports.
    pub async fn eval_snippet_at_with_gas(
        &mut self,
        source: &str,
        importer_path: impl AsRef<Path>,
        gas: &mut GasMeter,
    ) -> Result<Pointer, EngineError> {
        let path = importer_path.as_ref().to_path_buf();
        self.eval_snippet_with_gas_and_importer(source, gas, Some(path))
            .await
    }

    async fn eval_snippet_with_gas_and_importer(
        &mut self,
        source: &str,
        gas: &mut GasMeter,
        importer_path: Option<PathBuf>,
    ) -> Result<Pointer, EngineError> {
        let program = parse_program_from_source(source, None, Some(&mut *gas))?;

        let importer = importer_path.map(|p| ModuleId::Local {
            path: p,
            hash: "snippet".into(),
        });

        let alias_exports = self
            .alias_exports_for_decls_with_gas(&program.decls, importer.clone(), gas)
            .await?;

        let prefix = format!("@snippet{}", Uuid::new_v4());
        let qualified = qualify_program(&program, &prefix);
        validate_import_uses(&qualified, &alias_exports, None)?;
        let rewritten = rewrite_import_uses(&qualified, &alias_exports, None);

        self.inject_decls(&rewritten.decls)?;
        self.eval_with_gas(rewritten.expr.as_ref(), gas).await
    }
}

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use rex_ast::{
    ClassDecl, ClassMethodSig, CompilationUnit, Decl, DeclareFnDecl, Expr, FnDecl, InstanceDecl,
    InstanceMethodImpl, NameRef, Pattern, Symbol, TypeConstraint, TypeDecl, TypeExpr, TypeVariant,
    Var,
};

use crate::modules::{collect_pattern_bindings, types::qualify};

pub(crate) fn qualify_program(compilation_unit: &CompilationUnit, prefix: &str) -> CompilationUnit {
    let (value_renames, type_renames, class_renames) =
        collect_local_renames(compilation_unit, prefix);

    let decls = compilation_unit
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
                    .map(|v| TypeVariant {
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
                    .map(|m| ClassMethodSig {
                        name: m.name.clone(),
                        typ: rename_type_expr(&m.typ, &type_renames, &class_renames),
                    })
                    .collect();
                Some(Decl::Class(ClassDecl {
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
                    methods.push(InstanceMethodImpl {
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

    let body = compilation_unit.body.as_ref().map(|body| {
        let mut bound = BTreeSet::new();
        Arc::new(rename_expr(
            body.as_ref(),
            &mut bound,
            &value_renames,
            &type_renames,
            &class_renames,
        ))
    });

    CompilationUnit { decls, body }
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

pub(crate) fn collect_local_renames(
    compilation_unit: &CompilationUnit,
    prefix: &str,
) -> (
    BTreeMap<Symbol, Symbol>,
    BTreeMap<Symbol, Symbol>,
    BTreeMap<Symbol, Symbol>,
) {
    let mut values = BTreeMap::new();
    let mut types = BTreeMap::new();
    let mut classes = BTreeMap::new();

    for decl in &compilation_unit.decls {
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

use crate::prelude::*;
use crate::{completion::*, diagnostics::*, shared::*};

pub(crate) fn inject_program_decls(
    ts: &mut TypeSystem,
    compilation_unit: &CompilationUnit,
    want_prepared_instance: Option<usize>,
) -> std::result::Result<InjectedDecls, TsTypeError> {
    let mut instances = Vec::new();
    let mut prepared_target = None;
    let mut pending_non_instances: Vec<Decl> = Vec::new();

    let flush_non_instances =
        |ts: &mut TypeSystem, pending: &mut Vec<Decl>| -> std::result::Result<(), TsTypeError> {
            if pending.is_empty() {
                return Ok(());
            }
            ts.register_decls(pending)?;
            pending.clear();
            Ok(())
        };

    for (idx, decl) in compilation_unit.decls.iter().enumerate() {
        match decl {
            Decl::Instance(inst_decl) => {
                flush_non_instances(ts, &mut pending_non_instances)?;
                let prepared = ts.register_instance_decl(inst_decl)?;
                if want_prepared_instance.is_some_and(|want| want == idx) {
                    prepared_target = Some(prepared.clone());
                }
                instances.push((idx, prepared));
            }
            _ => pending_non_instances.push(decl.clone()),
        }
    }
    flush_non_instances(ts, &mut pending_non_instances)?;

    Ok((instances, prepared_target))
}

pub(crate) type PreparedInstance = (usize, PreparedInstanceDecl);
pub(crate) type InjectedDecls = (Vec<PreparedInstance>, Option<PreparedInstanceDecl>);

pub(crate) fn rewrite_type_expr(ty: &TypeExpr, type_map: &HashMap<Symbol, Symbol>) -> TypeExpr {
    match ty {
        TypeExpr::Name(span, name) => {
            if let Some(new) = type_map.get(&name.to_dotted_symbol()) {
                TypeExpr::Name(*span, NameRef::Unqualified(new.clone()))
            } else {
                TypeExpr::Name(*span, name.clone())
            }
        }
        TypeExpr::App(span, f, x) => TypeExpr::App(
            *span,
            Box::new(rewrite_type_expr(f, type_map)),
            Box::new(rewrite_type_expr(x, type_map)),
        ),
        TypeExpr::Fun(span, a, b) => TypeExpr::Fun(
            *span,
            Box::new(rewrite_type_expr(a, type_map)),
            Box::new(rewrite_type_expr(b, type_map)),
        ),
        TypeExpr::Tuple(span, elems) => TypeExpr::Tuple(
            *span,
            elems
                .iter()
                .map(|e| rewrite_type_expr(e, type_map))
                .collect(),
        ),
        TypeExpr::Record(span, fields) => TypeExpr::Record(
            *span,
            fields
                .iter()
                .map(|(name, ty)| (name.clone(), rewrite_type_expr(ty, type_map)))
                .collect(),
        ),
    }
}

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

pub(crate) fn rewrite_import_projections_expr(
    expr: &Expr,
    bound: &mut BTreeSet<Symbol>,
    imports: &HashMap<Symbol, ImportModuleInfo>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Expr {
    match expr {
        Expr::Project(span, base, field) => {
            if let Expr::Var(v) = base.as_ref()
                && !bound.contains(&v.name)
                && let Some(info) = imports.get(&v.name)
            {
                if let Some(internal) = info.value_map.get(field) {
                    return Expr::Var(Var {
                        span: *span,
                        name: internal.clone(),
                    });
                }
                diagnostics.push(diagnostic_for_span(
                    *span,
                    format!("module `{}` does not export `{}`", v.name, field),
                ));
            }
            Expr::Project(
                *span,
                std::sync::Arc::new(rewrite_import_projections_expr(
                    base,
                    bound,
                    imports,
                    diagnostics,
                )),
                field.clone(),
            )
        }
        Expr::Var(v) => Expr::Var(v.clone()),
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
                    std::sync::Arc::new(rewrite_import_projections_expr(
                        e,
                        bound,
                        imports,
                        diagnostics,
                    ))
                })
                .collect(),
        ),
        Expr::List(span, elems) => Expr::List(
            *span,
            elems
                .iter()
                .map(|e| {
                    std::sync::Arc::new(rewrite_import_projections_expr(
                        e,
                        bound,
                        imports,
                        diagnostics,
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
                        std::sync::Arc::new(rewrite_import_projections_expr(
                            v,
                            bound,
                            imports,
                            diagnostics,
                        )),
                    )
                })
                .collect(),
        ),
        Expr::RecordUpdate(span, base, updates) => Expr::RecordUpdate(
            *span,
            std::sync::Arc::new(rewrite_import_projections_expr(
                base,
                bound,
                imports,
                diagnostics,
            )),
            updates
                .iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        std::sync::Arc::new(rewrite_import_projections_expr(
                            v,
                            bound,
                            imports,
                            diagnostics,
                        )),
                    )
                })
                .collect(),
        ),
        Expr::App(span, f, x) => Expr::App(
            *span,
            std::sync::Arc::new(rewrite_import_projections_expr(
                f,
                bound,
                imports,
                diagnostics,
            )),
            std::sync::Arc::new(rewrite_import_projections_expr(
                x,
                bound,
                imports,
                diagnostics,
            )),
        ),
        Expr::Lam(span, scope, param, ann, constraints, body) => {
            let ann = ann
                .as_ref()
                .map(|t| rewrite_import_projections_type_expr(t, bound, imports));
            let constraints = constraints
                .iter()
                .map(|c| TypeConstraint {
                    class: rewrite_import_projections_class_name(&c.class, bound, imports),
                    typ: rewrite_import_projections_type_expr(&c.typ, bound, imports),
                })
                .collect();
            bound.insert(param.name.clone());
            let out = Expr::Lam(
                *span,
                scope.clone(),
                param.clone(),
                ann,
                constraints,
                std::sync::Arc::new(rewrite_import_projections_expr(
                    body,
                    bound,
                    imports,
                    diagnostics,
                )),
            );
            bound.remove(&param.name);
            out
        }
        Expr::Let(span, var, type_params, ann, val, body) => {
            let val = std::sync::Arc::new(rewrite_import_projections_expr(
                val,
                bound,
                imports,
                diagnostics,
            ));
            bound.insert(var.name.clone());
            let body = std::sync::Arc::new(rewrite_import_projections_expr(
                body,
                bound,
                imports,
                diagnostics,
            ));
            bound.remove(&var.name);
            Expr::Let(
                *span,
                var.clone(),
                type_params.clone(),
                ann.as_ref()
                    .map(|t| rewrite_import_projections_type_expr(t, bound, imports)),
                val,
                body,
            )
        }
        Expr::LetRec(span, bindings, body) => {
            let anns: Vec<Option<TypeExpr>> = bindings
                .iter()
                .map(|(_, _, ann, _)| {
                    ann.as_ref()
                        .map(|t| rewrite_import_projections_type_expr(t, bound, imports))
                })
                .collect();
            let names: Vec<Symbol> = bindings
                .iter()
                .map(|(var, _, _, _)| var.name.clone())
                .collect();
            for name in &names {
                bound.insert(name.clone());
            }
            let bindings = bindings
                .iter()
                .zip(anns)
                .map(|((var, type_params, _ann, def), ann)| {
                    (
                        var.clone(),
                        type_params.clone(),
                        ann,
                        std::sync::Arc::new(rewrite_import_projections_expr(
                            def,
                            bound,
                            imports,
                            diagnostics,
                        )),
                    )
                })
                .collect();
            let body = std::sync::Arc::new(rewrite_import_projections_expr(
                body,
                bound,
                imports,
                diagnostics,
            ));
            for name in &names {
                bound.remove(name);
            }
            Expr::LetRec(*span, bindings, body)
        }
        Expr::Ite(span, c, t, e) => Expr::Ite(
            *span,
            std::sync::Arc::new(rewrite_import_projections_expr(
                c,
                bound,
                imports,
                diagnostics,
            )),
            std::sync::Arc::new(rewrite_import_projections_expr(
                t,
                bound,
                imports,
                diagnostics,
            )),
            std::sync::Arc::new(rewrite_import_projections_expr(
                e,
                bound,
                imports,
                diagnostics,
            )),
        ),
        Expr::Match(span, scrutinee, arms) => {
            let scrutinee = std::sync::Arc::new(rewrite_import_projections_expr(
                scrutinee,
                bound,
                imports,
                diagnostics,
            ));
            let mut out_arms = Vec::new();
            for (pat, arm_expr) in arms {
                let mut binds = Vec::new();
                collect_pattern_bindings(pat, &mut binds);
                for b in &binds {
                    bound.insert(b.clone());
                }
                let arm_expr = std::sync::Arc::new(rewrite_import_projections_expr(
                    arm_expr,
                    bound,
                    imports,
                    diagnostics,
                ));
                for b in &binds {
                    bound.remove(b);
                }
                out_arms.push((pat.clone(), arm_expr));
            }
            Expr::Match(*span, scrutinee, out_arms)
        }
        Expr::Ann(span, e, t) => Expr::Ann(
            *span,
            std::sync::Arc::new(rewrite_import_projections_expr(
                e,
                bound,
                imports,
                diagnostics,
            )),
            rewrite_import_projections_type_expr(t, bound, imports),
        ),
    }
}

pub(crate) fn qualified_alias_member(name: &NameRef) -> Option<(&Symbol, &Symbol)> {
    match name {
        NameRef::Qualified(_, segments) if segments.len() == 2 => {
            Some((&segments[0], &segments[1]))
        }
        _ => None,
    }
}

pub(crate) fn rewrite_import_projections_class_name(
    class: &NameRef,
    bound: &BTreeSet<Symbol>,
    imports: &HashMap<Symbol, ImportModuleInfo>,
) -> NameRef {
    let Some((alias, member)) = qualified_alias_member(class) else {
        return class.clone();
    };
    if bound.contains(alias) {
        return class.clone();
    }
    let Some(info) = imports.get(alias) else {
        return class.clone();
    };
    info.class_map
        .get(member)
        .map(|s| NameRef::Unqualified(s.clone()))
        .unwrap_or_else(|| class.clone())
}

pub(crate) fn rewrite_import_projections_type_expr(
    ty: &TypeExpr,
    bound: &BTreeSet<Symbol>,
    imports: &HashMap<Symbol, ImportModuleInfo>,
) -> TypeExpr {
    match ty {
        TypeExpr::Name(span, name) => {
            let Some((alias, member)) = qualified_alias_member(name) else {
                return TypeExpr::Name(*span, name.clone());
            };
            if bound.contains(alias) {
                return TypeExpr::Name(*span, name.clone());
            }
            let Some(info) = imports.get(alias) else {
                return TypeExpr::Name(*span, name.clone());
            };
            if let Some(new) = info.type_map.get(member) {
                TypeExpr::Name(*span, NameRef::Unqualified(new.clone()))
            } else if let Some(new) = info.class_map.get(member) {
                TypeExpr::Name(*span, NameRef::Unqualified(new.clone()))
            } else {
                TypeExpr::Name(*span, name.clone())
            }
        }
        TypeExpr::App(span, f, x) => TypeExpr::App(
            *span,
            Box::new(rewrite_import_projections_type_expr(f, bound, imports)),
            Box::new(rewrite_import_projections_type_expr(x, bound, imports)),
        ),
        TypeExpr::Fun(span, a, b) => TypeExpr::Fun(
            *span,
            Box::new(rewrite_import_projections_type_expr(a, bound, imports)),
            Box::new(rewrite_import_projections_type_expr(b, bound, imports)),
        ),
        TypeExpr::Tuple(span, elems) => TypeExpr::Tuple(
            *span,
            elems
                .iter()
                .map(|e| rewrite_import_projections_type_expr(e, bound, imports))
                .collect(),
        ),
        TypeExpr::Record(span, fields) => TypeExpr::Record(
            *span,
            fields
                .iter()
                .map(|(name, t)| {
                    (
                        name.clone(),
                        rewrite_import_projections_type_expr(t, bound, imports),
                    )
                })
                .collect(),
        ),
    }
}

pub(crate) fn rewrite_program_import_projections(
    compilation_unit: &CompilationUnit,
    imports: &HashMap<Symbol, ImportModuleInfo>,
    diagnostics: &mut Vec<Diagnostic>,
) -> CompilationUnit {
    let decl_bound = BTreeSet::new();
    let decls = compilation_unit
        .decls
        .iter()
        .map(|decl| match decl {
            Decl::Fn(fd) => {
                let mut bound: BTreeSet<Symbol> =
                    fd.params.iter().map(|(v, _)| v.name.clone()).collect();
                let body = std::sync::Arc::new(rewrite_import_projections_expr(
                    fd.body.as_ref(),
                    &mut bound,
                    imports,
                    diagnostics,
                ));
                Decl::Fn(FnDecl {
                    span: fd.span,
                    is_pub: fd.is_pub,
                    name: fd.name.clone(),
                    type_params: fd.type_params.clone(),
                    params: fd
                        .params
                        .iter()
                        .map(|(v, t)| {
                            (
                                v.clone(),
                                rewrite_import_projections_type_expr(t, &decl_bound, imports),
                            )
                        })
                        .collect(),
                    ret: rewrite_import_projections_type_expr(&fd.ret, &decl_bound, imports),
                    constraints: fd
                        .constraints
                        .iter()
                        .map(|c| TypeConstraint {
                            class: rewrite_import_projections_class_name(
                                &c.class,
                                &decl_bound,
                                imports,
                            ),
                            typ: rewrite_import_projections_type_expr(&c.typ, &decl_bound, imports),
                        })
                        .collect(),
                    body,
                })
            }
            Decl::DeclareFn(df) => Decl::DeclareFn(DeclareFnDecl {
                span: df.span,
                is_pub: df.is_pub,
                name: df.name.clone(),
                type_params: df.type_params.clone(),
                params: df
                    .params
                    .iter()
                    .map(|(v, t)| {
                        (
                            v.clone(),
                            rewrite_import_projections_type_expr(t, &decl_bound, imports),
                        )
                    })
                    .collect(),
                ret: rewrite_import_projections_type_expr(&df.ret, &decl_bound, imports),
                constraints: df
                    .constraints
                    .iter()
                    .map(|c| TypeConstraint {
                        class: rewrite_import_projections_class_name(
                            &c.class,
                            &decl_bound,
                            imports,
                        ),
                        typ: rewrite_import_projections_type_expr(&c.typ, &decl_bound, imports),
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
                    .map(|v| TypeVariant {
                        name: v.name.clone(),
                        args: v
                            .args
                            .iter()
                            .map(|t| rewrite_import_projections_type_expr(t, &decl_bound, imports))
                            .collect(),
                    })
                    .collect(),
            }),
            Decl::Class(cd) => Decl::Class(ClassDecl {
                span: cd.span,
                is_pub: cd.is_pub,
                name: cd.name.clone(),
                params: cd.params.clone(),
                supers: cd
                    .supers
                    .iter()
                    .map(|c| TypeConstraint {
                        class: rewrite_import_projections_class_name(
                            &c.class,
                            &decl_bound,
                            imports,
                        ),
                        typ: rewrite_import_projections_type_expr(&c.typ, &decl_bound, imports),
                    })
                    .collect(),
                methods: cd
                    .methods
                    .iter()
                    .map(|m| ClassMethodSig {
                        name: m.name.clone(),
                        type_params: m.type_params.clone(),
                        typ: rewrite_import_projections_type_expr(&m.typ, &decl_bound, imports),
                    })
                    .collect(),
            }),
            Decl::Instance(inst) => {
                let methods = inst
                    .methods
                    .iter()
                    .map(|m| {
                        let mut bound = BTreeSet::new();
                        let body = std::sync::Arc::new(rewrite_import_projections_expr(
                            m.body.as_ref(),
                            &mut bound,
                            imports,
                            diagnostics,
                        ));
                        InstanceMethodImpl {
                            name: m.name.clone(),
                            type_params: m.type_params.clone(),
                            ann: m.ann.as_ref().map(|t| {
                                rewrite_import_projections_type_expr(t, &decl_bound, imports)
                            }),
                            body,
                        }
                    })
                    .collect();
                Decl::Instance(InstanceDecl {
                    span: inst.span,
                    is_pub: inst.is_pub,
                    type_params: inst.type_params.clone(),
                    class: rewrite_import_projections_class_name(
                        &NameRef::from_dotted(inst.class.as_ref()),
                        &decl_bound,
                        imports,
                    )
                    .to_dotted_symbol(),
                    head: rewrite_import_projections_type_expr(&inst.head, &decl_bound, imports),
                    context: inst
                        .context
                        .iter()
                        .map(|c| TypeConstraint {
                            class: rewrite_import_projections_class_name(
                                &c.class,
                                &decl_bound,
                                imports,
                            ),
                            typ: rewrite_import_projections_type_expr(&c.typ, &decl_bound, imports),
                        })
                        .collect(),
                    methods,
                })
            }
            other => other.clone(),
        })
        .collect();

    let body = compilation_unit.body.as_ref().map(|body| {
        let mut bound = BTreeSet::new();
        std::sync::Arc::new(rewrite_import_projections_expr(
            body.as_ref(),
            &mut bound,
            imports,
            diagnostics,
        ))
    });

    CompilationUnit { decls, body }
}

pub(crate) fn validate_import_projection_class_name(
    class: &NameRef,
    span: Span,
    bound: &BTreeSet<Symbol>,
    imports: &HashMap<Symbol, ImportModuleInfo>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some((alias, member)) = qualified_alias_member(class) else {
        return;
    };
    if bound.contains(alias) {
        return;
    }
    let Some(info) = imports.get(alias) else {
        return;
    };
    if info.class_map.contains_key(member) {
        return;
    }
    diagnostics.push(diagnostic_for_span(
        span,
        format!("module `{alias}` does not export `{member}`"),
    ));
}

pub(crate) fn validate_import_projection_type_expr(
    ty: &TypeExpr,
    bound: &BTreeSet<Symbol>,
    imports: &HashMap<Symbol, ImportModuleInfo>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match ty {
        TypeExpr::Name(span, name) => {
            let Some((alias, member)) = qualified_alias_member(name) else {
                return;
            };
            if bound.contains(alias) {
                return;
            }
            let Some(info) = imports.get(alias) else {
                return;
            };
            if info.type_map.contains_key(member) || info.class_map.contains_key(member) {
                return;
            }
            diagnostics.push(diagnostic_for_span(
                *span,
                format!("module `{alias}` does not export `{member}`"),
            ));
        }
        TypeExpr::App(_, f, x) => {
            validate_import_projection_type_expr(f, bound, imports, diagnostics);
            validate_import_projection_type_expr(x, bound, imports, diagnostics);
        }
        TypeExpr::Fun(_, a, b) => {
            validate_import_projection_type_expr(a, bound, imports, diagnostics);
            validate_import_projection_type_expr(b, bound, imports, diagnostics);
        }
        TypeExpr::Tuple(_, elems) => {
            for e in elems {
                validate_import_projection_type_expr(e, bound, imports, diagnostics);
            }
        }
        TypeExpr::Record(_, fields) => {
            for (_, t) in fields {
                validate_import_projection_type_expr(t, bound, imports, diagnostics);
            }
        }
    }
}

pub(crate) fn validate_import_projection_expr(
    expr: &Expr,
    bound: &mut BTreeSet<Symbol>,
    imports: &HashMap<Symbol, ImportModuleInfo>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::Lam(_, _, param, ann, constraints, body) => {
            if let Some(ann) = ann {
                validate_import_projection_type_expr(ann, bound, imports, diagnostics);
            }
            for c in constraints {
                validate_import_projection_class_name(
                    &c.class,
                    *c.typ.span(),
                    bound,
                    imports,
                    diagnostics,
                );
                validate_import_projection_type_expr(&c.typ, bound, imports, diagnostics);
            }
            bound.insert(param.name.clone());
            validate_import_projection_expr(body, bound, imports, diagnostics);
            bound.remove(&param.name);
        }
        Expr::Let(_, var, _type_params, ann, val, body) => {
            if let Some(ann) = ann {
                validate_import_projection_type_expr(ann, bound, imports, diagnostics);
            }
            validate_import_projection_expr(val, bound, imports, diagnostics);
            bound.insert(var.name.clone());
            validate_import_projection_expr(body, bound, imports, diagnostics);
            bound.remove(&var.name);
        }
        Expr::LetRec(_, bindings, body) => {
            for (_, _, ann, _) in bindings {
                if let Some(ann) = ann {
                    validate_import_projection_type_expr(ann, bound, imports, diagnostics);
                }
            }
            let names: Vec<_> = bindings
                .iter()
                .map(|(var, _, _, _)| var.name.clone())
                .collect();
            for name in &names {
                bound.insert(name.clone());
            }
            for (_, _, _ann, def) in bindings {
                validate_import_projection_expr(def, bound, imports, diagnostics);
            }
            validate_import_projection_expr(body, bound, imports, diagnostics);
            for name in &names {
                bound.remove(name);
            }
        }
        Expr::Match(_, scrutinee, arms) => {
            validate_import_projection_expr(scrutinee, bound, imports, diagnostics);
            for (pat, arm_expr) in arms {
                let mut binds = Vec::new();
                collect_pattern_bindings(pat, &mut binds);
                for b in &binds {
                    bound.insert(b.clone());
                }
                validate_import_projection_expr(arm_expr, bound, imports, diagnostics);
                for b in &binds {
                    bound.remove(b);
                }
            }
        }
        Expr::Tuple(_, elems) | Expr::List(_, elems) => {
            for e in elems {
                validate_import_projection_expr(e, bound, imports, diagnostics);
            }
        }
        Expr::Dict(_, kvs) => {
            for v in kvs.values() {
                validate_import_projection_expr(v, bound, imports, diagnostics);
            }
        }
        Expr::RecordUpdate(_, base, updates) => {
            validate_import_projection_expr(base, bound, imports, diagnostics);
            for v in updates.values() {
                validate_import_projection_expr(v, bound, imports, diagnostics);
            }
        }
        Expr::App(_, f, x) => {
            validate_import_projection_expr(f, bound, imports, diagnostics);
            validate_import_projection_expr(x, bound, imports, diagnostics);
        }
        Expr::Ite(_, c, t, e) => {
            validate_import_projection_expr(c, bound, imports, diagnostics);
            validate_import_projection_expr(t, bound, imports, diagnostics);
            validate_import_projection_expr(e, bound, imports, diagnostics);
        }
        Expr::Ann(_, e, t) => {
            validate_import_projection_expr(e, bound, imports, diagnostics);
            validate_import_projection_type_expr(t, bound, imports, diagnostics);
        }
        Expr::Project(_, base, _) => {
            validate_import_projection_expr(base, bound, imports, diagnostics);
        }
        Expr::Var(..)
        | Expr::Bool(..)
        | Expr::Uint(..)
        | Expr::Int(..)
        | Expr::Float(..)
        | Expr::String(..)
        | Expr::Uuid(..)
        | Expr::DateTime(..)
        | Expr::Hole(..) => {}
    }
}

pub(crate) fn validate_import_projection_uses(
    compilation_unit: &CompilationUnit,
    imports: &HashMap<Symbol, ImportModuleInfo>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let decl_bound = BTreeSet::new();
    for decl in &compilation_unit.decls {
        match decl {
            Decl::Fn(fd) => {
                for (_, t) in &fd.params {
                    validate_import_projection_type_expr(t, &decl_bound, imports, diagnostics);
                }
                validate_import_projection_type_expr(&fd.ret, &decl_bound, imports, diagnostics);
                for c in &fd.constraints {
                    validate_import_projection_class_name(
                        &c.class,
                        *c.typ.span(),
                        &decl_bound,
                        imports,
                        diagnostics,
                    );
                    validate_import_projection_type_expr(&c.typ, &decl_bound, imports, diagnostics);
                }
                let mut bound: BTreeSet<Symbol> =
                    fd.params.iter().map(|(v, _)| v.name.clone()).collect();
                validate_import_projection_expr(fd.body.as_ref(), &mut bound, imports, diagnostics);
            }
            Decl::DeclareFn(df) => {
                for (_, t) in &df.params {
                    validate_import_projection_type_expr(t, &decl_bound, imports, diagnostics);
                }
                validate_import_projection_type_expr(&df.ret, &decl_bound, imports, diagnostics);
                for c in &df.constraints {
                    validate_import_projection_class_name(
                        &c.class,
                        *c.typ.span(),
                        &decl_bound,
                        imports,
                        diagnostics,
                    );
                    validate_import_projection_type_expr(&c.typ, &decl_bound, imports, diagnostics);
                }
            }
            Decl::Type(td) => {
                for v in &td.variants {
                    for t in &v.args {
                        validate_import_projection_type_expr(t, &decl_bound, imports, diagnostics);
                    }
                }
            }
            Decl::Class(cd) => {
                for c in &cd.supers {
                    validate_import_projection_class_name(
                        &c.class,
                        *c.typ.span(),
                        &decl_bound,
                        imports,
                        diagnostics,
                    );
                    validate_import_projection_type_expr(&c.typ, &decl_bound, imports, diagnostics);
                }
                for m in &cd.methods {
                    validate_import_projection_type_expr(&m.typ, &decl_bound, imports, diagnostics);
                }
            }
            Decl::Instance(inst) => {
                validate_import_projection_class_name(
                    &NameRef::from_dotted(inst.class.as_ref()),
                    inst.span,
                    &decl_bound,
                    imports,
                    diagnostics,
                );
                validate_import_projection_type_expr(&inst.head, &decl_bound, imports, diagnostics);
                for c in &inst.context {
                    validate_import_projection_class_name(
                        &c.class,
                        *c.typ.span(),
                        &decl_bound,
                        imports,
                        diagnostics,
                    );
                    validate_import_projection_type_expr(&c.typ, &decl_bound, imports, diagnostics);
                }
                for m in &inst.methods {
                    let mut bound = BTreeSet::new();
                    validate_import_projection_expr(
                        m.body.as_ref(),
                        &mut bound,
                        imports,
                        diagnostics,
                    );
                }
            }
            Decl::Import(..) => {}
        }
    }
    if let Some(body) = &compilation_unit.body {
        let mut bound = BTreeSet::new();
        validate_import_projection_expr(body.as_ref(), &mut bound, imports, diagnostics);
    }
}

pub type PreparedProgram = (
    CompilationUnit,
    TypeSystem,
    HashMap<Symbol, ImportModuleInfo>,
    Vec<Diagnostic>,
);

pub fn prepare_program_with_imports(
    uri: &Url,
    compilation_unit: &CompilationUnit,
) -> std::result::Result<PreparedProgram, String> {
    let mut ts =
        TypeSystem::new_with_prelude().map_err(|e| format!("failed to build prelude: {e}"))?;
    let mut diagnostics = Vec::new();

    let module_service = LspModuleService::current();

    let mut imports: HashMap<Symbol, ImportModuleInfo> = HashMap::new();

    for decl in &compilation_unit.decls {
        let Decl::Import(ImportDecl {
            span, path, alias, ..
        }) = decl
        else {
            continue;
        };
        let import_span = *span;

        let segments = match path {
            ImportPath::Local { segments, .. } => segments.as_slice(),
            ImportPath::Remote { .. } => {
                // LSP does not attempt network fetches; leave it unresolved.
                continue;
            }
        };

        let module_name = segments
            .iter()
            .map(|s| s.as_ref())
            .collect::<Vec<_>>()
            .join(".");

        let module = match module_service.load_import_path(uri, path) {
            Ok(Some(module)) => module,
            Ok(None) => {
                if uri_to_file_path(uri).is_some() {
                    diagnostics.push(diagnostic_for_span(
                        import_span,
                        format!("module not found for import `{module_name}`"),
                    ));
                }
                continue;
            }
            Err(err) => {
                diagnostics.push(diagnostic_for_span(import_span, err.to_string()));
                continue;
            }
        };
        let module_path = module.path.clone();
        let hash = module.hash;
        let source = module.source;
        let module_label = module.label;
        let keep_constraints = module.keep_constraints;

        let (tokens, module_program) = match tokenize_and_parse(&source) {
            Ok(v) => v,
            Err(TokenizeOrParseError::Lex(err)) => {
                let msg = match err {
                    LexicalError::UnexpectedToken(span) => format!(
                        "lex error in module `{}` at {}:{}",
                        module_label, span.begin.line, span.begin.column
                    ),
                    LexicalError::UnclosedBlockComment(span) => format!(
                        "lex error in module `{}` at {}:{}: unclosed block comment opener (/*)",
                        module_label, span.begin.line, span.begin.column
                    ),
                    LexicalError::UnmatchedBlockCommentClose(span) => format!(
                        "lex error in module `{}` at {}:{}: unmatched block comment closer (*/)",
                        module_label, span.begin.line, span.begin.column
                    ),
                    LexicalError::InvalidLiteral {
                        kind,
                        text,
                        error,
                        span,
                    } => format!(
                        "lex error in module `{}` at {}:{}: invalid {kind} literal `{text}`: {error}",
                        module_label, span.begin.line, span.begin.column
                    ),
                    LexicalError::Internal(msg) => {
                        format!("internal lexer error in module `{module_label}`: {msg}")
                    }
                };
                diagnostics.push(diagnostic_for_span(import_span, msg));
                continue;
            }
            Err(TokenizeOrParseError::Parse(errs)) => {
                for err in errs {
                    diagnostics.push(diagnostic_for_span(
                        import_span,
                        format!(
                            "parse error in module `{}` at {}:{}: {}",
                            module_label, err.span.begin.line, err.span.begin.column, err.message
                        ),
                    ));
                    if diagnostics.len() >= MAX_DIAGNOSTICS {
                        break;
                    }
                }
                continue;
            }
        };

        let index = index_decl_spans(&module_program, &tokens);
        let prefix = module_prefix(&hash);

        let mut type_map: HashMap<Symbol, Symbol> = HashMap::new();
        let mut class_map: HashMap<Symbol, Symbol> = HashMap::new();
        for decl in &module_program.decls {
            match decl {
                Decl::Type(td) => {
                    type_map.insert(
                        td.name.clone(),
                        Symbol::intern(&format!("{prefix}.{}", td.name.as_ref())),
                    );
                }
                Decl::Class(cd) => {
                    class_map.insert(
                        cd.name.clone(),
                        Symbol::intern(&format!("{prefix}.{}", cd.name.as_ref())),
                    );
                }
                _ => {}
            }
        }

        // Inject module type decls (renamed) so exported signatures can refer to them.
        for decl in &module_program.decls {
            let Decl::Type(td) = decl else { continue };
            let name = type_map
                .get(&td.name)
                .cloned()
                .unwrap_or_else(|| td.name.clone());
            let variants = td
                .variants
                .iter()
                .map(|v| TypeVariant {
                    name: Symbol::intern(&format!("{prefix}.{}", v.name.as_ref())),
                    args: v
                        .args
                        .iter()
                        .map(|t| rewrite_type_expr(t, &type_map))
                        .collect(),
                })
                .collect();
            let td2 = TypeDecl {
                span: td.span,
                is_pub: td.is_pub,
                name,
                params: td.params.clone(),
                variants,
            };
            let _ = ts.register_type_decl(&td2);
        }

        let mut value_map: HashMap<Symbol, Symbol> = HashMap::new();
        let mut export_names: BTreeSet<String> = BTreeSet::new();

        // Exported functions (pub only)
        for decl in &module_program.decls {
            match decl {
                Decl::Fn(fd) if fd.is_pub => {
                    let internal = Symbol::intern(&format!("{prefix}.{}", fd.name.name.as_ref()));
                    value_map.insert(Symbol::intern(fd.name.name.as_ref()), internal.clone());
                    export_names.insert(fd.name.name.as_ref().to_string());

                    let params = fd
                        .params
                        .iter()
                        .map(|(v, ty)| (v.clone(), rewrite_type_expr(ty, &type_map)))
                        .collect();
                    let ret = rewrite_type_expr(&fd.ret, &type_map);
                    let decl = DeclareFnDecl {
                        span: fd.span,
                        is_pub: true,
                        name: Var {
                            span: fd.name.span,
                            name: internal,
                        },
                        type_params: fd.type_params.clone(),
                        params,
                        ret,
                        constraints: if keep_constraints {
                            fd.constraints.clone()
                        } else {
                            Default::default()
                        },
                    };
                    let _ = ts.inject_declare_fn_decl(&decl);
                }
                Decl::DeclareFn(df) if df.is_pub => {
                    let internal = Symbol::intern(&format!("{prefix}.{}", df.name.name.as_ref()));
                    value_map.insert(Symbol::intern(df.name.name.as_ref()), internal.clone());
                    export_names.insert(df.name.name.as_ref().to_string());

                    let params = df
                        .params
                        .iter()
                        .map(|(v, ty)| (v.clone(), rewrite_type_expr(ty, &type_map)))
                        .collect();
                    let ret = rewrite_type_expr(&df.ret, &type_map);
                    let decl = DeclareFnDecl {
                        span: df.span,
                        is_pub: true,
                        name: Var {
                            span: df.name.span,
                            name: internal,
                        },
                        type_params: df.type_params.clone(),
                        params,
                        ret,
                        constraints: if keep_constraints {
                            df.constraints.clone()
                        } else {
                            Default::default()
                        },
                    };
                    let _ = ts.inject_declare_fn_decl(&decl);
                }
                Decl::Type(td) if td.is_pub => {
                    // Public constructors are accessible as values.
                    for variant in &td.variants {
                        let internal =
                            Symbol::intern(&format!("{prefix}.{}", variant.name.as_ref()));
                        value_map.insert(variant.name.clone(), internal);
                        export_names.insert(variant.name.as_ref().to_string());
                    }
                }
                _ => {}
            }
        }

        let mut export_defs = HashMap::new();
        for name in &export_names {
            if let Some(span) = index
                .fn_defs
                .get(name)
                .copied()
                .or_else(|| index.ctor_defs.get(name).copied())
            {
                export_defs.insert(name.clone(), span);
            }
        }

        imports.insert(
            alias.clone(),
            ImportModuleInfo {
                path: module_path,
                value_map,
                type_map,
                class_map,
                export_defs,
            },
        );
    }

    validate_import_projection_uses(compilation_unit, &imports, &mut diagnostics);
    let rewritten =
        rewrite_program_import_projections(compilation_unit, &imports, &mut diagnostics);
    Ok((rewritten, ts, imports, diagnostics))
}

pub(crate) fn completion_exports_for_module_alias(
    uri: &Url,
    compilation_unit: &CompilationUnit,
    alias: &str,
) -> std::result::Result<Vec<String>, String> {
    let alias_sym = Symbol::intern(alias);
    let Some(import_decl) = compilation_unit.decls.iter().find_map(|d| {
        let Decl::Import(id) = d else { return None };
        if id.alias == alias_sym {
            Some(id)
        } else {
            None
        }
    }) else {
        return Ok(Vec::new());
    };

    let Some(module) = LspModuleService::current()
        .load_import_path(uri, &import_decl.path)
        .map_err(|err| err.to_string())?
    else {
        return Ok(Vec::new());
    };
    let source = module.source;
    let (_tokens, module_program) =
        tokenize_and_parse(&source).map_err(|_| "parse error".to_string())?;

    let mut exports = BTreeSet::new();
    for decl in &module_program.decls {
        match decl {
            Decl::Fn(fd) if fd.is_pub => {
                exports.insert(fd.name.name.as_ref().to_string());
            }
            Decl::DeclareFn(df) if df.is_pub => {
                exports.insert(df.name.name.as_ref().to_string());
            }
            Decl::Type(td) if td.is_pub => {
                for variant in &td.variants {
                    exports.insert(variant.name.as_ref().to_string());
                }
            }
            _ => {}
        }
    }
    Ok(exports.into_iter().collect())
}

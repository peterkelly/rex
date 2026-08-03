use crate::{
    builder::registry::NativeRegistry, env::RootedEnvironment, error::EngineError,
    modules::collect_pattern_bindings,
};
use rex_ast::{Expr, Span, Symbol};
use rex_typesystem::{
    error::TypeError,
    inference::infer_typed,
    types::{BuiltinTypeId, Predicate, Type, TypeKind, TypedExpr, TypedExprKind, Types},
    typesystem::{TypeSystem, entails},
    unification::{Subst, compose_subst, unify},
};

pub(crate) fn type_check_engine<State>(
    type_system: &mut TypeSystem,
    env: &RootedEnvironment,
    natives: &NativeRegistry<State>,
    expr: &Expr,
) -> Result<TypedExpr, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    if let Some(span) = first_hole_span(expr) {
        return Err(EngineError::Type(TypeError::Spanned {
            span,
            error: Box::new(TypeError::UnsupportedExpr(
                "typed hole `?` must be filled before evaluation",
            )),
        }));
    }
    let (typed, preds, _ty) = infer_typed(type_system, expr)?;
    let (typed, preds) = default_ambiguous_types(type_system, typed, preds)?;
    check_predicates(type_system, &preds)?;
    check_natives(type_system, env, natives, &typed)?;
    Ok(typed)
}

fn check_predicates(type_system: &TypeSystem, preds: &[Predicate]) -> Result<(), EngineError> {
    for pred in preds {
        if pred.typ.ftv().is_empty() {
            let ok = entails(&type_system.classes, &[], pred)?;
            if !ok {
                return Err(EngineError::Type(TypeError::NoInstance(
                    pred.class.clone(),
                    pred.typ.to_string(),
                )));
            }
        }
    }
    Ok(())
}

pub(crate) fn check_natives<State>(
    type_system: &TypeSystem,
    env: &RootedEnvironment,
    natives: &NativeRegistry<State>,
    expr: &TypedExpr,
) -> Result<(), EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    enum ScopeWalkStep<'b> {
        Expr(&'b TypedExpr),
        Push(Symbol),
        PushMany(Vec<Symbol>),
        Pop(usize),
    }

    let mut bound: Vec<Symbol> = Vec::new();
    let mut stack = vec![ScopeWalkStep::Expr(expr)];
    while let Some(frame) = stack.pop() {
        match frame {
            ScopeWalkStep::Expr(expr) => match expr.kind.as_ref() {
                TypedExprKind::Var { name, overloads } => {
                    if bound.iter().any(|n| n == name) {
                        continue;
                    }
                    if !natives.has_name(name) {
                        if env.contains(name) {
                            continue;
                        }
                        if type_system.class_methods.contains_key(name) {
                            continue;
                        }
                        return Err(EngineError::UnknownVar(name.clone()));
                    }
                    if !overloads.is_empty()
                        && expr.typ.ftv().is_empty()
                        && !overloads.iter().any(|t| unify(t, &expr.typ).is_ok())
                    {
                        return Err(EngineError::MissingImpl {
                            name: name.clone(),
                            typ: expr.typ.to_string(),
                        });
                    }
                    if expr.typ.ftv().is_empty() {
                        let _ = natives.resolve(name, &expr.typ)?;
                    }
                }
                TypedExprKind::Tuple(elems) | TypedExprKind::List(elems) => {
                    for elem in elems.iter().rev() {
                        stack.push(ScopeWalkStep::Expr(elem));
                    }
                }
                TypedExprKind::Dict(kvs) => {
                    for v in kvs.values().rev() {
                        stack.push(ScopeWalkStep::Expr(v));
                    }
                }
                TypedExprKind::RecordUpdate { base, updates } => {
                    for v in updates.values().rev() {
                        stack.push(ScopeWalkStep::Expr(v));
                    }
                    stack.push(ScopeWalkStep::Expr(base));
                }
                TypedExprKind::App(f, x) => {
                    stack.push(ScopeWalkStep::Expr(x));
                    stack.push(ScopeWalkStep::Expr(f));
                }
                TypedExprKind::Project { expr, .. } => stack.push(ScopeWalkStep::Expr(expr)),
                TypedExprKind::Lam { param, body } => {
                    stack.push(ScopeWalkStep::Pop(1));
                    stack.push(ScopeWalkStep::Expr(body));
                    stack.push(ScopeWalkStep::Push(param.clone()));
                }
                TypedExprKind::Let { name, def, body } => {
                    stack.push(ScopeWalkStep::Pop(1));
                    stack.push(ScopeWalkStep::Expr(body));
                    stack.push(ScopeWalkStep::Push(name.clone()));
                    stack.push(ScopeWalkStep::Expr(def));
                }
                TypedExprKind::LetRec { bindings, body } => {
                    if !bindings.is_empty() {
                        stack.push(ScopeWalkStep::Pop(bindings.len()));
                        stack.push(ScopeWalkStep::Expr(body));
                        for (_, def) in bindings.iter().rev() {
                            stack.push(ScopeWalkStep::Expr(def));
                        }
                        stack.push(ScopeWalkStep::PushMany(
                            bindings.iter().map(|(name, _)| name.clone()).collect(),
                        ));
                    } else {
                        stack.push(ScopeWalkStep::Expr(body));
                    }
                }
                TypedExprKind::Ite {
                    cond,
                    then_expr,
                    else_expr,
                } => {
                    stack.push(ScopeWalkStep::Expr(else_expr));
                    stack.push(ScopeWalkStep::Expr(then_expr));
                    stack.push(ScopeWalkStep::Expr(cond));
                }
                TypedExprKind::Match { scrutinee, arms } => {
                    for (pat, arm_expr) in arms.iter().rev() {
                        let mut bindings = Vec::new();
                        collect_pattern_bindings(pat, &mut bindings);
                        let count = bindings.len();
                        if count != 0 {
                            stack.push(ScopeWalkStep::Pop(count));
                            stack.push(ScopeWalkStep::Expr(arm_expr));
                            stack.push(ScopeWalkStep::PushMany(bindings));
                        } else {
                            stack.push(ScopeWalkStep::Expr(arm_expr));
                        }
                    }
                    stack.push(ScopeWalkStep::Expr(scrutinee));
                }
                TypedExprKind::Bool(..)
                | TypedExprKind::Uint(..)
                | TypedExprKind::Int(..)
                | TypedExprKind::Float(..)
                | TypedExprKind::String(..)
                | TypedExprKind::Uuid(..)
                | TypedExprKind::DateTime(..) => {}
                TypedExprKind::Hole => return Err(EngineError::UnsupportedExpr),
            },
            ScopeWalkStep::Push(sym) => bound.push(sym),
            ScopeWalkStep::PushMany(syms) => bound.extend(syms),
            ScopeWalkStep::Pop(count) => bound.truncate(bound.len().saturating_sub(count)),
        }
    }
    Ok(())
}

fn first_hole_span(expr: &Expr) -> Option<Span> {
    let mut stack = vec![expr];
    while let Some(expr) = stack.pop() {
        match expr {
            Expr::Hole(span) => return Some(*span),
            Expr::App(_, f, x) => {
                stack.push(x);
                stack.push(f);
            }
            Expr::Project(_, base, _) | Expr::Ann(_, base, _) => stack.push(base),
            Expr::Lam(_, _scope, _param, _ann, _constraints, body) => stack.push(body),
            Expr::Let(_, _var, _type_params, _ann, def, body) => {
                stack.push(body);
                stack.push(def);
            }
            Expr::LetRec(_, bindings, body) => {
                stack.push(body);
                for (_var, _type_params, _ann, def) in bindings.iter().rev() {
                    stack.push(def);
                }
            }
            Expr::Ite(_, cond, then_expr, else_expr) => {
                stack.push(else_expr);
                stack.push(then_expr);
                stack.push(cond);
            }
            Expr::Match(_, scrutinee, arms) => {
                for (_pat, arm) in arms.iter().rev() {
                    stack.push(arm);
                }
                stack.push(scrutinee);
            }
            Expr::Tuple(_, elems) | Expr::List(_, elems) => {
                for elem in elems.iter().rev() {
                    stack.push(elem);
                }
            }
            Expr::Dict(_, kvs) => {
                for value in kvs.values().rev() {
                    stack.push(value);
                }
            }
            Expr::RecordUpdate(_, base, kvs) => {
                for value in kvs.values().rev() {
                    stack.push(value);
                }
                stack.push(base);
            }
            Expr::Bool(..)
            | Expr::Uint(..)
            | Expr::Int(..)
            | Expr::Float(..)
            | Expr::String(..)
            | Expr::Uuid(..)
            | Expr::DateTime(..)
            | Expr::Var(..) => {}
        }
    }
    None
}

fn default_ambiguous_types(
    type_system: &TypeSystem,
    typed: TypedExpr,
    mut preds: Vec<Predicate>,
) -> Result<(TypedExpr, Vec<Predicate>), EngineError> {
    let mut candidates = Vec::new();
    collect_default_candidates(&typed, &mut candidates);
    for ty in [
        Type::builtin(BuiltinTypeId::F32),
        Type::builtin(BuiltinTypeId::I32),
        Type::builtin(BuiltinTypeId::String),
    ] {
        push_unique_type(&mut candidates, ty);
    }

    let mut subst = Subst::new_sync();
    loop {
        let vars: Vec<_> = preds.ftv().into_iter().collect();
        let mut progress = false;
        for tv in vars {
            if subst.get(&tv).is_some() {
                continue;
            }
            let mut relevant = Vec::new();
            let mut simple = true;
            for pred in &preds {
                if pred.typ.ftv().contains(&tv) {
                    match pred.typ.as_ref() {
                        TypeKind::Var(v) if v.id == tv => relevant.push(pred.clone()),
                        _ => {
                            simple = false;
                            break;
                        }
                    }
                }
            }
            if !simple || !predicates_are_defaultable(&relevant) {
                continue;
            }
            if let Some(choice) = choose_default_type(type_system, &relevant, &candidates)? {
                let mut next = Subst::new_sync();
                next = next.insert(tv, choice.clone());
                preds = preds.apply(&next);
                subst = compose_subst(next, subst);
                progress = true;
            }
        }
        if !progress {
            break;
        }
    }
    Ok((typed.apply(&subst), preds))
}

fn collect_default_candidates(expr: &TypedExpr, out: &mut Vec<Type>) {
    let mut stack: Vec<&TypedExpr> = vec![expr];
    while let Some(expr) = stack.pop() {
        if expr.typ.ftv().is_empty()
            && let TypeKind::Con(tc) = expr.typ.as_ref()
            && tc.arity() == 0
        {
            push_unique_type(out, expr.typ.clone());
        }

        match expr.kind.as_ref() {
            TypedExprKind::Tuple(elems) | TypedExprKind::List(elems) => {
                for elem in elems.iter().rev() {
                    stack.push(elem);
                }
            }
            TypedExprKind::Dict(kvs) => {
                for value in kvs.values().rev() {
                    stack.push(value);
                }
            }
            TypedExprKind::RecordUpdate { base, updates } => {
                for value in updates.values().rev() {
                    stack.push(value);
                }
                stack.push(base);
            }
            TypedExprKind::App(f, x) => {
                stack.push(x);
                stack.push(f);
            }
            TypedExprKind::Project { expr, .. } => stack.push(expr),
            TypedExprKind::Lam { body, .. } => stack.push(body),
            TypedExprKind::Let { def, body, .. } => {
                stack.push(body);
                stack.push(def);
            }
            TypedExprKind::LetRec { bindings, body } => {
                stack.push(body);
                for (_, def) in bindings.iter().rev() {
                    stack.push(def);
                }
            }
            TypedExprKind::Ite {
                cond,
                then_expr,
                else_expr,
            } => {
                stack.push(else_expr);
                stack.push(then_expr);
                stack.push(cond);
            }
            TypedExprKind::Match { scrutinee, arms } => {
                for (_, expr) in arms.iter().rev() {
                    stack.push(expr);
                }
                stack.push(scrutinee);
            }
            TypedExprKind::Var { .. }
            | TypedExprKind::Bool(..)
            | TypedExprKind::Uint(..)
            | TypedExprKind::Int(..)
            | TypedExprKind::Float(..)
            | TypedExprKind::String(..)
            | TypedExprKind::Uuid(..)
            | TypedExprKind::DateTime(..)
            | TypedExprKind::Hole => {}
        }
    }
}

fn push_unique_type(out: &mut Vec<Type>, typ: Type) {
    if !out.iter().any(|t| t == &typ) {
        out.push(typ);
    }
}

fn predicates_are_defaultable(preds: &[Predicate]) -> bool {
    let has_numeric_predicate = preds
        .iter()
        .any(|pred| numeric_defaultable_class(&pred.class));
    has_numeric_predicate
        && preds.iter().all(|pred| {
            numeric_defaultable_class(&pred.class) || defaultable_companion_class(&pred.class)
        })
}

fn choose_default_type(
    type_system: &TypeSystem,
    preds: &[Predicate],
    candidates: &[Type],
) -> Result<Option<Type>, EngineError> {
    for candidate in candidates {
        let mut ok = true;
        for pred in preds {
            let test = Predicate::new(pred.class.clone(), candidate.clone());
            if !entails(&type_system.classes, &[], &test)? {
                ok = false;
                break;
            }
        }
        if ok {
            return Ok(Some(candidate.clone()));
        }
    }
    Ok(None)
}

fn numeric_defaultable_class(class: &Symbol) -> bool {
    matches!(
        class.as_ref(),
        "AdditiveMonoid"
            | "MultiplicativeMonoid"
            | "Subtractive"
            | "AdditiveGroup"
            | "Ring"
            | "Divisive"
            | "Field"
            | "Integral"
    )
}

fn defaultable_companion_class(class: &Symbol) -> bool {
    matches!(class.as_ref(), "Eq" | "Ord")
}

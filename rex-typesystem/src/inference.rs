use crate::{
    error::TypeError,
    types::{
        AdtDecl, AdtVariant, BuiltinTypeId, Predicate, Scheme, Type, TypeConst, TypeEnv, TypeKind,
        TypeVar, TypeVarId, TypedExpr, TypedExprKind, Types,
    },
    typesystem::{
        TypeSystem, TypeVarSupply, instantiate, is_integral_literal_expr,
        predicates_from_constraints, reject_ambiguous_scheme, type_from_annotation_expr,
        type_from_annotation_expr_vars,
    },
    unification::{Subst, Unifier, compose_subst, subst_is_empty, unify},
};
use rex_ast::expr::{Expr, Pattern, Symbol, TypeConstraint, TypeExpr, sym};
use rex_util::gas::GasMeter;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

fn dedup_preds(preds: Vec<Predicate>) -> Vec<Predicate> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::with_capacity(preds.len());
    for pred in preds {
        if seen.insert(pred.clone()) {
            out.push(pred);
        }
    }
    out
}

fn is_integral_primitive(typ: &Type) -> bool {
    matches!(
        typ.as_ref(),
        TypeKind::Con(TypeConst {
            builtin_id: Some(
                BuiltinTypeId::U8
                    | BuiltinTypeId::U16
                    | BuiltinTypeId::U32
                    | BuiltinTypeId::U64
                    | BuiltinTypeId::I8
                    | BuiltinTypeId::I16
                    | BuiltinTypeId::I32
                    | BuiltinTypeId::I64
            ),
            ..
        })
    )
}

fn finalize_infer_for_public_api(
    mut preds: Vec<Predicate>,
    mut typ: Type,
) -> Result<(Vec<Predicate>, Type), TypeError> {
    let mut subst = Subst::new_sync();
    for pred in &preds {
        if pred.class.as_ref() == "Integral"
            && let TypeKind::Var(tv) = pred.typ.as_ref()
        {
            subst = subst.insert(tv.id, Type::builtin(BuiltinTypeId::I32));
        }
    }

    if !subst_is_empty(&subst) {
        preds = dedup_preds(preds.apply(&subst));
        typ = typ.apply(&subst);
    }

    for pred in &preds {
        if pred.class.as_ref() != "Integral" {
            continue;
        }
        if matches!(pred.typ.as_ref(), TypeKind::Var(_)) || is_integral_primitive(&pred.typ) {
            continue;
        }
        return Err(TypeError::Unification("i32".into(), pred.typ.to_string()));
    }

    Ok((preds, typ))
}

#[derive(Clone, Debug)]
struct KnownVariant {
    adt: Symbol,
    variant: Symbol,
}

type KnownVariants = BTreeMap<Symbol, KnownVariant>;

fn apply_scheme_with_unifier(scheme: &Scheme, unifier: &mut Unifier<'_>) -> Scheme {
    let preds = scheme
        .preds
        .iter()
        .map(|pred| Predicate::new(pred.class.clone(), unifier.apply_type(&pred.typ)))
        .collect();
    let typ = unifier.apply_type(&scheme.typ);
    Scheme::new(scheme.vars.clone(), preds, typ)
}

fn scheme_ftv_with_unifier(scheme: &Scheme, unifier: &mut Unifier<'_>) -> BTreeSet<TypeVarId> {
    let mut ftv = unifier.apply_type(&scheme.typ).ftv();
    for pred in &scheme.preds {
        ftv.extend(unifier.apply_type(&pred.typ).ftv());
    }
    for var in &scheme.vars {
        ftv.remove(&var.id);
    }
    ftv
}

fn env_ftv_with_unifier(env: &TypeEnv, unifier: &mut Unifier<'_>) -> BTreeSet<TypeVarId> {
    let mut out = BTreeSet::new();
    for (_name, schemes) in env.values.iter() {
        for scheme in schemes {
            out.extend(scheme_ftv_with_unifier(scheme, unifier));
        }
    }
    out
}

fn generalize_with_unifier(
    env: &TypeEnv,
    preds: Vec<Predicate>,
    typ: Type,
    unifier: &mut Unifier<'_>,
) -> Scheme {
    let preds: Vec<Predicate> = preds
        .into_iter()
        .map(|pred| Predicate::new(pred.class, unifier.apply_type(&pred.typ)))
        .collect();
    let typ = unifier.apply_type(&typ);
    let mut vars: Vec<TypeVar> = typ
        .ftv()
        .union(&preds.ftv())
        .copied()
        .collect::<BTreeSet<_>>()
        .difference(&env_ftv_with_unifier(env, unifier))
        .cloned()
        .map(|id| TypeVar::new(id, None))
        .collect();
    vars.sort_by_key(|v| v.id);
    Scheme::new(vars, preds, typ)
}

fn monomorphic_scheme_with_unifier(
    preds: Vec<Predicate>,
    typ: Type,
    unifier: &mut Unifier<'_>,
) -> Scheme {
    let preds = dedup_preds(
        preds
            .into_iter()
            .map(|pred| Predicate::new(pred.class, unifier.apply_type(&pred.typ)))
            .collect(),
    );
    let typ = unifier.apply_type(&typ);
    Scheme::new(vec![], preds, typ)
}

pub fn infer_typed(
    type_system: &mut TypeSystem,
    expr: &Expr,
) -> Result<(TypedExpr, Vec<Predicate>, Type), TypeError> {
    infer_typed_inner(type_system, expr)
}

pub fn infer_typed_with_gas(
    type_system: &mut TypeSystem,
    expr: &Expr,
    gas: &mut GasMeter,
) -> Result<(TypedExpr, Vec<Predicate>, Type), TypeError> {
    let known = KnownVariants::new();
    let mut unifier = Unifier::with_gas(gas, type_system.limits.max_infer_depth);
    let (preds, t, typed) = infer_expr(
        &mut unifier,
        &mut type_system.supply,
        &type_system.env,
        &type_system.adts,
        &known,
        expr,
    )
    .map_err(|err| err.with_span(expr.span()))?;
    let subst = unifier.into_subst();
    let mut typed = typed.apply(&subst);
    let mut preds = dedup_preds(preds.apply(&subst));
    let mut t = t.apply(&subst);
    let improve = improve_indexable(&preds)?;
    if !subst_is_empty(&improve) {
        typed = typed.apply(&improve);
        preds = dedup_preds(preds.apply(&improve));
        t = t.apply(&improve);
    }
    type_system.check_predicate_kinds(&preds)?;
    Ok((typed, preds, t))
}

fn infer_typed_inner(
    type_system: &mut TypeSystem,
    expr: &Expr,
) -> Result<(TypedExpr, Vec<Predicate>, Type), TypeError> {
    let known = KnownVariants::new();
    let mut unifier = Unifier::new(type_system.limits.max_infer_depth);
    let (preds, t, typed) = infer_expr(
        &mut unifier,
        &mut type_system.supply,
        &type_system.env,
        &type_system.adts,
        &known,
        expr,
    )
    .map_err(|err| err.with_span(expr.span()))?;
    let subst = unifier.into_subst();
    let mut typed = typed.apply(&subst);
    let mut preds = dedup_preds(preds.apply(&subst));
    let mut t = t.apply(&subst);
    let improve = improve_indexable(&preds)?;
    if !subst_is_empty(&improve) {
        typed = typed.apply(&improve);
        preds = dedup_preds(preds.apply(&improve));
        t = t.apply(&improve);
    }
    type_system.check_predicate_kinds(&preds)?;
    Ok((typed, preds, t))
}

pub fn infer(
    type_system: &mut TypeSystem,
    expr: &Expr,
) -> Result<(Vec<Predicate>, Type), TypeError> {
    infer_inner(type_system, expr)
}

pub fn infer_with_gas(
    type_system: &mut TypeSystem,
    expr: &Expr,
    gas: &mut GasMeter,
) -> Result<(Vec<Predicate>, Type), TypeError> {
    let known = KnownVariants::new();
    let mut unifier = Unifier::with_gas(gas, type_system.limits.max_infer_depth);
    let (preds, t) = infer_expr_type(
        &mut unifier,
        &mut type_system.supply,
        &type_system.env,
        &type_system.adts,
        &known,
        expr,
    )
    .map_err(|err| err.with_span(expr.span()))?;
    let subst = unifier.into_subst();
    let preds = dedup_preds(preds.apply(&subst));
    let t = t.apply(&subst);
    type_system.check_predicate_kinds(&preds)?;
    finalize_infer_for_public_api(preds, t)
}

fn infer_inner(
    type_system: &mut TypeSystem,
    expr: &Expr,
) -> Result<(Vec<Predicate>, Type), TypeError> {
    let known = KnownVariants::new();
    let mut unifier = Unifier::new(type_system.limits.max_infer_depth);
    let (preds, t) = infer_expr_type(
        &mut unifier,
        &mut type_system.supply,
        &type_system.env,
        &type_system.adts,
        &known,
        expr,
    )
    .map_err(|err| err.with_span(expr.span()))?;
    let subst = unifier.into_subst();
    let mut preds = dedup_preds(preds.apply(&subst));
    let mut t = t.apply(&subst);
    let improve = improve_indexable(&preds)?;
    if !subst_is_empty(&improve) {
        preds = dedup_preds(preds.apply(&improve));
        t = t.apply(&improve);
    }
    type_system.check_predicate_kinds(&preds)?;
    finalize_infer_for_public_api(preds, t)
}

fn improve_indexable(preds: &[Predicate]) -> Result<Subst, TypeError> {
    let mut subst = Subst::new_sync();
    loop {
        let mut changed = false;
        for pred in preds {
            let pred = pred.apply(&subst);
            if pred.class.as_ref() != "Indexable" {
                continue;
            }
            let TypeKind::Tuple(parts) = pred.typ.as_ref() else {
                continue;
            };
            if parts.len() != 2 {
                continue;
            }
            let container = parts[0].clone();
            let elem = parts[1].clone();
            let s = indexable_elem_subst(&container, &elem)?;
            if !subst_is_empty(&s) {
                subst = compose_subst(s, subst);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    Ok(subst)
}

fn indexable_elem_subst(container: &Type, elem: &Type) -> Result<Subst, TypeError> {
    match container.as_ref() {
        TypeKind::App(head, arg) => match head.as_ref() {
            TypeKind::Con(tc)
                if matches!(
                    tc.builtin_id,
                    Some(BuiltinTypeId::List | BuiltinTypeId::Array)
                ) =>
            {
                unify(elem, arg)
            }
            _ => Ok(Subst::new_sync()),
        },
        TypeKind::Tuple(elems) => {
            if elems.is_empty() {
                return Ok(Subst::new_sync());
            }
            let mut subst = Subst::new_sync();
            let mut cur = elems[0].clone();
            for ty in elems.iter().skip(1) {
                let s_next = unify(&cur.apply(&subst), &ty.apply(&subst))?;
                subst = compose_subst(s_next, subst);
                cur = cur.apply(&subst);
            }
            let elem = elem.apply(&subst);
            let s_elem = unify(&elem, &cur.apply(&subst))?;
            Ok(compose_subst(s_elem, subst))
        }
        _ => Ok(Subst::new_sync()),
    }
}

type LambdaChain<'a> = (
    Vec<(Symbol, Option<TypeExpr>)>,
    Vec<TypeConstraint>,
    &'a Expr,
);

fn collect_lambda_chain<'a>(expr: &'a Expr) -> LambdaChain<'a> {
    let mut params = Vec::new();
    let mut constraints = Vec::new();
    let mut cur = expr;
    let mut seen_constraints = false;
    while let Expr::Lam(_, _scope, param, ann, lam_constraints, body) = cur {
        if !lam_constraints.is_empty() {
            if seen_constraints {
                break;
            }
            constraints = lam_constraints.clone();
            seen_constraints = true;
        }
        params.push((param.name.clone(), ann.clone()));
        cur = body.as_ref();
    }
    (params, constraints, cur)
}

fn collect_app_chain(expr: &Expr) -> (&Expr, Vec<&Expr>) {
    let mut args = Vec::new();
    let mut cur = expr;
    while let Expr::App(_, f, x) = cur {
        args.push(x.as_ref());
        cur = f.as_ref();
    }
    args.reverse();
    (cur, args)
}

fn narrow_overload_candidates(candidates: &[Type], arg_ty: &Type) -> Vec<Type> {
    let mut out = Vec::new();
    for candidate in candidates {
        let Some((params, ret)) = decompose_fun(candidate, 1) else {
            continue;
        };
        let param = &params[0];
        if let Ok(s) = unify(param, arg_ty) {
            out.push(ret.apply(&s));
        }
    }
    out
}

fn unary_app_arg(typ: &Type, ctor_name: &str) -> Option<Type> {
    let TypeKind::App(head, arg) = typ.as_ref() else {
        return None;
    };
    let TypeKind::Con(tc) = head.as_ref() else {
        return None;
    };
    (tc.name.as_ref() == ctor_name && tc.arity == 1).then(|| arg.clone())
}

fn infer_app_arg_type(
    unifier: &mut Unifier<'_>,
    supply: &mut TypeVarSupply,
    env: &TypeEnv,
    adts: &BTreeMap<Symbol, AdtDecl>,
    known: &KnownVariants,
    arg_hint: Option<Type>,
    arg: &Expr,
) -> Result<(Vec<Predicate>, Type), TypeError> {
    match (arg_hint, arg) {
        (Some(arg_hint), Expr::RecordUpdate(_, base, updates)) => {
            infer_record_update_type_with_hint(
                unifier,
                supply,
                env,
                adts,
                known,
                base.as_ref(),
                updates,
                &arg_hint,
            )
        }
        (Some(arg_hint), Expr::Dict(_, kvs))
            if matches!(arg_hint.as_ref(), TypeKind::Record(..)) =>
        {
            let TypeKind::Record(fields) = arg_hint.as_ref() else {
                unreachable!("guarded by matches!")
            };
            let expected: BTreeMap<_, _> =
                fields.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            let mut seen = BTreeSet::new();
            let mut preds = Vec::new();
            for (k, v) in kvs {
                let expected_ty = expected
                    .get(k)
                    .ok_or_else(|| TypeError::UnknownField {
                        field: k.clone(),
                        typ: Type::record(fields.clone()).to_string(),
                    })?
                    .clone();
                let (p1, t1) = infer_expr_type(unifier, supply, env, adts, known, v.as_ref())?;
                unifier.unify(&t1, &expected_ty)?;
                preds.extend(p1);
                seen.insert(k.clone());
            }
            for key in expected.keys() {
                if !seen.contains(key.as_ref()) {
                    return Err(TypeError::UnknownField {
                        field: key.clone(),
                        typ: Type::record(fields.clone()).to_string(),
                    });
                }
            }
            let record_ty = Type::record(
                fields
                    .iter()
                    .map(|(k, v)| (k.clone(), unifier.apply_type(v)))
                    .collect(),
            );
            Ok((preds, record_ty))
        }
        _ => infer_expr_type(unifier, supply, env, adts, known, arg),
    }
}

fn infer_app_arg_typed(
    unifier: &mut Unifier<'_>,
    supply: &mut TypeVarSupply,
    env: &TypeEnv,
    adts: &BTreeMap<Symbol, AdtDecl>,
    known: &KnownVariants,
    arg_hint: Option<Type>,
    arg: &Expr,
) -> Result<(Vec<Predicate>, Type, TypedExpr), TypeError> {
    match (arg_hint, arg) {
        (Some(arg_hint), Expr::RecordUpdate(_, base, updates)) => {
            infer_record_update_typed_with_hint(
                unifier,
                supply,
                env,
                adts,
                known,
                base.as_ref(),
                updates,
                &arg_hint,
            )
        }
        (Some(arg_hint), Expr::Dict(_, kvs))
            if matches!(arg_hint.as_ref(), TypeKind::Record(..)) =>
        {
            let TypeKind::Record(fields) = arg_hint.as_ref() else {
                unreachable!("guarded by matches!")
            };
            let mut preds = Vec::new();
            let mut typed_kvs = BTreeMap::new();
            let expected: BTreeMap<_, _> =
                fields.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            for (k, v) in kvs {
                let expected_ty = expected
                    .get(k)
                    .ok_or_else(|| TypeError::UnknownField {
                        field: k.clone(),
                        typ: Type::record(fields.clone()).to_string(),
                    })?
                    .clone();
                let (p1, t1, typed_v) = infer_expr(unifier, supply, env, adts, known, v.as_ref())?;
                unifier.unify(&t1, &expected_ty)?;
                preds.extend(p1);
                typed_kvs.insert(k.clone(), typed_v);
            }
            for key in expected.keys() {
                if !typed_kvs.contains_key(key.as_ref()) {
                    return Err(TypeError::UnknownField {
                        field: key.clone(),
                        typ: Type::record(fields.clone()).to_string(),
                    });
                }
            }
            let record_ty = Type::record(
                fields
                    .iter()
                    .map(|(k, v)| (k.clone(), unifier.apply_type(v)))
                    .collect(),
            );
            let typed = TypedExpr::new(record_ty.clone(), TypedExprKind::Dict(typed_kvs));
            Ok((preds, record_ty, typed))
        }
        _ => infer_expr(unifier, supply, env, adts, known, arg),
    }
}

#[allow(clippy::too_many_arguments)]
fn infer_record_update_type_with_hint(
    unifier: &mut Unifier<'_>,
    supply: &mut TypeVarSupply,
    env: &TypeEnv,
    adts: &BTreeMap<Symbol, AdtDecl>,
    known: &KnownVariants,
    base: &Expr,
    updates: &BTreeMap<Symbol, Arc<Expr>>,
    hint_ty: &Type,
) -> Result<(Vec<Predicate>, Type), TypeError> {
    let (p_base, t_base) = infer_expr_type(unifier, supply, env, adts, known, base)?;
    unifier.unify(&t_base, hint_ty)?;
    let base_ty = unifier.apply_type(&t_base);
    let known_variant = known_variant_from_expr_with_known(base, &base_ty, adts, known);
    let update_fields: Vec<Symbol> = updates.keys().cloned().collect();
    let (result_ty, fields) = resolve_record_update(
        unifier,
        supply,
        adts,
        &base_ty,
        known_variant,
        &update_fields,
    )?;
    let expected: BTreeMap<_, _> = fields.into_iter().collect();

    let mut preds = p_base;
    for (k, v) in updates {
        let expected_ty = expected.get(k).ok_or_else(|| TypeError::UnknownField {
            field: k.clone(),
            typ: result_ty.to_string(),
        })?;
        let (p1, t1) = infer_expr_type(unifier, supply, env, adts, known, v.as_ref())?;
        unifier.unify(&t1, expected_ty)?;
        preds.extend(p1);
    }
    Ok((preds, result_ty))
}

#[allow(clippy::too_many_arguments)]
fn infer_record_update_typed_with_hint(
    unifier: &mut Unifier<'_>,
    supply: &mut TypeVarSupply,
    env: &TypeEnv,
    adts: &BTreeMap<Symbol, AdtDecl>,
    known: &KnownVariants,
    base: &Expr,
    updates: &BTreeMap<Symbol, Arc<Expr>>,
    hint_ty: &Type,
) -> Result<(Vec<Predicate>, Type, TypedExpr), TypeError> {
    let (p_base, t_base, typed_base) = infer_expr(unifier, supply, env, adts, known, base)?;
    unifier.unify(&t_base, hint_ty)?;
    let base_ty = unifier.apply_type(&t_base);
    let known_variant = known_variant_from_expr_with_known(base, &base_ty, adts, known);
    let update_fields: Vec<Symbol> = updates.keys().cloned().collect();
    let (result_ty, fields) = resolve_record_update(
        unifier,
        supply,
        adts,
        &base_ty,
        known_variant,
        &update_fields,
    )?;
    let expected: BTreeMap<_, _> = fields.into_iter().collect();

    let mut preds = p_base;
    let mut typed_updates = BTreeMap::new();
    for (k, v) in updates {
        let expected_ty = expected.get(k).ok_or_else(|| TypeError::UnknownField {
            field: k.clone(),
            typ: result_ty.to_string(),
        })?;
        let (p1, t1, typed_v) = infer_expr(unifier, supply, env, adts, known, v.as_ref())?;
        unifier.unify(&t1, expected_ty)?;
        preds.extend(p1);
        typed_updates.insert(k.clone(), typed_v);
    }

    let typed = TypedExpr::new(
        result_ty.clone(),
        TypedExprKind::RecordUpdate {
            base: Box::new(typed_base),
            updates: typed_updates,
        },
    );
    Ok((preds, result_ty, typed))
}

fn infer_expr_type(
    unifier: &mut Unifier<'_>,
    supply: &mut TypeVarSupply,
    env: &TypeEnv,
    adts: &BTreeMap<Symbol, AdtDecl>,
    known: &KnownVariants,
    expr: &Expr,
) -> Result<(Vec<Predicate>, Type), TypeError> {
    let span = *expr.span();
    let res = unifier.with_infer_depth(span, |unifier| {
        infer_expr_type_inner(unifier, supply, env, adts, known, expr)
    });
    res.map_err(|err| err.with_span(&span))
}

fn infer_expr_type_inner(
    unifier: &mut Unifier<'_>,
    supply: &mut TypeVarSupply,
    env: &TypeEnv,
    adts: &BTreeMap<Symbol, AdtDecl>,
    known: &KnownVariants,
    expr: &Expr,
) -> Result<(Vec<Predicate>, Type), TypeError> {
    unifier.charge_infer_node()?;
    match expr {
        Expr::Bool(_, _) => Ok((vec![], Type::builtin(BuiltinTypeId::Bool))),
        Expr::Uint(_, _) => {
            let lit_ty = Type::var(supply.fresh(Some(sym("n"))));
            Ok((vec![Predicate::new("Integral", lit_ty.clone())], lit_ty))
        }
        Expr::Int(_, _) => {
            let lit_ty = Type::var(supply.fresh(Some(sym("n"))));
            Ok((
                vec![
                    Predicate::new("Integral", lit_ty.clone()),
                    Predicate::new("AdditiveGroup", lit_ty.clone()),
                ],
                lit_ty,
            ))
        }
        Expr::Float(_, _) => Ok((vec![], Type::builtin(BuiltinTypeId::F32))),
        Expr::String(_, _) => Ok((vec![], Type::builtin(BuiltinTypeId::String))),
        Expr::Uuid(_, _) => Ok((vec![], Type::builtin(BuiltinTypeId::Uuid))),
        Expr::DateTime(_, _) => Ok((vec![], Type::builtin(BuiltinTypeId::DateTime))),
        Expr::Hole(_) => {
            let t = Type::var(supply.fresh(Some(sym("hole"))));
            Ok((vec![], t))
        }
        Expr::Var(var) => {
            let schemes = env
                .lookup(&var.name)
                .ok_or_else(|| TypeError::UnknownVar(var.name.clone()))?;
            if schemes.len() == 1 {
                let scheme = apply_scheme_with_unifier(&schemes[0], unifier);
                let (preds, t) = instantiate(&scheme, supply);
                Ok((preds, t))
            } else {
                for scheme in schemes {
                    if !scheme.vars.is_empty() || !scheme.preds.is_empty() {
                        return Err(TypeError::AmbiguousOverload(var.name.clone()));
                    }
                }
                let t = Type::var(supply.fresh(Some(var.name.clone())));
                Ok((vec![], t))
            }
        }
        Expr::Lam(..) => {
            let (params, constraints, body) = collect_lambda_chain(expr);
            let mut ann_vars = BTreeMap::new();
            let mut param_tys = Vec::with_capacity(params.len());
            for (name, ann) in &params {
                let param_ty = match ann {
                    Some(ann) => type_from_annotation_expr_vars(adts, ann, &mut ann_vars, supply)?,
                    None => Type::var(supply.fresh(Some(name.clone()))),
                };
                param_tys.push((name.clone(), param_ty));
            }

            let mut env1 = env.clone();
            let mut known_body = known.clone();
            for (name, param_ty) in &param_tys {
                env1.extend(name.clone(), Scheme::new(vec![], vec![], param_ty.clone()));
                known_body.remove(name);
            }

            let (mut preds, body_ty) =
                infer_expr_type(unifier, supply, &env1, adts, &known_body, body)?;
            let constraint_preds =
                predicates_from_constraints(adts, &constraints, &mut ann_vars, supply)?;
            preds.extend(constraint_preds);

            let mut fun_ty = unifier.apply_type(&body_ty);
            for (_, param_ty) in param_tys.iter().rev() {
                fun_ty = Type::fun(unifier.apply_type(param_ty), fun_ty);
            }
            Ok((preds, fun_ty))
        }
        Expr::App(..) => {
            let (head, args) = collect_app_chain(expr);
            let (mut preds, mut func_ty) =
                infer_expr_type(unifier, supply, env, adts, known, head)?;
            let mut overload_name = None;
            let mut overload_candidates = if let Expr::Var(var) = head {
                if let Some(schemes) = env.lookup(&var.name) {
                    if schemes.len() <= 1 {
                        None
                    } else {
                        let mut candidates = Vec::new();
                        for scheme in schemes {
                            if !scheme.vars.is_empty() || !scheme.preds.is_empty() {
                                return Err(TypeError::AmbiguousOverload(var.name.clone()));
                            }
                            let scheme = apply_scheme_with_unifier(scheme, unifier);
                            let (p, typ) = instantiate(&scheme, supply);
                            if !p.is_empty() {
                                return Err(TypeError::AmbiguousOverload(var.name.clone()));
                            }
                            candidates.push(typ);
                        }
                        overload_name = Some(var.name.clone());
                        Some(candidates)
                    }
                } else {
                    None
                }
            } else {
                None
            };
            for arg in args {
                let arg_hint = match unifier.apply_type(&func_ty).as_ref() {
                    TypeKind::Fun(arg, _) => Some(arg.clone()),
                    _ => None,
                };
                let (p_arg, arg_ty) =
                    infer_app_arg_type(unifier, supply, env, adts, known, arg_hint, arg)?;
                let arg_ty = unifier.apply_type(&arg_ty);
                if let Some(candidates) = overload_candidates.take() {
                    let candidates = candidates
                        .into_iter()
                        .map(|t| unifier.apply_type(&t))
                        .collect::<Vec<_>>();
                    let narrowed = narrow_overload_candidates(&candidates, &arg_ty);
                    if narrowed.is_empty()
                        && let Some(name) = &overload_name
                    {
                        return Err(TypeError::AmbiguousOverload(name.clone()));
                    }
                    overload_candidates = Some(narrowed);
                }
                let res_ty = match overload_candidates.as_ref() {
                    Some(candidates) if candidates.len() == 1 => candidates[0].clone(),
                    _ => Type::var(supply.fresh(Some("r".into()))),
                };
                unifier.unify(&func_ty, &Type::fun(arg_ty, res_ty.clone()))?;
                preds.extend(p_arg);
                func_ty = match overload_candidates.as_ref() {
                    Some(candidates) if candidates.len() == 1 => unifier.apply_type(&candidates[0]),
                    _ => unifier.apply_type(&res_ty),
                };
            }
            Ok((preds, func_ty))
        }
        Expr::Project(_, base, field) => {
            let (p1, t1) = infer_expr_type(unifier, supply, env, adts, known, base)?;
            let base_ty = unifier.apply_type(&t1);
            let known_variant = known_variant_from_expr_with_known(base, &base_ty, adts, known);
            let field_ty =
                resolve_projection(unifier, supply, adts, &base_ty, known_variant, field)?;
            Ok((p1, field_ty))
        }
        Expr::RecordUpdate(_, base, updates) => {
            let (p_base, t_base) = infer_expr_type(unifier, supply, env, adts, known, base)?;
            let base_ty = unifier.apply_type(&t_base);
            let known_variant = known_variant_from_expr_with_known(base, &base_ty, adts, known);
            let update_fields: Vec<Symbol> = updates.keys().cloned().collect();
            let (result_ty, fields) = resolve_record_update(
                unifier,
                supply,
                adts,
                &base_ty,
                known_variant,
                &update_fields,
            )?;
            let expected: BTreeMap<_, _> = fields.into_iter().collect();

            let mut preds = p_base;
            for (k, v) in updates {
                let expected_ty = expected.get(k).ok_or_else(|| TypeError::UnknownField {
                    field: k.clone(),
                    typ: result_ty.to_string(),
                })?;
                let (p1, t1) = infer_expr_type(unifier, supply, env, adts, known, v.as_ref())?;
                unifier.unify(&t1, expected_ty)?;
                preds.extend(p1);
            }
            Ok((preds, result_ty))
        }
        Expr::Let(..) => {
            let mut bindings = Vec::new();
            let mut cur = expr;
            while let Expr::Let(_, v, ann, d, b) = cur {
                bindings.push((v.clone(), ann.clone(), d.clone()));
                cur = b.as_ref();
            }

            let mut env_cur = env.clone();
            let mut known_cur = known.clone();
            for (v, ann, d) in bindings {
                let (p1, t1) = if let Some(ref ann_expr) = ann {
                    let mut ann_vars = BTreeMap::new();
                    let ann_ty =
                        type_from_annotation_expr_vars(adts, ann_expr, &mut ann_vars, supply)?;
                    match d.as_ref() {
                        Expr::RecordUpdate(_, base, updates) => infer_record_update_type_with_hint(
                            unifier,
                            supply,
                            &env_cur,
                            adts,
                            &known_cur,
                            base.as_ref(),
                            updates,
                            &ann_ty,
                        )?,
                        _ => {
                            let (p1, t1) =
                                infer_expr_type(unifier, supply, &env_cur, adts, &known_cur, &d)?;
                            unifier.unify(&t1, &ann_ty)?;
                            (p1, t1)
                        }
                    }
                } else {
                    infer_expr_type(unifier, supply, &env_cur, adts, &known_cur, &d)?
                };
                let def_ty = unifier.apply_type(&t1);
                let scheme = if ann.is_none() && is_integral_literal_expr(&d) {
                    monomorphic_scheme_with_unifier(p1, def_ty.clone(), unifier)
                } else {
                    let scheme = generalize_with_unifier(&env_cur, p1, def_ty.clone(), unifier);
                    reject_ambiguous_scheme(&scheme)?;
                    scheme
                };
                env_cur.extend(v.name.clone(), scheme);
                if let Some(known_variant) =
                    known_variant_from_expr_with_known(&d, &def_ty, adts, &known_cur)
                {
                    known_cur.insert(
                        v.name.clone(),
                        KnownVariant {
                            adt: known_variant.adt,
                            variant: known_variant.variant,
                        },
                    );
                } else {
                    known_cur.remove(&v.name);
                }
            }

            let (p_body, t_body) =
                infer_expr_type(unifier, supply, &env_cur, adts, &known_cur, cur)?;
            Ok((p_body, t_body))
        }
        Expr::LetRec(_, bindings, body) => {
            let mut env_seed = env.clone();
            let mut known_seed = known.clone();
            let mut binding_tys = BTreeMap::new();
            for (var, _ann, _def) in bindings {
                let tv = Type::var(supply.fresh(Some(var.name.clone())));
                env_seed.extend(var.name.clone(), Scheme::new(vec![], vec![], tv.clone()));
                known_seed.remove(&var.name);
                binding_tys.insert(var.name.clone(), tv);
            }

            let mut inferred = Vec::with_capacity(bindings.len());
            for (var, ann, def) in bindings {
                let (preds, def_ty) =
                    infer_expr_type(unifier, supply, &env_seed, adts, &known_seed, def)?;
                if let Some(ann) = ann {
                    let mut ann_vars = BTreeMap::new();
                    let ann_ty = type_from_annotation_expr_vars(adts, ann, &mut ann_vars, supply)?;
                    unifier.unify(&def_ty, &ann_ty)?;
                }
                let binding_ty = binding_tys
                    .get(&var.name)
                    .cloned()
                    .ok_or_else(|| TypeError::UnknownVar(var.name.clone()))?;
                unifier.unify(&binding_ty, &def_ty)?;
                let resolved_ty = unifier.apply_type(&binding_ty);

                if let Some(known_variant) =
                    known_variant_from_expr_with_known(def, &resolved_ty, adts, &known_seed)
                {
                    known_seed.insert(
                        var.name.clone(),
                        KnownVariant {
                            adt: known_variant.adt,
                            variant: known_variant.variant,
                        },
                    );
                } else {
                    known_seed.remove(&var.name);
                }
                inferred.push((var.name.clone(), preds, resolved_ty));
            }

            let mut env_body = env.clone();
            for (name, preds, def_ty) in inferred {
                let scheme = generalize_with_unifier(&env_body, preds, def_ty, unifier);
                reject_ambiguous_scheme(&scheme)?;
                env_body.extend(name, scheme);
            }

            let (p_body, t_body) =
                infer_expr_type(unifier, supply, &env_body, adts, &known_seed, body)?;
            Ok((p_body, t_body))
        }
        Expr::Ite(_, cond, then_expr, else_expr) => {
            let (p1, t1) = infer_expr_type(unifier, supply, env, adts, known, cond)?;
            unifier.unify(&t1, &Type::builtin(BuiltinTypeId::Bool))?;
            let (p2, t2) = infer_expr_type(unifier, supply, env, adts, known, then_expr)?;
            let (p3, t3) = infer_expr_type(unifier, supply, env, adts, known, else_expr)?;
            unifier.unify(&t2, &t3)?;
            let out_ty = unifier.apply_type(&t2);
            let mut preds = p1;
            preds.extend(p2);
            preds.extend(p3);
            Ok((preds, out_ty))
        }
        Expr::Tuple(_, elems) => {
            let mut preds = Vec::new();
            let mut types = Vec::new();
            for elem in elems {
                let (p1, t1) = infer_expr_type(unifier, supply, env, adts, known, elem.as_ref())?;
                preds.extend(p1);
                types.push(unifier.apply_type(&t1));
            }
            let tuple_ty = Type::tuple(types);
            Ok((preds, tuple_ty))
        }
        Expr::List(_, elems) => {
            let elem_tv = Type::var(supply.fresh(Some("a".into())));
            let mut preds = Vec::new();
            for elem in elems {
                let (p1, t1) = infer_expr_type(unifier, supply, env, adts, known, elem.as_ref())?;
                unifier.unify(&t1, &elem_tv)?;
                preds.extend(p1);
            }
            let list_ty = Type::app(
                Type::builtin(BuiltinTypeId::List),
                unifier.apply_type(&elem_tv),
            );
            Ok((preds, list_ty))
        }
        Expr::Dict(_, kvs) => {
            let elem_tv = Type::var(supply.fresh(Some("v".into())));
            let mut preds = Vec::new();
            for v in kvs.values() {
                let (p1, t1) = infer_expr_type(unifier, supply, env, adts, known, v.as_ref())?;
                unifier.unify(&t1, &elem_tv)?;
                preds.extend(p1);
            }
            let dict_ty = Type::app(
                Type::builtin(BuiltinTypeId::Dict),
                unifier.apply_type(&elem_tv),
            );
            Ok((preds, dict_ty))
        }
        Expr::Match(_, scrutinee, arms) => {
            let (p1, t1) = infer_expr_type(unifier, supply, env, adts, known, scrutinee.as_ref())?;
            let mut preds = p1;
            let res_ty = Type::var(supply.fresh(Some("match".into())));
            let patterns: Vec<Pattern> = arms.iter().map(|(pat, _)| pat.clone()).collect();

            for (pat, expr) in arms {
                let scrutinee_ty = unifier.apply_type(&t1);
                let (p_pat, binds) = infer_pattern(unifier, supply, env, pat, &scrutinee_ty)?;
                preds.extend(p_pat);

                let mut env_arm = env.clone();
                for (name, ty) in binds {
                    env_arm.extend(name, Scheme::new(vec![], vec![], unifier.apply_type(&ty)));
                }
                let mut known_arm = known.clone();
                if let Expr::Var(var) = scrutinee.as_ref() {
                    match pat {
                        Pattern::Named(_, name, _) => {
                            let name_sym = name.to_dotted_symbol();
                            if let Some((adt, _variant)) = ctor_lookup(adts, &name_sym) {
                                known_arm.insert(
                                    var.name.clone(),
                                    KnownVariant {
                                        adt: adt.name.clone(),
                                        variant: name_sym,
                                    },
                                );
                            } else {
                                known_arm.remove(&var.name);
                            }
                        }
                        _ => {
                            known_arm.remove(&var.name);
                        }
                    }
                }
                let (p_expr, t_expr) =
                    infer_expr_type(unifier, supply, &env_arm, adts, &known_arm, expr)?;
                unifier.unify(&res_ty, &t_expr)?;
                preds.extend(p_expr);
            }

            let scrutinee_ty = unifier.apply_type(&t1);
            check_match_exhaustive(adts, &scrutinee_ty, &patterns)?;
            let out_ty = unifier.apply_type(&res_ty);
            Ok((preds, out_ty))
        }
        Expr::Ann(_, expr, ann) => {
            let ann_ty = type_from_annotation_expr(adts, ann)?;
            match expr.as_ref() {
                Expr::RecordUpdate(_, base, updates) => {
                    let (preds, out_ty) = infer_record_update_type_with_hint(
                        unifier,
                        supply,
                        env,
                        adts,
                        known,
                        base.as_ref(),
                        updates,
                        &ann_ty,
                    )?;
                    Ok((preds, out_ty))
                }
                _ => {
                    let (preds, expr_ty) =
                        infer_expr_type(unifier, supply, env, adts, known, expr)?;
                    unifier.unify(&expr_ty, &ann_ty)?;
                    let out_ty = unifier.apply_type(&ann_ty);
                    Ok((preds, out_ty))
                }
            }
        }
    }
}

fn infer_expr(
    unifier: &mut Unifier<'_>,
    supply: &mut TypeVarSupply,
    env: &TypeEnv,
    adts: &BTreeMap<Symbol, AdtDecl>,
    known: &KnownVariants,
    expr: &Expr,
) -> Result<(Vec<Predicate>, Type, TypedExpr), TypeError> {
    let span = *expr.span();
    let res = unifier.with_infer_depth(span, |unifier| {
        (|| {
            unifier.charge_infer_node()?;
            match expr {
                Expr::Bool(_, v) => {
                    let t = Type::builtin(BuiltinTypeId::Bool);
                    Ok((
                        vec![],
                        t.clone(),
                        TypedExpr::new(t, TypedExprKind::Bool(*v)),
                    ))
                }
                Expr::Uint(_, v) => {
                    let t = Type::var(supply.fresh(Some(sym("n"))));
                    Ok((
                        vec![Predicate::new("Integral", t.clone())],
                        t.clone(),
                        TypedExpr::new(t, TypedExprKind::Uint(*v)),
                    ))
                }
                Expr::Int(_, v) => {
                    let t = Type::var(supply.fresh(Some(sym("n"))));
                    Ok((
                        vec![
                            Predicate::new("Integral", t.clone()),
                            Predicate::new("AdditiveGroup", t.clone()),
                        ],
                        t.clone(),
                        TypedExpr::new(t, TypedExprKind::Int(*v)),
                    ))
                }
                Expr::Float(_, v) => {
                    let t = Type::builtin(BuiltinTypeId::F32);
                    Ok((
                        vec![],
                        t.clone(),
                        TypedExpr::new(t, TypedExprKind::Float(*v)),
                    ))
                }
                Expr::String(_, v) => {
                    let t = Type::builtin(BuiltinTypeId::String);
                    Ok((
                        vec![],
                        t.clone(),
                        TypedExpr::new(t, TypedExprKind::String(v.clone())),
                    ))
                }
                Expr::Uuid(_, v) => {
                    let t = Type::builtin(BuiltinTypeId::Uuid);
                    Ok((
                        vec![],
                        t.clone(),
                        TypedExpr::new(t, TypedExprKind::Uuid(*v)),
                    ))
                }
                Expr::DateTime(_, v) => {
                    let t = Type::builtin(BuiltinTypeId::DateTime);
                    Ok((
                        vec![],
                        t.clone(),
                        TypedExpr::new(t, TypedExprKind::DateTime(*v)),
                    ))
                }
                Expr::Hole(_) => {
                    let t = Type::var(supply.fresh(Some(sym("hole"))));
                    Ok((vec![], t.clone(), TypedExpr::new(t, TypedExprKind::Hole)))
                }
                Expr::Var(var) => {
                    let schemes = env
                        .lookup(&var.name)
                        .ok_or_else(|| TypeError::UnknownVar(var.name.clone()))?;
                    if schemes.len() == 1 {
                        let scheme = apply_scheme_with_unifier(&schemes[0], unifier);
                        let (preds, t) = instantiate(&scheme, supply);
                        let typed = TypedExpr::new(
                            t.clone(),
                            TypedExprKind::Var {
                                name: var.name.clone(),
                                overloads: vec![],
                            },
                        );
                        Ok((preds, t, typed))
                    } else {
                        let mut overloads = Vec::new();
                        for scheme in schemes {
                            if !scheme.preds.is_empty() {
                                return Err(TypeError::AmbiguousOverload(var.name.clone()));
                            }

                            let scheme = apply_scheme_with_unifier(scheme, unifier);
                            let (preds, typ) = instantiate(&scheme, supply);
                            if !preds.is_empty() {
                                return Err(TypeError::AmbiguousOverload(var.name.clone()));
                            }
                            overloads.push(typ);
                        }
                        let t = Type::var(supply.fresh(Some(var.name.clone())));
                        let typed = TypedExpr::new(
                            t.clone(),
                            TypedExprKind::Var {
                                name: var.name.clone(),
                                overloads,
                            },
                        );
                        Ok((vec![], t, typed))
                    }
                }
                Expr::Lam(..) => {
                    let (params, constraints, body) = collect_lambda_chain(expr);
                    let mut ann_vars = BTreeMap::new();
                    let mut param_tys = Vec::with_capacity(params.len());
                    for (name, ann) in &params {
                        let param_ty = match ann {
                            Some(ann) => {
                                type_from_annotation_expr_vars(adts, ann, &mut ann_vars, supply)?
                            }
                            None => Type::var(supply.fresh(Some(name.clone()))),
                        };
                        param_tys.push((name.clone(), param_ty));
                    }

                    let mut env1 = env.clone();
                    let mut known_body = known.clone();
                    for (name, param_ty) in &param_tys {
                        env1.extend(name.clone(), Scheme::new(vec![], vec![], param_ty.clone()));
                        known_body.remove(name);
                    }

                    let (mut preds, body_ty, typed_body) =
                        infer_expr(unifier, supply, &env1, adts, &known_body, body)?;
                    let constraint_preds =
                        predicates_from_constraints(adts, &constraints, &mut ann_vars, supply)?;
                    preds.extend(constraint_preds);

                    let mut typed = typed_body;
                    let mut fun_ty = unifier.apply_type(&body_ty);
                    for (name, param_ty) in param_tys.iter().rev() {
                        fun_ty = Type::fun(unifier.apply_type(param_ty), fun_ty);
                        typed = TypedExpr::new(
                            fun_ty.clone(),
                            TypedExprKind::Lam {
                                param: name.clone(),
                                body: Box::new(typed),
                            },
                        );
                    }

                    Ok((preds, fun_ty, typed))
                }
                Expr::App(..) => {
                    let (head, args) = collect_app_chain(expr);
                    let (mut preds, mut func_ty, mut typed) =
                        infer_expr(unifier, supply, env, adts, known, head)?;
                    let mut overload_name = None;
                    let mut overload_candidates = match &typed.kind {
                        TypedExprKind::Var { name, overloads } if !overloads.is_empty() => {
                            overload_name = Some(name.clone());
                            Some(overloads.clone())
                        }
                        _ => None,
                    };
                    for arg in args {
                        let expected_arg = match unifier.apply_type(&func_ty).as_ref() {
                            TypeKind::Fun(arg, _) => Some(arg.clone()),
                            _ => None,
                        };
                        let arg_hint = match unifier.apply_type(&func_ty).as_ref() {
                            TypeKind::Fun(arg, _) => Some(arg.clone()),
                            _ => None,
                        };
                        let (p_arg, arg_ty, typed_arg) =
                            infer_app_arg_typed(unifier, supply, env, adts, known, arg_hint, arg)?;
                        let mut arg_ty = unifier.apply_type(&arg_ty);
                        let mut typed_arg = typed_arg;

                        if let Some(expected_arg) = expected_arg {
                            let expected_arg = unifier.apply_type(&expected_arg);
                            if let (Some(expected_elem), Some(arg_elem)) = (
                                unary_app_arg(&expected_arg, "Array"),
                                unary_app_arg(&arg_ty, "List"),
                            ) {
                                unifier.unify(&expected_elem, &arg_elem)?;
                                let elem_ty = unifier.apply_type(&expected_elem);
                                let list_ty = Type::list(elem_ty.clone());
                                let array_ty = Type::array(elem_ty);
                                let coercion_ty = Type::fun(list_ty, array_ty.clone());
                                let coercion_fn = TypedExpr::new(
                                    coercion_ty,
                                    TypedExprKind::Var {
                                        name: sym("prim_array_from_list"),
                                        overloads: vec![],
                                    },
                                );
                                typed_arg = TypedExpr::new(
                                    array_ty.clone(),
                                    TypedExprKind::App(Box::new(coercion_fn), Box::new(typed_arg)),
                                );
                                arg_ty = array_ty;
                            }
                        }
                        if let Some(candidates) = overload_candidates.take() {
                            let candidates = candidates
                                .into_iter()
                                .map(|t| unifier.apply_type(&t))
                                .collect::<Vec<_>>();
                            let narrowed = narrow_overload_candidates(&candidates, &arg_ty);
                            if narrowed.is_empty()
                                && let Some(name) = &overload_name
                            {
                                return Err(TypeError::AmbiguousOverload(name.clone()));
                            }
                            overload_candidates = Some(narrowed);
                        }
                        let res_ty = match overload_candidates.as_ref() {
                            Some(candidates) if candidates.len() == 1 => candidates[0].clone(),
                            _ => Type::var(supply.fresh(Some("r".into()))),
                        };
                        unifier.unify(&func_ty, &Type::fun(arg_ty, res_ty.clone()))?;
                        let result_ty = match overload_candidates.as_ref() {
                            Some(candidates) if candidates.len() == 1 => {
                                unifier.apply_type(&candidates[0])
                            }
                            _ => unifier.apply_type(&res_ty),
                        };
                        preds.extend(p_arg);
                        typed = TypedExpr::new(
                            result_ty.clone(),
                            TypedExprKind::App(Box::new(typed), Box::new(typed_arg)),
                        );
                        func_ty = result_ty;
                    }
                    Ok((preds, func_ty, typed))
                }
                Expr::Project(_, base, field) => {
                    let (p1, t1, typed_base) = infer_expr(unifier, supply, env, adts, known, base)?;
                    let base_ty = unifier.apply_type(&t1);
                    let known_variant =
                        known_variant_from_expr_with_known(base, &base_ty, adts, known);
                    let field_ty =
                        resolve_projection(unifier, supply, adts, &base_ty, known_variant, field)?;
                    let typed = TypedExpr::new(
                        field_ty.clone(),
                        TypedExprKind::Project {
                            expr: Box::new(typed_base),
                            field: field.clone(),
                        },
                    );
                    Ok((p1, field_ty, typed))
                }
                Expr::RecordUpdate(_, base, updates) => {
                    let (p_base, t_base, typed_base) =
                        infer_expr(unifier, supply, env, adts, known, base)?;
                    let base_ty = unifier.apply_type(&t_base);
                    let known_variant =
                        known_variant_from_expr_with_known(base, &base_ty, adts, known);
                    let update_fields: Vec<Symbol> = updates.keys().cloned().collect();
                    let (result_ty, fields) = resolve_record_update(
                        unifier,
                        supply,
                        adts,
                        &base_ty,
                        known_variant,
                        &update_fields,
                    )?;
                    let expected: BTreeMap<_, _> = fields.into_iter().collect();

                    let mut preds = p_base;
                    let mut typed_updates = BTreeMap::new();
                    for (k, v) in updates {
                        let expected_ty =
                            expected.get(k).ok_or_else(|| TypeError::UnknownField {
                                field: k.clone(),
                                typ: result_ty.to_string(),
                            })?;
                        let (p1, t1, typed_v) =
                            infer_expr(unifier, supply, env, adts, known, v.as_ref())?;
                        unifier.unify(&t1, expected_ty)?;
                        preds.extend(p1);
                        typed_updates.insert(k.clone(), typed_v);
                    }
                    let typed = TypedExpr::new(
                        result_ty.clone(),
                        TypedExprKind::RecordUpdate {
                            base: Box::new(typed_base),
                            updates: typed_updates,
                        },
                    );
                    Ok((preds, result_ty, typed))
                }
                Expr::Let(..) => {
                    let mut bindings = Vec::new();
                    let mut cur = expr;
                    while let Expr::Let(_, v, ann, d, b) = cur {
                        bindings.push((v.clone(), ann.clone(), d.clone()));
                        cur = b.as_ref();
                    }

                    let mut env_cur = env.clone();
                    let mut known_cur = known.clone();
                    let mut typed_defs = Vec::new();
                    for (v, ann, d) in bindings {
                        let (p1, t1, typed_def) = if let Some(ref ann_expr) = ann {
                            let mut ann_vars = BTreeMap::new();
                            let ann_ty = type_from_annotation_expr_vars(
                                adts,
                                ann_expr,
                                &mut ann_vars,
                                supply,
                            )?;
                            match d.as_ref() {
                                Expr::RecordUpdate(_, base, updates) => {
                                    infer_record_update_typed_with_hint(
                                        unifier,
                                        supply,
                                        &env_cur,
                                        adts,
                                        &known_cur,
                                        base.as_ref(),
                                        updates,
                                        &ann_ty,
                                    )?
                                }
                                _ => {
                                    let (p1, t1, typed_def) = infer_expr(
                                        unifier, supply, &env_cur, adts, &known_cur, &d,
                                    )?;
                                    unifier.unify(&t1, &ann_ty)?;
                                    (p1, t1, typed_def)
                                }
                            }
                        } else {
                            infer_expr(unifier, supply, &env_cur, adts, &known_cur, &d)?
                        };
                        let def_ty = unifier.apply_type(&t1);
                        let scheme = if ann.is_none() && is_integral_literal_expr(&d) {
                            monomorphic_scheme_with_unifier(p1, def_ty.clone(), unifier)
                        } else {
                            let scheme =
                                generalize_with_unifier(&env_cur, p1, def_ty.clone(), unifier);
                            reject_ambiguous_scheme(&scheme)?;
                            scheme
                        };
                        env_cur.extend(v.name.clone(), scheme);
                        if let Some(known_variant) =
                            known_variant_from_expr_with_known(&d, &def_ty, adts, &known_cur)
                        {
                            known_cur.insert(
                                v.name.clone(),
                                KnownVariant {
                                    adt: known_variant.adt,
                                    variant: known_variant.variant,
                                },
                            );
                        } else {
                            known_cur.remove(&v.name);
                        }
                        typed_defs.push((v.name.clone(), typed_def));
                    }

                    let (p_body, t_body, typed_body) =
                        infer_expr(unifier, supply, &env_cur, adts, &known_cur, cur)?;

                    let mut typed = typed_body;
                    for (name, def) in typed_defs.into_iter().rev() {
                        typed = TypedExpr::new(
                            t_body.clone(),
                            TypedExprKind::Let {
                                name,
                                def: Box::new(def),
                                body: Box::new(typed),
                            },
                        );
                    }
                    Ok((p_body, t_body, typed))
                }
                Expr::LetRec(_, bindings, body) => {
                    let mut env_seed = env.clone();
                    let mut known_seed = known.clone();
                    let mut binding_tys = BTreeMap::new();
                    for (var, _ann, _def) in bindings {
                        let tv = Type::var(supply.fresh(Some(var.name.clone())));
                        env_seed.extend(var.name.clone(), Scheme::new(vec![], vec![], tv.clone()));
                        known_seed.remove(&var.name);
                        binding_tys.insert(var.name.clone(), tv);
                    }

                    let mut inferred_defs = Vec::with_capacity(bindings.len());
                    for (var, ann, def) in bindings {
                        let (preds, def_ty, typed_def) =
                            infer_expr(unifier, supply, &env_seed, adts, &known_seed, def)?;
                        if let Some(ann) = ann {
                            let mut ann_vars = BTreeMap::new();
                            let ann_ty =
                                type_from_annotation_expr_vars(adts, ann, &mut ann_vars, supply)?;
                            unifier.unify(&def_ty, &ann_ty)?;
                        }
                        let binding_ty = binding_tys
                            .get(&var.name)
                            .cloned()
                            .ok_or_else(|| TypeError::UnknownVar(var.name.clone()))?;
                        unifier.unify(&binding_ty, &def_ty)?;
                        let resolved_ty = unifier.apply_type(&binding_ty);

                        if let Some(known_variant) =
                            known_variant_from_expr_with_known(def, &resolved_ty, adts, &known_seed)
                        {
                            known_seed.insert(
                                var.name.clone(),
                                KnownVariant {
                                    adt: known_variant.adt,
                                    variant: known_variant.variant,
                                },
                            );
                        } else {
                            known_seed.remove(&var.name);
                        }
                        inferred_defs.push((var.name.clone(), preds, resolved_ty, typed_def));
                    }

                    let mut env_body = env.clone();
                    let mut typed_bindings = Vec::with_capacity(inferred_defs.len());
                    for (name, preds, def_ty, typed_def) in inferred_defs {
                        let scheme = generalize_with_unifier(&env_body, preds, def_ty, unifier);
                        reject_ambiguous_scheme(&scheme)?;
                        env_body.extend(name.clone(), scheme);
                        typed_bindings.push((name, typed_def));
                    }

                    let (p_body, t_body, typed_body) =
                        infer_expr(unifier, supply, &env_body, adts, &known_seed, body)?;
                    let typed = TypedExpr::new(
                        t_body.clone(),
                        TypedExprKind::LetRec {
                            bindings: typed_bindings,
                            body: Box::new(typed_body),
                        },
                    );
                    Ok((p_body, t_body, typed))
                }
                Expr::Ite(_, cond, then_expr, else_expr) => {
                    let (p1, t1, typed_cond) = infer_expr(unifier, supply, env, adts, known, cond)?;
                    unifier.unify(&t1, &Type::builtin(BuiltinTypeId::Bool))?;
                    let (p2, t2, typed_then) =
                        infer_expr(unifier, supply, env, adts, known, then_expr)?;
                    let (p3, t3, typed_else) =
                        infer_expr(unifier, supply, env, adts, known, else_expr)?;
                    unifier.unify(&t2, &t3)?;
                    let out_ty = unifier.apply_type(&t2);
                    let mut preds = p1;
                    preds.extend(p2);
                    preds.extend(p3);
                    let typed = TypedExpr::new(
                        out_ty.clone(),
                        TypedExprKind::Ite {
                            cond: Box::new(typed_cond),
                            then_expr: Box::new(typed_then),
                            else_expr: Box::new(typed_else),
                        },
                    );
                    Ok((preds, out_ty, typed))
                }
                Expr::Tuple(_, elems) => {
                    let mut preds = Vec::new();
                    let mut types = Vec::new();
                    let mut typed_elems = Vec::new();
                    for elem in elems {
                        let (p1, t1, typed_elem) =
                            infer_expr(unifier, supply, env, adts, known, elem)?;
                        preds.extend(p1);
                        types.push(unifier.apply_type(&t1));
                        typed_elems.push(typed_elem);
                    }
                    let tuple_ty = Type::tuple(types);
                    let typed = TypedExpr::new(tuple_ty.clone(), TypedExprKind::Tuple(typed_elems));
                    Ok((preds, tuple_ty, typed))
                }
                Expr::List(_, elems) => {
                    let elem_tv = Type::var(supply.fresh(Some("a".into())));
                    let mut preds = Vec::new();
                    let mut typed_elems = Vec::new();
                    for elem in elems {
                        let (p1, t1, typed_elem) =
                            infer_expr(unifier, supply, env, adts, known, elem)?;
                        unifier.unify(&t1, &elem_tv)?;
                        preds.extend(p1);
                        typed_elems.push(typed_elem);
                    }
                    let list_ty = Type::app(
                        Type::builtin(BuiltinTypeId::List),
                        unifier.apply_type(&elem_tv),
                    );
                    let typed = TypedExpr::new(list_ty.clone(), TypedExprKind::List(typed_elems));
                    Ok((preds, list_ty, typed))
                }
                Expr::Dict(_, kvs) => {
                    let elem_tv = Type::var(supply.fresh(Some("v".into())));
                    let mut preds = Vec::new();
                    let mut typed_kvs = BTreeMap::new();
                    for (k, v) in kvs {
                        let (p1, t1, typed_v) = infer_expr(unifier, supply, env, adts, known, v)?;
                        unifier.unify(&t1, &elem_tv)?;
                        preds.extend(p1);
                        typed_kvs.insert(k.clone(), typed_v);
                    }
                    let dict_ty = Type::app(
                        Type::builtin(BuiltinTypeId::Dict),
                        unifier.apply_type(&elem_tv),
                    );
                    let typed = TypedExpr::new(dict_ty.clone(), TypedExprKind::Dict(typed_kvs));
                    Ok((preds, dict_ty, typed))
                }
                Expr::Match(_, scrutinee, arms) => {
                    let (p1, t1, typed_scrutinee) =
                        infer_expr(unifier, supply, env, adts, known, scrutinee)?;
                    let mut preds = p1;
                    let mut typed_arms = Vec::new();
                    let res_ty = Type::var(supply.fresh(Some("match".into())));
                    let patterns: Vec<Pattern> = arms.iter().map(|(pat, _)| pat.clone()).collect();

                    for (pat, expr) in arms {
                        let scrutinee_ty = unifier.apply_type(&t1);
                        let (p_pat, binds) =
                            infer_pattern(unifier, supply, env, pat, &scrutinee_ty)?;
                        preds.extend(p_pat);

                        let mut env_arm = env.clone();
                        for (name, ty) in binds {
                            env_arm
                                .extend(name, Scheme::new(vec![], vec![], unifier.apply_type(&ty)));
                        }
                        let mut known_arm = known.clone();
                        if let Expr::Var(var) = scrutinee.as_ref() {
                            match pat {
                                Pattern::Named(_, name, _) => {
                                    let name_sym = name.to_dotted_symbol();
                                    if let Some((adt, _variant)) = ctor_lookup(adts, &name_sym) {
                                        known_arm.insert(
                                            var.name.clone(),
                                            KnownVariant {
                                                adt: adt.name.clone(),
                                                variant: name_sym,
                                            },
                                        );
                                    } else {
                                        known_arm.remove(&var.name);
                                    }
                                }
                                _ => {
                                    known_arm.remove(&var.name);
                                }
                            }
                        }
                        let (p_expr, t_expr, typed_expr) =
                            infer_expr(unifier, supply, &env_arm, adts, &known_arm, expr)?;
                        unifier.unify(&res_ty, &t_expr)?;
                        preds.extend(p_expr);
                        typed_arms.push((pat.clone(), typed_expr));
                    }

                    let scrutinee_ty = unifier.apply_type(&t1);
                    check_match_exhaustive(adts, &scrutinee_ty, &patterns)?;
                    let out_ty = unifier.apply_type(&res_ty);
                    let typed = TypedExpr::new(
                        out_ty.clone(),
                        TypedExprKind::Match {
                            scrutinee: Box::new(typed_scrutinee),
                            arms: typed_arms,
                        },
                    );
                    Ok((preds, out_ty, typed))
                }
                Expr::Ann(_, expr, ann) => {
                    let ann_ty = type_from_annotation_expr(adts, ann)?;
                    match expr.as_ref() {
                        Expr::RecordUpdate(_, base, updates) => {
                            infer_record_update_typed_with_hint(
                                unifier,
                                supply,
                                env,
                                adts,
                                known,
                                base.as_ref(),
                                updates,
                                &ann_ty,
                            )
                        }
                        _ => {
                            let (preds, expr_ty, typed_expr) =
                                infer_expr(unifier, supply, env, adts, known, expr)?;
                            unifier.unify(&expr_ty, &ann_ty)?;
                            let out_ty = unifier.apply_type(&ann_ty);
                            Ok((preds, out_ty, typed_expr))
                        }
                    }
                }
            }
        })()
    });
    res.map_err(|err| err.with_span(&span))
}

fn ctor_lookup<'a>(
    adts: &'a BTreeMap<Symbol, AdtDecl>,
    name: &Symbol,
) -> Option<(&'a AdtDecl, &'a AdtVariant)> {
    let mut found = None;
    for adt in adts.values() {
        if let Some(variant) = adt.variants.iter().find(|v| &v.name == name) {
            if found.is_some() {
                return None;
            }
            found = Some((adt, variant));
        }
    }
    found
}

fn record_fields(variant: &AdtVariant) -> Option<&[(Symbol, Type)]> {
    if variant.args.len() != 1 {
        return None;
    }
    match variant.args[0].as_ref() {
        TypeKind::Record(fields) => Some(fields),
        _ => None,
    }
}

fn instantiate_variant_fields(
    adt: &AdtDecl,
    variant: &AdtVariant,
    supply: &mut TypeVarSupply,
) -> Option<(Type, Vec<(Symbol, Type)>)> {
    let fields = record_fields(variant)?;
    let mut subst = Subst::new_sync();
    for param in &adt.params {
        let fresh = Type::var(supply.fresh(param.var.name.clone()));
        subst = subst.insert(param.var.id, fresh);
    }
    let result_ty = adt.result_type().apply(&subst);
    let fields = fields
        .iter()
        .map(|(name, ty)| (name.clone(), ty.apply(&subst)))
        .collect();
    Some((result_ty, fields))
}

fn known_variant_from_expr(
    expr: &Expr,
    expr_ty: &Type,
    adts: &BTreeMap<Symbol, AdtDecl>,
) -> Option<KnownVariant> {
    let mut expr = expr;
    while let Expr::Ann(_, inner, _) = expr {
        expr = inner.as_ref();
    }
    if matches!(expr_ty.as_ref(), TypeKind::Fun(..)) {
        return None;
    }
    let ctor = match expr {
        Expr::App(_, f, _) => match f.as_ref() {
            Expr::Var(var) => var.name.clone(),
            _ => return None,
        },
        _ => return None,
    };
    let (adt, variant) = ctor_lookup(adts, &ctor)?;
    record_fields(variant)?;
    Some(KnownVariant {
        adt: adt.name.clone(),
        variant: variant.name.clone(),
    })
}

fn known_variant_from_expr_with_known(
    expr: &Expr,
    expr_ty: &Type,
    adts: &BTreeMap<Symbol, AdtDecl>,
    known: &KnownVariants,
) -> Option<KnownVariant> {
    let mut expr = expr;
    while let Expr::Ann(_, inner, _) = expr {
        expr = inner.as_ref();
    }
    match expr {
        Expr::Var(var) => known.get(&var.name).cloned(),
        Expr::RecordUpdate(_, base, _) => {
            known_variant_from_expr_with_known(base.as_ref(), expr_ty, adts, known)
        }
        _ => known_variant_from_expr(expr, expr_ty, adts),
    }
}

fn select_record_variant<'a, F>(
    adts: &'a BTreeMap<Symbol, AdtDecl>,
    base_ty: &Type,
    known_variant: Option<KnownVariant>,
    field_for_errors: &Symbol,
    matches_fields: F,
) -> Result<(&'a AdtDecl, &'a AdtVariant), TypeError>
where
    F: Fn(&[(Symbol, Type)]) -> bool,
{
    if let Some(info) = known_variant {
        let adt = adts
            .get(&info.adt)
            .ok_or_else(|| TypeError::UnknownTypeName(info.adt.clone()))?;
        let variant = adt
            .variants
            .iter()
            .find(|v| v.name == info.variant)
            .ok_or_else(|| TypeError::UnknownField {
                field: field_for_errors.clone(),
                typ: base_ty.to_string(),
            })?;
        return Ok((adt, variant));
    }

    if let Some(adt_name) = type_head_name(base_ty) {
        let adt = adts.get(adt_name).ok_or_else(|| TypeError::UnknownField {
            field: field_for_errors.clone(),
            typ: base_ty.to_string(),
        })?;
        if adt.variants.len() == 1 {
            return Ok((adt, &adt.variants[0]));
        }
        return Err(TypeError::FieldNotKnown {
            field: field_for_errors.clone(),
            typ: base_ty.to_string(),
        });
    }

    if matches!(base_ty.as_ref(), TypeKind::Var(_)) {
        let mut candidates = Vec::new();
        for adt in adts.values() {
            if adt.variants.len() != 1 {
                continue;
            }
            let variant = &adt.variants[0];
            let Some(fields) = record_fields(variant) else {
                continue;
            };
            if matches_fields(fields) {
                candidates.push((adt, variant));
            }
        }
        if candidates.len() == 1 {
            return Ok(candidates.remove(0));
        }
        if candidates.is_empty() {
            return Err(TypeError::UnknownField {
                field: field_for_errors.clone(),
                typ: base_ty.to_string(),
            });
        }
        return Err(TypeError::FieldNotKnown {
            field: field_for_errors.clone(),
            typ: base_ty.to_string(),
        });
    }

    Err(TypeError::UnknownField {
        field: field_for_errors.clone(),
        typ: base_ty.to_string(),
    })
}

fn resolve_record_update(
    unifier: &mut Unifier<'_>,
    supply: &mut TypeVarSupply,
    adts: &BTreeMap<Symbol, AdtDecl>,
    base_ty: &Type,
    known_variant: Option<KnownVariant>,
    update_fields: &[Symbol],
) -> Result<(Type, Vec<(Symbol, Type)>), TypeError> {
    if let TypeKind::Record(fields) = base_ty.as_ref() {
        return Ok((base_ty.clone(), fields.clone()));
    }

    let field_for_errors = update_fields.first().cloned().unwrap_or_else(|| sym("_"));

    let (adt, variant) =
        select_record_variant(adts, base_ty, known_variant, &field_for_errors, |fields| {
            update_fields
                .iter()
                .all(|field| fields.iter().any(|(name, _)| name == field))
        })?;

    let (result_ty, fields) =
        instantiate_variant_fields(adt, variant, supply).ok_or_else(|| {
            TypeError::UnknownField {
                field: field_for_errors.clone(),
                typ: base_ty.to_string(),
            }
        })?;

    for field in update_fields {
        if fields.iter().all(|(name, _)| name != field) {
            return Err(TypeError::UnknownField {
                field: field.clone(),
                typ: base_ty.to_string(),
            });
        }
    }

    unifier.unify(base_ty, &result_ty)?;
    let result_ty = unifier.apply_type(&result_ty);
    let fields = fields
        .into_iter()
        .map(|(name, ty)| (name, unifier.apply_type(&ty)))
        .collect();
    Ok((result_ty, fields))
}

fn resolve_projection(
    unifier: &mut Unifier<'_>,
    supply: &mut TypeVarSupply,
    adts: &BTreeMap<Symbol, AdtDecl>,
    base_ty: &Type,
    known_variant: Option<KnownVariant>,
    field: &Symbol,
) -> Result<Type, TypeError> {
    if let Ok(index) = field.as_ref().parse::<usize>() {
        let elem_ty = match base_ty.as_ref() {
            TypeKind::Tuple(elems) => {
                elems
                    .get(index)
                    .cloned()
                    .ok_or_else(|| TypeError::UnknownField {
                        field: field.clone(),
                        typ: base_ty.to_string(),
                    })?
            }
            TypeKind::Var(_) => {
                let mut elems = Vec::with_capacity(index + 1);
                for _ in 0..=index {
                    elems.push(Type::var(supply.fresh(Some(sym("t")))));
                }
                let tuple_ty = Type::tuple(elems.clone());
                unifier.unify(base_ty, &tuple_ty)?;
                elems[index].clone()
            }
            _ => {
                return Err(TypeError::UnknownField {
                    field: field.clone(),
                    typ: base_ty.to_string(),
                });
            }
        };
        return Ok(unifier.apply_type(&elem_ty));
    }

    let (adt, variant) = select_record_variant(adts, base_ty, known_variant, field, |fields| {
        fields.iter().any(|(name, _)| name == field)
    })?;

    let (result_ty, fields) =
        instantiate_variant_fields(adt, variant, supply).ok_or_else(|| {
            TypeError::UnknownField {
                field: field.clone(),
                typ: base_ty.to_string(),
            }
        })?;
    let field_ty = fields
        .iter()
        .find(|(name, _)| name == field)
        .map(|(_, ty)| ty.clone())
        .ok_or_else(|| TypeError::UnknownField {
            field: field.clone(),
            typ: base_ty.to_string(),
        })?;
    unifier.unify(base_ty, &result_ty)?;
    Ok(unifier.apply_type(&field_ty))
}

fn decompose_fun(typ: &Type, arity: usize) -> Option<(Vec<Type>, Type)> {
    let mut args = Vec::with_capacity(arity);
    let mut cur = typ.clone();
    for _ in 0..arity {
        match cur.as_ref() {
            TypeKind::Fun(a, b) => {
                args.push(a.clone());
                cur = b.clone();
            }
            _ => return None,
        }
    }
    Some((args, cur))
}

type InferPatternResult = (Vec<Predicate>, Vec<(Symbol, Type)>);

fn infer_pattern(
    unifier: &mut Unifier<'_>,
    supply: &mut TypeVarSupply,
    env: &TypeEnv,
    pat: &Pattern,
    scrutinee_ty: &Type,
) -> Result<InferPatternResult, TypeError> {
    let span = *pat.span();
    let res = (|| {
        unifier.charge_infer_node()?;
        match pat {
            Pattern::Wildcard(..) => Ok((vec![], vec![])),
            Pattern::Var(var) => Ok((
                vec![],
                vec![(var.name.clone(), unifier.apply_type(scrutinee_ty))],
            )),
            Pattern::Named(_, name, ps) => {
                let ctor_name = name.to_dotted_symbol();
                let schemes = env
                    .lookup(&ctor_name)
                    .ok_or_else(|| TypeError::UnknownVar(ctor_name.clone()))?;
                if schemes.len() != 1 {
                    return Err(TypeError::AmbiguousOverload(ctor_name));
                }
                let scheme = apply_scheme_with_unifier(&schemes[0], unifier);
                let (preds, ctor_ty) = instantiate(&scheme, supply);
                let (arg_tys, res_ty) = decompose_fun(&ctor_ty, ps.len())
                    .ok_or(TypeError::UnsupportedExpr("pattern constructor"))?;
                unifier.unify(&res_ty, scrutinee_ty)?;
                let mut all_preds = preds;
                let mut bindings = Vec::new();
                for (p, arg_ty) in ps.iter().zip(arg_tys.iter()) {
                    let arg_ty = unifier.apply_type(arg_ty);
                    let (p1, binds1) = infer_pattern(unifier, supply, env, p, &arg_ty)?;
                    all_preds.extend(p1);
                    bindings.extend(binds1);
                }
                let bindings = bindings
                    .into_iter()
                    .map(|(name, ty)| (name, unifier.apply_type(&ty)))
                    .collect();
                Ok((all_preds, bindings))
            }
            Pattern::List(_, ps) => {
                let elem_tv = Type::var(supply.fresh(Some("a".into())));
                let list_ty = Type::app(Type::builtin(BuiltinTypeId::List), elem_tv.clone());
                unifier.unify(scrutinee_ty, &list_ty)?;
                let mut preds = Vec::new();
                let mut bindings = Vec::new();
                for p in ps {
                    let elem_ty = unifier.apply_type(&elem_tv);
                    let (p1, binds1) = infer_pattern(unifier, supply, env, p, &elem_ty)?;
                    preds.extend(p1);
                    bindings.extend(binds1);
                }
                let bindings = bindings
                    .into_iter()
                    .map(|(name, ty)| (name, unifier.apply_type(&ty)))
                    .collect();
                Ok((preds, bindings))
            }
            Pattern::Cons(_, head, tail) => {
                let elem_tv = Type::var(supply.fresh(Some("a".into())));
                let list_ty = Type::app(Type::builtin(BuiltinTypeId::List), elem_tv.clone());
                unifier.unify(scrutinee_ty, &list_ty)?;
                let mut preds = Vec::new();
                let mut bindings = Vec::new();

                let head_ty = unifier.apply_type(&elem_tv);
                let (p1, binds1) = infer_pattern(unifier, supply, env, head, &head_ty)?;
                preds.extend(p1);
                bindings.extend(binds1);

                let tail_ty = Type::app(
                    Type::builtin(BuiltinTypeId::List),
                    unifier.apply_type(&elem_tv),
                );
                let (p2, binds2) = infer_pattern(unifier, supply, env, tail, &tail_ty)?;
                preds.extend(p2);
                bindings.extend(binds2);

                let bindings = bindings
                    .into_iter()
                    .map(|(name, ty)| (name, unifier.apply_type(&ty)))
                    .collect();
                Ok((preds, bindings))
            }
            Pattern::Tuple(_, elems) => {
                let mut elem_tys: Vec<Type> = (0..elems.len())
                    .map(|i| Type::var(supply.fresh(Some(format!("t{i}").into()))))
                    .collect();
                let expected = Type::tuple(elem_tys.clone());
                unifier.unify(scrutinee_ty, &expected)?;
                elem_tys = elem_tys
                    .into_iter()
                    .map(|t| unifier.apply_type(&t))
                    .collect();

                let mut preds = Vec::new();
                let mut bindings = Vec::new();
                for (p, ty) in elems.iter().zip(elem_tys.iter()) {
                    let (p_preds, p_binds) = infer_pattern(unifier, supply, env, p, ty)?;
                    preds.extend(p_preds);
                    bindings.extend(p_binds);
                }
                let bindings = bindings
                    .into_iter()
                    .map(|(name, ty)| (name, unifier.apply_type(&ty)))
                    .collect();
                Ok((preds, bindings))
            }
            Pattern::Dict(_, fields) => {
                if let TypeKind::Record(ty_fields) = scrutinee_ty.as_ref() {
                    let mut preds = Vec::new();
                    let mut bindings = Vec::new();
                    for (key, pat) in fields {
                        let ty = ty_fields
                            .iter()
                            .find(|(name, _)| name == key)
                            .map(|(_, ty)| unifier.apply_type(ty))
                            .ok_or_else(|| TypeError::UnknownField {
                                field: key.clone(),
                                typ: scrutinee_ty.to_string(),
                            })?;
                        let (p_preds, p_binds) = infer_pattern(unifier, supply, env, pat, &ty)?;
                        preds.extend(p_preds);
                        bindings.extend(p_binds);
                    }
                    let bindings = bindings
                        .into_iter()
                        .map(|(name, ty)| (name, unifier.apply_type(&ty)))
                        .collect();
                    Ok((preds, bindings))
                } else {
                    let elem_tv = Type::var(supply.fresh(Some("v".into())));
                    let dict_ty = Type::app(Type::builtin(BuiltinTypeId::Dict), elem_tv.clone());
                    unifier.unify(scrutinee_ty, &dict_ty)?;
                    let elem_ty = unifier.apply_type(&elem_tv);

                    let mut preds = Vec::new();
                    let mut bindings = Vec::new();
                    for (_key, pat) in fields {
                        let (p_preds, p_binds) =
                            infer_pattern(unifier, supply, env, pat, &elem_ty)?;
                        preds.extend(p_preds);
                        bindings.extend(p_binds);
                    }
                    let bindings = bindings
                        .into_iter()
                        .map(|(name, ty)| (name, unifier.apply_type(&ty)))
                        .collect();
                    Ok((preds, bindings))
                }
            }
        }
    })();
    res.map_err(|err| err.with_span(&span))
}

fn type_head_name(typ: &Type) -> Option<&Symbol> {
    let mut cur = typ;
    while let TypeKind::App(head, _) = cur.as_ref() {
        cur = head;
    }
    match cur.as_ref() {
        TypeKind::Con(tc) => Some(&tc.name),
        _ => None,
    }
}

fn adt_name_from_patterns(
    adts: &BTreeMap<Symbol, AdtDecl>,
    patterns: &[Pattern],
) -> Option<Symbol> {
    let mut candidate: Option<Symbol> = None;
    for pat in patterns {
        let next = match pat {
            Pattern::Named(_, name, _) => {
                let name_sym = name.to_dotted_symbol();
                ctor_lookup(adts, &name_sym).map(|(adt, _)| adt.name.clone())
            }
            Pattern::List(..) | Pattern::Cons(..) => Some(sym("List")),
            _ => None,
        };
        if let Some(next) = next {
            match &candidate {
                None => candidate = Some(next),
                Some(prev) if *prev == next => {}
                Some(_) => return None,
            }
        }
    }
    candidate
}

fn check_match_exhaustive(
    adts: &BTreeMap<Symbol, AdtDecl>,
    scrutinee_ty: &Type,
    patterns: &[Pattern],
) -> Result<(), TypeError> {
    if patterns
        .iter()
        .any(|p| matches!(p, Pattern::Wildcard(..) | Pattern::Var(_)))
    {
        return Ok(());
    }
    let adt_name = match type_head_name(scrutinee_ty).cloned() {
        Some(name) => name,
        None => match adt_name_from_patterns(adts, patterns) {
            Some(name) => name,
            None => return Ok(()),
        },
    };
    let adt = match adts.get(&adt_name) {
        Some(adt) => adt,
        None => return Ok(()),
    };
    let ctor_names: BTreeSet<Symbol> = adt.variants.iter().map(|v| v.name.clone()).collect();
    if ctor_names.is_empty() {
        return Ok(());
    }
    let mut covered = BTreeSet::new();
    for pat in patterns {
        match pat {
            Pattern::Named(_, name, _) => {
                let name_sym = name.to_dotted_symbol();
                if ctor_names.contains(&name_sym) {
                    covered.insert(name_sym);
                }
            }
            Pattern::List(_, elems) if adt_name.as_ref() == "List" && elems.is_empty() => {
                covered.insert(sym("Empty"));
            }
            Pattern::Cons(..) if adt_name.as_ref() == "List" => {
                covered.insert(sym("Cons"));
            }
            _ => {}
        }
    }
    let mut missing: Vec<Symbol> = ctor_names.difference(&covered).cloned().collect();
    if missing.is_empty() {
        return Ok(());
    }
    missing.sort();
    Err(TypeError::NonExhaustiveMatch {
        typ: scrutinee_ty.to_string(),
        missing,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        types::collect_adts_in_types,
        typesystem::{TypeSystemLimits, entails, generalize},
        unification::bind,
    };
    use rex_lexer::{Token, span::Span};
    use rex_parser::Parser;
    use rex_util::{GasCosts, GasMeter};

    fn tvar(id: TypeVarId, name: &str) -> Type {
        Type::var(TypeVar::new(id, Some(sym(name))))
    }

    fn dict_of(elem: Type) -> Type {
        Type::app(Type::builtin(BuiltinTypeId::Dict), elem)
    }

    #[test]
    fn unify_simple() {
        let t1 = Type::fun(tvar(0, "a"), Type::builtin(BuiltinTypeId::U32));
        let t2 = Type::fun(Type::builtin(BuiltinTypeId::U16), tvar(1, "b"));
        let subst = unify(&t1, &t2).unwrap();
        assert_eq!(subst.get(&0), Some(&Type::builtin(BuiltinTypeId::U16)));
        assert_eq!(subst.get(&1), Some(&Type::builtin(BuiltinTypeId::U32)));
    }

    #[test]
    fn occurs_check_blocks_infinite_type() {
        let tv = TypeVar::new(0, Some(sym("a")));
        let t = Type::fun(Type::var(tv.clone()), Type::builtin(BuiltinTypeId::U8));
        let err = bind(&tv, &t).unwrap_err();
        assert!(matches!(err, TypeError::Occurs(_, _)));
    }

    #[test]
    fn instantiate_and_generalize_round_trip() {
        let mut supply = TypeVarSupply::new();
        let a = Type::var(supply.fresh(Some(sym("a"))));
        let scheme = generalize(&TypeEnv::new(), vec![], Type::fun(a.clone(), a.clone()));
        let (preds, inst) = instantiate(&scheme, &mut supply);
        assert!(preds.is_empty());
        if let TypeKind::Fun(l, r) = inst.as_ref() {
            match (l.as_ref(), r.as_ref()) {
                (TypeKind::Var(_), TypeKind::Var(_)) => {}
                _ => panic!("expected polymorphic identity"),
            }
        } else {
            panic!("expected function type");
        }
    }

    #[test]
    fn entail_superclasses() {
        let ts = TypeSystem::new_with_prelude().unwrap();
        let pred = Predicate::new("Semiring", Type::builtin(BuiltinTypeId::I32));
        let given = [Predicate::new(
            "AdditiveGroup",
            Type::builtin(BuiltinTypeId::I32),
        )];
        assert!(entails(&ts.classes, &given, &pred).unwrap());
    }

    #[test]
    fn entail_instances() {
        let ts = TypeSystem::new_with_prelude().unwrap();
        let pred = Predicate::new("Field", Type::builtin(BuiltinTypeId::F32));
        assert!(entails(&ts.classes, &[], &pred).unwrap());

        let pred_fail = Predicate::new("Field", Type::builtin(BuiltinTypeId::U32));
        assert!(!entails(&ts.classes, &[], &pred_fail).unwrap());
    }

    #[test]
    fn prelude_injects_functions() {
        let ts = TypeSystem::new_with_prelude().unwrap();
        let minus = ts.env.lookup(&sym("-")).expect("minus in env");
        let div = ts.env.lookup(&sym("/")).expect("div in env");
        assert_eq!(minus.len(), 1);
        assert_eq!(div.len(), 1);
        let minus = &minus[0];
        let div = &div[0];
        assert_eq!(minus.preds.len(), 1);
        assert_eq!(minus.vars.len(), 1);
        assert_eq!(div.preds.len(), 1);
        assert_eq!(div.vars.len(), 1);
    }

    #[test]
    fn adt_constructors_are_present() {
        let ts = TypeSystem::new_with_prelude().unwrap();
        assert!(ts.env.lookup(&sym("Empty")).is_some());
        assert!(ts.env.lookup(&sym("Cons")).is_some());
        assert!(ts.env.lookup(&sym("Ok")).is_some());
        assert!(ts.env.lookup(&sym("Err")).is_some());
        assert!(ts.env.lookup(&sym("Some")).is_some());
        assert!(ts.env.lookup(&sym("None")).is_some());
    }

    fn parse_expr(code: &str) -> std::sync::Arc<rex_ast::expr::Expr> {
        let mut parser = Parser::new(Token::tokenize(code).unwrap());
        parser.parse_program(&mut GasMeter::default()).unwrap().expr
    }

    fn parse_program(code: &str) -> rex_ast::expr::Program {
        let mut parser = Parser::new(Token::tokenize(code).unwrap());
        parser.parse_program(&mut GasMeter::default()).unwrap()
    }

    #[test]
    fn infer_deep_list_does_not_overflow() {
        const N: usize = 40;
        let mut code = String::new();
        code.push_str("let xs = ");
        for _ in 0..N {
            code.push_str("Cons 0 (");
        }
        code.push_str("Empty");
        for _ in 0..N {
            code.push(')');
        }
        code.push_str(" in xs");

        let parse_handle = std::thread::Builder::new()
            .name("infer_deep_list_parse".into())
            .stack_size(128 * 1024 * 1024)
            .spawn(move || {
                let tokens = Token::tokenize(&code).unwrap();
                let mut parser = Parser::new(tokens);
                parser.parse_program(&mut GasMeter::default())
            })
            .unwrap();
        let program = parse_handle.join().unwrap().unwrap();
        let expr = program.expr;
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let (_preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
        assert_eq!(
            ty,
            Type::app(
                Type::builtin(BuiltinTypeId::List),
                Type::builtin(BuiltinTypeId::I32)
            )
        );
    }

    #[test]
    fn collect_adts_in_types_finds_nested_unique_adts() {
        let foo = Type::user_con("Foo", 1);
        let bar = Type::user_con("Bar", 0);
        let ty = Type::fun(
            Type::app(
                Type::builtin(BuiltinTypeId::List),
                Type::app(foo.clone(), tvar(0, "a")),
            ),
            Type::tuple(vec![
                Type::app(foo.clone(), Type::builtin(BuiltinTypeId::I32)),
                bar.clone(),
            ]),
        );

        let adts = collect_adts_in_types(vec![ty]).unwrap();
        assert_eq!(adts, vec![foo, bar]);
    }

    #[test]
    fn collect_adts_in_types_rejects_conflicting_names() {
        let arity1 = Type::user_con("Thing", 1);
        let arity2 = Type::user_con("Thing", 2);

        let err = collect_adts_in_types(vec![arity1.clone(), arity2.clone()]).unwrap_err();
        assert_eq!(err.conflicts.len(), 1);
        let conflict = &err.conflicts[0];
        assert_eq!(conflict.name, sym("Thing"));
        assert_eq!(conflict.definitions, vec![arity1, arity2]);
    }

    #[test]
    fn infer_depth_limit_is_enforced() {
        const N: usize = 40;
        let mut code = String::new();
        code.push_str("let xs = ");
        for _ in 0..N {
            code.push_str("Cons 0 (");
        }
        code.push_str("Empty");
        for _ in 0..N {
            code.push(')');
        }
        code.push_str(" in xs");

        let program = parse_program(&code);
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        ts.set_limits(TypeSystemLimits {
            max_infer_depth: Some(8),
        });

        let err = infer(&mut ts, program.expr.as_ref()).unwrap_err();
        assert!(
            err.to_string().contains("maximum inference depth exceeded"),
            "expected a max-depth inference error, got: {err:?}"
        );
    }

    #[test]
    fn declare_fn_injects_scheme_for_use_sites() {
        let program = parse_program(
            r#"
            declare fn id x: a -> a
            id 1
            "#,
        );
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        ts.register_decls(&program.decls).unwrap();
        let (preds, ty) = infer(&mut ts, program.expr.as_ref()).unwrap();
        assert!(
            preds.is_empty()
                || preds.iter().all(|p| p.class.as_ref() == "Integral"
                    && p.typ == Type::builtin(BuiltinTypeId::I32))
        );
        assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    }

    #[test]
    fn declare_fn_is_noop_when_matching_existing_scheme() {
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        ts.add_value(
            "foo",
            Scheme::new(
                vec![],
                vec![],
                Type::fun(
                    Type::builtin(BuiltinTypeId::I32),
                    Type::builtin(BuiltinTypeId::I32),
                ),
            ),
        );

        let program = parse_program(
            r#"
            declare fn foo x: i32 -> i32
            0
            "#,
        );
        let rex_ast::expr::Decl::DeclareFn(fd) = &program.decls[0] else {
            panic!("expected declare fn decl");
        };
        ts.inject_declare_fn_decl(fd).unwrap();
    }

    #[test]
    fn unit_type_parses_and_infers() {
        let program = parse_program(
            r#"
            fn unit_id x: () -> () = x
            unit_id ()
            "#,
        );
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        ts.register_decls(&program.decls).unwrap();
        let (preds, ty) = infer(&mut ts, program.expr.as_ref()).unwrap();
        assert!(preds.is_empty());
        assert_eq!(ty, Type::tuple(vec![]));
    }

    fn strip_span(mut err: TypeError) -> TypeError {
        while let TypeError::Spanned { error, .. } = err {
            err = *error;
        }
        err
    }

    #[test]
    fn type_errors_include_span() {
        let expr = parse_expr("missing");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let err = infer(&mut ts, expr.as_ref()).unwrap_err();
        match err {
            TypeError::Spanned { span, error } => {
                assert_ne!(span, Span::default());
                assert!(matches!(
                    *error,
                    TypeError::UnknownVar(name) if name.as_ref() == "missing"
                ));
            }
            other => panic!("expected spanned error, got {other:?}"),
        }
    }

    #[test]
    fn infer_with_gas_rejects_out_of_budget() {
        let expr = parse_expr("1");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let mut gas = GasMeter::new(
            Some(0),
            GasCosts {
                infer_node: 1,
                unify_step: 0,
                ..GasCosts::sensible_defaults()
            },
        );
        let err = infer_with_gas(&mut ts, expr.as_ref(), &mut gas).unwrap_err();
        assert!(matches!(strip_span(err), TypeError::OutOfGas(..)));
    }

    #[test]
    fn reject_user_redefinition_of_primitive_type_name() {
        let program = parse_program("type i32 = I32Wrap i32");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let rex_ast::expr::Decl::Type(decl) = &program.decls[0] else {
            panic!("expected type decl");
        };
        let err = ts.register_type_decl(decl).unwrap_err();
        assert!(matches!(
            err,
            TypeError::ReservedTypeName(name) if name.as_ref() == "i32"
        ));
    }

    #[test]
    fn reject_user_redefinition_of_prelude_adt_name() {
        let program = parse_program("type Result e a = Nope e a");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let rex_ast::expr::Decl::Type(decl) = &program.decls[0] else {
            panic!("expected type decl");
        };
        let err = ts.register_type_decl(decl).unwrap_err();
        assert!(matches!(
            err,
            TypeError::ReservedTypeName(name) if name.as_ref() == "Result"
        ));
    }

    #[test]
    fn reject_user_redefinition_of_promise_type_name() {
        let program = parse_program("type Promise a = PromiseWrap a");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let rex_ast::expr::Decl::Type(decl) = &program.decls[0] else {
            panic!("expected type decl");
        };
        let err = ts.register_type_decl(decl).unwrap_err();
        assert!(matches!(
            err,
            TypeError::ReservedTypeName(name) if name.as_ref() == "Promise"
        ));
    }

    #[test]
    fn infer_polymorphic_id_tuple() {
        let expr = parse_expr(
            r#"
            let
                id = \x -> x
            in
                id (id 420, id 6.9, id "str")
            "#,
        );
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let (_preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
        let expected = Type::tuple(vec![
            Type::builtin(BuiltinTypeId::I32),
            Type::builtin(BuiltinTypeId::F32),
            Type::builtin(BuiltinTypeId::String),
        ]);
        assert_eq!(ty, expected);
    }

    #[test]
    fn infer_type_annotation_ok() {
        let expr = parse_expr("let x: i32 = 42 in x");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let (_preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
        assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    }

    #[test]
    fn infer_type_annotation_lambda_param() {
        let expr = parse_expr("\\ (a : f32) -> a");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let (_preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
        assert_eq!(
            ty,
            Type::fun(
                Type::builtin(BuiltinTypeId::F32),
                Type::builtin(BuiltinTypeId::F32)
            )
        );
    }

    #[test]
    fn infer_type_annotation_is_alias() {
        let expr = parse_expr("\"hi\" is str");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let (_preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
        assert_eq!(ty, Type::builtin(BuiltinTypeId::String));
    }

    #[test]
    fn infer_type_annotation_with_promise_constructor() {
        let expr = parse_expr("\\(x: Promise i32) -> x");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let (_preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
        let promise_i32 = Type::promise(Type::builtin(BuiltinTypeId::I32));
        assert_eq!(ty, Type::fun(promise_i32.clone(), promise_i32));
    }

    #[test]
    fn infer_type_annotation_mismatch_error() {
        let expr = parse_expr("let x: i32 = 3.14 in x");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
        assert!(matches!(err, TypeError::Unification(_, _)));
    }

    #[test]
    fn infer_project_single_variant_let() {
        let program = parse_program(
            r#"
            type MyADT = MyVariant1 { field1: i32, field2: f32 }
            let
                x = MyVariant1 { field1 = 1, field2 = 2.0 }
            in
                (x.field1, x.field2)
            "#,
        );
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        for decl in &program.decls {
            if let rex_ast::expr::Decl::Type(decl) = decl {
                ts.register_type_decl(decl).unwrap();
            }
        }
        let (_preds, ty) = infer(&mut ts, program.expr.as_ref()).unwrap();
        let expected = Type::tuple(vec![
            Type::builtin(BuiltinTypeId::I32),
            Type::builtin(BuiltinTypeId::F32),
        ]);
        assert_eq!(ty, expected);
    }

    #[test]
    fn infer_project_known_variant_let() {
        let program = parse_program(
            r#"
            type MyADT = MyVariant1 { field1: i32, field2: f32 } | MyVariant2 i32 f32
            let
                x = MyVariant1 { field1 = 1, field2 = 2.0 }
            in
                x.field1
            "#,
        );
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        for decl in &program.decls {
            if let rex_ast::expr::Decl::Type(decl) = decl {
                ts.register_type_decl(decl).unwrap();
            }
        }
        let (_preds, ty) = infer(&mut ts, program.expr.as_ref()).unwrap();
        assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    }

    #[test]
    fn infer_project_unknown_variant_error() {
        let program = parse_program(
            r#"
            type MyADT = MyVariant1 { field1: i32, field2: f32 } | MyVariant2 i32 f32
            let
                x = MyVariant2 1 2.0
            in
                x.field1
            "#,
        );
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        for decl in &program.decls {
            if let rex_ast::expr::Decl::Type(decl) = decl {
                ts.register_type_decl(decl).unwrap();
            }
        }
        let err = strip_span(infer(&mut ts, program.expr.as_ref()).unwrap_err());
        assert!(matches!(err, TypeError::FieldNotKnown { .. }));
    }

    #[test]
    fn infer_project_lambda_param_single_variant() {
        let program = parse_program(
            r#"
            type Boxed = Boxed { value: i32 }
            let
                f = \x -> x.value
            in
                f (Boxed { value = 1 })
            "#,
        );
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        for decl in &program.decls {
            if let rex_ast::expr::Decl::Type(decl) = decl {
                ts.register_type_decl(decl).unwrap();
            }
        }
        let (_preds, ty) = infer(&mut ts, program.expr.as_ref()).unwrap();
        assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    }

    #[test]
    fn infer_project_in_match_arm() {
        let program = parse_program(
            r#"
            type MyADT = MyVariant1 { field1: i32 } | MyVariant2 i32
            let
                x = MyVariant1 { field1 = 1 }
            in
                match x
                    when MyVariant1 { field1 } -> x.field1
                    when MyVariant2 _ -> 0
            "#,
        );
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        for decl in &program.decls {
            if let rex_ast::expr::Decl::Type(decl) = decl {
                ts.register_type_decl(decl).unwrap();
            }
        }
        let (_preds, ty) = infer(&mut ts, program.expr.as_ref()).unwrap();
        assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    }

    #[test]
    fn infer_nested_let_lambda_match_option() {
        let expr = parse_expr(
            r#"
            let
                choose = \flag a b -> if flag then a else b,
                build = \flag ->
                    let
                        pick = choose flag,
                        val = pick 1 2
                    in
                        Some val
            in
                match (build true)
                    when Some x -> x
                    when None -> 0
            "#,
        );
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let (_preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
        assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    }

    #[test]
    fn infer_polymorphic_apply_in_tuple() {
        let expr = parse_expr(
            r#"
            let
                apply = \f x -> f x,
                id = \x -> x,
                wrap = \x -> (x, x)
            in
                (apply id 1, apply id "hi", apply wrap true)
            "#,
        );
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let (_preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
        let expected = Type::tuple(vec![
            Type::builtin(BuiltinTypeId::I32),
            Type::builtin(BuiltinTypeId::String),
            Type::tuple(vec![
                Type::builtin(BuiltinTypeId::Bool),
                Type::builtin(BuiltinTypeId::Bool),
            ]),
        ]);
        assert_eq!(ty, expected);
    }

    #[test]
    fn infer_nested_result_option_match() {
        let expr = parse_expr(
            r#"
            let
                unwrap = \x ->
                    match x
                        when Ok (Some v) -> v
                        when Ok None -> 0
                        when Err _ -> 0
            in
                unwrap (Ok (Some 5))
            "#,
        );
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let (_preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
        assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    }

    #[test]
    fn infer_head_or_list_match() {
        let expr = parse_expr(
            r#"
            let
                head_or = \fallback xs ->
                    match xs
                        when [] -> fallback
                        when x::xs -> x
            in
                (head_or 0 [1, 2, 3], head_or 0 [])
            "#,
        );
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let (_preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
        let expected = Type::tuple(vec![
            Type::builtin(BuiltinTypeId::I32),
            Type::builtin(BuiltinTypeId::I32),
        ]);
        assert_eq!(ty, expected);
    }

    #[test]
    fn infer_head_or_list_match_cons_constructor_form() {
        let expr = parse_expr(
            r#"
            let
                head_or = \fallback xs ->
                    match xs
                        when [] -> fallback
                        when Cons x xs1 -> x
            in
                (head_or 0 (Cons 1 (Cons 2 Empty)), head_or 0 Empty)
            "#,
        );
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let (_preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
        let expected = Type::tuple(vec![
            Type::builtin(BuiltinTypeId::I32),
            Type::builtin(BuiltinTypeId::I32),
        ]);
        assert_eq!(ty, expected);
    }

    #[test]
    fn infer_record_pattern_in_lambda() {
        let program = parse_program(
            r#"
            type Pair = Pair { left: i32, right: i32 }
            let
                sum = \p ->
                    match p
                        when Pair { left, right } -> left + right
            in
                sum (Pair { left = 1, right = 2 })
            "#,
        );
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        for decl in &program.decls {
            if let rex_ast::expr::Decl::Type(decl) = decl {
                ts.register_type_decl(decl).unwrap();
            }
        }
        let (_preds, ty) = infer(&mut ts, program.expr.as_ref()).unwrap();
        assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    }

    #[test]
    fn infer_fn_decl_simple() {
        let program = parse_program(
            r#"
            fn add (x: i32, y: i32) -> i32 = x + y
            add 1 2
            "#,
        );
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let expr = program.expr_with_fns();
        let (_preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
        assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    }

    #[test]
    fn infer_fn_decl_signature_form() {
        let program = parse_program(
            r#"
            fn add : i32 -> i32 -> i32 = \x y -> x + y
            add 1 2
            "#,
        );
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let expr = program.expr_with_fns();
        let (_preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
        assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    }

    #[test]
    fn infer_fn_decl_polymorphic_where_constraints() {
        let program = parse_program(
            r#"
            fn my_add (x: a, y: a) -> a where AdditiveMonoid a = x + y
            (my_add 1 2, my_add 1.0 2.0)
            "#,
        );
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let expr = program.expr_with_fns();
        let (_preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
        assert_eq!(
            ty,
            Type::tuple(vec![
                Type::builtin(BuiltinTypeId::I32),
                Type::builtin(BuiltinTypeId::F32)
            ])
        );
    }

    #[test]
    fn infer_additive_monoid_constraint() {
        let expr = parse_expr("\\x y -> x + y");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let (preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
        assert_eq!(preds.len(), 1);
        assert_eq!(preds[0].class.as_ref(), "AdditiveMonoid");

        if let TypeKind::Fun(a, rest) = ty.as_ref()
            && let TypeKind::Fun(b, c) = rest.as_ref()
        {
            assert_eq!(a.as_ref(), b.as_ref());
            assert_eq!(b.as_ref(), c.as_ref());
            assert_eq!(preds[0].typ, a.clone());
            return;
        }
        panic!("expected a -> a -> a");
    }

    #[test]
    fn infer_multiplicative_monoid_constraint() {
        let expr = parse_expr("\\x y -> x * y");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let (preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
        assert_eq!(preds.len(), 1);
        assert_eq!(preds[0].class.as_ref(), "MultiplicativeMonoid");

        if let TypeKind::Fun(a, rest) = ty.as_ref()
            && let TypeKind::Fun(b, c) = rest.as_ref()
        {
            assert_eq!(a.as_ref(), b.as_ref());
            assert_eq!(b.as_ref(), c.as_ref());
            assert_eq!(preds[0].typ, a.clone());
            return;
        }
        panic!("expected a -> a -> a");
    }

    #[test]
    fn infer_additive_group_constraint() {
        let expr = parse_expr("\\x y -> x - y");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let (preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
        assert_eq!(preds.len(), 1);
        assert_eq!(preds[0].class.as_ref(), "AdditiveGroup");

        if let TypeKind::Fun(a, rest) = ty.as_ref()
            && let TypeKind::Fun(b, c) = rest.as_ref()
        {
            assert_eq!(a.as_ref(), b.as_ref());
            assert_eq!(b.as_ref(), c.as_ref());
            assert_eq!(preds[0].typ, a.clone());
            return;
        }
        panic!("expected a -> a -> a");
    }

    #[test]
    fn infer_integral_constraint() {
        let expr = parse_expr("\\x y -> x % y");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let (preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
        assert_eq!(preds.len(), 1);
        assert_eq!(preds[0].class.as_ref(), "Integral");

        if let TypeKind::Fun(a, rest) = ty.as_ref()
            && let TypeKind::Fun(b, c) = rest.as_ref()
        {
            assert_eq!(a.as_ref(), b.as_ref());
            assert_eq!(b.as_ref(), c.as_ref());
            assert_eq!(preds[0].typ, a.clone());
            return;
        }
        panic!("expected a -> a -> a");
    }

    #[test]
    fn infer_literal_addition_defaults() {
        let expr = parse_expr("1 + 2");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let (preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
        assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
        assert_eq!(preds.len(), 2);
        assert!(preds.iter().any(|p| p.class.as_ref() == "AdditiveMonoid"));
        assert!(preds.iter().any(|p| p.class.as_ref() == "Integral"));
        assert!(
            preds
                .iter()
                .all(|p| p.typ == Type::builtin(BuiltinTypeId::I32))
        );
    }

    #[test]
    fn infer_mod_defaults() {
        let expr = parse_expr("1 % 2");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let (preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
        assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
        assert_eq!(preds.len(), 1);
        assert_eq!(preds[0].class.as_ref(), "Integral");
        assert_eq!(preds[0].typ, Type::builtin(BuiltinTypeId::I32));
    }

    #[test]
    fn infer_get_list_type() {
        let expr = parse_expr("get 1 [1, 2, 3]");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let (preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
        assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
        assert!(preds.iter().any(|p| p.class.as_ref() == "Indexable"));
        assert!(preds.iter().all(|p| {
            p.class.as_ref() == "Indexable"
                || (p.class.as_ref() == "Integral" && p.typ == Type::builtin(BuiltinTypeId::I32))
        }));
        for pred in preds.iter().filter(|p| p.class.as_ref() == "Indexable") {
            assert!(entails(&ts.classes, &[], pred).unwrap());
        }
    }

    #[test]
    fn infer_get_tuple_type() {
        let expr = parse_expr("(1, 'Hello', true).0");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let (preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
        assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
        assert!(preds.is_empty() || preds.iter().all(|p| p.class.as_ref() == "Integral"));

        let expr = parse_expr("(1, 'Hello', true).1");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let (preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
        assert_eq!(ty, Type::builtin(BuiltinTypeId::String));
        assert!(preds.is_empty() || preds.iter().all(|p| p.class.as_ref() == "Integral"));

        let expr = parse_expr("(1, 'Hello', true).2");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let (preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
        assert_eq!(ty, Type::builtin(BuiltinTypeId::Bool));
        assert!(preds.is_empty() || preds.iter().all(|p| p.class.as_ref() == "Integral"));
    }

    #[test]
    fn infer_division_defaults() {
        let expr = parse_expr("1.0 / 2.0");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let (preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
        assert_eq!(ty, Type::builtin(BuiltinTypeId::F32));
        assert_eq!(preds.len(), 1);
        assert_eq!(preds[0].class.as_ref(), "Field");
        assert_eq!(preds[0].typ, Type::builtin(BuiltinTypeId::F32));
        assert!(entails(&ts.classes, &[], &preds[0]).unwrap());
    }

    #[test]
    fn infer_unbound_variable_error() {
        let expr = parse_expr("missing");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
        assert!(matches!(
            err,
            TypeError::UnknownVar(name) if name.as_ref() == "missing"
        ));
    }

    #[test]
    fn infer_if_branch_type_mismatch_error() {
        let expr = parse_expr(r#"if true then 1 else "no""#);
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
        match err {
            TypeError::Unification(a, b) => {
                let ok = (a == "i32" && b == "string") || (a == "string" && b == "i32");
                assert!(ok, "expected i32 vs string, got {a} vs {b}");
            }
            other => panic!("expected unification error, got {other:?}"),
        }
    }

    #[test]
    fn infer_unknown_pattern_constructor_error() {
        let expr = parse_expr("match 1 when Nope -> 1");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
        assert!(matches!(
            err,
            TypeError::UnknownVar(name) if name.as_ref() == "Nope"
        ));
    }

    #[test]
    fn infer_ambiguous_overload_error() {
        let mut ts = TypeSystem::new();
        let a = TypeVar::new(0, Some(sym("a")));
        let b = TypeVar::new(1, Some(sym("b")));
        let scheme_a = Scheme::new(vec![a.clone()], vec![], Type::var(a));
        let scheme_b = Scheme::new(vec![b.clone()], vec![], Type::var(b));
        ts.add_overload(sym("dup"), scheme_a);
        ts.add_overload(sym("dup"), scheme_b);
        let expr = parse_expr("dup");
        let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
        assert!(matches!(
            err,
            TypeError::AmbiguousOverload(name) if name.as_ref() == "dup"
        ));
    }

    #[test]
    fn infer_if_cond_not_bool_error() {
        let expr = parse_expr("if 1 then 2 else 3");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
        match err {
            TypeError::Unification(a, b) => {
                let ok = (a == "bool" && b == "i32") || (a == "i32" && b == "bool");
                assert!(ok, "expected bool vs i32, got {a} vs {b}");
            }
            other => panic!("expected unification error, got {other:?}"),
        }
    }

    #[test]
    fn infer_apply_non_function_error() {
        let expr = parse_expr("1 2");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
        assert!(matches!(err, TypeError::Unification(_, _)));
    }

    #[test]
    fn infer_list_element_mismatch_error() {
        let expr = parse_expr("[1, true]");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
        match err {
            TypeError::Unification(a, b) => {
                let ok = (a == "i32" && b == "bool") || (a == "bool" && b == "i32");
                assert!(ok, "expected i32 vs bool, got {a} vs {b}");
            }
            other => panic!("expected unification error, got {other:?}"),
        }
    }

    #[test]
    fn infer_dict_value_mismatch_error() {
        let expr = parse_expr("{a = 1, b = true}");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
        match err {
            TypeError::Unification(a, b) => {
                let ok = (a == "i32" && b == "bool") || (a == "bool" && b == "i32");
                assert!(ok, "expected i32 vs bool, got {a} vs {b}");
            }
            other => panic!("expected unification error, got {other:?}"),
        }
    }

    #[test]
    fn infer_match_list_on_non_list_error() {
        let expr = parse_expr("match 1 when [x] -> x");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        assert!(infer(&mut ts, expr.as_ref()).is_err());
    }

    #[test]
    fn infer_pattern_constructor_arity_error() {
        let expr = parse_expr("match (Ok 1) when Ok x y -> x");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
        assert!(matches!(
            err,
            TypeError::UnsupportedExpr("pattern constructor")
        ));
    }

    #[test]
    fn infer_match_arm_type_mismatch_error() {
        let expr = parse_expr(r#"match 1 when _ -> 1 when _ -> "no""#);
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
        match err {
            TypeError::Unification(a, b) => {
                let ok = (a == "i32" && b == "string") || (a == "string" && b == "i32");
                assert!(ok, "expected i32 vs string, got {a} vs {b}");
            }
            other => panic!("expected unification error, got {other:?}"),
        }
    }

    #[test]
    fn infer_match_option_on_non_option_error() {
        let expr = parse_expr("match 1 when Some x -> x");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        assert!(infer(&mut ts, expr.as_ref()).is_err());
    }

    #[test]
    fn infer_dict_pattern_on_non_dict_error() {
        let expr = parse_expr("match 1 when {a} -> a");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
        assert!(matches!(err, TypeError::Unification(_, _)));
    }

    #[test]
    fn infer_cons_pattern_on_non_list_error() {
        let expr = parse_expr("match 1 when x::xs -> x");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        assert!(infer(&mut ts, expr.as_ref()).is_err());
    }

    #[test]
    fn infer_apply_wrong_arg_type_error() {
        let expr = parse_expr("(\\x -> x + 1) true");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
        assert!(matches!(err, TypeError::Unification(_, _)));
    }

    #[test]
    fn infer_self_application_occurs_error() {
        let expr = parse_expr("\\x -> x x");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
        assert!(matches!(err, TypeError::Occurs(_, _)));
    }

    #[test]
    fn infer_apply_constructor_too_many_args_error() {
        let expr = parse_expr("Some 1 2");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
        assert!(matches!(err, TypeError::Unification(_, _)));
    }

    #[test]
    fn infer_operator_type_mismatch_error() {
        let expr = parse_expr("1 + true");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
        assert!(matches!(err, TypeError::Unification(_, _)));
    }

    #[test]
    fn infer_non_exhaustive_match_is_error() {
        let expr = parse_expr("match (Ok 1) when Ok x -> x");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
        assert!(matches!(err, TypeError::NonExhaustiveMatch { .. }));
    }

    #[test]
    fn infer_non_exhaustive_match_on_bound_var_error() {
        let expr = parse_expr("let x = Ok 1 in match x when Ok y -> y");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
        assert!(matches!(err, TypeError::NonExhaustiveMatch { .. }));
    }

    #[test]
    fn infer_non_exhaustive_match_in_lambda_error() {
        let expr = parse_expr("\\x -> match x when Ok y -> y");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
        assert!(matches!(err, TypeError::NonExhaustiveMatch { .. }));
    }

    #[test]
    fn infer_non_exhaustive_option_match_error() {
        let expr = parse_expr("match (Some 1) when Some x -> x");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
        match err {
            TypeError::NonExhaustiveMatch { missing, .. } => {
                assert_eq!(missing, vec![sym("None")]);
            }
            other => panic!("expected non-exhaustive match, got {other:?}"),
        }
    }

    #[test]
    fn infer_non_exhaustive_result_match_error() {
        let expr = parse_expr("match (Err 1) when Ok x -> x");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
        match err {
            TypeError::NonExhaustiveMatch { missing, .. } => {
                assert_eq!(missing, vec![sym("Err")]);
            }
            other => panic!("expected non-exhaustive match, got {other:?}"),
        }
    }

    #[test]
    fn infer_non_exhaustive_list_missing_empty_error() {
        let expr = parse_expr("match [1, 2] when x::xs -> x");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
        match err {
            TypeError::NonExhaustiveMatch { missing, .. } => {
                assert_eq!(missing, vec![sym("Empty")]);
            }
            other => panic!("expected non-exhaustive match, got {other:?}"),
        }
    }

    #[test]
    fn infer_non_exhaustive_list_match_on_bound_var_error() {
        let expr = parse_expr("let xs = [1, 2] in match xs when x::xs -> x");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
        assert!(matches!(err, TypeError::NonExhaustiveMatch { .. }));
    }

    #[test]
    fn infer_non_exhaustive_list_missing_cons_error() {
        let expr = parse_expr("match [1] when [] -> 0");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
        match err {
            TypeError::NonExhaustiveMatch { missing, .. } => {
                assert_eq!(missing, vec![sym("Cons")]);
            }
            other => panic!("expected non-exhaustive match, got {other:?}"),
        }
    }

    #[test]
    fn infer_match_list_patterns_on_result_error() {
        let expr = parse_expr("match (Ok 1) when [] -> 0 when x::xs -> 1");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
        assert!(matches!(err, TypeError::Unification(_, _)));
    }

    #[test]
    fn infer_missing_instances_produce_unsatisfied_predicates() {
        for (name, code) in [
            ("division", "1 / 2"),
            ("eq_dict", "{a = 1} == {a = 2}"),
            ("min_bool", "min [true]"),
            ("map_dict", r#"map (\x -> x) {a = 1}"#),
        ] {
            let (class, pred_type, expected_ty) = match name {
                "division" => (
                    "Field",
                    Type::builtin(BuiltinTypeId::I32),
                    Some(Type::builtin(BuiltinTypeId::I32)),
                ),
                "eq_dict" => ("Eq", dict_of(Type::builtin(BuiltinTypeId::I32)), None),
                "min_bool" => ("Ord", Type::builtin(BuiltinTypeId::Bool), None),
                "map_dict" => ("Functor", Type::builtin(BuiltinTypeId::Dict), None),
                _ => unreachable!("unknown test case {name}"),
            };

            let expr = parse_expr(code);
            let mut ts = TypeSystem::new_with_prelude().unwrap();
            let (preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
            if let Some(expected) = expected_ty {
                assert_eq!(ty, expected, "{name}");
            }

            let pred = preds
                .iter()
                .find(|p| p.class.as_ref() == class && p.typ == pred_type)
                .unwrap();
            assert!(!entails(&ts.classes, &[], pred).unwrap(), "{name}");
        }
    }

    #[test]
    fn record_update_single_variant_adt_infers() {
        let program = parse_program(
            r#"
            type Foo = Bar { x: i32, y: i32 }
            let
              foo: Foo = Bar { x = 1, y = 2 },
              bar = { foo with { x = 3 } }
            in
              bar
            "#,
        );
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        ts.register_decls(&program.decls).unwrap();
        let (_preds, typ) = infer(&mut ts, program.expr.as_ref()).unwrap();
        assert_eq!(typ.to_string(), "Foo");
    }

    #[test]
    fn record_update_unknown_field_errors() {
        let program = parse_program(
            r#"
            type Foo = Bar { x: i32 }
            let
              foo: Foo = Bar { x = 1 }
            in
              { foo with { y = 2 } }
            "#,
        );
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        ts.register_decls(&program.decls).unwrap();
        let err = infer(&mut ts, program.expr.as_ref()).unwrap_err();
        let err = strip_span(err);
        assert!(matches!(err, TypeError::UnknownField { .. }));
    }

    #[test]
    fn record_update_requires_refined_variant_for_sum_types() {
        let program = parse_program(
            r#"
            type Foo = Bar { x: i32 } | Baz { x: i32 }
            let
              f = \ (foo : Foo) -> { foo with { x = 2 } }
            in
              f (Bar { x = 1 })
            "#,
        );
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        ts.register_decls(&program.decls).unwrap();
        let err = infer(&mut ts, program.expr.as_ref()).unwrap_err();
        let err = strip_span(err);
        assert!(matches!(err, TypeError::FieldNotKnown { .. }));
    }

    #[test]
    fn record_update_allowed_after_match_refines_variant() {
        let program = parse_program(
            r#"
            type Foo = Bar { x: i32 } | Baz { x: i32 }
            let
              f = \ (foo : Foo) ->
                match foo
                  when Bar {x} -> { foo with { x = x + 1 } }
                  when Baz {x} -> { foo with { x = x + 2 } }
            in
              f (Bar { x = 1 })
            "#,
        );
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        ts.register_decls(&program.decls).unwrap();
        let (_preds, typ) = infer(&mut ts, program.expr.as_ref()).unwrap();
        assert_eq!(typ.to_string(), "Foo");
    }

    #[test]
    fn record_update_plain_record_type() {
        let program = parse_program(
            r#"
            let
              f = \ (r : { x: i32, y: i32 }) -> { r with { y = 9 } }
            in
              f { x = 1, y = 2 }
            "#,
        );
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        ts.register_decls(&program.decls).unwrap();
        let (_preds, typ) = infer(&mut ts, program.expr.as_ref()).unwrap();
        assert_eq!(typ.to_string(), "{x: i32, y: i32}");
    }

    #[test]
    fn infer_typed_hole_expr_is_hole_kind() {
        let expr = parse_expr("?");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let (typed, _preds, _ty) = infer_typed(&mut ts, expr.as_ref()).unwrap();
        assert!(
            matches!(typed.kind, TypedExprKind::Hole),
            "typed={typed:#?}"
        );
    }

    #[test]
    fn infer_hole_with_annotation_unifies_to_annotation() {
        let expr = parse_expr("let x : i32 = ? in x");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let (_preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
        assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    }

    #[test]
    fn infer_hole_in_if_condition_is_bool_constrained() {
        let expr = parse_expr("if ? then 1 else 2");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let (_preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
        assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    }

    #[test]
    fn infer_hole_in_arithmetic_is_numeric_constrained() {
        let expr = parse_expr("? + 1");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let (_preds, ty) = infer(&mut ts, expr.as_ref()).unwrap();
        assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    }

    #[test]
    fn infer_hole_arithmetic_conflicting_annotation_failure() {
        let expr = parse_expr("let x : string = (? + 1) in x");
        let mut ts = TypeSystem::new_with_prelude().unwrap();
        let err = strip_span(infer(&mut ts, expr.as_ref()).unwrap_err());
        assert!(matches!(err, TypeError::Unification(_, _)), "err={err:#?}");
    }
}

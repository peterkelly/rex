use crate::{
    error::TypeError,
    types::{BuiltinTypeId, Scheme, Type, TypeKind, TypeVar, TypeVarId, Types},
};
use rexlang_ast::expr::Symbol;
use rexlang_lexer::span::Span;
use rexlang_util::gas::GasMeter;
use rpds::HashTrieMapSync;

pub type Subst = HashTrieMapSync<TypeVarId, Type>;

#[derive(Debug)]
pub(crate) struct Unifier<'g> {
    // `subs[id] = Some(t)` means type variable `id` has been bound to `t`.
    //
    // This is intentionally a dense `Vec` rather than a `BTreeMap`: inference
    // generates `TypeVarId`s from a monotonic counter, so the common case is
    // “small id space, lots of lookups”. This makes the cost model obvious:
    // you pay O(max_id) space, and you get O(1) binds/queries.
    subs: Vec<Option<Type>>,
    gas: Option<&'g mut GasMeter>,
    max_infer_depth: Option<usize>,
    infer_depth: usize,
}

impl<'g> Unifier<'g> {
    pub(crate) fn new(max_infer_depth: Option<usize>) -> Self {
        Self {
            subs: Vec::new(),
            gas: None,
            max_infer_depth,
            infer_depth: 0,
        }
    }

    pub(crate) fn with_gas(gas: &'g mut GasMeter, max_infer_depth: Option<usize>) -> Self {
        Self {
            subs: Vec::new(),
            gas: Some(gas),
            max_infer_depth,
            infer_depth: 0,
        }
    }

    pub(crate) fn with_infer_depth<T>(
        &mut self,
        span: Span,
        f: impl FnOnce(&mut Self) -> Result<T, TypeError>,
    ) -> Result<T, TypeError> {
        if let Some(max) = self.max_infer_depth
            && self.infer_depth >= max
        {
            return Err(TypeError::Spanned {
                span,
                error: Box::new(TypeError::Internal(format!(
                    "maximum inference depth exceeded (max {max})"
                ))),
            });
        }
        self.infer_depth += 1;
        let res = f(self);
        self.infer_depth = self.infer_depth.saturating_sub(1);
        res
    }

    pub(crate) fn charge_infer_node(&mut self) -> Result<(), TypeError> {
        let Some(gas) = self.gas.as_mut() else {
            return Ok(());
        };
        let cost = gas.costs.infer_node;
        gas.charge(cost)?;
        Ok(())
    }

    fn charge_unify_step(&mut self) -> Result<(), TypeError> {
        let Some(gas) = self.gas.as_mut() else {
            return Ok(());
        };
        let cost = gas.costs.unify_step;
        gas.charge(cost)?;
        Ok(())
    }

    fn bind_var(&mut self, id: TypeVarId, ty: Type) {
        if id >= self.subs.len() {
            self.subs.resize(id + 1, None);
        }
        self.subs[id] = Some(ty);
    }

    fn prune(&mut self, ty: &Type) -> Type {
        match ty.as_ref() {
            TypeKind::Var(tv) => {
                let bound = self.subs.get(tv.id).and_then(|t| t.clone());
                match bound {
                    Some(bound) => {
                        let pruned = self.prune(&bound);
                        self.bind_var(tv.id, pruned.clone());
                        pruned
                    }
                    None => ty.clone(),
                }
            }
            TypeKind::Con(_) => ty.clone(),
            TypeKind::App(l, r) => {
                let l = self.prune(l);
                let r = self.prune(r);
                Type::app(l, r)
            }
            TypeKind::Fun(a, b) => {
                let a = self.prune(a);
                let b = self.prune(b);
                Type::fun(a, b)
            }
            TypeKind::Tuple(ts) => {
                Type::new(TypeKind::Tuple(ts.iter().map(|t| self.prune(t)).collect()))
            }
            TypeKind::Record(fields) => Type::new(TypeKind::Record(
                fields
                    .iter()
                    .map(|(name, ty)| (name.clone(), self.prune(ty)))
                    .collect(),
            )),
        }
    }

    pub(crate) fn apply_type(&mut self, ty: &Type) -> Type {
        self.prune(ty)
    }

    fn occurs(&mut self, id: TypeVarId, ty: &Type) -> bool {
        match self.prune(ty).as_ref() {
            TypeKind::Var(tv) => tv.id == id,
            TypeKind::Con(_) => false,
            TypeKind::App(l, r) => self.occurs(id, l) || self.occurs(id, r),
            TypeKind::Fun(a, b) => self.occurs(id, a) || self.occurs(id, b),
            TypeKind::Tuple(ts) => ts.iter().any(|t| self.occurs(id, t)),
            TypeKind::Record(fields) => fields.iter().any(|(_, ty)| self.occurs(id, ty)),
        }
    }

    pub(crate) fn unify(&mut self, t1: &Type, t2: &Type) -> Result<(), TypeError> {
        self.charge_unify_step()?;
        let t1 = self.prune(t1);
        let t2 = self.prune(t2);
        match (t1.as_ref(), t2.as_ref()) {
            (TypeKind::Var(a), TypeKind::Var(b)) if a.id == b.id => Ok(()),
            (TypeKind::Var(tv), other) | (other, TypeKind::Var(tv)) => {
                if self.occurs(tv.id, &Type::new(other.clone())) {
                    Err(TypeError::Occurs(
                        tv.id,
                        Type::new(other.clone()).to_string(),
                    ))
                } else {
                    self.bind_var(tv.id, Type::new(other.clone()));
                    Ok(())
                }
            }
            (TypeKind::Con(c1), TypeKind::Con(c2)) if c1 == c2 => Ok(()),
            (TypeKind::App(l1, r1), TypeKind::App(l2, r2)) => {
                self.unify(l1, l2)?;
                self.unify(r1, r2)
            }
            (TypeKind::Fun(a1, b1), TypeKind::Fun(a2, b2)) => {
                self.unify(a1, a2)?;
                self.unify(b1, b2)
            }
            (TypeKind::Tuple(ts1), TypeKind::Tuple(ts2)) => {
                if ts1.len() != ts2.len() {
                    return Err(TypeError::Unification(t1.to_string(), t2.to_string()));
                }
                for (a, b) in ts1.iter().zip(ts2.iter()) {
                    self.unify(a, b)?;
                }
                Ok(())
            }
            (TypeKind::Record(f1), TypeKind::Record(f2)) => {
                if f1.len() != f2.len() {
                    return Err(TypeError::Unification(t1.to_string(), t2.to_string()));
                }
                for ((n1, t1), (n2, t2)) in f1.iter().zip(f2.iter()) {
                    if n1 != n2 {
                        return Err(TypeError::Unification(t1.to_string(), t2.to_string()));
                    }
                    self.unify(t1, t2)?;
                }
                Ok(())
            }
            (TypeKind::Record(fields), TypeKind::App(head, arg))
            | (TypeKind::App(head, arg), TypeKind::Record(fields)) => match head.as_ref() {
                TypeKind::Con(c) if c.builtin_id == Some(BuiltinTypeId::Dict) => {
                    let elem_ty = record_elem_type_unifier(fields, self)?;
                    self.unify(arg, &elem_ty)
                }
                TypeKind::Var(tv) => {
                    self.unify(
                        &Type::new(TypeKind::Var(tv.clone())),
                        &Type::builtin(BuiltinTypeId::Dict),
                    )?;
                    let elem_ty = record_elem_type_unifier(fields, self)?;
                    self.unify(arg, &elem_ty)
                }
                _ => Err(TypeError::Unification(t1.to_string(), t2.to_string())),
            },
            _ => Err(TypeError::Unification(t1.to_string(), t2.to_string())),
        }
    }

    pub(crate) fn into_subst(mut self) -> Subst {
        let mut out = Subst::new_sync();
        for id in 0..self.subs.len() {
            if let Some(ty) = self.subs[id].clone() {
                let pruned = self.prune(&ty);
                out = out.insert(id, pruned);
            }
        }
        out
    }
}

/// Compose substitutions `a` after `b`.
///
/// If `t.apply(&b)` is “apply `b` first”, then:
/// `t.apply(&compose_subst(a, b)) == t.apply(&b).apply(&a)`.
pub fn compose_subst(a: Subst, b: Subst) -> Subst {
    if subst_is_empty(&a) {
        return b;
    }
    if subst_is_empty(&b) {
        return a;
    }
    let mut res = Subst::new_sync();
    for (k, v) in b.iter() {
        res = res.insert(*k, v.apply(&a));
    }
    for (k, v) in a.iter() {
        res = res.insert(*k, v.clone());
    }
    res
}

pub(crate) fn subst_is_empty(s: &Subst) -> bool {
    s.iter().next().is_none()
}

pub(crate) fn scheme_compatible(existing: &Scheme, declared: &Scheme) -> bool {
    let s = match unify(&existing.typ, &declared.typ) {
        Ok(s) => s,
        Err(_) => return false,
    };

    let existing_preds = existing.preds.apply(&s);
    let declared_preds = declared.preds.apply(&s);

    let mut lhs: Vec<(Symbol, String)> = existing_preds
        .iter()
        .map(|p| (p.class.clone(), p.typ.to_string()))
        .collect();
    let mut rhs: Vec<(Symbol, String)> = declared_preds
        .iter()
        .map(|p| (p.class.clone(), p.typ.to_string()))
        .collect();
    lhs.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    rhs.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    lhs == rhs
}

fn record_elem_type_unifier(
    fields: &[(Symbol, Type)],
    unifier: &mut Unifier<'_>,
) -> Result<Type, TypeError> {
    let mut iter = fields.iter();
    let first = match iter.next() {
        Some((_, ty)) => ty.clone(),
        None => return Err(TypeError::UnsupportedExpr("empty record")),
    };
    for (_, ty) in iter {
        unifier.unify(&first, ty)?;
    }
    Ok(unifier.apply_type(&first))
}

pub(crate) fn bind(tv: &TypeVar, t: &Type) -> Result<Subst, TypeError> {
    if let TypeKind::Var(var) = t.as_ref()
        && var.id == tv.id
    {
        return Ok(Subst::new_sync());
    }
    if t.ftv().contains(&tv.id) {
        Err(TypeError::Occurs(tv.id, t.to_string()))
    } else {
        Ok(Subst::new_sync().insert(tv.id, t.clone()))
    }
}

fn record_elem_type(fields: &[(Symbol, Type)]) -> Result<(Subst, Type), TypeError> {
    let mut iter = fields.iter();
    let first = match iter.next() {
        Some((_, ty)) => ty.clone(),
        None => return Err(TypeError::UnsupportedExpr("empty record")),
    };
    let mut subst = Subst::new_sync();
    let mut current = first;
    for (_, ty) in iter {
        let s_next = unify(&current.apply(&subst), &ty.apply(&subst))?;
        subst = compose_subst(s_next, subst);
        current = current.apply(&subst);
    }
    Ok((subst.clone(), current.apply(&subst)))
}

/// Compute a most-general unifier for two types.
///
/// This is the “pure” unifier: it returns an explicit substitution map and is
/// easy to read/compose in isolation. The type inference engine uses `Unifier`
/// directly to avoid allocating and composing persistent maps at every
/// unification step.
pub fn unify(t1: &Type, t2: &Type) -> Result<Subst, TypeError> {
    match (t1.as_ref(), t2.as_ref()) {
        (TypeKind::Fun(l1, r1), TypeKind::Fun(l2, r2)) => {
            let s1 = unify(l1, l2)?;
            let s2 = unify(&r1.apply(&s1), &r2.apply(&s1))?;
            Ok(compose_subst(s2, s1))
        }
        (TypeKind::Record(f1), TypeKind::Record(f2)) => {
            if f1.len() != f2.len() {
                return Err(TypeError::Unification(t1.to_string(), t2.to_string()));
            }
            let mut subst = Subst::new_sync();
            for ((n1, t1), (n2, t2)) in f1.iter().zip(f2.iter()) {
                if n1 != n2 {
                    return Err(TypeError::Unification(t1.to_string(), t2.to_string()));
                }
                let s_next = unify(&t1.apply(&subst), &t2.apply(&subst))?;
                subst = compose_subst(s_next, subst);
            }
            Ok(subst)
        }
        (TypeKind::Record(fields), TypeKind::App(head, arg))
        | (TypeKind::App(head, arg), TypeKind::Record(fields)) => match head.as_ref() {
            TypeKind::Con(c) if c.builtin_id == Some(BuiltinTypeId::Dict) => {
                let (s_fields, elem_ty) = record_elem_type(fields)?;
                let s_arg = unify(&arg.apply(&s_fields), &elem_ty)?;
                Ok(compose_subst(s_arg, s_fields))
            }
            TypeKind::Var(tv) => {
                let s_head = bind(tv, &Type::builtin(BuiltinTypeId::Dict))?;
                let arg = arg.apply(&s_head);
                let (s_fields, elem_ty) = record_elem_type(fields)?;
                let s_arg = unify(&arg.apply(&s_fields), &elem_ty)?;
                Ok(compose_subst(s_arg, compose_subst(s_fields, s_head)))
            }
            _ => Err(TypeError::Unification(t1.to_string(), t2.to_string())),
        },
        (TypeKind::App(l1, r1), TypeKind::App(l2, r2)) => {
            let s1 = unify(l1, l2)?;
            let s2 = unify(&r1.apply(&s1), &r2.apply(&s1))?;
            Ok(compose_subst(s2, s1))
        }
        (TypeKind::Tuple(ts1), TypeKind::Tuple(ts2)) => {
            if ts1.len() != ts2.len() {
                return Err(TypeError::Unification(t1.to_string(), t2.to_string()));
            }
            let mut s = Subst::new_sync();
            for (a, b) in ts1.iter().zip(ts2.iter()) {
                let s_next = unify(&a.apply(&s), &b.apply(&s))?;
                s = compose_subst(s_next, s);
            }
            Ok(s)
        }
        (TypeKind::Var(tv), t) | (t, TypeKind::Var(tv)) => bind(tv, &Type::new(t.clone())),
        (TypeKind::Con(c1), TypeKind::Con(c2)) if c1 == c2 => Ok(Subst::new_sync()),
        _ => Err(TypeError::Unification(t1.to_string(), t2.to_string())),
    }
}

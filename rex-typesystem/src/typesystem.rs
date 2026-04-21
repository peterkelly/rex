//! Core type system implementation for Rex.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use rex_ast::expr::{
    ClassDecl, ClassMethodSig, Decl, DeclareFnDecl, Expr, FnDecl, InstanceDecl, InstanceMethodImpl,
    Scope, Symbol, TypeConstraint, TypeDecl, TypeExpr, sym,
};
use rex_lexer::span::Span;

use crate::prelude;

pub use crate::{
    // inference::{
    //     infer,
    //     infer_typed,
    //     infer_with_gas,
    //     infer_typed_with_gas,
    // },
    unification::{Subst, compose_subst, unify},
};

use crate::{
    error::TypeError,
    inference::infer_typed,
    types::{
        AdtDecl, BuiltinTypeId, ClassEnv, Instance, Predicate, Scheme, Type, TypeEnv, TypeKind,
        TypeVar, TypeVarId, TypedExpr, Types,
    },
    unification::scheme_compatible,
};

fn format_constraints_referencing_vars(preds: &[Predicate], vars: &[TypeVarId]) -> String {
    if vars.is_empty() {
        return String::new();
    }
    let var_set: BTreeSet<TypeVarId> = vars.iter().copied().collect();
    let mut parts = Vec::new();
    for pred in preds {
        let ftv = pred.ftv();
        if ftv.iter().any(|v| var_set.contains(v)) {
            parts.push(format!("{} {}", pred.class, pred.typ));
        }
    }
    if parts.is_empty() {
        // Fallback: show all constraints if the filtering logic misses something.
        for pred in preds {
            parts.push(format!("{} {}", pred.class, pred.typ));
        }
    }
    parts.join(", ")
}

pub(crate) fn reject_ambiguous_scheme(scheme: &Scheme) -> Result<(), TypeError> {
    // Only reject *quantified* ambiguous variables. Variables free in the
    // environment are allowed to appear only in predicates, since they can be
    // determined by outer context.
    let quantified: BTreeSet<TypeVarId> = scheme.vars.iter().map(|v| v.id).collect();
    if quantified.is_empty() {
        return Ok(());
    }

    let typ_ftv = scheme.typ.ftv();
    let mut vars = BTreeSet::new();
    for pred in &scheme.preds {
        let TypeKind::Var(tv) = pred.typ.as_ref() else {
            continue;
        };
        if quantified.contains(&tv.id) && !typ_ftv.contains(&tv.id) {
            vars.insert(tv.id);
        }
    }

    if vars.is_empty() {
        return Ok(());
    }
    let mut vars: Vec<TypeVarId> = vars.into_iter().collect();
    vars.sort_unstable();
    let constraints = format_constraints_referencing_vars(&scheme.preds, &vars);
    Err(TypeError::AmbiguousTypeVars { vars, constraints })
}

#[derive(Clone, Copy, Debug)]
pub struct TypeSystemLimits {
    pub max_infer_depth: Option<usize>,
}

impl TypeSystemLimits {
    pub fn unlimited() -> Self {
        Self {
            max_infer_depth: None,
        }
    }

    pub fn safe_defaults() -> Self {
        Self {
            max_infer_depth: Some(4096),
        }
    }
}

impl Default for TypeSystemLimits {
    fn default() -> Self {
        Self::safe_defaults()
    }
}

fn superclass_closure(class_env: &ClassEnv, given: &[Predicate]) -> Vec<Predicate> {
    let mut closure: Vec<Predicate> = given.to_vec();
    let mut i = 0;
    while i < closure.len() {
        let p = closure[i].clone();
        for sup in class_env.supers_of(&p.class) {
            closure.push(Predicate::new(sup, p.typ.clone()));
        }
        i += 1;
    }
    closure
}

fn check_non_ground_predicates_declared(
    class_env: &ClassEnv,
    declared: &[Predicate],
    inferred: &[Predicate],
) -> Result<(), TypeError> {
    // Compare by a stable, user-facing rendering (`Default a`, `Foldable t`, ...),
    // rather than `TypeVarId`, so signature variables that only appear in
    // predicates (and thus aren't related by unification) still match up.
    let closure = superclass_closure(class_env, declared);
    let closure_keys: BTreeSet<String> = closure
        .iter()
        .map(|p| format!("{} {}", p.class, p.typ))
        .collect();
    let mut missing = Vec::new();
    for pred in inferred {
        if pred.typ.ftv().is_empty() {
            continue;
        }
        let key = format!("{} {}", pred.class, pred.typ);
        if !closure_keys.contains(&key) {
            missing.push(key);
        }
    }

    missing.sort();
    missing.dedup();
    if missing.is_empty() {
        return Ok(());
    }
    Err(TypeError::MissingConstraints {
        constraints: missing.join(", "),
    })
}

fn type_term_remaining_arity(ty: &Type) -> Option<usize> {
    match ty.as_ref() {
        TypeKind::Var(_) => None,
        TypeKind::Con(tc) => Some(tc.arity),
        TypeKind::App(l, _) => {
            let a = type_term_remaining_arity(l)?;
            Some(a.saturating_sub(1))
        }
        TypeKind::Fun(..) | TypeKind::Tuple(..) | TypeKind::Record(..) => Some(0),
    }
}

fn max_head_app_arity_for_var(ty: &Type, var_id: TypeVarId) -> usize {
    let mut max_arity = 0usize;
    let mut stack: Vec<&Type> = vec![ty];
    while let Some(t) = stack.pop() {
        match t.as_ref() {
            TypeKind::Var(_) | TypeKind::Con(_) => {}
            TypeKind::App(l, r) => {
                // Record the full application depth at this node.
                let mut head = t;
                let mut args = 0usize;
                while let TypeKind::App(left, _) = head.as_ref() {
                    args += 1;
                    head = left;
                }
                if let TypeKind::Var(tv) = head.as_ref()
                    && tv.id == var_id
                {
                    max_arity = max_arity.max(args);
                }
                stack.push(l);
                stack.push(r);
            }
            TypeKind::Fun(a, b) => {
                stack.push(a);
                stack.push(b);
            }
            TypeKind::Tuple(ts) => {
                for t in ts {
                    stack.push(t);
                }
            }
            TypeKind::Record(fields) => {
                for (_, t) in fields {
                    stack.push(t);
                }
            }
        }
    }
    max_arity
}

#[derive(Default, Debug, Clone)]
pub struct TypeVarSupply {
    counter: TypeVarId,
}

impl TypeVarSupply {
    pub fn new() -> Self {
        Self { counter: 0 }
    }

    pub fn fresh(&mut self, name_hint: impl Into<Option<Symbol>>) -> TypeVar {
        let tv = TypeVar::new(self.counter, name_hint.into());
        self.counter += 1;
        tv
    }
}

pub(crate) fn is_integral_literal_expr(expr: &Expr) -> bool {
    matches!(expr, Expr::Int(..) | Expr::Uint(..))
}

/// Turn a monotype `typ` (plus constraints `preds`) into a polymorphic `Scheme`
/// by quantifying over the type variables not free in `env`.
pub fn generalize(env: &TypeEnv, preds: Vec<Predicate>, typ: Type) -> Scheme {
    let mut vars: Vec<TypeVar> = typ
        .ftv()
        .union(&preds.ftv())
        .copied()
        .collect::<BTreeSet<_>>()
        .difference(&env.ftv())
        .cloned()
        .map(|id| TypeVar::new(id, None))
        .collect();
    vars.sort_by_key(|v| v.id);
    Scheme::new(vars, preds, typ)
}

pub fn instantiate(scheme: &Scheme, supply: &mut TypeVarSupply) -> (Vec<Predicate>, Type) {
    // Instantiate replaces all quantified variables with fresh unification
    // variables, preserving the original name as a debugging hint.
    let mut subst = Subst::new_sync();
    for v in &scheme.vars {
        subst = subst.insert(v.id, Type::var(supply.fresh(v.name.clone())));
    }
    (scheme.preds.apply(&subst), scheme.typ.apply(&subst))
}

pub fn entails(
    class_env: &ClassEnv,
    given: &[Predicate],
    pred: &Predicate,
) -> Result<bool, TypeError> {
    // Expand given with superclasses.
    let mut closure: Vec<Predicate> = given.to_vec();
    let mut i = 0;
    while i < closure.len() {
        let p = closure[i].clone();
        for sup in class_env.supers_of(&p.class) {
            closure.push(Predicate::new(sup, p.typ.clone()));
        }
        i += 1;
    }

    if closure
        .iter()
        .any(|p| p.class == pred.class && p.typ == pred.typ)
    {
        return Ok(true);
    }

    if !class_env.classes.contains_key(&pred.class) {
        return Err(TypeError::UnknownClass(pred.class.clone()));
    }

    if let Some(instances) = class_env.instances.get(&pred.class) {
        for inst in instances {
            if let Ok(s) = unify(&inst.head.typ, &pred.typ) {
                let ctx = inst.context.apply(&s);
                if ctx
                    .iter()
                    .all(|c| entails(class_env, &closure, c).unwrap_or(false))
                {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

#[derive(Default, Debug, Clone)]
pub struct TypeSystem {
    pub env: TypeEnv,
    pub classes: ClassEnv,
    pub adts: BTreeMap<Symbol, AdtDecl>,
    pub class_info: BTreeMap<Symbol, ClassInfo>,
    pub class_methods: BTreeMap<Symbol, ClassMethodInfo>,
    /// Names introduced by `declare fn` (forward declarations).
    ///
    /// These are placeholders in the type environment and must not block a later
    /// real definition (e.g. `fn foo = ...` or host/CLI injection).
    pub declared_values: BTreeSet<Symbol>,
    pub supply: TypeVarSupply,
    pub limits: TypeSystemLimits,
}

/// Semantic information about a type class declaration, derived from Rex source.
///
/// Design notes (WARM):
/// - We keep this explicit and data-oriented: it makes review easy and keeps costs visible.
/// - Rex represents multi-parameter classes by encoding the parameters as a tuple in the
///   single `Predicate.typ` slot. For a unary class `C a` the predicate is `C a`. For a
///   binary class `C t a` the predicate is `C (t, a)`, etc.
/// - This keeps the runtime/type-inference machinery simple: instance matching is still
///   “unify the predicate types”, and no separate arity tracking is needed.
#[derive(Clone, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub struct ClassInfo {
    pub name: Symbol,
    pub params: Vec<Symbol>,
    pub supers: Vec<Symbol>,
    pub methods: BTreeMap<Symbol, Scheme>,
}

#[derive(Clone, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub struct ClassMethodInfo {
    pub class: Symbol,
    pub scheme: Scheme,
}

#[derive(Clone, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub struct PreparedInstanceDecl {
    pub span: Span,
    pub class: Symbol,
    pub head: Type,
    pub context: Vec<Predicate>,
}

impl TypeSystem {
    pub fn new() -> Self {
        Self {
            env: TypeEnv::new(),
            classes: ClassEnv::new(),
            adts: BTreeMap::new(),
            class_info: BTreeMap::new(),
            class_methods: BTreeMap::new(),
            declared_values: BTreeSet::new(),
            supply: TypeVarSupply::new(),
            limits: TypeSystemLimits::default(),
        }
    }

    pub fn fresh_type_var(&mut self, name: Option<Symbol>) -> TypeVar {
        self.supply.fresh(name)
    }

    pub fn set_limits(&mut self, limits: TypeSystemLimits) {
        self.limits = limits;
    }

    pub fn new_with_prelude() -> Result<Self, TypeError> {
        let mut ts = TypeSystem::new();
        prelude::build_prelude(&mut ts)?;
        Ok(ts)
    }

    fn register_decl(&mut self, decl: &Decl) -> Result<(), TypeError> {
        match decl {
            Decl::Type(ty) => self.register_type_decl(ty),
            Decl::Class(class_decl) => self.register_class_decl(class_decl),
            Decl::Instance(inst_decl) => {
                let _ = self.register_instance_decl(inst_decl)?;
                Ok(())
            }
            Decl::Fn(fd) => self.register_fn_decls(std::slice::from_ref(fd)),
            Decl::DeclareFn(fd) => self.inject_declare_fn_decl(fd),
            Decl::Import(..) => Ok(()),
        }
    }

    pub fn register_decls(&mut self, decls: &[Decl]) -> Result<(), TypeError> {
        let mut pending_fns: Vec<FnDecl> = Vec::new();
        for decl in decls {
            if let Decl::Fn(fd) = decl {
                pending_fns.push(fd.clone());
                continue;
            }

            if !pending_fns.is_empty() {
                self.register_fn_decls(&pending_fns)?;
                pending_fns.clear();
            }

            self.register_decl(decl)?;
        }
        if !pending_fns.is_empty() {
            self.register_fn_decls(&pending_fns)?;
        }
        Ok(())
    }

    pub fn add_value(&mut self, name: impl AsRef<str>, scheme: Scheme) {
        let name = sym(name.as_ref());
        self.declared_values.remove(&name);
        self.env.extend(name, scheme);
    }

    pub fn add_overload(&mut self, name: impl AsRef<str>, scheme: Scheme) {
        let name = sym(name.as_ref());
        self.declared_values.remove(&name);
        self.env.extend_overload(name, scheme);
    }

    pub fn register_instance(&mut self, class: impl AsRef<str>, inst: Instance) {
        self.classes.add_instance(sym(class.as_ref()), inst);
    }

    pub fn register_class_decl(&mut self, decl: &ClassDecl) -> Result<(), TypeError> {
        let span = decl.span;
        (|| {
            // Classes are global, and Rex does not support reopening/merging them.
            // Allowing that would be a long-term maintenance hazard: it creates
            // spooky-action-at-a-distance across modules and makes reviews harder.
            if self.class_info.contains_key(&decl.name)
                || self.classes.classes.contains_key(&decl.name)
            {
                return Err(TypeError::DuplicateClass(decl.name.clone()));
            }
            if decl.params.is_empty() {
                return Err(TypeError::InvalidClassArity {
                    class: decl.name.clone(),
                    got: decl.params.len(),
                });
            }
            let params = decl.params.clone();

            // Register the superclass relationships in the class environment.
            //
            // We only accept `<= C param` style superclasses for now. Anything
            // fancier would require storing type-level relationships in `ClassEnv`,
            // which Rex does not currently model.
            let mut supers = Vec::with_capacity(decl.supers.len());
            if !decl.supers.is_empty() && params.len() != 1 {
                return Err(TypeError::UnsupportedExpr(
                    "multi-parameter classes cannot declare superclasses yet",
                ));
            }
            for sup in &decl.supers {
                let mut vars = BTreeMap::new();
                let param = params[0].clone();
                let param_tv = self.supply.fresh(Some(param.clone()));
                vars.insert(param, param_tv.clone());
                let sup_ty = type_from_annotation_expr_vars(
                    &self.adts,
                    &sup.typ,
                    &mut vars,
                    &mut self.supply,
                )?;
                if sup_ty != Type::var(param_tv) {
                    return Err(TypeError::UnsupportedExpr(
                        "superclass constraints must be of the form `<= C a`",
                    ));
                }
                supers.push(sup.class.to_dotted_symbol());
            }

            self.classes.add_class(decl.name.clone(), supers.clone());

            let mut methods = BTreeMap::new();
            for ClassMethodSig { name, typ } in &decl.methods {
                if self.env.lookup(name).is_some() || self.class_methods.contains_key(name) {
                    return Err(TypeError::DuplicateClassMethod(name.clone()));
                }

                let mut vars: BTreeMap<Symbol, TypeVar> = BTreeMap::new();
                let mut param_tvs: Vec<TypeVar> = Vec::with_capacity(params.len());
                for param in &params {
                    let tv = self.supply.fresh(Some(param.clone()));
                    vars.insert(param.clone(), tv.clone());
                    param_tvs.push(tv);
                }

                let ty =
                    type_from_annotation_expr_vars(&self.adts, typ, &mut vars, &mut self.supply)?;

                let mut scheme_vars: Vec<TypeVar> = vars.values().cloned().collect();
                scheme_vars.sort_by_key(|tv| tv.id);
                scheme_vars.dedup_by_key(|tv| tv.id);

                let class_pred = Predicate {
                    class: decl.name.clone(),
                    typ: if param_tvs.len() == 1 {
                        Type::var(param_tvs[0].clone())
                    } else {
                        Type::tuple(param_tvs.into_iter().map(Type::var).collect())
                    },
                };
                let scheme = Scheme::new(scheme_vars, vec![class_pred], ty);

                self.env.extend(name.clone(), scheme.clone());
                self.class_methods.insert(
                    name.clone(),
                    ClassMethodInfo {
                        class: decl.name.clone(),
                        scheme: scheme.clone(),
                    },
                );
                methods.insert(name.clone(), scheme);
            }

            self.class_info.insert(
                decl.name.clone(),
                ClassInfo {
                    name: decl.name.clone(),
                    params,
                    supers,
                    methods,
                },
            );
            Ok(())
        })()
        .map_err(|err| err.with_span(&span))
    }

    pub fn register_instance_decl(
        &mut self,
        decl: &InstanceDecl,
    ) -> Result<PreparedInstanceDecl, TypeError> {
        let span = decl.span;
        (|| {
            let class = decl.class.clone();
            if !self.class_info.contains_key(&class) && !self.classes.classes.contains_key(&class) {
                return Err(TypeError::UnknownClass(class));
            }

            let mut vars: BTreeMap<Symbol, TypeVar> = BTreeMap::new();
            let head = type_from_annotation_expr_vars(
                &self.adts,
                &decl.head,
                &mut vars,
                &mut self.supply,
            )?;
            let context = predicates_from_constraints(
                &self.adts,
                &decl.context,
                &mut vars,
                &mut self.supply,
            )?;

            let inst = Instance::new(
                context.clone(),
                Predicate {
                    class: decl.class.clone(),
                    typ: head.clone(),
                },
            );

            // Validate method list against the class declaration if present.
            if let Some(info) = self.class_info.get(&decl.class) {
                for method in &decl.methods {
                    if !info.methods.contains_key(&method.name) {
                        return Err(TypeError::UnknownInstanceMethod {
                            class: decl.class.clone(),
                            method: method.name.clone(),
                        });
                    }
                }
                for method_name in info.methods.keys() {
                    if !decl.methods.iter().any(|m| &m.name == method_name) {
                        return Err(TypeError::MissingInstanceMethod {
                            class: decl.class.clone(),
                            method: method_name.clone(),
                        });
                    }
                }
            }

            self.classes.add_instance(decl.class.clone(), inst);
            Ok(PreparedInstanceDecl {
                span,
                class: decl.class.clone(),
                head,
                context,
            })
        })()
        .map_err(|err| err.with_span(&span))
    }

    pub fn prepare_instance_decl(
        &mut self,
        decl: &InstanceDecl,
    ) -> Result<PreparedInstanceDecl, TypeError> {
        let span = decl.span;
        (|| {
            let class = decl.class.clone();
            if !self.class_info.contains_key(&class) && !self.classes.classes.contains_key(&class) {
                return Err(TypeError::UnknownClass(class));
            }

            let mut vars: BTreeMap<Symbol, TypeVar> = BTreeMap::new();
            let head = type_from_annotation_expr_vars(
                &self.adts,
                &decl.head,
                &mut vars,
                &mut self.supply,
            )?;
            let context = predicates_from_constraints(
                &self.adts,
                &decl.context,
                &mut vars,
                &mut self.supply,
            )?;

            // Validate method list against the class declaration if present.
            if let Some(info) = self.class_info.get(&decl.class) {
                for method in &decl.methods {
                    if !info.methods.contains_key(&method.name) {
                        return Err(TypeError::UnknownInstanceMethod {
                            class: decl.class.clone(),
                            method: method.name.clone(),
                        });
                    }
                }
                for method_name in info.methods.keys() {
                    if !decl.methods.iter().any(|m| &m.name == method_name) {
                        return Err(TypeError::MissingInstanceMethod {
                            class: decl.class.clone(),
                            method: method_name.clone(),
                        });
                    }
                }
            }

            Ok(PreparedInstanceDecl {
                span,
                class: decl.class.clone(),
                head,
                context,
            })
        })()
        .map_err(|err| err.with_span(&span))
    }

    pub fn register_fn_decls(&mut self, decls: &[FnDecl]) -> Result<(), TypeError> {
        if decls.is_empty() {
            return Ok(());
        }

        let saved_env = self.env.clone();
        let saved_declared = self.declared_values.clone();

        let result: Result<(), TypeError> = (|| {
            #[derive(Clone)]
            struct FnInfo {
                decl: FnDecl,
                expected: Type,
                declared_preds: Vec<Predicate>,
                scheme: Scheme,
                ann_vars: BTreeMap<Symbol, TypeVar>,
            }

            let mut infos: Vec<FnInfo> = Vec::with_capacity(decls.len());
            let mut seen_names = BTreeSet::new();

            for decl in decls {
                let span = decl.span;
                let info = (|| {
                    let name = &decl.name.name;
                    if !seen_names.insert(name.clone()) {
                        return Err(TypeError::DuplicateValue(name.clone()));
                    }

                    if self.env.lookup(name).is_some() {
                        if self.declared_values.remove(name) {
                            // A forward declaration should not block the real definition.
                            self.env.remove(name);
                        } else {
                            return Err(TypeError::DuplicateValue(name.clone()));
                        }
                    }

                    let mut sig = decl.ret.clone();
                    for (_, ann) in decl.params.iter().rev() {
                        let span = Span::from_begin_end(ann.span().begin, sig.span().end);
                        sig = TypeExpr::Fun(span, Box::new(ann.clone()), Box::new(sig));
                    }

                    let mut ann_vars: BTreeMap<Symbol, TypeVar> = BTreeMap::new();
                    let expected = type_from_annotation_expr_vars(
                        &self.adts,
                        &sig,
                        &mut ann_vars,
                        &mut self.supply,
                    )?;
                    let declared_preds = predicates_from_constraints(
                        &self.adts,
                        &decl.constraints,
                        &mut ann_vars,
                        &mut self.supply,
                    )?;

                    // Validate that declared constraints are well-formed.
                    let var_arities: BTreeMap<TypeVarId, usize> = ann_vars
                        .values()
                        .map(|tv| (tv.id, max_head_app_arity_for_var(&expected, tv.id)))
                        .collect();
                    for pred in &declared_preds {
                        let _ = entails(&self.classes, &[], pred)?;
                        let Some(expected_arities) = self.expected_class_param_arities(&pred.class)
                        else {
                            continue;
                        };
                        let args: Vec<Type> = if expected_arities.len() == 1 {
                            vec![pred.typ.clone()]
                        } else if let TypeKind::Tuple(parts) = pred.typ.as_ref() {
                            if parts.len() != expected_arities.len() {
                                continue;
                            }
                            parts.clone()
                        } else {
                            continue;
                        };

                        for (arg, expected_arity) in
                            args.iter().zip(expected_arities.iter().copied())
                        {
                            let got =
                                type_term_remaining_arity(arg).or_else(|| match arg.as_ref() {
                                    TypeKind::Var(tv) => var_arities.get(&tv.id).copied(),
                                    _ => None,
                                });
                            let Some(got) = got else {
                                continue;
                            };
                            if got != expected_arity {
                                return Err(TypeError::KindMismatch {
                                    class: pred.class.clone(),
                                    expected: expected_arity,
                                    got,
                                    typ: arg.to_string(),
                                });
                            }
                        }
                    }

                    let mut vars: Vec<TypeVar> = ann_vars.values().cloned().collect();
                    vars.sort_by_key(|v| v.id);
                    let scheme = Scheme::new(vars, declared_preds.clone(), expected.clone());
                    reject_ambiguous_scheme(&scheme)?;

                    Ok(FnInfo {
                        decl: decl.clone(),
                        expected,
                        declared_preds,
                        scheme,
                        ann_vars,
                    })
                })();

                infos.push(info.map_err(|err| err.with_span(&span))?);
            }

            // Seed environment with all declared signatures first so fn bodies
            // can reference each other recursively (let-rec semantics).
            for info in &infos {
                self.env
                    .extend(info.decl.name.name.clone(), info.scheme.clone());
            }

            for info in infos {
                let span = info.decl.span;
                let mut lam_body = info.decl.body.clone();
                let mut lam_end = lam_body.span().end;
                for (param, ann) in info.decl.params.iter().rev() {
                    let lam_constraints = Vec::new();
                    let span = Span::from_begin_end(param.span.begin, lam_end);
                    lam_body = Arc::new(Expr::Lam(
                        span,
                        Scope::new_sync(),
                        param.clone(),
                        Some(ann.clone()),
                        lam_constraints,
                        lam_body,
                    ));
                    lam_end = lam_body.span().end;
                }

                let (typed, preds, inferred) = infer_typed(self, lam_body.as_ref())?;
                let s = unify(&inferred, &info.expected)?;
                let preds = preds.apply(&s);
                let inferred = inferred.apply(&s);
                let declared_preds = info.declared_preds.apply(&s);
                let expected = info.expected.apply(&s);

                // Keep kind checks aligned with existing `inject_fn_decl` logic.
                let var_arities: BTreeMap<TypeVarId, usize> = info
                    .ann_vars
                    .values()
                    .map(|tv| (tv.id, max_head_app_arity_for_var(&expected, tv.id)))
                    .collect();
                for pred in &declared_preds {
                    let _ = entails(&self.classes, &[], pred)?;
                    let Some(expected_arities) = self.expected_class_param_arities(&pred.class)
                    else {
                        continue;
                    };
                    let args: Vec<Type> = if expected_arities.len() == 1 {
                        vec![pred.typ.clone()]
                    } else if let TypeKind::Tuple(parts) = pred.typ.as_ref() {
                        if parts.len() != expected_arities.len() {
                            continue;
                        }
                        parts.clone()
                    } else {
                        continue;
                    };

                    for (arg, expected_arity) in args.iter().zip(expected_arities.iter().copied()) {
                        let got = type_term_remaining_arity(arg).or_else(|| match arg.as_ref() {
                            TypeKind::Var(tv) => var_arities.get(&tv.id).copied(),
                            _ => None,
                        });
                        let Some(got) = got else {
                            continue;
                        };
                        if got != expected_arity {
                            let err = TypeError::KindMismatch {
                                class: pred.class.clone(),
                                expected: expected_arity,
                                got,
                                typ: arg.to_string(),
                            };
                            return Err(err.with_span(&span));
                        }
                    }
                }

                check_non_ground_predicates_declared(&self.classes, &declared_preds, &preds)
                    .map_err(|err| err.with_span(&span))?;

                let _ = inferred;
                let _ = typed;
            }

            Ok(())
        })();

        if result.is_err() {
            self.env = saved_env;
            self.declared_values = saved_declared;
        }
        result
    }

    pub fn inject_declare_fn_decl(&mut self, decl: &DeclareFnDecl) -> Result<(), TypeError> {
        let span = decl.span;
        (|| {
            // Build the declared signature type.
            let mut sig = decl.ret.clone();
            for (_, ann) in decl.params.iter().rev() {
                let span = Span::from_begin_end(ann.span().begin, sig.span().end);
                sig = TypeExpr::Fun(span, Box::new(ann.clone()), Box::new(sig));
            }

            let mut ann_vars: BTreeMap<Symbol, TypeVar> = BTreeMap::new();
            let expected =
                type_from_annotation_expr_vars(&self.adts, &sig, &mut ann_vars, &mut self.supply)?;
            let declared_preds = predicates_from_constraints(
                &self.adts,
                &decl.constraints,
                &mut ann_vars,
                &mut self.supply,
            )?;

            let mut vars: Vec<TypeVar> = ann_vars.values().cloned().collect();
            vars.sort_by_key(|v| v.id);
            let scheme = Scheme::new(vars, declared_preds, expected);
            reject_ambiguous_scheme(&scheme)?;

            // Validate referenced classes exist (and are spelled correctly).
            for pred in &scheme.preds {
                let _ = entails(&self.classes, &[], pred)?;
            }

            let name = &decl.name.name;

            // If there is already a real definition (prelude/host/`fn`), treat
            // `declare fn` as documentation only and ignore it.
            if self.env.lookup(name).is_some() && !self.declared_values.contains(name) {
                return Ok(());
            }

            if let Some(existing) = self.env.lookup(name) {
                if existing.iter().any(|s| scheme_compatible(s, &scheme)) {
                    return Ok(());
                }
                return Err(TypeError::DuplicateValue(decl.name.name.clone()));
            }

            self.env.extend(decl.name.name.clone(), scheme);
            self.declared_values.insert(decl.name.name.clone());
            Ok(())
        })()
        .map_err(|err| err.with_span(&span))
    }

    pub fn instantiate_class_method_for_head(
        &mut self,
        class: &Symbol,
        method: &Symbol,
        head: &Type,
    ) -> Result<Type, TypeError> {
        let info = self
            .class_info
            .get(class)
            .ok_or_else(|| TypeError::UnknownClass(class.clone()))?;
        let scheme = info
            .methods
            .get(method)
            .ok_or_else(|| TypeError::UnknownInstanceMethod {
                class: class.clone(),
                method: method.clone(),
            })?;

        let (preds, typ) = instantiate(scheme, &mut self.supply);
        let class_pred =
            preds
                .iter()
                .find(|p| &p.class == class)
                .ok_or(TypeError::UnsupportedExpr(
                    "class method scheme missing class predicate",
                ))?;
        let s = unify(&class_pred.typ, head)?;
        Ok(typ.apply(&s))
    }

    pub fn typecheck_instance_method(
        &mut self,
        prepared: &PreparedInstanceDecl,
        method: &InstanceMethodImpl,
    ) -> Result<TypedExpr, TypeError> {
        let expected =
            self.instantiate_class_method_for_head(&prepared.class, &method.name, &prepared.head)?;
        let (typed, preds, actual) = infer_typed(self, method.body.as_ref())?;
        let s = unify(&actual, &expected)?;
        let typed = typed.apply(&s);
        let preds = preds.apply(&s);

        // The only legal “given” constraints inside an instance method are the
        // instance context (plus superclass closure, plus the instance head
        // itself). We do *not* allow instance
        // search for non-ground constraints here, because that would be unsound:
        // a type variable would unify with any concrete instance head.
        let mut given = prepared.context.clone();

        // Allow recursive instance methods (e.g. `Eq (List a)` calling `(==)`
        // on the tail). This is dictionary recursion, not instance search.
        given.push(Predicate::new(
            prepared.class.clone(),
            prepared.head.clone(),
        ));
        let mut i = 0;
        while i < given.len() {
            let p = given[i].clone();
            for sup in self.classes.supers_of(&p.class) {
                given.push(Predicate::new(sup, p.typ.clone()));
            }
            i += 1;
        }

        for pred in &preds {
            if pred.typ.ftv().is_empty() {
                if !entails(&self.classes, &given, pred)? {
                    return Err(TypeError::NoInstance(
                        pred.class.clone(),
                        pred.typ.to_string(),
                    ));
                }
            } else if !given
                .iter()
                .any(|p| p.class == pred.class && p.typ == pred.typ)
            {
                return Err(TypeError::MissingInstanceConstraint {
                    method: method.name.clone(),
                    class: pred.class.clone(),
                    typ: pred.typ.to_string(),
                });
            }
        }

        Ok(typed)
    }

    /// Register constructor schemes for an ADT in the type environment.
    /// This makes constructors (e.g. `Some`, `None`, `Ok`, `Err`) available
    /// to the type checker as normal values.
    pub fn register_adt(&mut self, adt: &AdtDecl) {
        self.adts.insert(adt.name.clone(), adt.clone());
        for (name, scheme) in adt.constructor_schemes() {
            self.register_value_scheme(&name, scheme);
        }
    }

    pub fn adt_from_decl(&mut self, decl: &TypeDecl) -> Result<AdtDecl, TypeError> {
        let mut adt = AdtDecl::new(&decl.name, &decl.params, &mut self.supply);
        let mut param_map: BTreeMap<Symbol, TypeVar> = BTreeMap::new();
        for param in &adt.params {
            param_map.insert(param.name.clone(), param.var.clone());
        }

        for variant in &decl.variants {
            let mut args = Vec::new();
            for arg in &variant.args {
                let ty = self.type_from_expr(decl, &param_map, arg)?;
                args.push(ty);
            }
            adt.add_variant(variant.name.clone(), args);
        }
        Ok(adt)
    }

    pub fn register_type_decl(&mut self, decl: &TypeDecl) -> Result<(), TypeError> {
        if BuiltinTypeId::from_symbol(&decl.name).is_some() {
            return Err(TypeError::ReservedTypeName(decl.name.clone()));
        }
        let adt = self.adt_from_decl(decl)?;
        self.register_adt(&adt);
        Ok(())
    }

    fn type_from_expr(
        &mut self,
        decl: &TypeDecl,
        params: &BTreeMap<Symbol, TypeVar>,
        expr: &TypeExpr,
    ) -> Result<Type, TypeError> {
        let span = *expr.span();
        let res = (|| match expr {
            TypeExpr::Name(_, name) => {
                let name_sym = name.to_dotted_symbol();
                if let Some(tv) = params.get(&name_sym) {
                    Ok(Type::var(tv.clone()))
                } else {
                    let name = normalize_type_name(&name_sym);
                    if let Some(arity) = self.type_arity(decl, &name) {
                        Ok(Type::con(name, arity))
                    } else {
                        Err(TypeError::UnknownTypeName(name))
                    }
                }
            }
            TypeExpr::App(_, fun, arg) => {
                let fty = self.type_from_expr(decl, params, fun)?;
                let aty = self.type_from_expr(decl, params, arg)?;
                Ok(type_app_with_result_syntax(fty, aty))
            }
            TypeExpr::Fun(_, arg, ret) => {
                let arg_ty = self.type_from_expr(decl, params, arg)?;
                let ret_ty = self.type_from_expr(decl, params, ret)?;
                Ok(Type::fun(arg_ty, ret_ty))
            }
            TypeExpr::Tuple(_, elems) => {
                let mut out = Vec::new();
                for elem in elems {
                    out.push(self.type_from_expr(decl, params, elem)?);
                }
                Ok(Type::tuple(out))
            }
            TypeExpr::Record(_, fields) => {
                let mut out = Vec::new();
                for (name, ty) in fields {
                    out.push((name.clone(), self.type_from_expr(decl, params, ty)?));
                }
                Ok(Type::record(out))
            }
        })();
        res.map_err(|err| err.with_span(&span))
    }

    fn type_arity(&self, decl: &TypeDecl, name: &Symbol) -> Option<usize> {
        if &decl.name == name {
            return Some(decl.params.len());
        }
        if let Some(adt) = self.adts.get(name) {
            return Some(adt.params.len());
        }
        BuiltinTypeId::from_symbol(name).map(BuiltinTypeId::arity)
    }

    fn register_value_scheme(&mut self, name: &Symbol, scheme: Scheme) {
        match self.env.lookup(name) {
            None => self.env.extend(name.clone(), scheme),
            Some(existing) => {
                if existing.iter().any(|s| unify(&s.typ, &scheme.typ).is_ok()) {
                    return;
                }
                self.env.extend_overload(name.clone(), scheme);
            }
        }
    }

    fn expected_class_param_arities(&self, class: &Symbol) -> Option<Vec<usize>> {
        let info = self.class_info.get(class)?;
        let mut out = vec![0usize; info.params.len()];
        for scheme in info.methods.values() {
            for (idx, param) in info.params.iter().enumerate() {
                let Some(tv) = scheme.vars.iter().find(|v| v.name.as_ref() == Some(param)) else {
                    continue;
                };
                out[idx] = out[idx].max(max_head_app_arity_for_var(&scheme.typ, tv.id));
            }
        }
        Some(out)
    }

    fn check_predicate_kind(&self, pred: &Predicate) -> Result<(), TypeError> {
        let Some(expected) = self.expected_class_param_arities(&pred.class) else {
            // Host-injected classes (via Rust API) won't have `class_info`.
            return Ok(());
        };

        let args: Vec<Type> = if expected.len() == 1 {
            vec![pred.typ.clone()]
        } else if let TypeKind::Tuple(parts) = pred.typ.as_ref() {
            if parts.len() != expected.len() {
                return Ok(());
            }
            parts.clone()
        } else {
            return Ok(());
        };

        for (arg, expected_arity) in args.iter().zip(expected.iter().copied()) {
            let Some(got) = type_term_remaining_arity(arg) else {
                // If we can't determine the arity (e.g. a bare type var), skip:
                // call sites may fix it up, and Rex does not currently do full
                // kind inference.
                continue;
            };
            if got != expected_arity {
                return Err(TypeError::KindMismatch {
                    class: pred.class.clone(),
                    expected: expected_arity,
                    got,
                    typ: arg.to_string(),
                });
            }
        }
        Ok(())
    }

    pub(crate) fn check_predicate_kinds(&self, preds: &[Predicate]) -> Result<(), TypeError> {
        for pred in preds {
            self.check_predicate_kind(pred)?;
        }
        Ok(())
    }
}

pub(crate) fn type_from_annotation_expr(
    adts: &BTreeMap<Symbol, AdtDecl>,
    expr: &TypeExpr,
) -> Result<Type, TypeError> {
    let span = *expr.span();
    let res = (|| match expr {
        TypeExpr::Name(_, name) => {
            let name = normalize_type_name(&name.to_dotted_symbol());
            match annotation_type_arity(adts, &name) {
                Some(arity) => Ok(Type::con(name, arity)),
                None => Err(TypeError::UnknownTypeName(name)),
            }
        }
        TypeExpr::App(_, fun, arg) => {
            let fty = type_from_annotation_expr(adts, fun)?;
            let aty = type_from_annotation_expr(adts, arg)?;
            Ok(type_app_with_result_syntax(fty, aty))
        }
        TypeExpr::Fun(_, arg, ret) => {
            let arg_ty = type_from_annotation_expr(adts, arg)?;
            let ret_ty = type_from_annotation_expr(adts, ret)?;
            Ok(Type::fun(arg_ty, ret_ty))
        }
        TypeExpr::Tuple(_, elems) => {
            let mut out = Vec::new();
            for elem in elems {
                out.push(type_from_annotation_expr(adts, elem)?);
            }
            Ok(Type::tuple(out))
        }
        TypeExpr::Record(_, fields) => {
            let mut out = Vec::new();
            for (name, ty) in fields {
                out.push((name.clone(), type_from_annotation_expr(adts, ty)?));
            }
            Ok(Type::record(out))
        }
    })();
    res.map_err(|err| err.with_span(&span))
}

pub(crate) fn type_from_annotation_expr_vars(
    adts: &BTreeMap<Symbol, AdtDecl>,
    expr: &TypeExpr,
    vars: &mut BTreeMap<Symbol, TypeVar>,
    supply: &mut TypeVarSupply,
) -> Result<Type, TypeError> {
    let span = *expr.span();
    let res = (|| match expr {
        TypeExpr::Name(_, name) => {
            let name = normalize_type_name(&name.to_dotted_symbol());
            if let Some(arity) = annotation_type_arity(adts, &name) {
                Ok(Type::con(name, arity))
            } else if let Some(tv) = vars.get(&name) {
                Ok(Type::var(tv.clone()))
            } else {
                let is_upper = name
                    .chars()
                    .next()
                    .map(|c| c.is_uppercase())
                    .unwrap_or(false);
                if is_upper {
                    return Err(TypeError::UnknownTypeName(name));
                }
                let tv = supply.fresh(Some(name.clone()));
                vars.insert(name.clone(), tv.clone());
                Ok(Type::var(tv))
            }
        }
        TypeExpr::App(_, fun, arg) => {
            let fty = type_from_annotation_expr_vars(adts, fun, vars, supply)?;
            let aty = type_from_annotation_expr_vars(adts, arg, vars, supply)?;
            Ok(type_app_with_result_syntax(fty, aty))
        }
        TypeExpr::Fun(_, arg, ret) => {
            let arg_ty = type_from_annotation_expr_vars(adts, arg, vars, supply)?;
            let ret_ty = type_from_annotation_expr_vars(adts, ret, vars, supply)?;
            Ok(Type::fun(arg_ty, ret_ty))
        }
        TypeExpr::Tuple(_, elems) => {
            let mut out = Vec::new();
            for elem in elems {
                out.push(type_from_annotation_expr_vars(adts, elem, vars, supply)?);
            }
            Ok(Type::tuple(out))
        }
        TypeExpr::Record(_, fields) => {
            let mut out = Vec::new();
            for (name, ty) in fields {
                out.push((
                    name.clone(),
                    type_from_annotation_expr_vars(adts, ty, vars, supply)?,
                ));
            }
            Ok(Type::record(out))
        }
    })();
    res.map_err(|err| err.with_span(&span))
}

fn annotation_type_arity(adts: &BTreeMap<Symbol, AdtDecl>, name: &Symbol) -> Option<usize> {
    if let Some(adt) = adts.get(name) {
        return Some(adt.params.len());
    }
    BuiltinTypeId::from_symbol(name).map(BuiltinTypeId::arity)
}

fn normalize_type_name(name: &Symbol) -> Symbol {
    if name.as_ref() == "str" {
        BuiltinTypeId::String.as_symbol()
    } else {
        name.clone()
    }
}

fn type_app_with_result_syntax(fun: Type, arg: Type) -> Type {
    if let TypeKind::App(head, ok) = fun.as_ref()
        && matches!(
            head.as_ref(),
            TypeKind::Con(c)
                if c.builtin_id == Some(BuiltinTypeId::Result) && c.arity == 2
        )
    {
        return Type::app(Type::app(head.clone(), arg), ok.clone());
    }
    Type::app(fun, arg)
}

pub(crate) fn predicates_from_constraints(
    adts: &BTreeMap<Symbol, AdtDecl>,
    constraints: &[TypeConstraint],
    vars: &mut BTreeMap<Symbol, TypeVar>,
    supply: &mut TypeVarSupply,
) -> Result<Vec<Predicate>, TypeError> {
    let mut out = Vec::with_capacity(constraints.len());
    for constraint in constraints {
        let ty = type_from_annotation_expr_vars(adts, &constraint.typ, vars, supply)?;
        out.push(Predicate::new(constraint.class.as_ref(), ty));
    }
    Ok(out)
}

//! Hindley-Milner type system with parametric polymorphism, type classes, and ADTs.
//! The goal is to provide a reusable library for building typing environments for Rex.
//! Features:
//! - Type variables, type constructors, function and tuple types.
//! - Schemes with quantified variables and class constraints.
//! - Type classes with superclass relationships and instance resolution.
//! - Basic ADTs (List, Result, Option) and numeric/string primitives in the prelude.
//! - Utilities to register additional function/type declarations (e.g. `(-)`, `(/)`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::{self, Display, Formatter};

use chrono::{DateTime, Utc};
use rex_ast::expr::{Expr, Pattern, TypeDecl, TypeExpr};
use rex_lexer::span::Span;
use uuid::Uuid;

pub type TypeVarId = usize;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TypeVar {
    pub id: TypeVarId,
    pub name: Option<String>,
}

impl TypeVar {
    pub fn new(id: TypeVarId, name: impl Into<Option<String>>) -> Self {
        Self {
            id,
            name: name.into(),
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TypeConst {
    pub name: String,
    pub arity: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Type {
    Var(TypeVar),
    Con(TypeConst),
    App(Box<Type>, Box<Type>),
    Fun(Box<Type>, Box<Type>),
    Tuple(Vec<Type>),
}

impl Type {
    pub fn con(name: &str, arity: usize) -> Self {
        Type::Con(TypeConst {
            name: name.to_string(),
            arity,
        })
    }

    pub fn fun(a: Type, b: Type) -> Self {
        Type::Fun(Box::new(a), Box::new(b))
    }

    pub fn app(f: Type, arg: Type) -> Self {
        Type::App(Box::new(f), Box::new(arg))
    }
}

impl Display for Type {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Type::Var(tv) => match &tv.name {
                Some(name) => write!(f, "{}", name),
                None => write!(f, "t{}", tv.id),
            },
            Type::Con(c) => write!(f, "{}", c.name),
            Type::App(l, r) => write!(f, "({} {})", l, r),
            Type::Fun(a, b) => write!(f, "({} -> {})", a, b),
            Type::Tuple(elems) => {
                write!(f, "(")?;
                for (i, t) in elems.iter().enumerate() {
                    write!(f, "{}", t)?;
                    if i + 1 < elems.len() {
                        write!(f, ", ")?;
                    }
                }
                write!(f, ")")
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Predicate {
    pub class: String,
    pub typ: Type,
}

impl Predicate {
    pub fn new(class: impl Into<String>, typ: Type) -> Self {
        Self {
            class: class.into(),
            typ,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Scheme {
    pub vars: Vec<TypeVar>,
    pub preds: Vec<Predicate>,
    pub typ: Type,
}

impl Scheme {
    pub fn new(vars: Vec<TypeVar>, preds: Vec<Predicate>, typ: Type) -> Self {
        Self { vars, preds, typ }
    }
}

pub type Subst = HashMap<TypeVarId, Type>;

pub trait Types: Sized {
    fn apply(&self, s: &Subst) -> Self;
    fn ftv(&self) -> HashSet<TypeVarId>;
}

impl Types for Type {
    fn apply(&self, s: &Subst) -> Self {
        match self {
            Type::Var(tv) => s.get(&tv.id).cloned().unwrap_or_else(|| self.clone()),
            Type::Con(_) => self.clone(),
            Type::App(l, r) => Type::app(l.apply(s), r.apply(s)),
            Type::Fun(a, b) => Type::fun(a.apply(s), b.apply(s)),
            Type::Tuple(ts) => Type::Tuple(ts.iter().map(|t| t.apply(s)).collect()),
        }
    }

    fn ftv(&self) -> HashSet<TypeVarId> {
        match self {
            Type::Var(tv) => [tv.id].into_iter().collect(),
            Type::Con(_) => HashSet::new(),
            Type::App(l, r) => l.ftv().union(&r.ftv()).copied().collect(),
            Type::Fun(a, b) => a.ftv().union(&b.ftv()).copied().collect(),
            Type::Tuple(ts) => ts.iter().flat_map(Types::ftv).collect(),
        }
    }
}

impl Types for Predicate {
    fn apply(&self, s: &Subst) -> Self {
        Predicate::new(self.class.clone(), self.typ.apply(s))
    }

    fn ftv(&self) -> HashSet<TypeVarId> {
        self.typ.ftv()
    }
}

impl Types for Scheme {
    fn apply(&self, s: &Subst) -> Self {
        let s_pruned: Subst = s
            .iter()
            .filter(|(k, _)| !self.vars.iter().any(|v| &v.id == *k))
            .map(|(k, v)| (*k, v.clone()))
            .collect();
        Scheme::new(
            self.vars.clone(),
            self.preds.iter().map(|p| p.apply(&s_pruned)).collect(),
            self.typ.apply(&s_pruned),
        )
    }

    fn ftv(&self) -> HashSet<TypeVarId> {
        let mut ftv = self.typ.ftv();
        for p in &self.preds {
            ftv.extend(p.ftv());
        }
        for v in &self.vars {
            ftv.remove(&v.id);
        }
        ftv
    }
}

impl<T: Types> Types for Vec<T> {
    fn apply(&self, s: &Subst) -> Self {
        self.iter().map(|t| t.apply(s)).collect()
    }

    fn ftv(&self) -> HashSet<TypeVarId> {
        self.iter().flat_map(Types::ftv).collect()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TypedExpr {
    pub typ: Type,
    pub kind: TypedExprKind,
}

impl TypedExpr {
    pub fn new(typ: Type, kind: TypedExprKind) -> Self {
        Self { typ, kind }
    }

    pub fn apply(&self, s: &Subst) -> Self {
        let typ = self.typ.apply(s);
        let kind = match &self.kind {
            TypedExprKind::Bool(v) => TypedExprKind::Bool(*v),
            TypedExprKind::Uint(v) => TypedExprKind::Uint(*v),
            TypedExprKind::Int(v) => TypedExprKind::Int(*v),
            TypedExprKind::Float(v) => TypedExprKind::Float(*v),
            TypedExprKind::String(v) => TypedExprKind::String(v.clone()),
            TypedExprKind::Uuid(v) => TypedExprKind::Uuid(*v),
            TypedExprKind::DateTime(v) => TypedExprKind::DateTime(*v),
            TypedExprKind::Tuple(elems) => {
                TypedExprKind::Tuple(elems.iter().map(|e| e.apply(s)).collect())
            }
            TypedExprKind::List(elems) => {
                TypedExprKind::List(elems.iter().map(|e| e.apply(s)).collect())
            }
            TypedExprKind::Dict(kvs) => {
                let mut out = BTreeMap::new();
                for (k, v) in kvs {
                    out.insert(k.clone(), v.apply(s));
                }
                TypedExprKind::Dict(out)
            }
            TypedExprKind::Var { name, overloads } => TypedExprKind::Var {
                name: name.clone(),
                overloads: overloads.iter().map(|t| t.apply(s)).collect(),
            },
            TypedExprKind::App(f, x) => {
                TypedExprKind::App(Box::new(f.apply(s)), Box::new(x.apply(s)))
            }
            TypedExprKind::Lam { param, body } => TypedExprKind::Lam {
                param: param.clone(),
                body: Box::new(body.apply(s)),
            },
            TypedExprKind::Let { name, def, body } => TypedExprKind::Let {
                name: name.clone(),
                def: Box::new(def.apply(s)),
                body: Box::new(body.apply(s)),
            },
            TypedExprKind::Ite {
                cond,
                then_expr,
                else_expr,
            } => TypedExprKind::Ite {
                cond: Box::new(cond.apply(s)),
                then_expr: Box::new(then_expr.apply(s)),
                else_expr: Box::new(else_expr.apply(s)),
            },
            TypedExprKind::Match { scrutinee, arms } => TypedExprKind::Match {
                scrutinee: Box::new(scrutinee.apply(s)),
                arms: arms.iter().map(|(p, e)| (p.clone(), e.apply(s))).collect(),
            },
        };
        TypedExpr { typ, kind }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum TypedExprKind {
    Bool(bool),
    Uint(u64),
    Int(i64),
    Float(f64),
    String(String),
    Uuid(Uuid),
    DateTime(DateTime<Utc>),
    Tuple(Vec<TypedExpr>),
    List(Vec<TypedExpr>),
    Dict(BTreeMap<String, TypedExpr>),
    Var { name: String, overloads: Vec<Type> },
    App(Box<TypedExpr>, Box<TypedExpr>),
    Lam { param: String, body: Box<TypedExpr> },
    Let {
        name: String,
        def: Box<TypedExpr>,
        body: Box<TypedExpr>,
    },
    Ite {
        cond: Box<TypedExpr>,
        then_expr: Box<TypedExpr>,
        else_expr: Box<TypedExpr>,
    },
    Match {
        scrutinee: Box<TypedExpr>,
        arms: Vec<(Pattern, TypedExpr)>,
    },
}

pub fn compose_subst(a: Subst, b: Subst) -> Subst {
    let mut res: Subst = b.into_iter().map(|(k, v)| (k, v.apply(&a))).collect();
    for (k, v) in a {
        res.insert(k, v);
    }
    res
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TypeError {
    #[error("types do not unify: {0} vs {1}")]
    Unification(String, String),
    #[error("occurs check failed for {0} in {1}")]
    Occurs(TypeVarId, String),
    #[error("unknown class {0}")]
    UnknownClass(String),
    #[error("no instance for {0} {1}")]
    NoInstance(String, String),
    #[error("unknown type {0}")]
    UnknownTypeName(String),
    #[error("unbound variable {0}")]
    UnknownVar(String),
    #[error("ambiguous overload for {0}")]
    AmbiguousOverload(String),
    #[error("unsupported expression {0}")]
    UnsupportedExpr(&'static str),
    #[error("non-exhaustive match for {typ}: missing {missing:?}")]
    NonExhaustiveMatch { typ: String, missing: Vec<String> },
    #[error("at {span}: {error}")]
    Spanned { span: Span, error: Box<TypeError> },
}

fn with_span(span: &Span, err: TypeError) -> TypeError {
    match err {
        TypeError::Spanned { .. } => err,
        other => TypeError::Spanned {
            span: *span,
            error: Box::new(other),
        },
    }
}

fn bind(tv: &TypeVar, t: &Type) -> Result<Subst, TypeError> {
    if let Type::Var(var) = t {
        if var.id == tv.id {
            return Ok(Subst::new());
        }
    }
    if t.ftv().contains(&tv.id) {
        Err(TypeError::Occurs(tv.id, t.to_string()))
    } else {
        let mut s = Subst::new();
        s.insert(tv.id, t.clone());
        Ok(s)
    }
}

pub fn unify(t1: &Type, t2: &Type) -> Result<Subst, TypeError> {
    match (t1, t2) {
        (Type::Fun(l1, r1), Type::Fun(l2, r2)) => {
            let s1 = unify(l1, l2)?;
            let s2 = unify(&r1.apply(&s1), &r2.apply(&s1))?;
            Ok(compose_subst(s2, s1))
        }
        (Type::App(l1, r1), Type::App(l2, r2)) => {
            let s1 = unify(l1, l2)?;
            let s2 = unify(&r1.apply(&s1), &r2.apply(&s1))?;
            Ok(compose_subst(s2, s1))
        }
        (Type::Tuple(ts1), Type::Tuple(ts2)) => {
            if ts1.len() != ts2.len() {
                return Err(TypeError::Unification(t1.to_string(), t2.to_string()));
            }
            let mut s = Subst::new();
            for (a, b) in ts1.iter().zip(ts2.iter()) {
                let s_next = unify(&a.apply(&s), &b.apply(&s))?;
                s = compose_subst(s_next, s);
            }
            Ok(s)
        }
        (Type::Var(tv), t) | (t, Type::Var(tv)) => bind(tv, t),
        (Type::Con(c1), Type::Con(c2)) if c1 == c2 => Ok(Subst::new()),
        _ => Err(TypeError::Unification(t1.to_string(), t2.to_string())),
    }
}

#[derive(Default, Debug, Clone)]
pub struct TypeEnv {
    pub values: HashMap<String, Vec<Scheme>>,
}

impl TypeEnv {
    pub fn new() -> Self {
        Self {
            values: HashMap::new(),
        }
    }

    pub fn extend(&mut self, name: impl Into<String>, scheme: Scheme) {
        self.values.insert(name.into(), vec![scheme]);
    }

    pub fn extend_overload(&mut self, name: impl Into<String>, scheme: Scheme) {
        self.values.entry(name.into()).or_default().push(scheme);
    }

    pub fn lookup(&self, name: &str) -> Option<&[Scheme]> {
        self.values.get(name).map(|schemes| schemes.as_slice())
    }
}

impl Types for TypeEnv {
    fn apply(&self, s: &Subst) -> Self {
        let values = self
            .values
            .iter()
            .map(|(k, v)| (k.clone(), v.iter().map(|scheme| scheme.apply(s)).collect()))
            .collect();
        TypeEnv { values }
    }

    fn ftv(&self) -> HashSet<TypeVarId> {
        self.values
            .values()
            .flat_map(|schemes| schemes.iter().flat_map(Types::ftv))
            .collect()
    }
}

#[derive(Default, Debug, Clone)]
pub struct TypeVarSupply {
    counter: TypeVarId,
}

impl TypeVarSupply {
    pub fn new() -> Self {
        Self { counter: 0 }
    }

    pub fn fresh(&mut self, name_hint: impl Into<Option<String>>) -> TypeVar {
        let tv = TypeVar::new(self.counter, name_hint.into());
        self.counter += 1;
        tv
    }
}

pub fn generalize(env: &TypeEnv, preds: Vec<Predicate>, typ: Type) -> Scheme {
    let mut vars: Vec<TypeVar> = typ
        .ftv()
        .union(&preds.ftv())
        .copied()
        .collect::<HashSet<_>>()
        .difference(&env.ftv())
        .cloned()
        .map(|id| TypeVar::new(id, None))
        .collect();
    vars.sort_by_key(|v| v.id);
    Scheme::new(vars, preds, typ)
}

pub fn instantiate(scheme: &Scheme, supply: &mut TypeVarSupply) -> (Vec<Predicate>, Type) {
    let mut subst = Subst::new();
    for v in &scheme.vars {
        subst.insert(v.id, Type::Var(supply.fresh(v.name.clone())));
    }
    (scheme.preds.apply(&subst), scheme.typ.apply(&subst))
}

/// A named type parameter for an ADT (e.g. `a` in `List a`).
#[derive(Clone, Debug)]
pub struct AdtParam {
    pub name: String,
    pub var: TypeVar,
}

/// A single ADT variant with zero or more constructor arguments.
#[derive(Clone, Debug)]
pub struct AdtVariant {
    pub name: String,
    pub args: Vec<Type>,
}

/// A type declaration for an algebraic data type.
///
/// This only describes the *type* surface (params + variants). It does not
/// introduce any runtime values by itself. Runtime values are created by
/// injecting constructor schemes into the environment (see `inject_adt`).
#[derive(Clone, Debug)]
pub struct AdtDecl {
    pub name: String,
    pub params: Vec<AdtParam>,
    pub variants: Vec<AdtVariant>,
}

impl AdtDecl {
    pub fn new(name: impl Into<String>, param_names: &[&str], supply: &mut TypeVarSupply) -> Self {
        let params = param_names
            .iter()
            .map(|p| AdtParam {
                name: (*p).to_string(),
                var: supply.fresh(Some((*p).to_string())),
            })
            .collect();
        Self {
            name: name.into(),
            params,
            variants: Vec::new(),
        }
    }

    pub fn param_type(&self, name: &str) -> Option<Type> {
        self.params
            .iter()
            .find(|p| p.name == name)
            .map(|p| Type::Var(p.var.clone()))
    }

    pub fn add_variant(&mut self, name: impl Into<String>, args: Vec<Type>) {
        self.variants.push(AdtVariant {
            name: name.into(),
            args,
        });
    }

    pub fn result_type(&self) -> Type {
        let mut ty = Type::con(&self.name, self.params.len());
        for param in &self.params {
            ty = Type::app(ty, Type::Var(param.var.clone()));
        }
        ty
    }

    /// Build constructor schemes of the form:
    /// `C :: a1 -> a2 -> ... -> T params`.
    pub fn constructor_schemes(&self) -> Vec<(String, Scheme)> {
        let result_ty = self.result_type();
        let vars: Vec<TypeVar> = self.params.iter().map(|p| p.var.clone()).collect();
        let mut out = Vec::new();
        for variant in &self.variants {
            let mut typ = result_ty.clone();
            for arg in variant.args.iter().rev() {
                typ = Type::fun(arg.clone(), typ);
            }
            out.push((variant.name.clone(), Scheme::new(vars.clone(), vec![], typ)));
        }
        out
    }
}

#[derive(Clone, Debug)]
pub struct Class {
    pub supers: Vec<String>,
}

impl Class {
    pub fn new(supers: Vec<String>) -> Self {
        Self { supers }
    }
}

#[derive(Clone, Debug)]
pub struct Instance {
    pub context: Vec<Predicate>,
    pub head: Predicate,
}

impl Instance {
    pub fn new(context: Vec<Predicate>, head: Predicate) -> Self {
        Self { context, head }
    }
}

#[derive(Default, Debug, Clone)]
pub struct ClassEnv {
    pub classes: HashMap<String, Class>,
    pub instances: HashMap<String, Vec<Instance>>,
}

impl ClassEnv {
    pub fn new() -> Self {
        Self {
            classes: HashMap::new(),
            instances: HashMap::new(),
        }
    }

    pub fn add_class(&mut self, name: impl Into<String>, supers: Vec<String>) {
        self.classes.insert(name.into(), Class::new(supers));
    }

    pub fn add_instance(&mut self, class: impl Into<String>, inst: Instance) {
        self.instances.entry(class.into()).or_default().push(inst);
    }

    pub fn supers_of(&self, class: &str) -> Vec<String> {
        self.classes
            .get(class)
            .map(|c| c.supers.clone())
            .unwrap_or_default()
    }
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

    let instances = class_env
        .instances
        .get(&pred.class)
        .ok_or_else(|| TypeError::UnknownClass(pred.class.clone()))?;

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
    Ok(false)
}

#[derive(Default, Debug, Clone)]
pub struct TypeSystem {
    pub env: TypeEnv,
    pub classes: ClassEnv,
    pub adts: HashMap<String, AdtDecl>,
    pub supply: TypeVarSupply,
}

impl TypeSystem {
    pub fn new() -> Self {
        Self {
            env: TypeEnv::new(),
            classes: ClassEnv::new(),
            adts: HashMap::new(),
            supply: TypeVarSupply::new(),
        }
    }

    pub fn with_prelude() -> Self {
        let mut ts = TypeSystem::new();
        build_prelude(&mut ts);
        ts
    }

    pub fn add_value(&mut self, name: impl Into<String>, scheme: Scheme) {
        self.env.extend(name, scheme);
    }

    pub fn add_overload(&mut self, name: impl Into<String>, scheme: Scheme) {
        self.env.extend_overload(name, scheme);
    }

    pub fn inject_class(&mut self, name: impl Into<String>, supers: Vec<String>) {
        self.classes.add_class(name, supers);
    }

    pub fn inject_instance(&mut self, class: impl Into<String>, inst: Instance) {
        self.classes.add_instance(class, inst);
    }

    /// Register constructor schemes for an ADT in the type environment.
    /// This makes constructors (e.g. `Some`, `None`, `Ok`, `Err`) available
    /// to the type checker as normal values.
    pub fn inject_adt(&mut self, adt: &AdtDecl) {
        self.adts.insert(adt.name.clone(), adt.clone());
        for (name, scheme) in adt.constructor_schemes() {
            self.register_value_scheme(&name, scheme);
        }
    }

    pub fn adt_from_decl(&mut self, decl: &TypeDecl) -> Result<AdtDecl, TypeError> {
        let params: Vec<&str> = decl.params.iter().map(|p| p.as_str()).collect();
        let mut adt = AdtDecl::new(&decl.name, &params, &mut self.supply);
        let mut param_map = HashMap::new();
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

    pub fn inject_type_decl(&mut self, decl: &TypeDecl) -> Result<(), TypeError> {
        let adt = self.adt_from_decl(decl)?;
        self.inject_adt(&adt);
        Ok(())
    }

    fn type_from_expr(
        &mut self,
        decl: &TypeDecl,
        params: &HashMap<String, TypeVar>,
        expr: &TypeExpr,
    ) -> Result<Type, TypeError> {
        let span = *expr.span();
        let res = (|| match expr {
            TypeExpr::Name(_, name) => {
                if let Some(tv) = params.get(name) {
                    Ok(Type::Var(tv.clone()))
                } else if let Some(arity) = self.type_arity(decl, name) {
                    Ok(Type::con(name, arity))
                } else {
                    Err(TypeError::UnknownTypeName(name.clone()))
                }
            }
            TypeExpr::App(_, fun, arg) => {
                let fty = self.type_from_expr(decl, params, fun)?;
                let aty = self.type_from_expr(decl, params, arg)?;
                Ok(Type::app(fty, aty))
            }
            TypeExpr::Tuple(_, elems) => {
                let mut out = Vec::new();
                for elem in elems {
                    out.push(self.type_from_expr(decl, params, elem)?);
                }
                Ok(Type::Tuple(out))
            }
            TypeExpr::Record(_, fields) => {
                if fields.is_empty() {
                    let tv = self.supply.fresh(Some("v".into()));
                    return Ok(Type::app(Type::con("Dict", 1), Type::Var(tv)));
                }
                let mut subst = Subst::new();
                let mut types = Vec::new();
                for (_, ty) in fields {
                    types.push(self.type_from_expr(decl, params, ty)?);
                }
                let mut cur = types[0].clone();
                for ty in types.iter().skip(1) {
                    let s_next = unify(&cur.apply(&subst), &ty.apply(&subst))?;
                    subst = compose_subst(s_next, subst);
                    cur = cur.apply(&subst);
                }
                Ok(Type::app(Type::con("Dict", 1), cur.apply(&subst)))
            }
        })();
        res.map_err(|err| with_span(&span, err))
    }

    fn type_arity(&self, decl: &TypeDecl, name: &str) -> Option<usize> {
        if decl.name == name {
            return Some(decl.params.len());
        }
        if let Some(adt) = self.adts.get(name) {
            return Some(adt.params.len());
        }
        match name {
            "u8" | "u16" | "u32" | "u64" | "i8" | "i16" | "i32" | "i64" | "f32" | "f64"
            | "bool" | "string" | "uuid" | "datetime" => Some(0),
            "Dict" | "Array" => Some(1),
            _ => None,
        }
    }

    fn register_value_scheme(&mut self, name: &str, scheme: Scheme) {
        match self.env.lookup(name) {
            None => self.env.extend(name.to_string(), scheme),
            Some(existing) => {
                if existing.iter().any(|s| unify(&s.typ, &scheme.typ).is_ok()) {
                    return;
                }
                self.env.extend_overload(name.to_string(), scheme);
            }
        }
    }

    pub fn infer_typed(
        &mut self,
        expr: &Expr,
    ) -> Result<(TypedExpr, Vec<Predicate>, Type), TypeError> {
        let (s, preds, t, typed) = infer_expr(&mut self.supply, &self.env, &self.adts, expr)
            .map_err(|err| with_span(expr.span(), err))?;
        let mut typed = typed.apply(&s);
        let mut preds = preds.apply(&s);
        let mut t = t.apply(&s);
        let improve = improve_indexable(&preds)?;
        if !improve.is_empty() {
            typed = typed.apply(&improve);
            preds = preds.apply(&improve);
            t = t.apply(&improve);
        }
        Ok((typed, preds, t))
    }

    pub fn infer(&mut self, expr: &Expr) -> Result<(Vec<Predicate>, Type), TypeError> {
        let (_typed, preds, t) = self.infer_typed(expr)?;
        Ok((preds, t))
    }
}

fn improve_indexable(preds: &[Predicate]) -> Result<Subst, TypeError> {
    let mut subst = Subst::new();
    loop {
        let mut changed = false;
        for pred in preds {
            let pred = pred.apply(&subst);
            if pred.class != "Indexable" {
                continue;
            }
            let Type::Tuple(parts) = &pred.typ else {
                continue;
            };
            if parts.len() != 2 {
                continue;
            }
            let container = parts[0].clone();
            let elem = parts[1].clone();
            let s = indexable_elem_subst(&container, &elem)?;
            if !s.is_empty() {
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
    match container {
        Type::App(head, arg) => match head.as_ref() {
            Type::Con(tc) if tc.name == "List" || tc.name == "Array" => unify(elem, arg),
            _ => Ok(Subst::new()),
        },
        Type::Tuple(elems) => {
            if elems.is_empty() {
                return Ok(Subst::new());
            }
            let mut subst = Subst::new();
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
        _ => Ok(Subst::new()),
    }
}

fn infer_expr(
    supply: &mut TypeVarSupply,
    env: &TypeEnv,
    adts: &HashMap<String, AdtDecl>,
    expr: &Expr,
) -> Result<(Subst, Vec<Predicate>, Type, TypedExpr), TypeError> {
    let span = *expr.span();
    let res = (|| match expr {
        Expr::Bool(_, v) => {
            let t = Type::con("bool", 0);
            Ok((
                Subst::new(),
                vec![],
                t.clone(),
                TypedExpr::new(t, TypedExprKind::Bool(*v)),
            ))
        }
        Expr::Uint(_, v) => {
            let t = Type::con("i32", 0);
            Ok((
                Subst::new(),
                vec![],
                t.clone(),
                TypedExpr::new(t, TypedExprKind::Uint(*v)),
            ))
        }
        Expr::Int(_, v) => {
            let t = Type::con("i32", 0);
            Ok((
                Subst::new(),
                vec![],
                t.clone(),
                TypedExpr::new(t, TypedExprKind::Int(*v)),
            ))
        }
        Expr::Float(_, v) => {
            let t = Type::con("f32", 0);
            Ok((
                Subst::new(),
                vec![],
                t.clone(),
                TypedExpr::new(t, TypedExprKind::Float(*v)),
            ))
        }
        Expr::String(_, v) => {
            let t = Type::con("string", 0);
            Ok((
                Subst::new(),
                vec![],
                t.clone(),
                TypedExpr::new(t, TypedExprKind::String(v.clone())),
            ))
        }
        Expr::Uuid(_, v) => {
            let t = Type::con("uuid", 0);
            Ok((
                Subst::new(),
                vec![],
                t.clone(),
                TypedExpr::new(t, TypedExprKind::Uuid(*v)),
            ))
        }
        Expr::DateTime(_, v) => {
            let t = Type::con("datetime", 0);
            Ok((
                Subst::new(),
                vec![],
                t.clone(),
                TypedExpr::new(t, TypedExprKind::DateTime(*v)),
            ))
        }
        Expr::Var(var) => {
            let schemes = env
                .lookup(&var.name)
                .ok_or_else(|| TypeError::UnknownVar(var.name.clone()))?;
            if schemes.len() == 1 {
                let (preds, t) = instantiate(&schemes[0], supply);
                let typed = TypedExpr::new(
                    t.clone(),
                    TypedExprKind::Var {
                        name: var.name.clone(),
                        overloads: vec![],
                    },
                );
                Ok((Subst::new(), preds, t, typed))
            } else {
                let mut overloads = Vec::new();
                for scheme in schemes {
                    if !scheme.vars.is_empty() || !scheme.preds.is_empty() {
                        return Err(TypeError::AmbiguousOverload(var.name.clone()));
                    }
                    overloads.push(scheme.typ.clone());
                }
                let t = Type::Var(supply.fresh(Some(var.name.clone())));
                let typed = TypedExpr::new(
                    t.clone(),
                    TypedExprKind::Var {
                        name: var.name.clone(),
                        overloads,
                    },
                );
                Ok((Subst::new(), vec![], t, typed))
            }
        }
        Expr::Lam(_, _scope, param, body) => {
            let param_ty = Type::Var(supply.fresh(Some(param.name.clone())));
            let mut env1 = env.clone();
            env1.extend(
                param.name.clone(),
                Scheme::new(vec![], vec![], param_ty.clone()),
            );
            let (s1, preds, body_ty, typed_body) = infer_expr(supply, &env1, adts, body)?;
            let fun_ty = Type::fun(param_ty.apply(&s1), body_ty.clone());
            let typed = TypedExpr::new(
                fun_ty.clone(),
                TypedExprKind::Lam {
                    param: param.name.clone(),
                    body: Box::new(typed_body),
                },
            );
            Ok((s1.clone(), preds, fun_ty, typed))
        }
        Expr::App(_, f, x) => {
            let (s1, p1, t1, typed_f) = infer_expr(supply, env, adts, f)?;
            let (s2, p2, t2, typed_x) = infer_expr(supply, &env.apply(&s1), adts, x)?;
            let res_ty = Type::Var(supply.fresh(Some("r".into())));
            let s3 = unify(&t1.apply(&s2), &Type::fun(t2.clone(), res_ty.clone()))?;
            let s = compose_subst(s3.clone(), compose_subst(s2, s1));
            let preds = {
                let mut out = p1.apply(&s3);
                out.extend(p2.apply(&s3));
                out
            };
            let result_ty = res_ty.apply(&s3);
            let typed = TypedExpr::new(
                result_ty.clone(),
                TypedExprKind::App(Box::new(typed_f), Box::new(typed_x)),
            );
            Ok((s, preds, result_ty, typed))
        }
        Expr::Let(..) => {
            let mut bindings = Vec::new();
            let mut cur = expr;
            loop {
                match cur {
                    Expr::Let(_, v, d, b) => {
                        bindings.push((v.clone(), d.clone()));
                        cur = b.as_ref();
                    }
                    _ => break,
                }
            }

            let mut subst = Subst::new();
            let mut env_cur = env.clone();
            let mut typed_defs = Vec::new();
            for (v, d) in bindings {
                let (s1, p1, t1, typed_def) = infer_expr(supply, &env_cur, adts, &d)?;
                let env1 = env_cur.apply(&s1);
                let scheme = generalize(&env1, p1, t1.apply(&s1));
                env_cur = env1.clone();
                env_cur.extend(v.name.clone(), scheme);
                typed_defs.push((v.name.clone(), typed_def));
                subst = compose_subst(s1, subst);
            }

            let (s_body, p_body, t_body, typed_body) = infer_expr(supply, &env_cur, adts, cur)?;
            subst = compose_subst(s_body.clone(), subst);

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
            Ok((subst, p_body, t_body, typed))
        }
        Expr::Ite(_, cond, then_expr, else_expr) => {
            let (s1, p1, t1, typed_cond) = infer_expr(supply, env, adts, cond)?;
            let s2 = unify(&t1, &Type::con("bool", 0))?;
            let env1 = env.apply(&compose_subst(s2.clone(), s1.clone()));
            let (s3, p2, t2, typed_then) = infer_expr(supply, &env1, adts, then_expr)?;
            let (s4, p3, t3, typed_else) = infer_expr(supply, &env1.apply(&s3), adts, else_expr)?;
            let s5 = unify(&t2.apply(&s4), &t3.apply(&s4))?;
            let s = compose_subst(s5.clone(), compose_subst(s4, compose_subst(s3, s2)));
            let mut preds = p1;
            preds.extend(p2);
            preds.extend(p3);
            let out_ty = t3.apply(&s5);
            let typed = TypedExpr::new(
                out_ty.clone(),
                TypedExprKind::Ite {
                    cond: Box::new(typed_cond),
                    then_expr: Box::new(typed_then),
                    else_expr: Box::new(typed_else),
                },
            );
            Ok((s, preds.apply(&s5), out_ty, typed))
        }
        Expr::Tuple(_, elems) => {
            let mut subst = Subst::new();
            let mut preds = Vec::new();
            let mut types = Vec::new();
            let mut typed_elems = Vec::new();
            for elem in elems {
                let (s1, p1, t1, typed_elem) = infer_expr(supply, &env.apply(&subst), adts, elem)?;
                subst = compose_subst(s1.clone(), subst);
                preds.extend(p1.apply(&s1));
                types.push(t1.apply(&s1));
                typed_elems.push(typed_elem);
            }
            let tuple_ty = Type::Tuple(types);
            let typed = TypedExpr::new(tuple_ty.clone(), TypedExprKind::Tuple(typed_elems));
            Ok((subst, preds, tuple_ty, typed))
        }
        Expr::List(_, elems) => {
            let elem_tv = Type::Var(supply.fresh(Some("a".into())));
            let mut subst = Subst::new();
            let mut preds = Vec::new();
            let mut typed_elems = Vec::new();
            for elem in elems {
                let (s1, p1, t1, typed_elem) = infer_expr(supply, &env.apply(&subst), adts, elem)?;
                let s2 = unify(&t1.apply(&s1), &elem_tv.apply(&subst))?;
                subst = compose_subst(s2, compose_subst(s1, subst));
                preds.extend(p1);
                typed_elems.push(typed_elem);
            }
            let list_ty = Type::app(Type::con("List", 1), elem_tv.apply(&subst));
            let typed = TypedExpr::new(list_ty.clone(), TypedExprKind::List(typed_elems));
            Ok((subst.clone(), preds.apply(&subst), list_ty, typed))
        }
        Expr::Dict(_, kvs) => {
            let elem_tv = Type::Var(supply.fresh(Some("v".into())));
            let mut subst = Subst::new();
            let mut preds = Vec::new();
            let mut typed_kvs = BTreeMap::new();
            for (k, v) in kvs {
                let (s1, p1, t1, typed_v) = infer_expr(supply, &env.apply(&subst), adts, v)?;
                let s2 = unify(&t1.apply(&s1), &elem_tv.apply(&subst))?;
                subst = compose_subst(s2, compose_subst(s1, subst));
                preds.extend(p1);
                typed_kvs.insert(k.clone(), typed_v);
            }
            let dict_ty = Type::app(Type::con("Dict", 1), elem_tv.apply(&subst));
            let typed = TypedExpr::new(dict_ty.clone(), TypedExprKind::Dict(typed_kvs));
            Ok((subst.clone(), preds.apply(&subst), dict_ty, typed))
        }
        Expr::Match(_, scrutinee, arms) => {
            let (s1, p1, t1, typed_scrutinee) = infer_expr(supply, env, adts, scrutinee)?;
            let mut subst = s1;
            let mut preds = p1;
            let mut typed_arms = Vec::new();
            let res_ty = Type::Var(supply.fresh(Some("match".into())));
            let patterns: Vec<Pattern> = arms.iter().map(|(pat, _)| pat.clone()).collect();

            for (pat, expr) in arms {
                let scrutinee_ty = t1.apply(&subst);
                let (s_pat, p_pat, binds) = infer_pattern(supply, env, pat, &scrutinee_ty)?;
                subst = compose_subst(s_pat.clone(), subst);
                preds.extend(p_pat);

                let mut env_arm = env.apply(&subst);
                for (name, ty) in binds {
                    env_arm.extend(name, Scheme::new(vec![], vec![], ty));
                }
                let (s_expr, p_expr, t_expr, typed_expr) =
                    infer_expr(supply, &env_arm, adts, expr)?;
                let res_ty = res_ty.apply(&subst);
                let s_unify = unify(&res_ty, &t_expr.apply(&s_expr))?;
                subst = compose_subst(s_unify.clone(), compose_subst(s_expr, subst));
                preds.extend(p_expr);
                typed_arms.push((pat.clone(), typed_expr));
            }

            let scrutinee_ty = t1.apply(&subst);
            check_match_exhaustive(adts, &scrutinee_ty, &patterns)?;
            let out_ty = res_ty.apply(&subst);
            let typed = TypedExpr::new(
                out_ty.clone(),
                TypedExprKind::Match {
                    scrutinee: Box::new(typed_scrutinee),
                    arms: typed_arms,
                },
            );
            Ok((subst, preds, out_ty, typed))
        }
    })();
    res.map_err(|err| with_span(&span, err))
}

fn decompose_fun(typ: &Type, arity: usize) -> Option<(Vec<Type>, Type)> {
    let mut args = Vec::with_capacity(arity);
    let mut cur = typ.clone();
    for _ in 0..arity {
        match cur {
            Type::Fun(a, b) => {
                args.push(*a);
                cur = *b;
            }
            _ => return None,
        }
    }
    Some((args, cur))
}

fn infer_pattern(
    supply: &mut TypeVarSupply,
    env: &TypeEnv,
    pat: &Pattern,
    scrutinee_ty: &Type,
) -> Result<(Subst, Vec<Predicate>, Vec<(String, Type)>), TypeError> {
    let span = *pat.span();
    let res = (|| match pat {
        Pattern::Wildcard(..) => Ok((Subst::new(), vec![], vec![])),
        Pattern::Var(var) => Ok((
            Subst::new(),
            vec![],
            vec![(var.name.clone(), scrutinee_ty.clone())],
        )),
        Pattern::Named(_, name, ps) => {
            let schemes = env
                .lookup(name)
                .ok_or_else(|| TypeError::UnknownVar(name.clone()))?;
            if schemes.len() != 1 {
                return Err(TypeError::AmbiguousOverload(name.clone()));
            }
            let (preds, ctor_ty) = instantiate(&schemes[0], supply);
            let (arg_tys, res_ty) = decompose_fun(&ctor_ty, ps.len())
                .ok_or(TypeError::UnsupportedExpr("pattern constructor"))?;
            let s0 = unify(&res_ty, scrutinee_ty)?;
            let mut subst = s0.clone();
            let mut all_preds = preds.apply(&s0);
            let mut bindings = Vec::new();
            for (p, arg_ty) in ps.iter().zip(arg_tys.iter()) {
                let arg_ty = arg_ty.apply(&subst);
                let (s1, p1, binds1) = infer_pattern(supply, env, p, &arg_ty)?;
                subst = compose_subst(s1.clone(), subst);
                all_preds.extend(p1.apply(&s1));
                bindings.extend(binds1);
            }
            let bindings = bindings
                .into_iter()
                .map(|(name, ty)| (name, ty.apply(&subst)))
                .collect();
            Ok((subst, all_preds, bindings))
        }
        Pattern::List(_, ps) => {
            let elem_tv = Type::Var(supply.fresh(Some("a".into())));
            let list_ty = Type::app(Type::con("List", 1), elem_tv.clone());
            let s0 = unify(scrutinee_ty, &list_ty)?;
            let mut subst = s0.clone();
            let mut preds = Vec::new();
            let mut bindings = Vec::new();
            for p in ps {
                let elem_ty = elem_tv.apply(&subst);
                let (s1, p1, binds1) = infer_pattern(supply, env, p, &elem_ty)?;
                subst = compose_subst(s1.clone(), subst);
                preds.extend(p1.apply(&s1));
                bindings.extend(binds1);
            }
            let bindings = bindings
                .into_iter()
                .map(|(name, ty)| (name, ty.apply(&subst)))
                .collect();
            Ok((subst, preds, bindings))
        }
        Pattern::Cons(_, head, tail) => {
            let elem_tv = Type::Var(supply.fresh(Some("a".into())));
            let list_ty = Type::app(Type::con("List", 1), elem_tv.clone());
            let s0 = unify(scrutinee_ty, &list_ty)?;
            let mut subst = s0.clone();
            let mut preds = Vec::new();
            let mut bindings = Vec::new();

            let head_ty = elem_tv.apply(&subst);
            let (s1, p1, binds1) = infer_pattern(supply, env, head, &head_ty)?;
            subst = compose_subst(s1.clone(), subst);
            preds.extend(p1.apply(&s1));
            bindings.extend(binds1);

            let tail_ty = Type::app(Type::con("List", 1), elem_tv.apply(&subst));
            let (s2, p2, binds2) = infer_pattern(supply, env, tail, &tail_ty)?;
            subst = compose_subst(s2.clone(), subst);
            preds.extend(p2.apply(&s2));
            bindings.extend(binds2);

            let bindings = bindings
                .into_iter()
                .map(|(name, ty)| (name, ty.apply(&subst)))
                .collect();
            Ok((subst, preds, bindings))
        }
        Pattern::Dict(_, keys) => {
            let elem_tv = Type::Var(supply.fresh(Some("v".into())));
            let dict_ty = Type::app(Type::con("Dict", 1), elem_tv.clone());
            let s0 = unify(scrutinee_ty, &dict_ty)?;
            let elem_ty = elem_tv.apply(&s0);
            let bindings = keys
                .iter()
                .map(|k| (k.clone(), elem_ty.clone()))
                .collect();
            Ok((s0, vec![], bindings))
        }
    })();
    res.map_err(|err| with_span(&span, err))
}

fn type_head_name(typ: &Type) -> Option<&str> {
    let mut cur = typ;
    while let Type::App(head, _) = cur {
        cur = head;
    }
    match cur {
        Type::Con(tc) => Some(tc.name.as_str()),
        _ => None,
    }
}

fn check_match_exhaustive(
    adts: &HashMap<String, AdtDecl>,
    scrutinee_ty: &Type,
    patterns: &[Pattern],
) -> Result<(), TypeError> {
    if patterns
        .iter()
        .any(|p| matches!(p, Pattern::Wildcard(..) | Pattern::Var(_)))
    {
        return Ok(());
    }
    let adt_name = match type_head_name(scrutinee_ty) {
        Some(name) => name,
        None => return Ok(()),
    };
    let adt = match adts.get(adt_name) {
        Some(adt) => adt,
        None => return Ok(()),
    };
    let ctor_names: HashSet<String> = adt.variants.iter().map(|v| v.name.clone()).collect();
    if ctor_names.is_empty() {
        return Ok(());
    }
    let mut covered = HashSet::new();
    for pat in patterns {
        match pat {
            Pattern::Named(_, name, _) if ctor_names.contains(name) => {
                covered.insert(name.clone());
            }
            Pattern::List(_, elems) if adt_name == "List" && elems.is_empty() => {
                covered.insert("Empty".to_string());
            }
            Pattern::Cons(..) if adt_name == "List" => {
                covered.insert("Cons".to_string());
            }
            _ => {}
        }
    }
    let mut missing: Vec<String> = ctor_names.difference(&covered).cloned().collect();
    if missing.is_empty() {
        return Ok(());
    }
    missing.sort();
    Err(TypeError::NonExhaustiveMatch {
        typ: scrutinee_ty.to_string(),
        missing,
    })
}

fn build_prelude(ts: &mut TypeSystem) {
    // Primitive type constructors
    let prims = [
        "u8",
        "u16",
        "u32",
        "u64",
        "i8",
        "i16",
        "i32",
        "i64",
        "f32",
        "f64",
        "bool",
        "string",
        "uuid",
        "datetime",
    ];
    for prim in prims {
        ts.env
            .extend(prim, Scheme::new(vec![], vec![], Type::con(prim, 0)));
    }

    // Type constructors for ADTs and host-native arrays
    let list_con = Type::con("List", 1);
    let result_con = Type::con("Result", 2);
    let option_con = Type::con("Option", 1);
    let array_con = Type::con("Array", 1);

    // Register ADT constructors as value-level functions.
    let fresh_tv = |ts: &mut TypeSystem, name: &str| ts.supply.fresh(Some(name.to_string()));
    {
        let mut list_adt = AdtDecl::new("List", &["a"], &mut ts.supply);
        let a = list_adt.param_type("a").unwrap();
        let list_a = list_adt.result_type();
        list_adt.add_variant("Empty", vec![]);
        list_adt.add_variant("Cons", vec![a.clone(), list_a.clone()]);
        ts.inject_adt(&list_adt);
    }
    {
        let mut option_adt = AdtDecl::new("Option", &["t"], &mut ts.supply);
        let t = option_adt.param_type("t").unwrap();
        option_adt.add_variant("Some", vec![t]);
        option_adt.add_variant("None", vec![]);
        ts.inject_adt(&option_adt);
    }
    {
        let mut result_adt = AdtDecl::new("Result", &["e", "t"], &mut ts.supply);
        let e = result_adt.param_type("e").unwrap();
        let t = result_adt.param_type("t").unwrap();
        result_adt.add_variant("Err", vec![e]);
        result_adt.add_variant("Ok", vec![t]);
        ts.inject_adt(&result_adt);
    }

    // Classes
    ts.inject_class("AdditiveMonoid", vec![]);
    ts.inject_class("MultiplicativeMonoid", vec![]);
    ts.inject_class(
        "Semiring",
        vec!["AdditiveMonoid".into(), "MultiplicativeMonoid".into()],
    );
    ts.inject_class("AdditiveGroup", vec!["Semiring".into()]);
    ts.inject_class(
        "Ring",
        vec!["AdditiveGroup".into(), "MultiplicativeMonoid".into()],
    );
    ts.inject_class("Field", vec!["Ring".into()]);
    ts.inject_class("Integral", vec![]);
    ts.inject_class("Eq", vec![]);
    ts.inject_class("Ord", vec!["Eq".into()]);
    ts.inject_class("Functor", vec![]);
    ts.inject_class("Applicative", vec!["Functor".into()]);
    ts.inject_class("Monad", vec!["Applicative".into()]);
    ts.inject_class("Foldable", vec![]);
    ts.inject_class("Filterable", vec!["Functor".into()]);
    ts.inject_class("Sequence", vec!["Functor".into(), "Foldable".into()]);
    ts.inject_class("Alternative", vec!["Applicative".into()]);
    ts.inject_class("Indexable", vec![]);

    let numeric = |name: &str| Type::con(name, 0);
    let list_of = |t: Type| Type::app(list_con.clone(), t);
    let option_of = |t: Type| Type::app(option_con.clone(), t);
    let result_of = |t: Type, e: Type| Type::app(Type::app(result_con.clone(), e), t);
    let array_of = |t: Type| Type::app(array_con.clone(), t);
    let indexable_of = |container: Type, elem: Type| Type::Tuple(vec![container, elem]);

    let additive_only = ["string"];
    for name in additive_only {
        ts.inject_instance(
            "AdditiveMonoid",
            Instance::new(vec![], Predicate::new("AdditiveMonoid", numeric(name))),
        );
    }

    let semiring_names = [
        "u8", "u16", "u32", "u64", "i8", "i16", "i32", "i64", "f32", "f64",
    ];
    for name in semiring_names {
        let ty = numeric(name);
        ts.inject_instance(
            "AdditiveMonoid",
            Instance::new(vec![], Predicate::new("AdditiveMonoid", ty.clone())),
        );
        ts.inject_instance(
            "MultiplicativeMonoid",
            Instance::new(vec![], Predicate::new("MultiplicativeMonoid", ty.clone())),
        );
        ts.inject_instance(
            "Semiring",
            Instance::new(vec![], Predicate::new("Semiring", ty.clone())),
        );
    }

    let additive_group = ["i8", "i16", "i32", "i64", "f32", "f64"];
    for name in additive_group {
        let ty = numeric(name);
        ts.inject_instance(
            "AdditiveGroup",
            Instance::new(
                vec![Predicate::new("Semiring", ty.clone())],
                Predicate::new("AdditiveGroup", ty.clone()),
            ),
        );
        ts.inject_instance(
            "Ring",
            Instance::new(
                vec![Predicate::new("AdditiveGroup", ty.clone())],
                Predicate::new("Ring", ty.clone()),
            ),
        );
    }

    let fields = ["f32", "f64"];
    for name in fields {
        let ty = numeric(name);
        ts.inject_instance(
            "Field",
            Instance::new(
                vec![Predicate::new("Ring", ty.clone())],
                Predicate::new("Field", ty.clone()),
            ),
        );
    }

    let integral_types = ["u8", "u16", "u32", "u64", "i8", "i16", "i32", "i64"];
    for name in integral_types {
        let ty = numeric(name);
        ts.inject_instance(
            "Integral",
            Instance::new(vec![], Predicate::new("Integral", ty)),
        );
    }

    let eq_types = [
        "u8", "u16", "u32", "u64", "i8", "i16", "i32", "i64", "f32", "f64", "bool", "string",
        "uuid", "datetime",
    ];
    for name in eq_types {
        let ty = numeric(name);
        ts.inject_instance(
            "Eq",
            Instance::new(vec![], Predicate::new("Eq", ty)),
        );
    }

    let ord_types = ["u8", "u16", "u32", "u64", "i8", "i16", "i32", "i64", "f32", "f64", "string"];
    for name in ord_types {
        let ty = numeric(name);
        ts.inject_instance(
            "Ord",
            Instance::new(vec![Predicate::new("Eq", ty.clone())], Predicate::new("Ord", ty)),
        );
    }

    // Eq instances for parameterized types
    {
        let a_tv = fresh_tv(ts, "a");
        let a = Type::Var(a_tv.clone());
        ts.inject_instance(
            "Eq",
            Instance::new(
                vec![Predicate::new("Eq", a.clone())],
                Predicate::new("Eq", list_of(a.clone())),
            ),
        );
        ts.inject_instance(
            "Eq",
            Instance::new(
                vec![Predicate::new("Eq", a.clone())],
                Predicate::new("Eq", option_of(a.clone())),
            ),
        );
        ts.inject_instance(
            "Eq",
            Instance::new(
                vec![Predicate::new("Eq", a.clone())],
                Predicate::new("Eq", array_of(a.clone())),
            ),
        );
        let b_tv = fresh_tv(ts, "b");
        let b = Type::Var(b_tv.clone());
        ts.inject_instance(
            "Eq",
            Instance::new(
                vec![Predicate::new("Eq", a.clone()), Predicate::new("Eq", b.clone())],
                Predicate::new("Eq", result_of(a.clone(), b.clone())),
            ),
        );
    }

    // Functor / Applicative / Monad / Foldable / Filterable / Sequence / Alternative instances
    {
        let list = list_con.clone();
        let option = option_con.clone();
        let array = array_con.clone();
        let result_e_tv = fresh_tv(ts, "e");
        let result_e = Type::app(result_con.clone(), Type::Var(result_e_tv));

        let functors = [list.clone(), option.clone(), array.clone(), result_e.clone()];
        for f in functors {
            ts.inject_instance("Functor", Instance::new(vec![], Predicate::new("Functor", f)));
        }

        let applicatives = [list.clone(), option.clone(), array.clone(), result_e.clone()];
        for f in applicatives {
            ts.inject_instance(
                "Applicative",
                Instance::new(vec![Predicate::new("Functor", f.clone())], Predicate::new("Applicative", f)),
            );
        }

        let monads = [list.clone(), option.clone(), array.clone(), result_e.clone()];
        for m in monads {
            ts.inject_instance(
                "Monad",
                Instance::new(vec![Predicate::new("Applicative", m.clone())], Predicate::new("Monad", m)),
            );
        }

        let foldables = [list.clone(), option.clone(), array.clone()];
        for f in foldables {
            ts.inject_instance("Foldable", Instance::new(vec![], Predicate::new("Foldable", f)));
        }

        let filterables = [list.clone(), option.clone(), array.clone()];
        for f in filterables {
            ts.inject_instance(
                "Filterable",
                Instance::new(vec![Predicate::new("Functor", f.clone())], Predicate::new("Filterable", f)),
            );
        }

        let sequences = [list.clone(), array.clone()];
        for f in sequences {
            ts.inject_instance(
                "Sequence",
                Instance::new(
                    vec![Predicate::new("Functor", f.clone()), Predicate::new("Foldable", f.clone())],
                    Predicate::new("Sequence", f),
                ),
            );
        }

        let alternatives = [list.clone(), option.clone(), array.clone(), result_e];
        for f in alternatives {
            ts.inject_instance(
                "Alternative",
                Instance::new(vec![Predicate::new("Applicative", f.clone())], Predicate::new("Alternative", f)),
            );
        }
    }

    // Indexable instances for list, array, and homogeneous tuples (up to 32).
    {
        let a_tv = fresh_tv(ts, "a");
        let a = Type::Var(a_tv.clone());
        ts.inject_instance(
            "Indexable",
            Instance::new(vec![], Predicate::new("Indexable", indexable_of(list_of(a.clone()), a.clone()))),
        );
        let a_tv = fresh_tv(ts, "a");
        let a = Type::Var(a_tv.clone());
        ts.inject_instance(
            "Indexable",
            Instance::new(vec![], Predicate::new("Indexable", indexable_of(array_of(a.clone()), a.clone()))),
        );

        for size in 2..=32 {
            let a_tv = fresh_tv(ts, "a");
            let a = Type::Var(a_tv.clone());
            let elems = vec![a.clone(); size];
            let tuple_ty = Type::Tuple(elems);
            ts.inject_instance(
                "Indexable",
                Instance::new(
                    vec![],
                    Predicate::new("Indexable", indexable_of(tuple_ty, a.clone())),
                ),
            );
        }
    }

    // Inject provided function declarations and operator schemes.
    let a_tv = ts.supply.fresh(Some("a".into()));
    let a = Type::Var(a_tv.clone());
    let add_monoid_a = Predicate::new("AdditiveMonoid", a.clone());
    let add_group_a = Predicate::new("AdditiveGroup", a.clone());
    let integral_a = Predicate::new("Integral", a.clone());
    let plus_scheme = Scheme::new(
        vec![a_tv.clone()],
        vec![add_monoid_a],
        Type::fun(a.clone(), Type::fun(a.clone(), a.clone())),
    );
    ts.add_value("+", plus_scheme.clone());

    let mul_scheme = Scheme::new(
        vec![a_tv.clone()],
        vec![Predicate::new("MultiplicativeMonoid", a.clone())],
        Type::fun(a.clone(), Type::fun(a.clone(), a.clone())),
    );
    ts.add_value("*", mul_scheme);

    let mod_scheme = Scheme::new(
        vec![a_tv.clone()],
        vec![integral_a],
        Type::fun(a.clone(), Type::fun(a.clone(), a.clone())),
    );
    ts.add_value("%", mod_scheme);

    let negate_scheme = Scheme::new(
        vec![a_tv.clone()],
        vec![add_group_a.clone()],
        Type::fun(a.clone(), a.clone()),
    );
    ts.add_value("negate", negate_scheme);

    let minus_scheme = Scheme::new(
        vec![a_tv.clone()],
        vec![add_group_a],
        Type::fun(a.clone(), Type::fun(a.clone(), a.clone())),
    );
    ts.add_value("-", minus_scheme.clone());
    ts.add_value("(-)", minus_scheme);

    let b_tv = ts.supply.fresh(Some("b".into()));
    let b = Type::Var(b_tv.clone());
    let field_b = Predicate::new("Field", b.clone());
    let div_scheme = Scheme::new(
        vec![b_tv.clone()],
        vec![field_b],
        Type::fun(b.clone(), Type::fun(b.clone(), b.clone())),
    );
    ts.add_value("/", div_scheme.clone());
    ts.add_value("(/)", div_scheme);

    // zero/one for monoids
    ts.add_value(
        "zero",
        Scheme::new(
            vec![a_tv.clone()],
            vec![Predicate::new("AdditiveMonoid", a.clone())],
            a.clone(),
        ),
    );
    ts.add_value(
        "one",
        Scheme::new(
            vec![a_tv.clone()],
            vec![Predicate::new("MultiplicativeMonoid", a.clone())],
            a.clone(),
        ),
    );

    // Equality operators
    let eq_a = Predicate::new("Eq", a.clone());
    ts.add_value(
        "==",
        Scheme::new(
            vec![a_tv.clone()],
            vec![eq_a.clone()],
            Type::fun(a.clone(), Type::fun(a.clone(), Type::con("bool", 0))),
        ),
    );
    ts.add_value(
        "!=",
        Scheme::new(
            vec![a_tv.clone()],
            vec![eq_a],
            Type::fun(a.clone(), Type::fun(a.clone(), Type::con("bool", 0))),
        ),
    );

    // Ordering operators
    let ord_a = Predicate::new("Ord", a.clone());
    ts.add_value(
        "<",
        Scheme::new(
            vec![a_tv.clone()],
            vec![ord_a.clone()],
            Type::fun(a.clone(), Type::fun(a.clone(), Type::con("bool", 0))),
        ),
    );
    ts.add_value(
        "<=",
        Scheme::new(
            vec![a_tv.clone()],
            vec![ord_a.clone()],
            Type::fun(a.clone(), Type::fun(a.clone(), Type::con("bool", 0))),
        ),
    );
    ts.add_value(
        ">",
        Scheme::new(
            vec![a_tv.clone()],
            vec![ord_a.clone()],
            Type::fun(a.clone(), Type::fun(a.clone(), Type::con("bool", 0))),
        ),
    );
    ts.add_value(
        ">=",
        Scheme::new(
            vec![a_tv.clone()],
            vec![ord_a],
            Type::fun(a.clone(), Type::fun(a.clone(), Type::con("bool", 0))),
        ),
    );

    // Boolean operators
    let bool_ty = Type::con("bool", 0);
    ts.add_value(
        "&&",
        Scheme::new(vec![], vec![], Type::fun(bool_ty.clone(), Type::fun(bool_ty.clone(), bool_ty.clone()))),
    );
    ts.add_value(
        "||",
        Scheme::new(vec![], vec![], Type::fun(bool_ty.clone(), Type::fun(bool_ty.clone(), bool_ty.clone()))),
    );

    // Collection combinators (type class based)
    {
        let f_tv = fresh_tv(ts, "f");
        let a_tv = fresh_tv(ts, "a");
        let b_tv = fresh_tv(ts, "b");
        let f = Type::Var(f_tv.clone());
        let a = Type::Var(a_tv.clone());
        let b = Type::Var(b_tv.clone());
        let fa = Type::app(f.clone(), a.clone());
        let fb = Type::app(f.clone(), b.clone());

        ts.add_value(
            "map",
            Scheme::new(
                vec![f_tv.clone(), a_tv.clone(), b_tv.clone()],
                vec![Predicate::new("Functor", f.clone())],
                Type::fun(Type::fun(a.clone(), b.clone()), Type::fun(fa.clone(), fb.clone())),
            ),
        );
        ts.add_value(
            "foldl",
            Scheme::new(
                vec![f_tv.clone(), a_tv.clone(), b_tv.clone()],
                vec![Predicate::new("Foldable", f.clone())],
                Type::fun(
                    Type::fun(b.clone(), Type::fun(a.clone(), b.clone())),
                    Type::fun(b.clone(), Type::fun(fa.clone(), b.clone())),
                ),
            ),
        );
        ts.add_value(
            "foldr",
            Scheme::new(
                vec![f_tv.clone(), a_tv.clone(), b_tv.clone()],
                vec![Predicate::new("Foldable", f.clone())],
                Type::fun(
                    Type::fun(a.clone(), Type::fun(b.clone(), b.clone())),
                    Type::fun(b.clone(), Type::fun(fa.clone(), b.clone())),
                ),
            ),
        );
        ts.add_value(
            "fold",
            Scheme::new(
                vec![f_tv.clone(), a_tv.clone(), b_tv.clone()],
                vec![Predicate::new("Foldable", f.clone())],
                Type::fun(
                    Type::fun(b.clone(), Type::fun(a.clone(), b.clone())),
                    Type::fun(b.clone(), Type::fun(fa.clone(), b.clone())),
                ),
            ),
        );
        ts.add_value(
            "filter",
            Scheme::new(
                vec![f_tv.clone(), a_tv.clone()],
                vec![Predicate::new("Filterable", f.clone())],
                Type::fun(Type::fun(a.clone(), bool_ty.clone()), Type::fun(fa.clone(), fa.clone())),
            ),
        );
        ts.add_value(
            "filter_map",
            Scheme::new(
                vec![f_tv.clone(), a_tv.clone(), b_tv.clone()],
                vec![Predicate::new("Filterable", f.clone())],
                Type::fun(
                    Type::fun(a.clone(), option_of(b.clone())),
                    Type::fun(fa.clone(), fb.clone()),
                ),
            ),
        );
        ts.add_value(
            "flat_map",
            Scheme::new(
                vec![f_tv.clone(), a_tv.clone(), b_tv.clone()],
                vec![Predicate::new("Monad", f.clone())],
                Type::fun(Type::fun(a.clone(), fb.clone()), Type::fun(fa.clone(), fb.clone())),
            ),
        );
        ts.add_value(
            "and_then",
            Scheme::new(
                vec![f_tv.clone(), a_tv.clone(), b_tv.clone()],
                vec![Predicate::new("Monad", f.clone())],
                Type::fun(
                    Type::fun(a.clone(), fb.clone()),
                    Type::fun(fa.clone(), fb.clone()),
                ),
            ),
        );
        ts.add_value(
            "or_else",
            Scheme::new(
                vec![f_tv.clone(), a_tv.clone()],
                vec![Predicate::new("Alternative", f.clone())],
                Type::fun(Type::fun(fa.clone(), fa.clone()), Type::fun(fa.clone(), fa.clone())),
            ),
        );
        ts.add_value(
            "sum",
            Scheme::new(
                vec![f_tv.clone(), a_tv.clone()],
                vec![Predicate::new("Foldable", f.clone()), Predicate::new("AdditiveMonoid", a.clone())],
                Type::fun(fa.clone(), a.clone()),
            ),
        );
        ts.add_value(
            "mean",
            Scheme::new(
                vec![f_tv.clone(), a_tv.clone()],
                vec![Predicate::new("Foldable", f.clone()), Predicate::new("Field", a.clone())],
                Type::fun(fa.clone(), a.clone()),
            ),
        );
        ts.add_value(
            "count",
            Scheme::new(
                vec![f_tv.clone(), a_tv.clone()],
                vec![Predicate::new("Foldable", f.clone())],
                Type::fun(fa.clone(), Type::con("i32", 0)),
            ),
        );
        ts.add_value(
            "take",
            Scheme::new(
                vec![f_tv.clone(), a_tv.clone()],
                vec![Predicate::new("Sequence", f.clone())],
                Type::fun(Type::con("i32", 0), Type::fun(fa.clone(), fa.clone())),
            ),
        );
        ts.add_value(
            "skip",
            Scheme::new(
                vec![f_tv.clone(), a_tv.clone()],
                vec![Predicate::new("Sequence", f.clone())],
                Type::fun(Type::con("i32", 0), Type::fun(fa.clone(), fa.clone())),
            ),
        );
        ts.add_value(
            "zip",
            Scheme::new(
                vec![f_tv.clone(), a_tv.clone(), b_tv.clone()],
                vec![Predicate::new("Sequence", f.clone())],
                Type::fun(
                    fa.clone(),
                    Type::fun(fb.clone(), Type::app(f.clone(), Type::Tuple(vec![a.clone(), b.clone()]))),
                ),
            ),
        );
        ts.add_value(
            "unzip",
            Scheme::new(
                vec![f_tv.clone(), a_tv.clone(), b_tv.clone()],
                vec![Predicate::new("Sequence", f.clone())],
                Type::fun(
                    Type::app(f.clone(), Type::Tuple(vec![a.clone(), b.clone()])),
                    Type::Tuple(vec![fa.clone(), fb.clone()]),
                ),
            ),
        );
        ts.add_value(
            "min",
            Scheme::new(
                vec![f_tv.clone(), a_tv.clone()],
                vec![Predicate::new("Foldable", f.clone()), Predicate::new("Ord", a.clone())],
                Type::fun(fa.clone(), a.clone()),
            ),
        );
        ts.add_value(
            "max",
            Scheme::new(
                vec![f_tv, a_tv],
                vec![Predicate::new("Foldable", f.clone()), Predicate::new("Ord", a.clone())],
                Type::fun(fa.clone(), a.clone()),
            ),
        );
    }

    // Indexable access
    {
        let t_tv = fresh_tv(ts, "t");
        let a_tv = fresh_tv(ts, "a");
        let t = Type::Var(t_tv.clone());
        let a = Type::Var(a_tv.clone());
        let pred = Predicate::new("Indexable", indexable_of(t.clone(), a.clone()));
        ts.add_value(
            "get",
            Scheme::new(
                vec![t_tv, a_tv],
                vec![pred],
                Type::fun(Type::con("i32", 0), Type::fun(t, a)),
            ),
        );
    }

    // Option helpers
    {
        let a_tv = fresh_tv(ts, "a");
        let a = Type::Var(a_tv.clone());
        let opt_a = option_of(a.clone());
        ts.add_value(
            "is_some",
            Scheme::new(vec![a_tv.clone()], vec![], Type::fun(opt_a.clone(), bool_ty.clone())),
        );
        ts.add_value(
            "is_none",
            Scheme::new(vec![a_tv.clone()], vec![], Type::fun(opt_a.clone(), bool_ty.clone())),
        );
    }

    // Result helpers
    {
        let t_tv = fresh_tv(ts, "t");
        let e_tv = fresh_tv(ts, "e");
        let t = Type::Var(t_tv.clone());
        let e = Type::Var(e_tv.clone());
        let res_te = result_of(t.clone(), e.clone());
        ts.add_value(
            "is_ok",
            Scheme::new(vec![t_tv.clone(), e_tv.clone()], vec![], Type::fun(res_te.clone(), bool_ty.clone())),
        );
        ts.add_value(
            "is_err",
            Scheme::new(vec![t_tv.clone(), e_tv.clone()], vec![], Type::fun(res_te.clone(), bool_ty.clone())),
        );
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    fn tvar(id: TypeVarId, name: &str) -> Type {
        Type::Var(TypeVar::new(id, Some(name.to_string())))
    }

    fn dict_of(elem: Type) -> Type {
        Type::app(Type::con("Dict", 1), elem)
    }

    #[test]
    fn unify_simple() {
        let t1 = Type::fun(tvar(0, "a"), Type::con("u32", 0));
        let t2 = Type::fun(Type::con("u16", 0), tvar(1, "b"));
        let subst = unify(&t1, &t2).unwrap();
        assert_eq!(subst.get(&0), Some(&Type::con("u16", 0)));
        assert_eq!(subst.get(&1), Some(&Type::con("u32", 0)));
    }

    #[test]
    fn occurs_check_blocks_infinite_type() {
        let tv = TypeVar::new(0, Some("a".into()));
        let t = Type::fun(Type::Var(tv.clone()), Type::con("u8", 0));
        let err = bind(&tv, &t).unwrap_err();
        assert!(matches!(err, TypeError::Occurs(_, _)));
    }

    #[test]
    fn instantiate_and_generalize_round_trip() {
        let mut supply = TypeVarSupply::new();
        let a = Type::Var(supply.fresh(Some("a".into())));
        let scheme = generalize(&TypeEnv::new(), vec![], Type::fun(a.clone(), a.clone()));
        let (preds, inst) = instantiate(&scheme, &mut supply);
        assert!(preds.is_empty());
        if let Type::Fun(l, r) = inst {
            match (*l, *r) {
                (Type::Var(_), Type::Var(_)) => {}
                _ => panic!("expected polymorphic identity"),
            }
        } else {
            panic!("expected function type");
        }
    }

    #[test]
    fn entail_superclasses() {
        let ts = TypeSystem::with_prelude();
        let pred = Predicate::new("Semiring", Type::con("i32", 0));
        let given = [Predicate::new("AdditiveGroup", Type::con("i32", 0))];
        assert!(entails(&ts.classes, &given, &pred).unwrap());
    }

    #[test]
    fn entail_instances() {
        let ts = TypeSystem::with_prelude();
        let pred = Predicate::new("Field", Type::con("f32", 0));
        assert!(entails(&ts.classes, &[], &pred).unwrap());

        let pred_fail = Predicate::new("Field", Type::con("u32", 0));
        assert!(!entails(&ts.classes, &[], &pred_fail).unwrap());
    }

    #[test]
    fn prelude_injects_functions() {
        let ts = TypeSystem::with_prelude();
        let minus = ts.env.lookup("(-)").expect("minus in env");
        let div = ts.env.lookup("(/)").expect("div in env");
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
        let ts = TypeSystem::with_prelude();
        assert!(ts.env.lookup("Empty").is_some());
        assert!(ts.env.lookup("Cons").is_some());
        assert!(ts.env.lookup("Ok").is_some());
        assert!(ts.env.lookup("Err").is_some());
        assert!(ts.env.lookup("Some").is_some());
        assert!(ts.env.lookup("None").is_some());
    }

    fn parse_expr(code: &str) -> std::sync::Arc<rex_ast::expr::Expr> {
        let mut parser = rex_parser::Parser::new(rex_lexer::Token::tokenize(code).unwrap());
        parser.parse_program().unwrap().expr
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
        let mut ts = TypeSystem::with_prelude();
        let err = ts.infer(expr.as_ref()).unwrap_err();
        match err {
            TypeError::Spanned { span, error } => {
                assert_ne!(span, Span::default());
                assert!(matches!(*error, TypeError::UnknownVar(name) if name == "missing"));
            }
            other => panic!("expected spanned error, got {other:?}"),
        }
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
        let mut ts = TypeSystem::with_prelude();
        let (_preds, ty) = ts.infer(expr.as_ref()).unwrap();
        let expected = Type::Tuple(vec![
            Type::con("i32", 0),
            Type::con("f32", 0),
            Type::con("string", 0),
        ]);
        assert_eq!(ty, expected);
    }

    #[test]
    fn infer_additive_monoid_constraint() {
        let expr = parse_expr("\\x y -> x + y");
        let mut ts = TypeSystem::with_prelude();
        let (preds, ty) = ts.infer(expr.as_ref()).unwrap();
        assert_eq!(preds.len(), 1);
        assert_eq!(preds[0].class, "AdditiveMonoid");

        if let Type::Fun(a, rest) = ty {
            if let Type::Fun(b, c) = *rest {
                assert_eq!(*a, *b);
                assert_eq!(*b, *c);
                assert_eq!(preds[0].typ, *a);
                return;
            }
        }
        panic!("expected a -> a -> a");
    }

    #[test]
    fn infer_multiplicative_monoid_constraint() {
        let expr = parse_expr("\\x y -> x * y");
        let mut ts = TypeSystem::with_prelude();
        let (preds, ty) = ts.infer(expr.as_ref()).unwrap();
        assert_eq!(preds.len(), 1);
        assert_eq!(preds[0].class, "MultiplicativeMonoid");

        if let Type::Fun(a, rest) = ty {
            if let Type::Fun(b, c) = *rest {
                assert_eq!(*a, *b);
                assert_eq!(*b, *c);
                assert_eq!(preds[0].typ, *a);
                return;
            }
        }
        panic!("expected a -> a -> a");
    }

    #[test]
    fn infer_additive_group_constraint() {
        let expr = parse_expr("\\x y -> x - y");
        let mut ts = TypeSystem::with_prelude();
        let (preds, ty) = ts.infer(expr.as_ref()).unwrap();
        assert_eq!(preds.len(), 1);
        assert_eq!(preds[0].class, "AdditiveGroup");

        if let Type::Fun(a, rest) = ty {
            if let Type::Fun(b, c) = *rest {
                assert_eq!(*a, *b);
                assert_eq!(*b, *c);
                assert_eq!(preds[0].typ, *a);
                return;
            }
        }
        panic!("expected a -> a -> a");
    }

    #[test]
    fn infer_integral_constraint() {
        let expr = parse_expr("\\x y -> x % y");
        let mut ts = TypeSystem::with_prelude();
        let (preds, ty) = ts.infer(expr.as_ref()).unwrap();
        assert_eq!(preds.len(), 1);
        assert_eq!(preds[0].class, "Integral");

        if let Type::Fun(a, rest) = ty {
            if let Type::Fun(b, c) = *rest {
                assert_eq!(*a, *b);
                assert_eq!(*b, *c);
                assert_eq!(preds[0].typ, *a);
                return;
            }
        }
        panic!("expected a -> a -> a");
    }

    #[test]
    fn infer_literal_addition_defaults() {
        let expr = parse_expr("1 + 2");
        let mut ts = TypeSystem::with_prelude();
        let (preds, ty) = ts.infer(expr.as_ref()).unwrap();
        assert_eq!(ty, Type::con("i32", 0));
        assert_eq!(preds.len(), 1);
        assert_eq!(preds[0].class, "AdditiveMonoid");
        assert_eq!(preds[0].typ, Type::con("i32", 0));
        assert!(entails(&ts.classes, &[], &preds[0]).unwrap());
    }

    #[test]
    fn infer_mod_defaults() {
        let expr = parse_expr("1 % 2");
        let mut ts = TypeSystem::with_prelude();
        let (preds, ty) = ts.infer(expr.as_ref()).unwrap();
        assert_eq!(ty, Type::con("i32", 0));
        assert_eq!(preds.len(), 1);
        assert_eq!(preds[0].class, "Integral");
        assert_eq!(preds[0].typ, Type::con("i32", 0));
        assert!(entails(&ts.classes, &[], &preds[0]).unwrap());
    }

    #[test]
    fn infer_get_list_type() {
        let expr = parse_expr("get 1 [1, 2, 3]");
        let mut ts = TypeSystem::with_prelude();
        let (preds, ty) = ts.infer(expr.as_ref()).unwrap();
        assert_eq!(ty, Type::con("i32", 0));
        assert_eq!(preds.len(), 1);
        assert_eq!(preds[0].class, "Indexable");
        assert!(entails(&ts.classes, &[], &preds[0]).unwrap());
    }

    #[test]
    fn infer_get_tuple_type() {
        let expr = parse_expr("get 1 (1, 2, 3)");
        let mut ts = TypeSystem::with_prelude();
        let (preds, ty) = ts.infer(expr.as_ref()).unwrap();
        assert_eq!(ty, Type::con("i32", 0));
        assert_eq!(preds.len(), 1);
        assert_eq!(preds[0].class, "Indexable");
        assert!(entails(&ts.classes, &[], &preds[0]).unwrap());
    }

    #[test]
    fn infer_division_defaults() {
        let expr = parse_expr("1.0 / 2.0");
        let mut ts = TypeSystem::with_prelude();
        let (preds, ty) = ts.infer(expr.as_ref()).unwrap();
        assert_eq!(ty, Type::con("f32", 0));
        assert_eq!(preds.len(), 1);
        assert_eq!(preds[0].class, "Field");
        assert_eq!(preds[0].typ, Type::con("f32", 0));
        assert!(entails(&ts.classes, &[], &preds[0]).unwrap());
    }

    #[test]
    fn infer_unbound_variable_error() {
        let expr = parse_expr("missing");
        let mut ts = TypeSystem::with_prelude();
        let err = strip_span(ts.infer(expr.as_ref()).unwrap_err());
        assert!(matches!(err, TypeError::UnknownVar(name) if name == "missing"));
    }

    #[test]
    fn infer_if_branch_type_mismatch_error() {
        let expr = parse_expr(r#"if true then 1 else "no""#);
        let mut ts = TypeSystem::with_prelude();
        let err = strip_span(ts.infer(expr.as_ref()).unwrap_err());
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
        let mut ts = TypeSystem::with_prelude();
        let err = strip_span(ts.infer(expr.as_ref()).unwrap_err());
        assert!(matches!(err, TypeError::UnknownVar(name) if name == "Nope"));
    }

    #[test]
    fn infer_ambiguous_overload_error() {
        let mut ts = TypeSystem::new();
        let a = TypeVar::new(0, Some("a".into()));
        let b = TypeVar::new(1, Some("b".into()));
        let scheme_a = Scheme::new(vec![a.clone()], vec![], Type::Var(a));
        let scheme_b = Scheme::new(vec![b.clone()], vec![], Type::Var(b));
        ts.add_overload("dup", scheme_a);
        ts.add_overload("dup", scheme_b);
        let expr = parse_expr("dup");
        let err = strip_span(ts.infer(expr.as_ref()).unwrap_err());
        assert!(matches!(err, TypeError::AmbiguousOverload(name) if name == "dup"));
    }

    #[test]
    fn infer_if_cond_not_bool_error() {
        let expr = parse_expr("if 1 then 2 else 3");
        let mut ts = TypeSystem::with_prelude();
        let err = strip_span(ts.infer(expr.as_ref()).unwrap_err());
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
        let mut ts = TypeSystem::with_prelude();
        let err = strip_span(ts.infer(expr.as_ref()).unwrap_err());
        assert!(matches!(err, TypeError::Unification(_, _)));
    }

    #[test]
    fn infer_list_element_mismatch_error() {
        let expr = parse_expr("[1, true]");
        let mut ts = TypeSystem::with_prelude();
        let err = strip_span(ts.infer(expr.as_ref()).unwrap_err());
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
        let mut ts = TypeSystem::with_prelude();
        let err = strip_span(ts.infer(expr.as_ref()).unwrap_err());
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
        let mut ts = TypeSystem::with_prelude();
        let err = strip_span(ts.infer(expr.as_ref()).unwrap_err());
        assert!(matches!(err, TypeError::Unification(_, _)));
    }

    #[test]
    fn infer_pattern_constructor_arity_error() {
        let expr = parse_expr("match (Ok 1) when Ok x y -> x");
        let mut ts = TypeSystem::with_prelude();
        let err = strip_span(ts.infer(expr.as_ref()).unwrap_err());
        assert!(matches!(err, TypeError::UnsupportedExpr("pattern constructor")));
    }

    #[test]
    fn infer_match_arm_type_mismatch_error() {
        let expr = parse_expr(r#"match 1 when _ -> 1 when _ -> "no""#);
        let mut ts = TypeSystem::with_prelude();
        let err = strip_span(ts.infer(expr.as_ref()).unwrap_err());
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
        let mut ts = TypeSystem::with_prelude();
        let err = strip_span(ts.infer(expr.as_ref()).unwrap_err());
        assert!(matches!(err, TypeError::Unification(_, _)));
    }

    #[test]
    fn infer_dict_pattern_on_non_dict_error() {
        let expr = parse_expr("match 1 when {a} -> a");
        let mut ts = TypeSystem::with_prelude();
        let err = strip_span(ts.infer(expr.as_ref()).unwrap_err());
        assert!(matches!(err, TypeError::Unification(_, _)));
    }

    #[test]
    fn infer_cons_pattern_on_non_list_error() {
        let expr = parse_expr("match 1 when x:xs -> x");
        let mut ts = TypeSystem::with_prelude();
        let err = strip_span(ts.infer(expr.as_ref()).unwrap_err());
        assert!(matches!(err, TypeError::Unification(_, _)));
    }

    #[test]
    fn infer_apply_wrong_arg_type_error() {
        let expr = parse_expr("(\\x -> x + 1) true");
        let mut ts = TypeSystem::with_prelude();
        let err = strip_span(ts.infer(expr.as_ref()).unwrap_err());
        assert!(matches!(err, TypeError::Unification(_, _)));
    }

    #[test]
    fn infer_self_application_occurs_error() {
        let expr = parse_expr("\\x -> x x");
        let mut ts = TypeSystem::with_prelude();
        let err = strip_span(ts.infer(expr.as_ref()).unwrap_err());
        assert!(matches!(err, TypeError::Occurs(_, _)));
    }

    #[test]
    fn infer_apply_constructor_too_many_args_error() {
        let expr = parse_expr("Some 1 2");
        let mut ts = TypeSystem::with_prelude();
        let err = strip_span(ts.infer(expr.as_ref()).unwrap_err());
        assert!(matches!(err, TypeError::Unification(_, _)));
    }

    #[test]
    fn infer_operator_type_mismatch_error() {
        let expr = parse_expr("1 + true");
        let mut ts = TypeSystem::with_prelude();
        let err = strip_span(ts.infer(expr.as_ref()).unwrap_err());
        assert!(matches!(err, TypeError::Unification(_, _)));
    }

    #[test]
    fn infer_non_exhaustive_match_is_error() {
        let expr = parse_expr("match (Ok 1) when Ok x -> x");
        let mut ts = TypeSystem::with_prelude();
        let err = strip_span(ts.infer(expr.as_ref()).unwrap_err());
        assert!(matches!(err, TypeError::NonExhaustiveMatch { .. }));
    }

    #[test]
    fn infer_non_exhaustive_option_match_error() {
        let expr = parse_expr("match (Some 1) when Some x -> x");
        let mut ts = TypeSystem::with_prelude();
        let err = strip_span(ts.infer(expr.as_ref()).unwrap_err());
        match err {
            TypeError::NonExhaustiveMatch { missing, .. } => {
                assert_eq!(missing, vec!["None".to_string()]);
            }
            other => panic!("expected non-exhaustive match, got {other:?}"),
        }
    }

    #[test]
    fn infer_non_exhaustive_result_match_error() {
        let expr = parse_expr("match (Err 1) when Ok x -> x");
        let mut ts = TypeSystem::with_prelude();
        let err = strip_span(ts.infer(expr.as_ref()).unwrap_err());
        match err {
            TypeError::NonExhaustiveMatch { missing, .. } => {
                assert_eq!(missing, vec!["Err".to_string()]);
            }
            other => panic!("expected non-exhaustive match, got {other:?}"),
        }
    }

    #[test]
    fn infer_non_exhaustive_list_missing_empty_error() {
        let expr = parse_expr("match [1, 2] when x:xs -> x");
        let mut ts = TypeSystem::with_prelude();
        let err = strip_span(ts.infer(expr.as_ref()).unwrap_err());
        match err {
            TypeError::NonExhaustiveMatch { missing, .. } => {
                assert_eq!(missing, vec!["Empty".to_string()]);
            }
            other => panic!("expected non-exhaustive match, got {other:?}"),
        }
    }

    #[test]
    fn infer_non_exhaustive_list_missing_cons_error() {
        let expr = parse_expr("match [1] when [] -> 0");
        let mut ts = TypeSystem::with_prelude();
        let err = strip_span(ts.infer(expr.as_ref()).unwrap_err());
        match err {
            TypeError::NonExhaustiveMatch { missing, .. } => {
                assert_eq!(missing, vec!["Cons".to_string()]);
            }
            other => panic!("expected non-exhaustive match, got {other:?}"),
        }
    }

    #[test]
    fn infer_match_list_patterns_on_result_error() {
        let expr = parse_expr("match (Ok 1) when [] -> 0 when x:xs -> 1");
        let mut ts = TypeSystem::with_prelude();
        let err = strip_span(ts.infer(expr.as_ref()).unwrap_err());
        assert!(matches!(err, TypeError::Unification(_, _)));
    }

    #[test]
    fn infer_division_missing_field_instance() {
        let expr = parse_expr("1 / 2");
        let mut ts = TypeSystem::with_prelude();
        let (preds, ty) = ts.infer(expr.as_ref()).unwrap();
        assert_eq!(ty, Type::con("i32", 0));
        let pred = preds
            .iter()
            .find(|p| p.class == "Field" && p.typ == Type::con("i32", 0))
            .expect("expected Field i32 constraint");
        assert!(!entails(&ts.classes, &[], pred).unwrap());
    }

    #[test]
    fn infer_eq_missing_instance_for_dict() {
        let expr = parse_expr("{a = 1} == {a = 2}");
        let mut ts = TypeSystem::with_prelude();
        let (preds, _ty) = ts.infer(expr.as_ref()).unwrap();
        let dict_i32 = dict_of(Type::con("i32", 0));
        let pred = preds
            .iter()
            .find(|p| p.class == "Eq" && p.typ == dict_i32)
            .expect("expected Eq (Dict i32) constraint");
        assert!(!entails(&ts.classes, &[], pred).unwrap());
    }

    #[test]
    fn infer_min_missing_ord_instance_for_bool() {
        let expr = parse_expr("min [true]");
        let mut ts = TypeSystem::with_prelude();
        let (preds, _ty) = ts.infer(expr.as_ref()).unwrap();
        let pred = preds
            .iter()
            .find(|p| p.class == "Ord" && p.typ == Type::con("bool", 0))
            .expect("expected Ord bool constraint");
        assert!(!entails(&ts.classes, &[], pred).unwrap());
    }

    #[test]
    fn infer_map_missing_functor_instance_for_dict() {
        let expr = parse_expr(r#"map (\x -> x) {a = 1}"#);
        let mut ts = TypeSystem::with_prelude();
        let (preds, _ty) = ts.infer(expr.as_ref()).unwrap();
        let pred = preds
            .iter()
            .find(|p| p.class == "Functor" && p.typ == Type::con("Dict", 1))
            .expect("expected Functor Dict constraint");
        assert!(!entails(&ts.classes, &[], pred).unwrap());
    }
}

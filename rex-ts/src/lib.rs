//! Hindley-Milner type system with parametric polymorphism, type classes, and ADTs.
//! The goal is to provide a reusable library for building typing environments for Rex.
//! Features:
//! - Type variables, type constructors, function and tuple types.
//! - Schemes with quantified variables and class constraints.
//! - Type classes with superclass relationships and instance resolution.
//! - Basic ADTs (List, Result, Option) and numeric/string primitives in the prelude.
//! - Utilities to register additional function/type declarations (e.g. `(-)`, `(/)`).
#![forbid(unsafe_code)]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::{self, Display, Formatter};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use rex_ast::expr::{
    ClassDecl, ClassMethodSig, Decl, Expr, FnDecl, InstanceDecl, InstanceMethodImpl, Pattern,
    Scope, Symbol, TypeConstraint, TypeDecl, TypeExpr, intern,
};
use rex_lexer::{Token, span::Span};
use rex_parser::Parser;
use rpds::HashTrieMapSync;
use uuid::Uuid;

pub type TypeVarId = usize;

fn sym(name: &str) -> Symbol {
    intern(name)
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TypeVar {
    pub id: TypeVarId,
    pub name: Option<Symbol>,
}

impl TypeVar {
    pub fn new(id: TypeVarId, name: impl Into<Option<Symbol>>) -> Self {
        Self {
            id,
            name: name.into(),
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TypeConst {
    pub name: Symbol,
    pub arity: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Type(Arc<TypeKind>);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum TypeKind {
    Var(TypeVar),
    Con(TypeConst),
    App(Type, Type),
    Fun(Type, Type),
    Tuple(Vec<Type>),
    /// Record type `{a: T, b: U}`.
    ///
    /// Invariant: fields are sorted by name. This makes record equality and
    /// unification a cheap zip over two vectors, and it makes printing stable.
    Record(Vec<(Symbol, Type)>),
}

impl Type {
    pub fn new(kind: TypeKind) -> Self {
        Type(Arc::new(kind))
    }

    pub fn con(name: impl AsRef<str>, arity: usize) -> Self {
        Type::new(TypeKind::Con(TypeConst {
            name: intern(name.as_ref()),
            arity,
        }))
    }

    pub fn var(tv: TypeVar) -> Self {
        Type::new(TypeKind::Var(tv))
    }

    pub fn fun(a: Type, b: Type) -> Self {
        Type::new(TypeKind::Fun(a, b))
    }

    pub fn app(f: Type, arg: Type) -> Self {
        Type::new(TypeKind::App(f, arg))
    }

    pub fn tuple(elems: Vec<Type>) -> Self {
        Type::new(TypeKind::Tuple(elems))
    }

    pub fn record(mut fields: Vec<(Symbol, Type)>) -> Self {
        // Canonicalize records so downstream code can rely on “same shape means
        // same ordering”. (This is a correctness invariant, not a nicety.)
        fields.sort_by(|a, b| a.0.as_ref().cmp(b.0.as_ref()));
        Type::new(TypeKind::Record(fields))
    }

    pub fn as_ref(&self) -> &TypeKind {
        &self.0
    }

    fn apply_with_change(&self, s: &Subst) -> (Type, bool) {
        match self.as_ref() {
            TypeKind::Var(tv) => match s.get(&tv.id) {
                Some(ty) => (ty.clone(), true),
                None => (self.clone(), false),
            },
            TypeKind::Con(_) => (self.clone(), false),
            TypeKind::App(l, r) => {
                let (l_new, l_changed) = l.apply_with_change(s);
                let (r_new, r_changed) = r.apply_with_change(s);
                if l_changed || r_changed {
                    (Type::app(l_new, r_new), true)
                } else {
                    (self.clone(), false)
                }
            }
            TypeKind::Fun(a, b) => {
                let (a_new, a_changed) = a.apply_with_change(s);
                let (b_new, b_changed) = b.apply_with_change(s);
                if a_changed || b_changed {
                    (Type::fun(a_new, b_new), true)
                } else {
                    (self.clone(), false)
                }
            }
            TypeKind::Tuple(ts) => {
                let mut changed = false;
                let mut out = Vec::with_capacity(ts.len());
                for t in ts {
                    let (t_new, t_changed) = t.apply_with_change(s);
                    changed |= t_changed;
                    out.push(t_new);
                }
                if changed {
                    (Type::new(TypeKind::Tuple(out)), true)
                } else {
                    (self.clone(), false)
                }
            }
            TypeKind::Record(fields) => {
                let mut changed = false;
                let mut out = Vec::with_capacity(fields.len());
                for (k, v) in fields {
                    let (v_new, v_changed) = v.apply_with_change(s);
                    changed |= v_changed;
                    out.push((k.clone(), v_new));
                }
                if changed {
                    (Type::new(TypeKind::Record(out)), true)
                } else {
                    (self.clone(), false)
                }
            }
        }
    }
}

impl std::ops::Deref for Type {
    type Target = TypeKind;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Display for Type {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.as_ref() {
            TypeKind::Var(tv) => match &tv.name {
                Some(name) => write!(f, "{}", name),
                None => write!(f, "t{}", tv.id),
            },
            TypeKind::Con(c) => write!(f, "{}", c.name),
            TypeKind::App(l, r) => write!(f, "({} {})", l, r),
            TypeKind::Fun(a, b) => write!(f, "({} -> {})", a, b),
            TypeKind::Tuple(elems) => {
                write!(f, "(")?;
                for (i, t) in elems.iter().enumerate() {
                    write!(f, "{}", t)?;
                    if i + 1 < elems.len() {
                        write!(f, ", ")?;
                    }
                }
                write!(f, ")")
            }
            TypeKind::Record(fields) => {
                write!(f, "{{")?;
                for (i, (name, ty)) in fields.iter().enumerate() {
                    write!(f, "{}: {}", name, ty)?;
                    if i + 1 < fields.len() {
                        write!(f, ", ")?;
                    }
                }
                write!(f, "}}")
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Predicate {
    pub class: Symbol,
    pub typ: Type,
}

impl Predicate {
    pub fn new(class: impl AsRef<str>, typ: Type) -> Self {
        Self {
            class: intern(class.as_ref()),
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

pub type Subst = HashTrieMapSync<TypeVarId, Type>;

pub trait Types: Sized {
    fn apply(&self, s: &Subst) -> Self;
    fn ftv(&self) -> HashSet<TypeVarId>;
}

impl Types for Type {
    fn apply(&self, s: &Subst) -> Self {
        self.apply_with_change(s).0
    }

    fn ftv(&self) -> HashSet<TypeVarId> {
        match self.as_ref() {
            TypeKind::Var(tv) => [tv.id].into_iter().collect(),
            TypeKind::Con(_) => HashSet::new(),
            TypeKind::App(l, r) => l.ftv().union(&r.ftv()).copied().collect(),
            TypeKind::Fun(a, b) => a.ftv().union(&b.ftv()).copied().collect(),
            TypeKind::Tuple(ts) => ts.iter().flat_map(Types::ftv).collect(),
            TypeKind::Record(fields) => fields.iter().flat_map(|(_, ty)| ty.ftv()).collect(),
        }
    }
}

impl Types for Predicate {
    fn apply(&self, s: &Subst) -> Self {
        Predicate {
            class: self.class.clone(),
            typ: self.typ.apply(s),
        }
    }

    fn ftv(&self) -> HashSet<TypeVarId> {
        self.typ.ftv()
    }
}

impl Types for Scheme {
    fn apply(&self, s: &Subst) -> Self {
        let mut s_pruned = Subst::new_sync();
        for (k, v) in s.iter() {
            if !self.vars.iter().any(|var| var.id == *k) {
                s_pruned = s_pruned.insert(*k, v.clone());
            }
        }
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
            TypedExprKind::Project { expr, field } => TypedExprKind::Project {
                expr: Box::new(expr.apply(s)),
                field: field.clone(),
            },
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
    Dict(BTreeMap<Symbol, TypedExpr>),
    Var {
        name: Symbol,
        overloads: Vec<Type>,
    },
    App(Box<TypedExpr>, Box<TypedExpr>),
    Project {
        expr: Box<TypedExpr>,
        field: Symbol,
    },
    Lam {
        param: Symbol,
        body: Box<TypedExpr>,
    },
    Let {
        name: Symbol,
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

fn subst_is_empty(s: &Subst) -> bool {
    s.iter().next().is_none()
}

fn dedup_preds(preds: Vec<Predicate>) -> Vec<Predicate> {
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(preds.len());
    for pred in preds {
        if seen.insert(pred.clone()) {
            out.push(pred);
        }
    }
    out
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TypeError {
    #[error("types do not unify: {0} vs {1}")]
    Unification(String, String),
    #[error("occurs check failed for {0} in {1}")]
    Occurs(TypeVarId, String),
    #[error("unknown class {0}")]
    UnknownClass(Symbol),
    #[error("no instance for {0} {1}")]
    NoInstance(Symbol, String),
    #[error("unknown type {0}")]
    UnknownTypeName(Symbol),
    #[error("duplicate value definition `{0}`")]
    DuplicateValue(Symbol),
    #[error("class `{class}` must have exactly one type parameter (got {got})")]
    InvalidClassArity { class: Symbol, got: usize },
    #[error("duplicate class method `{0}`")]
    DuplicateClassMethod(Symbol),
    #[error("unknown method `{method}` in instance of class `{class}`")]
    UnknownInstanceMethod { class: Symbol, method: Symbol },
    #[error("missing implementation of `{method}` for instance of class `{class}`")]
    MissingInstanceMethod { class: Symbol, method: Symbol },
    #[error("instance method `{method}` requires constraint {class} {typ}, but it is not in the instance context")]
    MissingInstanceConstraint {
        method: Symbol,
        class: Symbol,
        typ: String,
    },
    #[error("unbound variable {0}")]
    UnknownVar(Symbol),
    #[error("ambiguous overload for {0}")]
    AmbiguousOverload(Symbol),
    #[error("unsupported expression {0}")]
    UnsupportedExpr(&'static str),
    #[error("unknown field `{field}` on {typ}")]
    UnknownField { field: Symbol, typ: String },
    #[error("field `{field}` is not definitely available on {typ}")]
    FieldNotKnown { field: Symbol, typ: String },
    #[error("non-exhaustive match for {typ}: missing {missing:?}")]
    NonExhaustiveMatch { typ: String, missing: Vec<Symbol> },
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

#[derive(Default, Debug)]
struct Unifier {
    // `subs[id] = Some(t)` means type variable `id` has been bound to `t`.
    //
    // This is intentionally a dense `Vec` rather than a `HashMap`: inference
    // generates `TypeVarId`s from a monotonic counter, so the common case is
    // “small id space, lots of lookups”. This makes the cost model obvious:
    // you pay O(max_id) space, and you get O(1) binds/queries.
    subs: Vec<Option<Type>>,
}

impl Unifier {
    fn new() -> Self {
        Self { subs: Vec::new() }
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

    fn apply_type(&mut self, ty: &Type) -> Type {
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

    fn unify(&mut self, t1: &Type, t2: &Type) -> Result<(), TypeError> {
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
                TypeKind::Con(c) if c.name.as_ref() == "Dict" => {
                    let elem_ty = record_elem_type_unifier(fields, self)?;
                    self.unify(arg, &elem_ty)
                }
                TypeKind::Var(tv) => {
                    self.unify(&Type::new(TypeKind::Var(tv.clone())), &Type::con("Dict", 1))?;
                    let elem_ty = record_elem_type_unifier(fields, self)?;
                    self.unify(arg, &elem_ty)
                }
                _ => Err(TypeError::Unification(t1.to_string(), t2.to_string())),
            },
            _ => Err(TypeError::Unification(t1.to_string(), t2.to_string())),
        }
    }

    fn into_subst(mut self) -> Subst {
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

fn record_elem_type_unifier(
    fields: &[(Symbol, Type)],
    unifier: &mut Unifier,
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

fn bind(tv: &TypeVar, t: &Type) -> Result<Subst, TypeError> {
    if let TypeKind::Var(var) = t.as_ref() {
        if var.id == tv.id {
            return Ok(Subst::new_sync());
        }
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
            TypeKind::Con(c) if c.name.as_ref() == "Dict" => {
                let (s_fields, elem_ty) = record_elem_type(fields)?;
                let s_arg = unify(&arg.apply(&s_fields), &elem_ty)?;
                Ok(compose_subst(s_arg, s_fields))
            }
            TypeKind::Var(tv) => {
                let s_head = bind(tv, &Type::con("Dict", 1))?;
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

#[derive(Default, Debug, Clone)]
pub struct TypeEnv {
    pub values: HashTrieMapSync<Symbol, Vec<Scheme>>,
}

impl TypeEnv {
    pub fn new() -> Self {
        Self {
            values: HashTrieMapSync::new_sync(),
        }
    }

    pub fn extend(&mut self, name: Symbol, scheme: Scheme) {
        self.values = self.values.insert(name, vec![scheme]);
    }

    pub fn extend_overload(&mut self, name: Symbol, scheme: Scheme) {
        let mut schemes = self.values.get(&name).cloned().unwrap_or_default();
        schemes.push(scheme);
        self.values = self.values.insert(name, schemes);
    }

    pub fn lookup(&self, name: &Symbol) -> Option<&[Scheme]> {
        self.values.get(name).map(|schemes| schemes.as_slice())
    }
}

impl Types for TypeEnv {
    fn apply(&self, s: &Subst) -> Self {
        let mut values = HashTrieMapSync::new_sync();
        for (k, v) in self.values.iter() {
            let updated = v
                .iter()
                .map(|scheme| {
                    // Most schemes in environments are monomorphic. Don't walk
                    // and rebuild trees unless we actually have work to do.
                    if scheme.vars.is_empty() && !subst_is_empty(s) {
                        scheme.apply(s)
                    } else {
                        scheme.clone()
                    }
                })
                .collect();
            values = values.insert(k.clone(), updated);
        }
        TypeEnv { values }
    }

    fn ftv(&self) -> HashSet<TypeVarId> {
        self.values
            .iter()
            .flat_map(|(_, schemes)| schemes.iter().flat_map(Types::ftv))
            .collect()
    }
}

#[derive(Clone, Debug)]
struct KnownVariant {
    adt: Symbol,
    variant: Symbol,
}

type KnownVariants = HashMap<Symbol, KnownVariant>;

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

fn apply_scheme_with_unifier(scheme: &Scheme, unifier: &mut Unifier) -> Scheme {
    let preds = scheme
        .preds
        .iter()
        .map(|pred| Predicate::new(pred.class.clone(), unifier.apply_type(&pred.typ)))
        .collect();
    let typ = unifier.apply_type(&scheme.typ);
    Scheme::new(scheme.vars.clone(), preds, typ)
}

fn scheme_ftv_with_unifier(scheme: &Scheme, unifier: &mut Unifier) -> HashSet<TypeVarId> {
    let mut ftv = unifier.apply_type(&scheme.typ).ftv();
    for pred in &scheme.preds {
        ftv.extend(unifier.apply_type(&pred.typ).ftv());
    }
    for var in &scheme.vars {
        ftv.remove(&var.id);
    }
    ftv
}

fn env_ftv_with_unifier(env: &TypeEnv, unifier: &mut Unifier) -> HashSet<TypeVarId> {
    let mut out = HashSet::new();
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
    unifier: &mut Unifier,
) -> Scheme {
    // This is `generalize`, but operating in the “imperative unifier world”.
    // It avoids constructing intermediate `Subst` maps while inference is
    // still mutating type variables.
    let preds: Vec<Predicate> = preds
        .into_iter()
        .map(|pred| Predicate::new(pred.class, unifier.apply_type(&pred.typ)))
        .collect();
    let typ = unifier.apply_type(&typ);
    let mut vars: Vec<TypeVar> = typ
        .ftv()
        .union(&preds.ftv())
        .copied()
        .collect::<HashSet<_>>()
        .difference(&env_ftv_with_unifier(env, unifier))
        .cloned()
        .map(|id| TypeVar::new(id, None))
        .collect();
    vars.sort_by_key(|v| v.id);
    Scheme::new(vars, preds, typ)
}

/// Turn a monotype `typ` (plus constraints `preds`) into a polymorphic `Scheme`
/// by quantifying over the type variables not free in `env`.
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
    // Instantiate replaces all quantified variables with fresh unification
    // variables, preserving the original name as a debugging hint.
    let mut subst = Subst::new_sync();
    for v in &scheme.vars {
        subst = subst.insert(v.id, Type::var(supply.fresh(v.name.clone())));
    }
    (scheme.preds.apply(&subst), scheme.typ.apply(&subst))
}

/// A named type parameter for an ADT (e.g. `a` in `List a`).
#[derive(Clone, Debug)]
pub struct AdtParam {
    pub name: Symbol,
    pub var: TypeVar,
}

/// A single ADT variant with zero or more constructor arguments.
#[derive(Clone, Debug)]
pub struct AdtVariant {
    pub name: Symbol,
    pub args: Vec<Type>,
}

/// A type declaration for an algebraic data type.
///
/// This only describes the *type* surface (params + variants). It does not
/// introduce any runtime values by itself. Runtime values are created by
/// injecting constructor schemes into the environment (see `inject_adt`).
#[derive(Clone, Debug)]
pub struct AdtDecl {
    pub name: Symbol,
    pub params: Vec<AdtParam>,
    pub variants: Vec<AdtVariant>,
}

impl AdtDecl {
    pub fn new(name: &Symbol, param_names: &[Symbol], supply: &mut TypeVarSupply) -> Self {
        let params = param_names
            .iter()
            .map(|p| AdtParam {
                name: p.clone(),
                var: supply.fresh(Some(p.clone())),
            })
            .collect();
        Self {
            name: name.clone(),
            params,
            variants: Vec::new(),
        }
    }

    pub fn param_type(&self, name: &Symbol) -> Option<Type> {
        self.params
            .iter()
            .find(|p| &p.name == name)
            .map(|p| Type::var(p.var.clone()))
    }

    pub fn add_variant(&mut self, name: Symbol, args: Vec<Type>) {
        self.variants.push(AdtVariant { name, args });
    }

    pub fn result_type(&self) -> Type {
        let mut ty = Type::con(&self.name, self.params.len());
        for param in &self.params {
            ty = Type::app(ty, Type::var(param.var.clone()));
        }
        ty
    }

    /// Build constructor schemes of the form:
    /// `C :: a1 -> a2 -> ... -> T params`.
    pub fn constructor_schemes(&self) -> Vec<(Symbol, Scheme)> {
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
    pub supers: Vec<Symbol>,
}

impl Class {
    pub fn new(supers: Vec<Symbol>) -> Self {
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
    pub classes: HashMap<Symbol, Class>,
    pub instances: HashMap<Symbol, Vec<Instance>>,
}

impl ClassEnv {
    pub fn new() -> Self {
        Self {
            classes: HashMap::new(),
            instances: HashMap::new(),
        }
    }

    pub fn add_class(&mut self, name: Symbol, supers: Vec<Symbol>) {
        self.classes.insert(name, Class::new(supers));
    }

    pub fn add_instance(&mut self, class: Symbol, inst: Instance) {
        self.instances.entry(class).or_default().push(inst);
    }

    pub fn supers_of(&self, class: &Symbol) -> Vec<Symbol> {
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
    pub adts: HashMap<Symbol, AdtDecl>,
    pub class_info: HashMap<Symbol, ClassInfo>,
    pub class_methods: HashMap<Symbol, ClassMethodInfo>,
    pub supply: TypeVarSupply,
}

/// Semantic information about a type class declaration, derived from Rex source.
///
/// Design notes (WARM):
/// - We keep this explicit and data-oriented: it makes review easy and keeps costs visible.
/// - Today Rex classes are unary (one type parameter). That matches the existing `Predicate`
///   model and avoids adding half-finished multi-parameter machinery.
#[derive(Clone, Debug)]
pub struct ClassInfo {
    pub name: Symbol,
    pub param: Symbol,
    pub supers: Vec<Symbol>,
    pub methods: BTreeMap<Symbol, Scheme>,
}

#[derive(Clone, Debug)]
pub struct ClassMethodInfo {
    pub class: Symbol,
    pub scheme: Scheme,
}

#[derive(Clone, Debug)]
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
            adts: HashMap::new(),
            class_info: HashMap::new(),
            class_methods: HashMap::new(),
            supply: TypeVarSupply::new(),
        }
    }

    pub fn with_prelude() -> Self {
        let mut ts = TypeSystem::new();
        build_prelude(&mut ts);
        ts
    }

    pub fn add_value(&mut self, name: impl AsRef<str>, scheme: Scheme) {
        self.env.extend(sym(name.as_ref()), scheme);
    }

    pub fn add_overload(&mut self, name: impl AsRef<str>, scheme: Scheme) {
        self.env.extend_overload(sym(name.as_ref()), scheme);
    }

    pub fn inject_class(&mut self, name: impl AsRef<str>, supers: Vec<Symbol>) {
        self.classes.add_class(sym(name.as_ref()), supers);
    }

    pub fn inject_instance(&mut self, class: impl AsRef<str>, inst: Instance) {
        self.classes.add_instance(sym(class.as_ref()), inst);
    }

    pub fn inject_class_decl(&mut self, decl: &ClassDecl) -> Result<(), TypeError> {
        let span = decl.span;
        (|| {
            if decl.params.len() != 1 {
                return Err(TypeError::InvalidClassArity {
                    class: decl.name.clone(),
                    got: decl.params.len(),
                });
            }
            let param = decl.params[0].clone();

            // Register the superclass relationships in the class environment.
            //
            // We only accept `<= C param` style superclasses for now. Anything
            // fancier would require storing type-level relationships in `ClassEnv`,
            // which Rex does not currently model.
            let mut supers = Vec::with_capacity(decl.supers.len());
            for sup in &decl.supers {
                let mut vars = HashMap::new();
                let param_tv = self.supply.fresh(Some(param.clone()));
                vars.insert(param.clone(), param_tv.clone());
                let sup_ty =
                    type_from_annotation_expr_vars(&self.adts, &sup.typ, &mut vars, &mut self.supply)?;
                if sup_ty != Type::var(param_tv) {
                    return Err(TypeError::UnsupportedExpr(
                        "superclass constraints must be of the form `<= C a`",
                    ));
                }
                supers.push(sup.class.clone());
            }

            self.classes.add_class(decl.name.clone(), supers.clone());

            let mut methods = BTreeMap::new();
            for ClassMethodSig { name, typ } in &decl.methods {
                if self.env.lookup(name).is_some() || self.class_methods.contains_key(name) {
                    return Err(TypeError::DuplicateClassMethod(name.clone()));
                }

                let mut vars: HashMap<Symbol, TypeVar> = HashMap::new();
                let param_tv = self.supply.fresh(Some(param.clone()));
                vars.insert(param.clone(), param_tv.clone());

                let ty =
                    type_from_annotation_expr_vars(&self.adts, typ, &mut vars, &mut self.supply)?;

                let mut scheme_vars: Vec<TypeVar> = vars.values().cloned().collect();
                scheme_vars.sort_by_key(|tv| tv.id);
                scheme_vars.dedup_by_key(|tv| tv.id);

                let class_pred = Predicate {
                    class: decl.name.clone(),
                    typ: Type::var(param_tv),
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
                    param,
                    supers,
                    methods,
                },
            );
            Ok(())
        })()
        .map_err(|err| with_span(&span, err))
    }

    pub fn inject_instance_decl(&mut self, decl: &InstanceDecl) -> Result<PreparedInstanceDecl, TypeError> {
        let span = decl.span;
        (|| {
            let class = decl.class.clone();
            if !self.class_info.contains_key(&class) && !self.classes.classes.contains_key(&class) {
                return Err(TypeError::UnknownClass(class));
            }

            let mut vars: HashMap<Symbol, TypeVar> = HashMap::new();
            let head =
                type_from_annotation_expr_vars(&self.adts, &decl.head, &mut vars, &mut self.supply)?;
            let context =
                predicates_from_constraints(&self.adts, &decl.context, &mut vars, &mut self.supply)?;

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
        .map_err(|err| with_span(&span, err))
    }

    pub fn inject_fn_decl(&mut self, decl: &FnDecl) -> Result<(), TypeError> {
        let span = decl.span;
        (|| {
            if self.env.lookup(&decl.name.name).is_some() {
                return Err(TypeError::DuplicateValue(decl.name.name.clone()));
            }

            // Lower a `fn` declaration to the same lambda shape used by
            // `Program::expr_with_fns`, then infer it and generalize into
            // a top-level scheme.
            let mut lam_body = decl.body.clone();
            let mut lam_end = lam_body.span().end;
            for (idx, (param, ann)) in decl.params.iter().enumerate().rev() {
                let lam_constraints = if idx == 0 {
                    decl.constraints.clone()
                } else {
                    Vec::new()
                };
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

            let mut sig = decl.ret.clone();
            for (_, ann) in decl.params.iter().rev() {
                let span = Span::from_begin_end(ann.span().begin, sig.span().end);
                sig = TypeExpr::Fun(span, Box::new(ann.clone()), Box::new(sig));
            }

            let (typed, preds, inferred) = self.infer_typed(lam_body.as_ref())?;

            let mut ann_vars: HashMap<Symbol, TypeVar> = HashMap::new();
            let expected = type_from_annotation_expr_vars(
                &self.adts,
                &sig,
                &mut ann_vars,
                &mut self.supply,
            )?;
            let s = unify(&inferred, &expected)?;
            let preds = preds.apply(&s);
            let inferred = inferred.apply(&s);

            // The scheme is the generalized inferred type with its required
            // class predicates. This is the same shape a `let` binding would
            // produce, but done explicitly so other decls (e.g. instances) can
            // typecheck against it.
            let scheme = generalize(&self.env, preds, inferred);
            self.env.extend(decl.name.name.clone(), scheme);

            // `typed` is intentionally unused here. Typechecking instance
            // methods only needs the scheme in the environment; evaluation
            // happens in `rex-engine`.
            let _ = typed;
            Ok(())
        })()
        .map_err(|err| with_span(&span, err))
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
        let class_pred = preds
            .iter()
            .find(|p| &p.class == class)
            .ok_or_else(|| TypeError::UnsupportedExpr("class method scheme missing class predicate"))?;
        let s = unify(&class_pred.typ, head)?;
        Ok(typ.apply(&s))
    }

    pub fn typecheck_instance_method(
        &mut self,
        prepared: &PreparedInstanceDecl,
        method: &InstanceMethodImpl,
    ) -> Result<TypedExpr, TypeError> {
        let expected = self.instantiate_class_method_for_head(&prepared.class, &method.name, &prepared.head)?;
        let (typed, preds, actual) = self.infer_typed(method.body.as_ref())?;
        let s = unify(&actual, &expected)?;
        let typed = typed.apply(&s);
        let preds = preds.apply(&s);

        // The only legal “given” constraints inside an instance method are the
        // instance context (plus superclass closure). We do *not* allow instance
        // search for non-ground constraints here, because that would be unsound:
        // a type variable would unify with any concrete instance head.
        let mut given = prepared.context.clone();
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
            } else if !given.iter().any(|p| p.class == pred.class && p.typ == pred.typ) {
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
    pub fn inject_adt(&mut self, adt: &AdtDecl) {
        self.adts.insert(adt.name.clone(), adt.clone());
        for (name, scheme) in adt.constructor_schemes() {
            self.register_value_scheme(&name, scheme);
        }
    }

    pub fn adt_from_decl(&mut self, decl: &TypeDecl) -> Result<AdtDecl, TypeError> {
        let mut adt = AdtDecl::new(&decl.name, &decl.params, &mut self.supply);
        let mut param_map: HashMap<Symbol, TypeVar> = HashMap::new();
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
        params: &HashMap<Symbol, TypeVar>,
        expr: &TypeExpr,
    ) -> Result<Type, TypeError> {
        let span = *expr.span();
        let res = (|| match expr {
            TypeExpr::Name(_, name) => {
                if let Some(tv) = params.get(name) {
                    Ok(Type::var(tv.clone()))
                } else {
                    let name = normalize_type_name(name);
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
                Ok(Type::app(fty, aty))
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
        res.map_err(|err| with_span(&span, err))
    }

    fn type_arity(&self, decl: &TypeDecl, name: &Symbol) -> Option<usize> {
        if &decl.name == name {
            return Some(decl.params.len());
        }
        if let Some(adt) = self.adts.get(name) {
            return Some(adt.params.len());
        }
        match name.as_ref() {
            "u8" | "u16" | "u32" | "u64" | "i8" | "i16" | "i32" | "i64" | "f32" | "f64"
            | "bool" | "string" | "uuid" | "datetime" => Some(0),
            "Dict" | "Array" => Some(1),
            _ => None,
        }
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

    pub fn infer_typed(
        &mut self,
        expr: &Expr,
    ) -> Result<(TypedExpr, Vec<Predicate>, Type), TypeError> {
        let known = KnownVariants::new();
        let mut unifier = Unifier::new();
        let (preds, t, typed) = infer_expr(
            &mut unifier,
            &mut self.supply,
            &self.env,
            &self.adts,
            &known,
            expr,
        )
        .map_err(|err| with_span(expr.span(), err))?;
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
        Ok((typed, preds, t))
    }

    pub fn infer(&mut self, expr: &Expr) -> Result<(Vec<Predicate>, Type), TypeError> {
        let known = KnownVariants::new();
        let mut unifier = Unifier::new();
        let (preds, t) = infer_expr_type(
            &mut unifier,
            &mut self.supply,
            &self.env,
            &self.adts,
            &known,
            expr,
        )
        .map_err(|err| with_span(expr.span(), err))?;
        let subst = unifier.into_subst();
        let mut preds = dedup_preds(preds.apply(&subst));
        let mut t = t.apply(&subst);
        let improve = improve_indexable(&preds)?;
        if !subst_is_empty(&improve) {
            preds = dedup_preds(preds.apply(&improve));
            t = t.apply(&improve);
        }
        Ok((preds, t))
    }
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
            TypeKind::Con(tc) if tc.name.as_ref() == "List" || tc.name.as_ref() == "Array" => {
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

fn type_from_annotation_expr(
    adts: &HashMap<Symbol, AdtDecl>,
    expr: &TypeExpr,
) -> Result<Type, TypeError> {
    let span = *expr.span();
    let res = (|| match expr {
        TypeExpr::Name(_, name) => {
            let name = normalize_type_name(name);
            match annotation_type_arity(adts, &name) {
                Some(arity) => Ok(Type::con(name, arity)),
                None => Err(TypeError::UnknownTypeName(name)),
            }
        }
        TypeExpr::App(_, fun, arg) => {
            let fty = type_from_annotation_expr(adts, fun)?;
            let aty = type_from_annotation_expr(adts, arg)?;
            Ok(Type::app(fty, aty))
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
    res.map_err(|err| with_span(&span, err))
}

fn type_from_annotation_expr_vars(
    adts: &HashMap<Symbol, AdtDecl>,
    expr: &TypeExpr,
    vars: &mut HashMap<Symbol, TypeVar>,
    supply: &mut TypeVarSupply,
) -> Result<Type, TypeError> {
    let span = *expr.span();
    let res = (|| match expr {
        TypeExpr::Name(_, name) => {
            let name = normalize_type_name(name);
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
            Ok(Type::app(fty, aty))
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
    res.map_err(|err| with_span(&span, err))
}

fn annotation_type_arity(adts: &HashMap<Symbol, AdtDecl>, name: &Symbol) -> Option<usize> {
    if let Some(adt) = adts.get(name) {
        return Some(adt.params.len());
    }
    match name.as_ref() {
        "u8" | "u16" | "u32" | "u64" | "i8" | "i16" | "i32" | "i64" | "f32" | "f64" | "bool"
        | "string" | "uuid" | "datetime" => Some(0),
        "Dict" | "Array" => Some(1),
        _ => None,
    }
}

fn normalize_type_name(name: &Symbol) -> Symbol {
    if name.as_ref() == "str" {
        sym("string")
    } else {
        name.clone()
    }
}

fn collect_lambda_chain(
    expr: &Expr,
) -> (Vec<(Symbol, Option<TypeExpr>)>, Vec<TypeConstraint>, &Expr) {
    let mut params = Vec::new();
    let mut constraints = Vec::new();
    let mut cur = expr;
    let mut first = true;
    loop {
        match cur {
            Expr::Lam(_, _scope, param, ann, lam_constraints, body) => {
                if !first && !lam_constraints.is_empty() {
                    break;
                }
                if first {
                    constraints = lam_constraints.clone();
                }
                params.push((param.name.clone(), ann.clone()));
                cur = body.as_ref();
                first = false;
            }
            _ => break,
        }
    }
    (params, constraints, cur)
}

fn predicates_from_constraints(
    adts: &HashMap<Symbol, AdtDecl>,
    constraints: &[TypeConstraint],
    vars: &mut HashMap<Symbol, TypeVar>,
    supply: &mut TypeVarSupply,
) -> Result<Vec<Predicate>, TypeError> {
    let mut out = Vec::with_capacity(constraints.len());
    for constraint in constraints {
        let ty = type_from_annotation_expr_vars(adts, &constraint.typ, vars, supply)?;
        out.push(Predicate::new(constraint.class.clone(), ty));
    }
    Ok(out)
}

fn infer_expr_type(
    unifier: &mut Unifier,
    supply: &mut TypeVarSupply,
    env: &TypeEnv,
    adts: &HashMap<Symbol, AdtDecl>,
    known: &KnownVariants,
    expr: &Expr,
) -> Result<(Vec<Predicate>, Type), TypeError> {
    let span = *expr.span();
    let res = (|| match expr {
        Expr::Bool(_, _) => Ok((vec![], Type::con("bool", 0))),
        Expr::Uint(_, _) => Ok((vec![], Type::con("i32", 0))),
        Expr::Int(_, _) => Ok((vec![], Type::con("i32", 0))),
        Expr::Float(_, _) => Ok((vec![], Type::con("f32", 0))),
        Expr::String(_, _) => Ok((vec![], Type::con("string", 0))),
        Expr::Uuid(_, _) => Ok((vec![], Type::con("uuid", 0))),
        Expr::DateTime(_, _) => Ok((vec![], Type::con("datetime", 0))),
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
            let mut ann_vars = HashMap::new();
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
        Expr::App(_, f, x) => {
            let (p1, t1) = infer_expr_type(unifier, supply, env, adts, known, f)?;
            let arg_hint = match unifier.apply_type(&t1).as_ref() {
                TypeKind::Fun(arg, _) => Some(arg.clone()),
                _ => None,
            };
            let (p2, t2) = match (arg_hint, x.as_ref()) {
                (Some(arg_hint), Expr::Dict(_, kvs)) => {
                    if let TypeKind::Record(fields) = arg_hint.as_ref() {
                        let expected: HashMap<_, _> =
                            fields.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                        let mut seen = HashSet::new();
                        let mut preds = Vec::new();
                        for (k, v) in kvs {
                            let expected_ty = expected
                                .get(k)
                                .ok_or_else(|| TypeError::UnknownField {
                                    field: k.clone(),
                                    typ: Type::record(fields.clone()).to_string(),
                                })?
                                .clone();
                            let (p1, t1) =
                                infer_expr_type(unifier, supply, env, adts, known, v.as_ref())?;
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
                    } else {
                        infer_expr_type(unifier, supply, env, adts, known, x)
                    }
                }
                _ => infer_expr_type(unifier, supply, env, adts, known, x),
            }?;
            let res_ty = Type::var(supply.fresh(Some("r".into())));
            unifier.unify(&t1, &Type::fun(t2.clone(), res_ty.clone()))?;
            let result_ty = unifier.apply_type(&res_ty);
            let mut preds = p1;
            preds.extend(p2);
            Ok((preds, result_ty))
        }
        Expr::Project(_, base, field) => {
            let (p1, t1) = infer_expr_type(unifier, supply, env, adts, known, base)?;
            let base_ty = unifier.apply_type(&t1);
            let mut known_variant = None;
            if let Expr::Var(var) = base.as_ref() {
                if let Some(info) = known.get(&var.name) {
                    known_variant = Some(info.clone());
                }
            }
            if known_variant.is_none() {
                known_variant = known_variant_from_expr(base, &base_ty, adts);
            }
            let field_ty =
                resolve_projection(unifier, supply, adts, &base_ty, known_variant, field)?;
            Ok((p1, field_ty))
        }
        Expr::Let(..) => {
            let mut bindings = Vec::new();
            let mut cur = expr;
            loop {
                match cur {
                    Expr::Let(_, v, ann, d, b) => {
                        bindings.push((v.clone(), ann.clone(), d.clone()));
                        cur = b.as_ref();
                    }
                    _ => break,
                }
            }

            let mut env_cur = env.clone();
            let mut known_cur = known.clone();
            for (v, ann, d) in bindings {
                let (p1, t1) = infer_expr_type(unifier, supply, &env_cur, adts, &known_cur, &d)?;
                if let Some(ann) = ann {
                    let mut ann_vars = HashMap::new();
                    let ann_ty = type_from_annotation_expr_vars(adts, &ann, &mut ann_vars, supply)?;
                    unifier.unify(&t1, &ann_ty)?;
                }
                let def_ty = unifier.apply_type(&t1);
                let scheme = generalize_with_unifier(&env_cur, p1, def_ty.clone(), unifier);
                env_cur.extend(v.name.clone(), scheme);
                if let Some(known_variant) = known_variant_from_expr(&d, &def_ty, adts) {
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
        Expr::Ite(_, cond, then_expr, else_expr) => {
            let (p1, t1) = infer_expr_type(unifier, supply, env, adts, known, cond)?;
            unifier.unify(&t1, &Type::con("bool", 0))?;
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
            let list_ty = Type::app(Type::con("List", 1), unifier.apply_type(&elem_tv));
            Ok((preds, list_ty))
        }
        Expr::Dict(_, kvs) => {
            let elem_tv = Type::var(supply.fresh(Some("v".into())));
            let mut preds = Vec::new();
            for (_k, v) in kvs {
                let (p1, t1) = infer_expr_type(unifier, supply, env, adts, known, v.as_ref())?;
                unifier.unify(&t1, &elem_tv)?;
                preds.extend(p1);
            }
            let dict_ty = Type::app(Type::con("Dict", 1), unifier.apply_type(&elem_tv));
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
                            if let Some((adt, _variant)) = ctor_lookup(adts, name) {
                                known_arm.insert(
                                    var.name.clone(),
                                    KnownVariant {
                                        adt: adt.name.clone(),
                                        variant: name.clone(),
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
            let (preds, expr_ty) = infer_expr_type(unifier, supply, env, adts, known, expr)?;
            let ann_ty = type_from_annotation_expr(adts, ann)?;
            unifier.unify(&expr_ty, &ann_ty)?;
            let out_ty = unifier.apply_type(&ann_ty);
            Ok((preds, out_ty))
        }
    })();
    res.map_err(|err| with_span(&span, err))
}

fn infer_expr(
    unifier: &mut Unifier,
    supply: &mut TypeVarSupply,
    env: &TypeEnv,
    adts: &HashMap<Symbol, AdtDecl>,
    known: &KnownVariants,
    expr: &Expr,
) -> Result<(Vec<Predicate>, Type, TypedExpr), TypeError> {
    let span = *expr.span();
    let res = (|| match expr {
        Expr::Bool(_, v) => {
            let t = Type::con("bool", 0);
            Ok((
                vec![],
                t.clone(),
                TypedExpr::new(t, TypedExprKind::Bool(*v)),
            ))
        }
        Expr::Uint(_, v) => {
            let t = Type::con("i32", 0);
            Ok((
                vec![],
                t.clone(),
                TypedExpr::new(t, TypedExprKind::Uint(*v)),
            ))
        }
        Expr::Int(_, v) => {
            let t = Type::con("i32", 0);
            Ok((vec![], t.clone(), TypedExpr::new(t, TypedExprKind::Int(*v))))
        }
        Expr::Float(_, v) => {
            let t = Type::con("f32", 0);
            Ok((
                vec![],
                t.clone(),
                TypedExpr::new(t, TypedExprKind::Float(*v)),
            ))
        }
        Expr::String(_, v) => {
            let t = Type::con("string", 0);
            Ok((
                vec![],
                t.clone(),
                TypedExpr::new(t, TypedExprKind::String(v.clone())),
            ))
        }
        Expr::Uuid(_, v) => {
            let t = Type::con("uuid", 0);
            Ok((
                vec![],
                t.clone(),
                TypedExpr::new(t, TypedExprKind::Uuid(*v)),
            ))
        }
        Expr::DateTime(_, v) => {
            let t = Type::con("datetime", 0);
            Ok((
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
                    if !scheme.vars.is_empty() || !scheme.preds.is_empty() {
                        return Err(TypeError::AmbiguousOverload(var.name.clone()));
                    }
                    let scheme = apply_scheme_with_unifier(scheme, unifier);
                    overloads.push(scheme.typ);
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
            let mut ann_vars = HashMap::new();
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
        Expr::App(_, f, x) => {
            let (p1, t1, typed_f) = infer_expr(unifier, supply, env, adts, known, f)?;
            let arg_hint = match unifier.apply_type(&t1).as_ref() {
                TypeKind::Fun(arg, _) => Some(arg.clone()),
                _ => None,
            };
            let (p2, t2, typed_x) = match (arg_hint, x.as_ref()) {
                (Some(arg_hint), Expr::Dict(_, kvs)) => {
                    if let TypeKind::Record(fields) = arg_hint.as_ref() {
                        let mut preds = Vec::new();
                        let mut typed_kvs = BTreeMap::new();
                        let expected: HashMap<_, _> =
                            fields.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                        for (k, v) in kvs {
                            let expected_ty = expected
                                .get(k)
                                .ok_or_else(|| TypeError::UnknownField {
                                    field: k.clone(),
                                    typ: Type::record(fields.clone()).to_string(),
                                })?
                                .clone();
                            let (p1, t1, typed_v) =
                                infer_expr(unifier, supply, env, adts, known, v.as_ref())?;
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
                        let typed =
                            TypedExpr::new(record_ty.clone(), TypedExprKind::Dict(typed_kvs));
                        Ok((preds, record_ty, typed))
                    } else {
                        infer_expr(unifier, supply, env, adts, known, x)
                    }
                }
                _ => infer_expr(unifier, supply, env, adts, known, x),
            }?;
            let res_ty = Type::var(supply.fresh(Some("r".into())));
            unifier.unify(&t1, &Type::fun(t2.clone(), res_ty.clone()))?;
            let result_ty = unifier.apply_type(&res_ty);
            let mut preds = p1;
            preds.extend(p2);
            let typed = TypedExpr::new(
                result_ty.clone(),
                TypedExprKind::App(Box::new(typed_f), Box::new(typed_x)),
            );
            Ok((preds, result_ty, typed))
        }
        Expr::Project(_, base, field) => {
            let (p1, t1, typed_base) = infer_expr(unifier, supply, env, adts, known, base)?;
            let base_ty = unifier.apply_type(&t1);
            let mut known_variant = None;
            if let Expr::Var(var) = base.as_ref() {
                if let Some(info) = known.get(&var.name) {
                    known_variant = Some(info.clone());
                }
            }
            if known_variant.is_none() {
                known_variant = known_variant_from_expr(base, &base_ty, adts);
            }
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
        Expr::Let(..) => {
            let mut bindings = Vec::new();
            let mut cur = expr;
            loop {
                match cur {
                    Expr::Let(_, v, ann, d, b) => {
                        bindings.push((v.clone(), ann.clone(), d.clone()));
                        cur = b.as_ref();
                    }
                    _ => break,
                }
            }

            let mut env_cur = env.clone();
            let mut known_cur = known.clone();
            let mut typed_defs = Vec::new();
            for (v, ann, d) in bindings {
                let (p1, t1, typed_def) =
                    infer_expr(unifier, supply, &env_cur, adts, &known_cur, &d)?;
                if let Some(ann) = ann {
                    let mut ann_vars = HashMap::new();
                    let ann_ty = type_from_annotation_expr_vars(adts, &ann, &mut ann_vars, supply)?;
                    unifier.unify(&t1, &ann_ty)?;
                }
                let def_ty = unifier.apply_type(&t1);
                let scheme = generalize_with_unifier(&env_cur, p1, def_ty.clone(), unifier);
                env_cur.extend(v.name.clone(), scheme);
                if let Some(known_variant) = known_variant_from_expr(&d, &def_ty, adts) {
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
        Expr::Ite(_, cond, then_expr, else_expr) => {
            let (p1, t1, typed_cond) = infer_expr(unifier, supply, env, adts, known, cond)?;
            unifier.unify(&t1, &Type::con("bool", 0))?;
            let (p2, t2, typed_then) = infer_expr(unifier, supply, env, adts, known, then_expr)?;
            let (p3, t3, typed_else) = infer_expr(unifier, supply, env, adts, known, else_expr)?;
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
                let (p1, t1, typed_elem) = infer_expr(unifier, supply, env, adts, known, elem)?;
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
                let (p1, t1, typed_elem) = infer_expr(unifier, supply, env, adts, known, elem)?;
                unifier.unify(&t1, &elem_tv)?;
                preds.extend(p1);
                typed_elems.push(typed_elem);
            }
            let list_ty = Type::app(Type::con("List", 1), unifier.apply_type(&elem_tv));
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
            let dict_ty = Type::app(Type::con("Dict", 1), unifier.apply_type(&elem_tv));
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
                            if let Some((adt, _variant)) = ctor_lookup(adts, name) {
                                known_arm.insert(
                                    var.name.clone(),
                                    KnownVariant {
                                        adt: adt.name.clone(),
                                        variant: name.clone(),
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
            let (preds, expr_ty, typed_expr) = infer_expr(unifier, supply, env, adts, known, expr)?;
            let ann_ty = type_from_annotation_expr(adts, ann)?;
            unifier.unify(&expr_ty, &ann_ty)?;
            let out_ty = unifier.apply_type(&ann_ty);
            Ok((preds, out_ty, typed_expr))
        }
    })();
    res.map_err(|err| with_span(&span, err))
}

fn ctor_lookup<'a>(
    adts: &'a HashMap<Symbol, AdtDecl>,
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
    adts: &HashMap<Symbol, AdtDecl>,
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
    if record_fields(variant).is_none() {
        return None;
    }
    Some(KnownVariant {
        adt: adt.name.clone(),
        variant: variant.name.clone(),
    })
}

fn resolve_projection(
    unifier: &mut Unifier,
    supply: &mut TypeVarSupply,
    adts: &HashMap<Symbol, AdtDecl>,
    base_ty: &Type,
    known_variant: Option<KnownVariant>,
    field: &Symbol,
) -> Result<Type, TypeError> {
    let (adt, variant) = if let Some(info) = known_variant {
        let adt = adts
            .get(&info.adt)
            .ok_or_else(|| TypeError::UnknownTypeName(info.adt.clone()))?;
        let variant = adt
            .variants
            .iter()
            .find(|v| v.name == info.variant)
            .ok_or_else(|| TypeError::UnknownField {
                field: field.clone(),
                typ: base_ty.to_string(),
            })?;
        (adt, variant)
    } else if let Some(adt_name) = type_head_name(base_ty) {
        let adt = adts.get(adt_name).ok_or_else(|| TypeError::UnknownField {
            field: field.clone(),
            typ: base_ty.to_string(),
        })?;
        if adt.variants.len() == 1 {
            (adt, &adt.variants[0])
        } else {
            return Err(TypeError::FieldNotKnown {
                field: field.clone(),
                typ: base_ty.to_string(),
            });
        }
    } else if matches!(base_ty.as_ref(), TypeKind::Var(_)) {
        let mut candidates = Vec::new();
        for adt in adts.values() {
            if adt.variants.len() != 1 {
                continue;
            }
            let variant = &adt.variants[0];
            if let Some(fields) = record_fields(variant) {
                if fields.iter().any(|(name, _)| name == field) {
                    candidates.push((adt, variant));
                }
            }
        }
        if candidates.len() == 1 {
            candidates.remove(0)
        } else if candidates.is_empty() {
            return Err(TypeError::UnknownField {
                field: field.clone(),
                typ: base_ty.to_string(),
            });
        } else {
            return Err(TypeError::FieldNotKnown {
                field: field.clone(),
                typ: base_ty.to_string(),
            });
        }
    } else {
        return Err(TypeError::UnknownField {
            field: field.clone(),
            typ: base_ty.to_string(),
        });
    };

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

fn infer_pattern(
    unifier: &mut Unifier,
    supply: &mut TypeVarSupply,
    env: &TypeEnv,
    pat: &Pattern,
    scrutinee_ty: &Type,
) -> Result<(Vec<Predicate>, Vec<(Symbol, Type)>), TypeError> {
    let span = *pat.span();
    let res = (|| match pat {
        Pattern::Wildcard(..) => Ok((vec![], vec![])),
        Pattern::Var(var) => Ok((
            vec![],
            vec![(var.name.clone(), unifier.apply_type(scrutinee_ty))],
        )),
        Pattern::Named(_, name, ps) => {
            let schemes = env
                .lookup(name)
                .ok_or_else(|| TypeError::UnknownVar(name.clone()))?;
            if schemes.len() != 1 {
                return Err(TypeError::AmbiguousOverload(name.clone()));
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
            let list_ty = Type::app(Type::con("List", 1), elem_tv.clone());
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
            let list_ty = Type::app(Type::con("List", 1), elem_tv.clone());
            unifier.unify(scrutinee_ty, &list_ty)?;
            let mut preds = Vec::new();
            let mut bindings = Vec::new();

            let head_ty = unifier.apply_type(&elem_tv);
            let (p1, binds1) = infer_pattern(unifier, supply, env, head, &head_ty)?;
            preds.extend(p1);
            bindings.extend(binds1);

            let tail_ty = Type::app(Type::con("List", 1), unifier.apply_type(&elem_tv));
            let (p2, binds2) = infer_pattern(unifier, supply, env, tail, &tail_ty)?;
            preds.extend(p2);
            bindings.extend(binds2);

            let bindings = bindings
                .into_iter()
                .map(|(name, ty)| (name, unifier.apply_type(&ty)))
                .collect();
            Ok((preds, bindings))
        }
        Pattern::Dict(_, keys) => {
            if let TypeKind::Record(fields) = scrutinee_ty.as_ref() {
                let mut bindings = Vec::new();
                for key in keys {
                    let ty = fields
                        .iter()
                        .find(|(name, _)| name == key)
                        .map(|(_, ty)| unifier.apply_type(ty))
                        .ok_or_else(|| TypeError::UnknownField {
                            field: key.clone(),
                            typ: scrutinee_ty.to_string(),
                        })?;
                    bindings.push((key.clone(), ty));
                }
                Ok((vec![], bindings))
            } else {
                let elem_tv = Type::var(supply.fresh(Some("v".into())));
                let dict_ty = Type::app(Type::con("Dict", 1), elem_tv.clone());
                unifier.unify(scrutinee_ty, &dict_ty)?;
                let elem_ty = unifier.apply_type(&elem_tv);
                let bindings = keys.iter().map(|k| (k.clone(), elem_ty.clone())).collect();
                Ok((vec![], bindings))
            }
        }
    })();
    res.map_err(|err| with_span(&span, err))
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

fn adt_name_from_patterns(adts: &HashMap<Symbol, AdtDecl>, patterns: &[Pattern]) -> Option<Symbol> {
    let mut candidate: Option<Symbol> = None;
    for pat in patterns {
        let next = match pat {
            Pattern::Named(_, name, _) => ctor_lookup(adts, name).map(|(adt, _)| adt.name.clone()),
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
    adts: &HashMap<Symbol, AdtDecl>,
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
    let ctor_names: HashSet<Symbol> = adt.variants.iter().map(|v| v.name.clone()).collect();
    if ctor_names.is_empty() {
        return Ok(());
    }
    let mut covered = HashSet::new();
    for pat in patterns {
        match pat {
            Pattern::Named(_, name, _) if ctor_names.contains(name) => {
                covered.insert(name.clone());
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

fn inject_prelude_classes_and_instances(ts: &mut TypeSystem) {
    let source = include_str!("prelude_typeclasses.rex");
    let tokens = Token::tokenize(source).expect("failed to lex Rex prelude type class source");
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().unwrap_or_else(|errs| {
        let mut out = String::from("failed to parse Rex prelude type class source:");
        for err in errs {
            out.push_str(&format!("\n  {err}"));
        }
        panic!("{out}");
    });

    for decl in &program.decls {
        match decl {
            Decl::Class(class_decl) => ts
                .inject_class_decl(class_decl)
                .expect("failed to inject prelude class decl"),
            Decl::Instance(inst_decl) => {
                ts.inject_instance_decl(inst_decl)
                    .expect("failed to inject prelude instance decl");
            }
            Decl::Type(..) | Decl::Fn(..) => {}
        }
    }
}

fn build_prelude(ts: &mut TypeSystem) {
    // Primitive type constructors
    let prims = [
        "u8", "u16", "u32", "u64", "i8", "i16", "i32", "i64", "f32", "f64", "bool", "string",
        "uuid", "datetime",
    ];
    for prim in prims {
        ts.env
            .extend(sym(prim), Scheme::new(vec![], vec![], Type::con(prim, 0)));
    }

    // Type constructors for ADTs used in prelude schemes.
    let result_con = Type::con("Result", 2);
    let option_con = Type::con("Option", 1);

    // Register ADT constructors as value-level functions.
    {
        let list_name = sym("List");
        let a_name = sym("a");
        let list_params = vec![a_name.clone()];
        let mut list_adt = AdtDecl::new(&list_name, &list_params, &mut ts.supply);
        let a = list_adt.param_type(&a_name).unwrap();
        let list_a = list_adt.result_type();
        list_adt.add_variant(sym("Empty"), vec![]);
        list_adt.add_variant(sym("Cons"), vec![a.clone(), list_a.clone()]);
        ts.inject_adt(&list_adt);
    }
    {
        let option_name = sym("Option");
        let t_name = sym("t");
        let option_params = vec![t_name.clone()];
        let mut option_adt = AdtDecl::new(&option_name, &option_params, &mut ts.supply);
        let t = option_adt.param_type(&t_name).unwrap();
        option_adt.add_variant(sym("Some"), vec![t]);
        option_adt.add_variant(sym("None"), vec![]);
        ts.inject_adt(&option_adt);
    }
    {
        let result_name = sym("Result");
        let e_name = sym("e");
        let t_name = sym("t");
        let result_params = vec![e_name.clone(), t_name.clone()];
        let mut result_adt = AdtDecl::new(&result_name, &result_params, &mut ts.supply);
        let e = result_adt.param_type(&e_name).unwrap();
        let t = result_adt.param_type(&t_name).unwrap();
        result_adt.add_variant(sym("Err"), vec![e]);
        result_adt.add_variant(sym("Ok"), vec![t]);
        ts.inject_adt(&result_adt);
    }

    inject_prelude_classes_and_instances(ts);

    // Helper constructors used to describe prelude schemes below.
    let fresh_tv = |ts: &mut TypeSystem, name: &str| ts.supply.fresh(Some(sym(name)));
    let option_of = |t: Type| Type::app(option_con.clone(), t);
    let result_of = |t: Type, e: Type| Type::app(Type::app(result_con.clone(), e), t);
    let indexable_of = |container: Type, elem: Type| Type::tuple(vec![container, elem]);

    // Inject provided function declarations and operator schemes.
    let a_tv = ts.supply.fresh(Some("a".into()));
    let a = Type::var(a_tv.clone());
    let add_monoid_a = Predicate::new("AdditiveMonoid", a.clone());
    let add_group_a = Predicate::new("AdditiveGroup", a.clone());
    let integral_a = Predicate::new("Integral", a.clone());
    let plus_scheme = Scheme::new(
        vec![a_tv.clone()],
        vec![add_monoid_a],
        Type::fun(a.clone(), Type::fun(a.clone(), a.clone())),
    );
    ts.add_value(sym("+"), plus_scheme.clone());

    let mul_scheme = Scheme::new(
        vec![a_tv.clone()],
        vec![Predicate::new("MultiplicativeMonoid", a.clone())],
        Type::fun(a.clone(), Type::fun(a.clone(), a.clone())),
    );
    ts.add_value(sym("*"), mul_scheme);

    let mod_scheme = Scheme::new(
        vec![a_tv.clone()],
        vec![integral_a],
        Type::fun(a.clone(), Type::fun(a.clone(), a.clone())),
    );
    ts.add_value(sym("%"), mod_scheme);

    let negate_scheme = Scheme::new(
        vec![a_tv.clone()],
        vec![add_group_a.clone()],
        Type::fun(a.clone(), a.clone()),
    );
    ts.add_value(sym("negate"), negate_scheme);

    let minus_scheme = Scheme::new(
        vec![a_tv.clone()],
        vec![add_group_a],
        Type::fun(a.clone(), Type::fun(a.clone(), a.clone())),
    );
    ts.add_value(sym("-"), minus_scheme.clone());
    ts.add_value(sym("(-)"), minus_scheme);

    let b_tv = ts.supply.fresh(Some("b".into()));
    let b = Type::var(b_tv.clone());
    let field_b = Predicate::new("Field", b.clone());
    let div_scheme = Scheme::new(
        vec![b_tv.clone()],
        vec![field_b],
        Type::fun(b.clone(), Type::fun(b.clone(), b.clone())),
    );
    ts.add_value(sym("/"), div_scheme.clone());
    ts.add_value(sym("(/)"), div_scheme);

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
        Scheme::new(
            vec![],
            vec![],
            Type::fun(bool_ty.clone(), Type::fun(bool_ty.clone(), bool_ty.clone())),
        ),
    );
    ts.add_value(
        "||",
        Scheme::new(
            vec![],
            vec![],
            Type::fun(bool_ty.clone(), Type::fun(bool_ty.clone(), bool_ty.clone())),
        ),
    );

    // Collection combinators (type class based)
    {
        let f_tv = fresh_tv(ts, "f");
        let a_tv = fresh_tv(ts, "a");
        let b_tv = fresh_tv(ts, "b");
        let f = Type::var(f_tv.clone());
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let fa = Type::app(f.clone(), a.clone());
        let fb = Type::app(f.clone(), b.clone());

        ts.add_value(
            "map",
            Scheme::new(
                vec![f_tv.clone(), a_tv.clone(), b_tv.clone()],
                vec![Predicate::new("Functor", f.clone())],
                Type::fun(
                    Type::fun(a.clone(), b.clone()),
                    Type::fun(fa.clone(), fb.clone()),
                ),
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
                Type::fun(
                    Type::fun(a.clone(), bool_ty.clone()),
                    Type::fun(fa.clone(), fa.clone()),
                ),
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
                Type::fun(
                    Type::fun(a.clone(), fb.clone()),
                    Type::fun(fa.clone(), fb.clone()),
                ),
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
                Type::fun(
                    Type::fun(fa.clone(), fa.clone()),
                    Type::fun(fa.clone(), fa.clone()),
                ),
            ),
        );
        ts.add_value(
            "sum",
            Scheme::new(
                vec![f_tv.clone(), a_tv.clone()],
                vec![
                    Predicate::new("Foldable", f.clone()),
                    Predicate::new("AdditiveMonoid", a.clone()),
                ],
                Type::fun(fa.clone(), a.clone()),
            ),
        );
        ts.add_value(
            "mean",
            Scheme::new(
                vec![f_tv.clone(), a_tv.clone()],
                vec![
                    Predicate::new("Foldable", f.clone()),
                    Predicate::new("Field", a.clone()),
                ],
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
                    Type::fun(
                        fb.clone(),
                        Type::app(f.clone(), Type::tuple(vec![a.clone(), b.clone()])),
                    ),
                ),
            ),
        );
        ts.add_value(
            "unzip",
            Scheme::new(
                vec![f_tv.clone(), a_tv.clone(), b_tv.clone()],
                vec![Predicate::new("Sequence", f.clone())],
                Type::fun(
                    Type::app(f.clone(), Type::tuple(vec![a.clone(), b.clone()])),
                    Type::tuple(vec![fa.clone(), fb.clone()]),
                ),
            ),
        );
        ts.add_value(
            "min",
            Scheme::new(
                vec![f_tv.clone(), a_tv.clone()],
                vec![
                    Predicate::new("Foldable", f.clone()),
                    Predicate::new("Ord", a.clone()),
                ],
                Type::fun(fa.clone(), a.clone()),
            ),
        );
        ts.add_value(
            "max",
            Scheme::new(
                vec![f_tv, a_tv],
                vec![
                    Predicate::new("Foldable", f.clone()),
                    Predicate::new("Ord", a.clone()),
                ],
                Type::fun(fa.clone(), a.clone()),
            ),
        );
    }

    // Indexable access
    {
        let t_tv = fresh_tv(ts, "t");
        let a_tv = fresh_tv(ts, "a");
        let t = Type::var(t_tv.clone());
        let a = Type::var(a_tv.clone());
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
        let a = Type::var(a_tv.clone());
        let opt_a = option_of(a.clone());
        ts.add_value(
            "is_some",
            Scheme::new(
                vec![a_tv.clone()],
                vec![],
                Type::fun(opt_a.clone(), bool_ty.clone()),
            ),
        );
        ts.add_value(
            "is_none",
            Scheme::new(
                vec![a_tv.clone()],
                vec![],
                Type::fun(opt_a.clone(), bool_ty.clone()),
            ),
        );
    }

    // Result helpers
    {
        let t_tv = fresh_tv(ts, "t");
        let e_tv = fresh_tv(ts, "e");
        let t = Type::var(t_tv.clone());
        let e = Type::var(e_tv.clone());
        let res_te = result_of(t.clone(), e.clone());
        ts.add_value(
            "is_ok",
            Scheme::new(
                vec![t_tv.clone(), e_tv.clone()],
                vec![],
                Type::fun(res_te.clone(), bool_ty.clone()),
            ),
        );
        ts.add_value(
            "is_err",
            Scheme::new(
                vec![t_tv.clone(), e_tv.clone()],
                vec![],
                Type::fun(res_te.clone(), bool_ty.clone()),
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tvar(id: TypeVarId, name: &str) -> Type {
        Type::var(TypeVar::new(id, Some(sym(name))))
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
        let tv = TypeVar::new(0, Some(sym("a")));
        let t = Type::fun(Type::var(tv.clone()), Type::con("u8", 0));
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
        let minus = ts.env.lookup(&sym("(-)")).expect("minus in env");
        let div = ts.env.lookup(&sym("(/)")).expect("div in env");
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
        assert!(ts.env.lookup(&sym("Empty")).is_some());
        assert!(ts.env.lookup(&sym("Cons")).is_some());
        assert!(ts.env.lookup(&sym("Ok")).is_some());
        assert!(ts.env.lookup(&sym("Err")).is_some());
        assert!(ts.env.lookup(&sym("Some")).is_some());
        assert!(ts.env.lookup(&sym("None")).is_some());
    }

    fn parse_expr(code: &str) -> std::sync::Arc<rex_ast::expr::Expr> {
        let mut parser = rex_parser::Parser::new(rex_lexer::Token::tokenize(code).unwrap());
        parser.parse_program().unwrap().expr
    }

    fn parse_program(code: &str) -> rex_ast::expr::Program {
        let mut parser = rex_parser::Parser::new(rex_lexer::Token::tokenize(code).unwrap());
        parser.parse_program().unwrap()
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
                assert!(matches!(
                    *error,
                    TypeError::UnknownVar(name) if name.as_ref() == "missing"
                ));
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
        let expected = Type::tuple(vec![
            Type::con("i32", 0),
            Type::con("f32", 0),
            Type::con("string", 0),
        ]);
        assert_eq!(ty, expected);
    }

    #[test]
    fn infer_type_annotation_ok() {
        let expr = parse_expr("let x: i32 = 42 in x");
        let mut ts = TypeSystem::with_prelude();
        let (_preds, ty) = ts.infer(expr.as_ref()).unwrap();
        assert_eq!(ty, Type::con("i32", 0));
    }

    #[test]
    fn infer_type_annotation_lambda_param() {
        let expr = parse_expr("\\ (a : f32) -> a");
        let mut ts = TypeSystem::with_prelude();
        let (_preds, ty) = ts.infer(expr.as_ref()).unwrap();
        assert_eq!(ty, Type::fun(Type::con("f32", 0), Type::con("f32", 0)));
    }

    #[test]
    fn infer_type_annotation_is_alias() {
        let expr = parse_expr("\"hi\" is str");
        let mut ts = TypeSystem::with_prelude();
        let (_preds, ty) = ts.infer(expr.as_ref()).unwrap();
        assert_eq!(ty, Type::con("string", 0));
    }

    #[test]
    fn infer_type_annotation_mismatch_error() {
        let expr = parse_expr("let x: i32 = 3.14 in x");
        let mut ts = TypeSystem::with_prelude();
        let err = strip_span(ts.infer(expr.as_ref()).unwrap_err());
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
        let mut ts = TypeSystem::with_prelude();
        for decl in &program.decls {
            if let rex_ast::expr::Decl::Type(decl) = decl {
                ts.inject_type_decl(decl).unwrap();
            }
        }
        let (_preds, ty) = ts.infer(program.expr.as_ref()).unwrap();
        let expected = Type::tuple(vec![Type::con("i32", 0), Type::con("f32", 0)]);
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
        let mut ts = TypeSystem::with_prelude();
        for decl in &program.decls {
            if let rex_ast::expr::Decl::Type(decl) = decl {
                ts.inject_type_decl(decl).unwrap();
            }
        }
        let (_preds, ty) = ts.infer(program.expr.as_ref()).unwrap();
        assert_eq!(ty, Type::con("i32", 0));
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
        let mut ts = TypeSystem::with_prelude();
        for decl in &program.decls {
            if let rex_ast::expr::Decl::Type(decl) = decl {
                ts.inject_type_decl(decl).unwrap();
            }
        }
        let err = strip_span(ts.infer(program.expr.as_ref()).unwrap_err());
        assert!(matches!(err, TypeError::FieldNotKnown { .. }));
    }

    #[test]
    fn infer_project_lambda_param_single_variant() {
        let program = parse_program(
            r#"
            type Boxed = Boxed { value: i32 }
            let
                f = \x -> x:value
            in
                f (Boxed { value = 1 })
            "#,
        );
        let mut ts = TypeSystem::with_prelude();
        for decl in &program.decls {
            if let rex_ast::expr::Decl::Type(decl) = decl {
                ts.inject_type_decl(decl).unwrap();
            }
        }
        let (_preds, ty) = ts.infer(program.expr.as_ref()).unwrap();
        assert_eq!(ty, Type::con("i32", 0));
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
        let mut ts = TypeSystem::with_prelude();
        for decl in &program.decls {
            if let rex_ast::expr::Decl::Type(decl) = decl {
                ts.inject_type_decl(decl).unwrap();
            }
        }
        let (_preds, ty) = ts.infer(program.expr.as_ref()).unwrap();
        assert_eq!(ty, Type::con("i32", 0));
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
        let mut ts = TypeSystem::with_prelude();
        let (_preds, ty) = ts.infer(expr.as_ref()).unwrap();
        assert_eq!(ty, Type::con("i32", 0));
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
        let mut ts = TypeSystem::with_prelude();
        let (_preds, ty) = ts.infer(expr.as_ref()).unwrap();
        let expected = Type::tuple(vec![
            Type::con("i32", 0),
            Type::con("string", 0),
            Type::tuple(vec![Type::con("bool", 0), Type::con("bool", 0)]),
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
        let mut ts = TypeSystem::with_prelude();
        let (_preds, ty) = ts.infer(expr.as_ref()).unwrap();
        assert_eq!(ty, Type::con("i32", 0));
    }

    #[test]
    fn infer_head_or_list_match() {
        let expr = parse_expr(
            r#"
            let
                head_or = \fallback xs ->
                    match xs
                        when [] -> fallback
                        when x:xs -> x
            in
                (head_or 0 [1, 2, 3], head_or 0 [])
            "#,
        );
        let mut ts = TypeSystem::with_prelude();
        let (_preds, ty) = ts.infer(expr.as_ref()).unwrap();
        let expected = Type::tuple(vec![Type::con("i32", 0), Type::con("i32", 0)]);
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
        let mut ts = TypeSystem::with_prelude();
        for decl in &program.decls {
            if let rex_ast::expr::Decl::Type(decl) = decl {
                ts.inject_type_decl(decl).unwrap();
            }
        }
        let (_preds, ty) = ts.infer(program.expr.as_ref()).unwrap();
        assert_eq!(ty, Type::con("i32", 0));
    }

    #[test]
    fn infer_fn_decl_simple() {
        let program = parse_program(
            r#"
            fn add (x: i32, y: i32) -> i32 = x + y
            add 1 2
            "#,
        );
        let mut ts = TypeSystem::with_prelude();
        let expr = program.expr_with_fns();
        let (_preds, ty) = ts.infer(expr.as_ref()).unwrap();
        assert_eq!(ty, Type::con("i32", 0));
    }

    #[test]
    fn infer_fn_decl_polymorphic_where_constraints() {
        let program = parse_program(
            r#"
            fn my_add (x: a, y: a) -> a where AdditiveMonoid a = x + y
            (my_add 1 2, my_add 1.0 2.0)
            "#,
        );
        let mut ts = TypeSystem::with_prelude();
        let expr = program.expr_with_fns();
        let (_preds, ty) = ts.infer(expr.as_ref()).unwrap();
        assert_eq!(
            ty,
            Type::tuple(vec![Type::con("i32", 0), Type::con("f32", 0)])
        );
    }

    #[test]
    fn infer_additive_monoid_constraint() {
        let expr = parse_expr("\\x y -> x + y");
        let mut ts = TypeSystem::with_prelude();
        let (preds, ty) = ts.infer(expr.as_ref()).unwrap();
        assert_eq!(preds.len(), 1);
        assert_eq!(preds[0].class.as_ref(), "AdditiveMonoid");

        if let TypeKind::Fun(a, rest) = ty.as_ref() {
            if let TypeKind::Fun(b, c) = rest.as_ref() {
                assert_eq!(a.as_ref(), b.as_ref());
                assert_eq!(b.as_ref(), c.as_ref());
                assert_eq!(preds[0].typ, a.clone());
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
        assert_eq!(preds[0].class.as_ref(), "MultiplicativeMonoid");

        if let TypeKind::Fun(a, rest) = ty.as_ref() {
            if let TypeKind::Fun(b, c) = rest.as_ref() {
                assert_eq!(a.as_ref(), b.as_ref());
                assert_eq!(b.as_ref(), c.as_ref());
                assert_eq!(preds[0].typ, a.clone());
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
        assert_eq!(preds[0].class.as_ref(), "AdditiveGroup");

        if let TypeKind::Fun(a, rest) = ty.as_ref() {
            if let TypeKind::Fun(b, c) = rest.as_ref() {
                assert_eq!(a.as_ref(), b.as_ref());
                assert_eq!(b.as_ref(), c.as_ref());
                assert_eq!(preds[0].typ, a.clone());
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
        assert_eq!(preds[0].class.as_ref(), "Integral");

        if let TypeKind::Fun(a, rest) = ty.as_ref() {
            if let TypeKind::Fun(b, c) = rest.as_ref() {
                assert_eq!(a.as_ref(), b.as_ref());
                assert_eq!(b.as_ref(), c.as_ref());
                assert_eq!(preds[0].typ, a.clone());
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
        assert_eq!(preds[0].class.as_ref(), "AdditiveMonoid");
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
        assert_eq!(preds[0].class.as_ref(), "Integral");
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
        assert_eq!(preds[0].class.as_ref(), "Indexable");
        assert!(entails(&ts.classes, &[], &preds[0]).unwrap());
    }

    #[test]
    fn infer_get_tuple_type() {
        let expr = parse_expr("get 1 (1, 2, 3)");
        let mut ts = TypeSystem::with_prelude();
        let (preds, ty) = ts.infer(expr.as_ref()).unwrap();
        assert_eq!(ty, Type::con("i32", 0));
        assert_eq!(preds.len(), 1);
        assert_eq!(preds[0].class.as_ref(), "Indexable");
        assert!(entails(&ts.classes, &[], &preds[0]).unwrap());
    }

    #[test]
    fn infer_division_defaults() {
        let expr = parse_expr("1.0 / 2.0");
        let mut ts = TypeSystem::with_prelude();
        let (preds, ty) = ts.infer(expr.as_ref()).unwrap();
        assert_eq!(ty, Type::con("f32", 0));
        assert_eq!(preds.len(), 1);
        assert_eq!(preds[0].class.as_ref(), "Field");
        assert_eq!(preds[0].typ, Type::con("f32", 0));
        assert!(entails(&ts.classes, &[], &preds[0]).unwrap());
    }

    #[test]
    fn infer_unbound_variable_error() {
        let expr = parse_expr("missing");
        let mut ts = TypeSystem::with_prelude();
        let err = strip_span(ts.infer(expr.as_ref()).unwrap_err());
        assert!(matches!(
            err,
            TypeError::UnknownVar(name) if name.as_ref() == "missing"
        ));
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
        let err = strip_span(ts.infer(expr.as_ref()).unwrap_err());
        assert!(matches!(
            err,
            TypeError::AmbiguousOverload(name) if name.as_ref() == "dup"
        ));
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
        assert!(matches!(
            err,
            TypeError::UnsupportedExpr("pattern constructor")
        ));
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
    fn infer_non_exhaustive_match_on_bound_var_error() {
        let expr = parse_expr("let x = Ok 1 in match x when Ok y -> y");
        let mut ts = TypeSystem::with_prelude();
        let err = strip_span(ts.infer(expr.as_ref()).unwrap_err());
        assert!(matches!(err, TypeError::NonExhaustiveMatch { .. }));
    }

    #[test]
    fn infer_non_exhaustive_match_in_lambda_error() {
        let expr = parse_expr("\\x -> match x when Ok y -> y");
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
                assert_eq!(missing, vec![sym("None")]);
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
                assert_eq!(missing, vec![sym("Err")]);
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
                assert_eq!(missing, vec![sym("Empty")]);
            }
            other => panic!("expected non-exhaustive match, got {other:?}"),
        }
    }

    #[test]
    fn infer_non_exhaustive_list_match_on_bound_var_error() {
        let expr = parse_expr("let xs = [1, 2] in match xs when x:xs -> x");
        let mut ts = TypeSystem::with_prelude();
        let err = strip_span(ts.infer(expr.as_ref()).unwrap_err());
        assert!(matches!(err, TypeError::NonExhaustiveMatch { .. }));
    }

    #[test]
    fn infer_non_exhaustive_list_missing_cons_error() {
        let expr = parse_expr("match [1] when [] -> 0");
        let mut ts = TypeSystem::with_prelude();
        let err = strip_span(ts.infer(expr.as_ref()).unwrap_err());
        match err {
            TypeError::NonExhaustiveMatch { missing, .. } => {
                assert_eq!(missing, vec![sym("Cons")]);
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
            .find(|p| p.class.as_ref() == "Field" && p.typ == Type::con("i32", 0))
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
            .find(|p| p.class.as_ref() == "Eq" && p.typ == dict_i32)
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
            .find(|p| p.class.as_ref() == "Ord" && p.typ == Type::con("bool", 0))
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
            .find(|p| p.class.as_ref() == "Functor" && p.typ == Type::con("Dict", 1))
            .expect("expected Functor Dict constraint");
        assert!(!entails(&ts.classes, &[], pred).unwrap());
    }
}

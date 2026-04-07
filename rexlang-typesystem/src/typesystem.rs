//! Core type system implementation for Rex.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::{self, Display, Formatter};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use rexlang_ast::expr::{
    ClassDecl, ClassMethodSig, Decl, DeclareFnDecl, Expr, FnDecl, InstanceDecl, InstanceMethodImpl,
    Pattern, Scope, Symbol, TypeConstraint, TypeDecl, TypeExpr, intern, sym,
};
use rexlang_lexer::span::Span;
use rexlang_util::{GasMeter, OutOfGas};
use rpds::HashTrieMapSync;
use uuid::Uuid;

use crate::prelude;

#[path = "inference.rs"]
pub mod inference;

pub use inference::{infer, infer_typed, infer_typed_with_gas, infer_with_gas};

pub type TypeVarId = usize;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum BuiltinTypeId {
    U8,
    U16,
    U32,
    U64,
    I8,
    I16,
    I32,
    I64,
    F32,
    F64,
    Bool,
    String,
    Uuid,
    DateTime,
    List,
    Array,
    Dict,
    Option,
    Promise,
    Result,
}

impl BuiltinTypeId {
    pub fn as_symbol(self) -> Symbol {
        sym(self.as_str())
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::U8 => "u8",
            Self::U16 => "u16",
            Self::U32 => "u32",
            Self::U64 => "u64",
            Self::I8 => "i8",
            Self::I16 => "i16",
            Self::I32 => "i32",
            Self::I64 => "i64",
            Self::F32 => "f32",
            Self::F64 => "f64",
            Self::Bool => "bool",
            Self::String => "string",
            Self::Uuid => "uuid",
            Self::DateTime => "datetime",
            Self::List => "List",
            Self::Array => "Array",
            Self::Dict => "Dict",
            Self::Option => "Option",
            Self::Promise => "Promise",
            Self::Result => "Result",
        }
    }

    pub fn arity(self) -> usize {
        match self {
            Self::List | Self::Array | Self::Dict | Self::Option | Self::Promise => 1,
            Self::Result => 2,
            _ => 0,
        }
    }

    pub fn from_symbol(name: &Symbol) -> Option<Self> {
        Self::from_name(name.as_ref())
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "u8" => Some(Self::U8),
            "u16" => Some(Self::U16),
            "u32" => Some(Self::U32),
            "u64" => Some(Self::U64),
            "i8" => Some(Self::I8),
            "i16" => Some(Self::I16),
            "i32" => Some(Self::I32),
            "i64" => Some(Self::I64),
            "f32" => Some(Self::F32),
            "f64" => Some(Self::F64),
            "bool" => Some(Self::Bool),
            "string" => Some(Self::String),
            "uuid" => Some(Self::Uuid),
            "datetime" => Some(Self::DateTime),
            "List" => Some(Self::List),
            "Array" => Some(Self::Array),
            "Dict" => Some(Self::Dict),
            "Option" => Some(Self::Option),
            "Promise" => Some(Self::Promise),
            "Result" => Some(Self::Result),
            _ => None,
        }
    }
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
    pub builtin_id: Option<BuiltinTypeId>,
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
        if let Some(id) = BuiltinTypeId::from_name(name.as_ref())
            && id.arity() == arity
        {
            return Self::builtin(id);
        }
        Self::user_con(name, arity)
    }

    pub fn user_con(name: impl AsRef<str>, arity: usize) -> Self {
        Type::new(TypeKind::Con(TypeConst {
            name: intern(name.as_ref()),
            arity,
            builtin_id: None,
        }))
    }

    pub fn builtin(id: BuiltinTypeId) -> Self {
        Type::new(TypeKind::Con(TypeConst {
            name: id.as_symbol(),
            arity: id.arity(),
            builtin_id: Some(id),
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

    pub fn list(elem: Type) -> Type {
        Type::app(Type::builtin(BuiltinTypeId::List), elem)
    }

    pub fn array(elem: Type) -> Type {
        Type::app(Type::builtin(BuiltinTypeId::Array), elem)
    }

    pub fn dict(elem: Type) -> Type {
        Type::app(Type::builtin(BuiltinTypeId::Dict), elem)
    }

    pub fn option(elem: Type) -> Type {
        Type::app(Type::builtin(BuiltinTypeId::Option), elem)
    }

    pub fn promise(elem: Type) -> Type {
        Type::app(Type::builtin(BuiltinTypeId::Promise), elem)
    }

    pub fn result(ok: Type, err: Type) -> Type {
        Type::app(Type::app(Type::builtin(BuiltinTypeId::Result), err), ok)
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
            TypeKind::Fun(_, _) => {
                // Avoid recursive descent on long function chains like
                // `a1 -> a2 -> ... -> an -> r`.
                let mut args = Vec::new();
                let mut changed = false;
                let mut cur: &Type = self;
                while let TypeKind::Fun(a, b) = cur.as_ref() {
                    let (a_new, a_changed) = a.apply_with_change(s);
                    changed |= a_changed;
                    args.push(a_new);
                    cur = b;
                }
                let (ret_new, ret_changed) = cur.apply_with_change(s);
                changed |= ret_changed;
                if !changed {
                    return (self.clone(), false);
                }
                let mut out = ret_new;
                for a_new in args.into_iter().rev() {
                    out = Type::fun(a_new, out);
                }
                (out, true)
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

impl AsRef<TypeKind> for Type {
    fn as_ref(&self) -> &TypeKind {
        self.0.as_ref()
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
                Some(name) => write!(f, "'{}", name),
                None => write!(f, "t{}", tv.id),
            },
            TypeKind::Con(c) => write!(f, "{}", c.name),
            TypeKind::App(l, r) => {
                // Internally `Result` is represented as `Result err ok` so it can be partially
                // applied as `Result err` for HKTs (Functor/Monad/etc).
                //
                // User-facing syntax is `Result ok err` (Rust-style), so render the fully
                // applied form with swapped arguments.
                if let TypeKind::App(head, err) = l.as_ref()
                    && matches!(
                        head.as_ref(),
                        TypeKind::Con(c)
                            if c.builtin_id == Some(BuiltinTypeId::Result) && c.arity == 2
                    )
                {
                    return write!(f, "(Result {} {})", r, err);
                }
                write!(f, "({} {})", l, r)
            }
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
        let mut out = HashSet::new();
        let mut stack: Vec<&Type> = vec![self];
        while let Some(t) = stack.pop() {
            match t.as_ref() {
                TypeKind::Var(tv) => {
                    out.insert(tv.id);
                }
                TypeKind::Con(_) => {}
                TypeKind::App(l, r) => {
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
                    for (_, ty) in fields {
                        stack.push(ty);
                    }
                }
            }
        }
        out
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
        match &self.kind {
            TypedExprKind::Lam { .. } => {
                let mut params: Vec<(Symbol, Type)> = Vec::new();
                let mut cur = self;
                while let TypedExprKind::Lam { param, body } = &cur.kind {
                    params.push((param.clone(), cur.typ.apply(s)));
                    cur = body.as_ref();
                }
                let mut out = cur.apply(s);
                for (param, typ) in params.into_iter().rev() {
                    out = TypedExpr {
                        typ,
                        kind: TypedExprKind::Lam {
                            param,
                            body: Box::new(out),
                        },
                    };
                }
                return out;
            }
            TypedExprKind::App(..) => {
                let mut apps: Vec<(Type, &TypedExpr)> = Vec::new();
                let mut cur = self;
                while let TypedExprKind::App(f, x) = &cur.kind {
                    apps.push((cur.typ.apply(s), x.as_ref()));
                    cur = f.as_ref();
                }
                let mut out = cur.apply(s);
                for (typ, arg) in apps.into_iter().rev() {
                    out = TypedExpr {
                        typ,
                        kind: TypedExprKind::App(Box::new(out), Box::new(arg.apply(s))),
                    };
                }
                return out;
            }
            _ => {}
        }

        let typ = self.typ.apply(s);
        let kind = match &self.kind {
            TypedExprKind::Bool(v) => TypedExprKind::Bool(*v),
            TypedExprKind::Uint(v) => TypedExprKind::Uint(*v),
            TypedExprKind::Int(v) => TypedExprKind::Int(*v),
            TypedExprKind::Float(v) => TypedExprKind::Float(*v),
            TypedExprKind::String(v) => TypedExprKind::String(v.clone()),
            TypedExprKind::Uuid(v) => TypedExprKind::Uuid(*v),
            TypedExprKind::DateTime(v) => TypedExprKind::DateTime(*v),
            TypedExprKind::Hole => TypedExprKind::Hole,
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
            TypedExprKind::RecordUpdate { base, updates } => {
                let mut out = BTreeMap::new();
                for (k, v) in updates {
                    out.insert(k.clone(), v.apply(s));
                }
                TypedExprKind::RecordUpdate {
                    base: Box::new(base.apply(s)),
                    updates: out,
                }
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
            TypedExprKind::LetRec { bindings, body } => TypedExprKind::LetRec {
                bindings: bindings
                    .iter()
                    .map(|(name, def)| (name.clone(), def.apply(s)))
                    .collect(),
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
    Hole,
    Tuple(Vec<TypedExpr>),
    List(Vec<TypedExpr>),
    Dict(BTreeMap<Symbol, TypedExpr>),
    RecordUpdate {
        base: Box<TypedExpr>,
        updates: BTreeMap<Symbol, TypedExpr>,
    },
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
    LetRec {
        bindings: Vec<(Symbol, TypedExpr)>,
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
    #[error("cannot redefine reserved builtin type `{0}`")]
    ReservedTypeName(Symbol),
    #[error("duplicate value definition `{0}`")]
    DuplicateValue(Symbol),
    #[error("duplicate class definition `{0}`")]
    DuplicateClass(Symbol),
    #[error("class `{class}` must have at least one type parameter (got {got})")]
    InvalidClassArity { class: Symbol, got: usize },
    #[error("duplicate class method `{0}`")]
    DuplicateClassMethod(Symbol),
    #[error("unknown method `{method}` in instance of class `{class}`")]
    UnknownInstanceMethod { class: Symbol, method: Symbol },
    #[error("missing implementation of `{method}` for instance of class `{class}`")]
    MissingInstanceMethod { class: Symbol, method: Symbol },
    #[error(
        "instance method `{method}` requires constraint {class} {typ}, but it is not in the instance context"
    )]
    MissingInstanceConstraint {
        method: Symbol,
        class: Symbol,
        typ: String,
    },
    #[error("unbound variable {0}")]
    UnknownVar(Symbol),
    #[error("ambiguous overload for {0}")]
    AmbiguousOverload(Symbol),
    #[error("ambiguous type variable(s) {vars:?} in constraints: {constraints}")]
    AmbiguousTypeVars {
        vars: Vec<TypeVarId>,
        constraints: String,
    },
    #[error(
        "kind mismatch for class `{class}`: expected {expected} type argument(s) remaining, got {got} for {typ}"
    )]
    KindMismatch {
        class: Symbol,
        expected: usize,
        got: usize,
        typ: String,
    },
    #[error("missing type class constraint(s): {constraints}")]
    MissingConstraints { constraints: String },
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
    #[error("internal error: {0}")]
    Internal(String),
    #[error("{0}")]
    OutOfGas(#[from] OutOfGas),
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

fn format_constraints_referencing_vars(preds: &[Predicate], vars: &[TypeVarId]) -> String {
    if vars.is_empty() {
        return String::new();
    }
    let var_set: HashSet<TypeVarId> = vars.iter().copied().collect();
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

fn reject_ambiguous_scheme(scheme: &Scheme) -> Result<(), TypeError> {
    // Only reject *quantified* ambiguous variables. Variables free in the
    // environment are allowed to appear only in predicates, since they can be
    // determined by outer context.
    let quantified: HashSet<TypeVarId> = scheme.vars.iter().map(|v| v.id).collect();
    if quantified.is_empty() {
        return Ok(());
    }

    let typ_ftv = scheme.typ.ftv();
    let mut vars = HashSet::new();
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

fn scheme_compatible(existing: &Scheme, declared: &Scheme) -> bool {
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

#[derive(Debug)]
struct Unifier<'g> {
    // `subs[id] = Some(t)` means type variable `id` has been bound to `t`.
    //
    // This is intentionally a dense `Vec` rather than a `HashMap`: inference
    // generates `TypeVarId`s from a monotonic counter, so the common case is
    // “small id space, lots of lookups”. This makes the cost model obvious:
    // you pay O(max_id) space, and you get O(1) binds/queries.
    subs: Vec<Option<Type>>,
    gas: Option<&'g mut GasMeter>,
    max_infer_depth: Option<usize>,
    infer_depth: usize,
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
    let closure_keys: HashSet<String> = closure
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

impl<'g> Unifier<'g> {
    fn new(max_infer_depth: Option<usize>) -> Self {
        Self {
            subs: Vec::new(),
            gas: None,
            max_infer_depth,
            infer_depth: 0,
        }
    }

    fn with_gas(gas: &'g mut GasMeter, max_infer_depth: Option<usize>) -> Self {
        Self {
            subs: Vec::new(),
            gas: Some(gas),
            max_infer_depth,
            infer_depth: 0,
        }
    }

    fn with_infer_depth<T>(
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

    fn charge_infer_node(&mut self) -> Result<(), TypeError> {
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

fn bind(tv: &TypeVar, t: &Type) -> Result<Subst, TypeError> {
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

    pub fn remove(&mut self, name: &Symbol) {
        self.values = self.values.remove(name);
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

fn is_integral_literal_expr(expr: &Expr) -> bool {
    matches!(expr, Expr::Int(..) | Expr::Uint(..))
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
    pub adts: HashMap<Symbol, AdtDecl>,
    pub class_info: HashMap<Symbol, ClassInfo>,
    pub class_methods: HashMap<Symbol, ClassMethodInfo>,
    /// Names introduced by `declare fn` (forward declarations).
    ///
    /// These are placeholders in the type environment and must not block a later
    /// real definition (e.g. `fn foo = ...` or host/CLI injection).
    pub declared_values: HashSet<Symbol>,
    pub supply: TypeVarSupply,
    limits: TypeSystemLimits,
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
#[derive(Clone, Debug)]
pub struct ClassInfo {
    pub name: Symbol,
    pub params: Vec<Symbol>,
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
            declared_values: HashSet::new(),
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
                let mut vars = HashMap::new();
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

                let mut vars: HashMap<Symbol, TypeVar> = HashMap::new();
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
        .map_err(|err| with_span(&span, err))
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

            let mut vars: HashMap<Symbol, TypeVar> = HashMap::new();
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
        .map_err(|err| with_span(&span, err))
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

            let mut vars: HashMap<Symbol, TypeVar> = HashMap::new();
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
        .map_err(|err| with_span(&span, err))
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
                ann_vars: HashMap<Symbol, TypeVar>,
            }

            let mut infos: Vec<FnInfo> = Vec::with_capacity(decls.len());
            let mut seen_names = HashSet::new();

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

                    let mut ann_vars: HashMap<Symbol, TypeVar> = HashMap::new();
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
                    let var_arities: HashMap<TypeVarId, usize> = ann_vars
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

                infos.push(info.map_err(|err| with_span(&span, err))?);
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
                let var_arities: HashMap<TypeVarId, usize> = info
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
                            return Err(with_span(
                                &span,
                                TypeError::KindMismatch {
                                    class: pred.class.clone(),
                                    expected: expected_arity,
                                    got,
                                    typ: arg.to_string(),
                                },
                            ));
                        }
                    }
                }

                check_non_ground_predicates_declared(&self.classes, &declared_preds, &preds)
                    .map_err(|err| with_span(&span, err))?;

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

            let mut ann_vars: HashMap<Symbol, TypeVar> = HashMap::new();
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
        params: &HashMap<Symbol, TypeVar>,
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
        res.map_err(|err| with_span(&span, err))
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

    fn check_predicate_kinds(&self, preds: &[Predicate]) -> Result<(), TypeError> {
        for pred in preds {
            self.check_predicate_kind(pred)?;
        }
        Ok(())
    }
}

fn type_from_annotation_expr(
    adts: &HashMap<Symbol, AdtDecl>,
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
    res.map_err(|err| with_span(&span, err))
}

fn annotation_type_arity(adts: &HashMap<Symbol, AdtDecl>, name: &Symbol) -> Option<usize> {
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

fn predicates_from_constraints(
    adts: &HashMap<Symbol, AdtDecl>,
    constraints: &[TypeConstraint],
    vars: &mut HashMap<Symbol, TypeVar>,
    supply: &mut TypeVarSupply,
) -> Result<Vec<Predicate>, TypeError> {
    let mut out = Vec::with_capacity(constraints.len());
    for constraint in constraints {
        let ty = type_from_annotation_expr_vars(adts, &constraint.typ, vars, supply)?;
        out.push(Predicate::new(constraint.class.as_ref(), ty));
    }
    Ok(out)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdtConflict {
    pub name: Symbol,
    pub definitions: Vec<Type>,
}

#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
#[error("conflicting ADT definitions: {conflicts:?}")]
pub struct CollectAdtsError {
    pub conflicts: Vec<AdtConflict>,
}

/// Collect all user-defined ADT constructors referenced by the provided types.
///
/// This walks each type recursively (including nested occurrences), returns a
/// deduplicated list of constructor heads, and rejects ambiguous constructor
/// names that appear with incompatible definitions.
///
/// The returned `Type`s are constructor heads (for example `Foo`), suitable
/// for passing to embedder utilities that derive `AdtDecl`s from type
/// constructors.
///
/// # Examples
///
/// ```rust,ignore
/// use rex_ts::{collect_adts_in_types, BuiltinTypeId, Type};
///
/// let types = vec![
///     Type::app(Type::user_con("Foo", 1), Type::builtin(BuiltinTypeId::I32)),
///     Type::fun(Type::user_con("Bar", 0), Type::user_con("Foo", 1)),
/// ];
///
/// let adts = collect_adts_in_types(types).unwrap();
/// assert_eq!(adts, vec![Type::user_con("Foo", 1), Type::user_con("Bar", 0)]);
/// ```
///
/// ```rust,ignore
/// use rex_ts::{collect_adts_in_types, Type};
///
/// let err = collect_adts_in_types(vec![
///     Type::user_con("Thing", 1),
///     Type::user_con("Thing", 2),
/// ])
/// .unwrap_err();
///
/// assert_eq!(err.conflicts.len(), 1);
/// assert_eq!(err.conflicts[0].name.as_ref(), "Thing");
/// ```
pub fn collect_adts_in_types(types: Vec<Type>) -> Result<Vec<Type>, CollectAdtsError> {
    fn visit(
        typ: &Type,
        out: &mut Vec<Type>,
        seen: &mut HashSet<Type>,
        defs_by_name: &mut BTreeMap<Symbol, Vec<Type>>,
    ) {
        match typ.as_ref() {
            TypeKind::Var(_) => {}
            TypeKind::Con(tc) => {
                // Builtins are not embeddable ADT declarations.
                if tc.builtin_id.is_none() {
                    let adt = Type::new(TypeKind::Con(tc.clone()));
                    if seen.insert(adt.clone()) {
                        out.push(adt.clone());
                    }
                    let defs = defs_by_name.entry(tc.name.clone()).or_default();
                    if !defs.contains(&adt) {
                        defs.push(adt);
                    }
                }
            }
            TypeKind::App(fun, arg) => {
                visit(fun, out, seen, defs_by_name);
                visit(arg, out, seen, defs_by_name);
            }
            TypeKind::Fun(arg, ret) => {
                visit(arg, out, seen, defs_by_name);
                visit(ret, out, seen, defs_by_name);
            }
            TypeKind::Tuple(elems) => {
                for elem in elems {
                    visit(elem, out, seen, defs_by_name);
                }
            }
            TypeKind::Record(fields) => {
                for (_name, field_ty) in fields {
                    visit(field_ty, out, seen, defs_by_name);
                }
            }
        }
    }

    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut defs_by_name: BTreeMap<Symbol, Vec<Type>> = BTreeMap::new();
    for typ in &types {
        visit(typ, &mut out, &mut seen, &mut defs_by_name);
    }

    let conflicts: Vec<AdtConflict> = defs_by_name
        .into_iter()
        .filter_map(|(name, definitions)| {
            (definitions.len() > 1).then_some(AdtConflict { name, definitions })
        })
        .collect();
    if !conflicts.is_empty() {
        return Err(CollectAdtsError { conflicts });
    }

    Ok(out)
}

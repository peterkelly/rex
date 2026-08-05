use crate::{
    error::{AdtConflict, CollectAdtsError, TypeError},
    typesystem::TypeVarSupply,
    unification::{Subst, subst_is_empty},
};
use blake3::Hash;
use chrono::{DateTime, Utc};
use rex_ast::{Pattern, Symbol};
use rpds::HashTrieMapSync;
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet, HashMap},
    fmt::{self, Display, Formatter},
    mem,
    sync::Arc,
};
use uuid::Uuid;

pub type TypeVarId = usize;

#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
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
    Hash,
    DateTime,
    List,
    Dict,
    Option,
    Promise,
    Result,
}

impl BuiltinTypeId {
    pub fn as_symbol(self) -> Symbol {
        Symbol::intern(self.as_str())
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
            Self::Bool => "Bool",
            Self::String => "String",
            Self::Uuid => "UUID",
            Self::Hash => "Hash",
            Self::DateTime => "DateTime",
            Self::List => "List",
            Self::Dict => "Dict",
            Self::Option => "Option",
            Self::Promise => "Promise",
            Self::Result => "Result",
        }
    }

    pub fn arity(self) -> usize {
        match self {
            Self::List | Self::Dict | Self::Option | Self::Promise => 1,
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
            "Bool" => Some(Self::Bool),
            "String" => Some(Self::String),
            "UUID" => Some(Self::Uuid),
            "Hash" => Some(Self::Hash),
            "DateTime" => Some(Self::DateTime),
            "List" => Some(Self::List),
            "Dict" => Some(Self::Dict),
            "Option" => Some(Self::Option),
            "Promise" => Some(Self::Promise),
            "Result" => Some(Self::Result),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
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

#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub enum TypeConst {
    Builtin(BuiltinTypeId),
    User { name: Symbol, arity: usize },
}

impl TypeConst {
    pub fn builtin_id(&self) -> Option<BuiltinTypeId> {
        match self {
            Self::Builtin(id) => Some(*id),
            Self::User { .. } => None,
        }
    }

    pub fn is_builtin(&self, id: BuiltinTypeId) -> bool {
        self.builtin_id() == Some(id)
    }

    pub fn name(&self) -> Symbol {
        match self {
            Self::Builtin(id) => id.as_symbol(),
            Self::User { name, .. } => name.clone(),
        }
    }

    pub fn name_str(&self) -> &str {
        match self {
            Self::Builtin(id) => id.as_str(),
            Self::User { name, .. } => name.as_ref(),
        }
    }

    pub fn user_name(&self) -> Option<&Symbol> {
        match self {
            Self::Builtin(_) => None,
            Self::User { name, .. } => Some(name),
        }
    }

    pub fn arity(&self) -> usize {
        match self {
            Self::Builtin(id) => id.arity(),
            Self::User { arity, .. } => *arity,
        }
    }
}

impl Ord for TypeConst {
    fn cmp(&self, other: &Self) -> Ordering {
        self.name_str()
            .cmp(other.name_str())
            .then_with(|| self.arity().cmp(&other.arity()))
            .then_with(|| self.builtin_id().cmp(&other.builtin_id()))
    }
}

impl PartialOrd for TypeConst {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub struct Type(Arc<TypeKind>);

impl From<&Type> for Type {
    fn from(value: &Type) -> Self {
        value.clone()
    }
}

#[derive(Clone, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
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
        Type::new(TypeKind::Con(TypeConst::User {
            name: Symbol::intern(name.as_ref()),
            arity,
        }))
    }

    pub fn builtin(id: BuiltinTypeId) -> Self {
        Type::new(TypeKind::Con(TypeConst::Builtin(id)))
    }

    pub fn var(tv: TypeVar) -> Self {
        Type::new(TypeKind::Var(tv))
    }

    pub fn fun(a: impl Into<Type>, b: impl Into<Type>) -> Self {
        Type::new(TypeKind::Fun(a.into(), b.into()))
    }

    pub fn app(f: impl Into<Type>, arg: impl Into<Type>) -> Self {
        Type::new(TypeKind::App(f.into(), arg.into()))
    }

    pub fn tuple<I, T>(elems: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<Type>,
    {
        Type::new(TypeKind::Tuple(elems.into_iter().map(Into::into).collect()))
    }

    pub fn record<I, T>(fields: I) -> Self
    where
        I: IntoIterator<Item = (Symbol, T)>,
        T: Into<Type>,
    {
        let mut fields: Vec<(Symbol, Type)> = fields
            .into_iter()
            .map(|(name, typ)| (name, typ.into()))
            .collect();
        // Canonicalize records so downstream code can rely on “same shape means
        // same ordering”. (This is a correctness invariant, not a nicety.)
        fields.sort_by(|a, b| a.0.as_ref().cmp(b.0.as_ref()));
        Type::new(TypeKind::Record(fields))
    }

    pub fn list(elem: impl Into<Type>) -> Type {
        Type::app(Type::builtin(BuiltinTypeId::List), elem)
    }

    pub fn dict(elem: impl Into<Type>) -> Type {
        Type::app(Type::builtin(BuiltinTypeId::Dict), elem)
    }

    pub fn option(elem: impl Into<Type>) -> Type {
        Type::app(Type::builtin(BuiltinTypeId::Option), elem)
    }

    pub fn promise(elem: impl Into<Type>) -> Type {
        Type::app(Type::builtin(BuiltinTypeId::Promise), elem)
    }

    pub fn result(ok: impl Into<Type>, err: impl Into<Type>) -> Type {
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

    pub fn for_each<F>(&self, mut f: F) -> Type
    where
        F: FnMut(&Type),
    {
        self.transform(|t| {
            f(t);
            None
        })
    }

    pub fn transform<F>(&self, mut f: F) -> Type
    where
        F: FnMut(&Type) -> Option<Type>,
    {
        self.transform_ref(&mut f)
    }

    fn transform_ref<F>(&self, f: &mut F) -> Type
    where
        F: FnMut(&Type) -> Option<Type>,
    {
        if let Some(repl) = f(self) {
            return repl;
        }

        match self.as_ref() {
            TypeKind::Var(type_var) => Type(Arc::new(TypeKind::Var(type_var.clone()))),
            TypeKind::Con(type_const) => Type(Arc::new(TypeKind::Con(type_const.clone()))),
            TypeKind::App(fun, arg) => Type(Arc::new(TypeKind::App(
                fun.transform_ref(f),
                arg.transform_ref(f),
            ))),
            TypeKind::Fun(arg, res) => Type(Arc::new(TypeKind::Fun(
                arg.transform_ref(f),
                res.transform_ref(f),
            ))),
            TypeKind::Tuple(ts) => Type(Arc::new(TypeKind::Tuple(
                ts.iter().map(|t| t.transform_ref(f)).collect(),
            ))),
            TypeKind::Record(fields) => Type(Arc::new(TypeKind::Record(
                fields
                    .iter()
                    .map(|(s, t)| (s.clone(), t.transform_ref(f)))
                    .collect(),
            ))),
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
            TypeKind::Con(c) => write!(f, "{}", c.name_str()),
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
                            if c.is_builtin(BuiltinTypeId::Result) && c.arity() == 2
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

#[derive(Clone, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub struct Predicate {
    pub class: Symbol,
    pub typ: Type,
}

impl Predicate {
    pub fn new(class: impl AsRef<str>, typ: impl Into<Type>) -> Self {
        Self {
            class: Symbol::intern(class.as_ref()),
            typ: typ.into(),
        }
    }
}

#[derive(Clone, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub struct Scheme {
    pub vars: Vec<TypeVar>,
    pub preds: Vec<Predicate>,
    pub typ: Type,
}

impl Scheme {
    pub fn new(vars: Vec<TypeVar>, preds: Vec<Predicate>, typ: impl Into<Type>) -> Self {
        Self {
            vars,
            preds,
            typ: typ.into(),
        }
    }
}

pub trait Types: Sized {
    fn apply(&self, s: &Subst) -> Self;
    fn ftv(&self) -> BTreeSet<TypeVarId>;
}

impl Types for Type {
    fn apply(&self, s: &Subst) -> Self {
        self.apply_with_change(s).0
    }

    fn ftv(&self) -> BTreeSet<TypeVarId> {
        let mut out = BTreeSet::new();
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

    fn ftv(&self) -> BTreeSet<TypeVarId> {
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

    fn ftv(&self) -> BTreeSet<TypeVarId> {
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

    fn ftv(&self) -> BTreeSet<TypeVarId> {
        self.iter().flat_map(Types::ftv).collect()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TypedExpr {
    pub typ: Type,
    pub kind: Arc<TypedExprKind>,
}

struct TypedTailAppFrame {
    head: Arc<TypedExpr>,
    prefix_args: Vec<(Type, Arc<TypedExpr>)>,
    tail_result_type: Type,
}

fn collect_typed_app_chain(expr: &TypedExpr) -> (Arc<TypedExpr>, Vec<(Type, Arc<TypedExpr>)>) {
    let mut args = Vec::new();
    let mut cur = expr;
    while let TypedExprKind::App(f, x) = cur.kind.as_ref() {
        args.push((cur.typ.clone(), Arc::clone(x)));
        cur = f.as_ref();
    }
    args.reverse();
    (Arc::new(cur.clone()), args)
}

fn collect_typed_tail_app_chain(
    expr: &TypedExpr,
) -> Option<(Arc<TypedExpr>, Vec<TypedTailAppFrame>)> {
    let mut frames = Vec::new();
    let mut cur = Arc::new(expr.clone());
    while matches!(cur.kind.as_ref(), TypedExprKind::App(..)) {
        let (head, mut args) = collect_typed_app_chain(cur.as_ref());
        let Some((tail_result_type, tail)) = args.pop() else {
            break;
        };
        if !matches!(tail.kind.as_ref(), TypedExprKind::App(..)) {
            break;
        }
        frames.push(TypedTailAppFrame {
            head,
            prefix_args: args,
            tail_result_type,
        });
        cur = tail;
    }
    (!frames.is_empty()).then_some((cur, frames))
}

fn typed_drop_placeholder() -> Arc<TypedExpr> {
    Arc::new(TypedExpr::new(
        Type::tuple(Vec::<Type>::new()),
        TypedExprKind::Hole,
    ))
}

fn drain_typed_expr_kind(kind: &mut TypedExprKind, stack: &mut Vec<Arc<TypedExpr>>) {
    match kind {
        TypedExprKind::Tuple(elems) | TypedExprKind::List(elems) => {
            stack.extend(mem::take(elems));
        }
        TypedExprKind::Dict(kvs) => {
            stack.extend(mem::take(kvs).into_values());
        }
        TypedExprKind::RecordUpdate { base, updates } => {
            stack.push(mem::replace(base, typed_drop_placeholder()));
            stack.extend(mem::take(updates).into_values());
        }
        TypedExprKind::App(f, x) => {
            stack.push(mem::replace(f, typed_drop_placeholder()));
            stack.push(mem::replace(x, typed_drop_placeholder()));
        }
        TypedExprKind::Project { expr, .. } => {
            stack.push(mem::replace(expr, typed_drop_placeholder()));
        }
        TypedExprKind::Lam { body, .. } => {
            stack.push(mem::replace(body, typed_drop_placeholder()));
        }
        TypedExprKind::Let { def, body, .. } => {
            stack.push(mem::replace(def, typed_drop_placeholder()));
            stack.push(mem::replace(body, typed_drop_placeholder()));
        }
        TypedExprKind::LetRec { bindings, body } => {
            for (_name, def) in mem::take(bindings) {
                stack.push(def);
            }
            stack.push(mem::replace(body, typed_drop_placeholder()));
        }
        TypedExprKind::Ite {
            cond,
            then_expr,
            else_expr,
        } => {
            stack.push(mem::replace(cond, typed_drop_placeholder()));
            stack.push(mem::replace(then_expr, typed_drop_placeholder()));
            stack.push(mem::replace(else_expr, typed_drop_placeholder()));
        }
        TypedExprKind::Match { scrutinee, arms } => {
            stack.push(mem::replace(scrutinee, typed_drop_placeholder()));
            for (_pat, arm) in mem::take(arms) {
                stack.push(arm);
            }
        }
        TypedExprKind::Bool(..)
        | TypedExprKind::Uint(..)
        | TypedExprKind::Int(..)
        | TypedExprKind::Float(..)
        | TypedExprKind::String(..)
        | TypedExprKind::Uuid(..)
        | TypedExprKind::DateTime(..)
        | TypedExprKind::Hole
        | TypedExprKind::Var { .. } => {}
    }
}

impl Drop for TypedExpr {
    fn drop(&mut self) {
        let Some(kind) = Arc::get_mut(&mut self.kind) else {
            return;
        };
        let mut stack = Vec::new();
        drain_typed_expr_kind(kind, &mut stack);
        while let Some(mut expr) = stack.pop() {
            let Some(expr) = Arc::get_mut(&mut expr) else {
                continue;
            };
            let Some(kind) = Arc::get_mut(&mut expr.kind) else {
                continue;
            };
            drain_typed_expr_kind(kind, &mut stack);
        }
    }
}

impl TypedExpr {
    pub fn new(typ: Type, kind: TypedExprKind) -> Self {
        Self {
            typ,
            kind: Arc::new(kind),
        }
    }

    pub fn apply(&self, s: &Subst) -> Self {
        // TODO: This still allocates a transformed expression tree. That may
        // become too expensive for hot polymorphic apply paths once evaluator
        // frames retain shared typed AST nodes.
        match self.kind.as_ref() {
            TypedExprKind::Lam { .. } => {
                let mut params: Vec<(Symbol, Type)> = Vec::new();
                let mut cur = self;
                while let TypedExprKind::Lam { param, body } = cur.kind.as_ref() {
                    params.push((param.clone(), cur.typ.apply(s)));
                    cur = body.as_ref();
                }
                let mut out = cur.apply(s);
                for (param, typ) in params.into_iter().rev() {
                    out = TypedExpr::new(
                        typ,
                        TypedExprKind::Lam {
                            param,
                            body: Arc::new(out),
                        },
                    );
                }
                return out;
            }
            TypedExprKind::App(..) => {
                if let Some((leaf, frames)) = collect_typed_tail_app_chain(self) {
                    let mut out = leaf.apply(s);
                    for frame in frames.into_iter().rev() {
                        let mut typed = frame.head.apply(s);
                        for (typ, arg) in frame.prefix_args {
                            typed = TypedExpr::new(
                                typ.apply(s),
                                TypedExprKind::App(Arc::new(typed), Arc::new(arg.apply(s))),
                            );
                        }
                        out = TypedExpr::new(
                            frame.tail_result_type.apply(s),
                            TypedExprKind::App(Arc::new(typed), Arc::new(out)),
                        );
                    }
                    return out;
                }

                let mut apps: Vec<(Type, Arc<TypedExpr>)> = Vec::new();
                let mut cur = self;
                while let TypedExprKind::App(f, x) = cur.kind.as_ref() {
                    apps.push((cur.typ.apply(s), Arc::clone(x)));
                    cur = f.as_ref();
                }
                let mut out = cur.apply(s);
                for (typ, arg) in apps.into_iter().rev() {
                    out = TypedExpr::new(
                        typ,
                        TypedExprKind::App(Arc::new(out), Arc::new(arg.apply(s))),
                    );
                }
                return out;
            }
            _ => {}
        }

        let typ = self.typ.apply(s);
        let kind = match self.kind.as_ref() {
            TypedExprKind::Bool(v) => TypedExprKind::Bool(*v),
            TypedExprKind::Uint(v) => TypedExprKind::Uint(*v),
            TypedExprKind::Int(v) => TypedExprKind::Int(*v),
            TypedExprKind::Float(v) => TypedExprKind::Float(*v),
            TypedExprKind::String(v) => TypedExprKind::String(v.clone()),
            TypedExprKind::Uuid(v) => TypedExprKind::Uuid(*v),
            TypedExprKind::DateTime(v) => TypedExprKind::DateTime(*v),
            TypedExprKind::Hole => TypedExprKind::Hole,
            TypedExprKind::Tuple(elems) => {
                TypedExprKind::Tuple(elems.iter().map(|e| Arc::new(e.apply(s))).collect())
            }
            TypedExprKind::List(elems) => {
                TypedExprKind::List(elems.iter().map(|e| Arc::new(e.apply(s))).collect())
            }
            TypedExprKind::Dict(kvs) => {
                let mut out = BTreeMap::new();
                for (k, v) in kvs {
                    out.insert(k.clone(), Arc::new(v.apply(s)));
                }
                TypedExprKind::Dict(out)
            }
            TypedExprKind::RecordUpdate { base, updates } => {
                let mut out = BTreeMap::new();
                for (k, v) in updates {
                    out.insert(k.clone(), Arc::new(v.apply(s)));
                }
                TypedExprKind::RecordUpdate {
                    base: Arc::new(base.apply(s)),
                    updates: out,
                }
            }
            TypedExprKind::Var { name, overloads } => TypedExprKind::Var {
                name: name.clone(),
                overloads: overloads.iter().map(|t| t.apply(s)).collect(),
            },
            TypedExprKind::App(f, x) => {
                TypedExprKind::App(Arc::new(f.apply(s)), Arc::new(x.apply(s)))
            }
            TypedExprKind::Project { expr, field } => TypedExprKind::Project {
                expr: Arc::new(expr.apply(s)),
                field: field.clone(),
            },
            TypedExprKind::Lam { param, body } => TypedExprKind::Lam {
                param: param.clone(),
                body: Arc::new(body.apply(s)),
            },
            TypedExprKind::Let { name, def, body } => TypedExprKind::Let {
                name: name.clone(),
                def: Arc::new(def.apply(s)),
                body: Arc::new(body.apply(s)),
            },
            TypedExprKind::LetRec { bindings, body } => TypedExprKind::LetRec {
                bindings: bindings
                    .iter()
                    .map(|(name, def)| (name.clone(), Arc::new(def.apply(s))))
                    .collect(),
                body: Arc::new(body.apply(s)),
            },
            TypedExprKind::Ite {
                cond,
                then_expr,
                else_expr,
            } => TypedExprKind::Ite {
                cond: Arc::new(cond.apply(s)),
                then_expr: Arc::new(then_expr.apply(s)),
                else_expr: Arc::new(else_expr.apply(s)),
            },
            TypedExprKind::Match { scrutinee, arms } => TypedExprKind::Match {
                scrutinee: Arc::new(scrutinee.apply(s)),
                arms: arms
                    .iter()
                    .map(|(p, e)| (p.clone(), Arc::new(e.apply(s))))
                    .collect(),
            },
        };
        TypedExpr::new(typ, kind)
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
    Tuple(Vec<Arc<TypedExpr>>),
    List(Vec<Arc<TypedExpr>>),
    Dict(BTreeMap<Symbol, Arc<TypedExpr>>),
    RecordUpdate {
        base: Arc<TypedExpr>,
        updates: BTreeMap<Symbol, Arc<TypedExpr>>,
    },
    Var {
        name: Symbol,
        overloads: Vec<Type>,
    },
    App(Arc<TypedExpr>, Arc<TypedExpr>),
    Project {
        expr: Arc<TypedExpr>,
        field: Symbol,
    },
    Lam {
        param: Symbol,
        body: Arc<TypedExpr>,
    },
    Let {
        name: Symbol,
        def: Arc<TypedExpr>,
        body: Arc<TypedExpr>,
    },
    LetRec {
        bindings: Vec<(Symbol, Arc<TypedExpr>)>,
        body: Arc<TypedExpr>,
    },
    Ite {
        cond: Arc<TypedExpr>,
        then_expr: Arc<TypedExpr>,
        else_expr: Arc<TypedExpr>,
    },
    Match {
        scrutinee: Arc<TypedExpr>,
        arms: Vec<(Pattern, Arc<TypedExpr>)>,
    },
}

#[derive(Default, Debug, Clone)]
pub struct TypeEnv {
    pub values: HashTrieMapSync<Symbol, Vec<Scheme>>,
    pub type_vars: BTreeMap<Symbol, TypeVar>,
}

impl TypeEnv {
    pub fn new() -> Self {
        Self {
            values: HashTrieMapSync::new_sync(),
            type_vars: BTreeMap::new(),
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
        TypeEnv {
            values,
            type_vars: self.type_vars.clone(),
        }
    }

    fn ftv(&self) -> BTreeSet<TypeVarId> {
        self.values
            .iter()
            .flat_map(|(_, schemes)| schemes.iter().flat_map(Types::ftv))
            .collect()
    }
}

/// A named type parameter for an ADT (e.g. `a` in `List a`).
#[derive(Clone, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub struct AdtParam {
    pub name: Symbol,
    pub var: TypeVar,
}

/// A single ADT variant with zero or more constructor arguments.
#[derive(Clone, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub struct AdtVariant {
    pub name: Symbol,
    pub args: Vec<Type>,
}

/// A type declaration for an algebraic data type.
///
/// This only describes the *type* surface (params + variants). It does not
/// introduce any runtime values by itself. Runtime values are created by
/// injecting constructor schemes into the environment (see `inject_adt`).
#[derive(Clone, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
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

/// Rust-side type metadata for values that can appear at a Rex boundary.
///
/// Implement this trait for any Rust type that appears in a typed host function
/// signature, a derived Rex ADT field, or other embedder-facing conversion
/// point. The returned [`Type`] is the Rex type that users see in signatures and
/// type errors.
///
/// Primitive Rust types such as integers, floats, `bool`, `String`, `Vec<T>`,
/// `Option<T>`, and `Result<T, E>` already implement `RexType`. For Rust structs
/// and enums that should be visible as Rex algebraic data types, prefer
/// `#[derive(rex::Rex)]`, which implements both `RexType` and [`RexAdt`].
pub trait RexType {
    /// Return the Rex type corresponding to `Self`.
    ///
    /// This type is used when Rex builds host function signatures, checks calls
    /// to native functions, and discovers the declarations needed for ADT
    /// registration.
    fn rex_type() -> Type;

    /// Append Rex ADT declarations required by this type to `out`.
    ///
    /// The default implementation is intentionally empty, which is correct for
    /// primitive and leaf types that do not introduce Rex ADT declarations.
    /// Derived ADTs override this to collect declarations for the full acyclic
    /// family reachable from `Self` and then append their own declaration.
    ///
    /// Callers that register the family are responsible for ordering and
    /// validating the declarations before injection.
    fn collect_rex_family(_out: &mut Vec<AdtDecl>) -> Result<(), TypeError> {
        Ok(())
    }
}

/// Rust-side declaration metadata for a type represented as a Rex ADT.
///
/// `RexAdt` extends [`RexType`] for Rust structs and enums that have a named Rex
/// algebraic data type declaration. The engine and module APIs use this trait to
/// register constructors and type declarations before Rex code constructs or
/// consumes values of the Rust type.
///
/// Most embedders should derive this with `#[derive(rex::Rex)]`. Manual
/// implementations are useful for hand-written bridges or types whose Rex shape
/// differs from their Rust fields.
pub trait RexAdt: RexType {
    /// Return the single Rex ADT declaration for `Self`.
    ///
    /// This should describe only the type represented by `Self`; dependencies
    /// belong in [`RexType::collect_rex_family`].
    fn rex_adt_decl() -> Result<AdtDecl, TypeError>;

    /// Return the ADT family needed to register `Self`.
    ///
    /// The default implementation delegates to [`RexType::collect_rex_family`].
    /// Derived implementations of `collect_rex_family` include dependencies and
    /// `Self`; manual implementations can override this method when they need a
    /// custom family collection strategy.
    fn rex_adt_family() -> Result<Vec<AdtDecl>, TypeError> {
        let mut out = Vec::new();
        <Self as RexType>::collect_rex_family(&mut out)?;
        Ok(out)
    }
}

impl RexType for bool {
    fn rex_type() -> Type {
        Type::builtin(BuiltinTypeId::Bool)
    }
}

impl RexType for u8 {
    fn rex_type() -> Type {
        Type::builtin(BuiltinTypeId::U8)
    }
}

impl RexType for u16 {
    fn rex_type() -> Type {
        Type::builtin(BuiltinTypeId::U16)
    }
}

impl RexType for u32 {
    fn rex_type() -> Type {
        Type::builtin(BuiltinTypeId::U32)
    }
}

impl RexType for u64 {
    fn rex_type() -> Type {
        Type::builtin(BuiltinTypeId::U64)
    }
}

impl RexType for i8 {
    fn rex_type() -> Type {
        Type::builtin(BuiltinTypeId::I8)
    }
}

impl RexType for i16 {
    fn rex_type() -> Type {
        Type::builtin(BuiltinTypeId::I16)
    }
}

impl RexType for i32 {
    fn rex_type() -> Type {
        Type::builtin(BuiltinTypeId::I32)
    }
}

impl RexType for i64 {
    fn rex_type() -> Type {
        Type::builtin(BuiltinTypeId::I64)
    }
}

impl RexType for f32 {
    fn rex_type() -> Type {
        Type::builtin(BuiltinTypeId::F32)
    }
}

impl RexType for f64 {
    fn rex_type() -> Type {
        Type::builtin(BuiltinTypeId::F64)
    }
}

impl RexType for String {
    fn rex_type() -> Type {
        Type::builtin(BuiltinTypeId::String)
    }
}

impl RexType for &str {
    fn rex_type() -> Type {
        Type::builtin(BuiltinTypeId::String)
    }
}

impl RexType for Uuid {
    fn rex_type() -> Type {
        Type::builtin(BuiltinTypeId::Uuid)
    }
}

impl RexType for Hash {
    fn rex_type() -> Type {
        Type::builtin(BuiltinTypeId::Hash)
    }
}

impl RexType for DateTime<Utc> {
    fn rex_type() -> Type {
        Type::builtin(BuiltinTypeId::DateTime)
    }
}

impl<T: RexType> RexType for Vec<T> {
    fn rex_type() -> Type {
        Type::app(Type::builtin(BuiltinTypeId::List), T::rex_type())
    }
}

impl<T: RexType> RexType for BTreeMap<String, T> {
    fn rex_type() -> Type {
        Type::dict(T::rex_type())
    }
}

impl<T: RexType> RexType for HashMap<String, T> {
    fn rex_type() -> Type {
        Type::dict(T::rex_type())
    }
}

impl<T: RexType> RexType for Option<T> {
    fn rex_type() -> Type {
        Type::app(Type::builtin(BuiltinTypeId::Option), T::rex_type())
    }
}

impl<T: RexType, E: RexType> RexType for Result<T, E> {
    fn rex_type() -> Type {
        Type::app(
            Type::app(Type::builtin(BuiltinTypeId::Result), E::rex_type()),
            T::rex_type(),
        )
    }
}

impl RexType for () {
    fn rex_type() -> Type {
        Type::tuple(Vec::<Type>::new())
    }
}

macro_rules! impl_tuple_rex_type {
    ($($name:ident),+) => {
        impl<$($name: RexType),+> RexType for ($($name,)+) {
            fn rex_type() -> Type {
                Type::tuple(vec![$($name::rex_type()),+])
            }
        }
    };
}

impl_tuple_rex_type!(A0);
impl_tuple_rex_type!(A0, A1);
impl_tuple_rex_type!(A0, A1, A2);
impl_tuple_rex_type!(A0, A1, A2, A3);
impl_tuple_rex_type!(A0, A1, A2, A3, A4);
impl_tuple_rex_type!(A0, A1, A2, A3, A4, A5);
impl_tuple_rex_type!(A0, A1, A2, A3, A4, A5, A6);
impl_tuple_rex_type!(A0, A1, A2, A3, A4, A5, A6, A7);

#[derive(Clone, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub struct Class {
    pub supers: Vec<Symbol>,
}

impl Class {
    pub fn new(supers: Vec<Symbol>) -> Self {
        Self { supers }
    }
}

#[derive(Clone, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
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
    pub classes: BTreeMap<Symbol, Class>,
    pub instances: BTreeMap<Symbol, Vec<Instance>>,
}

impl ClassEnv {
    pub fn new() -> Self {
        Self {
            classes: BTreeMap::new(),
            instances: BTreeMap::new(),
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
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    let mut defs_by_name: BTreeMap<Symbol, Vec<Type>> = BTreeMap::new();
    for typ in &types {
        typ.for_each(|t| {
            if let TypeKind::Con(tc) = t.as_ref() {
                // Builtins are not embeddable ADT declarations.
                if let Some(name) = tc.user_name() {
                    let adt = Type::new(TypeKind::Con(tc.clone()));
                    if seen.insert(adt.clone()) {
                        out.push(adt.clone());
                    }
                    let defs = defs_by_name.entry(name.clone()).or_default();
                    if !defs.contains(&adt) {
                        defs.push(adt);
                    }
                }
            }
        });
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

fn collect_adts_error_to_type(err: CollectAdtsError) -> TypeError {
    let details = err
        .conflicts
        .into_iter()
        .map(|conflict| {
            let defs = conflict
                .definitions
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            format!("{}: [{defs}]", conflict.name)
        })
        .collect::<Vec<_>>()
        .join("; ");
    TypeError::Internal(format!(
        "conflicting ADT definitions discovered in input types: {details}"
    ))
}

fn type_head_and_args_for_adt_family(typ: &Type) -> Result<(Symbol, usize, Vec<Type>), TypeError> {
    let mut args = Vec::new();
    let mut head = typ;
    while let TypeKind::App(f, arg) = head.as_ref() {
        args.push(arg.clone());
        head = f;
    }
    args.reverse();

    let TypeKind::Con(con) = head.as_ref() else {
        return Err(TypeError::Internal(format!(
            "cannot build ADT declaration from non-constructor type `{typ}`"
        )));
    };
    if !args.is_empty() && args.len() != con.arity() {
        return Err(TypeError::Internal(format!(
            "constructor `{}` expected {} type arguments but got {} in `{typ}`",
            con.name_str(),
            con.arity(),
            args.len()
        )));
    }
    Ok((con.name(), con.arity(), args))
}

fn type_head_for_adt_family(typ: &Type) -> Result<Type, TypeError> {
    let (name, arity, _args) = type_head_and_args_for_adt_family(typ)?;
    Ok(Type::con(name.as_ref(), arity))
}

pub fn adt_shape(adt: &AdtDecl) -> String {
    let param_names: BTreeMap<_, _> = adt
        .params
        .iter()
        .enumerate()
        .map(|(idx, param)| (param.var.id, format!("t{idx}")))
        .collect();
    let mut variants = adt
        .variants
        .iter()
        .map(|variant| {
            let args = variant
                .args
                .iter()
                .map(|arg| normalize_type_for_shape(arg, &param_names))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{}({args})", variant.name)
        })
        .collect::<Vec<_>>();
    variants.sort();
    format!("{}[{}]", adt.name, variants.join(" | "))
}

fn normalize_type_for_shape(typ: &Type, param_names: &BTreeMap<usize, String>) -> String {
    match typ.as_ref() {
        TypeKind::Var(tv) => param_names
            .get(&tv.id)
            .cloned()
            .unwrap_or_else(|| format!("v{}", tv.id)),
        TypeKind::Con(con) => con.name_str().to_string(),
        TypeKind::App(fun, arg) => format!(
            "({} {})",
            normalize_type_for_shape(fun, param_names),
            normalize_type_for_shape(arg, param_names)
        ),
        TypeKind::Fun(arg, ret) => format!(
            "({} -> {})",
            normalize_type_for_shape(arg, param_names),
            normalize_type_for_shape(ret, param_names)
        ),
        TypeKind::Tuple(elems) => format!(
            "({})",
            elems
                .iter()
                .map(|elem| normalize_type_for_shape(elem, param_names))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        TypeKind::Record(fields) => format!(
            "{{{}}}",
            fields
                .iter()
                .map(|(name, typ)| format!(
                    "{name}: {}",
                    normalize_type_for_shape(typ, param_names)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

pub fn adt_shape_eq(left: &AdtDecl, right: &AdtDecl) -> bool {
    adt_shape(left) == adt_shape(right)
}

fn adt_direct_dependencies(adt: &AdtDecl) -> Result<Vec<Type>, TypeError> {
    let types = adt
        .variants
        .iter()
        .flat_map(|variant| variant.args.iter().cloned())
        .collect::<Vec<_>>();
    let deps = collect_adts_in_types(types).map_err(collect_adts_error_to_type)?;
    deps.into_iter()
        .map(|typ| type_head_for_adt_family(&typ))
        .collect()
}

/// Order a family of algebraic data type declarations for registration.
///
/// An ADT family is the root ADT an embedder wants to expose plus the
/// user-defined ADTs that appear in its variant fields, recursively. For
/// example, if Rust type `Workflow` contains a `Step` field and `Step` contains
/// a `Resource` field, then `Workflow`, `Step`, and `Resource` form the family
/// that must be registered together.
///
/// This function deduplicates identical declarations, rejects conflicting
/// declarations for the same ADT name, rejects dependency cycles, and returns
/// the declarations in dependency order so nested ADTs are registered before
/// the ADTs that refer to them.
pub fn order_adt_family(adts: Vec<AdtDecl>) -> Result<Vec<AdtDecl>, TypeError> {
    let mut unique = BTreeMap::new();
    for adt in adts {
        match unique.get(&adt.name) {
            Some(existing) if adt_shape_eq(existing, &adt) => {}
            Some(existing) => {
                return Err(TypeError::Internal(format!(
                    "conflicting ADT family definitions for `{}`: {} vs {}",
                    adt.name,
                    adt_shape(existing),
                    adt_shape(&adt)
                )));
            }
            None => {
                unique.insert(adt.name.clone(), adt);
            }
        }
    }

    let mut visiting = Vec::<Symbol>::new();
    let mut visited = BTreeSet::<Symbol>::new();
    let mut ordered = Vec::<AdtDecl>::new();

    fn visit(
        name: &Symbol,
        unique: &BTreeMap<Symbol, AdtDecl>,
        visiting: &mut Vec<Symbol>,
        visited: &mut BTreeSet<Symbol>,
        ordered: &mut Vec<AdtDecl>,
    ) -> Result<(), TypeError> {
        if visited.contains(name) {
            return Ok(());
        }
        if let Some(idx) = visiting.iter().position(|current| current == name) {
            let mut cycle = visiting[idx..]
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>();
            cycle.push(name.to_string());
            return Err(TypeError::Internal(format!(
                "cyclic ADT auto-registration is not supported yet: {}",
                cycle.join(" -> ")
            )));
        }

        let adt = unique
            .get(name)
            .ok_or_else(|| TypeError::Internal(format!("missing ADT `{name}` during ordering")))?;
        visiting.push(name.clone());
        for dep in adt_direct_dependencies(adt)? {
            let dep_head = type_head_for_adt_family(&dep)?;
            let TypeKind::Con(dep_con) = dep_head.as_ref() else {
                return Err(TypeError::Internal(format!(
                    "dependency head for `{name}` was not a constructor"
                )));
            };
            if let Some(name) = dep_con.user_name()
                && unique.contains_key(name)
            {
                visit(name, unique, visiting, visited, ordered)?;
            }
        }
        visiting.pop();
        visited.insert(name.clone());
        ordered.push(adt.clone());
        Ok(())
    }

    let mut names = unique.keys().cloned().collect::<Vec<_>>();
    names.sort();
    for name in names {
        visit(&name, &unique, &mut visiting, &mut visited, &mut ordered)?;
    }
    Ok(ordered)
}

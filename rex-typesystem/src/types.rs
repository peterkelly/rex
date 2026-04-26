use crate::{
    error::{AdtConflict, CollectAdtsError},
    typesystem::TypeVarSupply,
    unification::{Subst, subst_is_empty},
};
use chrono::{DateTime, Utc};
use rex_ast::expr::{Pattern, Symbol, intern, sym};
use rpds::HashTrieMapSync;
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::{self, Display, Formatter},
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

#[derive(Clone, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub struct TypeConst {
    pub name: Symbol,
    pub arity: usize,
    pub builtin_id: Option<BuiltinTypeId>,
}

#[derive(Clone, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub struct Type(Arc<TypeKind>);

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

#[derive(Clone, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
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

#[derive(Clone, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
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

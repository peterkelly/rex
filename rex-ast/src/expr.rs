use std::{
    borrow::Borrow,
    collections::{BTreeMap, HashMap},
    fmt::{self, Display, Formatter},
    ops::Deref,
    sync::{Arc, Mutex, OnceLock},
};

use rex_lexer::span::{Position, Span};
use rpds::HashTrieMapSync;

use chrono::{DateTime, Utc};
use uuid::Uuid;

#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Deserialize, serde::Serialize,
)]
#[serde(transparent)]
pub struct Symbol(Arc<str>);

impl Symbol {
    pub fn as_str(&self) -> &str {
        self.0.as_ref()
    }
}

impl Deref for Symbol {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl AsRef<str> for Symbol {
    fn as_ref(&self) -> &str {
        self.0.as_ref()
    }
}

impl Borrow<str> for Symbol {
    fn borrow(&self) -> &str {
        self.0.as_ref()
    }
}

impl Display for Symbol {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl From<&str> for Symbol {
    fn from(value: &str) -> Self {
        Symbol(Arc::from(value))
    }
}

impl From<String> for Symbol {
    fn from(value: String) -> Self {
        Symbol(Arc::from(value))
    }
}

impl From<Arc<str>> for Symbol {
    fn from(value: Arc<str>) -> Self {
        Symbol(value)
    }
}

pub type Scope = HashTrieMapSync<Symbol, Arc<Expr>>;

// Global symbol interner.
//
// Design constraints:
// - Symbols wrap `Arc<str>` so cloning them is cheap and comparisons are fast.
// - The table is process-global and monotonically grows; that's fine for a
//   typical “compile a program, then exit” workflow.
// - Locking makes the cost model explicit (and obvious in profiles). If this
//   ever shows up hot, the first step is usually “reduce calls to `intern`”,
//   not “invent a clever interner”.
static INTERNER: OnceLock<Mutex<HashMap<String, Symbol>>> = OnceLock::new();

pub fn intern(name: &str) -> Symbol {
    let mutex = INTERNER.get_or_init(|| Mutex::new(HashMap::new()));
    let mut table = match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some(existing) = table.get(name) {
        return existing.clone();
    }
    let sym = Symbol(Arc::from(name));
    table.insert(name.to_string(), sym.clone());
    sym
}

pub fn sym(name: &str) -> Symbol {
    intern(name)
}

pub fn sym_eq(name: &Symbol, expected: &str) -> bool {
    name.as_ref() == expected
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub struct Var {
    pub span: Span,
    pub name: Symbol,
}

impl Var {
    pub fn new(name: impl ToString) -> Self {
        Self {
            span: Span::default(),
            name: intern(&name.to_string()),
        }
    }

    pub fn with_span(span: Span, name: impl ToString) -> Self {
        Self {
            span,
            name: intern(&name.to_string()),
        }
    }

    pub fn reset_spans(&self) -> Var {
        Var {
            span: Span::default(),
            name: self.name.clone(),
        }
    }
}

impl Display for Var {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.name.as_ref() {
            "+" | "-" | "*" | "/" | "==" | ">=" | ">" | "<=" | "<" | "++" | "." => {
                '('.fmt(f)?;
                self.name.fmt(f)?;
                ')'.fmt(f)
            }
            _ => self.name.fmt(f),
        }
    }
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Pattern {
    Wildcard(Span),                         // _
    Var(Var),                               // x
    Named(Span, Symbol, Vec<Pattern>),      // Ok x y z
    Tuple(Span, Vec<Pattern>),              // (x, y, z)
    List(Span, Vec<Pattern>),               // [x, y, z]
    Cons(Span, Box<Pattern>, Box<Pattern>), // x:xs
    Dict(Span, Vec<(Symbol, Pattern)>),     // {a, b, c} or {a: x, b: y}
}

impl Pattern {
    pub fn span(&self) -> &Span {
        match self {
            Pattern::Wildcard(span, ..)
            | Pattern::Var(Var { span, .. })
            | Pattern::Named(span, ..)
            | Pattern::Tuple(span, ..)
            | Pattern::List(span, ..)
            | Pattern::Cons(span, ..)
            | Pattern::Dict(span, ..) => span,
        }
    }

    pub fn with_span(&self, span: Span) -> Pattern {
        match self {
            Pattern::Wildcard(..) => Pattern::Wildcard(span),
            Pattern::Var(var) => Pattern::Var(Var {
                span,
                name: var.name.clone(),
            }),
            Pattern::Named(_, name, ps) => Pattern::Named(span, name.clone(), ps.clone()),
            Pattern::Tuple(_, ps) => Pattern::Tuple(span, ps.clone()),
            Pattern::List(_, ps) => Pattern::List(span, ps.clone()),
            Pattern::Cons(_, head, tail) => Pattern::Cons(span, head.clone(), tail.clone()),
            Pattern::Dict(_, fields) => Pattern::Dict(span, fields.clone()),
        }
    }

    pub fn reset_spans(&self) -> Pattern {
        match self {
            Pattern::Wildcard(..) => Pattern::Wildcard(Span::default()),
            Pattern::Var(var) => Pattern::Var(var.reset_spans()),
            Pattern::Named(_, name, ps) => Pattern::Named(
                Span::default(),
                name.clone(),
                ps.iter().map(|p| p.reset_spans()).collect(),
            ),
            Pattern::Tuple(_, ps) => Pattern::Tuple(
                Span::default(),
                ps.iter().map(|p| p.reset_spans()).collect(),
            ),
            Pattern::List(_, ps) => Pattern::List(
                Span::default(),
                ps.iter().map(|p| p.reset_spans()).collect(),
            ),
            Pattern::Cons(_, head, tail) => Pattern::Cons(
                Span::default(),
                Box::new(head.reset_spans()),
                Box::new(tail.reset_spans()),
            ),
            Pattern::Dict(_, fields) => Pattern::Dict(
                Span::default(),
                fields
                    .iter()
                    .map(|(k, p)| (k.clone(), p.reset_spans()))
                    .collect(),
            ),
        }
    }
}

impl Display for Pattern {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Pattern::Wildcard(..) => write!(f, "_"),
            Pattern::Var(var) => var.fmt(f),
            Pattern::Named(_, name, ps) => {
                write!(f, "{}", name)?;
                for p in ps {
                    write!(f, " {}", p)?;
                }
                Ok(())
            }
            Pattern::Tuple(_, ps) => {
                '('.fmt(f)?;
                for (i, p) in ps.iter().enumerate() {
                    p.fmt(f)?;
                    if i + 1 < ps.len() {
                        ", ".fmt(f)?;
                    }
                }
                ')'.fmt(f)
            }
            Pattern::List(_, ps) => {
                '['.fmt(f)?;
                for (i, p) in ps.iter().enumerate() {
                    p.fmt(f)?;
                    if i + 1 < ps.len() {
                        ", ".fmt(f)?;
                    }
                }
                ']'.fmt(f)
            }
            Pattern::Cons(_, head, tail) => write!(f, "{}:{}", head, tail),
            Pattern::Dict(_, fields) => {
                '{'.fmt(f)?;
                for (i, (key, pat)) in fields.iter().enumerate() {
                    // Use shorthand when possible to keep output stable with old syntax.
                    match pat {
                        Pattern::Var(var) if var.name == *key => {
                            key.fmt(f)?;
                        }
                        _ => {
                            key.fmt(f)?;
                            ": ".fmt(f)?;
                            pat.fmt(f)?;
                        }
                    }
                    if i + 1 < fields.len() {
                        ", ".fmt(f)?;
                    }
                }
                '}'.fmt(f)
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TypeExpr {
    Name(Span, Symbol),
    App(Span, Box<TypeExpr>, Box<TypeExpr>),
    Fun(Span, Box<TypeExpr>, Box<TypeExpr>),
    Tuple(Span, Vec<TypeExpr>),
    Record(Span, Vec<(Symbol, TypeExpr)>),
}

impl TypeExpr {
    pub fn span(&self) -> &Span {
        match self {
            TypeExpr::Name(span, ..)
            | TypeExpr::App(span, ..)
            | TypeExpr::Fun(span, ..)
            | TypeExpr::Tuple(span, ..)
            | TypeExpr::Record(span, ..) => span,
        }
    }

    pub fn reset_spans(&self) -> TypeExpr {
        match self {
            TypeExpr::Name(_, name) => TypeExpr::Name(Span::default(), name.clone()),
            TypeExpr::App(_, fun, arg) => TypeExpr::App(
                Span::default(),
                Box::new(fun.reset_spans()),
                Box::new(arg.reset_spans()),
            ),
            TypeExpr::Fun(_, arg, ret) => TypeExpr::Fun(
                Span::default(),
                Box::new(arg.reset_spans()),
                Box::new(ret.reset_spans()),
            ),
            TypeExpr::Tuple(_, elems) => TypeExpr::Tuple(
                Span::default(),
                elems.iter().map(|e| e.reset_spans()).collect(),
            ),
            TypeExpr::Record(_, fields) => TypeExpr::Record(
                Span::default(),
                fields
                    .iter()
                    .map(|(name, ty)| (name.clone(), ty.reset_spans()))
                    .collect(),
            ),
        }
    }
}

impl Display for TypeExpr {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            TypeExpr::Name(_, name) => name.fmt(f),
            TypeExpr::App(_, fun, arg) => {
                match fun.as_ref() {
                    TypeExpr::Name(..) | TypeExpr::App(..) => fun.fmt(f)?,
                    _ => {
                        '('.fmt(f)?;
                        fun.fmt(f)?;
                        ')'.fmt(f)?;
                    }
                }
                ' '.fmt(f)?;
                match arg.as_ref() {
                    TypeExpr::Name(..)
                    | TypeExpr::App(..)
                    | TypeExpr::Tuple(..)
                    | TypeExpr::Record(..) => arg.fmt(f),
                    _ => {
                        '('.fmt(f)?;
                        arg.fmt(f)?;
                        ')'.fmt(f)
                    }
                }
            }
            TypeExpr::Fun(_, arg, ret) => {
                match arg.as_ref() {
                    TypeExpr::Fun(..) => {
                        '('.fmt(f)?;
                        arg.fmt(f)?;
                        ')'.fmt(f)?;
                    }
                    _ => arg.fmt(f)?,
                }
                " -> ".fmt(f)?;
                ret.fmt(f)
            }
            TypeExpr::Tuple(_, elems) => {
                '('.fmt(f)?;
                for (i, elem) in elems.iter().enumerate() {
                    elem.fmt(f)?;
                    if i + 1 < elems.len() {
                        ", ".fmt(f)?;
                    }
                }
                ')'.fmt(f)
            }
            TypeExpr::Record(_, fields) => {
                '{'.fmt(f)?;
                for (i, (name, ty)) in fields.iter().enumerate() {
                    name.fmt(f)?;
                    ": ".fmt(f)?;
                    ty.fmt(f)?;
                    if i + 1 < fields.len() {
                        ", ".fmt(f)?;
                    }
                }
                '}'.fmt(f)
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct TypeConstraint {
    pub class: Symbol,
    pub typ: TypeExpr,
}

impl TypeConstraint {
    pub fn new(class: Symbol, typ: TypeExpr) -> Self {
        Self { class, typ }
    }
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct TypeVariant {
    pub name: Symbol,
    pub args: Vec<TypeExpr>,
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct TypeDecl {
    pub span: Span,
    pub is_pub: bool,
    pub name: Symbol,
    pub params: Vec<Symbol>,
    pub variants: Vec<TypeVariant>,
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct FnDecl {
    pub span: Span,
    pub is_pub: bool,
    pub name: Var,
    pub params: Vec<(Var, TypeExpr)>,
    pub ret: TypeExpr,
    pub constraints: Vec<TypeConstraint>,
    pub body: Arc<Expr>,
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct DeclareFnDecl {
    pub span: Span,
    pub is_pub: bool,
    pub name: Var,
    pub params: Vec<(Var, TypeExpr)>,
    pub ret: TypeExpr,
    pub constraints: Vec<TypeConstraint>,
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ClassMethodSig {
    pub name: Symbol,
    pub typ: TypeExpr,
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ClassDecl {
    pub span: Span,
    pub is_pub: bool,
    pub name: Symbol,
    pub params: Vec<Symbol>,
    pub supers: Vec<TypeConstraint>,
    pub methods: Vec<ClassMethodSig>,
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct InstanceMethodImpl {
    pub name: Symbol,
    pub body: Arc<Expr>,
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct InstanceDecl {
    pub span: Span,
    pub is_pub: bool,
    pub class: Symbol,
    pub head: TypeExpr,
    pub context: Vec<TypeConstraint>,
    pub methods: Vec<InstanceMethodImpl>,
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ImportPath {
    Local {
        segments: Vec<Symbol>,
        sha: Option<String>,
    },
    Remote {
        url: String,
        sha: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ImportItem {
    pub name: Symbol,
    pub alias: Option<Symbol>,
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ImportClause {
    All,
    Items(Vec<ImportItem>),
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ImportDecl {
    pub span: Span,
    pub is_pub: bool,
    pub path: ImportPath,
    pub alias: Symbol,
    pub clause: Option<ImportClause>,
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum Decl {
    Type(TypeDecl),
    Fn(FnDecl),
    DeclareFn(DeclareFnDecl),
    Import(ImportDecl),
    Class(ClassDecl),
    Instance(InstanceDecl),
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Program {
    pub decls: Vec<Decl>,
    pub expr: Arc<Expr>,
}

impl Program {
    /// Lower top-level `fn` declarations into nested `let` bindings around `expr`.
    ///
    /// This keeps the surface syntax (`Decl::Fn`) intact for tools, while giving
    /// the type checker and evaluator a plain expression to work with.
    pub fn expr_with_fns(&self) -> Arc<Expr> {
        let mut out = self.expr.clone();
        for decl in self.decls.iter().rev() {
            let Decl::Fn(fd) = decl else {
                continue;
            };

            let mut lam_body = fd.body.clone();
            let mut lam_end = lam_body.span().end;
            for (idx, (param, ann)) in fd.params.iter().enumerate().rev() {
                let lam_constraints = if idx == 0 {
                    fd.constraints.clone()
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

            let mut sig = fd.ret.clone();
            for (_, ann) in fd.params.iter().rev() {
                let span = Span::from_begin_end(ann.span().begin, sig.span().end);
                sig = TypeExpr::Fun(span, Box::new(ann.clone()), Box::new(sig));
            }

            let span = Span::from_begin_end(fd.span.begin, out.span().end);
            out = Arc::new(Expr::Let(span, fd.name.clone(), Some(sig), lam_body, out));
        }
        out
    }
}

#[derive(Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Expr {
    Bool(Span, bool),              // true
    Uint(Span, u64),               // 69
    Int(Span, i64),                // -420
    Float(Span, f64),              // 3.14
    String(Span, String),          // "hello"
    Uuid(Span, Uuid),              // a550c18e-36e1-4f6d-8c8e-2d2b1e5f3c3a
    DateTime(Span, DateTime<Utc>), // 2023-01-01T12:00:00Z

    Tuple(Span, Vec<Arc<Expr>>),             // (e1, e2, e3)
    List(Span, Vec<Arc<Expr>>),              // [e1, e2, e3]
    Dict(Span, BTreeMap<Symbol, Arc<Expr>>), // {k1 = v1, k2 = v2}
    RecordUpdate(Span, Arc<Expr>, BTreeMap<Symbol, Arc<Expr>>), // {base with {k1 = v1, ...}}

    Var(Var),                         // x
    App(Span, Arc<Expr>, Arc<Expr>),  // f x
    Project(Span, Arc<Expr>, Symbol), // x.field
    Lam(
        Span,
        Scope,
        Var,
        Option<TypeExpr>,
        Vec<TypeConstraint>,
        Arc<Expr>,
    ), // λx → e
    Let(Span, Var, Option<TypeExpr>, Arc<Expr>, Arc<Expr>), // let x = e1 in e2
    LetRec(Span, Vec<(Var, Option<TypeExpr>, Arc<Expr>)>, Arc<Expr>), // let rec f = e1 and g = e2 in e3
    Ite(Span, Arc<Expr>, Arc<Expr>, Arc<Expr>),                       // if e1 then e2 else e3
    Match(Span, Arc<Expr>, Vec<(Pattern, Arc<Expr>)>),                // match e1 with patterns
    Ann(Span, Arc<Expr>, TypeExpr),                                   // e is t
}

impl Expr {
    pub fn span(&self) -> &Span {
        match self {
            Self::Bool(span, ..)
            | Self::Uint(span, ..)
            | Self::Int(span, ..)
            | Self::Float(span, ..)
            | Self::String(span, ..)
            | Self::Uuid(span, ..)
            | Self::DateTime(span, ..)
            | Self::Tuple(span, ..)
            | Self::List(span, ..)
            | Self::Dict(span, ..)
            | Self::RecordUpdate(span, ..)
            | Self::Var(Var { span, .. })
            | Self::App(span, ..)
            | Self::Project(span, ..)
            | Self::Lam(span, ..)
            | Self::Let(span, ..)
            | Self::LetRec(span, ..)
            | Self::Ite(span, ..)
            | Self::Match(span, ..)
            | Self::Ann(span, ..) => span,
        }
    }

    pub fn span_mut(&mut self) -> &mut Span {
        match self {
            Self::Bool(span, ..)
            | Self::Uint(span, ..)
            | Self::Int(span, ..)
            | Self::Float(span, ..)
            | Self::String(span, ..)
            | Self::Uuid(span, ..)
            | Self::DateTime(span, ..)
            | Self::Tuple(span, ..)
            | Self::List(span, ..)
            | Self::Dict(span, ..)
            | Self::RecordUpdate(span, ..)
            | Self::Var(Var { span, .. })
            | Self::App(span, ..)
            | Self::Project(span, ..)
            | Self::Lam(span, ..)
            | Self::Let(span, ..)
            | Self::LetRec(span, ..)
            | Self::Ite(span, ..)
            | Self::Match(span, ..)
            | Self::Ann(span, ..) => span,
        }
    }

    pub fn with_span_begin_end(&self, begin: Position, end: Position) -> Expr {
        self.with_span(Span::from_begin_end(begin, end))
    }

    pub fn with_span_begin(&self, begin: Position) -> Expr {
        let end = self.span().end;
        self.with_span(Span::from_begin_end(begin, end))
    }

    pub fn with_span_end(&self, end: Position) -> Expr {
        let begin = self.span().begin;
        self.with_span(Span::from_begin_end(begin, end))
    }

    pub fn with_span(&self, span: Span) -> Expr {
        match self {
            Expr::Bool(_, x) => Expr::Bool(span, *x),
            Expr::Uint(_, x) => Expr::Uint(span, *x),
            Expr::Int(_, x) => Expr::Int(span, *x),
            Expr::Float(_, x) => Expr::Float(span, *x),
            Expr::String(_, x) => Expr::String(span, x.clone()),
            Expr::Uuid(_, x) => Expr::Uuid(span, *x),
            Expr::DateTime(_, x) => Expr::DateTime(span, *x),
            Expr::Tuple(_, elems) => Expr::Tuple(span, elems.clone()),
            Expr::List(_, elems) => Expr::List(span, elems.clone()),
            Expr::Dict(_, kvs) => Expr::Dict(
                span,
                BTreeMap::from_iter(kvs.iter().map(|(k, v)| (k.clone(), v.clone()))),
            ),
            Expr::RecordUpdate(_, base, updates) => Expr::RecordUpdate(
                span,
                base.clone(),
                BTreeMap::from_iter(updates.iter().map(|(k, v)| (k.clone(), v.clone()))),
            ),
            Expr::Var(var) => Expr::Var(Var {
                span,
                name: var.name.clone(),
            }),
            Expr::App(_, f, x) => Expr::App(span, f.clone(), x.clone()),
            Expr::Project(_, base, field) => Expr::Project(span, base.clone(), field.clone()),
            Expr::Lam(_, scope, param, ann, constraints, body) => Expr::Lam(
                span,
                scope.clone(),
                param.clone(),
                ann.clone(),
                constraints.clone(),
                body.clone(),
            ),
            Expr::Let(_, var, ann, def, body) => {
                Expr::Let(span, var.clone(), ann.clone(), def.clone(), body.clone())
            }
            Expr::LetRec(_, bindings, body) => Expr::LetRec(
                span,
                bindings
                    .iter()
                    .map(|(var, ann, def)| (var.clone(), ann.clone(), def.clone()))
                    .collect(),
                body.clone(),
            ),
            Expr::Ite(_, cond, then, r#else) => {
                Expr::Ite(span, cond.clone(), then.clone(), r#else.clone())
            }
            Expr::Match(_, scrutinee, arms) => Expr::Match(
                span,
                scrutinee.clone(),
                arms.iter()
                    .map(|(pat, expr)| (pat.clone(), expr.clone()))
                    .collect(),
            ),
            Expr::Ann(_, expr, ann) => Expr::Ann(span, expr.clone(), ann.clone()),
        }
    }

    pub fn reset_spans(&self) -> Expr {
        match self {
            Expr::Bool(_, x) => Expr::Bool(Span::default(), *x),
            Expr::Uint(_, x) => Expr::Uint(Span::default(), *x),
            Expr::Int(_, x) => Expr::Int(Span::default(), *x),
            Expr::Float(_, x) => Expr::Float(Span::default(), *x),
            Expr::String(_, x) => Expr::String(Span::default(), x.clone()),
            Expr::Uuid(_, x) => Expr::Uuid(Span::default(), *x),
            Expr::DateTime(_, x) => Expr::DateTime(Span::default(), *x),
            Expr::Tuple(_, elems) => Expr::Tuple(
                Span::default(),
                elems.iter().map(|x| Arc::new(x.reset_spans())).collect(),
            ),
            Expr::List(_, elems) => Expr::List(
                Span::default(),
                elems.iter().map(|x| Arc::new(x.reset_spans())).collect(),
            ),
            Expr::Dict(_, kvs) => Expr::Dict(
                Span::default(),
                BTreeMap::from_iter(
                    kvs.iter()
                        .map(|(k, v)| (k.clone(), Arc::new(v.reset_spans()))),
                ),
            ),
            Expr::RecordUpdate(_, base, updates) => Expr::RecordUpdate(
                Span::default(),
                Arc::new(base.reset_spans()),
                BTreeMap::from_iter(
                    updates
                        .iter()
                        .map(|(k, v)| (k.clone(), Arc::new(v.reset_spans()))),
                ),
            ),
            Expr::Var(var) => Expr::Var(var.reset_spans()),
            Expr::App(_, f, x) => Expr::App(
                Span::default(),
                Arc::new(f.reset_spans()),
                Arc::new(x.reset_spans()),
            ),
            Expr::Project(_, base, field) => {
                Expr::Project(Span::default(), Arc::new(base.reset_spans()), field.clone())
            }
            Expr::Lam(_, scope, param, ann, constraints, body) => Expr::Lam(
                Span::default(),
                scope.clone(),
                param.reset_spans(),
                ann.as_ref().map(TypeExpr::reset_spans),
                constraints
                    .iter()
                    .map(|constraint| TypeConstraint {
                        class: constraint.class.clone(),
                        typ: constraint.typ.reset_spans(),
                    })
                    .collect(),
                Arc::new(body.reset_spans()),
            ),
            Expr::Let(_, var, ann, def, body) => Expr::Let(
                Span::default(),
                var.reset_spans(),
                ann.as_ref().map(|t| t.reset_spans()),
                Arc::new(def.reset_spans()),
                Arc::new(body.reset_spans()),
            ),
            Expr::LetRec(_, bindings, body) => Expr::LetRec(
                Span::default(),
                bindings
                    .iter()
                    .map(|(var, ann, def)| {
                        (
                            var.reset_spans(),
                            ann.as_ref().map(TypeExpr::reset_spans),
                            Arc::new(def.reset_spans()),
                        )
                    })
                    .collect(),
                Arc::new(body.reset_spans()),
            ),
            Expr::Ite(_, cond, then, r#else) => Expr::Ite(
                Span::default(),
                Arc::new(cond.reset_spans()),
                Arc::new(then.reset_spans()),
                Arc::new(r#else.reset_spans()),
            ),
            Expr::Match(_, scrutinee, arms) => Expr::Match(
                Span::default(),
                Arc::new(scrutinee.reset_spans()),
                arms.iter()
                    .map(|(pat, expr)| (pat.reset_spans(), Arc::new(expr.reset_spans())))
                    .collect(),
            ),
            Expr::Ann(_, expr, ann) => Expr::Ann(
                Span::default(),
                Arc::new(expr.reset_spans()),
                ann.reset_spans(),
            ),
        }
    }
}

impl Display for Expr {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bool(_span, x) => x.fmt(f),
            Self::Uint(_span, x) => x.fmt(f),
            Self::Int(_span, x) => x.fmt(f),
            Self::Float(_span, x) => x.fmt(f),
            Self::String(_span, x) => write!(f, "{:?}", x),
            Self::Uuid(_span, x) => x.fmt(f),
            Self::DateTime(_span, x) => x.fmt(f),
            Self::List(_span, xs) => {
                '['.fmt(f)?;
                for (i, x) in xs.iter().enumerate() {
                    x.fmt(f)?;
                    if i + 1 < xs.len() {
                        ", ".fmt(f)?;
                    }
                }
                ']'.fmt(f)
            }
            Self::Tuple(_span, xs) => {
                '('.fmt(f)?;
                for (i, x) in xs.iter().enumerate() {
                    x.fmt(f)?;
                    if i + 1 < xs.len() {
                        ", ".fmt(f)?;
                    }
                }
                ')'.fmt(f)
            }
            Self::Dict(_span, kvs) => {
                '{'.fmt(f)?;
                for (i, (k, v)) in kvs.iter().enumerate() {
                    k.fmt(f)?;
                    " = ".fmt(f)?;
                    v.fmt(f)?;
                    if i + 1 < kvs.len() {
                        ", ".fmt(f)?;
                    }
                }
                '}'.fmt(f)
            }
            Self::RecordUpdate(_span, base, kvs) => {
                '{'.fmt(f)?;
                base.fmt(f)?;
                " with ".fmt(f)?;
                '{'.fmt(f)?;
                for (i, (k, v)) in kvs.iter().enumerate() {
                    k.fmt(f)?;
                    " = ".fmt(f)?;
                    v.fmt(f)?;
                    if i + 1 < kvs.len() {
                        ", ".fmt(f)?;
                    }
                }
                '}'.fmt(f)?;
                '}'.fmt(f)
            }
            Self::Var(var) => var.fmt(f),
            Self::App(_span, g, x) => {
                g.fmt(f)?;
                ' '.fmt(f)?;
                match x.as_ref() {
                    Self::Bool(..)
                    | Self::Uint(..)
                    | Self::Int(..)
                    | Self::Float(..)
                    | Self::String(..)
                    | Self::List(..)
                    | Self::Tuple(..)
                    | Self::Dict(..)
                    | Self::RecordUpdate(..)
                    | Self::Project(..)
                    | Self::Var(..) => x.fmt(f),
                    _ => {
                        '('.fmt(f)?;
                        x.fmt(f)?;
                        ')'.fmt(f)
                    }
                }
            }
            Self::Lam(_span, _scope, param, ann, constraints, body) => {
                'λ'.fmt(f)?;
                if let Some(ann) = ann {
                    '('.fmt(f)?;
                    param.fmt(f)?;
                    " : ".fmt(f)?;
                    ann.fmt(f)?;
                    ')'.fmt(f)?;
                } else {
                    param.fmt(f)?;
                }
                if !constraints.is_empty() {
                    " where ".fmt(f)?;
                    for (i, constraint) in constraints.iter().enumerate() {
                        constraint.class.fmt(f)?;
                        ' '.fmt(f)?;
                        constraint.typ.fmt(f)?;
                        if i + 1 < constraints.len() {
                            ", ".fmt(f)?;
                        }
                    }
                }
                " → ".fmt(f)?;
                body.fmt(f)
            }
            Self::Let(_span, var, ann, def, body) => {
                "let ".fmt(f)?;
                var.fmt(f)?;
                if let Some(ann) = ann {
                    ": ".fmt(f)?;
                    ann.fmt(f)?;
                }
                " = ".fmt(f)?;
                def.fmt(f)?;
                " in ".fmt(f)?;
                body.fmt(f)
            }
            Self::LetRec(_span, bindings, body) => {
                "let rec ".fmt(f)?;
                for (idx, (var, ann, def)) in bindings.iter().enumerate() {
                    if idx > 0 {
                        " and ".fmt(f)?;
                    }
                    var.fmt(f)?;
                    if let Some(ann) = ann {
                        ": ".fmt(f)?;
                        ann.fmt(f)?;
                    }
                    " = ".fmt(f)?;
                    def.fmt(f)?;
                }
                " in ".fmt(f)?;
                body.fmt(f)
            }
            Self::Ite(_span, cond, then, r#else) => {
                "if ".fmt(f)?;
                cond.fmt(f)?;
                " then ".fmt(f)?;
                then.fmt(f)?;
                " else ".fmt(f)?;
                r#else.fmt(f)
            }
            Self::Match(_span, scrutinee, arms) => {
                "match ".fmt(f)?;
                scrutinee.fmt(f)?;
                ' '.fmt(f)?;
                for (i, (pat, expr)) in arms.iter().enumerate() {
                    "when ".fmt(f)?;
                    pat.fmt(f)?;
                    " -> ".fmt(f)?;
                    expr.fmt(f)?;
                    if i + 1 < arms.len() {
                        ' '.fmt(f)?;
                    }
                }
                Ok(())
            }
            Self::Project(_span, base, field) => {
                base.fmt(f)?;
                ".".fmt(f)?;
                field.fmt(f)
            }
            Self::Ann(_span, expr, ann) => {
                expr.fmt(f)?;
                " is ".fmt(f)?;
                ann.fmt(f)
            }
        }
    }
}

use std::{
    collections::BTreeMap,
    fmt::{self, Display, Formatter},
    sync::Arc,
};

use rex_lexer::span::{Position, Span};
use rpds::HashTrieMapSync;

use chrono::{DateTime, Utc};
use uuid::Uuid;

pub type Scope = HashTrieMapSync<String, Arc<Expr>>;

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub struct Var {
    pub span: Span,
    pub name: String,
}

impl Var {
    pub fn new(name: impl ToString) -> Self {
        Self {
            span: Span::default(),
            name: name.to_string(),
        }
    }

    pub fn with_span(span: Span, name: impl ToString) -> Self {
        Self {
            span,
            name: name.to_string(),
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
    Wildcard(Span),                             // _
    Var(Var),                                   // x
    Named(Span, String, Vec<Pattern>),          // Ok x y z
    List(Span, Vec<Pattern>),                   // [x, y, z]
    Cons(Span, Box<Pattern>, Box<Pattern>),     // x:xs
    Dict(Span, Vec<String>),                    // {a, b, c}
}

impl Pattern {
    pub fn span(&self) -> &Span {
        match self {
            Pattern::Wildcard(span, ..)
            | Pattern::Var(Var { span, .. })
            | Pattern::Named(span, ..)
            | Pattern::List(span, ..)
            | Pattern::Cons(span, ..)
            | Pattern::Dict(span, ..) => span,
        }
    }

    pub fn with_span(&self, span: Span) -> Pattern {
        match self {
            Pattern::Wildcard(..) => Pattern::Wildcard(span),
            Pattern::Var(var) => Pattern::Var(Var::with_span(span, var.name.clone())),
            Pattern::Named(_, name, ps) => Pattern::Named(span, name.clone(), ps.clone()),
            Pattern::List(_, ps) => Pattern::List(span, ps.clone()),
            Pattern::Cons(_, head, tail) => Pattern::Cons(span, head.clone(), tail.clone()),
            Pattern::Dict(_, keys) => Pattern::Dict(span, keys.clone()),
        }
    }

    pub fn reset_spans(&self) -> Pattern {
        match self {
            Pattern::Wildcard(..) => Pattern::Wildcard(Span::default()),
            Pattern::Var(var) => Pattern::Var(var.reset_spans()),
            Pattern::Named(_, name, ps) => {
                Pattern::Named(Span::default(), name.clone(), ps.iter().map(|p| p.reset_spans()).collect())
            }
            Pattern::List(_, ps) => {
                Pattern::List(Span::default(), ps.iter().map(|p| p.reset_spans()).collect())
            }
            Pattern::Cons(_, head, tail) => Pattern::Cons(
                Span::default(),
                Box::new(head.reset_spans()),
                Box::new(tail.reset_spans()),
            ),
            Pattern::Dict(_, keys) => Pattern::Dict(Span::default(), keys.clone()),
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
            Pattern::Dict(_, keys) => {
                '{'.fmt(f)?;
                for (i, key) in keys.iter().enumerate() {
                    key.fmt(f)?;
                    if i + 1 < keys.len() {
                        ", ".fmt(f)?;
                    }
                }
                '}'.fmt(f)
            }
        }
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
    Dict(Span, BTreeMap<String, Arc<Expr>>), // {k1 = v1, k2 = v2}

    Var(Var),                                   // x
    App(Span, Arc<Expr>, Arc<Expr>),            // f x
    Lam(Span, Scope, Var, Arc<Expr>),           // λx → e
    Let(Span, Var, Arc<Expr>, Arc<Expr>),       // let x = e1 in e2
    Ite(Span, Arc<Expr>, Arc<Expr>, Arc<Expr>), // if e1 then e2 else e3
    Match(Span, Arc<Expr>, Vec<(Pattern, Arc<Expr>)>), // match e1 with patterns
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
            | Self::Var(Var { span, .. })
            | Self::App(span, ..)
            | Self::Lam(span, ..)
            | Self::Let(span, ..)
            | Self::Ite(span, ..)
            | Self::Match(span, ..) => span,
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
            | Self::Var(Var { span, .. })
            | Self::App(span, ..)
            | Self::Lam(span, ..)
            | Self::Let(span, ..)
            | Self::Ite(span, ..)
            | Self::Match(span, ..) => span,
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
            Expr::Var(var) => Expr::Var(Var::with_span(span, &var.name)),
            Expr::App(_, f, x) => Expr::App(span, f.clone(), x.clone()),
            Expr::Lam(_, scope, param, body) => {
                Expr::Lam(span, scope.clone(), param.clone(), body.clone())
            }
            Expr::Let(_, var, def, body) => Expr::Let(span, var.clone(), def.clone(), body.clone()),
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
            Expr::Var(var) => Expr::Var(var.reset_spans()),
            Expr::App(_, f, x) => Expr::App(
                Span::default(),
                Arc::new(f.reset_spans()),
                Arc::new(x.reset_spans()),
            ),
            Expr::Lam(_, scope, param, body) => Expr::Lam(
                Span::default(),
                scope.clone(),
                param.reset_spans(),
                Arc::new(body.reset_spans()),
            ),
            Expr::Let(_, var, def, body) => Expr::Let(
                Span::default(),
                var.reset_spans(),
                Arc::new(def.reset_spans()),
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
                    | Self::Var(..) => x.fmt(f),
                    _ => {
                        '('.fmt(f)?;
                        x.fmt(f)?;
                        ')'.fmt(f)
                    }
                }
            }
            Self::Lam(_span, _scope, param, body) => {
                'λ'.fmt(f)?;
                param.fmt(f)?;
                " → ".fmt(f)?;
                body.fmt(f)
            }
            Self::Let(_span, var, def, body) => {
                "let ".fmt(f)?;
                var.fmt(f)?;
                " = ".fmt(f)?;
                def.fmt(f)?;
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
        }
    }
}

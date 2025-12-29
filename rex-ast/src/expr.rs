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
    Named(Span, String, Option<Arc<Expr>>),  //  MyVariant1 {k1 = v1, k2 = v2}

    Var(Var),                                   // x
    App(Span, Arc<Expr>, Arc<Expr>),            // f x
    Lam(Span, Scope, Var, Arc<Expr>),           // λx → e
    Let(Span, Var, Arc<Expr>, Arc<Expr>),       // let x = e1 in e2
    Ite(Span, Arc<Expr>, Arc<Expr>, Arc<Expr>), // if e1 then e2 else e3
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
            | Self::Named(span, ..)
            | Self::Var(Var { span, .. })
            | Self::App(span, ..)
            | Self::Lam(span, ..)
            | Self::Let(span, ..)
            | Self::Ite(span, ..) => span,
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
            | Self::Named(span, ..)
            | Self::Var(Var { span, .. })
            | Self::App(span, ..)
            | Self::Lam(span, ..)
            | Self::Let(span, ..)
            | Self::Ite(span, ..) => span,
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
            Expr::Named(_, name, inner) => Expr::Named(span, name.clone(), inner.as_ref().cloned()),
            Expr::Var(var) => Expr::Var(Var::with_span(span, &var.name)),
            Expr::App(_, f, x) => Expr::App(span, f.clone(), x.clone()),
            Expr::Lam(_, scope, param, body) => {
                Expr::Lam(span, scope.clone(), param.clone(), body.clone())
            }
            Expr::Let(_, var, def, body) => Expr::Let(span, var.clone(), def.clone(), body.clone()),
            Expr::Ite(_, cond, then, r#else) => {
                Expr::Ite(span, cond.clone(), then.clone(), r#else.clone())
            }
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
            Expr::Named(_, name, inner) => Expr::Named(
                Span::default(),
                name.clone(),
                inner.as_ref().map(|x| Arc::new(x.reset_spans())),
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
            Self::Named(_span, name, inner) => {
                name.fmt(f)?;
                if let Some(inner) = inner {
                    '('.fmt(f)?;
                    inner.fmt(f)?;
                    ')'.fmt(f)?;
                }
                Ok(())
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
        }
    }
}

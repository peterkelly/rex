#[macro_export]
macro_rules! assert_expr_eq {
    ($lhs:expr, $rhs:expr) => {{
        assert_eq!($lhs, $rhs);
    }};

    ($lhs:expr, $rhs:expr; ignore span) => {{
        // override the id so we can assert equality without worrying about
        // them, because they are usually randomly generated UUIDs
        let lhs = ($lhs).reset_spans();
        let rhs = ($rhs).reset_spans();

        assert_eq!(lhs, rhs);
    }};
}

#[macro_export]
macro_rules! b {
    ($x:expr) => {
        ::std::sync::Arc::new($crate::expr::Expr::Bool(
            ::rex_lexer::span::Span::default(),
            $x,
        ))
    };

    ($span:expr; $x:expr) => {
        ::std::sync::Arc::new($crate::expr::Expr::Bool(($span).into(), $x))
    };

    ($id:expr, $span:expr; $x:expr) => {
        ::std::sync::Arc::new($crate::expr::Expr::Bool(($id).into(), ($span).into(), $x))
    };
}

#[macro_export]
macro_rules! u {
    ($x:expr) => {
        ::std::sync::Arc::new($crate::expr::Expr::Uint(
            ::rex_lexer::span::Span::default(),
            $x,
        ))
    };

    ($span:expr; $x:expr) => {
        ::std::sync::Arc::new($crate::expr::Expr::Uint(($span).into(), $x))
    };

    ($id:expr, $span:expr; $x:expr) => {
        ::std::sync::Arc::new($crate::expr::Expr::Uint(($id).into(), ($span).into(), $x))
    };
}

#[macro_export]
macro_rules! i {
    ($x:expr) => {
        ::std::sync::Arc::new($crate::expr::Expr::Int(
            ::rex_lexer::span::Span::default(),
            $x,
        ))
    };

    ($span:expr; $x:expr) => {
        ::std::sync::Arc::new($crate::expr::Expr::Int(($span).into(), $x))
    };

    ($id:expr, $span:expr; $x:expr) => {
        ::std::sync::Arc::new($crate::expr::Expr::Int(($id).into(), ($span).into(), $x))
    };
}

#[macro_export]
macro_rules! f {
    ($x:expr) => {
        ::std::sync::Arc::new($crate::expr::Expr::Float(
            ::rex_lexer::span::Span::default(),
            $x,
        ))
    };

    ($span:expr; $x:expr) => {
        ::std::sync::Arc::new($crate::expr::Expr::Float(($span).into(), $x))
    };

    ($id:expr, $span:expr; $x:expr) => {
        ::std::sync::Arc::new($crate::expr::Expr::Float(($id).into(), ($span).into(), $x))
    };
}

#[macro_export]
macro_rules! s {
    ($x:expr) => {
        ::std::sync::Arc::new($crate::expr::Expr::String(
            ::rex_lexer::span::Span::default(),
            $x.to_string(),
        ))
    };

    ($span:expr; $x:expr) => {
        ::std::sync::Arc::new($crate::expr::Expr::String(($span).into(), ($x).to_string()))
    };

    ($id:expr, $span:expr; $x:expr) => {
        ::std::sync::Arc::new($crate::expr::Expr::String(
            ($id).into(),
            ($span).into(),
            ($x).to_string(),
        ))
    };
}

#[macro_export]
macro_rules! tup {
    ($($xs:expr),* $(,)?) => {
        ::std::sync::Arc::new($crate::expr::Expr::Tuple(::rex_lexer::span::Span::default(), vec![$($xs),*]))
    };

    ($span:expr; $($xs:expr),* $(,)?) => {
        ::std::sync::Arc::new($crate::expr::Expr::Tuple(($span).into(), vec![$($xs),*]))
    };

    ($id:expr, $span:expr; $($xs:expr),* $(,)?) => {
        ::std::sync::Arc::new($crate::expr::Expr::Tuple(($id).into(), ($span).into(), vec![$($xs),*]))
    };
}

#[macro_export]
macro_rules! l {
    ($($xs:expr),* $(,)?) => {
        ::std::sync::Arc::new($crate::expr::Expr::List(::rex_lexer::span::Span::default(), vec![$($xs),*]))
    };

    ($span:expr; $($xs:expr),* $(,)?) => {
        ::std::sync::Arc::new($crate::expr::Expr::List(($span).into(), vec![$($xs),*]))
    };

    ($id:expr, $span:expr; $($xs:expr),* $(,)?) => {
        ::std::sync::Arc::new($crate::expr::Expr::List(($id).into(), ($span).into(), vec![$($xs),*]))
    };
}

#[macro_export]
macro_rules! d {
    ($($k:ident = $v:expr),* $(,)?) => {
        ::std::sync::Arc::new($crate::expr::Expr::Dict(::rex_lexer::span::Span::default(), {
            let mut map = ::std::collections::BTreeMap::new();
            $(map.insert(stringify!($k).to_string(), $v);)*
            map
        }))
    };

    ($span:expr; $($k:ident = $v:expr),* $(,)?) => {
        ::std::sync::Arc::new($crate::expr::Expr::Dict(($span).into(), {
            let mut map = ::std::collections::BTreeMap::new();
            $(map.insert(stringify!($k).to_string(), $v);)*
            map
        }))
    };

    ($id:expr, $span:expr; $($k:ident = $v:expr),* $(,)?) => {
        ::std::sync::Arc::new($crate::expr::Expr::Dict(($id).into(), ($span).into(), {
            let mut map = ::std::collections::BTreeMap::new();
            $(map.insert(stringify!($k).to_string(), $v);)*
            map
        }))
    };
}

#[macro_export]
macro_rules! n {
    ($n: expr, $x:expr) => {
        ::std::sync::Arc::new($crate::expr::Expr::Named(
            ::rex_lexer::span::Span::default(),
            ($n).to_string(),
            ($x).map(|x| x),
        ))
    };
    ($span:expr; $n: expr, $x:expr) => {
        ::std::sync::Arc::new($crate::expr::Expr::Named(
            ($span).into(),
            ($n).to_string(),
            ($x),
        ))
    };
    ($id:expr, $span:expr; $n: expr, $x:expr) => {
        ::std::sync::Arc::new($crate::expr::Expr::Named(
            ($id).into(),
            ($span).into(),
            ($n).to_string(),
            ($x),
        ))
    };
}

#[macro_export]
macro_rules! v {
    ($x:expr) => {
        ::std::sync::Arc::new($crate::expr::Expr::Var($crate::expr::Var {
            span: ::rex_lexer::span::Span::default(),
            name: ($x).to_string(),
        }))
    };

    ($span:expr; $x:expr) => {
        ::std::sync::Arc::new($crate::expr::Expr::Var($crate::expr::Var {
            span: ($span).into(),
            name: ($x).to_string(),
        }))
    };

    ($id:expr, $span:expr; $x:expr) => {
        ::std::sync::Arc::new($crate::expr::Expr::Var($crate::expr::Var {
            id: ($id).into(),
            span: ($span).into(),
            name: ($x).to_string(),
        }))
    };
}

#[macro_export]
macro_rules! app {
    ($f:expr, $x:expr) => {
        ::std::sync::Arc::new($crate::expr::Expr::App(
            ::rex_lexer::span::Span::default(),
            ($f).into(),
            ($x).into(),
        ))
    };

    ($span:expr; $f:expr, $x:expr) => {
        ::std::sync::Arc::new($crate::expr::Expr::App(
            ($span).into(),
            ($f).into(),
            ($x).into(),
        ))
    };

    ($id:expr, $span:expr; $f:expr, $x:expr) => {
        ::std::sync::Arc::new($crate::expr::Expr::App(
            ($id).into(),
            ($span).into(),
            ($f).into(),
            ($x).into(),
        ))
    };
}

#[macro_export]
macro_rules! lam {
    (λ $x:ident -> $e:expr) => {
        ::std::sync::Arc::new($crate::expr::Expr::Lam(
            ::rex_lexer::span::Span::default(),
            $crate::expr::Scope::new_sync(),
            ($x).into(),
            ($e).into(),
        ))
    };

    ($span:expr; λ $x:ident -> $e:expr) => {
        ::std::sync::Arc::new($crate::expr::Expr::Lam(
            ($span).into(),
            $crate::expr::Scope::new_sync(),
            ($x).into(),
            ($e).into(),
        ))
    };

    ($id:expr, $span:expr; λ $x:ident -> $e:expr) => {
        ::std::sync::Arc::new($crate::expr::Expr::Lam(
            ($id).into(),
            ($span).into(),
            $crate::expr::Scope::new_sync(),
            ($x).into(),
            ($e).into(),
        ))
    };
}

#[macro_export]
macro_rules! let_in {
    (let $x:ident = ($e1:expr) in $e2:expr) => {
        ::std::sync::Arc::new($crate::expr::Expr::Let(
            ::rex_lexer::span::Span::default(),
            ($x).into(),
            ($e1).into(),
            ($e2).into(),
        ))
    };

    ($span:expr; let $x:ident = ($e1:expr) in $e2:expr) => {
        ::std::sync::Arc::new($crate::expr::Expr::Let(
            ($span).into(),
            ($x).into(),
            ($e1).into(),
            ($e2).into(),
        ))
    };

    ($id:expr, $span:expr; let $x:ident = ($e1:expr) in $e2:expr) => {
        ::std::sync::Arc::new($crate::expr::Expr::Let(
            ($id).into(),
            ($span).into(),
            ($x).into(),
            ($e1).into(),
            ($e2).into(),
        ))
    };
}

#[macro_export]
macro_rules! ite {
    (if ($e1:expr) { $e2:expr } else { $e3:expr }) => {
        ::std::sync::Arc::new($crate::expr::Expr::Ite(
            ::rex_lexer::span::Span::default(),
            ($e1).into(),
            ($e2).into(),
            ($e3).into(),
        ))
    };

    ($span:expr; if ($e1:expr) { $e2:expr } else { $e3:expr }) => {
        ::std::sync::Arc::new($crate::expr::Expr::Ite(
            ($span).into(),
            ($e1).into(),
            ($e2).into(),
            ($e3).into(),
        ))
    };

    ($id:expr, $span:expr; if ($e1:expr) { $e2:expr } else { $e3:expr }) => {
        ::std::sync::Arc::new($crate::expr::Expr::Ite(
            ($span).into(),
            ($e1).into(),
            ($e2).into(),
            ($e3).into(),
        ))
    };
}

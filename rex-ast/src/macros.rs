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
        ::std::sync::Arc::new($crate::Expr::Bool($crate::Span::default(), $x))
    };

    ($span:expr; $x:expr) => {
        ::std::sync::Arc::new($crate::Expr::Bool(($span).into(), $x))
    };

    ($id:expr, $span:expr; $x:expr) => {
        ::std::sync::Arc::new($crate::Expr::Bool(($id).into(), ($span).into(), $x))
    };
}

#[macro_export]
macro_rules! u {
    ($x:expr) => {
        ::std::sync::Arc::new($crate::Expr::Uint($crate::Span::default(), $x))
    };

    ($span:expr; $x:expr) => {
        ::std::sync::Arc::new($crate::Expr::Uint(($span).into(), $x))
    };

    ($id:expr, $span:expr; $x:expr) => {
        ::std::sync::Arc::new($crate::Expr::Uint(($id).into(), ($span).into(), $x))
    };
}

#[macro_export]
macro_rules! i {
    ($x:expr) => {
        ::std::sync::Arc::new($crate::Expr::Int($crate::Span::default(), $x))
    };

    ($span:expr; $x:expr) => {
        ::std::sync::Arc::new($crate::Expr::Int(($span).into(), $x))
    };

    ($id:expr, $span:expr; $x:expr) => {
        ::std::sync::Arc::new($crate::Expr::Int(($id).into(), ($span).into(), $x))
    };
}

#[macro_export]
macro_rules! f {
    ($x:expr) => {
        ::std::sync::Arc::new($crate::Expr::Float($crate::Span::default(), $x))
    };

    ($span:expr; $x:expr) => {
        ::std::sync::Arc::new($crate::Expr::Float(($span).into(), $x))
    };

    ($id:expr, $span:expr; $x:expr) => {
        ::std::sync::Arc::new($crate::Expr::Float(($id).into(), ($span).into(), $x))
    };
}

#[macro_export]
macro_rules! s {
    ($x:expr) => {
        ::std::sync::Arc::new($crate::Expr::String(
            $crate::Span::default(),
            $x.to_string(),
        ))
    };

    ($span:expr; $x:expr) => {
        ::std::sync::Arc::new($crate::Expr::String(($span).into(), ($x).to_string()))
    };

    ($id:expr, $span:expr; $x:expr) => {
        ::std::sync::Arc::new($crate::Expr::String(
            ($id).into(),
            ($span).into(),
            ($x).to_string(),
        ))
    };
}

#[macro_export]
macro_rules! tup {
    ($($xs:expr),* $(,)?) => {
        ::std::sync::Arc::new($crate::Expr::Tuple($crate::Span::default(), vec![$($xs),*]))
    };

    ($span:expr; $($xs:expr),* $(,)?) => {
        ::std::sync::Arc::new($crate::Expr::Tuple(($span).into(), vec![$($xs),*]))
    };

    ($id:expr, $span:expr; $($xs:expr),* $(,)?) => {
        ::std::sync::Arc::new($crate::Expr::Tuple(($id).into(), ($span).into(), vec![$($xs),*]))
    };
}

#[macro_export]
macro_rules! l {
    ($($xs:expr),* $(,)?) => {
        ::std::sync::Arc::new($crate::Expr::List($crate::Span::default(), vec![$($xs),*]))
    };

    ($span:expr; $($xs:expr),* $(,)?) => {
        ::std::sync::Arc::new($crate::Expr::List(($span).into(), vec![$($xs),*]))
    };

    ($id:expr, $span:expr; $($xs:expr),* $(,)?) => {
        ::std::sync::Arc::new($crate::Expr::List(($id).into(), ($span).into(), vec![$($xs),*]))
    };
}

#[macro_export]
macro_rules! d {
    ($($k:ident = $v:expr),* $(,)?) => {
        ::std::sync::Arc::new($crate::Expr::Dict($crate::Span::default(), {
            let mut map = ::std::collections::BTreeMap::new();
            $(map.insert($crate::Symbol::intern(stringify!($k)), $v);)*
            map
        }))
    };

    ($span:expr; $($k:ident = $v:expr),* $(,)?) => {
        ::std::sync::Arc::new($crate::Expr::Dict(($span).into(), {
            let mut map = ::std::collections::BTreeMap::new();
            $(map.insert($crate::Symbol::intern(stringify!($k)), $v);)*
            map
        }))
    };

    ($id:expr, $span:expr; $($k:ident = $v:expr),* $(,)?) => {
        ::std::sync::Arc::new($crate::Expr::Dict(($id).into(), ($span).into(), {
            let mut map = ::std::collections::BTreeMap::new();
            $(map.insert($crate::Symbol::intern(stringify!($k)), $v);)*
            map
        }))
    };
}

#[macro_export]
macro_rules! v {
    ($x:expr) => {
        ::std::sync::Arc::new($crate::Expr::Var($crate::Var {
            span: $crate::Span::default(),
            name: $crate::Symbol::intern(&($x).to_string()),
        }))
    };

    ($span:expr; $x:expr) => {
        ::std::sync::Arc::new($crate::Expr::Var($crate::Var {
            span: ($span).into(),
            name: $crate::Symbol::intern(&($x).to_string()),
        }))
    };

    ($id:expr, $span:expr; $x:expr) => {
        ::std::sync::Arc::new($crate::Expr::Var($crate::Var {
            id: ($id).into(),
            span: ($span).into(),
            name: $crate::Symbol::intern(&($x).to_string()),
        }))
    };
}

#[macro_export]
macro_rules! app {
    ($f:expr, $x:expr) => {
        ::std::sync::Arc::new($crate::Expr::App(
            $crate::Span::default(),
            ($f).into(),
            ($x).into(),
        ))
    };

    ($span:expr; $f:expr, $x:expr) => {
        ::std::sync::Arc::new($crate::Expr::App(($span).into(), ($f).into(), ($x).into()))
    };

    ($id:expr, $span:expr; $f:expr, $x:expr) => {
        ::std::sync::Arc::new($crate::Expr::App(
            ($id).into(),
            ($span).into(),
            ($f).into(),
            ($x).into(),
        ))
    };
}

#[macro_export]
macro_rules! lam {
    ($x:ident -> $e:expr) => {
        ::std::sync::Arc::new($crate::Expr::Lam(
            $crate::Span::default(),
            $crate::Scope::new_sync(),
            $crate::Var::new(stringify!($x)),
            None,
            Vec::new(),
            ($e).into(),
        ))
    };

    ($span:expr; $x:ident -> $e:expr) => {
        ::std::sync::Arc::new($crate::Expr::Lam(
            ($span).into(),
            $crate::Scope::new_sync(),
            $crate::Var::new(stringify!($x)),
            None,
            Vec::new(),
            ($e).into(),
        ))
    };

    ($id:expr, $span:expr; $x:ident -> $e:expr) => {
        ::std::sync::Arc::new($crate::Expr::Lam(
            ($id).into(),
            ($span).into(),
            $crate::Scope::new_sync(),
            $crate::Var::new(stringify!($x)),
            None,
            Vec::new(),
            ($e).into(),
        ))
    };
}

#[macro_export]
macro_rules! let_in {
    (let $x:ident = ($e1:expr) in $e2:expr) => {
        ::std::sync::Arc::new($crate::Expr::Let(
            $crate::Span::default(),
            $crate::Var::new(stringify!($x)),
            None,
            ($e1).into(),
            ($e2).into(),
        ))
    };

    ($span:expr; let $x:ident = ($e1:expr) in $e2:expr) => {
        ::std::sync::Arc::new($crate::Expr::Let(
            ($span).into(),
            $crate::Var::new(stringify!($x)),
            None,
            ($e1).into(),
            ($e2).into(),
        ))
    };

    ($id:expr, $span:expr; let $x:ident = ($e1:expr) in $e2:expr) => {
        ::std::sync::Arc::new($crate::Expr::Let(
            ($id).into(),
            ($span).into(),
            $crate::Var::new(stringify!($x)),
            None,
            ($e1).into(),
            ($e2).into(),
        ))
    };
}

#[macro_export]
macro_rules! ite {
    (if ($e1:expr) { $e2:expr } else { $e3:expr }) => {
        ::std::sync::Arc::new($crate::Expr::Ite(
            $crate::Span::default(),
            ($e1).into(),
            ($e2).into(),
            ($e3).into(),
        ))
    };

    ($span:expr; if ($e1:expr) { $e2:expr } else { $e3:expr }) => {
        ::std::sync::Arc::new($crate::Expr::Ite(
            ($span).into(),
            ($e1).into(),
            ($e2).into(),
            ($e3).into(),
        ))
    };

    ($id:expr, $span:expr; if ($e1:expr) { $e2:expr } else { $e3:expr }) => {
        ::std::sync::Arc::new($crate::Expr::Ite(
            ($span).into(),
            ($e1).into(),
            ($e2).into(),
            ($e3).into(),
        ))
    };
}

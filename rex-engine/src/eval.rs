use std::{borrow::Borrow, cmp::Ordering, collections::BTreeMap, sync::Arc};

use futures::future;
use rex_ast::expr::{Expr, Scope, Var};
use rex_lexer::span::Span;

use crate::{
    error::{Error, Trace},
    ftable::Ftable,
};

#[derive(Clone)]
pub struct Context<State>
where
    State: Clone + Sync + 'static,
{
    pub scope: Scope,
    pub ftable: Arc<Ftable<State>>,
    pub state: Arc<State>,
}

impl<State> Context<State>
where
    State: Clone + Sync + 'static,
{
    pub fn with_scope(&self, scope: Scope) -> Self {
        Context {
            scope,
            ftable: self.ftable.clone(),
            state: self.state.clone(),
        }
    }
}

pub struct Stack<'a> {
    pub span: Span,
    pub parent: Option<&'a Stack<'a>>,
}

impl<'a> Stack<'a> {
    pub fn new(span: Span, parent: Option<&'a Stack<'a>>) -> Self {
        Stack { span, parent }
    }

    pub fn collect_vec(mut stack: Option<&Stack>, spans: &mut Vec<Span>) {
        while let Some(s) = stack {
            spans.push(s.span);
            stack = s.parent;
        }
    }

    pub fn to_vec(stack: Option<&Stack>) -> Vec<Span> {
        let mut spans = Vec::new();
        Self::collect_vec(stack, &mut spans);
        spans
    }
}

#[async_recursion::async_recursion]
pub async fn eval<State>(
    ctx: &Context<State>,
    expr: &Arc<Expr>,
    stack: Option<&Stack<'_>>,
) -> Result<Arc<Expr>, Error>
where
    State: Clone + Send + Sync + 'static,
{
    match &**expr {
        Expr::Bool(..)
        | Expr::Uint(..)
        | Expr::Int(..)
        | Expr::Float(..)
        | Expr::String(..)
        | Expr::Uuid(..)
        | Expr::DateTime(..)
        | Expr::Named(..)
        | Expr::Promise(..) => Ok(expr.clone()),
        Expr::Tuple(span, tuple) => Ok(eval_tuple(ctx, span, tuple, stack)
            .await?
            .unwrap_or_else(|| expr.clone())),
        Expr::List(span, list) => Ok(eval_list(ctx, span, list, stack)
            .await?
            .unwrap_or_else(|| expr.clone())),
        Expr::Dict(span, dict) => Ok(eval_dict(ctx, span, dict, stack)
            .await?
            .unwrap_or_else(|| expr.clone())),
        Expr::Var(var) => eval_var(ctx, var, stack).await,
        Expr::App(span, f, x) => eval_app(ctx, span, f, x, stack).await,
        Expr::Lam(span, scope, param, body) => eval_lam(ctx, span, scope, param, body).await,
        Expr::Let(span, var, def, body) => eval_let(ctx, span, var, def, body, stack).await,
        Expr::Ite(span, cond, then, r#else) => eval_ite(ctx, span, cond, then, r#else, stack).await,
        Expr::Curry(span, f, args) => Ok(Arc::new(Expr::Curry(*span, f.clone(), args.clone()))),
    }
}

pub async fn eval_tuple<State>(
    ctx: &Context<State>,
    span: &Span,
    tuple: &[Arc<Expr>],
    stack: Option<&Stack<'_>>,
) -> Result<Option<Arc<Expr>>, Error>
where
    State: Clone + Send + Sync + 'static,
{
    // Evaluate all elements of the tuple in parallel
    let futures = tuple
        .iter()
        .map(|v| eval(ctx, v, stack))
        .collect::<Vec<_>>();
    let results = future::join_all(futures)
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;

    // Check if any of the eval calls returned a different value to what was passed in.
    // If they are all the same, just return the original expression, instead of wasting
    // memory by keeping an identical copy.
    if expr_list_ptr_eq(tuple, &results) {
        return Ok(None);
    }

    Ok(Some(Arc::new(Expr::Tuple(*span, results))))
}

pub async fn eval_list<State>(
    ctx: &Context<State>,
    span: &Span,
    list: &[Arc<Expr>],
    stack: Option<&Stack<'_>>,
) -> Result<Option<Arc<Expr>>, Error>
where
    State: Clone + Send + Sync + 'static,
{
    // Evaluate all elements of the list in parallel
    let futures = list.iter().map(|v| eval(ctx, v, stack)).collect::<Vec<_>>();
    let results = future::join_all(futures)
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;

    // Check if any of the eval calls returned a different value to what was passed in.
    // If they are all the same, just return the original expression, instead of wasting
    // memory by keeping an identical copy.
    if expr_list_ptr_eq(list, &results) {
        return Ok(None);
    }

    Ok(Some(Arc::new(Expr::List(*span, results))))
}

fn expr_list_ptr_eq(vals: &[Arc<Expr>], results: &[Arc<Expr>]) -> bool {
    if vals.len() != results.len() {
        false
    } else {
        vals.iter()
            .zip(results.iter())
            .all(|(a, b)| Arc::ptr_eq(a, b))
    }
}

pub async fn eval_dict<State>(
    ctx: &Context<State>,
    span: &Span,
    dict: &BTreeMap<String, Arc<Expr>>,
    stack: Option<&Stack<'_>>,
) -> Result<Option<Arc<Expr>>, Error>
where
    State: Clone + Send + Sync + 'static,
{
    let mut keys = Vec::with_capacity(dict.len());
    let mut vals = Vec::with_capacity(dict.len());
    for (k, v) in dict {
        keys.push(k);
        vals.push(v.clone());
    }

    // Evaluate all entries in the dictionary in parallel
    let futures = vals.iter().map(|v| eval(ctx, v, stack)).collect::<Vec<_>>();
    let results = future::join_all(futures)
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;

    // Check if any of the eval calls returned a different value to what was passed in.
    // If they are all the same, just return the original expression, instead of wasting
    // memory by keeping an identical copy.
    if expr_list_ptr_eq(&vals, &results) {
        return Ok(None);
    }

    let mut result = BTreeMap::new();
    for (k, v) in keys.into_iter().zip(results) {
        result.insert(k.clone(), v);
    }
    Ok(Some(Arc::new(Expr::Dict(*span, result))))
}

#[async_recursion::async_recursion]
pub async fn eval_var<State>(
    ctx: &Context<State>,
    var: &Var,
    stack: Option<&Stack<'_>>,
) -> Result<Arc<Expr>, Error>
where
    State: Clone + Send + Sync + 'static,
{
    match ctx.scope.get(&var.name) {
        Some(expr) => Ok(expr.clone()),
        None => {
            if let Some(entry) = ctx.ftable.0.get(&var.name) {
                if entry.num_params == 0 {
                    if entry.items.len() != 1 {
                        panic!("Multiple functions: {}", var.name);
                    }
                    let f = &entry.items[0].1;
                    let res = f(ctx, &vec![]).await?;
                    return Ok(res);
                }

                return Ok(Arc::new(Expr::Curry(
                    Span::default(),
                    var.clone(),
                    Vec::new(),
                )));
            }

            Err(Error::VarNotFound {
                var: var.clone(),
                trace: Trace::from(stack),
            })
        }
    }
}

pub async fn eval_app<State>(
    ctx: &Context<State>,
    span: &Span,
    f: &Arc<Expr>,
    x: &Arc<Expr>,
    parent: Option<&Stack<'_>>,
) -> Result<Arc<Expr>, Error>
where
    State: Clone + Send + Sync + 'static,
{
    let stack = Stack::new(*span, parent);
    apply(ctx, f, x, Some(&stack)).await
}

pub async fn apply<State>(
    ctx: &Context<State>,
    f: impl Borrow<Arc<Expr>>,
    x: impl Borrow<Arc<Expr>>,
    stack: Option<&Stack<'_>>,
) -> Result<Arc<Expr>, Error>
where
    State: Clone + Send + Sync + 'static,
{
    let f = eval(ctx, f.borrow(), stack).await?;
    let x = eval(ctx, x.borrow(), stack).await?;

    match f.borrow() {
        Expr::Lam(_span, scope, param, body) => {
            let ctx = ctx.with_scope(scope.insert(param.name.clone(), x));
            eval(&ctx, body, stack).await
        }
        Expr::Curry(span, var, args) => {
            let mut args = args.clone();
            let entry = ctx
                .ftable
                .0
                .get(&var.name)
                .ok_or_else(|| Error::VarNotFound {
                    var: var.clone(),
                    trace: Trace::from(stack),
                })?;

            args.push(x.clone());
            match entry.num_params.cmp(&args.len()) {
                Ordering::Less => {
                    panic!("Too many arguments");
                }
                Ordering::Equal => {
                    let mut candidates: Vec<&super::ftable::FtableFn<State>> = Vec::new();
                    for (f_type, f) in entry.items.iter() {
                        if f_type.maybe_accepts_args(&args) {
                            candidates.push(f);
                        }
                    }

                    if candidates.is_empty() {
                        return Err(Error::Custom {
                            error: format!("0 candidates for function {}", var.name),
                            trace: Trace::from(stack),
                        });
                    }

                    if candidates.len() > 1 {
                        return Err(Error::Custom {
                            error: format!(
                                "{} candidates for function {}",
                                candidates.len(),
                                var.name
                            ),
                            trace: Trace::from(stack),
                        });
                    }

                    let f = candidates[0];

                    match f(ctx, &args).await {
                        Ok(res) => Ok(res),
                        Err(e) => Err(e.with_extra_trace(stack)),
                    }
                }
                Ordering::Greater => {
                    // TODO(loong): fix the span.
                    Ok(Arc::new(Expr::Curry(*span, var.clone(), args)))
                }
            }
        }
        _ => Err(Error::Custom {
            error: format!("Function application on non-function type: {}", f),
            trace: Trace::from(stack),
        }),
    }
}

pub async fn eval_lam<State>(
    ctx: &Context<State>,
    span: &Span,
    scope: &Scope,
    param: &Var,
    body: &Arc<Expr>,
) -> Result<Arc<Expr>, Error>
where
    State: Clone + Send + Sync + 'static,
{
    let mut scope = scope.clone();
    for entry in ctx.scope.iter() {
        scope.insert_mut(entry.0.clone(), entry.1.clone());
    }
    Ok(Arc::new(Expr::Lam(
        *span,
        scope,
        param.clone(),
        body.clone(),
    )))
}

pub async fn eval_let<State>(
    ctx: &Context<State>,
    _span: &Span,
    var: &Var,
    def: &Arc<Expr>,
    body: &Arc<Expr>,
    stack: Option<&Stack<'_>>,
) -> Result<Arc<Expr>, Error>
where
    State: Clone + Send + Sync + 'static,
{
    let mut ctx = ctx.clone();
    let value = eval(&ctx, def, stack).await?;
    ctx.scope = ctx.scope.insert(var.name.clone(), value);
    eval(&ctx, body, stack).await
}

pub async fn eval_ite<State>(
    ctx: &Context<State>,
    _span: &Span,
    cond: &Arc<Expr>,
    then: &Arc<Expr>,
    r#else: &Arc<Expr>,
    stack: Option<&Stack<'_>>,
) -> Result<Arc<Expr>, Error>
where
    State: Clone + Send + Sync + 'static,
{
    let cond = eval(ctx, cond, stack).await?;
    match &*cond {
        Expr::Bool(_, true) => eval(ctx, then, stack).await,
        Expr::Bool(_, false) => eval(ctx, r#else, stack).await,
        _ => unimplemented!(),
    }
}

#[cfg(test)]
pub mod test {
    use crate::{codec::Encode, engine::Builder, program::Program};
    use rex_ast::{assert_expr_eq, b, d, f, i, l, n, s, tup, u};
    use rex_type_system::{
        bool, dict, float, int, list, option, result, string, tuple,
        types::{Type, TypeCon},
        uint,
    };

    use super::*;

    /// Helper function for parsing, inferring, and evaluating a given code
    /// snippet. Pretty much all of the test suites can use this flow for
    /// testing that the engine is correctly evaluating types and expressions.
    async fn parse_infer_and_eval(code: &str) -> Result<(Arc<Expr>, Arc<Type>), Error> {
        let builder: Builder<()> = Builder::with_prelude().unwrap();
        let program = Program::compile(builder, code)?;
        let res_type = program.res_type.clone();
        let res = program.run(()).await?;
        Ok((res, res_type))
    }

    #[tokio::test]
    async fn test_literals() {
        let (res, res_type) = parse_infer_and_eval(r#"true"#).await.unwrap();
        assert_eq!(res_type, bool!());
        assert_expr_eq!(res, b!(true); ignore span);

        let (res, res_type) = parse_infer_and_eval(r#"420"#).await.unwrap();
        assert_eq!(res_type, uint!());
        assert_expr_eq!(res, u!(420); ignore span);

        let (res, res_type) = parse_infer_and_eval(r#"6.9"#).await.unwrap();
        assert_eq!(res_type, float!());
        assert_expr_eq!(res, f!(6.9); ignore span);

        let (res, res_type) = parse_infer_and_eval(r#""Hello, world!""#).await.unwrap();
        assert_eq!(res_type, string!());
        assert_expr_eq!(res, s!("Hello, world!"); ignore span);
    }

    #[tokio::test]
    async fn test_negate() {
        let (res, res_type) = parse_infer_and_eval(r#"-420"#).await.unwrap();
        assert_eq!(res_type, int!());
        assert_expr_eq!(res, i!(-420); ignore span);

        let (res, res_type) = parse_infer_and_eval(r#"-3.14"#).await.unwrap();
        assert_eq!(res_type, float!());
        assert_expr_eq!(res, f!(-3.14); ignore span);

        let (res, res_type) = parse_infer_and_eval(r#"(-3.14, -69)"#).await.unwrap();
        assert_eq!(res_type, tuple!(float!(), int!()));
        assert_expr_eq!(res, tup!(f!(-3.14), i!(-69)); ignore span);

        let (res, res_type) = parse_infer_and_eval(r#"[-0, -314, -69, -420]"#)
            .await
            .unwrap();
        assert_eq!(res_type, list!(int!()));
        assert_expr_eq!(res, l!(i!(0), i!(-314), i!(-69), i!(-420)); ignore span);

        let (res, res_type) = parse_infer_and_eval(r#"[-0.0, -3.14, -6.9, -42.0]"#)
            .await
            .unwrap();
        assert_eq!(res_type, list!(float!()));
        assert_expr_eq!(res, l!(f!(-0.0), f!(-3.14), f!(-6.9), f!(-42.0)); ignore span);

        let (res, res_type) = parse_infer_and_eval(r#"{ x = -3.14, y = -69 }"#)
            .await
            .unwrap();
        assert_eq!(res_type, dict! { x: float!(), y: int!() });
        assert_expr_eq!(res, d!{ x = f!(-3.14), y = i!(-69)}; ignore span);
    }

    #[tokio::test]
    async fn test_maths() {
        let (res, res_type) = parse_infer_and_eval(r#"6.9 + 4.20"#).await.unwrap();
        assert_eq!(res_type, float!());
        assert_expr_eq!(res, f!(11.100000000000001); ignore span);

        let (res, res_type) = parse_infer_and_eval(r#"6 + 9"#).await.unwrap();
        assert_eq!(res_type, uint!());
        assert_expr_eq!(res, u!(15); ignore span);

        let (res, res_type) = parse_infer_and_eval(r#"(-4) + (-20)"#).await.unwrap();
        assert_eq!(res_type, int!());
        assert_expr_eq!(res, i!(-24); ignore span);

        let (res, res_type) = parse_infer_and_eval(r#"(-4) + (int 20)"#).await.unwrap();
        assert_eq!(res_type, int!());
        assert_expr_eq!(res, i!(16); ignore span);

        let (res, res_type) = parse_infer_and_eval(r#"6.9 - 4.20"#).await.unwrap();
        assert_eq!(res_type, float!());
        assert_expr_eq!(res, f!(2.7); ignore span);

        let (res, res_type) = parse_infer_and_eval(r#"(int 6) - (int 9)"#).await.unwrap();
        assert_eq!(res_type, int!());
        assert_expr_eq!(res, i!(-3); ignore span);

        let (res, res_type) = parse_infer_and_eval(r#"(-) (int 4) (int 20)"#)
            .await
            .unwrap();
        assert_eq!(res_type, int!());
        assert_expr_eq!(res, i!(-16); ignore span);

        let (res, res_type) = parse_infer_and_eval(r#"4 * 20 + 6 * 9"#).await.unwrap();
        assert_eq!(res_type, uint!());
        assert_expr_eq!(res, u!(134); ignore span);

        let (res, res_type) = parse_infer_and_eval(r#"4 * (20 + 6) * 9"#).await.unwrap();
        assert_eq!(res_type, uint!());
        assert_expr_eq!(res, u!(936); ignore span);

        let (res, res_type) = parse_infer_and_eval(r#"((+) 3 14) * 4 + ((*) 20 6) / 9"#)
            .await
            .unwrap();
        assert_eq!(res_type, uint!());
        assert_expr_eq!(res, u!(81); ignore span);

        let (res, res_type) = parse_infer_and_eval(r#"((+) 3.0 1.4) * 4.0 + ((*) 2.0 6.9) / 3.14"#)
            .await
            .unwrap();
        assert_eq!(res_type, float!());
        assert_expr_eq!(res, f!(21.99490445859873); ignore span);

        let (res, res_type) = parse_infer_and_eval(r#"6.9 % 4.20"#).await.unwrap();
        assert_eq!(res_type, float!());
        assert_expr_eq!(res, f!(2.7); ignore span);

        let (res, res_type) = parse_infer_and_eval(r#"6 % 4"#).await.unwrap();
        assert_eq!(res_type, uint!());
        assert_expr_eq!(res, u!(2); ignore span);
    }

    #[tokio::test]
    async fn test_relational_operators_uint() {
        let values: Vec<u64> = vec![1, 2, 3, 4, 5];
        let expected = values
            .into_iter()
            .map(|x| vec![x == 3, x != 3, x < 3, x <= 3, x > 3, x >= 3])
            .collect::<Vec<_>>();
        let expected_expr = expected.try_encode(Span::default()).unwrap();

        let (res, res_type) = parse_infer_and_eval(
            r#"
            map (\x -> [x == 3, x != 3, x < 3, x <= 3, x > 3, x >= 3]) [1, 2, 3, 4, 5]
            "#,
        )
        .await
        .unwrap();
        assert_eq!(res_type, list!(list!(bool!())));
        assert_expr_eq!(res, expected_expr; ignore span);
    }

    #[tokio::test]
    async fn test_relational_operators_int() {
        let values: Vec<i64> = vec![1, 2, 3, 4, 5];
        let expected = values
            .into_iter()
            .map(|x| vec![x == 3, x != 3, x < 3, x <= 3, x > 3, x >= 3])
            .collect::<Vec<_>>();
        let expected_expr = expected.try_encode(Span::default()).unwrap();

        let (res, res_type) = parse_infer_and_eval(
            r#"
            let y = (int 3) in
            map (\x -> [x == y, x != y, x < y, x <= y, x > y, x >= y]) (map int [1, 2, 3, 4, 5])
            "#,
        )
        .await
        .unwrap();
        assert_eq!(res_type, list!(list!(bool!())));
        assert_expr_eq!(res, expected_expr; ignore span);
    }

    #[tokio::test]
    async fn test_relational_operators_float() {
        let values: Vec<f64> = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let expected = values
            .into_iter()
            .map(|x| vec![x == 3.0, x != 3.0, x < 3.0, x <= 3.0, x > 3.0, x >= 3.0])
            .collect::<Vec<_>>();
        let expected_expr = expected.try_encode(Span::default()).unwrap();

        let (res, res_type) = parse_infer_and_eval(
            r#"
            map
                (\x -> [x == 3.0, x != 3.0, x < 3.0, x <= 3.0, x > 3.0, x >= 3.0])
                [1.0, 2.0, 3.0, 4.0, 5.0]
            "#,
        )
        .await
        .unwrap();
        assert_eq!(res_type, list!(list!(bool!())));
        assert_expr_eq!(res, expected_expr; ignore span);
    }

    #[tokio::test]
    async fn test_relational_operators_string() {
        let values: Vec<&str> = vec!["a", "b", "c", "d", "e"];
        let expected = values
            .into_iter()
            .map(|x| vec![x == "c", x != "c", x < "c", x <= "c", x > "c", x >= "c"])
            .collect::<Vec<_>>();
        let expected_expr = expected.try_encode(Span::default()).unwrap();

        let (res, res_type) = parse_infer_and_eval(
            r#"
            map
                (\x -> [x == "c", x != "c", x < "c", x <= "c", x > "c", x >= "c"])
                ["a", "b", "c", "d", "e"]
            "#,
        )
        .await
        .unwrap();
        assert_eq!(res_type, list!(list!(bool!())));
        assert_expr_eq!(res, expected_expr; ignore span);
    }

    #[tokio::test]
    async fn test_append_list() {
        let (res, res_type) = parse_infer_and_eval(
            r#"
            [1, 2, 3] ++ [4] ++ [5, 6]
            "#,
        )
        .await
        .unwrap();
        assert_eq!(res_type, list!(uint!()));
        assert_expr_eq!(res, l!(u!(1), u!(2), u!(3), u!(4), u!(5), u!(6)); ignore span);
    }

    #[tokio::test]
    async fn test_append_string() {
        let (res, res_type) = parse_infer_and_eval(
            r#"
            "Hello" ++ " " ++ "World"
            "#,
        )
        .await
        .unwrap();
        assert_eq!(res_type, string!());
        assert_expr_eq!(res, s!("Hello World"); ignore span);
    }

    // TODO: get this test working again
    // #[tokio::test]
    async fn _test_let_add_in_add() {
        let (res, res_type) = parse_infer_and_eval(r#"let f = λx → x + x in f (6.9 + 3.14)"#)
            .await
            .unwrap();
        assert_eq!(res_type, float!());
        assert_expr_eq!(res, f!(20.080000000000002); ignore span);

        let (res, res_type) =
            parse_infer_and_eval(r#"let f = λx → x + x in f (identity 6.9 + identity 3.14)"#)
                .await
                .unwrap();
        assert_eq!(res_type, float!());
        assert_expr_eq!(res, f!(20.080000000000002); ignore span);

        let (res, res_type) =
            parse_infer_and_eval(r#"let f = λx → x + x in f (identity (6.9 + 3.14))"#)
                .await
                .unwrap();
        assert_eq!(res_type, float!());
        assert_expr_eq!(res, f!(20.080000000000002); ignore span);

        let (res, res_type) =
            parse_infer_and_eval(r#"let f = λx → x + x in identity (f (6.9 + 3.14))"#)
                .await
                .unwrap();
        assert_eq!(res_type, float!());
        assert_expr_eq!(res, f!(20.080000000000002); ignore span);

        let (res, res_type) =
            parse_infer_and_eval(r#"let f = λx → (identity x + identity x) in f (6.9 + 3.14)"#)
                .await
                .unwrap();
        assert_eq!(res_type, float!());
        assert_expr_eq!(res, f!(20.080000000000002); ignore span);

        let (res, res_type) =
            parse_infer_and_eval(r#"let f = λx → identity (x + x) in f (6.9 + 3.14)"#)
                .await
                .unwrap();
        assert_eq!(res_type, float!());
        assert_expr_eq!(res, f!(20.080000000000002); ignore span);
    }

    #[tokio::test]
    async fn test_let_id_in() {
        let (res, res_type) = parse_infer_and_eval(r#"let f = identity in identity 420"#)
            .await
            .unwrap();
        assert_eq!(res_type, uint!());
        assert_expr_eq!(res, u!(420); ignore span);

        let (res, res_type) = parse_infer_and_eval(r#"let f = identity in identity 6.9"#)
            .await
            .unwrap();
        assert_eq!(res_type, float!());
        assert_expr_eq!(res, f!(6.9); ignore span);

        let (res, res_type) = parse_infer_and_eval(r#"let f = identity in identity 69"#)
            .await
            .unwrap();
        assert_eq!(res_type, uint!());
        assert_expr_eq!(res, u!(69); ignore span);

        let (res, res_type) = parse_infer_and_eval(r#"let f = identity in identity 3.14"#)
            .await
            .unwrap();
        assert_eq!(res_type, float!());
        assert_expr_eq!(res, f!(3.14); ignore span);

        let (res, res_type) = parse_infer_and_eval(r#"let f = identity in (f 6.9) + (f 3.14)"#)
            .await
            .unwrap();
        assert_eq!(res_type, float!());
        assert_expr_eq!(res, f!(10.040000000000001); ignore span);
    }

    #[tokio::test]
    async fn test_tuple() {
        let (res, res_type) = parse_infer_and_eval(r#"(6.9, 420, true)"#).await.unwrap();
        assert_eq!(res_type, tuple!(float!(), uint!(), bool!()));
        assert_expr_eq!(res, tup!(f!(6.9), u!(420), b!(true)); ignore span);

        let (res, res_type) = parse_infer_and_eval(r#"(6.9, 420, true, )"#).await.unwrap();
        assert_eq!(res_type, tuple!(float!(), uint!(), bool!()));
        assert_expr_eq!(res, tup!(f!(6.9), u!(420), b!(true)); ignore span);

        let (res, res_type) =
            parse_infer_and_eval(r#"(3.14 * 6.9, 20 * 4, (*) (int 4) (int 105), true || false)"#)
                .await
                .unwrap();
        assert_eq!(res_type, tuple!(float!(), uint!(), int!(), bool!()));
        assert_expr_eq!(res, tup!(f!(21.666), u!(80), i!(420), b!(true)); ignore span);

        let (res, res_type) = parse_infer_and_eval(
            r#"
            (
                (identity 3.14) * (identity 6.9),
                (identity 20) * (identity 4),
                (*) (identity (int 4)) (identity (int 105)),
                (identity true) || (identity false)
            )
        "#,
        )
        .await
        .unwrap();
        assert_eq!(res_type, tuple!(float!(), uint!(), int!(), bool!()));
        assert_expr_eq!(res, tup!(f!(21.666), u!(80), i!(420), b!(true)); ignore span);

        let (res, res_type) = parse_infer_and_eval(
            r#"
            (
                identity (3.14 * 6.9),
                identity (20 * 4),
                identity ((*) (int 4) (int 105)),
                identity (true || false)
            )"#,
        )
        .await
        .unwrap();
        assert_eq!(res_type, tuple!(float!(), uint!(), int!(), bool!()));
        assert_expr_eq!(res, tup!(f!(21.666), u!(80), i!(420), b!(true)); ignore span);

        let (res, res_type) = parse_infer_and_eval(
            r#"
            (
                identity ((identity 3.14) * (identity 6.9)),
                identity ((identity 20) * (identity 4)),
                identity ((*) (identity (int 4)) (identity (int 105))),
                identity ((identity true) || (identity false))
            )
        "#,
        )
        .await
        .unwrap();
        assert_eq!(res_type, tuple!(float!(), uint!(), int!(), bool!()));
        assert_expr_eq!(res, tup!(f!(21.666), u!(80), i!(420), b!(true)); ignore span);

        // Check logic for tuple eval where some elements eval to the same value, some different
        let (res, res_type) = parse_infer_and_eval(r#"(1, 2 + 3, 4, 5 + 6)"#)
            .await
            .unwrap();
        assert_eq!(res_type, tuple!(uint!(), uint!(), uint!(), uint!()));
        assert_expr_eq!(res, tup!(u!(1), u!(5), u!(4), u!(11)); ignore span);
    }

    #[tokio::test]
    async fn test_list() {
        // Empty list
        let (res, res_type) = parse_infer_and_eval(r#"[]"#).await.unwrap();
        assert!(match &*res_type {
            Type::App(a, inner) => {
                match &**a {
                    Type::Con(TypeCon::List) => matches!(&**inner, Type::Var(_)),
                    _ => false,
                }
            }
            _ => false,
        });
        assert_expr_eq!(res, l!(); ignore span);

        // Single item
        let (res, res_type) = parse_infer_and_eval(r#"[420]"#).await.unwrap();
        assert_eq!(res_type, list!(uint!()));
        assert_expr_eq!(res, l!(u!(420)); ignore span);

        // Single item with trailing comma
        let (res, res_type) = parse_infer_and_eval(r#"[420,]"#).await.unwrap();
        assert_eq!(res_type, list!(uint!()));
        assert_expr_eq!(res, l!(u!(420)); ignore span);

        // Multiple items
        let (res, res_type) = parse_infer_and_eval(r#"[420, 69, 555]"#).await.unwrap();
        assert_eq!(res_type, list!(uint!()));
        assert_expr_eq!(res, l!(u!(420), u!(69), u!(555)); ignore span);

        // Multiple items with trailing comma
        let (res, res_type) = parse_infer_and_eval(r#"[420, 69, 555,]"#).await.unwrap();
        assert_eq!(res_type, list!(uint!()));
        assert_expr_eq!(res, l!(u!(420), u!(69), u!(555)); ignore span);

        // Check logic for list eval where some elements eval to the same value, some different
        let (res, res_type) = parse_infer_and_eval(r#"[1, 2 + 3, 4, 5 + 6]"#)
            .await
            .unwrap();
        assert_eq!(res_type, list!(uint!()));
        assert_expr_eq!(res, l!(u!(1), u!(5), u!(4), u!(11)); ignore span);
    }

    #[tokio::test]
    async fn test_dict() {
        // Empty dictionary
        let (res, res_type) = parse_infer_and_eval(r#"{}"#).await.unwrap();
        assert_eq!(res_type, dict! {});
        assert_expr_eq!(res, d!(); ignore span);

        // Single item
        let (res, res_type) = parse_infer_and_eval(r#"{ a = 420 }"#).await.unwrap();
        assert_eq!(res_type, dict! { a: uint!() });
        assert_expr_eq!(res, d!(a = u!(420)); ignore span);

        // Single item with trailing comma
        let (res, res_type) = parse_infer_and_eval(r#"{ a = 420, }"#).await.unwrap();
        assert_eq!(res_type, dict! { a: uint!() });
        assert_expr_eq!(res, d!( a = u!(420)); ignore span);

        // Multiple items
        let (res, res_type) = parse_infer_and_eval(r#"{ a = 420, b = 3.14, c = "hello" }"#)
            .await
            .unwrap();
        assert_eq!(res_type, dict! { a: uint!(), b: float!(), c: string!() });
        assert_expr_eq!(res, d!(a = u!(420), b = f!(3.14), c = s!("hello")); ignore span);

        // Multiple items with trailing comma
        let (res, res_type) = parse_infer_and_eval(r#"{ a = 420, b = 3.14, c = "hello", }"#)
            .await
            .unwrap();
        assert_eq!(res_type, dict! { a: uint!(), b: float!(), c: string!() });
        assert_expr_eq!(res, d!(a = u!(420), b = f!(3.14), c = s!("hello")); ignore span);

        // Check logic for list eval where some elements eval to the same value, some different
        let (res, res_type) = parse_infer_and_eval(r#"{a = 1, b = 2 + 3, c = 4, d = 5 + 6 }"#)
            .await
            .unwrap();
        assert_eq!(
            res_type,
            dict! { a: uint!(), b: uint!(), c: uint!(), d: uint!() }
        );
        assert_expr_eq!(res, d!(a = u!(1), b = u!(5), c = u!(4), d = u!(11)); ignore span);
    }

    #[tokio::test]
    async fn test_uuid() {
        let (res, res_type) = parse_infer_and_eval(r#"random_uuid"#).await.unwrap();
        assert!(matches!(&*res_type, Type::Con(TypeCon::Uuid)));
        assert!(matches!(&*res, Expr::Uuid(..))); // Don't check value; it's random!

        let (res, res_type) = parse_infer_and_eval(r#"string random_uuid"#).await.unwrap();
        assert!(matches!(&*res_type, Type::Con(TypeCon::String)));
        assert!(matches!(&*res, Expr::String(..))); // Don't check value; it's random!
    }

    #[tokio::test]
    async fn test_datetime() {
        let (res, res_type) = parse_infer_and_eval(r#"now"#).await.unwrap();
        assert!(matches!(&*res_type, Type::Con(TypeCon::DateTime)));
        assert!(matches!(&*res, Expr::DateTime(..))); // Don't check value; depends on current time

        let (res, res_type) = parse_infer_and_eval(r#"string now"#).await.unwrap();
        assert!(matches!(&*res_type, Type::Con(TypeCon::String)));
        assert!(matches!(&*res, Expr::String(..))); // Don't check value; depends on current time
    }

    #[tokio::test]
    async fn test_f_passthrough() {
        let (res, res_type) = parse_infer_and_eval(r#"(identity (&&)) true true"#)
            .await
            .unwrap();
        assert_eq!(res_type, bool!());
        assert_expr_eq!(res, b!(true); ignore span);

        let (res, res_type) = parse_infer_and_eval(r#"(identity (+)) 69 420"#)
            .await
            .unwrap();
        assert_eq!(res_type, uint!());
        assert_expr_eq!(res, u!(489); ignore span);

        let (res, res_type) = parse_infer_and_eval(r#"(identity (+)) 6.9 42.0"#)
            .await
            .unwrap();
        assert_eq!(res_type, float!());
        assert_expr_eq!(res, f!(48.9); ignore span);
    }

    #[tokio::test]
    async fn test_polymorphism() {
        let (res, res_type) =
            parse_infer_and_eval(r#"identity (identity 6.9, identity 420, identity (-420))"#)
                .await
                .unwrap();
        assert_eq!(res_type, tuple!(float!(), uint!(), int!()));
        assert_expr_eq!(res, tup!(f!(6.9), u!(420), i!(-420)); ignore span);

        let (res, res_type) =
            parse_infer_and_eval(r#"swap (swap (6.9, 420), swap (true, "hello, world!"))"#)
                .await
                .unwrap();
        assert_eq!(
            res_type,
            tuple!(tuple!(string!(), bool!()), tuple!(uint!(), float!())),
        );
        assert_expr_eq!(res, tup!(tup!(s!("hello, world!"), b!(true)), tup!(u!(420), f!(6.9))); ignore span);
    }

    #[tokio::test]
    async fn test_let_polymorphism() {
        let (res, res_type) =
            parse_infer_and_eval(r#"let f = λx → x in (f 6.9, f 420, f true, f "hello, world!")"#)
                .await
                .unwrap();
        assert_eq!(res_type, tuple!(float!(), uint!(), bool!(), string!()),);
        assert_expr_eq!(
            res,
            tup!(f!(6.9), u!(420), b!(true), s!("hello, world!")); ignore span
        );
    }

    #[tokio::test]
    async fn test_let_parametric_polymorphism() {
        let (res, res_type) = parse_infer_and_eval(
            r#"let f = identity in (f 6.9, f 420, f true, f "hello, world!")"#,
        )
        .await
        .unwrap();
        assert_eq!(res_type, tuple!(float!(), uint!(), bool!(), string!()),);
        assert_expr_eq!(
            res,
            tup!(f!(6.9), u!(420), b!(true), s!("hello, world!")); ignore span
        );
    }

    #[tokio::test]
    async fn test_let_overloaded_polymorphism() {
        let (res, res_type) =
            parse_infer_and_eval(r#"let f = λx → -x in (f 6.9, f 420, f (int 314))"#)
                .await
                .unwrap();
        assert_eq!(res_type, tuple!(float!(), int!(), int!()));
        assert_expr_eq!(res, tup!(f!(-6.9), i!(-420), i!(-314)); ignore span);
    }

    // TODO: get this test working again
    // #[tokio::test]
    async fn _test_parametric_overloaded_let_polymorphism() {
        let (res, res_type) =
            parse_infer_and_eval(r#"let f = λx → identity (x + x) in (f 6.9, f 420, f (-314))"#)
                .await
                .unwrap();
        assert_eq!(res_type, tuple!(float!(), uint!(), int!()));
        assert_expr_eq!(res, tup!(f!(13.8), u!(840), i!(-628)); ignore span);
    }

    #[tokio::test]
    async fn test_let_bind_to_ftable() {
        let (res, res_type) = parse_infer_and_eval(r#"let f = negate in f 420"#)
            .await
            .unwrap();
        assert_eq!(res_type, int!());
        assert_expr_eq!(res, i!(-420); ignore span);

        let (res, res_type) = parse_infer_and_eval(r#"let f = negate in f 6.9"#)
            .await
            .unwrap();
        assert_eq!(res_type, float!());
        assert_expr_eq!(res, f!(-6.9); ignore span);

        let (res, res_type) = parse_infer_and_eval(r#"let f = (+) in f 69 420"#)
            .await
            .unwrap();
        assert_eq!(res_type, uint!());
        assert_expr_eq!(res, u!(489); ignore span);

        let (res, res_type) = parse_infer_and_eval(r#"let f = (+) in f 6.9 42.0"#)
            .await
            .unwrap();
        assert_eq!(res_type, float!());
        assert_expr_eq!(res, f!(48.9); ignore span);
    }

    #[tokio::test]
    async fn test_let_in() {
        let (res, res_type) = parse_infer_and_eval(r#"let a = 420 in a"#).await.unwrap();
        assert_eq!(res_type, uint!());
        assert_expr_eq!(res, u!(420); ignore span);

        let (res, res_type) = parse_infer_and_eval(r#"let a = 420, in a"#).await.unwrap();
        assert_eq!(res_type, uint!());
        assert_expr_eq!(res, u!(420); ignore span);

        let (res, res_type) = parse_infer_and_eval(r#"let a = 420, b = 69 in (420, 69)"#)
            .await
            .unwrap();
        assert_eq!(res_type, tuple!(uint!(), uint!()));
        assert_expr_eq!(res, tup!(u!(420), u!(69)); ignore span);

        let (res, res_type) = parse_infer_and_eval(r#"let a = 420, b = 69, in (420, 69)"#)
            .await
            .unwrap();
        assert_eq!(res_type, tuple!(uint!(), uint!()));
        assert_expr_eq!(res, tup!(u!(420), u!(69)); ignore span);
    }

    #[tokio::test]
    async fn test_let_in_cascade() {
        let (res, res_type) =
            parse_infer_and_eval(r#"let f = (λx → -x), u = f 6.9, v = f u in (u, v)"#)
                .await
                .unwrap();
        assert_eq!(res_type, tuple!(float!(), float!()));
        assert_expr_eq!(res, tup!(f!(-6.9), f!(6.9)); ignore span);
    }

    #[tokio::test]
    async fn test_result() {
        let (res, res_type) = parse_infer_and_eval(r#"let a = Ok 4, b = Err "bad" in [a, b]"#)
            .await
            .unwrap();
        assert_eq!(res_type, list!(result!(string!(), uint!())));
        assert_expr_eq!(res, l!(n!("Ok", Some(u!(4))), n!("Err", Some(s!("bad")))); ignore span);

        let (res, res_type) = parse_infer_and_eval(r#"map is_ok [Ok 3, Err "bad"]"#)
            .await
            .unwrap();
        assert_eq!(res_type, list!(bool!()));
        assert_expr_eq!(res, l!(b!(true), b!(false)));

        let (res, res_type) = parse_infer_and_eval(r#"map is_err [Ok 3, Err "bad"]"#)
            .await
            .unwrap();
        assert_eq!(res_type, list!(bool!()));
        assert_expr_eq!(res, l!(b!(false), b!(true)));
    }

    #[tokio::test]
    async fn test_map_result() {
        let (res, res_type) = parse_infer_and_eval(
            r#"
                let
                    a = Ok 4,
                    b = Err "bad",
                    f = map (\x -> [x, x + 1, x + 2])
                in
                    map f [a, b]
                "#,
        )
        .await
        .unwrap();
        assert_eq!(res_type, list!(result!(string!(), list!(uint!()))));
        assert_expr_eq!(
            res,
            l!(n!("Ok", Some(l!(u!(4), u!(5), u!(6)))),
               n!("Err", Some(s!("bad"))));
            ignore span);
    }

    #[tokio::test]
    async fn test_and_then_result() {
        let (res, res_type) = parse_infer_and_eval(
            r#"
                let
                    a = Ok 0,
                    b = Ok 1,
                    c = Err "bad",
                    f = and_then (\x -> if x == 0 then Ok 3.14 else Err "nonzero")
                in
                    map f [a, b, c]
                "#,
        )
        .await
        .unwrap();
        assert_eq!(res_type, list!(result!(string!(), float!())));
        assert_expr_eq!(
            res,
            l!(
                n!("Ok", Some(f!(3.14))),
                n!("Err", Some(s!("nonzero"))),
                n!("Err", Some(s!("bad"))));
            ignore span);
    }

    #[tokio::test]
    async fn test_or_else_result() {
        let (res, res_type) = parse_infer_and_eval(
            r#"
                let
                    a = Ok "one",
                    b = Err 0,
                    c = Err 1,
                    f = or_else (\x -> if x == 0 then Ok "yes" else Err 3.14)
                in
                    map f [a, b, c]
                "#,
        )
        .await
        .unwrap();
        assert_eq!(res_type, list!(result!(float!(), string!())));
        assert_expr_eq!(
            res,
            l!(
                n!("Ok", Some(s!("one"))),
                n!("Ok", Some(s!("yes"))),
                n!("Err", Some(f!(3.14))));
            ignore span);
    }

    #[tokio::test]
    async fn test_unwrap_or_else_result() {
        let (res, res_type) = parse_infer_and_eval(
            r#"
                let
                    a = Ok 4,
                    b = Err "bad",
                    f = unwrap_or_else (\x -> 99)
                in
                    map f [a, b]
                "#,
        )
        .await
        .unwrap();
        assert_eq!(res_type, list!(uint!()));
        assert_expr_eq!(res, l!(u!(4), u!(99)); ignore span);
    }

    #[tokio::test]
    async fn test_unwrap_result() {
        let (res, res_type) = parse_infer_and_eval(r#"unwrap (Ok 4)"#).await.unwrap();
        assert_eq!(res_type, uint!());
        assert_expr_eq!(res, u!(4); ignore span);

        let res = parse_infer_and_eval(r#"unwrap (Err 'oops!')"#).await;
        assert_eq!(
            res,
            Err(Error::Custom {
                error: "unwrap called with Err: \"oops!\"".to_string(),
                trace: Trace(vec![Span::new(1, 1, 1, 21)])
            })
        );
    }

    #[tokio::test]
    async fn test_option() {
        let (res, res_type) = parse_infer_and_eval(r#"let a = Some 4, b = None in [a, b]"#)
            .await
            .unwrap();
        assert_eq!(res_type, list!(option!(uint!())));
        assert_expr_eq!(res, l!(n!("Some", Some(u!(4))), n!("None", None)); ignore span);

        let (res, res_type) = parse_infer_and_eval(r#"map is_some [Some 3, None]"#)
            .await
            .unwrap();
        assert_eq!(res_type, list!(bool!()));
        assert_expr_eq!(res, l!(b!(true), b!(false)));

        let (res, res_type) = parse_infer_and_eval(r#"map is_none [Some 3, None]"#)
            .await
            .unwrap();
        assert_eq!(res_type, list!(bool!()));
        assert_expr_eq!(res, l!(b!(false), b!(true)));
    }

    #[tokio::test]
    async fn test_map_option() {
        let (res, res_type) = parse_infer_and_eval(
            r#"
                let
                    a = Some 4,
                    b = None,
                    f = map (\x -> [x, x + 1, x + 2])
                in
                    map f [a, b]
                "#,
        )
        .await
        .unwrap();
        assert_eq!(res_type, list!(option!(list!(uint!()))));
        assert_expr_eq!(
            res,
            l!(n!("Some", Some(l!(u!(4), u!(5), u!(6)))),
               n!("None", None));
            ignore span);
    }

    #[tokio::test]
    async fn test_and_then_option() {
        let (res, res_type) = parse_infer_and_eval(
            r#"
                let
                    a = Some 0,
                    b = Some 1,
                    c = None,
                    f = and_then (\x -> if x == 0 then Some 3.14 else None)
                in
                    map f [a, b, c]
                "#,
        )
        .await
        .unwrap();
        assert_eq!(res_type, list!(option!(float!())));
        assert_expr_eq!(
            res,
            l!(
                n!("Some", Some(f!(3.14))),
                n!("None", None),
                n!("None", None));
            ignore span);
    }

    #[tokio::test]
    async fn test_or_else_option() {
        let (res, res_type) = parse_infer_and_eval(
            r#"
                let
                    a = Some 5.1,
                    b = None,
                    f = or_else (\x -> Some 3.14)
                in
                    map f [a, b]
                "#,
        )
        .await
        .unwrap();
        assert_eq!(res_type, list!(option!(float!())));
        assert_expr_eq!(
            res,
            l!(
                n!("Some", Some(f!(5.1))),
                n!("Some", Some(f!(3.14))));
            ignore span);

        let (res, res_type) = parse_infer_and_eval(
            r#"
                let
                    a = Some 5.1,
                    b = None,
                    f = or_else (\x -> None)
                in
                    map f [a, b]
                "#,
        )
        .await
        .unwrap();
        assert_eq!(res_type, list!(option!(float!())));
        assert_expr_eq!(
            res,
            l!(
                n!("Some", Some(f!(5.1))),
                n!("None", None));
            ignore span);
    }

    #[tokio::test]
    async fn test_or_else_overload() {
        let (res, res_type) = parse_infer_and_eval(
            r#"
                let
                    template = λa b c → or_else (λx → if (a x) then b else c),
                    f = template (λx → x == 0) (Ok "yes") (Err 2),
                    g = template (λx → true) (Some 1.5) (Some 2.5),
                in (
                    map f [Ok "one", Err 0, Err 1],
                    map g [Some 3.5, None]
                )
                "#,
        )
        .await
        .unwrap();
        assert_eq!(
            res_type,
            tuple!(list!(result!(uint!(), string!())), list!(option!(float!())),)
        );
        assert_expr_eq!(
            res,
            tup!(
                l!(
                    n!("Ok", Some(s!("one"))),
                    n!("Ok", Some(s!("yes"))),
                    n!("Err", Some(u!(2)))),
                l!(n!("Some", Some(f!(3.5))),
                    n!("Some", Some(f!(1.5)))))

            ;
            ignore span);
    }

    #[tokio::test]
    async fn test_unwrap_or_else_option() {
        let (res, res_type) = parse_infer_and_eval(
            r#"
                let
                    a = Some 4,
                    b = None,
                    f = unwrap_or_else (\x -> 99)
                in
                    map f [a, b]
                "#,
        )
        .await
        .unwrap();
        assert_eq!(res_type, list!(uint!()));
        assert_expr_eq!(res, l!(u!(4), u!(99)); ignore span);
    }

    #[tokio::test]
    async fn test_unwrap_or_else_overload() {
        let (res, res_type) = parse_infer_and_eval(
            r#"(
                map (unwrap_or_else (\z -> 99)) [Ok 4, Err "bad"],
                map (unwrap_or_else (\z -> 2.5)) [Some 4.5, None]
            )
            "#,
        )
        .await
        .unwrap();

        assert_eq!(res_type, tuple!(list!(uint!()), list!(float!())));
        assert_expr_eq!(res,tup!(l!(u!(4), u!(99)), l!(f!(4.5), f!(2.5))); ignore span);

        // FIXME(peter): let bindings are not properly generalized in this case
        // let (res, res_type) = parse_infer_and_eval(
        //     r#"
        //     let
        //         u = unwrap_or_else
        //     in (
        //         map (u (\z -> 99)) [Ok 4, Err "bad"],
        //         map (u (\z -> 2.5)) [Some 4.5, None]
        //     )
        //     "#,
        // )
        // .await
        // .unwrap();
        // assert_eq!(res_type, tuple!(list!(uint!()), list!(float!())));
        // assert_expr_eq!(res,tup!(l!(u!(4), u!(99)), l!(f!(4.5), f!(2.5))); ignore span);

        let (res, res_type) = parse_infer_and_eval(
            r#"
            let
                f = (unwrap_or_else (\z -> 99)),
                g = (unwrap_or_else (\z -> 99)),
            in (
                map f [Ok 4, Err "bad"],
                map g [Some 5, None]
            )
            "#,
        )
        .await
        .unwrap();
        assert_eq!(res_type, tuple!(list!(uint!()), list!(uint!())));
        assert_expr_eq!(res,tup!(l!(u!(4), u!(99)), l!(u!(5), u!(99))); ignore span);

        // FIXME(peter): let bindings are not properly generalized in this case
        // let (res, res_type) = parse_infer_and_eval(
        //     r#"
        //     let
        //         f = (unwrap_or_else (\z -> 99)),
        //     in (
        //         map f [Ok 4, Err "bad"],
        //         map f [Some 5, None]
        //     )
        //     "#,
        // )
        // .await
        // .unwrap();
        // assert_eq!(res_type, tuple!(list!(uint!()), list!(uint!())));
        // assert_expr_eq!(res,tup!(l!(u!(4), u!(99)), l!(u!(5), u!(99))); ignore span);
    }

    #[tokio::test]
    async fn test_unwrap_option() {
        let (res, res_type) = parse_infer_and_eval(r#"unwrap (Some 4)"#).await.unwrap();
        assert_eq!(res_type, uint!());
        assert_expr_eq!(res, u!(4); ignore span);

        let res = parse_infer_and_eval(r#"unwrap None"#).await;
        assert_eq!(
            res,
            Err(Error::Custom {
                error: "unwrap called with None".to_string(),
                trace: Trace(vec![Span::new(1, 1, 1, 12)])
            })
        );
    }

    #[tokio::test]
    async fn test_map() {
        let (res, res_type) = parse_infer_and_eval(r#"map (λx → -x) [3.14, 6.9, 42.0, 1.0]"#)
            .await
            .unwrap();
        assert_eq!(res_type, list!(float!()));
        assert_expr_eq!(res, l!(f!(-3.14), f!(-6.9), f!(-42.0), f!(-1.0)); ignore span);

        let (res, res_type) = parse_infer_and_eval(r#"map identity [3.14, 6.9, 42.0, 1.0]"#)
            .await
            .unwrap();
        assert_eq!(res_type, list!(float!()));
        assert_expr_eq!(res, l!(f!(3.14), f!(6.9), f!(42.0), f!(1.0)); ignore span);

        let (res, res_type) = parse_infer_and_eval(r#"map negate [3.14, 6.9, 42.0, 1.0]"#)
            .await
            .unwrap();
        assert_eq!(res_type, list!(float!()));
        assert_expr_eq!(res, l!(f!(-3.14), f!(-6.9), f!(-42.0), f!(-1.0)); ignore span);

        let (res, res_type) =
            parse_infer_and_eval(r#"map (let f = identity in f) [3.14, 6.9, 42.0, 1.0]"#)
                .await
                .unwrap();
        assert_eq!(res_type, list!(float!()));
        assert_expr_eq!(res, l!(f!(3.14), f!(6.9), f!(42.0), f!(1.0)); ignore span);

        let (res, res_type) =
            parse_infer_and_eval(r#"map (let f = (λx → -x) in f) [3.14, 6.9, 42.0, 1.0]"#)
                .await
                .unwrap();
        assert_eq!(res_type, list!(float!()));
        assert_expr_eq!(res, l!(f!(-3.14), f!(-6.9), f!(-42.0), f!(-1.0)); ignore span);

        // FIXME(loong): this test is not passing.
        // ```rex
        // let (res, res_type) =
        //     parse_infer_and_eval(r#"map (let f = (λx → identity (-x)) in f) [3.14, 6.9, 42.0, 1.0]"#)
        //         .await
        //         .unwrap();
        // assert_eq!(res_type, list!(float!()));
        // assert_expr_eq!(res, l!(f!(-3.14), f!(-6.9), f!(-42.0), f!(-1.0)); ignore span);
        // ```

        let (res, res_type) = parse_infer_and_eval(
            r#"map (let f = (λx → - (identity x)) in f) [3.14, 6.9, 42.0, 1.0]"#,
        )
        .await
        .unwrap();
        assert_eq!(res_type, list!(float!()));
        assert_expr_eq!(res, l!(f!(-3.14), f!(-6.9), f!(-42.0), f!(-1.0)); ignore span);

        // FIXME(loong): this test is not passing.
        // ```rex
        // let (res, res_type) = parse_infer_and_eval(
        //     r#"map (let f = (λx → identity (-(identity x))) in f) [3.14, 6.9, 42.0, 1.0]"#,
        // )
        // .await
        // .unwrap();
        // assert_eq!(res_type, list!(float!()));
        // assert_expr_eq!(res, l!(f!(-3.14), f!(-6.9), f!(-42.0), f!(-1.0)); ignore span);
        // ```

        let (res, res_type) =
            parse_infer_and_eval(r#"map (identity (λx → -x)) [3.14, 6.9, 42.0, 1.0]"#)
                .await
                .unwrap();
        assert_eq!(res_type, list!(float!()));
        assert_expr_eq!(res, l!(f!(-3.14), f!(-6.9), f!(-42.0), f!(-1.0)); ignore span);

        let (res, res_type) =
            parse_infer_and_eval(r#"map (let f = (λx → - (identity x)) in f) [3.14]"#)
                .await
                .unwrap();
        assert_eq!(res_type, list!(float!()));
        assert_expr_eq!(res, l!(f!(-3.14)); ignore span);

        let (res, res_type) = parse_infer_and_eval(r#"let n = (λx → - x) in (n 4, n 4.2)"#)
            .await
            .unwrap();
        assert_eq!(res_type, tuple!(int!(), float!()));
        assert_expr_eq!(res, tup!(i!(-4), f!(-4.2)); ignore span);
    }

    #[tokio::test]
    async fn test_filter() {
        let (res, res_type) = parse_infer_and_eval(
            r#"
            filter (\x -> x % 2 == 0) [1, 2, 3, 4, 5]
            "#,
        )
        .await
        .unwrap();
        assert_eq!(res_type, list!(uint!()));
        assert_expr_eq!(res, l!(u!(2), u!(4)); ignore span);

        let (res, res_type) = parse_infer_and_eval(
            r#"
            let
                is_even = (\x -> x % 2 == 0)
            in
                filter is_even [1, 2, 3, 4, 5]
            "#,
        )
        .await
        .unwrap();
        assert_eq!(res_type, list!(uint!()));
        assert_expr_eq!(res, l!(u!(2), u!(4)); ignore span);
    }

    #[tokio::test]
    async fn test_filter_map() {
        let (res, res_type) = parse_infer_and_eval(
            r#"
            filter_map
                (\x ->
                    if x % 2 == 0 then
                        (Some (100 + x))
                    else
                        None
                )
                 [1, 2, 3, 4, 5]
            "#,
        )
        .await
        .unwrap();
        assert_eq!(res_type, list!(uint!()));
        assert_expr_eq!(res, l!(u!(102), u!(104)); ignore span);

        let (res, res_type) = parse_infer_and_eval(
            r#"
            let
                results = [Ok "one", Err 2, Ok "three", Err 4],
            in
                filter_map (unwrap_or_else (λy → None)) (map (λx → map Some x) results)
            "#,
        )
        .await
        .unwrap();
        assert_eq!(res_type, list!(string!()));
        assert_expr_eq!(res, l!(s!("one"), s!("three")); ignore span);

        let (res, _res_type) = parse_infer_and_eval(
            r#"
            let
                results = [Ok "one", Err 2, Ok "three", Err 4],
                only_successful_results = λr →
                    filter_map (unwrap_or_else (λy → None)) (map (λx → map Some x) r),
            in
                only_successful_results results
            "#,
        )
        .await
        .unwrap();
        // FIXME(peter): let bindings are not properly generalized in this case
        // assert_eq!(res_type, list!(string!()));
        assert_expr_eq!(res, l!(s!("one"), s!("three")); ignore span);

        let (res, _res_type) = parse_infer_and_eval(
            r#"
            let
                results = [Ok "one", Err 2, Ok "three", Err 4],
                result_to_option = (λx → unwrap_or_else (λy → None) (map Some x)),
                only_successful_results = λr → filter_map result_to_option r
            in
                only_successful_results results
            "#,
        )
        .await
        .unwrap();
        // FIXME(peter): let bindings are not properly generalized in this case
        // assert_eq!(res_type, list!(string!()));
        assert_expr_eq!(res, l!(s!("one"), s!("three")); ignore span);
    }

    #[tokio::test]
    async fn test_regex_utils() {
        // Test for basic Regex parsing using capture gros
        let (res, res_type) = parse_infer_and_eval(r#"regex_captures "\d+" "111a222bc444""#)
            .await
            .unwrap();
        assert_eq!(res_type, list!(string!()));
        assert_expr_eq!(res, l!(s!("111"), s!("222"), s!("444")); ignore span);

        // Test for basic Regex parsing using simple is_match funnc
        let (res, _res_type) = parse_infer_and_eval(
            r#"regex_matches "\b\w{13}\b" "I categorically deny having triskaidekaphobia." "#,
        )
        .await
        .unwrap();
        assert_expr_eq!(res, b!(true));

        // Check error handling
        assert!(
            parse_infer_and_eval(r#"regex_matches "[[" "hi there, how are you""#)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn test_compose() {
        let (res, res_type) = parse_infer_and_eval(r#"(((*) 6.9) . ((+) 42.0)) 3.14"#)
            .await
            .unwrap();
        assert_eq!(res_type, float!());
        assert_expr_eq!(res, f!(311.466); ignore span);

        let (res, res_type) =
            parse_infer_and_eval(r#"let f = ((*) 6.9), g = ((+) 42.0) in (f . g) 3.14"#)
                .await
                .unwrap();
        assert_eq!(res_type, float!());
        assert_expr_eq!(res, f!(311.466); ignore span);

        let (res, res_type) = parse_infer_and_eval(r#"((λx → x * 6.9) . (λx → x + 42.0)) 3.14"#)
            .await
            .unwrap();
        assert_eq!(res_type, float!());
        assert_expr_eq!(res, f!(311.466); ignore span);

        let (res, res_type) =
            parse_infer_and_eval(r#"let f = (λx → x * 6.9), g = (λx → x + 42.0) in (f . g) 3.14"#)
                .await
                .unwrap();
        assert_eq!(res_type, float!());
        assert_expr_eq!(res, f!(311.466); ignore span);
    }

    #[tokio::test]
    async fn test_map_map() {
        let (res, res_type) =
            parse_infer_and_eval(r#"let f = (λx → -x) in map f (map f [3.14, 6.9, 42.0, 1.0])"#)
                .await
                .unwrap();
        assert_eq!(res_type, list!(float!()));
        assert_expr_eq!(res, l!(f!(3.14), f!(6.9), f!(42.0), f!(1.0)); ignore span);
    }

    #[tokio::test]
    async fn test_map_extensive() {
        let (res, res_type) = parse_infer_and_eval(
            r#"
                map
                    (let
                        g = λx → 2.0 * (identity x) - x
                    in
                        g)
                    (let
                        f = λx → -(identity x),
                        h = map f (map f [-1, -2, -3, -4])
                    in
                        map
                            f
                            (map
                                (λx → f (identity x))
                                [3.28 - 0.14, identity 6.9, (λx → x) 42.0, f (f 1.0)]))
                "#,
        )
        .await
        .unwrap();
        assert_eq!(res_type, list!(float!()));
        assert_expr_eq!(res, l!(f!(3.1399999999999997), f!(6.9), f!(42.0), f!(1.0)); ignore span);
    }

    #[tokio::test]
    async fn test_fold() {
        let (res, res_type) = parse_infer_and_eval(r#"foldl (-) 200.0 [100.0, 40.0, 8.0, 3.0]"#)
            .await
            .unwrap();
        assert_eq!(res_type, float!());
        assert_expr_eq!(res, f!(49.0); ignore span);

        let (res, res_type) = parse_infer_and_eval(r#"foldr (-) 200.0 [100.0, 40.0, 8.0, 3.0]"#)
            .await
            .unwrap();
        assert_eq!(res_type, float!());
        assert_expr_eq!(res, f!(265.0); ignore span);
    }

    #[tokio::test]
    async fn test_lambda_let_in_var() {
        let (res, res_type) = parse_infer_and_eval(r#"(λx → let y = identity x in y + y) 6.9"#)
            .await
            .unwrap();
        assert_eq!(res_type, float!());
        assert_expr_eq!(res, f!(13.8); ignore span);
    }

    #[tokio::test]
    async fn test_lambda_scope() {
        let (res, res_type) = parse_infer_and_eval(
            r#"
            let foo = [1, 2, 3] in map (\foo -> negate foo) foo
            "#,
        )
        .await
        .unwrap();
        assert_eq!(res_type, list!(int!()));
        assert_expr_eq!(res, l!(i!(-1), i!(-2), i!(-3)); ignore span);

        let (res, res_type) = parse_infer_and_eval(
            r#"
            let foo = ["one"] in (\foo -> negate foo) 2
            "#,
        )
        .await
        .unwrap();
        assert_eq!(res_type, int!());
        assert_expr_eq!(res, i!(-2); ignore span);

        let (res, res_type) = parse_infer_and_eval(
            r#"
            let
                foo = [(1, "one"), (2, "two")],
            in
                map (\foo -> (elem0 foo, elem1 foo)) foo
            "#,
        )
        .await
        .unwrap();
        assert_eq!(res_type, list!(tuple!(uint!(), string!())));
        assert_expr_eq!(res, l!(tup!(u!(1), s!("one")), tup!(u!(2), s!("two"))); ignore span);

        let (res, res_type) = parse_infer_and_eval(
            r#"
            let
                a = 10,
                f = let
                        a = 20,
                    in
                        (\x -> a + x),
            in
                f 3
            "#,
        )
        .await
        .unwrap();
        assert_eq!(res_type, uint!());
        assert_expr_eq!(res, u!(23); ignore span);
    }

    /// This test is meant to reflect the kind of usage pattern that we see in
    /// production code. However, it should not be taken as a comprehensive.
    #[tokio::test]
    async fn test_big_boy() {
        let (res, res_type) = parse_infer_and_eval(
            r#"
            (λxs ys zs →
                let
                    t = map (λx → ((get 0 xs) * x)) xs,
                    u = ys ++ [420],
                    v = take 2 zs,
                    f = (λx →
                        let
                            a = (identity x)
                        in
                            a + a
                    ),
                    g = (++) ((++) xs t)
                in
                    zip (g xs) [[u], v]
            ) [2.0, 3.0, 4.0] [4, 5, 6] [[6, 7, 8], [9, 10, 11], [12, 13, 14]]
            "#,
        )
        .await
        .unwrap();
        assert_eq!(res_type, list!(tuple!(float!(), list!(list!(uint!())))));
        assert_expr_eq!(
            res,
            l![
                tup!(f!(2.0), l![l![u!(4), u!(5), u!(6), u!(420)]]),
                tup!(f!(3.0), l![l![u!(6), u!(7), u!(8)], l![u!(9), u!(10), u!(11)]])
            ];
            ignore span
        );
    }

    #[tokio::test]
    async fn test_elem() {
        let (res, res_type) = parse_infer_and_eval(
            r#"
            let
                tuple = (3, 1.5, "test", true) in
            {
                field0 = (elem0 tuple),
                field1 = (elem1 tuple),
                field2 = (elem2 tuple),
                field3 = (elem3 tuple),
            }
        "#,
        )
        .await
        .unwrap();

        assert_eq!(
            res_type,
            dict! {
                field0: uint!(),
                field1: float!(),
                field2: string!(),
                field3: bool!(),
            }
        );

        assert_expr_eq!(
            res,
            d!{
                field0 = u!(3),
                field1 = f!(1.5),
                field2 = s!("test"),
                field3 = b!(true),
            };
            ignore span);

        let (res, res_type) = parse_infer_and_eval(
            r#"
            let
                two = (1.5, 2),
                three = (2, true, "hello"),
                four = (false, "two", 1.5, 2),
            in
                (elem1 two, elem1 three, elem1 four)
            "#,
        )
        .await
        .unwrap();
        assert_eq!(res_type, tuple!(uint!(), bool!(), string!()));
        assert_expr_eq!(res, tup!(u!(2), b!(true), s!("two")); ignore span);
    }

    #[tokio::test]
    async fn test_range() {
        let (res, res_type) = parse_infer_and_eval(r#"list_range 5 9 None"#)
            .await
            .unwrap();
        assert_eq!(res_type, list!(uint!()));
        assert_expr_eq!(res,l!{u!(5), u!(6), u!(7), u!(8)}; ignore span);

        let (res, res_type) = parse_infer_and_eval(r#"list_range 5 16 (Some 3)"#)
            .await
            .unwrap();
        assert_eq!(res_type, list!(uint!()));
        assert_expr_eq!(res,l!{u!(5), u!(8), u!(11), u!(14)}; ignore span);
    }

    #[tokio::test]
    async fn test_map_range() {
        let (res, res_type) = parse_infer_and_eval(r#"map (λx → 2 * x) (list_range 0 4 None)"#)
            .await
            .unwrap();
        assert_eq!(res_type, list!(uint!()));
        assert_expr_eq!(res,l!{u!(0), u!(2), u!(4), u!(6)}; ignore span);
    }

    #[tokio::test]
    async fn test_flatten() {
        let (res, res_type) = parse_infer_and_eval(r#"flatten [[1, 2, 3], [], [4, 5]]"#)
            .await
            .unwrap();
        assert_eq!(res_type, list!(uint!()));
        assert_expr_eq!(res, l!(u!(1), u!(2), u!(3), u!(4), u!(5)); ignore span);
    }

    #[tokio::test]
    async fn test_list_min() {
        let (res, res_type) = parse_infer_and_eval(r#"list_min [5, 4, 6, 3, 2, 7, 9]"#)
            .await
            .unwrap();
        assert_eq!(res_type, uint!());
        assert_expr_eq!(res, u!(2); ignore span);

        let (res, res_type) =
            parse_infer_and_eval(r#"list_min [int 5, int 4, (int 0) - (int 6), int 3, int 2]"#)
                .await
                .unwrap();
        assert_eq!(res_type, int!());
        assert_expr_eq!(res, i!(-6); ignore span);

        let (res, res_type) = parse_infer_and_eval(r#"list_min [5.0, 4.0, -6.5, 9.5, 2.0]"#)
            .await
            .unwrap();
        assert_eq!(res_type, float!());
        assert_expr_eq!(res, f!(-6.5); ignore span);
    }

    #[tokio::test]
    async fn test_list_max() {
        let (res, res_type) = parse_infer_and_eval(r#"list_max [5, 4, 6, 3, 2, 7, 9]"#)
            .await
            .unwrap();
        assert_eq!(res_type, uint!());
        assert_expr_eq!(res, u!(9); ignore span);

        let (res, res_type) =
            parse_infer_and_eval(r#"list_max [int 5, int 4, (int 0) - (int 6), int 3, int 2]"#)
                .await
                .unwrap();
        assert_eq!(res_type, int!());
        assert_expr_eq!(res, i!(5); ignore span);

        let (res, res_type) = parse_infer_and_eval(r#"list_max [5.0, 4.0, -6.5, 9.5, 2.0]"#)
            .await
            .unwrap();
        assert_eq!(res_type, float!());
        assert_expr_eq!(res, f!(9.5); ignore span);
    }

    #[tokio::test]
    async fn test_list_sum() {
        let (res, res_type) = parse_infer_and_eval(r#"list_sum [5, 4, 6, 3, 2, 7, 9]"#)
            .await
            .unwrap();
        assert_eq!(res_type, uint!());
        assert_expr_eq!(res, u!(36); ignore span);

        let (res, res_type) =
            parse_infer_and_eval(r#"list_sum [int 5, int 4, (int 0) - (int 60), int 3, int 2]"#)
                .await
                .unwrap();
        assert_eq!(res_type, int!());
        assert_expr_eq!(res, i!(-46); ignore span);

        let (res, res_type) = parse_infer_and_eval(r#"list_sum [5.0, 4.0, -60.5, 9.5, 2.5]"#)
            .await
            .unwrap();
        assert_eq!(res_type, float!());
        assert_expr_eq!(res, f!(-39.5); ignore span);
    }

    #[tokio::test]
    async fn test_list_avg() {
        let (res, res_type) = parse_infer_and_eval(r#"list_avg [3, 9, 6, 4]"#)
            .await
            .unwrap();
        assert_eq!(res_type, float!());
        assert_expr_eq!(res, f!(5.5); ignore span);

        let (res, res_type) =
            parse_infer_and_eval(r#"list_avg [int 3, int 9, (int 0) - (int 60), int 10]"#)
                .await
                .unwrap();
        assert_eq!(res_type, float!());
        assert_expr_eq!(res, f!(-9.5); ignore span);

        let (res, res_type) = parse_infer_and_eval(r#"list_avg [3.0, 9.5, 5.5, 4.0]"#)
            .await
            .unwrap();
        assert_eq!(res_type, float!());
        assert_expr_eq!(res, f!(5.5); ignore span);
    }

    #[tokio::test]
    async fn test_list_max_by() {
        // Test with int keys
        let (res, res_type) = parse_infer_and_eval(
            r#"
            list_max_by (\x -> elem0 x) [(int 5, "five"), (int -6, "neg"), (int 9, "nine"), (int 3, "three")]
            "#,
        )
        .await
        .unwrap();
        assert_eq!(res_type, tuple!(int!(), string!()));
        assert_expr_eq!(res, tup!(i!(9), s!("nine")); ignore span);

        // Test with float keys
        let (res, res_type) = parse_infer_and_eval(
            r#"
            list_max_by (\x -> elem0 x) [(5.0, "five"), (9.5, "nine"), (-6.5, "neg"), (3.0, "three")]
            "#,
        )
        .await
        .unwrap();
        assert_eq!(res_type, tuple!(float!(), string!()));
        assert_expr_eq!(res, tup!(f!(9.5), s!("nine")); ignore span);

        // Test with more complex key function
        let (res, res_type) = parse_infer_and_eval(
            r#"
            list_max_by (\x -> elem1 x) [("a", 5.0), ("b", 9.5), ("c", -6.5), ("d", 3.0)]
            "#,
        )
        .await
        .unwrap();
        assert_eq!(res_type, tuple!(string!(), float!()));
        assert_expr_eq!(res, tup!(s!("b"), f!(9.5)); ignore span);
    }

    #[tokio::test]
    async fn test_list_min_by() {
        // Test with int keys
        let (res, res_type) = parse_infer_and_eval(
            r#"
            list_min_by (\x -> elem0 x) [(int 5, "five"), (int -6, "neg"), (int 9, "nine"), (int 3, "three")]
            "#,
        )
        .await
        .unwrap();
        assert_eq!(res_type, tuple!(int!(), string!()));
        assert_expr_eq!(res, tup!(i!(-6), s!("neg")); ignore span);

        // Test with float keys
        let (res, res_type) = parse_infer_and_eval(
            r#"
            list_min_by (\x -> elem0 x) [(5.0, "five"), (9.5, "nine"), (-6.5, "neg"), (3.0, "three")]
            "#,
        )
        .await
        .unwrap();
        assert_eq!(res_type, tuple!(float!(), string!()));
        assert_expr_eq!(res, tup!(f!(-6.5), s!("neg")); ignore span);

        // Test with more complex key function
        let (res, res_type) = parse_infer_and_eval(
            r#"
            list_min_by (\x -> elem1 x) [("a", 5.0), ("b", 9.5), ("c", -6.5), ("d", 3.0)]
            "#,
        )
        .await
        .unwrap();
        assert_eq!(res_type, tuple!(string!(), float!()));
        assert_expr_eq!(res, tup!(s!("c"), f!(-6.5)); ignore span);
    }

    #[tokio::test]
    async fn test_match_result() {
        let (res, res_type) = parse_infer_and_eval(
            r#"
                let
                    a = Ok 3,
                    b = Err "foo",
                    c = Ok 4,
                    d = Err "bar",
                in
                    map
                    (\r ->
                        match_result r
                            (\value -> "True " ++ (string value))
                            (\error -> "False " ++ error)
                    )
                    [a, b, c, d]
            "#,
        )
        .await
        .unwrap();
        assert_eq!(res_type, list!(string!()));
        assert_expr_eq!(res, l!(s!("True 3"), s!("False foo"), s!("True 4"), s!("False bar")); ignore span);
    }

    #[tokio::test]
    async fn test_match_option() {
        let (res, res_type) = parse_infer_and_eval(
            r#"
                let
                    a = Some 3,
                    b = None,
                    c = Some 4,
                    d = None,
                in
                    map
                    (\r ->
                        match_option r
                            (\value -> "True " ++ (string value))
                            (\ignore -> "False")
                    )
                    [a, b, c, d]
            "#,
        )
        .await
        .unwrap();
        assert_eq!(res_type, list!(string!()));
        assert_expr_eq!(res, l!(s!("True 3"), s!("False"), s!("True 4"), s!("False")); ignore span);
    }
}

use std::sync::Arc;

use rex_ast::{
    app, assert_expr_eq, b, d,
    expr::{Decl, Expr, ImportClause, ImportPath, Pattern, Scope, TypeExpr, Var, intern},
    f, l, s, tup, u, v,
};
use rex_lexer::{Token, span, span::Span};
use rex_parser::error::ParserErr;

use rex_parser::{Parser, ParserLimits};
use rex_util::GasMeter;

fn parse(code: &str) -> Arc<Expr> {
    let mut parser = Parser::new(Token::tokenize(code).unwrap());
    parser.parse_program(&mut GasMeter::default()).unwrap().expr
}

fn lam(param: &str, body: Arc<Expr>) -> Arc<Expr> {
    Arc::new(Expr::Lam(
        Span::default(),
        Scope::new_sync(),
        Var::new(param),
        None,
        Vec::new(),
        body,
    ))
}

#[test]
fn test_parse_comment() {
    let mut parser = Parser::new(Token::tokenize("true {- this is a boolean -}").unwrap());
    let expr = parser.parse_program(&mut GasMeter::default()).unwrap().expr;
    assert_expr_eq!(expr, b!(span!(1:1 - 1:5); true));

    let mut parser = Parser::new(Token::tokenize("{- this is a boolean -} false").unwrap());
    let expr = parser.parse_program(&mut GasMeter::default()).unwrap().expr;
    assert_expr_eq!(expr, b!(span!(1:25 - 1:30); false));

    let mut parser = Parser::new(
        Token::tokenize(
            "(3.54 {- this is a float -}, {- this is an int -} 42, false {- this is a boolean -})",
        )
        .unwrap(),
    );
    let expr = parser.parse_program(&mut GasMeter::default()).unwrap().expr;
    assert_expr_eq!(
        expr,
        tup!(
            span!(1:1 - 1:85);
            f!(span!(1:2 - 1:6); 3.54),
            u!(span!(1:51 - 1:53); 42),
            b!(span!(1:55 - 1:60); false),
        )
    );
}

#[test]
fn test_max_nesting_depth_is_enforced_during_parse() {
    let code = format!("{}0{}", "(".repeat(6), ")".repeat(6));
    let mut parser = Parser::new(Token::tokenize(&code).unwrap());
    parser.set_limits(ParserLimits {
        max_nesting: Some(5),
    });

    let errs = parser.parse_program(&mut GasMeter::default()).unwrap_err();
    assert!(
        errs.iter()
            .any(|e| e.to_string().contains("maximum nesting depth exceeded")),
        "expected a max-nesting parse error, got: {errs:?}"
    );
}

#[test]
fn test_max_nesting_binary_chain() {
    let code = std::iter::repeat_n("1", 12).collect::<Vec<_>>().join(" + ");
    let mut parser = Parser::new(Token::tokenize(&code).unwrap());
    parser.set_limits(ParserLimits {
        max_nesting: Some(5),
    });

    let errs = parser.parse_program(&mut GasMeter::default()).unwrap_err();
    assert!(
        errs.iter()
            .any(|e| e.to_string().contains("maximum nesting depth exceeded")),
        "expected a max-nesting parse error, got: {errs:?}"
    );
}

#[test]
fn test_max_nesting_type_fun_chain() {
    let ty_chain = std::iter::repeat_n("a", 12)
        .collect::<Vec<_>>()
        .join(" -> ");
    let code = format!("let t: {ty_chain} = x in t");
    let mut parser = Parser::new(Token::tokenize(&code).unwrap());
    parser.set_limits(ParserLimits {
        max_nesting: Some(5),
    });

    let errs = parser.parse_program(&mut GasMeter::default()).unwrap_err();
    assert!(
        errs.iter()
            .any(|e| e.to_string().contains("maximum nesting depth exceeded")),
        "expected a max-nesting parse error, got: {errs:?}"
    );
}

#[test]
fn test_max_nesting_cons_pattern_chain() {
    let pattern = (1..=12)
        .map(|i| format!("x{i}"))
        .collect::<Vec<_>>()
        .join(" :: ");
    let code = format!("match xs when {pattern} -> xs");
    let mut parser = Parser::new(Token::tokenize(&code).unwrap());
    parser.set_limits(ParserLimits {
        max_nesting: Some(5),
    });

    let errs = parser.parse_program(&mut GasMeter::default()).unwrap_err();
    assert!(
        errs.iter()
            .any(|e| e.to_string().contains("maximum nesting depth exceeded")),
        "expected a max-nesting parse error, got: {errs:?}"
    );
}

#[test]
fn test_add() {
    let mut parser = Parser::new(Token::tokenize("1 + 2").unwrap());
    let expr = parser.parse_program(&mut GasMeter::default()).unwrap().expr;
    assert_expr_eq!(
        expr,
        app!(
            span!(1:1 - 1:6);
            app!(
                span!(1:1 - 1:4);
                v!(span!(1:3 - 1:4); "+"),
                u!(span!(1:1 - 1:2); 1)
            ),
            u!(span!(1:5 - 1:6); 2)
        )
    );

    let mut parser = Parser::new(Token::tokenize("(6.9 + 3.17)").unwrap());
    let expr = parser.parse_program(&mut GasMeter::default()).unwrap().expr;
    assert_expr_eq!(
        expr,
        app!(
            span!(1:1 - 1:13);
            app!(
                span!(1:2 - 1:7);
                v!(span!(1:6 - 1:7); "+"),
                f!(span!(1:2 - 1:5); 6.9)
            ),
            f!(span!(1:8 - 1:12); 3.17)
        )
    );

    let mut parser = Parser::new(Token::tokenize("(+) 420").unwrap());
    let expr = parser.parse_program(&mut GasMeter::default()).unwrap().expr;
    assert_expr_eq!(
        expr,
        app!(
            span!(1:1 - 1:8);
            v!(span!(1:1 - 1:4); "+"),
            u!(span!(1:5 - 1:8); 420)
        )
    );
}

#[test]
fn test_parse_type_decl() {
    let code = r#"
    type MyADT a b c = MyCtor1 | MyCtor2 a b | MyCtor3 { field1: c }
    42
    "#;
    let mut parser = Parser::new(Token::tokenize(code).unwrap());
    let program = parser.parse_program(&mut GasMeter::default()).unwrap();
    assert_eq!(program.decls.len(), 1);
    match &program.decls[0] {
        Decl::Type(decl) => {
            assert_eq!(decl.name, intern("MyADT"));
            assert_eq!(decl.params, vec![intern("a"), intern("b"), intern("c")]);
            assert_eq!(decl.variants.len(), 3);
            assert_eq!(decl.variants[0].name, intern("MyCtor1"));
            assert!(decl.variants[0].args.is_empty());
            assert_eq!(decl.variants[1].name, intern("MyCtor2"));
            assert_eq!(decl.variants[1].args.len(), 2);
            assert_eq!(decl.variants[2].name, intern("MyCtor3"));
            match &decl.variants[2].args[0] {
                TypeExpr::Record(_, fields) => {
                    assert_eq!(fields.len(), 1);
                    assert_eq!(fields[0].0, intern("field1"));
                    assert!(matches!(
                        fields[0].1,
                        TypeExpr::Name(_, ref n) if n.as_ref() == "c"
                    ));
                }
                other => panic!("expected record type, got {other:?}"),
            }
        }
        other => panic!("expected type decl, got {other:?}"),
    }
    assert_expr_eq!(program.expr, u!(span!(3:5 - 3:7); 42));
}

#[test]
fn test_parse_fn_decl_simple() {
    let code = r#"
    fn add x: i32 -> y: i32 -> i32 = x + y
    add 1 2
    "#;
    let mut parser = Parser::new(Token::tokenize(code).unwrap());
    let program = parser.parse_program(&mut GasMeter::default()).unwrap();
    assert_eq!(program.decls.len(), 1);
    match &program.decls[0] {
        Decl::Fn(fd) => {
            assert_eq!(fd.name.name, intern("add"));
            assert_eq!(fd.params.len(), 2);
            assert_eq!(fd.params[0].0.name, intern("x"));
            assert!(matches!(
                fd.params[0].1,
                TypeExpr::Name(_, ref n) if n.as_ref() == "i32"
            ));
            assert_eq!(fd.params[1].0.name, intern("y"));
            assert!(matches!(
                fd.params[1].1,
                TypeExpr::Name(_, ref n) if n.as_ref() == "i32"
            ));
            assert!(matches!(fd.ret, TypeExpr::Name(_, ref n) if n.as_ref() == "i32"));
            assert!(fd.constraints.is_empty());
        }
        other => panic!("expected fn decl, got {other:?}"),
    }
}

#[test]
fn test_parse_fn_decl_signature_form_with_lambda_body() {
    let code = r#"
    fn add : i32 -> i32 -> i32 = \x y -> x + y
    add 1 2
    "#;
    let mut parser = Parser::new(Token::tokenize(code).unwrap());
    let program = parser.parse_program(&mut GasMeter::default()).unwrap();
    assert_eq!(program.decls.len(), 1);
    match &program.decls[0] {
        Decl::Fn(fd) => {
            assert_eq!(fd.name.name, intern("add"));
            assert_eq!(fd.params.len(), 2);
            assert_eq!(fd.params[0].0.name, intern("x"));
            assert!(matches!(
                fd.params[0].1,
                TypeExpr::Name(_, ref n) if n.as_ref() == "i32"
            ));
            assert_eq!(fd.params[1].0.name, intern("y"));
            assert!(matches!(
                fd.params[1].1,
                TypeExpr::Name(_, ref n) if n.as_ref() == "i32"
            ));
            assert!(matches!(fd.ret, TypeExpr::Name(_, ref n) if n.as_ref() == "i32"));
            assert!(fd.constraints.is_empty());
            assert!(!matches!(fd.body.as_ref(), Expr::Lam(..)));
        }
        other => panic!("expected fn decl, got {other:?}"),
    }
}

#[test]
fn test_parse_fn_sig_multiline_lambda() {
    let code = r#"
    fn f : i32 -> i32 = \x ->
      x + 1
    f 1
    "#;
    let mut parser = Parser::new(Token::tokenize(code).unwrap());
    let program = parser.parse_program(&mut GasMeter::default()).unwrap();
    assert_eq!(program.decls.len(), 1);
    match &program.decls[0] {
        Decl::Fn(fd) => {
            assert_eq!(fd.name.name, intern("f"));
            assert_eq!(fd.params.len(), 1);
            assert_eq!(fd.params[0].0.name, intern("x"));
            assert!(matches!(
                fd.params[0].1,
                TypeExpr::Name(_, ref n) if n.as_ref() == "i32"
            ));
            assert!(matches!(fd.ret, TypeExpr::Name(_, ref n) if n.as_ref() == "i32"));
        }
        other => panic!("expected fn decl, got {other:?}"),
    }
}

#[test]
fn test_parse_fn_decl_signature_form_eta_expands_non_lambda_body() {
    let code = r#"
    fn inc : i32 -> i32 = add 1
    inc
    "#;
    let mut parser = Parser::new(Token::tokenize(code).unwrap());
    let program = parser.parse_program(&mut GasMeter::default()).unwrap();
    assert_eq!(program.decls.len(), 1);
    match &program.decls[0] {
        Decl::Fn(fd) => {
            assert_eq!(fd.name.name, intern("inc"));
            assert_eq!(fd.params.len(), 1);
            assert_eq!(fd.params[0].0.name, intern("_arg0"));
            assert!(matches!(
                fd.params[0].1,
                TypeExpr::Name(_, ref n) if n.as_ref() == "i32"
            ));
            assert!(matches!(fd.ret, TypeExpr::Name(_, ref n) if n.as_ref() == "i32"));
        }
        other => panic!("expected fn decl, got {other:?}"),
    }
}

#[test]
fn test_parse_fn_decl_signature_form_where_constraints() {
    let code = r#"
    fn my_fun : a -> b -> c where Iterable (a, b) = \x y -> x
    my_fun
    "#;
    let mut parser = Parser::new(Token::tokenize(code).unwrap());
    let program = parser.parse_program(&mut GasMeter::default()).unwrap();
    assert_eq!(program.decls.len(), 1);
    match &program.decls[0] {
        Decl::Fn(fd) => {
            assert_eq!(fd.name.name, intern("my_fun"));
            assert_eq!(fd.params.len(), 2);
            assert!(matches!(
                fd.constraints[0].class,
                ref n if n.as_ref() == "Iterable"
            ));
        }
        other => panic!("expected fn decl, got {other:?}"),
    }
}

#[test]
fn test_parse_fn_decl_signature_form_rejects_mismatched_lambda_arity() {
    let code = r#"
    fn add : i32 -> i32 -> i32 = \x -> x
    add
    "#;
    let mut parser = Parser::new(Token::tokenize(code).unwrap());
    assert!(parser.parse_program(&mut GasMeter::default()).is_err());
}

#[test]
fn test_parse_fn_decl_where_constraints() {
    let code = r#"
    fn my_fun x: a -> y: b -> c where Iterable (a, b) = x
    my_fun
    "#;
    let mut parser = Parser::new(Token::tokenize(code).unwrap());
    let program = parser.parse_program(&mut GasMeter::default()).unwrap();
    assert_eq!(program.decls.len(), 1);
    match &program.decls[0] {
        Decl::Fn(fd) => {
            assert_eq!(fd.name.name, intern("my_fun"));
            assert_eq!(fd.params.len(), 2);
            assert!(matches!(
                fd.constraints[0].class,
                ref n if n.as_ref() == "Iterable"
            ));
            match &fd.constraints[0].typ {
                TypeExpr::Tuple(_, elems) => {
                    assert_eq!(elems.len(), 2);
                    assert!(matches!(elems[0], TypeExpr::Name(_, ref n) if n.as_ref() == "a"));
                    assert!(matches!(elems[1], TypeExpr::Name(_, ref n) if n.as_ref() == "b"));
                }
                other => panic!("expected tuple constraint type, got {other:?}"),
            }
        }
        other => panic!("expected fn decl, got {other:?}"),
    }
}

#[test]
fn test_parse_declare_fn_decl_where_constraints() {
    let code = r#"
    declare fn my_fun x: a -> y: b -> c where Iterable (a, b)
    42
    "#;
    let mut parser = Parser::new(Token::tokenize(code).unwrap());
    let program = parser.parse_program(&mut GasMeter::default()).unwrap();
    assert_eq!(program.decls.len(), 1);
    match &program.decls[0] {
        Decl::DeclareFn(fd) => {
            assert_eq!(fd.name.name, intern("my_fun"));
            assert_eq!(fd.params.len(), 2);
            assert!(matches!(
                fd.constraints[0].class,
                ref n if n.as_ref() == "Iterable"
            ));
            match &fd.constraints[0].typ {
                TypeExpr::Tuple(_, elems) => {
                    assert_eq!(elems.len(), 2);
                    assert!(matches!(elems[0], TypeExpr::Name(_, ref n) if n.as_ref() == "a"));
                    assert!(matches!(elems[1], TypeExpr::Name(_, ref n) if n.as_ref() == "b"));
                }
                other => panic!("expected tuple constraint type, got {other:?}"),
            }
        }
        other => panic!("expected declare fn decl, got {other:?}"),
    }
    assert_expr_eq!(program.expr, u!(span!(3:5 - 3:7); 42));
}

#[test]
fn test_parse_declare_fn_decl_bare_signature() {
    let code = r#"
    declare fn info a -> string where Show a
    0
    "#;
    let mut parser = Parser::new(Token::tokenize(code).unwrap());
    let program = parser.parse_program(&mut GasMeter::default()).unwrap();
    assert_eq!(program.decls.len(), 1);
    match &program.decls[0] {
        Decl::DeclareFn(fd) => {
            assert_eq!(fd.name.name, intern("info"));
            assert_eq!(fd.params.len(), 1);
            assert!(matches!(
                fd.params[0].1,
                TypeExpr::Name(_, ref n) if n.as_ref() == "a"
            ));
            assert!(matches!(
                fd.ret,
                TypeExpr::Name(_, ref n) if n.as_ref() == "string"
            ));
            assert_eq!(fd.constraints.len(), 1);
            assert!(matches!(
                fd.constraints[0].class,
                ref n if n.as_ref() == "Show"
            ));
        }
        other => panic!("expected declare fn decl, got {other:?}"),
    }
}

#[test]
fn test_parse_declare_fn_decl_bare_signature_with_colon() {
    let code = r#"
    declare fn info : a -> string where Show a
    0
    "#;
    let mut parser = Parser::new(Token::tokenize(code).unwrap());
    let program = parser.parse_program(&mut GasMeter::default()).unwrap();
    assert_eq!(program.decls.len(), 1);
    match &program.decls[0] {
        Decl::DeclareFn(fd) => {
            assert_eq!(fd.name.name, intern("info"));
            assert_eq!(fd.params.len(), 1);
            assert!(matches!(
                fd.params[0].1,
                TypeExpr::Name(_, ref n) if n.as_ref() == "a"
            ));
            assert!(matches!(
                fd.ret,
                TypeExpr::Name(_, ref n) if n.as_ref() == "string"
            ));
            assert_eq!(fd.constraints.len(), 1);
            assert!(matches!(
                fd.constraints[0].class,
                ref n if n.as_ref() == "Show"
            ));
        }
        other => panic!("expected declare fn decl, got {other:?}"),
    }
}

#[test]
fn test_parse_declare_fn_decl_rejects_body() {
    let code = r#"
    declare fn my_fun x: a -> a = x
    0
    "#;
    let mut parser = Parser::new(Token::tokenize(code).unwrap());
    assert!(parser.parse_program(&mut GasMeter::default()).is_err());
}

#[test]
fn test_parse_fn_decl_param_fun_type_requires_parens() {
    let code = r#"
    fn apply x: (a -> c) -> y: a -> c = x y
    apply
    "#;
    let mut parser = Parser::new(Token::tokenize(code).unwrap());
    let program = parser.parse_program(&mut GasMeter::default()).unwrap();
    assert_eq!(program.decls.len(), 1);
    match &program.decls[0] {
        Decl::Fn(fd) => {
            assert_eq!(fd.name.name, intern("apply"));
            assert_eq!(fd.params.len(), 2);
            assert_eq!(fd.params[0].0.name, intern("x"));
            assert!(matches!(fd.params[0].1, TypeExpr::Fun(_, _, _)));
            assert_eq!(fd.params[1].0.name, intern("y"));
            assert!(matches!(
                fd.params[1].1,
                TypeExpr::Name(_, ref n) if n.as_ref() == "a"
            ));
            assert!(matches!(fd.ret, TypeExpr::Name(_, ref n) if n.as_ref() == "c"));
        }
        other => panic!("expected fn decl, got {other:?}"),
    }
}

#[test]
fn test_parse_fn_decl_parenthesized_params_allow_fun_types() {
    let code = r#"
    fn reduce (f: a -> a -> a) -> (x: t a) -> a = x
    reduce
    "#;
    let mut parser = Parser::new(Token::tokenize(code).unwrap());
    let program = parser.parse_program(&mut GasMeter::default()).unwrap();
    assert_eq!(program.decls.len(), 1);
    match &program.decls[0] {
        Decl::Fn(fd) => {
            assert_eq!(fd.name.name, intern("reduce"));
            assert_eq!(fd.params.len(), 2);
            assert_eq!(fd.params[0].0.name, intern("f"));
            assert!(matches!(fd.params[0].1, TypeExpr::Fun(..)));
            assert_eq!(fd.params[1].0.name, intern("x"));
            assert!(matches!(fd.params[1].1, TypeExpr::App(..)));
            assert!(matches!(fd.ret, TypeExpr::Name(_, ref n) if n.as_ref() == "a"));
        }
        other => panic!("expected fn decl, got {other:?}"),
    }
}

#[test]
fn test_parse_fn_decl_parenthesized_params_require_arrow_delimiter() {
    let code = r#"
    fn reduce (f: a -> a -> a) (x: t a) -> a = x
    reduce
    "#;
    let mut parser = Parser::new(Token::tokenize(code).unwrap());
    assert!(parser.parse_program(&mut GasMeter::default()).is_err());
}

#[test]
fn test_parse_unit_type() {
    let code = r#"
    fn unit_id x: () -> () = x
    unit_id ()
    "#;
    let mut parser = Parser::new(Token::tokenize(code).unwrap());
    let program = parser.parse_program(&mut GasMeter::default()).unwrap();
    assert_eq!(program.decls.len(), 1);
    match &program.decls[0] {
        Decl::Fn(fd) => {
            assert_eq!(fd.name.name, intern("unit_id"));
            assert_eq!(fd.params.len(), 1);
            assert!(matches!(fd.params[0].1, TypeExpr::Tuple(_, ref xs) if xs.is_empty()));
            assert!(matches!(fd.ret, TypeExpr::Tuple(_, ref xs) if xs.is_empty()));
        }
        other => panic!("expected fn decl, got {other:?}"),
    }
}

#[test]
fn test_sub() {
    let mut parser = Parser::new(Token::tokenize("1 - 2").unwrap());
    let expr = parser.parse_program(&mut GasMeter::default()).unwrap().expr;
    assert_expr_eq!(
        expr,
        app!(
            span!(1:1 - 1:6);
            app!(
                span!(1:1 - 1:4);
                v!(span!(1:3 - 1:4); "-"),
                u!(span!(1:1 - 1:2); 1)
            ),
            u!(span!(1:5 - 1:6); 2)
        )
    );

    let mut parser = Parser::new(Token::tokenize("(6.9 - 3.17)").unwrap());
    let expr = parser.parse_program(&mut GasMeter::default()).unwrap().expr;
    assert_expr_eq!(
        expr,
        app!(
            span!(1:1 - 1:13);
            app!(
                span!(1:2 - 1:7);
                v!(span!(1:6 - 1:7); "-"),
                f!(span!(1:2 - 1:5); 6.9)
            ),
            f!(span!(1:8 - 1:12); 3.17)
        )
    );

    let mut parser = Parser::new(Token::tokenize("(-) 4.20").unwrap());
    let expr = parser.parse_program(&mut GasMeter::default()).unwrap().expr;
    assert_expr_eq!(
        expr,
        app!(
            span!(1:1 - 1:9);
            v!(span!(1:1 - 1:4); "-"),
            f!(span!(1:5 - 1:9); 4.20)
        )
    );
}

#[test]
fn test_negate() {
    let mut parser = Parser::new(Token::tokenize("-1").unwrap());
    let expr = parser.parse_program(&mut GasMeter::default()).unwrap().expr;
    assert_expr_eq!(
        expr,
        app!(
            span!(1:1 - 1:3);
            v!(span!(1:1 - 1:2); "negate"),
            u!(span!(1:2 - 1:3); 1)
        )
    );

    let mut parser = Parser::new(Token::tokenize("(-1)").unwrap());
    let expr = parser.parse_program(&mut GasMeter::default()).unwrap().expr;
    assert_expr_eq!(
        expr,
        app!(
            span!(1:1 - 1:5);
            v!(span!(1:2 - 1:3); "negate"),
            u!(span!(1:3 - 1:4); 1)
        )
    );

    let mut parser = Parser::new(Token::tokenize("(- 6.9)").unwrap());
    let expr = parser.parse_program(&mut GasMeter::default()).unwrap().expr;
    assert_expr_eq!(
        expr,
        app!(
            span!(1:1 - 1:8);
            v!(span!(1:2 - 1:3); "negate"),
            f!(span!(1:4 - 1:7); 6.9)
        )
    );
}

#[test]
fn test_application_associativity() {
    let expr = parse("f x y z");
    let expected = app!(app!(app!(v!("f"), v!("x")), v!("y")), v!("z"));

    assert_expr_eq!(expr, expected; ignore span);
}

#[test]
fn test_projection_expr() {
    let expr = parse("x.field");
    let expected = Arc::new(Expr::Project(Span::default(), v!("x"), intern("field")));

    assert_expr_eq!(expr, expected; ignore span);
}

#[test]
fn test_projection_tuple_index_expr() {
    let expr = parse("x.0");
    let expected = Arc::new(Expr::Project(Span::default(), v!("x"), intern("0")));

    assert_expr_eq!(expr, expected; ignore span);
}

#[test]
fn test_projection_expr_colon_rejected() {
    let mut parser = Parser::new(Token::tokenize("x:field").unwrap());
    assert!(parser.parse_program(&mut GasMeter::default()).is_err());
}

#[test]
fn test_projection_binds_tighter_than_application() {
    let expr = parse("show p.x");
    let expected = app!(
        v!("show"),
        Arc::new(Expr::Project(Span::default(), v!("p"), intern("x")))
    );
    assert_expr_eq!(expr, expected; ignore span);
}

#[test]
fn test_projection_can_be_applied_without_parens() {
    let expr = parse("x.field y");
    let expected = app!(
        Arc::new(Expr::Project(Span::default(), v!("x"), intern("field"))),
        v!("y")
    );
    assert_expr_eq!(expr, expected; ignore span);
}

#[test]
fn test_list_cons_expr() {
    let expr = parse("x::xs");
    let expected = app!(app!(v!("Cons"), v!("x")), v!("xs"));
    assert_expr_eq!(expr, expected; ignore span);
}

#[test]
fn test_list_cons_expr_right_associative() {
    let expr = parse("x::y::zs");
    let expected = app!(
        app!(v!("Cons"), v!("x")),
        app!(app!(v!("Cons"), v!("y")), v!("zs"))
    );
    assert_expr_eq!(expr, expected; ignore span);
}

#[test]
fn test_list_cons_constructor_call_expr() {
    let expr = parse("Cons x xs");
    let expected = app!(app!(v!("Cons"), v!("x")), v!("xs"));
    assert_expr_eq!(expr, expected; ignore span);
}

#[test]
fn test_operator_precedence() {
    let expr = parse("1 + 2 * 3 - 4");
    let expected = app!(
        app!(v!("+"), u!(1)),
        app!(app!(v!("-"), app!(app!(v!("*"), u!(2)), u!(3))), u!(4))
    );

    assert_expr_eq!(expr, expected; ignore span);
}

#[test]
fn test_collections_and_tuples() {
    let expr = parse("([1, 2], { foo = \"bar\", baz = false }, (true, 9))");
    let expected = tup!(
        l!(u!(1), u!(2)),
        d!(foo = s!("bar"), baz = b!(false)),
        tup!(b!(true), u!(9))
    );

    assert_expr_eq!(expr, expected; ignore span);
}

#[test]
fn test_record_update_expr() {
    let expr = parse("{ foo with { x = 1, y = 2 } }");
    match expr.as_ref() {
        Expr::RecordUpdate(_, base, updates) => {
            assert_expr_eq!(base.clone(), v!("foo"); ignore span);
            assert_expr_eq!(updates.get(&intern("x")).unwrap().clone(), u!(1); ignore span);
            assert_expr_eq!(updates.get(&intern("y")).unwrap().clone(), u!(2); ignore span);
        }
        other => panic!("expected record update, got {other:?}"),
    }
}

#[test]
fn test_brace_expr_prefers_dict_literal() {
    let expr = parse("{ foo = 1 }");
    match expr.as_ref() {
        Expr::Dict(_, kvs) => {
            assert_eq!(kvs.len(), 1);
            assert_expr_eq!(kvs.get(&intern("foo")).unwrap().clone(), u!(1); ignore span);
        }
        other => panic!("expected dict literal, got {other:?}"),
    }
}

#[test]
fn test_record_update_empty_updates() {
    let expr = parse("{ foo with { } }");
    match expr.as_ref() {
        Expr::RecordUpdate(_, base, updates) => {
            assert_expr_eq!(base.clone(), v!("foo"); ignore span);
            assert!(updates.is_empty());
        }
        other => panic!("expected record update, got {other:?}"),
    }
}

#[test]
fn test_lambda_and_let_chain() {
    let expr = parse("let inc = \\x -> x + 1, dbl = \\x -> x * 2 in \\y -> inc (dbl y)");

    let inc = lam("x", app!(app!(v!("+"), v!("x")), u!(1)));
    let dbl = lam("x", app!(app!(v!("*"), v!("x")), u!(2)));
    let body = lam("y", app!(v!("inc"), app!(v!("dbl"), v!("y"))));

    let expected = Arc::new(Expr::Let(
        Span::default(),
        Var::new("inc"),
        None,
        inc,
        Arc::new(Expr::Let(Span::default(), Var::new("dbl"), None, dbl, body)),
    ));

    assert_expr_eq!(expr, expected; ignore span);
}

#[test]
fn test_let_rec_single_binding() {
    let expr = parse("let rec fact = \\n -> if n == 0 then 1 else n * fact (n - 1) in fact 5");
    match expr.as_ref() {
        Expr::LetRec(_, bindings, body) => {
            assert_eq!(bindings.len(), 1);
            let (name, ann, def) = &bindings[0];
            assert_eq!(name.name.as_ref(), "fact");
            assert!(ann.is_none());
            assert!(matches!(def.as_ref(), Expr::Lam(..)));
            assert_expr_eq!(body.clone(), app!(v!("fact"), u!(5)); ignore span);
        }
        other => panic!("expected let rec, got {other:?}"),
    }
}

#[test]
fn test_let_rec_mutual_bindings() {
    let expr = parse("let rec even = \\n -> odd n, odd = \\n -> even n in (even 0, odd 1)");
    match expr.as_ref() {
        Expr::LetRec(_, bindings, body) => {
            assert_eq!(bindings.len(), 2);
            assert_eq!(bindings[0].0.name.as_ref(), "even");
            assert_eq!(bindings[1].0.name.as_ref(), "odd");
            assert!(matches!(bindings[0].2.as_ref(), Expr::Lam(..)));
            assert!(matches!(bindings[1].2.as_ref(), Expr::Lam(..)));
            assert!(matches!(body.as_ref(), Expr::Tuple(..)));
        }
        other => panic!("expected let rec, got {other:?}"),
    }
}

#[test]
fn test_and_is_ident() {
    let expr = parse("let and = 1 in and");
    match expr.as_ref() {
        Expr::Let(_, var, _, def, body) => {
            assert_eq!(var.name.as_ref(), "and");
            assert_expr_eq!(def.clone(), u!(1); ignore span);
            assert_expr_eq!(body.clone(), v!("and"); ignore span);
        }
        other => panic!("expected let, got {other:?}"),
    }
}

#[test]
fn test_let_tuple_destructuring() {
    let expr = parse("let (x, y) = (1, 2) in x");

    let pat = Pattern::Tuple(
        Span::default(),
        vec![Pattern::Var(Var::new("x")), Pattern::Var(Var::new("y"))],
    );
    let expected = Arc::new(Expr::Match(
        Span::default(),
        tup!(u!(1), u!(2)),
        vec![(pat, v!("x"))],
    ));

    assert_expr_eq!(expr, expected; ignore span);
}

#[test]
fn test_type_annotations() {
    let expr = parse("let x: u8 = foo in x");
    match expr.as_ref() {
        Expr::Let(_, var, Some(TypeExpr::Name(_, name)), _def, _body) => {
            assert_eq!(var.name.as_ref(), "x");
            assert_eq!(name.as_ref(), "u8");
        }
        other => panic!("expected typed let, got {other:?}"),
    }

    let expr = parse("foo bar is u8");
    match expr.as_ref() {
        Expr::Ann(_, inner, TypeExpr::Name(_, name)) => {
            assert_eq!(name.as_ref(), "u8");
            assert!(matches!(inner.as_ref(), Expr::App(..)));
        }
        other => panic!("expected type assertion, got {other:?}"),
    }

    let expr = parse("foo is Sample.Correctness");
    match expr.as_ref() {
        Expr::Ann(_, inner, TypeExpr::Name(_, name)) => {
            assert_eq!(name.as_ref(), "Sample.Correctness");
            assert!(matches!(inner.as_ref(), Expr::Var(_)));
        }
        other => panic!("expected qualified type assertion, got {other:?}"),
    }

    let expr = parse("\\ (a : f32) -> a");
    match expr.as_ref() {
        Expr::Lam(_, _scope, param, Some(TypeExpr::Name(_, name)), constraints, body) => {
            assert_eq!(param.name.as_ref(), "a");
            assert_eq!(name.as_ref(), "f32");
            assert!(constraints.is_empty());
            assert!(matches!(body.as_ref(), Expr::Var(_)));
        }
        other => panic!("expected typed lambda, got {other:?}"),
    }

    let expr = parse("let t: f32 -> str -> Result bool str = x in t");
    match expr.as_ref() {
        Expr::Let(_, _var, Some(ann), _def, _body) => {
            fn is_name(expr: &TypeExpr, expected: &str) -> bool {
                matches!(expr, TypeExpr::Name(_, name) if name.as_ref() == expected)
            }

            match ann {
                TypeExpr::Fun(_, arg, ret) => {
                    assert!(is_name(arg, "f32"));
                    match ret.as_ref() {
                        TypeExpr::Fun(_, arg2, ret2) => {
                            assert!(is_name(arg2, "str"));
                            match ret2.as_ref() {
                                TypeExpr::App(_, fun, arg3) => {
                                    match fun.as_ref() {
                                        TypeExpr::App(_, fun2, arg2) => {
                                            assert!(is_name(fun2, "Result"));
                                            assert!(is_name(arg2, "bool"));
                                        }
                                        _ => panic!("expected Result bool str"),
                                    }
                                    assert!(is_name(arg3, "str"));
                                }
                                _ => panic!("expected Result bool str"),
                            }
                        }
                        _ => panic!("expected f32 -> str -> Result bool str"),
                    }
                }
                _ => panic!("expected function type annotation"),
            }
        }
        other => panic!("expected typed let, got {other:?}"),
    }
}

#[test]
fn test_match_named_patterns() {
    let expr = parse("match named when Ok x -> x when Err e -> e when _ -> default");
    let expected = Arc::new(Expr::Match(
        Span::default(),
        v!("named"),
        vec![
            (
                Pattern::Named(
                    Span::default(),
                    "Ok".into(),
                    vec![Pattern::Var(Var::new("x"))],
                ),
                v!("x"),
            ),
            (
                Pattern::Named(
                    Span::default(),
                    "Err".into(),
                    vec![Pattern::Var(Var::new("e"))],
                ),
                v!("e"),
            ),
            (Pattern::Wildcard(Span::default()), v!("default")),
        ],
    ));

    assert_expr_eq!(expr, expected; ignore span);
}

#[test]
fn test_match_list_patterns() {
    let expr = parse(
        "match list when [] -> empty when [x] -> x when [x, y, z] -> z when x::xs -> xs when _ -> fallback",
    );
    let expected = Arc::new(Expr::Match(
        Span::default(),
        v!("list"),
        vec![
            (Pattern::List(Span::default(), vec![]), v!("empty")),
            (
                Pattern::List(Span::default(), vec![Pattern::Var(Var::new("x"))]),
                v!("x"),
            ),
            (
                Pattern::List(
                    Span::default(),
                    vec![
                        Pattern::Var(Var::new("x")),
                        Pattern::Var(Var::new("y")),
                        Pattern::Var(Var::new("z")),
                    ],
                ),
                v!("z"),
            ),
            (
                Pattern::Cons(
                    Span::default(),
                    Box::new(Pattern::Var(Var::new("x"))),
                    Box::new(Pattern::Var(Var::new("xs"))),
                ),
                v!("xs"),
            ),
            (Pattern::Wildcard(Span::default()), v!("fallback")),
        ],
    ));

    assert_expr_eq!(expr, expected; ignore span);
}

#[test]
fn test_match_nested_patterns() {
    let expr = parse("match t when Cons x (Cons _ xs) -> xs when Pair (Just a) (Just b) -> a");
    let expected = Arc::new(Expr::Match(
        Span::default(),
        v!("t"),
        vec![
            (
                Pattern::Named(
                    Span::default(),
                    "Cons".into(),
                    vec![
                        Pattern::Var(Var::new("x")),
                        Pattern::Named(
                            Span::default(),
                            "Cons".into(),
                            vec![
                                Pattern::Wildcard(Span::default()),
                                Pattern::Var(Var::new("xs")),
                            ],
                        ),
                    ],
                ),
                v!("xs"),
            ),
            (
                Pattern::Named(
                    Span::default(),
                    "Pair".into(),
                    vec![
                        Pattern::Named(
                            Span::default(),
                            "Just".into(),
                            vec![Pattern::Var(Var::new("a"))],
                        ),
                        Pattern::Named(
                            Span::default(),
                            "Just".into(),
                            vec![Pattern::Var(Var::new("b"))],
                        ),
                    ],
                ),
                v!("a"),
            ),
        ],
    ));

    assert_expr_eq!(expr, expected; ignore span);
}

#[test]
fn test_match_dict_pattern() {
    let expr = parse("match obj when {foo, bar} -> foo bar");
    let expected = Arc::new(Expr::Match(
        Span::default(),
        v!("obj"),
        vec![(
            Pattern::Dict(
                Span::default(),
                vec![
                    ("foo".into(), Pattern::Var(Var::new("foo"))),
                    ("bar".into(), Pattern::Var(Var::new("bar"))),
                ],
            ),
            app!(v!("foo"), v!("bar")),
        )],
    ));

    assert_expr_eq!(expr, expected; ignore span);
}

#[test]
fn test_match_cons_associativity() {
    let expr = parse("match xs when h::t::u -> u");
    let expected = Arc::new(Expr::Match(
        Span::default(),
        v!("xs"),
        vec![(
            Pattern::Cons(
                Span::default(),
                Box::new(Pattern::Var(Var::new("h"))),
                Box::new(Pattern::Cons(
                    Span::default(),
                    Box::new(Pattern::Var(Var::new("t"))),
                    Box::new(Pattern::Var(Var::new("u"))),
                )),
            ),
            v!("u"),
        )],
    ));

    assert_expr_eq!(expr, expected; ignore span);
}

#[test]
fn test_match_wildcard_cons() {
    let expr = parse("match xs when (_::_) -> xs");
    let expected = Arc::new(Expr::Match(
        Span::default(),
        v!("xs"),
        vec![(
            Pattern::Cons(
                Span::default(),
                Box::new(Pattern::Wildcard(Span::default())),
                Box::new(Pattern::Wildcard(Span::default())),
            ),
            v!("xs"),
        )],
    ));

    assert_expr_eq!(expr, expected; ignore span);
}

#[test]
fn test_match_empty_dict_pattern() {
    let expr = parse("match obj when {} -> obj");
    let expected = Arc::new(Expr::Match(
        Span::default(),
        v!("obj"),
        vec![(Pattern::Dict(Span::default(), vec![]), v!("obj"))],
    ));

    assert_expr_eq!(expr, expected; ignore span);
}

#[test]
fn test_import_clause_all() {
    let code = "import foo.bar (*)\n()";
    let mut parser = Parser::new(Token::tokenize(code).unwrap());
    let program = parser.parse_program(&mut GasMeter::default()).unwrap();
    let Decl::Import(import) = &program.decls[0] else {
        panic!("expected import decl");
    };
    assert_eq!(import.alias, intern("bar"));
    assert!(matches!(import.clause, Some(ImportClause::All)));
}

#[test]
fn test_import_clause_items_with_alias() {
    let code = "import foo.bar (x, y as z)\n()";
    let mut parser = Parser::new(Token::tokenize(code).unwrap());
    let program = parser.parse_program(&mut GasMeter::default()).unwrap();
    let Decl::Import(import) = &program.decls[0] else {
        panic!("expected import decl");
    };
    assert_eq!(import.alias, intern("bar"));
    let Some(ImportClause::Items(items)) = &import.clause else {
        panic!("expected import items");
    };
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].name, intern("x"));
    assert_eq!(items[0].alias, None);
    assert_eq!(items[1].name, intern("y"));
    assert_eq!(items[1].alias, Some(intern("z")));
}

#[test]
fn test_import_clause_rejects_module_alias_combo() {
    let code = "import foo.bar (x) as Bar\n()";
    let mut parser = Parser::new(Token::tokenize(code).unwrap());
    let err = parser.parse_program(&mut GasMeter::default()).unwrap_err();
    assert!(
        err[0]
            .message
            .contains("cannot combine `as <alias>` with import clause")
    );
}

#[test]
fn test_import_clause_rejects_duplicate_local_names() {
    let code = "import foo.bar (x, y as x)\n()";
    let mut parser = Parser::new(Token::tokenize(code).unwrap());
    let err = parser.parse_program(&mut GasMeter::default()).unwrap_err();
    assert!(err[0].message.contains("duplicate imported name `x`"));
}

#[test]
fn test_import_relative_current_dir_path() {
    let code = "import ./foo/bar (x)\n()";
    let mut parser = Parser::new(Token::tokenize(code).unwrap());
    let program = parser.parse_program(&mut GasMeter::default()).unwrap();
    let Decl::Import(import) = &program.decls[0] else {
        panic!("expected import decl");
    };
    match &import.path {
        ImportPath::Local { segments, sha } => {
            assert_eq!(segments, &vec![intern("foo"), intern("bar")]);
            assert_eq!(sha, &None);
        }
        other => panic!("expected local import path, got {other:?}"),
    }
    assert_eq!(import.alias, intern("bar"));
}

#[test]
fn test_import_relative_parent_dir_path() {
    let code = "import ../../foo/bar as FB\n()";
    let mut parser = Parser::new(Token::tokenize(code).unwrap());
    let program = parser.parse_program(&mut GasMeter::default()).unwrap();
    let Decl::Import(import) = &program.decls[0] else {
        panic!("expected import decl");
    };
    match &import.path {
        ImportPath::Local { segments, sha } => {
            assert_eq!(
                segments,
                &vec![
                    intern("super"),
                    intern("super"),
                    intern("foo"),
                    intern("bar")
                ]
            );
            assert_eq!(sha, &None);
        }
        other => panic!("expected local import path, got {other:?}"),
    }
    assert_eq!(import.alias, intern("FB"));
}

#[test]
fn test_errors() {
    let mut parser = Parser::new(Token::tokenize("1 + 2 + in + 3").unwrap());
    let res = parser.parse_program(&mut GasMeter::default());
    assert_eq!(
        res,
        Err(vec![ParserErr::new(
            Span::new(1, 9, 1, 11),
            "unexpected in"
        )])
    );

    let mut parser = Parser::new(Token::tokenize("1 + 2 in + 3").unwrap());
    let res = parser.parse_program(&mut GasMeter::default());
    assert_eq!(
        res,
        Err(vec![ParserErr::new(Span::new(1, 7, 1, 9), "unexpected in")])
    );

    let mut parser = Parser::new(Token::tokenize("get 0 [    ").unwrap());
    let res = parser.parse_program(&mut GasMeter::default());
    assert_eq!(
        res,
        Err(vec![ParserErr::new(
            Span::new(1, 12, 1, 12),
            "unexpected EOF"
        )])
    );

    let mut parser = Parser::new(Token::tokenize("elem0 (  ").unwrap());
    let res = parser.parse_program(&mut GasMeter::default());
    assert_eq!(
        res,
        Err(vec![ParserErr::new(
            Span::new(1, 10, 1, 10),
            "unexpected EOF"
        )])
    );

    let mut parser = Parser::new(
        Token::tokenize(
            "
        { a = 1, b }
        { a = 1, b = 2, c }
        { a = 1, b = 3, c = 3, d }
        ",
        )
        .unwrap(),
    );
    let res = parser.parse_program(&mut GasMeter::default());
    assert_eq!(
        res,
        Err(vec![
            ParserErr::new(Span::new(2, 20, 2, 21), "expected `=`"),
            ParserErr::new(Span::new(3, 27, 3, 28), "expected `=`"),
            ParserErr::new(Span::new(4, 34, 4, 35), "expected `=`")
        ])
    );
}

#[test]
fn test_typeclass_where_is_optional() {
    let code = r#"
class Default a
    default : a

instance Default i32
    default = 0

default
"#;

    let mut parser = Parser::new(Token::tokenize(code).unwrap());
    let program = parser.parse_program(&mut GasMeter::default()).unwrap();
    assert_eq!(program.decls.len(), 2);

    match &program.decls[0] {
        Decl::Class(decl) => {
            assert_eq!(decl.name, intern("Default"));
            assert_eq!(decl.methods.len(), 1);
            assert_eq!(decl.methods[0].name, intern("default"));
        }
        other => panic!("expected class decl, got {other:?}"),
    }

    match &program.decls[1] {
        Decl::Instance(decl) => {
            assert_eq!(decl.class, intern("Default"));
            assert_eq!(decl.methods.len(), 1);
            assert_eq!(decl.methods[0].name, intern("default"));
        }
        other => panic!("expected instance decl, got {other:?}"),
    }
}

#[test]
fn test_typeclass_where_optional_does_not_force_method_block() {
    // Without `where`, an indented expression after a class/instance header
    // is not treated as a method block unless it looks like `name :` / `name =`.
    let code = r#"
class Marker a

instance Marker i32

true
"#;

    let mut parser = Parser::new(Token::tokenize(code).unwrap());
    let program = parser.parse_program(&mut GasMeter::default()).unwrap();
    assert_eq!(program.decls.len(), 2);
    assert!(matches!(program.expr.as_ref(), Expr::Bool(..)));
}

#[test]
fn test_parse_instance_with_qualified_class_name() {
    let code = r#"
instance Sample.Default i32
    default = 0

default
"#;

    let mut parser = Parser::new(Token::tokenize(code).unwrap());
    let program = parser.parse_program(&mut GasMeter::default()).unwrap();
    assert_eq!(program.decls.len(), 1);

    let Decl::Instance(decl) = &program.decls[0] else {
        panic!("expected instance decl");
    };
    assert_eq!(decl.class, intern("Sample.Default"));
    assert_eq!(decl.methods.len(), 1);
    assert_eq!(decl.methods[0].name, intern("default"));
}

#[test]
fn test_parse_top_level_hole_expr() {
    let expr = parse("?");
    assert!(matches!(expr.as_ref(), Expr::Hole(..)), "expr={expr:#?}");
}

#[test]
fn test_parse_hole_in_let_with_annotation() {
    let expr = parse("let x : i32 = ? in x");
    match expr.as_ref() {
        Expr::Let(_, _var, ann, def, body) => {
            assert!(ann.is_some(), "expected annotation");
            assert!(matches!(def.as_ref(), Expr::Hole(..)), "def={def:#?}");
            assert!(matches!(body.as_ref(), Expr::Var(..)), "body={body:#?}");
        }
        other => panic!("expected let expr, got {other:#?}"),
    }
}

#[test]
fn test_parse_hole_in_nested_expression_positions() {
    let expr = parse("(\\f -> f ?) (\\x -> x)");
    match expr.as_ref() {
        Expr::App(_, lhs, rhs) => {
            assert!(matches!(rhs.as_ref(), Expr::Lam(..)), "rhs={rhs:#?}");
            match lhs.as_ref() {
                Expr::Lam(_, _, _param, _ann, _constraints, body) => match body.as_ref() {
                    Expr::App(_, f, arg) => {
                        assert!(matches!(f.as_ref(), Expr::Var(..)), "f={f:#?}");
                        assert!(matches!(arg.as_ref(), Expr::Hole(..)), "arg={arg:#?}");
                    }
                    other => panic!("expected app in lambda body, got {other:#?}"),
                },
                other => panic!("expected lambda lhs, got {other:#?}"),
            }
        }
        other => panic!("expected top-level app, got {other:#?}"),
    }
}

#[test]
fn test_parse_hole_not_allowed_in_type_annotation_failure_case() {
    let mut parser = Parser::new(Token::tokenize("let x : ? = 1 in x").unwrap());
    let errs = parser.parse_program(&mut GasMeter::default()).unwrap_err();
    assert!(
        errs.iter()
            .any(|e| e.message.contains("expected type") || e.message.contains("unexpected")),
        "errs={errs:#?}"
    );
}

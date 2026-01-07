use std::sync::OnceLock;

use rex_ast::expr::{Decl, Program};
use rex_lexer::Token;
use rex_parser::Parser;

use crate::{AdtDecl, Predicate, Scheme, Type, TypeSystem, sym};

fn inject_prelude_classes_and_instances(ts: &mut TypeSystem) {
    for decl in &prelude_typeclasses_program().decls {
        match decl {
            Decl::Class(class_decl) => ts
                .inject_class_decl(class_decl)
                .expect("failed to inject prelude class decl"),
            Decl::Instance(inst_decl) => {
                ts.inject_instance_decl(inst_decl)
                    .expect("failed to inject prelude instance decl");
            }
            Decl::Type(..) | Decl::Fn(..) => {}
        }
    }
}

pub fn prelude_typeclasses_program() -> &'static Program {
    static PROGRAM: OnceLock<Program> = OnceLock::new();
    PROGRAM.get_or_init(|| {
        let source = include_str!("prelude_typeclasses.rex");
        let tokens = Token::tokenize(source).expect("failed to lex Rex prelude type class source");
        let mut parser = Parser::new(tokens);
        parser.parse_program().unwrap_or_else(|errs| {
            let mut out = String::from("failed to parse Rex prelude type class source:");
            for err in errs {
                out.push_str(&format!("\n  {err}"));
            }
            panic!("{out}");
        })
    })
}

fn inject_prelude_primops(ts: &mut TypeSystem) {
    // Rust-backed intrinsics used by `rex-ts/src/prelude_typeclasses.rex`.
    //
    // These intentionally carry no typeclass predicates. An instance method
    // body should not need to assume the class it is defining.
    let bool_ty = Type::con("bool", 0);
    let i32_ty = Type::con("i32", 0);
    let string_ty = Type::con("string", 0);

    // Equality intrinsics.
    //
    // WARM note: we make these “math-style” monomorphic overloads. Each
    // `prim_eq`/`prim_ne` implementation is tied to one concrete runtime type.
    // This avoids a single universal `eq` routine that switches on types at
    // runtime (harder to reason about, harder to optimize).
    {
        let eq_types = [
            "bool", "u8", "u16", "u32", "u64", "i8", "i16", "i32", "i64", "f32", "f64", "string",
            "uuid", "datetime",
        ];
        for prim in eq_types {
            let t = Type::con(prim, 0);
            ts.add_overload(
                "prim_eq",
                Scheme::new(
                    vec![],
                    vec![],
                    Type::fun(t.clone(), Type::fun(t.clone(), bool_ty.clone())),
                ),
            );
            ts.add_overload(
                "prim_ne",
                Scheme::new(
                    vec![],
                    vec![],
                    Type::fun(t.clone(), Type::fun(t, bool_ty.clone())),
                ),
            );
        }
    }

    // Array equality is implemented by the runtime (it needs to iterate without
    // allocating) but it must respect `Eq a`, so the primitive calls `(==)` on
    // elements rather than doing structural `Value` equality.
    {
        let a_tv = ts.supply.fresh(Some(sym("a")));
        let a = Type::var(a_tv.clone());
        let array_a = Type::app(Type::con("Array", 1), a.clone());
        ts.add_value(
            "prim_array_eq",
            Scheme::new(
                vec![a_tv.clone()],
                vec![],
                Type::fun(array_a.clone(), Type::fun(array_a.clone(), bool_ty.clone())),
            ),
        );
        ts.add_value(
            "prim_array_ne",
            Scheme::new(
                vec![a_tv],
                vec![],
                Type::fun(array_a.clone(), Type::fun(array_a, bool_ty.clone())),
            ),
        );
    }

    // Numeric intrinsics (monomorphic overloads).
    {
        let additive = [
            "string", "u8", "u16", "u32", "u64", "i8", "i16", "i32", "i64", "f32", "f64",
        ];
        for prim in additive {
            let t = Type::con(prim, 0);
            ts.add_overload("prim_zero", Scheme::new(vec![], vec![], t.clone()));
            ts.add_overload(
                "prim_add",
                Scheme::new(vec![], vec![], Type::fun(t.clone(), Type::fun(t.clone(), t.clone()))),
            );
        }

        let multiplicative = ["u8", "u16", "u32", "u64", "i8", "i16", "i32", "i64", "f32", "f64"];
        for prim in multiplicative {
            let t = Type::con(prim, 0);
            ts.add_overload("prim_one", Scheme::new(vec![], vec![], t.clone()));
            ts.add_overload(
                "prim_mul",
                Scheme::new(vec![], vec![], Type::fun(t.clone(), Type::fun(t.clone(), t.clone()))),
            );
        }

        let signed = ["i8", "i16", "i32", "i64", "f32", "f64"];
        for prim in signed {
            let t = Type::con(prim, 0);
            ts.add_overload(
                "prim_sub",
                Scheme::new(vec![], vec![], Type::fun(t.clone(), Type::fun(t.clone(), t.clone()))),
            );
            ts.add_overload(
                "prim_negate",
                Scheme::new(vec![], vec![], Type::fun(t.clone(), t.clone())),
            );
        }

        for prim in ["f32", "f64"] {
            let t = Type::con(prim, 0);
            ts.add_overload(
                "prim_div",
                Scheme::new(vec![], vec![], Type::fun(t.clone(), Type::fun(t.clone(), t.clone()))),
            );
        }

        let integral = ["u8", "u16", "u32", "u64", "i8", "i16", "i32", "i64"];
        for prim in integral {
            let t = Type::con(prim, 0);
            ts.add_overload(
                "prim_mod",
                Scheme::new(vec![], vec![], Type::fun(t.clone(), Type::fun(t.clone(), t.clone()))),
            );
        }
    }

    // Ordering intrinsics (monomorphic overloads).
    {
        let ord = ["u8", "u16", "u32", "u64", "i8", "i16", "i32", "i64", "f32", "f64", "string"];
        for prim in ord {
            let t = Type::con(prim, 0);
            ts.add_overload(
                "prim_cmp",
                Scheme::new(
                    vec![],
                    vec![],
                    Type::fun(t.clone(), Type::fun(t.clone(), i32_ty.clone())),
                ),
            );
            for name in ["prim_lt", "prim_le", "prim_gt", "prim_ge"] {
                ts.add_overload(
                    name,
                    Scheme::new(
                        vec![],
                        vec![],
                        Type::fun(t.clone(), Type::fun(t.clone(), bool_ty.clone())),
                    ),
                );
            }
        }
    }

    // Pretty-printing intrinsics (monomorphic overloads).
    {
        let pretty_types = [
            "bool", "u8", "u16", "u32", "u64", "i8", "i16", "i32", "i64", "f32", "f64", "string",
            "uuid", "datetime",
        ];
        for prim in pretty_types {
            let t = Type::con(prim, 0);
            ts.add_overload(
                "prim_pretty",
                Scheme::new(vec![], vec![], Type::fun(t, string_ty.clone())),
            );
        }
    }

    // Collection intrinsics used by the standard type class instances.
    //
    // These are all `prim_` because they are the host-provided “bottom layer”.
    // The user-facing API is the class methods (`map`, `foldl`, `zip`, `get`, ...).
    {
        let list_con = Type::con("List", 1);
        let array_con = Type::con("Array", 1);
        let option_con = Type::con("Option", 1);
        let result_con = Type::con("Result", 2);

        let list_of = |t: Type| Type::app(list_con.clone(), t);
        let array_of = |t: Type| Type::app(array_con.clone(), t);
        let option_of = |t: Type| Type::app(option_con.clone(), t);
        let result_of = |ok: Type, err: Type| Type::app(Type::app(result_con.clone(), err), ok);

        // prim_map
        {
            let a_tv = ts.supply.fresh(Some(sym("a")));
            let b_tv = ts.supply.fresh(Some(sym("b")));
            let a = Type::var(a_tv.clone());
            let b = Type::var(b_tv.clone());
            ts.add_overload(
                "prim_map",
                Scheme::new(
                    vec![a_tv.clone(), b_tv.clone()],
                    vec![],
                    Type::fun(
                        Type::fun(a.clone(), b.clone()),
                        Type::fun(list_of(a.clone()), list_of(b.clone())),
                    ),
                ),
            );
            ts.add_overload(
                "prim_map",
                Scheme::new(
                    vec![a_tv.clone(), b_tv.clone()],
                    vec![],
                    Type::fun(
                        Type::fun(a.clone(), b.clone()),
                        Type::fun(array_of(a.clone()), array_of(b.clone())),
                    ),
                ),
            );
            ts.add_overload(
                "prim_map",
                Scheme::new(
                    vec![a_tv.clone(), b_tv.clone()],
                    vec![],
                    Type::fun(
                        Type::fun(a.clone(), b.clone()),
                        Type::fun(option_of(a.clone()), option_of(b.clone())),
                    ),
                ),
            );
            let e_tv = ts.supply.fresh(Some(sym("e")));
            let e = Type::var(e_tv.clone());
            ts.add_overload(
                "prim_map",
                Scheme::new(
                    vec![a_tv, b_tv, e_tv],
                    vec![],
                    Type::fun(
                        Type::fun(a.clone(), b.clone()),
                        Type::fun(result_of(a.clone(), e.clone()), result_of(b.clone(), e)),
                    ),
                ),
            );
        }

        // prim_array_singleton
        {
            let a_tv = ts.supply.fresh(Some(sym("a")));
            let a = Type::var(a_tv.clone());
            ts.add_value(
                "prim_array_singleton",
                Scheme::new(vec![a_tv], vec![], Type::fun(a.clone(), array_of(a))),
            );
        }

        // prim_foldl / prim_foldr / prim_fold
        {
            let a_tv = ts.supply.fresh(Some(sym("a")));
            let b_tv = ts.supply.fresh(Some(sym("b")));
            let a = Type::var(a_tv.clone());
            let b = Type::var(b_tv.clone());
            let step_l = Type::fun(b.clone(), Type::fun(a.clone(), b.clone()));
            let step_r = Type::fun(a.clone(), Type::fun(b.clone(), b.clone()));
            let mut add_for = |fa: Type| {
                ts.add_overload(
                    "prim_foldl",
                    Scheme::new(
                        vec![a_tv.clone(), b_tv.clone()],
                        vec![],
                        Type::fun(
                            step_l.clone(),
                            Type::fun(b.clone(), Type::fun(fa.clone(), b.clone())),
                        ),
                    ),
                );
                ts.add_overload(
                    "prim_foldr",
                    Scheme::new(
                        vec![a_tv.clone(), b_tv.clone()],
                        vec![],
                        Type::fun(
                            step_r.clone(),
                            Type::fun(b.clone(), Type::fun(fa.clone(), b.clone())),
                        ),
                    ),
                );
                ts.add_overload(
                    "prim_fold",
                    Scheme::new(
                        vec![a_tv.clone(), b_tv.clone()],
                        vec![],
                        Type::fun(step_l.clone(), Type::fun(b.clone(), Type::fun(fa, b.clone()))),
                    ),
                );
            };

            add_for(list_of(a.clone()));
            add_for(array_of(a.clone()));
            add_for(option_of(a.clone()));
        }

        // prim_filter / prim_filter_map
        {
            let a_tv = ts.supply.fresh(Some(sym("a")));
            let b_tv = ts.supply.fresh(Some(sym("b")));
            let a = Type::var(a_tv.clone());
            let b = Type::var(b_tv.clone());
            let pred = Type::fun(a.clone(), bool_ty.clone());
            let mapper = Type::fun(a.clone(), option_of(b.clone()));
            let mut add_for = |fa: Type, fb: Type| {
                ts.add_overload(
                    "prim_filter",
                    Scheme::new(
                        vec![a_tv.clone()],
                        vec![],
                        Type::fun(pred.clone(), Type::fun(fa.clone(), fa.clone())),
                    ),
                );
                ts.add_overload(
                    "prim_filter_map",
                    Scheme::new(
                        vec![a_tv.clone(), b_tv.clone()],
                        vec![],
                        Type::fun(mapper.clone(), Type::fun(fa, fb)),
                    ),
                );
            };

            add_for(list_of(a.clone()), list_of(b.clone()));
            add_for(array_of(a.clone()), array_of(b.clone()));
            add_for(option_of(a.clone()), option_of(b.clone()));
        }

        // prim_flat_map
        {
            // List / Array / Option
            let a_tv = ts.supply.fresh(Some(sym("a")));
            let b_tv = ts.supply.fresh(Some(sym("b")));
            let a = Type::var(a_tv.clone());
            let b = Type::var(b_tv.clone());
            let mut add_for = |fa: Type, fb: Type| {
                ts.add_overload(
                    "prim_flat_map",
                    Scheme::new(
                        vec![a_tv.clone(), b_tv.clone()],
                        vec![],
                        Type::fun(Type::fun(a.clone(), fb.clone()), Type::fun(fa, fb)),
                    ),
                );
            };

            add_for(list_of(a.clone()), list_of(b.clone()));
            add_for(array_of(a.clone()), array_of(b.clone()));
            add_for(option_of(a.clone()), option_of(b.clone()));

            // Result e
            let e_tv = ts.supply.fresh(Some(sym("e")));
            let e = Type::var(e_tv.clone());
            let ra = result_of(a.clone(), e.clone());
            let rb = result_of(b.clone(), e.clone());
            ts.add_overload(
                "prim_flat_map",
                Scheme::new(
                    vec![a_tv, b_tv, e_tv],
                    vec![],
                    Type::fun(Type::fun(a.clone(), rb.clone()), Type::fun(ra, rb)),
                ),
            );
        }

        // prim_or_else
        {
            let a_tv = ts.supply.fresh(Some(sym("a")));
            let a = Type::var(a_tv.clone());
            let mut add_for = |fa: Type| {
                let fa2 = fa.clone();
                ts.add_overload(
                    "prim_or_else",
                    Scheme::new(
                        vec![a_tv.clone()],
                        vec![],
                        Type::fun(Type::fun(fa.clone(), fa.clone()), Type::fun(fa2, fa)),
                    ),
                );
            };

            add_for(list_of(a.clone()));
            add_for(array_of(a.clone()));
            add_for(option_of(a.clone()));

            let e_tv = ts.supply.fresh(Some(sym("e")));
            let e = Type::var(e_tv.clone());
            let ra = result_of(a.clone(), e);
            ts.add_overload(
                "prim_or_else",
                Scheme::new(
                    vec![a_tv, e_tv],
                    vec![],
                    Type::fun(Type::fun(ra.clone(), ra.clone()), Type::fun(ra.clone(), ra)),
                ),
            );
        }

        // prim_take / prim_skip
        {
            let a_tv = ts.supply.fresh(Some(sym("a")));
            let a = Type::var(a_tv.clone());
            let mut add_for = |fa: Type| {
                let scheme = Scheme::new(
                    vec![a_tv.clone()],
                    vec![],
                    Type::fun(i32_ty.clone(), Type::fun(fa.clone(), fa)),
                );
                ts.add_overload("prim_take", scheme.clone());
                ts.add_overload("prim_skip", scheme);
            };
            add_for(list_of(a.clone()));
            add_for(array_of(a.clone()));
        }

        // prim_zip / prim_unzip
        {
            let a_tv = ts.supply.fresh(Some(sym("a")));
            let b_tv = ts.supply.fresh(Some(sym("b")));
            let a = Type::var(a_tv.clone());
            let b = Type::var(b_tv.clone());
            let pair = Type::tuple(vec![a.clone(), b.clone()]);
            let mut add_for = |fa: Type, fb: Type, fp: Type| {
                ts.add_overload(
                    "prim_zip",
                    Scheme::new(
                        vec![a_tv.clone(), b_tv.clone()],
                        vec![],
                        Type::fun(fa.clone(), Type::fun(fb.clone(), fp.clone())),
                    ),
                );
                ts.add_overload(
                    "prim_unzip",
                    Scheme::new(
                        vec![a_tv.clone(), b_tv.clone()],
                        vec![],
                        Type::fun(fp, Type::tuple(vec![fa, fb])),
                    ),
                );
            };

            add_for(list_of(a.clone()), list_of(b.clone()), list_of(pair.clone()));
            add_for(array_of(a.clone()), array_of(b.clone()), array_of(pair));
        }

        // prim_get
        {
            let a_tv = ts.supply.fresh(Some(sym("a")));
            let a = Type::var(a_tv.clone());
            let idx = i32_ty.clone();
            ts.add_overload(
                "prim_get",
                Scheme::new(
                    vec![a_tv.clone()],
                    vec![],
                    Type::fun(idx.clone(), Type::fun(list_of(a.clone()), a.clone())),
                ),
            );
            ts.add_overload(
                "prim_get",
                Scheme::new(
                    vec![a_tv.clone()],
                    vec![],
                    Type::fun(idx.clone(), Type::fun(array_of(a.clone()), a.clone())),
                ),
            );
            for size in 2..=32 {
                ts.add_overload(
                    "prim_get",
                    Scheme::new(
                        vec![a_tv.clone()],
                        vec![],
                        Type::fun(
                            idx.clone(),
                            Type::fun(Type::tuple(vec![a.clone(); size]), a.clone()),
                        ),
                    ),
                );
            }
        }
    }
}

pub(crate) fn build_prelude(ts: &mut TypeSystem) {
    // Primitive type constructors
    let prims = [
        "u8", "u16", "u32", "u64", "i8", "i16", "i32", "i64", "f32", "f64", "bool", "string",
        "uuid", "datetime",
    ];
    for prim in prims {
        ts.env
            .extend(sym(prim), Scheme::new(vec![], vec![], Type::con(prim, 0)));
    }

    // Type constructors for ADTs used in prelude schemes.
    let result_con = Type::con("Result", 2);
    let option_con = Type::con("Option", 1);

    // Register ADT constructors as value-level functions.
    {
        let list_name = sym("List");
        let a_name = sym("a");
        let list_params = vec![a_name.clone()];
        let mut list_adt = AdtDecl::new(&list_name, &list_params, &mut ts.supply);
        let a = list_adt.param_type(&a_name).unwrap();
        let list_a = list_adt.result_type();
        list_adt.add_variant(sym("Empty"), vec![]);
        list_adt.add_variant(sym("Cons"), vec![a.clone(), list_a.clone()]);
        ts.inject_adt(&list_adt);
    }
    {
        let option_name = sym("Option");
        let t_name = sym("t");
        let option_params = vec![t_name.clone()];
        let mut option_adt = AdtDecl::new(&option_name, &option_params, &mut ts.supply);
        let t = option_adt.param_type(&t_name).unwrap();
        option_adt.add_variant(sym("Some"), vec![t]);
        option_adt.add_variant(sym("None"), vec![]);
        ts.inject_adt(&option_adt);
    }
    {
        let result_name = sym("Result");
        let e_name = sym("e");
        let t_name = sym("t");
        let result_params = vec![e_name.clone(), t_name.clone()];
        let mut result_adt = AdtDecl::new(&result_name, &result_params, &mut ts.supply);
        let e = result_adt.param_type(&e_name).unwrap();
        let t = result_adt.param_type(&t_name).unwrap();
        result_adt.add_variant(sym("Err"), vec![e]);
        result_adt.add_variant(sym("Ok"), vec![t]);
        ts.inject_adt(&result_adt);
    }

    inject_prelude_primops(ts);
    inject_prelude_classes_and_instances(ts);

    // Helper constructors used to describe prelude schemes below.
    let fresh_tv = |ts: &mut TypeSystem, name: &str| ts.supply.fresh(Some(sym(name)));
    let option_of = |t: Type| Type::app(option_con.clone(), t);
    let result_of = |t: Type, e: Type| Type::app(Type::app(result_con.clone(), e), t);

    // Inject provided function declarations and schemes.

    // Boolean operators
    let bool_ty = Type::con("bool", 0);
    ts.add_value(
        "&&",
        Scheme::new(
            vec![],
            vec![],
            Type::fun(bool_ty.clone(), Type::fun(bool_ty.clone(), bool_ty.clone())),
        ),
    );
    ts.add_value(
        "||",
        Scheme::new(
            vec![],
            vec![],
            Type::fun(bool_ty.clone(), Type::fun(bool_ty.clone(), bool_ty.clone())),
        ),
    );

    // Collection helpers (type class based)
    {
        let f_tv = fresh_tv(ts, "f");
        let a_tv = fresh_tv(ts, "a");
        let f = Type::var(f_tv.clone());
        let a = Type::var(a_tv.clone());
        let fa = Type::app(f.clone(), a.clone());

        ts.add_value(
            "sum",
            Scheme::new(
                vec![f_tv.clone(), a_tv.clone()],
                vec![
                    Predicate::new("Foldable", f.clone()),
                    Predicate::new("AdditiveMonoid", a.clone()),
                ],
                Type::fun(fa.clone(), a.clone()),
            ),
        );
        ts.add_value(
            "mean",
            Scheme::new(
                vec![f_tv.clone(), a_tv.clone()],
                vec![
                    Predicate::new("Foldable", f.clone()),
                    Predicate::new("Field", a.clone()),
                ],
                Type::fun(fa.clone(), a.clone()),
            ),
        );
        ts.add_value(
            "count",
            Scheme::new(
                vec![f_tv.clone(), a_tv.clone()],
                vec![Predicate::new("Foldable", f.clone())],
                Type::fun(fa.clone(), Type::con("i32", 0)),
            ),
        );
        ts.add_value(
            "min",
            Scheme::new(
                vec![f_tv.clone(), a_tv.clone()],
                vec![
                    Predicate::new("Foldable", f.clone()),
                    Predicate::new("Ord", a.clone()),
                ],
                Type::fun(fa.clone(), a.clone()),
            ),
        );
        ts.add_value(
            "max",
            Scheme::new(
                vec![f_tv, a_tv],
                vec![
                    Predicate::new("Foldable", f.clone()),
                    Predicate::new("Ord", a.clone()),
                ],
                Type::fun(fa.clone(), a.clone()),
            ),
        );
    }

    // Option helpers
    {
        let a_tv = fresh_tv(ts, "a");
        let a = Type::var(a_tv.clone());
        let opt_a = option_of(a.clone());
        ts.add_value(
            "is_some",
            Scheme::new(vec![a_tv.clone()], vec![], Type::fun(opt_a.clone(), bool_ty.clone())),
        );
        ts.add_value(
            "is_none",
            Scheme::new(vec![a_tv.clone()], vec![], Type::fun(opt_a.clone(), bool_ty.clone())),
        );
    }

    // Result helpers
    {
        let t_tv = fresh_tv(ts, "t");
        let e_tv = fresh_tv(ts, "e");
        let t = Type::var(t_tv.clone());
        let e = Type::var(e_tv.clone());
        let res_te = result_of(t.clone(), e.clone());
        ts.add_value(
            "is_ok",
            Scheme::new(
                vec![t_tv.clone(), e_tv.clone()],
                vec![],
                Type::fun(res_te.clone(), bool_ty.clone()),
            ),
        );
        ts.add_value(
            "is_err",
            Scheme::new(
                vec![t_tv.clone(), e_tv.clone()],
                vec![],
                Type::fun(res_te.clone(), bool_ty.clone()),
            ),
        );
    }
}

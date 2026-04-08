use std::sync::OnceLock;

use rexlang_ast::expr::{Decl, Program};
use rexlang_lexer::Token;
use rexlang_parser::Parser;
use rexlang_util::GasMeter;

use crate::{AdtDecl, BuiltinTypeId, Predicate, Scheme, Type, TypeError, TypeSystem};
use rexlang_ast::expr::sym;

fn inject_prelude_classes_and_instances(ts: &mut TypeSystem) -> Result<(), TypeError> {
    let program = prelude_typeclasses_program()?;
    for decl in &program.decls {
        match decl {
            Decl::Class(class_decl) => ts.register_class_decl(class_decl)?,
            Decl::Instance(inst_decl) => {
                ts.register_instance_decl(inst_decl)?;
            }
            Decl::Type(..) | Decl::Fn(..) | Decl::DeclareFn(..) | Decl::Import(..) => {}
        }
    }
    Ok(())
}

pub fn prelude_typeclasses_program() -> Result<&'static Program, TypeError> {
    static PROGRAM: OnceLock<Result<Program, String>> = OnceLock::new();
    let parsed = PROGRAM.get_or_init(|| {
        let source = include_str!("prelude_typeclasses.rex");
        let tokens =
            Token::tokenize(source).map_err(|e| format!("prelude_typeclasses: lex error: {e}"))?;
        let mut parser = Parser::new(tokens);
        match parser.parse_program(&mut GasMeter::default()) {
            Ok(program) => Ok(program),
            Err(errs) => {
                let mut out = String::from("prelude_typeclasses: parse error:");
                for err in errs {
                    out.push_str(&format!("\n  {err}"));
                }
                Err(out)
            }
        }
    });
    match parsed {
        Ok(program) => Ok(program),
        Err(msg) => Err(TypeError::Internal(msg.clone())),
    }
}

fn inject_prelude_primops(ts: &mut TypeSystem) {
    // Rust-backed intrinsics used by `rexlang-typesystem/src/prelude_typeclasses.rex`.
    //
    // These intentionally carry no typeclass predicates. An instance method
    // body should not need to assume the class it is defining.
    let bool_ty = Type::builtin(BuiltinTypeId::Bool);
    let i32_ty = Type::builtin(BuiltinTypeId::I32);
    let string_ty = Type::builtin(BuiltinTypeId::String);

    // Equality intrinsics.
    //
    // Note: we make these “math-style” monomorphic overloads. Each
    // `prim_eq`/`prim_ne` implementation is tied to one concrete runtime type.
    // This avoids a single universal `eq` routine that switches on types at
    // runtime (harder to reason about, harder to optimize).
    {
        let eq_types = [
            BuiltinTypeId::Bool,
            BuiltinTypeId::U8,
            BuiltinTypeId::U16,
            BuiltinTypeId::U32,
            BuiltinTypeId::U64,
            BuiltinTypeId::I8,
            BuiltinTypeId::I16,
            BuiltinTypeId::I32,
            BuiltinTypeId::I64,
            BuiltinTypeId::F32,
            BuiltinTypeId::F64,
            BuiltinTypeId::String,
            BuiltinTypeId::Uuid,
            BuiltinTypeId::DateTime,
        ];
        for builtin in eq_types {
            let t = Type::builtin(builtin);
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
        let array_a = Type::app(Type::builtin(BuiltinTypeId::Array), a.clone());
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
            BuiltinTypeId::String,
            BuiltinTypeId::U8,
            BuiltinTypeId::U16,
            BuiltinTypeId::U32,
            BuiltinTypeId::U64,
            BuiltinTypeId::I8,
            BuiltinTypeId::I16,
            BuiltinTypeId::I32,
            BuiltinTypeId::I64,
            BuiltinTypeId::F32,
            BuiltinTypeId::F64,
        ];
        for builtin in additive {
            let t = Type::builtin(builtin);
            ts.add_overload("prim_zero", Scheme::new(vec![], vec![], t.clone()));
            ts.add_overload(
                "prim_add",
                Scheme::new(
                    vec![],
                    vec![],
                    Type::fun(t.clone(), Type::fun(t.clone(), t.clone())),
                ),
            );
        }

        let multiplicative = [
            BuiltinTypeId::U8,
            BuiltinTypeId::U16,
            BuiltinTypeId::U32,
            BuiltinTypeId::U64,
            BuiltinTypeId::I8,
            BuiltinTypeId::I16,
            BuiltinTypeId::I32,
            BuiltinTypeId::I64,
            BuiltinTypeId::F32,
            BuiltinTypeId::F64,
        ];
        for builtin in multiplicative {
            let t = Type::builtin(builtin);
            ts.add_overload("prim_one", Scheme::new(vec![], vec![], t.clone()));
            ts.add_overload(
                "prim_mul",
                Scheme::new(
                    vec![],
                    vec![],
                    Type::fun(t.clone(), Type::fun(t.clone(), t.clone())),
                ),
            );
        }

        let signed = [
            BuiltinTypeId::I8,
            BuiltinTypeId::I16,
            BuiltinTypeId::I32,
            BuiltinTypeId::I64,
            BuiltinTypeId::F32,
            BuiltinTypeId::F64,
        ];
        for builtin in signed {
            let t = Type::builtin(builtin);
            ts.add_overload(
                "prim_sub",
                Scheme::new(
                    vec![],
                    vec![],
                    Type::fun(t.clone(), Type::fun(t.clone(), t.clone())),
                ),
            );
            ts.add_overload(
                "prim_negate",
                Scheme::new(vec![], vec![], Type::fun(t.clone(), t.clone())),
            );
        }

        for builtin in [BuiltinTypeId::F32, BuiltinTypeId::F64] {
            let t = Type::builtin(builtin);
            ts.add_overload(
                "prim_div",
                Scheme::new(
                    vec![],
                    vec![],
                    Type::fun(t.clone(), Type::fun(t.clone(), t.clone())),
                ),
            );
        }

        let integral = [
            BuiltinTypeId::U8,
            BuiltinTypeId::U16,
            BuiltinTypeId::U32,
            BuiltinTypeId::U64,
            BuiltinTypeId::I8,
            BuiltinTypeId::I16,
            BuiltinTypeId::I32,
            BuiltinTypeId::I64,
        ];
        for builtin in integral {
            let t = Type::builtin(builtin);
            ts.add_overload(
                "prim_mod",
                Scheme::new(
                    vec![],
                    vec![],
                    Type::fun(t.clone(), Type::fun(t.clone(), t.clone())),
                ),
            );
        }
    }

    // Ordering intrinsics (monomorphic overloads).
    {
        let ord = [
            BuiltinTypeId::U8,
            BuiltinTypeId::U16,
            BuiltinTypeId::U32,
            BuiltinTypeId::U64,
            BuiltinTypeId::I8,
            BuiltinTypeId::I16,
            BuiltinTypeId::I32,
            BuiltinTypeId::I64,
            BuiltinTypeId::F32,
            BuiltinTypeId::F64,
            BuiltinTypeId::String,
        ];
        for builtin in ord {
            let t = Type::builtin(builtin);
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

    // Show-printing intrinsics (monomorphic overloads).
    {
        let show_types = [
            BuiltinTypeId::Bool,
            BuiltinTypeId::U8,
            BuiltinTypeId::U16,
            BuiltinTypeId::U32,
            BuiltinTypeId::U64,
            BuiltinTypeId::I8,
            BuiltinTypeId::I16,
            BuiltinTypeId::I32,
            BuiltinTypeId::I64,
            BuiltinTypeId::F32,
            BuiltinTypeId::F64,
            BuiltinTypeId::String,
            BuiltinTypeId::Uuid,
            BuiltinTypeId::DateTime,
        ];
        for builtin in show_types {
            let t = Type::builtin(builtin);
            ts.add_overload(
                "prim_show",
                Scheme::new(vec![], vec![], Type::fun(t, string_ty.clone())),
            );
        }
    }

    // JSON stringification (used by `std.json`'s `Show` instance).
    //
    // This is intentionally a `prim_` helper with a polymorphic type so the
    // `std.json` library can stay purely-Rex at the surface level.
    {
        let a_tv = ts.supply.fresh(Some(sym("a")));
        let a = Type::var(a_tv.clone());
        ts.add_value(
            "prim_json_stringify",
            Scheme::new(vec![a_tv], vec![], Type::fun(a, string_ty.clone())),
        );
    }

    // prim_json_parse : string -> Result a string
    //
    // The ok type is polymorphic so `std.json` can instantiate it as
    // `Result std.json.Value string` (and then wrap the string error into
    // `DecodeError`).
    {
        let a_tv = ts.supply.fresh(Some(sym("a")));
        let a = Type::var(a_tv.clone());
        let result_con = Type::builtin(BuiltinTypeId::Result);
        let result_as = Type::app(Type::app(result_con, string_ty.clone()), a);
        ts.add_value(
            "prim_json_parse",
            Scheme::new(vec![a_tv], vec![], Type::fun(string_ty.clone(), result_as)),
        );
    }

    // Collection intrinsics used by the standard type class instances.
    //
    // These are all `prim_` because they are the host-provided “bottom layer”.
    // The user-facing API is the class methods (`map`, `foldl`, `zip`, `get`, ...).
    {
        let list_con = Type::builtin(BuiltinTypeId::List);
        let array_con = Type::builtin(BuiltinTypeId::Array);
        let option_con = Type::builtin(BuiltinTypeId::Option);
        let result_con = Type::builtin(BuiltinTypeId::Result);

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
                        Type::fun(
                            step_l.clone(),
                            Type::fun(b.clone(), Type::fun(fa, b.clone())),
                        ),
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

            add_for(
                list_of(a.clone()),
                list_of(b.clone()),
                list_of(pair.clone()),
            );
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

        // List/Array conversion helpers.
        {
            let a_tv = ts.supply.fresh(Some(sym("a")));
            let a = Type::var(a_tv.clone());
            let list_a = list_of(a.clone());
            let array_a = array_of(a.clone());
            ts.add_value(
                "prim_array_from_list",
                Scheme::new(
                    vec![a_tv.clone()],
                    vec![],
                    Type::fun(list_a.clone(), array_a.clone()),
                ),
            );
            ts.add_value(
                "prim_list_from_array",
                Scheme::new(
                    vec![a_tv.clone()],
                    vec![],
                    Type::fun(array_a.clone(), list_a.clone()),
                ),
            );
            ts.add_value(
                "to_array",
                Scheme::new(
                    vec![a_tv.clone()],
                    vec![],
                    Type::fun(list_a.clone(), array_a.clone()),
                ),
            );
            ts.add_value(
                "to_list",
                Scheme::new(vec![a_tv], vec![], Type::fun(array_a, list_a)),
            );
        }

        // prim_dict_map : (a -> b) -> Dict a -> Dict b
        {
            let a_tv = ts.supply.fresh(Some(sym("a")));
            let b_tv = ts.supply.fresh(Some(sym("b")));
            let a = Type::var(a_tv.clone());
            let b = Type::var(b_tv.clone());
            let dict_a = Type::app(Type::builtin(BuiltinTypeId::Dict), a.clone());
            let dict_b = Type::app(Type::builtin(BuiltinTypeId::Dict), b.clone());
            ts.add_value(
                "prim_dict_map",
                Scheme::new(
                    vec![a_tv, b_tv],
                    vec![],
                    Type::fun(Type::fun(a, b), Type::fun(dict_a, dict_b)),
                ),
            );
        }

        // prim_dict_traverse_result : (a -> Result b e) -> Dict a -> Result (Dict b) e
        {
            let a_tv = ts.supply.fresh(Some(sym("a")));
            let b_tv = ts.supply.fresh(Some(sym("b")));
            let e_tv = ts.supply.fresh(Some(sym("e")));
            let a = Type::var(a_tv.clone());
            let b = Type::var(b_tv.clone());
            let e = Type::var(e_tv.clone());
            let dict_a = Type::app(Type::builtin(BuiltinTypeId::Dict), a.clone());
            let dict_b = Type::app(Type::builtin(BuiltinTypeId::Dict), b.clone());
            let result_eb = result_of(b.clone(), e.clone());
            let result_edictb = result_of(dict_b, e);
            ts.add_value(
                "prim_dict_traverse_result",
                Scheme::new(
                    vec![a_tv, b_tv, e_tv],
                    vec![],
                    Type::fun(Type::fun(a, result_eb), Type::fun(dict_a, result_edictb)),
                ),
            );
        }

        // Numeric conversions used by `std.json`.
        //
        // We model these as primitive intrinsics to keep Rex code simple and to
        // make overflow/rounding rules explicit at the host boundary.
        for src in [
            BuiltinTypeId::U8,
            BuiltinTypeId::U16,
            BuiltinTypeId::U32,
            BuiltinTypeId::U64,
            BuiltinTypeId::I8,
            BuiltinTypeId::I16,
            BuiltinTypeId::I32,
            BuiltinTypeId::I64,
            BuiltinTypeId::F32,
            BuiltinTypeId::F64,
        ] {
            let t = Type::builtin(src);
            ts.add_overload(
                "prim_to_f64",
                Scheme::new(
                    vec![],
                    vec![],
                    Type::fun(t, Type::builtin(BuiltinTypeId::F64)),
                ),
            );
        }

        for (name, dst) in [
            ("prim_f64_to_u8", BuiltinTypeId::U8),
            ("prim_f64_to_u16", BuiltinTypeId::U16),
            ("prim_f64_to_u32", BuiltinTypeId::U32),
            ("prim_f64_to_u64", BuiltinTypeId::U64),
            ("prim_f64_to_i8", BuiltinTypeId::I8),
            ("prim_f64_to_i16", BuiltinTypeId::I16),
            ("prim_f64_to_i32", BuiltinTypeId::I32),
            ("prim_f64_to_i64", BuiltinTypeId::I64),
            ("prim_f64_to_f32", BuiltinTypeId::F32),
        ] {
            let dst_ty = Type::builtin(dst);
            ts.add_value(
                name,
                Scheme::new(
                    vec![],
                    vec![],
                    Type::fun(Type::builtin(BuiltinTypeId::F64), option_of(dst_ty)),
                ),
            );
        }

        ts.add_value(
            "prim_parse_uuid",
            Scheme::new(
                vec![],
                vec![],
                Type::fun(
                    Type::builtin(BuiltinTypeId::String),
                    option_of(Type::builtin(BuiltinTypeId::Uuid)),
                ),
            ),
        );
        ts.add_value(
            "prim_parse_datetime",
            Scheme::new(
                vec![],
                vec![],
                Type::fun(
                    Type::builtin(BuiltinTypeId::String),
                    option_of(Type::builtin(BuiltinTypeId::DateTime)),
                ),
            ),
        );
    }
}

pub(crate) fn build_prelude(ts: &mut TypeSystem) -> Result<(), TypeError> {
    // Primitive type constructors
    let prims = [
        BuiltinTypeId::U8,
        BuiltinTypeId::U16,
        BuiltinTypeId::U32,
        BuiltinTypeId::U64,
        BuiltinTypeId::I8,
        BuiltinTypeId::I16,
        BuiltinTypeId::I32,
        BuiltinTypeId::I64,
        BuiltinTypeId::F32,
        BuiltinTypeId::F64,
        BuiltinTypeId::Bool,
        BuiltinTypeId::String,
        BuiltinTypeId::Uuid,
        BuiltinTypeId::DateTime,
    ];
    for prim in prims {
        ts.env.extend(
            prim.as_symbol(),
            Scheme::new(vec![], vec![], Type::builtin(prim)),
        );
    }

    // Type constructors for ADTs used in prelude schemes.
    let result_con = Type::builtin(BuiltinTypeId::Result);
    let option_con = Type::builtin(BuiltinTypeId::Option);

    // Register ADT constructors as value-level functions.
    {
        let list_name = sym("List");
        let a_name = sym("a");
        let list_params = vec![a_name.clone()];
        let mut list_adt = AdtDecl::new(&list_name, &list_params, &mut ts.supply);
        let a = list_adt.param_type(&a_name).ok_or_else(|| {
            TypeError::Internal("prelude: List is missing type parameter `a`".into())
        })?;
        let list_a = list_adt.result_type();
        list_adt.add_variant(sym("Empty"), vec![]);
        list_adt.add_variant(sym("Cons"), vec![a.clone(), list_a.clone()]);
        ts.register_adt(&list_adt);
    }
    {
        let option_name = sym("Option");
        let t_name = sym("t");
        let option_params = vec![t_name.clone()];
        let mut option_adt = AdtDecl::new(&option_name, &option_params, &mut ts.supply);
        let t = option_adt.param_type(&t_name).ok_or_else(|| {
            TypeError::Internal("prelude: Option is missing type parameter `t`".into())
        })?;
        option_adt.add_variant(sym("Some"), vec![t]);
        option_adt.add_variant(sym("None"), vec![]);
        ts.register_adt(&option_adt);
    }
    {
        let result_name = sym("Result");
        let e_name = sym("e");
        let t_name = sym("t");
        let result_params = vec![e_name.clone(), t_name.clone()];
        let mut result_adt = AdtDecl::new(&result_name, &result_params, &mut ts.supply);
        let e = result_adt.param_type(&e_name).ok_or_else(|| {
            TypeError::Internal("prelude: Result is missing type parameter `e`".into())
        })?;
        let t = result_adt.param_type(&t_name).ok_or_else(|| {
            TypeError::Internal("prelude: Result is missing type parameter `t`".into())
        })?;
        result_adt.add_variant(sym("Err"), vec![e]);
        result_adt.add_variant(sym("Ok"), vec![t]);
        ts.register_adt(&result_adt);
    }

    inject_prelude_primops(ts);
    inject_prelude_classes_and_instances(ts)?;

    // Helper constructors used to describe prelude schemes below.
    let fresh_tv = |ts: &mut TypeSystem, name: &str| ts.supply.fresh(Some(sym(name)));
    let option_of = |t: Type| Type::app(option_con.clone(), t);
    let result_of = |t: Type, e: Type| Type::app(Type::app(result_con.clone(), e), t);

    // Inject provided function declarations and schemes.

    // Boolean operators
    let bool_ty = Type::builtin(BuiltinTypeId::Bool);
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
                Type::fun(fa.clone(), Type::builtin(BuiltinTypeId::I32)),
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
            "unwrap",
            Scheme::new(
                vec![a_tv.clone()],
                vec![],
                Type::fun(opt_a.clone(), a.clone()),
            ),
        );
        ts.add_value(
            "is_some",
            Scheme::new(
                vec![a_tv.clone()],
                vec![],
                Type::fun(opt_a.clone(), bool_ty.clone()),
            ),
        );
        ts.add_value(
            "is_none",
            Scheme::new(
                vec![a_tv.clone()],
                vec![],
                Type::fun(opt_a.clone(), bool_ty.clone()),
            ),
        );
    }

    // Result helpers
    {
        let t_tv = fresh_tv(ts, "t");
        let e_tv = fresh_tv(ts, "e");
        let t = Type::var(t_tv.clone());
        let e = Type::var(e_tv.clone());
        let res_te = result_of(t.clone(), e.clone());
        ts.add_overload(
            "unwrap",
            Scheme::new(
                vec![t_tv.clone(), e_tv.clone()],
                vec![],
                Type::fun(res_te.clone(), t.clone()),
            ),
        );
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

    Ok(())
}

use rex_ast::{Decl, Symbol};
use rex_typesystem::{
    error::TypeError,
    types::{AdtArgument, AdtDecl, BuiltinTypeId, Predicate, Scheme, Type},
    typesystem::TypeSystem,
};

fn inject_prelude_classes_and_instances(
    ts: &mut TypeSystem,
    decls: &[Decl],
) -> Result<(), TypeError> {
    for decl in decls {
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

fn inject_prelude_primops(ts: &mut TypeSystem) {
    // Rust-backed intrinsics used by the engine-owned prelude typeclass definitions.
    //
    // These intentionally carry no typeclass predicates. An instance method
    // body should not need to assume the class it is defining.
    let bool_ty = Type::builtin(BuiltinTypeId::Bool);
    let u64_ty = Type::builtin(BuiltinTypeId::U64);
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
            BuiltinTypeId::Char,
            BuiltinTypeId::String,
            BuiltinTypeId::Uuid,
            BuiltinTypeId::Hash,
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

    // List equality is implemented by the runtime (it needs to iterate without
    // allocating) but it must respect `Eq a`, so the primitive calls `(==)` on
    // elements rather than doing structural `Value` equality.
    {
        let a_tv = ts.supply.fresh(Some(Symbol::intern("a")));
        let a = Type::var(a_tv.clone());
        let list_a = Type::app(Type::builtin(BuiltinTypeId::List), a.clone());
        ts.add_value(
            "prim_list_eq",
            Scheme::new(
                vec![a_tv.clone()],
                vec![],
                Type::fun(list_a.clone(), Type::fun(list_a.clone(), bool_ty.clone())),
            ),
        );
        ts.add_value(
            "prim_list_ne",
            Scheme::new(
                vec![a_tv],
                vec![],
                Type::fun(list_a.clone(), Type::fun(list_a, bool_ty.clone())),
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

        let subtractive = [
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
        for builtin in subtractive {
            let t = Type::builtin(builtin);
            ts.add_overload(
                "prim_sub",
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
                "prim_negate",
                Scheme::new(vec![], vec![], Type::fun(t.clone(), t.clone())),
            );
        }

        let divisive = [
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
        for builtin in divisive {
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
        let ordering_ty = Type::con("Ordering", 0);
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
            BuiltinTypeId::Char,
            BuiltinTypeId::String,
        ];
        for builtin in ord {
            let t = Type::builtin(builtin);
            ts.add_overload(
                "prim_cmp",
                Scheme::new(
                    vec![],
                    vec![],
                    Type::fun(t.clone(), Type::fun(t.clone(), ordering_ty.clone())),
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
            BuiltinTypeId::Char,
            BuiltinTypeId::String,
            BuiltinTypeId::Uuid,
            BuiltinTypeId::Hash,
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

    // String-parsing intrinsics (monomorphic overloads).
    {
        let parse_types = [
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
            BuiltinTypeId::Char,
            BuiltinTypeId::Uuid,
            BuiltinTypeId::Hash,
            BuiltinTypeId::DateTime,
        ];
        for builtin in parse_types {
            let t = Type::builtin(builtin);
            ts.add_overload(
                "prim_parse",
                Scheme::new(
                    vec![],
                    vec![],
                    Type::fun(string_ty.clone(), Type::option(t)),
                ),
            );
        }
    }

    // Collection intrinsics used by the standard type class instances.
    //
    // These are all `prim_` because they are the host-provided “bottom layer”.
    // The user-facing API is the class methods (`map`, `foldl`, `zip`, ...).
    {
        let list_con = Type::builtin(BuiltinTypeId::List);
        let option_con = Type::builtin(BuiltinTypeId::Option);
        let result_con = Type::builtin(BuiltinTypeId::Result);

        let list_of = |t: Type| Type::app(list_con.clone(), t);
        let option_of = |t: Type| Type::app(option_con.clone(), t);
        let result_of = |ok: Type, err: Type| Type::app(Type::app(result_con.clone(), err), ok);

        // Length primitives
        {
            let a_tv = ts.supply.fresh(Some(Symbol::intern("a")));
            let a = Type::var(a_tv.clone());
            ts.add_value(
                "prim_list_length",
                Scheme::new(
                    vec![a_tv.clone()],
                    vec![],
                    Type::fun(list_of(a.clone()), u64_ty.clone()),
                ),
            );
            ts.add_value(
                "prim_dict_length",
                Scheme::new(vec![a_tv], vec![], Type::fun(Type::dict(a), u64_ty.clone())),
            );
            ts.add_value(
                "prim_string_length",
                Scheme::new(vec![], vec![], Type::fun(string_ty.clone(), u64_ty.clone())),
            );
        }

        // prim_map
        {
            let a_tv = ts.supply.fresh(Some(Symbol::intern("a")));
            let b_tv = ts.supply.fresh(Some(Symbol::intern("b")));
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
                        Type::fun(option_of(a.clone()), option_of(b.clone())),
                    ),
                ),
            );
            let e_tv = ts.supply.fresh(Some(Symbol::intern("e")));
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

        // prim_foldl / prim_foldr / prim_fold
        {
            let a_tv = ts.supply.fresh(Some(Symbol::intern("a")));
            let b_tv = ts.supply.fresh(Some(Symbol::intern("b")));
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
            add_for(option_of(a.clone()));
        }

        // prim_filter / prim_filter_map
        {
            let a_tv = ts.supply.fresh(Some(Symbol::intern("a")));
            let b_tv = ts.supply.fresh(Some(Symbol::intern("b")));
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
            add_for(option_of(a.clone()), option_of(b.clone()));
        }

        // prim_flat_map
        {
            // List / Option
            let a_tv = ts.supply.fresh(Some(Symbol::intern("a")));
            let b_tv = ts.supply.fresh(Some(Symbol::intern("b")));
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
            add_for(option_of(a.clone()), option_of(b.clone()));

            // Result e
            let e_tv = ts.supply.fresh(Some(Symbol::intern("e")));
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
            let a_tv = ts.supply.fresh(Some(Symbol::intern("a")));
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
            add_for(option_of(a.clone()));

            let e_tv = ts.supply.fresh(Some(Symbol::intern("e")));
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
            let a_tv = ts.supply.fresh(Some(Symbol::intern("a")));
            let a = Type::var(a_tv.clone());
            let mut add_for = |fa: Type| {
                let scheme = Scheme::new(
                    vec![a_tv.clone()],
                    vec![],
                    Type::fun(u64_ty.clone(), Type::fun(fa.clone(), fa)),
                );
                ts.add_overload("prim_take", scheme.clone());
                ts.add_overload("prim_skip", scheme);
            };
            add_for(list_of(a.clone()));
        }

        // prim_zip / prim_unzip
        {
            let a_tv = ts.supply.fresh(Some(Symbol::intern("a")));
            let b_tv = ts.supply.fresh(Some(Symbol::intern("b")));
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
        }

        // prim_dict_map : (a -> b) -> Dict a -> Dict b
        {
            let a_tv = ts.supply.fresh(Some(Symbol::intern("a")));
            let b_tv = ts.supply.fresh(Some(Symbol::intern("b")));
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

        // Value-only dictionary filtering used by the Filterable Dict instance.
        {
            let a_tv = ts.supply.fresh(Some(Symbol::intern("a")));
            let b_tv = ts.supply.fresh(Some(Symbol::intern("b")));
            let a = Type::var(a_tv.clone());
            let b = Type::var(b_tv.clone());
            ts.add_value(
                "prim_dict_filter",
                Scheme::new(
                    vec![a_tv.clone()],
                    vec![],
                    Type::fun(
                        Type::fun(a.clone(), bool_ty.clone()),
                        Type::fun(Type::dict(a.clone()), Type::dict(a.clone())),
                    ),
                ),
            );
            ts.add_value(
                "prim_dict_filter_map",
                Scheme::new(
                    vec![a_tv, b_tv],
                    vec![],
                    Type::fun(
                        Type::fun(a.clone(), option_of(b.clone())),
                        Type::fun(Type::dict(a), Type::dict(b)),
                    ),
                ),
            );
        }

        // Numeric conversions.
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

        for (src, dst) in [
            (BuiltinTypeId::I8, BuiltinTypeId::I16),
            (BuiltinTypeId::I8, BuiltinTypeId::I32),
            (BuiltinTypeId::I8, BuiltinTypeId::I64),
            (BuiltinTypeId::I16, BuiltinTypeId::I32),
            (BuiltinTypeId::I16, BuiltinTypeId::I64),
            (BuiltinTypeId::I32, BuiltinTypeId::I64),
            (BuiltinTypeId::U8, BuiltinTypeId::U16),
            (BuiltinTypeId::U8, BuiltinTypeId::U32),
            (BuiltinTypeId::U8, BuiltinTypeId::U64),
            (BuiltinTypeId::U8, BuiltinTypeId::I16),
            (BuiltinTypeId::U8, BuiltinTypeId::I32),
            (BuiltinTypeId::U8, BuiltinTypeId::I64),
            (BuiltinTypeId::U16, BuiltinTypeId::U32),
            (BuiltinTypeId::U16, BuiltinTypeId::U64),
            (BuiltinTypeId::U16, BuiltinTypeId::I32),
            (BuiltinTypeId::U16, BuiltinTypeId::I64),
            (BuiltinTypeId::U32, BuiltinTypeId::U64),
            (BuiltinTypeId::U32, BuiltinTypeId::I64),
        ] {
            let src_ty = Type::builtin(src);
            let dst_ty = Type::builtin(dst);
            ts.add_overload(
                "prim_widen_int",
                Scheme::new(vec![], vec![], Type::fun(src_ty, dst_ty)),
            );
        }
    }
}

pub(super) fn inject_standard_prelude(
    ts: &mut TypeSystem,
    decls: &[Decl],
) -> Result<(), TypeError> {
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
        BuiltinTypeId::Char,
        BuiltinTypeId::String,
        BuiltinTypeId::Uuid,
        BuiltinTypeId::Hash,
        BuiltinTypeId::DateTime,
    ];
    for prim in prims {
        ts.env
            .extend(prim.as_symbol(), scheme!(Type::builtin(prim)));
    }

    // Register ADT constructors as value-level functions.
    {
        let list_name = Symbol::intern("List");
        let a_name = Symbol::intern("a");
        let list_params = vec![a_name.clone()];
        let mut list_adt = AdtDecl::new(&list_name, &list_params, &mut ts.supply);
        let a = list_adt.param_type(&a_name).ok_or_else(|| {
            TypeError::Internal("prelude: List is missing type parameter `a`".into())
        })?;
        let list_a = list_adt.result_type();
        list_adt.add_variant(Symbol::intern("Empty"), vec![], None);
        list_adt.add_variant(
            Symbol::intern("Cons"),
            vec![
                AdtArgument::positional(a.clone()),
                AdtArgument::positional(list_a.clone()),
            ],
            None,
        );
        ts.register_adt(&list_adt)?;
    }
    {
        let ordering_name = Symbol::intern("Ordering");
        let mut ordering_adt = AdtDecl::new(&ordering_name, &[], &mut ts.supply);
        ordering_adt.add_variant(Symbol::intern("Less"), vec![], None);
        ordering_adt.add_variant(Symbol::intern("Equal"), vec![], None);
        ordering_adt.add_variant(Symbol::intern("Greater"), vec![], None);
        ts.register_adt(&ordering_adt)?;
    }
    {
        let option_name = Symbol::intern("Option");
        let t_name = Symbol::intern("t");
        let option_params = vec![t_name.clone()];
        let mut option_adt = AdtDecl::new(&option_name, &option_params, &mut ts.supply);
        let t = option_adt.param_type(&t_name).ok_or_else(|| {
            TypeError::Internal("prelude: Option is missing type parameter `t`".into())
        })?;
        option_adt.add_variant(
            Symbol::intern("Some"),
            vec![AdtArgument::positional(t)],
            None,
        );
        option_adt.add_variant(Symbol::intern("None"), vec![], None);
        ts.register_adt(&option_adt)?;
    }
    {
        let result_name = Symbol::intern("Result");
        let e_name = Symbol::intern("e");
        let t_name = Symbol::intern("t");
        let result_params = vec![e_name.clone(), t_name.clone()];
        let mut result_adt = AdtDecl::new(&result_name, &result_params, &mut ts.supply);
        let e = result_adt.param_type(&e_name).ok_or_else(|| {
            TypeError::Internal("prelude: Result is missing type parameter `e`".into())
        })?;
        let t = result_adt.param_type(&t_name).ok_or_else(|| {
            TypeError::Internal("prelude: Result is missing type parameter `t`".into())
        })?;
        result_adt.add_variant(
            Symbol::intern("Err"),
            vec![AdtArgument::positional(e)],
            None,
        );
        result_adt.add_variant(Symbol::intern("Ok"), vec![AdtArgument::positional(t)], None);
        ts.register_adt(&result_adt)?;
    }

    inject_prelude_primops(ts);
    inject_prelude_classes_and_instances(ts, decls)?;

    // Inject provided function declarations and schemes.

    // Boolean operators
    let bool_ty = Type::builtin(BuiltinTypeId::Bool);
    ts.add_value(
        "&&",
        scheme!(Type::fun(&bool_ty, Type::fun(&bool_ty, &bool_ty))),
    );
    ts.add_value(
        "||",
        scheme!(Type::fun(&bool_ty, Type::fun(&bool_ty, &bool_ty))),
    );

    inject_string_builtin_schemes(ts);
    inject_list_builtin_schemes(ts);

    // Collection helpers (type class based)
    let sum_scheme = scheme!(&mut ts.supply; forall [f, a]
        where [Foldable(f), AdditiveMonoid(a)]
        => Type::fun(Type::app(f, a), a)
    );
    ts.add_value("sum", sum_scheme);
    let mean_scheme = scheme!(&mut ts.supply; forall [f, a]
        where [Foldable(f), Field(a)]
        => Type::fun(Type::app(f, a), a)
    );
    ts.add_value("mean", mean_scheme);

    // String-keyed immutable dictionary operations.
    {
        let a_tv = ts.supply.fresh(Some(Symbol::intern("a")));
        let b_tv = ts.supply.fresh(Some(Symbol::intern("b")));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let string_ty = Type::builtin(BuiltinTypeId::String);
        let dict_a = Type::dict(a.clone());
        let dict_b = Type::dict(b.clone());
        let option_a = Type::app(Type::builtin(BuiltinTypeId::Option), a.clone());
        let entry_a = Type::tuple(vec![string_ty.clone(), a.clone()]);
        let entry_b = Type::tuple(vec![string_ty.clone(), b.clone()]);

        ts.add_value(
            "dict_empty",
            Scheme::new(vec![a_tv.clone()], vec![], dict_a.clone()),
        );
        ts.add_value(
            "dict_singleton",
            Scheme::new(
                vec![a_tv.clone()],
                vec![],
                Type::fun(string_ty.clone(), Type::fun(a.clone(), dict_a.clone())),
            ),
        );
        ts.add_value(
            "dict_get",
            Scheme::new(
                vec![a_tv.clone()],
                vec![],
                Type::fun(
                    string_ty.clone(),
                    Type::fun(dict_a.clone(), option_a.clone()),
                ),
            ),
        );
        ts.add_value(
            "dict_has",
            Scheme::new(
                vec![a_tv.clone()],
                vec![],
                Type::fun(
                    string_ty.clone(),
                    Type::fun(dict_a.clone(), bool_ty.clone()),
                ),
            ),
        );
        ts.add_value(
            "dict_insert",
            Scheme::new(
                vec![a_tv.clone()],
                vec![],
                Type::fun(
                    string_ty.clone(),
                    Type::fun(a.clone(), Type::fun(dict_a.clone(), dict_a.clone())),
                ),
            ),
        );
        ts.add_value(
            "dict_remove",
            Scheme::new(
                vec![a_tv.clone()],
                vec![],
                Type::fun(string_ty.clone(), Type::fun(dict_a.clone(), dict_a.clone())),
            ),
        );
        ts.add_value(
            "dict_update",
            Scheme::new(
                vec![a_tv.clone()],
                vec![],
                Type::fun(
                    string_ty.clone(),
                    Type::fun(
                        Type::fun(option_a.clone(), option_a),
                        Type::fun(dict_a.clone(), dict_a.clone()),
                    ),
                ),
            ),
        );
        ts.add_value(
            "dict_is_empty",
            Scheme::new(
                vec![a_tv.clone()],
                vec![],
                Type::fun(dict_a.clone(), bool_ty.clone()),
            ),
        );
        ts.add_value(
            "dict_keys",
            Scheme::new(
                vec![a_tv.clone()],
                vec![],
                Type::fun(dict_a.clone(), Type::list(string_ty.clone())),
            ),
        );
        ts.add_value(
            "dict_values",
            Scheme::new(
                vec![a_tv.clone()],
                vec![],
                Type::fun(dict_a.clone(), Type::list(a.clone())),
            ),
        );
        ts.add_value(
            "dict_entries",
            Scheme::new(
                vec![a_tv.clone()],
                vec![],
                Type::fun(dict_a.clone(), Type::list(entry_a.clone())),
            ),
        );
        ts.add_value(
            "dict_from_entries",
            Scheme::new(
                vec![a_tv.clone()],
                vec![],
                Type::fun(Type::list(entry_a.clone()), dict_a.clone()),
            ),
        );
        ts.add_value(
            "dict_map",
            Scheme::new(
                vec![a_tv.clone(), b_tv],
                vec![],
                Type::fun(
                    Type::fun(entry_a.clone(), entry_b),
                    Type::fun(dict_a.clone(), dict_b),
                ),
            ),
        );
        ts.add_value(
            "dict_filter",
            Scheme::new(
                vec![a_tv],
                vec![],
                Type::fun(
                    Type::fun(entry_a, bool_ty.clone()),
                    Type::fun(dict_a.clone(), dict_a),
                ),
            ),
        );
    }
    let first_scheme = scheme!(&mut ts.supply; forall [a]
        => Type::fun(
            Type::builtin(BuiltinTypeId::I32),
            Type::fun(Type::list(a), Type::list(a)),
        )
    );
    ts.add_value("first", first_scheme);
    let last_scheme = scheme!(&mut ts.supply; forall [a]
        => Type::fun(
            Type::builtin(BuiltinTypeId::I32),
            Type::fun(Type::list(a), Type::list(a)),
        )
    );
    ts.add_value("last", last_scheme);
    let slice_scheme = scheme!(&mut ts.supply; forall [a]
        => Type::fun(
            Type::builtin(BuiltinTypeId::I32),
            Type::fun(
                Type::builtin(BuiltinTypeId::I32),
                Type::fun(Type::list(a), Type::list(a)),
            ),
        )
    );
    ts.add_value("slice", slice_scheme);
    let min_scheme = scheme!(&mut ts.supply; forall [f, a]
        where [Foldable(f), Ord(a)]
        => Type::fun(Type::app(f, a), a)
    );
    ts.add_value("min", min_scheme);
    let max_scheme = scheme!(&mut ts.supply; forall [f, a]
        where [Foldable(f), Ord(a)]
        => Type::fun(Type::app(f, a), a)
    );
    ts.add_value("max", max_scheme);

    // Option helpers
    let unwrap_option_scheme = scheme!(&mut ts.supply; forall [a] => Type::fun(Type::option(a), a));
    ts.add_value("unwrap", unwrap_option_scheme);
    let is_some_scheme =
        scheme!(&mut ts.supply; forall [a] => Type::fun(Type::option(a), &bool_ty));
    ts.add_value("is_some", is_some_scheme);
    let is_none_scheme =
        scheme!(&mut ts.supply; forall [a] => Type::fun(Type::option(a), &bool_ty));
    ts.add_value("is_none", is_none_scheme);

    // Result helpers
    let unwrap_result_scheme =
        scheme!(&mut ts.supply; forall [t, e] => Type::fun(Type::result(t, e), t));
    ts.add_overload("unwrap", unwrap_result_scheme);
    let is_ok_scheme =
        scheme!(&mut ts.supply; forall [t, e] => Type::fun(Type::result(t, e), &bool_ty));
    ts.add_value("is_ok", is_ok_scheme);
    let is_err_scheme =
        scheme!(&mut ts.supply; forall [t, e] => Type::fun(Type::result(t, e), &bool_ty));
    ts.add_value("is_err", is_err_scheme);

    Ok(())
}

fn inject_list_builtin_schemes(ts: &mut TypeSystem) {
    let bool_ty = Type::builtin(BuiltinTypeId::Bool);
    let u64_ty = Type::builtin(BuiltinTypeId::U64);

    macro_rules! add {
        ($name:literal, $scheme:expr $(,)?) => {{
            let scheme = $scheme;
            ts.add_value($name, scheme);
        }};
    }

    add!(
        "list_get",
        scheme!(&mut ts.supply; forall [a] =>
            Type::fun(&u64_ty, Type::fun(Type::list(a), Type::option(a)))
        ),
    );
    add!(
        "list_slice",
        scheme!(&mut ts.supply; forall [a] =>
            Type::fun(
                &u64_ty,
                Type::fun(
                    &u64_ty,
                    Type::fun(Type::list(a), Type::option(Type::list(a))),
                ),
            )
        ),
    );
    add!(
        "list_reverse",
        scheme!(&mut ts.supply; forall [a] => Type::fun(Type::list(a), Type::list(a))),
    );
    add!(
        "list_concat",
        scheme!(&mut ts.supply; forall [a] =>
            Type::fun(Type::list(Type::list(a)), Type::list(a))
        ),
    );
    add!(
        "list_repeat",
        scheme!(&mut ts.supply; forall [a] =>
            Type::fun(&u64_ty, Type::fun(a, Type::list(a)))
        ),
    );

    for name in ["list_any", "list_all"] {
        let scheme = scheme!(&mut ts.supply; forall [a] =>
            Type::fun(
                Type::fun(a, &bool_ty),
                Type::fun(Type::list(a), &bool_ty),
            )
        );
        ts.add_value(name, scheme);
    }
    add!(
        "list_find",
        scheme!(&mut ts.supply; forall [a] =>
            Type::fun(
                Type::fun(a, &bool_ty),
                Type::fun(Type::list(a), Type::option(a)),
            )
        ),
    );
    add!(
        "list_find_index",
        scheme!(&mut ts.supply; forall [a] =>
            Type::fun(
                Type::fun(a, &bool_ty),
                Type::fun(Type::list(a), Type::option(&u64_ty)),
            )
        ),
    );
    add!(
        "list_count",
        scheme!(&mut ts.supply; forall [a] =>
            Type::fun(
                Type::fun(a, &bool_ty),
                Type::fun(Type::list(a), &u64_ty),
            )
        ),
    );
    add!(
        "list_partition",
        scheme!(&mut ts.supply; forall [a] =>
            Type::fun(
                Type::fun(a, &bool_ty),
                Type::fun(
                    Type::list(a),
                    Type::tuple(vec![Type::list(a), Type::list(a)]),
                ),
            )
        ),
    );
}

// Register the user-facing string function signatures in the standard type system.
fn inject_string_builtin_schemes(ts: &mut TypeSystem) {
    let bool_ty = Type::builtin(BuiltinTypeId::Bool);
    let char_ty = Type::builtin(BuiltinTypeId::Char);
    let string_ty = Type::builtin(BuiltinTypeId::String);
    let u8_ty = Type::builtin(BuiltinTypeId::U8);
    let u64_ty = Type::builtin(BuiltinTypeId::U64);
    let list_char_ty = Type::list(char_ty.clone());
    let list_string_ty = Type::list(string_ty.clone());
    let list_u8_ty = Type::list(u8_ty);
    let option_char_ty = Type::option(char_ty);
    let option_string_ty = Type::option(string_ty.clone());
    let option_u64_ty = Type::option(u64_ty.clone());

    let mut add = |name: &str, args: &[Type], result: Type| {
        let typ = args
            .iter()
            .rev()
            .fold(result, |out, arg| Type::fun(arg.clone(), out));
        ts.add_value(name, Scheme::new(vec![], vec![], typ));
    };

    add(
        "string_get",
        &[u64_ty.clone(), string_ty.clone()],
        option_char_ty,
    );
    add(
        "string_slice",
        &[u64_ty.clone(), u64_ty.clone(), string_ty.clone()],
        option_string_ty.clone(),
    );
    add(
        "string_contains",
        &[string_ty.clone(), string_ty.clone()],
        bool_ty.clone(),
    );
    add(
        "string_starts_with",
        &[string_ty.clone(), string_ty.clone()],
        bool_ty.clone(),
    );
    add(
        "string_ends_with",
        &[string_ty.clone(), string_ty.clone()],
        bool_ty,
    );
    add(
        "string_find",
        &[string_ty.clone(), string_ty.clone()],
        option_u64_ty,
    );
    add(
        "string_split",
        &[string_ty.clone(), string_ty.clone()],
        list_string_ty.clone(),
    );
    add(
        "string_join",
        &[string_ty.clone(), list_string_ty],
        string_ty.clone(),
    );
    add(
        "string_replace",
        &[string_ty.clone(), string_ty.clone(), string_ty.clone()],
        string_ty.clone(),
    );
    for name in [
        "string_trim",
        "string_trim_start",
        "string_trim_end",
        "string_to_lower",
        "string_to_upper",
    ] {
        add(name, std::slice::from_ref(&string_ty), string_ty.clone());
    }
    add(
        "string_to_chars",
        std::slice::from_ref(&string_ty),
        list_char_ty.clone(),
    );
    add("chars_to_string", &[list_char_ty], string_ty.clone());
    add(
        "string_to_utf8",
        std::slice::from_ref(&string_ty),
        list_u8_ty.clone(),
    );
    add("utf8_to_string", &[list_u8_ty], option_string_ty);
}

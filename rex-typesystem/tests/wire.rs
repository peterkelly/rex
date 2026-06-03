use rex_ast::Symbol;
use rex_typesystem::{
    types::{AdtDecl, BuiltinTypeId, Predicate, Scheme, Type, TypeKind, TypeVar},
    typesystem::{TypeSystem, TypeVarSupply},
    wire::{
        TYPE_BUNDLE_SCHEMA_VERSION, TypeBundle, WireAdtDecl, WireAdtVariant, WireField, WireScheme,
        WireType, WireTypeVar,
    },
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::fmt::Debug;

fn assert_serialized<T>(value: &T, expected: Value) -> Value
where
    T: serde::Serialize + DeserializeOwned + PartialEq + Debug,
{
    let actual = serde_json::to_value(value).expect("serialize value");
    assert_eq!(actual, expected);
    let decoded =
        serde_json::from_value::<T>(actual.clone()).expect("deserialize serialized value");
    assert_eq!(&decoded, value);
    actual
}

fn named_arg_names(typ: &WireType) -> Vec<String> {
    let args = match typ {
        WireType::Named { args, .. } | WireType::Builtin { args, .. } => args,
        other => panic!("expected constructor wire type, got {other:?}"),
    };
    args.iter()
        .map(|arg| match arg {
            WireType::Named { name, .. } | WireType::Builtin { name, .. } => name.clone(),
            other => panic!("expected named arg, got {other:?}"),
        })
        .collect()
}

#[test]
fn zero_arity_builtin_types_serialize_as_builtin_types() {
    let wire_types = vec![
        WireType::from_type(&Type::builtin(BuiltinTypeId::U8)),
        WireType::from_type(&Type::builtin(BuiltinTypeId::U16)),
        WireType::from_type(&Type::builtin(BuiltinTypeId::U32)),
        WireType::from_type(&Type::builtin(BuiltinTypeId::U64)),
        WireType::from_type(&Type::builtin(BuiltinTypeId::I8)),
        WireType::from_type(&Type::builtin(BuiltinTypeId::I16)),
        WireType::from_type(&Type::builtin(BuiltinTypeId::I32)),
        WireType::from_type(&Type::builtin(BuiltinTypeId::I64)),
        WireType::from_type(&Type::builtin(BuiltinTypeId::F32)),
        WireType::from_type(&Type::builtin(BuiltinTypeId::F64)),
        WireType::from_type(&Type::builtin(BuiltinTypeId::Bool)),
        WireType::from_type(&Type::builtin(BuiltinTypeId::String)),
        WireType::from_type(&Type::builtin(BuiltinTypeId::Uuid)),
        WireType::from_type(&Type::builtin(BuiltinTypeId::DateTime)),
    ];

    assert_serialized(
        &wire_types,
        json!([
            { "kind": "builtin", "name": "u8" },
            { "kind": "builtin", "name": "u16" },
            { "kind": "builtin", "name": "u32" },
            { "kind": "builtin", "name": "u64" },
            { "kind": "builtin", "name": "i8" },
            { "kind": "builtin", "name": "i16" },
            { "kind": "builtin", "name": "i32" },
            { "kind": "builtin", "name": "i64" },
            { "kind": "builtin", "name": "f32" },
            { "kind": "builtin", "name": "f64" },
            { "kind": "builtin", "name": "bool" },
            { "kind": "builtin", "name": "string" },
            { "kind": "builtin", "name": "uuid" },
            { "kind": "builtin", "name": "datetime" }
        ]),
    );

    let decoded = wire_types
        .iter()
        .map(WireType::to_type)
        .collect::<Result<Vec<_>, _>>()
        .expect("decode wire types");
    assert_eq!(decoded[6], Type::builtin(BuiltinTypeId::I32));
    assert_eq!(decoded[11], Type::builtin(BuiltinTypeId::String));
}

#[test]
fn unary_builtin_containers_serialize_arguments() {
    let elem = Type::builtin(BuiltinTypeId::I32);
    let wire_types = vec![
        WireType::from_type(&Type::list(elem.clone())),
        WireType::from_type(&Type::array(elem.clone())),
        WireType::from_type(&Type::dict(elem.clone())),
        WireType::from_type(&Type::option(elem.clone())),
        WireType::from_type(&Type::promise(elem.clone())),
    ];

    assert_serialized(
        &wire_types,
        json!([
            {
                "kind": "builtin",
                "name": "List",
                "args": [{ "kind": "builtin", "name": "i32" }]
            },
            {
                "kind": "builtin",
                "name": "Array",
                "args": [{ "kind": "builtin", "name": "i32" }]
            },
            {
                "kind": "builtin",
                "name": "Dict",
                "args": [{ "kind": "builtin", "name": "i32" }]
            },
            {
                "kind": "builtin",
                "name": "Option",
                "args": [{ "kind": "builtin", "name": "i32" }]
            },
            {
                "kind": "builtin",
                "name": "Promise",
                "args": [{ "kind": "builtin", "name": "i32" }]
            }
        ]),
    );

    assert_eq!(
        wire_types[1].to_type().expect("decode Array"),
        Type::array(elem)
    );
}

#[test]
fn result_wire_type_uses_user_facing_argument_order() {
    let typ = Type::result(
        Type::builtin(BuiltinTypeId::I32),
        Type::builtin(BuiltinTypeId::String),
    );

    let wire = WireType::from_type(&typ);
    assert_eq!(named_arg_names(&wire), vec!["i32", "string"]);

    let json = assert_serialized(
        &wire,
        json!({
            "kind": "builtin",
            "name": "Result",
            "args": [
                { "kind": "builtin", "name": "i32" },
                { "kind": "builtin", "name": "string" }
            ]
        }),
    );

    let decoded = serde_json::from_value::<WireType>(json)
        .expect("deserialize wire type")
        .to_type()
        .expect("decode wire type");
    assert_eq!(decoded, typ);
}

#[test]
fn partial_result_wire_type_preserves_fixed_error_type() {
    let typ = Type::app(
        Type::builtin(BuiltinTypeId::Result),
        Type::builtin(BuiltinTypeId::String),
    );

    let wire = WireType::from_type(&typ);
    assert_eq!(named_arg_names(&wire), vec!["string"]);
    assert_serialized(
        &wire,
        json!({
            "kind": "builtin",
            "name": "Result",
            "args": [
                { "kind": "builtin", "name": "string" }
            ]
        }),
    );
    assert_eq!(wire.to_type().expect("decode wire type"), typ);
}

#[test]
fn user_defined_type_constructor_serializes_as_named_type() {
    let typ = Type::app(
        Type::user_con("Workflow", 1),
        Type::builtin(BuiltinTypeId::String),
    );
    let wire = WireType::from_type(&typ);

    assert_serialized(
        &wire,
        json!({
            "kind": "named",
            "name": "Workflow",
            "arity": 1,
            "args": [
                { "kind": "builtin", "name": "string" }
            ]
        }),
    );
    assert_eq!(wire.to_type().expect("decode user type"), typ);
}

#[test]
fn tuple_type_serializes_items_in_order() {
    let typ = Type::tuple(vec![
        Type::builtin(BuiltinTypeId::I32),
        Type::builtin(BuiltinTypeId::String),
        Type::option(Type::builtin(BuiltinTypeId::Bool)),
    ]);
    let wire = WireType::from_type(&typ);

    assert_serialized(
        &wire,
        json!({
            "kind": "tuple",
            "items": [
                { "kind": "builtin", "name": "i32" },
                { "kind": "builtin", "name": "string" },
                {
                    "kind": "builtin",
                    "name": "Option",
                    "args": [
                        { "kind": "builtin", "name": "bool" }
                    ]
                }
            ]
        }),
    );
    assert_eq!(wire.to_type().expect("decode tuple"), typ);
}

#[test]
fn unit_type_serializes_as_empty_tuple() {
    let typ = Type::tuple(vec![]);
    let wire = WireType::from_type(&typ);

    assert_serialized(&wire, json!({ "kind": "tuple", "items": [] }));
    assert_eq!(wire.to_type().expect("decode unit"), typ);
}

#[test]
fn function_type_serializes_flat_parameter_list() {
    let typ = Type::fun(
        Type::builtin(BuiltinTypeId::I32),
        Type::fun(
            Type::builtin(BuiltinTypeId::String),
            Type::builtin(BuiltinTypeId::Bool),
        ),
    );
    let wire = WireType::from_type(&typ);

    assert_serialized(
        &wire,
        json!({
            "kind": "fun",
            "params": [
                { "kind": "builtin", "name": "i32" },
                { "kind": "builtin", "name": "string" }
            ],
            "ret": { "kind": "builtin", "name": "bool" }
        }),
    );
    assert_eq!(wire.to_type().expect("decode function"), typ);
}

#[test]
fn function_type_with_function_argument_preserves_nested_function() {
    let arg = Type::fun(
        Type::builtin(BuiltinTypeId::I32),
        Type::builtin(BuiltinTypeId::String),
    );
    let typ = Type::fun(arg, Type::builtin(BuiltinTypeId::Bool));
    let wire = WireType::from_type(&typ);

    assert_serialized(
        &wire,
        json!({
            "kind": "fun",
            "params": [{
                "kind": "fun",
                "params": [{ "kind": "builtin", "name": "i32" }],
                "ret": { "kind": "builtin", "name": "string" }
            }],
            "ret": { "kind": "builtin", "name": "bool" }
        }),
    );
    assert_eq!(wire.to_type().expect("decode higher-order function"), typ);
}

#[test]
fn record_type_from_internal_representation_serializes_sorted_fields() {
    let typ = Type::record(vec![
        (Symbol::intern("z"), Type::builtin(BuiltinTypeId::Bool)),
        (Symbol::intern("a"), Type::builtin(BuiltinTypeId::I32)),
    ]);
    let wire = WireType::from_type(&typ);

    assert_serialized(
        &wire,
        json!({
            "kind": "record",
            "fields": [
                {
                    "name": "a",
                    "type": { "kind": "builtin", "name": "i32" }
                },
                {
                    "name": "z",
                    "type": { "kind": "builtin", "name": "bool" }
                }
            ]
        }),
    );
    assert_eq!(wire.to_type().expect("decode record"), typ);
}

#[test]
fn records_deserialized_from_unsorted_json_decode_to_sorted_internal_fields() {
    let wire = WireType::Record {
        fields: vec![
            WireField {
                name: "z".to_string(),
                typ: WireType::Builtin {
                    name: "bool".to_string(),
                    args: vec![],
                },
            },
            WireField {
                name: "a".to_string(),
                typ: WireType::Builtin {
                    name: "i32".to_string(),
                    args: vec![],
                },
            },
        ],
    };

    assert_serialized(
        &wire,
        json!({
            "kind": "record",
            "fields": [
                {
                    "name": "z",
                    "type": { "kind": "builtin", "name": "bool" }
                },
                {
                    "name": "a",
                    "type": { "kind": "builtin", "name": "i32" }
                }
            ]
        }),
    );
    let typ = wire.to_type().expect("decode record");
    let TypeKind::Record(fields) = typ.as_ref() else {
        panic!("expected record");
    };
    assert_eq!(
        fields
            .iter()
            .map(|(name, _)| name.as_ref())
            .collect::<Vec<_>>(),
        vec!["a", "z"]
    );
}

#[test]
fn free_type_variable_serializes_by_name() {
    let typ = Type::var(TypeVar::new(42, Some(Symbol::intern("item"))));
    let wire = WireType::from_type(&typ);

    assert_serialized(&wire, json!({ "kind": "var", "name": "item" }));
    let decoded = wire.to_type().expect("decode free var");
    let TypeKind::Var(tv) = decoded.as_ref() else {
        panic!("expected decoded type variable");
    };
    assert_eq!(tv.name.as_ref(), Some(&Symbol::intern("item")));
}

#[test]
fn unnamed_type_variable_serializes_with_stable_fallback_name() {
    let typ = Type::var(TypeVar::new(9, None));
    let wire = WireType::from_type(&typ);

    assert_serialized(&wire, json!({ "kind": "var", "name": "t9" }));
    let decoded = wire.to_type().expect("decode unnamed var");
    let TypeKind::Var(tv) = decoded.as_ref() else {
        panic!("expected decoded type variable");
    };
    assert_eq!(tv.name.as_ref(), Some(&Symbol::intern("t9")));
}

#[test]
fn app_fallback_serializes_when_head_is_type_variable() {
    let a = TypeVar::new(0, Some(Symbol::intern("f")));
    let typ = Type::app(Type::var(a), Type::builtin(BuiltinTypeId::I32));
    let wire = WireType::from_type(&typ);

    assert_serialized(
        &wire,
        json!({
            "kind": "app",
            "fun": { "kind": "var", "name": "f" },
            "arg": { "kind": "builtin", "name": "i32" }
        }),
    );
    assert_eq!(wire.to_type().expect("decode app"), typ);
}

#[test]
fn over_applied_constructor_serializes_extra_layer_as_app() {
    let list_i32 = Type::list(Type::builtin(BuiltinTypeId::I32));
    let typ = Type::app(list_i32.clone(), Type::builtin(BuiltinTypeId::String));
    let wire = WireType::from_type(&typ);

    assert_serialized(
        &wire,
        json!({
            "kind": "app",
            "fun": {
                "kind": "builtin",
                "name": "List",
                "args": [
                    { "kind": "builtin", "name": "i32" }
                ]
            },
            "arg": { "kind": "builtin", "name": "string" }
        }),
    );
    assert_eq!(wire.to_type().expect("decode over-applied type"), typ);
}

#[test]
fn monomorphic_scheme_serializes_without_empty_vars_or_constraints() {
    let scheme = Scheme::new(vec![], vec![], Type::builtin(BuiltinTypeId::I32));
    let wire = WireScheme::try_from_scheme(&scheme).expect("encode scheme");

    let json = assert_serialized(
        &wire,
        json!({
            "type": { "kind": "builtin", "name": "i32" }
        }),
    );
    let decoded = serde_json::from_value::<WireScheme>(json)
        .expect("deserialize scheme")
        .to_scheme()
        .expect("decode scheme");
    assert_eq!(decoded, scheme);
}

#[test]
fn scheme_roundtrips_quantified_vars_and_constraints() {
    let a = TypeVar::new(7, Some(Symbol::intern("a")));
    let typ = Type::fun(Type::var(a.clone()), Type::array(Type::var(a.clone())));
    let scheme = Scheme::new(
        vec![a.clone()],
        vec![Predicate::new("Show", Type::var(a.clone()))],
        typ,
    );

    let wire = WireScheme::try_from_scheme(&scheme).expect("encode scheme");
    let json = assert_serialized(
        &wire,
        json!({
            "vars": [{ "name": "a" }],
            "constraints": [{
                "class": "Show",
                "type": { "kind": "var", "name": "a" }
            }],
            "type": {
                "kind": "fun",
                "params": [{ "kind": "var", "name": "a" }],
                "ret": {
                    "kind": "builtin",
                    "name": "Array",
                    "args": [{ "kind": "var", "name": "a" }]
                }
            }
        }),
    );

    let decoded = serde_json::from_value::<WireScheme>(json)
        .expect("deserialize scheme")
        .to_scheme()
        .expect("decode scheme");
    assert_eq!(decoded.typ.to_string(), scheme.typ.to_string());
    assert_eq!(decoded.vars.len(), 1);
    assert_eq!(decoded.vars[0].name.as_ref(), Some(&Symbol::intern("a")));
    assert_eq!(decoded.preds.len(), 1);
    assert_eq!(decoded.preds[0].class.as_ref(), "Show");
}

#[test]
fn scheme_with_multiple_vars_and_constraints_serializes_all_sections() {
    let a = TypeVar::new(0, Some(Symbol::intern("a")));
    let b = TypeVar::new(1, Some(Symbol::intern("b")));
    let typ = Type::fun(
        Type::var(a.clone()),
        Type::result(Type::var(b.clone()), Type::var(a.clone())),
    );
    let scheme = Scheme::new(
        vec![a.clone(), b.clone()],
        vec![
            Predicate::new("Eq", Type::var(a.clone())),
            Predicate::new("Show", Type::var(b.clone())),
        ],
        typ,
    );
    let wire = WireScheme::try_from_scheme(&scheme).expect("encode scheme");

    assert_serialized(
        &wire,
        json!({
            "vars": [{ "name": "a" }, { "name": "b" }],
            "constraints": [
                { "class": "Eq", "type": { "kind": "var", "name": "a" } },
                { "class": "Show", "type": { "kind": "var", "name": "b" } }
            ],
            "type": {
                "kind": "fun",
                "params": [{ "kind": "var", "name": "a" }],
                "ret": {
                    "kind": "builtin",
                    "name": "Result",
                    "args": [
                        { "kind": "var", "name": "b" },
                        { "kind": "var", "name": "a" }
                    ]
                }
            }
        }),
    );
    assert_eq!(
        wire.to_scheme().expect("decode scheme").typ.to_string(),
        scheme.typ.to_string()
    );
}

#[test]
fn duplicate_type_variable_names_are_disambiguated_in_scheme_json() {
    let left = TypeVar::new(3, Some(Symbol::intern("a")));
    let right = TypeVar::new(4, Some(Symbol::intern("a")));
    let scheme = Scheme::new(
        vec![left.clone(), right.clone()],
        vec![],
        Type::tuple(vec![Type::var(left), Type::var(right)]),
    );
    let wire = WireScheme::try_from_scheme(&scheme).expect("encode scheme");

    assert_serialized(
        &wire,
        json!({
            "vars": [{ "name": "a" }, { "name": "a4" }],
            "type": {
                "kind": "tuple",
                "items": [
                    { "kind": "var", "name": "a" },
                    { "kind": "var", "name": "a4" }
                ]
            }
        }),
    );
}

#[test]
fn scheme_rejects_unquantified_vars_after_serializing_the_type_shape() {
    let a = TypeVar::new(0, Some(Symbol::intern("a")));
    let typ = Type::var(a);
    let wire_type = WireType::from_type(&typ);

    assert_serialized(&wire_type, json!({ "kind": "var", "name": "a" }));
    let err = WireScheme::try_from_scheme(&Scheme::new(vec![], vec![], typ))
        .expect_err("unquantified vars should be rejected");
    assert!(
        err.to_string()
            .contains("scheme contains unquantified type variable ids [0]")
    );
}

#[test]
fn adt_decl_with_type_params_serializes_params_and_variants() {
    let mut supply = TypeVarSupply::new();
    let mut maybe = AdtDecl::new(
        &Symbol::intern("MaybeTagged"),
        &[Symbol::intern("a")],
        &mut supply,
    );
    let a = maybe.param_type(&Symbol::intern("a")).expect("param type");
    maybe.add_variant(Symbol::intern("Missing"), vec![]);
    maybe.add_variant(
        Symbol::intern("Present"),
        vec![a, Type::builtin(BuiltinTypeId::String)],
    );

    let wire = WireAdtDecl::try_from_adt_decl(&maybe).expect("encode ADT");
    let json = assert_serialized(
        &wire,
        json!({
            "name": "MaybeTagged",
            "params": ["a"],
            "variants": [
                { "name": "Missing" },
                {
                    "name": "Present",
                    "args": [
                        { "kind": "var", "name": "a" },
                        { "kind": "builtin", "name": "string" }
                    ]
                }
            ]
        }),
    );
    let decoded = serde_json::from_value::<WireAdtDecl>(json)
        .expect("deserialize ADT")
        .to_adt_decl()
        .expect("decode ADT");
    assert_eq!(decoded.name.as_ref(), "MaybeTagged");
    assert_eq!(decoded.params[0].name.as_ref(), "a");
}

#[test]
fn adt_decl_with_record_tuple_and_result_fields_serializes_nested_types() {
    let mut supply = TypeVarSupply::new();
    let mut event = AdtDecl::new(&Symbol::intern("Event"), &[], &mut supply);
    event.add_variant(
        Symbol::intern("Event"),
        vec![
            Type::record(vec![
                (Symbol::intern("id"), Type::builtin(BuiltinTypeId::Uuid)),
                (Symbol::intern("ok"), Type::builtin(BuiltinTypeId::Bool)),
            ]),
            Type::tuple(vec![
                Type::builtin(BuiltinTypeId::I32),
                Type::builtin(BuiltinTypeId::String),
            ]),
            Type::result(
                Type::builtin(BuiltinTypeId::String),
                Type::builtin(BuiltinTypeId::I32),
            ),
        ],
    );
    let wire = WireAdtDecl::try_from_adt_decl(&event).expect("encode ADT");

    assert_serialized(
        &wire,
        json!({
            "name": "Event",
            "variants": [{
                "name": "Event",
                "args": [
                    {
                        "kind": "record",
                        "fields": [
                            {
                                "name": "id",
                                "type": { "kind": "builtin", "name": "uuid" }
                            },
                            {
                                "name": "ok",
                                "type": { "kind": "builtin", "name": "bool" }
                            }
                        ]
                    },
                    {
                        "kind": "tuple",
                        "items": [
                            { "kind": "builtin", "name": "i32" },
                            { "kind": "builtin", "name": "string" }
                        ]
                    },
                    {
                        "kind": "builtin",
                        "name": "Result",
                        "args": [
                            { "kind": "builtin", "name": "string" },
                            { "kind": "builtin", "name": "i32" }
                        ]
                    }
                ]
            }]
        }),
    );
}

#[test]
fn adt_decl_rejects_unbound_field_var_after_serializing_field_type_shape() {
    let mut supply = TypeVarSupply::new();
    let mut bad = AdtDecl::new(&Symbol::intern("Bad"), &[], &mut supply);
    let dangling = Type::var(TypeVar::new(99, Some(Symbol::intern("dangling"))));
    bad.add_variant(Symbol::intern("Bad"), vec![dangling.clone()]);

    assert_serialized(
        &WireType::from_type(&dangling),
        json!({ "kind": "var", "name": "dangling" }),
    );
    let err = WireAdtDecl::try_from_adt_decl(&bad).expect_err("unbound field var");
    assert!(
        err.to_string()
            .contains("ADT `Bad` contains unbound type variable ids [99]")
    );
}

#[test]
fn empty_type_bundle_serializes_only_schema_version() {
    let bundle = TypeBundle {
        schema_version: TYPE_BUNDLE_SCHEMA_VERSION,
        types: Default::default(),
        adts: vec![],
    };

    let json = assert_serialized(&bundle, json!({ "schemaVersion": 1 }));
    let decoded = serde_json::from_value::<TypeBundle>(json)
        .expect("deserialize bundle")
        .into_parts()
        .expect("decode bundle");
    assert!(decoded.0.is_empty());
    assert!(decoded.1.is_empty());
}

#[test]
fn bundle_with_monomorphic_types_serializes_type_map_without_adts() {
    let type_system = TypeSystem::new();
    let bundle = TypeBundle::from_schemes(
        [
            (
                "answer",
                Scheme::new(vec![], vec![], Type::builtin(BuiltinTypeId::I32)),
            ),
            (
                "flag",
                Scheme::new(vec![], vec![], Type::builtin(BuiltinTypeId::Bool)),
            ),
        ],
        &type_system,
    )
    .expect("build bundle");

    assert_serialized(
        &bundle,
        json!({
            "schemaVersion": 1,
            "types": {
                "answer": {
                    "type": { "kind": "builtin", "name": "i32" }
                },
                "flag": {
                    "type": { "kind": "builtin", "name": "bool" }
                }
            }
        }),
    );
}

#[test]
fn bundle_includes_transitive_adt_declarations() {
    let mut type_system = TypeSystem::new();
    let mut supply = TypeVarSupply::new();

    let mut inner = AdtDecl::new(&Symbol::intern("Inner"), &[], &mut supply);
    inner.add_variant(
        Symbol::intern("Inner"),
        vec![Type::builtin(BuiltinTypeId::I32)],
    );
    type_system.register_adt(&inner);

    let mut outer = AdtDecl::new(&Symbol::intern("Outer"), &[], &mut supply);
    outer.add_variant(Symbol::intern("Outer"), vec![Type::user_con("Inner", 0)]);
    type_system.register_adt(&outer);

    let main_scheme = Scheme::new(
        vec![],
        vec![],
        Type::fun(
            Type::user_con("Outer", 0),
            Type::builtin(BuiltinTypeId::String),
        ),
    );
    let bundle = TypeBundle::from_schemes([("main", main_scheme.clone())], &type_system)
        .expect("build type bundle");

    assert_eq!(bundle.schema_version, TYPE_BUNDLE_SCHEMA_VERSION);
    assert_eq!(
        bundle
            .adts
            .iter()
            .map(|adt| adt.name.as_str())
            .collect::<Vec<_>>(),
        vec!["Inner", "Outer"]
    );

    let json = assert_serialized(
        &bundle,
        json!({
            "schemaVersion": 1,
            "types": {
                "main": {
                    "type": {
                        "kind": "fun",
                        "params": [{
                            "kind": "named",
                            "name": "Outer",
                            "arity": 0
                        }],
                        "ret": { "kind": "builtin", "name": "string" }
                    }
                }
            },
            "adts": [
                {
                    "name": "Inner",
                    "variants": [{
                        "name": "Inner",
                        "args": [{ "kind": "builtin", "name": "i32" }]
                    }]
                },
                {
                    "name": "Outer",
                    "variants": [{
                        "name": "Outer",
                        "args": [{ "kind": "named", "name": "Inner", "arity": 0 }]
                    }]
                }
            ]
        }),
    );

    let decoded = serde_json::from_value::<TypeBundle>(json)
        .expect("deserialize bundle")
        .into_parts()
        .expect("decode bundle");
    let (adts, schemes) = decoded;
    assert_eq!(
        adts.iter().map(|adt| adt.name.as_ref()).collect::<Vec<_>>(),
        vec!["Inner", "Outer"]
    );
    assert_eq!(
        schemes.get("main").expect("main scheme").typ,
        main_scheme.typ
    );
}

#[test]
fn bundle_register_into_installs_adts_and_returns_schemes() {
    let bundle = TypeBundle {
        schema_version: TYPE_BUNDLE_SCHEMA_VERSION,
        types: [(
            "run".to_string(),
            WireScheme {
                vars: vec![],
                constraints: vec![],
                typ: WireType::Named {
                    name: "RunSpec".to_string(),
                    arity: 0,
                    args: vec![],
                },
            },
        )]
        .into_iter()
        .collect(),
        adts: vec![WireAdtDecl {
            name: "RunSpec".to_string(),
            params: vec![],
            variants: vec![WireAdtVariant {
                name: "RunSpec".to_string(),
                args: vec![WireType::Builtin {
                    name: "string".to_string(),
                    args: vec![],
                }],
            }],
        }],
    };

    let json = assert_serialized(
        &bundle,
        json!({
            "schemaVersion": 1,
            "types": {
                "run": {
                    "type": {
                        "kind": "named",
                        "name": "RunSpec",
                        "arity": 0
                    }
                }
            },
            "adts": [{
                "name": "RunSpec",
                "variants": [{
                    "name": "RunSpec",
                    "args": [{ "kind": "builtin", "name": "string" }]
                }]
            }]
        }),
    );

    let mut type_system = TypeSystem::new();
    let schemes = serde_json::from_value::<TypeBundle>(json)
        .expect("deserialize bundle")
        .register_into(&mut type_system)
        .expect("register bundle");
    assert!(type_system.adts.contains_key(&Symbol::intern("RunSpec")));
    assert_eq!(
        schemes.get("run").expect("run scheme").typ,
        Type::user_con("RunSpec", 0)
    );
}

#[test]
fn bundle_rejects_missing_referenced_adt() {
    let type_system = TypeSystem::new();
    let scheme = Scheme::new(vec![], vec![], Type::user_con("Missing", 0));
    let wire_scheme = WireScheme::try_from_scheme(&scheme).expect("encode scheme");

    assert_serialized(
        &wire_scheme,
        json!({
            "type": {
                "kind": "named",
                "name": "Missing",
                "arity": 0
            }
        }),
    );
    let err = TypeBundle::from_schemes([("main", scheme)], &type_system)
        .expect_err("missing ADT should fail");
    assert!(err.to_string().contains("unknown type Missing"));
}

#[test]
fn unsupported_bundle_schema_version_is_rejected_after_serialization() {
    let bundle = TypeBundle {
        schema_version: TYPE_BUNDLE_SCHEMA_VERSION + 1,
        types: Default::default(),
        adts: vec![],
    };

    assert_serialized(&bundle, json!({ "schemaVersion": 2 }));
    let err = bundle.into_parts().expect_err("unsupported version");
    assert!(
        err.to_string()
            .contains("unsupported type bundle schema version 2")
    );
}

#[test]
fn wire_type_rejects_builtin_encoded_as_user_type_after_serialization() {
    let wire = WireType::Named {
        name: "i32".to_string(),
        arity: 0,
        args: vec![],
    };

    assert_serialized(&wire, json!({ "kind": "named", "name": "i32", "arity": 0 }));
    let err = wire.to_type().expect_err("builtin encoded as user type");
    assert!(
        err.to_string()
            .contains("builtin type `i32` must use the builtin wire kind")
    );
}

#[test]
fn wire_type_rejects_too_many_builtin_args_after_serialization() {
    let wire = WireType::Builtin {
        name: "List".to_string(),
        args: vec![
            WireType::Builtin {
                name: "i32".to_string(),
                args: vec![],
            },
            WireType::Builtin {
                name: "string".to_string(),
                args: vec![],
            },
        ],
    };

    assert_serialized(
        &wire,
        json!({
            "kind": "builtin",
            "name": "List",
            "args": [
                { "kind": "builtin", "name": "i32" },
                { "kind": "builtin", "name": "string" }
            ]
        }),
    );
    let err = wire.to_type().expect_err("too many builtin args");
    assert!(
        err.to_string()
            .contains("type constructor `List` has arity 1 but got 2 argument(s)")
    );
}

#[test]
fn wire_scheme_rejects_unknown_quantified_var_after_serialization() {
    let wire = WireScheme {
        vars: vec![WireTypeVar {
            name: "a".to_string(),
        }],
        constraints: vec![],
        typ: WireType::Var {
            name: "b".to_string(),
        },
    };

    assert_serialized(
        &wire,
        json!({
            "vars": [{ "name": "a" }],
            "type": { "kind": "var", "name": "b" }
        }),
    );
    let err = wire.to_scheme().expect_err("unknown quantified var");
    assert!(err.to_string().contains("unknown type variable `b`"));
}

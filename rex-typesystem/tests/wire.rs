use rex_ast::Symbol;
use rex_typesystem::{
    types::{
        AdtArgument, AdtDecl, AdtField, BuiltinTypeId, Predicate, Scheme, Type, TypeKind, TypeVar,
    },
    typesystem::{TypeSystem, TypeVarSupply},
    wire::{
        TypeBundle, WireAdtArg, WireAdtDecl, WireAdtVariant, WireField, WireScheme, WireType,
        WireTypeVar, WireValueDecl,
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
        WireType::from_type(&Type::builtin(BuiltinTypeId::Char)),
        WireType::from_type(&Type::builtin(BuiltinTypeId::String)),
        WireType::from_type(&Type::builtin(BuiltinTypeId::Uuid)),
        WireType::from_type(&Type::builtin(BuiltinTypeId::Hash)),
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
            { "kind": "builtin", "name": "Bool" },
            { "kind": "builtin", "name": "Char" },
            { "kind": "builtin", "name": "String" },
            { "kind": "builtin", "name": "UUID" },
            { "kind": "builtin", "name": "Hash" },
            { "kind": "builtin", "name": "DateTime" }
        ]),
    );

    let decoded = wire_types
        .iter()
        .map(WireType::to_type)
        .collect::<Result<Vec<_>, _>>()
        .expect("decode wire types");
    assert_eq!(decoded[6], Type::builtin(BuiltinTypeId::I32));
    assert_eq!(decoded[11], Type::builtin(BuiltinTypeId::Char));
    assert_eq!(decoded[12], Type::builtin(BuiltinTypeId::String));
}

#[test]
fn unary_builtin_containers_serialize_arguments() {
    let elem = Type::builtin(BuiltinTypeId::I32);
    let wire_types = vec![
        WireType::from_type(&Type::list(elem.clone())),
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
        wire_types[0].to_type().expect("decode List"),
        Type::list(elem)
    );
}

#[test]
fn result_wire_type_uses_user_facing_argument_order() {
    let typ = Type::result(
        Type::builtin(BuiltinTypeId::I32),
        Type::builtin(BuiltinTypeId::String),
    );

    let wire = WireType::from_type(&typ);
    assert_eq!(named_arg_names(&wire), vec!["i32", "String"]);

    let json = assert_serialized(
        &wire,
        json!({
            "kind": "builtin",
            "name": "Result",
            "args": [
                { "kind": "builtin", "name": "i32" },
                { "kind": "builtin", "name": "String" }
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
    assert_eq!(named_arg_names(&wire), vec!["String"]);
    assert_serialized(
        &wire,
        json!({
            "kind": "builtin",
            "name": "Result",
            "args": [
                { "kind": "builtin", "name": "String" }
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
                { "kind": "builtin", "name": "String" }
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
                { "kind": "builtin", "name": "String" },
                {
                    "kind": "builtin",
                    "name": "Option",
                    "args": [
                        { "kind": "builtin", "name": "Bool" }
                    ]
                }
            ]
        }),
    );
    assert_eq!(wire.to_type().expect("decode tuple"), typ);
}

#[test]
fn unit_type_serializes_as_empty_tuple() {
    let typ = Type::tuple(Vec::<Type>::new());
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
                { "kind": "builtin", "name": "String" }
            ],
            "ret": { "kind": "builtin", "name": "Bool" }
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
                "ret": { "kind": "builtin", "name": "String" }
            }],
            "ret": { "kind": "builtin", "name": "Bool" }
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
                    "type": { "kind": "builtin", "name": "Bool" }
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
                    name: "Bool".to_string(),
                    args: vec![],
                },
                docs: None,
            },
            WireField {
                name: "a".to_string(),
                typ: WireType::Builtin {
                    name: "i32".to_string(),
                    args: vec![],
                },
                docs: None,
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
                    "type": { "kind": "builtin", "name": "Bool" }
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
fn structural_record_type_rejects_field_documentation_instead_of_dropping_it() {
    let wire = WireType::Record {
        fields: vec![WireField {
            name: "value".to_string(),
            typ: WireType::Builtin {
                name: "i32".to_string(),
                args: vec![],
            },
            docs: Some("A documented field.".to_string()),
        }],
    };

    let error = wire
        .to_type()
        .expect_err("structural record field docs cannot be represented semantically");
    assert!(
        error
            .to_string()
            .contains("structural record field `value` cannot carry documentation")
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
            "arg": { "kind": "builtin", "name": "String" }
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
    let typ = Type::fun(Type::var(a.clone()), Type::list(Type::var(a.clone())));
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
                    "name": "List",
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
    maybe.params[0].docs = Some("The stored value type.".to_string());
    maybe.add_variant(Symbol::intern("Missing"), vec![], None);
    maybe.add_variant(
        Symbol::intern("Present"),
        vec![
            AdtArgument::positional(a),
            AdtArgument::positional(Type::builtin(BuiltinTypeId::String)),
        ],
        None,
    );

    let wire = WireAdtDecl::try_from_adt_decl(&maybe).expect("encode ADT");
    let json = assert_serialized(
        &wire,
        json!({
            "name": "MaybeTagged",
            "params": [{
                "name": "a",
                "docs": "The stored value type."
            }],
            "variants": [
                { "name": "Missing" },
                {
                    "name": "Present",
                    "args": [
                        {
                            "kind": "positional",
                            "type": { "kind": "var", "name": "a" }
                        },
                        {
                            "kind": "positional",
                            "type": { "kind": "builtin", "name": "String" }
                        }
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
    assert_eq!(
        decoded.params[0].docs.as_deref(),
        Some("The stored value type.")
    );
}

#[test]
fn adt_decl_with_record_tuple_and_result_fields_serializes_nested_types() {
    let mut supply = TypeVarSupply::new();
    let mut event = AdtDecl::new(&Symbol::intern("Event"), &[], &mut supply);
    event.add_variant(
        Symbol::intern("Event"),
        vec![
            AdtArgument::positional(Type::record(vec![
                (Symbol::intern("id"), Type::builtin(BuiltinTypeId::Uuid)),
                (Symbol::intern("ok"), Type::builtin(BuiltinTypeId::Bool)),
            ])),
            AdtArgument::positional(Type::tuple(vec![
                Type::builtin(BuiltinTypeId::I32),
                Type::builtin(BuiltinTypeId::String),
            ])),
            AdtArgument::positional(Type::result(
                Type::builtin(BuiltinTypeId::String),
                Type::builtin(BuiltinTypeId::I32),
            )),
        ],
        None,
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
                        "kind": "positional",
                        "type": {
                            "kind": "record",
                            "fields": [
                                {
                                    "name": "id",
                                    "type": { "kind": "builtin", "name": "UUID" }
                                },
                                {
                                    "name": "ok",
                                    "type": { "kind": "builtin", "name": "Bool" }
                                }
                            ]
                        }
                    },
                    {
                        "kind": "positional",
                        "type": {
                            "kind": "tuple",
                            "items": [
                                { "kind": "builtin", "name": "i32" },
                                { "kind": "builtin", "name": "String" }
                            ]
                        }
                    },
                    {
                        "kind": "positional",
                        "type": {
                            "kind": "builtin",
                            "name": "Result",
                            "args": [
                                { "kind": "builtin", "name": "String" },
                                { "kind": "builtin", "name": "i32" }
                            ]
                        }
                    }
                ]
            }]
        }),
    );
}

#[test]
fn adt_documentation_and_structured_record_fields_roundtrip() {
    let mut supply = TypeVarSupply::new();
    let mut event = AdtDecl::new(&Symbol::intern("DocumentedEvent"), &[], &mut supply);
    event.docs = Some("An event exposed by the host.".to_string());
    event.add_variant(
        Symbol::intern("Created"),
        vec![AdtArgument::Record {
            fields: vec![AdtField {
                name: Symbol::intern("id"),
                typ: Type::builtin(BuiltinTypeId::Uuid),
                docs: Some("The stable event identifier.".to_string()),
            }],
            docs: Some("Fields supplied when creating an event.".to_string()),
        }],
        Some("A newly created event.".to_string()),
    );

    let wire = WireAdtDecl::try_from_adt_decl(&event).expect("encode documented ADT");
    let json = serde_json::to_value(&wire).expect("serialize documented ADT");
    assert_eq!(json["docs"], "An event exposed by the host.");
    assert_eq!(json["variants"][0]["docs"], "A newly created event.");
    assert_eq!(
        json["variants"][0]["args"][0]["docs"],
        "Fields supplied when creating an event."
    );
    assert_eq!(
        json["variants"][0]["args"][0]["fields"][0]["docs"],
        "The stable event identifier."
    );

    let decoded = serde_json::from_value::<WireAdtDecl>(json)
        .expect("deserialize documented ADT")
        .to_adt_decl()
        .expect("decode documented ADT");
    assert_eq!(decoded, event);
}

#[test]
fn adt_decl_rejects_unbound_field_var_after_serializing_field_type_shape() {
    let mut supply = TypeVarSupply::new();
    let mut bad = AdtDecl::new(&Symbol::intern("Bad"), &[], &mut supply);
    let dangling = Type::var(TypeVar::new(99, Some(Symbol::intern("dangling"))));
    bad.add_variant(
        Symbol::intern("Bad"),
        vec![AdtArgument::positional(dangling.clone())],
        None,
    );

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
fn empty_type_bundle_serializes_as_an_empty_object() {
    let bundle = TypeBundle {
        docs: None,
        values: Default::default(),
        adts: vec![],
    };

    let json = assert_serialized(&bundle, json!({}));
    let decoded = serde_json::from_value::<TypeBundle>(json)
        .expect("deserialize bundle")
        .into_parts()
        .expect("decode bundle");
    assert!(decoded.docs.is_none());
    assert!(decoded.adts.is_empty());
    assert!(decoded.values.is_empty());
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
            "values": {
                "answer": [{
                    "scheme": {
                        "type": { "kind": "builtin", "name": "i32" }
                    }
                }],
                "flag": [{
                    "scheme": {
                        "type": { "kind": "builtin", "name": "Bool" }
                    }
                }]
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
        vec![AdtArgument::positional(Type::builtin(BuiltinTypeId::I32))],
        None,
    );
    type_system.register_adt(&inner).expect("register Inner");

    let mut outer = AdtDecl::new(&Symbol::intern("Outer"), &[], &mut supply);
    outer.add_variant(
        Symbol::intern("Outer"),
        vec![AdtArgument::positional(Type::user_con("Inner", 0))],
        None,
    );
    type_system.register_adt(&outer).expect("register Outer");

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
            "values": {
                "main": [{
                    "scheme": {
                        "type": {
                            "kind": "fun",
                            "params": [{
                                "kind": "named",
                                "name": "Outer",
                                "arity": 0
                            }],
                            "ret": { "kind": "builtin", "name": "String" }
                        }
                    },
                    "params": ["arg0"]
                }]
            },
            "adts": [
                {
                    "name": "Inner",
                    "variants": [{
                        "name": "Inner",
                        "args": [{
                            "kind": "positional",
                            "type": { "kind": "builtin", "name": "i32" }
                        }]
                    }]
                },
                {
                    "name": "Outer",
                    "variants": [{
                        "name": "Outer",
                        "args": [{
                            "kind": "positional",
                            "type": { "kind": "named", "name": "Inner", "arity": 0 }
                        }]
                    }]
                }
            ]
        }),
    );

    let decoded = serde_json::from_value::<TypeBundle>(json)
        .expect("deserialize bundle")
        .into_parts()
        .expect("decode bundle");
    assert!(decoded.docs.is_none());
    assert_eq!(
        decoded
            .adts
            .iter()
            .map(|adt| adt.name.as_ref())
            .collect::<Vec<_>>(),
        vec!["Inner", "Outer"]
    );
    assert_eq!(
        decoded.values.get("main").expect("main value")[0]
            .scheme
            .typ,
        main_scheme.typ
    );
}

#[test]
fn bundle_register_into_installs_adts_and_returns_values() {
    let bundle = TypeBundle {
        docs: Some("Runnable APIs.".to_string()),
        values: [(
            "run".to_string(),
            vec![WireValueDecl {
                scheme: WireScheme {
                    vars: vec![],
                    constraints: vec![],
                    typ: WireType::Named {
                        name: "RunSpec".to_string(),
                        arity: 0,
                        args: vec![],
                    },
                },
                params: vec![],
                docs: Some("Run a specification.".to_string()),
            }],
        )]
        .into_iter()
        .collect(),
        adts: vec![WireAdtDecl {
            name: "RunSpec".to_string(),
            params: vec![],
            variants: vec![WireAdtVariant {
                name: "RunSpec".to_string(),
                args: vec![WireAdtArg::Positional {
                    typ: WireType::Builtin {
                        name: "String".to_string(),
                        args: vec![],
                    },
                    docs: Some("The source text.".to_string()),
                }],
                docs: Some("Construct a run specification.".to_string()),
            }],
            docs: Some("A run specification.".to_string()),
        }],
    };

    let json = assert_serialized(
        &bundle,
        json!({
            "docs": "Runnable APIs.",
            "values": {
                "run": [{
                    "scheme": {
                        "type": {
                            "kind": "named",
                            "name": "RunSpec",
                            "arity": 0
                        }
                    },
                    "docs": "Run a specification."
                }]
            },
            "adts": [{
                "name": "RunSpec",
                "variants": [{
                    "name": "RunSpec",
                    "args": [{
                        "kind": "positional",
                        "type": { "kind": "builtin", "name": "String" },
                        "docs": "The source text."
                    }],
                    "docs": "Construct a run specification."
                }],
                "docs": "A run specification."
            }]
        }),
    );

    let mut type_system = TypeSystem::new();
    let registered = serde_json::from_value::<TypeBundle>(json)
        .expect("deserialize bundle")
        .register_into(&mut type_system)
        .expect("register bundle");
    assert!(type_system.adts.contains_key(&Symbol::intern("RunSpec")));
    assert_eq!(registered.docs.as_deref(), Some("Runnable APIs."));
    assert_eq!(
        registered.values.get("run").expect("run value")[0]
            .scheme
            .typ,
        Type::user_con("RunSpec", 0)
    );
    assert_eq!(
        registered.values.get("run").expect("run value")[0]
            .docs
            .as_deref(),
        Some("Run a specification.")
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
                name: "String".to_string(),
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
                { "kind": "builtin", "name": "String" }
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

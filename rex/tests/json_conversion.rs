use rex::{
    AdtDecl, EngineError, EnumPatch, Heap, JsonOptions, Type, TypeSystem, expr_to_json, intern,
    json_to_expr, sym,
};
use rex_ts::TypeVarSupply;
use serde_json::json;

fn mk_type_system() -> TypeSystem {
    TypeSystem::with_prelude().unwrap()
}

fn mk_unit_enum(name: &str, variants: &[&str]) -> AdtDecl {
    let mut supply = TypeVarSupply::new();
    let mut adt = AdtDecl::new(&intern(name), &[], &mut supply);
    for variant in variants {
        adt.add_variant(intern(variant), vec![]);
    }
    adt
}

#[test]
fn primitive_roundtrip() {
    let ts = mk_type_system();
    let heap = Heap::new();
    let opts = JsonOptions::default();

    let cases = vec![
        (Type::con("bool", 0), json!(true)),
        (Type::con("i32", 0), json!(-7)),
        (Type::con("string", 0), json!("hello")),
    ];

    for (ty, expected_json) in cases {
        let ptr = json_to_expr(&heap, &expected_json, &ty, &ts, &opts).unwrap();
        let actual_json = expr_to_json(&heap, &ptr, &ty, &ts, &opts).unwrap();
        assert_eq!(actual_json, expected_json);
    }
}

#[test]
fn option_and_result_roundtrip() {
    let ts = mk_type_system();
    let heap = Heap::new();
    let opts = JsonOptions::default();

    let opt_ty = Type::option(Type::con("i32", 0));
    let some = json!(9);
    let none = serde_json::Value::Null;

    let some_ptr = json_to_expr(&heap, &some, &opt_ty, &ts, &opts).unwrap();
    let none_ptr = json_to_expr(&heap, &none, &opt_ty, &ts, &opts).unwrap();
    assert_eq!(
        expr_to_json(&heap, &some_ptr, &opt_ty, &ts, &opts).unwrap(),
        some
    );
    assert_eq!(
        expr_to_json(&heap, &none_ptr, &opt_ty, &ts, &opts).unwrap(),
        none
    );

    let res_ty = Type::result(Type::con("i32", 0), Type::con("string", 0));
    let ok_json = json!({ "Ok": 1 });
    let err_json = json!({ "Err": "bad" });
    let ok_ptr = json_to_expr(&heap, &ok_json, &res_ty, &ts, &opts).unwrap();
    let err_ptr = json_to_expr(&heap, &err_json, &res_ty, &ts, &opts).unwrap();
    assert_eq!(
        expr_to_json(&heap, &ok_ptr, &res_ty, &ts, &opts).unwrap(),
        ok_json
    );
    assert_eq!(
        expr_to_json(&heap, &err_ptr, &res_ty, &ts, &opts).unwrap(),
        err_json
    );
}

#[test]
fn json_array_maps_to_array_not_list() {
    let ts = mk_type_system();
    let heap = Heap::new();
    let opts = JsonOptions::default();
    let array_json = json!([1, 2, 3]);

    let array_ty = Type::array(Type::con("i32", 0));
    let array_ptr = json_to_expr(&heap, &array_json, &array_ty, &ts, &opts).unwrap();
    let items = heap.pointer_as_array(&array_ptr).unwrap();
    assert_eq!(items.len(), 3);
    assert_eq!(
        expr_to_json(&heap, &array_ptr, &array_ty, &ts, &opts).unwrap(),
        array_json
    );

    let list_ty = Type::list(Type::con("i32", 0));
    let list_ptr = json_to_expr(&heap, &array_json, &list_ty, &ts, &opts).unwrap();
    let (tag, _args) = heap.pointer_as_adt(&list_ptr).unwrap();
    assert_eq!(tag.as_ref(), "Cons");
}

#[test]
fn struct_like_single_variant_adt_roundtrip() {
    let mut ts = mk_type_system();
    let heap = Heap::new();
    let opts = JsonOptions::default();

    let mut supply = TypeVarSupply::new();
    let mut foo = AdtDecl::new(&sym("Foo"), &[], &mut supply);
    foo.add_variant(
        sym("Foo"),
        vec![Type::record(vec![
            (sym("a"), Type::con("u64", 0)),
            (sym("b"), Type::con("string", 0)),
        ])],
    );
    ts.inject_adt(&foo);

    let foo_ty = Type::con("Foo", 0);
    let foo_json = json!({ "a": 42, "b": "Hello" });

    let foo_ptr = json_to_expr(&heap, &foo_json, &foo_ty, &ts, &opts).unwrap();
    let (tag, args) = heap.pointer_as_adt(&foo_ptr).unwrap();
    assert_eq!(tag.as_ref(), "Foo");
    assert_eq!(args.len(), 1);
    assert_eq!(
        expr_to_json(&heap, &foo_ptr, &foo_ty, &ts, &opts).unwrap(),
        foo_json
    );
}

#[test]
fn unit_enum_string_roundtrip() {
    let mut ts = mk_type_system();
    let heap = Heap::new();
    let opts = JsonOptions::default();

    let color = mk_unit_enum("Color", &["Red", "Green", "Blue"]);
    ts.inject_adt(&color);
    let color_ty = Type::con("Color", 0);

    for v in [json!("Red"), json!("Green"), json!("Blue")] {
        let ptr = json_to_expr(&heap, &v, &color_ty, &ts, &opts).unwrap();
        let actual = expr_to_json(&heap, &ptr, &color_ty, &ts, &opts).unwrap();
        assert_eq!(actual, v);
    }
}

#[test]
fn unit_enum_integer_roundtrip_with_patches() {
    let mut ts = mk_type_system();
    let heap = Heap::new();
    let color = mk_unit_enum("Color", &["Red", "Green", "Blue"]);
    ts.inject_adt(&color);
    let color_ty = Type::con("Color", 0);

    let mut opts = JsonOptions::default();
    opts.add_int_enum("Color");

    let red = heap.alloc_adt(sym("Red"), vec![]).unwrap();
    let green = heap.alloc_adt(sym("Green"), vec![]).unwrap();
    let blue = heap.alloc_adt(sym("Blue"), vec![]).unwrap();
    assert_eq!(
        expr_to_json(&heap, &red, &color_ty, &ts, &opts).unwrap(),
        json!(0)
    );
    assert_eq!(
        expr_to_json(&heap, &green, &color_ty, &ts, &opts).unwrap(),
        json!(1)
    );
    assert_eq!(
        expr_to_json(&heap, &blue, &color_ty, &ts, &opts).unwrap(),
        json!(2)
    );

    let ptr = json_to_expr(&heap, &json!(2), &color_ty, &ts, &opts).unwrap();
    let (tag, args) = heap.pointer_as_adt(&ptr).unwrap();
    assert_eq!(tag.as_ref(), "Blue");
    assert!(args.is_empty());

    opts.add_int_enum_with_patches(
        "Color",
        vec![
            EnumPatch {
                enum_name: "Red".to_string(),
                discriminant: 10,
            },
            EnumPatch {
                enum_name: "Blue".to_string(),
                discriminant: 42,
            },
        ],
    );

    assert_eq!(
        expr_to_json(&heap, &red, &color_ty, &ts, &opts).unwrap(),
        json!(10)
    );
    assert_eq!(
        expr_to_json(&heap, &green, &color_ty, &ts, &opts).unwrap(),
        json!(1)
    );
    assert_eq!(
        expr_to_json(&heap, &blue, &color_ty, &ts, &opts).unwrap(),
        json!(42)
    );

    let blue_from_patch = json_to_expr(&heap, &json!(42), &color_ty, &ts, &opts).unwrap();
    let (tag, args) = heap.pointer_as_adt(&blue_from_patch).unwrap();
    assert_eq!(tag.as_ref(), "Blue");
    assert!(args.is_empty());
}

#[test]
fn unit_enum_integer_unknown_discriminant_errors() {
    let mut ts = mk_type_system();
    let heap = Heap::new();
    let color = mk_unit_enum("Color", &["Red", "Green", "Blue"]);
    ts.inject_adt(&color);
    let color_ty = Type::con("Color", 0);

    let mut opts = JsonOptions::default();
    opts.add_int_enum("Color");

    let err = json_to_expr(&heap, &json!(99), &color_ty, &ts, &opts).unwrap_err();
    let EngineError::Custom(msg) = err else {
        panic!("expected EngineError::Custom");
    };
    assert!(msg.contains("expected integer enum JSON for `Color`"));
}

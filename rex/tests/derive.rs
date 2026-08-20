mod common;

use rex::{
    Rex,
    ast::Symbol,
    engine::{Builder, EngineError, FromRex, IntoRex, Module, Value, virtual_export_name},
    json::rex_to_json,
    parser::parse as parse_rex,
    typesystem::{BuiltinTypeId, RexAdt, RexType, Type},
};
use serde::Serialize;
use std::collections::HashMap;

/// A documented generic API type.
#[derive(Rex, Debug, PartialEq)]
enum Documented<
    #[allow(unused_doc_comments)]
    /// The payload type stored by this API value.
    T,
> {
    /// No payload is available.
    Missing,
    /// A tuple-shaped payload.
    Present(
        /// The stored payload.
        T,
        /// A human-readable label.
        String,
    ),
    /// A record-shaped payload.
    Record {
        /// The numeric value exposed under its Rex name.
        #[serde(rename = "renamed")]
        value: i32,
    },
}

#[test]
fn derive_preserves_rust_docs_for_adts_variants_and_fields() {
    let adt = Documented::<i32>::rex_adt_decl().unwrap();
    assert_eq!(adt.docs.as_deref(), Some("A documented generic API type."));
    assert_eq!(
        adt.params[0].docs.as_deref(),
        Some("The payload type stored by this API value.")
    );

    let missing = adt
        .variants
        .iter()
        .find(|variant| variant.name.as_ref() == "Missing")
        .unwrap();
    assert_eq!(missing.docs.as_deref(), Some("No payload is available."));

    let present = adt
        .variants
        .iter()
        .find(|variant| variant.name.as_ref() == "Present")
        .unwrap();
    assert_eq!(present.docs.as_deref(), Some("A tuple-shaped payload."));
    assert_eq!(present.args[0].docs(), Some("The stored payload."));
    assert_eq!(present.args[1].docs(), Some("A human-readable label."));

    let record = adt
        .variants
        .iter()
        .find(|variant| variant.name.as_ref() == "Record")
        .unwrap();
    let rex::typesystem::AdtArgument::Record { fields, .. } = &record.args[0] else {
        panic!("expected structured record argument");
    };
    assert_eq!(fields[0].name.as_ref(), "renamed");
    assert_eq!(
        fields[0].docs.as_deref(),
        Some("The numeric value exposed under its Rex name.")
    );
}

#[test]
fn named_module_preserves_derived_generic_parameter_docs() {
    let adt = Documented::<i32>::rex_adt_decl().unwrap();
    let mut module = Module::new("host.documented_generic", None);
    module.add_adt_decl(adt).unwrap();

    let staged = module.declarations();
    assert_eq!(
        staged.types[0].params[0].docs.as_deref(),
        Some("The payload type stored by this API value.")
    );

    let mut builder = Builder::with_prelude(()).unwrap();
    builder.inject_module(module).unwrap();
    let qualified_name = Symbol::intern(&virtual_export_name(
        "host.documented_generic",
        "Documented",
    ));
    let registered = builder
        .type_system()
        .adts
        .get(&qualified_name)
        .expect("registered generic ADT");
    assert_eq!(
        registered.params[0].docs.as_deref(),
        Some("The payload type stored by this API value.")
    );
}

async fn eval(code: &str) -> Result<((), Value, Type), EngineError> {
    let mut builder = Builder::with_prelude(())?;
    MyInnerStruct::inject_rex(&mut builder)?;
    MyStruct::inject_rex(&mut builder)?;
    Boxed::<i32>::inject_rex(&mut builder)?;
    Maybe::<i32>::inject_rex(&mut builder)?;
    Shape::inject_rex(&mut builder)?;
    common::eval_source(builder, code).await
}

#[derive(Rex, Debug, PartialEq, Serialize, Clone)]
struct MyInnerStruct {
    x: bool,
    y: i32,
}

#[derive(Rex, Debug, PartialEq, Serialize, Clone)]
struct MyStruct {
    x: bool,
    y: i32,
    tags: Vec<String>,
    props: HashMap<String, i32>,
    #[serde(default = "xxx")] // should have no effect
    inner: MyInnerStruct,
    #[serde(alias = "ignore")] // should have no effect
    pair: (i32, String, bool),
    #[serde(rename = "renamed")]
    renamed_field: i32,
}

#[derive(Rex, Debug, PartialEq)]
struct Boxed<T> {
    value: T,
}

#[derive(Rex, Clone, Debug, PartialEq)]
struct HashedValue {
    hash: blake3::Hash,
}

#[derive(Rex, Debug, PartialEq)]
enum Maybe<T> {
    Just(T),
    Nothing,
}

#[test]
fn derive_from_rex_rejects_qualified_foreign_constructor() {
    let value = Value::Adt(Symbol::intern("another.module.Just"), vec![Value::I32(1)]);
    assert!(Maybe::<i32>::from_rex(value).is_err());
}

#[test]
fn derive_treats_hash_as_a_primitive_field() {
    let expected = HashedValue {
        hash: blake3::hash(b"derived hash"),
    };
    let family = HashedValue::rex_adt_family().unwrap();
    assert_eq!(family.len(), 1);

    let value = expected.clone().into_rex().unwrap();
    assert_eq!(HashedValue::from_rex(value).unwrap(), expected);
}

#[derive(Rex, Debug, PartialEq)]
struct SharedLeaf {
    value: i32,
}

#[derive(Rex, Debug, PartialEq)]
struct LeftBranch {
    leaf: SharedLeaf,
}

#[derive(Rex, Debug, PartialEq)]
struct RightBranch {
    leaf: SharedLeaf,
}

#[derive(Rex, Debug, PartialEq)]
struct RootNode {
    left: LeftBranch,
    right: RightBranch,
}

#[derive(Debug, PartialEq, Clone)]
struct AtomRef(i32);

impl RexType for AtomRef {
    fn rex_type() -> Type {
        i32::rex_type()
    }
}

impl IntoRex for AtomRef {
    fn into_rex(self) -> Result<Value, EngineError> {
        self.0.into_rex()
    }
}

impl FromRex for AtomRef {
    fn from_rex(value: Value) -> Result<Self, EngineError> {
        Ok(Self(i32::from_rex(value)?))
    }
}

#[derive(Rex, Debug, PartialEq)]
struct Fragment(Vec<AtomRef>);

#[derive(Rex, Debug, PartialEq)]
struct VecFieldSnapshot {
    values: Vec<i32>,
}

#[derive(Debug, PartialEq, Clone)]
struct Xyzf32([f32; 3]);

impl RexType for Xyzf32 {
    fn rex_type() -> Type {
        Type::tuple(vec![f32::rex_type(), f32::rex_type(), f32::rex_type()])
    }
}

impl IntoRex for Xyzf32 {
    fn into_rex(self) -> Result<Value, EngineError> {
        (self.0[0], self.0[1], self.0[2]).into_rex()
    }
}

impl FromRex for Xyzf32 {
    fn from_rex(value: Value) -> Result<Self, EngineError> {
        let (x, y, z) = <(f32, f32, f32)>::from_rex(value)?;
        Ok(Self([x, y, z]))
    }
}

#[derive(Rex, Debug, PartialEq)]
struct BoundingBox {
    min: Xyzf32,
    max: Xyzf32,
}

#[tokio::test]
async fn derive_struct_roundtrip_value() {
    let (_heap, v_handle, ty) = eval(
        r#"
        MyStruct {
            x = true,
            y = 42,
            tags = ["a", "b", "c"],
            props = { a = 1, b = 2 },
            inner = MyInnerStruct { x = false, y = 7 },
            pair = (1, "hi", true),
            renamed = 9
        }
        "#,
    )
    .await
    .unwrap();
    assert_eq!(ty, MyStruct::rex_type());

    let decoded = MyStruct::from_rex(v_handle).unwrap();
    assert_eq!(
        decoded,
        MyStruct {
            x: true,
            y: 42,
            tags: vec!["a".into(), "b".into(), "c".into()],
            props: HashMap::from([("a".into(), 1), ("b".into(), 2)]),
            inner: MyInnerStruct { x: false, y: 7 },
            pair: (1, "hi".into(), true),
            renamed_field: 9,
        }
    );
}

#[tokio::test]
async fn derive_generic_struct_roundtrip_value() {
    let (_heap, v_handle, ty) = eval("Boxed { value = 123 }").await.unwrap();
    assert_eq!(ty, Boxed::<i32>::rex_type());
    let decoded = Boxed::<i32>::from_rex(v_handle).unwrap();
    assert_eq!(decoded, Boxed { value: 123 });
}

#[tokio::test]
async fn derive_struct_eval_json_matches_rust_serde_json() {
    let code = r#"
        MyStruct {
            x = true,
            y = 42,
            tags = ["a", "b", "c"],
            props = { a = 1, b = 2 },
            inner = MyInnerStruct { x = false, y = 7 },
            pair = (1, "hi", true),
            renamed = 9
        }
    "#;

    let expected = serde_json::json!({
        "x": true,
        "y": 42,
        "tags": ["a", "b", "c"],
        "props": { "a": 1, "b": 2 },
        "inner": { "x": false, "y": 7 },
        "pair": [1, "hi", true],
        "renamed": 9
    });

    let program = parse_rex(code).unwrap();

    let mut builder = Builder::with_prelude(()).unwrap();
    MyInnerStruct::inject_rex(&mut builder).unwrap();
    MyStruct::inject_rex(&mut builder).unwrap();
    let type_system = builder.type_system().clone();
    let (v_handle, ty) = common::run_program(builder, &program).await.unwrap();

    let actual_rex = rex_to_json(&v_handle, &ty, &type_system).unwrap();

    let actual_serde = serde_json::to_value(MyStruct {
        x: true,
        y: 42,
        tags: vec!["a".into(), "b".into(), "c".into()],
        props: HashMap::from([("a".into(), 1), ("b".into(), 2)]),
        inner: MyInnerStruct { x: false, y: 7 },
        pair: (1, "hi".into(), true),
        renamed_field: 9,
    })
    .unwrap();

    assert_eq!(actual_rex, expected);
    assert_eq!(actual_serde, expected);
}

#[tokio::test]
async fn derive_generic_worked_example_polymorphic_adt() {
    // Worked example: `Maybe<T>` is injected into Rex once, but constructors stay polymorphic.
    //
    // The proc-macro generates *both*:
    // - `RexType` for Rust values (e.g. `Maybe<i32>` -> `Maybe i32`)
    // - an `AdtDecl` with a type parameter `T` (so `Just` has scheme `a -> Maybe a`)
    let mut builder = Builder::with_prelude(()).unwrap();

    // Build the ADT surface (params + variants) and sanity-check that it really uses a type var.
    let adt = Maybe::<i32>::rex_adt_decl().unwrap();
    assert_eq!(adt.name.as_ref(), "Maybe");
    assert_eq!(adt.params.len(), 1);

    let t = adt
        .param_type(&Symbol::intern("T"))
        .expect("expected `T` param type");

    let just = adt
        .variants
        .iter()
        .find(|v| v.name.as_ref() == "Just")
        .expect("expected `Just` variant");
    assert_eq!(
        just.args.iter().map(|arg| arg.typ()).collect::<Vec<_>>(),
        vec![t.clone()]
    );

    let nothing = adt
        .variants
        .iter()
        .find(|v| v.name.as_ref() == "Nothing")
        .expect("expected `Nothing` variant");
    assert!(nothing.args.is_empty());

    // Inject the ADT once: constructor *schemes* are registered in the type system, and runtime
    // constructor *functions* are registered in the evaluator.
    let mut module = Module::global();
    module.add_adt_decl(adt).unwrap();
    builder.inject_module(module).unwrap();

    // On the Rust side, `RexType` is the nominal head applied to the Rust generic arguments.
    assert_eq!(
        Maybe::<i32>::rex_type(),
        Type::app(Type::con("Maybe", 1), <i32 as RexType>::rex_type())
    );
    assert_eq!(
        Maybe::<bool>::rex_type(),
        Type::app(Type::con("Maybe", 1), <bool as RexType>::rex_type())
    );

    // On the Rex side, `Just` stays polymorphic because the injected `AdtDecl` used a type var `T`
    // in the argument type. That lets the same constructor be used at multiple instantiations.
    let program = parse_rex(
        r#"
        let id = \x -> Just x in
            (id 1, id true)
        "#,
    )
    .map_err(|errs| format!("parse error: {errs:?}"))
    .unwrap();

    let (v_handle, ty) = common::run_program(builder, &program).await.unwrap();
    let expected_ty = Type::tuple(vec![Maybe::<i32>::rex_type(), Maybe::<bool>::rex_type()]);
    assert_eq!(ty, expected_ty);
    let Value::Tuple(items) = v_handle else {
        panic!("expected tuple");
    };
    assert_eq!(items.len(), 2);
    assert_eq!(
        Maybe::<i32>::from_rex(items[0].clone()).unwrap(),
        Maybe::Just(1)
    );
    assert_eq!(
        Maybe::<bool>::from_rex(items[1].clone()).unwrap(),
        Maybe::Just(true)
    );
}

#[derive(Rex, Debug, PartialEq, Clone)]
enum Shape {
    Rectangle(i32, i32),
    Circle(i32),
}

#[tokio::test]
async fn derive_can_be_used_in_injected_native_functions() {
    let program = parse_rex(
        r#"
        bump_y (MyStruct {
            x = true,
            y = 42,
            tags = ["a", "b", "c"],
            props = { a = 1, b = 2 },
            inner = MyInnerStruct { x = false, y = 7 },
            pair = (1, "hi", true),
            renamed = 9
        })
        "#,
    )
    .unwrap();

    fn builder_with_struct_exports() -> Builder {
        let mut builder = Builder::with_prelude(()).unwrap();
        MyInnerStruct::inject_rex(&mut builder).unwrap();
        MyStruct::inject_rex(&mut builder).unwrap();

        common::inject_globals(&mut builder, |module| {
            module.export("bump_y", |_: (), mut s: MyStruct| {
                s.y += 1;
                Ok(s)
            })
        })
        .unwrap();
        common::inject_globals(&mut builder, |module| {
            module.export_value(
                "const_struct",
                MyStruct {
                    x: false,
                    y: 100,
                    tags: vec![],
                    props: HashMap::new(),
                    inner: MyInnerStruct { x: true, y: 1 },
                    pair: (2, "ok".into(), false),
                    renamed_field: 0,
                },
            )
        })
        .unwrap();
        builder
    }

    let (v_handle, ty) = common::run_program(builder_with_struct_exports(), &program)
        .await
        .unwrap();
    assert_eq!(ty, MyStruct::rex_type());
    let bumped = MyStruct::from_rex(v_handle).unwrap();
    assert_eq!(bumped.y, 43);

    let program = parse_rex("const_struct.y").unwrap();
    let (v, ty) = common::run_program(builder_with_struct_exports(), &program)
        .await
        .unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    assert_eq!(v.as_i32().unwrap(), 100);
}

#[tokio::test]
async fn derive_enum_can_be_injected_as_value_and_pattern_matched() {
    let mut builder = Builder::with_prelude(()).unwrap();
    Shape::inject_rex(&mut builder).unwrap();

    common::inject_globals(&mut builder, |module| {
        module.export_value("shape", Shape::Rectangle(3, 4))
    })
    .unwrap();

    let program = parse_rex(
        r#"
        match shape with {
            case Rectangle w h -> w * h;
            case Circle r -> r;
        }
        "#,
    )
    .unwrap();
    let (v, ty) = common::run_program(builder, &program).await.unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    assert_eq!(v.as_i32().unwrap(), 12);
}

#[tokio::test]
async fn derive_types_implement_rex_adt_trait() {
    fn assert_derived_traits<T: Rex + RexAdt>() {}
    assert_derived_traits::<Shape>();

    let mut builder = Builder::with_prelude(()).unwrap();
    builder.inject_rex_adt::<Shape>().unwrap();

    let program = parse_rex(
        r#"
        match (Rectangle 2 5) with {
            case Rectangle w h -> w * h;
            case Circle r -> r;
        }
        "#,
    )
    .unwrap();
    let (v, ty) = common::run_program(builder, &program).await.unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    assert_eq!(v.as_i32().unwrap(), 10);
}

#[tokio::test]
async fn derive_generic_enum_can_be_used_as_injected_fn_arg_and_return() {
    let mut builder = Builder::with_prelude(()).unwrap();
    Maybe::<i32>::inject_rex(&mut builder).unwrap();

    common::inject_globals(&mut builder, |module| {
        module.export("unwrap_or_zero", |_: (), m: Maybe<i32>| {
            Ok(match m {
                Maybe::Just(v) => v,
                Maybe::Nothing => 0,
            })
        })
    })
    .unwrap();

    let program = parse_rex("(unwrap_or_zero (Just 5), unwrap_or_zero Nothing)").unwrap();
    let (v_handle, ty) = common::run_program(builder, &program).await.unwrap();
    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::builtin(BuiltinTypeId::I32),
            Type::builtin(BuiltinTypeId::I32)
        ])
    );
    let Value::Tuple(items) = v_handle else {
        panic!("expected tuple");
    };
    assert_eq!(items[0].as_i32().unwrap(), 5);
    assert_eq!(items[1].as_i32().unwrap(), 0);
}

#[tokio::test]
async fn derive_enum_constructor_currying() {
    let (_heap, v_handle, ty) = eval(
        r#"
        let partial = Rectangle (2 * 3) in
            (partial (3 * 4), partial (2 * 4))
        "#,
    )
    .await
    .unwrap();
    assert_eq!(ty, Type::tuple(vec![Shape::rex_type(), Shape::rex_type()]));

    let Value::Tuple(items) = v_handle else {
        panic!("expected tuple");
    };
    assert_eq!(items.len(), 2);
    let a = Shape::from_rex(items[0].clone()).unwrap();
    let b = Shape::from_rex(items[1].clone()).unwrap();
    assert_eq!(a, Shape::Rectangle(6, 12));
    assert_eq!(b, Shape::Rectangle(6, 8));
}

#[tokio::test]
async fn derive_inject_rex_registers_acyclic_dependency_closure() {
    let mut builder = Builder::with_prelude(()).unwrap();
    RootNode::inject_rex(&mut builder).unwrap();

    let adts = &builder.type_system().adts;
    assert!(adts.contains_key(&Symbol::intern("SharedLeaf")));
    assert!(adts.contains_key(&Symbol::intern("LeftBranch")));
    assert!(adts.contains_key(&Symbol::intern("RightBranch")));
    assert!(adts.contains_key(&Symbol::intern("RootNode")));

    let program = parse_rex(
        r#"
        RootNode {
            left = LeftBranch { leaf = SharedLeaf { value = 1 } },
            right = RightBranch { leaf = SharedLeaf { value = 2 } }
        }
        "#,
    )
    .unwrap();
    let (v_handle, ty) = common::run_program(builder, &program).await.unwrap();

    assert_eq!(ty, RootNode::rex_type());
    let decoded = RootNode::from_rex(v_handle).unwrap();
    assert_eq!(
        decoded,
        RootNode {
            left: LeftBranch {
                leaf: SharedLeaf { value: 1 },
            },
            right: RightBranch {
                leaf: SharedLeaf { value: 2 },
            },
        }
    );
}

#[test]
fn derive_vec_fields_serialize_and_deserialize_as_lists() {
    fn assert_values(values: Vec<i32>, expected: &[i32]) {
        let adt = VecFieldSnapshot::rex_adt_decl().unwrap();
        assert_eq!(
            adt.variants[0]
                .args
                .iter()
                .map(|arg| arg.typ())
                .collect::<Vec<_>>(),
            vec![Type::record(vec![(
                Symbol::intern("values"),
                Type::list(Type::builtin(BuiltinTypeId::I32)),
            )])]
        );

        let snapshot = VecFieldSnapshot {
            values: values.clone(),
        }
        .into_rex()
        .unwrap();
        let Value::Adt(tag, args) = &snapshot else {
            panic!("expected VecFieldSnapshot ADT");
        };
        assert_eq!(tag.as_ref(), "VecFieldSnapshot");
        assert_eq!(args.len(), 1);

        let Value::Dict(fields) = &args[0] else {
            panic!("expected record payload");
        };
        let list = fields.get("values").expect("expected `values` field");
        let actual = Vec::<i32>::from_rex(list.clone()).unwrap();

        assert_eq!(actual, expected);

        let decoded = VecFieldSnapshot::from_rex(snapshot).unwrap();
        assert_eq!(decoded, VecFieldSnapshot { values });
    }

    assert_values(vec![], &[]);
    assert_values(vec![7], &[7]);
    assert_values(vec![1, 2, 3], &[1, 2, 3]);
}

#[tokio::test]
async fn derive_leaf_rex_type_field_does_not_require_rex_adt_dependency() {
    let mut builder = Builder::with_prelude(()).unwrap();
    Fragment::inject_rex(&mut builder).unwrap();

    let program = parse_rex("Fragment [1, 2, 3]").unwrap();
    let (v_handle, ty) = common::run_program(builder, &program).await.unwrap();

    assert_eq!(ty, Fragment::rex_type());
    let decoded = Fragment::from_rex(v_handle).unwrap();
    assert_eq!(decoded, Fragment(vec![AtomRef(1), AtomRef(2), AtomRef(3)]));
}

#[tokio::test]
async fn derive_leaf_rex_type_record_fields_support_manual_leaf_types() {
    let mut builder = Builder::with_prelude(()).unwrap();
    BoundingBox::inject_rex(&mut builder).unwrap();

    let program =
        parse_rex("BoundingBox { min = (1.0, 2.0, 3.0), max = (4.0, 5.0, 6.0) }").unwrap();
    let (v_handle, ty) = common::run_program(builder, &program).await.unwrap();

    assert_eq!(ty, BoundingBox::rex_type());
    let decoded = BoundingBox::from_rex(v_handle).unwrap();
    assert_eq!(
        decoded,
        BoundingBox {
            min: Xyzf32([1.0, 2.0, 3.0]),
            max: Xyzf32([4.0, 5.0, 6.0]),
        }
    );
}

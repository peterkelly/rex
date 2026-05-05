use rex::{
    Rex,
    ast::Symbol,
    engine::{Engine, EngineError, FromRex, Handle, Heap, IntoRex, Module, Value},
    json::{JsonOptions, rex_to_json},
    parser::{Parser, Token},
    typesystem::{BuiltinTypeId, RexType, Type},
};
use serde::Serialize;
use std::collections::HashMap;

fn inject_globals(
    engine: &mut Engine<()>,
    build: impl FnOnce(&mut Module<()>) -> Result<(), EngineError>,
) -> Result<(), EngineError> {
    let mut module = Module::global();
    build(&mut module)?;
    engine.inject_module(module)
}

async fn eval(code: &str) -> Result<(Heap, Handle, Type), EngineError> {
    let tokens = Token::tokenize(code).unwrap();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().unwrap();

    let mut engine = Engine::with_prelude(())?;
    MyInnerStruct::inject_rex(&mut engine)?;
    MyStruct::inject_rex(&mut engine)?;
    Boxed::<i32>::inject_rex(&mut engine)?;
    Maybe::<i32>::inject_rex(&mut engine)?;
    Shape::inject_rex(&mut engine)?;

    let mut module = Module::global();
    module.add_decls(program.decls.clone());
    engine.inject_module(module)?;
    let heap = engine.heap.clone();
    let (handle, ty) = engine
        .into_evaluator()
        .eval(program.expr.as_ref())
        .await
        .map_err(|err| err.into_engine_error())?;
    Ok((heap, handle, ty))
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

#[derive(Rex, Debug, PartialEq)]
enum Maybe<T> {
    Just(T),
    Nothing,
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
    fn into_rex(self, heap: &Heap) -> Result<Handle, EngineError> {
        self.0.into_rex(heap)
    }
}

impl FromRex for AtomRef {
    fn from_rex(handle: &Handle) -> Result<Self, EngineError> {
        Ok(Self(i32::from_rex(handle)?))
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
    fn into_rex(self, heap: &Heap) -> Result<Handle, EngineError> {
        (self.0[0], self.0[1], self.0[2]).into_rex(heap)
    }
}

impl FromRex for Xyzf32 {
    fn from_rex(handle: &Handle) -> Result<Self, EngineError> {
        let (x, y, z) = <(f32, f32, f32)>::from_rex(handle)?;
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
            tags = to_array ["a", "b", "c"],
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

    let decoded = MyStruct::from_rex(&v_handle).unwrap();
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
    let decoded = Boxed::<i32>::from_rex(&v_handle).unwrap();
    assert_eq!(decoded, Boxed { value: 123 });
}

#[tokio::test]
async fn derive_struct_eval_json_matches_rust_serde_json() {
    let code = r#"
        MyStruct {
            x = true,
            y = 42,
            tags = to_array ["a", "b", "c"],
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

    let tokens = Token::tokenize(code).unwrap();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().unwrap();

    let mut engine = Engine::with_prelude(()).unwrap();
    MyInnerStruct::inject_rex(&mut engine).unwrap();
    MyStruct::inject_rex(&mut engine).unwrap();
    let mut module = Module::global();
    module.add_decls(program.decls.clone());
    engine.inject_module(module).unwrap();
    let type_system = engine.type_system.clone();
    let (v_handle, ty) = engine
        .into_evaluator()
        .eval(program.expr.as_ref())
        .await
        .unwrap();

    let actual_rex = rex_to_json(&v_handle, &ty, &type_system, &JsonOptions::default()).unwrap();

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
    let mut engine = Engine::with_prelude(()).unwrap();

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
    assert_eq!(just.args, vec![t.clone()]);

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
    engine.inject_module(module).unwrap();

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
    let tokens = Token::tokenize(
        r#"
        let id = \x -> Just x in
            (id 1, id true)
        "#,
    )
    .map_err(|e| format!("lex error: {e}"))
    .unwrap();
    let mut parser = Parser::new(tokens);
    let program = parser
        .parse_program()
        .map_err(|errs| format!("parse error: {errs:?}"))
        .unwrap();

    let mut module = Module::global();
    module.add_decls(program.decls.clone());
    engine.inject_module(module).unwrap();
    let (v_handle, ty) = engine
        .into_evaluator()
        .eval(program.expr.as_ref())
        .await
        .unwrap();
    let expected_ty = Type::tuple(vec![Maybe::<i32>::rex_type(), Maybe::<bool>::rex_type()]);
    assert_eq!(ty, expected_ty);
    let v = v_handle.value().unwrap();

    let Value::Tuple(items) = v else {
        panic!("expected tuple, got {}", v_handle.type_name().unwrap());
    };
    assert_eq!(items.len(), 2);
    assert_eq!(Maybe::<i32>::from_rex(&items[0]).unwrap(), Maybe::Just(1));
    assert_eq!(
        Maybe::<bool>::from_rex(&items[1]).unwrap(),
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
    let tokens = Token::tokenize(
        r#"
        bump_y (MyStruct {
            x = true,
            y = 42,
            tags = to_array ["a", "b", "c"],
            props = { a = 1, b = 2 },
            inner = MyInnerStruct { x = false, y = 7 },
            pair = (1, "hi", true),
            renamed = 9
        })
        "#,
    )
    .unwrap();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().unwrap();

    fn engine_with_struct_exports() -> Engine {
        let mut engine = Engine::with_prelude(()).unwrap();
        MyInnerStruct::inject_rex(&mut engine).unwrap();
        MyStruct::inject_rex(&mut engine).unwrap();

        inject_globals(&mut engine, |module| {
            module.export("bump_y", |_: &(), mut s: MyStruct| {
                s.y += 1;
                Ok(s)
            })
        })
        .unwrap();
        inject_globals(&mut engine, |module| {
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
        engine
    }

    let (v_handle, ty) = engine_with_struct_exports()
        .into_evaluator()
        .eval(program.expr.as_ref())
        .await
        .unwrap();
    assert_eq!(ty, MyStruct::rex_type());
    let bumped = MyStruct::from_rex(&v_handle).unwrap();
    assert_eq!(bumped.y, 43);

    let tokens = Token::tokenize("const_struct.y").unwrap();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().unwrap();
    let (v, ty) = engine_with_struct_exports()
        .into_evaluator()
        .eval(program.expr.as_ref())
        .await
        .unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    assert_eq!(v.as_i32().unwrap(), 100);
}

#[tokio::test]
async fn derive_enum_can_be_injected_as_value_and_pattern_matched() {
    let mut engine = Engine::with_prelude(()).unwrap();
    Shape::inject_rex(&mut engine).unwrap();

    inject_globals(&mut engine, |module| {
        module.export_value("shape", Shape::Rectangle(3, 4))
    })
    .unwrap();

    let tokens = Token::tokenize(
        r#"
        match shape with {
            when Rectangle w h -> w * h;
            when Circle r -> r;
        }
        "#,
    )
    .unwrap();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().unwrap();
    let (v, ty) = engine
        .into_evaluator()
        .eval(program.expr.as_ref())
        .await
        .unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    assert_eq!(v.as_i32().unwrap(), 12);
}

#[tokio::test]
async fn derive_types_implement_rex_adt_trait() {
    let mut engine = Engine::with_prelude(()).unwrap();
    engine.inject_rex_adt::<Shape>().unwrap();

    let tokens = Token::tokenize(
        r#"
        match (Rectangle 2 5) with {
            when Rectangle w h -> w * h;
            when Circle r -> r;
        }
        "#,
    )
    .unwrap();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().unwrap();
    let (v, ty) = engine
        .into_evaluator()
        .eval(program.expr.as_ref())
        .await
        .unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    assert_eq!(v.as_i32().unwrap(), 10);
}

#[tokio::test]
async fn derive_generic_enum_can_be_used_as_injected_fn_arg_and_return() {
    let mut engine = Engine::with_prelude(()).unwrap();
    Maybe::<i32>::inject_rex(&mut engine).unwrap();

    inject_globals(&mut engine, |module| {
        module.export("unwrap_or_zero", |_: &(), m: Maybe<i32>| {
            Ok(match m {
                Maybe::Just(v) => v,
                Maybe::Nothing => 0,
            })
        })
    })
    .unwrap();

    let tokens = Token::tokenize("(unwrap_or_zero (Just 5), unwrap_or_zero Nothing)").unwrap();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().unwrap();
    let (v_handle, ty) = engine
        .into_evaluator()
        .eval(program.expr.as_ref())
        .await
        .unwrap();
    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::builtin(BuiltinTypeId::I32),
            Type::builtin(BuiltinTypeId::I32)
        ])
    );
    let v = v_handle.value().unwrap();
    let Value::Tuple(items) = v else {
        panic!("expected tuple, got {}", v_handle.type_name().unwrap());
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

    let value = v_handle.value().unwrap();
    let Value::Tuple(items) = value else {
        panic!("expected tuple, got {}", v_handle.type_name().unwrap());
    };
    assert_eq!(items.len(), 2);
    let a = Shape::from_rex(&items[0]).unwrap();
    let b = Shape::from_rex(&items[1]).unwrap();
    assert_eq!(a, Shape::Rectangle(6, 12));
    assert_eq!(b, Shape::Rectangle(6, 8));
}

#[tokio::test]
async fn derive_inject_rex_registers_acyclic_dependency_closure() {
    let mut engine = Engine::with_prelude(()).unwrap();
    RootNode::inject_rex(&mut engine).unwrap();

    assert!(
        engine
            .type_system
            .adts
            .contains_key(&Symbol::intern("SharedLeaf"))
    );
    assert!(
        engine
            .type_system
            .adts
            .contains_key(&Symbol::intern("LeftBranch"))
    );
    assert!(
        engine
            .type_system
            .adts
            .contains_key(&Symbol::intern("RightBranch"))
    );
    assert!(
        engine
            .type_system
            .adts
            .contains_key(&Symbol::intern("RootNode"))
    );

    let tokens = Token::tokenize(
        r#"
        RootNode {
            left = LeftBranch { leaf = SharedLeaf { value = 1 } },
            right = RightBranch { leaf = SharedLeaf { value = 2 } }
        }
        "#,
    )
    .unwrap();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().unwrap();
    let (v_handle, ty) = engine
        .into_evaluator()
        .eval(program.expr.as_ref())
        .await
        .unwrap();

    assert_eq!(ty, RootNode::rex_type());
    let decoded = RootNode::from_rex(&v_handle).unwrap();
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
fn derive_vec_fields_serialize_and_deserialize_as_arrays() {
    fn array_from_values(heap: &Heap, values: &[i32]) -> Handle {
        let items = values
            .iter()
            .map(|value| heap.alloc_i32(*value).unwrap())
            .collect();
        heap.alloc_array(items).unwrap()
    }

    fn assert_values(values: Vec<i32>, expected: &[i32]) {
        let heap = Heap::new();
        let adt = VecFieldSnapshot::rex_adt_decl().unwrap();
        assert_eq!(
            adt.variants[0].args,
            vec![Type::record(vec![(
                Symbol::intern("values"),
                Type::array(Type::builtin(BuiltinTypeId::I32)),
            )])]
        );

        let snapshot = VecFieldSnapshot {
            values: values.clone(),
        }
        .into_rex(&heap)
        .unwrap();
        let Value::Adt(tag, args) = snapshot.value().unwrap() else {
            panic!("expected VecFieldSnapshot ADT");
        };
        assert_eq!(tag.as_ref(), "VecFieldSnapshot");
        assert_eq!(args.len(), 1);

        let Value::Dict(fields) = args[0].value().unwrap() else {
            panic!("expected record payload");
        };
        let array_handle = fields
            .get(&Symbol::intern("values"))
            .expect("expected `values` field");
        let Value::Array(array_items) = array_handle.value().unwrap() else {
            panic!("expected array field");
        };
        let actual = array_items
            .iter()
            .map(|item| item.to_rust::<i32>().unwrap())
            .collect::<Vec<_>>();

        assert_eq!(actual, expected);

        let mut fields = std::collections::BTreeMap::new();
        fields.insert(Symbol::intern("values"), array_from_values(&heap, expected));
        let handle = heap
            .alloc_adt(
                Symbol::intern("VecFieldSnapshot"),
                vec![heap.alloc_dict(fields).unwrap()],
            )
            .unwrap();
        let decoded = VecFieldSnapshot::from_rex(&handle).unwrap();
        assert_eq!(decoded, VecFieldSnapshot { values });
    }

    assert_values(vec![], &[]);
    assert_values(vec![7], &[7]);
    assert_values(vec![1, 2, 3], &[1, 2, 3]);
}

#[tokio::test]
async fn derive_leaf_rex_type_field_does_not_require_rex_adt_dependency() {
    let mut engine = Engine::with_prelude(()).unwrap();
    Fragment::inject_rex(&mut engine).unwrap();

    let tokens = Token::tokenize("Fragment [1, 2, 3]").unwrap();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().unwrap();
    let (v_handle, ty) = engine
        .into_evaluator()
        .eval(program.expr.as_ref())
        .await
        .unwrap();

    assert_eq!(ty, Fragment::rex_type());
    let decoded = Fragment::from_rex(&v_handle).unwrap();
    assert_eq!(decoded, Fragment(vec![AtomRef(1), AtomRef(2), AtomRef(3)]));
}

#[tokio::test]
async fn derive_leaf_rex_type_record_fields_support_manual_leaf_types() {
    let mut engine = Engine::with_prelude(()).unwrap();
    BoundingBox::inject_rex(&mut engine).unwrap();

    let tokens =
        Token::tokenize("BoundingBox { min = (1.0, 2.0, 3.0), max = (4.0, 5.0, 6.0) }").unwrap();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().unwrap();
    let (v_handle, ty) = engine
        .into_evaluator()
        .eval(program.expr.as_ref())
        .await
        .unwrap();

    assert_eq!(ty, BoundingBox::rex_type());
    let decoded = BoundingBox::from_rex(&v_handle).unwrap();
    assert_eq!(
        decoded,
        BoundingBox {
            min: Xyzf32([1.0, 2.0, 3.0]),
            max: Xyzf32([4.0, 5.0, 6.0]),
        }
    );
}

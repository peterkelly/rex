use rex::{
    BuiltinTypeId, Engine, EngineError, FromPointer, GasMeter, Heap, IntoPointer, JsonOptions,
    Module, Parser, Pointer, Rex, RexAdt, RexType, Token, Type, Value, assert_pointer_eq,
    rex_to_json,
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

async fn eval(code: &str) -> Result<(Heap, Pointer, Type), EngineError> {
    let tokens = Token::tokenize(code).unwrap();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program(&mut GasMeter::default()).unwrap();

    let mut engine = Engine::with_prelude(())?;
    MyInnerStruct::inject_rex(&mut engine)?;
    MyStruct::inject_rex(&mut engine)?;
    Boxed::<i32>::inject_rex(&mut engine)?;
    Maybe::<i32>::inject_rex(&mut engine)?;
    Shape::inject_rex(&mut engine)?;

    let mut module = Module::global();
    module.add_decls(program.decls.clone());
    engine.inject_module(module)?;
    let mut gas = GasMeter::default();
    let (pointer, ty) = rex::Evaluator::new_with_compiler(
        rex::RuntimeEnv::new(engine.clone()),
        rex::Compiler::new(engine.clone()),
    )
    .eval(program.expr.as_ref(), &mut gas)
    .await
    .map_err(|err| err.into_engine_error())?;
    let heap = engine.into_heap();
    Ok((heap, pointer, ty))
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

impl IntoPointer for AtomRef {
    fn into_pointer(self, heap: &rex::Heap) -> Result<Pointer, EngineError> {
        self.0.into_pointer(heap)
    }
}

impl FromPointer for AtomRef {
    fn from_pointer(heap: &rex::Heap, pointer: &Pointer) -> Result<Self, EngineError> {
        Ok(Self(i32::from_pointer(heap, pointer)?))
    }
}

#[derive(Rex, Debug, PartialEq)]
struct Fragment(Vec<AtomRef>);

#[derive(Debug, PartialEq, Clone)]
struct Xyzf32([f32; 3]);

impl RexType for Xyzf32 {
    fn rex_type() -> Type {
        Type::tuple(vec![f32::rex_type(), f32::rex_type(), f32::rex_type()])
    }
}

impl IntoPointer for Xyzf32 {
    fn into_pointer(self, heap: &rex::Heap) -> Result<Pointer, EngineError> {
        (self.0[0], self.0[1], self.0[2]).into_pointer(heap)
    }
}

impl FromPointer for Xyzf32 {
    fn from_pointer(heap: &rex::Heap, pointer: &Pointer) -> Result<Self, EngineError> {
        let (x, y, z) = <(f32, f32, f32)>::from_pointer(heap, pointer)?;
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
    let (heap, v_ptr, ty) = eval(
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

    let decoded = MyStruct::from_pointer(&heap, &v_ptr).unwrap();
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
    let (heap, v_ptr, ty) = eval("Boxed { value = 123 }").await.unwrap();
    assert_eq!(ty, Boxed::<i32>::rex_type());
    let decoded = Boxed::<i32>::from_pointer(&heap, &v_ptr).unwrap();
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

    let tokens = Token::tokenize(code).unwrap();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program(&mut GasMeter::default()).unwrap();

    let mut engine = Engine::with_prelude(()).unwrap();
    MyInnerStruct::inject_rex(&mut engine).unwrap();
    MyStruct::inject_rex(&mut engine).unwrap();
    let mut module = Module::global();
    module.add_decls(program.decls.clone());
    engine.inject_module(module).unwrap();

    let mut gas = GasMeter::default();
    let (v_ptr, ty) = rex::Evaluator::new_with_compiler(
        rex::RuntimeEnv::new(engine.clone()),
        rex::Compiler::new(engine.clone()),
    )
    .eval(program.expr.as_ref(), &mut gas)
    .await
    .unwrap();

    let actual_rex = rex_to_json(
        &engine.heap,
        &v_ptr,
        &ty,
        &engine.type_system,
        &JsonOptions::default(),
    )
    .unwrap();

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
        .param_type(&rex::intern("T"))
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
        .parse_program(&mut GasMeter::default())
        .map_err(|errs| format!("parse error: {errs:?}"))
        .unwrap();

    let mut module = Module::global();
    module.add_decls(program.decls.clone());
    engine.inject_module(module).unwrap();
    let mut gas = GasMeter::default();
    let (v_ptr, ty) = rex::Evaluator::new_with_compiler(
        rex::RuntimeEnv::new(engine.clone()),
        rex::Compiler::new(engine.clone()),
    )
    .eval(program.expr.as_ref(), &mut gas)
    .await
    .unwrap();
    let expected_ty = Type::tuple(vec![Maybe::<i32>::rex_type(), Maybe::<bool>::rex_type()]);
    assert_eq!(ty, expected_ty);
    let v = engine
        .heap
        .get(&v_ptr)
        .map(|value| value.as_ref().clone())
        .unwrap();

    let Value::Tuple(items) = v else {
        panic!(
            "expected tuple, got {}",
            engine.heap.type_name(&v_ptr).unwrap()
        );
    };
    assert_eq!(items.len(), 2);
    assert_eq!(
        Maybe::<i32>::from_pointer(&engine.heap, &items[0]).unwrap(),
        Maybe::Just(1)
    );
    assert_eq!(
        Maybe::<bool>::from_pointer(&engine.heap, &items[1]).unwrap(),
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
            tags = ["a", "b", "c"],
            props = { a = 1, b = 2 },
            inner = MyInnerStruct { x = false, y = 7 },
            pair = (1, "hi", true),
            renamed = 9
        })
        "#,
    )
    .unwrap();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program(&mut GasMeter::default()).unwrap();

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

    let mut gas = GasMeter::default();
    let (v_ptr, ty) = rex::Evaluator::new_with_compiler(
        rex::RuntimeEnv::new(engine.clone()),
        rex::Compiler::new(engine.clone()),
    )
    .eval(program.expr.as_ref(), &mut gas)
    .await
    .unwrap();
    assert_eq!(ty, MyStruct::rex_type());
    let bumped = MyStruct::from_pointer(&engine.heap, &v_ptr).unwrap();
    assert_eq!(bumped.y, 43);

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
    let tokens = Token::tokenize("const_struct.y").unwrap();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program(&mut GasMeter::default()).unwrap();
    let (v, ty) = rex::Evaluator::new_with_compiler(
        rex::RuntimeEnv::new(engine.clone()),
        rex::Compiler::new(engine.clone()),
    )
    .eval(program.expr.as_ref(), &mut gas)
    .await
    .unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    let heap = &engine.heap;
    assert_pointer_eq!(heap, v, heap.alloc_i32(100).unwrap());
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
        match shape
            when Rectangle w h -> w * h
            when Circle r -> r
        "#,
    )
    .unwrap();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program(&mut GasMeter::default()).unwrap();

    let mut gas = GasMeter::default();
    let (v, ty) = rex::Evaluator::new_with_compiler(
        rex::RuntimeEnv::new(engine.clone()),
        rex::Compiler::new(engine.clone()),
    )
    .eval(program.expr.as_ref(), &mut gas)
    .await
    .unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    let heap = &engine.heap;
    assert_pointer_eq!(heap, v, heap.alloc_i32(12).unwrap());
}

#[tokio::test]
async fn derive_types_implement_rex_adt_trait() {
    let mut engine = Engine::with_prelude(()).unwrap();
    <Shape as RexAdt>::inject_rex(&mut engine).unwrap();

    let tokens = Token::tokenize(
        r#"
        match (Rectangle 2 5)
            when Rectangle w h -> w * h
            when Circle r -> r
        "#,
    )
    .unwrap();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program(&mut GasMeter::default()).unwrap();

    let mut gas = GasMeter::default();
    let (v, ty) = rex::Evaluator::new_with_compiler(
        rex::RuntimeEnv::new(engine.clone()),
        rex::Compiler::new(engine.clone()),
    )
    .eval(program.expr.as_ref(), &mut gas)
    .await
    .unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    assert_pointer_eq!(&engine.heap, v, engine.heap.alloc_i32(10).unwrap());
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
    let mut gas = GasMeter::default();
    let program = parser.parse_program(&mut gas).unwrap();
    let (v_ptr, ty) = rex::Evaluator::new_with_compiler(
        rex::RuntimeEnv::new(engine.clone()),
        rex::Compiler::new(engine.clone()),
    )
    .eval(program.expr.as_ref(), &mut gas)
    .await
    .unwrap();
    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::builtin(BuiltinTypeId::I32),
            Type::builtin(BuiltinTypeId::I32)
        ])
    );
    let v = engine
        .heap
        .get(&v_ptr)
        .map(|value| value.as_ref().clone())
        .unwrap();
    let Value::Tuple(items) = v else {
        panic!(
            "expected tuple, got {}",
            engine.heap.type_name(&v_ptr).unwrap()
        );
    };
    let items = items
        .into_iter()
        .map(|item| {
            engine
                .heap
                .get(&item)
                .map(|value| value.as_ref().clone())
                .unwrap()
        })
        .collect::<Vec<_>>();
    let heap = &engine.heap;
    assert_pointer_eq!(
        heap,
        heap.alloc_value(items[0].clone()).unwrap(),
        heap.alloc_i32(5).unwrap()
    );
    assert_pointer_eq!(
        heap,
        heap.alloc_value(items[1].clone()).unwrap(),
        heap.alloc_i32(0).unwrap()
    );
}

#[tokio::test]
async fn derive_enum_constructor_currying() {
    let (heap, v_ptr, ty) = eval(
        r#"
        let partial = Rectangle (2 * 3) in
            (partial (3 * 4), partial (2 * 4))
        "#,
    )
    .await
    .unwrap();
    assert_eq!(ty, Type::tuple(vec![Shape::rex_type(), Shape::rex_type()]));

    let value = heap.get(&v_ptr).unwrap().as_ref().clone();
    let Value::Tuple(items) = value else {
        panic!("expected tuple, got {}", heap.type_name(&v_ptr).unwrap());
    };
    assert_eq!(items.len(), 2);
    let a = Shape::from_pointer(&heap, &items[0]).unwrap();
    let b = Shape::from_pointer(&heap, &items[1]).unwrap();
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
            .contains_key(&rex::sym("SharedLeaf"))
    );
    assert!(
        engine
            .type_system
            .adts
            .contains_key(&rex::sym("LeftBranch"))
    );
    assert!(
        engine
            .type_system
            .adts
            .contains_key(&rex::sym("RightBranch"))
    );
    assert!(engine.type_system.adts.contains_key(&rex::sym("RootNode")));

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
    let program = parser.parse_program(&mut GasMeter::default()).unwrap();

    let mut gas = GasMeter::default();
    let (v_ptr, ty) = rex::Evaluator::new_with_compiler(
        rex::RuntimeEnv::new(engine.clone()),
        rex::Compiler::new(engine.clone()),
    )
    .eval(program.expr.as_ref(), &mut gas)
    .await
    .unwrap();

    assert_eq!(ty, RootNode::rex_type());
    let decoded = RootNode::from_pointer(&engine.heap, &v_ptr).unwrap();
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

#[tokio::test]
async fn derive_leaf_rex_type_field_does_not_require_rex_adt_dependency() {
    let mut engine = Engine::with_prelude(()).unwrap();
    Fragment::inject_rex(&mut engine).unwrap();

    let tokens = Token::tokenize("Fragment [1, 2, 3]").unwrap();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program(&mut GasMeter::default()).unwrap();

    let mut gas = GasMeter::default();
    let (v_ptr, ty) = rex::Evaluator::new_with_compiler(
        rex::RuntimeEnv::new(engine.clone()),
        rex::Compiler::new(engine.clone()),
    )
    .eval(program.expr.as_ref(), &mut gas)
    .await
    .unwrap();

    assert_eq!(ty, Fragment::rex_type());
    let decoded = Fragment::from_pointer(&engine.heap, &v_ptr).unwrap();
    assert_eq!(decoded, Fragment(vec![AtomRef(1), AtomRef(2), AtomRef(3)]));
}

#[tokio::test]
async fn derive_leaf_rex_type_record_fields_support_manual_leaf_types() {
    let mut engine = Engine::with_prelude(()).unwrap();
    BoundingBox::inject_rex(&mut engine).unwrap();

    let tokens =
        Token::tokenize("BoundingBox { min = (1.0, 2.0, 3.0), max = (4.0, 5.0, 6.0) }").unwrap();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program(&mut GasMeter::default()).unwrap();

    let mut gas = GasMeter::default();
    let (v_ptr, ty) = rex::Evaluator::new_with_compiler(
        rex::RuntimeEnv::new(engine.clone()),
        rex::Compiler::new(engine.clone()),
    )
    .eval(program.expr.as_ref(), &mut gas)
    .await
    .unwrap();

    assert_eq!(ty, BoundingBox::rex_type());
    let decoded = BoundingBox::from_pointer(&engine.heap, &v_ptr).unwrap();
    assert_eq!(
        decoded,
        BoundingBox {
            min: Xyzf32([1.0, 2.0, 3.0]),
            max: Xyzf32([4.0, 5.0, 6.0]),
        }
    );
}

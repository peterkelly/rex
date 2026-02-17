use rex::{Engine, EngineError, GasMeter, Heap, Parser, Pointer, Token, Type};

fn register_integer_literal_natives(engine: &mut Engine<()>) -> Result<(), EngineError> {
    engine.export("num_u8", |_state: &(), x: u8| Ok(format!("{x}:u8")))?;
    engine.export("num_u16", |_state: &(), x: u16| Ok(format!("{x}:u16")))?;
    engine.export("num_u32", |_state: &(), x: u32| Ok(format!("{x}:u32")))?;
    engine.export("num_u64", |_state: &(), x: u64| Ok(format!("{x}:u64")))?;
    engine.export("num_i8", |_state: &(), x: i8| Ok(format!("{x}:i8")))?;
    engine.export("num_i16", |_state: &(), x: i16| Ok(format!("{x}:i16")))?;
    engine.export("num_i32", |_state: &(), x: i32| Ok(format!("{x}:i32")))?;
    engine.export("num_i64", |_state: &(), x: i64| Ok(format!("{x}:i64")))?;
    Ok(())
}

async fn eval(code: &str) -> Result<(Heap, Pointer, Type), EngineError> {
    let tokens = Token::tokenize(code).unwrap();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program(&mut GasMeter::default()).unwrap();

    let mut engine = Engine::with_prelude(()).unwrap();
    register_integer_literal_natives(&mut engine)?;
    engine.inject_decls(&program.decls)?;

    let mut gas = GasMeter::default();
    let (pointer, ty) = engine.eval(program.expr.as_ref(), &mut gas).await?;
    let heap = engine.into_heap();
    Ok((heap, pointer, ty))
}

fn expected_values() -> Vec<&'static str> {
    vec![
        "4:u8", "4:u16", "4:u32", "4:u64", "4:i8", "4:i16", "4:i32", "4:i64",
    ]
}

fn expected_type() -> Type {
    Type::tuple(vec![
        Type::con("string", 0),
        Type::con("string", 0),
        Type::con("string", 0),
        Type::con("string", 0),
        Type::con("string", 0),
        Type::con("string", 0),
        Type::con("string", 0),
        Type::con("string", 0),
    ])
}

fn expected_negative_signed_values() -> Vec<&'static str> {
    vec!["-3:i8", "-3:i16", "-3:i32", "-3:i64"]
}

fn expected_negative_signed_type() -> Type {
    Type::tuple(vec![
        Type::con("string", 0),
        Type::con("string", 0),
        Type::con("string", 0),
        Type::con("string", 0),
    ])
}

fn assert_tuple_of_strings(heap: &Heap, pointer: &Pointer) {
    let parts = heap.pointer_as_tuple(pointer).unwrap();
    let got: Vec<String> = parts
        .iter()
        .map(|p| heap.pointer_as_string(p).unwrap())
        .collect();
    let expected = expected_values()
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    assert_eq!(got, expected);
}

fn assert_tuple_of_negative_signed_strings(heap: &Heap, pointer: &Pointer) {
    let parts = heap.pointer_as_tuple(pointer).unwrap();
    let got: Vec<String> = parts
        .iter()
        .map(|p| heap.pointer_as_string(p).unwrap())
        .collect();
    let expected = expected_negative_signed_values()
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    assert_eq!(got, expected);
}

fn assert_type_error(err: EngineError) {
    assert!(
        matches!(err, EngineError::Type(_)),
        "expected type error, got {err}"
    );
}

#[tokio::test]
async fn integer_literal_calls_each_num_function_directly() {
    let code = r#"
(
  num_u8 4,
  num_u16 4,
  num_u32 4,
  num_u64 4,
  num_i8 4,
  num_i16 4,
  num_i32 4,
  num_i64 4
)
"#;
    let (heap, pointer, ty) = eval(code).await.unwrap();
    assert_eq!(ty, expected_type());
    assert_tuple_of_strings(&heap, &pointer);
}

#[tokio::test]
async fn negative_integer_literal_calls_each_signed_num_function_directly() {
    let code = r#"
(
  num_i8 (-3),
  num_i16 (-3),
  num_i32 (-3),
  num_i64 (-3)
)
"#;
    let (heap, pointer, ty) = eval(code).await.unwrap();
    assert_eq!(ty, expected_negative_signed_type());
    assert_tuple_of_negative_signed_strings(&heap, &pointer);
}

#[tokio::test]
async fn negative_integer_literal_direct_unsigned_calls_are_type_errors() {
    for code in [
        "num_u8 (-3)",
        "num_u16 (-3)",
        "num_u32 (-3)",
        "num_u64 (-3)",
    ] {
        match eval(code).await {
            Ok(_) => panic!("expected type error for `{code}`"),
            Err(err) => assert_type_error(err),
        }
    }
}

#[tokio::test]
async fn integer_literal_let_binding_per_type() {
    let code = r#"
(
  let x = 4 in num_u8 x,
  let x = 4 in num_u16 x,
  let x = 4 in num_u32 x,
  let x = 4 in num_u64 x,
  let x = 4 in num_i8 x,
  let x = 4 in num_i16 x,
  let x = 4 in num_i32 x,
  let x = 4 in num_i64 x
)
"#;
    let (heap, pointer, ty) = eval(code).await.unwrap();
    assert_eq!(ty, expected_type());
    assert_tuple_of_strings(&heap, &pointer);
}

#[tokio::test]
async fn integer_literal_lambda_binding_per_type() {
    let code = r#"
(
  let f = \x -> num_u8 x in f 4,
  let f = \x -> num_u16 x in f 4,
  let f = \x -> num_u32 x in f 4,
  let f = \x -> num_u64 x in f 4,
  let f = \x -> num_i8 x in f 4,
  let f = \x -> num_i16 x in f 4,
  let f = \x -> num_i32 x in f 4,
  let f = \x -> num_i64 x in f 4
)
"#;
    let (heap, pointer, ty) = eval(code).await.unwrap();
    assert_eq!(ty, expected_type());
    assert_tuple_of_strings(&heap, &pointer);
}

#[tokio::test]
async fn negative_integer_literal_let_binding_per_signed_type() {
    let code = r#"
(
  let x: i8 = -3 in num_i8 x,
  let x: i16 = -3 in num_i16 x,
  let x: i32 = -3 in num_i32 x,
  let x: i64 = -3 in num_i64 x
)
"#;
    let (heap, pointer, ty) = eval(code).await.unwrap();
    assert_eq!(ty, expected_negative_signed_type());
    assert_tuple_of_negative_signed_strings(&heap, &pointer);
}

#[tokio::test]
async fn negative_integer_literal_let_binding_unsigned_calls_are_type_errors() {
    for code in [
        "let x: u8 = -3 in num_u8 x",
        "let x: u16 = -3 in num_u16 x",
        "let x: u32 = -3 in num_u32 x",
        "let x: u64 = -3 in num_u64 x",
    ] {
        match eval(code).await {
            Ok(_) => panic!("expected type error for `{code}`"),
            Err(err) => assert_type_error(err),
        }
    }
}

#[tokio::test]
async fn negative_integer_literal_lambda_binding_per_signed_type() {
    let code = r#"
(
  let f = \x -> num_i8 x in f (-3),
  let f = \x -> num_i16 x in f (-3),
  let f = \x -> num_i32 x in f (-3),
  let f = \x -> num_i64 x in f (-3)
)
"#;
    let (heap, pointer, ty) = eval(code).await.unwrap();
    assert_eq!(ty, expected_negative_signed_type());
    assert_tuple_of_negative_signed_strings(&heap, &pointer);
}

#[tokio::test]
async fn negative_integer_literal_lambda_binding_unsigned_calls_are_type_errors() {
    for code in [
        "let f = \\x -> num_u8 x in f (-3)",
        "let f = \\x -> num_u16 x in f (-3)",
        "let f = \\x -> num_u32 x in f (-3)",
        "let f = \\x -> num_u64 x in f (-3)",
    ] {
        match eval(code).await {
            Ok(_) => panic!("expected type error for `{code}`"),
            Err(err) => assert_type_error(err),
        }
    }
}

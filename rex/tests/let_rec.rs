use rex::{
    Engine, EngineError, GasCosts, GasMeter, Heap, Parser, Pointer, Token, Type, TypeSystem, Value,
};
use rex_engine::assert_pointer_eq;

async fn eval(code: &str) -> Result<(Heap, Pointer), EngineError> {
    let tokens = Token::tokenize(code).unwrap();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program(&mut GasMeter::default()).unwrap();
    let mut engine = Engine::with_prelude(()).unwrap();
    engine.inject_decls(&program.decls)?;
    let mut gas = GasMeter::default();
    let pointer = engine.eval(program.expr.as_ref(), &mut gas).await?;
    let heap = engine.into_heap();
    Ok((heap, pointer))
}

fn type_of(code: &str) -> Type {
    let tokens = Token::tokenize(code).unwrap();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program(&mut GasMeter::default()).unwrap();
    let mut ts = TypeSystem::with_prelude().unwrap();
    ts.inject_decls(&program.decls).unwrap();
    let (_preds, ty) = ts.infer(program.expr.as_ref()).unwrap();
    ty
}

#[tokio::test]
async fn let_rec_self_recursive_factorial() {
    let (heap, pointer) =
        eval("let rec fact = \\n -> if n == 0 then 1 else n * fact (n - 1) in fact 6")
            .await
            .unwrap();
    let expected = heap.alloc_i32(720).unwrap();
    assert_pointer_eq!(&heap, &pointer, &expected);
}

#[tokio::test]
async fn let_rec_self_recursive_fibonacci() {
    let (heap, pointer) =
        eval("let rec fib = \\n -> if n <= 1 then n else fib (n - 1) + fib (n - 2) in fib 8")
            .await
            .unwrap();
    let expected = heap.alloc_i32(21).unwrap();
    assert_pointer_eq!(&heap, &pointer, &expected);
}

#[tokio::test]
async fn let_rec_mutual_even_odd() {
    let (heap, pointer) = eval(
        r#"
let rec
  even = \n -> if n == 0 then true else odd (n - 1)
and
  odd = \n -> if n == 0 then false else even (n - 1)
in
  (even 10, odd 10, even 11, odd 11)
"#,
    )
    .await
    .unwrap();

    let t0 = heap.alloc_bool(true).unwrap();
    let t1 = heap.alloc_bool(false).unwrap();
    let t2 = heap.alloc_bool(false).unwrap();
    let t3 = heap.alloc_bool(true).unwrap();
    let expected = heap.alloc_tuple(vec![t0, t1, t2, t3]).unwrap();
    assert_pointer_eq!(&heap, &pointer, &expected);
}

#[tokio::test]
async fn let_rec_mutual_three_function_group() {
    let (heap, pointer) = eval(
        r#"
let rec
  step0 = \n -> if n == 0 then 0 else step1 (n - 1)
and
  step1 = \n -> if n == 0 then 1 else step2 (n - 1)
and
  step2 = \n -> if n == 0 then 2 else step0 (n - 1)
in
  (step0 3, step1 3, step2 3)
"#,
    )
    .await
    .unwrap();

    let a = heap.alloc_i32(0).unwrap();
    let b = heap.alloc_i32(1).unwrap();
    let c = heap.alloc_i32(2).unwrap();
    let expected = heap.alloc_tuple(vec![a, b, c]).unwrap();
    assert_pointer_eq!(&heap, &pointer, &expected);
}

#[tokio::test]
async fn let_rec_function_is_still_polymorphic() {
    let (heap, pointer) = eval("let rec id = \\x -> x in (id 1, id true)")
        .await
        .unwrap();
    let one = heap.alloc_i32(1).unwrap();
    let tru = heap.alloc_bool(true).unwrap();
    let expected = heap.alloc_tuple(vec![one, tru]).unwrap();
    assert_pointer_eq!(&heap, &pointer, &expected);

    let ty = type_of("let rec id = \\x -> x in (id 1, id true)");
    let expected_ty = Type::tuple(vec![Type::con("i32", 0), Type::con("bool", 0)]);
    assert_eq!(ty, expected_ty);
}

#[tokio::test]
async fn let_rec_allows_self_referential_data_cycles() {
    let (heap, pointer) = eval("let rec xs = Cons 1 xs in xs").await.unwrap();
    let value = heap.get(&pointer).unwrap();
    let Value::Adt(tag, args) = value.as_ref() else {
        panic!(
            "expected list constructor, got {}",
            heap.type_name(&pointer).unwrap()
        );
    };
    assert_eq!(tag.as_ref(), "Cons");
    assert_eq!(args.len(), 2);
    assert_pointer_eq!(&heap, &pointer, &args[1]);
}

#[tokio::test]
async fn let_rec_allows_mutual_data_cycles() {
    let (heap, pointer) = eval("let rec a = Cons 1 b and b = Cons 2 a in (a, b)")
        .await
        .unwrap();
    let tuple = heap.get(&pointer).unwrap();
    let Value::Tuple(items) = tuple.as_ref() else {
        panic!("expected tuple, got {}", heap.type_name(&pointer).unwrap());
    };
    assert_eq!(items.len(), 2);
    let a_ptr = items[0].clone();
    let b_ptr = items[1].clone();

    let a_val = heap.get(&a_ptr).unwrap();
    let Value::Adt(_, a_args) = a_val.as_ref() else {
        panic!(
            "expected list constructor, got {}",
            heap.type_name(&a_ptr).unwrap()
        );
    };
    assert_eq!(a_args.len(), 2);

    let b_val = heap.get(&b_ptr).unwrap();
    let Value::Adt(_, b_args) = b_val.as_ref() else {
        panic!(
            "expected list constructor, got {}",
            heap.type_name(&b_ptr).unwrap()
        );
    };
    assert_eq!(b_args.len(), 2);

    assert_pointer_eq!(&heap, &a_args[1], &b_ptr);
    assert_pointer_eq!(&heap, &b_args[1], &a_ptr);
}

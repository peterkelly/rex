use rex::{Engine, EngineError, GasMeter, Heap, Parser, Pointer, Token};
use rex_engine::assert_pointer_eq;

async fn eval(source: &str) -> Result<(Heap, Pointer), EngineError> {
    let tokens = Token::tokenize(source).unwrap();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program(&mut GasMeter::default()).unwrap();
    let mut engine = Engine::with_prelude(()).unwrap();
    engine.inject_decls(&program.decls)?;
    let mut gas = GasMeter::default();
    let pointer = engine.eval(program.expr.as_ref(), &mut gas).await?;
    let heap = engine.into_heap();
    Ok((heap, pointer))
}

async fn assert_i32_result(source: &str, expected: i32) {
    let (heap, pointer) = eval(source).await.unwrap();
    let expected = heap.alloc_i32(expected).unwrap();
    assert_pointer_eq!(&heap, &pointer, &expected);
}

async fn assert_even_odd_tuple(source: &str) {
    let (heap, pointer) = eval(source).await.unwrap();
    let t0 = heap.alloc_bool(true).unwrap();
    let t1 = heap.alloc_bool(false).unwrap();
    let t2 = heap.alloc_bool(false).unwrap();
    let t3 = heap.alloc_bool(true).unwrap();
    let expected = heap.alloc_tuple(vec![t0, t1, t2, t3]).unwrap();
    assert_pointer_eq!(&heap, &pointer, &expected);
}

#[tokio::test]
async fn factorial_let_rec() {
    let expr = r#"
        let rec fact = \n ->
          if n == 0 then 1 else n * fact (n - 1)
        in
          fact 6
    "#;
    assert_i32_result(expr, 720).await;
}

#[tokio::test]
async fn mutual_even_odd_let_rec() {
    let expr = r#"
        let rec
          even = \n -> if n == 0 then true else odd (n - 1),
          odd = \n -> if n == 0 then false else even (n - 1)
        in
          (even 10, odd 10, even 11, odd 11)
    "#;
    assert_even_odd_tuple(expr).await;
}

#[tokio::test]
async fn mutual_even_odd_top_level_fn_decls() {
    let expr = r#"
        fn even (n: i32) -> bool = if n == 0 then true else odd (n - 1)
        fn odd (n: i32) -> bool = if n == 0 then false else even (n - 1)
        (even 10, odd 10, even 11, odd 11)
    "#;
    assert_even_odd_tuple(expr).await;
}

#[tokio::test]
async fn mutual_list_cycle_let_rec() {
    let expr = r#"
        let rec
          a = Cons 1 b,
          b = Cons 2 a
        in
        match b
          when Cons h _t -> h
          when Empty -> 0
    "#;
    assert_i32_result(expr, 2).await;
}

#[tokio::test]
async fn self_referential_list_let_rec() {
    let expr = r#"
        let rec xs = Cons 1 xs in
        match xs
          when Cons head _tail -> head
          when Empty -> 0
    "#;
    assert_i32_result(expr, 1).await;
}

#[tokio::test]
async fn factorial_plain_let() {
    let expr = r#"
        type Rec a b = Rec ((Rec a b) -> a -> b)

        let unrec = \r ->
          match r
            when Rec f -> f
        in
        let fix = \f ->
          let g = \x -> f (\v -> unrec x x v) in
          g (Rec g)
        in
        let fact = fix (\self -> \n ->
          if n == 0 then 1 else n * self (n - 1)
        )
        in
          fact 6
    "#;
    assert_i32_result(expr, 720).await;
}

#[tokio::test]
async fn mutual_even_odd_plain_let() {
    let expr = r#"
        type Rec a b = Rec ((Rec a b) -> a -> b)

        let unrec = \r ->
          match r
            when Rec f -> f
        in
        let fix = \f ->
          let g = \x -> f (\v -> unrec x x v) in
          g (Rec g)
        in
        let toggle = \b -> if b then false else true in
        let parity = fix (\self -> \is_even -> \n ->
          if n == 0 then is_even else self (toggle is_even) (n - 1)
        )
        in
          (parity true 10, parity false 10, parity true 11, parity false 11)
    "#;
    assert_even_odd_tuple(expr).await;
}

#[tokio::test]
async fn mutual_list_cycle_plain_let() {
    let expr = r#"
        type Rec a b = Rec ((Rec a b) -> a -> b)

        let unrec = \r ->
          match r
            when Rec f -> f
        in
        let fix = \f ->
          let g = \x -> f (\v -> unrec x x v) in
          g (Rec g)
        in
        let toggle = \b -> if b then false else true in
        let alternating_head = fix (\self -> \from_b -> \n ->
          if n == 0 then
            if from_b then 2 else 1
          else
            self (toggle from_b) (n - 1)
        )
        in
          alternating_head true 0
    "#;
    assert_i32_result(expr, 2).await;
}

#[tokio::test]
async fn self_referential_list_plain_let() {
    let expr = r#"
        type Rec a b = Rec ((Rec a b) -> a -> b)

        let unrec = \r ->
          match r
            when Rec f -> f
        in
        let fix = \f ->
          let g = \x -> f (\v -> unrec x x v) in
          g (Rec g)
        in
        let repeated_head = fix (\self -> \n ->
          if n == 0 then 1 else self (n - 1)
        )
        in
          repeated_head 8
    "#;
    assert_i32_result(expr, 1).await;
}

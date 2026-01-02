use rex_engine::Engine;
use rex_lexer::Token;
use rex_parser::Parser;

#[test]
fn record_update_end_to_end() {
    let code = include_str!("../examples/record_update.rex");
    let tokens = Token::tokenize(code).unwrap();
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().unwrap();

    let mut engine = Engine::with_prelude();
    engine.inject_decls(&program.decls).unwrap();
    let value = engine.eval(program.expr.as_ref()).unwrap();

    let rex_engine::Value::Tuple(items) = value else {
        panic!("expected tuple, got {value}");
    };
    assert_eq!(items.len(), 2);

    let rex_engine::Value::I32(a) = items[0] else {
        panic!("expected i32, got {}", items[0]);
    };
    let rex_engine::Value::I32(b) = items[1] else {
        panic!("expected i32, got {}", items[1]);
    };
    assert_eq!(a, 6);
    assert_eq!(b, 2);
}


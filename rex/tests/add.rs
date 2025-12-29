use rex_lexer::Token;
use rex_parser::Parser;

const code: &'static str = r#"
1 + 2
"#;

#[test]
fn test_addition() {
    let mut parser = Parser::new(Token::tokenize(code).unwrap());
    let expr = parser.parse_program().unwrap();
    // Add assertions or checks here as needed
    todo!()
}

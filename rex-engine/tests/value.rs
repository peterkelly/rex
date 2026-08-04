use rex_ast::Symbol;
use rex_engine::{Value, ValueDisplayOptions};

#[test]
fn value_display_default_strips_internal_noise() {
    assert_eq!(Value::I32(2).display().unwrap(), "2");
    assert_eq!(
        Value::Adt(Symbol::intern("@snippetabc.A"), vec![])
            .display()
            .unwrap(),
        "A"
    );
}

#[test]
fn value_display_unsanitized_keeps_suffixes_and_names() {
    let options = ValueDisplayOptions::unsanitized();
    assert_eq!(Value::I32(2).display_with(options).unwrap(), "2i32");
    assert_eq!(
        Value::Adt(Symbol::intern("@snippetabc.A"), vec![])
            .display_with(options)
            .unwrap(),
        "@snippetabc.A"
    );
}

#[test]
fn bytes_display_as_a_u8_list() {
    assert_eq!(Value::Bytes(vec![1, 2, 3]).display().unwrap(), "[1, 2, 3]");
    assert_eq!(
        Value::Bytes(vec![1, 2, 3])
            .display_with(ValueDisplayOptions::unsanitized())
            .unwrap(),
        "[1u8, 2u8, 3u8]"
    );
}

use rex_ast::Symbol;
use rex_typesystem::{
    types::{AdtArgument, AdtDecl, AdtField, BuiltinTypeId, Type, adt_shape_eq, merge_adt_docs},
    typesystem::TypeVarSupply,
};

fn field(name: &str, typ: Type, docs: Option<&str>) -> AdtField {
    AdtField {
        name: Symbol::intern(name),
        typ,
        docs: docs.map(str::to_owned),
    }
}

#[test]
fn merge_adt_docs_matches_record_fields_by_name() {
    let mut supply = TypeVarSupply::new();
    let mut target = AdtDecl::new(&Symbol::intern("Event"), &[], &mut supply);
    target.add_variant(
        Symbol::intern("Created"),
        vec![AdtArgument::Record {
            fields: vec![
                field("id", Type::builtin(BuiltinTypeId::I32), None),
                field("label", Type::builtin(BuiltinTypeId::String), None),
            ],
            docs: None,
        }],
        None,
    );

    let mut source = AdtDecl::new(&Symbol::intern("Event"), &[], &mut supply);
    source.add_variant(
        Symbol::intern("Created"),
        vec![AdtArgument::Record {
            fields: vec![
                field(
                    "label",
                    Type::builtin(BuiltinTypeId::String),
                    Some("The display label."),
                ),
                field(
                    "id",
                    Type::builtin(BuiltinTypeId::I32),
                    Some("The event identifier."),
                ),
            ],
            docs: None,
        }],
        None,
    );

    assert!(adt_shape_eq(&target, &source));
    merge_adt_docs(&mut target, &source).expect("merge documentation");

    let AdtArgument::Record { fields, .. } = &target.variants[0].args[0] else {
        panic!("expected record argument");
    };
    assert_eq!(fields[0].name.as_str(), "id");
    assert_eq!(fields[0].docs.as_deref(), Some("The event identifier."));
    assert_eq!(fields[1].name.as_str(), "label");
    assert_eq!(fields[1].docs.as_deref(), Some("The display label."));
}

#[test]
fn merge_adt_docs_leaves_target_unchanged_on_error() {
    let mut supply = TypeVarSupply::new();
    let mut target = AdtDecl::new(&Symbol::intern("Event"), &[], &mut supply);
    target.add_variant(
        Symbol::intern("Created"),
        vec![AdtArgument::positional(Type::builtin(BuiltinTypeId::I32))],
        Some("The target variant documentation.".to_owned()),
    );

    let mut source = AdtDecl::new(&Symbol::intern("Event"), &[], &mut supply);
    source.docs = Some("Documentation added before the conflict.".to_owned());
    source.add_variant(
        Symbol::intern("Created"),
        vec![AdtArgument::positional(Type::builtin(BuiltinTypeId::I32))],
        Some("Conflicting variant documentation.".to_owned()),
    );

    let original = target.clone();
    let error = merge_adt_docs(&mut target, &source).expect_err("reject conflicting documentation");

    assert!(
        error
            .to_string()
            .contains("conflicting documentation for variant `Created` of ADT `Event`")
    );
    assert_eq!(target, original);
}

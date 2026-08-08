use rex::{
    ast::Symbol,
    engine::{Builder, EngineError, virtual_export_name},
};

/// Host APIs used to test documented registration.
#[rex::module(name = "host.documented")]
mod host_documented {
    use rex::engine::EngineError;

    /// A request accepted by the host API.
    #[derive(Clone, Debug, rex::Rex)]
    #[rex(export)]
    pub struct Request {
        /// The value to double.
        pub value: i32,
    }

    /// Double the request's value.
    #[rex::export(name = "double")]
    pub fn double_request(_state: &(), request: Request) -> Result<Request, EngineError> {
        Ok(Request {
            value: request.value * 2,
        })
    }
}

#[rex::module(name = "host.generic_signature")]
mod host_generic_signature {
    use rex::engine::EngineError;

    #[derive(rex::Rex)]
    pub struct Leaf {
        pub value: i32,
    }

    #[derive(rex::Rex)]
    pub struct Wrapper<T> {
        pub value: T,
    }

    #[rex::export]
    pub fn read_wrapped_leaf(_state: &(), wrapped: Wrapper<Leaf>) -> Result<i32, EngineError> {
        Ok(wrapped.value.value)
    }
}

#[rex::module(name = "host.split_attributes")]
mod host_split_attributes {
    #[derive(rex::Rex)]
    #[rex(export)]
    #[rex(name = "RenamedType")]
    pub struct RustTypeName {
        pub value: i32,
    }
}

#[test]
fn module_and_export_macros_preserve_rust_docs_and_parameter_names() {
    let module = host_documented::rex_module().unwrap();
    assert_eq!(
        module.docs(),
        Some("Host APIs used to test documented registration.")
    );
    assert_eq!(module.exports().len(), 1);
    assert_eq!(
        module.exports()[0].docs(),
        Some("Double the request's value.")
    );
    assert_eq!(
        module.exports()[0].params().collect::<Vec<_>>(),
        vec!["request"]
    );

    let declarations = module.declarations();
    assert_eq!(declarations.types.len(), 1);
    assert_eq!(
        declarations.types[0].docs.as_deref(),
        Some("A request accepted by the host API.")
    );

    let mut builder = Builder::with_prelude(()).unwrap();
    builder.inject_module(module).unwrap();
    let registered_name = Symbol::intern(&virtual_export_name("host.documented", "double"));
    let registered = builder
        .type_system()
        .env
        .lookup(&registered_name)
        .expect("registered documented function");
    assert_eq!(registered.len(), 1);
    assert_eq!(
        registered[0].docs.as_deref(),
        Some("Double the request's value.")
    );
    assert_eq!(registered[0].params[0].as_ref(), "request");
}

#[test]
fn exported_signature_auto_registers_derived_adt_family() -> Result<(), EngineError> {
    let export = host_documented::double_request_rex_export()?;
    let mut module = rex::engine::Module::new(
        "host.signature",
        Some("Signature registration APIs.".to_owned()),
    );
    assert_eq!(module.docs(), Some("Signature registration APIs."));
    module.add_export(export)?;
    assert!(
        module
            .declarations()
            .types
            .iter()
            .any(|declaration| declaration.name.as_ref() == "Request")
    );
    Ok(())
}

#[test]
fn exported_signature_collects_concrete_generic_argument_adts() -> Result<(), EngineError> {
    let module = host_generic_signature::rex_module()?;
    let type_names = module
        .declarations()
        .types
        .into_iter()
        .map(|declaration| declaration.name)
        .collect::<Vec<_>>();
    assert!(type_names.iter().any(|name| name.as_ref() == "Leaf"));
    assert!(type_names.iter().any(|name| name.as_ref() == "Wrapper"));

    let mut builder = Builder::with_prelude(())?;
    builder.inject_module(module)?;
    Ok(())
}

#[test]
fn rex_name_is_found_after_a_separate_export_marker() -> Result<(), EngineError> {
    let module = host_split_attributes::rex_module()?;
    let declarations = module.declarations();
    assert_eq!(declarations.types.len(), 1);
    assert_eq!(declarations.types[0].name.as_ref(), "RenamedType");
    Ok(())
}

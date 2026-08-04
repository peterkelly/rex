#![allow(dead_code)]

use rex::{
    ast::CompilationUnit,
    engine::{Builder, CompileOptions, EngineError, Module, Value, ValueDisplayOptions},
    parser::parse as parse_rex,
    typesystem::{BuiltinTypeId, Type, TypeError, TypeKind},
};

pub fn strip_type_span(mut err: TypeError) -> TypeError {
    while let TypeError::Spanned { error, .. } = err {
        err = *error;
    }
    err
}

pub async fn assert_invalid_let_rec_value_dependency(
    source: &str,
    binding: &str,
    dependency: &str,
) {
    let err = match eval_source(Builder::with_prelude(()).unwrap(), source).await {
        Ok((_heap, handle, ty)) => panic!(
            "expected invalid let rec value dependency, got {} with type {ty}",
            handle.display().unwrap()
        ),
        Err(err) => err,
    };
    let EngineError::Type(err) = err else {
        panic!("expected type error, got {err:?}");
    };
    let err = strip_type_span(err);
    assert!(
        matches!(
            err,
            TypeError::InvalidLetRecValueDependency {
                binding: ref actual_binding,
                dependency: ref actual_dependency,
            } if actual_binding.as_ref() == binding && actual_dependency.as_ref() == dependency
        ),
        "unexpected type error: {err:?}"
    );
}

pub fn inject_globals<State: Clone + Send + Sync + 'static>(
    builder: &mut Builder<State>,
    build: impl FnOnce(&mut Module<State>) -> Result<(), EngineError>,
) -> Result<(), EngineError> {
    let mut module = Module::global();
    build(&mut module)?;
    builder.inject_module(module)
}

pub async fn run_program<State>(
    builder: Builder<State>,
    program: &CompilationUnit,
) -> Result<(Value, Type), EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let compiler = builder.build_compiler();
    let (compiled, evaluator) = compiler
        .compile_program(program, CompileOptions::for_module("test.main")?)
        .await?;
    let ty = compiled.result_type().clone();
    let value = evaluator.run(compiled, Default::default()).await?;
    Ok((value, ty))
}

pub async fn eval_source<State>(
    builder: Builder<State>,
    source: &str,
) -> Result<((), Value, Type), EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let program = parse_rex(source).unwrap();
    let (handle, ty) = run_program(builder, &program).await?;
    Ok(((), handle, ty))
}

pub fn tuple_items(value: &Value) -> Vec<Value> {
    let Value::Tuple(items) = value else {
        panic!("expected tuple, got {}", value.value_type_name());
    };
    items.clone()
}

pub fn list_elements(list: &Value) -> Vec<Value> {
    match list {
        Value::List(items) => items.clone(),
        Value::Bytes(bytes) => bytes.iter().copied().map(Value::U8).collect(),
        other => panic!("expected list, got {}", other.value_type_name()),
    }
}

pub fn list_from_values(values: Vec<Value>) -> Value {
    Value::List(values)
}

pub fn assert_values_eq(lhs: &Value, rhs: &Value) {
    assert_eq!(
        lhs,
        rhs,
        "left: {}, right: {}",
        lhs.display().unwrap(),
        rhs.display().unwrap()
    );
}

pub trait TestValue {
    fn rendered(&self) -> String;
}

impl TestValue for Value {
    fn rendered(&self) -> String {
        self.display_with(ValueDisplayOptions {
            include_numeric_suffixes: true,
            ..ValueDisplayOptions::default()
        })
        .unwrap()
    }
}

pub fn assert_handles_eq(lhs: &impl TestValue, rhs: &impl TestValue) {
    assert_eq!(lhs.rendered(), rhs.rendered());
}

pub fn is_i32_or_var(ty: &Type) -> bool {
    matches!(ty.as_ref(), TypeKind::Con(tc) if tc.name_str() == "i32")
        || matches!(ty.as_ref(), TypeKind::Var(_))
}

pub fn assert_i32_or_var(ty: &Type) {
    assert!(is_i32_or_var(ty), "expected i32 or type variable, got {ty}");
}

pub fn type_compatible(actual: &Type, expected: &Type) -> bool {
    match (actual.as_ref(), expected.as_ref()) {
        (TypeKind::Var(_), TypeKind::Con(tc)) if tc.name_str() == "i32" => true,
        (TypeKind::Con(a), TypeKind::Con(b)) => a == b,
        (TypeKind::App(af, aa), TypeKind::App(ef, ea))
        | (TypeKind::Fun(af, aa), TypeKind::Fun(ef, ea)) => {
            type_compatible(af, ef) && type_compatible(aa, ea)
        }
        (TypeKind::Tuple(as_), TypeKind::Tuple(es)) if as_.len() == es.len() => as_
            .iter()
            .zip(es.iter())
            .all(|(a, e)| type_compatible(a, e)),
        (TypeKind::Record(as_), TypeKind::Record(es)) if as_.len() == es.len() => as_
            .iter()
            .zip(es.iter())
            .all(|((an, at), (en, et))| an == en && type_compatible(at, et)),
        _ => false,
    }
}

fn strip_generated_type_prefixes(rendered: &str) -> String {
    let mut out = String::with_capacity(rendered.len());
    let mut rest = rendered;

    while let Some(start) = rest.find('@') {
        out.push_str(&rest[..start]);
        let after_marker = &rest[start + 1..];
        if let Some(dot) = after_marker.find('.') {
            let marker = &after_marker[..dot];
            if marker.starts_with("snippet")
                || marker
                    .strip_prefix('m')
                    .is_some_and(|hash| hash.chars().all(|ch| ch.is_ascii_hexdigit()))
            {
                rest = &after_marker[dot + 1..];
                continue;
            }
            out.push('@');
            rest = after_marker;
        } else {
            out.push('@');
            rest = after_marker;
        }
    }

    out.push_str(rest);
    out
}

pub async fn eval_to_display_string(code: &str, expected_ty: Type) -> Result<String, String> {
    let (_heap, handle, ty) =
        eval_source(Builder::with_prelude(()).map_err(|e| format!("{e}"))?, code)
            .await
            .map_err(|e| strip_generated_type_prefixes(&format!("{e}")))?;
    let actual_ty_display = strip_generated_type_prefixes(&ty.to_string());
    let expected_ty_display = strip_generated_type_prefixes(&expected_ty.to_string());
    // FIXME: Direct snippet compilation gives local test ADTs internal module
    // prefixes. Until public type rendering has a real namespace-to-surface-name
    // layer, strip them here so tests compare the user-facing type text they
    // actually care about.
    assert!(
        type_compatible(&ty, &expected_ty) || actual_ty_display == expected_ty_display,
        "eval returned unexpected type for: {code}\nactual: {actual_ty_display}\nexpected: {expected_ty_display}"
    );
    let opts = ValueDisplayOptions {
        include_numeric_suffixes: true,
        ..ValueDisplayOptions::default()
    };
    handle.display_with(opts).map_err(|e| format!("{e}"))
}

pub async fn assert_eval_display(code: &str, expected: &str, expected_ty: Type) {
    let actual = eval_to_display_string(code, expected_ty)
        .await
        .unwrap_or_else(|e| panic!("expected ok, got error: {e}"));
    assert_eq!(actual, expected);
}

pub async fn assert_eval_error_contains(code: &str, needle: &str) {
    let err = eval_to_display_string(code, Type::builtin(BuiltinTypeId::I32))
        .await
        .unwrap_err();
    assert!(
        err.contains(needle),
        "expected error containing {needle:?}, got: {err}"
    );
}

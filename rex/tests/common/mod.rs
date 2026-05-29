#![allow(dead_code)]

use rex::{
    ast::{CompilationUnit, Symbol},
    engine::{Engine, EngineError, Handle, Heap, Module, Value, ValueDisplayOptions},
    parser::parse as parse_rex,
    typesystem::{BuiltinTypeId, Type, TypeError, TypeKind},
};

pub fn strip_type_span(mut err: TypeError) -> TypeError {
    while let TypeError::Spanned { error, .. } = err {
        err = *error;
    }
    err
}

pub fn inject_globals<State: Clone + Send + Sync + 'static>(
    engine: &mut Engine<State>,
    build: impl FnOnce(&mut Module<State>) -> Result<(), EngineError>,
) -> Result<(), EngineError> {
    let mut module = Module::global();
    build(&mut module)?;
    engine.inject_module(module)
}

pub async fn run_program_body<State>(
    engine: Engine<State>,
    program: &CompilationUnit,
) -> Result<(Handle, Type), EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let mut compiler = engine.into_compiler();
    let body_program = CompilationUnit {
        decls: Vec::new(),
        body: program.body.clone(),
    };
    let compiled = compiler
        .compile_program(&body_program, Default::default())
        .await?;
    let ty = compiled.result_type().clone();
    let value = compiler
        .into_evaluator()
        .run(compiled, Default::default())
        .await?;
    Ok((value, ty))
}

pub async fn run_program_body_with_heap<State>(
    engine: Engine<State>,
    program: &CompilationUnit,
) -> Result<(Heap, Handle, Type), EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let heap = engine.heap.clone();
    let (handle, ty) = run_program_body(engine, program).await?;
    Ok((heap, handle, ty))
}

pub async fn eval_source_with_engine<State>(
    mut engine: Engine<State>,
    source: &str,
) -> Result<(Heap, Handle, Type), EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let program = parse_rex(source).unwrap();
    let mut module = Module::global();
    module.add_decls(program.decls.clone());
    engine.inject_module(module)?;
    run_program_body_with_heap(engine, &program).await
}

pub async fn eval_source(source: &str) -> Result<(Heap, Handle, Type), EngineError> {
    eval_source_with_engine(Engine::with_prelude(())?, source).await
}

pub async fn run_snippet<State>(
    engine: Engine<State>,
    source: &str,
) -> Result<(Handle, Type), EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let mut compiler = engine.into_compiler();
    let parsed = parse_rex(source).unwrap();
    let program = compiler
        .compile_program(&parsed, Default::default())
        .await?;
    let ty = program.result_type().clone();
    let value = compiler
        .into_evaluator()
        .run(program, Default::default())
        .await?;
    Ok((value, ty))
}

pub async fn infer_type<State>(engine: Engine<State>, source: &str) -> Result<Type, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let mut compiler = engine.into_compiler();
    let (_, ty) = compiler.infer_snippet(source, None).await?;
    Ok(ty)
}

pub fn tuple_items(value: &Handle) -> Vec<Handle> {
    let Value::Tuple(items) = value.value().unwrap() else {
        panic!("expected tuple, got {}", value.type_name().unwrap());
    };
    items
}

pub fn list_elements(list: &Handle) -> Vec<Handle> {
    let mut out = Vec::new();
    let mut cur = list.clone();
    loop {
        match cur.value().unwrap() {
            Value::Adt(tag, _args) if tag.as_ref() == "Empty" => return out,
            Value::Adt(tag, args) if tag.as_ref() == "Cons" => {
                assert_eq!(args.len(), 2, "Cons must have exactly two fields");
                out.push(args[0].clone());
                cur = args[1].clone();
            }
            other => panic!("expected list, got {}", other.value_type_name()),
        }
    }
}

pub fn list_from_handles(heap: &Heap, values: Vec<Handle>) -> Result<Handle, EngineError> {
    let mut list = heap.alloc_adt(Symbol::intern("Empty"), vec![])?;
    for value in values.into_iter().rev() {
        list = heap.alloc_adt(Symbol::intern("Cons"), vec![value, list])?;
    }
    Ok(list)
}

pub fn assert_handles_eq(lhs: &Handle, rhs: &Handle) {
    assert!(
        lhs.value_eq(rhs).unwrap(),
        "left: {}, right: {}",
        lhs.display().unwrap(),
        rhs.display().unwrap()
    );
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

pub async fn eval_to_display_string(code: &str, expected_ty: Type) -> Result<String, String> {
    let (_heap, handle, ty) = eval_source(code).await.map_err(|e| format!("{e}"))?;
    assert!(
        type_compatible(&ty, &expected_ty),
        "eval returned unexpected type for: {code}\nactual: {ty}\nexpected: {expected_ty}"
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

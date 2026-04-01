use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::FutureExt;
use rexlang_engine::{
    Engine, EngineOptions, EvaluatorRef, Library, Pointer, PreludeMode, Value, pointer_display,
};
use rexlang_typesystem::{BuiltinTypeId, Scheme, Type, TypeKind};
use rexlang_util::GasMeter;
use uuid::Uuid;

fn temp_dir(name: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!("rexlang-engine-test-{name}-{}", Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

fn engine_with_prelude() -> Engine {
    Engine::with_prelude(()).unwrap()
}

fn unlimited_gas() -> GasMeter {
    GasMeter::default()
}

fn i32_type() -> Type {
    Type::builtin(BuiltinTypeId::I32)
}

fn i32_binop_scheme() -> Scheme {
    Scheme::new(
        vec![],
        vec![],
        Type::fun(i32_type(), Type::fun(i32_type(), i32_type())),
    )
}

fn i32_value_scheme() -> Scheme {
    Scheme::new(vec![], vec![], i32_type())
}

#[tokio::test]
async fn prelude_module_can_be_imported_explicitly() {
    let mut engine = engine_with_prelude();
    let (value_ptr, ty) = eval_snippet(
        &mut engine,
        r#"
        import Prelude (map)
        map ((+) 1) [1, 2]
        "#,
    )
    .await
    .unwrap();

    assert_eq!(
        ty,
        Type::app(
            Type::builtin(BuiltinTypeId::List),
            Type::builtin(BuiltinTypeId::I32)
        )
    );
    let rendered = pointer_display(&engine.heap, &value_ptr).unwrap();
    assert_eq!(rendered, "[2, 3]");
}

#[tokio::test]
async fn engine_options_can_disable_prelude() {
    let mut engine = Engine::with_options(
        (),
        EngineOptions {
            prelude: PreludeMode::Disabled,
            default_imports: vec![],
        },
    )
    .unwrap();
    let err = eval_snippet(&mut engine, "map ((+) 1) [1, 2]")
        .await
        .expect_err("prelude should be unavailable when disabled");
    let msg = err.to_string();
    assert!(msg.contains("map"), "unexpected error: {msg}");
}

async fn eval_library_file<State: Clone + Send + Sync + 'static>(
    engine: &mut Engine<State>,
    path: &Path,
) -> Result<(Pointer, Type), rexlang_engine::EngineError> {
    let source = fs::read_to_string(path).unwrap();
    eval_snippet_at(engine, &source, path).await
}

async fn eval_snippet<State: Clone + Send + Sync + 'static>(
    engine: &mut Engine<State>,
    source: &str,
) -> Result<(Pointer, Type), rexlang_engine::EngineError> {
    let mut gas = unlimited_gas();
    rexlang_engine::Evaluator::new_with_compiler(
        rexlang_engine::RuntimeEnv::new(engine.clone()),
        rexlang_engine::Compiler::new(engine.clone()),
    )
    .eval_snippet(source, &mut gas)
    .await
    .map_err(|err| err.into_engine_error())
}

async fn eval_snippet_at<State: Clone + Send + Sync + 'static>(
    engine: &mut Engine<State>,
    source: &str,
    importer_path: impl AsRef<Path>,
) -> Result<(Pointer, Type), rexlang_engine::EngineError> {
    let mut gas = unlimited_gas();
    rexlang_engine::Evaluator::new_with_compiler(
        rexlang_engine::RuntimeEnv::new(engine.clone()),
        rexlang_engine::Compiler::new(engine.clone()),
    )
    .eval_snippet_at(source, importer_path, &mut gas)
    .await
    .map_err(|err| err.into_engine_error())
}

macro_rules! pvals {
    ($engine:expr, $vals:expr) => {
        $vals
            .iter()
            .map(|pointer| {
                (
                    pointer.clone(),
                    $engine
                        .heap
                        .get(pointer)
                        .map(|value| value.as_ref().clone())
                        .unwrap(),
                )
            })
            .collect::<Vec<(Pointer, Value)>>()
    };
}

#[tokio::test]
async fn library_import_local_pub() {
    let dir = temp_dir("library_import_local_pub");
    let main = dir.join("main.rex");
    let library = dir.join("foo").join("bar.rex");

    write_file(
        &library,
        r#"
        pub fn add x: i32 -> y: i32 -> i32 = x + y
        fn hidden x: i32 -> i32 = x + 1
        ()
"#,
    );
    write_file(
        &main,
        r#"
        import foo.bar as Bar
        Bar.add 1 2
"#,
    );

    let mut engine = engine_with_prelude();
    engine.add_default_resolvers();
    let (value_ptr, ty) = eval_library_file(&mut engine, &main).await.unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    let value = engine
        .heap
        .get(&value_ptr)
        .map(|value| value.as_ref().clone())
        .unwrap();
    match value {
        Value::I32(v) => assert_eq!(v, 3),
        _ => panic!(
            "expected i32, got {}",
            engine.heap.type_name(&value_ptr).unwrap()
        ),
    }
}

#[tokio::test]
async fn eval_library_file_reloads_when_local_file_changes() {
    let dir = temp_dir("eval_library_file_reloads_when_local_file_changes");
    let library = dir.join("foo.rex");
    let importer = dir.join("main.rex");
    write_file(&importer, "()");

    let mut engine = engine_with_prelude();
    engine.add_default_resolvers();

    write_file(&library, "pub fn value x: i32 -> i32 = x + 1");
    let mut gas = unlimited_gas();
    let _ = rexlang_engine::Evaluator::new_with_compiler(
        rexlang_engine::RuntimeEnv::new(engine.clone()),
        rexlang_engine::Compiler::new(engine.clone()),
    )
    .eval_library_file(&library, &mut gas)
    .await
    .unwrap();
    let (value_ptr, ty) = eval_snippet_at(&mut engine, "import foo (value)\nvalue 0", &importer)
        .await
        .unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    match engine.heap.get(&value_ptr).unwrap().as_ref() {
        Value::I32(v) => assert_eq!(*v, 1),
        other => panic!("expected i32, got {}", other.value_type_name()),
    }

    // Edit the same local library path and ensure the engine invalidates path-keyed
    // library cache entries before reloading.
    write_file(&library, "pub fn value x: i32 -> i32 = x + 2");
    let mut gas = unlimited_gas();
    let _ = rexlang_engine::Evaluator::new_with_compiler(
        rexlang_engine::RuntimeEnv::new(engine.clone()),
        rexlang_engine::Compiler::new(engine.clone()),
    )
    .eval_library_file(&library, &mut gas)
    .await
    .unwrap();
    let (value_ptr, ty) = eval_snippet_at(&mut engine, "import foo (value)\nvalue 0", &importer)
        .await
        .unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    match engine.heap.get(&value_ptr).unwrap().as_ref() {
        Value::I32(v) => assert_eq!(*v, 2),
        other => panic!("expected i32, got {}", other.value_type_name()),
    }
}

#[tokio::test]
async fn snippet_import_reloads_when_local_module_changes() {
    let dir = temp_dir("snippet_import_reloads_when_local_module_changes");
    let library = dir.join("foo.rex");
    let importer = dir.join("main.rex");
    write_file(&importer, "()");

    write_file(&library, "pub fn value x: i32 -> i32 = x + 1");
    let mut engine = engine_with_prelude();
    engine.add_default_resolvers();

    let (value_ptr, ty) = eval_snippet_at(&mut engine, "import foo (value)\nvalue 0", &importer)
        .await
        .unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    match engine.heap.get(&value_ptr).unwrap().as_ref() {
        Value::I32(v) => assert_eq!(*v, 1),
        other => panic!("expected i32, got {}", other.value_type_name()),
    }

    // Same library path, changed contents: import resolution must observe updated
    // source and invalidate stale per-library caches.
    write_file(&library, "pub fn value x: i32 -> i32 = x + 2");
    let (value_ptr, ty) = eval_snippet_at(&mut engine, "import foo (value)\nvalue 0", &importer)
        .await
        .unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    match engine.heap.get(&value_ptr).unwrap().as_ref() {
        Value::I32(v) => assert_eq!(*v, 2),
        other => panic!("expected i32, got {}", other.value_type_name()),
    }
}

#[tokio::test]
async fn imported_type_names_in_fn_signatures_are_rewritten() {
    let dir = temp_dir("imported_type_names_in_fn_signatures_are_rewritten");
    let a = dir.join("a.rex");
    let b = dir.join("b.rex");
    let importer = dir.join("main.rex");
    write_file(&importer, "()");

    write_file(
        &b,
        r#"
        pub type Boxed = Boxed i32
        "#,
    );
    write_file(
        &a,
        r#"
        import b as B
        pub fn id x: B.Boxed -> B.Boxed = x
        "#,
    );

    let mut engine = engine_with_prelude();
    engine.add_default_resolvers();
    let (value_ptr, _ty) = eval_snippet_at(
        &mut engine,
        r#"
        import a (id)
        import b (Boxed)
        id (Boxed 1)
        "#,
        &importer,
    )
    .await
    .unwrap();

    let value = engine.heap.get(&value_ptr).unwrap().as_ref().clone();
    match value {
        Value::Adt(_, fields) => {
            assert_eq!(fields.len(), 1);
            match engine.heap.get(&fields[0]).unwrap().as_ref() {
                Value::I32(v) => assert_eq!(*v, 1),
                other => panic!("expected i32 field, got {}", other.value_type_name()),
            }
        }
        other => panic!("expected adt value, got {}", other.value_type_name()),
    }
}

#[tokio::test]
async fn imported_class_names_in_instance_headers_are_rewritten() {
    let dir = temp_dir("imported_class_names_in_instance_headers_are_rewritten");
    let dep = dir.join("dep.rex");
    let importer = dir.join("main.rex");
    write_file(&importer, "()");

    write_file(
        &dep,
        r#"
        pub class Pick a where
            pick : a
        ()
        "#,
    );

    let mut engine = engine_with_prelude();
    engine.add_default_resolvers();
    let (value_ptr, ty) = eval_snippet_at(
        &mut engine,
        r#"
        import dep as D

        instance D.Pick i32 where
            pick = 7

        pick is i32
        "#,
        &importer,
    )
    .await
    .unwrap();

    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    match engine.heap.get(&value_ptr).unwrap().as_ref() {
        Value::I32(v) => assert_eq!(*v, 7),
        other => panic!("expected i32, got {}", other.value_type_name()),
    }
}

#[tokio::test]
async fn imported_type_alias_in_lambda_annotation_is_not_shadowed_by_param_name() {
    let dir = temp_dir("imported_type_alias_in_lambda_annotation_is_not_shadowed_by_param_name");
    let dep = dir.join("dep.rex");
    let importer = dir.join("main.rex");
    write_file(&importer, "()");

    write_file(
        &dep,
        r#"
        pub type Boxed = Boxed i32
        ()
        "#,
    );

    let mut engine = engine_with_prelude();
    engine.add_default_resolvers();
    let (value_ptr, ty) = eval_snippet_at(
        &mut engine,
        r#"
        import dep as D

        let f = \ (D : D.Boxed) -> 0 in
        0
        "#,
        &importer,
    )
    .await
    .unwrap();

    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    match engine.heap.get(&value_ptr).unwrap().as_ref() {
        Value::I32(v) => assert_eq!(*v, 0),
        other => panic!("expected i32 value, got {}", other.value_type_name()),
    }
}

#[tokio::test]
async fn module_cycle_with_pub_function_signatures_resolves() {
    let dir = temp_dir("module_cycle_with_pub_function_signatures_resolves");
    let main = dir.join("main.rex");
    let a = dir.join("a.rex");
    let b = dir.join("b.rex");

    write_file(
        &a,
        r#"
        import b as B
        pub fn fa x: i32 -> i32 = if x == 0 then 0 else B.fb (x - 1)
        ()
"#,
    );
    write_file(
        &b,
        r#"
        import a as A
        pub fn fb x: i32 -> i32 = if x == 0 then 0 else A.fa (x - 1)
        ()
"#,
    );
    write_file(
        &main,
        r#"
        import a as A
        A.fa 6
"#,
    );

    let mut engine = engine_with_prelude();
    engine.add_default_resolvers();
    let (value_ptr, ty) = eval_library_file(&mut engine, &main).await.unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    let value = engine.heap.get(&value_ptr).unwrap();
    match value.as_ref() {
        Value::I32(v) => assert_eq!(*v, 0),
        _ => panic!("expected i32"),
    }
}

#[tokio::test]
async fn module_injected_from_rust_sync_and_async_exports() {
    let mut engine = engine_with_prelude();
    engine.add_default_resolvers();

    let mut library = Library::new("host.math");
    library
        .export("inc", |_state: &(), x: i32| Ok(x + 1))
        .unwrap();
    library
        .export_async(
            "double_async",
            |_state: &(), x: i32| async move { Ok(x * 2) },
        )
        .unwrap();
    engine.inject_library(library).unwrap();

    let (value_ptr, ty) = eval_snippet(
        &mut engine,
        r#"
        import host.math (inc, double_async as d)
        inc (d 20)
"#,
    )
    .await
    .unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    let value = engine
        .heap
        .get(&value_ptr)
        .map(|value| value.as_ref().clone())
        .unwrap();
    match value {
        Value::I32(v) => assert_eq!(v, 41),
        _ => panic!(
            "expected i32, got {}",
            engine.heap.type_name(&value_ptr).unwrap()
        ),
    }
}

#[tokio::test]
async fn module_injected_from_rust_native_pointer_exports_sync() {
    let mut engine = Engine::with_prelude(true).unwrap();
    engine.add_default_resolvers();

    let mut library = Library::new("host.ptrsync");
    library
        .export_native(
            "pick",
            i32_binop_scheme(),
            2,
            |engine: EvaluatorRef<'_, bool>, _: &Type, args: &[Pointer]| {
                let idx = if *engine.state.as_ref() { 1 } else { 0 };
                args.get(idx)
                    .cloned()
                    .ok_or_else(|| rexlang_engine::EngineError::Internal("missing argument".into()))
            },
        )
        .unwrap();
    library
        .export_native(
            "heap_i32",
            i32_value_scheme(),
            0,
            |engine: EvaluatorRef<'_, bool>, _: &Type, _args| engine.heap.alloc_i32(123),
        )
        .unwrap();

    let expected_type = i32_binop_scheme().typ.clone();
    let typed_called = Arc::new(AtomicBool::new(false));
    library
        .export_native("pick_typed", i32_binop_scheme(), 2, {
            let typed_called = Arc::clone(&typed_called);
            move |engine: EvaluatorRef<'_, bool>, typ: &Type, args: &[Pointer]| {
                if typ == &expected_type {
                    typed_called.store(true, Ordering::Relaxed);
                }
                let idx = if *engine.state.as_ref() { 1 } else { 0 };
                args.get(idx)
                    .cloned()
                    .ok_or_else(|| rexlang_engine::EngineError::Internal("missing argument".into()))
            }
        })
        .unwrap();

    engine.inject_library(library).unwrap();

    let (value_ptr, ty) = eval_snippet(
        &mut engine,
        r#"
        import host.ptrsync (pick, pick_typed, heap_i32)
        (pick 10 42, pick_typed 5 99, heap_i32)
"#,
    )
    .await
    .unwrap();
    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::builtin(BuiltinTypeId::I32),
            Type::builtin(BuiltinTypeId::I32),
            Type::builtin(BuiltinTypeId::I32),
        ])
    );

    let value = engine.heap.get(&value_ptr).unwrap().as_ref().clone();
    let Value::Tuple(xs) = value else {
        panic!(
            "expected tuple, got {}",
            engine.heap.type_name(&value_ptr).unwrap()
        );
    };
    let xs = pvals!(engine, xs);
    let got: Vec<i32> = xs
        .into_iter()
        .map(|(pointer, v)| match v {
            Value::I32(n) => n,
            _ => panic!(
                "expected i32, got {}",
                engine.heap.type_name(&pointer).unwrap()
            ),
        })
        .collect();
    assert_eq!(got, vec![42, 99, 123]);
    assert!(typed_called.load(Ordering::Relaxed));
}

#[tokio::test]
async fn module_injected_from_rust_allows_overloaded_export_names() {
    let mut engine = engine_with_prelude();
    engine.add_default_resolvers();

    let mut library = Library::new("host.over");
    library.export("id", |_state: &(), x: i32| Ok(x)).unwrap();
    library
        .export("id", |_state: &(), x: String| Ok(x))
        .unwrap();
    engine.inject_library(library).unwrap();

    let (value_ptr, ty) = eval_snippet(
        &mut engine,
        r#"
        import host.over (id)
        (id 7, id "ok")
"#,
    )
    .await
    .unwrap();
    let TypeKind::Tuple(items) = ty.as_ref() else {
        panic!("expected tuple type, got {ty}");
    };
    assert_eq!(items.len(), 2);
    assert!(
        matches!(items[0].as_ref(), TypeKind::Con(tc) if tc.name.as_ref() == "i32")
            || matches!(items[0].as_ref(), TypeKind::Var(_))
    );
    assert_eq!(items[1], Type::builtin(BuiltinTypeId::String));

    let value = engine.heap.get(&value_ptr).unwrap().as_ref().clone();
    let Value::Tuple(xs) = value else {
        panic!(
            "expected tuple, got {}",
            engine.heap.type_name(&value_ptr).unwrap()
        );
    };
    let xs = pvals!(engine, xs);
    assert_eq!(xs.len(), 2);
    match &xs[0].1 {
        Value::I32(n) => assert_eq!(*n, 7),
        _ => panic!(
            "expected i32, got {}",
            engine.heap.type_name(&xs[0].0).unwrap()
        ),
    }
    match &xs[1].1 {
        Value::String(s) => assert_eq!(s, "ok"),
        _ => panic!(
            "expected string, got {}",
            engine.heap.type_name(&xs[1].0).unwrap()
        ),
    }
}

#[tokio::test]
async fn module_injected_from_rust_native_pointer_exports_async() {
    let mut engine = Engine::with_prelude(true).unwrap();
    engine.add_default_resolvers();

    let mut library = Library::new("host.ptrasync");
    library
        .export_native_async(
            "pick_async",
            i32_binop_scheme(),
            2,
            |engine: EvaluatorRef<'_, bool>, _: Type, args: Vec<Pointer>| {
                let idx = if *engine.state.as_ref() { 1 } else { 0 };
                async move {
                    args.get(idx).cloned().ok_or_else(|| {
                        rexlang_engine::EngineError::Internal("missing argument".into())
                    })
                }
                .boxed()
            },
        )
        .unwrap();
    library
        .export_native_async(
            "heap_i32_async",
            i32_value_scheme(),
            0,
            |engine: EvaluatorRef<'_, bool>, _: Type, _args: Vec<Pointer>| {
                async move { engine.heap.alloc_i32(77) }.boxed()
            },
        )
        .unwrap();

    let expected_type = i32_binop_scheme().typ.clone();
    let typed_called = Arc::new(AtomicBool::new(false));
    library
        .export_native_async("pick_typed_async", i32_binop_scheme(), 2, {
            let typed_called = Arc::clone(&typed_called);
            move |engine: EvaluatorRef<'_, bool>, typ: Type, args: Vec<Pointer>| {
                let type_match = typ == expected_type;
                let idx = if *engine.state.as_ref() { 1 } else { 0 };
                let typed_called = Arc::clone(&typed_called);
                async move {
                    if type_match {
                        typed_called.store(true, Ordering::Relaxed);
                    }
                    args.get(idx).cloned().ok_or_else(|| {
                        rexlang_engine::EngineError::Internal("missing argument".into())
                    })
                }
                .boxed()
            }
        })
        .unwrap();

    engine.inject_library(library).unwrap();

    let (value_ptr, ty) = eval_snippet(
        &mut engine,
        r#"
        import host.ptrasync (pick_async, pick_typed_async as pta, heap_i32_async)
        (pick_async 7 21, pta 1 2, heap_i32_async)
"#,
    )
    .await
    .unwrap();
    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::builtin(BuiltinTypeId::I32),
            Type::builtin(BuiltinTypeId::I32),
            Type::builtin(BuiltinTypeId::I32),
        ])
    );

    let value = engine.heap.get(&value_ptr).unwrap().as_ref().clone();
    let Value::Tuple(xs) = value else {
        panic!(
            "expected tuple, got {}",
            engine.heap.type_name(&value_ptr).unwrap()
        );
    };
    let xs = pvals!(engine, xs);
    let got: Vec<i32> = xs
        .into_iter()
        .map(|(pointer, v)| match v {
            Value::I32(n) => n,
            _ => panic!(
                "expected i32, got {}",
                engine.heap.type_name(&pointer).unwrap()
            ),
        })
        .collect();
    assert_eq!(got, vec![21, 2, 77]);
    assert!(typed_called.load(Ordering::Relaxed));
}

#[test]
fn module_native_pointer_export_rejects_invalid_arity_scheme_pair() {
    let mut library = Library::new("host.invalid");
    let unary_scheme = Scheme::new(vec![], vec![], Type::fun(i32_type(), i32_type()));

    let err = library
        .export_native(
            "bad",
            unary_scheme,
            2,
            |_engine: EvaluatorRef<'_, ()>, _: &Type, _args: &[Pointer]| {
                Err(rexlang_engine::EngineError::Internal("unused".into()))
            },
        )
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("does not accept 2 argument(s)"),
        "unexpected error: {msg}"
    );
}

#[test]
fn module_native_async_pointer_export_rejects_invalid_arity_scheme_pair() {
    let mut library = Library::new("host.invalid.async");
    let unary_scheme = Scheme::new(vec![], vec![], Type::fun(i32_type(), i32_type()));

    let err = library
        .export_native_async(
            "bad_async",
            unary_scheme,
            2,
            |_engine: EvaluatorRef<'_, ()>, _: Type, _args: Vec<Pointer>| {
                async { Err(rexlang_engine::EngineError::Internal("unused".into())) }.boxed()
            },
        )
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("does not accept 2 argument(s)"),
        "unexpected error: {msg}"
    );
}

#[tokio::test]
async fn module_injected_from_rust_wildcard_import() {
    let mut engine = engine_with_prelude();
    engine.add_default_resolvers();

    let mut library = Library::new("host.ops");
    library
        .export("triple", |_state: &(), x: i32| Ok(x * 3))
        .unwrap();
    library
        .export("add", |_state: &(), a: i32, b: i32| Ok(a + b))
        .unwrap();
    engine.inject_library(library).unwrap();

    let (value_ptr, ty) = eval_snippet(
        &mut engine,
        r#"
        import host.ops (*)
        add (triple 10) 2
"#,
    )
    .await
    .unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    let value = engine
        .heap
        .get(&value_ptr)
        .map(|value| value.as_ref().clone())
        .unwrap();
    match value {
        Value::I32(v) => assert_eq!(v, 32),
        _ => panic!(
            "expected i32, got {}",
            engine.heap.type_name(&value_ptr).unwrap()
        ),
    }
}

#[tokio::test]
async fn module_injected_from_rust_rejects_duplicate_module_name() {
    let mut engine = engine_with_prelude();

    let mut one = Library::new("host.dupe");
    one.export("x", |_state: &(), x: i32| Ok(x)).unwrap();
    engine.inject_library(one).unwrap();

    let mut two = Library::new("host.dupe");
    two.export("y", |_state: &(), x: i32| Ok(x)).unwrap();
    let err = engine.inject_library(two).unwrap_err();
    assert!(err.to_string().contains("already injected"));
}

#[tokio::test]
async fn library_import_rejects_private_access() {
    let dir = temp_dir("library_import_rejects_private_access");
    let main = dir.join("main.rex");
    let library = dir.join("foo").join("bar.rex");

    write_file(
        &library,
        r#"
        pub fn add x: i32 -> y: i32 -> i32 = x + y
        fn hidden x: i32 -> i32 = x + 1
        ()
"#,
    );
    write_file(
        &main,
        r#"
        import foo.bar as Bar
        Bar.hidden 1
"#,
    );

    let mut engine = engine_with_prelude();
    engine.add_default_resolvers();
    let err = match eval_library_file(&mut engine, &main).await {
        Ok(v) => panic!("expected error, got {v:?}"),
        Err(e) => e,
    };
    let msg = err.to_string();
    assert!(msg.contains("does not export"), "{msg}");
}

#[tokio::test]
async fn library_import_include_roots() {
    let dir = temp_dir("library_import_include_roots");
    let include_root = dir.join("includes");
    let main_root = dir.join("src");
    let main = main_root.join("main.rex");

    let library = include_root.join("lib").join("math.rex");
    write_file(
        &library,
        r#"
        pub fn inc x: i32 -> i32 = x + 1
        ()
"#,
    );

    write_file(
        &main,
        r#"
        import lib.math as Math
        Math.inc 41
"#,
    );

    let mut engine = engine_with_prelude();
    engine.add_default_resolvers();
    engine.add_include_resolver(&include_root).unwrap();
    let (value_ptr, ty) = eval_library_file(&mut engine, &main).await.unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    let value = engine
        .heap
        .get(&value_ptr)
        .map(|value| value.as_ref().clone())
        .unwrap();
    match value {
        Value::I32(v) => assert_eq!(v, 42),
        _ => panic!(
            "expected i32, got {}",
            engine.heap.type_name(&value_ptr).unwrap()
        ),
    }
}

#[tokio::test]
async fn snippet_can_import_with_explicit_base() {
    let dir = temp_dir("snippet_can_import_with_explicit_base");
    let library = dir.join("foo").join("bar.rex");
    write_file(
        &library,
        r#"
        pub fn add x: i32 -> y: i32 -> i32 = x + y
        ()
"#,
    );

    let mut engine = engine_with_prelude();
    engine.add_default_resolvers();
    let (value_ptr, ty) = eval_snippet_at(
        &mut engine,
        r#"
        import foo.bar as Bar
        Bar.add 20 22
"#,
        dir.join("_snippet.rex"),
    )
    .await
    .unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    let value = engine
        .heap
        .get(&value_ptr)
        .map(|value| value.as_ref().clone())
        .unwrap();

    match value {
        Value::I32(v) => assert_eq!(v, 42),
        _ => panic!(
            "expected i32, got {}",
            engine.heap.type_name(&value_ptr).unwrap()
        ),
    }
}

#[tokio::test]
async fn library_import_wildcard_clause() {
    let dir = temp_dir("library_import_wildcard_clause");
    let main = dir.join("main.rex");
    let library = dir.join("foo").join("bar.rex");

    write_file(
        &library,
        r#"
        pub fn add x: i32 -> y: i32 -> i32 = x + y
        pub fn triple x: i32 -> i32 = x * 3
        fn hidden x: i32 -> i32 = x + 1
        ()
"#,
    );
    write_file(
        &main,
        r#"
        import foo.bar (*)
        add (triple 10) 2
"#,
    );

    let mut engine = engine_with_prelude();
    engine.add_default_resolvers();
    let (value_ptr, ty) = eval_library_file(&mut engine, &main).await.unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    let value = engine
        .heap
        .get(&value_ptr)
        .map(|value| value.as_ref().clone())
        .unwrap();
    match value {
        Value::I32(v) => assert_eq!(v, 32),
        _ => panic!(
            "expected i32, got {}",
            engine.heap.type_name(&value_ptr).unwrap()
        ),
    }
}

#[tokio::test]
async fn library_import_selected_clause_with_alias() {
    let dir = temp_dir("library_import_selected_clause_with_alias");
    let main = dir.join("main.rex");
    let library = dir.join("foo").join("bar.rex");

    write_file(
        &library,
        r#"
        pub fn add x: i32 -> y: i32 -> i32 = x + y
        pub fn triple x: i32 -> i32 = x * 3
        ()
"#,
    );
    write_file(
        &main,
        r#"
        import foo.bar (add, triple as t)
        add (t 10) 2
"#,
    );

    let mut engine = engine_with_prelude();
    engine.add_default_resolvers();
    let (value_ptr, ty) = eval_library_file(&mut engine, &main).await.unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    let value = engine
        .heap
        .get(&value_ptr)
        .map(|value| value.as_ref().clone())
        .unwrap();
    match value {
        Value::I32(v) => assert_eq!(v, 32),
        _ => panic!(
            "expected i32, got {}",
            engine.heap.type_name(&value_ptr).unwrap()
        ),
    }
}

#[tokio::test]
async fn library_import_selected_clause_missing_export() {
    let dir = temp_dir("library_import_selected_clause_missing_export");
    let main = dir.join("main.rex");
    let library = dir.join("foo").join("bar.rex");

    write_file(
        &library,
        r#"
        pub fn add x: i32 -> y: i32 -> i32 = x + y
        ()
"#,
    );
    write_file(
        &main,
        r#"
        import foo.bar (missing)
        missing 1 2
"#,
    );

    let mut engine = engine_with_prelude();
    engine.add_default_resolvers();
    let err = match eval_library_file(&mut engine, &main).await {
        Ok(v) => panic!("expected error, got {v:?}"),
        Err(e) => e,
    };
    let msg = err.to_string();
    assert!(msg.contains("does not export"), "{msg}");
}

#[tokio::test]
async fn library_import_missing_class_export_in_instance_header() {
    let dir = temp_dir("library_import_missing_class_export_in_instance_header");
    let main = dir.join("main.rex");
    let dep = dir.join("dep.rex");

    write_file(
        &dep,
        r#"
        pub class Present a where
            present : a
        ()
"#,
    );
    write_file(
        &main,
        r#"
        import dep as D

        instance D.Missing i32 where
            missing = 1

        0
"#,
    );

    let mut engine = engine_with_prelude();
    engine.add_default_resolvers();
    let err = match eval_library_file(&mut engine, &main).await {
        Ok(v) => panic!("expected error, got {v:?}"),
        Err(e) => e,
    };
    let msg = err.to_string();
    assert!(msg.contains("does not export"), "{msg}");
    assert!(msg.contains("Missing"), "{msg}");
}

#[tokio::test]
async fn library_import_missing_type_export_in_fn_signature() {
    let dir = temp_dir("library_import_missing_type_export_in_fn_signature");
    let main = dir.join("main.rex");
    let dep = dir.join("dep.rex");

    write_file(
        &dep,
        r#"
        pub type Present = Present i32
        ()
"#,
    );
    write_file(
        &main,
        r#"
        import dep as D

        fn id x: D.Missing -> D.Missing = x

        0
"#,
    );

    let mut engine = engine_with_prelude();
    engine.add_default_resolvers();
    let err = match eval_library_file(&mut engine, &main).await {
        Ok(v) => panic!("expected error, got {v:?}"),
        Err(e) => e,
    };
    let msg = err.to_string();
    assert!(msg.contains("does not export"), "{msg}");
    assert!(msg.contains("Missing"), "{msg}");
}

#[tokio::test]
async fn library_import_missing_type_export_in_instance_head() {
    let dir = temp_dir("library_import_missing_type_export_in_instance_head");
    let main = dir.join("main.rex");
    let dep = dir.join("dep.rex");

    write_file(
        &dep,
        r#"
        pub class Marker a where
            marker : i32
        ()
"#,
    );
    write_file(
        &main,
        r#"
        import dep as D

        instance D.Marker D.Missing where
            marker = 1

        0
"#,
    );

    let mut engine = engine_with_prelude();
    engine.add_default_resolvers();
    let err = match eval_library_file(&mut engine, &main).await {
        Ok(v) => panic!("expected error, got {v:?}"),
        Err(e) => e,
    };
    let msg = err.to_string();
    assert!(msg.contains("does not export"), "{msg}");
    assert!(msg.contains("Missing"), "{msg}");
}

#[tokio::test]
async fn library_import_missing_class_export_in_fn_where_constraint() {
    let dir = temp_dir("library_import_missing_class_export_in_fn_where_constraint");
    let main = dir.join("main.rex");
    let dep = dir.join("dep.rex");

    write_file(
        &dep,
        r#"
        pub class Present a where
            present : a
        ()
"#,
    );
    write_file(
        &main,
        r#"
        import dep as D

        fn id x: i32 -> i32 where D.Missing i32 = x

        0
"#,
    );

    let mut engine = engine_with_prelude();
    engine.add_default_resolvers();
    let err = match eval_library_file(&mut engine, &main).await {
        Ok(v) => panic!("expected error, got {v:?}"),
        Err(e) => e,
    };
    let msg = err.to_string();
    assert!(msg.contains("does not export"), "{msg}");
    assert!(msg.contains("Missing"), "{msg}");
}

#[tokio::test]
async fn library_import_missing_class_export_in_declare_fn_where_constraint() {
    let dir = temp_dir("library_import_missing_class_export_in_declare_fn_where_constraint");
    let main = dir.join("main.rex");
    let dep = dir.join("dep.rex");

    write_file(
        &dep,
        r#"
        pub class Present a where
            present : a
        ()
"#,
    );
    write_file(
        &main,
        r#"
        import dep as D

        declare fn id x: i32 -> i32 where D.Missing i32

        0
"#,
    );

    let mut engine = engine_with_prelude();
    engine.add_default_resolvers();
    let err = match eval_library_file(&mut engine, &main).await {
        Ok(v) => panic!("expected error, got {v:?}"),
        Err(e) => e,
    };
    let msg = err.to_string();
    assert!(msg.contains("does not export"), "{msg}");
    assert!(msg.contains("Missing"), "{msg}");
}

#[tokio::test]
async fn library_import_missing_class_export_in_class_super_constraint() {
    let dir = temp_dir("library_import_missing_class_export_in_class_super_constraint");
    let main = dir.join("main.rex");
    let dep = dir.join("dep.rex");

    write_file(
        &dep,
        r#"
        pub class Present a where
            present : a
        ()
"#,
    );
    write_file(
        &main,
        r#"
        import dep as D

        class Local a <= D.Missing a where
            local : a

        0
"#,
    );

    let mut engine = engine_with_prelude();
    engine.add_default_resolvers();
    let err = match eval_library_file(&mut engine, &main).await {
        Ok(v) => panic!("expected error, got {v:?}"),
        Err(e) => e,
    };
    let msg = err.to_string();
    assert!(msg.contains("does not export"), "{msg}");
    assert!(msg.contains("Missing"), "{msg}");
}

#[tokio::test]
async fn library_import_missing_type_export_in_letrec_annotation_with_alias_named_binding() {
    let dir = temp_dir(
        "library_import_missing_type_export_in_letrec_annotation_with_alias_named_binding",
    );
    let main = dir.join("main.rex");
    let dep = dir.join("dep.rex");

    write_file(
        &dep,
        r#"
        pub type Present = Present i32
        ()
"#,
    );
    write_file(
        &main,
        r#"
        import dep as D

        let rec D: D.Missing = 1 in
        0
"#,
    );

    let mut engine = engine_with_prelude();
    engine.add_default_resolvers();
    let err = match eval_library_file(&mut engine, &main).await {
        Ok(v) => panic!("expected error, got {v:?}"),
        Err(e) => e,
    };
    let msg = err.to_string();
    assert!(msg.contains("does not export"), "{msg}");
    assert!(msg.contains("Missing"), "{msg}");
}

#[tokio::test]
async fn letrec_annotation_with_alias_named_binding_still_rewrites_valid_imported_type() {
    let dir =
        temp_dir("letrec_annotation_with_alias_named_binding_still_rewrites_valid_imported_type");
    let main = dir.join("main.rex");
    let dep = dir.join("dep.rex");

    write_file(
        &dep,
        r#"
        pub type Num = Num i32
        ()
"#,
    );
    write_file(
        &main,
        r#"
        import dep as D
        import dep (Num)

        let rec D: D.Num -> i32 = \_ -> 0 in
        0
"#,
    );

    let mut engine = engine_with_prelude();
    engine.add_default_resolvers();
    let (value_ptr, ty) = eval_library_file(&mut engine, &main).await.unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    match engine.heap.get(&value_ptr).unwrap().as_ref() {
        Value::I32(v) => assert_eq!(*v, 0),
        other => panic!("expected i32, got {}", other.value_type_name()),
    }
}

#[tokio::test]
async fn let_annotation_with_alias_named_binding_still_rewrites_valid_imported_type() {
    let dir =
        temp_dir("let_annotation_with_alias_named_binding_still_rewrites_valid_imported_type");
    let main = dir.join("main.rex");
    let dep = dir.join("dep.rex");

    write_file(
        &dep,
        r#"
        pub type Num = Num i32
        ()
"#,
    );
    write_file(
        &main,
        r#"
        import dep as D

        let D: D.Num -> i32 = \_ -> 0 in
        0
"#,
    );

    let mut engine = engine_with_prelude();
    engine.add_default_resolvers();
    let (value_ptr, ty) = eval_library_file(&mut engine, &main).await.unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    match engine.heap.get(&value_ptr).unwrap().as_ref() {
        Value::I32(v) => assert_eq!(*v, 0),
        other => panic!("expected i32, got {}", other.value_type_name()),
    }
}

#[tokio::test]
async fn library_import_missing_type_export_in_let_annotation_with_alias_named_binding() {
    let dir =
        temp_dir("library_import_missing_type_export_in_let_annotation_with_alias_named_binding");
    let main = dir.join("main.rex");
    let dep = dir.join("dep.rex");

    write_file(
        &dep,
        r#"
        pub type Present = Present i32
        ()
"#,
    );
    write_file(
        &main,
        r#"
        import dep as D

        let D: D.Missing = 1 in
        0
"#,
    );

    let mut engine = engine_with_prelude();
    engine.add_default_resolvers();
    let err = match eval_library_file(&mut engine, &main).await {
        Ok(v) => panic!("expected error, got {v:?}"),
        Err(e) => e,
    };
    let msg = err.to_string();
    assert!(msg.contains("does not export"), "{msg}");
    assert!(msg.contains("Missing"), "{msg}");
}

#[tokio::test]
async fn library_import_selected_clause_duplicate_name() {
    let dir = temp_dir("library_import_selected_clause_duplicate_name");
    let main = dir.join("main.rex");
    let left = dir.join("foo").join("left.rex");
    let right = dir.join("foo").join("right.rex");

    write_file(
        &left,
        r#"
        pub fn add x: i32 -> y: i32 -> i32 = x + y
        ()
"#,
    );
    write_file(
        &right,
        r#"
        pub fn mul x: i32 -> y: i32 -> i32 = x * y
        ()
"#,
    );
    write_file(
        &main,
        r#"
        import foo.left (add)
        import foo.right (mul as add)
        add 1 2
"#,
    );

    let mut engine = engine_with_prelude();
    engine.add_default_resolvers();
    let err = match eval_library_file(&mut engine, &main).await {
        Ok(v) => panic!("expected error, got {v:?}"),
        Err(e) => e,
    };
    let msg = err.to_string();
    assert!(msg.contains("duplicate imported name `add`"), "{msg}");
}

#[tokio::test]
async fn library_import_selected_clause_conflicts_with_local() {
    let dir = temp_dir("library_import_selected_clause_conflicts_with_local");
    let main = dir.join("main.rex");
    let library = dir.join("foo").join("bar.rex");

    write_file(
        &library,
        r#"
        pub fn add x: i32 -> y: i32 -> i32 = x + y
        ()
"#,
    );
    write_file(
        &main,
        r#"
        import foo.bar (add)
        fn add x: i32 -> y: i32 -> i32 = x - y
        add 10 2
"#,
    );

    let mut engine = engine_with_prelude();
    engine.add_default_resolvers();
    let err = match eval_library_file(&mut engine, &main).await {
        Ok(v) => panic!("expected error, got {v:?}"),
        Err(e) => e,
    };
    let msg = err.to_string();
    assert!(msg.contains("conflicts with local declaration"), "{msg}");
}

#[tokio::test]
async fn std_json_encode_decode_smoke() {
    let mut engine = engine_with_prelude();
    engine.add_default_resolvers();
    let (value_ptr, ty) = eval_snippet(
        &mut engine,
        r#"
        import std.json as Json

        let
          b_ok =
            match (Json.from_json (Json.to_json true))
              when Ok b -> if b then 1 else 0
              when Err _ -> -1
        in
          b_ok
"#,
    )
    .await
    .unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::I32));
    let value = engine.heap.pointer_as_i32(&value_ptr).unwrap();
    assert_eq!(value, 1);
}

#[tokio::test]
async fn std_json_roundtrip_nested() {
    let mut engine = engine_with_prelude();
    engine.add_default_resolvers();
    let (value_ptr, ty) = eval_snippet(
        &mut engine,
        r#"
        import std.json as Json

        let
          xs: List (Option (Result i32 string)) =
            [ Some (Ok (1 is i32))
            , None
            , Some (Err "no")
            , Some (Ok (42 is i32))
            ],

          xs_ok =
            match (Json.from_json (Json.to_json xs))
              when Ok ys -> if ys == xs then 1 else 0
              when Err _ -> -1,

          arr: Array (Result i32 string) =
            prim_array_from_list [Ok (1 is i32), Err "bad", Ok (3 is i32)],

          arr_ok =
            match (Json.from_json (Json.to_json arr))
              when Ok ys -> if ys == arr then 1 else 0
              when Err _ -> -1
        in
          (xs_ok, arr_ok)
"#,
    )
    .await
    .unwrap();
    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::builtin(BuiltinTypeId::I32),
            Type::builtin(BuiltinTypeId::I32)
        ])
    );
    let value = engine
        .heap
        .get(&value_ptr)
        .map(|value| value.as_ref().clone())
        .unwrap();

    let Value::Tuple(xs) = value else {
        panic!(
            "expected tuple, got {}",
            engine.heap.type_name(&value_ptr).unwrap()
        );
    };
    let xs = pvals!(engine, xs);
    let got: Vec<i32> = xs
        .into_iter()
        .map(|(pointer, v)| match v {
            Value::I32(n) => n,
            _ => panic!(
                "expected i32, got {}",
                engine.heap.type_name(&pointer).unwrap()
            ),
        })
        .collect();
    assert_eq!(got, vec![1, 1]);
}

#[tokio::test]
async fn std_json_decode_errors_have_useful_messages() {
    let mut engine = engine_with_prelude();
    engine.add_default_resolvers();
    let (value_ptr, ty) = eval_snippet(
        &mut engine,
        r#"
        import std.json as Json

        let
          both =
            let v = Json.Object { ok = Json.Number (prim_to_f64 (1 is i32)), err = Json.String "bad" } in
            match (Json.from_json v)
              when Ok r -> let _r: Result i32 string = r in "unexpected ok"
              when Err e -> e.message,

          neither =
            let v = Json.Object {} in
            match (Json.from_json v)
              when Ok r -> let _r: Result i32 string = r in "unexpected ok"
              when Err e -> e.message,

          wrong_kind =
            let v = Json.Bool true in
            match (Json.from_json v)
              when Ok xs -> let _xs: List i32 = xs in "unexpected ok"
              when Err e -> e.message,

          bad_list_elem =
            let v =
              Json.Array (prim_array_from_list [Json.Number (prim_to_f64 (1 is i32)), Json.String "oops"])
            in
            match (Json.from_json v)
              when Ok xs -> let _xs: List i32 = xs in "unexpected ok"
              when Err e -> e.message
        in
          (both, neither, wrong_kind, bad_list_elem)
"#,
    )
    .await
    .unwrap();
    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::builtin(BuiltinTypeId::String),
            Type::builtin(BuiltinTypeId::String),
            Type::builtin(BuiltinTypeId::String),
            Type::builtin(BuiltinTypeId::String),
        ])
    );
    let value = engine
        .heap
        .get(&value_ptr)
        .map(|value| value.as_ref().clone())
        .unwrap();

    let Value::Tuple(parts) = value else {
        panic!(
            "expected tuple, got {}",
            engine.heap.type_name(&value_ptr).unwrap()
        );
    };
    let parts = pvals!(engine, parts);
    let got: Vec<String> = parts
        .into_iter()
        .map(|(pointer, v)| match v {
            Value::String(s) => s,
            _ => panic!(
                "expected string, got {}",
                engine.heap.type_name(&pointer).unwrap()
            ),
        })
        .collect();

    assert!(got[0].contains("exactly one"), "{}", got[0]);
    assert!(got[1].contains("{ok} or {err}"), "{}", got[1]);
    assert!(got[2].contains("expected array, got bool"), "{}", got[2]);
    assert!(got[3].contains("expected number, got string"), "{}", got[3]);
}

#[tokio::test]
async fn std_json_numeric_decode_errors() {
    let mut engine = engine_with_prelude();
    engine.add_default_resolvers();
    let (value_ptr, ty) = eval_snippet(
        &mut engine,
        r#"
        import std.json as Json

        let
          u8_overflow =
            match (Json.from_json (Json.Number (prim_to_f64 (256 is i32))))
              when Ok n -> let _n: u8 = n in "unexpected ok"
              when Err e -> e.message,

          i32_fractional =
            match (Json.from_json (Json.Number (prim_to_f64 1.5)))
              when Ok n -> let _n: i32 = n in "unexpected ok"
              when Err e -> e.message
        in
          (u8_overflow, i32_fractional)
"#,
    )
    .await
    .unwrap();
    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::builtin(BuiltinTypeId::String),
            Type::builtin(BuiltinTypeId::String)
        ])
    );
    let value = engine
        .heap
        .get(&value_ptr)
        .map(|value| value.as_ref().clone())
        .unwrap();

    let Value::Tuple(parts) = value else {
        panic!(
            "expected tuple, got {}",
            engine.heap.type_name(&value_ptr).unwrap()
        );
    };
    let parts = pvals!(engine, parts);
    let got: Vec<String> = parts
        .into_iter()
        .map(|(pointer, v)| match v {
            Value::String(s) => s,
            _ => panic!(
                "expected string, got {}",
                engine.heap.type_name(&pointer).unwrap()
            ),
        })
        .collect();

    assert!(got[0].contains("representable as u8"), "{}", got[0]);
    assert!(got[1].contains("representable as i32"), "{}", got[1]);
}

#[tokio::test]
async fn std_json_show_renders_valid_json() {
    let mut engine = engine_with_prelude();
    engine.add_default_resolvers();
    let (value_ptr, ty) = eval_snippet(
        &mut engine,
        r#"
        import std.json as Json

        let
          v =
            Json.Object {
              a = Json.Number (prim_to_f64 (1 is i32)),
              b = Json.String "a\"b\\c\n",
              c =
                Json.Array (prim_array_from_list [
                  Json.Null,
                  Json.Bool true,
                  Json.Number (prim_to_f64 (0.0 / 0.0))
                ])
            }
        in
          show v
"#,
    )
    .await
    .unwrap();
    assert_eq!(ty, Type::builtin(BuiltinTypeId::String));
    let value = engine
        .heap
        .get(&value_ptr)
        .map(|value| value.as_ref().clone())
        .unwrap();

    let Value::String(rendered) = value else {
        panic!(
            "expected string, got {}",
            engine.heap.type_name(&value_ptr).unwrap()
        );
    };

    let parsed: serde_json::Value = serde_json::from_str(&rendered).unwrap();
    let obj = parsed.as_object().expect("expected object");

    assert_eq!(obj.get("a").and_then(|v| v.as_f64()), Some(1.0));
    assert_eq!(
        obj.get("b").and_then(|v| v.as_str()),
        Some("a\\\"b\\\\c\\n")
    );
    let arr = obj
        .get("c")
        .and_then(|v| v.as_array())
        .expect("expected array");
    assert_eq!(arr.len(), 3);
    assert_eq!(arr[0], serde_json::Value::Null);
    assert_eq!(arr[1], serde_json::Value::Bool(true));
    assert_eq!(arr[2], serde_json::Value::Null);
}

#[tokio::test]
async fn std_json_parse_and_from_string_roundtrip() {
    let mut engine = engine_with_prelude();
    engine.add_default_resolvers();
    let (value_ptr, ty) = eval_snippet(
        &mut engine,
        r#"
        import std.json as Json

        let
          v =
            Json.Object {
              a = Json.Number (prim_to_f64 (1 is i32)),
              b = Json.String "a\"b\\c\n",
              c = Json.Array (prim_array_from_list [Json.Null, Json.Bool true])
            },

          parsed_ok =
            match (Json.parse (show v))
              when Ok v2 -> if show v2 == show v then 1 else 0
              when Err _ -> -1,

          xs: List i32 = [(1 is i32), (2 is i32), (3 is i32)],
          s = Json.stringify (Json.to_json xs),
          decoded_ok =
            match (Json.parse s)
              when Err _ -> -1
              when Ok v0 ->
                (
                  match (Json.from_json v0)
                    when Ok ys -> if ys == xs then 1 else 0
                    when Err _ -> -2
                ),

          bad_s = "{",
          parse_err =
            match (Json.parse bad_s)
              when Ok _ -> 0
              when Err e -> if e.message != "" then 1 else 0
        in
          (parsed_ok, decoded_ok, parse_err)
"#,
    )
    .await
    .unwrap();
    assert_eq!(
        ty,
        Type::tuple(vec![
            Type::builtin(BuiltinTypeId::I32),
            Type::builtin(BuiltinTypeId::I32),
            Type::builtin(BuiltinTypeId::I32),
        ])
    );
    let value = engine
        .heap
        .get(&value_ptr)
        .map(|value| value.as_ref().clone())
        .unwrap();

    let Value::Tuple(xs) = value else {
        panic!(
            "expected tuple, got {}",
            engine.heap.type_name(&value_ptr).unwrap()
        );
    };
    let xs = pvals!(engine, xs);
    let got: Vec<i32> = xs
        .into_iter()
        .map(|(pointer, v)| match v {
            Value::I32(n) => n,
            _ => panic!(
                "expected i32, got {}",
                engine.heap.type_name(&pointer).unwrap()
            ),
        })
        .collect();
    assert_eq!(got, vec![1, 1, 1]);
}

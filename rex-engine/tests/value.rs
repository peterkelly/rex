use rex_ast::expr::sym;
use rex_engine::{EngineError, Heap, Value, ValueDisplayOptions};

#[test]
fn heap_rejects_handle_from_different_heap() {
    let heap_a = Heap::new();
    let heap_b = Heap::new();
    let handle = heap_a.alloc_i32(42).expect("alloc_i32 should succeed");

    let err = match heap_b.alloc_tuple(vec![handle]) {
        Ok(_) => panic!("cross-heap handle use should fail"),
        Err(err) => err,
    };
    let EngineError::Internal(msg) = err else {
        panic!("expected internal error for cross-heap handle");
    };
    assert!(msg.contains("different heap"), "unexpected error: {msg}");
}

#[test]
fn scoped_heap_allocates_and_reads() {
    Heap::scoped(|heap| {
        let handle = heap.alloc_i32(7).expect("alloc_i32 should succeed");
        assert_eq!(handle.to_rust::<i32>().unwrap(), 7);
    });
}

#[test]
fn handle_decode_reports_mismatch_with_native_type_error() {
    let heap = Heap::new();
    let handle = heap.alloc_bool(true).expect("alloc_bool should succeed");
    let err = handle
        .to_rust::<i32>()
        .expect_err("bool should not coerce to i32");
    match err {
        EngineError::NativeType { expected, got } => {
            assert_eq!(expected, "i32");
            assert_eq!(got, "bool");
        }
        other => panic!("unexpected error variant: {other:?}"),
    }
}

#[test]
fn handle_value_returns_payload_on_match() {
    let heap = Heap::new();
    let handle = heap.alloc_i32(42).expect("alloc_i32 should succeed");
    match handle.value().unwrap() {
        Value::I32(value) => assert_eq!(value, 42),
        _ => panic!("expected i32"),
    }
}

#[test]
fn value_display_default_strips_internal_noise() {
    let heap = Heap::new();
    let num = heap.alloc_i32(2).expect("alloc i32");
    assert_eq!(num.display().expect("display i32"), "2");

    let ctor = heap
        .alloc_adt(sym("@snippetabc.A"), vec![])
        .expect("alloc adt");
    assert_eq!(ctor.display().expect("display adt"), "A");
}

#[test]
fn value_display_unsanitized_keeps_suffixes_and_names() {
    let heap = Heap::new();
    let opts = ValueDisplayOptions::unsanitized();
    let num = heap.alloc_i32(2).expect("alloc i32");
    assert_eq!(num.display_with(opts).expect("display i32"), "2i32");

    let ctor = heap
        .alloc_adt(sym("@snippetabc.A"), vec![])
        .expect("alloc adt");
    assert_eq!(
        ctor.display_with(opts).expect("display adt"),
        "@snippetabc.A"
    );
}

#[test]
fn value_display_docs_mode_strips_internal_noise() {
    let heap = Heap::new();
    let opts = ValueDisplayOptions::docs();
    let num = heap.alloc_i32(2).expect("alloc i32");
    assert_eq!(num.display_with(opts).expect("display i32 docs"), "2");

    let ctor = heap
        .alloc_adt(sym("@snippetabc.A"), vec![])
        .expect("alloc adt");
    assert_eq!(ctor.display_with(opts).expect("display adt docs"), "A");

    let non_snippet = heap.alloc_adt(sym("pkg.A"), vec![]).expect("alloc adt");
    assert_eq!(
        non_snippet
            .display_with(opts)
            .expect("display non-snippet adt docs"),
        "pkg.A"
    );
}

use rex_ast::expr::sym;
use rex_engine::{
    EngineError, Heap, Value, ValueDisplayOptions, pointer_display, pointer_display_with,
};

#[test]
fn heap_rejects_pointer_from_different_heap() {
    let heap_a = Heap::new();
    let heap_b = Heap::new();
    let pointer = heap_a.alloc_i32(42).expect("alloc_i32 should succeed");

    let err = match heap_b.get(&pointer) {
        Ok(_) => panic!("cross-heap pointer use should fail"),
        Err(err) => err,
    };
    let EngineError::Internal(msg) = err else {
        panic!("expected internal error for cross-heap pointer");
    };
    assert!(msg.contains("different heap"), "unexpected error: {msg}");
}

#[test]
fn scoped_heap_allocates_and_reads() {
    Heap::scoped(|heap| {
        let pointer = heap.alloc_i32(7).expect("alloc_i32 should succeed");
        let value = heap.get(&pointer).expect("pointer should resolve");
        assert!(matches!(value.as_ref(), Value::I32(7)));
    });
}

#[test]
fn value_as_reports_mismatch_with_value_type_error() {
    let value = Value::Bool(true);
    let err = value
        .value_as_i32()
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
fn pointer_as_reports_mismatch_with_value_type_error() {
    let heap = Heap::new();
    let pointer = heap.alloc_bool(true).expect("alloc_bool should succeed");
    let err = heap
        .pointer_as_i32(&pointer)
        .expect_err("bool pointer should not coerce to i32");
    match err {
        EngineError::NativeType { expected, got } => {
            assert_eq!(expected, "i32");
            assert_eq!(got, "bool");
        }
        other => panic!("unexpected error variant: {other:?}"),
    }
}

#[test]
fn pointer_as_returns_payload_on_match() {
    let heap = Heap::new();
    let pointer = heap.alloc_i32(42).expect("alloc_i32 should succeed");
    let value = heap
        .pointer_as_i32(&pointer)
        .expect("i32 pointer should decode");
    assert_eq!(value, 42);
}

#[test]
fn value_display_default_keeps_suffixes_and_names() {
    let heap = Heap::new();
    let num = heap.alloc_i32(2).expect("alloc i32");
    let got_num = pointer_display(&heap, &num).expect("display i32");
    assert_eq!(got_num, "2");

    let ctor = heap
        .alloc_adt(sym("@snippetabc.A"), vec![])
        .expect("alloc adt");
    let got_ctor = pointer_display(&heap, &ctor).expect("display adt");
    assert_eq!(got_ctor, "A");
}

#[test]
fn value_display_unsanitized_keeps_suffixes_and_names() {
    let heap = Heap::new();
    let opts = ValueDisplayOptions::unsanitized();
    let num = heap.alloc_i32(2).expect("alloc i32");
    let got_num = pointer_display_with(&heap, &num, opts).expect("display i32");
    assert_eq!(got_num, "2i32");

    let ctor = heap
        .alloc_adt(sym("@snippetabc.A"), vec![])
        .expect("alloc adt");
    let got_ctor = pointer_display_with(&heap, &ctor, opts).expect("display adt");
    assert_eq!(got_ctor, "@snippetabc.A");
}

#[test]
fn value_display_docs_mode_strips_internal_noise() {
    let heap = Heap::new();
    let opts = ValueDisplayOptions::docs();
    let num = heap.alloc_i32(2).expect("alloc i32");
    let got_num = pointer_display_with(&heap, &num, opts).expect("display i32 docs");
    assert_eq!(got_num, "2");

    let ctor = heap
        .alloc_adt(sym("@snippetabc.A"), vec![])
        .expect("alloc adt");
    let got_ctor = pointer_display_with(&heap, &ctor, opts).expect("display adt docs");
    assert_eq!(got_ctor, "A");

    let non_snippet = heap.alloc_adt(sym("pkg.A"), vec![]).expect("alloc adt");
    let got_non_snippet =
        pointer_display_with(&heap, &non_snippet, opts).expect("display non-snippet adt docs");
    assert_eq!(got_non_snippet, "pkg.A");
}

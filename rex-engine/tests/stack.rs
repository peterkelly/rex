use std::sync::Arc;

use rex_ast::expr::sym;
use rex_engine::{Environment, FrSequenceState, FrTuple, Frame, Heap, Value, assert_pointer_eq};
use rex_typesystem::types::{BuiltinTypeId, Type, TypedExpr, TypedExprKind};

fn i32_type() -> Type {
    Type::builtin(BuiltinTypeId::I32)
}

fn int_expr(value: i64) -> Arc<TypedExpr> {
    Arc::new(TypedExpr::new(i32_type(), TypedExprKind::Int(value)))
}

fn tuple_expr() -> Arc<TypedExpr> {
    Arc::new(TypedExpr::new(
        Type::tuple(vec![i32_type(), i32_type()]),
        TypedExprKind::Tuple(vec![int_expr(1), int_expr(2)]),
    ))
}

#[test]
fn root_frame_parent_is_u64_zero_sentinel() {
    let heap = Heap::new();

    let root_parent = heap.alloc_root_frame_parent().unwrap();

    assert_eq!(heap.pointer_as_u64(&root_parent).unwrap(), 0);
}

#[test]
fn heap_allocates_reads_and_updates_frame_payloads() {
    let heap = Heap::new();
    let root_parent = heap.alloc_root_frame_parent().unwrap();
    let env_value = heap.alloc_i32(7).unwrap();
    let env = Environment::new().extend(sym("x"), env_value);
    let expr = tuple_expr();
    let frame = Frame::Tuple(FrTuple {
        parent: root_parent,
        expr: Arc::clone(&expr),
        env: env.clone(),
        state: FrSequenceState::Enter,
        next_index: 0,
        values: Vec::new(),
    });

    let frame_ptr = heap.alloc_frame(frame).unwrap();
    let stored = heap.pointer_as_frame(&frame_ptr).unwrap();
    assert_eq!(stored.parent(), &root_parent);
    assert!(Arc::ptr_eq(stored.expr(), &expr));
    assert_eq!(stored.env(), &env);

    let value = heap.alloc_i32(1).unwrap();
    let updated_len = heap
        .update_frame(&frame_ptr, |frame| match frame {
            Frame::Tuple(tuple) => {
                tuple.state = FrSequenceState::EvalItem;
                tuple.next_index = 1;
                tuple.values.push(value);
                Ok(tuple.values.len())
            }
            _ => panic!("expected tuple frame"),
        })
        .unwrap();

    assert_eq!(updated_len, 1);
    match heap.pointer_as_frame(&frame_ptr).unwrap() {
        Frame::Tuple(tuple) => {
            assert_eq!(tuple.state, FrSequenceState::EvalItem);
            assert_eq!(tuple.next_index, 1);
            assert_eq!(tuple.values, vec![value]);
        }
        _ => panic!("expected tuple frame"),
    }
}

#[test]
fn replace_frame_rejects_non_frame_pointers() {
    let heap = Heap::new();
    let root_parent = heap.alloc_root_frame_parent().unwrap();
    let non_frame = heap.alloc_i32(1).unwrap();
    let frame = Frame::Tuple(FrTuple {
        parent: root_parent,
        expr: tuple_expr(),
        env: Environment::new(),
        state: FrSequenceState::Enter,
        next_index: 0,
        values: Vec::new(),
    });

    let err = heap.replace_frame(&non_frame, frame).unwrap_err();

    match err {
        rex_engine::EngineError::NativeType { expected, got } => {
            assert_eq!(expected, "frame");
            assert_eq!(got, "i32");
        }
        other => panic!("expected NativeType, got {other:?}"),
    }
    assert_pointer_eq!(&heap, non_frame, heap.alloc_i32(1).unwrap());
}

#[test]
fn frames_remain_runtime_values() {
    let heap = Heap::new();
    let root_parent = heap.alloc_root_frame_parent().unwrap();
    let frame = Frame::Tuple(FrTuple {
        parent: root_parent,
        expr: tuple_expr(),
        env: Environment::new(),
        state: FrSequenceState::Enter,
        next_index: 0,
        values: Vec::new(),
    });

    let frame_ptr = heap.alloc_frame(frame).unwrap();
    let value = heap.get(&frame_ptr).unwrap();

    assert!(matches!(value.as_ref(), Value::Frame(_)));
    assert_eq!(heap.type_name(&frame_ptr).unwrap(), "frame");
}

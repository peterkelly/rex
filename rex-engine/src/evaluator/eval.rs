use crate::{
    env::Environment,
    error::EngineError,
    evaluator::{
        CallSite, application_result_type,
        context::Context,
        native_callable::NativeCallResult,
        native_functions::{NativeTask, eval_native_enter, eval_native_receive},
        resolve_arg_type,
        runtime_core::RuntimeCore,
        scheduler::{EvalScheduler, EvalWorkItem, poll_pending_native},
    },
    handlers::NativeAsyncCall,
    native_fn::NativeApplyResult,
    overloaded_fn::OverloadedFn,
    stack::{
        FrApp, FrAppArg, FrAppState, FrBool, FrBranchState, FrDateTime, FrDict, FrFloat, FrHole,
        FrInt, FrIte, FrLam, FrLet, FrLetRec, FrLetRecState, FrLetState, FrList, FrMatch,
        FrMatchArm, FrMatchState, FrNativeAsync, FrNativeCall, FrNativeCallState, FrProject,
        FrRecordUpdate, FrRecordUpdateState, FrSequenceState, FrString, FrTuple, FrUint, FrUuid,
        FrValueState, FrVar, Frame,
    },
    util::{is_function_type, split_fun},
    value::{Cell, Closure, Collection, Heap, HeapAccess, Pointer, TempRoots, list_to_vec},
};
use rex_ast::{Pattern, Symbol};
use rex_typesystem::{
    types::{BuiltinTypeId, Type, TypeKind, TypedExpr, TypedExprKind, Types},
    unification::{Subst, compose_subst, unify},
};
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

pub(crate) enum EvalControl<State: Clone + Send + Sync + 'static> {
    Push {
        expr: Arc<TypedExpr>,
        env: Environment,
    },
    PushFrame(Box<Frame>),
    Schedule(Vec<Pointer>),
    Wait,
    AwaitNative(NativeAsyncCall<State>),
    Return(Pointer),
}

pub(crate) async fn eval_typed_expr<State>(
    runtime: RuntimeCore<State>,
    env: Environment,
    expr: Arc<TypedExpr>,
) -> Result<Pointer, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let root_parent = runtime.heap.alloc_ptr_root_frame_parent()?;
    eval_typed_expr_from_parent(runtime, root_parent, EvalStop::RootSentinel, env, expr).await
}

#[derive(Clone, Copy)]
pub(crate) enum EvalStop {
    RootSentinel,
    #[allow(dead_code)]
    Parent(Pointer),
}

pub(crate) async fn eval_typed_expr_from_parent<State>(
    mut runtime: RuntimeCore<State>,
    initial_parent: Pointer,
    stop: EvalStop,
    env: Environment,
    expr: Arc<TypedExpr>,
) -> Result<Pointer, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let root_frame = runtime
        .heap
        .alloc_ptr_frame(frame_for_expr(initial_parent, expr, env))?;
    let mut scheduler = EvalScheduler::new(root_frame, runtime.parallelism_controller.clone());

    loop {
        if poll_pending_native(&mut runtime, &mut scheduler, false).await? {
            continue;
        }

        let mut item = match scheduler.pop_next() {
            Some(item) => item,
            None => {
                if poll_pending_native(&mut runtime, &mut scheduler, true).await? {
                    continue;
                }
                return Err(EngineError::Internal(
                    "eval scheduler ran out of ready work".into(),
                ));
            }
        };
        let mut protected = Vec::new();
        item.trace_pointers(&mut protected);
        scheduler.trace_pointers(&mut protected);
        runtime.trace_pointers(&mut protected)?;
        let roots = runtime.heap.temp_roots(protected)?;

        let mut cursor = 0;
        item.refresh_from_roots(&roots, &mut cursor)?;
        scheduler.refresh_from_roots(&roots, &mut cursor)?;
        runtime.refresh_from_roots(&roots, &mut cursor)?;

        let frame = runtime.heap.pointer_as_frame(&item.frame)?;
        let control = match item.returned {
            Some(returned) => {
                eval_receive(&runtime, item.frame, frame, returned.child, returned.value)?
            }
            None => eval_enter(&runtime, item.frame, frame)?,
        };

        let mut cursor = 0;
        item.refresh_from_roots(&roots, &mut cursor)?;
        scheduler.refresh_from_roots(&roots, &mut cursor)?;
        runtime.refresh_from_roots(&roots, &mut cursor)?;

        match control {
            EvalControl::Push { expr, env } => {
                let child = runtime
                    .heap
                    .alloc_ptr_frame(frame_for_expr(item.frame, expr, env))?;
                refresh_eval_roots(&mut runtime, &mut item, &mut scheduler, &roots)?;
                scheduler.schedule_next(EvalWorkItem::enter(child));
            }
            EvalControl::PushFrame(frame) => {
                let child = runtime.heap.alloc_ptr_frame(*frame)?;
                refresh_eval_roots(&mut runtime, &mut item, &mut scheduler, &roots)?;
                scheduler.schedule_next(EvalWorkItem::enter(child));
            }
            EvalControl::Schedule(children) => {
                for child in children.into_iter().rev() {
                    scheduler.schedule_next(EvalWorkItem::enter(child));
                }
            }
            EvalControl::Wait => {}
            EvalControl::AwaitNative(mut call) => {
                let mut protected = Vec::new();
                call.trace_pointers(&mut protected);
                let call_roots = runtime.heap.temp_roots(protected)?;
                let child = runtime
                    .heap
                    .alloc_ptr_frame(Frame::NativeAsync(FrNativeAsync { parent: item.frame }))?;
                refresh_eval_roots(&mut runtime, &mut item, &mut scheduler, &roots)?;
                let mut cursor = 0;
                call.refresh_from_roots(&call_roots, &mut cursor)?;
                scheduler.schedule_pending_native(child, call);
            }
            EvalControl::Return(value) => {
                let mut frame = runtime.heap.pointer_as_frame(&item.frame)?;
                let parent = *frame.parent();
                frame.mark_complete(value);
                runtime.heap.replace_frame(&item.frame, frame)?;
                match stop {
                    EvalStop::RootSentinel => {
                        if is_root_frame_parent(&runtime.heap, &parent)? {
                            return Ok(value);
                        }
                    }
                    EvalStop::Parent(stop_parent) => {
                        if parent == stop_parent {
                            return Ok(value);
                        }
                        if is_root_frame_parent(&runtime.heap, &parent)? {
                            return Err(EngineError::Internal(
                                "child evaluation reached root before parent frame".into(),
                            ));
                        }
                    }
                }
                scheduler.schedule_next(EvalWorkItem::receive(parent, item.frame, value));
            }
        }
    }
}

fn refresh_eval_roots<State>(
    runtime: &mut RuntimeCore<State>,
    item: &mut EvalWorkItem,
    scheduler: &mut EvalScheduler<State>,
    roots: &TempRoots,
) -> Result<(), EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let mut cursor = 0;
    item.refresh_from_roots(roots, &mut cursor)?;
    scheduler.refresh_from_roots(roots, &mut cursor)?;
    runtime.refresh_from_roots(roots, &mut cursor)
}

pub(crate) fn frame_for_expr(parent: Pointer, expr: Arc<TypedExpr>, env: Environment) -> Frame {
    let kind = Arc::clone(&expr.kind);
    match kind.as_ref() {
        TypedExprKind::Bool(_) => Frame::Bool(FrBool {
            parent,
            expr,
            env,
            state: FrValueState::Enter,
            value: None,
        }),
        TypedExprKind::Uint(_) => Frame::Uint(FrUint {
            parent,
            expr,
            env,
            state: FrValueState::Enter,
            value: None,
        }),
        TypedExprKind::Int(_) => Frame::Int(FrInt {
            parent,
            expr,
            env,
            state: FrValueState::Enter,
            value: None,
        }),
        TypedExprKind::Float(_) => Frame::Float(FrFloat {
            parent,
            expr,
            env,
            state: FrValueState::Enter,
            value: None,
        }),
        TypedExprKind::String(_) => Frame::String(FrString {
            parent,
            expr,
            env,
            state: FrValueState::Enter,
            value: None,
        }),
        TypedExprKind::Uuid(_) => Frame::Uuid(FrUuid {
            parent,
            expr,
            env,
            state: FrValueState::Enter,
            value: None,
        }),
        TypedExprKind::DateTime(_) => Frame::DateTime(FrDateTime {
            parent,
            expr,
            env,
            state: FrValueState::Enter,
            value: None,
        }),
        TypedExprKind::Hole => Frame::Hole(FrHole {
            parent,
            expr,
            env,
            state: FrValueState::Enter,
            value: None,
        }),
        TypedExprKind::Tuple(_) => Frame::Tuple(FrTuple {
            parent,
            expr,
            env,
            state: FrSequenceState::Enter,
            children: Vec::new(),
            values: Vec::new(),
            remaining: 0,
        }),
        TypedExprKind::List(_) => Frame::List(FrList {
            parent,
            expr,
            env,
            state: FrSequenceState::Enter,
            children: Vec::new(),
            values: Vec::new(),
            remaining: 0,
        }),
        TypedExprKind::Dict(kvs) => Frame::Dict(FrDict {
            parent,
            expr,
            env,
            state: FrSequenceState::Enter,
            keys: kvs.keys().cloned().collect(),
            children: Vec::new(),
            values: Vec::new(),
            remaining: 0,
        }),
        TypedExprKind::RecordUpdate { updates, .. } => Frame::RecordUpdate(FrRecordUpdate {
            parent,
            expr,
            env,
            state: FrRecordUpdateState::Enter,
            base_value: None,
            update_keys: updates.keys().cloned().collect(),
            update_children: Vec::new(),
            update_values: Vec::new(),
            remaining_updates: 0,
        }),
        TypedExprKind::Var { .. } => Frame::Var(FrVar {
            parent,
            expr,
            env,
            state: FrValueState::Enter,
            value: None,
        }),
        TypedExprKind::App(..) => Frame::App(FrApp {
            parent,
            expr,
            env,
            state: FrAppState::Enter,
            head: None,
            spine: Vec::new(),
            head_child: None,
            arg_children: Vec::new(),
            arg_values: Vec::new(),
            remaining: 0,
            next_arg_index: 0,
            func: None,
            arg: None,
        }),
        TypedExprKind::Project { .. } => Frame::Project(FrProject {
            parent,
            expr,
            env,
            state: FrValueState::Enter,
            value: None,
        }),
        TypedExprKind::Lam { .. } => Frame::Lam(FrLam {
            parent,
            expr,
            env,
            state: FrValueState::Enter,
            value: None,
        }),
        TypedExprKind::Let { .. } => Frame::Let(FrLet {
            parent,
            expr,
            env,
            state: FrLetState::Enter,
            def_value: None,
        }),
        TypedExprKind::LetRec { .. } => Frame::LetRec(FrLetRec {
            parent,
            expr,
            env,
            state: FrLetRecState::Enter,
            recursive_env: None,
            slots: Vec::new(),
            next_binding_index: 0,
            binding_value: None,
        }),
        TypedExprKind::Ite { .. } => Frame::Ite(FrIte {
            parent,
            expr,
            env,
            state: FrBranchState::Enter,
            cond_value: None,
            selected: None,
        }),
        TypedExprKind::Match { arms, .. } => Frame::Match(FrMatch {
            parent,
            expr,
            env,
            state: FrMatchState::Enter,
            scrutinee_value: None,
            arms: arms
                .iter()
                .map(|(pattern, expr)| FrMatchArm {
                    pattern: pattern.clone(),
                    expr: Arc::clone(expr),
                })
                .collect(),
            next_arm_index: 0,
            matched_env: None,
        }),
    }
}

fn eval_enter<State>(
    runtime: &RuntimeCore<State>,
    frame_ptr: Pointer,
    frame: Frame,
) -> Result<EvalControl<State>, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    match frame {
        Frame::Bool(frame) => match frame.expr.kind.as_ref() {
            TypedExprKind::Bool(value) => {
                Ok(EvalControl::Return(runtime.heap.alloc_ptr_bool(*value)?))
            }
            _ => frame_kind_error("bool"),
        },
        Frame::Uint(frame) => match frame.expr.kind.as_ref() {
            TypedExprKind::Uint(value) => Ok(EvalControl::Return(alloc_uint_literal_as(
                runtime,
                *value,
                &frame.expr.typ,
            )?)),
            _ => frame_kind_error("uint"),
        },
        Frame::Int(frame) => match frame.expr.kind.as_ref() {
            TypedExprKind::Int(value) => Ok(EvalControl::Return(alloc_int_literal_as(
                runtime,
                *value,
                &frame.expr.typ,
            )?)),
            _ => frame_kind_error("int"),
        },
        Frame::Float(frame) => match frame.expr.kind.as_ref() {
            TypedExprKind::Float(value) => Ok(EvalControl::Return(alloc_float_literal_as(
                runtime,
                *value,
                &frame.expr.typ,
            )?)),
            _ => frame_kind_error("float"),
        },
        Frame::String(frame) => match frame.expr.kind.as_ref() {
            TypedExprKind::String(value) => Ok(EvalControl::Return(
                runtime.heap.alloc_ptr_string(value.clone())?,
            )),
            _ => frame_kind_error("string"),
        },
        Frame::Uuid(frame) => match frame.expr.kind.as_ref() {
            TypedExprKind::Uuid(value) => {
                Ok(EvalControl::Return(runtime.heap.alloc_ptr_uuid(*value)?))
            }
            _ => frame_kind_error("uuid"),
        },
        Frame::DateTime(frame) => match frame.expr.kind.as_ref() {
            TypedExprKind::DateTime(value) => Ok(EvalControl::Return(
                runtime.heap.alloc_ptr_datetime(*value)?,
            )),
            _ => frame_kind_error("datetime"),
        },
        Frame::Hole(_) => Err(EngineError::UnsupportedExpr),
        Frame::Tuple(frame) => eval_tuple_enter(runtime, frame_ptr, frame),
        Frame::List(frame) => eval_list_enter(runtime, frame_ptr, frame),
        Frame::Dict(frame) => eval_dict_enter(runtime, frame_ptr, frame),
        Frame::RecordUpdate(mut frame) => {
            let base = match frame.expr.kind.as_ref() {
                TypedExprKind::RecordUpdate { base, .. } => Arc::clone(base),
                _ => return frame_kind_error("record update"),
            };
            frame.state = FrRecordUpdateState::EvalBase;
            let env = frame.env.clone();
            runtime
                .heap
                .replace_frame(&frame_ptr, Frame::RecordUpdate(frame))?;
            Ok(EvalControl::Push { expr: base, env })
        }
        Frame::Var(mut frame) => match frame.expr.kind.as_ref() {
            TypedExprKind::Var { name, .. } => {
                match eval_resolve_var(runtime, frame_ptr, &frame.env, name, &frame.expr.typ)? {
                    EvalVarResult::Value(value) => Ok(EvalControl::Return(value)),
                    EvalVarResult::Push { expr, env } => {
                        frame.state = FrValueState::Enter;
                        runtime.heap.replace_frame(&frame_ptr, Frame::Var(frame))?;
                        Ok(EvalControl::Push { expr, env })
                    }
                    EvalVarResult::AwaitNative(future) => {
                        frame.state = FrValueState::Enter;
                        runtime.heap.replace_frame(&frame_ptr, Frame::Var(frame))?;
                        Ok(EvalControl::AwaitNative(future))
                    }
                }
            }
            _ => frame_kind_error("var"),
        },
        Frame::App(frame) => eval_app_enter(runtime, frame_ptr, frame),
        Frame::Project(mut frame) => {
            let expr = match frame.expr.kind.as_ref() {
                TypedExprKind::Project { expr, .. } => Arc::clone(expr),
                _ => return frame_kind_error("project"),
            };
            frame.state = FrValueState::Enter;
            let env = frame.env.clone();
            runtime
                .heap
                .replace_frame(&frame_ptr, Frame::Project(frame))?;
            Ok(EvalControl::Push { expr, env })
        }
        Frame::Lam(frame) => match frame.expr.kind.as_ref() {
            TypedExprKind::Lam { param, body } => {
                let param_ty = split_fun(&frame.expr.typ)
                    .map(|(arg, _)| arg)
                    .ok_or_else(|| EngineError::NotCallable(frame.expr.typ.to_string()))?;
                let value = runtime.heap.alloc_ptr_closure(
                    frame.env.clone(),
                    param.clone(),
                    param_ty,
                    frame.expr.typ.clone(),
                    Arc::clone(body),
                )?;
                Ok(EvalControl::Return(value))
            }
            _ => frame_kind_error("lambda"),
        },
        Frame::Let(mut frame) => {
            let def = match frame.expr.kind.as_ref() {
                TypedExprKind::Let { def, .. } => Arc::clone(def),
                _ => return frame_kind_error("let"),
            };
            frame.state = FrLetState::EvalDef;
            let env = frame.env.clone();
            runtime.heap.replace_frame(&frame_ptr, Frame::Let(frame))?;
            Ok(EvalControl::Push { expr: def, env })
        }
        Frame::LetRec(frame) => {
            let TypedExprKind::LetRec { bindings, body } = frame.expr.kind.as_ref() else {
                return frame_kind_error("let rec");
            };
            let bindings = bindings.clone();
            let body = Arc::clone(body);
            let mut protected = vec![frame_ptr];
            Frame::LetRec(frame.clone()).trace_pointers(&mut protected);
            let frame_roots = runtime.heap.temp_roots(protected.clone())?;
            let mut slot_roots = Vec::with_capacity(bindings.len());
            for (name, _) in &bindings {
                let placeholder = runtime.heap.alloc_ptr_uninitialized(name.clone())?;
                slot_roots.push(runtime.heap.temp_roots(vec![placeholder])?);
            }
            let frame_ptr = frame_roots.get(0)?;
            let mut wrapped = Frame::LetRec(frame);
            refresh_frame_from_roots(&mut wrapped, &protected, &frame_roots, 1)?;
            let Frame::LetRec(mut frame) = wrapped else {
                return frame_kind_error("let rec");
            };
            let mut recursive_env = frame.env.clone();
            let mut slots = Vec::with_capacity(bindings.len());
            for ((name, _), root) in bindings.iter().zip(slot_roots.iter()) {
                let placeholder = root.get(0)?;
                recursive_env = recursive_env.extend(name.clone(), placeholder);
                slots.push(placeholder);
            }
            frame.recursive_env = Some(recursive_env.clone());
            frame.slots = slots;
            if bindings.is_empty() {
                frame.state = FrLetRecState::EvalBody;
                runtime
                    .heap
                    .replace_frame(&frame_ptr, Frame::LetRec(frame))?;
                return Ok(EvalControl::Push {
                    expr: body,
                    env: recursive_env,
                });
            }
            frame.state = FrLetRecState::EvalBinding;
            let def = Arc::clone(&bindings[0].1);
            runtime
                .heap
                .replace_frame(&frame_ptr, Frame::LetRec(frame))?;
            Ok(EvalControl::Push {
                expr: def,
                env: recursive_env,
            })
        }
        Frame::Ite(mut frame) => {
            let cond = match frame.expr.kind.as_ref() {
                TypedExprKind::Ite { cond, .. } => Arc::clone(cond),
                _ => return frame_kind_error("if"),
            };
            frame.state = FrBranchState::EvalCondition;
            let env = frame.env.clone();
            runtime.heap.replace_frame(&frame_ptr, Frame::Ite(frame))?;
            Ok(EvalControl::Push { expr: cond, env })
        }
        Frame::Match(mut frame) => {
            let scrutinee = match frame.expr.kind.as_ref() {
                TypedExprKind::Match { scrutinee, .. } => Arc::clone(scrutinee),
                _ => return frame_kind_error("match"),
            };
            frame.state = FrMatchState::EvalScrutinee;
            let env = frame.env.clone();
            runtime
                .heap
                .replace_frame(&frame_ptr, Frame::Match(frame))?;
            Ok(EvalControl::Push {
                expr: scrutinee,
                env,
            })
        }
        Frame::NativeCall(frame) => eval_native_enter(runtime, frame_ptr, frame),
        Frame::NativeAsync(_) => unexpected_child_result("native async"),
    }
}

fn eval_tuple_enter<State>(
    runtime: &RuntimeCore<State>,
    frame_ptr: Pointer,
    mut frame: FrTuple,
) -> Result<EvalControl<State>, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let elems = match frame.expr.kind.as_ref() {
        TypedExprKind::Tuple(elems) => elems.clone(),
        _ => return frame_kind_error("tuple"),
    };
    if elems.is_empty() {
        return Ok(EvalControl::Return(runtime.heap.alloc_ptr_tuple(vec![])?));
    }

    frame.state = FrSequenceState::EvalItem;
    frame.children = Vec::with_capacity(elems.len());
    frame.values = vec![None; elems.len()];
    frame.remaining = elems.len();
    runtime
        .heap
        .replace_frame(&frame_ptr, Frame::Tuple(frame))?;

    let roots = runtime.heap.temp_roots(vec![frame_ptr])?;
    for expr in elems {
        let current_frame_ptr = roots.get(0)?;
        let current_frame = match runtime.heap.pointer_as_frame(&current_frame_ptr)? {
            Frame::Tuple(frame) => frame,
            _ => return frame_kind_error("tuple"),
        };
        let child = runtime.heap.alloc_ptr_frame(frame_for_expr(
            current_frame_ptr,
            expr,
            current_frame.env.clone(),
        ))?;
        let current_frame_ptr = roots.get(0)?;
        let mut current_frame = match runtime.heap.pointer_as_frame(&current_frame_ptr)? {
            Frame::Tuple(frame) => frame,
            _ => return frame_kind_error("tuple"),
        };
        current_frame.children.push(child);
        runtime
            .heap
            .replace_frame(&current_frame_ptr, Frame::Tuple(current_frame))?;
    }

    let current_frame_ptr = roots.get(0)?;
    let current_frame = match runtime.heap.pointer_as_frame(&current_frame_ptr)? {
        Frame::Tuple(frame) => frame,
        _ => return frame_kind_error("tuple"),
    };
    Ok(EvalControl::Schedule(current_frame.children))
}

fn eval_list_enter<State>(
    runtime: &RuntimeCore<State>,
    frame_ptr: Pointer,
    mut frame: FrList,
) -> Result<EvalControl<State>, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let elems = match frame.expr.kind.as_ref() {
        TypedExprKind::List(elems) => elems.clone(),
        _ => return frame_kind_error("list"),
    };
    if elems.is_empty() {
        return Ok(EvalControl::Return(
            runtime
                .heap
                .alloc_ptr_adt(Symbol::intern("Empty"), vec![])?,
        ));
    }

    frame.state = FrSequenceState::EvalItem;
    frame.children = Vec::with_capacity(elems.len());
    frame.values = vec![None; elems.len()];
    frame.remaining = elems.len();
    runtime.heap.replace_frame(&frame_ptr, Frame::List(frame))?;

    let roots = runtime.heap.temp_roots(vec![frame_ptr])?;
    for expr in elems {
        let current_frame_ptr = roots.get(0)?;
        let current_frame = match runtime.heap.pointer_as_frame(&current_frame_ptr)? {
            Frame::List(frame) => frame,
            _ => return frame_kind_error("list"),
        };
        let child = runtime.heap.alloc_ptr_frame(frame_for_expr(
            current_frame_ptr,
            expr,
            current_frame.env.clone(),
        ))?;
        let current_frame_ptr = roots.get(0)?;
        let mut current_frame = match runtime.heap.pointer_as_frame(&current_frame_ptr)? {
            Frame::List(frame) => frame,
            _ => return frame_kind_error("list"),
        };
        current_frame.children.push(child);
        runtime
            .heap
            .replace_frame(&current_frame_ptr, Frame::List(current_frame))?;
    }

    let current_frame_ptr = roots.get(0)?;
    let current_frame = match runtime.heap.pointer_as_frame(&current_frame_ptr)? {
        Frame::List(frame) => frame,
        _ => return frame_kind_error("list"),
    };
    Ok(EvalControl::Schedule(current_frame.children))
}

fn eval_dict_enter<State>(
    runtime: &RuntimeCore<State>,
    frame_ptr: Pointer,
    mut frame: FrDict,
) -> Result<EvalControl<State>, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let exprs = dict_exprs_for_keys(&frame, &frame.keys)?;
    if exprs.is_empty() {
        return Ok(EvalControl::Return(
            runtime.heap.alloc_ptr_dict(BTreeMap::new())?,
        ));
    }

    frame.state = FrSequenceState::EvalItem;
    frame.children = Vec::with_capacity(exprs.len());
    frame.values = vec![None; exprs.len()];
    frame.remaining = exprs.len();
    runtime.heap.replace_frame(&frame_ptr, Frame::Dict(frame))?;

    let roots = runtime.heap.temp_roots(vec![frame_ptr])?;
    for expr in exprs {
        let current_frame_ptr = roots.get(0)?;
        let current_frame = match runtime.heap.pointer_as_frame(&current_frame_ptr)? {
            Frame::Dict(frame) => frame,
            _ => return frame_kind_error("dict"),
        };
        let child = runtime.heap.alloc_ptr_frame(frame_for_expr(
            current_frame_ptr,
            expr,
            current_frame.env.clone(),
        ))?;
        let current_frame_ptr = roots.get(0)?;
        let mut current_frame = match runtime.heap.pointer_as_frame(&current_frame_ptr)? {
            Frame::Dict(frame) => frame,
            _ => return frame_kind_error("dict"),
        };
        current_frame.children.push(child);
        runtime
            .heap
            .replace_frame(&current_frame_ptr, Frame::Dict(current_frame))?;
    }

    let current_frame_ptr = roots.get(0)?;
    let current_frame = match runtime.heap.pointer_as_frame(&current_frame_ptr)? {
        Frame::Dict(frame) => frame,
        _ => return frame_kind_error("dict"),
    };
    Ok(EvalControl::Schedule(current_frame.children))
}

fn eval_record_update_updates_enter<State>(
    runtime: &RuntimeCore<State>,
    frame_ptr: Pointer,
    mut frame: FrRecordUpdate,
) -> Result<EvalControl<State>, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let exprs = record_update_exprs_for_keys(&frame, &frame.update_keys)?;
    frame.state = FrRecordUpdateState::EvalUpdate;
    frame.update_children = Vec::with_capacity(exprs.len());
    frame.update_values = vec![None; exprs.len()];
    frame.remaining_updates = exprs.len();
    runtime
        .heap
        .replace_frame(&frame_ptr, Frame::RecordUpdate(frame))?;

    let roots = runtime.heap.temp_roots(vec![frame_ptr])?;
    for expr in exprs {
        let current_frame_ptr = roots.get(0)?;
        let current_frame = match runtime.heap.pointer_as_frame(&current_frame_ptr)? {
            Frame::RecordUpdate(frame) => frame,
            _ => return frame_kind_error("record update"),
        };
        let child = runtime.heap.alloc_ptr_frame(frame_for_expr(
            current_frame_ptr,
            expr,
            current_frame.env.clone(),
        ))?;
        let current_frame_ptr = roots.get(0)?;
        let mut current_frame = match runtime.heap.pointer_as_frame(&current_frame_ptr)? {
            Frame::RecordUpdate(frame) => frame,
            _ => return frame_kind_error("record update"),
        };
        current_frame.update_children.push(child);
        runtime
            .heap
            .replace_frame(&current_frame_ptr, Frame::RecordUpdate(current_frame))?;
    }

    let current_frame_ptr = roots.get(0)?;
    let current_frame = match runtime.heap.pointer_as_frame(&current_frame_ptr)? {
        Frame::RecordUpdate(frame) => frame,
        _ => return frame_kind_error("record update"),
    };
    Ok(EvalControl::Schedule(current_frame.update_children))
}

fn eval_app_enter<State>(
    runtime: &RuntimeCore<State>,
    frame_ptr: Pointer,
    mut frame: FrApp,
) -> Result<EvalControl<State>, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let mut spine = Vec::new();
    let mut head = Arc::clone(&frame.expr);
    while let TypedExprKind::App(func, arg) = head.kind.as_ref() {
        spine.push(FrAppArg {
            func_type: func.typ.clone(),
            expr: Arc::clone(arg),
        });
        head = Arc::clone(func);
    }
    spine.reverse();

    let arg_exprs = spine
        .iter()
        .map(|arg| Arc::clone(&arg.expr))
        .collect::<Vec<_>>();
    frame.state = FrAppState::EvalChildren;
    frame.head = Some(Arc::clone(&head));
    frame.spine = spine;
    frame.head_child = None;
    frame.arg_children = Vec::with_capacity(arg_exprs.len());
    frame.arg_values = vec![None; arg_exprs.len()];
    frame.remaining = arg_exprs.len() + 1;
    frame.next_arg_index = 0;
    frame.func = None;
    frame.arg = None;
    runtime.heap.replace_frame(&frame_ptr, Frame::App(frame))?;

    let roots = runtime.heap.temp_roots(vec![frame_ptr])?;
    let current_frame_ptr = roots.get(0)?;
    let current_frame = match runtime.heap.pointer_as_frame(&current_frame_ptr)? {
        Frame::App(frame) => frame,
        _ => return frame_kind_error("application"),
    };
    let head_child = runtime.heap.alloc_ptr_frame(frame_for_expr(
        current_frame_ptr,
        head,
        current_frame.env.clone(),
    ))?;
    let current_frame_ptr = roots.get(0)?;
    let mut current_frame = match runtime.heap.pointer_as_frame(&current_frame_ptr)? {
        Frame::App(frame) => frame,
        _ => return frame_kind_error("application"),
    };
    current_frame.head_child = Some(head_child);
    runtime
        .heap
        .replace_frame(&current_frame_ptr, Frame::App(current_frame))?;

    for expr in arg_exprs {
        let current_frame_ptr = roots.get(0)?;
        let current_frame = match runtime.heap.pointer_as_frame(&current_frame_ptr)? {
            Frame::App(frame) => frame,
            _ => return frame_kind_error("application"),
        };
        let child = runtime.heap.alloc_ptr_frame(frame_for_expr(
            current_frame_ptr,
            expr,
            current_frame.env.clone(),
        ))?;
        let current_frame_ptr = roots.get(0)?;
        let mut current_frame = match runtime.heap.pointer_as_frame(&current_frame_ptr)? {
            Frame::App(frame) => frame,
            _ => return frame_kind_error("application"),
        };
        current_frame.arg_children.push(child);
        runtime
            .heap
            .replace_frame(&current_frame_ptr, Frame::App(current_frame))?;
    }

    let current_frame_ptr = roots.get(0)?;
    let current_frame = match runtime.heap.pointer_as_frame(&current_frame_ptr)? {
        Frame::App(frame) => frame,
        _ => return frame_kind_error("application"),
    };
    let mut children = Vec::with_capacity(1 + current_frame.arg_children.len());
    children.push(
        current_frame
            .head_child
            .ok_or_else(|| EngineError::Internal("application frame missing head child".into()))?,
    );
    children.extend(current_frame.arg_children.iter().copied());
    Ok(EvalControl::Schedule(children))
}

fn receive_sequence_value(
    kind: &'static str,
    children: &[Pointer],
    values: &mut [Option<Pointer>],
    remaining: &mut usize,
    child: Pointer,
    value: Pointer,
) -> Result<(), EngineError> {
    let index = children
        .iter()
        .position(|candidate| *candidate == child)
        .ok_or_else(|| {
            EngineError::Internal(format!("{kind} received result from unknown child"))
        })?;
    if values.get(index).and_then(|value| *value).is_some() {
        return Err(EngineError::Internal(format!(
            "{kind} received duplicate result from child"
        )));
    }
    let slot = values
        .get_mut(index)
        .ok_or_else(|| EngineError::Internal(format!("{kind} result slot index out of bounds")))?;
    *slot = Some(value);
    *remaining = remaining.checked_sub(1).ok_or_else(|| {
        EngineError::Internal(format!("{kind} received more results than expected"))
    })?;
    Ok(())
}

fn receive_app_child_value(
    frame: &mut FrApp,
    child: Pointer,
    value: Pointer,
) -> Result<(), EngineError> {
    if frame.head_child == Some(child) {
        if frame.func.is_some() {
            return Err(EngineError::Internal(
                "application received duplicate result from head child".into(),
            ));
        }
        frame.func = Some(value);
    } else {
        let index = frame
            .arg_children
            .iter()
            .position(|candidate| *candidate == child)
            .ok_or_else(|| {
                EngineError::Internal("application received result from unknown child".into())
            })?;
        if frame
            .arg_values
            .get(index)
            .and_then(|value| *value)
            .is_some()
        {
            return Err(EngineError::Internal(
                "application received duplicate result from argument child".into(),
            ));
        }
        let slot = frame.arg_values.get_mut(index).ok_or_else(|| {
            EngineError::Internal("application argument result slot index out of bounds".into())
        })?;
        *slot = Some(value);
    }
    frame.remaining = frame.remaining.checked_sub(1).ok_or_else(|| {
        EngineError::Internal("application received more results than expected".into())
    })?;
    Ok(())
}

fn completed_values(
    kind: &'static str,
    values: &[Option<Pointer>],
) -> Result<Vec<Pointer>, EngineError> {
    values
        .iter()
        .copied()
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| EngineError::Internal(format!("{kind} completed with missing result")))
}

fn map_keys_to_values(
    kind: &'static str,
    keys: &[Symbol],
    values: Vec<Pointer>,
) -> Result<BTreeMap<Symbol, Pointer>, EngineError> {
    if keys.len() != values.len() {
        return Err(EngineError::Internal(format!(
            "{kind} completed with mismatched keys and values"
        )));
    }
    Ok(keys.iter().cloned().zip(values).collect())
}

fn dict_exprs_for_keys(
    frame: &FrDict,
    keys: &[Symbol],
) -> Result<Vec<Arc<TypedExpr>>, EngineError> {
    match frame.expr.kind.as_ref() {
        TypedExprKind::Dict(kvs) => keys
            .iter()
            .map(|key| {
                kvs.get(key)
                    .cloned()
                    .ok_or_else(|| EngineError::Internal("dict frame key missing".into()))
            })
            .collect(),
        _ => frame_kind_error("dict"),
    }
}

fn record_update_exprs_for_keys(
    frame: &FrRecordUpdate,
    keys: &[Symbol],
) -> Result<Vec<Arc<TypedExpr>>, EngineError> {
    match frame.expr.kind.as_ref() {
        TypedExprKind::RecordUpdate { updates, .. } => {
            keys.iter()
                .map(|key| {
                    updates.get(key).cloned().ok_or_else(|| {
                        EngineError::Internal("record update frame key missing".into())
                    })
                })
                .collect()
        }
        _ => frame_kind_error("record update"),
    }
}

fn eval_receive<State>(
    runtime: &RuntimeCore<State>,
    frame_ptr: Pointer,
    frame: Frame,
    child: Pointer,
    value: Pointer,
) -> Result<EvalControl<State>, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    match frame {
        Frame::Tuple(mut frame) => {
            if frame.state != FrSequenceState::EvalItem {
                return unexpected_child_result("tuple");
            }
            let index = frame
                .children
                .iter()
                .position(|candidate| *candidate == child)
                .ok_or_else(|| {
                    EngineError::Internal("tuple received result from unknown child".into())
                })?;
            if frame.values.get(index).and_then(|value| *value).is_some() {
                return Err(EngineError::Internal(
                    "tuple received duplicate result from child".into(),
                ));
            }
            let slot = frame.values.get_mut(index).ok_or_else(|| {
                EngineError::Internal("tuple result slot index out of bounds".into())
            })?;
            *slot = Some(value);
            frame.remaining = frame.remaining.checked_sub(1).ok_or_else(|| {
                EngineError::Internal("tuple received more results than expected".into())
            })?;
            if frame.remaining == 0 {
                let values = frame
                    .values
                    .iter()
                    .copied()
                    .collect::<Option<Vec<_>>>()
                    .ok_or_else(|| {
                        EngineError::Internal("tuple completed with missing result".into())
                    })?;
                return Ok(EvalControl::Return(runtime.heap.alloc_ptr_tuple(values)?));
            }
            runtime
                .heap
                .replace_frame(&frame_ptr, Frame::Tuple(frame))?;
            Ok(EvalControl::Wait)
        }
        Frame::List(mut frame) => {
            if frame.state != FrSequenceState::EvalItem {
                return unexpected_child_result("list");
            }
            receive_sequence_value(
                "list",
                &frame.children,
                &mut frame.values,
                &mut frame.remaining,
                child,
                value,
            )?;
            if frame.remaining == 0 {
                let list = runtime
                    .heap
                    .alloc_ptr_list(completed_values("list", &frame.values)?)?;
                return Ok(EvalControl::Return(list));
            }
            runtime.heap.replace_frame(&frame_ptr, Frame::List(frame))?;
            Ok(EvalControl::Wait)
        }
        Frame::Dict(mut frame) => {
            if frame.state != FrSequenceState::EvalItem {
                return unexpected_child_result("dict");
            }
            receive_sequence_value(
                "dict",
                &frame.children,
                &mut frame.values,
                &mut frame.remaining,
                child,
                value,
            )?;
            if frame.remaining == 0 {
                let values = map_keys_to_values(
                    "dict",
                    &frame.keys,
                    completed_values("dict", &frame.values)?,
                )?;
                return Ok(EvalControl::Return(runtime.heap.alloc_ptr_dict(values)?));
            }
            runtime.heap.replace_frame(&frame_ptr, Frame::Dict(frame))?;
            Ok(EvalControl::Wait)
        }
        Frame::RecordUpdate(mut frame) => match frame.state {
            FrRecordUpdateState::EvalBase => {
                frame.base_value = Some(value);
                if frame.update_keys.is_empty() {
                    let result = apply_record_update_values(runtime, value, BTreeMap::new())?;
                    return Ok(EvalControl::Return(result));
                }
                eval_record_update_updates_enter(runtime, frame_ptr, frame)
            }
            FrRecordUpdateState::EvalUpdate => {
                receive_sequence_value(
                    "record update",
                    &frame.update_children,
                    &mut frame.update_values,
                    &mut frame.remaining_updates,
                    child,
                    value,
                )?;
                if frame.remaining_updates == 0 {
                    let base = frame.base_value.ok_or_else(|| {
                        EngineError::Internal("record update frame missing base".into())
                    })?;
                    let update_values = map_keys_to_values(
                        "record update",
                        &frame.update_keys,
                        completed_values("record update", &frame.update_values)?,
                    )?;
                    let result = apply_record_update_values(runtime, base, update_values)?;
                    return Ok(EvalControl::Return(result));
                }
                runtime
                    .heap
                    .replace_frame(&frame_ptr, Frame::RecordUpdate(frame))?;
                Ok(EvalControl::Wait)
            }
            _ => unexpected_child_result("record update"),
        },
        Frame::Var(_) => Ok(EvalControl::Return(value)),
        Frame::App(mut frame) => match frame.state {
            FrAppState::EvalChildren => {
                receive_app_child_value(&mut frame, child, value)?;
                if frame.remaining == 0 {
                    frame.next_arg_index = 0;
                    return continue_app_after_apply(runtime, frame_ptr, frame, None);
                }
                runtime.heap.replace_frame(&frame_ptr, Frame::App(frame))?;
                Ok(EvalControl::Wait)
            }
            FrAppState::ApplyArg => {
                continue_app_after_apply(runtime, frame_ptr, frame, Some(value))
            }
            _ => unexpected_child_result("application"),
        },
        Frame::Project(frame) => match frame.expr.kind.as_ref() {
            TypedExprKind::Project { field, .. } => Ok(EvalControl::Return(project_pointer(
                &runtime.heap,
                field,
                &value,
            )?)),
            _ => frame_kind_error("project"),
        },
        Frame::Let(mut frame) => match frame.state {
            FrLetState::EvalDef => {
                let TypedExprKind::Let { name, body, .. } = frame.expr.kind.as_ref() else {
                    return frame_kind_error("let");
                };
                frame.def_value = Some(value);
                frame.state = FrLetState::EvalBody;
                let env = frame.env.extend(name.clone(), value);
                let body = Arc::clone(body);
                runtime.heap.replace_frame(&frame_ptr, Frame::Let(frame))?;
                Ok(EvalControl::Push { expr: body, env })
            }
            FrLetState::EvalBody => Ok(EvalControl::Return(value)),
            _ => unexpected_child_result("let"),
        },
        Frame::LetRec(mut frame) => match frame.state {
            FrLetRecState::EvalBinding => {
                let TypedExprKind::LetRec { bindings, body } = frame.expr.kind.as_ref() else {
                    return frame_kind_error("let rec");
                };
                let idx = frame.next_binding_index;
                let slot = *frame.slots.get(idx).ok_or_else(|| {
                    EngineError::Internal("let rec frame slot index out of bounds".into())
                })?;
                let cell = runtime.heap.clone_cell(&value)?;
                runtime.heap.overwrite(&slot, cell)?;
                frame.binding_value = Some(value);
                frame.next_binding_index += 1;
                let recursive_env = frame.recursive_env.clone().ok_or_else(|| {
                    EngineError::Internal("let rec frame missing recursive environment".into())
                })?;
                if frame.next_binding_index == bindings.len() {
                    frame.state = FrLetRecState::EvalBody;
                    let body = Arc::clone(body);
                    runtime
                        .heap
                        .replace_frame(&frame_ptr, Frame::LetRec(frame))?;
                    return Ok(EvalControl::Push {
                        expr: body,
                        env: recursive_env,
                    });
                }
                let def = Arc::clone(&bindings[frame.next_binding_index].1);
                runtime
                    .heap
                    .replace_frame(&frame_ptr, Frame::LetRec(frame))?;
                Ok(EvalControl::Push {
                    expr: def,
                    env: recursive_env,
                })
            }
            FrLetRecState::EvalBody => Ok(EvalControl::Return(value)),
            _ => unexpected_child_result("let rec"),
        },
        Frame::Ite(mut frame) => match frame.state {
            FrBranchState::EvalCondition => {
                let TypedExprKind::Ite {
                    then_expr,
                    else_expr,
                    ..
                } = frame.expr.kind.as_ref()
                else {
                    return frame_kind_error("if");
                };
                let selected = match runtime.heap.pointer_as_bool(&value) {
                    Ok(true) => Arc::clone(then_expr),
                    Ok(false) => Arc::clone(else_expr),
                    Err(EngineError::NativeType { got, .. }) => {
                        return Err(EngineError::ExpectedBool(got));
                    }
                    Err(err) => return Err(err),
                };
                frame.cond_value = Some(value);
                frame.selected = Some(Arc::clone(&selected));
                frame.state = FrBranchState::EvalSelected;
                let env = frame.env.clone();
                runtime.heap.replace_frame(&frame_ptr, Frame::Ite(frame))?;
                Ok(EvalControl::Push {
                    expr: selected,
                    env,
                })
            }
            FrBranchState::EvalSelected => Ok(EvalControl::Return(value)),
            _ => unexpected_child_result("if"),
        },
        Frame::Match(mut frame) => match frame.state {
            FrMatchState::EvalScrutinee => {
                frame.scrutinee_value = Some(value);
                for idx in frame.next_arm_index..frame.arms.len() {
                    let arm = &frame.arms[idx];
                    if let Some(bindings) = match_pattern_ptr(&runtime.heap, &arm.pattern, &value) {
                        let env = frame.env.extend_many(bindings);
                        let expr = Arc::clone(&arm.expr);
                        frame.next_arm_index = idx;
                        frame.matched_env = Some(env.clone());
                        frame.state = FrMatchState::EvalArm;
                        runtime
                            .heap
                            .replace_frame(&frame_ptr, Frame::Match(frame))?;
                        return Ok(EvalControl::Push { expr, env });
                    }
                }
                Err(EngineError::MatchFailure)
            }
            FrMatchState::EvalArm => Ok(EvalControl::Return(value)),
            _ => unexpected_child_result("match"),
        },
        Frame::NativeCall(frame) => eval_native_receive(runtime, frame_ptr, frame, child, value),
        Frame::NativeAsync(_) => Ok(EvalControl::Return(value)),
        _ => unexpected_child_result("value"),
    }
}

pub(crate) fn refresh_frame_from_roots(
    frame: &mut Frame,
    originals: &[Pointer],
    roots: &TempRoots,
    start: usize,
) -> Result<(), EngineError> {
    if !roots.has_collected_since_creation()? {
        return Ok(());
    }

    let mut rewrites = HashMap::with_capacity(originals.len().saturating_sub(start));
    for (idx, original) in originals.iter().enumerate().skip(start) {
        rewrites.insert(*original, roots.get(idx)?);
    }
    frame.rewrite_pointers(&mut |pointer| Ok(rewrites.get(&pointer).copied().unwrap_or(pointer)))
}

fn eval_apply_overloaded_arg<State>(
    runtime: &RuntimeCore<State>,
    parent: Pointer,
    mut over: OverloadedFn,
    arg: Pointer,
    func_type: Option<&Type>,
    arg_type: Option<&Type>,
) -> Result<EvalApplyResult<State>, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    if let Some(expected) = func_type {
        let subst = unify(&over.typ, expected).map_err(|_| EngineError::NativeType {
            expected: over.typ.to_string(),
            got: expected.to_string(),
        })?;
        over.typ = over.typ.apply(&subst);
    }
    let (arg_ty, rest_ty) =
        split_fun(&over.typ).ok_or_else(|| EngineError::NotCallable(over.typ.to_string()))?;
    let actual_ty = resolve_arg_type(&runtime.heap, arg_type, &arg)?;
    let subst = unify(&arg_ty, &actual_ty).map_err(|_| EngineError::NativeType {
        expected: arg_ty.to_string(),
        got: actual_ty.to_string(),
    })?;
    let rest_ty = rest_ty.apply(&subst);
    over.applied.push(arg);
    over.applied_types.push(actual_ty);
    if is_function_type(&rest_ty) {
        return Ok(EvalApplyResult::Value(runtime.heap.alloc_ptr_overloaded(
            over.name,
            rest_ty,
            over.applied,
            over.applied_types,
        )?));
    }

    let mut full_ty = rest_ty;
    for arg_ty in over.applied_types.iter().rev() {
        full_ty = Type::fun(arg_ty.clone(), full_ty);
    }

    if runtime.type_system.class_methods.contains_key(&over.name) {
        let ctx = Context::new_with_parent(runtime, parent);
        return match ctx.resolve_class_method_plan(&over.name, &full_ty)? {
            Ok((env, method)) => {
                let args = over
                    .applied
                    .into_iter()
                    .zip(over.applied_types)
                    .collect::<Vec<_>>();
                let (env, expr) = synthetic_application_expr_from_head(env, method, &args)?;
                Ok(EvalApplyResult::Push {
                    expr: Arc::new(expr),
                    env,
                })
            }
            Err(pointer) => Ok(EvalApplyResult::Value(pointer)),
        };
    }

    let call_site = CallSite::child(parent);
    let ctx = Context::new_at_call_site(runtime, call_site)
        .resolve_native_impl(over.name.as_ref(), &full_ty)?;
    match ctx
        .func
        .call_at_site(runtime, full_ty, &over.applied, call_site)?
    {
        NativeCallResult::Ready(value) => Ok(EvalApplyResult::Value(value)),
        NativeCallResult::Pending(future) => Ok(EvalApplyResult::AwaitNative(future)),
    }
}

fn eval_apply_arg<State>(
    runtime: &RuntimeCore<State>,
    parent: Pointer,
    func: Pointer,
    arg: Pointer,
    func_type: Option<&Type>,
    arg_type: Option<&Type>,
) -> Result<EvalApplyResult<State>, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let func_value = runtime.heap.clone_cell(&func)?;
    match func_value {
        Cell::Closure(Closure {
            env,
            param,
            param_ty,
            typ,
            body,
        }) => {
            let mut subst = Subst::new_sync();
            if let Some(expected) = func_type {
                let s_fun = unify(&typ, expected).map_err(|_| EngineError::NativeType {
                    expected: typ.to_string(),
                    got: expected.to_string(),
                })?;
                subst = compose_subst(s_fun, subst);
            }
            let actual_ty = resolve_arg_type(&runtime.heap, arg_type, &arg)?;
            let param_ty = param_ty.apply(&subst);
            let s_arg = unify(&param_ty, &actual_ty).map_err(|_| EngineError::NativeType {
                expected: param_ty.to_string(),
                got: actual_ty.to_string(),
            })?;
            subst = compose_subst(s_arg, subst);
            Ok(EvalApplyResult::Push {
                expr: Arc::new(body.apply(&subst)),
                env: env.extend(param, arg),
            })
        }
        Cell::Native(native) => {
            match native.apply_at_site(runtime, arg, arg_type, CallSite::child(parent))? {
                NativeApplyResult::Value(value) => Ok(EvalApplyResult::Value(value)),
                NativeApplyResult::Task(task) => Ok(EvalApplyResult::PushNative(task)),
                NativeApplyResult::Pending(future) => Ok(EvalApplyResult::AwaitNative(future)),
            }
        }
        Cell::Overloaded(over) => {
            eval_apply_overloaded_arg(runtime, parent, over, arg, func_type, arg_type)
        }
        _ => Err(EngineError::NotCallable(
            runtime.heap.type_name(&func)?.into(),
        )),
    }
}

fn continue_app_after_apply<State>(
    runtime: &RuntimeCore<State>,
    mut frame_ptr: Pointer,
    mut frame: FrApp,
    applied: Option<Pointer>,
) -> Result<EvalControl<State>, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    if let Some(applied) = applied {
        frame.arg = None;
        frame.func = Some(applied);
        frame.next_arg_index += 1;
        runtime
            .heap
            .replace_frame(&frame_ptr, Frame::App(frame.clone()))?;
    }

    loop {
        if frame.next_arg_index > frame.spine.len() {
            return Err(EngineError::Internal(
                "application frame advanced past final argument".into(),
            ));
        }

        let func = frame
            .func
            .ok_or_else(|| EngineError::Internal("application frame missing function".into()))?;
        if frame.next_arg_index == frame.spine.len() {
            return Ok(EvalControl::Return(func));
        }

        let idx = frame.next_arg_index;
        let arg_info =
            frame.spine.get(idx).cloned().ok_or_else(|| {
                EngineError::Internal("application frame index out of bounds".into())
            })?;
        let arg = frame
            .arg_values
            .get(idx)
            .copied()
            .flatten()
            .ok_or_else(|| {
                EngineError::Internal("application frame missing argument value".into())
            })?;

        frame.arg = Some(arg);
        frame.state = FrAppState::ApplyArg;
        runtime
            .heap
            .replace_frame(&frame_ptr, Frame::App(frame.clone()))?;
        let roots = runtime.heap.temp_roots(vec![frame_ptr, func, arg])?;
        let apply_result = eval_apply_arg(
            runtime,
            frame_ptr,
            func,
            arg,
            Some(&arg_info.func_type),
            Some(&arg_info.expr.typ),
        )?;
        frame_ptr = roots.get(0)?;
        frame = match runtime.heap.pointer_as_frame(&frame_ptr)? {
            Frame::App(frame) => frame,
            _ => return frame_kind_error("application"),
        };
        match apply_result {
            EvalApplyResult::Value(applied) => {
                frame.arg = None;
                frame.func = Some(applied);
                frame.next_arg_index += 1;
                runtime
                    .heap
                    .replace_frame(&frame_ptr, Frame::App(frame.clone()))?;
            }
            EvalApplyResult::Push { expr, env } => return Ok(EvalControl::Push { expr, env }),
            EvalApplyResult::PushNative(task) => {
                return Ok(EvalControl::PushFrame(Box::new(Frame::NativeCall(
                    FrNativeCall {
                        parent: frame_ptr,
                        state: FrNativeCallState::Enter,
                        task,
                    },
                ))));
            }
            EvalApplyResult::AwaitNative(future) => return Ok(EvalControl::AwaitNative(future)),
        }
    }
}

fn eval_resolve_var<State>(
    runtime: &RuntimeCore<State>,
    parent: Pointer,
    env: &Environment,
    name: &Symbol,
    typ: &Type,
) -> Result<EvalVarResult<State>, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    if let Some(ptr) = env.get(name) {
        let native = runtime.heap.with_access(|heap| match heap.get(&ptr)? {
            Cell::Native(native) if native.arity == 0 && native.applied.is_empty() => {
                Ok(Some(native.clone()))
            }
            _ => Ok(None),
        })?;
        if let Some(native) = native {
            match native.call_zero_at_site(runtime, CallSite::child(parent))? {
                NativeCallResult::Ready(value) => Ok(EvalVarResult::Value(value)),
                NativeCallResult::Pending(future) => Ok(EvalVarResult::AwaitNative(future)),
            }
        } else {
            Ok(EvalVarResult::Value(ptr))
        }
    } else if runtime.type_system.class_methods.contains_key(name) {
        let ctx = Context::new_with_parent(runtime, parent);
        if let Some(pointer) = ctx.cached_class_method(name, typ) {
            return Ok(EvalVarResult::Value(pointer));
        }
        match ctx.resolve_class_method_plan(name, typ)? {
            Ok((env, specialized)) => Ok(EvalVarResult::Push {
                expr: Arc::new(specialized),
                env,
            }),
            Err(pointer) => Ok(EvalVarResult::Value(pointer)),
        }
    } else {
        let ctx = Context::new_with_parent(runtime, parent).resolve_native(name.as_ref(), typ)?;
        let native = runtime.heap.with_access(|heap| match heap.get(&ctx)? {
            Cell::Native(native) if native.arity == 0 && native.applied.is_empty() => {
                Ok(Some(native.clone()))
            }
            _ => Ok(None),
        })?;
        if let Some(native) = native {
            match native.call_zero_at_site(runtime, CallSite::child(parent))? {
                NativeCallResult::Ready(ctx) => Ok(EvalVarResult::Value(ctx)),
                NativeCallResult::Pending(future) => Ok(EvalVarResult::AwaitNative(future)),
            }
        } else {
            Ok(EvalVarResult::Value(ctx))
        }
    }
}

fn apply_record_update_values<State>(
    runtime: &RuntimeCore<State>,
    base_ptr: Pointer,
    update_vals: BTreeMap<Symbol, Pointer>,
) -> Result<Pointer, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    enum RecordUpdateTarget {
        Dict(BTreeMap<Symbol, Pointer>),
        Adt(Symbol, BTreeMap<Symbol, Pointer>),
    }

    let target = runtime.heap.with_access(|heap| {
        let base_val = heap.get(&base_ptr)?;
        match base_val {
            Cell::Dict(map) => Ok(RecordUpdateTarget::Dict(map.clone())),
            Cell::Adt(tag, args) if args.len() == 1 => {
                let inner = heap.get(&args[0])?;
                match inner {
                    Cell::Dict(map) => Ok(RecordUpdateTarget::Adt(tag.clone(), map.clone())),
                    _ => Err(EngineError::UnsupportedExpr),
                }
            }
            _ => Err(EngineError::UnsupportedExpr),
        }
    })?;

    match target {
        RecordUpdateTarget::Dict(mut map) => {
            for (key, value) in update_vals {
                map.insert(key, value);
            }
            runtime.heap.alloc_ptr_dict(map)
        }
        RecordUpdateTarget::Adt(tag, mut map) => {
            for (key, value) in update_vals {
                map.insert(key, value);
            }
            let dict = runtime.heap.alloc_ptr_dict(map)?;
            runtime.heap.alloc_ptr_adt(tag, vec![dict])
        }
    }
}

fn is_root_frame_parent(heap: &Heap, pointer: &Pointer) -> Result<bool, EngineError> {
    heap.with_access(|heap| match heap.get(pointer)? {
        Cell::U64(0) => Ok(true),
        Cell::Frame(_) => Ok(false),
        other => Err(EngineError::Internal(format!(
            "unexpected frame parent value {}",
            other.cell_type_name()
        ))),
    })
}

pub(crate) fn frame_kind_error<T>(expected: &'static str) -> Result<T, EngineError> {
    Err(EngineError::Internal(format!(
        "frame does not match typed expression kind `{expected}`"
    )))
}

pub(crate) fn unexpected_child_result<T>(frame: &'static str) -> Result<T, EngineError> {
    Err(EngineError::Internal(format!(
        "{frame} frame received an unexpected child result"
    )))
}

fn match_pattern_ptr(
    heap: &Heap,
    pat: &Pattern,
    value: &Pointer,
) -> Option<BTreeMap<Symbol, Pointer>> {
    heap.with_access(|heap| Ok(match_pattern_ptr_with_access(heap, pat, value)))
        .ok()
        .flatten()
}

fn match_pattern_ptr_with_access(
    heap: &HeapAccess<'_>,
    pat: &Pattern,
    value: &Pointer,
) -> Option<BTreeMap<Symbol, Pointer>> {
    match pat {
        Pattern::Wildcard(..) => Some(BTreeMap::new()),
        Pattern::Var(var) => {
            let mut bindings = BTreeMap::new();
            bindings.insert(var.name.clone(), *value);
            Some(bindings)
        }
        Pattern::Named(_, name, ps) => {
            let v = heap.get(value).ok()?;
            match v {
                Cell::Adt(vname, args)
                    if runtime_ctor_matches(vname, &name.to_dotted_symbol())
                        && args.len() == ps.len() =>
                {
                    match_patterns_with_access(heap, ps, args)
                }
                _ => None,
            }
        }
        Pattern::Tuple(_, ps) => {
            let v = heap.get(value).ok()?;
            match v {
                Cell::Tuple(xs) if xs.len() == ps.len() => match_patterns_with_access(heap, ps, xs),
                _ => None,
            }
        }
        Pattern::List(_, ps) => {
            let v = heap.get(value).ok()?;
            let values = list_to_vec(heap, v).ok()?;
            if values.len() == ps.len() {
                match_patterns_with_access(heap, ps, &values)
            } else {
                None
            }
        }
        Pattern::Cons(_, head, tail) => {
            let v = heap.get(value).ok()?;
            match v {
                Cell::Adt(tag, args) if tag.as_ref() == "Cons" && args.len() == 2 => {
                    let mut left = match_pattern_ptr_with_access(heap, head, &args[0])?;
                    let right = match_pattern_ptr_with_access(heap, tail, &args[1])?;
                    left.extend(right);
                    Some(left)
                }
                _ => None,
            }
        }
        Pattern::Dict(_, fields) => {
            let v = heap.get(value).ok()?;
            match v {
                Cell::Dict(map) => {
                    let mut bindings = BTreeMap::new();
                    for (key, pat) in fields {
                        let v = map.get(key)?;
                        let sub = match_pattern_ptr_with_access(heap, pat, v)?;
                        bindings.extend(sub);
                    }
                    Some(bindings)
                }
                _ => None,
            }
        }
    }
}

fn match_patterns_with_access(
    heap: &HeapAccess<'_>,
    patterns: &[Pattern],
    values: &[Pointer],
) -> Option<BTreeMap<Symbol, Pointer>> {
    let mut bindings = BTreeMap::new();
    for (p, v) in patterns.iter().zip(values.iter()) {
        let sub = match_pattern_ptr_with_access(heap, p, v)?;
        bindings.extend(sub);
    }
    Some(bindings)
}

fn runtime_ctor_matches(actual: &Symbol, expected: &Symbol) -> bool {
    actual
        .as_ref()
        .rsplit('.')
        .next()
        .unwrap_or(actual.as_ref())
        == expected
            .as_ref()
            .rsplit('.')
            .next()
            .unwrap_or(expected.as_ref())
}

fn alloc_uint_literal_as<State: Clone + Send + Sync + 'static>(
    engine: &RuntimeCore<State>,
    value: u64,
    typ: &Type,
) -> Result<Pointer, EngineError> {
    match typ.as_ref() {
        TypeKind::Var(_) => engine.heap.alloc_ptr_i32(i32::try_from(value).map_err(|_| {
            EngineError::NativeType {
                expected: "i32".into(),
                got: value.to_string(),
            }
        })?),
        TypeKind::Con(tc) => match tc.builtin_id() {
            Some(BuiltinTypeId::U8) => {
                engine.heap.alloc_ptr_u8(u8::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "u8".into(),
                        got: value.to_string(),
                    }
                })?)
            }
            Some(BuiltinTypeId::U16) => {
                engine.heap.alloc_ptr_u16(u16::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "u16".into(),
                        got: value.to_string(),
                    }
                })?)
            }
            Some(BuiltinTypeId::U32) => {
                engine.heap.alloc_ptr_u32(u32::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "u32".into(),
                        got: value.to_string(),
                    }
                })?)
            }
            Some(BuiltinTypeId::U64) => engine.heap.alloc_ptr_u64(value),
            Some(BuiltinTypeId::I8) => {
                engine.heap.alloc_ptr_i8(i8::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "i8".into(),
                        got: value.to_string(),
                    }
                })?)
            }
            Some(BuiltinTypeId::I16) => {
                engine.heap.alloc_ptr_i16(i16::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "i16".into(),
                        got: value.to_string(),
                    }
                })?)
            }
            Some(BuiltinTypeId::I32) => {
                engine.heap.alloc_ptr_i32(i32::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "i32".into(),
                        got: value.to_string(),
                    }
                })?)
            }
            Some(BuiltinTypeId::I64) => {
                engine.heap.alloc_ptr_i64(i64::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "i64".into(),
                        got: value.to_string(),
                    }
                })?)
            }
            _ => Err(EngineError::NativeType {
                expected: "integral".into(),
                got: typ.to_string(),
            }),
        },
        _ => Err(EngineError::NativeType {
            expected: "integral".into(),
            got: typ.to_string(),
        }),
    }
}

fn alloc_int_literal_as<State: Clone + Send + Sync + 'static>(
    engine: &RuntimeCore<State>,
    value: i64,
    typ: &Type,
) -> Result<Pointer, EngineError> {
    match typ.as_ref() {
        TypeKind::Var(_) => engine.heap.alloc_ptr_i32(i32::try_from(value).map_err(|_| {
            EngineError::NativeType {
                expected: "i32".into(),
                got: value.to_string(),
            }
        })?),
        TypeKind::Con(tc) => match tc.builtin_id() {
            Some(BuiltinTypeId::I8) => {
                engine.heap.alloc_ptr_i8(i8::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "i8".into(),
                        got: value.to_string(),
                    }
                })?)
            }
            Some(BuiltinTypeId::I16) => {
                engine.heap.alloc_ptr_i16(i16::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "i16".into(),
                        got: value.to_string(),
                    }
                })?)
            }
            Some(BuiltinTypeId::I32) => {
                engine.heap.alloc_ptr_i32(i32::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "i32".into(),
                        got: value.to_string(),
                    }
                })?)
            }
            Some(BuiltinTypeId::I64) => engine.heap.alloc_ptr_i64(value),
            Some(BuiltinTypeId::U8) => {
                engine.heap.alloc_ptr_u8(u8::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "u8".into(),
                        got: value.to_string(),
                    }
                })?)
            }
            Some(BuiltinTypeId::U16) => {
                engine.heap.alloc_ptr_u16(u16::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "u16".into(),
                        got: value.to_string(),
                    }
                })?)
            }
            Some(BuiltinTypeId::U32) => {
                engine.heap.alloc_ptr_u32(u32::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "u32".into(),
                        got: value.to_string(),
                    }
                })?)
            }
            Some(BuiltinTypeId::U64) => {
                engine.heap.alloc_ptr_u64(u64::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "u64".into(),
                        got: value.to_string(),
                    }
                })?)
            }
            _ => Err(EngineError::NativeType {
                expected: "integral".into(),
                got: typ.to_string(),
            }),
        },
        _ => Err(EngineError::NativeType {
            expected: "integral".into(),
            got: typ.to_string(),
        }),
    }
}

fn alloc_float_literal_as<State: Clone + Send + Sync + 'static>(
    engine: &RuntimeCore<State>,
    value: f64,
    typ: &Type,
) -> Result<Pointer, EngineError> {
    match typ.as_ref() {
        TypeKind::Var(_) => engine.heap.alloc_ptr_f32(value as f32),
        TypeKind::Con(tc) => match tc.builtin_id() {
            Some(BuiltinTypeId::F32) => engine.heap.alloc_ptr_f32(value as f32),
            Some(BuiltinTypeId::F64) => engine.heap.alloc_ptr_f64(value),
            _ => Err(EngineError::NativeType {
                expected: "f32 or f64".into(),
                got: typ.to_string(),
            }),
        },
        _ => Err(EngineError::NativeType {
            expected: "f32 or f64".into(),
            got: typ.to_string(),
        }),
    }
}

enum EvalApplyResult<State: Clone + Send + Sync + 'static> {
    Value(Pointer),
    Push {
        expr: Arc<TypedExpr>,
        env: Environment,
    },
    PushNative(NativeTask),
    AwaitNative(NativeAsyncCall<State>),
}

enum EvalVarResult<State: Clone + Send + Sync + 'static> {
    Value(Pointer),
    Push {
        expr: Arc<TypedExpr>,
        env: Environment,
    },
    AwaitNative(NativeAsyncCall<State>),
}

fn project_pointer(heap: &Heap, field: &Symbol, pointer: &Pointer) -> Result<Pointer, EngineError> {
    heap.with_access(|heap| project_pointer_with_access(heap, field, pointer))
}

fn project_pointer_with_access(
    heap: &HeapAccess<'_>,
    field: &Symbol,
    pointer: &Pointer,
) -> Result<Pointer, EngineError> {
    let value = heap.get(pointer)?;
    if let Ok(index) = field.as_ref().parse::<usize>() {
        return match value {
            Cell::Tuple(items) => {
                items
                    .get(index)
                    .cloned()
                    .ok_or_else(|| EngineError::UnknownField {
                        field: field.clone(),
                        value: "tuple".into(),
                    })
            }
            _ => Err(EngineError::UnknownField {
                field: field.clone(),
                value: heap.type_name(pointer)?.into(),
            }),
        };
    }
    match value {
        Cell::Adt(_, args) if args.len() == 1 => {
            let inner = heap.get(&args[0])?;
            match inner {
                Cell::Dict(map) => {
                    map.get(field)
                        .cloned()
                        .ok_or_else(|| EngineError::UnknownField {
                            field: field.clone(),
                            value: "record".into(),
                        })
                }
                _ => Err(EngineError::UnknownField {
                    field: field.clone(),
                    value: heap.type_name(&args[0])?.into(),
                }),
            }
        }
        _ => Err(EngineError::UnknownField {
            field: field.clone(),
            value: heap.type_name(pointer)?.into(),
        }),
    }
}

fn synthetic_application_expr_from_head(
    mut env: Environment,
    head: TypedExpr,
    args: &[(Pointer, Type)],
) -> Result<(Environment, TypedExpr), EngineError> {
    let mut expr = head;
    let mut cur_type = expr.typ.clone();

    for (idx, (arg, arg_type)) in args.iter().enumerate() {
        let arg_name = Symbol::intern(&format!("__rex_apply_arg_{idx}"));
        env = env.extend(arg_name.clone(), *arg);
        let arg_expr = TypedExpr::new(
            arg_type.clone(),
            TypedExprKind::Var {
                name: arg_name,
                overloads: Vec::new(),
            },
        );
        let result_type = application_result_type(&cur_type, arg_type)?;
        expr = TypedExpr::new(
            result_type.clone(),
            TypedExprKind::App(Arc::new(expr), Arc::new(arg_expr)),
        );
        cur_type = result_type;
    }

    Ok((env, expr))
}

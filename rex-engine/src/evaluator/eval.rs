use crate::{
    env::{Environment, RootedEnvironment},
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
    memory::{
        heap::{Cell, Closure, Handle, Heap, HeapState, Pointer, Reference, TempRoots},
        lists::ListItems,
        traits::Collection,
    },
    native_fn::NativeApplyResult,
    overloaded_fn::OverloadedFn,
    stack::{
        FrApp, FrAppArg, FrAppState, FrBool, FrBranchState, FrDateTime, FrDict, FrFloat, FrHole,
        FrInt, FrIte, FrLam, FrLet, FrLetRec, FrLetRecState, FrLetState, FrList, FrMatch,
        FrMatchArm, FrMatchState, FrNativeAsync, FrNativeCall, FrNativeCallState, FrProject,
        FrRecordUpdate, FrRecordUpdateState, FrSequenceState, FrString, FrTuple, FrUint, FrUuid,
        FrValueState, FrVar, Frame, FrameId, FrameStore,
    },
    util::{is_function_type, split_fun},
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
    Schedule(Vec<FrameId>),
    Wait,
    AwaitNative(NativeAsyncCall<State>),
    Return(Pointer),
}

pub(crate) async fn eval_typed_expr<State>(
    runtime: RuntimeCore<State>,
    rooted_env: RootedEnvironment,
    expr: Arc<TypedExpr>,
    input_args: Vec<(Handle, Type)>,
) -> Result<Handle, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let env = rooted_env.to_environment()?;
    let (env, expr) = if input_args.is_empty() {
        (env, expr)
    } else {
        let args = input_args
            .iter()
            .map(|(handle, typ)| Ok((handle.pointer_for_heap(&runtime.heap)?, typ.clone())))
            .collect::<Result<Vec<_>, EngineError>>()?;
        let (env, expr) = synthetic_application_expr_from_head(env, expr.as_ref().clone(), &args)?;
        (env, Arc::new(expr))
    };
    // rooted_env and input_args intentionally remain in scope across this
    // await; their handles root the raw pointers stored in env and the
    // synthetic input application.
    eval_typed_expr_inner(runtime, env, expr).await
}

async fn eval_typed_expr_inner<State>(
    mut runtime: RuntimeCore<State>,
    env: Environment,
    expr: Arc<TypedExpr>,
) -> Result<Handle, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let mut frames = FrameStore::default();
    let root_frame = frames.insert(frame_for_expr(None, expr, env));
    let mut scheduler = EvalScheduler::new(root_frame, runtime.parallelism_controller.clone());

    let mut iteration = 0;
    loop {
        if poll_pending_native(&mut runtime, &mut frames, &mut scheduler, false).await? {
            continue;
        }

        if iteration % 1000 == 0 {
            println!("Iteration {}", iteration);
        }
        iteration += 1;

        let mut item = match scheduler.pop_next() {
            Some(item) => item,
            None => {
                if poll_pending_native(&mut runtime, &mut frames, &mut scheduler, true).await? {
                    continue;
                }
                return Err(EngineError::Internal(
                    "eval scheduler ran out of ready work".into(),
                ));
            }
        };
        let mut protected = Vec::new();
        frames.trace_pointers(&mut protected);
        item.trace_pointers(&mut protected);
        scheduler.trace_pointers(&mut protected);
        runtime.trace_pointers(&mut protected)?;
        let roots = runtime.heap.temp_roots(protected.clone())?;
        refresh_eval_roots(
            &mut runtime,
            &mut frames,
            &mut item,
            &mut scheduler,
            &roots,
            &protected,
        )?;

        let frame = frames.get(item.frame)?.clone();
        let control = match item.returned {
            Some(returned) => eval_receive(
                &runtime,
                &mut frames,
                item.frame,
                frame,
                returned.child,
                returned.value,
            )?,
            None => eval_enter(&runtime, &mut frames, item.frame, frame)?,
        };
        refresh_eval_roots(
            &mut runtime,
            &mut frames,
            &mut item,
            &mut scheduler,
            &roots,
            &protected,
        )?;

        match control {
            EvalControl::Push { expr, env } => {
                let frame = frame_for_expr(Some(item.frame), expr, env);
                let child = frames.insert(frame);
                scheduler.schedule_next(EvalWorkItem::enter(child));
            }
            EvalControl::PushFrame(frame) => {
                let child = frames.insert(*frame);
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
                let frame = Frame::NativeAsync(FrNativeAsync {
                    parent: Some(item.frame),
                });
                let child = frames.insert(frame);
                if call_roots.has_collected_since_creation()? {
                    let mut cursor = 0;
                    call.refresh_from_roots(&call_roots, &mut cursor)?;
                }
                scheduler.schedule_pending_native(child, call);
            }
            EvalControl::Return(value) => {
                let frame = frames.remove(item.frame)?;
                let parent = frame.parent();
                let Some(parent) = parent else {
                    return runtime.heap.handle(value);
                };
                scheduler.schedule_next(EvalWorkItem::receive(parent, item.frame, value));
            }
        }
    }
}

fn refresh_eval_roots<State>(
    runtime: &mut RuntimeCore<State>,
    frames: &mut FrameStore,
    item: &mut EvalWorkItem,
    scheduler: &mut EvalScheduler<State>,
    roots: &TempRoots,
    originals: &[Pointer],
) -> Result<(), EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    if !roots.has_collected_since_creation()? {
        return Ok(());
    }

    let mut rewrites = HashMap::with_capacity(originals.len());
    for (index, original) in originals.iter().enumerate() {
        rewrites.insert(*original, roots.get(index)?);
    }
    let mut rewrite =
        |pointer| Ok::<_, EngineError>(rewrites.get(&pointer).copied().unwrap_or(pointer));
    frames.map_pointers(&mut rewrite)?;
    item.map_pointers(&mut rewrite)?;
    scheduler.map_pointers(&mut rewrite)?;
    runtime.map_pointers(&mut rewrite)
}

pub(crate) fn frame_for_expr(
    parent: Option<FrameId>,
    expr: Arc<TypedExpr>,
    env: Environment,
) -> Frame {
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
    frames: &mut FrameStore,
    frame_id: FrameId,
    frame: Frame,
) -> Result<EvalControl<State>, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    match frame {
        Frame::Bool(frame) => match frame.expr.kind.as_ref() {
            TypedExprKind::Bool(value) => {
                Ok(EvalControl::Return(runtime.heap.with_locked(|heap| {
                    Ok(heap.alloc_ptr_bool(*value)?.into_pointer())
                })?))
            }
            _ => frame_kind_error("bool"),
        },
        Frame::Uint(frame) => match frame.expr.kind.as_ref() {
            TypedExprKind::Uint(value) => {
                Ok(EvalControl::Return(runtime.heap.with_locked(|heap| {
                    Ok(alloc_uint_literal_as(heap, *value, &frame.expr.typ)?.into_pointer())
                })?))
            }
            _ => frame_kind_error("uint"),
        },
        Frame::Int(frame) => match frame.expr.kind.as_ref() {
            TypedExprKind::Int(value) => {
                Ok(EvalControl::Return(runtime.heap.with_locked(|heap| {
                    Ok(alloc_int_literal_as(heap, *value, &frame.expr.typ)?.into_pointer())
                })?))
            }
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
            TypedExprKind::String(value) => {
                Ok(EvalControl::Return(runtime.heap.with_locked(|heap| {
                    Ok(heap.alloc_ptr_string(value.clone())?.into_pointer())
                })?))
            }
            _ => frame_kind_error("string"),
        },
        Frame::Uuid(frame) => match frame.expr.kind.as_ref() {
            TypedExprKind::Uuid(value) => {
                Ok(EvalControl::Return(runtime.heap.with_locked(|heap| {
                    Ok(heap.alloc_ptr_uuid(*value)?.into_pointer())
                })?))
            }
            _ => frame_kind_error("uuid"),
        },
        Frame::DateTime(frame) => match frame.expr.kind.as_ref() {
            TypedExprKind::DateTime(value) => {
                Ok(EvalControl::Return(runtime.heap.with_locked(|heap| {
                    Ok(heap.alloc_ptr_datetime(*value)?.into_pointer())
                })?))
            }
            _ => frame_kind_error("datetime"),
        },
        Frame::Hole(_) => Err(EngineError::UnsupportedExpr),
        Frame::Tuple(frame) => eval_tuple_enter(runtime, frames, frame_id, frame),
        Frame::List(frame) => eval_list_enter(runtime, frames, frame_id, frame),
        Frame::Dict(frame) => eval_dict_enter(runtime, frames, frame_id, frame),
        Frame::RecordUpdate(mut frame) => {
            let base = match frame.expr.kind.as_ref() {
                TypedExprKind::RecordUpdate { base, .. } => Arc::clone(base),
                _ => return frame_kind_error("record update"),
            };
            frame.state = FrRecordUpdateState::EvalBase;
            let env = frame.env.clone();
            frames.replace(frame_id, Frame::RecordUpdate(frame))?;
            Ok(EvalControl::Push { expr: base, env })
        }
        Frame::Var(mut frame) => match frame.expr.kind.as_ref() {
            TypedExprKind::Var { name, .. } => {
                match eval_resolve_var(runtime, frame_id, &frame.env, name, &frame.expr.typ)? {
                    EvalVarResult::Value(value) => Ok(EvalControl::Return(value)),
                    EvalVarResult::Push { expr, env } => {
                        frame.state = FrValueState::Enter;
                        frames.replace(frame_id, Frame::Var(frame))?;
                        Ok(EvalControl::Push { expr, env })
                    }
                    EvalVarResult::AwaitNative(future) => {
                        frame.state = FrValueState::Enter;
                        frames.replace(frame_id, Frame::Var(frame))?;
                        Ok(EvalControl::AwaitNative(future))
                    }
                }
            }
            _ => frame_kind_error("var"),
        },
        Frame::App(frame) => eval_app_enter(runtime, frames, frame_id, frame),
        Frame::Project(mut frame) => {
            let expr = match frame.expr.kind.as_ref() {
                TypedExprKind::Project { expr, .. } => Arc::clone(expr),
                _ => return frame_kind_error("project"),
            };
            frame.state = FrValueState::Enter;
            let env = frame.env.clone();
            frames.replace(frame_id, Frame::Project(frame))?;
            Ok(EvalControl::Push { expr, env })
        }
        Frame::Lam(frame) => match frame.expr.kind.as_ref() {
            TypedExprKind::Lam { param, body } => {
                let param_ty = split_fun(&frame.expr.typ)
                    .map(|(arg, _)| arg)
                    .ok_or_else(|| EngineError::NotCallable(frame.expr.typ.to_string()))?;
                let value = runtime.heap.with_locked(|heap| {
                    Ok(heap
                        .alloc_ptr_closure(
                            frame.env.clone(),
                            param.clone(),
                            param_ty,
                            frame.expr.typ.clone(),
                            Arc::clone(body),
                        )?
                        .into_pointer())
                })?;
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
            frames.replace(frame_id, Frame::Let(frame))?;
            Ok(EvalControl::Push { expr: def, env })
        }
        Frame::LetRec(frame) => {
            let TypedExprKind::LetRec { bindings, body } = frame.expr.kind.as_ref() else {
                return frame_kind_error("let rec");
            };
            let bindings = bindings.clone();
            let body = Arc::clone(body);
            let mut protected = Vec::new();
            Frame::LetRec(frame.clone()).trace_pointers(&mut protected);
            let frame_roots = runtime.heap.temp_roots(protected.clone())?;
            let mut slot_roots = Vec::with_capacity(bindings.len());
            for (name, _) in &bindings {
                let placeholder = runtime.heap.with_locked(|heap| {
                    Ok(heap.alloc_ptr_uninitialized(name.clone())?.into_pointer())
                })?;
                slot_roots.push(runtime.heap.temp_roots(vec![placeholder])?);
            }
            let mut wrapped = Frame::LetRec(frame);
            refresh_frame_from_roots(&mut wrapped, &protected, &frame_roots, 0)?;
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
                frames.replace(frame_id, Frame::LetRec(frame))?;
                return Ok(EvalControl::Push {
                    expr: body,
                    env: recursive_env,
                });
            }
            frame.state = FrLetRecState::EvalBinding;
            let def = Arc::clone(&bindings[0].1);
            frames.replace(frame_id, Frame::LetRec(frame))?;
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
            frames.replace(frame_id, Frame::Ite(frame))?;
            Ok(EvalControl::Push { expr: cond, env })
        }
        Frame::Match(mut frame) => {
            let scrutinee = match frame.expr.kind.as_ref() {
                TypedExprKind::Match { scrutinee, .. } => Arc::clone(scrutinee),
                _ => return frame_kind_error("match"),
            };
            frame.state = FrMatchState::EvalScrutinee;
            let env = frame.env.clone();
            frames.replace(frame_id, Frame::Match(frame))?;
            Ok(EvalControl::Push {
                expr: scrutinee,
                env,
            })
        }
        Frame::NativeCall(frame) => eval_native_enter(runtime, frames, frame_id, frame),
        Frame::NativeAsync(_) => unexpected_child_result("native async"),
    }
}

fn eval_tuple_enter<State>(
    runtime: &RuntimeCore<State>,
    frames: &mut FrameStore,
    frame_id: FrameId,
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
        return Ok(EvalControl::Return(runtime.heap.with_locked(|heap| {
            Ok(heap.alloc_ptr_tuple(vec![])?.into_pointer())
        })?));
    }

    frame.state = FrSequenceState::EvalItem;
    frame.children = Vec::with_capacity(elems.len());
    frame.values = vec![None; elems.len()];
    frame.remaining = elems.len();
    let env = frame.env.clone();
    for expr in elems {
        let child = frames.insert(frame_for_expr(Some(frame_id), expr, env.clone()));
        frame.children.push(child);
    }
    let children = frame.children.clone();
    frames.replace(frame_id, Frame::Tuple(frame))?;
    Ok(EvalControl::Schedule(children))
}

fn eval_list_enter<State>(
    runtime: &RuntimeCore<State>,
    frames: &mut FrameStore,
    frame_id: FrameId,
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
        let ptr = runtime
            .heap
            .with_locked(|heap| Ok(heap.alloc_ptr_empty()?.into_pointer()))?;
        return Ok(EvalControl::Return(ptr));
    }

    frame.state = FrSequenceState::EvalItem;
    frame.children = Vec::with_capacity(elems.len());
    frame.values = vec![None; elems.len()];
    frame.remaining = elems.len();
    let env = frame.env.clone();
    for expr in elems {
        let child = frames.insert(frame_for_expr(Some(frame_id), expr, env.clone()));
        frame.children.push(child);
    }
    let children = frame.children.clone();
    frames.replace(frame_id, Frame::List(frame))?;
    Ok(EvalControl::Schedule(children))
}

fn eval_dict_enter<State>(
    runtime: &RuntimeCore<State>,
    frames: &mut FrameStore,
    frame_id: FrameId,
    mut frame: FrDict,
) -> Result<EvalControl<State>, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let exprs = dict_exprs_for_keys(&frame, &frame.keys)?;
    if exprs.is_empty() {
        return Ok(EvalControl::Return(runtime.heap.with_locked(|heap| {
            Ok(heap.alloc_ptr_dict(BTreeMap::new())?.into_pointer())
        })?));
    }

    frame.state = FrSequenceState::EvalItem;
    frame.children = Vec::with_capacity(exprs.len());
    frame.values = vec![None; exprs.len()];
    frame.remaining = exprs.len();
    let env = frame.env.clone();
    for expr in exprs {
        let child = frames.insert(frame_for_expr(Some(frame_id), expr, env.clone()));
        frame.children.push(child);
    }
    let children = frame.children.clone();
    frames.replace(frame_id, Frame::Dict(frame))?;
    Ok(EvalControl::Schedule(children))
}

fn eval_record_update_updates_enter<State>(
    _runtime: &RuntimeCore<State>,
    frames: &mut FrameStore,
    frame_id: FrameId,
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
    let env = frame.env.clone();
    for expr in exprs {
        let child = frames.insert(frame_for_expr(Some(frame_id), expr, env.clone()));
        frame.update_children.push(child);
    }
    let children = frame.update_children.clone();
    frames.replace(frame_id, Frame::RecordUpdate(frame))?;
    Ok(EvalControl::Schedule(children))
}

fn eval_app_enter<State>(
    _runtime: &RuntimeCore<State>,
    frames: &mut FrameStore,
    frame_id: FrameId,
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
    let env = frame.env.clone();
    let head_child = frames.insert(frame_for_expr(Some(frame_id), head, env.clone()));
    frame.head_child = Some(head_child);
    for expr in arg_exprs {
        let child = frames.insert(frame_for_expr(Some(frame_id), expr, env.clone()));
        frame.arg_children.push(child);
    }

    let mut children = Vec::with_capacity(1 + frame.arg_children.len());
    children.push(
        frame
            .head_child
            .ok_or_else(|| EngineError::Internal("application frame missing head child".into()))?,
    );
    children.extend(frame.arg_children.iter().copied());
    frames.replace(frame_id, Frame::App(frame))?;
    Ok(EvalControl::Schedule(children))
}

fn receive_sequence_value(
    kind: &'static str,
    children: &[FrameId],
    values: &mut [Option<Pointer>],
    remaining: &mut usize,
    child: FrameId,
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
    child: FrameId,
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
    frames: &mut FrameStore,
    frame_id: FrameId,
    frame: Frame,
    child: FrameId,
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
                return Ok(EvalControl::Return(runtime.heap.with_locked(|heap| {
                    Ok(heap.alloc_ptr_tuple(values)?.into_pointer())
                })?));
            }
            frames.replace(frame_id, Frame::Tuple(frame))?;
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
            frames.replace(frame_id, Frame::List(frame))?;
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
                return Ok(EvalControl::Return(runtime.heap.with_locked(|heap| {
                    Ok(heap.alloc_ptr_dict(values)?.into_pointer())
                })?));
            }
            frames.replace(frame_id, Frame::Dict(frame))?;
            Ok(EvalControl::Wait)
        }
        Frame::RecordUpdate(mut frame) => match frame.state {
            FrRecordUpdateState::EvalBase => {
                frame.base_value = Some(value);
                if frame.update_keys.is_empty() {
                    let result = apply_record_update_values(runtime, value, BTreeMap::new())?;
                    return Ok(EvalControl::Return(result));
                }
                eval_record_update_updates_enter(runtime, frames, frame_id, frame)
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
                frames.replace(frame_id, Frame::RecordUpdate(frame))?;
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
                    return continue_app_after_apply(runtime, frames, frame_id, frame, None);
                }
                frames.replace(frame_id, Frame::App(frame))?;
                Ok(EvalControl::Wait)
            }
            FrAppState::ApplyArg => {
                continue_app_after_apply(runtime, frames, frame_id, frame, Some(value))
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
                frames.replace(frame_id, Frame::Let(frame))?;
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
                runtime
                    .heap
                    .with_locked(|heap| heap.overwrite(&slot, cell))?;
                frame.binding_value = Some(value);
                frame.next_binding_index += 1;
                let recursive_env = frame.recursive_env.clone().ok_or_else(|| {
                    EngineError::Internal("let rec frame missing recursive environment".into())
                })?;
                if frame.next_binding_index == bindings.len() {
                    frame.state = FrLetRecState::EvalBody;
                    let body = Arc::clone(body);
                    frames.replace(frame_id, Frame::LetRec(frame))?;
                    return Ok(EvalControl::Push {
                        expr: body,
                        env: recursive_env,
                    });
                }
                let def = Arc::clone(&bindings[frame.next_binding_index].1);
                frames.replace(frame_id, Frame::LetRec(frame))?;
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
                let selected = match runtime
                    .heap
                    .with_locked(|heap| heap.pointer_as_bool(&value))
                {
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
                frames.replace(frame_id, Frame::Ite(frame))?;
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
                let mut protected = Vec::new();
                Frame::Match(frame.clone()).trace_pointers(&mut protected);
                let frame_roots = runtime.heap.temp_roots(protected.clone())?;
                loop {
                    let mut wrapped = Frame::Match(frame);
                    refresh_frame_from_roots(&mut wrapped, &protected, &frame_roots, 0)?;
                    frame = match wrapped {
                        Frame::Match(frame) => frame,
                        _ => return frame_kind_error("match"),
                    };
                    let value = frame.scrutinee_value.ok_or_else(|| {
                        EngineError::Internal("match frame missing scrutinee".into())
                    })?;
                    if frame.next_arm_index >= frame.arms.len() {
                        return Err(EngineError::MatchFailure);
                    }
                    let idx = frame.next_arm_index;
                    let arm = &frame.arms[idx];
                    let matched = match_pattern_ptr(&runtime.heap, &arm.pattern, &value)?;
                    let mut wrapped = Frame::Match(frame);
                    refresh_frame_from_roots(&mut wrapped, &protected, &frame_roots, 0)?;
                    frame = match wrapped {
                        Frame::Match(frame) => frame,
                        _ => return frame_kind_error("match"),
                    };
                    if let Some(bindings) = matched {
                        let env = frame.env.extend_many(bindings);
                        let expr = Arc::clone(&frame.arms[idx].expr);
                        frame.next_arm_index = idx;
                        frame.matched_env = Some(env.clone());
                        frame.state = FrMatchState::EvalArm;
                        frames.replace(frame_id, Frame::Match(frame))?;
                        return Ok(EvalControl::Push { expr, env });
                    }
                    frame.next_arm_index += 1;
                }
            }
            FrMatchState::EvalArm => Ok(EvalControl::Return(value)),
            _ => unexpected_child_result("match"),
        },
        Frame::NativeCall(frame) => {
            eval_native_receive(runtime, frames, frame_id, frame, child, value)
        }
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
    frame.map_pointers(&mut |pointer| Ok(rewrites.get(&pointer).copied().unwrap_or(pointer)))
}

fn eval_apply_overloaded_arg<State>(
    runtime: &RuntimeCore<State>,
    parent: FrameId,
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
        return Ok(EvalApplyResult::Value(runtime.heap.with_locked(
            |heap| {
                Ok(heap
                    .alloc_ptr_overloaded(over.name, rest_ty, over.applied, over.applied_types)?
                    .into_pointer())
            },
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
    parent: FrameId,
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
            runtime
                .heap
                .with_locked(|heap| heap.type_name(&func))?
                .into(),
        )),
    }
}

fn continue_app_after_apply<State>(
    runtime: &RuntimeCore<State>,
    frames: &mut FrameStore,
    frame_id: FrameId,
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
        frames.replace(frame_id, Frame::App(frame.clone()))?;
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
        frames.replace(frame_id, Frame::App(frame.clone()))?;
        let mut protected = vec![func, arg];
        Frame::App(frame.clone()).trace_pointers(&mut protected);
        let roots = runtime.heap.temp_roots(protected.clone())?;
        let apply_result = eval_apply_arg(
            runtime,
            frame_id,
            func,
            arg,
            Some(&arg_info.func_type),
            Some(&arg_info.expr.typ),
        )?;
        let mut wrapped = Frame::App(frame);
        refresh_frame_from_roots(&mut wrapped, &protected, &roots, 0)?;
        frame = match wrapped {
            Frame::App(frame) => frame,
            _ => return frame_kind_error("application"),
        };
        match apply_result {
            EvalApplyResult::Value(applied) => {
                frame.arg = None;
                frame.func = Some(applied);
                frame.next_arg_index += 1;
                frames.replace(frame_id, Frame::App(frame.clone()))?;
            }
            EvalApplyResult::Push { expr, env } => return Ok(EvalControl::Push { expr, env }),
            EvalApplyResult::PushNative(task) => {
                return Ok(EvalControl::PushFrame(Box::new(Frame::NativeCall(
                    FrNativeCall {
                        parent: Some(frame_id),
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
    parent: FrameId,
    env: &Environment,
    name: &Symbol,
    typ: &Type,
) -> Result<EvalVarResult<State>, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    if let Some(ptr) = env.get(name) {
        let native =
            runtime
                .heap
                .with_locked(|heap| match heap.get_cell_from_pointer(&ptr)? {
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
        let native =
            runtime
                .heap
                .with_locked(|heap| match heap.get_cell_from_pointer(&ctx)? {
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

    let target = runtime.heap.with_locked(|heap| {
        let base_val = heap.get_cell_from_pointer(&base_ptr)?;
        match base_val {
            Cell::Dict(map) => Ok(RecordUpdateTarget::Dict(map.clone())),
            Cell::Adt(tag, args) if args.len() == 1 => {
                let inner = heap.get_cell_from_pointer(&args[0])?;
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
            runtime
                .heap
                .with_locked(|heap| Ok(heap.alloc_ptr_dict(map)?.into_pointer()))
        }
        RecordUpdateTarget::Adt(tag, mut map) => {
            for (key, value) in update_vals {
                map.insert(key, value);
            }
            let dict = runtime
                .heap
                .with_locked(|heap| Ok(heap.alloc_ptr_dict(map)?.into_pointer()))?;
            runtime
                .heap
                .with_locked(|heap| Ok(heap.alloc_ptr_adt(tag, vec![dict])?.into_pointer()))
        }
    }
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
) -> Result<Option<BTreeMap<Symbol, Pointer>>, EngineError> {
    match pat {
        Pattern::Wildcard(..) => Ok(Some(BTreeMap::new())),
        Pattern::Var(var) => {
            let mut bindings = BTreeMap::new();
            bindings.insert(var.name.clone(), *value);
            Ok(Some(bindings))
        }
        Pattern::Named(_, name, ps) => {
            let expected = name.to_dotted_symbol();
            let (args, is_list) = heap.with_locked(|heap| {
                let v = heap.get_cell_from_pointer(value)?;
                match v {
                    Cell::Adt(vname, args)
                        if runtime_ctor_matches(vname, &expected) && args.len() == ps.len() =>
                    {
                        Ok((Some(args.clone()), false))
                    }
                    Cell::Empty | Cell::Cons(..) | Cell::ListSlice { .. } => Ok((None, true)),
                    _ => Ok((None, false)),
                }
            })?;
            if let Some(args) = args {
                return match_patterns(heap, ps, &args);
            }
            if !is_list {
                return Ok(None);
            }

            match expected
                .as_ref()
                .rsplit('.')
                .next()
                .unwrap_or(expected.as_ref())
            {
                "Empty" if ps.is_empty() => Ok((heap.list_len(value)? == 0).then(BTreeMap::new)),
                "Cons" if ps.len() == 2 => {
                    let Some((head, tail)) = heap.list_head_tail(value)? else {
                        return Ok(None);
                    };
                    match_patterns(heap, ps, &[head, tail])
                }
                _ => Ok(None),
            }
        }
        Pattern::Tuple(_, ps) => {
            let Some(xs) = heap.with_locked(|heap| match heap.get_cell_from_pointer(value)? {
                Cell::Tuple(xs) if xs.len() == ps.len() => Ok(Some(xs.clone())),
                _ => Ok(None),
            })?
            else {
                return Ok(None);
            };
            match_patterns(heap, ps, &xs)
        }
        Pattern::List(_, ps) => {
            if heap.list_len(value)? != ps.len() {
                return Ok(None);
            }
            if ps
                .iter()
                .all(|pattern| matches!(pattern, Pattern::Wildcard(..)))
            {
                return Ok(Some(BTreeMap::new()));
            }
            let values = heap.list_items(*value)?;
            match_list_patterns(heap, ps, values)
        }
        Pattern::Cons(_, head, tail) => {
            let Some((head_value, tail_value)) = heap.list_head_tail(value)? else {
                return Ok(None);
            };
            match_patterns(
                heap,
                &[head.as_ref().clone(), tail.as_ref().clone()],
                &[head_value, tail_value],
            )
        }
        Pattern::Dict(_, fields) => {
            let Some(values) = heap.with_locked(|heap| {
                let v = heap.get_cell_from_pointer(value)?;
                let Cell::Dict(map) = v else {
                    return Ok(None);
                };
                let mut values = Vec::with_capacity(fields.len());
                for (key, _) in fields {
                    let Some(pointer) = map.get(key) else {
                        return Ok(None);
                    };
                    values.push(*pointer);
                }
                Ok(Some(values))
            })?
            else {
                return Ok(None);
            };
            let patterns = fields
                .iter()
                .map(|(_, pattern)| pattern.clone())
                .collect::<Vec<_>>();
            match_patterns(heap, &patterns, &values)
        }
    }
}

fn match_list_patterns(
    heap: &Heap,
    patterns: &[Pattern],
    mut values: ListItems,
) -> Result<Option<BTreeMap<Symbol, Pointer>>, EngineError> {
    if patterns.len() != values.len() {
        return Ok(None);
    }

    let mut value_pointers = Vec::new();
    values.trace_pointers(&mut value_pointers);
    let value_roots = heap.temp_roots(value_pointers)?;
    let mut bindings = BTreeMap::new();

    for (index, pattern) in patterns.iter().enumerate() {
        if matches!(pattern, Pattern::Wildcard(..)) {
            continue;
        }

        let binding_roots = heap.temp_roots(bindings.values().copied().collect::<Vec<_>>())?;
        refresh_bindings_from_roots(&mut bindings, &binding_roots)?;
        refresh_list_items_from_roots(&mut values, &value_roots)?;

        let value = heap.with_locked(|heap| values.get(heap, index))?;
        let value_root = heap.temp_roots(vec![value])?;
        let value = value_root.get(0)?;
        let Some(sub) = match_pattern_ptr(heap, pattern, &value)? else {
            return Ok(None);
        };

        refresh_bindings_from_roots(&mut bindings, &binding_roots)?;
        refresh_list_items_from_roots(&mut values, &value_roots)?;
        bindings.extend(sub);
    }
    Ok(Some(bindings))
}

fn match_patterns(
    heap: &Heap,
    patterns: &[Pattern],
    values: &[Pointer],
) -> Result<Option<BTreeMap<Symbol, Pointer>>, EngineError> {
    let value_roots = heap.temp_roots(values.to_vec())?;
    let mut bindings = BTreeMap::new();
    for (index, p) in patterns.iter().enumerate() {
        let binding_roots = heap.temp_roots(bindings.values().copied().collect::<Vec<_>>())?;
        refresh_bindings_from_roots(&mut bindings, &binding_roots)?;
        let value = value_roots.get(index)?;
        let Some(sub) = match_pattern_ptr(heap, p, &value)? else {
            return Ok(None);
        };
        refresh_bindings_from_roots(&mut bindings, &binding_roots)?;
        bindings.extend(sub);
    }
    Ok(Some(bindings))
}

fn refresh_bindings_from_roots(
    bindings: &mut BTreeMap<Symbol, Pointer>,
    roots: &TempRoots,
) -> Result<(), EngineError> {
    for (index, pointer) in bindings.values_mut().enumerate() {
        *pointer = roots.get(index)?;
    }
    Ok(())
}

fn refresh_list_items_from_roots(
    items: &mut ListItems,
    roots: &TempRoots,
) -> Result<(), EngineError> {
    let mut index = 0;
    items.map_pointers(&mut |_| {
        let pointer = roots.get(index);
        index += 1;
        pointer
    })?;
    if index != roots.len() {
        return Err(EngineError::Internal(
            "list item root count does not match pointer count".into(),
        ));
    }
    Ok(())
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

fn alloc_uint_literal_as<'a>(
    heap: &'a mut HeapState,
    value: u64,
    typ: &Type,
) -> Result<Reference<'a>, EngineError> {
    match typ.as_ref() {
        TypeKind::Var(_) => Ok(heap.alloc_ptr_i32(i32::try_from(value).map_err(|_| {
            EngineError::NativeType {
                expected: "i32".into(),
                got: value.to_string(),
            }
        })?)?),
        TypeKind::Con(tc) => match tc.builtin_id() {
            Some(BuiltinTypeId::U8) => {
                Ok(heap.alloc_ptr_u8(u8::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "u8".into(),
                        got: value.to_string(),
                    }
                })?)?)
            }
            Some(BuiltinTypeId::U16) => {
                Ok(heap.alloc_ptr_u16(u16::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "u16".into(),
                        got: value.to_string(),
                    }
                })?)?)
            }
            Some(BuiltinTypeId::U32) => {
                Ok(heap.alloc_ptr_u32(u32::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "u32".into(),
                        got: value.to_string(),
                    }
                })?)?)
            }
            Some(BuiltinTypeId::U64) => Ok(heap.alloc_ptr_u64(value)?),
            Some(BuiltinTypeId::I8) => {
                Ok(heap.alloc_ptr_i8(i8::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "i8".into(),
                        got: value.to_string(),
                    }
                })?)?)
            }
            Some(BuiltinTypeId::I16) => {
                Ok(heap.alloc_ptr_i16(i16::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "i16".into(),
                        got: value.to_string(),
                    }
                })?)?)
            }
            Some(BuiltinTypeId::I32) => {
                Ok(heap.alloc_ptr_i32(i32::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "i32".into(),
                        got: value.to_string(),
                    }
                })?)?)
            }
            Some(BuiltinTypeId::I64) => {
                Ok(heap.alloc_ptr_i64(i64::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "i64".into(),
                        got: value.to_string(),
                    }
                })?)?)
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

fn alloc_int_literal_as<'a>(
    heap: &'a mut HeapState,
    value: i64,
    typ: &Type,
) -> Result<Reference<'a>, EngineError> {
    match typ.as_ref() {
        TypeKind::Var(_) => Ok(heap.alloc_ptr_i32(i32::try_from(value).map_err(|_| {
            EngineError::NativeType {
                expected: "i32".into(),
                got: value.to_string(),
            }
        })?)?),
        TypeKind::Con(tc) => match tc.builtin_id() {
            Some(BuiltinTypeId::I8) => {
                Ok(heap.alloc_ptr_i8(i8::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "i8".into(),
                        got: value.to_string(),
                    }
                })?)?)
            }
            Some(BuiltinTypeId::I16) => {
                Ok(heap.alloc_ptr_i16(i16::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "i16".into(),
                        got: value.to_string(),
                    }
                })?)?)
            }
            Some(BuiltinTypeId::I32) => {
                Ok(heap.alloc_ptr_i32(i32::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "i32".into(),
                        got: value.to_string(),
                    }
                })?)?)
            }
            Some(BuiltinTypeId::I64) => Ok(heap.alloc_ptr_i64(value)?),
            Some(BuiltinTypeId::U8) => {
                Ok(heap.alloc_ptr_u8(u8::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "u8".into(),
                        got: value.to_string(),
                    }
                })?)?)
            }
            Some(BuiltinTypeId::U16) => {
                Ok(heap.alloc_ptr_u16(u16::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "u16".into(),
                        got: value.to_string(),
                    }
                })?)?)
            }
            Some(BuiltinTypeId::U32) => {
                Ok(heap.alloc_ptr_u32(u32::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "u32".into(),
                        got: value.to_string(),
                    }
                })?)?)
            }
            Some(BuiltinTypeId::U64) => {
                Ok(heap.alloc_ptr_u64(u64::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "u64".into(),
                        got: value.to_string(),
                    }
                })?)?)
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
        TypeKind::Var(_) => engine
            .heap
            .with_locked(|heap| Ok(heap.alloc_ptr_f32(value as f32)?.into_pointer())),
        TypeKind::Con(tc) => match tc.builtin_id() {
            Some(BuiltinTypeId::F32) => engine
                .heap
                .with_locked(|heap| Ok(heap.alloc_ptr_f32(value as f32)?.into_pointer())),
            Some(BuiltinTypeId::F64) => engine
                .heap
                .with_locked(|heap| Ok(heap.alloc_ptr_f64(value)?.into_pointer())),
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
    heap.with_locked(|heap| project_pointer_with_access(heap, field, pointer))
}

fn project_pointer_with_access(
    heap: &HeapState,
    field: &Symbol,
    pointer: &Pointer,
) -> Result<Pointer, EngineError> {
    let value = heap.get_cell_from_pointer(pointer)?;
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
            let inner = heap.get_cell_from_pointer(&args[0])?;
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

pub(crate) fn synthetic_application_expr_from_head(
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

#[cfg(test)]
mod tests {
    use super::*;
    use rex_ast::{Span, Var};

    fn wildcard() -> Pattern {
        Pattern::Wildcard(Span::default())
    }

    #[test]
    fn binary_list_length_and_wildcard_patterns_do_not_allocate_elements() {
        let heap = Heap::new();
        let list = heap
            .alloc_ptr_binary_list(vec![10, 20, 30, 40])
            .expect("binary list should allocate");
        let list = heap.handle(list).expect("binary list should be rooted");
        let pointer = list.pointer().expect("binary list pointer should resolve");
        heap.with_locked_ok(|heap| heap.set_collect_on_every_alloc(true))
            .expect("collection setting should succeed");
        let collections_before = heap
            .with_locked_ok(|heap| heap.collection_count())
            .expect("collection count should be available");

        let empty = Pattern::List(Span::default(), Vec::new());
        assert!(
            match_pattern_ptr(&heap, &empty, &pointer)
                .expect("pattern matching should not error")
                .is_none()
        );

        let wrong_len = Pattern::List(Span::default(), vec![wildcard()]);
        assert!(
            match_pattern_ptr(&heap, &wrong_len, &pointer)
                .expect("pattern matching should not error")
                .is_none()
        );

        let exact_wildcards = Pattern::List(
            Span::default(),
            vec![wildcard(), wildcard(), wildcard(), wildcard()],
        );
        assert_eq!(
            match_pattern_ptr(&heap, &exact_wildcards, &pointer)
                .expect("pattern matching should not error"),
            Some(BTreeMap::new())
        );
        assert_eq!(
            heap.with_locked(|heap| Ok(heap.collection_count()))
                .expect("collection count should be available"),
            collections_before,
            "matching must not allocate a heap cell for each byte"
        );
    }

    #[test]
    fn nested_binary_list_bindings_survive_collection() {
        let heap = Heap::new();
        let first = heap
            .alloc_ptr_binary_list(vec![10])
            .expect("first binary list should allocate");
        let second = heap
            .alloc_ptr_binary_list(vec![20])
            .expect("second binary list should allocate");
        let outer = heap
            .alloc_ptr_list(vec![first, second])
            .expect("outer list should allocate");
        let outer = heap.handle(outer).expect("outer list should be rooted");
        let pointer = outer.pointer().expect("outer list pointer should resolve");
        let x = Symbol::intern("x");
        let y = Symbol::intern("y");
        let pattern = Pattern::List(
            Span::default(),
            vec![
                Pattern::List(Span::default(), vec![Pattern::Var(Var::new("x"))]),
                Pattern::List(Span::default(), vec![Pattern::Var(Var::new("y"))]),
            ],
        );

        heap.with_locked_ok(|heap| heap.set_collect_on_every_alloc(true))
            .expect("collection setting should succeed");
        let collections_before = heap
            .with_locked_ok(|heap| heap.collection_count())
            .expect("collection count should be available");
        let bindings = match_pattern_ptr(&heap, &pattern, &pointer)
            .expect("pattern matching should not error")
            .expect("nested list pattern should match");

        assert_eq!(
            heap.with_locked(
                |heap| heap.pointer_as_u8(bindings.get(&x).expect("x should be bound"))
            )
            .expect("x should be a u8"),
            10
        );
        assert_eq!(
            heap.with_locked(
                |heap| heap.pointer_as_u8(bindings.get(&y).expect("y should be bound"))
            )
            .expect("y should be a u8"),
            20
        );
        assert_eq!(
            heap.with_locked(|heap| Ok(heap.collection_count()))
                .expect("collection count should be available")
                - collections_before,
            2,
            "only the two bound bytes should be materialized"
        );
    }
}

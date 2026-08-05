use crate::{
    env::{RootedEnvironment, ScopedEnvironment},
    error::EngineError,
    evaluator::{
        application_result_type,
        context::ClassMethodPlan,
        native_functions::{NativeTask, eval_native_enter, eval_native_receive},
        resolve_arg_type,
        runtime_core::RuntimeCore,
        scheduler::{EvalScheduler, EvalWorkItem, HostScheduler, NativePoll, poll_pending_native},
    },
    handlers::{NativeCall, NativeCallRequest, NativeCompletion},
    memory::{
        heap::{Heap, RootScope, RootedCallable, RootedClosure, RootedPtr},
        lists::ListItems,
    },
    native_fn::NativeApplyResult,
    overloaded_fn::OverloadedFn,
    stack::{
        FrApp, FrAppArg, FrAppState, FrBranchState, FrHole, FrIte, FrLam, FrLet, FrLetRec,
        FrLetRecState, FrLetState, FrLiteral, FrMatch, FrMatchArm, FrMatchState, FrNativeCall,
        FrNativeCallState, FrNativeHost, FrProject, FrRecordUpdate, FrRecordUpdateState,
        FrSequence, FrSequenceState, FrValueState, FrVar, Frame, FrameId, FrameStore,
        FrameValueMapper,
    },
    util::{is_function_type, split_fun},
};
use rex_ast::{Pattern, Symbol};
use rex_typesystem::{
    types::{BuiltinTypeId, Type, TypeKind, TypedExpr, TypedExprKind, Types},
    unification::{Subst, compose_subst, unify},
};
use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
};

pub(crate) enum EvalControl {
    Push {
        expr: Arc<TypedExpr>,
        env: ScopedEnvironment,
    },
    PushFrame(Box<Frame<RootedPtr, ScopedEnvironment>>),
    Schedule(Vec<FrameId>),
    Wait,
    AwaitNative(NativeCallRequest),
    Return(RootedPtr),
}

struct EvalState<P, E> {
    frames: FrameStore<Frame<P, E>>,
    scheduler: EvalScheduler<P>,
}

type ScopedState = EvalState<RootedPtr, ScopedEnvironment>;

struct LiveRootMapper<'a> {
    live: &'a mut HashSet<RootedPtr>,
}

impl FrameValueMapper<RootedPtr, ScopedEnvironment> for LiveRootMapper<'_> {
    type Value = RootedPtr;
    type Environment = ScopedEnvironment;
    type Error = EngineError;

    fn map_value(&mut self, value: RootedPtr) -> Result<Self::Value, Self::Error> {
        self.live.insert(value);
        Ok(value)
    }

    fn map_environment(
        &mut self,
        env: ScopedEnvironment,
    ) -> Result<Self::Environment, Self::Error> {
        env.visit_values(&mut |value| {
            self.live.insert(value);
        });
        Ok(env)
    }
}

fn collect_machine_if_needed(
    runtime: &RuntimeCore<impl Clone + Send + Sync + 'static>,
    scope: &mut RootScope<'_>,
    state: &mut ScopedState,
) -> Result<(), EngineError> {
    if !scope.collection_needed() {
        return Ok(());
    }

    let mut live = HashSet::new();
    let frames = std::mem::take(&mut state.frames);
    let mut mapper = LiveRootMapper { live: &mut live };
    state.frames = frames.map_frames(|frame| frame.map_values(&mut mapper))?;
    state.scheduler.visit_values(&mut |value| {
        live.insert(*value);
    });
    runtime.typeclasses.visit_values(&mut |value| {
        live.insert(value);
    });
    runtime.natives.visit_values(&mut |value| {
        live.insert(value);
    });
    scope.collect_if_needed(&live)
}

pub(crate) async fn eval_typed_expr<State>(
    runtime: RuntimeCore<State>,
    heap: &mut Heap,
    rooted_env: RootedEnvironment,
    expr: Arc<TypedExpr>,
    input_args: Vec<(crate::Value, Type)>,
) -> Result<RootedPtr, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    eval_typed_expr_inner(runtime, heap, rooted_env, expr, input_args).await
}

async fn eval_typed_expr_inner<State>(
    runtime: RuntimeCore<State>,
    heap: &mut Heap,
    rooted_env: RootedEnvironment,
    expr: Arc<TypedExpr>,
    input_args: Vec<(crate::Value, Type)>,
) -> Result<RootedPtr, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let ready_work_limit = runtime.parallelism_controller.ready_work_limit();
    let mut host_scheduler = HostScheduler::new(runtime.parallelism_controller.clone());
    let mut state = heap.machine_root_scope(|scope| {
        let env = rooted_env.to_scoped_environment();
        let (env, expr) = if input_args.is_empty() {
            (env, expr)
        } else {
            let args = input_args
                .into_iter()
                .map(|(value, typ)| {
                    Ok((
                        scope.alloc_value(value, &typ, runtime.type_system.as_ref())?,
                        typ,
                    ))
                })
                .collect::<Result<Vec<_>, EngineError>>()?;
            let (env, expr) =
                synthetic_rooted_application_expr_from_head(env, expr.as_ref().clone(), &args)?;
            (env, Arc::new(expr))
        };
        let mut frames = FrameStore::default();
        let root_frame = frames.insert(frame_for_expr(None, expr, env));
        let scheduler = EvalScheduler::new(root_frame, ready_work_limit);
        Ok::<_, EngineError>(EvalState { frames, scheduler })
    })?;
    let mut boundary = EvalBoundary::MoreInternalWork;

    loop {
        let wait_for_host = boundary.wait_for_host();

        state
            .scheduler
            .set_ready_work_limit(runtime.parallelism_controller.ready_work_limit());
        let native_poll = poll_pending_native(&runtime, &mut host_scheduler, wait_for_host).await?;
        let completed_native = match native_poll {
            NativePoll::Progress => {
                boundary = EvalBoundary::from_state(&state, &host_scheduler)?;
                continue;
            }
            NativePoll::Completed { frame, completion } => Some((frame, completion)),
            NativePoll::Idle if !wait_for_host => None,
            NativePoll::Idle => {
                return Err(EngineError::Internal(
                    "eval scheduler ran out of host work".into(),
                ));
            }
        };

        let outcome = heap.machine_root_scope(|scope| {
            collect_machine_if_needed(&runtime, scope, &mut state)?;
            run_runtime_cycle(&runtime, scope, &mut state, completed_native)
        })?;
        match outcome {
            RuntimeCycleOutcome::Continue { queued_native } => {
                if let Some((frame, call)) = queued_native {
                    host_scheduler.schedule_native(frame, call);
                }
                boundary = EvalBoundary::from_state(&state, &host_scheduler)?;
            }
            RuntimeCycleOutcome::Completed(result) => return Ok(result),
        }
    }
}

enum EvalBoundary {
    MoreInternalWork,
    HostWork,
    WaitingForHost,
}

impl EvalBoundary {
    fn wait_for_host(&self) -> bool {
        matches!(self, Self::HostWork | Self::WaitingForHost)
    }

    fn from_state(
        state: &ScopedState,
        host_scheduler: &HostScheduler,
    ) -> Result<Self, EngineError> {
        if state.scheduler.has_ready_work() {
            Ok(Self::MoreInternalWork)
        } else if host_scheduler.has_queued_native_work() {
            Ok(Self::HostWork)
        } else if host_scheduler.has_pending_native_work() {
            Ok(Self::WaitingForHost)
        } else {
            Err(EngineError::Internal(
                "eval scheduler has no internal or host work".into(),
            ))
        }
    }
}

// Boxing `Continue` would add an allocation to every evaluator work item; the
// small `Completed` variant is taken only once at the end of evaluation.
#[allow(clippy::large_enum_variant)]
enum RuntimeCycleOutcome {
    Continue {
        queued_native: Option<(FrameId, NativeCall)>,
    },
    Completed(RootedPtr),
}

enum ScopedCycleOutcome {
    Continue {
        native_request: Option<(FrameId, NativeCallRequest)>,
    },
    Completed(RootedPtr),
}

fn run_runtime_cycle<'heap, State>(
    runtime: &RuntimeCore<State>,
    scope: &mut RootScope<'heap>,
    state: &mut ScopedState,
    completed_native: Option<(FrameId, NativeCompletion)>,
) -> Result<RuntimeCycleOutcome, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let outcome = run_scoped_eval_cycle(runtime, scope, state, completed_native)?;

    match outcome {
        ScopedCycleOutcome::Completed(value) => Ok(RuntimeCycleOutcome::Completed(value)),
        ScopedCycleOutcome::Continue { native_request } => {
            let queued_native = match native_request {
                Some((frame, call)) => Some((frame, call.prepare(scope, runtime)?)),
                None => None,
            };
            Ok(RuntimeCycleOutcome::Continue { queued_native })
        }
    }
}

fn run_scoped_eval_cycle<'heap, State>(
    runtime: &RuntimeCore<State>,
    scope: &mut RootScope<'heap>,
    state: &mut ScopedState,
    completed_native: Option<(FrameId, NativeCompletion)>,
) -> Result<ScopedCycleOutcome, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let EvalState { frames, scheduler } = state;
    if let Some((frame, completion)) = completed_native {
        let value = scope.alloc_value(
            completion.value,
            &completion.expected,
            runtime.type_system.as_ref(),
        )?;
        scheduler.schedule_next(EvalWorkItem::receive(frame, frame, value));
    }

    let item = scheduler
        .pop_next()
        .ok_or_else(|| EngineError::Internal("eval scheduler ran out of ready work".into()))?;
    let frame = frames.get(item.frame)?.clone();
    let returned = item
        .returned
        .as_ref()
        .map(|returned| (returned.child, returned.value));
    let control = match returned {
        Some((child, value)) => {
            eval_receive(runtime, scope, frames, item.frame, frame, child, value)?
        }
        None => eval_enter(runtime, scope, frames, item.frame, frame)?,
    };

    let mut native_request = None;
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
        EvalControl::AwaitNative(call) => {
            let frame = Frame::NativeHost(FrNativeHost {
                parent: Some(item.frame),
            });
            let child = frames.insert(frame);
            native_request = Some((child, call));
        }
        EvalControl::Return(value) => {
            let frame = frames.remove(item.frame)?;
            let parent = frame.parent();
            let Some(parent) = parent else {
                return Ok(ScopedCycleOutcome::Completed(value));
            };
            scheduler.schedule_next(EvalWorkItem::receive(parent, item.frame, value));
        }
    }

    Ok(ScopedCycleOutcome::Continue { native_request })
}

pub(crate) fn frame_for_expr(
    parent: Option<FrameId>,
    expr: Arc<TypedExpr>,
    env: ScopedEnvironment,
) -> Frame<RootedPtr, ScopedEnvironment> {
    let kind = Arc::clone(&expr.kind);
    match kind.as_ref() {
        TypedExprKind::Bool(_)
        | TypedExprKind::Uint(_)
        | TypedExprKind::Int(_)
        | TypedExprKind::Float(_)
        | TypedExprKind::Char(_)
        | TypedExprKind::String(_)
        | TypedExprKind::Uuid(_)
        | TypedExprKind::DateTime(_) => Frame::Literal(FrLiteral { parent, expr }),
        TypedExprKind::Hole => Frame::Hole(FrHole {
            parent,
            expr,
            env,
            state: FrValueState::Enter,
            value: None,
        }),
        TypedExprKind::Tuple(_) | TypedExprKind::List(_) | TypedExprKind::Dict(_) => {
            Frame::Sequence(FrSequence {
                parent,
                expr,
                env,
                state: FrSequenceState::Enter,
                children: Vec::new(),
                values: Vec::new(),
                remaining: 0,
            })
        }
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
    scope: &mut RootScope<'_>,
    frames: &mut FrameStore<Frame<RootedPtr, ScopedEnvironment>>,
    frame_id: FrameId,
    frame: Frame<RootedPtr, ScopedEnvironment>,
) -> Result<EvalControl, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    match frame {
        Frame::Literal(frame) => eval_literal_enter(scope, frame),
        Frame::Hole(_) => Err(EngineError::UnsupportedExpr),
        Frame::Sequence(frame) => eval_sequence_enter(scope, frames, frame_id, frame),
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
                match eval_resolve_var(runtime, scope, &frame.env, name, &frame.expr.typ)? {
                    EvalVarResult::Value(value) => Ok(EvalControl::Return(value)),
                    EvalVarResult::Push { expr, env } => {
                        frame.state = FrValueState::Enter;
                        frames.replace(frame_id, Frame::Var(frame))?;
                        Ok(EvalControl::Push { expr, env })
                    }
                    EvalVarResult::PushNative(task) => Ok(EvalControl::PushFrame(Box::new(
                        Frame::NativeCall(FrNativeCall {
                            parent: Some(frame_id),
                            state: FrNativeCallState::Enter,
                            task,
                        }),
                    ))),
                    EvalVarResult::AwaitNative(future) => {
                        frame.state = FrValueState::Enter;
                        frames.replace(frame_id, Frame::Var(frame))?;
                        Ok(EvalControl::AwaitNative(future))
                    }
                }
            }
            _ => frame_kind_error("var"),
        },
        Frame::App(frame) => eval_app_enter(frames, frame_id, frame),
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
                let root = scope.alloc_root_closure(
                    frame.env.clone(),
                    param.clone(),
                    param_ty,
                    frame.expr.typ.clone(),
                    Arc::clone(body),
                )?;
                Ok(EvalControl::Return(root))
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
            let mut slot_roots = Vec::with_capacity(bindings.len());
            for (name, _) in &bindings {
                slot_roots.push(scope.alloc_root_uninitialized(name.clone())?);
            }
            let mut frame = frame;
            let mut recursive_env = frame.env.clone();
            for ((name, _), root) in bindings.iter().zip(&slot_roots) {
                recursive_env = recursive_env.extend(name.clone(), *root);
            }
            frame.recursive_env = Some(recursive_env.clone());
            frame.slots = slot_roots;
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
        Frame::NativeCall(frame) => eval_native_enter(scope, frames, frame_id, frame),
        Frame::NativeHost(_) => unexpected_child_result("host native"),
    }
}

fn eval_literal_enter(
    scope: &mut RootScope<'_>,
    frame: FrLiteral,
) -> Result<EvalControl, EngineError> {
    let root = match frame.expr.kind.as_ref() {
        TypedExprKind::Bool(value) => scope.alloc_root_bool(*value)?,
        TypedExprKind::Uint(value) => alloc_uint_literal_as(scope, *value, &frame.expr.typ)?,
        TypedExprKind::Int(value) => alloc_int_literal_as(scope, *value, &frame.expr.typ)?,
        TypedExprKind::Float(value) => alloc_float_literal_as(scope, *value, &frame.expr.typ)?,
        TypedExprKind::Char(value) => scope.alloc_root_char(*value)?,
        TypedExprKind::String(value) => scope.alloc_root_string(value.clone())?,
        TypedExprKind::Uuid(value) => scope.alloc_root_uuid(*value)?,
        TypedExprKind::DateTime(value) => scope.alloc_root_datetime(*value)?,
        _ => return frame_kind_error("literal"),
    };
    Ok(EvalControl::Return(root))
}

fn eval_sequence_enter(
    scope: &mut RootScope<'_>,
    frames: &mut FrameStore<Frame<RootedPtr, ScopedEnvironment>>,
    frame_id: FrameId,
    mut frame: FrSequence<RootedPtr, ScopedEnvironment>,
) -> Result<EvalControl, EngineError> {
    let exprs = sequence_exprs(&frame.expr)?;
    if exprs.is_empty() {
        let root = alloc_sequence_values(scope, &frame.expr, Vec::new())?;
        return Ok(EvalControl::Return(root));
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
    frames.replace(frame_id, Frame::Sequence(frame))?;
    Ok(EvalControl::Schedule(children))
}

fn eval_record_update_updates_enter(
    frames: &mut FrameStore<Frame<RootedPtr, ScopedEnvironment>>,
    frame_id: FrameId,
    mut frame: FrRecordUpdate<RootedPtr, ScopedEnvironment>,
) -> Result<EvalControl, EngineError> {
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

fn eval_app_enter(
    frames: &mut FrameStore<Frame<RootedPtr, ScopedEnvironment>>,
    frame_id: FrameId,
    mut frame: FrApp<RootedPtr, ScopedEnvironment>,
) -> Result<EvalControl, EngineError> {
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

fn receive_sequence_value<P: Copy>(
    kind: &'static str,
    children: &[FrameId],
    values: &mut [Option<P>],
    remaining: &mut usize,
    child: FrameId,
    value: P,
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
    frame: &mut FrApp<RootedPtr, ScopedEnvironment>,
    child: FrameId,
    value: RootedPtr,
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

fn completed_values<P: Copy>(
    kind: &'static str,
    values: &[Option<P>],
) -> Result<Vec<P>, EngineError> {
    values
        .iter()
        .copied()
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| EngineError::Internal(format!("{kind} completed with missing result")))
}

fn map_keys_to_values<P>(
    kind: &'static str,
    keys: &[Symbol],
    values: Vec<P>,
) -> Result<BTreeMap<String, P>, EngineError> {
    if keys.len() != values.len() {
        return Err(EngineError::Internal(format!(
            "{kind} completed with mismatched keys and values"
        )));
    }
    Ok(keys.iter().map(ToString::to_string).zip(values).collect())
}

fn sequence_exprs(expr: &TypedExpr) -> Result<Vec<Arc<TypedExpr>>, EngineError> {
    match expr.kind.as_ref() {
        TypedExprKind::Tuple(elems) | TypedExprKind::List(elems) => Ok(elems.clone()),
        TypedExprKind::Dict(kvs) => Ok(kvs.values().cloned().collect()),
        _ => frame_kind_error("sequence"),
    }
}

fn sequence_kind(expr: &TypedExpr) -> Result<&'static str, EngineError> {
    match expr.kind.as_ref() {
        TypedExprKind::Tuple(_) => Ok("tuple"),
        TypedExprKind::List(_) => Ok("list"),
        TypedExprKind::Dict(_) => Ok("dict"),
        _ => frame_kind_error("sequence"),
    }
}

fn alloc_sequence_values(
    scope: &mut RootScope<'_>,
    expr: &TypedExpr,
    values: Vec<RootedPtr>,
) -> Result<RootedPtr, EngineError> {
    match expr.kind.as_ref() {
        TypedExprKind::Tuple(_) => scope.alloc_root_tuple(values),
        TypedExprKind::List(_) => scope.alloc_root_list(values),
        TypedExprKind::Dict(kvs) => {
            let keys = kvs.keys().cloned().collect::<Vec<_>>();
            let values = map_keys_to_values("dict", &keys, values)?;
            scope.alloc_root_dict(values)
        }
        _ => frame_kind_error("sequence"),
    }
}

fn record_update_exprs_for_keys<P, E>(
    frame: &FrRecordUpdate<P, E>,
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
    scope: &mut RootScope<'_>,
    frames: &mut FrameStore<Frame<RootedPtr, ScopedEnvironment>>,
    frame_id: FrameId,
    frame: Frame<RootedPtr, ScopedEnvironment>,
    child: FrameId,
    value: RootedPtr,
) -> Result<EvalControl, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    match frame {
        Frame::Sequence(mut frame) => {
            let kind = sequence_kind(&frame.expr)?;
            if frame.state != FrSequenceState::EvalItem {
                return unexpected_child_result(kind);
            }
            receive_sequence_value(
                kind,
                &frame.children,
                &mut frame.values,
                &mut frame.remaining,
                child,
                value,
            )?;
            if frame.remaining == 0 {
                let values = completed_values(kind, &frame.values)?;
                let root = alloc_sequence_values(scope, &frame.expr, values)?;
                return Ok(EvalControl::Return(root));
            }
            frames.replace(frame_id, Frame::Sequence(frame))?;
            Ok(EvalControl::Wait)
        }
        Frame::RecordUpdate(mut frame) => match frame.state {
            FrRecordUpdateState::EvalBase => {
                frame.base_value = Some(value);
                if frame.update_keys.is_empty() {
                    let root = apply_record_update_values(scope, value, BTreeMap::new())?;
                    return Ok(EvalControl::Return(root));
                }
                eval_record_update_updates_enter(frames, frame_id, frame)
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
                    let root = apply_record_update_values(scope, base, update_values)?;
                    return Ok(EvalControl::Return(root));
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
                    return continue_app_after_apply(runtime, scope, frames, frame_id, frame, None);
                }
                frames.replace(frame_id, Frame::App(frame))?;
                Ok(EvalControl::Wait)
            }
            FrAppState::ApplyArg => {
                continue_app_after_apply(runtime, scope, frames, frame_id, frame, Some(value))
            }
            _ => unexpected_child_result("application"),
        },
        Frame::Project(frame) => match frame.expr.kind.as_ref() {
            TypedExprKind::Project { field, .. } => {
                let res = project_pointer(scope, field, value)?;
                Ok(EvalControl::Return(res))
            }
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
                scope.overwrite_root(slot, value)?;
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
                let selected = match scope.root_as_bool(value) {
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
                loop {
                    if frame.next_arm_index >= frame.arms.len() {
                        return Err(EngineError::MatchFailure);
                    }
                    let idx = frame.next_arm_index;
                    let arm = &frame.arms[idx];
                    if let Some(bindings) = match_pattern_ptr(scope, &arm.pattern, value)? {
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
            eval_native_receive(scope, frames, frame_id, frame, child, value)
        }
        Frame::NativeHost(_) => Ok(EvalControl::Return(value)),
        _ => unexpected_child_result("value"),
    }
}

fn eval_apply_overloaded_arg<State>(
    runtime: &RuntimeCore<State>,
    scope: &mut RootScope<'_>,
    mut over: OverloadedFn<RootedPtr>,
    arg: RootedPtr,
    func_type: Option<&Type>,
    arg_type: Option<&Type>,
) -> Result<EvalApplyResult, EngineError>
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
    let actual_ty = resolve_arg_type(scope, arg_type, arg)?;
    let subst = unify(&arg_ty, &actual_ty).map_err(|_| EngineError::NativeType {
        expected: arg_ty.to_string(),
        got: actual_ty.to_string(),
    })?;
    let rest_ty = rest_ty.apply(&subst);
    over.applied.push(arg);
    over.applied_types.push(actual_ty);
    if is_function_type(&rest_ty) {
        let root =
            scope.alloc_root_overloaded(over.name, rest_ty, over.applied, over.applied_types)?;
        return Ok(EvalApplyResult::Value(root));
    }

    let mut full_ty = rest_ty;
    for arg_ty in over.applied_types.iter().rev() {
        full_ty = Type::fun(arg_ty.clone(), full_ty);
    }

    if runtime.type_system.class_methods.contains_key(&over.name) {
        return match runtime.resolve_class_method_plan(scope, &over.name, &full_ty)? {
            ClassMethodPlan::Evaluate { env, expr: method } => {
                let args = over
                    .applied
                    .into_iter()
                    .zip(over.applied_types)
                    .collect::<Vec<_>>();
                let (env, expr) = synthetic_rooted_application_expr_from_head(env, method, &args)?;
                Ok(EvalApplyResult::Push {
                    expr: Arc::new(expr),
                    env,
                })
            }
            ClassMethodPlan::Deferred(value) => Ok(EvalApplyResult::Value(value)),
        };
    }

    let (native_id, _, _) = runtime.resolve_native_parts(&over.name, &full_ty)?;
    runtime
        .native_callable(native_id)?
        .call(native_id, full_ty, &over.applied)
        .map(EvalApplyResult::AwaitNative)
}

fn eval_apply_arg<State>(
    runtime: &RuntimeCore<State>,
    scope: &mut RootScope<'_>,
    func: RootedPtr,
    arg: RootedPtr,
    func_type: Option<&Type>,
    arg_type: Option<&Type>,
) -> Result<EvalApplyResult, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    match scope.root_as_callable(func)? {
        Some(RootedCallable::Closure(RootedClosure {
            env,
            param,
            param_ty,
            typ,
            body,
        })) => {
            let mut subst = Subst::new_sync();
            if let Some(expected) = func_type {
                let s_fun = unify(&typ, expected).map_err(|_| EngineError::NativeType {
                    expected: typ.to_string(),
                    got: expected.to_string(),
                })?;
                subst = compose_subst(s_fun, subst);
            }
            let actual_ty = resolve_arg_type(scope, arg_type, arg)?;
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
        Some(RootedCallable::Native(native)) => {
            match native.apply(runtime, scope, arg, arg_type)? {
                NativeApplyResult::Value(value) => Ok(EvalApplyResult::Value(value)),
                NativeApplyResult::Task(task) => Ok(EvalApplyResult::PushNative(task)),
                NativeApplyResult::Pending(future) => Ok(EvalApplyResult::AwaitNative(future)),
            }
        }
        Some(RootedCallable::Overloaded(over)) => {
            eval_apply_overloaded_arg(runtime, scope, over, arg, func_type, arg_type)
        }
        None => Err(EngineError::NotCallable(scope.type_name(func)?.into())),
    }
}

fn continue_app_after_apply<State>(
    runtime: &RuntimeCore<State>,
    scope: &mut RootScope<'_>,
    frames: &mut FrameStore<Frame<RootedPtr, ScopedEnvironment>>,
    frame_id: FrameId,
    mut frame: FrApp<RootedPtr, ScopedEnvironment>,
    applied: Option<RootedPtr>,
) -> Result<EvalControl, EngineError>
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
        let apply_result = eval_apply_arg(
            runtime,
            scope,
            func,
            arg,
            Some(&arg_info.func_type),
            Some(&arg_info.expr.typ),
        )?;
        match apply_result {
            EvalApplyResult::Value(applied) => {
                frame.arg = None;
                frame.func = Some(applied);
                frame.next_arg_index += 1;
                frames.replace(frame_id, Frame::App(frame.clone()))?;
            }
            EvalApplyResult::Push { expr, env } => {
                return Ok(EvalControl::Push { expr, env });
            }
            EvalApplyResult::PushNative(task) => {
                return Ok(EvalControl::PushFrame(Box::new(Frame::NativeCall(
                    FrNativeCall {
                        parent: Some(frame_id),
                        state: FrNativeCallState::Enter,
                        task,
                    },
                ))));
            }
            EvalApplyResult::AwaitNative(future) => {
                return Ok(EvalControl::AwaitNative(future));
            }
        }
    }
}

fn eval_resolve_var<State>(
    runtime: &RuntimeCore<State>,
    scope: &mut RootScope<'_>,
    env: &ScopedEnvironment,
    name: &Symbol,
    typ: &Type,
) -> Result<EvalVarResult, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    if let Some(ptr) = env.get(name) {
        let native = scope
            .root_as_native(ptr)?
            .filter(|native| native.arity == 0 && native.applied.is_empty());
        if let Some(native) = native {
            match native.call_zero(runtime, scope)? {
                NativeApplyResult::Value(value) => Ok(EvalVarResult::Value(value)),
                NativeApplyResult::Task(task) => Ok(EvalVarResult::PushNative(task)),
                NativeApplyResult::Pending(call) => Ok(EvalVarResult::AwaitNative(call)),
            }
        } else {
            Ok(EvalVarResult::Value(ptr))
        }
    } else if runtime.type_system.class_methods.contains_key(name) {
        match runtime.resolve_class_method_plan(scope, name, typ)? {
            ClassMethodPlan::Evaluate { env, expr } => Ok(EvalVarResult::Push {
                expr: Arc::new(expr),
                env,
            }),
            ClassMethodPlan::Deferred(value) => Ok(EvalVarResult::Value(value)),
        }
    } else {
        let ctx_root = runtime.resolve_native(scope, name, typ)?;
        let native = scope
            .root_as_native(ctx_root)?
            .filter(|native| native.arity == 0 && native.applied.is_empty());
        if let Some(native) = native {
            match native.call_zero(runtime, scope)? {
                NativeApplyResult::Value(value) => Ok(EvalVarResult::Value(value)),
                NativeApplyResult::Task(task) => Ok(EvalVarResult::PushNative(task)),
                NativeApplyResult::Pending(call) => Ok(EvalVarResult::AwaitNative(call)),
            }
        } else {
            Ok(EvalVarResult::Value(ctx_root))
        }
    }
}

fn apply_record_update_values(
    scope: &mut RootScope<'_>,
    base_ptr: RootedPtr,
    update_vals: BTreeMap<String, RootedPtr>,
) -> Result<RootedPtr, EngineError> {
    enum RecordUpdateTarget {
        Dict(BTreeMap<String, RootedPtr>),
        Adt(Symbol, BTreeMap<String, RootedPtr>),
    }

    let target = match scope.type_name(base_ptr)? {
        "dict" => Ok(RecordUpdateTarget::Dict(scope.root_as_dict(base_ptr)?)),
        "adt" => {
            let (tag, args) = scope.root_as_adt(base_ptr)?;
            if args.len() != 1 || scope.type_name(args[0])? != "dict" {
                return Err(EngineError::UnsupportedExpr);
            }
            Ok(RecordUpdateTarget::Adt(tag, scope.root_as_dict(args[0])?))
        }
        _ => Err(EngineError::UnsupportedExpr),
    }?;

    match target {
        RecordUpdateTarget::Dict(mut map) => {
            for (key, value) in update_vals {
                map.insert(key, value);
            }
            let root = scope.alloc_root_dict(map)?;
            Ok(root)
        }
        RecordUpdateTarget::Adt(tag, mut map) => {
            for (key, value) in update_vals {
                map.insert(key, value);
            }
            let root = scope.alloc_root_dict(map)?;
            let root = scope.alloc_root_adt(tag, vec![root])?;
            Ok(root)
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
    scope: &mut RootScope<'_>,
    pat: &Pattern,
    value: RootedPtr,
) -> Result<Option<BTreeMap<Symbol, RootedPtr>>, EngineError> {
    match pat {
        Pattern::Wildcard(..) => Ok(Some(BTreeMap::new())),
        Pattern::Var(var) => {
            let mut bindings = BTreeMap::new();
            bindings.insert(var.name.clone(), value);
            Ok(Some(bindings))
        }
        Pattern::Named(_, name, ps) => {
            let expected = name.to_dotted_symbol();
            match scope.type_name(value)? {
                "adt" => {
                    let (vname, args) = scope.root_as_adt(value)?;
                    if runtime_ctor_matches(&vname, &expected) && args.len() == ps.len() {
                        return match_patterns(scope, ps, &args);
                    }
                    return Ok(None);
                }
                "list" => {}
                _ => return Ok(None),
            }

            match expected
                .as_ref()
                .rsplit('.')
                .next()
                .unwrap_or(expected.as_ref())
            {
                "Empty" if ps.is_empty() => Ok((scope.list_len(value)? == 0).then(BTreeMap::new)),
                "Cons" if ps.len() == 2 => {
                    let Some((head, tail)) = scope.list_head_tail(value)? else {
                        return Ok(None);
                    };
                    match_patterns(scope, ps, &[head, tail])
                }
                _ => Ok(None),
            }
        }
        Pattern::Tuple(_, ps) => {
            if scope.type_name(value)? != "tuple" {
                return Ok(None);
            }
            let xs = scope.root_as_tuple(value)?;
            if xs.len() == ps.len() {
                match_patterns(scope, ps, &xs)
            } else {
                Ok(None)
            }
        }
        Pattern::List(_, ps) => {
            if scope.list_len(value)? != ps.len() {
                return Ok(None);
            }
            if ps
                .iter()
                .all(|pattern| matches!(pattern, Pattern::Wildcard(..)))
            {
                return Ok(Some(BTreeMap::new()));
            }
            let values = scope.list_items(value)?;
            match_list_patterns(scope, ps, values)
        }
        Pattern::Cons(_, head, tail) => {
            let Some((head_value, tail_value)) = scope.list_head_tail(value)? else {
                return Ok(None);
            };
            match_patterns(
                scope,
                &[head.as_ref().clone(), tail.as_ref().clone()],
                &[head_value, tail_value],
            )
        }
        Pattern::Dict(_, fields) => {
            if scope.type_name(value)? != "dict" {
                return Ok(None);
            }
            let map = scope.root_as_dict(value)?;
            let Some(values) = fields
                .iter()
                .map(|(key, _)| map.get(key.as_ref()).copied())
                .collect::<Option<Vec<_>>>()
            else {
                return Ok(None);
            };
            let patterns = fields
                .iter()
                .map(|(_, pattern)| pattern.clone())
                .collect::<Vec<_>>();
            match_patterns(scope, &patterns, &values)
        }
    }
}

fn match_list_patterns(
    scope: &mut RootScope<'_>,
    patterns: &[Pattern],
    values: ListItems<RootedPtr>,
) -> Result<Option<BTreeMap<Symbol, RootedPtr>>, EngineError> {
    if patterns.len() != values.len() {
        return Ok(None);
    }

    let mut bindings = BTreeMap::new();
    for (index, pattern) in patterns.iter().enumerate() {
        if matches!(pattern, Pattern::Wildcard(..)) {
            continue;
        }
        let value = values.get(scope, index)?;
        let Some(sub) = match_pattern_ptr(scope, pattern, value)? else {
            return Ok(None);
        };
        bindings.extend(sub);
    }
    Ok(Some(bindings))
}

fn match_patterns(
    scope: &mut RootScope<'_>,
    patterns: &[Pattern],
    values: &[RootedPtr],
) -> Result<Option<BTreeMap<Symbol, RootedPtr>>, EngineError> {
    let mut bindings = BTreeMap::new();
    for (index, p) in patterns.iter().enumerate() {
        let value = values[index];
        let Some(sub) = match_pattern_ptr(scope, p, value)? else {
            return Ok(None);
        };
        bindings.extend(sub);
    }
    Ok(Some(bindings))
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

fn alloc_uint_literal_as(
    scope: &mut RootScope<'_>,
    value: u64,
    typ: &Type,
) -> Result<RootedPtr, EngineError> {
    match typ.as_ref() {
        TypeKind::Var(_) => Ok(scope.alloc_root_i32(i32::try_from(value).map_err(|_| {
            EngineError::NativeType {
                expected: "i32".into(),
                got: value.to_string(),
            }
        })?)?),
        TypeKind::Con(tc) => match tc.builtin_id() {
            Some(BuiltinTypeId::U8) => {
                Ok(scope.alloc_root_u8(u8::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "u8".into(),
                        got: value.to_string(),
                    }
                })?)?)
            }
            Some(BuiltinTypeId::U16) => {
                Ok(scope.alloc_root_u16(u16::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "u16".into(),
                        got: value.to_string(),
                    }
                })?)?)
            }
            Some(BuiltinTypeId::U32) => {
                Ok(scope.alloc_root_u32(u32::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "u32".into(),
                        got: value.to_string(),
                    }
                })?)?)
            }
            Some(BuiltinTypeId::U64) => Ok(scope.alloc_root_u64(value)?),
            Some(BuiltinTypeId::I8) => {
                Ok(scope.alloc_root_i8(i8::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "i8".into(),
                        got: value.to_string(),
                    }
                })?)?)
            }
            Some(BuiltinTypeId::I16) => {
                Ok(scope.alloc_root_i16(i16::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "i16".into(),
                        got: value.to_string(),
                    }
                })?)?)
            }
            Some(BuiltinTypeId::I32) => {
                Ok(scope.alloc_root_i32(i32::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "i32".into(),
                        got: value.to_string(),
                    }
                })?)?)
            }
            Some(BuiltinTypeId::I64) => {
                Ok(scope.alloc_root_i64(i64::try_from(value).map_err(|_| {
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

fn alloc_int_literal_as(
    scope: &mut RootScope<'_>,
    value: i64,
    typ: &Type,
) -> Result<RootedPtr, EngineError> {
    match typ.as_ref() {
        TypeKind::Var(_) => Ok(scope.alloc_root_i32(i32::try_from(value).map_err(|_| {
            EngineError::NativeType {
                expected: "i32".into(),
                got: value.to_string(),
            }
        })?)?),
        TypeKind::Con(tc) => match tc.builtin_id() {
            Some(BuiltinTypeId::I8) => {
                Ok(scope.alloc_root_i8(i8::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "i8".into(),
                        got: value.to_string(),
                    }
                })?)?)
            }
            Some(BuiltinTypeId::I16) => {
                Ok(scope.alloc_root_i16(i16::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "i16".into(),
                        got: value.to_string(),
                    }
                })?)?)
            }
            Some(BuiltinTypeId::I32) => {
                Ok(scope.alloc_root_i32(i32::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "i32".into(),
                        got: value.to_string(),
                    }
                })?)?)
            }
            Some(BuiltinTypeId::I64) => Ok(scope.alloc_root_i64(value)?),
            Some(BuiltinTypeId::U8) => {
                Ok(scope.alloc_root_u8(u8::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "u8".into(),
                        got: value.to_string(),
                    }
                })?)?)
            }
            Some(BuiltinTypeId::U16) => {
                Ok(scope.alloc_root_u16(u16::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "u16".into(),
                        got: value.to_string(),
                    }
                })?)?)
            }
            Some(BuiltinTypeId::U32) => {
                Ok(scope.alloc_root_u32(u32::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "u32".into(),
                        got: value.to_string(),
                    }
                })?)?)
            }
            Some(BuiltinTypeId::U64) => {
                Ok(scope.alloc_root_u64(u64::try_from(value).map_err(|_| {
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

fn alloc_float_literal_as(
    scope: &mut RootScope<'_>,
    value: f64,
    typ: &Type,
) -> Result<RootedPtr, EngineError> {
    match typ.as_ref() {
        TypeKind::Var(_) => scope.alloc_root_f32(value as f32),
        TypeKind::Con(tc) => match tc.builtin_id() {
            Some(BuiltinTypeId::F32) => scope.alloc_root_f32(value as f32),
            Some(BuiltinTypeId::F64) => scope.alloc_root_f64(value),
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

enum EvalApplyResult {
    Value(RootedPtr),
    Push {
        expr: Arc<TypedExpr>,
        env: ScopedEnvironment,
    },
    PushNative(NativeTask<RootedPtr>),
    AwaitNative(NativeCallRequest),
}

enum EvalVarResult {
    Value(RootedPtr),
    Push {
        expr: Arc<TypedExpr>,
        env: ScopedEnvironment,
    },
    PushNative(NativeTask<RootedPtr>),
    AwaitNative(NativeCallRequest),
}

fn project_pointer(
    scope: &mut RootScope<'_>,
    field: &Symbol,
    pointer: RootedPtr,
) -> Result<RootedPtr, EngineError> {
    if let Ok(index) = field.as_ref().parse::<usize>() {
        if scope.type_name(pointer)? != "tuple" {
            return Err(EngineError::UnknownField {
                field: field.clone(),
                value: scope.type_name(pointer)?.into(),
            });
        }
        return scope
            .root_as_tuple(pointer)?
            .get(index)
            .copied()
            .ok_or_else(|| EngineError::UnknownField {
                field: field.clone(),
                value: "tuple".into(),
            });
    }
    if scope.type_name(pointer)? != "adt" {
        return Err(EngineError::UnknownField {
            field: field.clone(),
            value: scope.type_name(pointer)?.into(),
        });
    }
    let (_, args) = scope.root_as_adt(pointer)?;
    let Some(record) = args.first().copied().filter(|_| args.len() == 1) else {
        return Err(EngineError::UnknownField {
            field: field.clone(),
            value: "adt".into(),
        });
    };
    if scope.type_name(record)? != "dict" {
        return Err(EngineError::UnknownField {
            field: field.clone(),
            value: scope.type_name(record)?.into(),
        });
    }
    scope
        .root_as_dict(record)?
        .get(field.as_ref())
        .copied()
        .ok_or_else(|| EngineError::UnknownField {
            field: field.clone(),
            value: "record".into(),
        })
}

fn synthetic_rooted_application_expr_from_head(
    mut env: ScopedEnvironment,
    head: TypedExpr,
    args: &[(RootedPtr, Type)],
) -> Result<(ScopedEnvironment, TypedExpr), EngineError> {
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
    use crate::memory::heap::Heap;
    use rex_ast::{Span, Var};

    fn wildcard() -> Pattern {
        Pattern::Wildcard(Span::default())
    }

    #[test]
    fn binary_list_length_and_wildcard_patterns_do_not_allocate_elements() {
        let mut heap = Heap::new();
        let list = heap
            .machine_root_scope(|scope| scope.alloc_root_binary_list(vec![10, 20, 30, 40]))
            .expect("binary list should allocate");
        heap.set_extreme_stress(true);
        let collections_before = heap.collection_count();

        let empty = Pattern::List(Span::default(), Vec::new());
        heap.root_scope(|scope| {
            assert!(
                match_pattern_ptr(scope, &empty, list)
                    .expect("pattern matching should not error")
                    .is_none()
            );
            Ok::<(), EngineError>(())
        })
        .unwrap();

        let wrong_len = Pattern::List(Span::default(), vec![wildcard()]);
        heap.root_scope(|scope| {
            assert!(
                match_pattern_ptr(scope, &wrong_len, list)
                    .expect("pattern matching should not error")
                    .is_none()
            );
            Ok::<(), EngineError>(())
        })
        .unwrap();

        let exact_wildcards = Pattern::List(
            Span::default(),
            vec![wildcard(), wildcard(), wildcard(), wildcard()],
        );
        heap.root_scope(|scope| {
            assert_eq!(
                match_pattern_ptr(scope, &exact_wildcards, list)
                    .expect("pattern matching should not error")
                    .unwrap()
                    .len(),
                0
            );
            Ok::<(), EngineError>(())
        })
        .unwrap();
        assert_eq!(
            heap.collection_count(),
            collections_before,
            "matching must not allocate a heap cell for each byte"
        );
    }

    #[test]
    fn nested_binary_list_bindings_survive_collection() {
        let mut heap = Heap::new();
        let outer = heap
            .machine_root_scope(|scope| {
                let first = scope.alloc_root_binary_list(vec![10])?;
                let second = scope.alloc_root_binary_list(vec![20])?;
                scope.alloc_root_list(vec![first, second])
            })
            .expect("nested binary lists should allocate");
        let x = Symbol::intern("x");
        let y = Symbol::intern("y");
        let pattern = Pattern::List(
            Span::default(),
            vec![
                Pattern::List(Span::default(), vec![Pattern::Var(Var::new("x"))]),
                Pattern::List(Span::default(), vec![Pattern::Var(Var::new("y"))]),
            ],
        );

        heap.set_extreme_stress(true);
        let collections_before = heap.collection_count();
        heap.root_scope(|scope| {
            let bindings = match_pattern_ptr(scope, &pattern, outer)
                .expect("pattern matching should not error")
                .expect("nested list pattern should match");
            let x = *bindings.get(&x).expect("x should be bound");
            assert_eq!(scope.root_as_u8(x).expect("x should be a u8"), 10);
            let y = *bindings.get(&y).expect("y should be bound");
            assert_eq!(scope.root_as_u8(y).expect("y should be a u8"), 20);
            Ok::<(), EngineError>(())
        })
        .unwrap();
        assert_eq!(
            heap.collection_count() - collections_before,
            2,
            "only the two bound bytes should be materialized"
        );
    }
}

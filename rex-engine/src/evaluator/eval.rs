use crate::{
    env::{PersistentEnvironment, RootedEnvironment, ScopedEnvironment},
    error::EngineError,
    evaluator::{
        CallSite, application_result_type,
        context::{ClassMethodPlan, InternalCtx},
        native_functions::{NativeTask, eval_native_enter, eval_native_receive},
        resolve_arg_type,
        runtime_core::RuntimeCore,
        scheduler::{EvalScheduler, EvalWorkItem, NativePoll, poll_pending_native},
    },
    handlers::NativeCallRequest,
    memory::{
        handle_promotion::{HandlePromoter, with_promotable_root_scope},
        heap::{
            Handle, Heap, PersistentPtr, PersistentRootStore, RootScope, RootedCallable,
            RootedClosure, RootedPtr,
        },
        lists::ListRootedItems,
    },
    native_fn::NativeApplyResult,
    overloaded_fn::OverloadedFn,
    stack::{
        FrApp, FrAppArg, FrAppState, FrBool, FrBranchState, FrDateTime, FrDict, FrFloat, FrHole,
        FrInt, FrIte, FrLam, FrLet, FrLetRec, FrLetRecState, FrLetState, FrList, FrMatch,
        FrMatchArm, FrMatchState, FrNativeCall, FrNativeCallState, FrNativeHost, FrProject,
        FrRecordUpdate, FrRecordUpdateState, FrSequenceState, FrString, FrTuple, FrUint, FrUuid,
        FrValueState, FrVar, Frame, FrameId, FrameStore, FrameValueMapper,
    },
    util::{is_function_type, split_fun},
};
use rex_ast::{Pattern, Symbol};
use rex_typesystem::{
    types::{BuiltinTypeId, Type, TypeKind, TypedExpr, TypedExprKind, Types},
    unification::{Subst, compose_subst, unify},
};
use std::{collections::BTreeMap, sync::Arc};

pub(crate) enum EvalControl<'scope, State: Clone + Send + Sync + 'static> {
    Push {
        expr: Arc<TypedExpr>,
        env: ScopedEnvironment<'scope>,
    },
    PushFrame(Box<Frame<RootedPtr<'scope>, ScopedEnvironment<'scope>>>),
    Schedule(Vec<FrameId>),
    Wait,
    AwaitNative(NativeCallRequest<'scope, State>),
    Return(RootedPtr<'scope>),
}

type PersistentFrame = Frame<PersistentPtr, PersistentEnvironment>;
type PersistentFrameStore = FrameStore<PersistentFrame>;

struct PersistentEvalState<State>
where
    State: Clone + Send + Sync + 'static,
{
    roots: PersistentRootStore,
    frames: PersistentFrameStore,
    scheduler: EvalScheduler<State, PersistentPtr>,
}

struct TransientEvalState<'scope, State>
where
    State: Clone + Send + Sync + 'static,
{
    previous_roots: PersistentRootStore,
    frames: FrameStore<Frame<RootedPtr<'scope>, ScopedEnvironment<'scope>>>,
    scheduler: EvalScheduler<State, RootedPtr<'scope>>,
}

struct PersistFrameMapper<'a, 'heap, 'scope> {
    roots: &'a mut PersistentRootStore,
    scope: &'a mut RootScope<'heap, 'scope>,
}

impl<'a, 'heap, 'scope> FrameValueMapper<RootedPtr<'scope>, ScopedEnvironment<'scope>>
    for PersistFrameMapper<'a, 'heap, 'scope>
{
    type Value = PersistentPtr;
    type Environment = PersistentEnvironment;
    type Error = EngineError;

    fn map_value(&mut self, value: RootedPtr<'scope>) -> Result<Self::Value, Self::Error> {
        self.roots.insert(self.scope, value)
    }

    fn map_environment(
        &mut self,
        env: ScopedEnvironment<'scope>,
    ) -> Result<Self::Environment, Self::Error> {
        PersistentEnvironment::persist(env, self.roots, self.scope)
    }
}

struct ResolveFrameMapper<'a, 'heap, 'scope> {
    roots: &'a PersistentRootStore,
    scope: &'a mut RootScope<'heap, 'scope>,
}

impl<'a, 'heap, 'scope> FrameValueMapper<PersistentPtr, PersistentEnvironment>
    for ResolveFrameMapper<'a, 'heap, 'scope>
{
    type Value = RootedPtr<'scope>;
    type Environment = ScopedEnvironment<'scope>;
    type Error = EngineError;

    fn map_value(&mut self, value: PersistentPtr) -> Result<Self::Value, Self::Error> {
        let rooted = self.roots.resolve(self.scope, &value)?;
        Ok(rooted)
    }

    fn map_environment(
        &mut self,
        env: PersistentEnvironment,
    ) -> Result<Self::Environment, Self::Error> {
        env.resolve(self.roots, self.scope)
    }
}

pub(crate) async fn eval_typed_expr<State>(
    runtime: RuntimeCore<State>,
    heap: &Heap,
    rooted_env: RootedEnvironment,
    expr: Arc<TypedExpr>,
    input_args: Vec<(Handle, Type)>,
) -> Result<Handle, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    eval_typed_expr_inner(runtime, heap, rooted_env, expr, input_args).await
}

fn persist_eval_state<'heap, 'scope, State>(
    scope: &mut RootScope<'heap, 'scope>,
    frames: FrameStore<Frame<RootedPtr<'scope>, ScopedEnvironment<'scope>>>,
    scheduler: EvalScheduler<State, RootedPtr<'scope>>,
    mut previous_roots: Option<PersistentRootStore>,
) -> Result<PersistentEvalState<State>, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let mut roots = scope.heap.persistent_root_store()?;

    let converted = (|| {
        let frames = {
            let mut mapper = PersistFrameMapper {
                roots: &mut roots,
                scope,
            };
            frames.map_frames(|frame| frame.map_values(&mut mapper))?
        };
        let scheduler = scheduler.map_values(&mut |value| roots.insert(scope, value))?;
        Ok::<_, EngineError>((frames, scheduler))
    })();

    let (frames, scheduler) = match converted {
        Ok(converted) => converted,
        Err(error) => {
            let _ = roots.clear(scope);
            if let Some(previous_roots) = previous_roots.as_mut() {
                let _ = previous_roots.clear(scope);
            }
            return Err(error);
        }
    };

    if let Some(previous_roots) = previous_roots.as_mut() {
        previous_roots.clear(scope)?;
    }
    Ok(PersistentEvalState {
        roots,
        frames,
        scheduler,
    })
}

fn resolve_eval_state<'heap, 'scope, State>(
    scope: &mut RootScope<'heap, 'scope>,
    state: PersistentEvalState<State>,
) -> Result<TransientEvalState<'scope, State>, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let PersistentEvalState {
        mut roots,
        frames,
        scheduler,
    } = state;

    let converted = (|| {
        let frames = {
            let mut mapper = ResolveFrameMapper {
                roots: &roots,
                scope,
            };
            frames.map_frames(|frame| frame.map_values(&mut mapper))?
        };
        let scheduler = scheduler.map_values(&mut |value| {
            let rooted = roots.resolve(scope, &value)?;
            Ok::<_, EngineError>(rooted)
        })?;
        Ok::<_, EngineError>((frames, scheduler))
    })();

    let (frames, scheduler) = match converted {
        Ok(converted) => converted,
        Err(error) => {
            let _ = roots.clear(scope);
            return Err(error);
        }
    };
    Ok(TransientEvalState {
        previous_roots: roots,
        frames,
        scheduler,
    })
}

async fn eval_typed_expr_inner<State>(
    mut runtime: RuntimeCore<State>,
    heap: &Heap,
    rooted_env: RootedEnvironment,
    expr: Arc<TypedExpr>,
    input_args: Vec<(Handle, Type)>,
) -> Result<Handle, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let state = heap.with_root_scope(|scope| {
        let env = rooted_env.to_scoped_environment(scope)?;
        let (env, expr) = if input_args.is_empty() {
            (env, expr)
        } else {
            let args = input_args
                .iter()
                .map(|(handle, typ)| {
                    let pointer = handle.pointer(scope.heap)?;
                    Ok((scope.root(pointer), typ.clone()))
                })
                .collect::<Result<Vec<_>, EngineError>>()?;
            let (env, expr) =
                synthetic_rooted_application_expr_from_head(env, expr.as_ref().clone(), &args)?;
            (env, Arc::new(expr))
        };
        let mut frames = FrameStore::default();
        let root_frame = frames.insert(frame_for_expr(None, expr, env));
        let scheduler = EvalScheduler::new(root_frame, runtime.parallelism_controller.clone());
        persist_eval_state(scope, frames, scheduler, None)
    })?;
    let mut boundary = EvalBoundary::MoreInternalWork(state);

    loop {
        let (mut state, wait_for_host) = match boundary {
            EvalBoundary::MoreInternalWork(state) => (state, false),
            EvalBoundary::HostWork(state) | EvalBoundary::WaitingForHost(state) => (state, true),
            EvalBoundary::Completed(result) => return Ok(result),
        };

        let completed_native =
            match poll_pending_native(&runtime, heap, &mut state.scheduler, wait_for_host).await? {
                NativePoll::Progress => {
                    boundary = EvalBoundary::from_state(state)?;
                    continue;
                }
                NativePoll::Completed { frame, handle } => Some((frame, handle)),
                NativePoll::Idle if !wait_for_host => None,
                NativePoll::Idle => {
                    return Err(EngineError::Internal(
                        "eval scheduler ran out of host work".into(),
                    ));
                }
            };

        let outcome = with_promotable_root_scope(heap, |scope, promoter| {
            run_locked_eval_cycle(&mut runtime, scope, promoter, state, completed_native)
        })?;
        drop(outcome.released_host_handle);
        boundary = outcome.boundary;
    }
}

enum EvalBoundary<State>
where
    State: Clone + Send + Sync + 'static,
{
    MoreInternalWork(PersistentEvalState<State>),
    HostWork(PersistentEvalState<State>),
    WaitingForHost(PersistentEvalState<State>),
    Completed(Handle),
}

impl<State> EvalBoundary<State>
where
    State: Clone + Send + Sync + 'static,
{
    fn from_state(state: PersistentEvalState<State>) -> Result<Self, EngineError> {
        if state.scheduler.has_ready_work() {
            Ok(Self::MoreInternalWork(state))
        } else if state.scheduler.has_queued_native_work() {
            Ok(Self::HostWork(state))
        } else if state.scheduler.has_pending_native_work() {
            Ok(Self::WaitingForHost(state))
        } else {
            Err(EngineError::Internal(
                "eval scheduler has no internal or host work".into(),
            ))
        }
    }
}

struct LockedCycleOutcome<State>
where
    State: Clone + Send + Sync + 'static,
{
    boundary: EvalBoundary<State>,
    released_host_handle: Option<Handle>,
}

fn run_locked_eval_cycle<'heap, 'scope, State>(
    runtime: &mut RuntimeCore<State>,
    scope: &mut RootScope<'heap, 'scope>,
    promoter: &HandlePromoter<'_>,
    state: PersistentEvalState<State>,
    completed_native: Option<(FrameId, Handle)>,
) -> Result<LockedCycleOutcome<State>, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let TransientEvalState {
        mut previous_roots,
        mut frames,
        mut scheduler,
    } = resolve_eval_state(scope, state)?;
    let released_host_handle = if let Some((frame, handle)) = completed_native {
        let value = scope.root(handle.pointer(scope.heap)?);
        scheduler.schedule_next(EvalWorkItem::receive(frame, frame, value));
        Some(handle)
    } else {
        None
    };

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
            eval_receive(runtime, scope, &mut frames, item.frame, frame, child, value)?
        }
        None => eval_enter(runtime, scope, &mut frames, item.frame, frame)?,
    };

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
            let call = call.promote(scope, promoter)?;
            let frame = Frame::NativeHost(FrNativeHost {
                parent: Some(item.frame),
            });
            let child = frames.insert(frame);
            scheduler.schedule_native(child, call);
        }
        EvalControl::Return(value) => {
            let frame = frames.remove(item.frame)?;
            let parent = frame.parent();
            let Some(parent) = parent else {
                previous_roots.clear(scope)?;
                let result = promoter.promote(scope, value)?;
                return Ok(LockedCycleOutcome {
                    boundary: EvalBoundary::Completed(result),
                    released_host_handle,
                });
            };
            scheduler.schedule_next(EvalWorkItem::receive(parent, item.frame, value));
        }
    }

    let state = persist_eval_state(scope, frames, scheduler, Some(previous_roots))?;
    Ok(LockedCycleOutcome {
        boundary: EvalBoundary::from_state(state)?,
        released_host_handle,
    })
}

pub(crate) fn frame_for_expr<'scope>(
    parent: Option<FrameId>,
    expr: Arc<TypedExpr>,
    env: ScopedEnvironment<'scope>,
) -> Frame<RootedPtr<'scope>, ScopedEnvironment<'scope>> {
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

fn eval_enter<'scope, State>(
    runtime: &RuntimeCore<State>,
    scope: &mut RootScope<'_, 'scope>,
    frames: &mut FrameStore<Frame<RootedPtr<'scope>, ScopedEnvironment<'scope>>>,
    frame_id: FrameId,
    frame: Frame<RootedPtr<'scope>, ScopedEnvironment<'scope>>,
) -> Result<EvalControl<'scope, State>, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    match frame {
        Frame::Bool(frame) => match frame.expr.kind.as_ref() {
            TypedExprKind::Bool(value) => {
                let root = scope.alloc_root_bool(*value)?;
                Ok(EvalControl::Return(root))
            }
            _ => frame_kind_error("bool"),
        },
        Frame::Uint(frame) => match frame.expr.kind.as_ref() {
            TypedExprKind::Uint(value) => {
                let root = alloc_uint_literal_as(scope, *value, &frame.expr.typ)?;
                Ok(EvalControl::Return(root))
            }
            _ => frame_kind_error("uint"),
        },
        Frame::Int(frame) => match frame.expr.kind.as_ref() {
            TypedExprKind::Int(value) => {
                let root = alloc_int_literal_as(scope, *value, &frame.expr.typ)?;
                Ok(EvalControl::Return(root))
            }
            _ => frame_kind_error("int"),
        },
        Frame::Float(frame) => match frame.expr.kind.as_ref() {
            TypedExprKind::Float(value) => {
                let root = alloc_float_literal_as(scope, *value, &frame.expr.typ)?;
                Ok(EvalControl::Return(root))
            }
            _ => frame_kind_error("float"),
        },
        Frame::String(frame) => match frame.expr.kind.as_ref() {
            TypedExprKind::String(value) => {
                let root = scope.alloc_root_string(value.clone())?;
                Ok(EvalControl::Return(root))
            }
            _ => frame_kind_error("string"),
        },
        Frame::Uuid(frame) => match frame.expr.kind.as_ref() {
            TypedExprKind::Uuid(value) => {
                let root = scope.alloc_root_uuid(*value)?;
                Ok(EvalControl::Return(root))
            }
            _ => frame_kind_error("uuid"),
        },
        Frame::DateTime(frame) => match frame.expr.kind.as_ref() {
            TypedExprKind::DateTime(value) => {
                let root = scope.alloc_root_datetime(*value)?;
                Ok(EvalControl::Return(root))
            }
            _ => frame_kind_error("datetime"),
        },
        Frame::Hole(_) => Err(EngineError::UnsupportedExpr),
        Frame::Tuple(frame) => eval_tuple_enter(scope, frames, frame_id, frame),
        Frame::List(frame) => eval_list_enter(scope, frames, frame_id, frame),
        Frame::Dict(frame) => eval_dict_enter(scope, frames, frame_id, frame),
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
                match eval_resolve_var(runtime, scope, frame_id, &frame.env, name, &frame.expr.typ)?
                {
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
                    frame.env.to_environment(scope),
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

fn eval_tuple_enter<'scope, State>(
    scope: &mut RootScope<'_, 'scope>,
    frames: &mut FrameStore<Frame<RootedPtr<'scope>, ScopedEnvironment<'scope>>>,
    frame_id: FrameId,
    mut frame: FrTuple<RootedPtr<'scope>, ScopedEnvironment<'scope>>,
) -> Result<EvalControl<'scope, State>, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let elems = match frame.expr.kind.as_ref() {
        TypedExprKind::Tuple(elems) => elems.clone(),
        _ => return frame_kind_error("tuple"),
    };
    if elems.is_empty() {
        let root = scope.alloc_root_tuple(vec![])?;
        return Ok(EvalControl::Return(root));
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

fn eval_list_enter<'scope, State>(
    scope: &mut RootScope<'_, 'scope>,
    frames: &mut FrameStore<Frame<RootedPtr<'scope>, ScopedEnvironment<'scope>>>,
    frame_id: FrameId,
    mut frame: FrList<RootedPtr<'scope>, ScopedEnvironment<'scope>>,
) -> Result<EvalControl<'scope, State>, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let elems = match frame.expr.kind.as_ref() {
        TypedExprKind::List(elems) => elems.clone(),
        _ => return frame_kind_error("list"),
    };
    if elems.is_empty() {
        let empty = scope.alloc_root_empty()?;
        return Ok(EvalControl::Return(empty));
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

fn eval_dict_enter<'scope, State>(
    scope: &mut RootScope<'_, 'scope>,
    frames: &mut FrameStore<Frame<RootedPtr<'scope>, ScopedEnvironment<'scope>>>,
    frame_id: FrameId,
    mut frame: FrDict<RootedPtr<'scope>, ScopedEnvironment<'scope>>,
) -> Result<EvalControl<'scope, State>, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let exprs = dict_exprs_for_keys(&frame, &frame.keys)?;
    if exprs.is_empty() {
        let root = scope.alloc_root_dict(BTreeMap::new())?;
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
    frames.replace(frame_id, Frame::Dict(frame))?;
    Ok(EvalControl::Schedule(children))
}

fn eval_record_update_updates_enter<'scope, State>(
    frames: &mut FrameStore<Frame<RootedPtr<'scope>, ScopedEnvironment<'scope>>>,
    frame_id: FrameId,
    mut frame: FrRecordUpdate<RootedPtr<'scope>, ScopedEnvironment<'scope>>,
) -> Result<EvalControl<'scope, State>, EngineError>
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

fn eval_app_enter<'scope, State>(
    frames: &mut FrameStore<Frame<RootedPtr<'scope>, ScopedEnvironment<'scope>>>,
    frame_id: FrameId,
    mut frame: FrApp<RootedPtr<'scope>, ScopedEnvironment<'scope>>,
) -> Result<EvalControl<'scope, State>, EngineError>
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

fn receive_app_child_value<'scope>(
    frame: &mut FrApp<RootedPtr<'scope>, ScopedEnvironment<'scope>>,
    child: FrameId,
    value: RootedPtr<'scope>,
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
) -> Result<BTreeMap<Symbol, P>, EngineError> {
    if keys.len() != values.len() {
        return Err(EngineError::Internal(format!(
            "{kind} completed with mismatched keys and values"
        )));
    }
    Ok(keys.iter().cloned().zip(values).collect())
}

fn dict_exprs_for_keys<P, E>(
    frame: &FrDict<P, E>,
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

fn eval_receive<'scope, State>(
    runtime: &RuntimeCore<State>,
    scope: &mut RootScope<'_, 'scope>,
    frames: &mut FrameStore<Frame<RootedPtr<'scope>, ScopedEnvironment<'scope>>>,
    frame_id: FrameId,
    frame: Frame<RootedPtr<'scope>, ScopedEnvironment<'scope>>,
    child: FrameId,
    value: RootedPtr<'scope>,
) -> Result<EvalControl<'scope, State>, EngineError>
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
                let root = scope.alloc_root_tuple(values)?;
                return Ok(EvalControl::Return(root));
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
                let values = completed_values("list", &frame.values)?;
                let root = scope.alloc_root_list(values)?;
                return Ok(EvalControl::Return(root));
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
                let root = scope.alloc_root_dict(values)?;
                return Ok(EvalControl::Return(root));
            }
            frames.replace(frame_id, Frame::Dict(frame))?;
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

fn eval_apply_overloaded_arg<'scope, State>(
    runtime: &RuntimeCore<State>,
    scope: &mut RootScope<'_, 'scope>,
    parent: FrameId,
    mut over: OverloadedFn<RootedPtr<'scope>>,
    arg: RootedPtr<'scope>,
    func_type: Option<&Type>,
    arg_type: Option<&Type>,
) -> Result<EvalApplyResult<'scope, State>, EngineError>
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
        let ctx = InternalCtx::new_with_parent(runtime, parent);
        return match ctx.resolve_class_method_plan(scope, &over.name, &full_ty)? {
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

    let call_site = CallSite::child(parent);
    let ctx = InternalCtx::new_at_call_site(runtime, call_site)
        .resolve_native_impl(over.name.as_ref(), &full_ty)?;
    ctx.func
        .call_at_site(full_ty, &over.applied, call_site)
        .map(EvalApplyResult::AwaitNative)
}

fn eval_apply_arg<'scope, State>(
    runtime: &RuntimeCore<State>,
    scope: &mut RootScope<'_, 'scope>,
    parent: FrameId,
    func: RootedPtr<'scope>,
    arg: RootedPtr<'scope>,
    func_type: Option<&Type>,
    arg_type: Option<&Type>,
) -> Result<EvalApplyResult<'scope, State>, EngineError>
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
            match native.apply_at_site(runtime, scope, arg, arg_type, CallSite::child(parent))? {
                NativeApplyResult::Value(value) => Ok(EvalApplyResult::Value(value)),
                NativeApplyResult::Task(task) => Ok(EvalApplyResult::PushNative(task)),
                NativeApplyResult::Pending(future) => Ok(EvalApplyResult::AwaitNative(future)),
            }
        }
        Some(RootedCallable::Overloaded(over)) => {
            eval_apply_overloaded_arg(runtime, scope, parent, over, arg, func_type, arg_type)
        }
        None => Err(EngineError::NotCallable(scope.type_name(func)?.into())),
    }
}

fn continue_app_after_apply<'scope, State>(
    runtime: &RuntimeCore<State>,
    scope: &mut RootScope<'_, 'scope>,
    frames: &mut FrameStore<Frame<RootedPtr<'scope>, ScopedEnvironment<'scope>>>,
    frame_id: FrameId,
    mut frame: FrApp<RootedPtr<'scope>, ScopedEnvironment<'scope>>,
    applied: Option<RootedPtr<'scope>>,
) -> Result<EvalControl<'scope, State>, EngineError>
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
            frame_id,
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

fn eval_resolve_var<'scope, State>(
    runtime: &RuntimeCore<State>,
    scope: &mut RootScope<'_, 'scope>,
    parent: FrameId,
    env: &ScopedEnvironment<'scope>,
    name: &Symbol,
    typ: &Type,
) -> Result<EvalVarResult<'scope, State>, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    if let Some(ptr) = env.get(name) {
        let native = scope
            .root_as_native(ptr)?
            .filter(|native| native.arity == 0 && native.applied.is_empty());
        if let Some(native) = native {
            native
                .call_zero_at_site(runtime, CallSite::child(parent))
                .map(EvalVarResult::AwaitNative)
        } else {
            Ok(EvalVarResult::Value(ptr))
        }
    } else if runtime.type_system.class_methods.contains_key(name) {
        let ctx = InternalCtx::new_with_parent(runtime, parent);
        match ctx.resolve_class_method_plan(scope, name, typ)? {
            ClassMethodPlan::Evaluate { env, expr } => Ok(EvalVarResult::Push {
                expr: Arc::new(expr),
                env,
            }),
            ClassMethodPlan::Deferred(value) => Ok(EvalVarResult::Value(value)),
        }
    } else {
        let ctx = InternalCtx::new_with_parent(runtime, parent);
        let ctx_root = ctx.resolve_native(scope, name.as_ref(), typ)?;
        let native = scope
            .root_as_native(ctx_root)?
            .filter(|native| native.arity == 0 && native.applied.is_empty());
        if let Some(native) = native {
            native
                .call_zero_at_site(runtime, CallSite::child(parent))
                .map(EvalVarResult::AwaitNative)
        } else {
            Ok(EvalVarResult::Value(ctx_root))
        }
    }
}

fn apply_record_update_values<'scope>(
    scope: &mut RootScope<'_, 'scope>,
    base_ptr: RootedPtr<'scope>,
    update_vals: BTreeMap<Symbol, RootedPtr<'scope>>,
) -> Result<RootedPtr<'scope>, EngineError> {
    enum RecordUpdateTarget<'a> {
        Dict(BTreeMap<Symbol, RootedPtr<'a>>),
        Adt(Symbol, BTreeMap<Symbol, RootedPtr<'a>>),
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

fn match_pattern_ptr<'scope>(
    scope: &mut RootScope<'_, 'scope>,
    pat: &Pattern,
    value: RootedPtr<'scope>,
) -> Result<Option<BTreeMap<Symbol, RootedPtr<'scope>>>, EngineError> {
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
            let values: ListRootedItems = scope.list_items(value)?;
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
                .map(|(key, _)| map.get(key).copied())
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

fn match_list_patterns<'scope>(
    scope: &mut RootScope<'_, 'scope>,
    patterns: &[Pattern],
    values: ListRootedItems<'scope>,
) -> Result<Option<BTreeMap<Symbol, RootedPtr<'scope>>>, EngineError> {
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

fn match_patterns<'scope>(
    scope: &mut RootScope<'_, 'scope>,
    patterns: &[Pattern],
    values: &[RootedPtr<'scope>],
) -> Result<Option<BTreeMap<Symbol, RootedPtr<'scope>>>, EngineError> {
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

fn alloc_uint_literal_as<'scope>(
    scope: &mut RootScope<'_, 'scope>,
    value: u64,
    typ: &Type,
) -> Result<RootedPtr<'scope>, EngineError> {
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

fn alloc_int_literal_as<'scope>(
    scope: &mut RootScope<'_, 'scope>,
    value: i64,
    typ: &Type,
) -> Result<RootedPtr<'scope>, EngineError> {
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

fn alloc_float_literal_as<'scope>(
    scope: &mut RootScope<'_, 'scope>,
    value: f64,
    typ: &Type,
) -> Result<RootedPtr<'scope>, EngineError> {
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

enum EvalApplyResult<'scope, State: Clone + Send + Sync + 'static> {
    Value(RootedPtr<'scope>),
    Push {
        expr: Arc<TypedExpr>,
        env: ScopedEnvironment<'scope>,
    },
    PushNative(NativeTask<RootedPtr<'scope>>),
    AwaitNative(NativeCallRequest<'scope, State>),
}

enum EvalVarResult<'scope, State: Clone + Send + Sync + 'static> {
    Value(RootedPtr<'scope>),
    Push {
        expr: Arc<TypedExpr>,
        env: ScopedEnvironment<'scope>,
    },
    AwaitNative(NativeCallRequest<'scope, State>),
}

fn project_pointer<'scope>(
    scope: &mut RootScope<'_, 'scope>,
    field: &Symbol,
    pointer: RootedPtr<'scope>,
) -> Result<RootedPtr<'scope>, EngineError> {
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
        .get(field)
        .copied()
        .ok_or_else(|| EngineError::UnknownField {
            field: field.clone(),
            value: "record".into(),
        })
}

fn synthetic_rooted_application_expr_from_head<'scope>(
    mut env: ScopedEnvironment<'scope>,
    head: TypedExpr,
    args: &[(RootedPtr<'scope>, Type)],
) -> Result<(ScopedEnvironment<'scope>, TypedExpr), EngineError> {
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
            .with_locked(|heap| {
                heap.root_scope(|scope| {
                    let root = scope.alloc_root_binary_list(vec![10, 20, 30, 40])?;
                    Ok(scope.pointer(root))
                })
            })
            .expect("binary list should allocate");
        let list = heap.handle(list).expect("binary list should be rooted");
        let pointer = heap
            .with_locked(|heap| list.pointer(heap))
            .expect("binary list pointer should resolve");
        heap.with_locked_ok(|heap| heap.set_collect_on_every_alloc(true))
            .expect("collection setting should succeed");
        let collections_before = heap
            .with_locked_ok(|heap| heap.collection_count())
            .expect("collection count should be available");

        let empty = Pattern::List(Span::default(), Vec::new());
        heap.with_locked(|heap| {
            heap.root_scope(|scope| {
                let pointer = scope.root(pointer);
                assert!(
                    match_pattern_ptr(scope, &empty, pointer)
                        .expect("pattern matching should not error")
                        .is_none()
                );
                Ok(())
            })
        })
        .unwrap();

        let wrong_len = Pattern::List(Span::default(), vec![wildcard()]);
        heap.with_locked(|heap| {
            heap.root_scope(|scope| {
                let pointer = scope.root(pointer);
                assert!(
                    match_pattern_ptr(scope, &wrong_len, pointer)
                        .expect("pattern matching should not error")
                        .is_none()
                );
                Ok(())
            })
        })
        .unwrap();

        let exact_wildcards = Pattern::List(
            Span::default(),
            vec![wildcard(), wildcard(), wildcard(), wildcard()],
        );
        heap.with_locked(|heap| {
            heap.root_scope(|scope| {
                let pointer = scope.root(pointer);
                assert_eq!(
                    match_pattern_ptr(scope, &exact_wildcards, pointer)
                        .expect("pattern matching should not error")
                        .unwrap()
                        .len(),
                    0
                );
                Ok(())
            })
        })
        .unwrap();
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
        let (_first, _second, outer) = heap
            .with_locked(|heap| {
                heap.root_scope(|scope| {
                    let first = scope
                        .alloc_root_binary_list(vec![10])
                        .expect("first binary list should allocate");
                    let second = scope
                        .alloc_root_binary_list(vec![20])
                        .expect("second binary list should allocate");
                    let outer = scope
                        .alloc_root_list(vec![first, second])
                        .expect("outer list should allocate");

                    let first = scope.pointer(first);
                    let second = scope.pointer(second);
                    let outer = scope.pointer(outer);
                    Ok((first, second, outer))
                })
            })
            .unwrap();
        let outer = heap.handle(outer).expect("outer list should be rooted");
        let pointer = heap
            .with_locked(|heap| outer.pointer(heap))
            .expect("outer list pointer should resolve");
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
        let bindings = heap
            .with_locked(|heap| {
                heap.root_scope(|scope| {
                    let pointer = scope.root(pointer);
                    let res = match_pattern_ptr(scope, &pattern, pointer)
                        .expect("pattern matching should not error")
                        .expect("nested list pattern should match");
                    Ok(BTreeMap::from_iter(
                        res.into_iter().map(|(k, v)| (k, scope.pointer(v))),
                    ))
                })
            })
            .unwrap();

        heap.with_locked(|heap| {
            heap.root_scope(|scope| {
                let x = bindings.get(&x).expect("x should be bound");
                let x = scope.root(*x);
                assert_eq!(scope.root_as_u8(x).expect("x should be a u8"), 10);
                let y = bindings.get(&y).expect("y should be bound");
                let y = scope.root(*y);
                assert_eq!(scope.root_as_u8(y).expect("y should be a u8"), 20);
                Ok(())
            })
        })
        .unwrap();
        assert_eq!(
            heap.with_locked(|heap| Ok(heap.collection_count()))
                .expect("collection count should be available")
                - collections_before,
            2,
            "only the two bound bytes should be materialized"
        );
    }
}

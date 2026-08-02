use crate::{
    env::Environment,
    error::EngineError,
    evaluator::{
        application_result_type,
        eval::{
            EvalControl, frame_for_expr, frame_kind_error, refresh_frame_from_roots,
            unexpected_child_result,
        },
    },
    memory::{
        heap::{HeapState, Pointer, RootScope, RootedPtr, TempRoots},
        lists::ListItems,
        traits::Collection,
    },
    overloaded_fn::OverloadedFn,
    stack::{
        FrNativeCall, FrNativeCallState, Frame, FrameId, FrameStore, NativeUnaryShape,
        rewrite_entries, rewrite_map_values, rewrite_option, rewrite_pointer, rewrite_slice,
    },
};
use rex_ast::Symbol;
use rex_typesystem::types::{BuiltinTypeId, Type, TypeKind, TypedExpr, TypedExprKind};
use std::{collections::BTreeMap, sync::Arc};

pub(crate) enum NativeStep {
    Wait,
    Push {
        expr: Arc<TypedExpr>,
        env: Environment,
    },
    Schedule(usize),
    Return(Pointer),
}

struct NativeChildSpec {
    expr: Arc<TypedExpr>,
    env: Environment,
}

fn native_step_to_control<'scope, State>(
    scope: &mut RootScope<'_, 'scope>,
    frames: &mut FrameStore<Frame<Pointer, Environment>>,
    frame_id: FrameId,
    mut frame: FrNativeCall<Pointer>,
    step: NativeStep,
) -> Result<EvalControl<State>, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    match step {
        NativeStep::Wait => {
            frames.replace(frame_id, Frame::NativeCall(frame))?;
            Ok(EvalControl::Wait)
        }
        NativeStep::Return(value) => Ok(EvalControl::Return(value)),
        NativeStep::Push { expr, env } => {
            frame.state = FrNativeCallState::Waiting;
            frames.replace(frame_id, Frame::NativeCall(frame))?;
            Ok(EvalControl::Push { expr, env })
        }
        NativeStep::Schedule(child_count) => {
            frame.state = FrNativeCallState::Waiting;
            frames.replace(frame_id, Frame::NativeCall(frame))?;

            for index in 0..child_count {
                let current_frame = match frames.get(frame_id)?.clone() {
                    Frame::NativeCall(frame) => frame,
                    _ => return frame_kind_error("native call"),
                };
                let child_spec = current_frame.task.scheduled_child_spec(scope, index)?;
                let frame = frame_for_expr(Some(frame_id), child_spec.expr, child_spec.env);
                let child = frames.insert(frame);
                let mut current_frame = match frames.get(frame_id)?.clone() {
                    Frame::NativeCall(frame) => frame,
                    _ => return frame_kind_error("native call"),
                };
                current_frame.task.push_scheduled_child(child)?;
                frames.replace(frame_id, Frame::NativeCall(current_frame))?;
            }

            let current_frame = match frames.get(frame_id)?.clone() {
                Frame::NativeCall(frame) => frame,
                _ => return frame_kind_error("native call"),
            };
            Ok(EvalControl::Schedule(
                current_frame.task.scheduled_children()?,
            ))
        }
    }
}

pub(crate) fn eval_native_enter<'scope, State>(
    scope: &mut RootScope<'_, 'scope>,
    frames: &mut FrameStore<Frame<Pointer, Environment>>,
    frame_id: FrameId,
    mut frame: FrNativeCall<Pointer>,
) -> Result<EvalControl<State>, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    if frame.state != FrNativeCallState::Enter {
        return unexpected_child_result("native call");
    }
    let mut protected = Vec::new();
    Frame::NativeCall(frame.clone()).trace_pointers(&mut protected);
    scope.with_temp_roots(protected.clone(), |scope, roots| {
        let step = frame.task.enter(scope)?;
        refresh_native_frame_from_roots(scope.heap, &mut frame, &protected, roots, 0)?;
        native_step_to_control(scope, frames, frame_id, frame, step)
    })
}

pub(crate) fn eval_native_receive<'scope, State>(
    scope: &mut RootScope<'_, 'scope>,
    frames: &mut FrameStore<Frame<Pointer, Environment>>,
    frame_id: FrameId,
    mut frame: FrNativeCall<Pointer>,
    child: FrameId,
    value: Pointer,
) -> Result<EvalControl<State>, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    if frame.state != FrNativeCallState::Waiting {
        return unexpected_child_result("native call");
    }
    if !<NativeTask<Pointer> as Coroutine>::receive_may_allocate(&frame.task) {
        let step = frame.task.receive(scope, child, value)?;
        return native_step_to_control(scope, frames, frame_id, frame, step);
    }

    let mut protected = vec![value];
    Frame::NativeCall(frame.clone()).trace_pointers(&mut protected);
    scope.with_temp_roots(protected.clone(), |scope, roots| {
        let step = frame.task.receive(scope, child, value)?;
        refresh_native_frame_from_roots(scope.heap, &mut frame, &protected, roots, 1)?;
        native_step_to_control(scope, frames, frame_id, frame, step)
    })
}

#[derive(Clone, Debug, PartialEq)]
pub enum NativeTask<P> {
    ApplyUnary(NativeApplyUnary<P>),
    SequenceMap(NativeSequenceMap<P>),
    SequenceFilter(NativeSequenceFilter<P>),
    SequenceFilterMap(NativeSequenceFilterMap<P>),
    SequenceFlatMap(NativeSequenceFlatMap<P>),
    UnaryMap(NativeUnaryMap<P>),
    UnaryFilter(NativeUnaryFilter<P>),
    UnaryFilterMap(NativeUnaryFilterMap<P>),
    UnaryFlatMap(NativeUnaryFlatMap<P>),
    Fold(NativeFold<P>),
    DictMap(NativeDictMap<P>),
    DictTraverse(NativeDictTraverse<P>),
    ArrayEq(NativeArrayEq<P>),
    Sum(NativeSum<P>),
    Mean(NativeMean<P>),
}

fn map_option_value<P, Q, E>(
    value: Option<P>,
    map: &mut impl FnMut(P) -> Result<Q, E>,
) -> Result<Option<Q>, E> {
    value.map(map).transpose()
}

fn map_option_values<P, Q, E>(
    values: Vec<Option<P>>,
    map: &mut impl FnMut(P) -> Result<Q, E>,
) -> Result<Vec<Option<Q>>, E> {
    values
        .into_iter()
        .map(|value| map_option_value(value, map))
        .collect()
}

fn map_nested_option_values<P, Q, E>(
    values: Vec<Option<Option<P>>>,
    map: &mut impl FnMut(P) -> Result<Q, E>,
) -> Result<Vec<Option<Option<Q>>>, E> {
    values
        .into_iter()
        .map(|value| value.map(|inner| map_option_value(inner, map)).transpose())
        .collect()
}

fn map_option_value_vecs<P, Q, E>(
    values: Vec<Option<Vec<P>>>,
    map: &mut impl FnMut(P) -> Result<Q, E>,
) -> Result<Vec<Option<Vec<Q>>>, E> {
    values
        .into_iter()
        .map(|value| {
            value
                .map(|items| items.into_iter().map(&mut *map).collect())
                .transpose()
        })
        .collect()
}

fn map_entries_into<P, Q, E>(
    entries: Vec<(Symbol, P)>,
    map: &mut impl FnMut(P) -> Result<Q, E>,
) -> Result<Vec<(Symbol, Q)>, E> {
    entries
        .into_iter()
        .map(|(name, value)| Ok((name, map(value)?)))
        .collect()
}

fn map_values_into<P, Q, E>(
    values: BTreeMap<Symbol, P>,
    map: &mut impl FnMut(P) -> Result<Q, E>,
) -> Result<BTreeMap<Symbol, Q>, E> {
    values
        .into_iter()
        .map(|(name, value)| Ok((name, map(value)?)))
        .collect()
}

impl<P> NativeTask<P> {
    pub(crate) fn map_values<Q, E>(
        self,
        map: &mut impl FnMut(P) -> Result<Q, E>,
    ) -> Result<NativeTask<Q>, E> {
        Ok(match self {
            Self::ApplyUnary(task) => NativeTask::ApplyUnary(NativeApplyUnary {
                func: map(task.func)?,
                func_type: task.func_type,
                arg: map(task.arg)?,
                arg_type: task.arg_type,
            }),
            Self::SequenceMap(task) => NativeTask::SequenceMap(NativeSequenceMap {
                func: map(task.func)?,
                func_type: task.func_type,
                elem_type: task.elem_type,
                values: task.values.map_values(map)?,
                shape: task.shape,
                children: task.children,
                output: map_option_values(task.output, map)?,
                remaining: task.remaining,
            }),
            Self::SequenceFilter(task) => NativeTask::SequenceFilter(NativeSequenceFilter {
                func: map(task.func)?,
                func_type: task.func_type,
                elem_type: task.elem_type,
                values: task.values.map_values(map)?,
                shape: task.shape,
                children: task.children,
                keep: task.keep,
                remaining: task.remaining,
            }),
            Self::SequenceFilterMap(task) => {
                NativeTask::SequenceFilterMap(NativeSequenceFilterMap {
                    func: map(task.func)?,
                    func_type: task.func_type,
                    elem_type: task.elem_type,
                    values: task.values.map_values(map)?,
                    shape: task.shape,
                    children: task.children,
                    output: map_nested_option_values(task.output, map)?,
                    remaining: task.remaining,
                })
            }
            Self::SequenceFlatMap(task) => NativeTask::SequenceFlatMap(NativeSequenceFlatMap {
                func: map(task.func)?,
                func_type: task.func_type,
                elem_type: task.elem_type,
                values: task.values.map_values(map)?,
                shape: task.shape,
                children: task.children,
                output: map_option_value_vecs(task.output, map)?,
                remaining: task.remaining,
            }),
            Self::UnaryMap(task) => NativeTask::UnaryMap(NativeUnaryMap {
                func: map(task.func)?,
                func_type: task.func_type,
                elem_type: task.elem_type,
                value: map(task.value)?,
                shape: task.shape,
            }),
            Self::UnaryFilter(task) => NativeTask::UnaryFilter(NativeUnaryFilter {
                func: map(task.func)?,
                func_type: task.func_type,
                elem_type: task.elem_type,
                value: map(task.value)?,
                original: map(task.original)?,
            }),
            Self::UnaryFilterMap(task) => NativeTask::UnaryFilterMap(NativeUnaryFilterMap {
                func: map(task.func)?,
                func_type: task.func_type,
                elem_type: task.elem_type,
                value: map(task.value)?,
            }),
            Self::UnaryFlatMap(task) => NativeTask::UnaryFlatMap(NativeUnaryFlatMap {
                func: map(task.func)?,
                func_type: task.func_type,
                elem_type: task.elem_type,
                value: map(task.value)?,
                shape: task.shape,
            }),
            Self::Fold(task) => NativeTask::Fold(NativeFold {
                func: map(task.func)?,
                func_type: task.func_type,
                acc_type: task.acc_type,
                elem_type: task.elem_type,
                values: task.values.map_values(map)?,
                acc: map(task.acc)?,
                order: task.order,
                state: task.state,
                next_index: task.next_index,
                step: map_option_value(task.step, map)?,
            }),
            Self::DictMap(task) => NativeTask::DictMap(NativeDictMap {
                func: map(task.func)?,
                func_type: task.func_type,
                elem_type: task.elem_type,
                entries: map_entries_into(task.entries, map)?,
                children: task.children,
                output: map_values_into(task.output, map)?,
                remaining: task.remaining,
            }),
            Self::DictTraverse(task) => NativeTask::DictTraverse(NativeDictTraverse {
                func: map(task.func)?,
                func_type: task.func_type,
                elem_type: task.elem_type,
                entries: map_entries_into(task.entries, map)?,
                next_index: task.next_index,
                output: map_values_into(task.output, map)?,
            }),
            Self::ArrayEq(task) => NativeTask::ArrayEq(NativeArrayEq {
                elem_type: task.elem_type,
                xs: task.xs.map_values(map)?,
                ys: task.ys.map_values(map)?,
                state: task.state,
                next_index: task.next_index,
                step: map_option_value(task.step, map)?,
                negate: task.negate,
            }),
            Self::Sum(task) => NativeTask::Sum(NativeSum {
                elem_type: task.elem_type,
                values: task.values.map_values(map)?,
                acc: map_option_value(task.acc, map)?,
                plus: map_option_value(task.plus, map)?,
                state: task.state,
                next_index: task.next_index,
                step: map_option_value(task.step, map)?,
            }),
            Self::Mean(task) => NativeTask::Mean(NativeMean {
                elem_type: task.elem_type,
                values: task.values.map_values(map)?,
                len: task.len,
                acc: map_option_value(task.acc, map)?,
                state: task.state,
                next_index: task.next_index,
                step: map_option_value(task.step, map)?,
                len_value: map_option_value(task.len_value, map)?,
            }),
        })
    }
}

impl Collection for NativeTask<Pointer> {
    fn map_pointers<E>(
        &mut self,
        map: &mut impl FnMut(Pointer) -> Result<Pointer, E>,
    ) -> Result<(), E> {
        match self {
            NativeTask::ApplyUnary(task) => task.map_pointers(map),
            NativeTask::SequenceMap(task) => task.map_pointers(map),
            NativeTask::SequenceFilter(task) => task.map_pointers(map),
            NativeTask::SequenceFilterMap(task) => task.map_pointers(map),
            NativeTask::SequenceFlatMap(task) => task.map_pointers(map),
            NativeTask::UnaryMap(task) => task.map_pointers(map),
            NativeTask::UnaryFilter(task) => task.map_pointers(map),
            NativeTask::UnaryFilterMap(task) => task.map_pointers(map),
            NativeTask::UnaryFlatMap(task) => task.map_pointers(map),
            NativeTask::Fold(task) => task.map_pointers(map),
            NativeTask::DictMap(task) => task.map_pointers(map),
            NativeTask::DictTraverse(task) => task.map_pointers(map),
            NativeTask::ArrayEq(task) => task.map_pointers(map),
            NativeTask::Sum(task) => task.map_pointers(map),
            NativeTask::Mean(task) => task.map_pointers(map),
        }
    }
}

impl NativeTask<Pointer> {
    fn push_scheduled_child(&mut self, child: FrameId) -> Result<(), EngineError> {
        match self {
            NativeTask::SequenceMap(task) => {
                task.children.push(child);
                Ok(())
            }
            NativeTask::SequenceFilter(task) => {
                task.children.push(child);
                Ok(())
            }
            NativeTask::SequenceFilterMap(task) => {
                task.children.push(child);
                Ok(())
            }
            NativeTask::SequenceFlatMap(task) => {
                task.children.push(child);
                Ok(())
            }
            NativeTask::DictMap(task) => {
                task.children.push(child);
                Ok(())
            }
            _ => Err(EngineError::Internal(
                "native task does not accept scheduled children".into(),
            )),
        }
    }

    fn scheduled_children(&self) -> Result<Vec<FrameId>, EngineError> {
        match self {
            NativeTask::SequenceMap(task) => Ok(task.children.clone()),
            NativeTask::SequenceFilter(task) => Ok(task.children.clone()),
            NativeTask::SequenceFilterMap(task) => Ok(task.children.clone()),
            NativeTask::SequenceFlatMap(task) => Ok(task.children.clone()),
            NativeTask::DictMap(task) => Ok(task.children.clone()),
            _ => Err(EngineError::Internal(
                "native task does not have scheduled children".into(),
            )),
        }
    }

    fn scheduled_child_spec<'scope>(
        &self,
        scope: &mut RootScope<'_, 'scope>,
        index: usize,
    ) -> Result<NativeChildSpec, EngineError> {
        match self {
            NativeTask::SequenceMap(task) => task.child_spec(scope, index),
            NativeTask::SequenceFilter(task) => task.child_spec(scope, index),
            NativeTask::SequenceFilterMap(task) => task.child_spec(scope, index),
            NativeTask::SequenceFlatMap(task) => task.child_spec(scope, index),
            NativeTask::DictMap(task) => task.child_spec(index),
            _ => Err(EngineError::Internal(
                "native task does not have scheduled child specs".into(),
            )),
        }
    }
}

impl Coroutine for NativeTask<Pointer> {
    fn enter<'scope>(
        &mut self,
        scope: &mut RootScope<'_, 'scope>,
    ) -> Result<NativeStep, EngineError> {
        match self {
            NativeTask::ApplyUnary(task) => task.enter(scope),
            NativeTask::SequenceMap(task) => task.enter(scope),
            NativeTask::SequenceFilter(task) => task.enter(scope),
            NativeTask::SequenceFilterMap(task) => task.enter(scope),
            NativeTask::SequenceFlatMap(task) => task.enter(scope),
            NativeTask::UnaryMap(task) => task.enter(scope),
            NativeTask::UnaryFilter(task) => task.enter(scope),
            NativeTask::UnaryFilterMap(task) => task.enter(scope),
            NativeTask::UnaryFlatMap(task) => task.enter(scope),
            NativeTask::Fold(task) => task.enter(scope),
            NativeTask::DictMap(task) => task.enter(scope),
            NativeTask::DictTraverse(task) => task.enter(scope),
            NativeTask::ArrayEq(task) => task.enter(scope),
            NativeTask::Sum(task) => task.enter(scope),
            NativeTask::Mean(task) => task.enter(scope),
        }
    }

    fn receive<'scope>(
        &mut self,
        scope: &mut RootScope<'_, 'scope>,
        child: FrameId,
        value: Pointer,
    ) -> Result<NativeStep, EngineError> {
        match self {
            NativeTask::ApplyUnary(task) => task.receive(scope, child, value),
            NativeTask::SequenceMap(task) => task.receive(scope, child, value),
            NativeTask::SequenceFilter(task) => task.receive(scope, child, value),
            NativeTask::SequenceFilterMap(task) => task.receive(scope, child, value),
            NativeTask::SequenceFlatMap(task) => task.receive(scope, child, value),
            NativeTask::UnaryMap(task) => task.receive(scope, child, value),
            NativeTask::UnaryFilter(task) => task.receive(scope, child, value),
            NativeTask::UnaryFilterMap(task) => task.receive(scope, child, value),
            NativeTask::UnaryFlatMap(task) => task.receive(scope, child, value),
            NativeTask::Fold(task) => task.receive(scope, child, value),
            NativeTask::DictMap(task) => task.receive(scope, child, value),
            NativeTask::DictTraverse(task) => task.receive(scope, child, value),
            NativeTask::ArrayEq(task) => task.receive(scope, child, value),
            NativeTask::Sum(task) => task.receive(scope, child, value),
            NativeTask::Mean(task) => task.receive(scope, child, value),
        }
    }

    fn receive_may_allocate(&self) -> bool {
        match self {
            NativeTask::ApplyUnary(task) => {
                <NativeApplyUnary<Pointer> as Coroutine>::receive_may_allocate(task)
            }
            NativeTask::SequenceMap(task) => {
                <NativeSequenceMap<Pointer> as Coroutine>::receive_may_allocate(task)
            }
            NativeTask::SequenceFilter(task) => {
                <NativeSequenceFilter<Pointer> as Coroutine>::receive_may_allocate(task)
            }
            NativeTask::SequenceFilterMap(task) => {
                <NativeSequenceFilterMap<Pointer> as Coroutine>::receive_may_allocate(task)
            }
            NativeTask::SequenceFlatMap(task) => {
                <NativeSequenceFlatMap<Pointer> as Coroutine>::receive_may_allocate(task)
            }
            NativeTask::UnaryMap(task) => {
                <NativeUnaryMap<Pointer> as Coroutine>::receive_may_allocate(task)
            }
            NativeTask::UnaryFilter(task) => {
                <NativeUnaryFilter<Pointer> as Coroutine>::receive_may_allocate(task)
            }
            NativeTask::UnaryFilterMap(task) => {
                <NativeUnaryFilterMap<Pointer> as Coroutine>::receive_may_allocate(task)
            }
            NativeTask::UnaryFlatMap(task) => {
                <NativeUnaryFlatMap<Pointer> as Coroutine>::receive_may_allocate(task)
            }
            NativeTask::Fold(task) => {
                <NativeFold<Pointer> as Coroutine>::receive_may_allocate(task)
            }
            NativeTask::DictMap(task) => {
                <NativeDictMap<Pointer> as Coroutine>::receive_may_allocate(task)
            }
            NativeTask::DictTraverse(task) => {
                <NativeDictTraverse<Pointer> as Coroutine>::receive_may_allocate(task)
            }
            NativeTask::ArrayEq(task) => {
                <NativeArrayEq<Pointer> as Coroutine>::receive_may_allocate(task)
            }
            NativeTask::Sum(task) => <NativeSum<Pointer> as Coroutine>::receive_may_allocate(task),
            NativeTask::Mean(task) => {
                <NativeMean<Pointer> as Coroutine>::receive_may_allocate(task)
            }
        }
    }
}

fn native_apply_step(
    func: Pointer,
    func_type: Type,
    arg: Pointer,
    arg_type: Type,
) -> Result<NativeStep, EngineError> {
    native_apply_spec(func, func_type, arg, arg_type).map(|spec| NativeStep::Push {
        expr: spec.expr,
        env: spec.env,
    })
}

fn native_apply_spec(
    func: Pointer,
    func_type: Type,
    arg: Pointer,
    arg_type: Type,
) -> Result<NativeChildSpec, EngineError> {
    let (env, expr) = synthetic_application_expr(func, func_type, &[(arg, arg_type)])?;
    Ok(NativeChildSpec {
        expr: Arc::new(expr),
        env,
    })
}

fn rewrite_options<E>(
    values: &mut [Option<Pointer>],
    rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, E>,
) -> Result<(), E> {
    for value in values.iter_mut().flatten() {
        rewrite_pointer(value, rewrite)?;
    }
    Ok(())
}

fn rewrite_nested_options<E>(
    values: &mut [Option<Option<Pointer>>],
    rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, E>,
) -> Result<(), E> {
    for value in values.iter_mut().filter_map(Option::as_mut).flatten() {
        rewrite_pointer(value, rewrite)?;
    }
    Ok(())
}

fn rewrite_option_vecs<E>(
    values: &mut [Option<Vec<Pointer>>],
    rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, E>,
) -> Result<(), E> {
    for values in values.iter_mut().flatten() {
        rewrite_slice(values, rewrite)?;
    }
    Ok(())
}

fn receive_continues_sequence(next_index: usize, len: usize) -> bool {
    match next_index.checked_add(1) {
        Some(next_index) => next_index < len,
        None => true,
    }
}

pub(crate) trait Coroutine {
    fn enter<'scope>(
        &mut self,
        scope: &mut RootScope<'_, 'scope>,
    ) -> Result<NativeStep, EngineError>;

    fn receive<'scope>(
        &mut self,
        scope: &mut RootScope<'_, 'scope>,
        child: FrameId,
        value: Pointer,
    ) -> Result<NativeStep, EngineError>;

    fn receive_may_allocate(&self) -> bool;
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeApplyUnary<P> {
    pub func: P,
    pub func_type: Type,
    pub arg: P,
    pub arg_type: Type,
}

impl Collection for NativeApplyUnary<Pointer> {
    fn map_pointers<E>(
        &mut self,
        map: &mut impl FnMut(Pointer) -> Result<Pointer, E>,
    ) -> Result<(), E> {
        rewrite_pointer(&mut self.func, map)?;
        rewrite_pointer(&mut self.arg, map)
    }
}

impl Coroutine for NativeApplyUnary<Pointer> {
    fn enter<'scope>(
        &mut self,
        _scope: &mut RootScope<'_, 'scope>,
    ) -> Result<NativeStep, EngineError> {
        native_apply_step(
            self.func,
            self.func_type.clone(),
            self.arg,
            self.arg_type.clone(),
        )
    }

    fn receive<'scope>(
        &mut self,
        _scope: &mut RootScope<'_, 'scope>,
        _child: FrameId,
        value: Pointer,
    ) -> Result<NativeStep, EngineError> {
        Ok(NativeStep::Return(value))
    }

    fn receive_may_allocate(&self) -> bool {
        false
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NativeSequenceMap<P> {
    pub func: P,
    pub func_type: Type,
    pub elem_type: Type,
    pub values: ListItems<P>,
    pub shape: NativeSequenceShape,
    pub children: Vec<FrameId>,
    pub output: Vec<Option<P>>,
    pub remaining: usize,
}

impl Collection for NativeSequenceMap<Pointer> {
    fn map_pointers<E>(
        &mut self,
        map: &mut impl FnMut(Pointer) -> Result<Pointer, E>,
    ) -> Result<(), E> {
        rewrite_pointer(&mut self.func, map)?;
        self.values.map_pointers(map)?;
        rewrite_options(&mut self.output, map)
    }
}
impl Coroutine for NativeSequenceMap<Pointer> {
    fn enter<'scope>(
        &mut self,
        scope: &mut RootScope<'_, 'scope>,
    ) -> Result<NativeStep, EngineError> {
        if self.values.is_empty() {
            let root = alloc_native_sequence(scope, &self.shape, Vec::new())?;
            let res = scope.pointer(root);
            return Ok(NativeStep::Return(res));
        }
        self.children.clear();
        self.output = vec![None; self.values.len()];
        self.remaining = self.values.len();
        Ok(NativeStep::Schedule(self.values.len()))
    }

    fn receive<'scope>(
        &mut self,
        scope: &mut RootScope<'_, 'scope>,
        child: FrameId,
        value: Pointer,
    ) -> Result<NativeStep, EngineError> {
        let index = self
            .children
            .iter()
            .position(|candidate| *candidate == child)
            .ok_or_else(|| {
                EngineError::Internal("native sequence map received unknown child".into())
            })?;
        let slot = self.output.get_mut(index).ok_or_else(|| {
            EngineError::Internal("native sequence map result slot out of bounds".into())
        })?;
        if slot.is_some() {
            return Err(EngineError::Internal(
                "native sequence map received duplicate child result".into(),
            ));
        }
        *slot = Some(value);
        self.remaining = self.remaining.checked_sub(1).ok_or_else(|| {
            EngineError::Internal("native sequence map received too many results".into())
        })?;
        if self.remaining == 0 {
            let output = self
                .output
                .iter()
                .copied()
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| {
                    EngineError::Internal(
                        "native sequence map completed with missing result".into(),
                    )
                })?;
            let output = output.into_iter().map(|x| scope.root(x)).collect();
            let root = alloc_native_sequence(scope, &self.shape, output)?;
            let res = scope.pointer(root);
            return Ok(NativeStep::Return(res));
        }
        Ok(NativeStep::Wait)
    }

    fn receive_may_allocate(&self) -> bool {
        self.remaining == 1
    }
}

impl NativeSequenceMap<Pointer> {
    fn child_spec<'scope>(
        &self,
        scope: &mut RootScope<'_, 'scope>,
        index: usize,
    ) -> Result<NativeChildSpec, EngineError> {
        let root = self.values.get(scope, index)?;
        let value = scope.pointer(root);
        native_apply_spec(
            self.func,
            self.func_type.clone(),
            value,
            self.elem_type.clone(),
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NativeSequenceFilter<P> {
    pub func: P,
    pub func_type: Type,
    pub elem_type: Type,
    pub values: ListItems<P>,
    pub shape: NativeSequenceShape,
    pub children: Vec<FrameId>,
    pub keep: Vec<Option<bool>>,
    pub remaining: usize,
}

impl Collection for NativeSequenceFilter<Pointer> {
    fn map_pointers<E>(
        &mut self,
        map: &mut impl FnMut(Pointer) -> Result<Pointer, E>,
    ) -> Result<(), E> {
        rewrite_pointer(&mut self.func, map)?;
        self.values.map_pointers(map)?;
        Ok(())
    }
}

impl Coroutine for NativeSequenceFilter<Pointer> {
    fn enter<'scope>(
        &mut self,
        scope: &mut RootScope<'_, 'scope>,
    ) -> Result<NativeStep, EngineError> {
        if self.values.is_empty() {
            let root = alloc_native_sequence(scope, &self.shape, Vec::new())?;
            let res = scope.pointer(root);
            return Ok(NativeStep::Return(res));
        }
        self.children.clear();
        self.keep = vec![None; self.values.len()];
        self.remaining = self.values.len();
        Ok(NativeStep::Schedule(self.values.len()))
    }

    fn receive<'scope>(
        &mut self,
        scope: &mut RootScope<'_, 'scope>,
        child: FrameId,
        value: Pointer,
    ) -> Result<NativeStep, EngineError> {
        let index = self
            .children
            .iter()
            .position(|candidate| *candidate == child)
            .ok_or_else(|| {
                EngineError::Internal("native sequence filter received unknown child".into())
            })?;
        let slot = self.keep.get_mut(index).ok_or_else(|| {
            EngineError::Internal("native sequence filter result slot out of bounds".into())
        })?;
        if slot.is_some() {
            return Err(EngineError::Internal(
                "native sequence filter received duplicate child result".into(),
            ));
        }
        let value = scope.root(value);
        *slot = Some(scope.root_as_bool(value)?);
        self.remaining = self.remaining.checked_sub(1).ok_or_else(|| {
            EngineError::Internal("native sequence filter received too many results".into())
        })?;
        if self.remaining == 0 {
            let mut output = Vec::new();
            for (index, keep) in self.keep.iter().enumerate() {
                match keep {
                    Some(true) => {
                        let root = self.values.get(scope, index)?;
                        output.push(scope.pointer(root));
                    }
                    Some(false) => {}
                    None => {
                        return Err(EngineError::Internal(
                            "native sequence filter completed with missing result".into(),
                        ));
                    }
                }
            }
            let output = output.into_iter().map(|x| scope.root(x)).collect();
            let root = alloc_native_sequence(scope, &self.shape, output)?;
            let res = scope.pointer(root);
            return Ok(NativeStep::Return(res));
        }
        Ok(NativeStep::Wait)
    }

    fn receive_may_allocate(&self) -> bool {
        self.remaining == 1
    }
}

impl NativeSequenceFilter<Pointer> {
    fn child_spec<'scope>(
        &self,
        scope: &mut RootScope<'_, 'scope>,
        index: usize,
    ) -> Result<NativeChildSpec, EngineError> {
        let root = self.values.get(scope, index)?;
        let value = scope.pointer(root);
        native_apply_spec(
            self.func,
            self.func_type.clone(),
            value,
            self.elem_type.clone(),
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NativeSequenceFilterMap<P> {
    pub func: P,
    pub func_type: Type,
    pub elem_type: Type,
    pub values: ListItems<P>,
    pub shape: NativeSequenceShape,
    pub children: Vec<FrameId>,
    pub output: Vec<Option<Option<P>>>,
    pub remaining: usize,
}

impl Collection for NativeSequenceFilterMap<Pointer> {
    fn map_pointers<E>(
        &mut self,
        map: &mut impl FnMut(Pointer) -> Result<Pointer, E>,
    ) -> Result<(), E> {
        rewrite_pointer(&mut self.func, map)?;
        self.values.map_pointers(map)?;
        rewrite_nested_options(&mut self.output, map)
    }
}

impl Coroutine for NativeSequenceFilterMap<Pointer> {
    fn enter<'scope>(
        &mut self,
        scope: &mut RootScope<'_, 'scope>,
    ) -> Result<NativeStep, EngineError> {
        if self.values.is_empty() {
            let root = alloc_native_sequence(scope, &self.shape, Vec::new())?;
            let res = scope.pointer(root);
            return Ok(NativeStep::Return(res));
        }
        self.children.clear();
        self.output = vec![None; self.values.len()];
        self.remaining = self.values.len();
        Ok(NativeStep::Schedule(self.values.len()))
    }

    fn receive<'scope>(
        &mut self,
        scope: &mut RootScope<'_, 'scope>,
        child: FrameId,
        value: Pointer,
    ) -> Result<NativeStep, EngineError> {
        let index = self
            .children
            .iter()
            .position(|candidate| *candidate == child)
            .ok_or_else(|| {
                EngineError::Internal("native sequence filter_map received unknown child".into())
            })?;
        let slot = self.output.get_mut(index).ok_or_else(|| {
            EngineError::Internal("native sequence filter_map result slot out of bounds".into())
        })?;
        if slot.is_some() {
            return Err(EngineError::Internal(
                "native sequence filter_map received duplicate child result".into(),
            ));
        }
        let value_root = scope.root(value);
        *slot = Some(option_value_ptr(scope, value_root)?.map(|x| scope.pointer(x)));
        self.remaining = self.remaining.checked_sub(1).ok_or_else(|| {
            EngineError::Internal("native sequence filter_map received too many results".into())
        })?;
        if self.remaining == 0 {
            let mut output = Vec::new();
            for value in &self.output {
                match value {
                    Some(Some(value)) => output.push(*value),
                    Some(None) => {}
                    None => {
                        return Err(EngineError::Internal(
                            "native sequence filter_map completed with missing result".into(),
                        ));
                    }
                }
            }
            let output = output.into_iter().map(|x| scope.root(x)).collect();
            let root = alloc_native_sequence(scope, &self.shape, output)?;
            let res = scope.pointer(root);
            return Ok(NativeStep::Return(res));
        }
        Ok(NativeStep::Wait)
    }

    fn receive_may_allocate(&self) -> bool {
        self.remaining == 1
    }
}

impl NativeSequenceFilterMap<Pointer> {
    fn child_spec<'scope>(
        &self,
        scope: &mut RootScope<'_, 'scope>,
        index: usize,
    ) -> Result<NativeChildSpec, EngineError> {
        let root = self.values.get(scope, index)?;
        let value = scope.pointer(root);
        native_apply_spec(
            self.func,
            self.func_type.clone(),
            value,
            self.elem_type.clone(),
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NativeSequenceFlatMap<P> {
    pub func: P,
    pub func_type: Type,
    pub elem_type: Type,
    pub values: ListItems<P>,
    pub shape: NativeSequenceShape,
    pub children: Vec<FrameId>,
    pub output: Vec<Option<Vec<P>>>,
    pub remaining: usize,
}

impl Collection for NativeSequenceFlatMap<Pointer> {
    fn map_pointers<E>(
        &mut self,
        map: &mut impl FnMut(Pointer) -> Result<Pointer, E>,
    ) -> Result<(), E> {
        rewrite_pointer(&mut self.func, map)?;
        self.values.map_pointers(map)?;
        rewrite_option_vecs(&mut self.output, map)
    }
}

impl Coroutine for NativeSequenceFlatMap<Pointer> {
    fn enter<'scope>(
        &mut self,
        scope: &mut RootScope<'_, 'scope>,
    ) -> Result<NativeStep, EngineError> {
        if self.values.is_empty() {
            let root = alloc_native_sequence(scope, &self.shape, Vec::new())?;
            let res = scope.pointer(root);
            return Ok(NativeStep::Return(res));
        }
        self.children.clear();
        self.output = vec![None; self.values.len()];
        self.remaining = self.values.len();
        Ok(NativeStep::Schedule(self.values.len()))
    }

    fn receive<'scope>(
        &mut self,
        scope: &mut RootScope<'_, 'scope>,
        child: FrameId,
        value: Pointer,
    ) -> Result<NativeStep, EngineError> {
        let index = self
            .children
            .iter()
            .position(|candidate| *candidate == child)
            .ok_or_else(|| {
                EngineError::Internal("native sequence flat_map received unknown child".into())
            })?;
        let slot = self.output.get_mut(index).ok_or_else(|| {
            EngineError::Internal("native sequence flat_map result slot out of bounds".into())
        })?;
        if slot.is_some() {
            return Err(EngineError::Internal(
                "native sequence flat_map received duplicate child result".into(),
            ));
        }

        let value_root = scope.root(value);
        let flattened = native_flatten_sequence(scope, &self.shape, value_root)?
            .into_iter()
            .map(|x| scope.pointer(x))
            .collect();
        *slot = Some(flattened);

        self.remaining = self.remaining.checked_sub(1).ok_or_else(|| {
            EngineError::Internal("native sequence flat_map received too many results".into())
        })?;
        if self.remaining == 0 {
            let mut output = Vec::new();
            for values in &self.output {
                match values {
                    Some(values) => output.extend(values.iter().copied()),
                    None => {
                        return Err(EngineError::Internal(
                            "native sequence flat_map completed with missing result".into(),
                        ));
                    }
                }
            }
            let output = output.into_iter().map(|x| scope.root(x)).collect();
            let root = alloc_native_sequence(scope, &self.shape, output)?;
            let res = scope.pointer(root);
            return Ok(NativeStep::Return(res));
        }
        Ok(NativeStep::Wait)
    }

    fn receive_may_allocate(&self) -> bool {
        self.remaining == 1
    }
}

impl NativeSequenceFlatMap<Pointer> {
    fn child_spec<'scope>(
        &self,
        scope: &mut RootScope<'_, 'scope>,
        index: usize,
    ) -> Result<NativeChildSpec, EngineError> {
        let root = self.values.get(scope, index)?;
        let value = scope.pointer(root);
        native_apply_spec(
            self.func,
            self.func_type.clone(),
            value,
            self.elem_type.clone(),
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NativeUnaryMap<P> {
    pub func: P,
    pub func_type: Type,
    pub elem_type: Type,
    pub value: P,
    pub shape: NativeUnaryShape,
}

impl Collection for NativeUnaryMap<Pointer> {
    fn map_pointers<E>(
        &mut self,
        map: &mut impl FnMut(Pointer) -> Result<Pointer, E>,
    ) -> Result<(), E> {
        rewrite_pointer(&mut self.func, map)?;
        rewrite_pointer(&mut self.value, map)
    }
}

impl Coroutine for NativeUnaryMap<Pointer> {
    fn enter<'scope>(
        &mut self,
        _scope: &mut RootScope<'_, 'scope>,
    ) -> Result<NativeStep, EngineError> {
        native_apply_step(
            self.func,
            self.func_type.clone(),
            self.value,
            self.elem_type.clone(),
        )
    }

    fn receive<'scope>(
        &mut self,
        scope: &mut RootScope<'_, 'scope>,
        _child: FrameId,
        value: Pointer,
    ) -> Result<NativeStep, EngineError> {
        let value = match &self.shape {
            NativeUnaryShape::Option => {
                let value = scope.root(value);
                let root = option_from_native_pointer(scope, Some(value))?;
                scope.pointer(root)
            }
            NativeUnaryShape::Result => {
                let value = scope.root(value);
                let root = result_from_native_pointer(scope, Ok(value))?;
                scope.pointer(root)
            }
        };
        Ok(NativeStep::Return(value))
    }

    fn receive_may_allocate(&self) -> bool {
        true
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NativeUnaryFilter<P> {
    pub func: P,
    pub func_type: Type,
    pub elem_type: Type,
    pub value: P,
    pub original: P,
}

impl Collection for NativeUnaryFilter<Pointer> {
    fn map_pointers<E>(
        &mut self,
        map: &mut impl FnMut(Pointer) -> Result<Pointer, E>,
    ) -> Result<(), E> {
        rewrite_pointer(&mut self.func, map)?;
        rewrite_pointer(&mut self.value, map)?;
        rewrite_pointer(&mut self.original, map)
    }
}

impl Coroutine for NativeUnaryFilter<Pointer> {
    fn enter<'scope>(
        &mut self,
        _scope: &mut RootScope<'_, 'scope>,
    ) -> Result<NativeStep, EngineError> {
        native_apply_step(
            self.func,
            self.func_type.clone(),
            self.value,
            self.elem_type.clone(),
        )
    }

    fn receive<'scope>(
        &mut self,
        scope: &mut RootScope<'_, 'scope>,
        _child: FrameId,
        value: Pointer,
    ) -> Result<NativeStep, EngineError> {
        let value_root = scope.root(value);
        let value = if scope.root_as_bool(value_root)? {
            scope.root(self.original)
        } else {
            option_from_native_pointer(scope, None)?
        };
        Ok(NativeStep::Return(scope.pointer(value)))
    }

    fn receive_may_allocate(&self) -> bool {
        true
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeUnaryFilterMap<P> {
    pub func: P,
    pub func_type: Type,
    pub elem_type: Type,
    pub value: P,
}

impl Collection for NativeUnaryFilterMap<Pointer> {
    fn map_pointers<E>(
        &mut self,
        map: &mut impl FnMut(Pointer) -> Result<Pointer, E>,
    ) -> Result<(), E> {
        rewrite_pointer(&mut self.func, map)?;
        rewrite_pointer(&mut self.value, map)
    }
}

impl Coroutine for NativeUnaryFilterMap<Pointer> {
    fn enter<'scope>(
        &mut self,
        _scope: &mut RootScope<'_, 'scope>,
    ) -> Result<NativeStep, EngineError> {
        native_apply_step(
            self.func,
            self.func_type.clone(),
            self.value,
            self.elem_type.clone(),
        )
    }

    fn receive<'scope>(
        &mut self,
        _scope: &mut RootScope<'_, 'scope>,
        _child: FrameId,
        value: Pointer,
    ) -> Result<NativeStep, EngineError> {
        Ok(NativeStep::Return(value))
    }

    fn receive_may_allocate(&self) -> bool {
        false
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NativeUnaryFlatMap<P> {
    pub func: P,
    pub func_type: Type,
    pub elem_type: Type,
    pub value: P,
    pub shape: NativeUnaryShape,
}

impl Collection for NativeUnaryFlatMap<Pointer> {
    fn map_pointers<E>(
        &mut self,
        map: &mut impl FnMut(Pointer) -> Result<Pointer, E>,
    ) -> Result<(), E> {
        rewrite_pointer(&mut self.func, map)?;
        rewrite_pointer(&mut self.value, map)
    }
}

impl Coroutine for NativeUnaryFlatMap<Pointer> {
    fn enter<'scope>(
        &mut self,
        _scope: &mut RootScope<'_, 'scope>,
    ) -> Result<NativeStep, EngineError> {
        native_apply_step(
            self.func,
            self.func_type.clone(),
            self.value,
            self.elem_type.clone(),
        )
    }

    fn receive<'scope>(
        &mut self,
        scope: &mut RootScope<'_, 'scope>,
        _child: FrameId,
        value: Pointer,
    ) -> Result<NativeStep, EngineError> {
        if matches!(self.shape, NativeUnaryShape::Result) {
            let value = scope.root(value);
            let _ = result_value_ptr(scope, value)?;
        }
        Ok(NativeStep::Return(value))
    }

    fn receive_may_allocate(&self) -> bool {
        false
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum NativeFoldOrder {
    Left,
    Right,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum NativeFoldState {
    Enter,
    ApplyFirst,
    ApplySecond,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NativeFold<P> {
    pub func: P,
    pub func_type: Type,
    pub acc_type: Type,
    pub elem_type: Type,
    pub values: ListItems<P>,
    pub acc: P,
    pub order: NativeFoldOrder,
    pub state: NativeFoldState,
    pub next_index: usize,
    pub step: Option<P>,
}

impl Collection for NativeFold<Pointer> {
    fn map_pointers<E>(
        &mut self,
        map: &mut impl FnMut(Pointer) -> Result<Pointer, E>,
    ) -> Result<(), E> {
        rewrite_pointer(&mut self.func, map)?;
        self.values.map_pointers(map)?;
        rewrite_pointer(&mut self.acc, map)?;
        rewrite_option(&mut self.step, map)
    }
}

impl Coroutine for NativeFold<Pointer> {
    fn enter<'scope>(
        &mut self,
        scope: &mut RootScope<'_, 'scope>,
    ) -> Result<NativeStep, EngineError> {
        if self.values.is_empty() {
            return Ok(NativeStep::Return(self.acc));
        }
        self.state = NativeFoldState::ApplyFirst;
        self.next_index = 0;
        self.apply_first(scope)
    }

    fn receive<'scope>(
        &mut self,
        scope: &mut RootScope<'_, 'scope>,
        _child: FrameId,
        value: Pointer,
    ) -> Result<NativeStep, EngineError> {
        match self.state {
            NativeFoldState::ApplyFirst => {
                self.step = Some(value);
                self.state = NativeFoldState::ApplySecond;
                self.apply_second(scope)
            }
            NativeFoldState::ApplySecond => {
                self.acc = value;
                self.step = None;
                self.next_index += 1;
                if self.next_index == self.values.len() {
                    return Ok(NativeStep::Return(self.acc));
                }
                self.state = NativeFoldState::ApplyFirst;
                self.apply_first(scope)
            }
            _ => unexpected_child_result("native fold"),
        }
    }

    fn receive_may_allocate(&self) -> bool {
        false
    }
}

impl NativeFold<Pointer> {
    fn apply_first<'scope>(
        &self,
        scope: &mut RootScope<'_, 'scope>,
    ) -> Result<NativeStep, EngineError> {
        let arg = match self.order {
            NativeFoldOrder::Left => self.acc,
            NativeFoldOrder::Right => self.value_at(scope, self.next_index)?,
        };
        let arg_type = match self.order {
            NativeFoldOrder::Left => self.acc_type.clone(),
            NativeFoldOrder::Right => self.elem_type.clone(),
        };
        native_apply_step(self.func, self.func_type.clone(), arg, arg_type)
    }

    fn apply_second<'scope>(
        &self,
        scope: &mut RootScope<'_, 'scope>,
    ) -> Result<NativeStep, EngineError> {
        let step = self
            .step
            .ok_or_else(|| EngineError::Internal("native fold missing step function".into()))?;
        let arg = match self.order {
            NativeFoldOrder::Left => self.value_at(scope, self.next_index)?,
            NativeFoldOrder::Right => self.acc,
        };
        let arg_type = match self.order {
            NativeFoldOrder::Left => self.elem_type.clone(),
            NativeFoldOrder::Right => self.acc_type.clone(),
        };
        let step_type = Type::fun(arg_type.clone(), self.acc_type.clone());
        native_apply_step(step, step_type, arg, arg_type)
    }

    fn value_at<'scope>(
        &self,
        scope: &mut RootScope<'_, 'scope>,
        index: usize,
    ) -> Result<Pointer, EngineError> {
        let index = match self.order {
            NativeFoldOrder::Left => index,
            NativeFoldOrder::Right => {
                self.values.len().checked_sub(index + 1).ok_or_else(|| {
                    EngineError::Internal("native fold index out of bounds".into())
                })?
            }
        };
        let root = self.values.get(scope, index)?;
        Ok(scope.pointer(root))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeDictMap<P> {
    pub func: P,
    pub func_type: Type,
    pub elem_type: Type,
    pub entries: Vec<(Symbol, P)>,
    pub children: Vec<FrameId>,
    pub output: BTreeMap<Symbol, P>,
    pub remaining: usize,
}

impl Collection for NativeDictMap<Pointer> {
    fn map_pointers<E>(
        &mut self,
        map: &mut impl FnMut(Pointer) -> Result<Pointer, E>,
    ) -> Result<(), E> {
        rewrite_pointer(&mut self.func, map)?;
        rewrite_entries(&mut self.entries, map)?;
        rewrite_map_values(&mut self.output, map)
    }
}

impl Coroutine for NativeDictMap<Pointer> {
    fn enter<'scope>(
        &mut self,
        scope: &mut RootScope<'_, 'scope>,
    ) -> Result<NativeStep, EngineError> {
        if self.entries.is_empty() {
            let root = scope.alloc_root_dict(BTreeMap::new())?;
            let res = scope.pointer(root);
            return Ok(NativeStep::Return(res));
        }
        self.children.clear();
        self.output.clear();
        self.remaining = self.entries.len();
        Ok(NativeStep::Schedule(self.entries.len()))
    }

    fn receive<'scope>(
        &mut self,
        scope: &mut RootScope<'_, 'scope>,
        child: FrameId,
        value: Pointer,
    ) -> Result<NativeStep, EngineError> {
        let index = self
            .children
            .iter()
            .position(|candidate| *candidate == child)
            .ok_or_else(|| {
                EngineError::Internal("native dict map received unknown child".into())
            })?;
        let (key, _) = self.entries.get(index).cloned().ok_or_else(|| {
            EngineError::Internal("native dict map result slot out of bounds".into())
        })?;
        if self.output.insert(key, value).is_some() {
            return Err(EngineError::Internal(
                "native dict map received duplicate child result".into(),
            ));
        }
        self.remaining = self.remaining.checked_sub(1).ok_or_else(|| {
            EngineError::Internal("native dict map received too many results".into())
        })?;
        if self.remaining == 0 {
            let output =
                BTreeMap::from_iter(self.output.iter().map(|(k, v)| (k.clone(), scope.root(*v))));
            let root = scope.alloc_root_dict(output)?;
            let res = scope.pointer(root);
            return Ok(NativeStep::Return(res));
        }
        Ok(NativeStep::Wait)
    }

    fn receive_may_allocate(&self) -> bool {
        self.remaining == 1
    }
}

impl NativeDictMap<Pointer> {
    fn child_spec(&self, index: usize) -> Result<NativeChildSpec, EngineError> {
        let (_, value) = self.entries.get(index).ok_or_else(|| {
            EngineError::Internal("native dict map child index out of bounds".into())
        })?;
        native_apply_spec(
            self.func,
            self.func_type.clone(),
            *value,
            self.elem_type.clone(),
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeDictTraverse<P> {
    pub func: P,
    pub func_type: Type,
    pub elem_type: Type,
    pub entries: Vec<(Symbol, P)>,
    pub next_index: usize,
    pub output: BTreeMap<Symbol, P>,
}

impl Collection for NativeDictTraverse<Pointer> {
    fn map_pointers<E>(
        &mut self,
        map: &mut impl FnMut(Pointer) -> Result<Pointer, E>,
    ) -> Result<(), E> {
        rewrite_pointer(&mut self.func, map)?;
        rewrite_entries(&mut self.entries, map)?;
        rewrite_map_values(&mut self.output, map)
    }
}

impl Coroutine for NativeDictTraverse<Pointer> {
    fn enter<'scope>(
        &mut self,
        scope: &mut RootScope<'_, 'scope>,
    ) -> Result<NativeStep, EngineError> {
        if self.entries.is_empty() {
            let root = scope.alloc_root_dict(BTreeMap::new())?;
            let dict = scope.pointer(root);
            let dict = scope.root(dict);
            let root = result_from_native_pointer(scope, Ok(dict))?;
            let result = scope.pointer(root);
            return Ok(NativeStep::Return(result));
        }
        self.next_index = 0;
        let (_, value) = self.entries[0].clone();
        native_apply_step(
            self.func,
            self.func_type.clone(),
            value,
            self.elem_type.clone(),
        )
    }

    fn receive<'scope>(
        &mut self,
        scope: &mut RootScope<'_, 'scope>,
        _child: FrameId,
        value: Pointer,
    ) -> Result<NativeStep, EngineError> {
        let value_root = scope.root(value);
        match result_value_ptr(scope, value_root)? {
            Ok(value) => {
                let (key, _) = self.entries[self.next_index].clone();
                self.output.insert(key, scope.pointer(value));
            }
            Err(err) => {
                let root = result_from_native_pointer(scope, Err(err))?;
                let result = scope.pointer(root);
                return Ok(NativeStep::Return(result));
            }
        }
        self.next_index += 1;
        if self.next_index == self.entries.len() {
            let output: BTreeMap<Symbol, RootedPtr<'_>> =
                BTreeMap::from_iter(self.output.iter().map(|(k, v)| (k.clone(), scope.root(*v))));
            let root = scope.alloc_root_dict(output)?;
            let dict = scope.pointer(root);
            let dict = scope.root(dict);
            let root = result_from_native_pointer(scope, Ok(dict))?;
            let result = scope.pointer(root);
            return Ok(NativeStep::Return(result));
        }
        let (_, value) = self.entries[self.next_index].clone();
        native_apply_step(
            self.func,
            self.func_type.clone(),
            value,
            self.elem_type.clone(),
        )
    }

    fn receive_may_allocate(&self) -> bool {
        true
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum NativeArrayEqState {
    Enter,
    ApplyFirst,
    ApplySecond,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeArrayEq<P> {
    pub elem_type: Type,
    pub xs: ListItems<P>,
    pub ys: ListItems<P>,
    pub state: NativeArrayEqState,
    pub next_index: usize,
    pub step: Option<P>,
    pub negate: bool,
}

impl Collection for NativeArrayEq<Pointer> {
    fn map_pointers<E>(
        &mut self,
        map: &mut impl FnMut(Pointer) -> Result<Pointer, E>,
    ) -> Result<(), E> {
        self.xs.map_pointers(map)?;
        self.ys.map_pointers(map)?;
        rewrite_option(&mut self.step, map)
    }
}

impl Coroutine for NativeArrayEq<Pointer> {
    fn enter<'scope>(
        &mut self,
        scope: &mut RootScope<'_, 'scope>,
    ) -> Result<NativeStep, EngineError> {
        if self.xs.len() != self.ys.len() {
            return self.result(scope, false);
        }
        if self.xs.is_empty() {
            return self.result(scope, true);
        }
        self.state = NativeArrayEqState::ApplyFirst;
        self.next_index = 0;
        self.apply_first(scope)
    }

    fn receive<'scope>(
        &mut self,
        scope: &mut RootScope<'_, 'scope>,
        _child: FrameId,
        value: Pointer,
    ) -> Result<NativeStep, EngineError> {
        match self.state {
            NativeArrayEqState::ApplyFirst => {
                self.step = Some(value);
                self.state = NativeArrayEqState::ApplySecond;
                self.apply_second(scope)
            }
            NativeArrayEqState::ApplySecond => {
                let value = scope.root(value);
                if !scope.root_as_bool(value)? {
                    return self.result(scope, false);
                }
                self.step = None;
                self.next_index += 1;
                if self.next_index == self.xs.len() {
                    return self.result(scope, true);
                }
                self.state = NativeArrayEqState::ApplyFirst;
                self.apply_first(scope)
            }
            _ => unexpected_child_result("native list equality"),
        }
    }

    fn receive_may_allocate(&self) -> bool {
        match self.state {
            NativeArrayEqState::Enter | NativeArrayEqState::ApplyFirst => false,
            NativeArrayEqState::ApplySecond => true,
        }
    }
}

impl NativeArrayEq<Pointer> {
    fn apply_first<'scope>(
        &self,
        scope: &mut RootScope<'_, 'scope>,
    ) -> Result<NativeStep, EngineError> {
        let bool_ty = Type::builtin(BuiltinTypeId::Bool);
        let eq_ty = Type::fun(
            self.elem_type.clone(),
            Type::fun(self.elem_type.clone(), bool_ty),
        );
        let lhs = self.xs.get(scope, self.next_index)?;
        let eq = overloaded_root(scope, "==", eq_ty.clone())?;
        native_apply_step(
            scope.pointer(eq),
            eq_ty,
            scope.pointer(lhs),
            self.elem_type.clone(),
        )
    }

    fn apply_second<'scope>(
        &self,
        scope: &mut RootScope<'_, 'scope>,
    ) -> Result<NativeStep, EngineError> {
        let step = self
            .step
            .ok_or_else(|| EngineError::Internal("native list equality missing step".into()))?;
        let bool_ty = Type::builtin(BuiltinTypeId::Bool);
        let step_ty = Type::fun(self.elem_type.clone(), bool_ty);
        let y = self.ys.get(scope, self.next_index)?;
        native_apply_step(step, step_ty, scope.pointer(y), self.elem_type.clone())
    }

    fn result<'scope>(
        &self,
        scope: &mut RootScope<'_, 'scope>,
        equal: bool,
    ) -> Result<NativeStep, EngineError> {
        let b = if self.negate { !equal } else { equal };
        let root = scope.alloc_root_bool(b)?;
        let res = scope.pointer(root);
        Ok(NativeStep::Return(res))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeSum<P> {
    pub elem_type: Type,
    pub values: ListItems<P>,
    pub acc: Option<P>,
    pub plus: Option<P>,
    pub state: NativeFoldState,
    pub next_index: usize,
    pub step: Option<P>,
}

impl Collection for NativeSum<Pointer> {
    fn map_pointers<E>(
        &mut self,
        map: &mut impl FnMut(Pointer) -> Result<Pointer, E>,
    ) -> Result<(), E> {
        self.values.map_pointers(map)?;
        rewrite_option(&mut self.acc, map)?;
        rewrite_option(&mut self.plus, map)?;
        rewrite_option(&mut self.step, map)
    }
}

impl Coroutine for NativeSum<Pointer> {
    fn enter<'scope>(
        &mut self,
        scope: &mut RootScope<'_, 'scope>,
    ) -> Result<NativeStep, EngineError> {
        if self.values.is_empty() {
            return Ok(native_eval_var_step(
                Symbol::intern("zero"),
                self.elem_type.clone(),
            ));
        }
        let root = self.values.get(scope, 0)?;
        let first = scope.pointer(root);
        self.acc = Some(first);
        self.next_index = 1;
        if self.next_index == self.values.len() {
            return Ok(NativeStep::Return(first));
        }
        self.state = NativeFoldState::ApplyFirst;
        self.apply_first_may_allocate(scope)
    }

    fn receive<'scope>(
        &mut self,
        scope: &mut RootScope<'_, 'scope>,
        _child: FrameId,
        value: Pointer,
    ) -> Result<NativeStep, EngineError> {
        match self.state {
            NativeFoldState::Enter => Ok(NativeStep::Return(value)),
            NativeFoldState::ApplyFirst => {
                self.step = Some(value);
                self.state = NativeFoldState::ApplySecond;
                self.apply_second(scope)
            }
            NativeFoldState::ApplySecond => {
                self.acc = Some(value);
                self.step = None;
                self.next_index += 1;
                if self.next_index == self.values.len() {
                    return Ok(NativeStep::Return(value));
                }
                self.state = NativeFoldState::ApplyFirst;
                self.apply_first()
            }
        }
    }

    fn receive_may_allocate(&self) -> bool {
        match self.state {
            NativeFoldState::Enter | NativeFoldState::ApplyFirst => false,
            NativeFoldState::ApplySecond => {
                self.plus.is_none()
                    && receive_continues_sequence(self.next_index, self.values.len())
            }
        }
    }
}

impl NativeSum<Pointer> {
    fn apply_first_may_allocate<'scope>(
        &mut self,
        scope: &mut RootScope<'_, 'scope>,
    ) -> Result<NativeStep, EngineError> {
        let plus_ty = binary_same_type(&self.elem_type);
        let acc = self
            .acc
            .ok_or_else(|| EngineError::Internal("native sum missing accumulator".into()))?;
        let acc = scope.root(acc);
        if self.plus.is_none() {
            let root = overloaded_root(scope, "+", plus_ty.clone())?;
            let pointer = scope.pointer(root);
            self.plus = Some(pointer);
        }
        self.acc = Some(scope.pointer(acc));
        self.apply_first()
    }

    fn apply_first(&self) -> Result<NativeStep, EngineError> {
        let plus_ty = binary_same_type(&self.elem_type);
        let plus = self
            .plus
            .ok_or_else(|| EngineError::Internal("native sum missing plus function".into()))?;
        let acc = self
            .acc
            .ok_or_else(|| EngineError::Internal("native sum missing accumulator".into()))?;
        native_apply_step(plus, plus_ty, acc, self.elem_type.clone())
    }

    fn apply_second<'scope>(
        &self,
        scope: &mut RootScope<'_, 'scope>,
    ) -> Result<NativeStep, EngineError> {
        let step = self
            .step
            .ok_or_else(|| EngineError::Internal("native sum missing step".into()))?;
        let step_ty = Type::fun(self.elem_type.clone(), self.elem_type.clone());
        let arg = self.values.get(scope, self.next_index)?;
        native_apply_step(step, step_ty, scope.pointer(arg), self.elem_type.clone())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum NativeMeanState {
    Enter,
    ApplyPlusFirst,
    ApplyPlusSecond,
    ApplyDivFirst,
    ApplyDivSecond,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeMean<P> {
    pub elem_type: Type,
    pub values: ListItems<P>,
    pub len: usize,
    pub acc: Option<P>,
    pub state: NativeMeanState,
    pub next_index: usize,
    pub step: Option<P>,
    pub len_value: Option<P>,
}

impl Collection for NativeMean<Pointer> {
    fn map_pointers<E>(
        &mut self,
        map: &mut impl FnMut(Pointer) -> Result<Pointer, E>,
    ) -> Result<(), E> {
        self.values.map_pointers(map)?;
        rewrite_option(&mut self.acc, map)?;
        rewrite_option(&mut self.step, map)?;
        rewrite_option(&mut self.len_value, map)
    }
}

impl Coroutine for NativeMean<Pointer> {
    fn enter<'scope>(
        &mut self,
        scope: &mut RootScope<'_, 'scope>,
    ) -> Result<NativeStep, EngineError> {
        if self.values.is_empty() {
            return Err(EngineError::EmptySequence);
        }
        let root = self.values.get(scope, 0)?;
        self.acc = Some(scope.pointer(root));
        self.next_index = 1;
        if self.next_index == self.values.len() {
            self.state = NativeMeanState::ApplyDivFirst;
            return self.apply_div_first(scope);
        }
        self.state = NativeMeanState::ApplyPlusFirst;
        self.apply_plus_first(scope)
    }

    fn receive<'scope>(
        &mut self,
        scope: &mut RootScope<'_, 'scope>,
        _child: FrameId,
        value: Pointer,
    ) -> Result<NativeStep, EngineError> {
        match self.state {
            NativeMeanState::ApplyPlusFirst => {
                self.step = Some(value);
                self.state = NativeMeanState::ApplyPlusSecond;
                self.apply_plus_second(scope)
            }
            NativeMeanState::ApplyPlusSecond => {
                self.acc = Some(value);
                self.step = None;
                self.next_index += 1;
                if self.next_index == self.values.len() {
                    self.state = NativeMeanState::ApplyDivFirst;
                    return self.apply_div_first(scope);
                }
                self.state = NativeMeanState::ApplyPlusFirst;
                self.apply_plus_first(scope)
            }
            NativeMeanState::ApplyDivFirst => {
                self.step = Some(value);
                self.state = NativeMeanState::ApplyDivSecond;
                self.apply_div_second()
            }
            NativeMeanState::ApplyDivSecond => Ok(NativeStep::Return(value)),
            NativeMeanState::Enter => unexpected_child_result("native mean"),
        }
    }

    fn receive_may_allocate(&self) -> bool {
        match self.state {
            NativeMeanState::Enter
            | NativeMeanState::ApplyPlusFirst
            | NativeMeanState::ApplyDivFirst
            | NativeMeanState::ApplyDivSecond => false,
            NativeMeanState::ApplyPlusSecond => true,
        }
    }
}

impl NativeMean<Pointer> {
    fn apply_plus_first<'scope>(
        &self,
        scope: &mut RootScope<'_, 'scope>,
    ) -> Result<NativeStep, EngineError> {
        let plus_ty = binary_same_type(&self.elem_type);
        let acc = self
            .acc
            .ok_or_else(|| EngineError::Internal("native mean missing accumulator".into()))?;
        let acc = scope.root(acc);
        let plus = overloaded_root(scope, "+", plus_ty.clone())?;
        native_apply_step(
            scope.pointer(plus),
            plus_ty,
            scope.pointer(acc),
            self.elem_type.clone(),
        )
    }

    fn apply_plus_second<'scope>(
        &self,
        scope: &mut RootScope<'_, 'scope>,
    ) -> Result<NativeStep, EngineError> {
        let step = self
            .step
            .ok_or_else(|| EngineError::Internal("native mean missing addition step".into()))?;
        let step_ty = Type::fun(self.elem_type.clone(), self.elem_type.clone());
        let arg = self.values.get(scope, self.next_index)?;
        native_apply_step(step, step_ty, scope.pointer(arg), self.elem_type.clone())
    }

    fn apply_div_first<'scope>(
        &mut self,
        scope: &mut RootScope<'_, 'scope>,
    ) -> Result<NativeStep, EngineError> {
        let div_ty = binary_same_type(&self.elem_type);
        let acc = self
            .acc
            .ok_or_else(|| EngineError::Internal("native mean missing accumulator".into()))?;
        let acc = scope.root(acc);
        let div = overloaded_root(scope, "/", div_ty.clone())?;
        if self.len_value.is_none() {
            let root = len_value_for_native_type(scope, &self.elem_type, self.len)?;
            let len_value = scope.pointer(root);
            self.len_value = Some(len_value);
        }
        native_apply_step(
            scope.pointer(div),
            div_ty,
            scope.pointer(acc),
            self.elem_type.clone(),
        )
    }

    fn apply_div_second(&self) -> Result<NativeStep, EngineError> {
        let step = self
            .step
            .ok_or_else(|| EngineError::Internal("native mean missing division step".into()))?;
        let len_value = self
            .len_value
            .ok_or_else(|| EngineError::Internal("native mean missing length value".into()))?;
        let step_ty = Type::fun(self.elem_type.clone(), self.elem_type.clone());
        native_apply_step(step, step_ty, len_value, self.elem_type.clone())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum NativeSequenceShape {
    List,
}

fn alloc_native_sequence<'scope>(
    scope: &mut RootScope<'_, 'scope>,
    shape: &NativeSequenceShape,
    values: Vec<RootedPtr<'scope>>,
) -> Result<RootedPtr<'scope>, EngineError> {
    match shape {
        NativeSequenceShape::List => scope.alloc_root_list(values),
    }
}

fn native_flatten_sequence<'scope>(
    scope: &mut RootScope<'_, 'scope>,
    shape: &NativeSequenceShape,
    root: RootedPtr<'scope>,
) -> Result<Vec<RootedPtr<'scope>>, EngineError> {
    match shape {
        NativeSequenceShape::List => scope.root_as_list(root),
    }
}

fn option_value_ptr<'scope>(
    scope: &mut RootScope<'_, 'scope>,
    pointer: RootedPtr<'scope>,
) -> Result<Option<RootedPtr<'scope>>, EngineError> {
    let (tag, args) = scope.root_as_adt(pointer)?;
    if tag.as_ref() == "Some" && args.len() == 1 {
        Ok(Some(args[0]))
    } else if tag.as_ref() == "None" && args.is_empty() {
        Ok(None)
    } else {
        Err(EngineError::NativeType {
            expected: "Option".into(),
            got: scope.type_name(pointer)?.into(),
        })
    }
}

fn option_from_native_pointer<'scope>(
    scope: &mut RootScope<'_, 'scope>,
    value: Option<RootedPtr<'scope>>,
) -> Result<RootedPtr<'scope>, EngineError> {
    match value {
        Some(value) => scope.alloc_root_adt(Symbol::intern("Some"), vec![value]),
        None => scope.alloc_root_adt(Symbol::intern("None"), vec![]),
    }
}

fn result_from_native_pointer<'scope>(
    scope: &mut RootScope<'_, 'scope>,
    value: Result<RootedPtr<'scope>, RootedPtr<'scope>>,
) -> Result<RootedPtr<'scope>, EngineError> {
    match value {
        Ok(value) => scope.alloc_root_adt(Symbol::intern("Ok"), vec![value]),
        Err(value) => scope.alloc_root_adt(Symbol::intern("Err"), vec![value]),
    }
}

fn result_value_ptr<'scope>(
    scope: &mut RootScope<'_, 'scope>,
    pointer: RootedPtr<'scope>,
) -> Result<Result<RootedPtr<'scope>, RootedPtr<'scope>>, EngineError> {
    let (tag, args) = scope.root_as_adt(pointer)?;
    if tag.as_ref() == "Ok" && args.len() == 1 {
        Ok(Ok(args[0]))
    } else if tag.as_ref() == "Err" && args.len() == 1 {
        Ok(Err(args[0]))
    } else {
        Err(EngineError::NativeType {
            expected: "Result".into(),
            got: scope.type_name(pointer)?.into(),
        })
    }
}

fn overloaded_root<'scope>(
    scope: &mut RootScope<'_, 'scope>,
    name: &str,
    typ: Type,
) -> Result<RootedPtr<'scope>, EngineError> {
    let (name, typ, applied, applied_types) =
        OverloadedFn::new(Symbol::intern(name), typ).into_parts();
    let applied = applied.into_iter().map(|x| scope.root(x)).collect();
    scope.alloc_root_overloaded(name, typ, applied, applied_types)
}

fn native_eval_var_step(name: Symbol, typ: Type) -> NativeStep {
    NativeStep::Push {
        expr: Arc::new(TypedExpr::new(
            typ,
            TypedExprKind::Var {
                name,
                overloads: Vec::new(),
            },
        )),
        env: Environment::new(),
    }
}

fn binary_same_type(typ: &Type) -> Type {
    Type::fun(typ.clone(), Type::fun(typ.clone(), typ.clone()))
}

fn len_value_for_native_type<'scope>(
    scope: &mut RootScope<'_, 'scope>,
    elem_ty: &Type,
    len: usize,
) -> Result<RootedPtr<'scope>, EngineError> {
    match elem_ty.as_ref() {
        TypeKind::Con(c) if c.is_builtin(BuiltinTypeId::F32) => scope.alloc_root_f32(len as f32),
        TypeKind::Con(c) if c.is_builtin(BuiltinTypeId::F64) => scope.alloc_root_f64(len as f64),
        _ => Err(EngineError::NativeType {
            expected: "f32 or f64".into(),
            got: elem_ty.to_string(),
        }),
    }
}

fn refresh_native_frame_from_roots(
    heap: &HeapState,
    frame: &mut FrNativeCall<Pointer>,
    originals: &[Pointer],
    roots: &TempRoots,
    start: usize,
) -> Result<(), EngineError> {
    let mut wrapped = Frame::NativeCall(frame.clone());
    refresh_frame_from_roots(heap, &mut wrapped, originals, roots, start)?;
    match wrapped {
        Frame::NativeCall(rewritten) => {
            *frame = rewritten;
            Ok(())
        }
        _ => frame_kind_error("native call"),
    }
}

fn synthetic_application_expr(
    func: Pointer,
    func_type: Type,
    args: &[(Pointer, Type)],
) -> Result<(Environment, TypedExpr), EngineError> {
    let func_name = Symbol::intern("__rex_apply_func");
    let mut env = Environment::new().extend(func_name.clone(), func);
    let mut expr = TypedExpr::new(
        func_type.clone(),
        TypedExprKind::Var {
            name: func_name,
            overloads: Vec::new(),
        },
    );
    let mut cur_type = func_type;

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

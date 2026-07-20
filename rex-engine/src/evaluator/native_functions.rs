use crate::{
    env::Environment,
    error::EngineError,
    evaluator::{
        application_result_type,
        eval::{
            EvalControl, frame_for_expr, frame_kind_error, refresh_frame_from_roots,
            unexpected_child_result,
        },
        runtime_core::RuntimeCore,
    },
    overloaded_fn::OverloadedFn,
    stack::{
        FrNativeCall, FrNativeCallState, Frame, NativeUnaryShape, rewrite_entries,
        rewrite_map_values, rewrite_option, rewrite_pointer, rewrite_slice, trace_option,
    },
    value::{Collection, ListItems, Pointer, TempRoots},
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

fn native_step_to_control<State>(
    runtime: &RuntimeCore<State>,
    frame_ptr: Pointer,
    mut frame: FrNativeCall,
    step: NativeStep,
) -> Result<EvalControl<State>, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    match step {
        NativeStep::Wait => {
            runtime
                .heap
                .with_locked(|heap| heap.replace_frame(&frame_ptr, Frame::NativeCall(frame)))?;
            Ok(EvalControl::Wait)
        }
        NativeStep::Return(value) => Ok(EvalControl::Return(value)),
        NativeStep::Push { expr, env } => {
            frame.state = FrNativeCallState::Waiting;
            runtime
                .heap
                .with_locked(|heap| heap.replace_frame(&frame_ptr, Frame::NativeCall(frame)))?;
            Ok(EvalControl::Push { expr, env })
        }
        NativeStep::Schedule(child_count) => {
            frame.state = FrNativeCallState::Waiting;
            runtime
                .heap
                .with_locked(|heap| heap.replace_frame(&frame_ptr, Frame::NativeCall(frame)))?;

            let roots = runtime.heap.temp_roots(vec![frame_ptr])?;
            for index in 0..child_count {
                let current_frame_ptr = roots.get(0)?;
                let current_frame = match runtime
                    .heap
                    .with_locked(|heap| heap.pointer_as_frame(&current_frame_ptr))?
                {
                    Frame::NativeCall(frame) => frame,
                    _ => return frame_kind_error("native call"),
                };
                let child_spec = current_frame.task.scheduled_child_spec(runtime, index)?;
                let current_frame_ptr = roots.get(0)?;
                let frame = frame_for_expr(current_frame_ptr, child_spec.expr, child_spec.env);
                let child = runtime
                    .heap
                    .with_locked(|heap| Ok(heap.alloc_ptr_frame(frame)?.into_pointer()))?;
                let current_frame_ptr = roots.get(0)?;
                let mut current_frame = match runtime
                    .heap
                    .with_locked(|heap| heap.pointer_as_frame(&current_frame_ptr))?
                {
                    Frame::NativeCall(frame) => frame,
                    _ => return frame_kind_error("native call"),
                };
                current_frame.task.push_scheduled_child(child)?;
                runtime.heap.with_locked(|heap| {
                    heap.replace_frame(&current_frame_ptr, Frame::NativeCall(current_frame))
                })?;
            }

            let current_frame_ptr = roots.get(0)?;
            let current_frame = match runtime
                .heap
                .with_locked(|heap| heap.pointer_as_frame(&current_frame_ptr))?
            {
                Frame::NativeCall(frame) => frame,
                _ => return frame_kind_error("native call"),
            };
            Ok(EvalControl::Schedule(
                current_frame.task.scheduled_children()?,
            ))
        }
    }
}

pub(crate) fn eval_native_enter<State>(
    runtime: &RuntimeCore<State>,
    frame_ptr: Pointer,
    mut frame: FrNativeCall,
) -> Result<EvalControl<State>, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    if frame.state != FrNativeCallState::Enter {
        return unexpected_child_result("native call");
    }
    let mut protected = vec![frame_ptr];
    Frame::NativeCall(frame.clone()).trace_pointers(&mut protected);
    let roots = runtime.heap.temp_roots(protected.clone())?;
    let step = frame.task.enter(runtime)?;
    let frame_ptr = roots.get(0)?;
    refresh_native_frame_from_roots(&mut frame, &protected, &roots, 1)?;
    native_step_to_control(runtime, frame_ptr, frame, step)
}

pub(crate) fn eval_native_receive<State>(
    runtime: &RuntimeCore<State>,
    frame_ptr: Pointer,
    mut frame: FrNativeCall,
    child: Pointer,
    value: Pointer,
) -> Result<EvalControl<State>, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    if frame.state != FrNativeCallState::Waiting {
        return unexpected_child_result("native call");
    }
    if !<NativeTask as Coroutine<State>>::receive_may_allocate(&frame.task) {
        let step = frame.task.receive(runtime, child, value)?;
        return native_step_to_control(runtime, frame_ptr, frame, step);
    }

    let mut protected = vec![frame_ptr, child, value];
    Frame::NativeCall(frame.clone()).trace_pointers(&mut protected);
    let roots = runtime.heap.temp_roots(protected.clone())?;
    let step = frame.task.receive(runtime, child, value)?;
    let frame_ptr = roots.get(0)?;
    refresh_native_frame_from_roots(&mut frame, &protected, &roots, 1)?;
    native_step_to_control(runtime, frame_ptr, frame, step)
}

#[derive(Clone, Debug, PartialEq)]
pub enum NativeTask {
    ApplyUnary(NativeApplyUnary),
    SequenceMap(NativeSequenceMap),
    SequenceFilter(NativeSequenceFilter),
    SequenceFilterMap(NativeSequenceFilterMap),
    SequenceFlatMap(NativeSequenceFlatMap),
    UnaryMap(NativeUnaryMap),
    UnaryFilter(NativeUnaryFilter),
    UnaryFilterMap(NativeUnaryFilterMap),
    UnaryFlatMap(NativeUnaryFlatMap),
    Fold(NativeFold),
    DictMap(NativeDictMap),
    DictTraverse(NativeDictTraverse),
    ArrayEq(NativeArrayEq),
    Sum(NativeSum),
    Mean(NativeMean),
}

impl Collection for NativeTask {
    fn trace_pointers(&self, out: &mut Vec<Pointer>) {
        match self {
            NativeTask::ApplyUnary(task) => task.trace_pointers(out),
            NativeTask::SequenceMap(task) => task.trace_pointers(out),
            NativeTask::SequenceFilter(task) => task.trace_pointers(out),
            NativeTask::SequenceFilterMap(task) => task.trace_pointers(out),
            NativeTask::SequenceFlatMap(task) => task.trace_pointers(out),
            NativeTask::UnaryMap(task) => task.trace_pointers(out),
            NativeTask::UnaryFilter(task) => task.trace_pointers(out),
            NativeTask::UnaryFilterMap(task) => task.trace_pointers(out),
            NativeTask::UnaryFlatMap(task) => task.trace_pointers(out),
            NativeTask::Fold(task) => task.trace_pointers(out),
            NativeTask::DictMap(task) => task.trace_pointers(out),
            NativeTask::DictTraverse(task) => task.trace_pointers(out),
            NativeTask::ArrayEq(task) => task.trace_pointers(out),
            NativeTask::Sum(task) => task.trace_pointers(out),
            NativeTask::Mean(task) => task.trace_pointers(out),
        }
    }

    fn rewrite_pointers(
        &mut self,
        rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, EngineError>,
    ) -> Result<(), EngineError> {
        match self {
            NativeTask::ApplyUnary(task) => task.rewrite_pointers(rewrite),
            NativeTask::SequenceMap(task) => task.rewrite_pointers(rewrite),
            NativeTask::SequenceFilter(task) => task.rewrite_pointers(rewrite),
            NativeTask::SequenceFilterMap(task) => task.rewrite_pointers(rewrite),
            NativeTask::SequenceFlatMap(task) => task.rewrite_pointers(rewrite),
            NativeTask::UnaryMap(task) => task.rewrite_pointers(rewrite),
            NativeTask::UnaryFilter(task) => task.rewrite_pointers(rewrite),
            NativeTask::UnaryFilterMap(task) => task.rewrite_pointers(rewrite),
            NativeTask::UnaryFlatMap(task) => task.rewrite_pointers(rewrite),
            NativeTask::Fold(task) => task.rewrite_pointers(rewrite),
            NativeTask::DictMap(task) => task.rewrite_pointers(rewrite),
            NativeTask::DictTraverse(task) => task.rewrite_pointers(rewrite),
            NativeTask::ArrayEq(task) => task.rewrite_pointers(rewrite),
            NativeTask::Sum(task) => task.rewrite_pointers(rewrite),
            NativeTask::Mean(task) => task.rewrite_pointers(rewrite),
        }
    }
}

impl NativeTask {
    fn push_scheduled_child(&mut self, child: Pointer) -> Result<(), EngineError> {
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

    fn scheduled_children(&self) -> Result<Vec<Pointer>, EngineError> {
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

    fn scheduled_child_spec<State>(
        &self,
        runtime: &RuntimeCore<State>,
        index: usize,
    ) -> Result<NativeChildSpec, EngineError>
    where
        State: Clone + Send + Sync + 'static,
    {
        match self {
            NativeTask::SequenceMap(task) => task.child_spec(runtime, index),
            NativeTask::SequenceFilter(task) => task.child_spec(runtime, index),
            NativeTask::SequenceFilterMap(task) => task.child_spec(runtime, index),
            NativeTask::SequenceFlatMap(task) => task.child_spec(runtime, index),
            NativeTask::DictMap(task) => task.child_spec(index),
            _ => Err(EngineError::Internal(
                "native task does not have scheduled child specs".into(),
            )),
        }
    }
}

impl<State> Coroutine<State> for NativeTask
where
    State: Clone + Send + Sync + 'static,
{
    fn enter(&mut self, runtime: &RuntimeCore<State>) -> Result<NativeStep, EngineError>
    where
        State: Clone + Send + Sync + 'static,
    {
        match self {
            NativeTask::ApplyUnary(task) => task.enter(runtime),
            NativeTask::SequenceMap(task) => task.enter(runtime),
            NativeTask::SequenceFilter(task) => task.enter(runtime),
            NativeTask::SequenceFilterMap(task) => task.enter(runtime),
            NativeTask::SequenceFlatMap(task) => task.enter(runtime),
            NativeTask::UnaryMap(task) => task.enter(runtime),
            NativeTask::UnaryFilter(task) => task.enter(runtime),
            NativeTask::UnaryFilterMap(task) => task.enter(runtime),
            NativeTask::UnaryFlatMap(task) => task.enter(runtime),
            NativeTask::Fold(task) => task.enter(runtime),
            NativeTask::DictMap(task) => task.enter(runtime),
            NativeTask::DictTraverse(task) => task.enter(runtime),
            NativeTask::ArrayEq(task) => task.enter(runtime),
            NativeTask::Sum(task) => task.enter(runtime),
            NativeTask::Mean(task) => task.enter(runtime),
        }
    }

    fn receive(
        &mut self,
        runtime: &RuntimeCore<State>,
        child: Pointer,
        value: Pointer,
    ) -> Result<NativeStep, EngineError>
    where
        State: Clone + Send + Sync + 'static,
    {
        match self {
            NativeTask::ApplyUnary(task) => task.receive(runtime, child, value),
            NativeTask::SequenceMap(task) => task.receive(runtime, child, value),
            NativeTask::SequenceFilter(task) => task.receive(runtime, child, value),
            NativeTask::SequenceFilterMap(task) => task.receive(runtime, child, value),
            NativeTask::SequenceFlatMap(task) => task.receive(runtime, child, value),
            NativeTask::UnaryMap(task) => task.receive(runtime, child, value),
            NativeTask::UnaryFilter(task) => task.receive(runtime, child, value),
            NativeTask::UnaryFilterMap(task) => task.receive(runtime, child, value),
            NativeTask::UnaryFlatMap(task) => task.receive(runtime, child, value),
            NativeTask::Fold(task) => task.receive(runtime, child, value),
            NativeTask::DictMap(task) => task.receive(runtime, child, value),
            NativeTask::DictTraverse(task) => task.receive(runtime, child, value),
            NativeTask::ArrayEq(task) => task.receive(runtime, child, value),
            NativeTask::Sum(task) => task.receive(runtime, child, value),
            NativeTask::Mean(task) => task.receive(runtime, child, value),
        }
    }

    fn receive_may_allocate(&self) -> bool {
        match self {
            NativeTask::ApplyUnary(task) => {
                <NativeApplyUnary as Coroutine<State>>::receive_may_allocate(task)
            }
            NativeTask::SequenceMap(task) => {
                <NativeSequenceMap as Coroutine<State>>::receive_may_allocate(task)
            }
            NativeTask::SequenceFilter(task) => {
                <NativeSequenceFilter as Coroutine<State>>::receive_may_allocate(task)
            }
            NativeTask::SequenceFilterMap(task) => {
                <NativeSequenceFilterMap as Coroutine<State>>::receive_may_allocate(task)
            }
            NativeTask::SequenceFlatMap(task) => {
                <NativeSequenceFlatMap as Coroutine<State>>::receive_may_allocate(task)
            }
            NativeTask::UnaryMap(task) => {
                <NativeUnaryMap as Coroutine<State>>::receive_may_allocate(task)
            }
            NativeTask::UnaryFilter(task) => {
                <NativeUnaryFilter as Coroutine<State>>::receive_may_allocate(task)
            }
            NativeTask::UnaryFilterMap(task) => {
                <NativeUnaryFilterMap as Coroutine<State>>::receive_may_allocate(task)
            }
            NativeTask::UnaryFlatMap(task) => {
                <NativeUnaryFlatMap as Coroutine<State>>::receive_may_allocate(task)
            }
            NativeTask::Fold(task) => <NativeFold as Coroutine<State>>::receive_may_allocate(task),
            NativeTask::DictMap(task) => {
                <NativeDictMap as Coroutine<State>>::receive_may_allocate(task)
            }
            NativeTask::DictTraverse(task) => {
                <NativeDictTraverse as Coroutine<State>>::receive_may_allocate(task)
            }
            NativeTask::ArrayEq(task) => {
                <NativeArrayEq as Coroutine<State>>::receive_may_allocate(task)
            }
            NativeTask::Sum(task) => <NativeSum as Coroutine<State>>::receive_may_allocate(task),
            NativeTask::Mean(task) => <NativeMean as Coroutine<State>>::receive_may_allocate(task),
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

fn rewrite_options(
    values: &mut [Option<Pointer>],
    rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, EngineError>,
) -> Result<(), EngineError> {
    for value in values.iter_mut().flatten() {
        rewrite_pointer(value, rewrite)?;
    }
    Ok(())
}

fn rewrite_nested_options(
    values: &mut [Option<Option<Pointer>>],
    rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, EngineError>,
) -> Result<(), EngineError> {
    for value in values.iter_mut().filter_map(Option::as_mut).flatten() {
        rewrite_pointer(value, rewrite)?;
    }
    Ok(())
}

fn rewrite_option_vecs(
    values: &mut [Option<Vec<Pointer>>],
    rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, EngineError>,
) -> Result<(), EngineError> {
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

pub(crate) trait Coroutine<State>
where
    State: Clone + Send + Sync + 'static,
{
    fn enter(&mut self, runtime: &RuntimeCore<State>) -> Result<NativeStep, EngineError>;

    fn receive(
        &mut self,
        runtime: &RuntimeCore<State>,
        child: Pointer,
        value: Pointer,
    ) -> Result<NativeStep, EngineError>;

    fn receive_may_allocate(&self) -> bool;
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeApplyUnary {
    pub func: Pointer,
    pub func_type: Type,
    pub arg: Pointer,
    pub arg_type: Type,
}

impl Collection for NativeApplyUnary {
    fn trace_pointers(&self, out: &mut Vec<Pointer>) {
        out.push(self.func);
        out.push(self.arg);
    }

    fn rewrite_pointers(
        &mut self,
        rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, EngineError>,
    ) -> Result<(), EngineError> {
        rewrite_pointer(&mut self.func, rewrite)?;
        rewrite_pointer(&mut self.arg, rewrite)
    }
}

impl<State> Coroutine<State> for NativeApplyUnary
where
    State: Clone + Send + Sync + 'static,
{
    fn enter(&mut self, _runtime: &RuntimeCore<State>) -> Result<NativeStep, EngineError> {
        native_apply_step(
            self.func,
            self.func_type.clone(),
            self.arg,
            self.arg_type.clone(),
        )
    }

    fn receive(
        &mut self,
        _runtime: &RuntimeCore<State>,
        _child: Pointer,
        value: Pointer,
    ) -> Result<NativeStep, EngineError> {
        Ok(NativeStep::Return(value))
    }

    fn receive_may_allocate(&self) -> bool {
        false
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NativeSequenceMap {
    pub func: Pointer,
    pub func_type: Type,
    pub elem_type: Type,
    pub values: ListItems,
    pub shape: NativeSequenceShape,
    pub children: Vec<Pointer>,
    pub output: Vec<Option<Pointer>>,
    pub remaining: usize,
}

impl Collection for NativeSequenceMap {
    fn trace_pointers(&self, out: &mut Vec<Pointer>) {
        out.push(self.func);
        self.values.trace_pointers(out);
        out.extend(self.children.iter().copied());
        out.extend(self.output.iter().flatten().copied());
    }

    fn rewrite_pointers(
        &mut self,
        rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, EngineError>,
    ) -> Result<(), EngineError> {
        rewrite_pointer(&mut self.func, rewrite)?;
        self.values.rewrite_pointers(rewrite)?;
        rewrite_slice(&mut self.children, rewrite)?;
        rewrite_options(&mut self.output, rewrite)
    }
}
impl<State> Coroutine<State> for NativeSequenceMap
where
    State: Clone + Send + Sync + 'static,
{
    fn enter(&mut self, runtime: &RuntimeCore<State>) -> Result<NativeStep, EngineError>
    where
        State: Clone + Send + Sync + 'static,
    {
        if self.values.is_empty() {
            return Ok(NativeStep::Return(alloc_native_sequence(
                runtime,
                &self.shape,
                Vec::new(),
            )?));
        }
        self.children.clear();
        self.output = vec![None; self.values.len()];
        self.remaining = self.values.len();
        Ok(NativeStep::Schedule(self.values.len()))
    }

    fn receive(
        &mut self,
        runtime: &RuntimeCore<State>,
        child: Pointer,
        value: Pointer,
    ) -> Result<NativeStep, EngineError>
    where
        State: Clone + Send + Sync + 'static,
    {
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
            return Ok(NativeStep::Return(alloc_native_sequence(
                runtime,
                &self.shape,
                output,
            )?));
        }
        Ok(NativeStep::Wait)
    }

    fn receive_may_allocate(&self) -> bool {
        self.remaining == 1
    }
}

impl NativeSequenceMap {
    fn child_spec<State>(
        &self,
        runtime: &RuntimeCore<State>,
        index: usize,
    ) -> Result<NativeChildSpec, EngineError>
    where
        State: Clone + Send + Sync + 'static,
    {
        let value = runtime
            .heap
            .with_locked(|heap| self.values.get(heap, index))?;
        native_apply_spec(
            self.func,
            self.func_type.clone(),
            value,
            self.elem_type.clone(),
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NativeSequenceFilter {
    pub func: Pointer,
    pub func_type: Type,
    pub elem_type: Type,
    pub values: ListItems,
    pub shape: NativeSequenceShape,
    pub children: Vec<Pointer>,
    pub keep: Vec<Option<bool>>,
    pub remaining: usize,
}

impl Collection for NativeSequenceFilter {
    fn trace_pointers(&self, out: &mut Vec<Pointer>) {
        out.push(self.func);
        self.values.trace_pointers(out);
        out.extend(self.children.iter().copied());
    }

    fn rewrite_pointers(
        &mut self,
        rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, EngineError>,
    ) -> Result<(), EngineError> {
        rewrite_pointer(&mut self.func, rewrite)?;
        self.values.rewrite_pointers(rewrite)?;
        rewrite_slice(&mut self.children, rewrite)
    }
}

impl<State> Coroutine<State> for NativeSequenceFilter
where
    State: Clone + Send + Sync + 'static,
{
    fn enter(&mut self, runtime: &RuntimeCore<State>) -> Result<NativeStep, EngineError>
    where
        State: Clone + Send + Sync + 'static,
    {
        if self.values.is_empty() {
            return Ok(NativeStep::Return(alloc_native_sequence(
                runtime,
                &self.shape,
                Vec::new(),
            )?));
        }
        self.children.clear();
        self.keep = vec![None; self.values.len()];
        self.remaining = self.values.len();
        Ok(NativeStep::Schedule(self.values.len()))
    }

    fn receive(
        &mut self,
        runtime: &RuntimeCore<State>,
        child: Pointer,
        value: Pointer,
    ) -> Result<NativeStep, EngineError>
    where
        State: Clone + Send + Sync + 'static,
    {
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
        *slot = Some(
            runtime
                .heap
                .with_locked(|heap| heap.pointer_as_bool(&value))?,
        );
        self.remaining = self.remaining.checked_sub(1).ok_or_else(|| {
            EngineError::Internal("native sequence filter received too many results".into())
        })?;
        if self.remaining == 0 {
            let mut output = Vec::new();
            for (index, keep) in self.keep.iter().enumerate() {
                match keep {
                    Some(true) => output.push(
                        runtime
                            .heap
                            .with_locked(|heap| self.values.get(heap, index))?,
                    ),
                    Some(false) => {}
                    None => {
                        return Err(EngineError::Internal(
                            "native sequence filter completed with missing result".into(),
                        ));
                    }
                }
            }
            return Ok(NativeStep::Return(alloc_native_sequence(
                runtime,
                &self.shape,
                output,
            )?));
        }
        Ok(NativeStep::Wait)
    }

    fn receive_may_allocate(&self) -> bool {
        self.remaining == 1
    }
}

impl NativeSequenceFilter {
    fn child_spec<State>(
        &self,
        runtime: &RuntimeCore<State>,
        index: usize,
    ) -> Result<NativeChildSpec, EngineError>
    where
        State: Clone + Send + Sync + 'static,
    {
        let value = runtime
            .heap
            .with_locked(|heap| self.values.get(heap, index))?;
        native_apply_spec(
            self.func,
            self.func_type.clone(),
            value,
            self.elem_type.clone(),
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NativeSequenceFilterMap {
    pub func: Pointer,
    pub func_type: Type,
    pub elem_type: Type,
    pub values: ListItems,
    pub shape: NativeSequenceShape,
    pub children: Vec<Pointer>,
    pub output: Vec<Option<Option<Pointer>>>,
    pub remaining: usize,
}

impl Collection for NativeSequenceFilterMap {
    fn trace_pointers(&self, out: &mut Vec<Pointer>) {
        out.push(self.func);
        self.values.trace_pointers(out);
        out.extend(self.children.iter().copied());
        out.extend(
            self.output
                .iter()
                .filter_map(Option::as_ref)
                .flatten()
                .copied(),
        );
    }

    fn rewrite_pointers(
        &mut self,
        rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, EngineError>,
    ) -> Result<(), EngineError> {
        rewrite_pointer(&mut self.func, rewrite)?;
        self.values.rewrite_pointers(rewrite)?;
        rewrite_slice(&mut self.children, rewrite)?;
        rewrite_nested_options(&mut self.output, rewrite)
    }
}

impl<State> Coroutine<State> for NativeSequenceFilterMap
where
    State: Clone + Send + Sync + 'static,
{
    fn enter(&mut self, runtime: &RuntimeCore<State>) -> Result<NativeStep, EngineError>
    where
        State: Clone + Send + Sync + 'static,
    {
        if self.values.is_empty() {
            return Ok(NativeStep::Return(alloc_native_sequence(
                runtime,
                &self.shape,
                Vec::new(),
            )?));
        }
        self.children.clear();
        self.output = vec![None; self.values.len()];
        self.remaining = self.values.len();
        Ok(NativeStep::Schedule(self.values.len()))
    }

    fn receive(
        &mut self,
        runtime: &RuntimeCore<State>,
        child: Pointer,
        value: Pointer,
    ) -> Result<NativeStep, EngineError>
    where
        State: Clone + Send + Sync + 'static,
    {
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
        *slot = Some(option_value_ptr(runtime, value)?);
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
            return Ok(NativeStep::Return(alloc_native_sequence(
                runtime,
                &self.shape,
                output,
            )?));
        }
        Ok(NativeStep::Wait)
    }

    fn receive_may_allocate(&self) -> bool {
        self.remaining == 1
    }
}

impl NativeSequenceFilterMap {
    fn child_spec<State>(
        &self,
        runtime: &RuntimeCore<State>,
        index: usize,
    ) -> Result<NativeChildSpec, EngineError>
    where
        State: Clone + Send + Sync + 'static,
    {
        let value = runtime
            .heap
            .with_locked(|heap| self.values.get(heap, index))?;
        native_apply_spec(
            self.func,
            self.func_type.clone(),
            value,
            self.elem_type.clone(),
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NativeSequenceFlatMap {
    pub func: Pointer,
    pub func_type: Type,
    pub elem_type: Type,
    pub values: ListItems,
    pub shape: NativeSequenceShape,
    pub children: Vec<Pointer>,
    pub output: Vec<Option<Vec<Pointer>>>,
    pub remaining: usize,
}

impl Collection for NativeSequenceFlatMap {
    fn trace_pointers(&self, out: &mut Vec<Pointer>) {
        out.push(self.func);
        self.values.trace_pointers(out);
        out.extend(self.children.iter().copied());
        out.extend(self.output.iter().flatten().flatten().copied());
    }

    fn rewrite_pointers(
        &mut self,
        rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, EngineError>,
    ) -> Result<(), EngineError> {
        rewrite_pointer(&mut self.func, rewrite)?;
        self.values.rewrite_pointers(rewrite)?;
        rewrite_slice(&mut self.children, rewrite)?;
        rewrite_option_vecs(&mut self.output, rewrite)
    }
}

impl<State> Coroutine<State> for NativeSequenceFlatMap
where
    State: Clone + Send + Sync + 'static,
{
    fn enter(&mut self, runtime: &RuntimeCore<State>) -> Result<NativeStep, EngineError>
    where
        State: Clone + Send + Sync + 'static,
    {
        if self.values.is_empty() {
            return Ok(NativeStep::Return(alloc_native_sequence(
                runtime,
                &self.shape,
                Vec::new(),
            )?));
        }
        self.children.clear();
        self.output = vec![None; self.values.len()];
        self.remaining = self.values.len();
        Ok(NativeStep::Schedule(self.values.len()))
    }

    fn receive(
        &mut self,
        runtime: &RuntimeCore<State>,
        child: Pointer,
        value: Pointer,
    ) -> Result<NativeStep, EngineError>
    where
        State: Clone + Send + Sync + 'static,
    {
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
        *slot = Some(native_flatten_sequence(runtime, &self.shape, value)?);
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
            return Ok(NativeStep::Return(alloc_native_sequence(
                runtime,
                &self.shape,
                output,
            )?));
        }
        Ok(NativeStep::Wait)
    }

    fn receive_may_allocate(&self) -> bool {
        self.remaining == 1
    }
}

impl NativeSequenceFlatMap {
    fn child_spec<State>(
        &self,
        runtime: &RuntimeCore<State>,
        index: usize,
    ) -> Result<NativeChildSpec, EngineError>
    where
        State: Clone + Send + Sync + 'static,
    {
        let value = runtime
            .heap
            .with_locked(|heap| self.values.get(heap, index))?;
        native_apply_spec(
            self.func,
            self.func_type.clone(),
            value,
            self.elem_type.clone(),
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NativeUnaryMap {
    pub func: Pointer,
    pub func_type: Type,
    pub elem_type: Type,
    pub value: Pointer,
    pub shape: NativeUnaryShape,
}

impl Collection for NativeUnaryMap {
    fn trace_pointers(&self, out: &mut Vec<Pointer>) {
        out.push(self.func);
        out.push(self.value);
    }

    fn rewrite_pointers(
        &mut self,
        rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, EngineError>,
    ) -> Result<(), EngineError> {
        rewrite_pointer(&mut self.func, rewrite)?;
        rewrite_pointer(&mut self.value, rewrite)
    }
}

impl<State> Coroutine<State> for NativeUnaryMap
where
    State: Clone + Send + Sync + 'static,
{
    fn enter(&mut self, _runtime: &RuntimeCore<State>) -> Result<NativeStep, EngineError> {
        native_apply_step(
            self.func,
            self.func_type.clone(),
            self.value,
            self.elem_type.clone(),
        )
    }

    fn receive(
        &mut self,
        runtime: &RuntimeCore<State>,
        _child: Pointer,
        value: Pointer,
    ) -> Result<NativeStep, EngineError>
    where
        State: Clone + Send + Sync + 'static,
    {
        let value = match &self.shape {
            NativeUnaryShape::Option => option_from_native_pointer(runtime, Some(value))?,
            NativeUnaryShape::Result => result_from_native_pointer(runtime, Ok(value))?,
        };
        Ok(NativeStep::Return(value))
    }

    fn receive_may_allocate(&self) -> bool {
        true
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NativeUnaryFilter {
    pub func: Pointer,
    pub func_type: Type,
    pub elem_type: Type,
    pub value: Pointer,
    pub original: Pointer,
}

impl Collection for NativeUnaryFilter {
    fn trace_pointers(&self, out: &mut Vec<Pointer>) {
        out.push(self.func);
        out.push(self.value);
        out.push(self.original);
    }

    fn rewrite_pointers(
        &mut self,
        rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, EngineError>,
    ) -> Result<(), EngineError> {
        rewrite_pointer(&mut self.func, rewrite)?;
        rewrite_pointer(&mut self.value, rewrite)?;
        rewrite_pointer(&mut self.original, rewrite)
    }
}

impl<State> Coroutine<State> for NativeUnaryFilter
where
    State: Clone + Send + Sync + 'static,
{
    fn enter(&mut self, _runtime: &RuntimeCore<State>) -> Result<NativeStep, EngineError> {
        native_apply_step(
            self.func,
            self.func_type.clone(),
            self.value,
            self.elem_type.clone(),
        )
    }

    fn receive(
        &mut self,
        runtime: &RuntimeCore<State>,
        _child: Pointer,
        value: Pointer,
    ) -> Result<NativeStep, EngineError>
    where
        State: Clone + Send + Sync + 'static,
    {
        let value = if runtime
            .heap
            .with_locked(|heap| heap.pointer_as_bool(&value))?
        {
            self.original
        } else {
            option_from_native_pointer(runtime, None)?
        };
        Ok(NativeStep::Return(value))
    }

    fn receive_may_allocate(&self) -> bool {
        true
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeUnaryFilterMap {
    pub func: Pointer,
    pub func_type: Type,
    pub elem_type: Type,
    pub value: Pointer,
}

impl Collection for NativeUnaryFilterMap {
    fn trace_pointers(&self, out: &mut Vec<Pointer>) {
        out.push(self.func);
        out.push(self.value);
    }

    fn rewrite_pointers(
        &mut self,
        rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, EngineError>,
    ) -> Result<(), EngineError> {
        rewrite_pointer(&mut self.func, rewrite)?;
        rewrite_pointer(&mut self.value, rewrite)
    }
}

impl<State> Coroutine<State> for NativeUnaryFilterMap
where
    State: Clone + Send + Sync + 'static,
{
    fn enter(&mut self, _runtime: &RuntimeCore<State>) -> Result<NativeStep, EngineError> {
        native_apply_step(
            self.func,
            self.func_type.clone(),
            self.value,
            self.elem_type.clone(),
        )
    }

    fn receive(
        &mut self,
        _runtime: &RuntimeCore<State>,
        _child: Pointer,
        value: Pointer,
    ) -> Result<NativeStep, EngineError> {
        Ok(NativeStep::Return(value))
    }

    fn receive_may_allocate(&self) -> bool {
        false
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NativeUnaryFlatMap {
    pub func: Pointer,
    pub func_type: Type,
    pub elem_type: Type,
    pub value: Pointer,
    pub shape: NativeUnaryShape,
}

impl Collection for NativeUnaryFlatMap {
    fn trace_pointers(&self, out: &mut Vec<Pointer>) {
        out.push(self.func);
        out.push(self.value);
    }

    fn rewrite_pointers(
        &mut self,
        rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, EngineError>,
    ) -> Result<(), EngineError> {
        rewrite_pointer(&mut self.func, rewrite)?;
        rewrite_pointer(&mut self.value, rewrite)
    }
}

impl<State> Coroutine<State> for NativeUnaryFlatMap
where
    State: Clone + Send + Sync + 'static,
{
    fn enter(&mut self, _runtime: &RuntimeCore<State>) -> Result<NativeStep, EngineError> {
        native_apply_step(
            self.func,
            self.func_type.clone(),
            self.value,
            self.elem_type.clone(),
        )
    }

    fn receive(
        &mut self,
        runtime: &RuntimeCore<State>,
        _child: Pointer,
        value: Pointer,
    ) -> Result<NativeStep, EngineError>
    where
        State: Clone + Send + Sync + 'static,
    {
        if matches!(self.shape, NativeUnaryShape::Result) {
            let _ = result_value_ptr(runtime, value)?;
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
pub(crate) struct NativeFold {
    pub func: Pointer,
    pub func_type: Type,
    pub acc_type: Type,
    pub elem_type: Type,
    pub values: ListItems,
    pub acc: Pointer,
    pub order: NativeFoldOrder,
    pub state: NativeFoldState,
    pub next_index: usize,
    pub step: Option<Pointer>,
}

impl Collection for NativeFold {
    fn trace_pointers(&self, out: &mut Vec<Pointer>) {
        out.push(self.func);
        self.values.trace_pointers(out);
        out.push(self.acc);
        trace_option(self.step, out);
    }

    fn rewrite_pointers(
        &mut self,
        rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, EngineError>,
    ) -> Result<(), EngineError> {
        rewrite_pointer(&mut self.func, rewrite)?;
        self.values.rewrite_pointers(rewrite)?;
        rewrite_pointer(&mut self.acc, rewrite)?;
        rewrite_option(&mut self.step, rewrite)
    }
}

impl<State> Coroutine<State> for NativeFold
where
    State: Clone + Send + Sync + 'static,
{
    fn enter(&mut self, runtime: &RuntimeCore<State>) -> Result<NativeStep, EngineError>
    where
        State: Clone + Send + Sync + 'static,
    {
        if self.values.is_empty() {
            return Ok(NativeStep::Return(self.acc));
        }
        self.state = NativeFoldState::ApplyFirst;
        self.next_index = 0;
        self.apply_first(runtime)
    }

    fn receive(
        &mut self,
        runtime: &RuntimeCore<State>,
        _child: Pointer,
        value: Pointer,
    ) -> Result<NativeStep, EngineError>
    where
        State: Clone + Send + Sync + 'static,
    {
        match self.state {
            NativeFoldState::ApplyFirst => {
                self.step = Some(value);
                self.state = NativeFoldState::ApplySecond;
                self.apply_second(runtime)
            }
            NativeFoldState::ApplySecond => {
                self.acc = value;
                self.step = None;
                self.next_index += 1;
                if self.next_index == self.values.len() {
                    return Ok(NativeStep::Return(self.acc));
                }
                self.state = NativeFoldState::ApplyFirst;
                self.apply_first(runtime)
            }
            _ => unexpected_child_result("native fold"),
        }
    }

    fn receive_may_allocate(&self) -> bool {
        false
    }
}

impl NativeFold {
    fn apply_first<State>(&self, runtime: &RuntimeCore<State>) -> Result<NativeStep, EngineError>
    where
        State: Clone + Send + Sync + 'static,
    {
        let arg = match self.order {
            NativeFoldOrder::Left => self.acc,
            NativeFoldOrder::Right => self.value_at(runtime, self.next_index)?,
        };
        let arg_type = match self.order {
            NativeFoldOrder::Left => self.acc_type.clone(),
            NativeFoldOrder::Right => self.elem_type.clone(),
        };
        native_apply_step(self.func, self.func_type.clone(), arg, arg_type)
    }

    fn apply_second<State>(&self, runtime: &RuntimeCore<State>) -> Result<NativeStep, EngineError>
    where
        State: Clone + Send + Sync + 'static,
    {
        let step = self
            .step
            .ok_or_else(|| EngineError::Internal("native fold missing step function".into()))?;
        let arg = match self.order {
            NativeFoldOrder::Left => self.value_at(runtime, self.next_index)?,
            NativeFoldOrder::Right => self.acc,
        };
        let arg_type = match self.order {
            NativeFoldOrder::Left => self.elem_type.clone(),
            NativeFoldOrder::Right => self.acc_type.clone(),
        };
        let step_type = Type::fun(arg_type.clone(), self.acc_type.clone());
        native_apply_step(step, step_type, arg, arg_type)
    }

    fn value_at<State>(
        &self,
        runtime: &RuntimeCore<State>,
        index: usize,
    ) -> Result<Pointer, EngineError>
    where
        State: Clone + Send + Sync + 'static,
    {
        let index = match self.order {
            NativeFoldOrder::Left => index,
            NativeFoldOrder::Right => {
                self.values.len().checked_sub(index + 1).ok_or_else(|| {
                    EngineError::Internal("native fold index out of bounds".into())
                })?
            }
        };
        runtime
            .heap
            .with_locked(|heap| self.values.get(heap, index))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeDictMap {
    pub func: Pointer,
    pub func_type: Type,
    pub elem_type: Type,
    pub entries: Vec<(Symbol, Pointer)>,
    pub children: Vec<Pointer>,
    pub output: BTreeMap<Symbol, Pointer>,
    pub remaining: usize,
}

impl Collection for NativeDictMap {
    fn trace_pointers(&self, out: &mut Vec<Pointer>) {
        out.push(self.func);
        out.extend(self.entries.iter().map(|(_, pointer)| *pointer));
        out.extend(self.children.iter().copied());
        out.extend(self.output.values().copied());
    }

    fn rewrite_pointers(
        &mut self,
        rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, EngineError>,
    ) -> Result<(), EngineError> {
        rewrite_pointer(&mut self.func, rewrite)?;
        rewrite_entries(&mut self.entries, rewrite)?;
        rewrite_slice(&mut self.children, rewrite)?;
        rewrite_map_values(&mut self.output, rewrite)
    }
}

impl<State> Coroutine<State> for NativeDictMap
where
    State: Clone + Send + Sync + 'static,
{
    fn enter(&mut self, runtime: &RuntimeCore<State>) -> Result<NativeStep, EngineError>
    where
        State: Clone + Send + Sync + 'static,
    {
        if self.entries.is_empty() {
            return Ok(NativeStep::Return(runtime.heap.with_locked(|heap| {
                Ok(heap.alloc_ptr_dict(BTreeMap::new())?.into_pointer())
            })?));
        }
        self.children.clear();
        self.output.clear();
        self.remaining = self.entries.len();
        Ok(NativeStep::Schedule(self.entries.len()))
    }

    fn receive(
        &mut self,
        runtime: &RuntimeCore<State>,
        child: Pointer,
        value: Pointer,
    ) -> Result<NativeStep, EngineError>
    where
        State: Clone + Send + Sync + 'static,
    {
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
            return Ok(NativeStep::Return(runtime.heap.with_locked(|heap| {
                Ok(heap.alloc_ptr_dict(self.output.clone())?.into_pointer())
            })?));
        }
        Ok(NativeStep::Wait)
    }

    fn receive_may_allocate(&self) -> bool {
        self.remaining == 1
    }
}

impl NativeDictMap {
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
pub struct NativeDictTraverse {
    pub func: Pointer,
    pub func_type: Type,
    pub elem_type: Type,
    pub entries: Vec<(Symbol, Pointer)>,
    pub next_index: usize,
    pub output: BTreeMap<Symbol, Pointer>,
}

impl Collection for NativeDictTraverse {
    fn trace_pointers(&self, out: &mut Vec<Pointer>) {
        out.push(self.func);
        out.extend(self.entries.iter().map(|(_, pointer)| *pointer));
        out.extend(self.output.values().copied());
    }

    fn rewrite_pointers(
        &mut self,
        rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, EngineError>,
    ) -> Result<(), EngineError> {
        rewrite_pointer(&mut self.func, rewrite)?;
        rewrite_entries(&mut self.entries, rewrite)?;
        rewrite_map_values(&mut self.output, rewrite)
    }
}

impl<State> Coroutine<State> for NativeDictTraverse
where
    State: Clone + Send + Sync + 'static,
{
    fn enter(&mut self, runtime: &RuntimeCore<State>) -> Result<NativeStep, EngineError>
    where
        State: Clone + Send + Sync + 'static,
    {
        if self.entries.is_empty() {
            let dict = runtime
                .heap
                .with_locked(|heap| Ok(heap.alloc_ptr_dict(BTreeMap::new())?.into_pointer()))?;
            return Ok(NativeStep::Return(result_from_native_pointer(
                runtime,
                Ok(dict),
            )?));
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

    fn receive(
        &mut self,
        runtime: &RuntimeCore<State>,
        _child: Pointer,
        value: Pointer,
    ) -> Result<NativeStep, EngineError>
    where
        State: Clone + Send + Sync + 'static,
    {
        match result_value_ptr(runtime, value)? {
            Ok(value) => {
                let (key, _) = self.entries[self.next_index].clone();
                self.output.insert(key, value);
            }
            Err(err) => {
                return Ok(NativeStep::Return(result_from_native_pointer(
                    runtime,
                    Err(err),
                )?));
            }
        }
        self.next_index += 1;
        if self.next_index == self.entries.len() {
            let dict = runtime
                .heap
                .with_locked(|heap| Ok(heap.alloc_ptr_dict(self.output.clone())?.into_pointer()))?;
            return Ok(NativeStep::Return(result_from_native_pointer(
                runtime,
                Ok(dict),
            )?));
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
pub struct NativeArrayEq {
    pub elem_type: Type,
    pub xs: ListItems,
    pub ys: ListItems,
    pub state: NativeArrayEqState,
    pub next_index: usize,
    pub step: Option<Pointer>,
    pub negate: bool,
}

impl Collection for NativeArrayEq {
    fn trace_pointers(&self, out: &mut Vec<Pointer>) {
        self.xs.trace_pointers(out);
        self.ys.trace_pointers(out);
        trace_option(self.step, out);
    }

    fn rewrite_pointers(
        &mut self,
        rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, EngineError>,
    ) -> Result<(), EngineError> {
        self.xs.rewrite_pointers(rewrite)?;
        self.ys.rewrite_pointers(rewrite)?;
        rewrite_option(&mut self.step, rewrite)
    }
}

impl<State> Coroutine<State> for NativeArrayEq
where
    State: Clone + Send + Sync + 'static,
{
    fn enter(&mut self, runtime: &RuntimeCore<State>) -> Result<NativeStep, EngineError>
    where
        State: Clone + Send + Sync + 'static,
    {
        if self.xs.len() != self.ys.len() {
            return self.result(runtime, false);
        }
        if self.xs.is_empty() {
            return self.result(runtime, true);
        }
        self.state = NativeArrayEqState::ApplyFirst;
        self.next_index = 0;
        self.apply_first(runtime)
    }

    fn receive(
        &mut self,
        runtime: &RuntimeCore<State>,
        _child: Pointer,
        value: Pointer,
    ) -> Result<NativeStep, EngineError>
    where
        State: Clone + Send + Sync + 'static,
    {
        match self.state {
            NativeArrayEqState::ApplyFirst => {
                self.step = Some(value);
                self.state = NativeArrayEqState::ApplySecond;
                self.apply_second(runtime)
            }
            NativeArrayEqState::ApplySecond => {
                if !runtime
                    .heap
                    .with_locked(|heap| heap.pointer_as_bool(&value))?
                {
                    return self.result(runtime, false);
                }
                self.step = None;
                self.next_index += 1;
                if self.next_index == self.xs.len() {
                    return self.result(runtime, true);
                }
                self.state = NativeArrayEqState::ApplyFirst;
                self.apply_first(runtime)
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

impl NativeArrayEq {
    fn apply_first<State>(&self, runtime: &RuntimeCore<State>) -> Result<NativeStep, EngineError>
    where
        State: Clone + Send + Sync + 'static,
    {
        let bool_ty = Type::builtin(BuiltinTypeId::Bool);
        let eq_ty = Type::fun(
            self.elem_type.clone(),
            Type::fun(self.elem_type.clone(), bool_ty),
        );
        let lhs = runtime
            .heap
            .with_locked(|heap| self.xs.get(heap, self.next_index))?;
        let roots = runtime.heap.temp_roots(vec![lhs])?;
        let eq = overloaded_pointer(runtime, "==", eq_ty.clone())?;
        let lhs = roots.get(0)?;
        native_apply_step(eq, eq_ty, lhs, self.elem_type.clone())
    }

    fn apply_second<State>(&self, runtime: &RuntimeCore<State>) -> Result<NativeStep, EngineError>
    where
        State: Clone + Send + Sync + 'static,
    {
        let step = self
            .step
            .ok_or_else(|| EngineError::Internal("native list equality missing step".into()))?;
        let bool_ty = Type::builtin(BuiltinTypeId::Bool);
        let step_ty = Type::fun(self.elem_type.clone(), bool_ty);
        native_apply_step(
            step,
            step_ty,
            runtime
                .heap
                .with_locked(|heap| self.ys.get(heap, self.next_index))?,
            self.elem_type.clone(),
        )
    }

    fn result<State>(
        &self,
        runtime: &RuntimeCore<State>,
        equal: bool,
    ) -> Result<NativeStep, EngineError>
    where
        State: Clone + Send + Sync + 'static,
    {
        let b = if self.negate { !equal } else { equal };
        Ok(NativeStep::Return(runtime.heap.with_locked(|heap| {
            Ok(heap.alloc_ptr_bool(b)?.into_pointer())
        })?))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeSum {
    pub elem_type: Type,
    pub values: ListItems,
    pub acc: Option<Pointer>,
    pub plus: Option<Pointer>,
    pub state: NativeFoldState,
    pub next_index: usize,
    pub step: Option<Pointer>,
}

impl Collection for NativeSum {
    fn trace_pointers(&self, out: &mut Vec<Pointer>) {
        self.values.trace_pointers(out);
        trace_option(self.acc, out);
        trace_option(self.plus, out);
        trace_option(self.step, out);
    }

    fn rewrite_pointers(
        &mut self,
        rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, EngineError>,
    ) -> Result<(), EngineError> {
        self.values.rewrite_pointers(rewrite)?;
        rewrite_option(&mut self.acc, rewrite)?;
        rewrite_option(&mut self.plus, rewrite)?;
        rewrite_option(&mut self.step, rewrite)
    }
}

impl<State> Coroutine<State> for NativeSum
where
    State: Clone + Send + Sync + 'static,
{
    fn enter(&mut self, runtime: &RuntimeCore<State>) -> Result<NativeStep, EngineError>
    where
        State: Clone + Send + Sync + 'static,
    {
        if self.values.is_empty() {
            return Ok(native_eval_var_step(
                Symbol::intern("zero"),
                self.elem_type.clone(),
            ));
        }
        let first = runtime.heap.with_locked(|heap| self.values.get(heap, 0))?;
        self.acc = Some(first);
        self.next_index = 1;
        if self.next_index == self.values.len() {
            return Ok(NativeStep::Return(first));
        }
        self.state = NativeFoldState::ApplyFirst;
        self.apply_first_may_allocate(runtime)
    }

    fn receive(
        &mut self,
        _runtime: &RuntimeCore<State>,
        _child: Pointer,
        value: Pointer,
    ) -> Result<NativeStep, EngineError>
    where
        State: Clone + Send + Sync + 'static,
    {
        match self.state {
            NativeFoldState::Enter => Ok(NativeStep::Return(value)),
            NativeFoldState::ApplyFirst => {
                self.step = Some(value);
                self.state = NativeFoldState::ApplySecond;
                self.apply_second(_runtime)
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

impl NativeSum {
    fn apply_first_may_allocate<State>(
        &mut self,
        runtime: &RuntimeCore<State>,
    ) -> Result<NativeStep, EngineError>
    where
        State: Clone + Send + Sync + 'static,
    {
        let plus_ty = binary_same_type(&self.elem_type);
        let acc = self
            .acc
            .ok_or_else(|| EngineError::Internal("native sum missing accumulator".into()))?;
        let acc_root = runtime.heap.temp_roots(vec![acc])?;
        if self.plus.is_none() {
            self.plus = Some(overloaded_pointer(runtime, "+", plus_ty.clone())?);
        }
        let acc = acc_root.get(0)?;
        self.acc = Some(acc);
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

    fn apply_second<State>(&self, runtime: &RuntimeCore<State>) -> Result<NativeStep, EngineError>
    where
        State: Clone + Send + Sync + 'static,
    {
        let step = self
            .step
            .ok_or_else(|| EngineError::Internal("native sum missing step".into()))?;
        let step_ty = Type::fun(self.elem_type.clone(), self.elem_type.clone());
        native_apply_step(
            step,
            step_ty,
            runtime
                .heap
                .with_locked(|heap| self.values.get(heap, self.next_index))?,
            self.elem_type.clone(),
        )
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
pub struct NativeMean {
    pub elem_type: Type,
    pub values: ListItems,
    pub len: usize,
    pub acc: Option<Pointer>,
    pub state: NativeMeanState,
    pub next_index: usize,
    pub step: Option<Pointer>,
    pub len_value: Option<Pointer>,
}

impl Collection for NativeMean {
    fn trace_pointers(&self, out: &mut Vec<Pointer>) {
        self.values.trace_pointers(out);
        trace_option(self.acc, out);
        trace_option(self.step, out);
        trace_option(self.len_value, out);
    }

    fn rewrite_pointers(
        &mut self,
        rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, EngineError>,
    ) -> Result<(), EngineError> {
        self.values.rewrite_pointers(rewrite)?;
        rewrite_option(&mut self.acc, rewrite)?;
        rewrite_option(&mut self.step, rewrite)?;
        rewrite_option(&mut self.len_value, rewrite)
    }
}

impl<State> Coroutine<State> for NativeMean
where
    State: Clone + Send + Sync + 'static,
{
    fn enter(&mut self, runtime: &RuntimeCore<State>) -> Result<NativeStep, EngineError>
    where
        State: Clone + Send + Sync + 'static,
    {
        if self.values.is_empty() {
            return Err(EngineError::EmptySequence);
        }
        self.acc = Some(runtime.heap.with_locked(|heap| self.values.get(heap, 0))?);
        self.next_index = 1;
        if self.next_index == self.values.len() {
            self.state = NativeMeanState::ApplyDivFirst;
            return self.apply_div_first(runtime);
        }
        self.state = NativeMeanState::ApplyPlusFirst;
        self.apply_plus_first(runtime)
    }

    fn receive(
        &mut self,
        runtime: &RuntimeCore<State>,
        _child: Pointer,
        value: Pointer,
    ) -> Result<NativeStep, EngineError>
    where
        State: Clone + Send + Sync + 'static,
    {
        match self.state {
            NativeMeanState::ApplyPlusFirst => {
                self.step = Some(value);
                self.state = NativeMeanState::ApplyPlusSecond;
                self.apply_plus_second(runtime)
            }
            NativeMeanState::ApplyPlusSecond => {
                self.acc = Some(value);
                self.step = None;
                self.next_index += 1;
                if self.next_index == self.values.len() {
                    self.state = NativeMeanState::ApplyDivFirst;
                    return self.apply_div_first(runtime);
                }
                self.state = NativeMeanState::ApplyPlusFirst;
                self.apply_plus_first(runtime)
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

impl NativeMean {
    fn apply_plus_first<State>(
        &self,
        runtime: &RuntimeCore<State>,
    ) -> Result<NativeStep, EngineError>
    where
        State: Clone + Send + Sync + 'static,
    {
        let plus_ty = binary_same_type(&self.elem_type);
        let acc = self
            .acc
            .ok_or_else(|| EngineError::Internal("native mean missing accumulator".into()))?;
        let roots = runtime.heap.temp_roots(vec![acc])?;
        let plus = overloaded_pointer(runtime, "+", plus_ty.clone())?;
        let acc = roots.get(0)?;
        native_apply_step(plus, plus_ty, acc, self.elem_type.clone())
    }

    fn apply_plus_second<State>(
        &self,
        runtime: &RuntimeCore<State>,
    ) -> Result<NativeStep, EngineError>
    where
        State: Clone + Send + Sync + 'static,
    {
        let step = self
            .step
            .ok_or_else(|| EngineError::Internal("native mean missing addition step".into()))?;
        let step_ty = Type::fun(self.elem_type.clone(), self.elem_type.clone());
        native_apply_step(
            step,
            step_ty,
            runtime
                .heap
                .with_locked(|heap| self.values.get(heap, self.next_index))?,
            self.elem_type.clone(),
        )
    }

    fn apply_div_first<State>(
        &mut self,
        runtime: &RuntimeCore<State>,
    ) -> Result<NativeStep, EngineError>
    where
        State: Clone + Send + Sync + 'static,
    {
        let div_ty = binary_same_type(&self.elem_type);
        let acc = self
            .acc
            .ok_or_else(|| EngineError::Internal("native mean missing accumulator".into()))?;
        let acc_root = runtime.heap.temp_roots(vec![acc])?;
        let div = overloaded_pointer(runtime, "/", div_ty.clone())?;
        let div_root = runtime.heap.temp_roots(vec![div])?;
        if self.len_value.is_none() {
            self.len_value = Some(len_value_for_native_type(
                runtime,
                &self.elem_type,
                self.len,
            )?);
        }
        let acc = acc_root.get(0)?;
        let div = div_root.get(0)?;
        native_apply_step(div, div_ty, acc, self.elem_type.clone())
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

fn alloc_native_sequence<State>(
    runtime: &RuntimeCore<State>,
    shape: &NativeSequenceShape,
    values: Vec<Pointer>,
) -> Result<Pointer, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    match shape {
        NativeSequenceShape::List => runtime.heap.alloc_ptr_list(values),
    }
}

fn native_flatten_sequence<State>(
    runtime: &RuntimeCore<State>,
    shape: &NativeSequenceShape,
    pointer: Pointer,
) -> Result<Vec<Pointer>, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    match shape {
        NativeSequenceShape::List => runtime.heap.pointer_as_list(&pointer),
    }
}

fn option_value_ptr<State>(
    runtime: &RuntimeCore<State>,
    pointer: Pointer,
) -> Result<Option<Pointer>, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let (tag, args) = runtime
        .heap
        .with_locked(|heap| heap.pointer_as_adt(&pointer))?;
    if tag.as_ref() == "Some" && args.len() == 1 {
        Ok(Some(args[0]))
    } else if tag.as_ref() == "None" && args.is_empty() {
        Ok(None)
    } else {
        Err(EngineError::NativeType {
            expected: "Option".into(),
            got: runtime
                .heap
                .with_locked(|heap| heap.type_name(&pointer))?
                .into(),
        })
    }
}

fn option_from_native_pointer<State>(
    runtime: &RuntimeCore<State>,
    value: Option<Pointer>,
) -> Result<Pointer, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    match value {
        Some(value) => runtime.heap.with_locked(|heap| {
            Ok(heap
                .alloc_ptr_adt(Symbol::intern("Some"), vec![value])?
                .into_pointer())
        }),
        None => runtime.heap.with_locked(|heap| {
            Ok(heap
                .alloc_ptr_adt(Symbol::intern("None"), vec![])?
                .into_pointer())
        }),
    }
}

fn result_from_native_pointer<State>(
    runtime: &RuntimeCore<State>,
    value: Result<Pointer, Pointer>,
) -> Result<Pointer, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    match value {
        Ok(value) => runtime.heap.with_locked(|heap| {
            Ok(heap
                .alloc_ptr_adt(Symbol::intern("Ok"), vec![value])?
                .into_pointer())
        }),
        Err(value) => runtime.heap.with_locked(|heap| {
            Ok(heap
                .alloc_ptr_adt(Symbol::intern("Err"), vec![value])?
                .into_pointer())
        }),
    }
}

fn result_value_ptr<State>(
    runtime: &RuntimeCore<State>,
    pointer: Pointer,
) -> Result<Result<Pointer, Pointer>, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let (tag, args) = runtime
        .heap
        .with_locked(|heap| heap.pointer_as_adt(&pointer))?;
    if tag.as_ref() == "Ok" && args.len() == 1 {
        Ok(Ok(args[0]))
    } else if tag.as_ref() == "Err" && args.len() == 1 {
        Ok(Err(args[0]))
    } else {
        Err(EngineError::NativeType {
            expected: "Result".into(),
            got: runtime
                .heap
                .with_locked(|heap| heap.type_name(&pointer))?
                .into(),
        })
    }
}

fn overloaded_pointer<State>(
    runtime: &RuntimeCore<State>,
    name: &str,
    typ: Type,
) -> Result<Pointer, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let (name, typ, applied, applied_types) =
        OverloadedFn::new(Symbol::intern(name), typ).into_parts();
    runtime.heap.with_locked(|heap| {
        Ok(heap
            .alloc_ptr_overloaded(name, typ, applied, applied_types)?
            .into_pointer())
    })
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

fn len_value_for_native_type<State>(
    runtime: &RuntimeCore<State>,
    elem_ty: &Type,
    len: usize,
) -> Result<Pointer, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    match elem_ty.as_ref() {
        TypeKind::Con(c) if c.is_builtin(BuiltinTypeId::F32) => runtime
            .heap
            .with_locked(|heap| Ok(heap.alloc_ptr_f32(len as f32)?.into_pointer())),
        TypeKind::Con(c) if c.is_builtin(BuiltinTypeId::F64) => runtime
            .heap
            .with_locked(|heap| Ok(heap.alloc_ptr_f64(len as f64)?.into_pointer())),
        _ => Err(EngineError::NativeType {
            expected: "f32 or f64".into(),
            got: elem_ty.to_string(),
        }),
    }
}

fn refresh_native_frame_from_roots(
    frame: &mut FrNativeCall,
    originals: &[Pointer],
    roots: &TempRoots,
    start: usize,
) -> Result<(), EngineError> {
    let mut wrapped = Frame::NativeCall(frame.clone());
    refresh_frame_from_roots(&mut wrapped, originals, roots, start)?;
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

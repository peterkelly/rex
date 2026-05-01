use std::collections::BTreeMap;
use std::sync::Arc;

use rex_ast::expr::{Pattern, Symbol};
use rex_typesystem::types::{Type, TypedExpr};

use crate::EngineError;
use crate::env::Environment;
use crate::value::Pointer;

#[derive(Clone, Debug, PartialEq)]
pub enum Frame {
    Bool(FrBool),
    Uint(FrUint),
    Int(FrInt),
    Float(FrFloat),
    String(FrString),
    Uuid(FrUuid),
    DateTime(FrDateTime),
    Hole(FrHole),
    Tuple(FrTuple),
    List(FrList),
    Dict(FrDict),
    RecordUpdate(FrRecordUpdate),
    Var(FrVar),
    App(FrApp),
    Project(FrProject),
    Lam(FrLam),
    Let(FrLet),
    LetRec(FrLetRec),
    Ite(FrIte),
    Match(FrMatch),
    NativeCall(FrNativeCall),
    NativeAsync(FrNativeAsync),
}

impl Frame {
    pub fn parent(&self) -> &Pointer {
        match self {
            Frame::Bool(frame) => &frame.parent,
            Frame::Uint(frame) => &frame.parent,
            Frame::Int(frame) => &frame.parent,
            Frame::Float(frame) => &frame.parent,
            Frame::String(frame) => &frame.parent,
            Frame::Uuid(frame) => &frame.parent,
            Frame::DateTime(frame) => &frame.parent,
            Frame::Hole(frame) => &frame.parent,
            Frame::Tuple(frame) => &frame.parent,
            Frame::List(frame) => &frame.parent,
            Frame::Dict(frame) => &frame.parent,
            Frame::RecordUpdate(frame) => &frame.parent,
            Frame::Var(frame) => &frame.parent,
            Frame::App(frame) => &frame.parent,
            Frame::Project(frame) => &frame.parent,
            Frame::Lam(frame) => &frame.parent,
            Frame::Let(frame) => &frame.parent,
            Frame::LetRec(frame) => &frame.parent,
            Frame::Ite(frame) => &frame.parent,
            Frame::Match(frame) => &frame.parent,
            Frame::NativeCall(frame) => &frame.parent,
            Frame::NativeAsync(frame) => &frame.parent,
        }
    }

    pub(crate) fn trace_pointers(&self, out: &mut Vec<Pointer>) {
        match self {
            Frame::Bool(frame) => trace_value_frame(&frame.parent, &frame.env, frame.value, out),
            Frame::Uint(frame) => trace_value_frame(&frame.parent, &frame.env, frame.value, out),
            Frame::Int(frame) => trace_value_frame(&frame.parent, &frame.env, frame.value, out),
            Frame::Float(frame) => trace_value_frame(&frame.parent, &frame.env, frame.value, out),
            Frame::String(frame) => trace_value_frame(&frame.parent, &frame.env, frame.value, out),
            Frame::Uuid(frame) => trace_value_frame(&frame.parent, &frame.env, frame.value, out),
            Frame::DateTime(frame) => {
                trace_value_frame(&frame.parent, &frame.env, frame.value, out)
            }
            Frame::Hole(frame) => trace_value_frame(&frame.parent, &frame.env, frame.value, out),
            Frame::Var(frame) => trace_value_frame(&frame.parent, &frame.env, frame.value, out),
            Frame::Project(frame) => trace_value_frame(&frame.parent, &frame.env, frame.value, out),
            Frame::Lam(frame) => trace_value_frame(&frame.parent, &frame.env, frame.value, out),
            Frame::Tuple(frame) => {
                out.push(frame.parent);
                frame.env.trace_pointers(out);
                out.extend(frame.children.iter().copied());
                out.extend(frame.values.iter().flatten().copied());
            }
            Frame::List(frame) => {
                out.push(frame.parent);
                frame.env.trace_pointers(out);
                out.extend(frame.children.iter().copied());
                out.extend(frame.values.iter().flatten().copied());
            }
            Frame::Dict(frame) => {
                out.push(frame.parent);
                frame.env.trace_pointers(out);
                out.extend(frame.children.iter().copied());
                out.extend(frame.values.iter().flatten().copied());
            }
            Frame::RecordUpdate(frame) => {
                out.push(frame.parent);
                frame.env.trace_pointers(out);
                trace_option(frame.base_value, out);
                out.extend(frame.update_children.iter().copied());
                out.extend(frame.update_values.iter().flatten().copied());
            }
            Frame::App(frame) => {
                out.push(frame.parent);
                frame.env.trace_pointers(out);
                trace_option(frame.func, out);
                trace_option(frame.arg, out);
            }
            Frame::Let(frame) => {
                out.push(frame.parent);
                frame.env.trace_pointers(out);
                trace_option(frame.def_value, out);
            }
            Frame::LetRec(frame) => {
                out.push(frame.parent);
                frame.env.trace_pointers(out);
                if let Some(env) = &frame.recursive_env {
                    env.trace_pointers(out);
                }
                out.extend(frame.slots.iter().copied());
                trace_option(frame.binding_value, out);
            }
            Frame::Ite(frame) => {
                out.push(frame.parent);
                frame.env.trace_pointers(out);
                trace_option(frame.cond_value, out);
            }
            Frame::Match(frame) => {
                out.push(frame.parent);
                frame.env.trace_pointers(out);
                trace_option(frame.scrutinee_value, out);
                if let Some(env) = &frame.matched_env {
                    env.trace_pointers(out);
                }
            }
            Frame::NativeCall(frame) => {
                out.push(frame.parent);
                frame.task.trace_pointers(out);
            }
            Frame::NativeAsync(frame) => out.push(frame.parent),
        }
    }

    pub(crate) fn rewrite_pointers(
        &mut self,
        rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, EngineError>,
    ) -> Result<(), EngineError> {
        match self {
            Frame::Bool(frame) => {
                rewrite_value_frame(&mut frame.parent, &mut frame.env, &mut frame.value, rewrite)
            }
            Frame::Uint(frame) => {
                rewrite_value_frame(&mut frame.parent, &mut frame.env, &mut frame.value, rewrite)
            }
            Frame::Int(frame) => {
                rewrite_value_frame(&mut frame.parent, &mut frame.env, &mut frame.value, rewrite)
            }
            Frame::Float(frame) => {
                rewrite_value_frame(&mut frame.parent, &mut frame.env, &mut frame.value, rewrite)
            }
            Frame::String(frame) => {
                rewrite_value_frame(&mut frame.parent, &mut frame.env, &mut frame.value, rewrite)
            }
            Frame::Uuid(frame) => {
                rewrite_value_frame(&mut frame.parent, &mut frame.env, &mut frame.value, rewrite)
            }
            Frame::DateTime(frame) => {
                rewrite_value_frame(&mut frame.parent, &mut frame.env, &mut frame.value, rewrite)
            }
            Frame::Hole(frame) => {
                rewrite_value_frame(&mut frame.parent, &mut frame.env, &mut frame.value, rewrite)
            }
            Frame::Var(frame) => {
                rewrite_value_frame(&mut frame.parent, &mut frame.env, &mut frame.value, rewrite)
            }
            Frame::Project(frame) => {
                rewrite_value_frame(&mut frame.parent, &mut frame.env, &mut frame.value, rewrite)
            }
            Frame::Lam(frame) => {
                rewrite_value_frame(&mut frame.parent, &mut frame.env, &mut frame.value, rewrite)
            }
            Frame::Tuple(frame) => {
                rewrite_pointer(&mut frame.parent, rewrite)?;
                frame.env.rewrite_pointers(rewrite)?;
                rewrite_slice(&mut frame.children, rewrite)?;
                rewrite_option_slice(&mut frame.values, rewrite)
            }
            Frame::List(frame) => {
                rewrite_pointer(&mut frame.parent, rewrite)?;
                frame.env.rewrite_pointers(rewrite)?;
                rewrite_slice(&mut frame.children, rewrite)?;
                rewrite_option_slice(&mut frame.values, rewrite)
            }
            Frame::Dict(frame) => {
                rewrite_pointer(&mut frame.parent, rewrite)?;
                frame.env.rewrite_pointers(rewrite)?;
                rewrite_slice(&mut frame.children, rewrite)?;
                rewrite_option_slice(&mut frame.values, rewrite)
            }
            Frame::RecordUpdate(frame) => {
                rewrite_pointer(&mut frame.parent, rewrite)?;
                frame.env.rewrite_pointers(rewrite)?;
                rewrite_option(&mut frame.base_value, rewrite)?;
                rewrite_slice(&mut frame.update_children, rewrite)?;
                rewrite_option_slice(&mut frame.update_values, rewrite)
            }
            Frame::App(frame) => {
                rewrite_pointer(&mut frame.parent, rewrite)?;
                frame.env.rewrite_pointers(rewrite)?;
                rewrite_option(&mut frame.func, rewrite)?;
                rewrite_option(&mut frame.arg, rewrite)
            }
            Frame::Let(frame) => {
                rewrite_pointer(&mut frame.parent, rewrite)?;
                frame.env.rewrite_pointers(rewrite)?;
                rewrite_option(&mut frame.def_value, rewrite)
            }
            Frame::LetRec(frame) => {
                rewrite_pointer(&mut frame.parent, rewrite)?;
                frame.env.rewrite_pointers(rewrite)?;
                if let Some(env) = &mut frame.recursive_env {
                    env.rewrite_pointers(rewrite)?;
                }
                rewrite_slice(&mut frame.slots, rewrite)?;
                rewrite_option(&mut frame.binding_value, rewrite)
            }
            Frame::Ite(frame) => {
                rewrite_pointer(&mut frame.parent, rewrite)?;
                frame.env.rewrite_pointers(rewrite)?;
                rewrite_option(&mut frame.cond_value, rewrite)
            }
            Frame::Match(frame) => {
                rewrite_pointer(&mut frame.parent, rewrite)?;
                frame.env.rewrite_pointers(rewrite)?;
                rewrite_option(&mut frame.scrutinee_value, rewrite)?;
                if let Some(env) = &mut frame.matched_env {
                    env.rewrite_pointers(rewrite)?;
                }
                Ok(())
            }
            Frame::NativeCall(frame) => {
                rewrite_pointer(&mut frame.parent, rewrite)?;
                frame.task.rewrite_pointers(rewrite)
            }
            Frame::NativeAsync(frame) => rewrite_pointer(&mut frame.parent, rewrite),
        }
    }
}

fn trace_option(pointer: Option<Pointer>, out: &mut Vec<Pointer>) {
    if let Some(pointer) = pointer {
        out.push(pointer);
    }
}

fn trace_value_frame(
    parent: &Pointer,
    env: &Environment,
    value: Option<Pointer>,
    out: &mut Vec<Pointer>,
) {
    out.push(*parent);
    env.trace_pointers(out);
    trace_option(value, out);
}

fn rewrite_pointer(
    pointer: &mut Pointer,
    rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, EngineError>,
) -> Result<(), EngineError> {
    *pointer = rewrite(*pointer)?;
    Ok(())
}

fn rewrite_option(
    pointer: &mut Option<Pointer>,
    rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, EngineError>,
) -> Result<(), EngineError> {
    if let Some(value) = pointer {
        rewrite_pointer(value, rewrite)?;
    }
    Ok(())
}

fn rewrite_slice(
    pointers: &mut [Pointer],
    rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, EngineError>,
) -> Result<(), EngineError> {
    for pointer in pointers {
        rewrite_pointer(pointer, rewrite)?;
    }
    Ok(())
}

fn rewrite_option_slice(
    pointers: &mut [Option<Pointer>],
    rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, EngineError>,
) -> Result<(), EngineError> {
    for pointer in pointers {
        rewrite_option(pointer, rewrite)?;
    }
    Ok(())
}

fn rewrite_map_values(
    values: &mut BTreeMap<Symbol, Pointer>,
    rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, EngineError>,
) -> Result<(), EngineError> {
    for pointer in values.values_mut() {
        rewrite_pointer(pointer, rewrite)?;
    }
    Ok(())
}

fn rewrite_value_frame(
    parent: &mut Pointer,
    env: &mut Environment,
    value: &mut Option<Pointer>,
    rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, EngineError>,
) -> Result<(), EngineError> {
    rewrite_pointer(parent, rewrite)?;
    env.rewrite_pointers(rewrite)?;
    rewrite_option(value, rewrite)
}

#[derive(Clone, Debug, PartialEq)]
pub enum FrValueState {
    Enter,
    Complete,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FrSequenceState {
    Enter,
    EvalItem,
    Complete,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FrRecordUpdateState {
    Enter,
    EvalBase,
    EvalUpdate,
    Complete,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FrAppState {
    Enter,
    EvalHead,
    EvalArg,
    ApplyArg,
    Complete,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FrLetState {
    Enter,
    EvalDef,
    EvalBody,
    Complete,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FrLetRecState {
    Enter,
    EvalBinding,
    EvalBody,
    Complete,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FrBranchState {
    Enter,
    EvalCondition,
    EvalSelected,
    Complete,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FrMatchState {
    Enter,
    EvalScrutinee,
    EvalArm,
    Complete,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FrNativeCallState {
    Enter,
    Waiting,
    Complete,
}

#[derive(Clone, Debug, PartialEq)]
pub enum NativeSequenceShape {
    List,
    Array,
}

#[derive(Clone, Debug, PartialEq)]
pub enum NativeUnaryShape {
    Option,
    Result,
}

#[derive(Clone, Debug, PartialEq)]
pub enum NativeFoldOrder {
    Left,
    Right,
}

#[derive(Clone, Debug, PartialEq)]
pub enum NativeFoldState {
    Enter,
    ApplyFirst,
    ApplySecond,
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
pub enum NativeArrayEqState {
    Enter,
    ApplyFirst,
    ApplySecond,
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
    DictTraverseResult(NativeDictTraverseResult),
    ArrayEq(NativeArrayEq),
    Sum(NativeSum),
    Mean(NativeMean),
    LogShow(NativeLogShow),
}

impl NativeTask {
    fn trace_pointers(&self, out: &mut Vec<Pointer>) {
        match self {
            NativeTask::ApplyUnary(task) => {
                out.push(task.func);
                out.push(task.arg);
            }
            NativeTask::SequenceMap(task) => {
                out.push(task.func);
                out.extend(task.values.iter().copied());
                out.extend(task.output.iter().copied());
            }
            NativeTask::SequenceFilter(task) => {
                out.push(task.func);
                out.extend(task.values.iter().copied());
                out.extend(task.output.iter().copied());
            }
            NativeTask::SequenceFilterMap(task) => {
                out.push(task.func);
                out.extend(task.values.iter().copied());
                out.extend(task.output.iter().copied());
            }
            NativeTask::SequenceFlatMap(task) => {
                out.push(task.func);
                out.extend(task.values.iter().copied());
                out.extend(task.output.iter().copied());
            }
            NativeTask::UnaryMap(task) => {
                out.push(task.func);
                out.push(task.value);
            }
            NativeTask::UnaryFilter(task) => {
                out.push(task.func);
                out.push(task.value);
                out.push(task.original);
            }
            NativeTask::UnaryFilterMap(task) => {
                out.push(task.func);
                out.push(task.value);
            }
            NativeTask::UnaryFlatMap(task) => {
                out.push(task.func);
                out.push(task.value);
            }
            NativeTask::Fold(task) => {
                out.push(task.func);
                out.extend(task.values.iter().copied());
                out.push(task.acc);
                trace_option(task.step, out);
            }
            NativeTask::DictMap(task) => {
                out.push(task.func);
                out.extend(task.entries.iter().map(|(_, pointer)| *pointer));
                out.extend(task.output.values().copied());
            }
            NativeTask::DictTraverseResult(task) => {
                out.push(task.func);
                out.extend(task.entries.iter().map(|(_, pointer)| *pointer));
                out.extend(task.output.values().copied());
            }
            NativeTask::ArrayEq(task) => {
                out.extend(task.xs.iter().copied());
                out.extend(task.ys.iter().copied());
                trace_option(task.step, out);
            }
            NativeTask::Sum(task) => {
                out.extend(task.values.iter().copied());
                trace_option(task.acc, out);
                trace_option(task.step, out);
            }
            NativeTask::Mean(task) => {
                out.extend(task.values.iter().copied());
                trace_option(task.acc, out);
                trace_option(task.step, out);
                trace_option(task.len_value, out);
            }
            NativeTask::LogShow(task) => out.push(task.arg),
        }
    }

    fn rewrite_pointers(
        &mut self,
        rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, EngineError>,
    ) -> Result<(), EngineError> {
        match self {
            NativeTask::ApplyUnary(task) => {
                rewrite_pointer(&mut task.func, rewrite)?;
                rewrite_pointer(&mut task.arg, rewrite)
            }
            NativeTask::SequenceMap(task) => {
                rewrite_pointer(&mut task.func, rewrite)?;
                rewrite_slice(&mut task.values, rewrite)?;
                rewrite_slice(&mut task.output, rewrite)
            }
            NativeTask::SequenceFilter(task) => {
                rewrite_pointer(&mut task.func, rewrite)?;
                rewrite_slice(&mut task.values, rewrite)?;
                rewrite_slice(&mut task.output, rewrite)
            }
            NativeTask::SequenceFilterMap(task) => {
                rewrite_pointer(&mut task.func, rewrite)?;
                rewrite_slice(&mut task.values, rewrite)?;
                rewrite_slice(&mut task.output, rewrite)
            }
            NativeTask::SequenceFlatMap(task) => {
                rewrite_pointer(&mut task.func, rewrite)?;
                rewrite_slice(&mut task.values, rewrite)?;
                rewrite_slice(&mut task.output, rewrite)
            }
            NativeTask::UnaryMap(task) => {
                rewrite_pointer(&mut task.func, rewrite)?;
                rewrite_pointer(&mut task.value, rewrite)
            }
            NativeTask::UnaryFilter(task) => {
                rewrite_pointer(&mut task.func, rewrite)?;
                rewrite_pointer(&mut task.value, rewrite)?;
                rewrite_pointer(&mut task.original, rewrite)
            }
            NativeTask::UnaryFilterMap(task) => {
                rewrite_pointer(&mut task.func, rewrite)?;
                rewrite_pointer(&mut task.value, rewrite)
            }
            NativeTask::UnaryFlatMap(task) => {
                rewrite_pointer(&mut task.func, rewrite)?;
                rewrite_pointer(&mut task.value, rewrite)
            }
            NativeTask::Fold(task) => {
                rewrite_pointer(&mut task.func, rewrite)?;
                rewrite_slice(&mut task.values, rewrite)?;
                rewrite_pointer(&mut task.acc, rewrite)?;
                rewrite_option(&mut task.step, rewrite)
            }
            NativeTask::DictMap(task) => {
                rewrite_pointer(&mut task.func, rewrite)?;
                rewrite_entries(&mut task.entries, rewrite)?;
                rewrite_map_values(&mut task.output, rewrite)
            }
            NativeTask::DictTraverseResult(task) => {
                rewrite_pointer(&mut task.func, rewrite)?;
                rewrite_entries(&mut task.entries, rewrite)?;
                rewrite_map_values(&mut task.output, rewrite)
            }
            NativeTask::ArrayEq(task) => {
                rewrite_slice(&mut task.xs, rewrite)?;
                rewrite_slice(&mut task.ys, rewrite)?;
                rewrite_option(&mut task.step, rewrite)
            }
            NativeTask::Sum(task) => {
                rewrite_slice(&mut task.values, rewrite)?;
                rewrite_option(&mut task.acc, rewrite)?;
                rewrite_option(&mut task.step, rewrite)
            }
            NativeTask::Mean(task) => {
                rewrite_slice(&mut task.values, rewrite)?;
                rewrite_option(&mut task.acc, rewrite)?;
                rewrite_option(&mut task.step, rewrite)?;
                rewrite_option(&mut task.len_value, rewrite)
            }
            NativeTask::LogShow(task) => rewrite_pointer(&mut task.arg, rewrite),
        }
    }
}

fn rewrite_entries(
    entries: &mut [(Symbol, Pointer)],
    rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, EngineError>,
) -> Result<(), EngineError> {
    for (_, pointer) in entries {
        rewrite_pointer(pointer, rewrite)?;
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeApplyUnary {
    pub func: Pointer,
    pub func_type: Type,
    pub arg: Pointer,
    pub arg_type: Type,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeSequenceMap {
    pub func: Pointer,
    pub func_type: Type,
    pub elem_type: Type,
    pub values: Vec<Pointer>,
    pub shape: NativeSequenceShape,
    pub next_index: usize,
    pub output: Vec<Pointer>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeSequenceFilter {
    pub func: Pointer,
    pub func_type: Type,
    pub elem_type: Type,
    pub values: Vec<Pointer>,
    pub shape: NativeSequenceShape,
    pub next_index: usize,
    pub output: Vec<Pointer>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeSequenceFilterMap {
    pub func: Pointer,
    pub func_type: Type,
    pub elem_type: Type,
    pub values: Vec<Pointer>,
    pub shape: NativeSequenceShape,
    pub next_index: usize,
    pub output: Vec<Pointer>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeSequenceFlatMap {
    pub func: Pointer,
    pub func_type: Type,
    pub elem_type: Type,
    pub values: Vec<Pointer>,
    pub shape: NativeSequenceShape,
    pub next_index: usize,
    pub output: Vec<Pointer>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeUnaryMap {
    pub func: Pointer,
    pub func_type: Type,
    pub elem_type: Type,
    pub value: Pointer,
    pub shape: NativeUnaryShape,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeUnaryFilter {
    pub func: Pointer,
    pub func_type: Type,
    pub elem_type: Type,
    pub value: Pointer,
    pub original: Pointer,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeUnaryFilterMap {
    pub func: Pointer,
    pub func_type: Type,
    pub elem_type: Type,
    pub value: Pointer,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeUnaryFlatMap {
    pub func: Pointer,
    pub func_type: Type,
    pub elem_type: Type,
    pub value: Pointer,
    pub shape: NativeUnaryShape,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeFold {
    pub func: Pointer,
    pub func_type: Type,
    pub acc_type: Type,
    pub elem_type: Type,
    pub values: Vec<Pointer>,
    pub acc: Pointer,
    pub order: NativeFoldOrder,
    pub state: NativeFoldState,
    pub next_index: usize,
    pub step: Option<Pointer>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeDictMap {
    pub func: Pointer,
    pub func_type: Type,
    pub elem_type: Type,
    pub entries: Vec<(Symbol, Pointer)>,
    pub next_index: usize,
    pub output: BTreeMap<Symbol, Pointer>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeDictTraverseResult {
    pub func: Pointer,
    pub func_type: Type,
    pub elem_type: Type,
    pub entries: Vec<(Symbol, Pointer)>,
    pub next_index: usize,
    pub output: BTreeMap<Symbol, Pointer>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeArrayEq {
    pub elem_type: Type,
    pub xs: Vec<Pointer>,
    pub ys: Vec<Pointer>,
    pub state: NativeArrayEqState,
    pub next_index: usize,
    pub step: Option<Pointer>,
    pub negate: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeSum {
    pub elem_type: Type,
    pub values: Vec<Pointer>,
    pub acc: Option<Pointer>,
    pub state: NativeFoldState,
    pub next_index: usize,
    pub step: Option<Pointer>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeMean {
    pub elem_type: Type,
    pub values: Vec<Pointer>,
    pub len: usize,
    pub acc: Option<Pointer>,
    pub state: NativeMeanState,
    pub next_index: usize,
    pub step: Option<Pointer>,
    pub len_value: Option<Pointer>,
}

#[derive(Clone, Debug)]
pub struct NativeLogShow {
    pub show_type: Type,
    pub arg_type: Type,
    pub arg: Pointer,
    pub log: fn(&str),
}

impl PartialEq for NativeLogShow {
    fn eq(&self, other: &Self) -> bool {
        self.show_type == other.show_type
            && self.arg_type == other.arg_type
            && self.arg == other.arg
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FrAppArg {
    pub func_type: Type,
    pub expr: Arc<TypedExpr>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FrMatchArm {
    pub pattern: Pattern,
    pub expr: Arc<TypedExpr>,
}

macro_rules! value_frame {
    ($name:ident) => {
        #[derive(Clone, Debug, PartialEq)]
        pub struct $name {
            pub parent: Pointer,
            pub expr: Arc<TypedExpr>,
            pub env: Environment,
            pub state: FrValueState,
            pub value: Option<Pointer>,
        }
    };
}

value_frame!(FrBool);
value_frame!(FrUint);
value_frame!(FrInt);
value_frame!(FrFloat);
value_frame!(FrString);
value_frame!(FrUuid);
value_frame!(FrDateTime);
value_frame!(FrHole);
value_frame!(FrVar);
value_frame!(FrProject);
value_frame!(FrLam);

#[derive(Clone, Debug, PartialEq)]
pub struct FrTuple {
    pub parent: Pointer,
    pub expr: Arc<TypedExpr>,
    pub env: Environment,
    pub state: FrSequenceState,
    pub children: Vec<Pointer>,
    pub values: Vec<Option<Pointer>>,
    pub remaining: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FrList {
    pub parent: Pointer,
    pub expr: Arc<TypedExpr>,
    pub env: Environment,
    pub state: FrSequenceState,
    pub children: Vec<Pointer>,
    pub values: Vec<Option<Pointer>>,
    pub remaining: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FrDict {
    pub parent: Pointer,
    pub expr: Arc<TypedExpr>,
    pub env: Environment,
    pub state: FrSequenceState,
    pub keys: Vec<Symbol>,
    pub children: Vec<Pointer>,
    pub values: Vec<Option<Pointer>>,
    pub remaining: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FrRecordUpdate {
    pub parent: Pointer,
    pub expr: Arc<TypedExpr>,
    pub env: Environment,
    pub state: FrRecordUpdateState,
    pub base_value: Option<Pointer>,
    pub update_keys: Vec<Symbol>,
    pub update_children: Vec<Pointer>,
    pub update_values: Vec<Option<Pointer>>,
    pub remaining_updates: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FrApp {
    pub parent: Pointer,
    pub expr: Arc<TypedExpr>,
    pub env: Environment,
    pub state: FrAppState,
    pub head: Option<Arc<TypedExpr>>,
    pub spine: Vec<FrAppArg>,
    pub next_arg_index: usize,
    pub func: Option<Pointer>,
    pub arg: Option<Pointer>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FrLet {
    pub parent: Pointer,
    pub expr: Arc<TypedExpr>,
    pub env: Environment,
    pub state: FrLetState,
    pub def_value: Option<Pointer>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FrLetRec {
    pub parent: Pointer,
    pub expr: Arc<TypedExpr>,
    pub env: Environment,
    pub state: FrLetRecState,
    pub recursive_env: Option<Environment>,
    pub slots: Vec<Pointer>,
    pub next_binding_index: usize,
    pub binding_value: Option<Pointer>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FrIte {
    pub parent: Pointer,
    pub expr: Arc<TypedExpr>,
    pub env: Environment,
    pub state: FrBranchState,
    pub cond_value: Option<Pointer>,
    pub selected: Option<Arc<TypedExpr>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FrMatch {
    pub parent: Pointer,
    pub expr: Arc<TypedExpr>,
    pub env: Environment,
    pub state: FrMatchState,
    pub scrutinee_value: Option<Pointer>,
    pub arms: Vec<FrMatchArm>,
    pub next_arm_index: usize,
    pub matched_env: Option<Environment>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FrNativeCall {
    pub parent: Pointer,
    pub state: FrNativeCallState,
    pub task: NativeTask,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FrNativeAsync {
    pub parent: Pointer,
}

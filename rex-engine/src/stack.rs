use std::collections::BTreeMap;
use std::sync::Arc;

use rex_ast::{Pattern, Symbol};
use rex_typesystem::types::{Type, TypedExpr};

use crate::{
    env::Environment,
    evaluator::native_functions::NativeTask,
    value::{Collection, Pointer},
};

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

    pub(crate) fn mark_complete(&mut self, value: Pointer) {
        match self {
            Frame::Bool(frame) => {
                frame.state = FrValueState::Complete;
                frame.value = Some(value);
            }
            Frame::Uint(frame) => {
                frame.state = FrValueState::Complete;
                frame.value = Some(value);
            }
            Frame::Int(frame) => {
                frame.state = FrValueState::Complete;
                frame.value = Some(value);
            }
            Frame::Float(frame) => {
                frame.state = FrValueState::Complete;
                frame.value = Some(value);
            }
            Frame::String(frame) => {
                frame.state = FrValueState::Complete;
                frame.value = Some(value);
            }
            Frame::Uuid(frame) => {
                frame.state = FrValueState::Complete;
                frame.value = Some(value);
            }
            Frame::DateTime(frame) => {
                frame.state = FrValueState::Complete;
                frame.value = Some(value);
            }
            Frame::Hole(frame) => {
                frame.state = FrValueState::Complete;
                frame.value = Some(value);
            }
            Frame::Tuple(frame) => frame.state = FrSequenceState::Complete,
            Frame::List(frame) => frame.state = FrSequenceState::Complete,
            Frame::Dict(frame) => frame.state = FrSequenceState::Complete,
            Frame::RecordUpdate(frame) => frame.state = FrRecordUpdateState::Complete,
            Frame::Var(frame) => {
                frame.state = FrValueState::Complete;
                frame.value = Some(value);
            }
            Frame::App(frame) => frame.state = FrAppState::Complete,
            Frame::Project(frame) => {
                frame.state = FrValueState::Complete;
                frame.value = Some(value);
            }
            Frame::Lam(frame) => {
                frame.state = FrValueState::Complete;
                frame.value = Some(value);
            }
            Frame::Let(frame) => frame.state = FrLetState::Complete,
            Frame::LetRec(frame) => frame.state = FrLetRecState::Complete,
            Frame::Ite(frame) => frame.state = FrBranchState::Complete,
            Frame::Match(frame) => frame.state = FrMatchState::Complete,
            Frame::NativeCall(frame) => frame.state = FrNativeCallState::Complete,
            Frame::NativeAsync(_) => {}
        }
    }
}

impl Collection for Frame {
    fn map_pointers<E>(
        &mut self,
        rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, E>,
    ) -> Result<(), E> {
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
                frame.env.map_pointers(rewrite)?;
                rewrite_slice(&mut frame.children, rewrite)?;
                rewrite_option_slice(&mut frame.values, rewrite)
            }
            Frame::List(frame) => {
                rewrite_pointer(&mut frame.parent, rewrite)?;
                frame.env.map_pointers(rewrite)?;
                rewrite_slice(&mut frame.children, rewrite)?;
                rewrite_option_slice(&mut frame.values, rewrite)
            }
            Frame::Dict(frame) => {
                rewrite_pointer(&mut frame.parent, rewrite)?;
                frame.env.map_pointers(rewrite)?;
                rewrite_slice(&mut frame.children, rewrite)?;
                rewrite_option_slice(&mut frame.values, rewrite)
            }
            Frame::RecordUpdate(frame) => {
                rewrite_pointer(&mut frame.parent, rewrite)?;
                frame.env.map_pointers(rewrite)?;
                rewrite_option(&mut frame.base_value, rewrite)?;
                rewrite_slice(&mut frame.update_children, rewrite)?;
                rewrite_option_slice(&mut frame.update_values, rewrite)
            }
            Frame::App(frame) => {
                rewrite_pointer(&mut frame.parent, rewrite)?;
                frame.env.map_pointers(rewrite)?;
                rewrite_option(&mut frame.head_child, rewrite)?;
                rewrite_slice(&mut frame.arg_children, rewrite)?;
                rewrite_option_slice(&mut frame.arg_values, rewrite)?;
                rewrite_option(&mut frame.func, rewrite)?;
                rewrite_option(&mut frame.arg, rewrite)
            }
            Frame::Let(frame) => {
                rewrite_pointer(&mut frame.parent, rewrite)?;
                frame.env.map_pointers(rewrite)?;
                rewrite_option(&mut frame.def_value, rewrite)
            }
            Frame::LetRec(frame) => {
                rewrite_pointer(&mut frame.parent, rewrite)?;
                frame.env.map_pointers(rewrite)?;
                if let Some(env) = &mut frame.recursive_env {
                    env.map_pointers(rewrite)?;
                }
                rewrite_slice(&mut frame.slots, rewrite)?;
                rewrite_option(&mut frame.binding_value, rewrite)
            }
            Frame::Ite(frame) => {
                rewrite_pointer(&mut frame.parent, rewrite)?;
                frame.env.map_pointers(rewrite)?;
                rewrite_option(&mut frame.cond_value, rewrite)
            }
            Frame::Match(frame) => {
                rewrite_pointer(&mut frame.parent, rewrite)?;
                frame.env.map_pointers(rewrite)?;
                rewrite_option(&mut frame.scrutinee_value, rewrite)?;
                if let Some(env) = &mut frame.matched_env {
                    env.map_pointers(rewrite)?;
                }
                Ok(())
            }
            Frame::NativeCall(frame) => {
                rewrite_pointer(&mut frame.parent, rewrite)?;
                frame.task.map_pointers(rewrite)
            }
            Frame::NativeAsync(frame) => rewrite_pointer(&mut frame.parent, rewrite),
        }
    }
}

pub(crate) fn rewrite_pointer<E>(
    pointer: &mut Pointer,
    rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, E>,
) -> Result<(), E> {
    *pointer = rewrite(*pointer)?;
    Ok(())
}

pub(crate) fn rewrite_option<E>(
    pointer: &mut Option<Pointer>,
    rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, E>,
) -> Result<(), E> {
    if let Some(value) = pointer {
        rewrite_pointer(value, rewrite)?;
    }
    Ok(())
}

pub(crate) fn rewrite_slice<E>(
    pointers: &mut [Pointer],
    rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, E>,
) -> Result<(), E> {
    for pointer in pointers {
        rewrite_pointer(pointer, rewrite)?;
    }
    Ok(())
}

fn rewrite_option_slice<E>(
    pointers: &mut [Option<Pointer>],
    rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, E>,
) -> Result<(), E> {
    for pointer in pointers {
        rewrite_option(pointer, rewrite)?;
    }
    Ok(())
}

pub(crate) fn rewrite_map_values<E>(
    values: &mut BTreeMap<Symbol, Pointer>,
    rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, E>,
) -> Result<(), E> {
    for pointer in values.values_mut() {
        rewrite_pointer(pointer, rewrite)?;
    }
    Ok(())
}

fn rewrite_value_frame<E>(
    parent: &mut Pointer,
    env: &mut Environment,
    value: &mut Option<Pointer>,
    rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, E>,
) -> Result<(), E> {
    rewrite_pointer(parent, rewrite)?;
    env.map_pointers(rewrite)?;
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
    EvalChildren,
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
pub enum NativeUnaryShape {
    Option,
    Result,
}

pub(crate) fn rewrite_entries<E>(
    entries: &mut [(Symbol, Pointer)],
    rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, E>,
) -> Result<(), E> {
    for (_, pointer) in entries {
        rewrite_pointer(pointer, rewrite)?;
    }
    Ok(())
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
    pub head_child: Option<Pointer>,
    pub arg_children: Vec<Pointer>,
    pub arg_values: Vec<Option<Pointer>>,
    pub remaining: usize,
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

use std::collections::BTreeMap;
use std::sync::Arc;

use rex_ast::{Pattern, Symbol};
use rex_typesystem::types::{Type, TypedExpr};

use crate::{
    env::Environment,
    error::EngineError,
    evaluator::native_functions::NativeTask,
    memory::{heap::Pointer, traits::Collection},
};

/// Stable identity for an evaluator control frame.
///
/// Frame identifiers select entries in one evaluation's [`FrameStore`]. They
/// are not heap pointers and therefore never need relocation by the collector.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct FrameId(usize);

/// Evaluator-owned storage for control frames.
///
/// The generic frame representation distinguishes transient `Pointer` frames,
/// which exist only during one locked synchronous cycle, from persistent
/// frames whose values are `PersistentPtr` tokens. Completed frames are
/// removed promptly; their identifiers are never reused.
pub(crate) struct FrameStore<F> {
    next_id: usize,
    frames: BTreeMap<FrameId, F>,
}

impl<F> Default for FrameStore<F> {
    fn default() -> Self {
        Self {
            next_id: 0,
            frames: BTreeMap::new(),
        }
    }
}

impl<F> FrameStore<F> {
    pub(crate) fn insert(&mut self, frame: F) -> FrameId {
        let id = FrameId(self.next_id);
        assert_ne!(
            self.next_id,
            usize::MAX,
            "evaluator frame identifier overflow"
        );
        self.next_id += 1;
        self.frames.insert(id, frame);
        id
    }

    pub(crate) fn get(&self, id: FrameId) -> Result<&F, EngineError> {
        self.frames
            .get(&id)
            .ok_or_else(|| EngineError::Internal(format!("unknown evaluator frame {id:?}")))
    }

    pub(crate) fn replace(&mut self, id: FrameId, frame: F) -> Result<(), EngineError> {
        let slot = self
            .frames
            .get_mut(&id)
            .ok_or_else(|| EngineError::Internal(format!("unknown evaluator frame {id:?}")))?;
        *slot = frame;
        Ok(())
    }

    pub(crate) fn remove(&mut self, id: FrameId) -> Result<F, EngineError> {
        self.frames
            .remove(&id)
            .ok_or_else(|| EngineError::Internal(format!("unknown evaluator frame {id:?}")))
    }
}

impl Collection for FrameStore<Frame<Pointer, Environment>> {
    fn map_pointers<E>(
        &mut self,
        map: &mut impl FnMut(Pointer) -> Result<Pointer, E>,
    ) -> Result<(), E> {
        for frame in self.frames.values_mut() {
            frame.map_pointers(map)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Frame<P, E> {
    Bool(FrBool<P, E>),
    Uint(FrUint<P, E>),
    Int(FrInt<P, E>),
    Float(FrFloat<P, E>),
    String(FrString<P, E>),
    Uuid(FrUuid<P, E>),
    DateTime(FrDateTime<P, E>),
    Hole(FrHole<P, E>),
    Tuple(FrTuple<P, E>),
    List(FrList<P, E>),
    Dict(FrDict<P, E>),
    RecordUpdate(FrRecordUpdate<P, E>),
    Var(FrVar<P, E>),
    App(FrApp<P, E>),
    Project(FrProject<P, E>),
    Lam(FrLam<P, E>),
    Let(FrLet<P, E>),
    LetRec(FrLetRec<P, E>),
    Ite(FrIte<P, E>),
    Match(FrMatch<P, E>),
    NativeCall(FrNativeCall<P>),
    NativeHost(FrNativeHost),
}

fn map_option_into<P, Q, Err>(
    value: Option<P>,
    map: &mut impl FnMut(P) -> Result<Q, Err>,
) -> Result<Option<Q>, Err> {
    value.map(map).transpose()
}

fn map_option_vec_into<P, Q, Err>(
    values: Vec<Option<P>>,
    map: &mut impl FnMut(P) -> Result<Q, Err>,
) -> Result<Vec<Option<Q>>, Err> {
    values
        .into_iter()
        .map(|value| map_option_into(value, map))
        .collect()
}

pub(crate) trait FrameValueMapper<P, E> {
    type Value;
    type Environment;
    type Error;

    fn map_value(&mut self, value: P) -> Result<Self::Value, Self::Error>;
    fn map_environment(&mut self, env: E) -> Result<Self::Environment, Self::Error>;
}

impl<P, E> Frame<P, E> {
    pub(crate) fn map_values<M>(
        self,
        mapper: &mut M,
    ) -> Result<Frame<M::Value, M::Environment>, M::Error>
    where
        M: FrameValueMapper<P, E>,
    {
        Ok(match self {
            Self::Bool(frame) => Frame::Bool(FrBool {
                parent: frame.parent,
                expr: frame.expr,
                env: mapper.map_environment(frame.env)?,
                state: frame.state,
                value: map_option_into(frame.value, &mut |value| mapper.map_value(value))?,
            }),
            Self::Uint(frame) => Frame::Uint(FrUint {
                parent: frame.parent,
                expr: frame.expr,
                env: mapper.map_environment(frame.env)?,
                state: frame.state,
                value: map_option_into(frame.value, &mut |value| mapper.map_value(value))?,
            }),
            Self::Int(frame) => Frame::Int(FrInt {
                parent: frame.parent,
                expr: frame.expr,
                env: mapper.map_environment(frame.env)?,
                state: frame.state,
                value: map_option_into(frame.value, &mut |value| mapper.map_value(value))?,
            }),
            Self::Float(frame) => Frame::Float(FrFloat {
                parent: frame.parent,
                expr: frame.expr,
                env: mapper.map_environment(frame.env)?,
                state: frame.state,
                value: map_option_into(frame.value, &mut |value| mapper.map_value(value))?,
            }),
            Self::String(frame) => Frame::String(FrString {
                parent: frame.parent,
                expr: frame.expr,
                env: mapper.map_environment(frame.env)?,
                state: frame.state,
                value: map_option_into(frame.value, &mut |value| mapper.map_value(value))?,
            }),
            Self::Uuid(frame) => Frame::Uuid(FrUuid {
                parent: frame.parent,
                expr: frame.expr,
                env: mapper.map_environment(frame.env)?,
                state: frame.state,
                value: map_option_into(frame.value, &mut |value| mapper.map_value(value))?,
            }),
            Self::DateTime(frame) => Frame::DateTime(FrDateTime {
                parent: frame.parent,
                expr: frame.expr,
                env: mapper.map_environment(frame.env)?,
                state: frame.state,
                value: map_option_into(frame.value, &mut |value| mapper.map_value(value))?,
            }),
            Self::Hole(frame) => Frame::Hole(FrHole {
                parent: frame.parent,
                expr: frame.expr,
                env: mapper.map_environment(frame.env)?,
                state: frame.state,
                value: map_option_into(frame.value, &mut |value| mapper.map_value(value))?,
            }),
            Self::Var(frame) => Frame::Var(FrVar {
                parent: frame.parent,
                expr: frame.expr,
                env: mapper.map_environment(frame.env)?,
                state: frame.state,
                value: map_option_into(frame.value, &mut |value| mapper.map_value(value))?,
            }),
            Self::Project(frame) => Frame::Project(FrProject {
                parent: frame.parent,
                expr: frame.expr,
                env: mapper.map_environment(frame.env)?,
                state: frame.state,
                value: map_option_into(frame.value, &mut |value| mapper.map_value(value))?,
            }),
            Self::Lam(frame) => Frame::Lam(FrLam {
                parent: frame.parent,
                expr: frame.expr,
                env: mapper.map_environment(frame.env)?,
                state: frame.state,
                value: map_option_into(frame.value, &mut |value| mapper.map_value(value))?,
            }),
            Self::Tuple(frame) => Frame::Tuple(FrTuple {
                parent: frame.parent,
                expr: frame.expr,
                env: mapper.map_environment(frame.env)?,
                state: frame.state,
                children: frame.children,
                values: map_option_vec_into(frame.values, &mut |value| mapper.map_value(value))?,
                remaining: frame.remaining,
            }),
            Self::List(frame) => Frame::List(FrList {
                parent: frame.parent,
                expr: frame.expr,
                env: mapper.map_environment(frame.env)?,
                state: frame.state,
                children: frame.children,
                values: map_option_vec_into(frame.values, &mut |value| mapper.map_value(value))?,
                remaining: frame.remaining,
            }),
            Self::Dict(frame) => Frame::Dict(FrDict {
                parent: frame.parent,
                expr: frame.expr,
                env: mapper.map_environment(frame.env)?,
                state: frame.state,
                keys: frame.keys,
                children: frame.children,
                values: map_option_vec_into(frame.values, &mut |value| mapper.map_value(value))?,
                remaining: frame.remaining,
            }),
            Self::RecordUpdate(frame) => Frame::RecordUpdate(FrRecordUpdate {
                parent: frame.parent,
                expr: frame.expr,
                env: mapper.map_environment(frame.env)?,
                state: frame.state,
                base_value: map_option_into(frame.base_value, &mut |value| {
                    mapper.map_value(value)
                })?,
                update_keys: frame.update_keys,
                update_children: frame.update_children,
                update_values: map_option_vec_into(frame.update_values, &mut |value| {
                    mapper.map_value(value)
                })?,
                remaining_updates: frame.remaining_updates,
            }),
            Self::App(frame) => Frame::App(FrApp {
                parent: frame.parent,
                expr: frame.expr,
                env: mapper.map_environment(frame.env)?,
                state: frame.state,
                head: frame.head,
                spine: frame.spine,
                head_child: frame.head_child,
                arg_children: frame.arg_children,
                arg_values: map_option_vec_into(frame.arg_values, &mut |value| {
                    mapper.map_value(value)
                })?,
                remaining: frame.remaining,
                next_arg_index: frame.next_arg_index,
                func: map_option_into(frame.func, &mut |value| mapper.map_value(value))?,
                arg: map_option_into(frame.arg, &mut |value| mapper.map_value(value))?,
            }),
            Self::Let(frame) => Frame::Let(FrLet {
                parent: frame.parent,
                expr: frame.expr,
                env: mapper.map_environment(frame.env)?,
                state: frame.state,
                def_value: map_option_into(frame.def_value, &mut |value| mapper.map_value(value))?,
            }),
            Self::LetRec(frame) => Frame::LetRec(FrLetRec {
                parent: frame.parent,
                expr: frame.expr,
                env: mapper.map_environment(frame.env)?,
                state: frame.state,
                recursive_env: frame
                    .recursive_env
                    .map(|env| mapper.map_environment(env))
                    .transpose()?,
                slots: frame
                    .slots
                    .into_iter()
                    .map(|value| mapper.map_value(value))
                    .collect::<Result<Vec<_>, _>>()?,
                next_binding_index: frame.next_binding_index,
                binding_value: map_option_into(frame.binding_value, &mut |value| {
                    mapper.map_value(value)
                })?,
            }),
            Self::Ite(frame) => Frame::Ite(FrIte {
                parent: frame.parent,
                expr: frame.expr,
                env: mapper.map_environment(frame.env)?,
                state: frame.state,
                cond_value: map_option_into(frame.cond_value, &mut |value| {
                    mapper.map_value(value)
                })?,
                selected: frame.selected,
            }),
            Self::Match(frame) => Frame::Match(FrMatch {
                parent: frame.parent,
                expr: frame.expr,
                env: mapper.map_environment(frame.env)?,
                state: frame.state,
                scrutinee_value: map_option_into(frame.scrutinee_value, &mut |value| {
                    mapper.map_value(value)
                })?,
                arms: frame.arms,
                next_arm_index: frame.next_arm_index,
                matched_env: frame
                    .matched_env
                    .map(|env| mapper.map_environment(env))
                    .transpose()?,
            }),
            Self::NativeCall(frame) => Frame::NativeCall(FrNativeCall {
                parent: frame.parent,
                state: frame.state,
                task: frame
                    .task
                    .map_values(&mut |value| mapper.map_value(value))?,
            }),
            Self::NativeHost(frame) => Frame::NativeHost(frame),
        })
    }
}

impl<F> FrameStore<F> {
    pub(crate) fn map_frames<G, Err>(
        self,
        mut map: impl FnMut(F) -> Result<G, Err>,
    ) -> Result<FrameStore<G>, Err> {
        Ok(FrameStore {
            next_id: self.next_id,
            frames: self
                .frames
                .into_iter()
                .map(|(id, frame)| Ok((id, map(frame)?)))
                .collect::<Result<_, _>>()?,
        })
    }
}

impl<P, E> Frame<P, E> {
    pub fn parent(&self) -> Option<FrameId> {
        match self {
            Frame::Bool(frame) => frame.parent,
            Frame::Uint(frame) => frame.parent,
            Frame::Int(frame) => frame.parent,
            Frame::Float(frame) => frame.parent,
            Frame::String(frame) => frame.parent,
            Frame::Uuid(frame) => frame.parent,
            Frame::DateTime(frame) => frame.parent,
            Frame::Hole(frame) => frame.parent,
            Frame::Tuple(frame) => frame.parent,
            Frame::List(frame) => frame.parent,
            Frame::Dict(frame) => frame.parent,
            Frame::RecordUpdate(frame) => frame.parent,
            Frame::Var(frame) => frame.parent,
            Frame::App(frame) => frame.parent,
            Frame::Project(frame) => frame.parent,
            Frame::Lam(frame) => frame.parent,
            Frame::Let(frame) => frame.parent,
            Frame::LetRec(frame) => frame.parent,
            Frame::Ite(frame) => frame.parent,
            Frame::Match(frame) => frame.parent,
            Frame::NativeCall(frame) => frame.parent,
            Frame::NativeHost(frame) => frame.parent,
        }
    }
}

impl Collection for Frame<Pointer, Environment> {
    fn map_pointers<E>(
        &mut self,
        rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, E>,
    ) -> Result<(), E> {
        match self {
            Frame::Bool(frame) => rewrite_value_frame(&mut frame.env, &mut frame.value, rewrite),
            Frame::Uint(frame) => rewrite_value_frame(&mut frame.env, &mut frame.value, rewrite),
            Frame::Int(frame) => rewrite_value_frame(&mut frame.env, &mut frame.value, rewrite),
            Frame::Float(frame) => rewrite_value_frame(&mut frame.env, &mut frame.value, rewrite),
            Frame::String(frame) => rewrite_value_frame(&mut frame.env, &mut frame.value, rewrite),
            Frame::Uuid(frame) => rewrite_value_frame(&mut frame.env, &mut frame.value, rewrite),
            Frame::DateTime(frame) => {
                rewrite_value_frame(&mut frame.env, &mut frame.value, rewrite)
            }
            Frame::Hole(frame) => rewrite_value_frame(&mut frame.env, &mut frame.value, rewrite),
            Frame::Var(frame) => rewrite_value_frame(&mut frame.env, &mut frame.value, rewrite),
            Frame::Project(frame) => rewrite_value_frame(&mut frame.env, &mut frame.value, rewrite),
            Frame::Lam(frame) => rewrite_value_frame(&mut frame.env, &mut frame.value, rewrite),
            Frame::Tuple(frame) => {
                frame.env.map_pointers(rewrite)?;
                rewrite_option_slice(&mut frame.values, rewrite)
            }
            Frame::List(frame) => {
                frame.env.map_pointers(rewrite)?;
                rewrite_option_slice(&mut frame.values, rewrite)
            }
            Frame::Dict(frame) => {
                frame.env.map_pointers(rewrite)?;
                rewrite_option_slice(&mut frame.values, rewrite)
            }
            Frame::RecordUpdate(frame) => {
                frame.env.map_pointers(rewrite)?;
                rewrite_option(&mut frame.base_value, rewrite)?;
                rewrite_option_slice(&mut frame.update_values, rewrite)
            }
            Frame::App(frame) => {
                frame.env.map_pointers(rewrite)?;
                rewrite_option_slice(&mut frame.arg_values, rewrite)?;
                rewrite_option(&mut frame.func, rewrite)?;
                rewrite_option(&mut frame.arg, rewrite)
            }
            Frame::Let(frame) => {
                frame.env.map_pointers(rewrite)?;
                rewrite_option(&mut frame.def_value, rewrite)
            }
            Frame::LetRec(frame) => {
                frame.env.map_pointers(rewrite)?;
                if let Some(env) = &mut frame.recursive_env {
                    env.map_pointers(rewrite)?;
                }
                rewrite_slice(&mut frame.slots, rewrite)?;
                rewrite_option(&mut frame.binding_value, rewrite)
            }
            Frame::Ite(frame) => {
                frame.env.map_pointers(rewrite)?;
                rewrite_option(&mut frame.cond_value, rewrite)
            }
            Frame::Match(frame) => {
                frame.env.map_pointers(rewrite)?;
                rewrite_option(&mut frame.scrutinee_value, rewrite)?;
                if let Some(env) = &mut frame.matched_env {
                    env.map_pointers(rewrite)?;
                }
                Ok(())
            }
            Frame::NativeCall(frame) => frame.task.map_pointers(rewrite),
            Frame::NativeHost(_) => Ok(()),
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
    env: &mut Environment,
    value: &mut Option<Pointer>,
    rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, E>,
) -> Result<(), E> {
    env.map_pointers(rewrite)?;
    rewrite_option(value, rewrite)
}

#[derive(Clone, Debug, PartialEq)]
pub enum FrValueState {
    Enter,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FrSequenceState {
    Enter,
    EvalItem,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FrRecordUpdateState {
    Enter,
    EvalBase,
    EvalUpdate,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FrAppState {
    Enter,
    EvalChildren,
    ApplyArg,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FrLetState {
    Enter,
    EvalDef,
    EvalBody,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FrLetRecState {
    Enter,
    EvalBinding,
    EvalBody,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FrBranchState {
    Enter,
    EvalCondition,
    EvalSelected,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FrMatchState {
    Enter,
    EvalScrutinee,
    EvalArm,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FrNativeCallState {
    Enter,
    Waiting,
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
        pub struct $name<P, E> {
            pub parent: Option<FrameId>,
            pub expr: Arc<TypedExpr>,
            pub env: E,
            pub state: FrValueState,
            pub value: Option<P>,
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
pub struct FrTuple<P, E> {
    pub parent: Option<FrameId>,
    pub expr: Arc<TypedExpr>,
    pub env: E,
    pub state: FrSequenceState,
    pub children: Vec<FrameId>,
    pub values: Vec<Option<P>>,
    pub remaining: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FrList<P, E> {
    pub parent: Option<FrameId>,
    pub expr: Arc<TypedExpr>,
    pub env: E,
    pub state: FrSequenceState,
    pub children: Vec<FrameId>,
    pub values: Vec<Option<P>>,
    pub remaining: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FrDict<P, E> {
    pub parent: Option<FrameId>,
    pub expr: Arc<TypedExpr>,
    pub env: E,
    pub state: FrSequenceState,
    pub keys: Vec<Symbol>,
    pub children: Vec<FrameId>,
    pub values: Vec<Option<P>>,
    pub remaining: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FrRecordUpdate<P, E> {
    pub parent: Option<FrameId>,
    pub expr: Arc<TypedExpr>,
    pub env: E,
    pub state: FrRecordUpdateState,
    pub base_value: Option<P>,
    pub update_keys: Vec<Symbol>,
    pub update_children: Vec<FrameId>,
    pub update_values: Vec<Option<P>>,
    pub remaining_updates: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FrApp<P, E> {
    pub parent: Option<FrameId>,
    pub expr: Arc<TypedExpr>,
    pub env: E,
    pub state: FrAppState,
    pub head: Option<Arc<TypedExpr>>,
    pub spine: Vec<FrAppArg>,
    pub head_child: Option<FrameId>,
    pub arg_children: Vec<FrameId>,
    pub arg_values: Vec<Option<P>>,
    pub remaining: usize,
    pub next_arg_index: usize,
    pub func: Option<P>,
    pub arg: Option<P>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FrLet<P, E> {
    pub parent: Option<FrameId>,
    pub expr: Arc<TypedExpr>,
    pub env: E,
    pub state: FrLetState,
    pub def_value: Option<P>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FrLetRec<P, E> {
    pub parent: Option<FrameId>,
    pub expr: Arc<TypedExpr>,
    pub env: E,
    pub state: FrLetRecState,
    pub recursive_env: Option<E>,
    pub slots: Vec<P>,
    pub next_binding_index: usize,
    pub binding_value: Option<P>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FrIte<P, E> {
    pub parent: Option<FrameId>,
    pub expr: Arc<TypedExpr>,
    pub env: E,
    pub state: FrBranchState,
    pub cond_value: Option<P>,
    pub selected: Option<Arc<TypedExpr>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FrMatch<P, E> {
    pub parent: Option<FrameId>,
    pub expr: Arc<TypedExpr>,
    pub env: E,
    pub state: FrMatchState,
    pub scrutinee_value: Option<P>,
    pub arms: Vec<FrMatchArm>,
    pub next_arm_index: usize,
    pub matched_env: Option<E>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FrNativeCall<P> {
    pub parent: Option<FrameId>,
    pub state: FrNativeCallState,
    pub task: NativeTask<P>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FrNativeHost {
    pub parent: Option<FrameId>,
}

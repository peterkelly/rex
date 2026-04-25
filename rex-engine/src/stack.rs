use std::collections::BTreeMap;
use std::sync::Arc;

use rex_ast::expr::{Pattern, Symbol};
use rex_typesystem::types::{Type, TypedExpr};

use crate::env::Environment;
use crate::value::Pointer;

pub const DEFAULT_STACK_SIZE_BYTES: usize = 16 * 1024 * 1024;

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
        }
    }

    pub fn expr(&self) -> &Arc<TypedExpr> {
        match self {
            Frame::Bool(frame) => &frame.expr,
            Frame::Uint(frame) => &frame.expr,
            Frame::Int(frame) => &frame.expr,
            Frame::Float(frame) => &frame.expr,
            Frame::String(frame) => &frame.expr,
            Frame::Uuid(frame) => &frame.expr,
            Frame::DateTime(frame) => &frame.expr,
            Frame::Hole(frame) => &frame.expr,
            Frame::Tuple(frame) => &frame.expr,
            Frame::List(frame) => &frame.expr,
            Frame::Dict(frame) => &frame.expr,
            Frame::RecordUpdate(frame) => &frame.expr,
            Frame::Var(frame) => &frame.expr,
            Frame::App(frame) => &frame.expr,
            Frame::Project(frame) => &frame.expr,
            Frame::Lam(frame) => &frame.expr,
            Frame::Let(frame) => &frame.expr,
            Frame::LetRec(frame) => &frame.expr,
            Frame::Ite(frame) => &frame.expr,
            Frame::Match(frame) => &frame.expr,
        }
    }

    pub fn env(&self) -> &Environment {
        match self {
            Frame::Bool(frame) => &frame.env,
            Frame::Uint(frame) => &frame.env,
            Frame::Int(frame) => &frame.env,
            Frame::Float(frame) => &frame.env,
            Frame::String(frame) => &frame.env,
            Frame::Uuid(frame) => &frame.env,
            Frame::DateTime(frame) => &frame.env,
            Frame::Hole(frame) => &frame.env,
            Frame::Tuple(frame) => &frame.env,
            Frame::List(frame) => &frame.env,
            Frame::Dict(frame) => &frame.env,
            Frame::RecordUpdate(frame) => &frame.env,
            Frame::Var(frame) => &frame.env,
            Frame::App(frame) => &frame.env,
            Frame::Project(frame) => &frame.env,
            Frame::Lam(frame) => &frame.env,
            Frame::Let(frame) => &frame.env,
            Frame::LetRec(frame) => &frame.env,
            Frame::Ite(frame) => &frame.env,
            Frame::Match(frame) => &frame.env,
        }
    }
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
    AllocateSlots,
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
    pub next_index: usize,
    pub values: Vec<Pointer>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FrList {
    pub parent: Pointer,
    pub expr: Arc<TypedExpr>,
    pub env: Environment,
    pub state: FrSequenceState,
    pub next_index: usize,
    pub values: Vec<Pointer>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FrDict {
    pub parent: Pointer,
    pub expr: Arc<TypedExpr>,
    pub env: Environment,
    pub state: FrSequenceState,
    pub keys: Vec<Symbol>,
    pub next_index: usize,
    pub values: BTreeMap<Symbol, Pointer>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FrRecordUpdate {
    pub parent: Pointer,
    pub expr: Arc<TypedExpr>,
    pub env: Environment,
    pub state: FrRecordUpdateState,
    pub base_value: Option<Pointer>,
    pub update_keys: Vec<Symbol>,
    pub next_update_index: usize,
    pub update_values: BTreeMap<Symbol, Pointer>,
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

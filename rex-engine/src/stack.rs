use crate::value::Pointer;

pub const DEFAULT_STACK_SIZE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrBool {
    pub parent: Pointer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrUint {
    pub parent: Pointer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrInt {
    pub parent: Pointer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrFloat {
    pub parent: Pointer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrString {
    pub parent: Pointer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrUuid {
    pub parent: Pointer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrDateTime {
    pub parent: Pointer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrHole {
    pub parent: Pointer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrTuple {
    pub parent: Pointer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrList {
    pub parent: Pointer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrDict {
    pub parent: Pointer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrRecordUpdate {
    pub parent: Pointer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrVar {
    pub parent: Pointer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrApp {
    pub parent: Pointer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrProject {
    pub parent: Pointer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrLam {
    pub parent: Pointer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrLet {
    pub parent: Pointer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrLetRec {
    pub parent: Pointer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrIte {
    pub parent: Pointer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrMatch {
    pub parent: Pointer,
}

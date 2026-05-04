//! Core engine implementation for Rex.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::fmt::{self, Write as _};
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use futures::{
    FutureExt,
    future::{BoxFuture, poll_fn},
};
use rex_ast::expr::{
    ClassDecl, Decl, DeclareFnDecl, Expr, FnDecl, InstanceDecl, NameRef, Pattern, Program, Scope,
    Symbol, TypeConstraint, TypeDecl, TypeExpr, Var,
};
use rex_lexer::span::Span;
use rex_typesystem::{
    error::{CollectAdtsError, TypeError},
    inference::{infer, infer_typed},
    prelude::prelude_typeclasses_program,
    types::{
        AdtDecl, BuiltinTypeId, Instance, Predicate, RexAdt, RexType, Scheme, Type, TypeKind,
        TypedExpr, TypedExprKind, Types, order_adt_family,
    },
    typesystem::{PreparedInstanceDecl, TypeSystem, TypeVarSupply},
    typesystem::{entails, instantiate},
    unification::{Subst, compose_subst, unify},
};

use crate::modules::{
    CanonicalSymbol, Module, ModuleExports, ModuleId, ModuleSystem, ResolveRequest, ResolvedModule,
    ResolvedModuleContent, SymbolKind, VirtualModule, interface_decls_from_program,
    module_key_for_module, parse_program_from_source, prefix_for_module, qualify_program,
    virtual_export_name,
};
use crate::prelude::{
    inject_boolean_ops, inject_equality_ops, inject_json_primops, inject_list_builtins,
    inject_numeric_ops, inject_option_result_builtins, inject_order_ops, inject_prelude_adts,
    inject_show_ops,
};
use crate::stack::{
    FrApp, FrAppArg, FrAppState, FrBool, FrBranchState, FrDateTime, FrDict, FrFloat, FrHole, FrInt,
    FrIte, FrLam, FrLet, FrLetRec, FrLetRecState, FrLetState, FrList, FrMatch, FrMatchArm,
    FrMatchState, FrNativeAsync, FrNativeCall, FrNativeCallState, FrProject, FrRecordUpdate,
    FrRecordUpdateState, FrSequenceState, FrString, FrTuple, FrUint, FrUuid, FrValueState, FrVar,
    Frame, NativeArrayEq, NativeArrayEqState, NativeDictMap, NativeDictTraverseResult, NativeFold,
    NativeFoldOrder, NativeFoldState, NativeLogShow, NativeMean, NativeMeanState,
    NativeSequenceFilter, NativeSequenceFilterMap, NativeSequenceFlatMap, NativeSequenceMap,
    NativeSequenceShape, NativeSum, NativeTask, NativeUnaryFilter, NativeUnaryFlatMap,
    NativeUnaryMap, NativeUnaryShape,
};
use crate::value::FromPointer;
use crate::value::{Cell, Closure, Handle, Heap, HeapAccess, Pointer, TempRoots, list_to_vec};
use crate::{
    EngineError, Environment, FromRex, IntoRex,
    evaluator::{EvalContext, EvaluatorRef},
};

fn runtime_ctor_symbol(name: &Symbol) -> Symbol {
    Symbol::intern(name.as_ref().rsplit('.').next().unwrap_or(name.as_ref()))
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

pub trait RexDefault<State>
where
    State: Clone + Send + Sync + 'static,
{
    fn rex_default(engine: &Engine<State>) -> Result<Handle, EngineError>;
}

pub const ROOT_MODULE_NAME: &str = "__root__";
pub const PRELUDE_MODULE_NAME: &str = "Prelude";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreludeMode {
    Enabled,
    Disabled,
}

#[derive(Clone, Debug)]
pub struct EngineOptions {
    pub prelude: PreludeMode,
    pub default_imports: Vec<String>,
}

impl Default for EngineOptions {
    fn default() -> Self {
        Self {
            prelude: PreludeMode::Enabled,
            default_imports: vec![PRELUDE_MODULE_NAME.to_string()],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExecutionBounds {
    max_ready_work: usize,
    max_pending_async_calls: usize,
}

impl ExecutionBounds {
    pub const DEFAULT_MAX_READY_WORK: usize = 1024;
    pub const DEFAULT_MAX_PENDING_ASYNC_CALLS: usize = 64;

    /// Create scheduler bounds for evaluator work.
    ///
    /// `max_ready_work` limits how many ready Rex frames are admitted to the
    /// active scheduler queue. `max_pending_async_calls` limits how many async
    /// host-call futures are admitted for polling or executor submission at
    /// once. Zero values are normalized to one so evaluation can always make
    /// progress.
    pub const fn new(max_ready_work: usize, max_pending_async_calls: usize) -> Self {
        Self {
            max_ready_work: if max_ready_work == 0 {
                1
            } else {
                max_ready_work
            },
            max_pending_async_calls: if max_pending_async_calls == 0 {
                1
            } else {
                max_pending_async_calls
            },
        }
    }

    /// Disable scheduler admission limits.
    pub const fn unbounded() -> Self {
        Self {
            max_ready_work: usize::MAX,
            max_pending_async_calls: usize::MAX,
        }
    }

    pub const fn max_ready_work(self) -> usize {
        self.max_ready_work
    }

    pub const fn max_pending_async_calls(self) -> usize {
        self.max_pending_async_calls
    }

    fn ready_work_limit(self) -> usize {
        self.max_ready_work
    }

    fn pending_async_limit(self) -> usize {
        self.max_pending_async_calls
    }
}

impl Default for ExecutionBounds {
    fn default() -> Self {
        Self::new(
            Self::DEFAULT_MAX_READY_WORK,
            Self::DEFAULT_MAX_PENDING_ASYNC_CALLS,
        )
    }
}

/// Execution hook for admitted async host calls.
///
/// The evaluator calls this only after an async host call has passed scheduler
/// admission. The pending-async bound is therefore applied before this hook
/// runs and before an executor can receive the future.
///
/// Returning the future unchanged preserves inline polling. Embedders can wrap
/// it to submit work to Tokio, a browser task queue, or another executor.
pub trait AsyncCallExecutor: Send + Sync + 'static {
    fn spawn(&self, future: NativeFuture) -> NativeFuture;
}

struct FnAsyncCallExecutor<F> {
    spawn: F,
}

impl<F> AsyncCallExecutor for FnAsyncCallExecutor<F>
where
    F: Fn(NativeFuture) -> NativeFuture + Send + Sync + 'static,
{
    fn spawn(&self, future: NativeFuture) -> NativeFuture {
        (self.spawn)(future)
    }
}

/// Policy used to run admitted async host-call futures.
///
/// The default `Inline` policy keeps Rex independent of any particular async
/// runtime. With this policy, admitted host futures are polled by the evaluator
/// task itself. This is portable, including to wasm builds, but a future that
/// performs blocking or CPU-heavy work will still block that evaluator task.
///
/// `Executor` lets embedders decide where admitted host futures should run.
/// The executor is deliberately host-provided so native builds can use Tokio,
/// wasm builds can use browser/task-queue integration, and other embedders can
/// use their own runtime without Rex depending on it directly.
#[derive(Clone, Default)]
pub enum AsyncCallPolicy {
    /// Poll admitted host futures in the evaluator task.
    #[default]
    Inline,
    /// Hand admitted host futures to an embedder-provided executor.
    Executor(Arc<dyn AsyncCallExecutor>),
}

impl AsyncCallPolicy {
    pub fn inline() -> Self {
        Self::Inline
    }

    pub fn executor(executor: impl AsyncCallExecutor) -> Self {
        Self::Executor(Arc::new(executor))
    }

    pub fn executor_arc(executor: Arc<dyn AsyncCallExecutor>) -> Self {
        Self::Executor(executor)
    }

    pub fn executor_fn(
        spawn: impl Fn(NativeFuture) -> NativeFuture + Send + Sync + 'static,
    ) -> Self {
        Self::executor(FnAsyncCallExecutor { spawn })
    }

    pub fn is_inline(&self) -> bool {
        matches!(self, Self::Inline)
    }

    pub fn is_executor(&self) -> bool {
        matches!(self, Self::Executor(_))
    }

    /// Apply the policy after an async host call has been admitted by the
    /// evaluator scheduler and produced its future.
    fn prepare(&self, future: NativeHandleFuture) -> NativeHandleFuture {
        match self {
            Self::Inline => future,
            Self::Executor(executor) => executor.spawn(future),
        }
    }
}

impl fmt::Debug for AsyncCallPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Inline => f.write_str("Inline"),
            Self::Executor(_) => f.write_str("Executor"),
        }
    }
}

impl PartialEq for AsyncCallPolicy {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Inline, Self::Inline) => true,
            (Self::Executor(lhs), Self::Executor(rhs)) => Arc::ptr_eq(lhs, rhs),
            _ => false,
        }
    }
}

impl Eq for AsyncCallPolicy {}

fn alloc_uint_literal_as<State: Clone + Send + Sync + 'static>(
    engine: &RuntimeSnapshot<State>,
    value: u64,
    typ: &Type,
) -> Result<Pointer, EngineError> {
    match typ.as_ref() {
        TypeKind::Var(_) => engine.heap.alloc_ptr_i32(i32::try_from(value).map_err(|_| {
            EngineError::NativeType {
                expected: "i32".into(),
                got: value.to_string(),
            }
        })?),
        TypeKind::Con(tc) => match tc.builtin_id() {
            Some(BuiltinTypeId::U8) => {
                engine.heap.alloc_ptr_u8(u8::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "u8".into(),
                        got: value.to_string(),
                    }
                })?)
            }
            Some(BuiltinTypeId::U16) => {
                engine.heap.alloc_ptr_u16(u16::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "u16".into(),
                        got: value.to_string(),
                    }
                })?)
            }
            Some(BuiltinTypeId::U32) => {
                engine.heap.alloc_ptr_u32(u32::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "u32".into(),
                        got: value.to_string(),
                    }
                })?)
            }
            Some(BuiltinTypeId::U64) => engine.heap.alloc_ptr_u64(value),
            Some(BuiltinTypeId::I8) => {
                engine.heap.alloc_ptr_i8(i8::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "i8".into(),
                        got: value.to_string(),
                    }
                })?)
            }
            Some(BuiltinTypeId::I16) => {
                engine.heap.alloc_ptr_i16(i16::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "i16".into(),
                        got: value.to_string(),
                    }
                })?)
            }
            Some(BuiltinTypeId::I32) => {
                engine.heap.alloc_ptr_i32(i32::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "i32".into(),
                        got: value.to_string(),
                    }
                })?)
            }
            Some(BuiltinTypeId::I64) => {
                engine.heap.alloc_ptr_i64(i64::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "i64".into(),
                        got: value.to_string(),
                    }
                })?)
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

fn alloc_int_literal_as<State: Clone + Send + Sync + 'static>(
    engine: &RuntimeSnapshot<State>,
    value: i64,
    typ: &Type,
) -> Result<Pointer, EngineError> {
    match typ.as_ref() {
        TypeKind::Var(_) => engine.heap.alloc_ptr_i32(i32::try_from(value).map_err(|_| {
            EngineError::NativeType {
                expected: "i32".into(),
                got: value.to_string(),
            }
        })?),
        TypeKind::Con(tc) => match tc.builtin_id() {
            Some(BuiltinTypeId::I8) => {
                engine.heap.alloc_ptr_i8(i8::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "i8".into(),
                        got: value.to_string(),
                    }
                })?)
            }
            Some(BuiltinTypeId::I16) => {
                engine.heap.alloc_ptr_i16(i16::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "i16".into(),
                        got: value.to_string(),
                    }
                })?)
            }
            Some(BuiltinTypeId::I32) => {
                engine.heap.alloc_ptr_i32(i32::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "i32".into(),
                        got: value.to_string(),
                    }
                })?)
            }
            Some(BuiltinTypeId::I64) => engine.heap.alloc_ptr_i64(value),
            Some(BuiltinTypeId::U8) => {
                engine.heap.alloc_ptr_u8(u8::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "u8".into(),
                        got: value.to_string(),
                    }
                })?)
            }
            Some(BuiltinTypeId::U16) => {
                engine.heap.alloc_ptr_u16(u16::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "u16".into(),
                        got: value.to_string(),
                    }
                })?)
            }
            Some(BuiltinTypeId::U32) => {
                engine.heap.alloc_ptr_u32(u32::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "u32".into(),
                        got: value.to_string(),
                    }
                })?)
            }
            Some(BuiltinTypeId::U64) => {
                engine.heap.alloc_ptr_u64(u64::try_from(value).map_err(|_| {
                    EngineError::NativeType {
                        expected: "u64".into(),
                        got: value.to_string(),
                    }
                })?)
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

pub(crate) fn type_head_is_var(typ: &Type) -> bool {
    let mut cur = typ;
    while let TypeKind::App(head, _) = cur.as_ref() {
        cur = head;
    }
    matches!(cur.as_ref(), TypeKind::Var(..))
}

fn sanitize_type_name_for_symbol(typ: &Type) -> String {
    typ.to_string()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect()
}

pub type NativeFuture = BoxFuture<'static, Result<Handle, EngineError>>;
pub(crate) type NativeHandleFuture = BoxFuture<'static, Result<Handle, EngineError>>;
type NativeId = u64;
pub(crate) const RUNTIME_LINK_ABI_VERSION: u32 = 1;
pub(crate) enum SchedulerNativeResult {
    Ready(Pointer),
    Task(NativeTask),
}

pub type SyncNativeCallable<State> = Arc<
    dyn for<'a> Fn(EvaluatorRef<State>, &'a Type, &'a [Handle]) -> Result<Handle, EngineError>
        + Send
        + Sync
        + 'static,
>;
pub type AsyncNativeCallable<State> =
    Arc<dyn Fn(EvaluatorRef<State>, Type, Vec<Handle>) -> NativeFuture + Send + Sync + 'static>;

pub(crate) type SyncNativePointerCallable<State> = Arc<
    dyn for<'a> Fn(EvaluatorRef<State>, &'a Type, &'a [Pointer]) -> Result<Pointer, EngineError>
        + Send
        + Sync
        + 'static,
>;
pub(crate) type SchedulerNativeCallable<State> = Arc<
    dyn for<'a> Fn(
            EvaluatorRef<State>,
            Type,
            &'a [Pointer],
        ) -> Result<SchedulerNativeResult, EngineError>
        + Send
        + Sync
        + 'static,
>;
pub(crate) type AsyncNativePointerCallable<State> = Arc<
    dyn Fn(EvaluatorRef<State>, Type, Vec<Pointer>) -> NativeHandleFuture + Send + Sync + 'static,
>;

type ExportInjector<State> =
    Box<dyn FnOnce(&mut Engine<State>, &str) -> Result<(), EngineError> + Send + 'static>;

struct NativeRegistration<State: Clone + Send + Sync + 'static> {
    scheme: Scheme,
    arity: usize,
    callable: NativeCallable<State>,
}

impl<State: Clone + Send + Sync + 'static> NativeRegistration<State> {
    fn sync(scheme: Scheme, arity: usize, func: SyncNativePointerCallable<State>) -> Self {
        Self {
            scheme,
            arity,
            callable: NativeCallable::Sync(func),
        }
    }

    fn scheduler(scheme: Scheme, arity: usize, func: SchedulerNativeCallable<State>) -> Self {
        Self {
            scheme,
            arity,
            callable: NativeCallable::Scheduler(func),
        }
    }

    fn r#async(scheme: Scheme, arity: usize, func: AsyncNativePointerCallable<State>) -> Self {
        Self {
            scheme,
            arity,
            callable: NativeCallable::Async(func),
        }
    }
}

pub trait HostFnSync<State: Clone + Send + Sync + 'static, Sig>: Send + Sync + 'static {
    fn interface_decl(export_name: &str) -> DeclareFnDecl;
    fn interface_decl_for(&self, export_name: &str) -> DeclareFnDecl {
        Self::interface_decl(export_name)
    }
    fn inject(self, engine: &mut Engine<State>, export_name: &str) -> Result<(), EngineError>;
}

pub trait HostFnAsync<State: Clone + Send + Sync + 'static, Sig>: Send + Sync + 'static {
    fn interface_decl(export_name: &str) -> DeclareFnDecl;
    fn interface_decl_for(&self, export_name: &str) -> DeclareFnDecl {
        Self::interface_decl(export_name)
    }
    fn inject_async(self, engine: &mut Engine<State>, export_name: &str)
    -> Result<(), EngineError>;
}

#[derive(Debug, Clone, Copy)]
struct NativeCallableSig;

#[derive(Debug, Clone, Copy)]
struct AsyncNativeCallableSig;

pub struct Export<State: Clone + Send + Sync + 'static> {
    pub name: String,
    interface: DeclareFnDecl,
    injector: ExportInjector<State>,
}

impl<State> Export<State>
where
    State: Clone + Send + Sync + 'static,
{
    fn from_injector(
        name: impl Into<String>,
        interface: DeclareFnDecl,
        injector: ExportInjector<State>,
    ) -> Result<Self, EngineError> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(EngineError::Internal("export name cannot be empty".into()));
        }
        let normalized = normalize_name(&name).to_string();
        Ok(Self {
            name: normalized,
            interface,
            injector,
        })
    }

    pub fn from_handler<Sig, H>(name: impl Into<String>, handler: H) -> Result<Self, EngineError>
    where
        H: HostFnSync<State, Sig>,
    {
        let name = name.into();
        let normalized = normalize_name(&name).to_string();
        let interface = handler.interface_decl_for(&normalized);
        let injector: ExportInjector<State> =
            Box::new(move |engine, qualified_name| handler.inject(engine, qualified_name));
        Self::from_injector(name, interface, injector)
    }

    pub fn from_async_handler<Sig, H>(
        name: impl Into<String>,
        handler: H,
    ) -> Result<Self, EngineError>
    where
        H: HostFnAsync<State, Sig>,
    {
        let name = name.into();
        let normalized = normalize_name(&name).to_string();
        let interface = handler.interface_decl_for(&normalized);
        let injector: ExportInjector<State> =
            Box::new(move |engine, qualified_name| handler.inject_async(engine, qualified_name));
        Self::from_injector(name, interface, injector)
    }

    pub fn from_native<F>(
        name: impl Into<String>,
        scheme: Scheme,
        arity: usize,
        handler: F,
    ) -> Result<Self, EngineError>
    where
        F: for<'a> Fn(EvaluatorRef<State>, &'a Type, &'a [Handle]) -> Result<Handle, EngineError>
            + Send
            + Sync
            + 'static,
    {
        validate_native_export_scheme(&scheme, arity)?;
        let name = name.into();
        let normalized = normalize_name(&name).to_string();
        let interface = declare_fn_decl_from_scheme(&normalized, &scheme);
        let handler = Arc::new(handler);
        let injector: ExportInjector<State> = Box::new(move |engine, qualified_name| {
            let handler = Arc::clone(&handler);
            let func: SyncNativePointerCallable<State> =
                Arc::new(move |engine, typ: &Type, args: &[Pointer]| {
                    let handles = engine.handles_from_pointers(args)?;
                    let value = handler(engine.clone(), typ, &handles)?;
                    value.pointer_for_heap(engine.heap())
                });
            let registration = NativeRegistration::sync(scheme.clone(), arity, func);
            engine.register_native_registration(ROOT_MODULE_NAME, qualified_name, registration)
        });
        Self::from_injector(name, interface, injector)
    }

    pub(crate) fn from_native_scheduler<F>(
        name: impl Into<String>,
        scheme: Scheme,
        arity: usize,
        handler: F,
    ) -> Result<Self, EngineError>
    where
        F: for<'a> Fn(
                EvaluatorRef<State>,
                Type,
                Vec<Pointer>,
            ) -> Result<SchedulerNativeResult, EngineError>
            + Send
            + Sync
            + 'static,
    {
        validate_native_export_scheme(&scheme, arity)?;
        let name = name.into();
        let normalized = normalize_name(&name).to_string();
        let interface = declare_fn_decl_from_scheme(&normalized, &scheme);
        let handler = Arc::new(handler);
        let injector: ExportInjector<State> = Box::new(move |engine, qualified_name| {
            let handler = Arc::clone(&handler);
            let func: SchedulerNativeCallable<State> = Arc::new(move |engine, typ, args| {
                let args = args.to_vec();
                let handler = Arc::clone(&handler);
                handler(engine, typ, args)
            });
            let registration = NativeRegistration::scheduler(scheme.clone(), arity, func);
            engine.register_native_registration(ROOT_MODULE_NAME, qualified_name, registration)
        });
        Self::from_injector(name, interface, injector)
    }

    pub fn from_native_async<F>(
        name: impl Into<String>,
        scheme: Scheme,
        arity: usize,
        handler: F,
    ) -> Result<Self, EngineError>
    where
        F: Fn(EvaluatorRef<State>, Type, Vec<Handle>) -> NativeFuture + Send + Sync + 'static,
    {
        validate_native_export_scheme(&scheme, arity)?;
        let name = name.into();
        let normalized = normalize_name(&name).to_string();
        let interface = declare_fn_decl_from_scheme(&normalized, &scheme);
        let handler = Arc::new(handler);
        let injector: ExportInjector<State> = Box::new(move |engine, qualified_name| {
            let handler = Arc::clone(&handler);
            let func: AsyncNativePointerCallable<State> = Arc::new(move |engine, typ, args| {
                let handler = Arc::clone(&handler);
                let handles = engine.handles_from_pointers(&args);
                async move {
                    let handles = handles?;
                    let value = handler(engine.clone(), typ, handles).await?;
                    value.pointer_for_heap(engine.heap())?;
                    Ok(value)
                }
                .boxed()
            });
            let registration = NativeRegistration::r#async(scheme.clone(), arity, func);
            engine.register_native_registration(ROOT_MODULE_NAME, qualified_name, registration)
        });
        Self::from_injector(name, interface, injector)
    }

    pub fn from_value<V>(name: impl Into<String>, value: V) -> Result<Self, EngineError>
    where
        V: IntoRex + RexType + Clone + Send + Sync + 'static,
    {
        let name = name.into();
        let typ = V::rex_type();
        let interface = declare_fn_decl_from_scheme(
            normalize_name(&name).as_ref(),
            &Scheme::new(vec![], vec![], typ.clone()),
        );
        let name = interface.name.name.to_string();
        let injector: ExportInjector<State> = Box::new(move |engine, qualified_name| {
            let stored = value.clone();
            let func: SyncNativePointerCallable<State> =
                Arc::new(move |engine, _: &Type, _args: &[Pointer]| {
                    stored.clone().into_rex(engine.heap())?.pointer()
                });
            let registration =
                NativeRegistration::sync(Scheme::new(vec![], vec![], typ.clone()), 0, func);
            engine.register_native_registration(ROOT_MODULE_NAME, qualified_name, registration)
        });
        Self::from_injector(name, interface, injector)
    }
}

fn declare_fn_decl_from_scheme(export_name: &str, scheme: &Scheme) -> DeclareFnDecl {
    let (params, ret) = decompose_fun_type(&scheme.typ);
    DeclareFnDecl {
        span: Span::default(),
        is_pub: true,
        name: Var {
            span: Span::default(),
            name: Symbol::intern(export_name),
        },
        params: params
            .into_iter()
            .enumerate()
            .map(|(idx, ty)| {
                (
                    Var {
                        span: Span::default(),
                        name: Symbol::intern(&format!("arg{idx}")),
                    },
                    type_expr_from_type(&ty),
                )
            })
            .collect(),
        ret: type_expr_from_type(&ret),
        constraints: scheme
            .preds
            .iter()
            .map(|pred| TypeConstraint {
                class: NameRef::Unqualified(pred.class.clone()),
                typ: type_expr_from_type(&pred.typ),
            })
            .collect(),
    }
}

fn render_type_decl(decl: &TypeDecl) -> String {
    let head = if decl.params.is_empty() {
        decl.name.to_string()
    } else {
        format!(
            "{} {}",
            decl.name,
            decl.params
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(" ")
        )
    };
    let variants = decl
        .variants
        .iter()
        .map(|variant| {
            if variant.args.is_empty() {
                variant.name.to_string()
            } else {
                format!(
                    "{} {}",
                    variant.name,
                    variant
                        .args
                        .iter()
                        .map(|arg| if matches!(arg, TypeExpr::Fun(..)) {
                            format!("({arg})")
                        } else {
                            arg.to_string()
                        })
                        .collect::<Vec<_>>()
                        .join(" ")
                )
            }
        })
        .collect::<Vec<_>>()
        .join(" | ");
    format!("pub type {head} = {variants}")
}

fn render_declare_fn_decl(decl: &DeclareFnDecl) -> String {
    let mut sig = decl.ret.clone();
    for (_, ann) in decl.params.iter().rev() {
        sig = TypeExpr::Fun(Span::default(), Box::new(ann.clone()), Box::new(sig));
    }
    let mut out = format!("pub declare fn {} : {}", decl.name.name, sig);
    if !decl.constraints.is_empty() {
        let preds = decl
            .constraints
            .iter()
            .map(|pred| format!("{} {}", pred.class, pred.typ))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(" where ");
        out.push_str(&preds);
    }
    out
}

fn render_virtual_decl(decl: &Decl) -> Option<String> {
    match decl {
        Decl::Type(td) => Some(render_type_decl(td)),
        Decl::DeclareFn(df) => Some(render_declare_fn_decl(df)),
        _ => None,
    }
}

fn decompose_fun_type(typ: &Type) -> (Vec<Type>, Type) {
    let mut params = Vec::new();
    let mut cur = typ.clone();
    while let Some((arg, ret)) = split_fun(&cur) {
        params.push(arg);
        cur = ret;
    }
    (params, cur)
}

fn type_expr_from_type(typ: &Type) -> TypeExpr {
    match typ.as_ref() {
        TypeKind::Var(tv) => {
            let name = tv
                .name
                .clone()
                .unwrap_or_else(|| Symbol::intern(&format!("t{}", tv.id)));
            TypeExpr::Name(Span::default(), NameRef::Unqualified(name))
        }
        TypeKind::Con(con) => TypeExpr::Name(Span::default(), NameRef::Unqualified(con.name())),
        TypeKind::App(fun, arg) => {
            if let TypeKind::App(head, err) = fun.as_ref()
                && let TypeKind::Con(con) = head.as_ref()
                && con.is_builtin(BuiltinTypeId::Result)
                && con.arity() == 2
            {
                let result = TypeExpr::Name(Span::default(), NameRef::Unqualified(con.name()));
                let ok_expr = type_expr_from_type(arg);
                let err_expr = type_expr_from_type(err);
                let app1 = TypeExpr::App(Span::default(), Box::new(result), Box::new(ok_expr));
                return TypeExpr::App(Span::default(), Box::new(app1), Box::new(err_expr));
            }
            TypeExpr::App(
                Span::default(),
                Box::new(type_expr_from_type(fun)),
                Box::new(type_expr_from_type(arg)),
            )
        }
        TypeKind::Fun(arg, ret) => TypeExpr::Fun(
            Span::default(),
            Box::new(type_expr_from_type(arg)),
            Box::new(type_expr_from_type(ret)),
        ),
        TypeKind::Tuple(elems) => TypeExpr::Tuple(
            Span::default(),
            elems.iter().map(type_expr_from_type).collect(),
        ),
        TypeKind::Record(fields) => TypeExpr::Record(
            Span::default(),
            fields
                .iter()
                .map(|(name, ty)| (name.clone(), type_expr_from_type(ty)))
                .collect(),
        ),
    }
}

fn module_local_type_names_from_declarations(declarations: &[String]) -> BTreeSet<Symbol> {
    let mut out = BTreeSet::new();
    for declaration in declarations {
        let mut s = declaration.trim_start();
        if let Some(rest) = s.strip_prefix("pub ") {
            s = rest.trim_start();
        }
        let Some(rest) = s.strip_prefix("type ") else {
            continue;
        };
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() {
            out.insert(Symbol::intern(&name));
        }
    }
    out
}

fn module_local_type_names_from_decls(decls: &[Decl]) -> BTreeSet<Symbol> {
    let mut out = BTreeSet::new();
    for decl in decls {
        if let Decl::Type(td) = decl {
            out.insert(td.name.clone());
        }
    }
    out
}

fn render_virtual_module_source(decls: &[Decl]) -> Option<String> {
    let rendered = decls
        .iter()
        .filter_map(render_virtual_decl)
        .collect::<Vec<_>>()
        .join("\n");
    (!rendered.is_empty()).then_some(rendered)
}

fn build_virtual_module_source<State: Clone + Send + Sync + 'static>(
    declarations: &[String],
    exports: &[Export<State>],
) -> String {
    let mut lines = declarations.to_vec();
    lines.extend(
        exports
            .iter()
            .map(|export| render_declare_fn_decl(&export.interface)),
    );
    lines.join("\n")
}

fn qualify_module_type_refs(
    typ: &Type,
    module_name: &str,
    local_type_names: &BTreeSet<Symbol>,
) -> Type {
    match typ.as_ref() {
        TypeKind::Con(tc) => {
            if let Some(name) = tc.user_name()
                && local_type_names.contains(name)
            {
                Type::con(virtual_export_name(module_name, name.as_ref()), tc.arity())
            } else {
                typ.clone()
            }
        }
        TypeKind::App(f, x) => Type::app(
            qualify_module_type_refs(f, module_name, local_type_names),
            qualify_module_type_refs(x, module_name, local_type_names),
        ),
        TypeKind::Fun(a, b) => Type::fun(
            qualify_module_type_refs(a, module_name, local_type_names),
            qualify_module_type_refs(b, module_name, local_type_names),
        ),
        TypeKind::Tuple(elems) => Type::tuple(
            elems
                .iter()
                .map(|t| qualify_module_type_refs(t, module_name, local_type_names))
                .collect(),
        ),
        TypeKind::Record(fields) => Type::new(TypeKind::Record(
            fields
                .iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        qualify_module_type_refs(v, module_name, local_type_names),
                    )
                })
                .collect(),
        )),
        TypeKind::Var(_) => typ.clone(),
    }
}

fn qualify_module_scheme_refs(
    scheme: &Scheme,
    module_name: &str,
    local_type_names: &BTreeSet<Symbol>,
) -> Scheme {
    let typ = qualify_module_type_refs(&scheme.typ, module_name, local_type_names);
    let preds = scheme
        .preds
        .iter()
        .map(|pred| {
            Predicate::new(
                pred.class.clone(),
                qualify_module_type_refs(&pred.typ, module_name, local_type_names),
            )
        })
        .collect();
    Scheme::new(scheme.vars.clone(), preds, typ)
}

/// Convert ADT collection conflicts into an embedder-facing `EngineError`.
///
/// # Examples
///
/// ```rust,ignore
/// use rex_engine::collect_adts_error_to_engine;
/// use rex_typesystem::{collect_adts_in_types, Type};
///
/// let err = collect_adts_in_types(vec![
///     Type::user_con("Thing", 1),
///     Type::user_con("Thing", 2),
/// ])
/// .unwrap_err();
///
/// let engine_err = collect_adts_error_to_engine(err);
/// assert!(engine_err.to_string().contains("conflicting ADT definitions"));
/// ```
pub fn collect_adts_error_to_engine(err: CollectAdtsError) -> EngineError {
    let details = err
        .conflicts
        .into_iter()
        .map(|conflict| {
            let defs = conflict
                .definitions
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            format!("{}: [{defs}]", conflict.name)
        })
        .collect::<Vec<_>>()
        .join("; ");
    EngineError::Custom(format!(
        "conflicting ADT definitions discovered in input types: {details}"
    ))
}

pub(crate) fn adt_family_error_to_engine(err: TypeError) -> EngineError {
    match err {
        TypeError::Internal(message) => EngineError::Custom(message),
        other => EngineError::Type(other),
    }
}

fn native_export_arg_types(
    scheme: &Scheme,
    arity: usize,
) -> Result<(Vec<Type>, Type), EngineError> {
    let mut args = Vec::with_capacity(arity);
    let mut rest = scheme.typ.clone();
    for _ in 0..arity {
        let Some((arg, tail)) = split_fun(&rest) else {
            return Err(EngineError::Internal(format!(
                "native export type `{}` does not accept {arity} argument(s)",
                scheme.typ
            )));
        };
        args.push(arg);
        rest = tail;
    }
    Ok((args, rest))
}

fn validate_native_export_scheme(scheme: &Scheme, arity: usize) -> Result<(), EngineError> {
    let _ = native_export_arg_types(scheme, arity)?;
    Ok(())
}

macro_rules! define_handler_impl {
    ([] ; $arity:literal ; $sig:ty) => {
        impl<State, F, R> HostFnSync<State, $sig> for F
        where
            State: Clone + Send + Sync + 'static,
            F: for<'a> Fn(&'a State) -> Result<R, EngineError> + Send + Sync + 'static,
            R: IntoRex + RexType,
        {
            fn interface_decl(export_name: &str) -> DeclareFnDecl {
                let scheme = Scheme::new(vec![], vec![], R::rex_type());
                declare_fn_decl_from_scheme(export_name, &scheme)
            }

            fn inject(
                self,
                engine: &mut Engine<State>,
                export_name: &str,
            ) -> Result<(), EngineError> {
                let name_sym = normalize_name(export_name);
                let func: SyncNativePointerCallable<State> = Arc::new(
                    move |engine, _: &Type, args: &[Pointer]| {
                        if args.len() != $arity {
                            return Err(EngineError::NativeArity {
                                name: name_sym.clone(),
                                expected: $arity,
                                got: args.len(),
                            });
                        }
                        let value = self(engine.state())?;
                        value.into_rex(engine.heap())?.pointer()
                    },
                );
                let scheme = Scheme::new(vec![], vec![], R::rex_type());
                let registration = NativeRegistration::sync(scheme, $arity, func);
                engine.register_native_registration(ROOT_MODULE_NAME, export_name, registration)
            }
        }

    };
    ([ $(($arg_ty:ident, $arg_name:ident, $idx:tt)),+ ] ; $arity:literal ; $sig:ty) => {
        impl<State, F, R, $($arg_ty),+> HostFnSync<State, $sig> for F
        where
            State: Clone + Send + Sync + 'static,
            F: for<'a> Fn(&'a State, $($arg_ty),+) -> Result<R, EngineError> + Send + Sync + 'static,
            R: IntoRex + RexType,
            $($arg_ty: FromRex + RexType),+
        {
            fn interface_decl(export_name: &str) -> DeclareFnDecl {
                let typ = native_fn_type!($($arg_ty),+ ; R);
                let scheme = Scheme::new(vec![], vec![], typ);
                declare_fn_decl_from_scheme(export_name, &scheme)
            }

            fn inject(
                self,
                engine: &mut Engine<State>,
                export_name: &str,
            ) -> Result<(), EngineError> {
                let name_sym = normalize_name(export_name);
                let func: SyncNativePointerCallable<State> = Arc::new(
                    move |engine, _: &Type, args: &[Pointer]| {
                        if args.len() != $arity {
                            return Err(EngineError::NativeArity {
                                name: name_sym.clone(),
                                expected: $arity,
                                got: args.len(),
                            });
                        }
                        let handles = engine.handles_from_pointers(args)?;
                        $(let $arg_name = {
                            let handle = &handles[$idx];
                            $arg_ty::from_rex(&handle)?
                        };)*
                        let value = self(engine.state(), $($arg_name),+)?;
                        value.into_rex(engine.heap())?.pointer()
                    },
                );
                let typ = native_fn_type!($($arg_ty),+ ; R);
                let scheme = Scheme::new(vec![], vec![], typ);
                let registration = NativeRegistration::sync(scheme, $arity, func);
                engine.register_native_registration(ROOT_MODULE_NAME, export_name, registration)
            }
        }

    };
}

impl<State> HostFnSync<State, NativeCallableSig> for (Scheme, usize, SyncNativeCallable<State>)
where
    State: Clone + Send + Sync + 'static,
{
    fn interface_decl(_export_name: &str) -> DeclareFnDecl {
        unreachable!("native callable handlers use interface_decl_for")
    }

    fn interface_decl_for(&self, export_name: &str) -> DeclareFnDecl {
        let (scheme, _, _) = self;
        declare_fn_decl_from_scheme(export_name, scheme)
    }

    fn inject(self, engine: &mut Engine<State>, export_name: &str) -> Result<(), EngineError> {
        let (scheme, arity, func) = self;
        validate_native_export_scheme(&scheme, arity)?;
        let pointer_func: SyncNativePointerCallable<State> =
            Arc::new(move |engine, typ: &Type, args: &[Pointer]| {
                let handles = engine.handles_from_pointers(args)?;
                let value = func(engine.clone(), typ, &handles)?;
                value.pointer_for_heap(engine.heap())
            });
        let registration = NativeRegistration::sync(scheme, arity, pointer_func);
        engine.register_native_registration(ROOT_MODULE_NAME, export_name, registration)
    }
}

macro_rules! define_async_handler_impl {
    ([] ; $arity:literal ; $sig:ty) => {
        impl<State, F, Fut, R> HostFnAsync<State, $sig> for F
        where
            State: Clone + Send + Sync + 'static,
            F: for<'a> Fn(&'a State) -> Fut + Send + Sync + 'static,
            Fut: Future<Output = Result<R, EngineError>> + Send + 'static,
            R: IntoRex + RexType,
        {
            fn interface_decl(export_name: &str) -> DeclareFnDecl {
                let scheme = Scheme::new(vec![], vec![], R::rex_type());
                declare_fn_decl_from_scheme(export_name, &scheme)
            }

            fn inject_async(
                self,
                engine: &mut Engine<State>,
                export_name: &str,
            ) -> Result<(), EngineError> {
                let f = Arc::new(self);
                let name_sym = normalize_name(export_name);
                let func: AsyncNativePointerCallable<State> = Arc::new(
                    move |engine, _: Type, args: Vec<Pointer>| -> NativeHandleFuture {
                        let f = Arc::clone(&f);
                        let name_sym = name_sym.clone();
                        let args = (|| {
                            if args.len() != $arity {
                                return Err(EngineError::NativeArity {
                                    name: name_sym.clone(),
                                    expected: $arity,
                                    got: args.len(),
                                });
                            }
                            Ok(())
                        })();
                        async move {
                            args?;
                            let value = f(engine.state()).await?;
                            value.into_rex(engine.heap())
                        }
                        .boxed()
                    },
                );
                let scheme = Scheme::new(vec![], vec![], R::rex_type());
                let registration = NativeRegistration::r#async(scheme, $arity, func);
                engine.register_native_registration(ROOT_MODULE_NAME, export_name, registration)
            }
        }
    };
    ([ $(($arg_ty:ident, $arg_name:ident, $idx:tt)),+ ] ; $arity:literal ; $sig:ty) => {
        impl<State, F, Fut, R, $($arg_ty),+> HostFnAsync<State, $sig> for F
        where
            State: Clone + Send + Sync + 'static,
            F: for<'a> Fn(&'a State, $($arg_ty),+) -> Fut + Send + Sync + 'static,
            Fut: Future<Output = Result<R, EngineError>> + Send + 'static,
            R: IntoRex + RexType,
            $($arg_ty: FromRex + RexType),+
        {
            fn interface_decl(export_name: &str) -> DeclareFnDecl {
                let typ = native_fn_type!($($arg_ty),+ ; R);
                let scheme = Scheme::new(vec![], vec![], typ);
                declare_fn_decl_from_scheme(export_name, &scheme)
            }

            fn inject_async(
                self,
                engine: &mut Engine<State>,
                export_name: &str,
            ) -> Result<(), EngineError> {
                let f = Arc::new(self);
                let name_sym = normalize_name(export_name);
                let func: AsyncNativePointerCallable<State> = Arc::new(
                    move |engine, _: Type, args: Vec<Pointer>| -> NativeHandleFuture {
                        let f = Arc::clone(&f);
                        let name_sym = name_sym.clone();
                        let args = (|| {
                            if args.len() != $arity {
                                return Err(EngineError::NativeArity {
                                    name: name_sym.clone(),
                                    expected: $arity,
                                    got: args.len(),
                                });
                            }
                            let handles = engine.handles_from_pointers(&args)?;
                            $(let $arg_name = {
                                let handle = &handles[$idx];
                                $arg_ty::from_rex(&handle)?
                            };)*
                            Ok(($($arg_name,)+))
                        })();
                        match args {
                            Ok(($($arg_name,)+)) => {
                                let future = f(engine.state(), $($arg_name),+);
                                async move {
                                    let value = future.await?;
                                    value.into_rex(engine.heap())
                                }
                                .boxed()
                            }
                            Err(err) => async move { Err(err) }.boxed(),
                        }
                    },
                );
                let typ = native_fn_type!($($arg_ty),+ ; R);
                let scheme = Scheme::new(vec![], vec![], typ);
                let registration = NativeRegistration::r#async(scheme, $arity, func);
                engine.register_native_registration(ROOT_MODULE_NAME, export_name, registration)
            }
        }
    };
}

impl<State> HostFnAsync<State, AsyncNativeCallableSig>
    for (Scheme, usize, AsyncNativeCallable<State>)
where
    State: Clone + Send + Sync + 'static,
{
    fn interface_decl(_export_name: &str) -> DeclareFnDecl {
        unreachable!("native async callable handlers use interface_decl_for")
    }

    fn interface_decl_for(&self, export_name: &str) -> DeclareFnDecl {
        let (scheme, _, _) = self;
        declare_fn_decl_from_scheme(export_name, scheme)
    }

    fn inject_async(
        self,
        engine: &mut Engine<State>,
        export_name: &str,
    ) -> Result<(), EngineError> {
        let (scheme, arity, func) = self;
        validate_native_export_scheme(&scheme, arity)?;
        let pointer_func: AsyncNativePointerCallable<State> = Arc::new(move |engine, typ, args| {
            let func = Arc::clone(&func);
            let handles = engine.handles_from_pointers(&args);
            async move {
                let handles = handles?;
                let value = func(engine.clone(), typ, handles).await?;
                value.pointer_for_heap(engine.heap())?;
                Ok(value)
            }
            .boxed()
        });
        let registration = NativeRegistration::r#async(scheme, arity, pointer_func);
        engine.register_native_registration(ROOT_MODULE_NAME, export_name, registration)
    }
}

#[derive(Clone)]
pub(crate) enum NativeCallable<State: Clone + Send + Sync + 'static> {
    Sync(SyncNativePointerCallable<State>),
    Scheduler(SchedulerNativeCallable<State>),
    Async(AsyncNativePointerCallable<State>),
}

impl<State: Clone + Send + Sync + 'static> PartialEq for NativeCallable<State> {
    fn eq(&self, _other: &NativeCallable<State>) -> bool {
        false
    }
}

impl<State: Clone + Send + Sync + 'static> std::fmt::Debug for NativeCallable<State> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        match self {
            NativeCallable::Sync(_) => write!(f, "Sync"),
            NativeCallable::Scheduler(_) => write!(f, "Scheduler"),
            NativeCallable::Async(_) => write!(f, "Async"),
        }
    }
}

pub(crate) struct NativeAsyncCall<State: Clone + Send + Sync + 'static> {
    callable: AsyncNativePointerCallable<State>,
    context: EvalContext,
    typ: Type,
    args: Vec<Pointer>,
}

impl<State> NativeAsyncCall<State>
where
    State: Clone + Send + Sync + 'static,
{
    fn new(
        callable: AsyncNativePointerCallable<State>,
        context: EvalContext,
        typ: Type,
        args: Vec<Pointer>,
    ) -> Self {
        Self {
            callable,
            context,
            typ,
            args,
        }
    }

    fn invoke(
        mut self,
        runtime: &RuntimeSnapshot<State>,
    ) -> Result<NativeHandleFuture, EngineError> {
        let mut protected = Vec::new();
        self.trace_pointers(&mut protected);
        let roots = runtime.heap.temp_roots(protected)?;
        let mut cursor = 0;
        self.refresh_from_roots(&roots, &mut cursor)?;
        let args = self.args;
        let future = (self.callable)(
            EvaluatorRef::new_with_context(runtime, self.context),
            self.typ,
            args,
        );
        Ok(runtime.async_call_policy.prepare(future))
    }

    fn trace_pointers(&self, out: &mut Vec<Pointer>) {
        if let Some(parent) = self.context.parent {
            out.push(parent);
        }
        out.extend(self.args.iter().copied());
    }

    fn refresh_from_roots(
        &mut self,
        roots: &TempRoots,
        cursor: &mut usize,
    ) -> Result<(), EngineError> {
        if self.context.parent.is_some() {
            self.context.parent = Some(roots.get(*cursor)?);
            *cursor += 1;
        }
        for arg in &mut self.args {
            *arg = roots.get(*cursor)?;
            *cursor += 1;
        }
        Ok(())
    }
}

pub(crate) enum NativeCallResult<State: Clone + Send + Sync + 'static> {
    Ready(Pointer),
    Pending(NativeAsyncCall<State>),
}

impl<State: Clone + Send + Sync + 'static> NativeCallable<State> {
    fn call_with_context(
        &self,
        runtime: &RuntimeSnapshot<State>,
        typ: Type,
        args: &[Pointer],
        context: EvalContext,
    ) -> Result<NativeCallResult<State>, EngineError> {
        match self {
            NativeCallable::Sync(f) => {
                (f)(EvaluatorRef::new_with_context(runtime, context), &typ, args)
                    .map(NativeCallResult::Ready)
            }
            NativeCallable::Scheduler(_) => Err(EngineError::Internal(
                "scheduler native called through pointer-returning native ABI".into(),
            )),
            NativeCallable::Async(f) => Ok(NativeCallResult::Pending(NativeAsyncCall::new(
                Arc::clone(f),
                context,
                typ,
                args.to_vec(),
            ))),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NativeFn {
    native_id: NativeId,
    name: Symbol,
    arity: usize,
    typ: Type,
    applied: Vec<Pointer>,
    applied_types: Vec<Type>,
}

enum NativeApplyResult<State: Clone + Send + Sync + 'static> {
    Value(Pointer),
    Task(NativeTask),
    Pending(NativeAsyncCall<State>),
}

impl NativeFn {
    fn new(native_id: NativeId, name: Symbol, arity: usize, typ: Type) -> Self {
        Self {
            native_id,
            name,
            arity,
            typ,
            applied: Vec::new(),
            applied_types: Vec::new(),
        }
    }

    pub(crate) fn from_parts(
        native_id: NativeId,
        name: Symbol,
        arity: usize,
        typ: Type,
        applied: Vec<Pointer>,
        applied_types: Vec<Type>,
    ) -> Self {
        Self {
            native_id,
            name,
            arity,
            typ,
            applied,
            applied_types,
        }
    }

    pub(crate) fn into_parts(self) -> (NativeId, Symbol, usize, Type, Vec<Pointer>, Vec<Type>) {
        (
            self.native_id,
            self.name,
            self.arity,
            self.typ,
            self.applied,
            self.applied_types,
        )
    }

    pub(crate) fn name(&self) -> &Symbol {
        &self.name
    }

    pub(crate) fn trace_pointers(&self, out: &mut Vec<Pointer>) {
        out.extend(self.applied.iter().copied());
    }

    pub(crate) fn rewrite_pointers(
        &mut self,
        rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, EngineError>,
    ) -> Result<(), EngineError> {
        for pointer in &mut self.applied {
            *pointer = rewrite(*pointer)?;
        }
        Ok(())
    }

    pub(crate) fn call_zero_with_context<State: Clone + Send + Sync + 'static>(
        &self,
        runtime: &RuntimeSnapshot<State>,
        context: EvalContext,
    ) -> Result<NativeCallResult<State>, EngineError> {
        if self.arity != 0 {
            return Err(EngineError::NativeArity {
                name: self.name.clone(),
                expected: self.arity,
                got: 0,
            });
        }
        runtime.native_callable(self.native_id)?.call_with_context(
            runtime,
            self.typ.clone(),
            &[],
            context,
        )
    }

    fn apply_with_context<State: Clone + Send + Sync + 'static>(
        mut self,
        runtime: &RuntimeSnapshot<State>,
        arg: Pointer,
        arg_type: Option<&Type>,
        context: EvalContext,
    ) -> Result<NativeApplyResult<State>, EngineError> {
        // `self` is an owned copy cloned from heap storage; we mutate it to
        // accumulate partial-application state and never mutate shared values.
        if self.arity == 0 {
            return Err(EngineError::NativeArity {
                name: self.name,
                expected: 0,
                got: 1,
            });
        }
        let (arg_ty, rest_ty) =
            split_fun(&self.typ).ok_or_else(|| EngineError::NotCallable(self.typ.to_string()))?;
        let actual_ty = resolve_arg_type(&runtime.heap, arg_type, &arg)?;
        let subst = unify(&arg_ty, &actual_ty).map_err(|_| EngineError::NativeType {
            expected: arg_ty.to_string(),
            got: actual_ty.to_string(),
        })?;
        self.typ = rest_ty.apply(&subst);
        self.applied.push(arg);
        self.applied_types.push(actual_ty);
        if is_function_type(&self.typ) {
            let NativeFn {
                native_id,
                name,
                arity,
                typ,
                applied,
                applied_types,
            } = self;
            return runtime
                .heap
                .alloc_ptr_native(native_id, name, arity, typ, applied, applied_types)
                .map(NativeApplyResult::Value);
        }

        let mut full_ty = self.typ.clone();
        for arg_ty in self.applied_types.iter().rev() {
            full_ty = Type::fun(arg_ty.clone(), full_ty);
        }

        match runtime.native_callable(self.native_id)? {
            NativeCallable::Scheduler(f) => {
                match f(
                    EvaluatorRef::new_with_context(runtime, context),
                    full_ty,
                    &self.applied,
                )? {
                    SchedulerNativeResult::Ready(value) => Ok(NativeApplyResult::Value(value)),
                    SchedulerNativeResult::Task(task) => Ok(NativeApplyResult::Task(task)),
                }
            }
            callable => {
                match callable.call_with_context(runtime, full_ty, &self.applied, context)? {
                    NativeCallResult::Ready(value) => Ok(NativeApplyResult::Value(value)),
                    NativeCallResult::Pending(future) => Ok(NativeApplyResult::Pending(future)),
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct OverloadedFn {
    name: Symbol,
    typ: Type,
    applied: Vec<Pointer>,
    applied_types: Vec<Type>,
}

impl OverloadedFn {
    pub(crate) fn new(name: Symbol, typ: Type) -> Self {
        Self {
            name,
            typ,
            applied: Vec::new(),
            applied_types: Vec::new(),
        }
    }

    pub(crate) fn from_parts(
        name: Symbol,
        typ: Type,
        applied: Vec<Pointer>,
        applied_types: Vec<Type>,
    ) -> Self {
        Self {
            name,
            typ,
            applied,
            applied_types,
        }
    }

    pub(crate) fn name(&self) -> &Symbol {
        &self.name
    }

    pub(crate) fn trace_pointers(&self, out: &mut Vec<Pointer>) {
        out.extend(self.applied.iter().copied());
    }

    pub(crate) fn rewrite_pointers(
        &mut self,
        rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, EngineError>,
    ) -> Result<(), EngineError> {
        for pointer in &mut self.applied {
            *pointer = rewrite(*pointer)?;
        }
        Ok(())
    }

    pub(crate) fn into_parts(self) -> (Symbol, Type, Vec<Pointer>, Vec<Type>) {
        (self.name, self.typ, self.applied, self.applied_types)
    }
}

#[derive(Clone)]
pub(crate) struct NativeImpl<State: Clone + Send + Sync + 'static> {
    id: NativeId,
    name: Symbol,
    arity: usize,
    scheme: Scheme,
    pub(crate) func: NativeCallable<State>,
}

impl<State: Clone + Send + Sync + 'static> NativeImpl<State> {
    pub(crate) fn to_native_fn(&self, typ: Type) -> NativeFn {
        NativeFn::new(self.id, self.name.clone(), self.arity, typ)
    }
}

#[derive(Clone)]
pub(crate) struct NativeRegistry<State: Clone + Send + Sync + 'static> {
    next_id: NativeId,
    entries: BTreeMap<Symbol, Vec<NativeImpl<State>>>,
    by_id: BTreeMap<NativeId, NativeImpl<State>>,
}

impl<State: Clone + Send + Sync + 'static> NativeRegistry<State> {
    fn insert(
        &mut self,
        name: Symbol,
        arity: usize,
        scheme: Scheme,
        func: NativeCallable<State>,
    ) -> Result<(), EngineError> {
        let entry = self.entries.entry(name.clone()).or_default();
        if entry.iter().any(|existing| existing.scheme == scheme) {
            return Err(EngineError::DuplicateImpl {
                name,
                typ: scheme.typ.to_string(),
            });
        }
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let imp = NativeImpl::<State> {
            id,
            name: name.clone(),
            arity,
            scheme,
            func,
        };
        self.by_id.insert(id, imp.clone());
        entry.push(imp);
        Ok(())
    }

    pub(crate) fn get(&self, name: &Symbol) -> Option<&[NativeImpl<State>]> {
        self.entries.get(name).map(|v| v.as_slice())
    }

    pub(crate) fn has_name(&self, name: &Symbol) -> bool {
        self.entries.contains_key(name)
    }

    fn by_id(&self, id: NativeId) -> Option<&NativeImpl<State>> {
        self.by_id.get(&id)
    }
}

impl<State: Clone + Send + Sync + 'static> Default for NativeRegistry<State> {
    fn default() -> Self {
        Self {
            next_id: 0,
            entries: BTreeMap::new(),
            by_id: BTreeMap::new(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct TypeclassInstance {
    head: Type,
    def_env: Environment,
    methods: BTreeMap<Symbol, Arc<TypedExpr>>,
}

#[derive(Default, Clone)]
pub(crate) struct TypeclassRegistry {
    entries: BTreeMap<Symbol, Vec<TypeclassInstance>>,
}

impl TypeclassRegistry {
    fn insert(
        &mut self,
        class: Symbol,
        head: Type,
        def_env: Environment,
        methods: BTreeMap<Symbol, Arc<TypedExpr>>,
    ) -> Result<(), EngineError> {
        let entry = self.entries.entry(class.clone()).or_default();
        for existing in entry.iter() {
            if unify(&existing.head, &head).is_ok() {
                return Err(EngineError::DuplicateTypeclassImpl {
                    class,
                    typ: head.to_string(),
                });
            }
        }
        entry.push(TypeclassInstance {
            head,
            def_env,
            methods,
        });
        Ok(())
    }

    pub(crate) fn resolve(
        &self,
        class: &Symbol,
        method: &Symbol,
        param_type: &Type,
    ) -> Result<(Environment, Arc<TypedExpr>, Subst), EngineError> {
        let instances =
            self.entries
                .get(class)
                .ok_or_else(|| EngineError::MissingTypeclassImpl {
                    class: class.clone(),
                    typ: param_type.to_string(),
                })?;

        let mut matches = Vec::new();
        for inst in instances {
            if let Ok(s) = unify(&inst.head, param_type) {
                matches.push((inst, s));
            }
        }
        match matches.len() {
            0 => Err(EngineError::MissingTypeclassImpl {
                class: class.clone(),
                typ: param_type.to_string(),
            }),
            1 => {
                let (inst, s) = matches.remove(0);
                let typed =
                    inst.methods
                        .get(method)
                        .ok_or_else(|| EngineError::MissingTypeclassImpl {
                            class: class.clone(),
                            typ: param_type.to_string(),
                        })?;
                Ok((inst.def_env.clone(), typed.clone(), s))
            }
            _ => Err(EngineError::AmbiguousTypeclassImpl {
                class: class.clone(),
                typ: param_type.to_string(),
            }),
        }
    }

    fn trace_pointers(&self, out: &mut Vec<Pointer>) {
        for instances in self.entries.values() {
            for instance in instances {
                instance.def_env.trace_pointers(out);
            }
        }
    }

    fn rewrite_pointers(
        &mut self,
        rewrite: &mut impl FnMut(Pointer) -> Result<Pointer, EngineError>,
    ) -> Result<(), EngineError> {
        for instances in self.entries.values_mut() {
            for instance in instances {
                instance.def_env.rewrite_pointers(rewrite)?;
            }
        }
        Ok(())
    }
}

pub struct Engine<State = ()>
where
    State: Clone + Send + Sync + 'static,
{
    pub state: Arc<State>,
    env: Environment,
    natives: NativeRegistry<State>,
    typeclasses: TypeclassRegistry,
    pub type_system: TypeSystem,
    typeclass_cache: Arc<Mutex<BTreeMap<(Symbol, Type), Pointer>>>,
    pub(crate) modules: ModuleSystem,
    injected_modules: BTreeSet<String>,
    pub(crate) module_exports_cache: BTreeMap<ModuleId, ModuleExports>,
    pub(crate) module_interface_cache: BTreeMap<ModuleId, Vec<Decl>>,
    pub(crate) module_sources: BTreeMap<ModuleId, String>,
    pub(crate) module_source_fingerprints: BTreeMap<ModuleId, String>,
    pub(crate) published_cycle_interfaces: BTreeSet<ModuleId>,
    default_imports: Vec<String>,
    virtual_modules: BTreeMap<String, VirtualModule>,
    module_local_type_names: BTreeMap<String, BTreeSet<Symbol>>,
    registration_module_context: Option<String>,
    async_call_policy: AsyncCallPolicy,
    execution_bounds: ExecutionBounds,
    pub heap: Heap,
}

#[derive(Clone)]
pub struct CompiledProgram {
    pub externs: CompiledExterns,
    link_contract: RuntimeLinkContract,
    pub(crate) env: Environment,
    pub(crate) expr: Arc<TypedExpr>,
}

impl CompiledProgram {
    pub(crate) fn new(
        externs: CompiledExterns,
        link_contract: RuntimeLinkContract,
        env: Environment,
        expr: TypedExpr,
    ) -> Self {
        Self {
            externs,
            link_contract,
            env,
            expr: Arc::new(expr),
        }
    }

    pub fn result_type(&self) -> &Type {
        &self.expr.typ
    }

    pub fn externs(&self) -> &CompiledExterns {
        &self.externs
    }

    pub fn link_contract(&self) -> &RuntimeLinkContract {
        &self.link_contract
    }

    pub fn link_fingerprint(&self) -> u64 {
        self.link_contract.fingerprint()
    }

    pub fn storage_boundary(&self) -> CompiledProgramBoundary {
        CompiledProgramBoundary {
            contains_prepared_expr: true,
            captures_process_local_env: true,
            serializable: false,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CompiledExterns {
    pub natives: Vec<Symbol>,
    pub class_methods: Vec<Symbol>,
}

impl CompiledExterns {
    pub fn is_empty(&self) -> bool {
        self.natives.is_empty() && self.class_methods.is_empty()
    }

    pub fn fingerprint(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        "natives".hash(&mut hasher);
        self.natives.hash(&mut hasher);
        "class_methods".hash(&mut hasher);
        self.class_methods.hash(&mut hasher);
        hasher.finish()
    }

    pub fn compatibility_with(&self, capabilities: &RuntimeCapabilities) -> RuntimeCompatibility {
        let natives = capabilities
            .natives
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        let class_methods = capabilities
            .class_methods
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();

        RuntimeCompatibility {
            expected_abi_version: capabilities.abi_version,
            actual_abi_version: capabilities.abi_version,
            missing_natives: self
                .natives
                .iter()
                .filter(|name| !natives.contains(*name))
                .cloned()
                .collect(),
            incompatible_natives: Vec::new(),
            missing_class_methods: self
                .class_methods
                .iter()
                .filter(|name| !class_methods.contains(*name))
                .cloned()
                .collect(),
            incompatible_class_methods: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub struct NativeRequirement {
    pub name: Symbol,
    pub typ: Type,
}

#[derive(Clone, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub struct ClassMethodRequirement {
    pub name: Symbol,
    pub typ: Type,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeLinkContract {
    pub abi_version: u32,
    pub natives: Vec<NativeRequirement>,
    pub class_methods: Vec<ClassMethodRequirement>,
}

impl RuntimeLinkContract {
    pub fn fingerprint(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.abi_version.hash(&mut hasher);
        self.natives.hash(&mut hasher);
        self.class_methods.hash(&mut hasher);
        hasher.finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompiledProgramBoundary {
    pub contains_prepared_expr: bool,
    pub captures_process_local_env: bool,
    pub serializable: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RuntimeCapabilities {
    pub abi_version: u32,
    pub natives: Vec<Symbol>,
    pub class_methods: Vec<Symbol>,
    pub(crate) native_impls: BTreeMap<Symbol, Vec<NativeCapability>>,
    pub(crate) class_method_impls: BTreeMap<Symbol, ClassMethodCapability>,
}

impl RuntimeCapabilities {
    pub fn fingerprint(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.abi_version.hash(&mut hasher);
        "natives".hash(&mut hasher);
        self.natives.hash(&mut hasher);
        "class_methods".hash(&mut hasher);
        self.class_methods.hash(&mut hasher);
        hasher.finish()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeCapability {
    pub name: Symbol,
    pub arity: usize,
    pub scheme: Scheme,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClassMethodCapability {
    pub name: Symbol,
    pub scheme: Scheme,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeCompatibility {
    pub expected_abi_version: u32,
    pub actual_abi_version: u32,
    pub missing_natives: Vec<Symbol>,
    pub incompatible_natives: Vec<Symbol>,
    pub missing_class_methods: Vec<Symbol>,
    pub incompatible_class_methods: Vec<Symbol>,
}

impl RuntimeCompatibility {
    pub fn is_compatible(&self) -> bool {
        self.expected_abi_version == self.actual_abi_version
            && self.missing_natives.is_empty()
            && self.incompatible_natives.is_empty()
            && self.missing_class_methods.is_empty()
            && self.incompatible_class_methods.is_empty()
    }
}

#[derive(Clone)]
pub struct RuntimeSnapshot<State = ()>
where
    State: Clone + Send + Sync + 'static,
{
    pub state: Arc<State>,
    pub(crate) natives: NativeRegistry<State>,
    pub(crate) typeclasses: TypeclassRegistry,
    pub type_system: TypeSystem,
    pub(crate) typeclass_cache: Arc<Mutex<BTreeMap<(Symbol, Type), Pointer>>>,
    pub(crate) async_call_policy: AsyncCallPolicy,
    pub(crate) execution_bounds: ExecutionBounds,
    pub heap: Heap,
}

impl<State> RuntimeSnapshot<State>
where
    State: Clone + Send + Sync + 'static,
{
    fn trace_pointers(&self, out: &mut Vec<Pointer>) -> Result<(), EngineError> {
        self.typeclasses.trace_pointers(out);
        let cache = self
            .typeclass_cache
            .lock()
            .map_err(|_| EngineError::Internal("typeclass cache poisoned".into()))?;
        out.extend(cache.values().copied());
        Ok(())
    }

    fn refresh_from_roots(
        &mut self,
        roots: &TempRoots,
        cursor: &mut usize,
    ) -> Result<(), EngineError> {
        self.typeclasses.rewrite_pointers(&mut |_| {
            let pointer = roots.get(*cursor)?;
            *cursor += 1;
            Ok(pointer)
        })?;
        let mut cache = self
            .typeclass_cache
            .lock()
            .map_err(|_| EngineError::Internal("typeclass cache poisoned".into()))?;
        for pointer in cache.values_mut() {
            *pointer = roots.get(*cursor)?;
            *cursor += 1;
        }
        Ok(())
    }
}

impl<State> Clone for Engine<State>
where
    State: Clone + Send + Sync + 'static,
{
    fn clone(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
            env: self.env.clone(),
            natives: self.natives.clone(),
            typeclasses: self.typeclasses.clone(),
            type_system: self.type_system.clone(),
            typeclass_cache: Arc::clone(&self.typeclass_cache),
            modules: self.modules.clone(),
            injected_modules: self.injected_modules.clone(),
            module_exports_cache: self.module_exports_cache.clone(),
            module_interface_cache: self.module_interface_cache.clone(),
            module_sources: self.module_sources.clone(),
            module_source_fingerprints: self.module_source_fingerprints.clone(),
            published_cycle_interfaces: self.published_cycle_interfaces.clone(),
            default_imports: self.default_imports.clone(),
            virtual_modules: self.virtual_modules.clone(),
            module_local_type_names: self.module_local_type_names.clone(),
            registration_module_context: self.registration_module_context.clone(),
            async_call_policy: self.async_call_policy.clone(),
            execution_bounds: self.execution_bounds,
            heap: self.heap.clone(),
        }
    }
}

impl<State> Default for Engine<State>
where
    State: Clone + Send + Sync + 'static + Default,
{
    fn default() -> Self {
        Self::new(State::default())
    }
}

macro_rules! native_fn_type {
    (; $ret:ident) => {
        $ret::rex_type()
    };
    ($arg_ty:ident $(, $rest:ident)* ; $ret:ident) => {
        Type::fun($arg_ty::rex_type(), native_fn_type!($($rest),* ; $ret))
    };
}

define_handler_impl!([] ; 0 ; fn() -> R);
define_handler_impl!([(A, a, 0)] ; 1 ; fn(A) -> R);
define_handler_impl!([(A, a, 0), (B, b, 1)] ; 2 ; fn(A, B) -> R);
define_handler_impl!([(A, a, 0), (B, b, 1), (C, c, 2)] ; 3 ; fn(A, B, C) -> R);
define_handler_impl!(
    [(A, a, 0), (B, b, 1), (C, c, 2), (D, d, 3)] ; 4 ; fn(A, B, C, D) -> R
);
define_handler_impl!(
    [(A, a, 0), (B, b, 1), (C, c, 2), (D, d, 3), (E, e, 4)] ; 5 ; fn(A, B, C, D, E) -> R
);
define_handler_impl!(
    [(A, a, 0), (B, b, 1), (C, c, 2), (D, d, 3), (E, e, 4), (G, g, 5)] ; 6 ; fn(A, B, C, D, E, G) -> R
);
define_handler_impl!(
    [(A, a, 0), (B, b, 1), (C, c, 2), (D, d, 3), (E, e, 4), (G, g, 5), (H, h, 6)] ; 7 ; fn(A, B, C, D, E, G, H) -> R
);
define_handler_impl!(
    [(A, a, 0), (B, b, 1), (C, c, 2), (D, d, 3), (E, e, 4), (G, g, 5), (H, h, 6), (I, i, 7)] ; 8 ; fn(A, B, C, D, E, G, H, I) -> R
);

define_async_handler_impl!([] ; 0 ; fn() -> R);
define_async_handler_impl!([(A, a, 0)] ; 1 ; fn(A) -> R);
define_async_handler_impl!([(A, a, 0), (B, b, 1)] ; 2 ; fn(A, B) -> R);
define_async_handler_impl!([(A, a, 0), (B, b, 1), (C, c, 2)] ; 3 ; fn(A, B, C) -> R);
define_async_handler_impl!(
    [(A, a, 0), (B, b, 1), (C, c, 2), (D, d, 3)] ; 4 ; fn(A, B, C, D) -> R
);
define_async_handler_impl!(
    [(A, a, 0), (B, b, 1), (C, c, 2), (D, d, 3), (E, e, 4)] ; 5 ; fn(A, B, C, D, E) -> R
);
define_async_handler_impl!(
    [(A, a, 0), (B, b, 1), (C, c, 2), (D, d, 3), (E, e, 4), (G, g, 5)] ; 6 ; fn(A, B, C, D, E, G) -> R
);
define_async_handler_impl!(
    [(A, a, 0), (B, b, 1), (C, c, 2), (D, d, 3), (E, e, 4), (G, g, 5), (H, h, 6)] ; 7 ; fn(A, B, C, D, E, G, H) -> R
);
define_async_handler_impl!(
    [(A, a, 0), (B, b, 1), (C, c, 2), (D, d, 3), (E, e, 4), (G, g, 5), (H, h, 6), (I, i, 7)] ; 8 ; fn(A, B, C, D, E, G, H, I) -> R
);

impl<State> Engine<State>
where
    State: Clone + Send + Sync + 'static,
{
    pub(crate) fn env_snapshot(&self) -> Environment {
        self.env.clone()
    }

    pub(crate) fn has_native_name(&self, name: &Symbol) -> bool {
        self.natives.has_name(name)
    }

    pub fn async_call_policy(&self) -> &AsyncCallPolicy {
        &self.async_call_policy
    }

    pub fn set_async_call_policy(&mut self, policy: AsyncCallPolicy) {
        self.async_call_policy = policy;
    }

    pub fn with_async_call_policy(mut self, policy: AsyncCallPolicy) -> Self {
        self.set_async_call_policy(policy);
        self
    }

    pub fn execution_bounds(&self) -> ExecutionBounds {
        self.execution_bounds
    }

    pub fn set_execution_bounds(&mut self, bounds: ExecutionBounds) {
        self.execution_bounds = bounds;
    }

    pub fn with_execution_bounds(mut self, bounds: ExecutionBounds) -> Self {
        self.set_execution_bounds(bounds);
        self
    }

    pub(crate) fn runtime_snapshot(&self) -> RuntimeSnapshot<State> {
        RuntimeSnapshot {
            state: Arc::clone(&self.state),
            natives: self.natives.clone(),
            typeclasses: self.typeclasses.clone(),
            type_system: self.type_system.clone(),
            typeclass_cache: Arc::clone(&self.typeclass_cache),
            async_call_policy: self.async_call_policy.clone(),
            execution_bounds: self.execution_bounds,
            heap: self.heap.clone(),
        }
    }

    pub(crate) fn runtime_capabilities_snapshot(&self) -> RuntimeCapabilities {
        let mut natives = self.natives.entries.keys().cloned().collect::<Vec<_>>();
        let mut class_methods = self
            .type_system
            .class_methods
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        let mut native_impls = BTreeMap::new();
        for (name, impls) in &self.natives.entries {
            let mut caps = impls
                .iter()
                .map(|imp| NativeCapability {
                    name: name.clone(),
                    arity: imp.arity,
                    scheme: imp.scheme.clone(),
                })
                .collect::<Vec<_>>();
            caps.sort_by(|a, b| {
                a.arity
                    .cmp(&b.arity)
                    .then_with(|| a.scheme.typ.to_string().cmp(&b.scheme.typ.to_string()))
            });
            native_impls.insert(name.clone(), caps);
        }
        let mut class_method_impls = BTreeMap::new();
        for (name, info) in &self.type_system.class_methods {
            class_method_impls.insert(
                name.clone(),
                ClassMethodCapability {
                    name: name.clone(),
                    scheme: info.scheme.clone(),
                },
            );
        }
        natives.sort();
        class_methods.sort();
        RuntimeCapabilities {
            abi_version: RUNTIME_LINK_ABI_VERSION,
            natives,
            class_methods,
            native_impls,
            class_method_impls,
        }
    }

    pub fn new(state: State) -> Self {
        Self {
            state: Arc::new(state),
            env: Environment::new(),
            natives: NativeRegistry::<State>::default(),
            typeclasses: TypeclassRegistry::default(),
            type_system: TypeSystem::new(),
            typeclass_cache: Arc::new(Mutex::new(BTreeMap::new())),
            modules: ModuleSystem::default(),
            injected_modules: BTreeSet::new(),
            module_exports_cache: BTreeMap::new(),
            module_interface_cache: BTreeMap::new(),
            module_sources: BTreeMap::new(),
            module_source_fingerprints: BTreeMap::new(),
            published_cycle_interfaces: BTreeSet::new(),
            default_imports: Vec::new(),
            virtual_modules: BTreeMap::new(),
            module_local_type_names: BTreeMap::new(),
            registration_module_context: None,
            async_call_policy: AsyncCallPolicy::default(),
            execution_bounds: ExecutionBounds::default(),
            heap: Heap::new(),
        }
    }

    pub fn with_prelude(state: State) -> Result<Self, EngineError> {
        Self::with_options(state, EngineOptions::default())
    }

    pub fn with_options(state: State, options: EngineOptions) -> Result<Self, EngineError> {
        let type_system = match options.prelude {
            PreludeMode::Enabled => TypeSystem::new_with_prelude()?,
            PreludeMode::Disabled => TypeSystem::new(),
        };
        let mut engine = Engine {
            state: Arc::new(state),
            env: Environment::new(),
            natives: NativeRegistry::<State>::default(),
            typeclasses: TypeclassRegistry::default(),
            type_system,
            typeclass_cache: Arc::new(Mutex::new(BTreeMap::new())),
            modules: ModuleSystem::default(),
            injected_modules: BTreeSet::new(),
            module_exports_cache: BTreeMap::new(),
            module_interface_cache: BTreeMap::new(),
            module_sources: BTreeMap::new(),
            module_source_fingerprints: BTreeMap::new(),
            published_cycle_interfaces: BTreeSet::new(),
            default_imports: options.default_imports,
            virtual_modules: BTreeMap::new(),
            module_local_type_names: BTreeMap::new(),
            registration_module_context: None,
            async_call_policy: AsyncCallPolicy::default(),
            execution_bounds: ExecutionBounds::default(),
            heap: Heap::new(),
        };
        if matches!(options.prelude, PreludeMode::Enabled) {
            engine.inject_prelude()?;
            engine.inject_prelude_virtual_module()?;
        }
        Ok(engine)
    }

    pub fn into_heap(self) -> Heap {
        self.heap
    }

    pub fn set_default_imports(&mut self, imports: Vec<String>) {
        self.default_imports = imports;
    }

    pub fn default_imports(&self) -> &[String] {
        &self.default_imports
    }

    /// Return a markdown document that inventories the currently-registered
    /// engine state.
    ///
    /// The report includes:
    /// - summary counts
    /// - modules and exports
    /// - ADTs
    /// - functions/values in the type environment
    /// - type classes, methods, and instances
    /// - native implementations
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use rex_engine::Engine;
    ///
    /// let engine = Engine::with_prelude(()).unwrap();
    /// let md = engine.registry_markdown();
    ///
    /// assert!(md.contains("# Engine Registry"));
    /// assert!(md.contains("## ADTs"));
    /// ```
    pub fn registry_markdown(&self) -> String {
        fn module_anchor(id: &ModuleId) -> String {
            let raw = format!("module-{id}").to_ascii_lowercase();
            let mut out = String::with_capacity(raw.len());
            let mut prev_dash = false;
            for ch in raw.chars() {
                let keep = ch.is_ascii_alphanumeric();
                let mapped = if keep { ch } else { '-' };
                if mapped == '-' {
                    if prev_dash {
                        continue;
                    }
                    prev_dash = true;
                } else {
                    prev_dash = false;
                }
                out.push(mapped);
            }
            out.trim_matches('-').to_string()
        }

        fn symbol_list(symbols: &[Symbol]) -> String {
            if symbols.is_empty() {
                "(none)".to_string()
            } else {
                symbols
                    .iter()
                    .map(|s| format!("`{s}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        }

        let mut out = String::new();
        let _ = writeln!(&mut out, "# Engine Registry");
        let _ = writeln!(&mut out);
        let mut module_ids: BTreeMap<String, ModuleId> = BTreeMap::new();
        for id in self.module_exports_cache.keys() {
            module_ids.insert(id.to_string(), id.clone());
        }
        for id in self.module_sources.keys() {
            module_ids.insert(id.to_string(), id.clone());
        }
        for module_name in self.virtual_modules.keys() {
            let id = ModuleId::Virtual(module_name.clone());
            module_ids.insert(id.to_string(), id);
        }
        for module_name in &self.injected_modules {
            let id = ModuleId::Virtual(module_name.clone());
            module_ids.insert(id.to_string(), id);
        }

        let _ = writeln!(&mut out, "## Summary");
        let env_value_count = self.type_system.env.values.size();
        let native_impl_count: usize = self.natives.entries.values().map(Vec::len).sum();
        let class_count = self.type_system.classes.classes.len();
        let class_instance_count: usize = self
            .type_system
            .classes
            .instances
            .values()
            .map(Vec::len)
            .sum();
        let _ = writeln!(&mut out, "- Modules (all kinds): {}", module_ids.len());
        let _ = writeln!(
            &mut out,
            "- Injected modules: {}",
            self.injected_modules.len()
        );
        let _ = writeln!(
            &mut out,
            "- Virtual modules: {}",
            self.virtual_modules.len()
        );
        let _ = writeln!(&mut out, "- ADTs: {}", self.type_system.adts.len());
        let _ = writeln!(
            &mut out,
            "- Values/functions in type env: {env_value_count}"
        );
        let _ = writeln!(&mut out, "- Type classes: {class_count}");
        let _ = writeln!(&mut out, "- Type class instances: {class_instance_count}");
        let _ = writeln!(&mut out, "- Native implementations: {native_impl_count}");
        let _ = writeln!(&mut out);

        let _ = writeln!(&mut out, "## Module Index");
        if module_ids.is_empty() {
            let _ = writeln!(&mut out, "_No modules registered._");
        } else {
            for (display, id) in &module_ids {
                let anchor = module_anchor(id);
                let _ = writeln!(&mut out, "- [`{display}`](#{anchor})");
            }
        }
        let _ = writeln!(&mut out);

        let _ = writeln!(&mut out, "## Modules");
        if module_ids.is_empty() {
            let _ = writeln!(&mut out, "_No modules registered._");
            let _ = writeln!(&mut out);
        } else {
            for (display, id) in module_ids {
                let anchor = module_anchor(&id);
                let _ = writeln!(&mut out, "<a id=\"{anchor}\"></a>");
                let _ = writeln!(&mut out, "### `{display}`");
                let virtual_source = match &id {
                    ModuleId::Virtual(name) => self.virtual_modules.get(name).and_then(|module| {
                        module
                            .source
                            .clone()
                            .or_else(|| render_virtual_module_source(&module.decls))
                    }),
                    _ => None,
                };
                if let Some(source) = self.module_sources.get(&id).cloned().or(virtual_source) {
                    if source.trim().is_empty() {
                        let _ = writeln!(&mut out, "_Module source is empty._");
                    } else {
                        let _ = writeln!(&mut out, "```rex");
                        let _ = writeln!(&mut out, "{}", source.trim_end());
                        let _ = writeln!(&mut out, "```");
                    }
                } else {
                    let _ = writeln!(&mut out, "_No captured source for this module._");
                }

                let exports = self.module_exports_cache.get(&id).or_else(|| match &id {
                    ModuleId::Virtual(name) => self.virtual_modules.get(name).map(|m| &m.exports),
                    _ => None,
                });
                if let Some(exports) = exports {
                    let mut values: Vec<Symbol> = exports.value_names();
                    let mut types: Vec<Symbol> = exports.type_names();
                    let mut classes: Vec<Symbol> = exports.class_names();
                    values.sort();
                    types.sort();
                    classes.sort();
                    let _ = writeln!(&mut out, "- Values: {}", symbol_list(&values));
                    let _ = writeln!(&mut out, "- Types: {}", symbol_list(&types));
                    let _ = writeln!(&mut out, "- Classes: {}", symbol_list(&classes));
                } else {
                    let _ = writeln!(&mut out, "- Exports: (none cached)");
                }
                let _ = writeln!(&mut out);
            }
        }

        let _ = writeln!(&mut out, "## ADTs");
        if self.type_system.adts.is_empty() {
            let _ = writeln!(&mut out, "_No ADTs registered._");
            let _ = writeln!(&mut out);
        } else {
            let mut adts: Vec<&AdtDecl> = self.type_system.adts.values().collect();
            adts.sort_by(|a, b| a.name.cmp(&b.name));
            for adt in adts {
                let params = if adt.params.is_empty() {
                    "(none)".to_string()
                } else {
                    adt.params
                        .iter()
                        .map(|p| format!("`{}`", p.name))
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                let _ = writeln!(&mut out, "### `{}`", adt.name);
                let _ = writeln!(&mut out, "- Parameters: {params}");
                if adt.variants.is_empty() {
                    let _ = writeln!(&mut out, "- Variants: (none)");
                } else {
                    let mut variants = adt.variants.clone();
                    variants.sort_by(|a, b| a.name.cmp(&b.name));
                    let _ = writeln!(&mut out, "- Variants:");
                    for variant in variants {
                        if variant.args.is_empty() {
                            let _ = writeln!(&mut out, "  - `{}`", variant.name);
                        } else {
                            let args = variant
                                .args
                                .iter()
                                .map(ToString::to_string)
                                .collect::<Vec<_>>()
                                .join(", ");
                            let _ = writeln!(&mut out, "  - `{}`({args})", variant.name);
                        }
                    }
                }
                let _ = writeln!(&mut out);
            }
        }

        let _ = writeln!(&mut out, "## Functions and Values");
        if self.type_system.env.values.is_empty() {
            let _ = writeln!(&mut out, "_No values registered._");
            let _ = writeln!(&mut out);
        } else {
            let mut names: Vec<Symbol> = self
                .type_system
                .env
                .values
                .iter()
                .map(|(name, _)| name.clone())
                .collect();
            names.sort();
            for name in names {
                if let Some(schemes) = self.type_system.env.lookup(&name) {
                    let mut scheme_strs: Vec<String> =
                        schemes.iter().map(|s| s.typ.to_string()).collect();
                    scheme_strs.sort();
                    scheme_strs.dedup();
                    let joined = scheme_strs
                        .into_iter()
                        .map(|s| format!("`{s}`"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    let _ = writeln!(&mut out, "- `{name}`: {joined}");
                }
            }
            let _ = writeln!(&mut out);
        }

        let _ = writeln!(&mut out, "## Type Classes");
        if self.type_system.classes.classes.is_empty() {
            let _ = writeln!(&mut out, "_No type classes registered._");
            let _ = writeln!(&mut out);
        } else {
            let mut class_names: Vec<Symbol> =
                self.type_system.classes.classes.keys().cloned().collect();
            class_names.sort();
            for class_name in class_names {
                let supers = self.type_system.classes.supers_of(&class_name);
                let mut supers_sorted = supers;
                supers_sorted.sort();
                let _ = writeln!(&mut out, "### `{class_name}`");
                let _ = writeln!(&mut out, "- Superclasses: {}", symbol_list(&supers_sorted));

                let mut methods: Vec<(Symbol, String)> = self
                    .type_system
                    .class_methods
                    .iter()
                    .filter(|(_, info)| info.class == class_name)
                    .map(|(name, info)| (name.clone(), info.scheme.typ.to_string()))
                    .collect();
                methods.sort_by(|a, b| a.0.cmp(&b.0));
                if methods.is_empty() {
                    let _ = writeln!(&mut out, "- Methods: (none)");
                } else {
                    let _ = writeln!(&mut out, "- Methods:");
                    for (method, scheme) in methods {
                        let _ = writeln!(&mut out, "  - `{method}`: `{scheme}`");
                    }
                }

                let mut instances = self
                    .type_system
                    .classes
                    .instances
                    .get(&class_name)
                    .cloned()
                    .unwrap_or_default();
                instances.sort_by_key(|a| a.head.typ.to_string());
                if instances.is_empty() {
                    let _ = writeln!(&mut out, "- Instances: (none)");
                } else {
                    let _ = writeln!(&mut out, "- Instances:");
                    for instance in instances {
                        let ctx = if instance.context.is_empty() {
                            String::new()
                        } else {
                            let mut parts: Vec<String> = instance
                                .context
                                .iter()
                                .map(|pred| format!("{} {}", pred.class, pred.typ))
                                .collect();
                            parts.sort();
                            format!("({}) => ", parts.join(", "))
                        };
                        let _ = writeln!(
                            &mut out,
                            "  - `{}{} {}`",
                            ctx, instance.head.class, instance.head.typ
                        );
                    }
                }
                let _ = writeln!(&mut out);
            }
        }

        let _ = writeln!(&mut out, "## Native Implementations");
        if self.natives.entries.is_empty() {
            let _ = writeln!(&mut out, "_No native implementations registered._");
        } else {
            let mut native_names: Vec<Symbol> = self.natives.entries.keys().cloned().collect();
            native_names.sort();
            for name in native_names {
                if let Some(impls) = self.natives.get(&name) {
                    let mut rows: Vec<(usize, String)> = impls
                        .iter()
                        .map(|imp| (imp.arity, imp.scheme.typ.to_string()))
                        .collect();
                    rows.sort_by(|a, b| a.1.cmp(&b.1));
                    let _ = writeln!(&mut out, "### `{name}`");
                    for (arity, typ) in rows {
                        let _ = writeln!(&mut out, "- arity `{arity}`, type `{typ}`");
                    }
                    let _ = writeln!(&mut out);
                }
            }
        }

        out
    }

    pub fn inject_module(&mut self, module: Module<State>) -> Result<(), EngineError> {
        let module_name = module.name.trim().to_string();
        if module_name.is_empty() {
            return Err(EngineError::Internal("module name cannot be empty".into()));
        }
        let is_global = module_name == ROOT_MODULE_NAME;
        if !is_global && self.injected_modules.contains(&module_name) {
            return Err(EngineError::Internal(format!(
                "module `{module_name}` already injected"
            )));
        }

        if is_global {
            for adt in &module.staged_adts {
                self.inject_adt(adt.clone())?;
            }

            let staged_adt_names: BTreeSet<Symbol> = module
                .staged_adts
                .iter()
                .map(|adt| adt.name.clone())
                .collect();
            let decls = if module.raw_declarations.is_empty() {
                module
                    .structured_decls
                    .iter()
                    .filter(|decl| match decl {
                        Decl::Type(ty) => !staged_adt_names.contains(&ty.name),
                        _ => true,
                    })
                    .cloned()
                    .collect()
            } else {
                let source = module.raw_declarations.join("\n");
                let context = ModuleId::Virtual(ROOT_MODULE_NAME.to_string());
                parse_program_from_source(&source, Some(&context))?.decls
            };

            for export in module.exports {
                self.inject_module_export(ROOT_MODULE_NAME, export)?;
            }
            self.inject_decls(&decls)?;
            return Ok(());
        }

        let module_id = ModuleId::Virtual(module_name.clone());

        if module.raw_declarations.is_empty() {
            let mut decls = module.structured_decls.clone();
            decls.extend(
                module
                    .exports
                    .iter()
                    .map(|export| Decl::DeclareFn(export.interface.clone())),
            );
            let local_type_names = module_local_type_names_from_decls(&decls);
            self.module_local_type_names
                .insert(module_name.clone(), local_type_names);

            let program = Program {
                decls,
                expr: Arc::new(Expr::Tuple(Span::default(), vec![])),
            };
            let prefix = prefix_for_module(&module_id);
            let exports = crate::modules::exports_from_program(&program, &prefix, &module_id);
            let qualified = qualify_program(&program, &prefix);
            let interfaces = interface_decls_from_program(&qualified);
            self.module_exports_cache
                .insert(module_id.clone(), exports.clone());
            self.module_interface_cache
                .insert(module_id.clone(), interfaces.clone());
            self.virtual_modules.insert(
                module_name.clone(),
                VirtualModule {
                    exports,
                    decls: program.decls.clone(),
                    source: None,
                },
            );

            for export in module.exports {
                self.inject_module_export(&module_name, export)?;
            }

            self.inject_decls(&qualified.decls)?;
            let resolver_module_name = module_name.clone();
            let resolver_program = program;
            self.add_resolver(
                format!("injected:{module_name}"),
                move |req: ResolveRequest| {
                    let requested = req
                        .module_name
                        .split_once('#')
                        .map(|(base, _)| base)
                        .unwrap_or(req.module_name.as_str());
                    if requested != resolver_module_name {
                        return Ok(None);
                    }
                    Ok(Some(ResolvedModule {
                        id: ModuleId::Virtual(resolver_module_name.clone()),
                        content: ResolvedModuleContent::Program(resolver_program.clone()),
                    }))
                },
            );
        } else {
            let full_source =
                build_virtual_module_source(&module.raw_declarations, &module.exports);
            let local_type_names =
                module_local_type_names_from_declarations(&module.raw_declarations);
            self.module_local_type_names
                .insert(module_name.clone(), local_type_names);
            self.module_sources
                .insert(module_id.clone(), full_source.clone());

            for export in module.exports {
                self.inject_module_export(&module_name, export)?;
            }
            let resolver_module_name = module_name.clone();
            let resolver_source = full_source;
            self.add_resolver(
                format!("injected:{module_name}"),
                move |req: ResolveRequest| {
                    let requested = req
                        .module_name
                        .split_once('#')
                        .map(|(base, _)| base)
                        .unwrap_or(req.module_name.as_str());
                    if requested != resolver_module_name {
                        return Ok(None);
                    }
                    Ok(Some(ResolvedModule {
                        id: ModuleId::Virtual(resolver_module_name.clone()),
                        content: ResolvedModuleContent::Source(resolver_source.clone()),
                    }))
                },
            );
        }

        self.injected_modules.insert(module_name);
        Ok(())
    }

    fn module_export_symbol(module_name: &str, export_name: &str) -> String {
        if module_name == ROOT_MODULE_NAME {
            normalize_name(export_name).to_string()
        } else {
            virtual_export_name(module_name, export_name)
        }
    }

    fn inject_module_export(
        &mut self,
        module_name: &str,
        export: Export<State>,
    ) -> Result<(), EngineError> {
        let Export {
            name,
            interface: _,
            injector,
        } = export;
        let qualified_name = Self::module_export_symbol(module_name, &name);
        let previous_context = self.registration_module_context.clone();
        self.registration_module_context = if module_name == ROOT_MODULE_NAME {
            None
        } else {
            Some(module_name.to_string())
        };
        let result = injector(self, &qualified_name);
        self.registration_module_context = previous_context;
        result
    }

    pub(crate) fn inject_root_export(&mut self, export: Export<State>) -> Result<(), EngineError> {
        self.inject_module_export(ROOT_MODULE_NAME, export)
    }

    fn register_native_registration(
        &mut self,
        module_name: &str,
        export_name: &str,
        registration: NativeRegistration<State>,
    ) -> Result<(), EngineError> {
        let NativeRegistration {
            mut scheme,
            arity,
            callable,
        } = registration;
        let scheme_module = if module_name == ROOT_MODULE_NAME {
            self.registration_module_context
                .as_deref()
                .unwrap_or(ROOT_MODULE_NAME)
        } else {
            module_name
        };
        if scheme_module != ROOT_MODULE_NAME
            && let Some(local_type_names) = self.module_local_type_names.get(scheme_module)
        {
            scheme = qualify_module_scheme_refs(&scheme, scheme_module, local_type_names);
        }
        let name = normalize_name(&Self::module_export_symbol(module_name, export_name));
        self.register_native(name, scheme, arity, callable)
    }

    pub(crate) fn export<Sig, H>(
        &mut self,
        name: impl Into<String>,
        handler: H,
    ) -> Result<(), EngineError>
    where
        H: HostFnSync<State, Sig>,
    {
        self.inject_root_export(Export::from_handler(name, handler)?)
    }

    pub(crate) fn export_native<F>(
        &mut self,
        name: impl Into<String>,
        scheme: Scheme,
        arity: usize,
        handler: F,
    ) -> Result<(), EngineError>
    where
        F: for<'a> Fn(EvaluatorRef<State>, &'a Type, &'a [Handle]) -> Result<Handle, EngineError>
            + Send
            + Sync
            + 'static,
    {
        validate_native_export_scheme(&scheme, arity)?;
        let name = name.into();
        let handler = Arc::new(handler);
        let func: SyncNativePointerCallable<State> =
            Arc::new(move |engine, typ: &Type, args: &[Pointer]| {
                let handles = engine.handles_from_pointers(args)?;
                let value = handler(engine.clone(), typ, &handles)?;
                value.pointer_for_heap(engine.heap())
            });
        let registration = NativeRegistration::sync(scheme, arity, func);
        self.register_native_registration(ROOT_MODULE_NAME, &name, registration)
    }

    pub fn inject_rex_default_instance<T>(&mut self) -> Result<(), EngineError>
    where
        T: RexType + RexDefault<State>,
    {
        let class = Symbol::intern("Default");
        let method = Symbol::intern("default");
        let head_ty = T::rex_type();

        if !self.type_system.class_methods.contains_key(&method) {
            return Err(EngineError::UnknownVar(method));
        }
        if !head_ty.ftv().is_empty() {
            return Err(EngineError::UnsupportedExpr);
        }

        if let Some(instances) = self.type_system.classes.instances.get(&class)
            && instances
                .iter()
                .any(|existing| unify(&existing.head.typ, &head_ty).is_ok())
        {
            return Err(EngineError::DuplicateTypeclassImpl {
                class,
                typ: head_ty.to_string(),
            });
        }

        let native_name = format!(
            "__rex_default_for_{}",
            sanitize_type_name_for_symbol(&head_ty)
        );
        let native_scheme = Scheme::new(vec![], vec![], head_ty.clone());
        let engine_for_default = self.clone();
        self.export_native(
            native_name.clone(),
            native_scheme,
            0,
            move |engine, _, _| {
                let _ = engine;
                T::rex_default(&engine_for_default)
            },
        )?;

        self.type_system.register_instance(
            "Default",
            Instance::new(vec![], Predicate::new(class.clone(), head_ty.clone())),
        );

        let mut methods: BTreeMap<Symbol, Arc<TypedExpr>> = BTreeMap::new();
        methods.insert(
            method.clone(),
            Arc::new(TypedExpr::new(
                head_ty.clone(),
                TypedExprKind::Var {
                    name: Symbol::intern(&native_name),
                    overloads: vec![],
                },
            )),
        );

        self.typeclasses
            .insert(class, head_ty, self.env.clone(), methods)?;

        Ok(())
    }

    pub(crate) fn export_native_scheduler<F>(
        &mut self,
        name: impl Into<String>,
        scheme: Scheme,
        arity: usize,
        handler: F,
    ) -> Result<(), EngineError>
    where
        F: for<'a> Fn(
                EvaluatorRef<State>,
                Type,
                Vec<Pointer>,
            ) -> Result<SchedulerNativeResult, EngineError>
            + Send
            + Sync
            + 'static,
    {
        validate_native_export_scheme(&scheme, arity)?;
        let name = name.into();
        let handler = Arc::new(handler);
        let func: SchedulerNativeCallable<State> = Arc::new(move |engine, typ, args| {
            let handler = Arc::clone(&handler);
            handler(engine, typ, args.to_vec())
        });
        let registration = NativeRegistration::scheduler(scheme, arity, func);
        self.register_native_registration(ROOT_MODULE_NAME, &name, registration)
    }

    pub(crate) fn export_value<V: IntoRex + RexType>(
        &mut self,
        name: &str,
        value: V,
    ) -> Result<(), EngineError> {
        let typ = V::rex_type();
        let value = value.into_rex(&self.heap)?;
        let func: SyncNativePointerCallable<State> =
            Arc::new(move |_engine, _: &Type, _args: &[Pointer]| value.pointer());
        let scheme = Scheme::new(vec![], vec![], typ);
        let registration = NativeRegistration::sync(scheme, 0, func);
        self.register_native_registration(ROOT_MODULE_NAME, name, registration)
    }

    pub fn adt_decl(&mut self, name: &str, params: &[&str]) -> AdtDecl {
        let name_sym = Symbol::intern(name);
        let param_syms: Vec<Symbol> = params.iter().map(|p| Symbol::intern(p)).collect();
        AdtDecl::new(&name_sym, &param_syms, &mut self.type_system.supply)
    }

    /// Seed an `AdtDecl` from a Rex type constructor.
    ///
    /// Accepted shapes:
    /// - `Type::con("Foo", 0)` -> `Foo` with no params
    /// - `Foo a b` (where args are type vars) -> `Foo` with params inferred from vars
    /// - `Type::con("Foo", n)` (bare higher-kinded head) -> `Foo` with generated params `t0..t{n-1}`
    pub fn adt_decl_from_type(&mut self, typ: &Type) -> Result<AdtDecl, EngineError> {
        let (name, arity, args) = type_head_and_args(typ)?;
        let param_names: Vec<String> = if args.is_empty() {
            (0..arity).map(|i| format!("t{i}")).collect()
        } else {
            let mut names = Vec::with_capacity(args.len());
            for arg in args {
                match arg.as_ref() {
                    TypeKind::Var(tv) => {
                        let name = tv
                            .name
                            .as_ref()
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| format!("t{}", tv.id));
                        names.push(name);
                    }
                    _ => {
                        return Err(EngineError::Custom(format!(
                            "cannot infer ADT params from `{typ}`: expected type variables, got `{arg}`"
                        )));
                    }
                }
            }
            names
        };
        let param_refs: Vec<&str> = param_names.iter().map(|s| s.as_str()).collect();
        Ok(self.adt_decl(name.as_ref(), &param_refs))
    }

    /// Same as `adt_decl_from_type`, but uses explicit parameter names.
    pub fn adt_decl_from_type_with_params(
        &mut self,
        typ: &Type,
        params: &[&str],
    ) -> Result<AdtDecl, EngineError> {
        let (name, arity, _args) = type_head_and_args(typ)?;
        if arity != params.len() {
            return Err(EngineError::Custom(format!(
                "type `{}` expects {} parameters, got {}",
                name,
                arity,
                params.len()
            )));
        }
        Ok(self.adt_decl(name.as_ref(), params))
    }

    pub fn inject_rex_adt<T: RexAdt>(&mut self) -> Result<(), EngineError> {
        for adt in order_adt_family(T::rex_adt_family()?).map_err(adt_family_error_to_engine)? {
            self.inject_adt(adt)?;
        }
        Ok(())
    }

    pub(crate) fn inject_adt(&mut self, adt: AdtDecl) -> Result<(), EngineError> {
        let register_type = match self.type_system.adts.get(&adt.name) {
            Some(existing) if adt_shape_eq(existing, &adt) => false,
            Some(existing) => {
                return Err(EngineError::Custom(format!(
                    "conflicting ADT registration for `{}`: existing={} new={}",
                    adt.name,
                    adt_shape(existing),
                    adt_shape(&adt)
                )));
            }
            None => true,
        };

        // Type system gets the constructor schemes; runtime gets constructor functions
        // that build `Cell::Adt` with the constructor tag and evaluated args.
        if register_type {
            self.type_system.register_adt(&adt);
        }
        for (ctor, scheme) in adt.constructor_schemes() {
            if self
                .natives
                .get(&ctor)
                .is_some_and(|existing| existing.iter().any(|imp| imp.scheme == scheme))
            {
                continue;
            }
            let ctor_name = ctor.clone();
            let func: SyncNativePointerCallable<State> = Arc::new(
                move |engine: EvaluatorRef<State>, _: &Type, args: &[Pointer]| {
                    engine
                        .heap()
                        .alloc_ptr_adt(runtime_ctor_symbol(&ctor_name), args.to_vec())
                },
            );
            let arity = type_arity(&scheme.typ);
            self.register_native(ctor, scheme, arity, NativeCallable::Sync(func))?;
        }
        Ok(())
    }

    pub(crate) fn inject_type_decl(&mut self, decl: &TypeDecl) -> Result<(), EngineError> {
        let adt = self
            .type_system
            .adt_from_decl(decl)
            .map_err(EngineError::Type)?;
        self.inject_adt(adt)
    }

    pub(crate) fn inject_class_decl(&mut self, decl: &ClassDecl) -> Result<(), EngineError> {
        self.type_system
            .register_class_decl(decl)
            .map_err(EngineError::Type)
    }

    pub(crate) fn inject_instance_decl(&mut self, decl: &InstanceDecl) -> Result<(), EngineError> {
        let prepared = self
            .type_system
            .register_instance_decl(decl)
            .map_err(EngineError::Type)?;
        self.register_typeclass_instance(decl, &prepared)
    }

    pub(crate) fn inject_fn_decls(&mut self, decls: &[FnDecl]) -> Result<(), EngineError> {
        if decls.is_empty() {
            return Ok(());
        }

        // Register declared types first so bodies can typecheck mutually-recursively.
        self.type_system
            .register_fn_decls(decls)
            .map_err(EngineError::Type)?;

        // Build a recursive runtime environment with placeholders, then fill each slot.
        let mut env_rec = self.env.clone();
        let mut slots = Vec::with_capacity(decls.len());
        for decl in decls {
            if let Some(existing) = env_rec.get(&decl.name.name) {
                slots.push(existing);
            } else {
                let placeholder = self.heap.alloc_ptr_uninitialized(decl.name.name.clone())?;
                env_rec = env_rec.extend(decl.name.name.clone(), placeholder);
                slots.push(placeholder);
            }
        }

        let saved_env = self.env.clone();
        self.env = env_rec.clone();

        let result: Result<(), EngineError> = (|| {
            for (decl, slot) in decls.iter().zip(slots.iter()) {
                let mut lam_body = decl.body.clone();
                for (idx, (param, ann)) in decl.params.iter().enumerate().rev() {
                    let lam_constraints = if idx == 0 {
                        decl.constraints.clone()
                    } else {
                        Vec::new()
                    };
                    let span = param.span;
                    lam_body = Arc::new(Expr::Lam(
                        span,
                        Scope::new_sync(),
                        param.clone(),
                        Some(ann.clone()),
                        lam_constraints,
                        lam_body,
                    ));
                }

                let typed = self.type_check_expr(lam_body.as_ref())?;
                let (param_ty, _ret_ty) = split_fun(&typed.typ)
                    .ok_or_else(|| EngineError::NotCallable(typed.typ.to_string()))?;
                let TypedExprKind::Lam { param, body } = typed.kind.as_ref() else {
                    return Err(EngineError::Internal(
                        "fn declaration did not lower to lambda".into(),
                    ));
                };
                let ptr = self.heap.alloc_ptr_closure(
                    self.env.clone(),
                    param.clone(),
                    param_ty,
                    typed.typ.clone(),
                    Arc::new(body.as_ref().clone()),
                )?;
                let value = self.heap.clone_cell(&ptr)?;
                self.heap.overwrite(slot, value)?;
            }
            Ok(())
        })();

        if result.is_err() {
            self.env = saved_env;
            return result;
        }

        self.env = env_rec;
        Ok(())
    }

    pub(crate) fn inject_decls(&mut self, decls: &[Decl]) -> Result<(), EngineError> {
        let mut pending_fns: Vec<FnDecl> = Vec::new();
        for decl in decls {
            if let Decl::Fn(fd) = decl {
                pending_fns.push(fd.clone());
                continue;
            }
            if !pending_fns.is_empty() {
                self.inject_fn_decls(&pending_fns)?;
                pending_fns.clear();
            }

            match decl {
                Decl::Type(ty) => self.inject_type_decl(ty)?,
                Decl::Class(class_decl) => self.inject_class_decl(class_decl)?,
                Decl::Instance(inst_decl) => self.inject_instance_decl(inst_decl)?,
                Decl::Fn(..) => {}
                Decl::DeclareFn(df) => {
                    self.type_system
                        .inject_declare_fn_decl(df)
                        .map_err(EngineError::Type)?;
                }
                Decl::Import(..) => {}
            }
        }
        if !pending_fns.is_empty() {
            self.inject_fn_decls(&pending_fns)?;
        }
        Ok(())
    }

    pub(crate) fn publish_runtime_decl_interfaces(
        &mut self,
        decls: &[DeclareFnDecl],
    ) -> Result<(), EngineError> {
        for df in decls {
            if self.env.get(&df.name.name).is_some() {
                continue;
            }
            let placeholder = self.heap.alloc_ptr_uninitialized(df.name.name.clone())?;
            self.env = self.env.extend(df.name.name.clone(), placeholder);
        }
        Ok(())
    }

    pub fn inject_instance(&mut self, class: &str, inst: Instance) {
        self.type_system.register_instance(class, inst);
    }

    fn inject_prelude(&mut self) -> Result<(), EngineError> {
        inject_prelude_adts(self)?;
        inject_equality_ops(self)?;
        inject_order_ops(self)?;
        inject_show_ops(self)?;
        inject_boolean_ops(self)?;
        inject_numeric_ops(self)?;
        inject_list_builtins(self)?;
        inject_option_result_builtins(self)?;
        inject_json_primops(self)?;
        self.register_prelude_typeclass_instances()?;
        Ok(())
    }

    fn inject_prelude_virtual_module(&mut self) -> Result<(), EngineError> {
        if self.virtual_modules.contains_key(PRELUDE_MODULE_NAME) {
            return Ok(());
        }

        let module_key = module_key_for_module(&ModuleId::Virtual(PRELUDE_MODULE_NAME.to_string()));
        let mut exports = ModuleExports::default();
        for (name, _) in self.type_system.env.values.iter() {
            if !name.as_ref().starts_with("@m") {
                exports.insert_value(
                    name.clone(),
                    CanonicalSymbol::from_symbol(
                        module_key,
                        SymbolKind::Value,
                        name.clone(),
                        name.clone(),
                    ),
                );
            }
        }

        for name in self.type_system.adts.keys() {
            if !name.as_ref().starts_with("@m") {
                exports.insert_type(
                    name.clone(),
                    CanonicalSymbol::from_symbol(
                        module_key,
                        SymbolKind::Type,
                        name.clone(),
                        name.clone(),
                    ),
                );
            }
        }

        for name in self.type_system.class_info.keys() {
            if !name.as_ref().starts_with("@m") {
                exports.insert_class(
                    name.clone(),
                    CanonicalSymbol::from_symbol(
                        module_key,
                        SymbolKind::Class,
                        name.clone(),
                        name.clone(),
                    ),
                );
            }
        }

        self.virtual_modules.insert(
            PRELUDE_MODULE_NAME.to_string(),
            VirtualModule {
                exports,
                decls: Vec::new(),
                source: None,
            },
        );
        Ok(())
    }

    pub(crate) fn virtual_module_exports(&self, module_name: &str) -> Option<ModuleExports> {
        self.virtual_modules
            .get(module_name)
            .map(|module| module.exports.clone())
    }

    fn register_prelude_typeclass_instances(&mut self) -> Result<(), EngineError> {
        // The type system prelude injects the *heads* of the standard instances.
        // The evaluator also needs the *method bodies* so class method lookup can
        // produce actual values at runtime.
        let program = prelude_typeclasses_program().map_err(EngineError::Type)?;
        for decl in program.decls.iter() {
            let Decl::Instance(inst_decl) = decl else {
                continue;
            };
            if inst_decl.methods.is_empty() {
                continue;
            }
            let prepared = self
                .type_system
                .prepare_instance_decl(inst_decl)
                .map_err(EngineError::Type)?;
            self.register_typeclass_instance(inst_decl, &prepared)?;
        }
        Ok(())
    }

    fn register_native(
        &mut self,
        name: Symbol,
        scheme: Scheme,
        arity: usize,
        func: NativeCallable<State>,
    ) -> Result<(), EngineError> {
        let expected = type_arity(&scheme.typ);
        if expected != arity {
            return Err(EngineError::NativeArity {
                name: name.clone(),
                expected,
                got: arity,
            });
        }
        self.register_type_scheme(&name, &scheme)?;
        self.natives.insert(name, arity, scheme, func)
    }

    fn register_type_scheme(
        &mut self,
        name: &Symbol,
        injected: &Scheme,
    ) -> Result<(), EngineError> {
        let schemes = self.type_system.env.lookup(name);
        match schemes {
            None => {
                self.type_system.add_value(name.as_ref(), injected.clone());
                Ok(())
            }
            Some(schemes) => {
                let has_poly = schemes
                    .iter()
                    .any(|s| !s.vars.is_empty() || !s.preds.is_empty());
                if has_poly {
                    for existing in schemes {
                        if scheme_accepts(&self.type_system, existing, &injected.typ)? {
                            return Ok(());
                        }
                    }
                    Err(EngineError::InvalidInjection {
                        name: name.clone(),
                        typ: injected.typ.to_string(),
                    })
                } else {
                    if schemes.iter().any(|s| s == injected) {
                        return Ok(());
                    }
                    self.type_system
                        .add_overload(name.as_ref(), injected.clone());
                    Ok(())
                }
            }
        }
    }

    pub(crate) fn infer_type(
        &mut self,
        expr: &Expr,
    ) -> Result<(Vec<Predicate>, Type), EngineError> {
        infer(&mut self.type_system, expr).map_err(EngineError::Type)
    }

    fn type_check_expr(&mut self, expr: &Expr) -> Result<TypedExpr, EngineError> {
        type_check_engine(self, expr)
    }

    fn check_natives(&self, expr: &TypedExpr) -> Result<(), EngineError> {
        check_natives_in_engine(self, expr)
    }

    fn register_typeclass_instance(
        &mut self,
        decl: &InstanceDecl,
        prepared: &PreparedInstanceDecl,
    ) -> Result<(), EngineError> {
        let mut methods: BTreeMap<Symbol, Arc<TypedExpr>> = BTreeMap::new();
        for method in &decl.methods {
            let typed = self
                .type_system
                .typecheck_instance_method(prepared, method)
                .map_err(EngineError::Type)?;
            self.check_natives(&typed)?;
            methods.insert(method.name.clone(), Arc::new(typed));
        }

        self.typeclasses.insert(
            prepared.class.clone(),
            prepared.head.clone(),
            self.env.clone(),
            methods,
        )?;
        Ok(())
    }

    pub(crate) fn lookup_scheme(&self, name: &Symbol) -> Result<Scheme, EngineError> {
        let schemes = self
            .type_system
            .env
            .lookup(name)
            .ok_or_else(|| EngineError::UnknownVar(name.clone()))?;
        if schemes.len() != 1 {
            return Err(EngineError::AmbiguousOverload { name: name.clone() });
        }
        Ok(schemes[0].clone())
    }
}

pub(crate) fn type_check_engine<State>(
    engine: &mut Engine<State>,
    expr: &Expr,
) -> Result<TypedExpr, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    if let Some(span) = first_hole_span(expr) {
        return Err(EngineError::Type(TypeError::Spanned {
            span,
            error: Box::new(TypeError::UnsupportedExpr(
                "typed hole `?` must be filled before evaluation",
            )),
        }));
    }
    let (typed, preds, _ty) = infer_typed(&mut engine.type_system, expr)?;
    let (typed, preds) = default_ambiguous_types(engine, typed, preds)?;
    check_predicates_in_engine(engine, &preds)?;
    check_natives_in_engine(engine, &typed)?;
    Ok(typed)
}

fn check_predicates_in_engine<State>(
    engine: &Engine<State>,
    preds: &[Predicate],
) -> Result<(), EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    for pred in preds {
        if pred.typ.ftv().is_empty() {
            let ok = entails(&engine.type_system.classes, &[], pred)?;
            if !ok {
                return Err(EngineError::Type(TypeError::NoInstance(
                    pred.class.clone(),
                    pred.typ.to_string(),
                )));
            }
        }
    }
    Ok(())
}

fn check_natives_in_engine<State>(
    engine: &Engine<State>,
    expr: &TypedExpr,
) -> Result<(), EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    enum ScopeWalkStep<'b> {
        Expr(&'b TypedExpr),
        Push(Symbol),
        PushMany(Vec<Symbol>),
        Pop(usize),
    }

    let mut bound: Vec<Symbol> = Vec::new();
    let mut stack = vec![ScopeWalkStep::Expr(expr)];
    while let Some(frame) = stack.pop() {
        match frame {
            ScopeWalkStep::Expr(expr) => match expr.kind.as_ref() {
                TypedExprKind::Var { name, overloads } => {
                    if bound.iter().any(|n| n == name) {
                        continue;
                    }
                    if !engine.natives.has_name(name) {
                        if engine.env.get(name).is_some() {
                            continue;
                        }
                        if engine.type_system.class_methods.contains_key(name) {
                            continue;
                        }
                        return Err(EngineError::UnknownVar(name.clone()));
                    }
                    if !overloads.is_empty()
                        && expr.typ.ftv().is_empty()
                        && !overloads.iter().any(|t| unify(t, &expr.typ).is_ok())
                    {
                        return Err(EngineError::MissingImpl {
                            name: name.clone(),
                            typ: expr.typ.to_string(),
                        });
                    }
                    if expr.typ.ftv().is_empty()
                        && !has_native_impl_in_engine(engine, name.as_ref(), &expr.typ)
                    {
                        return Err(EngineError::MissingImpl {
                            name: name.clone(),
                            typ: expr.typ.to_string(),
                        });
                    }
                }
                TypedExprKind::Tuple(elems) | TypedExprKind::List(elems) => {
                    for elem in elems.iter().rev() {
                        stack.push(ScopeWalkStep::Expr(elem));
                    }
                }
                TypedExprKind::Dict(kvs) => {
                    for v in kvs.values().rev() {
                        stack.push(ScopeWalkStep::Expr(v));
                    }
                }
                TypedExprKind::RecordUpdate { base, updates } => {
                    for v in updates.values().rev() {
                        stack.push(ScopeWalkStep::Expr(v));
                    }
                    stack.push(ScopeWalkStep::Expr(base));
                }
                TypedExprKind::App(f, x) => {
                    stack.push(ScopeWalkStep::Expr(x));
                    stack.push(ScopeWalkStep::Expr(f));
                }
                TypedExprKind::Project { expr, .. } => stack.push(ScopeWalkStep::Expr(expr)),
                TypedExprKind::Lam { param, body } => {
                    stack.push(ScopeWalkStep::Pop(1));
                    stack.push(ScopeWalkStep::Expr(body));
                    stack.push(ScopeWalkStep::Push(param.clone()));
                }
                TypedExprKind::Let { name, def, body } => {
                    stack.push(ScopeWalkStep::Pop(1));
                    stack.push(ScopeWalkStep::Expr(body));
                    stack.push(ScopeWalkStep::Push(name.clone()));
                    stack.push(ScopeWalkStep::Expr(def));
                }
                TypedExprKind::LetRec { bindings, body } => {
                    if !bindings.is_empty() {
                        stack.push(ScopeWalkStep::Pop(bindings.len()));
                        stack.push(ScopeWalkStep::Expr(body));
                        for (_, def) in bindings.iter().rev() {
                            stack.push(ScopeWalkStep::Expr(def));
                        }
                        stack.push(ScopeWalkStep::PushMany(
                            bindings.iter().map(|(name, _)| name.clone()).collect(),
                        ));
                    } else {
                        stack.push(ScopeWalkStep::Expr(body));
                    }
                }
                TypedExprKind::Ite {
                    cond,
                    then_expr,
                    else_expr,
                } => {
                    stack.push(ScopeWalkStep::Expr(else_expr));
                    stack.push(ScopeWalkStep::Expr(then_expr));
                    stack.push(ScopeWalkStep::Expr(cond));
                }
                TypedExprKind::Match { scrutinee, arms } => {
                    for (pat, arm_expr) in arms.iter().rev() {
                        let mut bindings = Vec::new();
                        collect_pattern_bindings(pat, &mut bindings);
                        let count = bindings.len();
                        if count != 0 {
                            stack.push(ScopeWalkStep::Pop(count));
                            stack.push(ScopeWalkStep::Expr(arm_expr));
                            stack.push(ScopeWalkStep::PushMany(bindings));
                        } else {
                            stack.push(ScopeWalkStep::Expr(arm_expr));
                        }
                    }
                    stack.push(ScopeWalkStep::Expr(scrutinee));
                }
                TypedExprKind::Bool(..)
                | TypedExprKind::Uint(..)
                | TypedExprKind::Int(..)
                | TypedExprKind::Float(..)
                | TypedExprKind::String(..)
                | TypedExprKind::Uuid(..)
                | TypedExprKind::DateTime(..) => {}
                TypedExprKind::Hole => return Err(EngineError::UnsupportedExpr),
            },
            ScopeWalkStep::Push(sym) => bound.push(sym),
            ScopeWalkStep::PushMany(syms) => bound.extend(syms),
            ScopeWalkStep::Pop(count) => bound.truncate(bound.len().saturating_sub(count)),
        }
    }
    Ok(())
}

fn has_native_impl_in_engine<State>(engine: &Engine<State>, name: &str, typ: &Type) -> bool
where
    State: Clone + Send + Sync + 'static,
{
    let sym_name = Symbol::intern(name);
    engine
        .natives
        .get(&sym_name)
        .map(|impls| impls.iter().any(|imp| impl_matches_type(imp, typ)))
        .unwrap_or(false)
}

impl<State> RuntimeSnapshot<State>
where
    State: Clone + Send + Sync + 'static,
{
    fn native_callable(&self, id: NativeId) -> Result<NativeCallable<State>, EngineError> {
        self.natives
            .by_id(id)
            .map(|imp| imp.func.clone())
            .ok_or_else(|| EngineError::Internal(format!("unknown native id: {id}")))
    }
}

fn first_hole_span(expr: &Expr) -> Option<Span> {
    let mut stack = vec![expr];
    while let Some(expr) = stack.pop() {
        match expr {
            Expr::Hole(span) => return Some(*span),
            Expr::App(_, f, x) => {
                stack.push(x);
                stack.push(f);
            }
            Expr::Project(_, base, _) | Expr::Ann(_, base, _) => stack.push(base),
            Expr::Lam(_, _scope, _param, _ann, _constraints, body) => stack.push(body),
            Expr::Let(_, _var, _ann, def, body) => {
                stack.push(body);
                stack.push(def);
            }
            Expr::LetRec(_, bindings, body) => {
                stack.push(body);
                for (_var, _ann, def) in bindings.iter().rev() {
                    stack.push(def);
                }
            }
            Expr::Ite(_, cond, then_expr, else_expr) => {
                stack.push(else_expr);
                stack.push(then_expr);
                stack.push(cond);
            }
            Expr::Match(_, scrutinee, arms) => {
                for (_pat, arm) in arms.iter().rev() {
                    stack.push(arm);
                }
                stack.push(scrutinee);
            }
            Expr::Tuple(_, elems) | Expr::List(_, elems) => {
                for elem in elems.iter().rev() {
                    stack.push(elem);
                }
            }
            Expr::Dict(_, kvs) => {
                for value in kvs.values().rev() {
                    stack.push(value);
                }
            }
            Expr::RecordUpdate(_, base, kvs) => {
                for value in kvs.values().rev() {
                    stack.push(value);
                }
                stack.push(base);
            }
            Expr::Bool(..)
            | Expr::Uint(..)
            | Expr::Int(..)
            | Expr::Float(..)
            | Expr::String(..)
            | Expr::Uuid(..)
            | Expr::DateTime(..)
            | Expr::Var(..) => {}
        }
    }
    None
}

fn normalize_name(name: &str) -> Symbol {
    if let Some(stripped) = name.strip_prefix('(').and_then(|s| s.strip_suffix(')')) {
        let ok = stripped
            .chars()
            .all(|c| !c.is_alphanumeric() && c != '_' && !c.is_whitespace());
        if ok {
            return Symbol::intern(stripped);
        }
    }
    Symbol::intern(name)
}

fn default_ambiguous_types<State: Clone + Send + Sync + 'static>(
    engine: &Engine<State>,
    typed: TypedExpr,
    mut preds: Vec<Predicate>,
) -> Result<(TypedExpr, Vec<Predicate>), EngineError> {
    let mut candidates = Vec::new();
    collect_default_candidates(&typed, &mut candidates);
    for ty in [
        Type::builtin(BuiltinTypeId::F32),
        Type::builtin(BuiltinTypeId::I32),
        Type::builtin(BuiltinTypeId::String),
    ] {
        push_unique_type(&mut candidates, ty);
    }

    let mut subst = Subst::new_sync();
    loop {
        let vars: Vec<_> = preds.ftv().into_iter().collect();
        let mut progress = false;
        for tv in vars {
            if subst.get(&tv).is_some() {
                continue;
            }
            let mut relevant = Vec::new();
            let mut simple = true;
            for pred in &preds {
                if pred.typ.ftv().contains(&tv) {
                    if !defaultable_class(&pred.class) {
                        simple = false;
                        break;
                    }
                    match pred.typ.as_ref() {
                        TypeKind::Var(v) if v.id == tv => relevant.push(pred.clone()),
                        _ => {
                            simple = false;
                            break;
                        }
                    }
                }
            }
            if !simple || relevant.is_empty() {
                continue;
            }
            if let Some(choice) = choose_default_type(engine, &relevant, &candidates)? {
                let mut next = Subst::new_sync();
                next = next.insert(tv, choice.clone());
                preds = preds.apply(&next);
                subst = compose_subst(next, subst);
                progress = true;
            }
        }
        if !progress {
            break;
        }
    }
    Ok((typed.apply(&subst), preds))
}

fn defaultable_class(class: &Symbol) -> bool {
    matches!(
        class.as_ref(),
        "AdditiveMonoid" | "MultiplicativeMonoid" | "AdditiveGroup" | "Ring" | "Field" | "Integral"
    )
}

fn collect_default_candidates(expr: &TypedExpr, out: &mut Vec<Type>) {
    let mut stack: Vec<&TypedExpr> = vec![expr];
    while let Some(expr) = stack.pop() {
        if expr.typ.ftv().is_empty()
            && let TypeKind::Con(tc) = expr.typ.as_ref()
            && tc.arity() == 0
        {
            push_unique_type(out, expr.typ.clone());
        }

        match expr.kind.as_ref() {
            TypedExprKind::Tuple(elems) | TypedExprKind::List(elems) => {
                for elem in elems.iter().rev() {
                    stack.push(elem);
                }
            }
            TypedExprKind::Dict(kvs) => {
                for value in kvs.values().rev() {
                    stack.push(value);
                }
            }
            TypedExprKind::RecordUpdate { base, updates } => {
                for value in updates.values().rev() {
                    stack.push(value);
                }
                stack.push(base);
            }
            TypedExprKind::App(f, x) => {
                stack.push(x);
                stack.push(f);
            }
            TypedExprKind::Project { expr, .. } => stack.push(expr),
            TypedExprKind::Lam { body, .. } => stack.push(body),
            TypedExprKind::Let { def, body, .. } => {
                stack.push(body);
                stack.push(def);
            }
            TypedExprKind::LetRec { bindings, body } => {
                stack.push(body);
                for (_, def) in bindings.iter().rev() {
                    stack.push(def);
                }
            }
            TypedExprKind::Ite {
                cond,
                then_expr,
                else_expr,
            } => {
                stack.push(else_expr);
                stack.push(then_expr);
                stack.push(cond);
            }
            TypedExprKind::Match { scrutinee, arms } => {
                for (_, expr) in arms.iter().rev() {
                    stack.push(expr);
                }
                stack.push(scrutinee);
            }
            TypedExprKind::Var { .. }
            | TypedExprKind::Bool(..)
            | TypedExprKind::Uint(..)
            | TypedExprKind::Int(..)
            | TypedExprKind::Float(..)
            | TypedExprKind::String(..)
            | TypedExprKind::Uuid(..)
            | TypedExprKind::DateTime(..)
            | TypedExprKind::Hole => {}
        }
    }
}

fn push_unique_type(out: &mut Vec<Type>, typ: Type) {
    if !out.iter().any(|t| t == &typ) {
        out.push(typ);
    }
}

fn choose_default_type<State: Clone + Send + Sync + 'static>(
    engine: &Engine<State>,
    preds: &[Predicate],
    candidates: &[Type],
) -> Result<Option<Type>, EngineError> {
    for candidate in candidates {
        let mut ok = true;
        for pred in preds {
            let test = Predicate::new(pred.class.clone(), candidate.clone());
            if !entails(&engine.type_system.classes, &[], &test)? {
                ok = false;
                break;
            }
        }
        if ok {
            return Ok(Some(candidate.clone()));
        }
    }
    Ok(None)
}

fn scheme_accepts(ts: &TypeSystem, scheme: &Scheme, typ: &Type) -> Result<bool, EngineError> {
    let mut supply = TypeVarSupply::new();
    let (preds, scheme_ty) = instantiate(scheme, &mut supply);
    let subst = match unify(&scheme_ty, typ) {
        Ok(s) => s,
        Err(_) => return Ok(false),
    };
    let preds = preds.apply(&subst);
    for pred in preds {
        if pred.typ.ftv().is_empty() {
            let ok = entails(&ts.classes, &[], &pred)?;
            if !ok {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

pub(crate) fn is_function_type(typ: &Type) -> bool {
    matches!(typ.as_ref(), TypeKind::Fun(..))
}

pub(crate) fn collect_pattern_bindings(pat: &Pattern, out: &mut Vec<Symbol>) {
    match pat {
        Pattern::Wildcard(..) => {}
        Pattern::Var(var) => out.push(var.name.clone()),
        Pattern::Named(_, _, ps) => {
            for p in ps {
                collect_pattern_bindings(p, out);
            }
        }
        Pattern::Tuple(_, ps) => {
            for p in ps {
                collect_pattern_bindings(p, out);
            }
        }
        Pattern::List(_, ps) => {
            for p in ps {
                collect_pattern_bindings(p, out);
            }
        }
        Pattern::Cons(_, head, tail) => {
            collect_pattern_bindings(head, out);
            collect_pattern_bindings(tail, out);
        }
        Pattern::Dict(_, fields) => {
            for (_key, pat) in fields {
                collect_pattern_bindings(pat, out);
            }
        }
    }
}

fn type_head_and_args(typ: &Type) -> Result<(Symbol, usize, Vec<Type>), EngineError> {
    let mut args = Vec::new();
    let mut head = typ;
    while let TypeKind::App(f, arg) = head.as_ref() {
        args.push(arg.clone());
        head = f;
    }
    args.reverse();

    let TypeKind::Con(con) = head.as_ref() else {
        return Err(EngineError::Custom(format!(
            "cannot build ADT declaration from non-constructor type `{typ}`"
        )));
    };
    if !args.is_empty() && args.len() != con.arity() {
        return Err(EngineError::Custom(format!(
            "constructor `{}` expected {} type arguments but got {} in `{typ}`",
            con.name_str(),
            con.arity(),
            args.len()
        )));
    }
    Ok((con.name(), con.arity(), args))
}

pub(crate) fn adt_shape(adt: &AdtDecl) -> String {
    let param_names: BTreeMap<_, _> = adt
        .params
        .iter()
        .enumerate()
        .map(|(idx, param)| (param.var.id, format!("t{idx}")))
        .collect();
    let mut variants = adt
        .variants
        .iter()
        .map(|variant| {
            let args = variant
                .args
                .iter()
                .map(|arg| normalize_type_for_shape(arg, &param_names))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{}({args})", variant.name)
        })
        .collect::<Vec<_>>();
    variants.sort();
    format!("{}[{}]", adt.name, variants.join(" | "))
}

fn normalize_type_for_shape(typ: &Type, param_names: &BTreeMap<usize, String>) -> String {
    match typ.as_ref() {
        TypeKind::Var(tv) => param_names
            .get(&tv.id)
            .cloned()
            .unwrap_or_else(|| format!("v{}", tv.id)),
        TypeKind::Con(con) => con.name_str().to_string(),
        TypeKind::App(fun, arg) => format!(
            "({} {})",
            normalize_type_for_shape(fun, param_names),
            normalize_type_for_shape(arg, param_names)
        ),
        TypeKind::Fun(arg, ret) => format!(
            "({} -> {})",
            normalize_type_for_shape(arg, param_names),
            normalize_type_for_shape(ret, param_names)
        ),
        TypeKind::Tuple(elems) => format!(
            "({})",
            elems
                .iter()
                .map(|elem| normalize_type_for_shape(elem, param_names))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        TypeKind::Record(fields) => format!(
            "{{{}}}",
            fields
                .iter()
                .map(|(name, typ)| format!(
                    "{name}: {}",
                    normalize_type_for_shape(typ, param_names)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

pub(crate) fn adt_shape_eq(left: &AdtDecl, right: &AdtDecl) -> bool {
    adt_shape(left) == adt_shape(right)
}

fn type_arity(typ: &Type) -> usize {
    let mut count = 0;
    let mut cur = typ;
    while let TypeKind::Fun(_, next) = cur.as_ref() {
        count += 1;
        cur = next;
    }
    count
}

fn split_fun(typ: &Type) -> Option<(Type, Type)> {
    match typ.as_ref() {
        TypeKind::Fun(a, b) => Some((a.clone(), b.clone())),
        _ => None,
    }
}

pub(crate) fn impl_matches_type<State: Clone + Send + Sync + 'static>(
    imp: &NativeImpl<State>,
    typ: &Type,
) -> bool {
    let mut supply = TypeVarSupply::new();
    let (_preds, scheme_ty) = instantiate(&imp.scheme, &mut supply);
    unify(&scheme_ty, typ).is_ok()
}

pub(crate) fn native_capability_matches_requirement(
    capability: &NativeCapability,
    requirement: &NativeRequirement,
) -> bool {
    let mut supply = TypeVarSupply::new();
    let (_preds, scheme_ty) = instantiate(&capability.scheme, &mut supply);
    capability.name == requirement.name
        && capability.arity == type_arity(&requirement.typ)
        && unify(&scheme_ty, &requirement.typ).is_ok()
}

pub(crate) fn class_method_capability_matches_requirement(
    capability: &ClassMethodCapability,
    requirement: &ClassMethodRequirement,
) -> bool {
    let mut supply = TypeVarSupply::new();
    let (_preds, scheme_ty) = instantiate(&capability.scheme, &mut supply);
    capability.name == requirement.name && unify(&scheme_ty, &requirement.typ).is_ok()
}

fn cell_type(heap: &HeapAccess<'_>, cell: &Cell) -> Result<Type, EngineError> {
    let pointer_type = |pointer: &Pointer| -> Result<Type, EngineError> {
        let cell = heap.get(pointer)?;
        cell_type(heap, cell)
    };

    match cell {
        Cell::Bool(..) => Ok(Type::builtin(BuiltinTypeId::Bool)),
        Cell::U8(..) => Ok(Type::builtin(BuiltinTypeId::U8)),
        Cell::U16(..) => Ok(Type::builtin(BuiltinTypeId::U16)),
        Cell::U32(..) => Ok(Type::builtin(BuiltinTypeId::U32)),
        Cell::U64(..) => Ok(Type::builtin(BuiltinTypeId::U64)),
        Cell::I8(..) => Ok(Type::builtin(BuiltinTypeId::I8)),
        Cell::I16(..) => Ok(Type::builtin(BuiltinTypeId::I16)),
        Cell::I32(..) => Ok(Type::builtin(BuiltinTypeId::I32)),
        Cell::I64(..) => Ok(Type::builtin(BuiltinTypeId::I64)),
        Cell::F32(..) => Ok(Type::builtin(BuiltinTypeId::F32)),
        Cell::F64(..) => Ok(Type::builtin(BuiltinTypeId::F64)),
        Cell::String(..) => Ok(Type::builtin(BuiltinTypeId::String)),
        Cell::Uuid(..) => Ok(Type::builtin(BuiltinTypeId::Uuid)),
        Cell::DateTime(..) => Ok(Type::builtin(BuiltinTypeId::DateTime)),
        Cell::Tuple(elems) => {
            let mut tys = Vec::with_capacity(elems.len());
            for elem in elems {
                tys.push(pointer_type(elem)?);
            }
            Ok(Type::tuple(tys))
        }
        Cell::Array(elems) => {
            let first = elems
                .first()
                .ok_or_else(|| EngineError::UnknownType(Symbol::intern("array")))?;
            let elem_ty = pointer_type(first)?;
            for elem in elems.iter().skip(1) {
                let ty = pointer_type(elem)?;
                if ty != elem_ty {
                    return Err(EngineError::NativeType {
                        expected: elem_ty.to_string(),
                        got: ty.to_string(),
                    });
                }
            }
            Ok(Type::app(Type::builtin(BuiltinTypeId::Array), elem_ty))
        }
        Cell::Dict(map) => {
            let first = map
                .values()
                .next()
                .ok_or_else(|| EngineError::UnknownType(Symbol::intern("dict")))?;
            let elem_ty = pointer_type(first)?;
            for val in map.values().skip(1) {
                let ty = pointer_type(val)?;
                if ty != elem_ty {
                    return Err(EngineError::NativeType {
                        expected: elem_ty.to_string(),
                        got: ty.to_string(),
                    });
                }
            }
            Ok(Type::app(Type::builtin(BuiltinTypeId::Dict), elem_ty))
        }
        Cell::Adt(tag, args) if tag.as_ref() == "Some" && args.len() == 1 => {
            let inner = pointer_type(&args[0])?;
            Ok(Type::app(Type::builtin(BuiltinTypeId::Option), inner))
        }
        Cell::Adt(tag, args) if tag.as_ref() == "None" && args.is_empty() => {
            Err(EngineError::UnknownType(Symbol::intern("option")))
        }
        Cell::Adt(tag, args)
            if (tag.as_ref() == "Ok" || tag.as_ref() == "Err") && args.len() == 1 =>
        {
            Err(EngineError::UnknownType(Symbol::intern("result")))
        }
        Cell::Adt(tag, args)
            if (tag.as_ref() == "Empty" || tag.as_ref() == "Cons") && args.len() <= 2 =>
        {
            let elems = list_to_vec(heap, cell)?;
            let first = elems
                .first()
                .ok_or_else(|| EngineError::UnknownType(Symbol::intern("list")))?;
            let elem_ty = pointer_type(first)?;
            for elem in elems.iter().skip(1) {
                let ty = pointer_type(elem)?;
                if ty != elem_ty {
                    return Err(EngineError::NativeType {
                        expected: elem_ty.to_string(),
                        got: ty.to_string(),
                    });
                }
            }
            Ok(Type::app(Type::builtin(BuiltinTypeId::List), elem_ty))
        }
        Cell::Adt(tag, _args) => Err(EngineError::UnknownType(tag.clone())),
        Cell::Uninitialized(..) => Err(EngineError::UnknownType(Symbol::intern("uninitialized"))),
        Cell::Frame(..) => Err(EngineError::UnknownType(Symbol::intern("frame"))),
        Cell::Closure(..) => Err(EngineError::UnknownType(Symbol::intern("closure"))),
        Cell::Native(..) => Err(EngineError::UnknownType(Symbol::intern("native"))),
        Cell::Overloaded(..) => Err(EngineError::UnknownType(Symbol::intern("overloaded"))),
    }
}

pub(crate) fn resolve_arg_type(
    heap: &Heap,
    arg_type: Option<&Type>,
    arg: &Pointer,
) -> Result<Type, EngineError> {
    let infer_from_cell = |ty_hint: Option<&Type>| -> Result<Type, EngineError> {
        heap.with_access(|heap| {
            let cell = heap.get(arg)?;
            match ty_hint {
                Some(ty) => match cell_type(heap, cell) {
                    Ok(val_ty) if val_ty.ftv().is_empty() => Ok(val_ty),
                    _ => Ok(ty.clone()),
                },
                None => cell_type(heap, cell),
            }
        })
    };
    match arg_type {
        Some(ty) if ty.ftv().is_empty() => Ok(ty.clone()),
        Some(ty) => infer_from_cell(Some(ty)),
        None => infer_from_cell(None),
    }
}

fn application_result_type(func_type: &Type, arg_type: &Type) -> Result<Type, EngineError> {
    let (expected_arg, result) =
        split_fun(func_type).ok_or_else(|| EngineError::NotCallable(func_type.to_string()))?;
    let subst = unify(&expected_arg, arg_type).map_err(|_| EngineError::NativeType {
        expected: expected_arg.to_string(),
        got: arg_type.to_string(),
    })?;
    Ok(result.apply(&subst))
}

pub(crate) fn synthetic_application_expr(
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

fn synthetic_application_expr_from_head(
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

pub(crate) fn binary_arg_types(typ: &Type) -> Result<(Type, Type), EngineError> {
    let (lhs, rest) = split_fun(typ).ok_or_else(|| EngineError::NativeType {
        expected: "binary function".into(),
        got: typ.to_string(),
    })?;
    let (rhs, _res) = split_fun(&rest).ok_or_else(|| EngineError::NativeType {
        expected: "binary function".into(),
        got: typ.to_string(),
    })?;
    Ok((lhs, rhs))
}

fn project_pointer(heap: &Heap, field: &Symbol, pointer: &Pointer) -> Result<Pointer, EngineError> {
    heap.with_access(|heap| project_pointer_with_access(heap, field, pointer))
}

fn project_pointer_with_access(
    heap: &HeapAccess<'_>,
    field: &Symbol,
    pointer: &Pointer,
) -> Result<Pointer, EngineError> {
    let value = heap.get(pointer)?;
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
            let inner = heap.get(&args[0])?;
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

enum EvalControl<State: Clone + Send + Sync + 'static> {
    Push {
        expr: Arc<TypedExpr>,
        env: Environment,
    },
    PushFrame(Box<Frame>),
    Schedule(Vec<Pointer>),
    Wait,
    AwaitNative(NativeAsyncCall<State>),
    Return(Pointer),
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

struct EvalWorkItem {
    frame: Pointer,
    returned: Option<EvalReturned>,
}

#[derive(Clone, Copy)]
struct EvalReturned {
    child: Pointer,
    value: Pointer,
}

impl EvalWorkItem {
    fn enter(frame: Pointer) -> Self {
        Self {
            frame,
            returned: None,
        }
    }

    fn receive(frame: Pointer, child: Pointer, value: Pointer) -> Self {
        Self {
            frame,
            returned: Some(EvalReturned { child, value }),
        }
    }

    fn trace_pointers(&self, out: &mut Vec<Pointer>) {
        out.push(self.frame);
        if let Some(returned) = self.returned.as_ref() {
            out.push(returned.child);
            out.push(returned.value);
        }
    }

    fn refresh_from_roots(
        &mut self,
        roots: &TempRoots,
        cursor: &mut usize,
    ) -> Result<(), EngineError> {
        self.frame = roots.get(*cursor)?;
        *cursor += 1;
        if let Some(returned) = self.returned.as_mut() {
            returned.child = roots.get(*cursor)?;
            *cursor += 1;
            returned.value = roots.get(*cursor)?;
            *cursor += 1;
        }
        Ok(())
    }
}

enum PendingNativeState {
    Polling(NativeHandleFuture),
    Ready(Result<Handle, EngineError>),
}

struct PendingNative {
    frame: Pointer,
    state: PendingNativeState,
}

struct DeferredNative<State: Clone + Send + Sync + 'static> {
    frame: Pointer,
    call: NativeAsyncCall<State>,
}

impl PendingNative {
    fn new(frame: Pointer, future: NativeHandleFuture) -> Self {
        Self {
            frame,
            state: PendingNativeState::Polling(future),
        }
    }

    fn ready(frame: Pointer, result: Result<Handle, EngineError>) -> Self {
        Self {
            frame,
            state: PendingNativeState::Ready(result),
        }
    }

    fn poll(&mut self, cx: &mut Context<'_>) -> bool {
        match &mut self.state {
            PendingNativeState::Polling(future) => match future.as_mut().poll(cx) {
                Poll::Ready(result) => {
                    self.state = PendingNativeState::Ready(result);
                    true
                }
                Poll::Pending => false,
            },
            PendingNativeState::Ready(_) => true,
        }
    }

    fn trace_pointers(&self, out: &mut Vec<Pointer>) {
        out.push(self.frame);
    }

    fn refresh_from_roots(
        &mut self,
        roots: &TempRoots,
        cursor: &mut usize,
    ) -> Result<(), EngineError> {
        self.frame = roots.get(*cursor)?;
        *cursor += 1;
        Ok(())
    }

    fn into_completion(self) -> Result<(Pointer, Handle), EngineError> {
        match self.state {
            PendingNativeState::Ready(result) => result.map(|handle| (self.frame, handle)),
            PendingNativeState::Polling(_) => Err(EngineError::Internal(
                "pending native future completed without a ready result".into(),
            )),
        }
    }
}

impl<State> DeferredNative<State>
where
    State: Clone + Send + Sync + 'static,
{
    fn new(frame: Pointer, call: NativeAsyncCall<State>) -> Self {
        Self { frame, call }
    }

    fn activate(self, runtime: &RuntimeSnapshot<State>) -> PendingNative {
        match self.call.invoke(runtime) {
            Ok(future) => PendingNative::new(self.frame, future),
            Err(err) => PendingNative::ready(self.frame, Err(err)),
        }
    }

    fn trace_pointers(&self, out: &mut Vec<Pointer>) {
        out.push(self.frame);
        self.call.trace_pointers(out);
    }

    fn refresh_from_roots(
        &mut self,
        roots: &TempRoots,
        cursor: &mut usize,
    ) -> Result<(), EngineError> {
        self.frame = roots.get(*cursor)?;
        *cursor += 1;
        self.call.refresh_from_roots(roots, cursor)?;
        Ok(())
    }
}

struct EvalScheduler<State: Clone + Send + Sync + 'static> {
    ready: VecDeque<EvalWorkItem>,
    deferred_ready: VecDeque<EvalWorkItem>,
    pending_native: Vec<PendingNative>,
    deferred_native: VecDeque<DeferredNative<State>>,
    bounds: ExecutionBounds,
}

impl<State> EvalScheduler<State>
where
    State: Clone + Send + Sync + 'static,
{
    fn new(root: Pointer, bounds: ExecutionBounds) -> Self {
        let mut ready = VecDeque::new();
        ready.push_front(EvalWorkItem::enter(root));
        Self {
            ready,
            deferred_ready: VecDeque::new(),
            pending_native: Vec::new(),
            deferred_native: VecDeque::new(),
            bounds,
        }
    }

    fn schedule_next(&mut self, item: EvalWorkItem) {
        self.ready.push_front(item);
        self.enforce_ready_limit();
    }

    fn schedule_pending_native(&mut self, frame: Pointer, call: NativeAsyncCall<State>) {
        self.deferred_native
            .push_back(DeferredNative::new(frame, call));
    }

    fn pop_next(&mut self) -> Option<EvalWorkItem> {
        self.admit_deferred_ready();
        let item = self.ready.pop_front();
        self.admit_deferred_ready();
        item
    }

    fn has_pending_native(&self) -> bool {
        !self.pending_native.is_empty()
    }

    fn enforce_ready_limit(&mut self) {
        let limit = self.bounds.ready_work_limit();
        while self.ready.len() > limit {
            if let Some(item) = self.ready.pop_back() {
                self.deferred_ready.push_front(item);
            }
        }
    }

    fn admit_deferred_ready(&mut self) {
        let limit = self.bounds.ready_work_limit();
        while self.ready.len() < limit {
            let Some(item) = self.deferred_ready.pop_front() else {
                break;
            };
            self.ready.push_back(item);
        }
    }

    fn admit_deferred_native(&mut self, runtime: &RuntimeSnapshot<State>) {
        let limit = self.bounds.pending_async_limit();
        while self.pending_native.len() < limit {
            let Some(deferred) = self.deferred_native.pop_front() else {
                break;
            };
            self.pending_native.push(deferred.activate(runtime));
        }
    }

    fn poll_pending_native_once(&mut self, cx: &mut Context<'_>) -> Option<usize> {
        for (index, pending) in self.pending_native.iter_mut().enumerate() {
            if pending.poll(cx) {
                return Some(index);
            }
        }
        None
    }

    fn take_pending_native_completion(
        &mut self,
        index: usize,
    ) -> Result<(Pointer, Handle), EngineError> {
        if index >= self.pending_native.len() {
            return Err(EngineError::Internal(
                "pending native completion index out of bounds".into(),
            ));
        }
        self.pending_native.remove(index).into_completion()
    }

    fn trace_pointers(&self, out: &mut Vec<Pointer>) {
        for item in &self.ready {
            item.trace_pointers(out);
        }
        for item in &self.deferred_ready {
            item.trace_pointers(out);
        }
        for pending in &self.pending_native {
            pending.trace_pointers(out);
        }
        for pending in &self.deferred_native {
            pending.trace_pointers(out);
        }
    }

    fn refresh_from_roots(
        &mut self,
        roots: &TempRoots,
        cursor: &mut usize,
    ) -> Result<(), EngineError> {
        for item in &mut self.ready {
            item.refresh_from_roots(roots, cursor)?;
        }
        for item in &mut self.deferred_ready {
            item.refresh_from_roots(roots, cursor)?;
        }
        for pending in &mut self.pending_native {
            pending.refresh_from_roots(roots, cursor)?;
        }
        for pending in &mut self.deferred_native {
            pending.refresh_from_roots(roots, cursor)?;
        }
        Ok(())
    }
}

async fn poll_pending_native<State>(
    runtime: &mut RuntimeSnapshot<State>,
    scheduler: &mut EvalScheduler<State>,
    wait: bool,
) -> Result<bool, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    scheduler.admit_deferred_native(runtime);
    if !scheduler.has_pending_native() {
        return Ok(false);
    }

    let mut protected = Vec::new();
    scheduler.trace_pointers(&mut protected);
    runtime.trace_pointers(&mut protected)?;
    let roots = runtime.heap.temp_roots(protected)?;

    let mut cursor = 0;
    scheduler.refresh_from_roots(&roots, &mut cursor)?;
    runtime.refresh_from_roots(&roots, &mut cursor)?;

    let ready_index = if wait {
        Some(
            poll_fn(|cx| match scheduler.poll_pending_native_once(cx) {
                Some(index) => Poll::Ready(index),
                None => Poll::Pending,
            })
            .await,
        )
    } else {
        poll_fn(|cx| Poll::Ready(scheduler.poll_pending_native_once(cx))).await
    };

    let mut cursor = 0;
    scheduler.refresh_from_roots(&roots, &mut cursor)?;
    runtime.refresh_from_roots(&roots, &mut cursor)?;

    let Some(index) = ready_index else {
        return Ok(false);
    };
    let (frame, handle) = scheduler.take_pending_native_completion(index)?;
    scheduler.admit_deferred_native(runtime);
    let value = handle.pointer_for_heap(&runtime.heap)?;
    scheduler.schedule_next(EvalWorkItem::receive(frame, frame, value));
    Ok(true)
}

pub(crate) async fn eval_typed_expr<State>(
    runtime: &RuntimeSnapshot<State>,
    env: &Environment,
    expr: &TypedExpr,
) -> Result<Pointer, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let root_parent = runtime.heap.alloc_ptr_root_frame_parent()?;
    eval_typed_expr_from_parent(runtime, root_parent, EvalStop::RootSentinel, env, expr).await
}

#[derive(Clone, Copy)]
pub(crate) enum EvalStop {
    RootSentinel,
    #[allow(dead_code)]
    Parent(Pointer),
}

pub(crate) async fn eval_typed_expr_from_parent<State>(
    runtime: &RuntimeSnapshot<State>,
    initial_parent: Pointer,
    stop: EvalStop,
    env: &Environment,
    expr: &TypedExpr,
) -> Result<Pointer, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let mut runtime = runtime.clone();
    let root_expr = Arc::new(expr.clone());
    let root_frame =
        runtime
            .heap
            .alloc_ptr_frame(frame_for_expr(initial_parent, root_expr, env.clone()))?;
    let mut scheduler = EvalScheduler::new(root_frame, runtime.execution_bounds);

    loop {
        if poll_pending_native(&mut runtime, &mut scheduler, false).await? {
            continue;
        }

        let mut item = match scheduler.pop_next() {
            Some(item) => item,
            None => {
                if poll_pending_native(&mut runtime, &mut scheduler, true).await? {
                    continue;
                }
                return Err(EngineError::Internal(
                    "eval scheduler ran out of ready work".into(),
                ));
            }
        };
        let mut protected = Vec::new();
        item.trace_pointers(&mut protected);
        scheduler.trace_pointers(&mut protected);
        runtime.trace_pointers(&mut protected)?;
        let roots = runtime.heap.temp_roots(protected)?;

        let mut cursor = 0;
        item.refresh_from_roots(&roots, &mut cursor)?;
        scheduler.refresh_from_roots(&roots, &mut cursor)?;
        runtime.refresh_from_roots(&roots, &mut cursor)?;

        let frame = runtime.heap.pointer_as_frame(&item.frame)?;
        let control = match item.returned {
            Some(returned) => {
                eval_receive(&runtime, item.frame, frame, returned.child, returned.value)?
            }
            None => eval_enter(&runtime, item.frame, frame)?,
        };

        let mut cursor = 0;
        item.refresh_from_roots(&roots, &mut cursor)?;
        scheduler.refresh_from_roots(&roots, &mut cursor)?;
        runtime.refresh_from_roots(&roots, &mut cursor)?;

        match control {
            EvalControl::Push { expr, env } => {
                let child = runtime
                    .heap
                    .alloc_ptr_frame(frame_for_expr(item.frame, expr, env))?;
                refresh_eval_roots(&mut runtime, &mut item, &mut scheduler, &roots)?;
                scheduler.schedule_next(EvalWorkItem::enter(child));
            }
            EvalControl::PushFrame(frame) => {
                let child = runtime.heap.alloc_ptr_frame(*frame)?;
                refresh_eval_roots(&mut runtime, &mut item, &mut scheduler, &roots)?;
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
                let child = runtime
                    .heap
                    .alloc_ptr_frame(Frame::NativeAsync(FrNativeAsync { parent: item.frame }))?;
                refresh_eval_roots(&mut runtime, &mut item, &mut scheduler, &roots)?;
                let mut cursor = 0;
                call.refresh_from_roots(&call_roots, &mut cursor)?;
                scheduler.schedule_pending_native(child, call);
            }
            EvalControl::Return(value) => {
                let mut frame = runtime.heap.pointer_as_frame(&item.frame)?;
                let parent = *frame.parent();
                mark_frame_complete(&mut frame, value);
                runtime.heap.replace_frame(&item.frame, frame)?;
                match stop {
                    EvalStop::RootSentinel => {
                        if is_root_frame_parent(&runtime.heap, &parent)? {
                            return Ok(value);
                        }
                    }
                    EvalStop::Parent(stop_parent) => {
                        if parent == stop_parent {
                            return Ok(value);
                        }
                        if is_root_frame_parent(&runtime.heap, &parent)? {
                            return Err(EngineError::Internal(
                                "child evaluation reached root before parent frame".into(),
                            ));
                        }
                    }
                }
                scheduler.schedule_next(EvalWorkItem::receive(parent, item.frame, value));
            }
        }
    }
}

fn refresh_eval_roots<State>(
    runtime: &mut RuntimeSnapshot<State>,
    item: &mut EvalWorkItem,
    scheduler: &mut EvalScheduler<State>,
    roots: &TempRoots,
) -> Result<(), EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let mut cursor = 0;
    item.refresh_from_roots(roots, &mut cursor)?;
    scheduler.refresh_from_roots(roots, &mut cursor)?;
    runtime.refresh_from_roots(roots, &mut cursor)
}

fn frame_for_expr(parent: Pointer, expr: Arc<TypedExpr>, env: Environment) -> Frame {
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
    runtime: &RuntimeSnapshot<State>,
    frame_ptr: Pointer,
    frame: Frame,
) -> Result<EvalControl<State>, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    match frame {
        Frame::Bool(frame) => match frame.expr.kind.as_ref() {
            TypedExprKind::Bool(value) => {
                Ok(EvalControl::Return(runtime.heap.alloc_ptr_bool(*value)?))
            }
            _ => frame_kind_error("bool"),
        },
        Frame::Uint(frame) => match frame.expr.kind.as_ref() {
            TypedExprKind::Uint(value) => Ok(EvalControl::Return(alloc_uint_literal_as(
                runtime,
                *value,
                &frame.expr.typ,
            )?)),
            _ => frame_kind_error("uint"),
        },
        Frame::Int(frame) => match frame.expr.kind.as_ref() {
            TypedExprKind::Int(value) => Ok(EvalControl::Return(alloc_int_literal_as(
                runtime,
                *value,
                &frame.expr.typ,
            )?)),
            _ => frame_kind_error("int"),
        },
        Frame::Float(frame) => match frame.expr.kind.as_ref() {
            TypedExprKind::Float(value) => Ok(EvalControl::Return(
                runtime.heap.alloc_ptr_f32(*value as f32)?,
            )),
            _ => frame_kind_error("float"),
        },
        Frame::String(frame) => match frame.expr.kind.as_ref() {
            TypedExprKind::String(value) => Ok(EvalControl::Return(
                runtime.heap.alloc_ptr_string(value.clone())?,
            )),
            _ => frame_kind_error("string"),
        },
        Frame::Uuid(frame) => match frame.expr.kind.as_ref() {
            TypedExprKind::Uuid(value) => {
                Ok(EvalControl::Return(runtime.heap.alloc_ptr_uuid(*value)?))
            }
            _ => frame_kind_error("uuid"),
        },
        Frame::DateTime(frame) => match frame.expr.kind.as_ref() {
            TypedExprKind::DateTime(value) => Ok(EvalControl::Return(
                runtime.heap.alloc_ptr_datetime(*value)?,
            )),
            _ => frame_kind_error("datetime"),
        },
        Frame::Hole(_) => Err(EngineError::UnsupportedExpr),
        Frame::Tuple(frame) => eval_tuple_enter(runtime, frame_ptr, frame),
        Frame::List(frame) => eval_list_enter(runtime, frame_ptr, frame),
        Frame::Dict(frame) => eval_dict_enter(runtime, frame_ptr, frame),
        Frame::RecordUpdate(mut frame) => {
            let base = match frame.expr.kind.as_ref() {
                TypedExprKind::RecordUpdate { base, .. } => Arc::clone(base),
                _ => return frame_kind_error("record update"),
            };
            frame.state = FrRecordUpdateState::EvalBase;
            let env = frame.env.clone();
            runtime
                .heap
                .replace_frame(&frame_ptr, Frame::RecordUpdate(frame))?;
            Ok(EvalControl::Push { expr: base, env })
        }
        Frame::Var(mut frame) => match frame.expr.kind.as_ref() {
            TypedExprKind::Var { name, .. } => {
                match eval_resolve_var(runtime, frame_ptr, &frame.env, name, &frame.expr.typ)? {
                    EvalVarResult::Value(value) => Ok(EvalControl::Return(value)),
                    EvalVarResult::Push { expr, env } => {
                        frame.state = FrValueState::Enter;
                        runtime.heap.replace_frame(&frame_ptr, Frame::Var(frame))?;
                        Ok(EvalControl::Push { expr, env })
                    }
                    EvalVarResult::AwaitNative(future) => {
                        frame.state = FrValueState::Enter;
                        runtime.heap.replace_frame(&frame_ptr, Frame::Var(frame))?;
                        Ok(EvalControl::AwaitNative(future))
                    }
                }
            }
            _ => frame_kind_error("var"),
        },
        Frame::App(frame) => eval_app_enter(runtime, frame_ptr, frame),
        Frame::Project(mut frame) => {
            let expr = match frame.expr.kind.as_ref() {
                TypedExprKind::Project { expr, .. } => Arc::clone(expr),
                _ => return frame_kind_error("project"),
            };
            frame.state = FrValueState::Enter;
            let env = frame.env.clone();
            runtime
                .heap
                .replace_frame(&frame_ptr, Frame::Project(frame))?;
            Ok(EvalControl::Push { expr, env })
        }
        Frame::Lam(frame) => match frame.expr.kind.as_ref() {
            TypedExprKind::Lam { param, body } => {
                let param_ty = split_fun(&frame.expr.typ)
                    .map(|(arg, _)| arg)
                    .ok_or_else(|| EngineError::NotCallable(frame.expr.typ.to_string()))?;
                let value = runtime.heap.alloc_ptr_closure(
                    frame.env.clone(),
                    param.clone(),
                    param_ty,
                    frame.expr.typ.clone(),
                    Arc::clone(body),
                )?;
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
            runtime.heap.replace_frame(&frame_ptr, Frame::Let(frame))?;
            Ok(EvalControl::Push { expr: def, env })
        }
        Frame::LetRec(frame) => {
            let TypedExprKind::LetRec { bindings, body } = frame.expr.kind.as_ref() else {
                return frame_kind_error("let rec");
            };
            let bindings = bindings.clone();
            let body = Arc::clone(body);
            let mut protected = vec![frame_ptr];
            Frame::LetRec(frame.clone()).trace_pointers(&mut protected);
            let frame_roots = runtime.heap.temp_roots(protected.clone())?;
            let mut slot_roots = Vec::with_capacity(bindings.len());
            for (name, _) in &bindings {
                let placeholder = runtime.heap.alloc_ptr_uninitialized(name.clone())?;
                slot_roots.push(runtime.heap.temp_roots(vec![placeholder])?);
            }
            let frame_ptr = frame_roots.get(0)?;
            let mut wrapped = Frame::LetRec(frame);
            refresh_frame_from_roots(&mut wrapped, &protected, &frame_roots, 1)?;
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
                runtime
                    .heap
                    .replace_frame(&frame_ptr, Frame::LetRec(frame))?;
                return Ok(EvalControl::Push {
                    expr: body,
                    env: recursive_env,
                });
            }
            frame.state = FrLetRecState::EvalBinding;
            let def = Arc::clone(&bindings[0].1);
            runtime
                .heap
                .replace_frame(&frame_ptr, Frame::LetRec(frame))?;
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
            runtime.heap.replace_frame(&frame_ptr, Frame::Ite(frame))?;
            Ok(EvalControl::Push { expr: cond, env })
        }
        Frame::Match(mut frame) => {
            let scrutinee = match frame.expr.kind.as_ref() {
                TypedExprKind::Match { scrutinee, .. } => Arc::clone(scrutinee),
                _ => return frame_kind_error("match"),
            };
            frame.state = FrMatchState::EvalScrutinee;
            let env = frame.env.clone();
            runtime
                .heap
                .replace_frame(&frame_ptr, Frame::Match(frame))?;
            Ok(EvalControl::Push {
                expr: scrutinee,
                env,
            })
        }
        Frame::NativeCall(frame) => eval_native_enter(runtime, frame_ptr, frame),
        Frame::NativeAsync(_) => unexpected_child_result("native async"),
    }
}

fn eval_tuple_enter<State>(
    runtime: &RuntimeSnapshot<State>,
    frame_ptr: Pointer,
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
        return Ok(EvalControl::Return(runtime.heap.alloc_ptr_tuple(vec![])?));
    }

    frame.state = FrSequenceState::EvalItem;
    frame.children = Vec::with_capacity(elems.len());
    frame.values = vec![None; elems.len()];
    frame.remaining = elems.len();
    runtime
        .heap
        .replace_frame(&frame_ptr, Frame::Tuple(frame))?;

    let roots = runtime.heap.temp_roots(vec![frame_ptr])?;
    for expr in elems {
        let current_frame_ptr = roots.get(0)?;
        let current_frame = match runtime.heap.pointer_as_frame(&current_frame_ptr)? {
            Frame::Tuple(frame) => frame,
            _ => return frame_kind_error("tuple"),
        };
        let child = runtime.heap.alloc_ptr_frame(frame_for_expr(
            current_frame_ptr,
            expr,
            current_frame.env.clone(),
        ))?;
        let current_frame_ptr = roots.get(0)?;
        let mut current_frame = match runtime.heap.pointer_as_frame(&current_frame_ptr)? {
            Frame::Tuple(frame) => frame,
            _ => return frame_kind_error("tuple"),
        };
        current_frame.children.push(child);
        runtime
            .heap
            .replace_frame(&current_frame_ptr, Frame::Tuple(current_frame))?;
    }

    let current_frame_ptr = roots.get(0)?;
    let current_frame = match runtime.heap.pointer_as_frame(&current_frame_ptr)? {
        Frame::Tuple(frame) => frame,
        _ => return frame_kind_error("tuple"),
    };
    Ok(EvalControl::Schedule(current_frame.children))
}

fn eval_list_enter<State>(
    runtime: &RuntimeSnapshot<State>,
    frame_ptr: Pointer,
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
        return Ok(EvalControl::Return(
            runtime
                .heap
                .alloc_ptr_adt(Symbol::intern("Empty"), vec![])?,
        ));
    }

    frame.state = FrSequenceState::EvalItem;
    frame.children = Vec::with_capacity(elems.len());
    frame.values = vec![None; elems.len()];
    frame.remaining = elems.len();
    runtime.heap.replace_frame(&frame_ptr, Frame::List(frame))?;

    let roots = runtime.heap.temp_roots(vec![frame_ptr])?;
    for expr in elems {
        let current_frame_ptr = roots.get(0)?;
        let current_frame = match runtime.heap.pointer_as_frame(&current_frame_ptr)? {
            Frame::List(frame) => frame,
            _ => return frame_kind_error("list"),
        };
        let child = runtime.heap.alloc_ptr_frame(frame_for_expr(
            current_frame_ptr,
            expr,
            current_frame.env.clone(),
        ))?;
        let current_frame_ptr = roots.get(0)?;
        let mut current_frame = match runtime.heap.pointer_as_frame(&current_frame_ptr)? {
            Frame::List(frame) => frame,
            _ => return frame_kind_error("list"),
        };
        current_frame.children.push(child);
        runtime
            .heap
            .replace_frame(&current_frame_ptr, Frame::List(current_frame))?;
    }

    let current_frame_ptr = roots.get(0)?;
    let current_frame = match runtime.heap.pointer_as_frame(&current_frame_ptr)? {
        Frame::List(frame) => frame,
        _ => return frame_kind_error("list"),
    };
    Ok(EvalControl::Schedule(current_frame.children))
}

fn eval_dict_enter<State>(
    runtime: &RuntimeSnapshot<State>,
    frame_ptr: Pointer,
    mut frame: FrDict,
) -> Result<EvalControl<State>, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let exprs = dict_exprs_for_keys(&frame, &frame.keys)?;
    if exprs.is_empty() {
        return Ok(EvalControl::Return(
            runtime.heap.alloc_ptr_dict(BTreeMap::new())?,
        ));
    }

    frame.state = FrSequenceState::EvalItem;
    frame.children = Vec::with_capacity(exprs.len());
    frame.values = vec![None; exprs.len()];
    frame.remaining = exprs.len();
    runtime.heap.replace_frame(&frame_ptr, Frame::Dict(frame))?;

    let roots = runtime.heap.temp_roots(vec![frame_ptr])?;
    for expr in exprs {
        let current_frame_ptr = roots.get(0)?;
        let current_frame = match runtime.heap.pointer_as_frame(&current_frame_ptr)? {
            Frame::Dict(frame) => frame,
            _ => return frame_kind_error("dict"),
        };
        let child = runtime.heap.alloc_ptr_frame(frame_for_expr(
            current_frame_ptr,
            expr,
            current_frame.env.clone(),
        ))?;
        let current_frame_ptr = roots.get(0)?;
        let mut current_frame = match runtime.heap.pointer_as_frame(&current_frame_ptr)? {
            Frame::Dict(frame) => frame,
            _ => return frame_kind_error("dict"),
        };
        current_frame.children.push(child);
        runtime
            .heap
            .replace_frame(&current_frame_ptr, Frame::Dict(current_frame))?;
    }

    let current_frame_ptr = roots.get(0)?;
    let current_frame = match runtime.heap.pointer_as_frame(&current_frame_ptr)? {
        Frame::Dict(frame) => frame,
        _ => return frame_kind_error("dict"),
    };
    Ok(EvalControl::Schedule(current_frame.children))
}

fn eval_record_update_updates_enter<State>(
    runtime: &RuntimeSnapshot<State>,
    frame_ptr: Pointer,
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
    runtime
        .heap
        .replace_frame(&frame_ptr, Frame::RecordUpdate(frame))?;

    let roots = runtime.heap.temp_roots(vec![frame_ptr])?;
    for expr in exprs {
        let current_frame_ptr = roots.get(0)?;
        let current_frame = match runtime.heap.pointer_as_frame(&current_frame_ptr)? {
            Frame::RecordUpdate(frame) => frame,
            _ => return frame_kind_error("record update"),
        };
        let child = runtime.heap.alloc_ptr_frame(frame_for_expr(
            current_frame_ptr,
            expr,
            current_frame.env.clone(),
        ))?;
        let current_frame_ptr = roots.get(0)?;
        let mut current_frame = match runtime.heap.pointer_as_frame(&current_frame_ptr)? {
            Frame::RecordUpdate(frame) => frame,
            _ => return frame_kind_error("record update"),
        };
        current_frame.update_children.push(child);
        runtime
            .heap
            .replace_frame(&current_frame_ptr, Frame::RecordUpdate(current_frame))?;
    }

    let current_frame_ptr = roots.get(0)?;
    let current_frame = match runtime.heap.pointer_as_frame(&current_frame_ptr)? {
        Frame::RecordUpdate(frame) => frame,
        _ => return frame_kind_error("record update"),
    };
    Ok(EvalControl::Schedule(current_frame.update_children))
}

fn eval_app_enter<State>(
    runtime: &RuntimeSnapshot<State>,
    frame_ptr: Pointer,
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
    runtime.heap.replace_frame(&frame_ptr, Frame::App(frame))?;

    let roots = runtime.heap.temp_roots(vec![frame_ptr])?;
    let current_frame_ptr = roots.get(0)?;
    let current_frame = match runtime.heap.pointer_as_frame(&current_frame_ptr)? {
        Frame::App(frame) => frame,
        _ => return frame_kind_error("application"),
    };
    let head_child = runtime.heap.alloc_ptr_frame(frame_for_expr(
        current_frame_ptr,
        head,
        current_frame.env.clone(),
    ))?;
    let current_frame_ptr = roots.get(0)?;
    let mut current_frame = match runtime.heap.pointer_as_frame(&current_frame_ptr)? {
        Frame::App(frame) => frame,
        _ => return frame_kind_error("application"),
    };
    current_frame.head_child = Some(head_child);
    runtime
        .heap
        .replace_frame(&current_frame_ptr, Frame::App(current_frame))?;

    for expr in arg_exprs {
        let current_frame_ptr = roots.get(0)?;
        let current_frame = match runtime.heap.pointer_as_frame(&current_frame_ptr)? {
            Frame::App(frame) => frame,
            _ => return frame_kind_error("application"),
        };
        let child = runtime.heap.alloc_ptr_frame(frame_for_expr(
            current_frame_ptr,
            expr,
            current_frame.env.clone(),
        ))?;
        let current_frame_ptr = roots.get(0)?;
        let mut current_frame = match runtime.heap.pointer_as_frame(&current_frame_ptr)? {
            Frame::App(frame) => frame,
            _ => return frame_kind_error("application"),
        };
        current_frame.arg_children.push(child);
        runtime
            .heap
            .replace_frame(&current_frame_ptr, Frame::App(current_frame))?;
    }

    let current_frame_ptr = roots.get(0)?;
    let current_frame = match runtime.heap.pointer_as_frame(&current_frame_ptr)? {
        Frame::App(frame) => frame,
        _ => return frame_kind_error("application"),
    };
    let mut children = Vec::with_capacity(1 + current_frame.arg_children.len());
    children.push(
        current_frame
            .head_child
            .ok_or_else(|| EngineError::Internal("application frame missing head child".into()))?,
    );
    children.extend(current_frame.arg_children.iter().copied());
    Ok(EvalControl::Schedule(children))
}

fn receive_sequence_value(
    kind: &'static str,
    children: &[Pointer],
    values: &mut [Option<Pointer>],
    remaining: &mut usize,
    child: Pointer,
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
    child: Pointer,
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
    runtime: &RuntimeSnapshot<State>,
    frame_ptr: Pointer,
    frame: Frame,
    child: Pointer,
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
                return Ok(EvalControl::Return(runtime.heap.alloc_ptr_tuple(values)?));
            }
            runtime
                .heap
                .replace_frame(&frame_ptr, Frame::Tuple(frame))?;
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
            runtime.heap.replace_frame(&frame_ptr, Frame::List(frame))?;
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
                return Ok(EvalControl::Return(runtime.heap.alloc_ptr_dict(values)?));
            }
            runtime.heap.replace_frame(&frame_ptr, Frame::Dict(frame))?;
            Ok(EvalControl::Wait)
        }
        Frame::RecordUpdate(mut frame) => match frame.state {
            FrRecordUpdateState::EvalBase => {
                frame.base_value = Some(value);
                if frame.update_keys.is_empty() {
                    let result = apply_record_update_values(runtime, value, BTreeMap::new())?;
                    return Ok(EvalControl::Return(result));
                }
                eval_record_update_updates_enter(runtime, frame_ptr, frame)
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
                runtime
                    .heap
                    .replace_frame(&frame_ptr, Frame::RecordUpdate(frame))?;
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
                    return continue_app_after_apply(runtime, frame_ptr, frame, None);
                }
                runtime.heap.replace_frame(&frame_ptr, Frame::App(frame))?;
                Ok(EvalControl::Wait)
            }
            FrAppState::ApplyArg => {
                continue_app_after_apply(runtime, frame_ptr, frame, Some(value))
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
                runtime.heap.replace_frame(&frame_ptr, Frame::Let(frame))?;
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
                runtime.heap.overwrite(&slot, cell)?;
                frame.binding_value = Some(value);
                frame.next_binding_index += 1;
                let recursive_env = frame.recursive_env.clone().ok_or_else(|| {
                    EngineError::Internal("let rec frame missing recursive environment".into())
                })?;
                if frame.next_binding_index == bindings.len() {
                    frame.state = FrLetRecState::EvalBody;
                    let body = Arc::clone(body);
                    runtime
                        .heap
                        .replace_frame(&frame_ptr, Frame::LetRec(frame))?;
                    return Ok(EvalControl::Push {
                        expr: body,
                        env: recursive_env,
                    });
                }
                let def = Arc::clone(&bindings[frame.next_binding_index].1);
                runtime
                    .heap
                    .replace_frame(&frame_ptr, Frame::LetRec(frame))?;
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
                let selected = match runtime.heap.pointer_as_bool(&value) {
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
                runtime.heap.replace_frame(&frame_ptr, Frame::Ite(frame))?;
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
                for idx in frame.next_arm_index..frame.arms.len() {
                    let arm = &frame.arms[idx];
                    if let Some(bindings) = match_pattern_ptr(&runtime.heap, &arm.pattern, &value) {
                        let env = frame.env.extend_many(bindings);
                        let expr = Arc::clone(&arm.expr);
                        frame.next_arm_index = idx;
                        frame.matched_env = Some(env.clone());
                        frame.state = FrMatchState::EvalArm;
                        runtime
                            .heap
                            .replace_frame(&frame_ptr, Frame::Match(frame))?;
                        return Ok(EvalControl::Push { expr, env });
                    }
                }
                Err(EngineError::MatchFailure)
            }
            FrMatchState::EvalArm => Ok(EvalControl::Return(value)),
            _ => unexpected_child_result("match"),
        },
        Frame::NativeCall(frame) => eval_native_receive(runtime, frame_ptr, frame, value),
        Frame::NativeAsync(_) => Ok(EvalControl::Return(value)),
        _ => unexpected_child_result("value"),
    }
}

enum NativeStep {
    Push {
        expr: Arc<TypedExpr>,
        env: Environment,
    },
    Return(Pointer),
}

fn eval_native_enter<State>(
    runtime: &RuntimeSnapshot<State>,
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
    let step = native_task_enter(runtime, &mut frame.task)?;
    let frame_ptr = roots.get(0)?;
    refresh_native_frame_from_roots(&mut frame, &protected, &roots, 1)?;
    native_step_to_control(runtime, frame_ptr, frame, step)
}

fn eval_native_receive<State>(
    runtime: &RuntimeSnapshot<State>,
    frame_ptr: Pointer,
    mut frame: FrNativeCall,
    value: Pointer,
) -> Result<EvalControl<State>, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    if frame.state != FrNativeCallState::Waiting {
        return unexpected_child_result("native call");
    }
    let mut protected = vec![frame_ptr, value];
    Frame::NativeCall(frame.clone()).trace_pointers(&mut protected);
    let roots = runtime.heap.temp_roots(protected.clone())?;
    let step = native_task_receive(runtime, &mut frame.task, value)?;
    let frame_ptr = roots.get(0)?;
    refresh_native_frame_from_roots(&mut frame, &protected, &roots, 1)?;
    native_step_to_control(runtime, frame_ptr, frame, step)
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

fn refresh_frame_from_roots(
    frame: &mut Frame,
    originals: &[Pointer],
    roots: &TempRoots,
    start: usize,
) -> Result<(), EngineError> {
    let mut rewrites = HashMap::new();
    for (idx, original) in originals.iter().enumerate().skip(start) {
        rewrites.insert(*original, roots.get(idx)?);
    }
    frame.rewrite_pointers(&mut |pointer| Ok(rewrites.get(&pointer).copied().unwrap_or(pointer)))
}

fn native_step_to_control<State>(
    runtime: &RuntimeSnapshot<State>,
    frame_ptr: Pointer,
    mut frame: FrNativeCall,
    step: NativeStep,
) -> Result<EvalControl<State>, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    match step {
        NativeStep::Return(value) => Ok(EvalControl::Return(value)),
        NativeStep::Push { expr, env } => {
            frame.state = FrNativeCallState::Waiting;
            runtime
                .heap
                .replace_frame(&frame_ptr, Frame::NativeCall(frame))?;
            Ok(EvalControl::Push { expr, env })
        }
    }
}

fn native_task_enter<State>(
    runtime: &RuntimeSnapshot<State>,
    task: &mut NativeTask,
) -> Result<NativeStep, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    match task {
        NativeTask::ApplyUnary(task) => native_apply_step(
            task.func,
            task.func_type.clone(),
            task.arg,
            task.arg_type.clone(),
        ),
        NativeTask::SequenceMap(task) => native_sequence_map_enter(runtime, task),
        NativeTask::SequenceFilter(task) => native_sequence_filter_enter(runtime, task),
        NativeTask::SequenceFilterMap(task) => native_sequence_filter_map_enter(runtime, task),
        NativeTask::SequenceFlatMap(task) => native_sequence_flat_map_enter(runtime, task),
        NativeTask::UnaryMap(task) => native_apply_step(
            task.func,
            task.func_type.clone(),
            task.value,
            task.elem_type.clone(),
        ),
        NativeTask::UnaryFilter(task) => native_apply_step(
            task.func,
            task.func_type.clone(),
            task.value,
            task.elem_type.clone(),
        ),
        NativeTask::UnaryFilterMap(task) => native_apply_step(
            task.func,
            task.func_type.clone(),
            task.value,
            task.elem_type.clone(),
        ),
        NativeTask::UnaryFlatMap(task) => native_apply_step(
            task.func,
            task.func_type.clone(),
            task.value,
            task.elem_type.clone(),
        ),
        NativeTask::Fold(task) => native_fold_enter(runtime, task),
        NativeTask::DictMap(task) => native_dict_map_enter(runtime, task),
        NativeTask::DictTraverseResult(task) => native_dict_traverse_enter(runtime, task),
        NativeTask::ArrayEq(task) => native_array_eq_enter(runtime, task),
        NativeTask::Sum(task) => native_sum_enter(runtime, task),
        NativeTask::Mean(task) => native_mean_enter(runtime, task),
        NativeTask::LogShow(task) => native_log_show_enter(runtime, task),
    }
}

fn native_task_receive<State>(
    runtime: &RuntimeSnapshot<State>,
    task: &mut NativeTask,
    value: Pointer,
) -> Result<NativeStep, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    match task {
        NativeTask::ApplyUnary(_) => Ok(NativeStep::Return(value)),
        NativeTask::SequenceMap(task) => native_sequence_map_receive(runtime, task, value),
        NativeTask::SequenceFilter(task) => native_sequence_filter_receive(runtime, task, value),
        NativeTask::SequenceFilterMap(task) => {
            native_sequence_filter_map_receive(runtime, task, value)
        }
        NativeTask::SequenceFlatMap(task) => native_sequence_flat_map_receive(runtime, task, value),
        NativeTask::UnaryMap(task) => native_unary_map_receive(runtime, task, value),
        NativeTask::UnaryFilter(task) => native_unary_filter_receive(runtime, task, value),
        NativeTask::UnaryFilterMap(_) => Ok(NativeStep::Return(value)),
        NativeTask::UnaryFlatMap(task) => native_unary_flat_map_receive(runtime, task, value),
        NativeTask::Fold(task) => native_fold_receive(runtime, task, value),
        NativeTask::DictMap(task) => native_dict_map_receive(runtime, task, value),
        NativeTask::DictTraverseResult(task) => native_dict_traverse_receive(runtime, task, value),
        NativeTask::ArrayEq(task) => native_array_eq_receive(runtime, task, value),
        NativeTask::Sum(task) => native_sum_receive(runtime, task, value),
        NativeTask::Mean(task) => native_mean_receive(runtime, task, value),
        NativeTask::LogShow(task) => native_log_show_receive(runtime, task, value),
    }
}

fn native_apply_step(
    func: Pointer,
    func_type: Type,
    arg: Pointer,
    arg_type: Type,
) -> Result<NativeStep, EngineError> {
    let (env, expr) = synthetic_application_expr(func, func_type, &[(arg, arg_type)])?;
    Ok(NativeStep::Push {
        expr: Arc::new(expr),
        env,
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

fn alloc_native_sequence<State>(
    runtime: &RuntimeSnapshot<State>,
    shape: &NativeSequenceShape,
    values: Vec<Pointer>,
) -> Result<Pointer, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    match shape {
        NativeSequenceShape::List => runtime.heap.alloc_ptr_list(values),
        NativeSequenceShape::Array => runtime.heap.alloc_ptr_array(values),
    }
}

fn option_from_native_pointer<State>(
    runtime: &RuntimeSnapshot<State>,
    value: Option<Pointer>,
) -> Result<Pointer, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    match value {
        Some(value) => runtime
            .heap
            .alloc_ptr_adt(Symbol::intern("Some"), vec![value]),
        None => runtime.heap.alloc_ptr_adt(Symbol::intern("None"), vec![]),
    }
}

fn result_from_native_pointer<State>(
    runtime: &RuntimeSnapshot<State>,
    value: Result<Pointer, Pointer>,
) -> Result<Pointer, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    match value {
        Ok(value) => runtime
            .heap
            .alloc_ptr_adt(Symbol::intern("Ok"), vec![value]),
        Err(value) => runtime
            .heap
            .alloc_ptr_adt(Symbol::intern("Err"), vec![value]),
    }
}

fn option_value_ptr<State>(
    runtime: &RuntimeSnapshot<State>,
    pointer: Pointer,
) -> Result<Option<Pointer>, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let (tag, args) = runtime.heap.pointer_as_adt(&pointer)?;
    if tag.as_ref() == "Some" && args.len() == 1 {
        Ok(Some(args[0]))
    } else if tag.as_ref() == "None" && args.is_empty() {
        Ok(None)
    } else {
        Err(EngineError::NativeType {
            expected: "Option".into(),
            got: runtime.heap.type_name(&pointer)?.into(),
        })
    }
}

fn result_value_ptr<State>(
    runtime: &RuntimeSnapshot<State>,
    pointer: Pointer,
) -> Result<Result<Pointer, Pointer>, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let (tag, args) = runtime.heap.pointer_as_adt(&pointer)?;
    if tag.as_ref() == "Ok" && args.len() == 1 {
        Ok(Ok(args[0]))
    } else if tag.as_ref() == "Err" && args.len() == 1 {
        Ok(Err(args[0]))
    } else {
        Err(EngineError::NativeType {
            expected: "Result".into(),
            got: runtime.heap.type_name(&pointer)?.into(),
        })
    }
}

fn native_flatten_sequence<State>(
    runtime: &RuntimeSnapshot<State>,
    shape: &NativeSequenceShape,
    pointer: Pointer,
) -> Result<Vec<Pointer>, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    match shape {
        NativeSequenceShape::List => runtime.heap.pointer_as_list(&pointer),
        NativeSequenceShape::Array => runtime.heap.pointer_as_array(&pointer),
    }
}

fn overloaded_pointer<State>(
    runtime: &RuntimeSnapshot<State>,
    name: &str,
    typ: Type,
) -> Result<Pointer, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let (name, typ, applied, applied_types) =
        OverloadedFn::new(Symbol::intern(name), typ).into_parts();
    runtime
        .heap
        .alloc_ptr_overloaded(name, typ, applied, applied_types)
}

fn binary_same_type(typ: &Type) -> Type {
    Type::fun(typ.clone(), Type::fun(typ.clone(), typ.clone()))
}

fn len_value_for_native_type<State>(
    runtime: &RuntimeSnapshot<State>,
    elem_ty: &Type,
    len: usize,
) -> Result<Pointer, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    match elem_ty.as_ref() {
        TypeKind::Con(c) if c.is_builtin(BuiltinTypeId::F32) => {
            runtime.heap.alloc_ptr_f32(len as f32)
        }
        TypeKind::Con(c) if c.is_builtin(BuiltinTypeId::F64) => {
            runtime.heap.alloc_ptr_f64(len as f64)
        }
        _ => Err(EngineError::NativeType {
            expected: "f32 or f64".into(),
            got: elem_ty.to_string(),
        }),
    }
}

fn native_sequence_map_enter<State>(
    runtime: &RuntimeSnapshot<State>,
    task: &mut NativeSequenceMap,
) -> Result<NativeStep, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    if task.values.is_empty() {
        return Ok(NativeStep::Return(alloc_native_sequence(
            runtime,
            &task.shape,
            Vec::new(),
        )?));
    }
    task.next_index = 0;
    native_apply_step(
        task.func,
        task.func_type.clone(),
        task.values[0],
        task.elem_type.clone(),
    )
}

fn native_sequence_map_receive<State>(
    runtime: &RuntimeSnapshot<State>,
    task: &mut NativeSequenceMap,
    value: Pointer,
) -> Result<NativeStep, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    task.output.push(value);
    task.next_index += 1;
    if task.next_index == task.values.len() {
        return Ok(NativeStep::Return(alloc_native_sequence(
            runtime,
            &task.shape,
            task.output.clone(),
        )?));
    }
    native_apply_step(
        task.func,
        task.func_type.clone(),
        task.values[task.next_index],
        task.elem_type.clone(),
    )
}

fn native_sequence_filter_enter<State>(
    runtime: &RuntimeSnapshot<State>,
    task: &mut NativeSequenceFilter,
) -> Result<NativeStep, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    if task.values.is_empty() {
        return Ok(NativeStep::Return(alloc_native_sequence(
            runtime,
            &task.shape,
            Vec::new(),
        )?));
    }
    task.next_index = 0;
    native_apply_step(
        task.func,
        task.func_type.clone(),
        task.values[0],
        task.elem_type.clone(),
    )
}

fn native_sequence_filter_receive<State>(
    runtime: &RuntimeSnapshot<State>,
    task: &mut NativeSequenceFilter,
    value: Pointer,
) -> Result<NativeStep, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    if runtime.heap.pointer_as_bool(&value)? {
        task.output.push(task.values[task.next_index]);
    }
    task.next_index += 1;
    if task.next_index == task.values.len() {
        return Ok(NativeStep::Return(alloc_native_sequence(
            runtime,
            &task.shape,
            task.output.clone(),
        )?));
    }
    native_apply_step(
        task.func,
        task.func_type.clone(),
        task.values[task.next_index],
        task.elem_type.clone(),
    )
}

fn native_sequence_filter_map_enter<State>(
    runtime: &RuntimeSnapshot<State>,
    task: &mut NativeSequenceFilterMap,
) -> Result<NativeStep, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    if task.values.is_empty() {
        return Ok(NativeStep::Return(alloc_native_sequence(
            runtime,
            &task.shape,
            Vec::new(),
        )?));
    }
    task.next_index = 0;
    native_apply_step(
        task.func,
        task.func_type.clone(),
        task.values[0],
        task.elem_type.clone(),
    )
}

fn native_sequence_filter_map_receive<State>(
    runtime: &RuntimeSnapshot<State>,
    task: &mut NativeSequenceFilterMap,
    value: Pointer,
) -> Result<NativeStep, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    if let Some(value) = option_value_ptr(runtime, value)? {
        task.output.push(value);
    }
    task.next_index += 1;
    if task.next_index == task.values.len() {
        return Ok(NativeStep::Return(alloc_native_sequence(
            runtime,
            &task.shape,
            task.output.clone(),
        )?));
    }
    native_apply_step(
        task.func,
        task.func_type.clone(),
        task.values[task.next_index],
        task.elem_type.clone(),
    )
}

fn native_sequence_flat_map_enter<State>(
    runtime: &RuntimeSnapshot<State>,
    task: &mut NativeSequenceFlatMap,
) -> Result<NativeStep, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    if task.values.is_empty() {
        return Ok(NativeStep::Return(alloc_native_sequence(
            runtime,
            &task.shape,
            Vec::new(),
        )?));
    }
    task.next_index = 0;
    native_apply_step(
        task.func,
        task.func_type.clone(),
        task.values[0],
        task.elem_type.clone(),
    )
}

fn native_sequence_flat_map_receive<State>(
    runtime: &RuntimeSnapshot<State>,
    task: &mut NativeSequenceFlatMap,
    value: Pointer,
) -> Result<NativeStep, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    task.output
        .extend(native_flatten_sequence(runtime, &task.shape, value)?);
    task.next_index += 1;
    if task.next_index == task.values.len() {
        return Ok(NativeStep::Return(alloc_native_sequence(
            runtime,
            &task.shape,
            task.output.clone(),
        )?));
    }
    native_apply_step(
        task.func,
        task.func_type.clone(),
        task.values[task.next_index],
        task.elem_type.clone(),
    )
}

fn native_unary_map_receive<State>(
    runtime: &RuntimeSnapshot<State>,
    task: &mut NativeUnaryMap,
    value: Pointer,
) -> Result<NativeStep, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let value = match &task.shape {
        NativeUnaryShape::Option => option_from_native_pointer(runtime, Some(value))?,
        NativeUnaryShape::Result => result_from_native_pointer(runtime, Ok(value))?,
    };
    Ok(NativeStep::Return(value))
}

fn native_unary_filter_receive<State>(
    runtime: &RuntimeSnapshot<State>,
    task: &mut NativeUnaryFilter,
    value: Pointer,
) -> Result<NativeStep, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let value = if runtime.heap.pointer_as_bool(&value)? {
        task.original
    } else {
        option_from_native_pointer(runtime, None)?
    };
    Ok(NativeStep::Return(value))
}

fn native_unary_flat_map_receive<State>(
    runtime: &RuntimeSnapshot<State>,
    task: &mut NativeUnaryFlatMap,
    value: Pointer,
) -> Result<NativeStep, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    if matches!(task.shape, NativeUnaryShape::Result) {
        let _ = result_value_ptr(runtime, value)?;
    }
    Ok(NativeStep::Return(value))
}

fn native_fold_enter<State>(
    runtime: &RuntimeSnapshot<State>,
    task: &mut NativeFold,
) -> Result<NativeStep, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    if task.values.is_empty() {
        return Ok(NativeStep::Return(task.acc));
    }
    task.state = NativeFoldState::ApplyFirst;
    task.next_index = 0;
    native_fold_apply_first(runtime, task)
}

fn native_fold_receive<State>(
    runtime: &RuntimeSnapshot<State>,
    task: &mut NativeFold,
    value: Pointer,
) -> Result<NativeStep, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    match task.state {
        NativeFoldState::ApplyFirst => {
            task.step = Some(value);
            task.state = NativeFoldState::ApplySecond;
            native_fold_apply_second(task)
        }
        NativeFoldState::ApplySecond => {
            task.acc = value;
            task.step = None;
            task.next_index += 1;
            if task.next_index == task.values.len() {
                return Ok(NativeStep::Return(task.acc));
            }
            task.state = NativeFoldState::ApplyFirst;
            native_fold_apply_first(runtime, task)
        }
        _ => unexpected_child_result("native fold"),
    }
}

fn native_fold_apply_first<State>(
    _runtime: &RuntimeSnapshot<State>,
    task: &NativeFold,
) -> Result<NativeStep, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let arg = match task.order {
        NativeFoldOrder::Left => task.acc,
        NativeFoldOrder::Right => task.values[task.next_index],
    };
    let arg_type = match task.order {
        NativeFoldOrder::Left => task.acc_type.clone(),
        NativeFoldOrder::Right => task.elem_type.clone(),
    };
    native_apply_step(task.func, task.func_type.clone(), arg, arg_type)
}

fn native_fold_apply_second(task: &NativeFold) -> Result<NativeStep, EngineError> {
    let step = task
        .step
        .ok_or_else(|| EngineError::Internal("native fold missing step function".into()))?;
    let arg = match task.order {
        NativeFoldOrder::Left => task.values[task.next_index],
        NativeFoldOrder::Right => task.acc,
    };
    let arg_type = match task.order {
        NativeFoldOrder::Left => task.elem_type.clone(),
        NativeFoldOrder::Right => task.acc_type.clone(),
    };
    let step_type = Type::fun(arg_type.clone(), task.acc_type.clone());
    native_apply_step(step, step_type, arg, arg_type)
}

fn native_dict_map_enter<State>(
    runtime: &RuntimeSnapshot<State>,
    task: &mut NativeDictMap,
) -> Result<NativeStep, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    if task.entries.is_empty() {
        return Ok(NativeStep::Return(
            runtime.heap.alloc_ptr_dict(BTreeMap::new())?,
        ));
    }
    task.next_index = 0;
    let (_, value) = task.entries[0].clone();
    native_apply_step(
        task.func,
        task.func_type.clone(),
        value,
        task.elem_type.clone(),
    )
}

fn native_dict_map_receive<State>(
    runtime: &RuntimeSnapshot<State>,
    task: &mut NativeDictMap,
    value: Pointer,
) -> Result<NativeStep, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let (key, _) = task.entries[task.next_index].clone();
    task.output.insert(key, value);
    task.next_index += 1;
    if task.next_index == task.entries.len() {
        return Ok(NativeStep::Return(
            runtime.heap.alloc_ptr_dict(task.output.clone())?,
        ));
    }
    let (_, value) = task.entries[task.next_index].clone();
    native_apply_step(
        task.func,
        task.func_type.clone(),
        value,
        task.elem_type.clone(),
    )
}

fn native_dict_traverse_enter<State>(
    runtime: &RuntimeSnapshot<State>,
    task: &mut NativeDictTraverseResult,
) -> Result<NativeStep, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    if task.entries.is_empty() {
        let dict = runtime.heap.alloc_ptr_dict(BTreeMap::new())?;
        return Ok(NativeStep::Return(result_from_native_pointer(
            runtime,
            Ok(dict),
        )?));
    }
    task.next_index = 0;
    let (_, value) = task.entries[0].clone();
    native_apply_step(
        task.func,
        task.func_type.clone(),
        value,
        task.elem_type.clone(),
    )
}

fn native_dict_traverse_receive<State>(
    runtime: &RuntimeSnapshot<State>,
    task: &mut NativeDictTraverseResult,
    value: Pointer,
) -> Result<NativeStep, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    match result_value_ptr(runtime, value)? {
        Ok(value) => {
            let (key, _) = task.entries[task.next_index].clone();
            task.output.insert(key, value);
        }
        Err(err) => {
            return Ok(NativeStep::Return(result_from_native_pointer(
                runtime,
                Err(err),
            )?));
        }
    }
    task.next_index += 1;
    if task.next_index == task.entries.len() {
        let dict = runtime.heap.alloc_ptr_dict(task.output.clone())?;
        return Ok(NativeStep::Return(result_from_native_pointer(
            runtime,
            Ok(dict),
        )?));
    }
    let (_, value) = task.entries[task.next_index].clone();
    native_apply_step(
        task.func,
        task.func_type.clone(),
        value,
        task.elem_type.clone(),
    )
}

fn native_array_eq_enter<State>(
    runtime: &RuntimeSnapshot<State>,
    task: &mut NativeArrayEq,
) -> Result<NativeStep, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    if task.xs.len() != task.ys.len() {
        return native_array_eq_result(runtime, task, false);
    }
    if task.xs.is_empty() {
        return native_array_eq_result(runtime, task, true);
    }
    task.state = NativeArrayEqState::ApplyFirst;
    task.next_index = 0;
    native_array_eq_apply_first(runtime, task)
}

fn native_array_eq_receive<State>(
    runtime: &RuntimeSnapshot<State>,
    task: &mut NativeArrayEq,
    value: Pointer,
) -> Result<NativeStep, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    match task.state {
        NativeArrayEqState::ApplyFirst => {
            task.step = Some(value);
            task.state = NativeArrayEqState::ApplySecond;
            native_array_eq_apply_second(task)
        }
        NativeArrayEqState::ApplySecond => {
            if !runtime.heap.pointer_as_bool(&value)? {
                return native_array_eq_result(runtime, task, false);
            }
            task.step = None;
            task.next_index += 1;
            if task.next_index == task.xs.len() {
                return native_array_eq_result(runtime, task, true);
            }
            task.state = NativeArrayEqState::ApplyFirst;
            native_array_eq_apply_first(runtime, task)
        }
        _ => unexpected_child_result("native array equality"),
    }
}

fn native_array_eq_apply_first<State>(
    runtime: &RuntimeSnapshot<State>,
    task: &NativeArrayEq,
) -> Result<NativeStep, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let bool_ty = Type::builtin(BuiltinTypeId::Bool);
    let eq_ty = Type::fun(
        task.elem_type.clone(),
        Type::fun(task.elem_type.clone(), bool_ty),
    );
    let lhs = task.xs[task.next_index];
    let roots = runtime.heap.temp_roots(vec![lhs])?;
    let eq = overloaded_pointer(runtime, "==", eq_ty.clone())?;
    let lhs = roots.get(0)?;
    native_apply_step(eq, eq_ty, lhs, task.elem_type.clone())
}

fn native_array_eq_apply_second(task: &NativeArrayEq) -> Result<NativeStep, EngineError> {
    let step = task
        .step
        .ok_or_else(|| EngineError::Internal("native array equality missing step".into()))?;
    let bool_ty = Type::builtin(BuiltinTypeId::Bool);
    let step_ty = Type::fun(task.elem_type.clone(), bool_ty);
    native_apply_step(
        step,
        step_ty,
        task.ys[task.next_index],
        task.elem_type.clone(),
    )
}

fn native_array_eq_result<State>(
    runtime: &RuntimeSnapshot<State>,
    task: &NativeArrayEq,
    equal: bool,
) -> Result<NativeStep, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    Ok(NativeStep::Return(runtime.heap.alloc_ptr_bool(
        if task.negate { !equal } else { equal },
    )?))
}

fn native_sum_enter<State>(
    runtime: &RuntimeSnapshot<State>,
    task: &mut NativeSum,
) -> Result<NativeStep, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    if task.values.is_empty() {
        return Ok(native_eval_var_step(
            Symbol::intern("zero"),
            task.elem_type.clone(),
        ));
    }
    task.acc = Some(task.values[0]);
    task.next_index = 1;
    if task.next_index == task.values.len() {
        return Ok(NativeStep::Return(task.values[0]));
    }
    task.state = NativeFoldState::ApplyFirst;
    native_sum_apply_first(runtime, task)
}

fn native_sum_receive<State>(
    runtime: &RuntimeSnapshot<State>,
    task: &mut NativeSum,
    value: Pointer,
) -> Result<NativeStep, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    match task.state {
        NativeFoldState::Enter => Ok(NativeStep::Return(value)),
        NativeFoldState::ApplyFirst => {
            task.step = Some(value);
            task.state = NativeFoldState::ApplySecond;
            native_sum_apply_second(task)
        }
        NativeFoldState::ApplySecond => {
            task.acc = Some(value);
            task.step = None;
            task.next_index += 1;
            if task.next_index == task.values.len() {
                return Ok(NativeStep::Return(value));
            }
            task.state = NativeFoldState::ApplyFirst;
            native_sum_apply_first(runtime, task)
        }
    }
}

fn native_sum_apply_first<State>(
    runtime: &RuntimeSnapshot<State>,
    task: &NativeSum,
) -> Result<NativeStep, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let plus_ty = binary_same_type(&task.elem_type);
    let acc = task
        .acc
        .ok_or_else(|| EngineError::Internal("native sum missing accumulator".into()))?;
    let roots = runtime.heap.temp_roots(vec![acc])?;
    let plus = overloaded_pointer(runtime, "+", plus_ty.clone())?;
    let acc = roots.get(0)?;
    native_apply_step(plus, plus_ty, acc, task.elem_type.clone())
}

fn native_sum_apply_second(task: &NativeSum) -> Result<NativeStep, EngineError> {
    let step = task
        .step
        .ok_or_else(|| EngineError::Internal("native sum missing step".into()))?;
    let step_ty = Type::fun(task.elem_type.clone(), task.elem_type.clone());
    native_apply_step(
        step,
        step_ty,
        task.values[task.next_index],
        task.elem_type.clone(),
    )
}

fn native_mean_enter<State>(
    runtime: &RuntimeSnapshot<State>,
    task: &mut NativeMean,
) -> Result<NativeStep, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    if task.values.is_empty() {
        return Err(EngineError::EmptySequence);
    }
    task.acc = Some(task.values[0]);
    task.next_index = 1;
    if task.next_index == task.values.len() {
        task.state = NativeMeanState::ApplyDivFirst;
        return native_mean_apply_div_first(runtime, task);
    }
    task.state = NativeMeanState::ApplyPlusFirst;
    native_mean_apply_plus_first(runtime, task)
}

fn native_mean_receive<State>(
    runtime: &RuntimeSnapshot<State>,
    task: &mut NativeMean,
    value: Pointer,
) -> Result<NativeStep, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    match task.state {
        NativeMeanState::ApplyPlusFirst => {
            task.step = Some(value);
            task.state = NativeMeanState::ApplyPlusSecond;
            native_mean_apply_plus_second(task)
        }
        NativeMeanState::ApplyPlusSecond => {
            task.acc = Some(value);
            task.step = None;
            task.next_index += 1;
            if task.next_index == task.values.len() {
                task.state = NativeMeanState::ApplyDivFirst;
                return native_mean_apply_div_first(runtime, task);
            }
            task.state = NativeMeanState::ApplyPlusFirst;
            native_mean_apply_plus_first(runtime, task)
        }
        NativeMeanState::ApplyDivFirst => {
            task.step = Some(value);
            task.state = NativeMeanState::ApplyDivSecond;
            native_mean_apply_div_second(task)
        }
        NativeMeanState::ApplyDivSecond => Ok(NativeStep::Return(value)),
        NativeMeanState::Enter => unexpected_child_result("native mean"),
    }
}

fn native_mean_apply_plus_first<State>(
    runtime: &RuntimeSnapshot<State>,
    task: &NativeMean,
) -> Result<NativeStep, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let plus_ty = binary_same_type(&task.elem_type);
    let acc = task
        .acc
        .ok_or_else(|| EngineError::Internal("native mean missing accumulator".into()))?;
    let roots = runtime.heap.temp_roots(vec![acc])?;
    let plus = overloaded_pointer(runtime, "+", plus_ty.clone())?;
    let acc = roots.get(0)?;
    native_apply_step(plus, plus_ty, acc, task.elem_type.clone())
}

fn native_mean_apply_plus_second(task: &NativeMean) -> Result<NativeStep, EngineError> {
    let step = task
        .step
        .ok_or_else(|| EngineError::Internal("native mean missing addition step".into()))?;
    let step_ty = Type::fun(task.elem_type.clone(), task.elem_type.clone());
    native_apply_step(
        step,
        step_ty,
        task.values[task.next_index],
        task.elem_type.clone(),
    )
}

fn native_mean_apply_div_first<State>(
    runtime: &RuntimeSnapshot<State>,
    task: &mut NativeMean,
) -> Result<NativeStep, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let div_ty = binary_same_type(&task.elem_type);
    let acc = task
        .acc
        .ok_or_else(|| EngineError::Internal("native mean missing accumulator".into()))?;
    let acc_root = runtime.heap.temp_roots(vec![acc])?;
    let div = overloaded_pointer(runtime, "/", div_ty.clone())?;
    let div_root = runtime.heap.temp_roots(vec![div])?;
    if task.len_value.is_none() {
        task.len_value = Some(len_value_for_native_type(
            runtime,
            &task.elem_type,
            task.len,
        )?);
    }
    let acc = acc_root.get(0)?;
    let div = div_root.get(0)?;
    native_apply_step(div, div_ty, acc, task.elem_type.clone())
}

fn native_mean_apply_div_second(task: &NativeMean) -> Result<NativeStep, EngineError> {
    let step = task
        .step
        .ok_or_else(|| EngineError::Internal("native mean missing division step".into()))?;
    let len_value = task
        .len_value
        .ok_or_else(|| EngineError::Internal("native mean missing length value".into()))?;
    let step_ty = Type::fun(task.elem_type.clone(), task.elem_type.clone());
    native_apply_step(step, step_ty, len_value, task.elem_type.clone())
}

fn native_log_show_enter<State>(
    runtime: &RuntimeSnapshot<State>,
    task: &NativeLogShow,
) -> Result<NativeStep, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let roots = runtime.heap.temp_roots(vec![task.arg])?;
    let show = overloaded_pointer(runtime, "show", task.show_type.clone())?;
    let arg = roots.get(0)?;
    native_apply_step(show, task.show_type.clone(), arg, task.arg_type.clone())
}

fn native_log_show_receive<State>(
    runtime: &RuntimeSnapshot<State>,
    task: &NativeLogShow,
    value: Pointer,
) -> Result<NativeStep, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let rendered = String::from_pointer(&runtime.heap, &value)?;
    (task.log)(&rendered);
    Ok(NativeStep::Return(runtime.heap.alloc_ptr_string(rendered)?))
}

fn eval_apply_overloaded_arg<State>(
    runtime: &RuntimeSnapshot<State>,
    parent: Pointer,
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
        return Ok(EvalApplyResult::Value(runtime.heap.alloc_ptr_overloaded(
            over.name,
            rest_ty,
            over.applied,
            over.applied_types,
        )?));
    }

    let mut full_ty = rest_ty;
    for arg_ty in over.applied_types.iter().rev() {
        full_ty = Type::fun(arg_ty.clone(), full_ty);
    }

    if runtime.type_system.class_methods.contains_key(&over.name) {
        let evaluator = EvaluatorRef::new_with_parent(runtime, parent);
        return match evaluator.resolve_class_method_plan(&over.name, &full_ty)? {
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

    let context = EvalContext::child(parent);
    let imp = EvaluatorRef::new_with_context(runtime, context)
        .resolve_native_impl(over.name.as_ref(), &full_ty)?;
    match imp
        .func
        .call_with_context(runtime, full_ty, &over.applied, context)?
    {
        NativeCallResult::Ready(value) => Ok(EvalApplyResult::Value(value)),
        NativeCallResult::Pending(future) => Ok(EvalApplyResult::AwaitNative(future)),
    }
}

fn eval_apply_arg<State>(
    runtime: &RuntimeSnapshot<State>,
    parent: Pointer,
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
            match native.apply_with_context(runtime, arg, arg_type, EvalContext::child(parent))? {
                NativeApplyResult::Value(value) => Ok(EvalApplyResult::Value(value)),
                NativeApplyResult::Task(task) => Ok(EvalApplyResult::PushNative(task)),
                NativeApplyResult::Pending(future) => Ok(EvalApplyResult::AwaitNative(future)),
            }
        }
        Cell::Overloaded(over) => {
            eval_apply_overloaded_arg(runtime, parent, over, arg, func_type, arg_type)
        }
        _ => Err(EngineError::NotCallable(
            runtime.heap.type_name(&func)?.into(),
        )),
    }
}

fn continue_app_after_apply<State>(
    runtime: &RuntimeSnapshot<State>,
    mut frame_ptr: Pointer,
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
        runtime
            .heap
            .replace_frame(&frame_ptr, Frame::App(frame.clone()))?;
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
        runtime
            .heap
            .replace_frame(&frame_ptr, Frame::App(frame.clone()))?;
        let roots = runtime.heap.temp_roots(vec![frame_ptr, func, arg])?;
        let apply_result = eval_apply_arg(
            runtime,
            frame_ptr,
            func,
            arg,
            Some(&arg_info.func_type),
            Some(&arg_info.expr.typ),
        )?;
        frame_ptr = roots.get(0)?;
        frame = match runtime.heap.pointer_as_frame(&frame_ptr)? {
            Frame::App(frame) => frame,
            _ => return frame_kind_error("application"),
        };
        match apply_result {
            EvalApplyResult::Value(applied) => {
                frame.arg = None;
                frame.func = Some(applied);
                frame.next_arg_index += 1;
                runtime
                    .heap
                    .replace_frame(&frame_ptr, Frame::App(frame.clone()))?;
            }
            EvalApplyResult::Push { expr, env } => return Ok(EvalControl::Push { expr, env }),
            EvalApplyResult::PushNative(task) => {
                return Ok(EvalControl::PushFrame(Box::new(Frame::NativeCall(
                    FrNativeCall {
                        parent: frame_ptr,
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
    runtime: &RuntimeSnapshot<State>,
    parent: Pointer,
    env: &Environment,
    name: &Symbol,
    typ: &Type,
) -> Result<EvalVarResult<State>, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    if let Some(ptr) = env.get(name) {
        let native = runtime.heap.with_access(|heap| match heap.get(&ptr)? {
            Cell::Native(native) if native.arity == 0 && native.applied.is_empty() => {
                Ok(Some(native.clone()))
            }
            _ => Ok(None),
        })?;
        if let Some(native) = native {
            match native.call_zero_with_context(runtime, EvalContext::child(parent))? {
                NativeCallResult::Ready(value) => Ok(EvalVarResult::Value(value)),
                NativeCallResult::Pending(future) => Ok(EvalVarResult::AwaitNative(future)),
            }
        } else {
            Ok(EvalVarResult::Value(ptr))
        }
    } else if runtime.type_system.class_methods.contains_key(name) {
        let evaluator = EvaluatorRef::new_with_parent(runtime, parent);
        if let Some(pointer) = evaluator.cached_class_method(name, typ) {
            return Ok(EvalVarResult::Value(pointer));
        }
        match evaluator.resolve_class_method_plan(name, typ)? {
            Ok((env, specialized)) => Ok(EvalVarResult::Push {
                expr: Arc::new(specialized),
                env,
            }),
            Err(pointer) => Ok(EvalVarResult::Value(pointer)),
        }
    } else {
        let value =
            EvaluatorRef::new_with_parent(runtime, parent).resolve_native(name.as_ref(), typ)?;
        let native = runtime.heap.with_access(|heap| match heap.get(&value)? {
            Cell::Native(native) if native.arity == 0 && native.applied.is_empty() => {
                Ok(Some(native.clone()))
            }
            _ => Ok(None),
        })?;
        if let Some(native) = native {
            match native.call_zero_with_context(runtime, EvalContext::child(parent))? {
                NativeCallResult::Ready(value) => Ok(EvalVarResult::Value(value)),
                NativeCallResult::Pending(future) => Ok(EvalVarResult::AwaitNative(future)),
            }
        } else {
            Ok(EvalVarResult::Value(value))
        }
    }
}

fn apply_record_update_values<State>(
    runtime: &RuntimeSnapshot<State>,
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

    let target = runtime.heap.with_access(|heap| {
        let base_val = heap.get(&base_ptr)?;
        match base_val {
            Cell::Dict(map) => Ok(RecordUpdateTarget::Dict(map.clone())),
            Cell::Adt(tag, args) if args.len() == 1 => {
                let inner = heap.get(&args[0])?;
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
            runtime.heap.alloc_ptr_dict(map)
        }
        RecordUpdateTarget::Adt(tag, mut map) => {
            for (key, value) in update_vals {
                map.insert(key, value);
            }
            let dict = runtime.heap.alloc_ptr_dict(map)?;
            runtime.heap.alloc_ptr_adt(tag, vec![dict])
        }
    }
}

fn mark_frame_complete(frame: &mut Frame, value: Pointer) {
    match frame {
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

fn is_root_frame_parent(heap: &Heap, pointer: &Pointer) -> Result<bool, EngineError> {
    heap.with_access(|heap| match heap.get(pointer)? {
        Cell::U64(0) => Ok(true),
        Cell::Frame(_) => Ok(false),
        other => Err(EngineError::Internal(format!(
            "unexpected frame parent value {}",
            other.cell_type_name()
        ))),
    })
}

fn frame_kind_error<T>(expected: &'static str) -> Result<T, EngineError> {
    Err(EngineError::Internal(format!(
        "frame does not match typed expression kind `{expected}`"
    )))
}

fn unexpected_child_result<T>(frame: &'static str) -> Result<T, EngineError> {
    Err(EngineError::Internal(format!(
        "{frame} frame received an unexpected child result"
    )))
}

fn match_pattern_ptr(
    heap: &Heap,
    pat: &Pattern,
    value: &Pointer,
) -> Option<BTreeMap<Symbol, Pointer>> {
    heap.with_access(|heap| Ok(match_pattern_ptr_with_access(heap, pat, value)))
        .ok()
        .flatten()
}

fn match_pattern_ptr_with_access(
    heap: &HeapAccess<'_>,
    pat: &Pattern,
    value: &Pointer,
) -> Option<BTreeMap<Symbol, Pointer>> {
    match pat {
        Pattern::Wildcard(..) => Some(BTreeMap::new()),
        Pattern::Var(var) => {
            let mut bindings = BTreeMap::new();
            bindings.insert(var.name.clone(), *value);
            Some(bindings)
        }
        Pattern::Named(_, name, ps) => {
            let v = heap.get(value).ok()?;
            match v {
                Cell::Adt(vname, args)
                    if runtime_ctor_matches(vname, &name.to_dotted_symbol())
                        && args.len() == ps.len() =>
                {
                    match_patterns_with_access(heap, ps, args)
                }
                _ => None,
            }
        }
        Pattern::Tuple(_, ps) => {
            let v = heap.get(value).ok()?;
            match v {
                Cell::Tuple(xs) if xs.len() == ps.len() => match_patterns_with_access(heap, ps, xs),
                _ => None,
            }
        }
        Pattern::List(_, ps) => {
            let v = heap.get(value).ok()?;
            let values = list_to_vec(heap, v).ok()?;
            if values.len() == ps.len() {
                match_patterns_with_access(heap, ps, &values)
            } else {
                None
            }
        }
        Pattern::Cons(_, head, tail) => {
            let v = heap.get(value).ok()?;
            match v {
                Cell::Adt(tag, args) if tag.as_ref() == "Cons" && args.len() == 2 => {
                    let mut left = match_pattern_ptr_with_access(heap, head, &args[0])?;
                    let right = match_pattern_ptr_with_access(heap, tail, &args[1])?;
                    left.extend(right);
                    Some(left)
                }
                _ => None,
            }
        }
        Pattern::Dict(_, fields) => {
            let v = heap.get(value).ok()?;
            match v {
                Cell::Dict(map) => {
                    let mut bindings = BTreeMap::new();
                    for (key, pat) in fields {
                        let v = map.get(key)?;
                        let sub = match_pattern_ptr_with_access(heap, pat, v)?;
                        bindings.extend(sub);
                    }
                    Some(bindings)
                }
                _ => None,
            }
        }
    }
}

fn match_patterns_with_access(
    heap: &HeapAccess<'_>,
    patterns: &[Pattern],
    values: &[Pointer],
) -> Option<BTreeMap<Symbol, Pointer>> {
    let mut bindings = BTreeMap::new();
    for (p, v) in patterns.iter().zip(values.iter()) {
        let sub = match_pattern_ptr_with_access(heap, p, v)?;
        bindings.extend(sub);
    }
    Some(bindings)
}

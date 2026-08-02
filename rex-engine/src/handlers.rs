use crate::{
    builder::{
        core::NativeRegistration,
        export::{ExportTarget, HostFnAsync, HostFnSync, NativeFuture},
        registry::NativeId,
    },
    error::EngineError,
    evaluator::{
        context::{Context, InternalCtx},
        native_callable::{NativeCallScheduling, NativeHandleCallable},
        runtime_core::RuntimeCore,
    },
    memory::{
        handle_promotion::HandlePromoter,
        heap::{Handle, Heap, RootScope, RootedPtr},
        traits::{FromRex, IntoRex},
    },
    modules::ROOT_MODULE_NAME,
    util::{normalize_name, split_fun, type_expr_from_type, validate_native_export_scheme},
};
use futures::{FutureExt, future::BoxFuture};
use rex_ast::Span;
use rex_ast::{DeclareFnDecl, NameRef, Symbol, TypeConstraint, Var};
use rex_typesystem::types::{RexType, Scheme, Type};
use std::{future::Future, sync::Arc};

pub trait RexDefault<State>
where
    State: Clone + Send + Sync + 'static,
{
    fn rex_default(ctx: Context<State>) -> Result<Handle, EngineError>;
}

pub(crate) type NativeHandleFuture = BoxFuture<'static, Result<Handle, EngineError>>;

pub type SyncNativeCallable<State> = Arc<
    dyn for<'a> Fn(Context<State>, &'a Type, &'a [Handle]) -> Result<Handle, EngineError>
        + Send
        + Sync
        + 'static,
>;
pub type AsyncNativeCallable<State> =
    Arc<dyn Fn(Context<State>, Type, Vec<Handle>) -> NativeFuture + Send + Sync + 'static>;

#[derive(Debug, Clone, Copy)]
struct NativeCallableSig;

#[derive(Debug, Clone, Copy)]
struct AsyncNativeCallableSig;

pub(crate) fn declare_fn_decl_from_scheme(export_name: &str, scheme: &Scheme) -> DeclareFnDecl {
    let (params, ret) = decompose_fun_type(&scheme.typ);
    DeclareFnDecl {
        span: Span::default(),
        is_pub: true,
        name: Var {
            span: Span::default(),
            name: Symbol::intern(export_name),
        },
        type_params: scheme
            .vars
            .iter()
            .map(|var| {
                var.name
                    .clone()
                    .unwrap_or_else(|| Symbol::intern(&format!("t{}", var.id)))
            })
            .collect(),
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

fn decompose_fun_type(typ: &Type) -> (Vec<Type>, Type) {
    let mut params = Vec::new();
    let mut cur = typ.clone();
    while let Some((arg, ret)) = split_fun(&cur) {
        params.push(arg);
        cur = ret;
    }
    (params, cur)
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
                engine: &mut dyn ExportTarget<State>,
                export_name: &str,
            ) -> Result<(), EngineError> {
                let name_sym = normalize_name(export_name);
                let func: NativeHandleCallable<State> = Arc::new(
                    move |engine, _: Type, args: Vec<Handle>| {
                        let result = (|| {
                            if args.len() != $arity {
                                return Err(EngineError::NativeArity {
                                    name: name_sym.clone(),
                                    expected: $arity,
                                    got: args.len(),
                                });
                            }
                            let value = self(engine.state())?;
                            value.into_rex(engine.heap())
                        })();
                        async move { result }.boxed()
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
                engine: &mut dyn ExportTarget<State>,
                export_name: &str,
            ) -> Result<(), EngineError> {
                let name_sym = normalize_name(export_name);
                let func: NativeHandleCallable<State> = Arc::new(
                    move |engine, _: Type, args: Vec<Handle>| {
                        let result = (|| {
                            if args.len() != $arity {
                                return Err(EngineError::NativeArity {
                                    name: name_sym.clone(),
                                    expected: $arity,
                                    got: args.len(),
                                });
                            }
                            $(let $arg_name = {
                                let handle = &args[$idx];
                                $arg_ty::from_rex(&handle)?
                            };)*
                            let value = self(engine.state(), $($arg_name),+)?;
                            value.into_rex(engine.heap())
                        })();
                        async move { result }.boxed()
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

    fn inject(
        self,
        engine: &mut dyn ExportTarget<State>,
        export_name: &str,
    ) -> Result<(), EngineError> {
        let (scheme, arity, func) = self;
        validate_native_export_scheme(&scheme, arity)?;
        let callable: NativeHandleCallable<State> = Arc::new(move |engine, typ, args| {
            let result = func(engine, &typ, &args);
            async move { result }.boxed()
        });
        let registration = NativeRegistration::sync(scheme, arity, callable);
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
                engine: &mut dyn ExportTarget<State>,
                export_name: &str,
            ) -> Result<(), EngineError> {
                let f = Arc::new(self);
                let name_sym = normalize_name(export_name);
                let func: NativeHandleCallable<State> = Arc::new(
                    move |engine, _: Type, args: Vec<Handle>| -> NativeHandleFuture {
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
                engine: &mut dyn ExportTarget<State>,
                export_name: &str,
            ) -> Result<(), EngineError> {
                let f = Arc::new(self);
                let name_sym = normalize_name(export_name);
                let func: NativeHandleCallable<State> = Arc::new(
                    move |engine, _: Type, args: Vec<Handle>| -> NativeHandleFuture {
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
                            $(let $arg_name = {
                                let handle = &args[$idx];
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
        engine: &mut dyn ExportTarget<State>,
        export_name: &str,
    ) -> Result<(), EngineError> {
        let (scheme, arity, func) = self;
        validate_native_export_scheme(&scheme, arity)?;
        let callable: NativeHandleCallable<State> = func;
        let registration = NativeRegistration::r#async(scheme, arity, callable);
        engine.register_native_registration(ROOT_MODULE_NAME, export_name, registration)
    }
}

// A short-lived request produced inside evaluator code. Its scope-rooted
// arguments cannot cross the synchronous cycle boundary; `promote` is the
// only conversion into the scheduler-owned form below.
pub(crate) struct NativeCallRequest<'scope> {
    native_id: NativeId,
    scheduling: NativeCallScheduling,
    typ: Type,
    args: Vec<RootedPtr<'scope>>,
}

impl<'scope> NativeCallRequest<'scope> {
    pub(crate) fn new(
        native_id: NativeId,
        scheduling: NativeCallScheduling,
        typ: Type,
        args: Vec<RootedPtr<'scope>>,
    ) -> Self {
        Self {
            native_id,
            scheduling,
            typ,
            args,
        }
    }

    pub(crate) fn promote<'heap>(
        self,
        scope: &mut RootScope<'heap, 'scope>,
        promoter: &HandlePromoter<'_>,
    ) -> Result<NativeCall, EngineError> {
        let Self {
            native_id,
            scheduling,
            typ,
            args,
        } = self;
        let args = promoter.promote_all(scope, &args)?;
        Ok(NativeCall {
            native_id,
            scheduling,
            typ,
            args,
        })
    }
}

// A host call that is safe to queue or suspend because every argument is a
// registered `Handle` root. This is deliberately distinct from evaluator-owned
// `PersistentPtr` state.
pub(crate) struct NativeCall {
    native_id: NativeId,
    scheduling: NativeCallScheduling,
    typ: Type,
    args: Vec<Handle>,
}

impl NativeCall {
    pub(crate) fn scheduling(&self) -> NativeCallScheduling {
        self.scheduling
    }

    pub(crate) fn invoke<State>(
        self,
        runtime: &RuntimeCore<State>,
        heap: &Heap,
    ) -> Result<NativeHandleFuture, EngineError>
    where
        State: Clone + Send + Sync + 'static,
    {
        let callable = match runtime.native_callable(self.native_id)? {
            crate::evaluator::native_callable::NativeCallable::Host { callable, .. } => callable,
            crate::evaluator::native_callable::NativeCallable::Scheduler(_) => {
                return Err(EngineError::Internal(
                    "scheduler native queued through host native ABI".into(),
                ));
            }
        };
        let ctx = InternalCtx::new(runtime);
        let wrapped = Context::new(ctx, heap.clone());
        let future = (callable)(wrapped, self.typ, self.args);
        let result_heap = heap.clone();
        let future = async move {
            let value = future.await?;
            value.ensure_heap(&result_heap)?;
            Ok(value)
        }
        .boxed();
        Ok(match self.scheduling {
            NativeCallScheduling::Immediate => future,
            NativeCallScheduling::Deferred => runtime.async_call_policy.prepare(future),
        })
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

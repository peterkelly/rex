use crate::{
    Value,
    builder::{
        core::NativeRegistration,
        export::{ExportTarget, HostFnAsync, HostFnSync, NativeFuture},
        registry::NativeId,
    },
    error::EngineError,
    evaluator::{
        context::Context,
        native_callable::{HostValueCallable, NativeCallScheduling},
        runtime_core::RuntimeCore,
    },
    memory::{
        heap::{RootScope, RootedPtr},
        traits::{FromRex, IntoRex},
    },
    modules::ROOT_MODULE_NAME,
    util::{normalize_name, split_fun, type_expr_from_type, validate_host_value_export_scheme},
};
use futures::{FutureExt, future::BoxFuture};
use rex_ast::Span;
use rex_ast::{DeclareFnDecl, NameRef, Symbol, TypeConstraint, Var};
use rex_typesystem::{
    types::{RexType, Scheme, Type, Types},
    unification::unify,
};
use std::{future::Future, sync::Arc};

pub trait RexDefault<State>
where
    State: Clone + Send + Sync + 'static,
{
    fn rex_default(ctx: Context<State>) -> Result<Value, EngineError>;
}

pub(crate) type NativeValueFuture = NativeFuture;

pub type SyncNativeCallable<State> = Arc<
    dyn for<'a> Fn(Context<State>, &'a Type, Vec<Value>) -> Result<Value, EngineError>
        + Send
        + Sync
        + 'static,
>;
pub type AsyncNativeCallable<State> =
    Arc<dyn Fn(Context<State>, Type, Vec<Value>) -> NativeFuture + Send + Sync + 'static>;

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
                let func: HostValueCallable<State> = Arc::new(
                    move |engine, _: Type, args: Vec<Value>| {
                        let result = (|| {
                            if args.len() != $arity {
                                return Err(EngineError::NativeArity {
                                    name: name_sym.clone(),
                                    expected: $arity,
                                    got: args.len(),
                                });
                            }
                            let value = self(engine.state())?;
                            value.into_rex()
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
                let func: HostValueCallable<State> = Arc::new(
                    move |engine, _: Type, args: Vec<Value>| {
                        let result = (|| {
                            if args.len() != $arity {
                                return Err(EngineError::NativeArity {
                                    name: name_sym.clone(),
                                    expected: $arity,
                                    got: args.len(),
                                });
                            }
                            let mut args = args.into_iter();
                            $(let $arg_name = $arg_ty::from_rex(args.next().ok_or_else(|| EngineError::NativeArity {
                                name: name_sym.clone(),
                                expected: $arity,
                                got: $idx,
                            })?)?;)*
                            let value = self(engine.state(), $($arg_name),+)?;
                            value.into_rex()
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
        validate_host_value_export_scheme(&scheme, arity)?;
        let callable: HostValueCallable<State> = Arc::new(move |engine, typ, args| {
            let result = func(engine, &typ, args);
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
                let func: HostValueCallable<State> = Arc::new(
                    move |engine, _: Type, args: Vec<Value>| -> NativeValueFuture {
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
                            value.into_rex()
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
                let func: HostValueCallable<State> = Arc::new(
                    move |engine, _: Type, args: Vec<Value>| -> NativeValueFuture {
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
                            let mut args = args.into_iter();
                            $(let $arg_name = $arg_ty::from_rex(args.next().ok_or_else(|| EngineError::NativeArity {
                                name: name_sym.clone(),
                                expected: $arity,
                                got: $idx,
                            })?)?;)*
                            Ok(($($arg_name,)+))
                        })();
                        match args {
                            Ok(($($arg_name,)+)) => {
                                let future = f(engine.state(), $($arg_name),+);
                                async move {
                                    let value = future.await?;
                                    value.into_rex()
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
        validate_host_value_export_scheme(&scheme, arity)?;
        let callable: HostValueCallable<State> = func;
        let registration = NativeRegistration::r#async(scheme, arity, callable);
        engine.register_native_registration(ROOT_MODULE_NAME, export_name, registration)
    }
}

// A short-lived request produced inside evaluator code. It is converted to
// owned host values before it leaves the active runtime scope.
pub(crate) struct NativeCallRequest {
    native_id: NativeId,
    scheduling: NativeCallScheduling,
    typ: Type,
    args: Vec<RootedPtr>,
}

impl NativeCallRequest {
    pub(crate) fn new(
        native_id: NativeId,
        scheduling: NativeCallScheduling,
        typ: Type,
        args: Vec<RootedPtr>,
    ) -> Self {
        Self {
            native_id,
            scheduling,
            typ,
            args,
        }
    }

    pub(crate) fn prepare<State>(
        self,
        scope: &mut RootScope<'_>,
        runtime: &RuntimeCore<State>,
    ) -> Result<NativeCall, EngineError>
    where
        State: Clone + Send + Sync + 'static,
    {
        let Self {
            native_id,
            scheduling,
            typ,
            args,
        } = self;
        let typ = resolve_call_type(scope, &typ, &args)?;
        let (argument_types, result_type) = decompose_call_type(&typ, args.len())?;
        let args = args
            .into_iter()
            .zip(&argument_types)
            .map(|(root, expected)| {
                scope.export_value(root, expected, runtime.type_system.as_ref())
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(NativeCall {
            native_id,
            scheduling,
            typ,
            args,
            result_type,
        })
    }
}

// A host call that is safe to queue or suspend because all arguments are owned
// values and no runtime or heap capability escapes with it.
pub(crate) struct NativeCall {
    native_id: NativeId,
    scheduling: NativeCallScheduling,
    typ: Type,
    args: Vec<Value>,
    result_type: Type,
}

pub(crate) struct NativeCompletion {
    pub(crate) value: Value,
    pub(crate) expected: Type,
}

pub(crate) type NativeCompletionFuture = BoxFuture<'static, Result<NativeCompletion, EngineError>>;

impl NativeCall {
    pub(crate) fn scheduling(&self) -> NativeCallScheduling {
        self.scheduling
    }

    pub(crate) fn invoke<State>(
        self,
        runtime: &RuntimeCore<State>,
    ) -> Result<NativeCompletionFuture, EngineError>
    where
        State: Clone + Send + Sync + 'static,
    {
        match runtime.native_callable(self.native_id)? {
            crate::evaluator::native_callable::NativeCallable::Host { callable, .. } => {
                let ctx = Context::new(runtime);
                let future = (callable)(ctx, self.typ, self.args);
                let future = match self.scheduling {
                    NativeCallScheduling::Immediate => future,
                    NativeCallScheduling::Deferred => runtime.async_call_policy.prepare(future),
                };
                let expected = self.result_type;
                Ok(async move {
                    Ok(NativeCompletion {
                        value: future.await?,
                        expected,
                    })
                }
                .boxed())
            }
            crate::evaluator::native_callable::NativeCallable::Constant(_) => Err(
                EngineError::Internal("constant queued through host native ABI".into()),
            ),
            crate::evaluator::native_callable::NativeCallable::Scheduler(_) => Err(
                EngineError::Internal("scheduler native queued through host native ABI".into()),
            ),
        }
    }
}

fn resolve_call_type(
    scope: &mut RootScope<'_>,
    typ: &Type,
    args: &[RootedPtr],
) -> Result<Type, EngineError> {
    let mut resolved = typ.clone();
    for (index, arg) in args.iter().enumerate() {
        let (argument_types, _) = decompose_call_type(&resolved, args.len())?;
        let expected = &argument_types[index];
        if expected.ftv().is_empty() {
            continue;
        }
        let actual = scope.infer_type(*arg)?;
        let subst = unify(expected, &actual).map_err(|_| EngineError::NativeType {
            expected: expected.to_string(),
            got: actual.to_string(),
        })?;
        resolved = resolved.apply(&subst);
    }
    Ok(resolved)
}

fn decompose_call_type(typ: &Type, arity: usize) -> Result<(Vec<Type>, Type), EngineError> {
    let mut arguments = Vec::with_capacity(arity);
    let mut result = typ.clone();
    for _ in 0..arity {
        let (argument, rest) =
            split_fun(&result).ok_or_else(|| EngineError::NotCallable(result.to_string()))?;
        arguments.push(argument);
        result = rest;
    }
    Ok((arguments, result))
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

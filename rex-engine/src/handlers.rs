use crate::{
    builder::{
        engine::{Engine, NativeRegistration},
        export::{HostFnAsync, HostFnSync, NativeFuture},
    },
    error::EngineError,
    evaluator::{
        CallSite,
        context::Context,
        native_callable::{AsyncNativePointerCallable, SyncNativePointerCallable},
        runtime_core::RuntimeCore,
    },
    modules::ROOT_MODULE_NAME,
    util::{normalize_name, split_fun, type_expr_from_type, validate_native_export_scheme},
    value::{FromRex, Handle, IntoRex, Pointer, TempRoots},
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
        type_params: Vec::new(),
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

pub(crate) struct NativeAsyncCall<State: Clone + Send + Sync + 'static> {
    pub(crate) callable: AsyncNativePointerCallable<State>,
    pub(crate) call_site: CallSite,
    pub(crate) typ: Type,
    pub(crate) args: Vec<Pointer>,
}

impl<State> NativeAsyncCall<State>
where
    State: Clone + Send + Sync + 'static,
{
    pub(crate) fn new(
        callable: AsyncNativePointerCallable<State>,
        call_site: CallSite,
        typ: Type,
        args: Vec<Pointer>,
    ) -> Self {
        Self {
            callable,
            call_site,
            typ,
            args,
        }
    }

    pub(crate) fn invoke(
        mut self,
        runtime: &RuntimeCore<State>,
    ) -> Result<NativeHandleFuture, EngineError> {
        let mut protected = Vec::new();
        self.trace_pointers(&mut protected);
        let roots = runtime.heap.temp_roots(protected)?;
        let mut cursor = 0;
        self.refresh_from_roots(&roots, &mut cursor)?;
        let args = self.args;
        let ctx = Context::new_at_call_site(runtime, self.call_site);
        let future = (self.callable)(ctx, self.typ, args);
        Ok(runtime.async_call_policy.prepare(future))
    }

    pub(crate) fn trace_pointers(&self, out: &mut Vec<Pointer>) {
        if let Some(parent) = self.call_site.parent {
            out.push(parent);
        }
        out.extend(self.args.iter().copied());
    }

    pub(crate) fn refresh_from_roots(
        &mut self,
        roots: &TempRoots,
        cursor: &mut usize,
    ) -> Result<(), EngineError> {
        if self.call_site.parent.is_some() {
            self.call_site.parent = Some(roots.get(*cursor)?);
            *cursor += 1;
        }
        for arg in &mut self.args {
            *arg = roots.get(*cursor)?;
            *cursor += 1;
        }
        Ok(())
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

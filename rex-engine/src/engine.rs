//! Core engine implementation for Rex.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::sync::{Arc, Mutex};

use async_recursion::async_recursion;
use futures::{FutureExt, future::BoxFuture, pin_mut};
use rex_ast::expr::{
    ClassDecl, Decl, Expr, FnDecl, InstanceDecl, Pattern, Scope, Symbol, TypeDecl, sym, sym_eq,
};
use rex_ts::{
    AdtDecl, Instance, Predicate, PreparedInstanceDecl, Scheme, Subst, Type, TypeError, TypeKind,
    TypeSystem, TypeSystemLimits, TypeVarSupply, TypedExpr, TypedExprKind, Types, compose_subst,
    entails, instantiate, unify,
};
use rex_util::GasMeter;

use crate::modules::{ModuleId, ModuleSystem, ResolveRequest, ResolvedModule, virtual_export_name};
use crate::prelude::{
    inject_boolean_ops, inject_equality_ops, inject_json_primops, inject_list_builtins,
    inject_numeric_ops, inject_option_result_builtins, inject_order_ops, inject_prelude_adts,
    inject_pretty_ops,
};
use crate::value::{Closure, Heap, Pointer, Value, list_to_vec};
use crate::{CancellationToken, EngineError, Env, FromPointer, IntoPointer, RexType};

fn check_cancelled<State: Clone + Send + Sync + 'static>(
    engine: &Engine<State>,
) -> Result<(), EngineError> {
    if engine.cancel.is_cancelled() {
        Err(EngineError::Cancelled)
    } else {
        Ok(())
    }
}

fn type_head_is_var(typ: &Type) -> bool {
    let mut cur = typ;
    while let TypeKind::App(head, _) = cur.as_ref() {
        cur = head;
    }
    matches!(cur.as_ref(), TypeKind::Var(..))
}

type NativeFuture<'a> = BoxFuture<'a, Result<Pointer, EngineError>>;
type NativeId = u64;
type SyncNativeCallable<State> = Arc<
    dyn Fn(&Engine<State>, &Type, &[Pointer]) -> Result<Pointer, EngineError>
        + Send
        + Sync
        + 'static,
>;
type AsyncNativeCallable<State> = Arc<
    dyn for<'a> Fn(&'a Engine<State>, Type, &'a [Pointer]) -> NativeFuture<'a>
        + Send
        + Sync
        + 'static,
>;
type AsyncNativeCallableCancellable<State> = Arc<
    dyn for<'a> Fn(&'a Engine<State>, CancellationToken, Type, &'a [Pointer]) -> NativeFuture<'a>
        + Send
        + Sync
        + 'static,
>;

type ExportInjector<State> =
    Box<dyn FnOnce(&mut Engine<State>, &str) -> Result<(), EngineError> + Send + 'static>;

struct NativeRegistration<State: Clone + Send + Sync + 'static> {
    scheme: Scheme,
    arity: usize,
    callable: NativeCallable<State>,
    gas_cost: u64,
}

impl<State: Clone + Send + Sync + 'static> NativeRegistration<State> {
    fn sync(scheme: Scheme, arity: usize, func: SyncNativeCallable<State>, gas_cost: u64) -> Self {
        Self {
            scheme,
            arity,
            callable: NativeCallable::Sync(func),
            gas_cost,
        }
    }

    fn r#async(
        scheme: Scheme,
        arity: usize,
        func: AsyncNativeCallable<State>,
        gas_cost: u64,
    ) -> Self {
        Self {
            scheme,
            arity,
            callable: NativeCallable::Async(func),
            gas_cost,
        }
    }

    fn async_cancellable(
        scheme: Scheme,
        arity: usize,
        func: AsyncNativeCallableCancellable<State>,
        gas_cost: u64,
    ) -> Self {
        Self {
            scheme,
            arity,
            callable: NativeCallable::AsyncCancellable(func),
            gas_cost,
        }
    }
}

pub trait Handler<State: Clone + Send + Sync + 'static, Sig>: Send + Sync + 'static {
    fn declaration(export_name: &str) -> String;
    fn declaration_for(&self, export_name: &str) -> String {
        Self::declaration(export_name)
    }
    fn inject(self, engine: &mut Engine<State>, export_name: &str) -> Result<(), EngineError>;
}

pub trait AsyncHandler<State: Clone + Send + Sync + 'static, Sig>: Send + Sync + 'static {
    fn declaration(export_name: &str) -> String;
    fn declaration_for(&self, export_name: &str) -> String {
        Self::declaration(export_name)
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
    declaration: String,
    injector: ExportInjector<State>,
}

impl<State> Export<State>
where
    State: Clone + Send + Sync + 'static,
{
    fn from_injector(
        name: impl Into<String>,
        declaration: String,
        injector: ExportInjector<State>,
    ) -> Result<Self, EngineError> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(EngineError::Internal("export name cannot be empty".into()));
        }
        let normalized = normalize_name(&name).to_string();
        Ok(Self {
            name: normalized,
            declaration,
            injector,
        })
    }

    pub fn from_handler<Sig, H>(name: impl Into<String>, handler: H) -> Result<Self, EngineError>
    where
        H: Handler<State, Sig>,
    {
        let name = name.into();
        let normalized = normalize_name(&name).to_string();
        let declaration = handler.declaration_for(&normalized);
        let injector: ExportInjector<State> =
            Box::new(move |engine, qualified_name| handler.inject(engine, qualified_name));
        Self::from_injector(name, declaration, injector)
    }

    pub fn from_async_handler<Sig, H>(
        name: impl Into<String>,
        handler: H,
    ) -> Result<Self, EngineError>
    where
        H: AsyncHandler<State, Sig>,
    {
        let name = name.into();
        let normalized = normalize_name(&name).to_string();
        let declaration = handler.declaration_for(&normalized);
        let injector: ExportInjector<State> =
            Box::new(move |engine, qualified_name| handler.inject_async(engine, qualified_name));
        Self::from_injector(name, declaration, injector)
    }

    pub fn from_native<F>(
        name: impl Into<String>,
        scheme: Scheme,
        arity: usize,
        handler: F,
    ) -> Result<Self, EngineError>
    where
        F: for<'a> Fn(&'a Engine<State>, &'a Type, &'a [Pointer]) -> Result<Pointer, EngineError>
            + Send
            + Sync
            + 'static,
    {
        validate_native_export_scheme(&scheme, arity)?;
        let handler = Arc::new(handler);
        let func: SyncNativeCallable<State> = Arc::new(
            move |engine: &Engine<State>, typ: &Type, args: &[Pointer]| handler(engine, typ, args),
        );
        Self::from_handler::<NativeCallableSig, _>(name, (scheme, arity, func))
    }

    pub fn from_native_async<F>(
        name: impl Into<String>,
        scheme: Scheme,
        arity: usize,
        handler: F,
    ) -> Result<Self, EngineError>
    where
        F: for<'a> Fn(&'a Engine<State>, Type, Vec<Pointer>) -> NativeFuture<'a>
            + Send
            + Sync
            + 'static,
    {
        validate_native_export_scheme(&scheme, arity)?;
        let handler = Arc::new(handler);
        let func: AsyncNativeCallable<State> = Arc::new(move |engine, typ, args| {
            let args = args.to_vec();
            let handler = Arc::clone(&handler);
            handler(engine, typ, args)
        });
        Self::from_async_handler::<AsyncNativeCallableSig, _>(name, (scheme, arity, func))
    }
}

pub struct Module<State: Clone + Send + Sync + 'static> {
    pub name: String,
    exports: Vec<Export<State>>,
}

impl<State> Module<State>
where
    State: Clone + Send + Sync + 'static,
{
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            exports: Vec::new(),
        }
    }

    pub fn add_export(&mut self, export: Export<State>) {
        self.exports.push(export);
    }

    pub fn export<Sig, H>(&mut self, name: impl Into<String>, handler: H) -> Result<(), EngineError>
    where
        H: Handler<State, Sig>,
    {
        self.exports.push(Export::from_handler(name, handler)?);
        Ok(())
    }

    pub fn export_async<Sig, H>(
        &mut self,
        name: impl Into<String>,
        handler: H,
    ) -> Result<(), EngineError>
    where
        H: AsyncHandler<State, Sig>,
    {
        self.exports
            .push(Export::from_async_handler(name, handler)?);
        Ok(())
    }

    pub fn export_native<F>(
        &mut self,
        name: impl Into<String>,
        scheme: Scheme,
        arity: usize,
        handler: F,
    ) -> Result<(), EngineError>
    where
        F: for<'a> Fn(&'a Engine<State>, &'a Type, &'a [Pointer]) -> Result<Pointer, EngineError>
            + Send
            + Sync
            + 'static,
    {
        self.exports
            .push(Export::from_native(name, scheme, arity, handler)?);
        Ok(())
    }

    pub fn export_native_async<F>(
        &mut self,
        name: impl Into<String>,
        scheme: Scheme,
        arity: usize,
        handler: F,
    ) -> Result<(), EngineError>
    where
        F: for<'a> Fn(&'a Engine<State>, Type, Vec<Pointer>) -> NativeFuture<'a>
            + Send
            + Sync
            + 'static,
    {
        self.exports
            .push(Export::from_native_async(name, scheme, arity, handler)?);
        Ok(())
    }
}

fn declaration_type_string(arg_types: &[Type], ret: Type) -> String {
    if arg_types.is_empty() {
        return ret.to_string();
    }
    let mut out = ret.to_string();
    for arg in arg_types.iter().rev() {
        out = format!("{arg} -> {out}");
    }
    out
}

fn declaration_line(export_name: &str, arg_types: &[Type], ret: Type) -> String {
    format!(
        "pub declare fn {export_name} {}",
        declaration_type_string(arg_types, ret)
    )
}

fn declaration_line_from_scheme(export_name: &str, scheme: &Scheme) -> String {
    let mut out = format!("pub declare fn {export_name} : {}", scheme.typ);
    if !scheme.preds.is_empty() {
        let preds = scheme
            .preds
            .iter()
            .map(|p| format!("{} {}", p.class, p.typ))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(" where ");
        out.push_str(&preds);
    }
    out
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
        impl<State, F, R> Handler<State, $sig> for F
        where
            State: Clone + Send + Sync + 'static,
            F: for<'a> Fn(&'a State) -> Result<R, EngineError> + Send + Sync + 'static,
            R: IntoPointer + RexType,
        {
            fn declaration(export_name: &str) -> String {
                declaration_line(export_name, &[], R::rex_type())
            }

            fn inject(
                self,
                engine: &mut Engine<State>,
                export_name: &str,
            ) -> Result<(), EngineError> {
                let name_sym = normalize_name(export_name);
                let func: SyncNativeCallable<State> = Arc::new(
                    move |engine: &Engine<State>, _: &Type, args: &[Pointer]| {
                        if args.len() != $arity {
                            return Err(EngineError::NativeArity {
                                name: name_sym.clone(),
                                expected: $arity,
                                got: args.len(),
                            });
                        }
                        let value = self(engine.state.as_ref())?;
                        value.into_pointer(engine.heap())
                    },
                );
                let scheme = Scheme::new(vec![], vec![], R::rex_type());
                let registration = NativeRegistration::sync(scheme, $arity, func, 0);
                engine.register_native_registration(export_name, registration)
            }
        }
    };
    ([ $(($arg_ty:ident, $arg_name:ident, $idx:tt)),+ ] ; $arity:literal ; $sig:ty) => {
        impl<State, F, R, $($arg_ty),+> Handler<State, $sig> for F
        where
            State: Clone + Send + Sync + 'static,
            F: for<'a> Fn(&'a State, $($arg_ty),+) -> Result<R, EngineError> + Send + Sync + 'static,
            R: IntoPointer + RexType,
            $($arg_ty: FromPointer + RexType),+
        {
            fn declaration(export_name: &str) -> String {
                let args = vec![$($arg_ty::rex_type()),+];
                declaration_line(export_name, &args, R::rex_type())
            }

            fn inject(
                self,
                engine: &mut Engine<State>,
                export_name: &str,
            ) -> Result<(), EngineError> {
                let name_sym = normalize_name(export_name);
                let func: SyncNativeCallable<State> = Arc::new(
                    move |engine: &Engine<State>, _: &Type, args: &[Pointer]| {
                        if args.len() != $arity {
                            return Err(EngineError::NativeArity {
                                name: name_sym.clone(),
                                expected: $arity,
                                got: args.len(),
                            });
                        }
                        $(let $arg_name = $arg_ty::from_pointer(engine.heap(), &args[$idx])?;)*
                        let value = self(engine.state.as_ref(), $($arg_name),+)?;
                        value.into_pointer(engine.heap())
                    },
                );
                let typ = native_fn_type!($($arg_ty),+ ; R);
                let scheme = Scheme::new(vec![], vec![], typ);
                let registration = NativeRegistration::sync(scheme, $arity, func, 0);
                engine.register_native_registration(export_name, registration)
            }
        }
    };
}

impl<State> Handler<State, NativeCallableSig> for (Scheme, usize, SyncNativeCallable<State>)
where
    State: Clone + Send + Sync + 'static,
{
    fn declaration(_export_name: &str) -> String {
        unreachable!("native callable handlers use declaration_for")
    }

    fn declaration_for(&self, export_name: &str) -> String {
        let (scheme, _, _) = self;
        declaration_line_from_scheme(export_name, scheme)
    }

    fn inject(self, engine: &mut Engine<State>, export_name: &str) -> Result<(), EngineError> {
        let (scheme, arity, func) = self;
        validate_native_export_scheme(&scheme, arity)?;
        let registration = NativeRegistration::sync(scheme, arity, func, 0);
        engine.register_native_registration(export_name, registration)
    }
}

macro_rules! define_async_handler_impl {
    ([] ; $arity:literal ; $sig:ty) => {
        impl<State, F, Fut, R> AsyncHandler<State, $sig> for F
        where
            State: Clone + Send + Sync + 'static,
            F: for<'a> Fn(&'a State) -> Fut + Send + Sync + 'static,
            Fut: Future<Output = Result<R, EngineError>> + Send + 'static,
            R: IntoPointer + RexType,
        {
            fn declaration(export_name: &str) -> String {
                declaration_line(export_name, &[], R::rex_type())
            }

            fn inject_async(
                self,
                engine: &mut Engine<State>,
                export_name: &str,
            ) -> Result<(), EngineError> {
                let f = Arc::new(self);
                let name_sym = normalize_name(export_name);
                let func: AsyncNativeCallable<State> = Arc::new(
                    move |engine: &Engine<State>, _: Type, args: &[Pointer]| -> NativeFuture<'_> {
                        let f = Arc::clone(&f);
                        let name_sym = name_sym.clone();
                        async move {
                            if args.len() != $arity {
                                return Err(EngineError::NativeArity {
                                    name: name_sym.clone(),
                                    expected: $arity,
                                    got: args.len(),
                                });
                            }
                            let value = f(engine.state.as_ref()).await?;
                            value.into_pointer(engine.heap())
                        }
                        .boxed()
                    },
                );
                let scheme = Scheme::new(vec![], vec![], R::rex_type());
                let registration = NativeRegistration::r#async(scheme, $arity, func, 0);
                engine.register_native_registration(export_name, registration)
            }
        }
    };
    ([ $(($arg_ty:ident, $arg_name:ident, $idx:tt)),+ ] ; $arity:literal ; $sig:ty) => {
        impl<State, F, Fut, R, $($arg_ty),+> AsyncHandler<State, $sig> for F
        where
            State: Clone + Send + Sync + 'static,
            F: for<'a> Fn(&'a State, $($arg_ty),+) -> Fut + Send + Sync + 'static,
            Fut: Future<Output = Result<R, EngineError>> + Send + 'static,
            R: IntoPointer + RexType,
            $($arg_ty: FromPointer + RexType),+
        {
            fn declaration(export_name: &str) -> String {
                let args = vec![$($arg_ty::rex_type()),+];
                declaration_line(export_name, &args, R::rex_type())
            }

            fn inject_async(
                self,
                engine: &mut Engine<State>,
                export_name: &str,
            ) -> Result<(), EngineError> {
                let f = Arc::new(self);
                let name_sym = normalize_name(export_name);
                let func: AsyncNativeCallable<State> = Arc::new(
                    move |engine: &Engine<State>, _: Type, args: &[Pointer]| -> NativeFuture<'_> {
                        let f = Arc::clone(&f);
                        let name_sym = name_sym.clone();
                        async move {
                            if args.len() != $arity {
                                return Err(EngineError::NativeArity {
                                    name: name_sym.clone(),
                                    expected: $arity,
                                    got: args.len(),
                                });
                            }
                            $(let $arg_name = $arg_ty::from_pointer(engine.heap(), &args[$idx])?;)*
                            let value = f(engine.state.as_ref(), $($arg_name),+).await?;
                            value.into_pointer(engine.heap())
                        }
                        .boxed()
                    },
                );
                let typ = native_fn_type!($($arg_ty),+ ; R);
                let scheme = Scheme::new(vec![], vec![], typ);
                let registration = NativeRegistration::r#async(scheme, $arity, func, 0);
                engine.register_native_registration(export_name, registration)
            }
        }
    };
}

impl<State> AsyncHandler<State, AsyncNativeCallableSig>
    for (Scheme, usize, AsyncNativeCallable<State>)
where
    State: Clone + Send + Sync + 'static,
{
    fn declaration(_export_name: &str) -> String {
        unreachable!("native async callable handlers use declaration_for")
    }

    fn declaration_for(&self, export_name: &str) -> String {
        let (scheme, _, _) = self;
        declaration_line_from_scheme(export_name, scheme)
    }

    fn inject_async(
        self,
        engine: &mut Engine<State>,
        export_name: &str,
    ) -> Result<(), EngineError> {
        let (scheme, arity, func) = self;
        validate_native_export_scheme(&scheme, arity)?;
        let registration = NativeRegistration::r#async(scheme, arity, func, 0);
        engine.register_native_registration(export_name, registration)
    }
}

#[derive(Clone)]
pub(crate) enum NativeCallable<State: Clone + Send + Sync + 'static> {
    Sync(SyncNativeCallable<State>),
    Async(AsyncNativeCallable<State>),
    AsyncCancellable(AsyncNativeCallableCancellable<State>),
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
            NativeCallable::Async(_) => write!(f, "Async"),
            NativeCallable::AsyncCancellable(_) => write!(f, "AsyncCancellable"),
        }
    }
}

impl<State: Clone + Send + Sync + 'static> NativeCallable<State> {
    async fn call(
        &self,
        engine: &Engine<State>,
        typ: Type,
        args: &[Pointer],
    ) -> Result<Pointer, EngineError> {
        let token = engine.cancellation_token();
        if token.is_cancelled() {
            return Err(EngineError::Cancelled);
        }

        match self {
            NativeCallable::Sync(f) => (f)(engine, &typ, args),
            NativeCallable::Async(f) => {
                let call_fut = (f)(engine, typ, args).fuse();
                let cancel_fut = token.cancelled().fuse();
                pin_mut!(call_fut, cancel_fut);
                futures::select! {
                    _ = cancel_fut => Err(EngineError::Cancelled),
                    res = call_fut => {
                        if token.is_cancelled() {
                            Err(EngineError::Cancelled)
                        } else {
                            res
                        }
                    },
                }
            }
            NativeCallable::AsyncCancellable(f) => {
                let call_fut = (f)(engine, token.clone(), typ, args).fuse();
                let cancel_fut = token.cancelled().fuse();
                pin_mut!(call_fut, cancel_fut);
                futures::select! {
                    _ = cancel_fut => Err(EngineError::Cancelled),
                    res = call_fut => {
                        if token.is_cancelled() {
                            Err(EngineError::Cancelled)
                        } else {
                            res
                        }
                    },
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeFn {
    native_id: NativeId,
    name: Symbol,
    arity: usize,
    typ: Type,
    gas_cost: u64,
    applied: Vec<Pointer>,
    applied_types: Vec<Type>,
}

impl NativeFn {
    fn new(native_id: NativeId, name: Symbol, arity: usize, typ: Type, gas_cost: u64) -> Self {
        Self {
            native_id,
            name,
            arity,
            typ,
            gas_cost,
            applied: Vec::new(),
            applied_types: Vec::new(),
        }
    }

    pub(crate) fn from_parts(
        native_id: NativeId,
        name: Symbol,
        arity: usize,
        typ: Type,
        gas_cost: u64,
        applied: Vec<Pointer>,
        applied_types: Vec<Type>,
    ) -> Self {
        Self {
            native_id,
            name,
            arity,
            typ,
            gas_cost,
            applied,
            applied_types,
        }
    }

    pub(crate) fn name(&self) -> &Symbol {
        &self.name
    }

    async fn call_zero<State: Clone + Send + Sync + 'static>(
        &self,
        engine: &Engine<State>,
        gas: &mut GasMeter,
    ) -> Result<Pointer, EngineError> {
        let amount = gas
            .costs
            .native_call_base
            .saturating_add(self.gas_cost)
            .saturating_add(gas.costs.native_call_per_arg.saturating_mul(0));
        gas.charge(amount)?;
        if self.arity != 0 {
            return Err(EngineError::NativeArity {
                name: self.name.clone(),
                expected: self.arity,
                got: 0,
            });
        }
        engine
            .native_callable(self.native_id)?
            .call(engine, self.typ.clone(), &[])
            .await
    }

    async fn apply<State: Clone + Send + Sync + 'static>(
        mut self,
        engine: &Engine<State>,
        arg: Pointer,
        arg_type: Option<&Type>,
        gas: &mut GasMeter,
    ) -> Result<Pointer, EngineError> {
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
        let actual_ty = resolve_arg_type(engine.heap(), arg_type, &arg)?;
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
                gas_cost,
                applied,
                applied_types,
            } = self;
            return engine.heap().alloc_native(
                native_id,
                name,
                arity,
                typ,
                gas_cost,
                applied,
                applied_types,
            );
        }

        let mut full_ty = self.typ.clone();
        for arg_ty in self.applied_types.iter().rev() {
            full_ty = Type::fun(arg_ty.clone(), full_ty);
        }

        let amount = gas
            .costs
            .native_call_base
            .saturating_add(self.gas_cost)
            .saturating_add(
                gas.costs
                    .native_call_per_arg
                    .saturating_mul(self.applied.len() as u64),
            );
        gas.charge(amount)?;
        engine
            .native_callable(self.native_id)?
            .call(engine, full_ty, &self.applied)
            .await
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct OverloadedFn {
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

    pub(crate) fn into_parts(self) -> (Symbol, Type, Vec<Pointer>, Vec<Type>) {
        (self.name, self.typ, self.applied, self.applied_types)
    }

    async fn apply<State: Clone + Send + Sync + 'static>(
        mut self,
        engine: &Engine<State>,
        arg: Pointer,
        func_type: Option<&Type>,
        arg_type: Option<&Type>,
        gas: &mut GasMeter,
    ) -> Result<Pointer, EngineError> {
        if let Some(expected) = func_type {
            let subst = unify(&self.typ, expected).map_err(|_| EngineError::NativeType {
                expected: self.typ.to_string(),
                got: expected.to_string(),
            })?;
            self.typ = self.typ.apply(&subst);
        }
        let (arg_ty, rest_ty) =
            split_fun(&self.typ).ok_or_else(|| EngineError::NotCallable(self.typ.to_string()))?;
        let actual_ty = resolve_arg_type(engine.heap(), arg_type, &arg)?;
        let subst = unify(&arg_ty, &actual_ty).map_err(|_| EngineError::NativeType {
            expected: arg_ty.to_string(),
            got: actual_ty.to_string(),
        })?;
        let rest_ty = rest_ty.apply(&subst);
        self.applied.push(arg);
        self.applied_types.push(actual_ty);
        if is_function_type(&rest_ty) {
            return engine.heap().alloc_overloaded(
                self.name,
                rest_ty,
                self.applied,
                self.applied_types,
            );
        }
        let mut full_ty = rest_ty;
        for arg_ty in self.applied_types.iter().rev() {
            full_ty = Type::fun(arg_ty.clone(), full_ty);
        }
        if engine.types.class_methods.contains_key(&self.name) {
            let mut func = engine
                .resolve_class_method(&self.name, &full_ty, gas)
                .await?;
            let mut cur_ty = full_ty;
            for (applied, applied_ty) in self.applied.into_iter().zip(self.applied_types.iter()) {
                let (arg_ty, rest_ty) = split_fun(&cur_ty)
                    .ok_or_else(|| EngineError::NotCallable(cur_ty.to_string()))?;
                let subst = unify(&arg_ty, applied_ty).map_err(|_| EngineError::NativeType {
                    expected: arg_ty.to_string(),
                    got: applied_ty.to_string(),
                })?;
                let rest_ty = rest_ty.apply(&subst);
                func = apply(engine, func, applied, Some(&cur_ty), Some(applied_ty), gas).await?;
                cur_ty = rest_ty;
            }
            return Ok(func);
        }

        let imp = engine.resolve_native_impl(self.name.as_ref(), &full_ty)?;
        let amount = gas
            .costs
            .native_call_base
            .saturating_add(imp.gas_cost)
            .saturating_add(
                gas.costs
                    .native_call_per_arg
                    .saturating_mul(self.applied.len() as u64),
            );
        gas.charge(amount)?;
        imp.func.call(engine, full_ty, &self.applied).await
    }
}

#[derive(Clone)]
struct NativeImpl<State: Clone + Send + Sync + 'static> {
    id: NativeId,
    name: Symbol,
    arity: usize,
    scheme: Scheme,
    func: NativeCallable<State>,
    gas_cost: u64,
}

impl<State: Clone + Send + Sync + 'static> NativeImpl<State> {
    fn to_native_fn(&self, typ: Type) -> NativeFn {
        NativeFn::new(self.id, self.name.clone(), self.arity, typ, self.gas_cost)
    }
}

#[derive(Clone)]
struct NativeRegistry<State: Clone + Send + Sync + 'static> {
    next_id: NativeId,
    entries: HashMap<Symbol, Vec<NativeImpl<State>>>,
    by_id: HashMap<NativeId, NativeImpl<State>>,
}

impl<State: Clone + Send + Sync + 'static> NativeRegistry<State> {
    fn insert(
        &mut self,
        name: Symbol,
        arity: usize,
        scheme: Scheme,
        func: NativeCallable<State>,
        gas_cost: u64,
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
            gas_cost,
        };
        self.by_id.insert(id, imp.clone());
        entry.push(imp);
        Ok(())
    }

    fn get(&self, name: &Symbol) -> Option<&[NativeImpl<State>]> {
        self.entries.get(name).map(|v| v.as_slice())
    }

    fn has_name(&self, name: &Symbol) -> bool {
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
            entries: HashMap::new(),
            by_id: HashMap::new(),
        }
    }
}

#[derive(Clone)]
struct TypeclassInstance {
    head: Type,
    def_env: Env,
    methods: HashMap<Symbol, Arc<TypedExpr>>,
}

#[derive(Default, Clone)]
struct TypeclassRegistry {
    entries: HashMap<Symbol, Vec<TypeclassInstance>>,
}

impl TypeclassRegistry {
    fn insert(
        &mut self,
        class: Symbol,
        head: Type,
        def_env: Env,
        methods: HashMap<Symbol, Arc<TypedExpr>>,
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

    fn resolve(
        &self,
        class: &Symbol,
        method: &Symbol,
        param_type: &Type,
    ) -> Result<(Env, Arc<TypedExpr>, Subst), EngineError> {
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
}

pub struct Engine<State = ()>
where
    State: Clone + Send + Sync + 'static,
{
    pub state: Arc<State>,
    env: Env,
    natives: NativeRegistry<State>,
    typeclasses: TypeclassRegistry,
    types: TypeSystem,
    typeclass_cache: Arc<Mutex<HashMap<(Symbol, Type), Pointer>>>,
    pub(crate) modules: ModuleSystem,
    injected_modules: HashSet<String>,
    cancel: CancellationToken,
    heap: Heap,
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
    pub fn new(state: State) -> Self {
        Self {
            state: Arc::new(state),
            env: Env::new(),
            natives: NativeRegistry::<State>::default(),
            typeclasses: TypeclassRegistry::default(),
            types: TypeSystem::new(),
            typeclass_cache: Arc::new(Mutex::new(HashMap::new())),
            modules: ModuleSystem::default(),
            injected_modules: HashSet::new(),
            cancel: CancellationToken::new(),
            heap: Heap::new(),
        }
    }

    pub(crate) fn fresh_type_var(&mut self, name: Option<Symbol>) -> rex_ts::TypeVar {
        self.types.supply.fresh(name)
    }

    pub fn with_prelude(state: State) -> Result<Self, EngineError> {
        let types = TypeSystem::with_prelude()?;
        let mut engine = Engine {
            state: Arc::new(state),
            env: Env::new(),
            natives: NativeRegistry::<State>::default(),
            typeclasses: TypeclassRegistry::default(),
            types,
            typeclass_cache: Arc::new(Mutex::new(HashMap::new())),
            modules: ModuleSystem::default(),
            injected_modules: HashSet::new(),
            cancel: CancellationToken::new(),
            heap: Heap::new(),
        };
        engine.inject_prelude()?;
        Ok(engine)
    }

    pub fn set_type_system_limits(&mut self, limits: TypeSystemLimits) {
        self.types.set_limits(limits);
    }

    pub fn into_heap(self) -> Heap {
        self.heap
    }

    pub fn heap(&self) -> &Heap {
        &self.heap
    }

    pub fn type_system(&self) -> &TypeSystem {
        &self.types
    }

    /// Inject `debug`/`info`/`warn`/`error` logging functions backed by `tracing`.
    ///
    /// Each function has the Rex type `a -> str where Pretty a` and logs
    /// `pretty x` at the corresponding level, returning the rendered string.
    pub fn inject_tracing_log_functions(&mut self) -> Result<(), EngineError> {
        let string = Type::con("string", 0);

        let make_scheme = |engine: &mut Engine<State>| {
            let a_tv = engine.types.supply.fresh(Some("a".into()));
            let a = Type::var(a_tv.clone());
            Scheme::new(
                vec![a_tv],
                vec![Predicate::new("Pretty", a.clone())],
                Type::fun(a, string.clone()),
            )
        };

        let debug_scheme = make_scheme(self);
        self.inject_tracing_log_function_with_scheme("debug", debug_scheme, |s| {
            tracing::debug!("{s}")
        })?;
        let info_scheme = make_scheme(self);
        self.inject_tracing_log_function_with_scheme("info", info_scheme, |s| {
            tracing::info!("{s}")
        })?;
        let warn_scheme = make_scheme(self);
        self.inject_tracing_log_function_with_scheme("warn", warn_scheme, |s| {
            tracing::warn!("{s}")
        })?;
        let error_scheme = make_scheme(self);
        self.inject_tracing_log_function_with_scheme("error", error_scheme, |s| {
            tracing::error!("{s}")
        })?;
        Ok(())
    }

    pub fn inject_tracing_log_function(
        &mut self,
        name: &str,
        log: fn(&str),
    ) -> Result<(), EngineError> {
        let string = Type::con("string", 0);
        let a_tv = self.types.supply.fresh(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let scheme = Scheme::new(
            vec![a_tv],
            vec![Predicate::new("Pretty", a.clone())],
            Type::fun(a, string),
        );
        self.inject_tracing_log_function_with_scheme(name, scheme, log)
    }

    fn inject_tracing_log_function_with_scheme(
        &mut self,
        name: &str,
        scheme: Scheme,
        log: fn(&str),
    ) -> Result<(), EngineError> {
        let name_sym = sym(name);
        self.export_native_async(name, scheme, 1, move |engine, call_type, args| {
            let name_sym = name_sym.clone();
            async move {
                if args.len() != 1 {
                    return Err(EngineError::NativeArity {
                        name: name_sym.clone(),
                        expected: 1,
                        got: args.len(),
                    });
                }

                let (arg_ty, _ret_ty) = split_fun(&call_type)
                    .ok_or_else(|| EngineError::NotCallable(call_type.to_string()))?;
                let pretty_ty = Type::fun(arg_ty.clone(), Type::con("string", 0));
                let mut gas = GasMeter::default();
                let pretty_ptr = engine
                    .resolve_class_method(&sym("pretty"), &pretty_ty, &mut gas)
                    .await?;
                let rendered_ptr = apply(
                    engine,
                    pretty_ptr,
                    args[0].clone(),
                    Some(&pretty_ty),
                    Some(&arg_ty),
                    &mut gas,
                )
                .await?;
                let message = engine.heap().pointer_as_string(&rendered_ptr)?;

                log(&message);
                engine.heap().alloc_string(message)
            }
            .boxed()
        })
    }

    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    pub fn inject_module(&mut self, module: Module<State>) -> Result<(), EngineError> {
        let module_name = module.name.trim().to_string();
        if module_name.is_empty() {
            return Err(EngineError::Internal("module name cannot be empty".into()));
        }
        if self.injected_modules.contains(&module_name) {
            return Err(EngineError::Internal(format!(
                "module `{module_name}` already injected"
            )));
        }

        let mut source = String::new();
        for export in &module.exports {
            source.push_str(&export.declaration);
            source.push('\n');
        }

        for export in module.exports {
            let qualified = virtual_export_name(&module_name, &export.name);
            (export.injector)(self, &qualified)?;
        }

        let resolver_module_name = module_name.clone();
        let resolver_source = source.clone();
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
                    source: resolver_source.clone(),
                }))
            },
        );

        self.injected_modules.insert(module_name);
        Ok(())
    }

    fn inject_root_export(&mut self, export: Export<State>) -> Result<(), EngineError> {
        let Export {
            name,
            declaration: _,
            injector,
        } = export;
        injector(self, &name)
    }

    fn register_native_registration(
        &mut self,
        export_name: &str,
        registration: NativeRegistration<State>,
    ) -> Result<(), EngineError> {
        let NativeRegistration {
            scheme,
            arity,
            callable,
            gas_cost,
        } = registration;
        let name = normalize_name(export_name);
        self.register_native(name, scheme, arity, callable, gas_cost)
    }

    pub fn export<Sig, H>(&mut self, name: impl Into<String>, handler: H) -> Result<(), EngineError>
    where
        H: Handler<State, Sig>,
    {
        self.inject_root_export(Export::from_handler(name, handler)?)
    }

    pub fn export_async<Sig, H>(
        &mut self,
        name: impl Into<String>,
        handler: H,
    ) -> Result<(), EngineError>
    where
        H: AsyncHandler<State, Sig>,
    {
        self.inject_root_export(Export::from_async_handler(name, handler)?)
    }

    pub fn export_native<F>(
        &mut self,
        name: impl Into<String>,
        scheme: Scheme,
        arity: usize,
        handler: F,
    ) -> Result<(), EngineError>
    where
        F: for<'a> Fn(&'a Engine<State>, &'a Type, &'a [Pointer]) -> Result<Pointer, EngineError>
            + Send
            + Sync
            + 'static,
    {
        self.export_native_with_gas_cost(name, scheme, arity, 0, handler)
    }

    pub fn export_native_async<F>(
        &mut self,
        name: impl Into<String>,
        scheme: Scheme,
        arity: usize,
        handler: F,
    ) -> Result<(), EngineError>
    where
        F: for<'a> Fn(&'a Engine<State>, Type, Vec<Pointer>) -> NativeFuture<'a>
            + Send
            + Sync
            + 'static,
    {
        self.export_native_async_with_gas_cost(name, scheme, arity, 0, handler)
    }

    pub fn export_value<V: IntoPointer + RexType>(
        &mut self,
        name: &str,
        value: V,
    ) -> Result<(), EngineError> {
        let typ = V::rex_type();
        let value = value.into_pointer(self.heap())?;
        let func: SyncNativeCallable<State> =
            Arc::new(move |_engine: &Engine<State>, _: &Type, _args: &[Pointer]| Ok(value.clone()));
        let scheme = Scheme::new(vec![], vec![], typ);
        let registration = NativeRegistration::sync(scheme, 0, func, 0);
        self.register_native_registration(name, registration)
    }

    pub fn export_value_typed(
        &mut self,
        name: &str,
        typ: Type,
        value: Value,
    ) -> Result<(), EngineError> {
        let value = self.heap().alloc_value(value)?;
        let func: SyncNativeCallable<State> =
            Arc::new(move |_engine: &Engine<State>, _: &Type, _args: &[Pointer]| Ok(value.clone()));
        let scheme = Scheme::new(vec![], vec![], typ);
        let registration = NativeRegistration::sync(scheme, 0, func, 0);
        self.register_native_registration(name, registration)
    }

    pub fn export_native_with_gas_cost<F>(
        &mut self,
        name: impl Into<String>,
        scheme: Scheme,
        arity: usize,
        gas_cost: u64,
        handler: F,
    ) -> Result<(), EngineError>
    where
        F: for<'a> Fn(&'a Engine<State>, &'a Type, &'a [Pointer]) -> Result<Pointer, EngineError>
            + Send
            + Sync
            + 'static,
    {
        validate_native_export_scheme(&scheme, arity)?;
        let name = name.into();
        let handler = Arc::new(handler);
        let func: SyncNativeCallable<State> = Arc::new(
            move |engine: &Engine<State>, typ: &Type, args: &[Pointer]| handler(engine, typ, args),
        );
        let registration = NativeRegistration::sync(scheme, arity, func, gas_cost);
        self.register_native_registration(&name, registration)
    }

    pub fn export_native_async_with_gas_cost<F>(
        &mut self,
        name: impl Into<String>,
        scheme: Scheme,
        arity: usize,
        gas_cost: u64,
        handler: F,
    ) -> Result<(), EngineError>
    where
        F: for<'a> Fn(&'a Engine<State>, Type, Vec<Pointer>) -> NativeFuture<'a>
            + Send
            + Sync
            + 'static,
    {
        validate_native_export_scheme(&scheme, arity)?;
        let name = name.into();
        let handler = Arc::new(handler);
        let func: AsyncNativeCallable<State> = Arc::new(move |engine, typ, args| {
            let handler = Arc::clone(&handler);
            handler(engine, typ, args.to_vec())
        });
        let registration = NativeRegistration::r#async(scheme, arity, func, gas_cost);
        self.register_native_registration(&name, registration)
    }

    pub fn export_native_async_cancellable<F>(
        &mut self,
        name: impl Into<String>,
        scheme: Scheme,
        arity: usize,
        handler: F,
    ) -> Result<(), EngineError>
    where
        F: for<'a> Fn(
                &'a Engine<State>,
                CancellationToken,
                Type,
                &'a [Pointer],
            ) -> NativeFuture<'a>
            + Send
            + Sync
            + 'static,
    {
        self.export_native_async_cancellable_with_gas_cost(name, scheme, arity, 0, handler)
    }

    pub fn export_native_async_cancellable_with_gas_cost<F>(
        &mut self,
        name: impl Into<String>,
        scheme: Scheme,
        arity: usize,
        gas_cost: u64,
        handler: F,
    ) -> Result<(), EngineError>
    where
        F: for<'a> Fn(
                &'a Engine<State>,
                CancellationToken,
                Type,
                &'a [Pointer],
            ) -> NativeFuture<'a>
            + Send
            + Sync
            + 'static,
    {
        validate_native_export_scheme(&scheme, arity)?;
        let name = name.into();
        let handler = Arc::new(handler);
        let func: AsyncNativeCallableCancellable<State> =
            Arc::new(move |engine, token, typ, args| handler(engine, token, typ, args));
        let registration = NativeRegistration::async_cancellable(scheme, arity, func, gas_cost);
        self.register_native_registration(&name, registration)
    }

    pub fn adt_decl(&mut self, name: &str, params: &[&str]) -> AdtDecl {
        let name_sym = sym(name);
        let param_syms: Vec<Symbol> = params.iter().map(|p| sym(p)).collect();
        AdtDecl::new(&name_sym, &param_syms, &mut self.types.supply)
    }

    pub fn inject_adt(&mut self, adt: AdtDecl) -> Result<(), EngineError> {
        // Type system gets the constructor schemes; runtime gets constructor functions
        // that build `Value::Adt` with the constructor tag and evaluated args.
        self.types.inject_adt(&adt);
        for (ctor, scheme) in adt.constructor_schemes() {
            let ctor_name = ctor.clone();
            let func = Arc::new(move |engine: &Engine<State>, _: &Type, args: &[Pointer]| {
                engine.heap().alloc_adt(ctor_name.clone(), args.to_vec())
            });
            let arity = type_arity(&scheme.typ);
            self.register_native(ctor, scheme, arity, NativeCallable::Sync(func), 0)?;
        }
        Ok(())
    }

    pub fn inject_type_decl(&mut self, decl: &TypeDecl) -> Result<(), EngineError> {
        let adt = self.types.adt_from_decl(decl).map_err(EngineError::Type)?;
        self.inject_adt(adt)
    }

    pub fn inject_class_decl(&mut self, decl: &ClassDecl) -> Result<(), EngineError> {
        self.types
            .inject_class_decl(decl)
            .map_err(EngineError::Type)
    }

    pub fn inject_instance_decl(&mut self, decl: &InstanceDecl) -> Result<(), EngineError> {
        let prepared = self
            .types
            .inject_instance_decl(decl)
            .map_err(EngineError::Type)?;
        self.register_typeclass_instance(decl, &prepared)
    }

    pub fn inject_fn_decl(&mut self, decl: &FnDecl) -> Result<(), EngineError> {
        self.inject_fn_decls(std::slice::from_ref(decl))
    }

    pub fn inject_fn_decls(&mut self, decls: &[FnDecl]) -> Result<(), EngineError> {
        if decls.is_empty() {
            return Ok(());
        }

        // Register declared types first so bodies can typecheck mutually-recursively.
        self.types
            .inject_fn_decls(decls)
            .map_err(EngineError::Type)?;

        // Build a recursive runtime environment with placeholders, then fill each slot.
        let mut env_rec = self.env.clone();
        let mut slots = Vec::with_capacity(decls.len());
        for decl in decls {
            let placeholder = self.heap().alloc_uninitialized(decl.name.name.clone())?;
            env_rec = env_rec.extend(decl.name.name.clone(), placeholder.clone());
            slots.push(placeholder);
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

                let typed = self.type_check(lam_body.as_ref())?;
                let (param_ty, _ret_ty) = split_fun(&typed.typ)
                    .ok_or_else(|| EngineError::NotCallable(typed.typ.to_string()))?;
                let TypedExprKind::Lam { param, body } = &typed.kind else {
                    return Err(EngineError::Internal(
                        "fn declaration did not lower to lambda".into(),
                    ));
                };
                let ptr = self.heap().alloc_closure(
                    self.env.clone(),
                    param.clone(),
                    param_ty,
                    typed.typ.clone(),
                    Arc::new(body.as_ref().clone()),
                )?;
                let value = self.heap().get(&ptr)?;
                self.heap().overwrite(slot, value.as_ref().clone())?;
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

    pub fn inject_decls(&mut self, decls: &[Decl]) -> Result<(), EngineError> {
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
                    self.types
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

    pub fn inject_class(&mut self, name: &str, supers: Vec<String>) {
        let supers = supers.into_iter().map(|s| sym(&s)).collect();
        self.types.inject_class(name, supers);
    }

    pub fn inject_instance(&mut self, class: &str, inst: Instance) {
        self.types.inject_instance(class, inst);
    }

    pub async fn eval(
        &mut self,
        expr: &Expr,
        gas: &mut GasMeter,
    ) -> Result<(Pointer, Type), EngineError> {
        check_cancelled(self)?;
        let typed = self.type_check(expr)?;
        let typ = typed.typ.clone();
        let value = eval_typed_expr(self, &self.env, &typed, gas).await?;
        Ok((value, typ))
    }

    fn inject_prelude(&mut self) -> Result<(), EngineError> {
        inject_prelude_adts(self)?;
        inject_equality_ops(self)?;
        inject_order_ops(self)?;
        inject_pretty_ops(self)?;
        inject_boolean_ops(self)?;
        inject_numeric_ops(self)?;
        inject_list_builtins(self)?;
        inject_option_result_builtins(self)?;
        inject_json_primops(self)?;
        self.register_prelude_typeclass_instances()?;
        Ok(())
    }

    fn register_prelude_typeclass_instances(&mut self) -> Result<(), EngineError> {
        // The type system prelude injects the *heads* of the standard instances.
        // The evaluator also needs the *method bodies* so class method lookup can
        // produce actual values at runtime.
        let program = rex_ts::prelude_typeclasses_program().map_err(EngineError::Type)?;
        for decl in program.decls.iter() {
            let Decl::Instance(inst_decl) = decl else {
                continue;
            };
            if inst_decl.methods.is_empty() {
                continue;
            }
            let prepared = self
                .types
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
        gas_cost: u64,
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
        self.natives.insert(name, arity, scheme, func, gas_cost)
    }

    fn register_type_scheme(
        &mut self,
        name: &Symbol,
        injected: &Scheme,
    ) -> Result<(), EngineError> {
        let schemes = self.types.env.lookup(name);
        match schemes {
            None => {
                self.types.add_value(name.as_ref(), injected.clone());
                Ok(())
            }
            Some(schemes) => {
                let has_poly = schemes
                    .iter()
                    .any(|s| !s.vars.is_empty() || !s.preds.is_empty());
                if has_poly {
                    for existing in schemes {
                        if scheme_accepts(&self.types, existing, &injected.typ)? {
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
                    self.types.add_overload(name.as_ref(), injected.clone());
                    Ok(())
                }
            }
        }
    }

    fn type_check(&mut self, expr: &Expr) -> Result<TypedExpr, EngineError> {
        let (typed, preds, _ty) = self.types.infer_typed(expr)?;
        let (typed, preds) = default_ambiguous_types(self, typed, preds)?;
        self.check_predicates(&preds)?;
        self.check_natives(&typed)?;
        Ok(typed)
    }

    pub(crate) fn infer_type(
        &mut self,
        expr: &Expr,
        gas: &mut GasMeter,
    ) -> Result<(Vec<Predicate>, Type), EngineError> {
        self.types
            .infer_with_gas(expr, gas)
            .map_err(EngineError::Type)
    }

    fn check_predicates(&self, preds: &[Predicate]) -> Result<(), EngineError> {
        for pred in preds {
            if pred.typ.ftv().is_empty() {
                let ok = entails(&self.types.classes, &[], pred)?;
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

    fn check_natives(&self, expr: &TypedExpr) -> Result<(), EngineError> {
        enum Frame<'a> {
            Expr(&'a TypedExpr),
            Push(Symbol),
            PushMany(Vec<Symbol>),
            Pop(usize),
        }

        let mut bound: Vec<Symbol> = Vec::new();
        let mut stack = vec![Frame::Expr(expr)];
        while let Some(frame) = stack.pop() {
            match frame {
                Frame::Expr(expr) => match &expr.kind {
                    TypedExprKind::Var { name, overloads } => {
                        if bound.iter().any(|n| n == name) {
                            continue;
                        }
                        if !self.natives.has_name(name) {
                            if self.env.get(name).is_some() {
                                continue;
                            }
                            if self.types.class_methods.contains_key(name) {
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
                        if expr.typ.ftv().is_empty() && !self.has_impl(name, &expr.typ) {
                            return Err(EngineError::MissingImpl {
                                name: name.clone(),
                                typ: expr.typ.to_string(),
                            });
                        }
                    }
                    TypedExprKind::Tuple(elems) | TypedExprKind::List(elems) => {
                        for elem in elems.iter().rev() {
                            stack.push(Frame::Expr(elem));
                        }
                    }
                    TypedExprKind::Dict(kvs) => {
                        for v in kvs.values().rev() {
                            stack.push(Frame::Expr(v));
                        }
                    }
                    TypedExprKind::RecordUpdate { base, updates } => {
                        for v in updates.values().rev() {
                            stack.push(Frame::Expr(v));
                        }
                        stack.push(Frame::Expr(base));
                    }
                    TypedExprKind::App(f, x) => {
                        // Process function, then argument.
                        stack.push(Frame::Expr(x));
                        stack.push(Frame::Expr(f));
                    }
                    TypedExprKind::Project { expr, .. } => {
                        stack.push(Frame::Expr(expr));
                    }
                    TypedExprKind::Lam { param, body } => {
                        stack.push(Frame::Pop(1));
                        stack.push(Frame::Expr(body));
                        stack.push(Frame::Push(param.clone()));
                    }
                    TypedExprKind::Let { name, def, body } => {
                        stack.push(Frame::Pop(1));
                        stack.push(Frame::Expr(body));
                        stack.push(Frame::Push(name.clone()));
                        stack.push(Frame::Expr(def));
                    }
                    TypedExprKind::LetRec { bindings, body } => {
                        if !bindings.is_empty() {
                            stack.push(Frame::Pop(bindings.len()));
                            stack.push(Frame::Expr(body));
                            for (_, def) in bindings.iter().rev() {
                                stack.push(Frame::Expr(def));
                            }
                            stack.push(Frame::PushMany(
                                bindings.iter().map(|(name, _)| name.clone()).collect(),
                            ));
                        } else {
                            stack.push(Frame::Expr(body));
                        }
                    }
                    TypedExprKind::Ite {
                        cond,
                        then_expr,
                        else_expr,
                    } => {
                        stack.push(Frame::Expr(else_expr));
                        stack.push(Frame::Expr(then_expr));
                        stack.push(Frame::Expr(cond));
                    }
                    TypedExprKind::Match { scrutinee, arms } => {
                        for (pat, arm_expr) in arms.iter().rev() {
                            let mut bindings = Vec::new();
                            collect_pattern_bindings(pat, &mut bindings);
                            let count = bindings.len();
                            if count != 0 {
                                stack.push(Frame::Pop(count));
                                stack.push(Frame::Expr(arm_expr));
                                stack.push(Frame::PushMany(bindings));
                            } else {
                                stack.push(Frame::Expr(arm_expr));
                            }
                        }
                        stack.push(Frame::Expr(scrutinee));
                    }
                    TypedExprKind::Bool(..)
                    | TypedExprKind::Uint(..)
                    | TypedExprKind::Int(..)
                    | TypedExprKind::Float(..)
                    | TypedExprKind::String(..)
                    | TypedExprKind::Uuid(..)
                    | TypedExprKind::DateTime(..) => {}
                },
                Frame::Push(sym) => bound.push(sym),
                Frame::PushMany(syms) => bound.extend(syms),
                Frame::Pop(count) => {
                    bound.truncate(bound.len().saturating_sub(count));
                }
            }
        }
        Ok(())
    }

    fn register_typeclass_instance(
        &mut self,
        decl: &InstanceDecl,
        prepared: &PreparedInstanceDecl,
    ) -> Result<(), EngineError> {
        let mut methods: HashMap<Symbol, Arc<TypedExpr>> = HashMap::new();
        for method in &decl.methods {
            let typed = self
                .types
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

    fn resolve_typeclass_method_impl(
        &self,
        name: &Symbol,
        call_type: &Type,
    ) -> Result<(Env, Arc<TypedExpr>, Subst), EngineError> {
        let info = self
            .types
            .class_methods
            .get(name)
            .ok_or_else(|| EngineError::UnknownVar(name.clone()))?;

        let s_method = unify(&info.scheme.typ, call_type).map_err(EngineError::Type)?;
        let class_pred = info
            .scheme
            .preds
            .iter()
            .find(|p| p.class == info.class)
            .ok_or(EngineError::Type(TypeError::UnsupportedExpr(
                "method scheme missing class predicate",
            )))?;
        let param_type = class_pred.typ.apply(&s_method);
        if type_head_is_var(&param_type) {
            return Err(EngineError::AmbiguousOverload { name: name.clone() });
        }

        self.typeclasses.resolve(&info.class, name, &param_type)
    }

    fn cached_class_method(&self, name: &Symbol, typ: &Type) -> Option<Pointer> {
        if !typ.ftv().is_empty() {
            return None;
        }
        let cache = self.typeclass_cache.lock().ok()?;
        cache.get(&(name.clone(), typ.clone())).cloned()
    }

    fn insert_cached_class_method(&self, name: &Symbol, typ: &Type, pointer: &Pointer) {
        if typ.ftv().is_empty()
            && let Ok(mut cache) = self.typeclass_cache.lock()
        {
            cache.insert((name.clone(), typ.clone()), pointer.clone());
        }
    }

    fn resolve_class_method_plan(
        &self,
        name: &Symbol,
        typ: &Type,
    ) -> Result<Result<(Env, TypedExpr), Pointer>, EngineError> {
        let (def_env, typed, s) = match self.resolve_typeclass_method_impl(name, typ) {
            Ok(res) => res,
            Err(EngineError::AmbiguousOverload { .. }) if is_function_type(typ) => {
                let (name, typ, applied, applied_types) =
                    OverloadedFn::new(name.clone(), typ.clone()).into_parts();
                let pointer = self
                    .heap()
                    .alloc_overloaded(name, typ, applied, applied_types)?;
                return Ok(Err(pointer));
            }
            Err(err) => return Err(err),
        };
        let specialized = typed.as_ref().apply(&s);
        Ok(Ok((def_env, specialized)))
    }

    async fn resolve_class_method(
        &self,
        name: &Symbol,
        typ: &Type,
        gas: &mut GasMeter,
    ) -> Result<Pointer, EngineError> {
        if let Some(pointer) = self.cached_class_method(name, typ) {
            return Ok(pointer);
        }

        let pointer = match self.resolve_class_method_plan(name, typ)? {
            Ok((def_env, specialized)) => {
                eval_typed_expr(self, &def_env, &specialized, gas).await?
            }
            Err(pointer) => pointer,
        };

        if typ.ftv().is_empty() {
            self.insert_cached_class_method(name, typ, &pointer);
        }
        Ok(pointer)
    }

    pub(crate) async fn resolve_global(
        &self,
        name: &Symbol,
        typ: &Type,
    ) -> Result<Pointer, EngineError> {
        if let Some(ptr) = self.env.get(name) {
            let value = self.heap().get(&ptr)?;
            match value.as_ref() {
                Value::Native(native) if native.arity == 0 && native.applied.is_empty() => {
                    let mut gas = GasMeter::default();
                    native.call_zero(self, &mut gas).await
                }
                _ => Ok(ptr),
            }
        } else if self.types.class_methods.contains_key(name) {
            let mut gas = GasMeter::default();
            self.resolve_class_method(name, typ, &mut gas).await
        } else {
            let mut gas = GasMeter::default();
            let pointer = self.resolve_native(name.as_ref(), typ, &mut gas)?;
            let value = self.heap().get(&pointer)?;
            match value.as_ref() {
                Value::Native(native) if native.arity == 0 && native.applied.is_empty() => {
                    let mut gas = GasMeter::default();
                    native.call_zero(self, &mut gas).await
                }
                _ => Ok(pointer),
            }
        }
    }

    pub(crate) fn lookup_scheme(&self, name: &Symbol) -> Result<Scheme, EngineError> {
        let schemes = self
            .types
            .env
            .lookup(name)
            .ok_or_else(|| EngineError::UnknownVar(name.clone()))?;
        if schemes.len() != 1 {
            return Err(EngineError::AmbiguousOverload { name: name.clone() });
        }
        Ok(schemes[0].clone())
    }

    fn has_impl(&self, name: &str, typ: &Type) -> bool {
        let sym_name = sym(name);
        self.natives
            .get(&sym_name)
            .map(|impls| impls.iter().any(|imp| impl_matches_type(imp, typ)))
            .unwrap_or(false)
    }

    fn resolve_native_impl(
        &self,
        name: &str,
        typ: &Type,
    ) -> Result<NativeImpl<State>, EngineError> {
        let sym_name = sym(name);
        let impls = self
            .natives
            .get(&sym_name)
            .ok_or_else(|| EngineError::UnknownVar(sym_name.clone()))?;
        let matches: Vec<NativeImpl<State>> = impls
            .iter()
            .filter(|imp| impl_matches_type(imp, typ))
            .cloned()
            .collect();
        match matches.len() {
            0 => Err(EngineError::MissingImpl {
                name: sym_name.clone(),
                typ: typ.to_string(),
            }),
            1 => Ok(matches[0].clone()),
            _ => Err(EngineError::AmbiguousImpl {
                name: sym_name,
                typ: typ.to_string(),
            }),
        }
    }

    fn native_callable(&self, id: NativeId) -> Result<NativeCallable<State>, EngineError> {
        self.natives
            .by_id(id)
            .map(|imp| imp.func.clone())
            .ok_or_else(|| EngineError::Internal(format!("unknown native id: {id}")))
    }

    pub(crate) async fn call_native_impl(
        &self,
        name: &str,
        typ: &Type,
        args: &[Pointer],
    ) -> Result<Pointer, EngineError> {
        let imp = self.resolve_native_impl(name, typ)?;
        imp.func.call(self, typ.clone(), args).await
    }

    fn resolve_native(
        &self,
        name: &str,
        typ: &Type,
        _gas: &mut GasMeter,
    ) -> Result<Pointer, EngineError> {
        let sym_name = sym(name);
        let impls = self
            .natives
            .get(&sym_name)
            .ok_or_else(|| EngineError::UnknownVar(sym_name.clone()))?;
        let matches: Vec<NativeImpl<State>> = impls
            .iter()
            .filter(|imp| impl_matches_type(imp, typ))
            .cloned()
            .collect();
        match matches.len() {
            0 => Err(EngineError::MissingImpl {
                name: sym_name.clone(),
                typ: typ.to_string(),
            }),
            1 => {
                let imp = matches[0].clone();
                let NativeFn {
                    native_id,
                    name,
                    arity,
                    typ,
                    gas_cost,
                    applied,
                    applied_types,
                } = imp.to_native_fn(typ.clone());
                self.heap().alloc_native(
                    native_id,
                    name,
                    arity,
                    typ,
                    gas_cost,
                    applied,
                    applied_types,
                )
            }
            _ => {
                if typ.ftv().is_empty() {
                    Err(EngineError::AmbiguousImpl {
                        name: sym_name.clone(),
                        typ: typ.to_string(),
                    })
                } else if is_function_type(typ) {
                    let (name, typ, applied, applied_types) =
                        OverloadedFn::new(sym_name.clone(), typ.clone()).into_parts();
                    self.heap()
                        .alloc_overloaded(name, typ, applied, applied_types)
                } else {
                    Err(EngineError::AmbiguousOverload { name: sym_name })
                }
            }
        }
    }
}

fn normalize_name(name: &str) -> Symbol {
    if let Some(stripped) = name.strip_prefix('(').and_then(|s| s.strip_suffix(')')) {
        let ok = stripped
            .chars()
            .all(|c| !c.is_alphanumeric() && c != '_' && !c.is_whitespace());
        if ok {
            return sym(stripped);
        }
    }
    sym(name)
}

fn default_ambiguous_types<State: Clone + Send + Sync + 'static>(
    engine: &Engine<State>,
    typed: TypedExpr,
    mut preds: Vec<Predicate>,
) -> Result<(TypedExpr, Vec<Predicate>), EngineError> {
    let mut candidates = Vec::new();
    collect_default_candidates(&typed, &mut candidates);
    for ty in [
        Type::con("f32", 0),
        Type::con("i32", 0),
        Type::con("string", 0),
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
            && tc.arity == 0
        {
            push_unique_type(out, expr.typ.clone());
        }

        match &expr.kind {
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
            | TypedExprKind::DateTime(..) => {}
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
            if !entails(&engine.types.classes, &[], &test)? {
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

fn is_function_type(typ: &Type) -> bool {
    matches!(typ.as_ref(), TypeKind::Fun(..))
}

fn collect_pattern_bindings(pat: &Pattern, out: &mut Vec<Symbol>) {
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

fn impl_matches_type<State: Clone + Send + Sync + 'static>(
    imp: &NativeImpl<State>,
    typ: &Type,
) -> bool {
    let mut supply = TypeVarSupply::new();
    let (_preds, scheme_ty) = instantiate(&imp.scheme, &mut supply);
    unify(&scheme_ty, typ).is_ok()
}

fn value_type(heap: &Heap, value: &Value) -> Result<Type, EngineError> {
    let pointer_type = |pointer: &Pointer| -> Result<Type, EngineError> {
        let value = heap.get(pointer)?;
        value_type(heap, value.as_ref())
    };

    match value {
        Value::Bool(..) => Ok(Type::con("bool", 0)),
        Value::U8(..) => Ok(Type::con("u8", 0)),
        Value::U16(..) => Ok(Type::con("u16", 0)),
        Value::U32(..) => Ok(Type::con("u32", 0)),
        Value::U64(..) => Ok(Type::con("u64", 0)),
        Value::I8(..) => Ok(Type::con("i8", 0)),
        Value::I16(..) => Ok(Type::con("i16", 0)),
        Value::I32(..) => Ok(Type::con("i32", 0)),
        Value::I64(..) => Ok(Type::con("i64", 0)),
        Value::F32(..) => Ok(Type::con("f32", 0)),
        Value::F64(..) => Ok(Type::con("f64", 0)),
        Value::String(..) => Ok(Type::con("string", 0)),
        Value::Uuid(..) => Ok(Type::con("uuid", 0)),
        Value::DateTime(..) => Ok(Type::con("datetime", 0)),
        Value::Tuple(elems) => {
            let mut tys = Vec::with_capacity(elems.len());
            for elem in elems {
                tys.push(pointer_type(elem)?);
            }
            Ok(Type::tuple(tys))
        }
        Value::Array(elems) => {
            let first = elems
                .first()
                .ok_or_else(|| EngineError::UnknownType(sym("array")))?;
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
            Ok(Type::app(Type::con("Array", 1), elem_ty))
        }
        Value::Dict(map) => {
            let first = map
                .values()
                .next()
                .ok_or_else(|| EngineError::UnknownType(sym("dict")))?;
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
            Ok(Type::app(Type::con("Dict", 1), elem_ty))
        }
        Value::Adt(tag, args) if sym_eq(tag, "Some") && args.len() == 1 => {
            let inner = pointer_type(&args[0])?;
            Ok(Type::app(Type::con("Option", 1), inner))
        }
        Value::Adt(tag, args) if sym_eq(tag, "None") && args.is_empty() => {
            Err(EngineError::UnknownType(sym("option")))
        }
        Value::Adt(tag, args) if (sym_eq(tag, "Ok") || sym_eq(tag, "Err")) && args.len() == 1 => {
            Err(EngineError::UnknownType(sym("result")))
        }
        Value::Adt(tag, args)
            if (sym_eq(tag, "Empty") || sym_eq(tag, "Cons")) && args.len() <= 2 =>
        {
            let elems = list_to_vec(heap, value)?;
            let first = elems
                .first()
                .ok_or_else(|| EngineError::UnknownType(sym("list")))?;
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
            Ok(Type::app(Type::con("List", 1), elem_ty))
        }
        Value::Adt(tag, _args) => Err(EngineError::UnknownType(tag.clone())),
        Value::Uninitialized(..) => Err(EngineError::UnknownType(sym("uninitialized"))),
        Value::Closure(..) => Err(EngineError::UnknownType(sym("closure"))),
        Value::Native(..) => Err(EngineError::UnknownType(sym("native"))),
        Value::Overloaded(..) => Err(EngineError::UnknownType(sym("overloaded"))),
    }
}

fn resolve_arg_type(
    heap: &Heap,
    arg_type: Option<&Type>,
    arg: &Pointer,
) -> Result<Type, EngineError> {
    let infer_from_value = |ty_hint: Option<&Type>| -> Result<Type, EngineError> {
        let value = heap.get(arg)?;
        match ty_hint {
            Some(ty) => match value_type(heap, value.as_ref()) {
                Ok(val_ty) if val_ty.ftv().is_empty() => Ok(val_ty),
                _ => Ok(ty.clone()),
            },
            None => value_type(heap, value.as_ref()),
        }
    };
    match arg_type {
        Some(ty) if ty.ftv().is_empty() => Ok(ty.clone()),
        Some(ty) => infer_from_value(Some(ty)),
        None => infer_from_value(None),
    }
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
    let value = heap.get(pointer)?;
    if let Ok(index) = field.as_ref().parse::<usize>() {
        return match value.as_ref() {
            Value::Tuple(items) => {
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
    match value.as_ref() {
        Value::Adt(_, args) if args.len() == 1 => {
            let inner = heap.get(&args[0])?;
            match inner.as_ref() {
                Value::Dict(map) => {
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

#[async_recursion]
async fn eval_typed_expr<State>(
    engine: &Engine<State>,
    env: &Env,
    expr: &TypedExpr,
    gas: &mut GasMeter,
) -> Result<Pointer, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    check_cancelled(engine)?;
    let mut env = env.clone();
    let mut cur = expr;
    loop {
        check_cancelled(engine)?;
        match &cur.kind {
            TypedExprKind::Let { name, def, body } => {
                gas.charge(gas.costs.eval_node)?;
                let ptr = eval_typed_expr(engine, &env, def, gas).await?;
                env = env.extend(name.clone(), ptr);
                cur = body;
            }
            _ => break,
        }
    }

    gas.charge(gas.costs.eval_node)?;
    match &cur.kind {
        TypedExprKind::Bool(v) => engine.heap().alloc_bool(*v),
        TypedExprKind::Uint(v) => engine.heap().alloc_i32(*v as i32),
        TypedExprKind::Int(v) => engine.heap().alloc_i32(*v as i32),
        TypedExprKind::Float(v) => engine.heap().alloc_f32(*v as f32),
        TypedExprKind::String(v) => engine.heap().alloc_string(v.clone()),
        TypedExprKind::Uuid(v) => engine.heap().alloc_uuid(*v),
        TypedExprKind::DateTime(v) => engine.heap().alloc_datetime(*v),
        TypedExprKind::Tuple(elems) => {
            let mut values = Vec::with_capacity(elems.len());
            for elem in elems {
                check_cancelled(engine)?;
                values.push(eval_typed_expr(engine, &env, elem, gas).await?);
            }
            engine.heap().alloc_tuple(values)
        }
        TypedExprKind::List(elems) => {
            let mut values = Vec::with_capacity(elems.len());
            for elem in elems {
                check_cancelled(engine)?;
                values.push(eval_typed_expr(engine, &env, elem, gas).await?);
            }
            let mut list = engine.heap().alloc_adt(sym("Empty"), vec![])?;
            for value in values.into_iter().rev() {
                list = engine.heap().alloc_adt(sym("Cons"), vec![value, list])?;
            }
            Ok(list)
        }
        TypedExprKind::Dict(kvs) => {
            let mut out = BTreeMap::new();
            for (k, v) in kvs {
                check_cancelled(engine)?;
                out.insert(k.clone(), eval_typed_expr(engine, &env, v, gas).await?);
            }
            engine.heap().alloc_dict(out)
        }
        TypedExprKind::RecordUpdate { base, updates } => {
            let base_ptr = eval_typed_expr(engine, &env, base, gas).await?;
            let mut update_vals = BTreeMap::new();
            for (k, v) in updates {
                check_cancelled(engine)?;
                update_vals.insert(k.clone(), eval_typed_expr(engine, &env, v, gas).await?);
            }

            let base_val = engine.heap().get(&base_ptr)?;
            match base_val.as_ref() {
                Value::Dict(map) => {
                    let mut map = map.clone();
                    for (k, v) in update_vals {
                        gas.charge(gas.costs.eval_record_update_field)?;
                        map.insert(k, v);
                    }
                    engine.heap().alloc_dict(map)
                }
                Value::Adt(tag, args) if args.len() == 1 => {
                    let inner = engine.heap().get(&args[0])?;
                    match inner.as_ref() {
                        Value::Dict(map) => {
                            let mut out = map.clone();
                            for (k, v) in update_vals {
                                gas.charge(gas.costs.eval_record_update_field)?;
                                out.insert(k, v);
                            }
                            let dict = engine.heap().alloc_dict(out)?;
                            engine.heap().alloc_adt(tag.clone(), vec![dict])
                        }
                        _ => Err(EngineError::UnsupportedExpr),
                    }
                }
                _ => Err(EngineError::UnsupportedExpr),
            }
        }
        TypedExprKind::Var { name, .. } => {
            if let Some(ptr) = env.get(name) {
                let value = engine.heap().get(&ptr)?;
                match value.as_ref() {
                    Value::Native(native) if native.arity == 0 && native.applied.is_empty() => {
                        native.call_zero(engine, gas).await
                    }
                    _ => Ok(ptr),
                }
            } else if engine.types.class_methods.contains_key(name) {
                engine.resolve_class_method(name, &cur.typ, gas).await
            } else {
                let value = engine.resolve_native(name.as_ref(), &cur.typ, gas)?;
                match engine.heap().get(&value)?.as_ref() {
                    Value::Native(native) if native.arity == 0 && native.applied.is_empty() => {
                        native.call_zero(engine, gas).await
                    }
                    _ => Ok(value),
                }
            }
        }
        TypedExprKind::App(..) => {
            let mut spine: Vec<(Type, &TypedExpr)> = Vec::new();
            let mut head = cur;
            while let TypedExprKind::App(f, x) = &head.kind {
                check_cancelled(engine)?;
                spine.push((f.typ.clone(), x.as_ref()));
                head = f.as_ref();
            }
            spine.reverse();

            let mut func = eval_typed_expr(engine, &env, head, gas).await?;
            for (func_type, arg_expr) in spine {
                check_cancelled(engine)?;
                gas.charge(gas.costs.eval_app_step)?;
                let arg = eval_typed_expr(engine, &env, arg_expr, gas).await?;
                func = apply(
                    engine,
                    func,
                    arg,
                    Some(&func_type),
                    Some(&arg_expr.typ),
                    gas,
                )
                .await?;
            }
            Ok(func)
        }
        TypedExprKind::Project { expr, field } => {
            let value = eval_typed_expr(engine, &env, expr, gas).await?;
            project_pointer(engine.heap(), field, &value)
        }
        TypedExprKind::Lam { param, body } => {
            let param_ty = split_fun(&expr.typ)
                .map(|(arg, _)| arg)
                .ok_or_else(|| EngineError::NotCallable(expr.typ.to_string()))?;
            engine.heap().alloc_closure(
                env.clone(),
                param.clone(),
                param_ty,
                expr.typ.clone(),
                Arc::new(body.as_ref().clone()),
            )
        }
        TypedExprKind::LetRec { bindings, body } => {
            let mut env_rec = env.clone();
            let mut slots = Vec::with_capacity(bindings.len());
            for (name, _) in bindings {
                let placeholder = engine.heap().alloc_uninitialized(name.clone())?;
                env_rec = env_rec.extend(name.clone(), placeholder.clone());
                slots.push(placeholder);
            }

            for ((_, def), slot) in bindings.iter().zip(slots.iter()) {
                check_cancelled(engine)?;
                gas.charge(gas.costs.eval_node)?;
                let def_ptr = eval_typed_expr(engine, &env_rec, def, gas).await?;
                let def_value = engine.heap().get(&def_ptr)?;
                engine.heap().overwrite(slot, def_value.as_ref().clone())?;
            }

            eval_typed_expr(engine, &env_rec, body, gas).await
        }
        TypedExprKind::Ite {
            cond,
            then_expr,
            else_expr,
        } => {
            let cond_ptr = eval_typed_expr(engine, &env, cond, gas).await?;
            match engine.heap().pointer_as_bool(&cond_ptr) {
                Ok(true) => eval_typed_expr(engine, &env, then_expr, gas).await,
                Ok(false) => eval_typed_expr(engine, &env, else_expr, gas).await,
                Err(EngineError::NativeType { got, .. }) => Err(EngineError::ExpectedBool(got)),
                Err(err) => Err(err),
            }
        }
        TypedExprKind::Match { scrutinee, arms } => {
            let value = eval_typed_expr(engine, &env, scrutinee, gas).await?;
            for (pat, expr) in arms {
                check_cancelled(engine)?;
                gas.charge(gas.costs.eval_match_arm)?;
                if let Some(bindings) = match_pattern_ptr(engine.heap(), pat, &value) {
                    let env = env.extend_many(bindings);
                    return eval_typed_expr(engine, &env, expr, gas).await;
                }
            }
            Err(EngineError::MatchFailure)
        }
        TypedExprKind::Let { .. } => {
            unreachable!("let chain handled in eval_typed_expr loop")
        }
    }
}

#[async_recursion]
pub(crate) async fn apply<State>(
    engine: &Engine<State>,
    func: Pointer,
    arg: Pointer,
    func_type: Option<&Type>,
    arg_type: Option<&Type>,
    gas: &mut GasMeter,
) -> Result<Pointer, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let func_value = engine.heap().get(&func)?.as_ref().clone();
    match func_value {
        Value::Closure(Closure {
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
            let actual_ty = resolve_arg_type(engine.heap(), arg_type, &arg)?;
            let param_ty = param_ty.apply(&subst);
            let s_arg = unify(&param_ty, &actual_ty).map_err(|_| EngineError::NativeType {
                expected: param_ty.to_string(),
                got: actual_ty.to_string(),
            })?;
            subst = compose_subst(s_arg, subst);
            let env = env.extend(param, arg);
            let body = body.apply(&subst);
            eval_typed_expr(engine, &env, &body, gas).await
        }
        Value::Native(native) => native.apply(engine, arg, arg_type, gas).await,
        Value::Overloaded(over) => over.apply(engine, arg, func_type, arg_type, gas).await,
        _ => Err(EngineError::NotCallable(
            engine.heap().type_name(&func)?.into(),
        )),
    }
}

fn match_pattern_ptr(
    heap: &Heap,
    pat: &Pattern,
    value: &Pointer,
) -> Option<HashMap<Symbol, Pointer>> {
    match pat {
        Pattern::Wildcard(..) => Some(HashMap::new()),
        Pattern::Var(var) => {
            let mut bindings = HashMap::new();
            bindings.insert(var.name.clone(), value.clone());
            Some(bindings)
        }
        Pattern::Named(_, name, ps) => {
            let v = heap.get(value).ok()?;
            match v.as_ref() {
                Value::Adt(vname, args) if vname == name && args.len() == ps.len() => {
                    match_patterns(heap, ps, args)
                }
                _ => None,
            }
        }
        Pattern::Tuple(_, ps) => {
            let v = heap.get(value).ok()?;
            match v.as_ref() {
                Value::Tuple(xs) if xs.len() == ps.len() => match_patterns(heap, ps, xs),
                _ => None,
            }
        }
        Pattern::List(_, ps) => {
            let v = heap.get(value).ok()?;
            let values = list_to_vec(heap, v.as_ref()).ok()?;
            if values.len() == ps.len() {
                match_patterns(heap, ps, &values)
            } else {
                None
            }
        }
        Pattern::Cons(_, head, tail) => {
            let v = heap.get(value).ok()?;
            match v.as_ref() {
                Value::Adt(tag, args) if sym_eq(tag, "Cons") && args.len() == 2 => {
                    let mut left = match_pattern_ptr(heap, head, &args[0])?;
                    let right = match_pattern_ptr(heap, tail, &args[1])?;
                    left.extend(right);
                    Some(left)
                }
                _ => None,
            }
        }
        Pattern::Dict(_, fields) => {
            let v = heap.get(value).ok()?;
            match v.as_ref() {
                Value::Dict(map) => {
                    let mut bindings = HashMap::new();
                    for (key, pat) in fields {
                        let v = map.get(key)?;
                        let sub = match_pattern_ptr(heap, pat, v)?;
                        bindings.extend(sub);
                    }
                    Some(bindings)
                }
                _ => None,
            }
        }
    }
}

fn match_patterns(
    heap: &Heap,
    patterns: &[Pattern],
    values: &[Pointer],
) -> Option<HashMap<Symbol, Pointer>> {
    let mut bindings = HashMap::new();
    for (p, v) in patterns.iter().zip(values.iter()) {
        let sub = match_pattern_ptr(heap, p, v)?;
        bindings.extend(sub);
    }
    Some(bindings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rex_util::{GasCosts, GasMeter};
    use std::path::Path;

    use crate::ReplState;

    fn parse(code: &str) -> Arc<Expr> {
        let mut parser = rex_parser::Parser::new(rex_lexer::Token::tokenize(code).unwrap());
        parser.parse_program(&mut GasMeter::default()).unwrap().expr
    }

    fn parse_program(code: &str) -> rex_ast::expr::Program {
        let mut parser = rex_parser::Parser::new(rex_lexer::Token::tokenize(code).unwrap());
        parser.parse_program(&mut GasMeter::default()).unwrap()
    }

    fn strip_span(mut err: TypeError) -> TypeError {
        while let TypeError::Spanned { error, .. } = err {
            err = *error;
        }
        err
    }

    fn engine_with_arith() -> Engine {
        Engine::with_prelude(()).unwrap()
    }

    fn unlimited_gas() -> GasMeter {
        GasMeter::default()
    }

    #[tokio::test]
    async fn repl_persists_function_definitions() {
        let mut gas = unlimited_gas();
        let mut engine = Engine::with_prelude(()).unwrap();
        engine.add_default_resolvers();
        let mut state = ReplState::new();

        let program1 = parse_program("fn inc (x: i32) -> i32 = x + 1\ninc 1");
        let (v1, t1) = engine
            .eval_repl_program(&program1, &mut state, &mut gas)
            .await
            .unwrap();
        assert_eq!(t1, Type::con("i32", 0));
        let expected = engine.heap().alloc_i32(2).unwrap();
        assert!(crate::pointer_eq(engine.heap(), &v1, &expected).unwrap());

        let program2 = parse_program("inc 2");
        let (v2, t2) = engine
            .eval_repl_program(&program2, &mut state, &mut gas)
            .await
            .unwrap();
        assert_eq!(t2, Type::con("i32", 0));
        let expected = engine.heap().alloc_i32(3).unwrap();
        assert!(crate::pointer_eq(engine.heap(), &v2, &expected).unwrap());
    }

    #[tokio::test]
    async fn repl_persists_import_aliases() {
        let mut gas = unlimited_gas();
        let mut engine = Engine::with_prelude(()).unwrap();
        engine.add_default_resolvers();

        let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../rex/examples/modules_basic");
        engine.add_include_resolver(&examples).unwrap();

        let mut state = ReplState::new();
        let program1 = parse_program("import foo.bar as Bar\n()");
        let (v1, t1) = engine
            .eval_repl_program(&program1, &mut state, &mut gas)
            .await
            .unwrap();
        assert_eq!(t1, Type::tuple(vec![]));
        let expected = engine.heap().alloc_tuple(vec![]).unwrap();
        assert!(crate::pointer_eq(engine.heap(), &v1, &expected).unwrap());

        let program2 = parse_program("Bar.triple 10");
        let (v2, t2) = engine
            .eval_repl_program(&program2, &mut state, &mut gas)
            .await
            .unwrap();
        assert_eq!(t2, Type::con("i32", 0));
        let expected = engine.heap().alloc_i32(30).unwrap();
        assert!(crate::pointer_eq(engine.heap(), &v2, &expected).unwrap());
    }

    #[tokio::test]
    async fn repl_persists_imported_values() {
        let mut gas = unlimited_gas();
        let mut engine = Engine::with_prelude(()).unwrap();
        engine.add_default_resolvers();

        let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../rex/examples/modules_basic");
        engine.add_include_resolver(&examples).unwrap();

        let mut state = ReplState::new();
        let program1 = parse_program("import foo.bar (triple as t)\n()");
        let (v1, t1) = engine
            .eval_repl_program(&program1, &mut state, &mut gas)
            .await
            .unwrap();
        assert_eq!(t1, Type::tuple(vec![]));
        let expected = engine.heap().alloc_tuple(vec![]).unwrap();
        assert!(crate::pointer_eq(engine.heap(), &v1, &expected).unwrap());

        let program2 = parse_program("t 10");
        let (v2, t2) = engine
            .eval_repl_program(&program2, &mut state, &mut gas)
            .await
            .unwrap();
        assert_eq!(t2, Type::con("i32", 0));
        let expected = engine.heap().alloc_i32(30).unwrap();
        assert!(crate::pointer_eq(engine.heap(), &v2, &expected).unwrap());
    }

    #[tokio::test]
    async fn eval_can_be_cancelled_while_waiting_on_async_native() {
        let expr = parse("stall");
        let mut engine = Engine::with_prelude(()).unwrap();

        let (started_tx, started_rx) = std::sync::mpsc::channel::<()>();
        let scheme = Scheme::new(vec![], vec![], Type::con("i32", 0));
        engine
            .export_native_async_cancellable(
                "stall",
                scheme,
                0,
                move |_engine, token: CancellationToken, _, _args| {
                    let started_tx = started_tx.clone();
                    async move {
                        let _ = started_tx.send(());
                        token.cancelled().await;
                        _engine.heap().alloc_i32(0)
                    }
                    .boxed()
                },
            )
            .unwrap();

        let token = engine.cancellation_token();
        let canceller = std::thread::spawn(move || {
            let recv = started_rx.recv_timeout(std::time::Duration::from_secs(2));
            assert!(recv.is_ok(), "stall native never started");
            token.cancel();
        });

        let mut gas = unlimited_gas();
        let res = engine.eval(expr.as_ref(), &mut gas).await;
        let joined = canceller.join();
        assert!(joined.is_ok(), "cancel thread panicked");
        assert!(matches!(res, Err(EngineError::Cancelled)));
    }

    #[tokio::test]
    async fn eval_can_be_cancelled_while_waiting_on_non_cancellable_async_native() {
        let expr = parse("stall");
        let mut engine = Engine::with_prelude(()).unwrap();

        let (started_tx, started_rx) = std::sync::mpsc::channel::<()>();
        engine
            .export_async("stall", move |_state: &()| {
                let started_tx = started_tx.clone();
                async move {
                    let _ = started_tx.send(());
                    futures::future::pending::<Result<i32, EngineError>>().await
                }
            })
            .unwrap();

        let token = engine.cancellation_token();
        let canceller = std::thread::spawn(move || {
            let recv = started_rx.recv_timeout(std::time::Duration::from_secs(2));
            assert!(recv.is_ok(), "stall native never started");
            token.cancel();
        });

        let mut gas = unlimited_gas();
        let res = engine.eval(expr.as_ref(), &mut gas).await;
        let joined = canceller.join();
        assert!(joined.is_ok(), "cancel thread panicked");
        assert!(matches!(res, Err(EngineError::Cancelled)));
    }

    #[tokio::test]
    async fn native_per_impl_gas_cost_is_charged() {
        let expr = parse("foo");
        let mut engine = Engine::with_prelude(()).unwrap();
        let scheme = Scheme::new(vec![], vec![], Type::con("i32", 0));
        engine
            .export_native_with_gas_cost("foo", scheme, 0, 50, |engine, _t, _args| {
                engine.heap().alloc_i32(1)
            })
            .unwrap();

        let mut gas = GasMeter::new(
            Some(10),
            GasCosts {
                eval_node: 1,
                native_call_base: 1,
                native_call_per_arg: 0,
                ..GasCosts::sensible_defaults()
            },
        );
        let err = match engine.eval(expr.as_ref(), &mut gas).await {
            Ok(_) => panic!("expected out of gas"),
            Err(e) => e,
        };
        assert!(matches!(err, EngineError::OutOfGas(..)));
    }

    #[tokio::test]
    async fn export_value_typed_registers_global_value() {
        let expr = parse("answer");
        let mut engine = Engine::with_prelude(()).unwrap();
        engine
            .export_value_typed("answer", Type::con("i32", 0), Value::I32(42))
            .unwrap();

        let mut gas = unlimited_gas();
        let (value, ty) = engine.eval(expr.as_ref(), &mut gas).await.unwrap();
        assert_eq!(ty, Type::con("i32", 0));
        let expected = engine.heap().alloc_i32(42).unwrap();
        assert!(crate::pointer_eq(engine.heap(), &value, &expected).unwrap());
    }

    #[tokio::test]
    async fn async_native_per_impl_gas_cost_is_charged() {
        let expr = parse("foo");
        let mut engine = Engine::with_prelude(()).unwrap();
        let scheme = Scheme::new(vec![], vec![], Type::con("i32", 0));
        engine
            .export_native_async_with_gas_cost("foo", scheme, 0, 50, |engine, _t, _args| {
                async move { engine.heap().alloc_i32(1) }.boxed()
            })
            .unwrap();

        let mut gas = GasMeter::new(
            Some(10),
            GasCosts {
                eval_node: 1,
                native_call_base: 1,
                native_call_per_arg: 0,
                ..GasCosts::sensible_defaults()
            },
        );
        let err = match engine.eval(expr.as_ref(), &mut gas).await {
            Ok(_) => panic!("expected out of gas"),
            Err(e) => e,
        };
        assert!(matches!(err, EngineError::OutOfGas(..)));
    }

    #[tokio::test]
    async fn cancellable_async_native_per_impl_gas_cost_is_charged() {
        let expr = parse("foo");
        let mut engine = Engine::with_prelude(()).unwrap();
        let scheme = Scheme::new(vec![], vec![], Type::con("i32", 0));
        engine
            .export_native_async_cancellable_with_gas_cost(
                "foo",
                scheme,
                0,
                50,
                |engine, _token: CancellationToken, _t, _args| {
                    async move { engine.heap().alloc_i32(1) }.boxed()
                },
            )
            .unwrap();

        let mut gas = GasMeter::new(
            Some(10),
            GasCosts {
                eval_node: 1,
                native_call_base: 1,
                native_call_per_arg: 0,
                ..GasCosts::sensible_defaults()
            },
        );
        let err = match engine.eval(expr.as_ref(), &mut gas).await {
            Ok(_) => panic!("expected out of gas"),
            Err(e) => e,
        };
        assert!(matches!(err, EngineError::OutOfGas(..)));
    }

    #[tokio::test]
    async fn record_update_requires_known_variant_for_sum_types() {
        let program = parse_program(
            r#"
            type Foo = Bar { x: i32 } | Baz { x: i32 }
            let
              f = \ (foo : Foo) -> { foo with { x = 2 } }
            in
              f (Bar { x = 1 })
            "#,
        );
        let mut engine = engine_with_arith();
        engine.inject_decls(&program.decls).unwrap();
        let mut gas = unlimited_gas();
        match engine.eval(program.expr.as_ref(), &mut gas).await {
            Err(EngineError::Type(err)) => {
                let err = strip_span(err);
                assert!(matches!(err, TypeError::FieldNotKnown { .. }));
            }
            _ => panic!("expected type error"),
        }
    }
}

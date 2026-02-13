//! Core engine implementation for Rex.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use async_recursion::async_recursion;
use futures::{FutureExt, future::BoxFuture, pin_mut};
use rex_ast::expr::{
    ClassDecl, Decl, Expr, FnDecl, InstanceDecl, Pattern, Scope, Symbol, TypeDecl, sym, sym_eq,
};
use rex_ts::{
    AdtDecl, Instance, Predicate, PreparedInstanceDecl, Scheme, Subst, Type, TypeError, TypeKind,
    TypeSystem, TypeVarSupply, TypedExpr, TypedExprKind, Types, compose_subst, entails,
    instantiate, unify,
};
use rex_util::{GasCosts, GasMeter};

use crate::modules::ModuleSystem;
use crate::prelude::{
    inject_boolean_ops, inject_equality_ops, inject_json_primops, inject_list_builtins,
    inject_numeric_ops, inject_option_result_builtins, inject_order_ops, inject_prelude_adts,
    inject_pretty_ops,
};
use crate::value::{Closure, Heap, Pointer, Value, list_to_vec};
use crate::{CancellationToken, EngineError, Env, FromPointer, IntoPointer, RexType};

fn check_cancelled(engine: &Engine) -> Result<(), EngineError> {
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
type SyncNativeCallable =
    Arc<dyn Fn(&Engine, &Type, &[Pointer]) -> Result<Pointer, EngineError> + Send + Sync + 'static>;
type AsyncNativeCallable =
    Arc<dyn for<'a> Fn(&'a Engine, Type, Vec<Pointer>) -> NativeFuture<'a> + Send + Sync + 'static>;
type AsyncNativeCallableCancellable = Arc<
    dyn for<'a> Fn(&'a Engine, CancellationToken, Type, Vec<Pointer>) -> NativeFuture<'a>
        + Send
        + Sync
        + 'static,
>;

#[derive(Clone)]
pub(crate) enum NativeCallable {
    Sync(SyncNativeCallable),
    Async(AsyncNativeCallable),
    AsyncCancellable(AsyncNativeCallableCancellable),
}

impl PartialEq for NativeCallable {
    fn eq(&self, _other: &NativeCallable) -> bool {
        false
    }
}

impl std::fmt::Debug for NativeCallable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        match self {
            NativeCallable::Sync(_) => write!(f, "Sync"),
            NativeCallable::Async(_) => write!(f, "Async"),
            NativeCallable::AsyncCancellable(_) => write!(f, "AsyncCancellable"),
        }
    }
}

impl NativeCallable {
    fn call_sync(
        &self,
        engine: &Engine,
        typ: &Type,
        args: &[Pointer],
    ) -> Result<Pointer, EngineError> {
        match self {
            NativeCallable::Sync(f) => (f)(engine, typ, args),
            NativeCallable::Async(..) | NativeCallable::AsyncCancellable(..) => {
                futures::executor::block_on(self.call_async(engine, typ.clone(), args.to_vec()))
            }
        }
    }

    async fn call_async(
        &self,
        engine: &Engine,
        typ: Type,
        args: Vec<Pointer>,
    ) -> Result<Pointer, EngineError> {
        let token = engine.cancellation_token();
        if token.is_cancelled() {
            return Err(EngineError::Cancelled);
        }

        match self {
            NativeCallable::Sync(f) => (f)(engine, &typ, &args),
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
    name: Symbol,
    arity: usize,
    typ: Type,
    func: NativeCallable,
    gas_cost: u64,
    applied: Vec<Pointer>,
    applied_types: Vec<Type>,
}

impl NativeFn {
    fn new(name: Symbol, arity: usize, typ: Type, func: NativeCallable, gas_cost: u64) -> Self {
        Self {
            name,
            arity,
            typ,
            func,
            gas_cost,
            applied: Vec::new(),
            applied_types: Vec::new(),
        }
    }

    pub(crate) fn from_parts(
        name: Symbol,
        arity: usize,
        typ: Type,
        func: NativeCallable,
        gas_cost: u64,
        applied: Vec<Pointer>,
        applied_types: Vec<Type>,
    ) -> Self {
        Self {
            name,
            arity,
            typ,
            func,
            gas_cost,
            applied,
            applied_types,
        }
    }

    pub(crate) fn name(&self) -> &Symbol {
        &self.name
    }

    fn apply(
        mut self,
        engine: &Engine,
        arg: Pointer,
        arg_type: Option<&Type>,
    ) -> Result<Pointer, EngineError> {
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
                name,
                arity,
                typ,
                func,
                gas_cost,
                applied,
                applied_types,
            } = self;
            return engine.heap().alloc_native(
                name,
                arity,
                typ,
                func,
                gas_cost,
                applied,
                applied_types,
            );
        }
        let mut full_ty = self.typ.clone();
        for arg_ty in self.applied_types.iter().rev() {
            full_ty = Type::fun(arg_ty.clone(), full_ty);
        }
        self.func.call_sync(engine, &full_ty, &self.applied)
    }

    fn call_zero(&self, engine: &Engine) -> Result<Pointer, EngineError> {
        if self.arity != 0 {
            return Err(EngineError::NativeArity {
                name: self.name.clone(),
                expected: self.arity,
                got: 0,
            });
        }
        self.func.call_sync(engine, &self.typ, &[])
    }

    async fn call_zero_async(&self, engine: &Engine) -> Result<Pointer, EngineError> {
        if self.arity != 0 {
            return Err(EngineError::NativeArity {
                name: self.name.clone(),
                expected: self.arity,
                got: 0,
            });
        }
        self.func
            .call_async(engine, self.typ.clone(), Vec::new())
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

    fn apply(
        mut self,
        engine: &Engine,
        arg: Pointer,
        func_type: Option<&Type>,
        arg_type: Option<&Type>,
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
            // Defer typeclass method selection until we have concrete argument
            // types. This mirrors the native-overload behavior and keeps
            // polymorphic code runnable without guessing an instance.
            let mut func = engine.resolve_class_method(&self.name, &full_ty)?;
            let mut cur_ty = full_ty;
            for (applied, applied_ty) in self.applied.into_iter().zip(self.applied_types.iter()) {
                let (arg_ty, rest_ty) = split_fun(&cur_ty)
                    .ok_or_else(|| EngineError::NotCallable(cur_ty.to_string()))?;
                let subst = unify(&arg_ty, applied_ty).map_err(|_| EngineError::NativeType {
                    expected: arg_ty.to_string(),
                    got: applied_ty.to_string(),
                })?;
                let rest_ty = rest_ty.apply(&subst);
                func = apply(engine, func, applied, Some(&cur_ty), Some(applied_ty))?;
                cur_ty = rest_ty;
            }
            return Ok(func);
        }

        let imp = engine.resolve_native_impl(self.name.as_ref(), &full_ty)?;
        imp.func.call_sync(engine, &full_ty, &self.applied)
    }

    fn apply_with_gas(
        mut self,
        engine: &Engine,
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
            let mut func = engine.resolve_class_method_with_gas(&self.name, &full_ty, gas)?;
            let mut cur_ty = full_ty;
            for (applied, applied_ty) in self.applied.into_iter().zip(self.applied_types.iter()) {
                let (arg_ty, rest_ty) = split_fun(&cur_ty)
                    .ok_or_else(|| EngineError::NotCallable(cur_ty.to_string()))?;
                let subst = unify(&arg_ty, applied_ty).map_err(|_| EngineError::NativeType {
                    expected: arg_ty.to_string(),
                    got: applied_ty.to_string(),
                })?;
                let rest_ty = rest_ty.apply(&subst);
                func = apply_with_gas(engine, func, applied, Some(&cur_ty), Some(applied_ty), gas)?;
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
        imp.func.call_sync(engine, &full_ty, &self.applied)
    }

    async fn apply_async_with_gas(
        mut self,
        engine: &Engine,
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
            let mut func = engine.resolve_class_method_with_gas(&self.name, &full_ty, gas)?;
            let mut cur_ty = full_ty;
            for (applied, applied_ty) in self.applied.into_iter().zip(self.applied_types.iter()) {
                let (arg_ty, rest_ty) = split_fun(&cur_ty)
                    .ok_or_else(|| EngineError::NotCallable(cur_ty.to_string()))?;
                let subst = unify(&arg_ty, applied_ty).map_err(|_| EngineError::NativeType {
                    expected: arg_ty.to_string(),
                    got: applied_ty.to_string(),
                })?;
                let rest_ty = rest_ty.apply(&subst);
                func = apply_async_with_gas(
                    engine,
                    func,
                    applied,
                    Some(&cur_ty),
                    Some(applied_ty),
                    gas,
                )
                .await?;
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
        imp.func.call_async(engine, full_ty, self.applied).await
    }
}

#[derive(Clone)]
struct NativeImpl {
    name: Symbol,
    arity: usize,
    scheme: Scheme,
    func: NativeCallable,
    gas_cost: u64,
}

impl NativeImpl {
    fn to_native_fn(&self, typ: Type) -> NativeFn {
        NativeFn::new(
            self.name.clone(),
            self.arity,
            typ,
            self.func.clone(),
            self.gas_cost,
        )
    }

    pub(crate) fn call_sync(
        &self,
        engine: &Engine,
        typ: &Type,
        args: &[Pointer],
    ) -> Result<Pointer, EngineError> {
        self.func.call_sync(engine, typ, args)
    }
}

#[derive(Default, Clone)]
struct NativeRegistry {
    entries: HashMap<Symbol, Vec<NativeImpl>>,
}

impl NativeRegistry {
    fn insert(&mut self, name: Symbol, imp: NativeImpl) -> Result<(), EngineError> {
        let entry = self.entries.entry(name.clone()).or_default();
        if entry.iter().any(|existing| existing.scheme == imp.scheme) {
            return Err(EngineError::DuplicateImpl {
                name,
                typ: imp.scheme.typ.to_string(),
            });
        }
        entry.push(imp);
        Ok(())
    }

    fn get(&self, name: &Symbol) -> Option<&[NativeImpl]> {
        self.entries.get(name).map(|v| v.as_slice())
    }

    fn has_name(&self, name: &Symbol) -> bool {
        self.entries.contains_key(name)
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

pub struct Engine {
    env: Env,
    natives: NativeRegistry,
    typeclasses: TypeclassRegistry,
    types: TypeSystem,
    typeclass_cache: Arc<Mutex<HashMap<(Symbol, Type), Pointer>>>,
    pub(crate) modules: ModuleSystem,
    cancel: CancellationToken,
    heap: Heap,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    pub fn new() -> Self {
        Self {
            env: Env::new(),
            natives: NativeRegistry::default(),
            typeclasses: TypeclassRegistry::default(),
            types: TypeSystem::new(),
            typeclass_cache: Arc::new(Mutex::new(HashMap::new())),
            modules: ModuleSystem::default(),
            cancel: CancellationToken::new(),
            heap: Heap::new(),
        }
    }

    pub(crate) fn fresh_type_var(&mut self, name: Option<Symbol>) -> rex_ts::TypeVar {
        self.types.supply.fresh(name)
    }

    pub fn with_prelude() -> Result<Self, EngineError> {
        let types = TypeSystem::with_prelude()?;
        let mut engine = Engine {
            env: Env::new(),
            natives: NativeRegistry::default(),
            typeclasses: TypeclassRegistry::default(),
            types,
            typeclass_cache: Arc::new(Mutex::new(HashMap::new())),
            modules: ModuleSystem::default(),
            cancel: CancellationToken::new(),
            heap: Heap::new(),
        };
        engine.inject_prelude()?;
        Ok(engine)
    }

    pub fn into_heap(self) -> Heap {
        self.heap
    }

    pub fn heap(&self) -> &Heap {
        &self.heap
    }

    /// Inject `debug`/`info`/`warn`/`error` logging functions backed by `tracing`.
    ///
    /// Each function has the Rex type `a -> str where Pretty a` and logs
    /// `pretty x` at the corresponding level, returning the rendered string.
    pub fn inject_tracing_log_functions(&mut self) -> Result<(), EngineError> {
        let string = Type::con("string", 0);

        let make_scheme = |engine: &mut Engine| {
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
        self.inject_native_scheme_typed(name, scheme, 1, move |engine, call_type, args| {
            if args.len() != 1 {
                return Err(EngineError::NativeArity {
                    name: name_sym.clone(),
                    expected: 1,
                    got: args.len(),
                });
            }

            let (arg_ty, _ret_ty) = split_fun(call_type)
                .ok_or_else(|| EngineError::NotCallable(call_type.to_string()))?;
            let pretty_ty = Type::fun(arg_ty.clone(), Type::con("string", 0));
            let pretty_ptr = engine.resolve_class_method(&sym("pretty"), &pretty_ty)?;
            let rendered_ptr = apply(
                engine,
                pretty_ptr,
                args[0].clone(),
                Some(&pretty_ty),
                Some(&arg_ty),
            )?;
            let message = engine.heap().pointer_as_string(&rendered_ptr)?;

            log(&message);
            engine.heap().alloc_string(message)
        })
    }

    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    pub fn inject_value<V: IntoPointer + RexType>(
        &mut self,
        name: &str,
        value: V,
    ) -> Result<(), EngineError> {
        let name = normalize_name(name);
        let typ = V::rex_type();
        let value = value.into_pointer(self.heap())?;
        let func =
            Arc::new(move |_engine: &Engine, _typ: &Type, _args: &[Pointer]| Ok(value.clone()));
        let scheme = Scheme::new(vec![], vec![], typ);
        self.register_native(name, scheme, 0, NativeCallable::Sync(func), 0)
    }

    pub fn inject_value_typed(
        &mut self,
        name: &str,
        typ: Type,
        value: Value,
    ) -> Result<(), EngineError> {
        let name = normalize_name(name);
        let value = self.heap().alloc_value(value)?;
        let func =
            Arc::new(move |_engine: &Engine, _typ: &Type, _args: &[Pointer]| Ok(value.clone()));
        let scheme = Scheme::new(vec![], vec![], typ);
        self.register_native(name, scheme, 0, NativeCallable::Sync(func), 0)
    }

    pub fn inject_fn0<F, R>(&mut self, name: &str, f: F) -> Result<(), EngineError>
    where
        F: Fn() -> R + Send + Sync + 'static,
        R: IntoPointer + RexType,
    {
        let name = normalize_name(name);
        let name_string = name.clone();
        let name_for_fn = name.clone();
        let func = Arc::new(move |engine: &Engine, _typ: &Type, args: &[Pointer]| {
            if !args.is_empty() {
                return Err(EngineError::NativeArity {
                    name: name_for_fn.clone(),
                    expected: 0,
                    got: args.len(),
                });
            }
            f().into_pointer(engine.heap())
        });
        let typ = R::rex_type();
        let scheme = Scheme::new(vec![], vec![], typ);
        self.register_native(name_string, scheme, 0, NativeCallable::Sync(func), 0)
    }

    pub fn inject_fn1<F, A, R>(&mut self, name: &str, f: F) -> Result<(), EngineError>
    where
        F: Fn(A) -> R + Send + Sync + 'static,
        A: FromPointer + RexType,
        R: IntoPointer + RexType,
    {
        let name = normalize_name(name);
        let name_string = name.clone();
        let func = Arc::new(move |engine: &Engine, _typ: &Type, args: &[Pointer]| {
            if args.len() != 1 {
                return Err(EngineError::NativeArity {
                    name: name_string.clone(),
                    expected: 1,
                    got: args.len(),
                });
            }
            let a = A::from_pointer(engine.heap(), &args[0])?;
            f(a).into_pointer(engine.heap())
        });
        let typ = Type::fun(A::rex_type(), R::rex_type());
        let scheme = Scheme::new(vec![], vec![], typ);
        self.register_native(name, scheme, 1, NativeCallable::Sync(func), 0)
    }

    pub fn inject_fn2<F, A, B, R>(&mut self, name: &str, f: F) -> Result<(), EngineError>
    where
        F: Fn(A, B) -> R + Send + Sync + 'static,
        A: FromPointer + RexType,
        B: FromPointer + RexType,
        R: IntoPointer + RexType,
    {
        let name = normalize_name(name);
        let name_string = name.clone();
        let func = Arc::new(move |engine: &Engine, _typ: &Type, args: &[Pointer]| {
            if args.len() != 2 {
                return Err(EngineError::NativeArity {
                    name: name_string.clone(),
                    expected: 2,
                    got: args.len(),
                });
            }
            let a = A::from_pointer(engine.heap(), &args[0])?;
            let b = B::from_pointer(engine.heap(), &args[1])?;
            f(a, b).into_pointer(engine.heap())
        });
        let typ = Type::fun(A::rex_type(), Type::fun(B::rex_type(), R::rex_type()));
        let scheme = Scheme::new(vec![], vec![], typ);
        self.register_native(name, scheme, 2, NativeCallable::Sync(func), 0)
    }

    pub fn inject_async_fn0<F, Fut, R>(&mut self, name: &str, f: F) -> Result<(), EngineError>
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = R> + Send + 'static,
        R: IntoPointer + RexType,
    {
        let name = normalize_name(name);
        let name_sym = name.clone();
        let f = Arc::new(f);
        let func: AsyncNativeCallable = Arc::new(
            move |engine: &Engine, _typ: Type, args: Vec<Pointer>| -> NativeFuture<'_> {
                let f = f.clone();
                let name_sym = name_sym.clone();
                async move {
                    if !args.is_empty() {
                        return Err(EngineError::NativeArity {
                            name: name_sym.clone(),
                            expected: 0,
                            got: args.len(),
                        });
                    }
                    f().await.into_pointer(engine.heap())
                }
                .boxed()
            },
        );
        let typ = R::rex_type();
        let scheme = Scheme::new(vec![], vec![], typ);
        self.register_native(name, scheme, 0, NativeCallable::Async(func), 0)
    }

    pub fn inject_async_fn1<F, Fut, A, R>(&mut self, name: &str, f: F) -> Result<(), EngineError>
    where
        F: Fn(A) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = R> + Send + 'static,
        A: FromPointer + RexType,
        R: IntoPointer + RexType,
    {
        let name = normalize_name(name);
        let name_sym = name.clone();
        let f = Arc::new(f);
        let func: AsyncNativeCallable = Arc::new(
            move |engine: &Engine, _typ: Type, args: Vec<Pointer>| -> NativeFuture<'_> {
                let f = f.clone();
                let name_sym = name_sym.clone();
                async move {
                    if args.len() != 1 {
                        return Err(EngineError::NativeArity {
                            name: name_sym.clone(),
                            expected: 1,
                            got: args.len(),
                        });
                    }
                    let a = A::from_pointer(engine.heap(), &args[0])?;
                    f(a).await.into_pointer(engine.heap())
                }
                .boxed()
            },
        );
        let typ = Type::fun(A::rex_type(), R::rex_type());
        let scheme = Scheme::new(vec![], vec![], typ);
        self.register_native(name, scheme, 1, NativeCallable::Async(func), 0)
    }

    pub fn inject_async_fn2<F, Fut, A, B, R>(&mut self, name: &str, f: F) -> Result<(), EngineError>
    where
        F: Fn(A, B) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = R> + Send + 'static,
        A: FromPointer + RexType,
        B: FromPointer + RexType,
        R: IntoPointer + RexType,
    {
        let name = normalize_name(name);
        let name_sym = name.clone();
        let f = Arc::new(f);
        let func: AsyncNativeCallable = Arc::new(
            move |engine: &Engine, _typ: Type, args: Vec<Pointer>| -> NativeFuture<'_> {
                let f = f.clone();
                let name_sym = name_sym.clone();
                async move {
                    if args.len() != 2 {
                        return Err(EngineError::NativeArity {
                            name: name_sym.clone(),
                            expected: 2,
                            got: args.len(),
                        });
                    }
                    let a = A::from_pointer(engine.heap(), &args[0])?;
                    let b = B::from_pointer(engine.heap(), &args[1])?;
                    f(a, b).await.into_pointer(engine.heap())
                }
                .boxed()
            },
        );
        let typ = Type::fun(A::rex_type(), Type::fun(B::rex_type(), R::rex_type()));
        let scheme = Scheme::new(vec![], vec![], typ);
        self.register_native(name, scheme, 2, NativeCallable::Async(func), 0)
    }

    pub fn inject_async_fn0_cancellable<F, Fut, R>(
        &mut self,
        name: &str,
        f: F,
    ) -> Result<(), EngineError>
    where
        F: Fn(CancellationToken) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = R> + Send + 'static,
        R: IntoPointer + RexType,
    {
        let name = normalize_name(name);
        let name_sym = name.clone();
        let f = Arc::new(f);
        let func: AsyncNativeCallableCancellable = Arc::new(
            move |engine: &Engine,
                  token: CancellationToken,
                  _typ: Type,
                  args: Vec<Pointer>|
                  -> NativeFuture<'_> {
                let f = f.clone();
                let name_sym = name_sym.clone();
                async move {
                    if !args.is_empty() {
                        return Err(EngineError::NativeArity {
                            name: name_sym.clone(),
                            expected: 0,
                            got: args.len(),
                        });
                    }
                    f(token).await.into_pointer(engine.heap())
                }
                .boxed()
            },
        );
        let typ = R::rex_type();
        let scheme = Scheme::new(vec![], vec![], typ);
        self.register_native(name, scheme, 0, NativeCallable::AsyncCancellable(func), 0)
    }

    pub fn inject_async_fn1_cancellable<F, Fut, A, R>(
        &mut self,
        name: &str,
        f: F,
    ) -> Result<(), EngineError>
    where
        F: Fn(CancellationToken, A) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = R> + Send + 'static,
        A: FromPointer + RexType,
        R: IntoPointer + RexType,
    {
        let name = normalize_name(name);
        let name_sym = name.clone();
        let f = Arc::new(f);
        let func: AsyncNativeCallableCancellable = Arc::new(
            move |engine: &Engine,
                  token: CancellationToken,
                  _typ: Type,
                  args: Vec<Pointer>|
                  -> NativeFuture<'_> {
                let f = f.clone();
                let name_sym = name_sym.clone();
                async move {
                    if args.len() != 1 {
                        return Err(EngineError::NativeArity {
                            name: name_sym.clone(),
                            expected: 1,
                            got: args.len(),
                        });
                    }
                    let a = A::from_pointer(engine.heap(), &args[0])?;
                    f(token, a).await.into_pointer(engine.heap())
                }
                .boxed()
            },
        );
        let typ = Type::fun(A::rex_type(), R::rex_type());
        let scheme = Scheme::new(vec![], vec![], typ);
        self.register_native(name, scheme, 1, NativeCallable::AsyncCancellable(func), 0)
    }

    pub fn inject_async_fn2_cancellable<F, Fut, A, B, R>(
        &mut self,
        name: &str,
        f: F,
    ) -> Result<(), EngineError>
    where
        F: Fn(CancellationToken, A, B) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = R> + Send + 'static,
        A: FromPointer + RexType,
        B: FromPointer + RexType,
        R: IntoPointer + RexType,
    {
        let name = normalize_name(name);
        let name_sym = name.clone();
        let f = Arc::new(f);
        let func: AsyncNativeCallableCancellable = Arc::new(
            move |engine: &Engine,
                  token: CancellationToken,
                  _typ: Type,
                  args: Vec<Pointer>|
                  -> NativeFuture<'_> {
                let f = f.clone();
                let name_sym = name_sym.clone();
                async move {
                    if args.len() != 2 {
                        return Err(EngineError::NativeArity {
                            name: name_sym.clone(),
                            expected: 2,
                            got: args.len(),
                        });
                    }
                    let a = A::from_pointer(engine.heap(), &args[0])?;
                    let b = B::from_pointer(engine.heap(), &args[1])?;
                    f(token, a, b).await.into_pointer(engine.heap())
                }
                .boxed()
            },
        );
        let typ = Type::fun(A::rex_type(), Type::fun(B::rex_type(), R::rex_type()));
        let scheme = Scheme::new(vec![], vec![], typ);
        self.register_native(name, scheme, 2, NativeCallable::AsyncCancellable(func), 0)
    }

    pub fn inject_native<F>(
        &mut self,
        name: &str,
        typ: Type,
        arity: usize,
        func: F,
    ) -> Result<(), EngineError>
    where
        F: Fn(&Engine, &[Pointer]) -> Result<Pointer, EngineError> + Send + Sync + 'static,
    {
        let name = normalize_name(name);
        let scheme = Scheme::new(vec![], vec![], typ);
        let func =
            Arc::new(move |engine: &Engine, _typ: &Type, args: &[Pointer]| func(engine, args));
        self.register_native(name, scheme, arity, NativeCallable::Sync(func), 0)
    }

    pub fn inject_native_scheme<F>(
        &mut self,
        name: &str,
        scheme: Scheme,
        arity: usize,
        func: F,
    ) -> Result<(), EngineError>
    where
        F: Fn(&Engine, &[Pointer]) -> Result<Pointer, EngineError> + Send + Sync + 'static,
    {
        let name = normalize_name(name);
        let func =
            Arc::new(move |engine: &Engine, _typ: &Type, args: &[Pointer]| func(engine, args));
        self.register_native(name, scheme, arity, NativeCallable::Sync(func), 0)
    }

    pub fn inject_native_scheme_typed<F>(
        &mut self,
        name: &str,
        scheme: Scheme,
        arity: usize,
        func: F,
    ) -> Result<(), EngineError>
    where
        F: Fn(&Engine, &Type, &[Pointer]) -> Result<Pointer, EngineError> + Send + Sync + 'static,
    {
        let name = normalize_name(name);
        let func =
            Arc::new(move |engine: &Engine, typ: &Type, args: &[Pointer]| func(engine, typ, args));
        self.register_native(name, scheme, arity, NativeCallable::Sync(func), 0)
    }

    pub fn inject_native_scheme_typed_value<F>(
        &mut self,
        name: &str,
        scheme: Scheme,
        arity: usize,
        func: F,
    ) -> Result<(), EngineError>
    where
        F: Fn(&Engine, &Type, &[Pointer]) -> Result<Pointer, EngineError> + Send + Sync + 'static,
    {
        self.inject_native_scheme_typed(name, scheme, arity, func)
    }

    pub fn inject_native_scheme_typed_with_gas_cost<F>(
        &mut self,
        name: &str,
        scheme: Scheme,
        arity: usize,
        gas_cost: u64,
        func: F,
    ) -> Result<(), EngineError>
    where
        F: Fn(&Engine, &Type, &[Pointer]) -> Result<Pointer, EngineError> + Send + Sync + 'static,
    {
        let name = normalize_name(name);
        let func =
            Arc::new(move |engine: &Engine, typ: &Type, args: &[Pointer]| func(engine, typ, args));
        self.register_native(name, scheme, arity, NativeCallable::Sync(func), gas_cost)
    }

    pub fn inject_native_scheme_typed_async<F>(
        &mut self,
        name: &str,
        scheme: Scheme,
        arity: usize,
        func: F,
    ) -> Result<(), EngineError>
    where
        F: for<'a> Fn(&'a Engine, Type, Vec<Pointer>) -> NativeFuture<'a> + Send + Sync + 'static,
    {
        let name = normalize_name(name);
        let func: AsyncNativeCallable = Arc::new(move |engine, typ, args| func(engine, typ, args));
        self.register_native(name, scheme, arity, NativeCallable::Async(func), 0)
    }

    pub fn inject_native_scheme_typed_async_with_gas_cost<F>(
        &mut self,
        name: &str,
        scheme: Scheme,
        arity: usize,
        gas_cost: u64,
        func: F,
    ) -> Result<(), EngineError>
    where
        F: for<'a> Fn(&'a Engine, Type, Vec<Pointer>) -> NativeFuture<'a> + Send + Sync + 'static,
    {
        let name = normalize_name(name);
        let func: AsyncNativeCallable = Arc::new(move |engine, typ, args| func(engine, typ, args));
        self.register_native(name, scheme, arity, NativeCallable::Async(func), gas_cost)
    }

    pub fn inject_native_scheme_typed_async_cancellable<F>(
        &mut self,
        name: &str,
        scheme: Scheme,
        arity: usize,
        func: F,
    ) -> Result<(), EngineError>
    where
        F: for<'a> Fn(&'a Engine, CancellationToken, Type, Vec<Pointer>) -> NativeFuture<'a>
            + Send
            + Sync
            + 'static,
    {
        let name = normalize_name(name);
        let func: AsyncNativeCallableCancellable =
            Arc::new(move |engine, token, typ, args| func(engine, token, typ, args));
        self.register_native(
            name,
            scheme,
            arity,
            NativeCallable::AsyncCancellable(func),
            0,
        )
    }

    pub fn inject_native_scheme_typed_async_cancellable_with_gas_cost<F>(
        &mut self,
        name: &str,
        scheme: Scheme,
        arity: usize,
        gas_cost: u64,
        func: F,
    ) -> Result<(), EngineError>
    where
        F: for<'a> Fn(&'a Engine, CancellationToken, Type, Vec<Pointer>) -> NativeFuture<'a>
            + Send
            + Sync
            + 'static,
    {
        let name = normalize_name(name);
        let func: AsyncNativeCallableCancellable =
            Arc::new(move |engine, token, typ, args| func(engine, token, typ, args));
        self.register_native(
            name,
            scheme,
            arity,
            NativeCallable::AsyncCancellable(func),
            gas_cost,
        )
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
            let func = Arc::new(move |engine: &Engine, _typ: &Type, args: &[Pointer]| {
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
        // First, register the generalized scheme in the type environment so
        // later declarations (including instance method bodies) can typecheck.
        self.types.inject_fn_decl(decl).map_err(EngineError::Type)?;

        // Then, evaluate the lowered lambda and stash the runtime value in the
        // global environment. This makes function values visible to instance
        // methods without relying on call-site environments.
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
        let ptr = eval_typed_expr(self, &self.env, &typed)?;
        self.env = self.env.extend(decl.name.name.clone(), ptr);
        Ok(())
    }

    pub fn inject_decls(&mut self, decls: &[Decl]) -> Result<(), EngineError> {
        for decl in decls {
            match decl {
                Decl::Type(ty) => self.inject_type_decl(ty)?,
                Decl::Class(class_decl) => self.inject_class_decl(class_decl)?,
                Decl::Instance(inst_decl) => self.inject_instance_decl(inst_decl)?,
                Decl::Fn(fd) => self.inject_fn_decl(fd)?,
                Decl::DeclareFn(df) => {
                    self.types
                        .inject_declare_fn_decl(df)
                        .map_err(EngineError::Type)?;
                }
                Decl::Import(..) => {}
            }
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

    pub fn eval(&mut self, expr: &Expr) -> Result<Pointer, EngineError> {
        self.eval_inner(expr)
    }

    pub async fn eval_async(&mut self, expr: &Expr) -> Result<Pointer, EngineError> {
        check_cancelled(self)?;
        let typed = self.type_check(expr)?;
        eval_typed_expr_async(self, &self.env, &typed).await
    }

    pub fn eval_with_gas(
        &mut self,
        expr: &Expr,
        gas: &mut GasMeter,
    ) -> Result<Pointer, EngineError> {
        check_cancelled(self)?;
        let typed = self.type_check(expr)?;
        eval_typed_expr_with_gas(self, &self.env, &typed, gas)
    }

    pub async fn eval_async_with_gas(
        &mut self,
        expr: &Expr,
        gas: &mut GasMeter,
    ) -> Result<Pointer, EngineError> {
        check_cancelled(self)?;
        let typed = self.type_check(expr)?;
        eval_typed_expr_async_with_gas(self, &self.env, &typed, gas).await
    }

    pub fn eval_with_stack_size(
        &mut self,
        expr: &Expr,
        stack_size: usize,
    ) -> Result<Pointer, EngineError> {
        crate::stack::run_with_stack_size(stack_size, || self.eval_inner(expr))?
    }

    fn eval_inner(&mut self, expr: &Expr) -> Result<Pointer, EngineError> {
        check_cancelled(self)?;
        let typed = self.type_check(expr)?;
        eval_typed_expr(self, &self.env, &typed)
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
        func: NativeCallable,
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
        let imp = NativeImpl {
            name: name.clone(),
            arity,
            scheme,
            func,
            gas_cost,
        };
        self.natives.insert(name, imp)
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

    pub(crate) fn infer_type_with_gas(
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

    fn resolve_class_method(&self, name: &Symbol, typ: &Type) -> Result<Pointer, EngineError> {
        if typ.ftv().is_empty()
            && let Ok(cache) = self.typeclass_cache.lock()
            && let Some(pointer) = cache.get(&(name.clone(), typ.clone()))
        {
            return Ok(pointer.clone());
        }

        let (def_env, typed, s) = match self.resolve_typeclass_method_impl(name, typ) {
            Ok(res) => res,
            Err(EngineError::AmbiguousOverload { .. }) if is_function_type(typ) => {
                let (name, typ, applied, applied_types) =
                    OverloadedFn::new(name.clone(), typ.clone()).into_parts();
                return self
                    .heap()
                    .alloc_overloaded(name, typ, applied, applied_types);
            }
            Err(err) => return Err(err),
        };
        let specialized = typed.as_ref().apply(&s);
        let pointer = eval_typed_expr(self, &def_env, &specialized)?;

        if typ.ftv().is_empty()
            && let Ok(mut cache) = self.typeclass_cache.lock()
        {
            cache.insert((name.clone(), typ.clone()), pointer.clone());
        }
        Ok(pointer)
    }

    fn resolve_class_method_with_gas(
        &self,
        name: &Symbol,
        typ: &Type,
        gas: &mut GasMeter,
    ) -> Result<Pointer, EngineError> {
        let (def_env, typed, s) = match self.resolve_typeclass_method_impl(name, typ) {
            Ok(res) => res,
            Err(EngineError::AmbiguousOverload { .. }) if is_function_type(typ) => {
                let (name, typ, applied, applied_types) =
                    OverloadedFn::new(name.clone(), typ.clone()).into_parts();
                return self
                    .heap()
                    .alloc_overloaded(name, typ, applied, applied_types);
            }
            Err(err) => return Err(err),
        };
        let specialized = typed.as_ref().apply(&s);
        eval_typed_expr_with_gas(self, &def_env, &specialized, gas)
    }

    pub(crate) fn resolve_global(&self, name: &Symbol, typ: &Type) -> Result<Pointer, EngineError> {
        if let Some(ptr) = self.env.get(name) {
            let value = self.heap().get(&ptr)?;
            match value.as_ref() {
                Value::Native(native) if native.arity == 0 && native.applied.is_empty() => {
                    native.call_zero(self)
                }
                _ => Ok(ptr),
            }
        } else if self.types.class_methods.contains_key(name) {
            self.resolve_class_method(name, typ)
        } else {
            let pointer = self.resolve_native(name.as_ref(), typ)?;
            let value = self.heap().get(&pointer)?;
            match value.as_ref() {
                Value::Native(native) if native.arity == 0 && native.applied.is_empty() => {
                    native.call_zero(self)
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

    fn resolve_native_impl(&self, name: &str, typ: &Type) -> Result<NativeImpl, EngineError> {
        let sym_name = sym(name);
        let impls = self
            .natives
            .get(&sym_name)
            .ok_or_else(|| EngineError::UnknownVar(sym_name.clone()))?;
        let matches: Vec<NativeImpl> = impls
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

    pub(crate) fn call_native_impl_sync(
        &self,
        name: &str,
        typ: &Type,
        args: &[Pointer],
    ) -> Result<Pointer, EngineError> {
        let imp = self.resolve_native_impl(name, typ)?;
        imp.call_sync(self, typ, args)
    }

    fn resolve_native(&self, name: &str, typ: &Type) -> Result<Pointer, EngineError> {
        let sym_name = sym(name);
        let impls = self
            .natives
            .get(&sym_name)
            .ok_or_else(|| EngineError::UnknownVar(sym_name.clone()))?;
        let matches: Vec<NativeImpl> = impls
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
                    name,
                    arity,
                    typ,
                    func,
                    gas_cost,
                    applied,
                    applied_types,
                } = imp.to_native_fn(typ.clone());
                self.heap()
                    .alloc_native(name, arity, typ, func, gas_cost, applied, applied_types)
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

    fn resolve_native_with_gas(
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
        let matches: Vec<NativeImpl> = impls
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
                    name,
                    arity,
                    typ,
                    func,
                    gas_cost,
                    applied,
                    applied_types,
                } = imp.to_native_fn(typ.clone());
                self.heap()
                    .alloc_native(name, arity, typ, func, gas_cost, applied, applied_types)
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

fn default_ambiguous_types(
    engine: &Engine,
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

fn choose_default_type(
    engine: &Engine,
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

fn impl_matches_type(imp: &NativeImpl, typ: &Type) -> bool {
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

fn eval_typed_expr(engine: &Engine, env: &Env, expr: &TypedExpr) -> Result<Pointer, EngineError> {
    let costs = GasCosts::sensible_defaults();
    let mut gas = GasMeter::unlimited(costs);
    eval_typed_expr_with_gas(engine, env, expr, &mut gas)
}

pub(crate) fn apply(
    engine: &Engine,
    func: Pointer,
    arg: Pointer,
    func_type: Option<&Type>,
    arg_type: Option<&Type>,
) -> Result<Pointer, EngineError> {
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
            eval_typed_expr(engine, &env, &body)
        }
        Value::Native(native) => native.apply(engine, arg, arg_type),
        Value::Overloaded(over) => over.apply(engine, arg, func_type, arg_type),
        _ => Err(EngineError::NotCallable(
            engine.heap().type_name(&func)?.into(),
        )),
    }
}

impl NativeFn {
    fn call_zero_with_gas(
        &self,
        engine: &Engine,
        gas: &mut GasMeter,
    ) -> Result<Pointer, EngineError> {
        let amount = gas
            .costs
            .native_call_base
            .saturating_add(self.gas_cost)
            .saturating_add(gas.costs.native_call_per_arg.saturating_mul(0));
        gas.charge(amount)?;
        self.call_zero(engine)
    }

    fn apply_with_gas(
        mut self,
        engine: &Engine,
        arg: Pointer,
        arg_type: Option<&Type>,
        gas: &mut GasMeter,
    ) -> Result<Pointer, EngineError> {
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
                name,
                arity,
                typ,
                func,
                gas_cost,
                applied,
                applied_types,
            } = self;
            return engine.heap().alloc_native(
                name,
                arity,
                typ,
                func,
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
        self.func.call_sync(engine, &full_ty, &self.applied)
    }

    async fn call_zero_async_with_gas(
        &self,
        engine: &Engine,
        gas: &mut GasMeter,
    ) -> Result<Pointer, EngineError> {
        let amount = gas
            .costs
            .native_call_base
            .saturating_add(self.gas_cost)
            .saturating_add(gas.costs.native_call_per_arg.saturating_mul(0));
        gas.charge(amount)?;
        self.call_zero_async(engine).await
    }

    async fn apply_async_with_gas(
        mut self,
        engine: &Engine,
        arg: Pointer,
        arg_type: Option<&Type>,
        gas: &mut GasMeter,
    ) -> Result<Pointer, EngineError> {
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
                name,
                arity,
                typ,
                func,
                gas_cost,
                applied,
                applied_types,
            } = self;
            return engine.heap().alloc_native(
                name,
                arity,
                typ,
                func,
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
        self.func.call_async(engine, full_ty, self.applied).await
    }
}

fn eval_typed_expr_with_gas(
    engine: &Engine,
    env: &Env,
    expr: &TypedExpr,
    gas: &mut GasMeter,
) -> Result<Pointer, EngineError> {
    check_cancelled(engine)?;
    let mut env = env.clone();
    let mut cur = expr;
    loop {
        check_cancelled(engine)?;
        match &cur.kind {
            TypedExprKind::Let { name, def, body } => {
                gas.charge(gas.costs.eval_node)?;
                let ptr = eval_typed_expr_with_gas(engine, &env, def, gas)?;
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
                values.push(eval_typed_expr_with_gas(engine, &env, elem, gas)?);
            }
            engine.heap().alloc_tuple(values)
        }
        TypedExprKind::List(elems) => {
            let mut values = Vec::with_capacity(elems.len());
            for elem in elems {
                check_cancelled(engine)?;
                values.push(eval_typed_expr_with_gas(engine, &env, elem, gas)?);
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
                out.insert(k.clone(), eval_typed_expr_with_gas(engine, &env, v, gas)?);
            }
            engine.heap().alloc_dict(out)
        }
        TypedExprKind::RecordUpdate { base, updates } => {
            let base_ptr = eval_typed_expr_with_gas(engine, &env, base, gas)?;
            let mut update_vals = BTreeMap::new();
            for (k, v) in updates {
                check_cancelled(engine)?;
                update_vals.insert(k.clone(), eval_typed_expr_with_gas(engine, &env, v, gas)?);
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
                        native.call_zero_with_gas(engine, gas)
                    }
                    _ => Ok(ptr),
                }
            } else if engine.types.class_methods.contains_key(name) {
                engine.resolve_class_method_with_gas(name, &cur.typ, gas)
            } else {
                let value = engine.resolve_native_with_gas(name.as_ref(), &cur.typ, gas)?;
                match engine.heap().get(&value)?.as_ref() {
                    Value::Native(native) if native.arity == 0 && native.applied.is_empty() => {
                        native.call_zero_with_gas(engine, gas)
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

            let mut func = eval_typed_expr_with_gas(engine, &env, head, gas)?;
            for (func_type, arg_expr) in spine {
                check_cancelled(engine)?;
                gas.charge(gas.costs.eval_app_step)?;
                let arg = eval_typed_expr_with_gas(engine, &env, arg_expr, gas)?;
                func = apply_with_gas(
                    engine,
                    func,
                    arg,
                    Some(&func_type),
                    Some(&arg_expr.typ),
                    gas,
                )?;
            }
            Ok(func)
        }
        TypedExprKind::Project { expr, field } => {
            let value = eval_typed_expr_with_gas(engine, &env, expr, gas)?;
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
                let def_ptr = eval_typed_expr_with_gas(engine, &env_rec, def, gas)?;
                let def_value = engine.heap().get(&def_ptr)?;
                engine.heap().overwrite(slot, def_value.as_ref().clone())?;
            }

            eval_typed_expr_with_gas(engine, &env_rec, body, gas)
        }
        TypedExprKind::Ite {
            cond,
            then_expr,
            else_expr,
        } => {
            let cond_ptr = eval_typed_expr_with_gas(engine, &env, cond, gas)?;
            match engine.heap().pointer_as_bool(&cond_ptr) {
                Ok(true) => eval_typed_expr_with_gas(engine, &env, then_expr, gas),
                Ok(false) => eval_typed_expr_with_gas(engine, &env, else_expr, gas),
                Err(EngineError::NativeType { got, .. }) => Err(EngineError::ExpectedBool(got)),
                Err(err) => Err(err),
            }
        }
        TypedExprKind::Match { scrutinee, arms } => {
            let value = eval_typed_expr_with_gas(engine, &env, scrutinee, gas)?;
            for (pat, expr) in arms {
                check_cancelled(engine)?;
                gas.charge(gas.costs.eval_match_arm)?;
                if let Some(bindings) = match_pattern_ptr(engine.heap(), pat, &value) {
                    let env = env.extend_many(bindings);
                    return eval_typed_expr_with_gas(engine, &env, expr, gas);
                }
            }
            Err(EngineError::MatchFailure)
        }
        TypedExprKind::Let { .. } => {
            unreachable!("let chain handled in eval_typed_expr_with_gas loop")
        }
    }
}

fn apply_with_gas(
    engine: &Engine,
    func: Pointer,
    arg: Pointer,
    func_type: Option<&Type>,
    arg_type: Option<&Type>,
    gas: &mut GasMeter,
) -> Result<Pointer, EngineError> {
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
            eval_typed_expr_with_gas(engine, &env, &body, gas)
        }
        Value::Native(native) => native.apply_with_gas(engine, arg, arg_type, gas),
        Value::Overloaded(over) => over.apply_with_gas(engine, arg, func_type, arg_type, gas),
        _ => Err(EngineError::NotCallable(
            engine.heap().type_name(&func)?.into(),
        )),
    }
}

#[async_recursion]
async fn eval_typed_expr_async(
    engine: &Engine,
    env: &Env,
    expr: &TypedExpr,
) -> Result<Pointer, EngineError> {
    let costs = GasCosts::sensible_defaults();
    let mut gas = GasMeter::unlimited(costs);
    eval_typed_expr_async_with_gas(engine, env, expr, &mut gas).await
}

#[async_recursion]
async fn eval_typed_expr_async_with_gas(
    engine: &Engine,
    env: &Env,
    expr: &TypedExpr,
    gas: &mut GasMeter,
) -> Result<Pointer, EngineError> {
    check_cancelled(engine)?;
    let mut env = env.clone();
    let mut cur = expr;
    loop {
        check_cancelled(engine)?;
        match &cur.kind {
            TypedExprKind::Let { name, def, body } => {
                gas.charge(gas.costs.eval_node)?;
                let ptr = eval_typed_expr_async_with_gas(engine, &env, def, gas).await?;
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
                values.push(eval_typed_expr_async_with_gas(engine, &env, elem, gas).await?);
            }
            engine.heap().alloc_tuple(values)
        }
        TypedExprKind::List(elems) => {
            let mut values = Vec::with_capacity(elems.len());
            for elem in elems {
                check_cancelled(engine)?;
                values.push(eval_typed_expr_async_with_gas(engine, &env, elem, gas).await?);
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
                out.insert(
                    k.clone(),
                    eval_typed_expr_async_with_gas(engine, &env, v, gas).await?,
                );
            }
            engine.heap().alloc_dict(out)
        }
        TypedExprKind::RecordUpdate { base, updates } => {
            let base_ptr = eval_typed_expr_async_with_gas(engine, &env, base, gas).await?;
            let mut update_vals = BTreeMap::new();
            for (k, v) in updates {
                check_cancelled(engine)?;
                update_vals.insert(
                    k.clone(),
                    eval_typed_expr_async_with_gas(engine, &env, v, gas).await?,
                );
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
                        native.call_zero_async_with_gas(engine, gas).await
                    }
                    _ => Ok(ptr),
                }
            } else if engine.types.class_methods.contains_key(name) {
                engine.resolve_class_method_with_gas(name, &cur.typ, gas)
            } else {
                let value = engine.resolve_native_with_gas(name.as_ref(), &cur.typ, gas)?;
                match engine.heap().get(&value)?.as_ref() {
                    Value::Native(native) if native.arity == 0 && native.applied.is_empty() => {
                        native.call_zero_async_with_gas(engine, gas).await
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

            let mut func = eval_typed_expr_async_with_gas(engine, &env, head, gas).await?;
            for (func_type, arg_expr) in spine {
                check_cancelled(engine)?;
                gas.charge(gas.costs.eval_app_step)?;
                let arg = eval_typed_expr_async_with_gas(engine, &env, arg_expr, gas).await?;
                func = apply_async_with_gas(
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
            let value = eval_typed_expr_async_with_gas(engine, &env, expr, gas).await?;
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
                let def_ptr = eval_typed_expr_async_with_gas(engine, &env_rec, def, gas).await?;
                let def_value = engine.heap().get(&def_ptr)?;
                engine.heap().overwrite(slot, def_value.as_ref().clone())?;
            }

            eval_typed_expr_async_with_gas(engine, &env_rec, body, gas).await
        }
        TypedExprKind::Ite {
            cond,
            then_expr,
            else_expr,
        } => {
            let cond_ptr = eval_typed_expr_async_with_gas(engine, &env, cond, gas).await?;
            match engine.heap().pointer_as_bool(&cond_ptr) {
                Ok(true) => eval_typed_expr_async_with_gas(engine, &env, then_expr, gas).await,
                Ok(false) => eval_typed_expr_async_with_gas(engine, &env, else_expr, gas).await,
                Err(EngineError::NativeType { got, .. }) => Err(EngineError::ExpectedBool(got)),
                Err(err) => Err(err),
            }
        }
        TypedExprKind::Match { scrutinee, arms } => {
            let value = eval_typed_expr_async_with_gas(engine, &env, scrutinee, gas).await?;
            for (pat, expr) in arms {
                check_cancelled(engine)?;
                gas.charge(gas.costs.eval_match_arm)?;
                if let Some(bindings) = match_pattern_ptr(engine.heap(), pat, &value) {
                    let env = env.extend_many(bindings);
                    return eval_typed_expr_async_with_gas(engine, &env, expr, gas).await;
                }
            }
            Err(EngineError::MatchFailure)
        }
        TypedExprKind::Let { .. } => {
            unreachable!("let chain handled in eval_typed_expr_async_with_gas loop")
        }
    }
}

#[async_recursion]
async fn apply_async_with_gas(
    engine: &Engine,
    func: Pointer,
    arg: Pointer,
    func_type: Option<&Type>,
    arg_type: Option<&Type>,
    gas: &mut GasMeter,
) -> Result<Pointer, EngineError> {
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
            eval_typed_expr_async_with_gas(engine, &env, &body, gas).await
        }
        Value::Native(native) => {
            native
                .apply_async_with_gas(engine, arg, arg_type, gas)
                .await
        }
        Value::Overloaded(over) => {
            over.apply_async_with_gas(engine, arg, func_type, arg_type, gas)
                .await
        }
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
        parser.parse_program().unwrap().expr
    }

    fn parse_program(code: &str) -> rex_ast::expr::Program {
        let mut parser = rex_parser::Parser::new(rex_lexer::Token::tokenize(code).unwrap());
        parser.parse_program().unwrap()
    }

    fn strip_span(mut err: TypeError) -> TypeError {
        while let TypeError::Spanned { error, .. } = err {
            err = *error;
        }
        err
    }

    fn engine_with_arith() -> Engine {
        Engine::with_prelude().unwrap()
    }

    #[test]
    fn repl_persists_function_definitions() {
        let costs = GasCosts::sensible_defaults();
        let mut gas = GasMeter::unlimited(costs);
        let mut engine = Engine::with_prelude().unwrap();
        engine.add_default_resolvers();
        let mut state = ReplState::new();

        let program1 = parse_program("fn inc (x: i32) -> i32 = x + 1\ninc 1");
        let v1 = engine
            .eval_repl_program_with_gas(&program1, &mut state, &mut gas)
            .unwrap();
        let expected = engine.heap().alloc_i32(2).unwrap();
        assert!(crate::pointer_eq(engine.heap(), &v1, &expected).unwrap());

        let program2 = parse_program("inc 2");
        let v2 = engine
            .eval_repl_program_with_gas(&program2, &mut state, &mut gas)
            .unwrap();
        let expected = engine.heap().alloc_i32(3).unwrap();
        assert!(crate::pointer_eq(engine.heap(), &v2, &expected).unwrap());
    }

    #[test]
    fn repl_persists_import_aliases() {
        let costs = GasCosts::sensible_defaults();
        let mut gas = GasMeter::unlimited(costs);
        let mut engine = Engine::with_prelude().unwrap();
        engine.add_default_resolvers();

        let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../rex/examples/modules_basic");
        engine.add_include_resolver(&examples).unwrap();

        let mut state = ReplState::new();
        let program1 = parse_program("import foo.bar as Bar\n()");
        engine
            .eval_repl_program_with_gas(&program1, &mut state, &mut gas)
            .unwrap();

        let program2 = parse_program("Bar.triple 10");
        let v2 = engine
            .eval_repl_program_with_gas(&program2, &mut state, &mut gas)
            .unwrap();
        let expected = engine.heap().alloc_i32(30).unwrap();
        assert!(crate::pointer_eq(engine.heap(), &v2, &expected).unwrap());
    }

    #[test]
    fn sync_eval_can_be_cancelled_while_blocking_on_async_native() {
        let expr = parse("stall");
        let mut engine = Engine::with_prelude().unwrap();

        let (started_tx, started_rx) = std::sync::mpsc::channel::<()>();
        engine
            .inject_async_fn0_cancellable("stall", move |token: crate::CancellationToken| {
                let started_tx = started_tx.clone();
                async move {
                    let _ = started_tx.send(());
                    token.cancelled().await;
                    0i32
                }
            })
            .unwrap();

        let token = engine.cancellation_token();
        let handle = std::thread::spawn(move || engine.eval(expr.as_ref()));

        started_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("stall native never started");
        token.cancel();

        let res = handle.join().unwrap();
        assert!(matches!(res, Err(EngineError::Cancelled)));
    }

    #[test]
    fn native_per_impl_gas_cost_is_charged() {
        let expr = parse("foo");
        let mut engine = Engine::with_prelude().unwrap();
        let scheme = Scheme::new(vec![], vec![], Type::con("i32", 0));
        engine
            .inject_native_scheme_typed_with_gas_cost("foo", scheme, 0, 50, |engine, _t, _args| {
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
        let err = match engine.eval_with_gas(expr.as_ref(), &mut gas) {
            Ok(_) => panic!("expected out of gas"),
            Err(e) => e,
        };
        assert!(matches!(err, EngineError::OutOfGas(..)));
    }

    #[test]
    fn record_update_requires_known_variant_for_sum_types() {
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
        match engine.eval(program.expr.as_ref()) {
            Err(EngineError::Type(err)) => {
                let err = strip_span(err);
                assert!(matches!(err, TypeError::FieldNotKnown { .. }));
            }
            _ => panic!("expected type error"),
        }
    }
}

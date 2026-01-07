//! Core engine implementation for Rex.

use std::collections::{BTreeMap, HashMap};
use std::fmt::{self, Display, Formatter};
use std::sync::{Arc, Mutex};

use async_recursion::async_recursion;
use chrono::{DateTime, Utc};
use futures::{FutureExt, future::BoxFuture, pin_mut};
use rex_gas::GasMeter;
use rex_ast::expr::{
    ClassDecl, Decl, Expr, FnDecl, InstanceDecl, Pattern, Scope, Symbol, TypeDecl, intern,
};
use rex_ts::{
    AdtDecl, Instance, Predicate, PreparedInstanceDecl, Scheme, Subst, Type, TypeError, TypeKind,
    TypeSystem, TypeVarSupply, TypedExpr, TypedExprKind, Types, compose_subst, entails,
    instantiate, unify,
};
use uuid::Uuid;

use crate::{CancellationToken, Env, EngineError};

fn sym(name: &str) -> Symbol {
    intern(name)
}

fn sym_eq(name: &Symbol, expected: &str) -> bool {
    name.as_ref() == expected
}

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

type NativeFuture = BoxFuture<'static, Result<Value, EngineError>>;
type SyncNativeCallable =
    Arc<dyn Fn(&Engine, &Type, &[Value]) -> Result<Value, EngineError> + Send + Sync>;
type AsyncNativeCallable =
    Arc<dyn Fn(Engine, Type, Vec<Value>) -> NativeFuture + Send + Sync>;
type AsyncNativeCallableCancellable =
    Arc<dyn Fn(Engine, CancellationToken, Type, Vec<Value>) -> NativeFuture + Send + Sync>;

#[derive(Clone)]
enum NativeCallable {
    Sync(SyncNativeCallable),
    Async(AsyncNativeCallable),
    AsyncCancellable(AsyncNativeCallableCancellable),
}

impl NativeCallable {
    fn call_sync(&self, engine: &Engine, typ: &Type, args: &[Value]) -> Result<Value, EngineError> {
        match self {
            NativeCallable::Sync(f) => (f)(engine, typ, args),
            NativeCallable::Async(..) | NativeCallable::AsyncCancellable(..) => {
                futures::executor::block_on(self.call_async(
                    engine.clone(),
                    typ.clone(),
                    args.to_vec(),
                ))
            }
        }
    }

    async fn call_async(
        &self,
        engine: Engine,
        typ: Type,
        args: Vec<Value>,
    ) -> Result<Value, EngineError> {
        let token = engine.cancellation_token();
        if token.is_cancelled() {
            return Err(EngineError::Cancelled);
        }

        match self {
            NativeCallable::Sync(f) => (f)(&engine, &typ, &args),
            NativeCallable::Async(f) => {
                let call_fut = (f)(engine, typ, args).fuse();
                let cancel_fut = token.cancelled().fuse();
                pin_mut!(call_fut, cancel_fut);
                futures::select! {
                    _ = cancel_fut => Err(EngineError::Cancelled),
                    res = call_fut => res,
                }
            }
            NativeCallable::AsyncCancellable(f) => {
                let call_fut = (f)(engine, token.clone(), typ, args).fuse();
                let cancel_fut = token.cancelled().fuse();
                pin_mut!(call_fut, cancel_fut);
                futures::select! {
                    _ = cancel_fut => Err(EngineError::Cancelled),
                    res = call_fut => res,
                }
            }
        }
    }
}

#[derive(Clone)]
pub enum Value {
    Bool(bool),
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    I8(i8),
    I16(i16),
    I32(i32),
    I64(i64),
    F32(f32),
    F64(f64),
    String(String),
    Uuid(Uuid),
    DateTime(DateTime<Utc>),
    Tuple(Vec<Value>),
    Array(Vec<Value>),
    Dict(BTreeMap<Symbol, Value>),
    Adt(Symbol, Vec<Value>),
    Closure {
        env: Env,
        param: Symbol,
        param_ty: Type,
        typ: Type,
        body: Arc<TypedExpr>,
    },
    Native(NativeFn),
    Overloaded(OverloadedFn),
}

impl Value {
    fn type_name(&self) -> &'static str {
        match self {
            Value::Bool(..) => "bool",
            Value::U8(..) => "u8",
            Value::U16(..) => "u16",
            Value::U32(..) => "u32",
            Value::U64(..) => "u64",
            Value::I8(..) => "i8",
            Value::I16(..) => "i16",
            Value::I32(..) => "i32",
            Value::I64(..) => "i64",
            Value::F32(..) => "f32",
            Value::F64(..) => "f64",
            Value::String(..) => "string",
            Value::Uuid(..) => "uuid",
            Value::DateTime(..) => "datetime",
            Value::Tuple(..) => "tuple",
            Value::Array(..) => "array",
            Value::Dict(..) => "dict",
            Value::Adt(name, ..) if sym_eq(name, "Empty") || sym_eq(name, "Cons") => "list",
            Value::Adt(..) => "adt",
            Value::Closure { .. } => "closure",
            Value::Native(..) => "native",
            Value::Overloaded(..) => "overloaded",
        }
    }
}

impl Display for Value {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Value::Bool(v) => write!(f, "{}", v),
            Value::U8(v) => write!(f, "{}u8", v),
            Value::U16(v) => write!(f, "{}u16", v),
            Value::U32(v) => write!(f, "{}u32", v),
            Value::U64(v) => write!(f, "{}u64", v),
            Value::I8(v) => write!(f, "{}i8", v),
            Value::I16(v) => write!(f, "{}i16", v),
            Value::I32(v) => write!(f, "{}i32", v),
            Value::I64(v) => write!(f, "{}i64", v),
            Value::F32(v) => write!(f, "{}f32", v),
            Value::F64(v) => write!(f, "{}f64", v),
            Value::String(v) => write!(f, "{:?}", v),
            Value::Uuid(v) => write!(f, "{}", v),
            Value::DateTime(v) => write!(f, "{}", v),
            Value::Tuple(xs) => {
                write!(f, "(")?;
                for (i, x) in xs.iter().enumerate() {
                    write!(f, "{}", x)?;
                    if i + 1 < xs.len() {
                        write!(f, ", ")?;
                    }
                }
                write!(f, ")")
            }
            Value::Array(xs) => {
                write!(f, "<array ")?;
                for (i, x) in xs.iter().enumerate() {
                    write!(f, "{}", x)?;
                    if i + 1 < xs.len() {
                        write!(f, ", ")?;
                    }
                }
                write!(f, ">")
            }
            Value::Dict(kvs) => {
                write!(f, "{{")?;
                for (i, (k, v)) in kvs.iter().enumerate() {
                    write!(f, "{} = {}", k, v)?;
                    if i + 1 < kvs.len() {
                        write!(f, ", ")?;
                    }
                }
                write!(f, "}}")
            }
            Value::Adt(name, args) => {
                if let Some(list) = list_to_vec_opt(self) {
                    write!(f, "[")?;
                    for (i, x) in list.iter().enumerate() {
                        write!(f, "{}", x)?;
                        if i + 1 < list.len() {
                            write!(f, ", ")?;
                        }
                    }
                    write!(f, "]")?;
                    return Ok(());
                }
                write!(f, "{}", name)?;
                for arg in args {
                    write!(f, " {}", arg)?;
                }
                Ok(())
            }
            Value::Closure { .. } => write!(f, "<closure>"),
            Value::Native(native) => write!(f, "<native:{}>", native.name),
            Value::Overloaded(over) => write!(f, "<overloaded:{}>", over.name),
        }
    }
}

#[derive(Clone)]
pub struct NativeFn {
    name: Symbol,
    arity: usize,
    typ: Type,
    func: NativeCallable,
    gas_cost: u64,
    applied: Vec<Value>,
    applied_types: Vec<Type>,
}

impl NativeFn {
    fn new(
        name: Symbol,
        arity: usize,
        typ: Type,
        func: NativeCallable,
        gas_cost: u64,
    ) -> Self {
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

    fn apply(
        mut self,
        engine: &Engine,
        arg: Value,
        arg_type: Option<&Type>,
    ) -> Result<Value, EngineError> {
        if self.arity == 0 {
            return Err(EngineError::NativeArity {
                name: self.name,
                expected: 0,
                got: 1,
            });
        }
        let (arg_ty, rest_ty) =
            split_fun(&self.typ).ok_or_else(|| EngineError::NotCallable(self.typ.to_string()))?;
        let actual_ty = resolve_arg_type(arg_type, &arg)?;
        let subst = unify(&arg_ty, &actual_ty).map_err(|_| EngineError::NativeType {
            name: self.name.clone(),
            expected: arg_ty.to_string(),
            got: actual_ty.to_string(),
        })?;
        self.typ = rest_ty.apply(&subst);
        self.applied.push(arg);
        self.applied_types.push(actual_ty);
        if is_function_type(&self.typ) {
            return Ok(Value::Native(self));
        }
        let mut full_ty = self.typ.clone();
        for arg_ty in self.applied_types.iter().rev() {
            full_ty = Type::fun(arg_ty.clone(), full_ty);
        }
        self.func.call_sync(engine, &full_ty, &self.applied)
    }

    fn call_zero(&self, engine: &Engine) -> Result<Value, EngineError> {
        if self.arity != 0 {
            return Err(EngineError::NativeArity {
                name: self.name.clone(),
                expected: self.arity,
                got: 0,
            });
        }
        self.func.call_sync(engine, &self.typ, &[])
    }

    async fn apply_async(
        mut self,
        engine: &Engine,
        arg: Value,
        arg_type: Option<&Type>,
    ) -> Result<Value, EngineError> {
        if self.arity == 0 {
            return Err(EngineError::NativeArity {
                name: self.name,
                expected: 0,
                got: 1,
            });
        }
        let (arg_ty, rest_ty) =
            split_fun(&self.typ).ok_or_else(|| EngineError::NotCallable(self.typ.to_string()))?;
        let actual_ty = resolve_arg_type(arg_type, &arg)?;
        let subst = unify(&arg_ty, &actual_ty).map_err(|_| EngineError::NativeType {
            name: self.name.clone(),
            expected: arg_ty.to_string(),
            got: actual_ty.to_string(),
        })?;
        self.typ = rest_ty.apply(&subst);
        self.applied.push(arg);
        self.applied_types.push(actual_ty);
        if is_function_type(&self.typ) {
            return Ok(Value::Native(self));
        }
        let mut full_ty = self.typ.clone();
        for arg_ty in self.applied_types.iter().rev() {
            full_ty = Type::fun(arg_ty.clone(), full_ty);
        }
        self.func
            .call_async(engine.clone(), full_ty, self.applied)
            .await
    }

    async fn call_zero_async(&self, engine: &Engine) -> Result<Value, EngineError> {
        if self.arity != 0 {
            return Err(EngineError::NativeArity {
                name: self.name.clone(),
                expected: self.arity,
                got: 0,
            });
        }
        self.func
            .call_async(engine.clone(), self.typ.clone(), Vec::new())
            .await
    }
}

#[derive(Clone)]
pub struct OverloadedFn {
    name: Symbol,
    typ: Type,
    applied: Vec<Value>,
    applied_types: Vec<Type>,
}

impl OverloadedFn {
    fn new(name: Symbol, typ: Type) -> Self {
        Self {
            name,
            typ,
            applied: Vec::new(),
            applied_types: Vec::new(),
        }
    }

    fn apply(
        mut self,
        engine: &Engine,
        arg: Value,
        arg_type: Option<&Type>,
    ) -> Result<Value, EngineError> {
        let (arg_ty, rest_ty) =
            split_fun(&self.typ).ok_or_else(|| EngineError::NotCallable(self.typ.to_string()))?;
        let actual_ty = resolve_arg_type(arg_type, &arg)?;
        let subst = unify(&arg_ty, &actual_ty).map_err(|_| EngineError::NativeType {
            name: self.name.clone(),
            expected: arg_ty.to_string(),
            got: actual_ty.to_string(),
        })?;
        let rest_ty = rest_ty.apply(&subst);
        self.applied.push(arg);
        self.applied_types.push(actual_ty);
        if is_function_type(&rest_ty) {
            return Ok(Value::Overloaded(OverloadedFn {
                name: self.name,
                typ: rest_ty,
                applied: self.applied,
                applied_types: self.applied_types,
            }));
        }
        let mut full_ty = rest_ty;
        for arg_ty in self.applied_types.iter().rev() {
            full_ty = Type::fun(arg_ty.clone(), full_ty);
        }
        if engine.types.class_methods.contains_key(&self.name) {
            // Defer typeclass method selection until we have concrete argument
            // types. This mirrors the native-overload behavior and keeps
            // polymorphic code runnable without guessing an instance.
            let mut func = engine.resolve_class_method_value(&self.name, &full_ty)?;
            let mut cur_ty = full_ty;
            for (applied, applied_ty) in self.applied.into_iter().zip(self.applied_types.iter()) {
                let (arg_ty, rest_ty) = split_fun(&cur_ty)
                    .ok_or_else(|| EngineError::NotCallable(cur_ty.to_string()))?;
                let subst = unify(&arg_ty, applied_ty).map_err(|_| EngineError::NativeType {
                    name: self.name.clone(),
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
        arg: Value,
        arg_type: Option<&Type>,
        gas: &mut GasMeter,
    ) -> Result<Value, EngineError> {
        let (arg_ty, rest_ty) =
            split_fun(&self.typ).ok_or_else(|| EngineError::NotCallable(self.typ.to_string()))?;
        let actual_ty = resolve_arg_type(arg_type, &arg)?;
        let subst = unify(&arg_ty, &actual_ty).map_err(|_| EngineError::NativeType {
            name: self.name.clone(),
            expected: arg_ty.to_string(),
            got: actual_ty.to_string(),
        })?;
        let rest_ty = rest_ty.apply(&subst);
        self.applied.push(arg);
        self.applied_types.push(actual_ty);
        if is_function_type(&rest_ty) {
            return Ok(Value::Overloaded(OverloadedFn {
                name: self.name,
                typ: rest_ty,
                applied: self.applied,
                applied_types: self.applied_types,
            }));
        }
        let mut full_ty = rest_ty;
        for arg_ty in self.applied_types.iter().rev() {
            full_ty = Type::fun(arg_ty.clone(), full_ty);
        }
        if engine.types.class_methods.contains_key(&self.name) {
            let mut func = engine.resolve_class_method_value_with_gas(&self.name, &full_ty, gas)?;
            let mut cur_ty = full_ty;
            for (applied, applied_ty) in self.applied.into_iter().zip(self.applied_types.iter()) {
                let (arg_ty, rest_ty) = split_fun(&cur_ty)
                    .ok_or_else(|| EngineError::NotCallable(cur_ty.to_string()))?;
                let subst = unify(&arg_ty, applied_ty).map_err(|_| EngineError::NativeType {
                    name: self.name.clone(),
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
            .saturating_add(gas.costs.native_call_per_arg.saturating_mul(self.applied.len() as u64));
        gas.charge(amount)?;
        imp.func.call_sync(engine, &full_ty, &self.applied)
    }

    async fn apply_async(
        mut self,
        engine: &Engine,
        arg: Value,
        arg_type: Option<&Type>,
    ) -> Result<Value, EngineError> {
        let (arg_ty, rest_ty) =
            split_fun(&self.typ).ok_or_else(|| EngineError::NotCallable(self.typ.to_string()))?;
        let actual_ty = resolve_arg_type(arg_type, &arg)?;
        let subst = unify(&arg_ty, &actual_ty).map_err(|_| EngineError::NativeType {
            name: self.name.clone(),
            expected: arg_ty.to_string(),
            got: actual_ty.to_string(),
        })?;
        let rest_ty = rest_ty.apply(&subst);
        self.applied.push(arg);
        self.applied_types.push(actual_ty);
        if is_function_type(&rest_ty) {
            return Ok(Value::Overloaded(OverloadedFn {
                name: self.name,
                typ: rest_ty,
                applied: self.applied,
                applied_types: self.applied_types,
            }));
        }
        let mut full_ty = rest_ty;
        for arg_ty in self.applied_types.iter().rev() {
            full_ty = Type::fun(arg_ty.clone(), full_ty);
        }
        if engine.types.class_methods.contains_key(&self.name) {
            let mut func = engine.resolve_class_method_value(&self.name, &full_ty)?;
            let mut cur_ty = full_ty;
            for (applied, applied_ty) in self.applied.into_iter().zip(self.applied_types.iter()) {
                let (arg_ty, rest_ty) = split_fun(&cur_ty)
                    .ok_or_else(|| EngineError::NotCallable(cur_ty.to_string()))?;
                let subst = unify(&arg_ty, applied_ty).map_err(|_| EngineError::NativeType {
                    name: self.name.clone(),
                    expected: arg_ty.to_string(),
                    got: applied_ty.to_string(),
                })?;
                let rest_ty = rest_ty.apply(&subst);
                func = apply_async(engine, func, applied, Some(&cur_ty), Some(applied_ty)).await?;
                cur_ty = rest_ty;
            }
            return Ok(func);
        }

        let imp = engine.resolve_native_impl(self.name.as_ref(), &full_ty)?;
        imp.func
            .call_async(engine.clone(), full_ty, self.applied)
            .await
    }

    async fn apply_async_with_gas(
        mut self,
        engine: &Engine,
        arg: Value,
        arg_type: Option<&Type>,
        gas: &mut GasMeter,
    ) -> Result<Value, EngineError> {
        let (arg_ty, rest_ty) =
            split_fun(&self.typ).ok_or_else(|| EngineError::NotCallable(self.typ.to_string()))?;
        let actual_ty = resolve_arg_type(arg_type, &arg)?;
        let subst = unify(&arg_ty, &actual_ty).map_err(|_| EngineError::NativeType {
            name: self.name.clone(),
            expected: arg_ty.to_string(),
            got: actual_ty.to_string(),
        })?;
        let rest_ty = rest_ty.apply(&subst);
        self.applied.push(arg);
        self.applied_types.push(actual_ty);
        if is_function_type(&rest_ty) {
            return Ok(Value::Overloaded(OverloadedFn {
                name: self.name,
                typ: rest_ty,
                applied: self.applied,
                applied_types: self.applied_types,
            }));
        }
        let mut full_ty = rest_ty;
        for arg_ty in self.applied_types.iter().rev() {
            full_ty = Type::fun(arg_ty.clone(), full_ty);
        }
        if engine.types.class_methods.contains_key(&self.name) {
            let mut func = engine.resolve_class_method_value_with_gas(&self.name, &full_ty, gas)?;
            let mut cur_ty = full_ty;
            for (applied, applied_ty) in self.applied.into_iter().zip(self.applied_types.iter()) {
                let (arg_ty, rest_ty) = split_fun(&cur_ty)
                    .ok_or_else(|| EngineError::NotCallable(cur_ty.to_string()))?;
                let subst = unify(&arg_ty, applied_ty).map_err(|_| EngineError::NativeType {
                    name: self.name.clone(),
                    expected: arg_ty.to_string(),
                    got: applied_ty.to_string(),
                })?;
                let rest_ty = rest_ty.apply(&subst);
                func =
                    apply_async_with_gas(engine, func, applied, Some(&cur_ty), Some(applied_ty), gas)
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
            .saturating_add(gas.costs.native_call_per_arg.saturating_mul(self.applied.len() as u64));
        gas.charge(amount)?;
        imp.func
            .call_async(engine.clone(), full_ty, self.applied)
            .await
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
        let instances = self
            .entries
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

pub trait IntoValue {
    fn into_value(self) -> Value;
}

pub trait FromValue: Sized {
    fn from_value(value: &Value, name: &str) -> Result<Self, EngineError>;
}

pub trait RexType {
    fn rex_type() -> Type;
}

impl IntoValue for Value {
    fn into_value(self) -> Value {
        self
    }
}

impl IntoValue for bool {
    fn into_value(self) -> Value {
        Value::Bool(self)
    }
}

impl IntoValue for u8 {
    fn into_value(self) -> Value {
        Value::U8(self)
    }
}

impl IntoValue for u16 {
    fn into_value(self) -> Value {
        Value::U16(self)
    }
}

impl IntoValue for u32 {
    fn into_value(self) -> Value {
        Value::U32(self)
    }
}

impl IntoValue for u64 {
    fn into_value(self) -> Value {
        Value::U64(self)
    }
}

impl IntoValue for i8 {
    fn into_value(self) -> Value {
        Value::I8(self)
    }
}

impl IntoValue for i16 {
    fn into_value(self) -> Value {
        Value::I16(self)
    }
}

impl IntoValue for i32 {
    fn into_value(self) -> Value {
        Value::I32(self)
    }
}

impl IntoValue for i64 {
    fn into_value(self) -> Value {
        Value::I64(self)
    }
}

impl IntoValue for f32 {
    fn into_value(self) -> Value {
        Value::F32(self)
    }
}

impl IntoValue for f64 {
    fn into_value(self) -> Value {
        Value::F64(self)
    }
}

impl IntoValue for String {
    fn into_value(self) -> Value {
        Value::String(self)
    }
}

impl IntoValue for &str {
    fn into_value(self) -> Value {
        Value::String(self.to_string())
    }
}

impl<T: IntoValue> IntoValue for Vec<T> {
    fn into_value(self) -> Value {
        Value::Array(self.into_iter().map(IntoValue::into_value).collect())
    }
}

impl IntoValue for Uuid {
    fn into_value(self) -> Value {
        Value::Uuid(self)
    }
}

impl IntoValue for DateTime<Utc> {
    fn into_value(self) -> Value {
        Value::DateTime(self)
    }
}

impl RexType for bool {
    fn rex_type() -> Type {
        Type::con("bool", 0)
    }
}

impl RexType for u8 {
    fn rex_type() -> Type {
        Type::con("u8", 0)
    }
}

impl RexType for u16 {
    fn rex_type() -> Type {
        Type::con("u16", 0)
    }
}

impl RexType for u32 {
    fn rex_type() -> Type {
        Type::con("u32", 0)
    }
}

impl RexType for u64 {
    fn rex_type() -> Type {
        Type::con("u64", 0)
    }
}

impl RexType for i8 {
    fn rex_type() -> Type {
        Type::con("i8", 0)
    }
}

impl RexType for i16 {
    fn rex_type() -> Type {
        Type::con("i16", 0)
    }
}

impl RexType for i32 {
    fn rex_type() -> Type {
        Type::con("i32", 0)
    }
}

impl RexType for i64 {
    fn rex_type() -> Type {
        Type::con("i64", 0)
    }
}

impl RexType for f32 {
    fn rex_type() -> Type {
        Type::con("f32", 0)
    }
}

impl RexType for f64 {
    fn rex_type() -> Type {
        Type::con("f64", 0)
    }
}

impl RexType for String {
    fn rex_type() -> Type {
        Type::con("string", 0)
    }
}

impl RexType for &str {
    fn rex_type() -> Type {
        Type::con("string", 0)
    }
}

impl RexType for Uuid {
    fn rex_type() -> Type {
        Type::con("uuid", 0)
    }
}

impl RexType for DateTime<Utc> {
    fn rex_type() -> Type {
        Type::con("datetime", 0)
    }
}

impl<T: RexType> RexType for Vec<T> {
    fn rex_type() -> Type {
        Type::app(Type::con("Array", 1), T::rex_type())
    }
}

impl RexType for () {
    fn rex_type() -> Type {
        Type::tuple(vec![])
    }
}

impl<A: RexType, B: RexType> RexType for (A, B) {
    fn rex_type() -> Type {
        Type::tuple(vec![A::rex_type(), B::rex_type()])
    }
}

impl FromValue for bool {
    fn from_value(value: &Value, name: &str) -> Result<Self, EngineError> {
        match value {
            Value::Bool(v) => Ok(*v),
            _ => Err(EngineError::NativeType {
                name: sym(name),
                expected: "bool".into(),
                got: value.type_name().into(),
            }),
        }
    }
}

macro_rules! impl_from_value_num {
    ($t:ty, $variant:ident, $label:literal) => {
        impl FromValue for $t {
            fn from_value(value: &Value, name: &str) -> Result<Self, EngineError> {
                match value {
                    Value::$variant(v) => Ok(*v as $t),
                    _ => Err(EngineError::NativeType {
                        name: sym(name),
                        expected: $label.into(),
                        got: value.type_name().into(),
                    }),
                }
            }
        }
    };
}

impl_from_value_num!(u8, U8, "u8");
impl_from_value_num!(u16, U16, "u16");
impl_from_value_num!(u32, U32, "u32");
impl_from_value_num!(u64, U64, "u64");
impl_from_value_num!(i8, I8, "i8");
impl_from_value_num!(i16, I16, "i16");
impl_from_value_num!(i32, I32, "i32");
impl_from_value_num!(i64, I64, "i64");
impl_from_value_num!(f32, F32, "f32");
impl_from_value_num!(f64, F64, "f64");

impl FromValue for String {
    fn from_value(value: &Value, name: &str) -> Result<Self, EngineError> {
        match value {
            Value::String(v) => Ok(v.clone()),
            _ => Err(EngineError::NativeType {
                name: sym(name),
                expected: "string".into(),
                got: value.type_name().into(),
            }),
        }
    }
}

impl FromValue for Uuid {
    fn from_value(value: &Value, name: &str) -> Result<Self, EngineError> {
        match value {
            Value::Uuid(v) => Ok(*v),
            _ => Err(EngineError::NativeType {
                name: sym(name),
                expected: "uuid".into(),
                got: value.type_name().into(),
            }),
        }
    }
}

impl FromValue for DateTime<Utc> {
    fn from_value(value: &Value, name: &str) -> Result<Self, EngineError> {
        match value {
            Value::DateTime(v) => Ok(*v),
            _ => Err(EngineError::NativeType {
                name: sym(name),
                expected: "datetime".into(),
                got: value.type_name().into(),
            }),
        }
    }
}

impl FromValue for Value {
    fn from_value(value: &Value, _name: &str) -> Result<Self, EngineError> {
        Ok(value.clone())
    }
}

#[derive(Clone)]
pub struct Engine {
    env: Env,
    natives: NativeRegistry,
    typeclasses: TypeclassRegistry,
    types: TypeSystem,
    typeclass_cache: Arc<Mutex<HashMap<(Symbol, Type), Value>>>,
    cancel: CancellationToken,
}

impl Engine {
    pub fn new() -> Self {
        Self {
            env: Env::new(),
            natives: NativeRegistry::default(),
            typeclasses: TypeclassRegistry::default(),
            types: TypeSystem::new(),
            typeclass_cache: Arc::new(Mutex::new(HashMap::new())),
            cancel: CancellationToken::new(),
        }
    }

    pub fn with_prelude() -> Self {
        let mut engine = Engine {
            env: Env::new(),
            natives: NativeRegistry::default(),
            typeclasses: TypeclassRegistry::default(),
            types: TypeSystem::with_prelude(),
            typeclass_cache: Arc::new(Mutex::new(HashMap::new())),
            cancel: CancellationToken::new(),
        };
        engine.inject_prelude().expect("prelude injection failed");
        engine
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

        let inject = |engine: &mut Engine,
                      name: &'static str,
                      log: fn(&str)|
         -> Result<(), EngineError> {
            let name_sym = sym(name);
            let scheme = make_scheme(engine);
            engine.inject_native_scheme_typed(name, scheme, 1, move |engine, call_type, args| {
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
                let pretty = engine.resolve_class_method_value(&sym("pretty"), &pretty_ty)?;
                let rendered = apply(
                    engine,
                    pretty,
                    args[0].clone(),
                    Some(&pretty_ty),
                    Some(&arg_ty),
                )?;

                let Value::String(message) = rendered else {
                    return Err(EngineError::NativeType {
                        name: name_sym.clone(),
                        expected: "string".to_string(),
                        got: rendered.type_name().to_string(),
                    });
                };

                log(&message);
                Ok(Value::String(message))
            })
        };

        inject(self, "debug", |s| tracing::debug!("{s}"))?;
        inject(self, "info", |s| tracing::info!("{s}"))?;
        inject(self, "warn", |s| tracing::warn!("{s}"))?;
        inject(self, "error", |s| tracing::error!("{s}"))?;
        Ok(())
    }

    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    pub fn inject_value<V: IntoValue + RexType>(
        &mut self,
        name: &str,
        value: V,
    ) -> Result<(), EngineError> {
        let name = normalize_name(name);
        let typ = V::rex_type();
        let value = value.into_value();
        let func =
            Arc::new(move |_engine: &Engine, _typ: &Type, _args: &[Value]| Ok(value.clone()));
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
        let func =
            Arc::new(move |_engine: &Engine, _typ: &Type, _args: &[Value]| Ok(value.clone()));
        let scheme = Scheme::new(vec![], vec![], typ);
        self.register_native(name, scheme, 0, NativeCallable::Sync(func), 0)
    }

    pub fn inject_fn0<F, R>(&mut self, name: &str, f: F) -> Result<(), EngineError>
    where
        F: Fn() -> R + Send + Sync + 'static,
        R: IntoValue + RexType,
    {
        let name = normalize_name(name);
        let name_string = name.clone();
        let name_for_fn = name.clone();
        let func = Arc::new(move |_engine: &Engine, _typ: &Type, args: &[Value]| {
            if !args.is_empty() {
                return Err(EngineError::NativeArity {
                    name: name_for_fn.clone(),
                    expected: 0,
                    got: args.len(),
                });
            }
            Ok(f().into_value())
        });
        let typ = R::rex_type();
        let scheme = Scheme::new(vec![], vec![], typ);
        self.register_native(name_string, scheme, 0, NativeCallable::Sync(func), 0)
    }

    pub fn inject_fn1<F, A, R>(&mut self, name: &str, f: F) -> Result<(), EngineError>
    where
        F: Fn(A) -> R + Send + Sync + 'static,
        A: FromValue + RexType,
        R: IntoValue + RexType,
    {
        let name = normalize_name(name);
        let name_string = name.clone();
        let func = Arc::new(move |_engine: &Engine, _typ: &Type, args: &[Value]| {
            if args.len() != 1 {
                return Err(EngineError::NativeArity {
                    name: name_string.clone(),
                    expected: 1,
                    got: args.len(),
                });
            }
            let a = A::from_value(&args[0], name_string.as_ref())?;
            Ok(f(a).into_value())
        });
        let typ = Type::fun(A::rex_type(), R::rex_type());
        let scheme = Scheme::new(vec![], vec![], typ);
        self.register_native(name, scheme, 1, NativeCallable::Sync(func), 0)
    }

    pub fn inject_fn2<F, A, B, R>(&mut self, name: &str, f: F) -> Result<(), EngineError>
    where
        F: Fn(A, B) -> R + Send + Sync + 'static,
        A: FromValue + RexType,
        B: FromValue + RexType,
        R: IntoValue + RexType,
    {
        let name = normalize_name(name);
        let name_string = name.clone();
        let func = Arc::new(move |_engine: &Engine, _typ: &Type, args: &[Value]| {
            if args.len() != 2 {
                return Err(EngineError::NativeArity {
                    name: name_string.clone(),
                    expected: 2,
                    got: args.len(),
                });
            }
            let a = A::from_value(&args[0], name_string.as_ref())?;
            let b = B::from_value(&args[1], name_string.as_ref())?;
            Ok(f(a, b).into_value())
        });
        let typ = Type::fun(A::rex_type(), Type::fun(B::rex_type(), R::rex_type()));
        let scheme = Scheme::new(vec![], vec![], typ);
        self.register_native(name, scheme, 2, NativeCallable::Sync(func), 0)
    }

    pub fn inject_async_fn0<F, Fut, R>(&mut self, name: &str, f: F) -> Result<(), EngineError>
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = R> + Send + 'static,
        R: IntoValue + RexType,
    {
        let name = normalize_name(name);
        let name_sym = name.clone();
        let f = Arc::new(f);
        let func = Arc::new(move |_engine: Engine, _typ: Type, args: Vec<Value>| {
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
                Ok(f().await.into_value())
            }
            .boxed()
        });
        let typ = R::rex_type();
        let scheme = Scheme::new(vec![], vec![], typ);
        self.register_native(name, scheme, 0, NativeCallable::Async(func), 0)
    }

    pub fn inject_async_fn1<F, Fut, A, R>(&mut self, name: &str, f: F) -> Result<(), EngineError>
    where
        F: Fn(A) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = R> + Send + 'static,
        A: FromValue + RexType,
        R: IntoValue + RexType,
    {
        let name = normalize_name(name);
        let name_sym = name.clone();
        let f = Arc::new(f);
        let func = Arc::new(move |_engine: Engine, _typ: Type, args: Vec<Value>| {
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
                let a = A::from_value(&args[0], name_sym.as_ref())?;
                Ok(f(a).await.into_value())
            }
            .boxed()
        });
        let typ = Type::fun(A::rex_type(), R::rex_type());
        let scheme = Scheme::new(vec![], vec![], typ);
        self.register_native(name, scheme, 1, NativeCallable::Async(func), 0)
    }

    pub fn inject_async_fn2<F, Fut, A, B, R>(&mut self, name: &str, f: F) -> Result<(), EngineError>
    where
        F: Fn(A, B) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = R> + Send + 'static,
        A: FromValue + RexType,
        B: FromValue + RexType,
        R: IntoValue + RexType,
    {
        let name = normalize_name(name);
        let name_sym = name.clone();
        let f = Arc::new(f);
        let func = Arc::new(move |_engine: Engine, _typ: Type, args: Vec<Value>| {
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
                let a = A::from_value(&args[0], name_sym.as_ref())?;
                let b = B::from_value(&args[1], name_sym.as_ref())?;
                Ok(f(a, b).await.into_value())
            }
            .boxed()
        });
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
        R: IntoValue + RexType,
    {
        let name = normalize_name(name);
        let name_sym = name.clone();
        let f = Arc::new(f);
        let func = Arc::new(move |_engine: Engine, token: CancellationToken, _typ: Type, args: Vec<Value>| {
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
                Ok(f(token).await.into_value())
            }
            .boxed()
        });
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
        A: FromValue + RexType,
        R: IntoValue + RexType,
    {
        let name = normalize_name(name);
        let name_sym = name.clone();
        let f = Arc::new(f);
        let func = Arc::new(move |_engine: Engine, token: CancellationToken, _typ: Type, args: Vec<Value>| {
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
                let a = A::from_value(&args[0], name_sym.as_ref())?;
                Ok(f(token, a).await.into_value())
            }
            .boxed()
        });
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
        A: FromValue + RexType,
        B: FromValue + RexType,
        R: IntoValue + RexType,
    {
        let name = normalize_name(name);
        let name_sym = name.clone();
        let f = Arc::new(f);
        let func = Arc::new(move |_engine: Engine, token: CancellationToken, _typ: Type, args: Vec<Value>| {
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
                let a = A::from_value(&args[0], name_sym.as_ref())?;
                let b = B::from_value(&args[1], name_sym.as_ref())?;
                Ok(f(token, a, b).await.into_value())
            }
            .boxed()
        });
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
        F: Fn(&Engine, &[Value]) -> Result<Value, EngineError> + Send + Sync + 'static,
    {
        let name = normalize_name(name);
        let scheme = Scheme::new(vec![], vec![], typ);
        let func = Arc::new(move |engine: &Engine, _typ: &Type, args: &[Value]| func(engine, args));
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
        F: Fn(&Engine, &[Value]) -> Result<Value, EngineError> + Send + Sync + 'static,
    {
        let name = normalize_name(name);
        let func = Arc::new(move |engine: &Engine, _typ: &Type, args: &[Value]| func(engine, args));
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
        F: Fn(&Engine, &Type, &[Value]) -> Result<Value, EngineError> + Send + Sync + 'static,
    {
        let name = normalize_name(name);
        self.register_native(name, scheme, arity, NativeCallable::Sync(Arc::new(func)), 0)
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
        F: Fn(&Engine, &Type, &[Value]) -> Result<Value, EngineError> + Send + Sync + 'static,
    {
        let name = normalize_name(name);
        self.register_native(
            name,
            scheme,
            arity,
            NativeCallable::Sync(Arc::new(func)),
            gas_cost,
        )
    }

    pub fn inject_native_scheme_typed_async<F, Fut>(
        &mut self,
        name: &str,
        scheme: Scheme,
        arity: usize,
        func: F,
    ) -> Result<(), EngineError>
    where
        F: Fn(Engine, Type, Vec<Value>) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<Value, EngineError>> + Send + 'static,
    {
        let name = normalize_name(name);
        let func = Arc::new(move |engine: Engine, typ: Type, args: Vec<Value>| func(engine, typ, args).boxed());
        self.register_native(name, scheme, arity, NativeCallable::Async(func), 0)
    }

    pub fn inject_native_scheme_typed_async_with_gas_cost<F, Fut>(
        &mut self,
        name: &str,
        scheme: Scheme,
        arity: usize,
        gas_cost: u64,
        func: F,
    ) -> Result<(), EngineError>
    where
        F: Fn(Engine, Type, Vec<Value>) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<Value, EngineError>> + Send + 'static,
    {
        let name = normalize_name(name);
        let func =
            Arc::new(move |engine: Engine, typ: Type, args: Vec<Value>| func(engine, typ, args).boxed());
        self.register_native(name, scheme, arity, NativeCallable::Async(func), gas_cost)
    }

    pub fn inject_native_scheme_typed_async_cancellable<F, Fut>(
        &mut self,
        name: &str,
        scheme: Scheme,
        arity: usize,
        func: F,
    ) -> Result<(), EngineError>
    where
        F: Fn(Engine, CancellationToken, Type, Vec<Value>) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<Value, EngineError>> + Send + 'static,
    {
        let name = normalize_name(name);
        let func = Arc::new(move |engine: Engine, token: CancellationToken, typ: Type, args: Vec<Value>| {
            func(engine, token, typ, args).boxed()
        });
        self.register_native(name, scheme, arity, NativeCallable::AsyncCancellable(func), 0)
    }

    pub fn inject_native_scheme_typed_async_cancellable_with_gas_cost<F, Fut>(
        &mut self,
        name: &str,
        scheme: Scheme,
        arity: usize,
        gas_cost: u64,
        func: F,
    ) -> Result<(), EngineError>
    where
        F: Fn(Engine, CancellationToken, Type, Vec<Value>) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<Value, EngineError>> + Send + 'static,
    {
        let name = normalize_name(name);
        let func = Arc::new(move |engine: Engine, token: CancellationToken, typ: Type, args: Vec<Value>| {
            func(engine, token, typ, args).boxed()
        });
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
            let func = Arc::new(move |_engine: &Engine, _typ: &Type, args: &[Value]| {
                Ok(Value::Adt(ctor_name.clone(), args.to_vec()))
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
        self.types.inject_class_decl(decl).map_err(EngineError::Type)
    }

    pub fn inject_instance_decl(&mut self, decl: &InstanceDecl) -> Result<(), EngineError> {
        let prepared = self.types.inject_instance_decl(decl).map_err(EngineError::Type)?;
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
        let value = eval_typed_expr(self, &self.env, &typed)?;
        self.env = self.env.extend(decl.name.name.clone(), value);
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

    pub fn eval(&mut self, expr: &Expr) -> Result<Value, EngineError> {
        self.eval_inner(expr)
    }

    pub async fn eval_async(&mut self, expr: &Expr) -> Result<Value, EngineError> {
        check_cancelled(self)?;
        let typed = self.type_check(expr)?;
        eval_typed_expr_async(self, &self.env, &typed).await
    }

    pub fn eval_with_gas(&mut self, expr: &Expr, gas: &mut GasMeter) -> Result<Value, EngineError> {
        check_cancelled(self)?;
        let typed = self.type_check(expr)?;
        eval_typed_expr_with_gas(self, &self.env, &typed, gas)
    }

    pub async fn eval_async_with_gas(
        &mut self,
        expr: &Expr,
        gas: &mut GasMeter,
    ) -> Result<Value, EngineError> {
        check_cancelled(self)?;
        let typed = self.type_check(expr)?;
        eval_typed_expr_async_with_gas(self, &self.env, &typed, gas).await
    }

    pub fn eval_with_stack_size(
        &mut self,
        expr: &Expr,
        stack_size: usize,
    ) -> Result<Value, EngineError> {
        crate::stack::run_with_stack_size(stack_size, || self.eval_inner(expr))?
    }

    fn eval_inner(&mut self, expr: &Expr) -> Result<Value, EngineError> {
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
        self.register_prelude_typeclass_instances()?;
        Ok(())
    }

    fn register_prelude_typeclass_instances(&mut self) -> Result<(), EngineError> {
        // The type system prelude injects the *heads* of the standard instances.
        // The evaluator also needs the *method bodies* so class method lookup can
        // produce actual values at runtime.
        let program = rex_ts::prelude_typeclasses_program();
        for decl in &program.decls {
            let Decl::Instance(inst_decl) = decl else { continue };
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
                        if !overloads.is_empty() {
                            if expr.typ.ftv().is_empty()
                                && !overloads.iter().any(|t| unify(t, &expr.typ).is_ok())
                            {
                                return Err(EngineError::MissingImpl {
                                    name: name.clone(),
                                    typ: expr.typ.to_string(),
                                });
                            }
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

        self.typeclasses
            .insert(
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
            .ok_or_else(|| EngineError::Type(TypeError::UnsupportedExpr("method scheme missing class predicate")))?;
        let param_type = class_pred.typ.apply(&s_method);
        if type_head_is_var(&param_type) {
            return Err(EngineError::AmbiguousOverload { name: name.clone() });
        }

        self.typeclasses.resolve(&info.class, name, &param_type)
    }

    fn resolve_class_method_value(&self, name: &Symbol, typ: &Type) -> Result<Value, EngineError> {
        if typ.ftv().is_empty() {
            if let Ok(cache) = self.typeclass_cache.lock() {
                if let Some(value) = cache.get(&(name.clone(), typ.clone())) {
                    return Ok(value.clone());
                }
            }
        }

        let (def_env, typed, s) = match self.resolve_typeclass_method_impl(name, typ) {
            Ok(res) => res,
            Err(EngineError::AmbiguousOverload { .. }) if is_function_type(typ) => {
                return Ok(Value::Overloaded(OverloadedFn::new(name.clone(), typ.clone())));
            }
            Err(err) => return Err(err),
        };
        let specialized = typed.as_ref().apply(&s);
        let value = eval_typed_expr(self, &def_env, &specialized)?;

        if typ.ftv().is_empty() {
            if let Ok(mut cache) = self.typeclass_cache.lock() {
                cache.insert((name.clone(), typ.clone()), value.clone());
            }
        }
        Ok(value)
    }

    fn resolve_class_method_value_with_gas(
        &self,
        name: &Symbol,
        typ: &Type,
        gas: &mut GasMeter,
    ) -> Result<Value, EngineError> {
        let (def_env, typed, s) = match self.resolve_typeclass_method_impl(name, typ) {
            Ok(res) => res,
            Err(EngineError::AmbiguousOverload { .. }) if is_function_type(typ) => {
                return Ok(Value::Overloaded(OverloadedFn::new(name.clone(), typ.clone())));
            }
            Err(err) => return Err(err),
        };
        let specialized = typed.as_ref().apply(&s);
        eval_typed_expr_with_gas(self, &def_env, &specialized, gas)
    }

    fn resolve_global_value(&self, name: &Symbol, typ: &Type) -> Result<Value, EngineError> {
        if let Some(value) = self.env.get(name) {
            match value {
                Value::Native(native) if native.arity == 0 && native.applied.is_empty() => {
                    native.call_zero(self)
                }
                _ => Ok(value),
            }
        } else if self.types.class_methods.contains_key(name) {
            self.resolve_class_method_value(name, typ)
        } else {
            let value = self.resolve_native_value(name.as_ref(), typ)?;
            match &value {
                Value::Native(native) if native.arity == 0 && native.applied.is_empty() => {
                    native.call_zero(self)
                }
                _ => Ok(value),
            }
        }
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

    fn resolve_native_value(&self, name: &str, typ: &Type) -> Result<Value, EngineError> {
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
                Ok(Value::Native(imp.to_native_fn(typ.clone())))
            }
            _ => {
                if typ.ftv().is_empty() {
                    Err(EngineError::AmbiguousImpl {
                        name: sym_name.clone(),
                        typ: typ.to_string(),
                    })
                } else if is_function_type(typ) {
                    Ok(Value::Overloaded(OverloadedFn::new(
                        sym_name.clone(),
                        typ.clone(),
                    )))
                } else {
                    Err(EngineError::AmbiguousOverload { name: sym_name })
                }
            }
        }
    }

    fn resolve_native_value_with_gas(
        &self,
        name: &str,
        typ: &Type,
        _gas: &mut GasMeter,
    ) -> Result<Value, EngineError> {
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
                Ok(Value::Native(imp.to_native_fn(typ.clone())))
            }
            _ => {
                if typ.ftv().is_empty() {
                    Err(EngineError::AmbiguousImpl {
                        name: sym_name.clone(),
                        typ: typ.to_string(),
                    })
                } else if is_function_type(typ) {
                    Ok(Value::Overloaded(OverloadedFn::new(sym_name.clone(), typ.clone())))
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
        if expr.typ.ftv().is_empty() {
            if let TypeKind::Con(tc) = expr.typ.as_ref() {
                if tc.arity == 0 {
                    push_unique_type(out, expr.typ.clone());
                }
            }
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

fn lookup_scheme(engine: &Engine, name: &Symbol) -> Result<Scheme, EngineError> {
    let schemes = engine
        .types
        .env
        .lookup(name)
        .ok_or_else(|| EngineError::UnknownVar(name.clone()))?;
    if schemes.len() != 1 {
        return Err(EngineError::AmbiguousOverload { name: name.clone() });
    }
    Ok(schemes[0].clone())
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

fn list_to_vec(value: &Value, name: &str) -> Result<Vec<Value>, EngineError> {
    let mut out = Vec::new();
    let mut cur = value;
    loop {
        match cur {
            Value::Adt(tag, args) if sym_eq(tag, "Empty") && args.is_empty() => return Ok(out),
            Value::Adt(tag, args) if sym_eq(tag, "Cons") && args.len() == 2 => {
                out.push(args[0].clone());
                cur = &args[1];
            }
            _ => {
                return Err(EngineError::NativeType {
                    name: sym(name),
                    expected: "list".into(),
                    got: value.type_name().into(),
                });
            }
        }
    }
}

fn list_to_vec_opt(value: &Value) -> Option<Vec<Value>> {
    let mut out = Vec::new();
    let mut cur = value;
    loop {
        match cur {
            Value::Adt(tag, args) if sym_eq(tag, "Empty") && args.is_empty() => return Some(out),
            Value::Adt(tag, args) if sym_eq(tag, "Cons") && args.len() == 2 => {
                out.push(args[0].clone());
                cur = &args[1];
            }
            _ => return None,
        }
    }
}

fn list_from_vec(values: Vec<Value>) -> Value {
    let mut list = Value::Adt(sym("Empty"), vec![]);
    for v in values.into_iter().rev() {
        list = Value::Adt(sym("Cons"), vec![v, list]);
    }
    list
}

fn value_type(value: &Value) -> Result<Type, EngineError> {
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
                tys.push(value_type(elem)?);
            }
            Ok(Type::tuple(tys))
        }
        Value::Array(elems) => {
            let first = elems
                .get(0)
                .ok_or_else(|| EngineError::UnknownType(sym("array")))?;
            let elem_ty = value_type(first)?;
            for elem in elems.iter().skip(1) {
                let ty = value_type(elem)?;
                if ty != elem_ty {
                    return Err(EngineError::NativeType {
                        name: sym("array"),
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
            let elem_ty = value_type(first)?;
            for val in map.values().skip(1) {
                let ty = value_type(val)?;
                if ty != elem_ty {
                    return Err(EngineError::NativeType {
                        name: sym("dict"),
                        expected: elem_ty.to_string(),
                        got: ty.to_string(),
                    });
                }
            }
            Ok(Type::app(Type::con("Dict", 1), elem_ty))
        }
        Value::Adt(tag, args) if sym_eq(tag, "Some") && args.len() == 1 => {
            let inner = value_type(&args[0])?;
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
            let elems = list_to_vec(value, "list")?;
            let first = elems
                .get(0)
                .ok_or_else(|| EngineError::UnknownType(sym("list")))?;
            let elem_ty = value_type(first)?;
            for elem in elems.iter().skip(1) {
                let ty = value_type(elem)?;
                if ty != elem_ty {
                    return Err(EngineError::NativeType {
                        name: sym("list"),
                        expected: elem_ty.to_string(),
                        got: ty.to_string(),
                    });
                }
            }
            Ok(Type::app(Type::con("List", 1), elem_ty))
        }
        Value::Adt(tag, _args) => Err(EngineError::UnknownType(tag.clone())),
        Value::Closure { .. } => Err(EngineError::UnknownType(sym("closure"))),
        Value::Native(..) => Err(EngineError::UnknownType(sym("native"))),
        Value::Overloaded(..) => Err(EngineError::UnknownType(sym("overloaded"))),
    }
}

fn resolve_arg_type(arg_type: Option<&Type>, arg: &Value) -> Result<Type, EngineError> {
    match arg_type {
        Some(ty) if ty.ftv().is_empty() => Ok(ty.clone()),
        Some(ty) => match value_type(arg) {
            Ok(val_ty) if val_ty.ftv().is_empty() => Ok(val_ty),
            _ => Ok(ty.clone()),
        },
        None => value_type(arg),
    }
}

fn resolve_binary_op(engine: &Engine, name: &str, elem_ty: &Type) -> Result<Value, EngineError> {
    let op_ty = Type::fun(elem_ty.clone(), Type::fun(elem_ty.clone(), elem_ty.clone()));
    engine.resolve_global_value(&sym(name), &op_ty)
}

#[cfg(any())]
fn eq_via_class(
    engine: &Engine,
    op_name: &Symbol,
    typ: &Type,
    lhs: &Value,
    rhs: &Value,
) -> Result<bool, EngineError> {
    let bool_ty = Type::con("bool", 0);
    let eq_ty = Type::fun(typ.clone(), Type::fun(typ.clone(), bool_ty.clone()));
    let func = engine.resolve_global_value(&sym("=="), &eq_ty)?;
    let step = apply(engine, func, lhs.clone(), Some(&eq_ty), Some(typ))?;
    let res = apply(
        engine,
        step,
        rhs.clone(),
        Some(&Type::fun(typ.clone(), bool_ty)),
        Some(typ),
    )?;
    match res {
        Value::Bool(b) => Ok(b),
        other => Err(EngineError::NativeType {
            name: op_name.clone(),
            expected: "bool".into(),
            got: other.type_name().into(),
        }),
    }
}

fn len_value_for_type(elem_ty: &Type, len: usize, name: &str) -> Result<Value, EngineError> {
    match elem_ty.as_ref() {
        TypeKind::Con(c) if sym_eq(&c.name, "f32") => Ok(Value::F32(len as f32)),
        TypeKind::Con(c) if sym_eq(&c.name, "f64") => Ok(Value::F64(len as f64)),
        _ => Err(EngineError::NativeType {
            name: sym(name),
            expected: "f32 or f64".into(),
            got: elem_ty.to_string(),
        }),
    }
}

fn expect_array(value: &Value, name: &str) -> Result<Vec<Value>, EngineError> {
    match value {
        Value::Array(xs) => Ok(xs.clone()),
        _ => Err(EngineError::NativeType {
            name: sym(name),
            expected: "array".into(),
            got: value.type_name().into(),
        }),
    }
}

fn expect_bool(value: &Value, name: &str) -> Result<bool, EngineError> {
    match value {
        Value::Bool(v) => Ok(*v),
        _ => Err(EngineError::NativeType {
            name: sym(name),
            expected: "bool".into(),
            got: value.type_name().into(),
        }),
    }
}

fn option_value(value: &Value) -> Result<Option<Value>, EngineError> {
    match value {
        Value::Adt(name, args) if sym_eq(name, "Some") && args.len() == 1 => {
            Ok(Some(args[0].clone()))
        }
        Value::Adt(name, args) if sym_eq(name, "None") && args.is_empty() => Ok(None),
        _ => Err(EngineError::NativeType {
            name: sym("option"),
            expected: "Option".into(),
            got: value.type_name().into(),
        }),
    }
}

fn option_from_value(value: Option<Value>) -> Value {
    match value {
        Some(v) => Value::Adt(sym("Some"), vec![v]),
        None => Value::Adt(sym("None"), vec![]),
    }
}

fn result_value(value: &Value) -> Result<Result<Value, Value>, EngineError> {
    match value {
        Value::Adt(name, args) if sym_eq(name, "Ok") && args.len() == 1 => Ok(Ok(args[0].clone())),
        Value::Adt(name, args) if sym_eq(name, "Err") && args.len() == 1 => {
            Ok(Err(args[0].clone()))
        }
        _ => Err(EngineError::NativeType {
            name: sym("result"),
            expected: "Result".into(),
            got: value.type_name().into(),
        }),
    }
}

fn result_from_value(value: Result<Value, Value>) -> Value {
    match value {
        Ok(v) => Value::Adt(sym("Ok"), vec![v]),
        Err(v) => Value::Adt(sym("Err"), vec![v]),
    }
}

fn binary_arg_types(name: &str, typ: &Type) -> Result<(Type, Type), EngineError> {
    let (lhs, rest) = split_fun(typ).ok_or_else(|| EngineError::NativeType {
        name: sym(name),
        expected: "binary function".into(),
        got: typ.to_string(),
    })?;
    let (rhs, _res) = split_fun(&rest).ok_or_else(|| EngineError::NativeType {
        name: sym(name),
        expected: "binary function".into(),
        got: typ.to_string(),
    })?;
    Ok((lhs, rhs))
}

fn split_fun_chain(name: &str, typ: &Type, count: usize) -> Result<(Vec<Type>, Type), EngineError> {
    let mut args = Vec::with_capacity(count);
    let mut cur = typ.clone();
    for _ in 0..count {
        let (arg, rest) = split_fun(&cur).ok_or_else(|| EngineError::NativeType {
            name: sym(name),
            expected: format!("function of arity {}", count),
            got: typ.to_string(),
        })?;
        args.push(arg);
        cur = rest;
    }
    Ok((args, cur))
}

fn list_type(elem: Type) -> Type {
    Type::app(Type::con("List", 1), elem)
}

fn array_type(elem: Type) -> Type {
    Type::app(Type::con("Array", 1), elem)
}

fn option_type(elem: Type) -> Type {
    Type::app(Type::con("Option", 1), elem)
}

fn result_type(ok: Type, err: Type) -> Type {
    Type::app(Type::app(Type::con("Result", 2), err), ok)
}

fn list_elem_type(typ: &Type, name: &str) -> Result<Type, EngineError> {
    match typ.as_ref() {
        TypeKind::App(head, elem) if matches!(head.as_ref(), TypeKind::Con(c) if sym_eq(&c.name, "List")) => {
            Ok(elem.clone())
        }
        _ => Err(EngineError::NativeType {
            name: sym(name),
            expected: "List a".into(),
            got: typ.to_string(),
        }),
    }
}

fn array_elem_type(typ: &Type, name: &str) -> Result<Type, EngineError> {
    match typ.as_ref() {
        TypeKind::App(head, elem) if matches!(head.as_ref(), TypeKind::Con(c) if sym_eq(&c.name, "Array")) => {
            Ok(elem.clone())
        }
        _ => Err(EngineError::NativeType {
            name: sym(name),
            expected: "Array a".into(),
            got: typ.to_string(),
        }),
    }
}

fn option_elem_type(typ: &Type, name: &str) -> Result<Type, EngineError> {
    match typ.as_ref() {
        TypeKind::App(head, elem) if matches!(head.as_ref(), TypeKind::Con(c) if sym_eq(&c.name, "Option")) => {
            Ok(elem.clone())
        }
        _ => Err(EngineError::NativeType {
            name: sym(name),
            expected: "Option a".into(),
            got: typ.to_string(),
        }),
    }
}

fn result_types(typ: &Type, name: &str) -> Result<(Type, Type), EngineError> {
    match typ.as_ref() {
        TypeKind::App(head, ok) => match head.as_ref() {
            TypeKind::App(head, err) if matches!(head.as_ref(), TypeKind::Con(c) if sym_eq(&c.name, "Result")) => {
                Ok((ok.clone(), err.clone()))
            }
            _ => Err(EngineError::NativeType {
                name: sym(name),
                expected: "Result e a".into(),
                got: typ.to_string(),
            }),
        },
        _ => Err(EngineError::NativeType {
            name: sym(name),
            expected: "Result e a".into(),
            got: typ.to_string(),
        }),
    }
}

fn tuple_elem_type(typ: &Type, name: &str) -> Result<Type, EngineError> {
    match typ.as_ref() {
        TypeKind::Tuple(elems) if !elems.is_empty() => {
            let first = elems[0].clone();
            for elem in elems.iter().skip(1) {
                if *elem != first {
                    return Err(EngineError::NativeType {
                        name: sym(name),
                        expected: first.to_string(),
                        got: elem.to_string(),
                    });
                }
            }
            Ok(first)
        }
        _ => Err(EngineError::NativeType {
            name: sym(name),
            expected: "tuple".into(),
            got: typ.to_string(),
        }),
    }
}

#[cfg(any())]
fn eq_value_by_type(
    engine: &Engine,
    op_name: &Symbol,
    typ: &Type,
    lhs: &Value,
    rhs: &Value,
) -> Result<bool, EngineError> {
    match typ.as_ref() {
        TypeKind::Con(tc) => match tc.name.as_ref() {
            "bool" => match (lhs, rhs) {
                (Value::Bool(a), Value::Bool(b)) => Ok(a == b),
                _ => Err(EngineError::NativeType {
                    name: op_name.clone(),
                    expected: tc.name.to_string(),
                    got: format!("{}, {}", lhs.type_name(), rhs.type_name()),
                }),
            },
            "u8" => match (lhs, rhs) {
                (Value::U8(a), Value::U8(b)) => Ok(a == b),
                _ => Err(EngineError::NativeType {
                    name: op_name.clone(),
                    expected: tc.name.to_string(),
                    got: format!("{}, {}", lhs.type_name(), rhs.type_name()),
                }),
            },
            "u16" => match (lhs, rhs) {
                (Value::U16(a), Value::U16(b)) => Ok(a == b),
                _ => Err(EngineError::NativeType {
                    name: op_name.clone(),
                    expected: tc.name.to_string(),
                    got: format!("{}, {}", lhs.type_name(), rhs.type_name()),
                }),
            },
            "u32" => match (lhs, rhs) {
                (Value::U32(a), Value::U32(b)) => Ok(a == b),
                _ => Err(EngineError::NativeType {
                    name: op_name.clone(),
                    expected: tc.name.to_string(),
                    got: format!("{}, {}", lhs.type_name(), rhs.type_name()),
                }),
            },
            "u64" => match (lhs, rhs) {
                (Value::U64(a), Value::U64(b)) => Ok(a == b),
                _ => Err(EngineError::NativeType {
                    name: op_name.clone(),
                    expected: tc.name.to_string(),
                    got: format!("{}, {}", lhs.type_name(), rhs.type_name()),
                }),
            },
            "i8" => match (lhs, rhs) {
                (Value::I8(a), Value::I8(b)) => Ok(a == b),
                _ => Err(EngineError::NativeType {
                    name: op_name.clone(),
                    expected: tc.name.to_string(),
                    got: format!("{}, {}", lhs.type_name(), rhs.type_name()),
                }),
            },
            "i16" => match (lhs, rhs) {
                (Value::I16(a), Value::I16(b)) => Ok(a == b),
                _ => Err(EngineError::NativeType {
                    name: op_name.clone(),
                    expected: tc.name.to_string(),
                    got: format!("{}, {}", lhs.type_name(), rhs.type_name()),
                }),
            },
            "i32" => match (lhs, rhs) {
                (Value::I32(a), Value::I32(b)) => Ok(a == b),
                _ => Err(EngineError::NativeType {
                    name: op_name.clone(),
                    expected: tc.name.to_string(),
                    got: format!("{}, {}", lhs.type_name(), rhs.type_name()),
                }),
            },
            "i64" => match (lhs, rhs) {
                (Value::I64(a), Value::I64(b)) => Ok(a == b),
                _ => Err(EngineError::NativeType {
                    name: op_name.clone(),
                    expected: tc.name.to_string(),
                    got: format!("{}, {}", lhs.type_name(), rhs.type_name()),
                }),
            },
            "f32" => match (lhs, rhs) {
                (Value::F32(a), Value::F32(b)) => Ok((*a - *b).abs() < f32::EPSILON),
                _ => Err(EngineError::NativeType {
                    name: op_name.clone(),
                    expected: tc.name.to_string(),
                    got: format!("{}, {}", lhs.type_name(), rhs.type_name()),
                }),
            },
            "f64" => match (lhs, rhs) {
                (Value::F64(a), Value::F64(b)) => Ok((*a - *b).abs() < f64::EPSILON),
                _ => Err(EngineError::NativeType {
                    name: op_name.clone(),
                    expected: tc.name.to_string(),
                    got: format!("{}, {}", lhs.type_name(), rhs.type_name()),
                }),
            },
            "string" => match (lhs, rhs) {
                (Value::String(a), Value::String(b)) => Ok(a == b),
                _ => Err(EngineError::NativeType {
                    name: op_name.clone(),
                    expected: tc.name.to_string(),
                    got: format!("{}, {}", lhs.type_name(), rhs.type_name()),
                }),
            },
            "uuid" => match (lhs, rhs) {
                (Value::Uuid(a), Value::Uuid(b)) => Ok(a == b),
                _ => Err(EngineError::NativeType {
                    name: op_name.clone(),
                    expected: tc.name.to_string(),
                    got: format!("{}, {}", lhs.type_name(), rhs.type_name()),
                }),
            },
            "datetime" => match (lhs, rhs) {
                (Value::DateTime(a), Value::DateTime(b)) => Ok(a == b),
                _ => Err(EngineError::NativeType {
                    name: op_name.clone(),
                    expected: tc.name.to_string(),
                    got: format!("{}, {}", lhs.type_name(), rhs.type_name()),
                }),
            },
            _ => Err(EngineError::NativeType {
                name: op_name.clone(),
                expected: tc.name.to_string(),
                got: format!("{}, {}", lhs.type_name(), rhs.type_name()),
            }),
        },
        TypeKind::App(head, elem) => {
            if let TypeKind::Con(tc) = head.as_ref() {
                if sym_eq(&tc.name, "List") {
                    let left = list_to_vec(lhs, op_name.as_ref())?;
                    let right = list_to_vec(rhs, op_name.as_ref())?;
                    if left.len() != right.len() {
                        return Ok(false);
                    }
                    for (l, r) in left.iter().zip(right.iter()) {
                        if !eq_via_class(engine, op_name, elem, l, r)? {
                            return Ok(false);
                        }
                    }
                    return Ok(true);
                }
                if sym_eq(&tc.name, "Option") {
                    return match (lhs, rhs) {
                        (Value::Adt(tag_a, _), Value::Adt(tag_b, _)) if tag_a != tag_b => Ok(false),
                        (Value::Adt(tag, _), Value::Adt(_, _)) if sym_eq(tag, "None") => Ok(true),
                        (Value::Adt(tag, args_a), Value::Adt(_, args_b)) if sym_eq(tag, "Some") => {
                            if args_a.len() != 1 || args_b.len() != 1 {
                                return Err(EngineError::NativeType {
                                    name: op_name.clone(),
                                    expected: "Option".into(),
                                    got: lhs.type_name().into(),
                                });
                            }
                            eq_via_class(engine, op_name, elem, &args_a[0], &args_b[0])
                        }
                        _ => Err(EngineError::NativeType {
                            name: op_name.clone(),
                            expected: "Option".into(),
                            got: format!("{}, {}", lhs.type_name(), rhs.type_name()),
                        }),
                    };
                }
                if sym_eq(&tc.name, "Array") {
                    return match (lhs, rhs) {
                        (Value::Array(left), Value::Array(right)) => {
                            if left.len() != right.len() {
                                return Ok(false);
                            }
                            for (l, r) in left.iter().zip(right.iter()) {
                                if !eq_via_class(engine, op_name, elem, l, r)? {
                                    return Ok(false);
                                }
                            }
                            Ok(true)
                        }
                        _ => Err(EngineError::NativeType {
                            name: op_name.clone(),
                            expected: "Array".into(),
                            got: format!("{}, {}", lhs.type_name(), rhs.type_name()),
                        }),
                    };
                }
            }
            if let TypeKind::App(head, err_ty) = head.as_ref() {
                if let TypeKind::Con(tc) = head.as_ref() {
                    if sym_eq(&tc.name, "Result") {
                        return match (lhs, rhs) {
                            (Value::Adt(tag_a, _), Value::Adt(tag_b, _)) if tag_a != tag_b => {
                                Ok(false)
                            }
                            (Value::Adt(tag, args_a), Value::Adt(_, args_b))
                                if sym_eq(tag, "Ok") =>
                            {
                                if args_a.len() != 1 || args_b.len() != 1 {
                                    return Err(EngineError::NativeType {
                                        name: op_name.clone(),
                                        expected: "Result".into(),
                                        got: lhs.type_name().into(),
                                    });
                                }
                                eq_via_class(engine, op_name, elem, &args_a[0], &args_b[0])
                            }
                            (Value::Adt(tag, args_a), Value::Adt(_, args_b))
                                if sym_eq(tag, "Err") =>
                            {
                                if args_a.len() != 1 || args_b.len() != 1 {
                                    return Err(EngineError::NativeType {
                                        name: op_name.clone(),
                                        expected: "Result".into(),
                                        got: lhs.type_name().into(),
                                    });
                                }
                                eq_via_class(engine, op_name, err_ty, &args_a[0], &args_b[0])
                            }
                            _ => Err(EngineError::NativeType {
                                name: op_name.clone(),
                                expected: "Result".into(),
                                got: format!("{}, {}", lhs.type_name(), rhs.type_name()),
                            }),
                        };
                    }
                }
            }
            Err(EngineError::NativeType {
                name: op_name.clone(),
                expected: typ.to_string(),
                got: format!("{}, {}", lhs.type_name(), rhs.type_name()),
            })
        }
        _ => {
            Err(EngineError::NativeType {
                name: op_name.clone(),
                expected: typ.to_string(),
                got: format!("{}, {}", lhs.type_name(), rhs.type_name()),
            })
        }
    }
}

fn cmp_value_by_type(
    op_name: &Symbol,
    typ: &Type,
    lhs: &Value,
    rhs: &Value,
) -> Result<std::cmp::Ordering, EngineError> {
    match typ.as_ref() {
        TypeKind::Con(tc) => match tc.name.as_ref() {
            "u8" => match (lhs, rhs) {
                (Value::U8(a), Value::U8(b)) => Ok(a.cmp(b)),
                _ => Err(EngineError::NativeType {
                    name: op_name.clone(),
                    expected: tc.name.to_string(),
                    got: format!("{}, {}", lhs.type_name(), rhs.type_name()),
                }),
            },
            "u16" => match (lhs, rhs) {
                (Value::U16(a), Value::U16(b)) => Ok(a.cmp(b)),
                _ => Err(EngineError::NativeType {
                    name: op_name.clone(),
                    expected: tc.name.to_string(),
                    got: format!("{}, {}", lhs.type_name(), rhs.type_name()),
                }),
            },
            "u32" => match (lhs, rhs) {
                (Value::U32(a), Value::U32(b)) => Ok(a.cmp(b)),
                _ => Err(EngineError::NativeType {
                    name: op_name.clone(),
                    expected: tc.name.to_string(),
                    got: format!("{}, {}", lhs.type_name(), rhs.type_name()),
                }),
            },
            "u64" => match (lhs, rhs) {
                (Value::U64(a), Value::U64(b)) => Ok(a.cmp(b)),
                _ => Err(EngineError::NativeType {
                    name: op_name.clone(),
                    expected: tc.name.to_string(),
                    got: format!("{}, {}", lhs.type_name(), rhs.type_name()),
                }),
            },
            "i8" => match (lhs, rhs) {
                (Value::I8(a), Value::I8(b)) => Ok(a.cmp(b)),
                _ => Err(EngineError::NativeType {
                    name: op_name.clone(),
                    expected: tc.name.to_string(),
                    got: format!("{}, {}", lhs.type_name(), rhs.type_name()),
                }),
            },
            "i16" => match (lhs, rhs) {
                (Value::I16(a), Value::I16(b)) => Ok(a.cmp(b)),
                _ => Err(EngineError::NativeType {
                    name: op_name.clone(),
                    expected: tc.name.to_string(),
                    got: format!("{}, {}", lhs.type_name(), rhs.type_name()),
                }),
            },
            "i32" => match (lhs, rhs) {
                (Value::I32(a), Value::I32(b)) => Ok(a.cmp(b)),
                _ => Err(EngineError::NativeType {
                    name: op_name.clone(),
                    expected: tc.name.to_string(),
                    got: format!("{}, {}", lhs.type_name(), rhs.type_name()),
                }),
            },
            "i64" => match (lhs, rhs) {
                (Value::I64(a), Value::I64(b)) => Ok(a.cmp(b)),
                _ => Err(EngineError::NativeType {
                    name: op_name.clone(),
                    expected: tc.name.to_string(),
                    got: format!("{}, {}", lhs.type_name(), rhs.type_name()),
                }),
            },
            "f32" => match (lhs, rhs) {
                (Value::F32(a), Value::F32(b)) => {
                    a.partial_cmp(b).ok_or_else(|| EngineError::NativeType {
                        name: op_name.clone(),
                        expected: tc.name.to_string(),
                        got: "nan".into(),
                    })
                }
                _ => Err(EngineError::NativeType {
                    name: op_name.clone(),
                    expected: tc.name.to_string(),
                    got: format!("{}, {}", lhs.type_name(), rhs.type_name()),
                }),
            },
            "f64" => match (lhs, rhs) {
                (Value::F64(a), Value::F64(b)) => {
                    a.partial_cmp(b).ok_or_else(|| EngineError::NativeType {
                        name: op_name.clone(),
                        expected: tc.name.to_string(),
                        got: "nan".into(),
                    })
                }
                _ => Err(EngineError::NativeType {
                    name: op_name.clone(),
                    expected: tc.name.to_string(),
                    got: format!("{}, {}", lhs.type_name(), rhs.type_name()),
                }),
            },
            "string" => match (lhs, rhs) {
                (Value::String(a), Value::String(b)) => Ok(a.cmp(b)),
                _ => Err(EngineError::NativeType {
                    name: op_name.clone(),
                    expected: tc.name.to_string(),
                    got: format!("{}, {}", lhs.type_name(), rhs.type_name()),
                }),
            },
            _ => Err(EngineError::NativeType {
                name: op_name.clone(),
                expected: "orderable".into(),
                got: typ.to_string(),
            }),
        },
        _ => Err(EngineError::NativeType {
            name: op_name.clone(),
            expected: "orderable".into(),
            got: typ.to_string(),
        }),
    }
}

#[cfg(any())]
fn eq_impl(engine: &Engine, call_type: &Type, args: &[Value]) -> Result<Value, EngineError> {
    let eq_name = sym("prim_eq");
    let (lhs_ty, rhs_ty) = binary_arg_types(eq_name.as_ref(), call_type)?;
    let subst = unify(&lhs_ty, &rhs_ty).map_err(|_| EngineError::NativeType {
        name: eq_name.clone(),
        expected: lhs_ty.to_string(),
        got: rhs_ty.to_string(),
    })?;
    let lhs_ty = lhs_ty.apply(&subst);
    let ok = eq_value_by_type(engine, &eq_name, &lhs_ty, &args[0], &args[1])?;
    Ok(Value::Bool(ok))
}

#[cfg(any())]
fn ne_impl(engine: &Engine, call_type: &Type, args: &[Value]) -> Result<Value, EngineError> {
    let ne_name = sym("prim_ne");
    let (lhs_ty, rhs_ty) = binary_arg_types(ne_name.as_ref(), call_type)?;
    let subst = unify(&lhs_ty, &rhs_ty).map_err(|_| EngineError::NativeType {
        name: ne_name.clone(),
        expected: lhs_ty.to_string(),
        got: rhs_ty.to_string(),
    })?;
    let lhs_ty = lhs_ty.apply(&subst);
    let ok = eq_value_by_type(engine, &ne_name, &lhs_ty, &args[0], &args[1])?;
    Ok(Value::Bool(!ok))
}

fn inject_prelude_adts(engine: &mut Engine) -> Result<(), EngineError> {
    let mut list_adt = engine.adt_decl("List", &["a"]);
    let a_name = sym("a");
    let a = list_adt
        .param_type(&a_name)
        .ok_or_else(|| EngineError::UnknownType(sym("List")))?;
    let list_a = list_adt.result_type();
    list_adt.add_variant(sym("Empty"), vec![]);
    list_adt.add_variant(sym("Cons"), vec![a, list_a]);
    engine.inject_adt(list_adt)?;

    let mut option_adt = engine.adt_decl("Option", &["t"]);
    let t_name = sym("t");
    let t = option_adt
        .param_type(&t_name)
        .ok_or_else(|| EngineError::UnknownType(sym("Option")))?;
    option_adt.add_variant(sym("Some"), vec![t]);
    option_adt.add_variant(sym("None"), vec![]);
    engine.inject_adt(option_adt)?;

    let mut result_adt = engine.adt_decl("Result", &["e", "t"]);
    let e_name = sym("e");
    let t_name = sym("t");
    let e = result_adt
        .param_type(&e_name)
        .ok_or_else(|| EngineError::UnknownType(sym("Result")))?;
    let t = result_adt
        .param_type(&t_name)
        .ok_or_else(|| EngineError::UnknownType(sym("Result")))?;
    result_adt.add_variant(sym("Err"), vec![e]);
    result_adt.add_variant(sym("Ok"), vec![t]);
    engine.inject_adt(result_adt)?;
    Ok(())
}

fn inject_equality_ops(engine: &mut Engine) -> Result<(), EngineError> {
    // Equality primitives are monomorphic overloads (same name, different
    // concrete types), matching the numeric `prim_add` style.
    engine.inject_fn2("prim_eq", |a: bool, b: bool| -> bool { a == b })?;
    engine.inject_fn2("prim_ne", |a: bool, b: bool| -> bool { a != b })?;

    engine.inject_fn2("prim_eq", |a: u8, b: u8| -> bool { a == b })?;
    engine.inject_fn2("prim_ne", |a: u8, b: u8| -> bool { a != b })?;
    engine.inject_fn2("prim_eq", |a: u16, b: u16| -> bool { a == b })?;
    engine.inject_fn2("prim_ne", |a: u16, b: u16| -> bool { a != b })?;
    engine.inject_fn2("prim_eq", |a: u32, b: u32| -> bool { a == b })?;
    engine.inject_fn2("prim_ne", |a: u32, b: u32| -> bool { a != b })?;
    engine.inject_fn2("prim_eq", |a: u64, b: u64| -> bool { a == b })?;
    engine.inject_fn2("prim_ne", |a: u64, b: u64| -> bool { a != b })?;

    engine.inject_fn2("prim_eq", |a: i8, b: i8| -> bool { a == b })?;
    engine.inject_fn2("prim_ne", |a: i8, b: i8| -> bool { a != b })?;
    engine.inject_fn2("prim_eq", |a: i16, b: i16| -> bool { a == b })?;
    engine.inject_fn2("prim_ne", |a: i16, b: i16| -> bool { a != b })?;
    engine.inject_fn2("prim_eq", |a: i32, b: i32| -> bool { a == b })?;
    engine.inject_fn2("prim_ne", |a: i32, b: i32| -> bool { a != b })?;
    engine.inject_fn2("prim_eq", |a: i64, b: i64| -> bool { a == b })?;
    engine.inject_fn2("prim_ne", |a: i64, b: i64| -> bool { a != b })?;

    engine.inject_fn2("prim_eq", |a: f32, b: f32| -> bool { a == b })?;
    engine.inject_fn2("prim_ne", |a: f32, b: f32| -> bool { a != b })?;
    engine.inject_fn2("prim_eq", |a: f64, b: f64| -> bool { a == b })?;
    engine.inject_fn2("prim_ne", |a: f64, b: f64| -> bool { a != b })?;

    engine.inject_fn2("prim_eq", |a: String, b: String| -> bool { a == b })?;
    engine.inject_fn2("prim_ne", |a: String, b: String| -> bool { a != b })?;
    engine.inject_fn2("prim_eq", |a: Uuid, b: Uuid| -> bool { a == b })?;
    engine.inject_fn2("prim_ne", |a: Uuid, b: Uuid| -> bool { a != b })?;
    engine.inject_fn2("prim_eq", |a: DateTime<Utc>, b: DateTime<Utc>| -> bool { a == b })?;
    engine.inject_fn2("prim_ne", |a: DateTime<Utc>, b: DateTime<Utc>| -> bool { a != b })?;

    // Array equality must respect `Eq a`. We can't express the loop without a
    // primitive, but we *can* express the element comparison: the primitive
    // calls `(==)` on each pair.
    {
        let a_tv = engine.types.supply.fresh(Some(sym("a")));
        let a = Type::var(a_tv.clone());
        let array_a = Type::app(Type::con("Array", 1), a);
        let bool_ty = Type::con("bool", 0);
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(
                array_a.clone(),
                Type::fun(array_a.clone(), bool_ty.clone()),
            ),
        );
        engine.inject_native_scheme_typed("prim_array_eq", scheme.clone(), 2, |engine, call_type, args| {
            let (lhs_ty, rhs_ty) = binary_arg_types("prim_array_eq", call_type)?;
            let subst = unify(&lhs_ty, &rhs_ty).map_err(|_| EngineError::NativeType {
                name: sym("prim_array_eq"),
                expected: lhs_ty.to_string(),
                got: rhs_ty.to_string(),
            })?;
            let array_ty = lhs_ty.apply(&subst);
            let elem_ty = array_elem_type(&array_ty, "prim_array_eq")?;
            let xs = expect_array(&args[0], "prim_array_eq")?;
            let ys = expect_array(&args[1], "prim_array_eq")?;
            if xs.len() != ys.len() {
                return Ok(Value::Bool(false));
            }

            let bool_ty = Type::con("bool", 0);
            let eq_ty = Type::fun(elem_ty.clone(), Type::fun(elem_ty.clone(), bool_ty.clone()));
            let step_ty = Type::fun(elem_ty.clone(), bool_ty);
            for (x, y) in xs.iter().zip(ys.iter()) {
                let f = Value::Overloaded(OverloadedFn::new(sym("=="), eq_ty.clone()));
                let f = apply(engine, f, x.clone(), Some(&eq_ty), Some(&elem_ty))?;
                let r = apply(engine, f, y.clone(), Some(&step_ty), Some(&elem_ty))?;
                if !expect_bool(&r, "prim_array_eq")? {
                    return Ok(Value::Bool(false));
                }
            }
            Ok(Value::Bool(true))
        })?;

        let scheme = Scheme::new(
            vec![a_tv],
            vec![],
            Type::fun(array_a.clone(), Type::fun(array_a, bool_ty.clone())),
        );
        engine.inject_native_scheme_typed("prim_array_ne", scheme, 2, |engine, call_type, args| {
            let imp = engine.resolve_native_impl("prim_array_eq", call_type)?;
            let eq = imp.func.call_sync(engine, call_type, args)?;
            Ok(Value::Bool(!expect_bool(&eq, "prim_array_ne")?))
        })?;
    }

    Ok(())
}

fn inject_order_ops(engine: &mut Engine) -> Result<(), EngineError> {
    fn cmp_to_i32(ord: std::cmp::Ordering) -> i32 {
        match ord {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        }
    }

    // Integer and string comparisons can be injected as direct typed natives,
    // with no runtime type switching.
    engine.inject_fn2("prim_lt", |a: u8, b: u8| -> bool { a < b })?;
    engine.inject_fn2("prim_le", |a: u8, b: u8| -> bool { a <= b })?;
    engine.inject_fn2("prim_gt", |a: u8, b: u8| -> bool { a > b })?;
    engine.inject_fn2("prim_ge", |a: u8, b: u8| -> bool { a >= b })?;
    engine.inject_fn2("prim_cmp", |a: u8, b: u8| -> i32 { cmp_to_i32(a.cmp(&b)) })?;

    engine.inject_fn2("prim_lt", |a: u16, b: u16| -> bool { a < b })?;
    engine.inject_fn2("prim_le", |a: u16, b: u16| -> bool { a <= b })?;
    engine.inject_fn2("prim_gt", |a: u16, b: u16| -> bool { a > b })?;
    engine.inject_fn2("prim_ge", |a: u16, b: u16| -> bool { a >= b })?;
    engine.inject_fn2("prim_cmp", |a: u16, b: u16| -> i32 { cmp_to_i32(a.cmp(&b)) })?;

    engine.inject_fn2("prim_lt", |a: u32, b: u32| -> bool { a < b })?;
    engine.inject_fn2("prim_le", |a: u32, b: u32| -> bool { a <= b })?;
    engine.inject_fn2("prim_gt", |a: u32, b: u32| -> bool { a > b })?;
    engine.inject_fn2("prim_ge", |a: u32, b: u32| -> bool { a >= b })?;
    engine.inject_fn2("prim_cmp", |a: u32, b: u32| -> i32 { cmp_to_i32(a.cmp(&b)) })?;

    engine.inject_fn2("prim_lt", |a: u64, b: u64| -> bool { a < b })?;
    engine.inject_fn2("prim_le", |a: u64, b: u64| -> bool { a <= b })?;
    engine.inject_fn2("prim_gt", |a: u64, b: u64| -> bool { a > b })?;
    engine.inject_fn2("prim_ge", |a: u64, b: u64| -> bool { a >= b })?;
    engine.inject_fn2("prim_cmp", |a: u64, b: u64| -> i32 { cmp_to_i32(a.cmp(&b)) })?;

    engine.inject_fn2("prim_lt", |a: i8, b: i8| -> bool { a < b })?;
    engine.inject_fn2("prim_le", |a: i8, b: i8| -> bool { a <= b })?;
    engine.inject_fn2("prim_gt", |a: i8, b: i8| -> bool { a > b })?;
    engine.inject_fn2("prim_ge", |a: i8, b: i8| -> bool { a >= b })?;
    engine.inject_fn2("prim_cmp", |a: i8, b: i8| -> i32 { cmp_to_i32(a.cmp(&b)) })?;

    engine.inject_fn2("prim_lt", |a: i16, b: i16| -> bool { a < b })?;
    engine.inject_fn2("prim_le", |a: i16, b: i16| -> bool { a <= b })?;
    engine.inject_fn2("prim_gt", |a: i16, b: i16| -> bool { a > b })?;
    engine.inject_fn2("prim_ge", |a: i16, b: i16| -> bool { a >= b })?;
    engine.inject_fn2("prim_cmp", |a: i16, b: i16| -> i32 { cmp_to_i32(a.cmp(&b)) })?;

    engine.inject_fn2("prim_lt", |a: i32, b: i32| -> bool { a < b })?;
    engine.inject_fn2("prim_le", |a: i32, b: i32| -> bool { a <= b })?;
    engine.inject_fn2("prim_gt", |a: i32, b: i32| -> bool { a > b })?;
    engine.inject_fn2("prim_ge", |a: i32, b: i32| -> bool { a >= b })?;
    engine.inject_fn2("prim_cmp", |a: i32, b: i32| -> i32 { cmp_to_i32(a.cmp(&b)) })?;

    engine.inject_fn2("prim_lt", |a: i64, b: i64| -> bool { a < b })?;
    engine.inject_fn2("prim_le", |a: i64, b: i64| -> bool { a <= b })?;
    engine.inject_fn2("prim_gt", |a: i64, b: i64| -> bool { a > b })?;
    engine.inject_fn2("prim_ge", |a: i64, b: i64| -> bool { a >= b })?;
    engine.inject_fn2("prim_cmp", |a: i64, b: i64| -> i32 { cmp_to_i32(a.cmp(&b)) })?;

    engine.inject_fn2("prim_lt", |a: String, b: String| -> bool { a < b })?;
    engine.inject_fn2("prim_le", |a: String, b: String| -> bool { a <= b })?;
    engine.inject_fn2("prim_gt", |a: String, b: String| -> bool { a > b })?;
    engine.inject_fn2("prim_ge", |a: String, b: String| -> bool { a >= b })?;
    engine.inject_fn2("prim_cmp", |a: String, b: String| -> i32 { cmp_to_i32(a.cmp(&b)) })?;

    // Floats: preserve the existing “NaN is a type error” semantics.
    let bool_ty = Type::con("bool", 0);
    let i32_ty = Type::con("i32", 0);

    let f32_ty = Type::con("f32", 0);
    let f32_bool = Scheme::new(vec![], vec![], Type::fun(f32_ty.clone(), Type::fun(f32_ty.clone(), bool_ty.clone())));
    let f32_cmp = Scheme::new(vec![], vec![], Type::fun(f32_ty.clone(), Type::fun(f32_ty.clone(), i32_ty.clone())));
    for (name, pred) in [
        ("prim_lt", (|o: std::cmp::Ordering| o == std::cmp::Ordering::Less) as fn(std::cmp::Ordering) -> bool),
        ("prim_le", (|o: std::cmp::Ordering| o != std::cmp::Ordering::Greater) as fn(std::cmp::Ordering) -> bool),
        ("prim_gt", (|o: std::cmp::Ordering| o == std::cmp::Ordering::Greater) as fn(std::cmp::Ordering) -> bool),
        ("prim_ge", (|o: std::cmp::Ordering| o != std::cmp::Ordering::Less) as fn(std::cmp::Ordering) -> bool),
    ] {
        let scheme = f32_bool.clone();
        engine.inject_native_scheme_typed(name, scheme, 2, move |_engine, _call_type, args| {
            let Value::F32(a) = &args[0] else {
                return Err(EngineError::NativeType {
                    name: sym(name),
                    expected: "f32".into(),
                    got: args[0].type_name().into(),
                });
            };
            let Value::F32(b) = &args[1] else {
                return Err(EngineError::NativeType {
                    name: sym(name),
                    expected: "f32".into(),
                    got: args[1].type_name().into(),
                });
            };
            let ord = a.partial_cmp(b).ok_or_else(|| EngineError::NativeType {
                name: sym(name),
                expected: "f32".into(),
                got: "nan".into(),
            })?;
            Ok(Value::Bool(pred(ord)))
        })?;
    }
    engine.inject_native_scheme_typed("prim_cmp", f32_cmp, 2, |_engine, _call_type, args| {
        let Value::F32(a) = &args[0] else {
            return Err(EngineError::NativeType {
                name: sym("prim_cmp"),
                expected: "f32".into(),
                got: args[0].type_name().into(),
            });
        };
        let Value::F32(b) = &args[1] else {
            return Err(EngineError::NativeType {
                name: sym("prim_cmp"),
                expected: "f32".into(),
                got: args[1].type_name().into(),
            });
        };
        let ord = a.partial_cmp(b).ok_or_else(|| EngineError::NativeType {
            name: sym("prim_cmp"),
            expected: "f32".into(),
            got: "nan".into(),
        })?;
        Ok(Value::I32(cmp_to_i32(ord)))
    })?;

    let f64_ty = Type::con("f64", 0);
    let f64_bool = Scheme::new(vec![], vec![], Type::fun(f64_ty.clone(), Type::fun(f64_ty.clone(), bool_ty.clone())));
    let f64_cmp = Scheme::new(vec![], vec![], Type::fun(f64_ty.clone(), Type::fun(f64_ty.clone(), i32_ty)));
    for (name, pred) in [
        ("prim_lt", (|o: std::cmp::Ordering| o == std::cmp::Ordering::Less) as fn(std::cmp::Ordering) -> bool),
        ("prim_le", (|o: std::cmp::Ordering| o != std::cmp::Ordering::Greater) as fn(std::cmp::Ordering) -> bool),
        ("prim_gt", (|o: std::cmp::Ordering| o == std::cmp::Ordering::Greater) as fn(std::cmp::Ordering) -> bool),
        ("prim_ge", (|o: std::cmp::Ordering| o != std::cmp::Ordering::Less) as fn(std::cmp::Ordering) -> bool),
    ] {
        let scheme = f64_bool.clone();
        engine.inject_native_scheme_typed(name, scheme, 2, move |_engine, _call_type, args| {
            let Value::F64(a) = &args[0] else {
                return Err(EngineError::NativeType {
                    name: sym(name),
                    expected: "f64".into(),
                    got: args[0].type_name().into(),
                });
            };
            let Value::F64(b) = &args[1] else {
                return Err(EngineError::NativeType {
                    name: sym(name),
                    expected: "f64".into(),
                    got: args[1].type_name().into(),
                });
            };
            let ord = a.partial_cmp(b).ok_or_else(|| EngineError::NativeType {
                name: sym(name),
                expected: "f64".into(),
                got: "nan".into(),
            })?;
            Ok(Value::Bool(pred(ord)))
        })?;
    }
    engine.inject_native_scheme_typed("prim_cmp", f64_cmp, 2, |_engine, _call_type, args| {
        let Value::F64(a) = &args[0] else {
            return Err(EngineError::NativeType {
                name: sym("prim_cmp"),
                expected: "f64".into(),
                got: args[0].type_name().into(),
            });
        };
        let Value::F64(b) = &args[1] else {
            return Err(EngineError::NativeType {
                name: sym("prim_cmp"),
                expected: "f64".into(),
                got: args[1].type_name().into(),
            });
        };
        let ord = a.partial_cmp(b).ok_or_else(|| EngineError::NativeType {
            name: sym("prim_cmp"),
            expected: "f64".into(),
            got: "nan".into(),
        })?;
        Ok(Value::I32(cmp_to_i32(ord)))
    })?;

    Ok(())
}

fn inject_pretty_ops(engine: &mut Engine) -> Result<(), EngineError> {
    engine.inject_fn1("prim_pretty", |x: bool| -> String { x.to_string() })?;
    engine.inject_fn1("prim_pretty", |x: u8| -> String { x.to_string() })?;
    engine.inject_fn1("prim_pretty", |x: u16| -> String { x.to_string() })?;
    engine.inject_fn1("prim_pretty", |x: u32| -> String { x.to_string() })?;
    engine.inject_fn1("prim_pretty", |x: u64| -> String { x.to_string() })?;
    engine.inject_fn1("prim_pretty", |x: i8| -> String { x.to_string() })?;
    engine.inject_fn1("prim_pretty", |x: i16| -> String { x.to_string() })?;
    engine.inject_fn1("prim_pretty", |x: i32| -> String { x.to_string() })?;
    engine.inject_fn1("prim_pretty", |x: i64| -> String { x.to_string() })?;
    engine.inject_fn1("prim_pretty", |x: f32| -> String { x.to_string() })?;
    engine.inject_fn1("prim_pretty", |x: f64| -> String { x.to_string() })?;
    engine.inject_fn1("prim_pretty", |x: String| -> String { x })?;
    engine.inject_fn1("prim_pretty", |x: Uuid| -> String { x.to_string() })?;
    engine.inject_fn1("prim_pretty", |x: DateTime<Utc>| -> String { x.to_string() })?;
    Ok(())
}

fn inject_boolean_ops(engine: &mut Engine) -> Result<(), EngineError> {
    engine.inject_fn2("(&&)", |a: bool, b: bool| -> bool { a && b })?;
    engine.inject_fn2("(||)", |a: bool, b: bool| -> bool { a || b })?;
    Ok(())
}

fn inject_numeric_ops(engine: &mut Engine) -> Result<(), EngineError> {
    // Additive identity
    engine.inject_value("prim_zero", String::new())?;
    engine.inject_value("prim_zero", 0u8)?;
    engine.inject_value("prim_zero", 0u16)?;
    engine.inject_value("prim_zero", 0u32)?;
    engine.inject_value("prim_zero", 0u64)?;
    engine.inject_value("prim_zero", 0i8)?;
    engine.inject_value("prim_zero", 0i16)?;
    engine.inject_value("prim_zero", 0i32)?;
    engine.inject_value("prim_zero", 0i64)?;
    engine.inject_value("prim_zero", 0.0f32)?;
    engine.inject_value("prim_zero", 0.0f64)?;

    // Multiplicative identity
    engine.inject_value("prim_one", 1u8)?;
    engine.inject_value("prim_one", 1u16)?;
    engine.inject_value("prim_one", 1u32)?;
    engine.inject_value("prim_one", 1u64)?;
    engine.inject_value("prim_one", 1i8)?;
    engine.inject_value("prim_one", 1i16)?;
    engine.inject_value("prim_one", 1i32)?;
    engine.inject_value("prim_one", 1i64)?;
    engine.inject_value("prim_one", 1.0f32)?;
    engine.inject_value("prim_one", 1.0f64)?;

    // Addition
    engine.inject_fn2("prim_add", |a: u8, b: u8| -> u8 { a + b })?;
    engine.inject_fn2("prim_add", |a: u16, b: u16| -> u16 { a + b })?;
    engine.inject_fn2("prim_add", |a: u32, b: u32| -> u32 { a + b })?;
    engine.inject_fn2("prim_add", |a: u64, b: u64| -> u64 { a + b })?;
    engine.inject_fn2("prim_add", |a: i8, b: i8| -> i8 { a + b })?;
    engine.inject_fn2("prim_add", |a: i16, b: i16| -> i16 { a + b })?;
    engine.inject_fn2("prim_add", |a: i32, b: i32| -> i32 { a + b })?;
    engine.inject_fn2("prim_add", |a: i64, b: i64| -> i64 { a + b })?;
    engine.inject_fn2("prim_add", |a: f32, b: f32| -> f32 { a + b })?;
    engine.inject_fn2("prim_add", |a: f64, b: f64| -> f64 { a + b })?;
    engine.inject_fn2("prim_add", |a: String, b: String| -> String { format!("{}{}", a, b) })?;

    // Subtraction and negation
    engine.inject_fn2("prim_sub", |a: i8, b: i8| -> i8 { a - b })?;
    engine.inject_fn2("prim_sub", |a: i16, b: i16| -> i16 { a - b })?;
    engine.inject_fn2("prim_sub", |a: i32, b: i32| -> i32 { a - b })?;
    engine.inject_fn2("prim_sub", |a: i64, b: i64| -> i64 { a - b })?;
    engine.inject_fn2("prim_sub", |a: f32, b: f32| -> f32 { a - b })?;
    engine.inject_fn2("prim_sub", |a: f64, b: f64| -> f64 { a - b })?;
    engine.inject_fn1("prim_negate", |a: i8| -> i8 { -a })?;
    engine.inject_fn1("prim_negate", |a: i16| -> i16 { -a })?;
    engine.inject_fn1("prim_negate", |a: i32| -> i32 { -a })?;
    engine.inject_fn1("prim_negate", |a: i64| -> i64 { -a })?;
    engine.inject_fn1("prim_negate", |a: f32| -> f32 { -a })?;
    engine.inject_fn1("prim_negate", |a: f64| -> f64 { -a })?;

    // Multiplication and division
    engine.inject_fn2("prim_mul", |a: u8, b: u8| -> u8 { a * b })?;
    engine.inject_fn2("prim_mul", |a: u16, b: u16| -> u16 { a * b })?;
    engine.inject_fn2("prim_mul", |a: u32, b: u32| -> u32 { a * b })?;
    engine.inject_fn2("prim_mul", |a: u64, b: u64| -> u64 { a * b })?;
    engine.inject_fn2("prim_mul", |a: i8, b: i8| -> i8 { a * b })?;
    engine.inject_fn2("prim_mul", |a: i16, b: i16| -> i16 { a * b })?;
    engine.inject_fn2("prim_mul", |a: i32, b: i32| -> i32 { a * b })?;
    engine.inject_fn2("prim_mul", |a: i64, b: i64| -> i64 { a * b })?;
    engine.inject_fn2("prim_mul", |a: f32, b: f32| -> f32 { a * b })?;
    engine.inject_fn2("prim_mul", |a: f64, b: f64| -> f64 { a * b })?;
    engine.inject_fn2("prim_div", |a: f32, b: f32| -> f32 { a / b })?;
    engine.inject_fn2("prim_div", |a: f64, b: f64| -> f64 { a / b })?;

    // Remainder
    engine.inject_fn2("prim_mod", |a: u8, b: u8| -> u8 { a % b })?;
    engine.inject_fn2("prim_mod", |a: u16, b: u16| -> u16 { a % b })?;
    engine.inject_fn2("prim_mod", |a: u32, b: u32| -> u32 { a % b })?;
    engine.inject_fn2("prim_mod", |a: u64, b: u64| -> u64 { a % b })?;
    engine.inject_fn2("prim_mod", |a: i8, b: i8| -> i8 { a % b })?;
    engine.inject_fn2("prim_mod", |a: i16, b: i16| -> i16 { a % b })?;
    engine.inject_fn2("prim_mod", |a: i32, b: i32| -> i32 { a % b })?;
    engine.inject_fn2("prim_mod", |a: i64, b: i64| -> i64 { a % b })?;
    Ok(())
}

fn inject_list_builtins(engine: &mut Engine) -> Result<(), EngineError> {
    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let b_tv = engine.types.supply.fresh(Some("b".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let list_a = list_type(a.clone());
        let list_b = list_type(b.clone());
        let scheme = Scheme::new(
            vec![a_tv, b_tv],
            vec![],
            Type::fun(
                Type::fun(a.clone(), b.clone()),
                Type::fun(list_a.clone(), list_b),
            ),
        );
        engine.inject_native_scheme_typed("prim_map", scheme, 2, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain("prim_map", call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let list_ty = arg_tys[1].clone();
            let elem_ty = list_elem_type(&list_ty, "prim_map")?;
            let mut out = Vec::new();
            for value in list_to_vec(&args[1], "prim_map")? {
                let mapped = apply(
                    engine,
                    args[0].clone(),
                    value,
                    Some(&func_ty),
                    Some(&elem_ty),
                )?;
                out.push(mapped);
            }
            Ok(list_from_vec(out))
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let b_tv = engine.types.supply.fresh(Some("b".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let array_a = array_type(a.clone());
        let array_b = array_type(b.clone());
        let scheme = Scheme::new(
            vec![a_tv, b_tv],
            vec![],
            Type::fun(
                Type::fun(a.clone(), b.clone()),
                Type::fun(array_a.clone(), array_b),
            ),
        );
        engine.inject_native_scheme_typed("prim_map", scheme, 2, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain("prim_map", call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let array_ty = arg_tys[1].clone();
            let elem_ty = array_elem_type(&array_ty, "prim_map")?;
            let mut out = Vec::new();
            for value in expect_array(&args[1], "prim_map")? {
                let mapped = apply(
                    engine,
                    args[0].clone(),
                    value,
                    Some(&func_ty),
                    Some(&elem_ty),
                )?;
                out.push(mapped);
            }
            Ok(Value::Array(out))
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let array_a = array_type(a.clone());
        let scheme = Scheme::new(vec![a_tv], vec![], Type::fun(a, array_a));
        engine.inject_native_scheme_typed(
            "prim_array_singleton",
            scheme,
            1,
            |_engine, _call_type, args| Ok(Value::Array(vec![args[0].clone()])),
        )?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let b_tv = engine.types.supply.fresh(Some("b".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let opt_a = option_type(a.clone());
        let opt_b = option_type(b.clone());
        let scheme = Scheme::new(
            vec![a_tv, b_tv],
            vec![],
            Type::fun(
                Type::fun(a.clone(), b.clone()),
                Type::fun(opt_a.clone(), opt_b),
            ),
        );
        engine.inject_native_scheme_typed("prim_map", scheme, 2, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain("prim_map", call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let opt_ty = arg_tys[1].clone();
            let elem_ty = option_elem_type(&opt_ty, "prim_map")?;
            match option_value(&args[1])? {
                Some(v) => {
                    let mapped = apply(engine, args[0].clone(), v, Some(&func_ty), Some(&elem_ty))?;
                    Ok(option_from_value(Some(mapped)))
                }
                None => Ok(option_from_value(None)),
            }
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let b_tv = engine.types.supply.fresh(Some("b".into()));
        let e_tv = engine.types.supply.fresh(Some("e".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let e = Type::var(e_tv.clone());
        let result_a = result_type(a.clone(), e.clone());
        let result_b = result_type(b.clone(), e.clone());
        let scheme = Scheme::new(
            vec![a_tv, b_tv, e_tv],
            vec![],
            Type::fun(
                Type::fun(a.clone(), b.clone()),
                Type::fun(result_a.clone(), result_b),
            ),
        );
        engine.inject_native_scheme_typed("prim_map", scheme, 2, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain("prim_map", call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let result_ty = arg_tys[1].clone();
            let (ok_ty, _err_ty) = result_types(&result_ty, "prim_map")?;
            match result_value(&args[1])? {
                Ok(v) => {
                    let mapped = apply(engine, args[0].clone(), v, Some(&func_ty), Some(&ok_ty))?;
                    Ok(result_from_value(Ok(mapped)))
                }
                Err(e) => Ok(result_from_value(Err(e))),
            }
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let b_tv = engine.types.supply.fresh(Some("b".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let list_a = list_type(a.clone());
        let scheme = Scheme::new(
            vec![a_tv, b_tv],
            vec![],
            Type::fun(
                Type::fun(b.clone(), Type::fun(a.clone(), b.clone())),
                Type::fun(b.clone(), Type::fun(list_a.clone(), b.clone())),
            ),
        );
        engine.inject_native_scheme_typed("prim_foldl", scheme, 3, |engine, call_type, args| {
            let (arg_tys, res_ty) = split_fun_chain("prim_foldl", call_type, 3)?;
            let func_ty = arg_tys[0].clone();
            let acc_ty = arg_tys[1].clone();
            let list_ty = arg_tys[2].clone();
            if acc_ty != res_ty {
                return Err(EngineError::NativeType {
                    name: sym("prim_foldl"),
                    expected: acc_ty.to_string(),
                    got: res_ty.to_string(),
                });
            }
            let elem_ty = list_elem_type(&list_ty, "prim_foldl")?;
            let step_ty = Type::fun(elem_ty.clone(), acc_ty.clone());
            let mut acc = args[1].clone();
            for value in list_to_vec(&args[2], "prim_foldl")? {
                let step = apply(engine, args[0].clone(), acc, Some(&func_ty), Some(&acc_ty))?;
                acc = apply(engine, step, value, Some(&step_ty), Some(&elem_ty))?;
            }
            Ok(acc)
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let b_tv = engine.types.supply.fresh(Some("b".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let array_a = array_type(a.clone());
        let scheme = Scheme::new(
            vec![a_tv, b_tv],
            vec![],
            Type::fun(
                Type::fun(b.clone(), Type::fun(a.clone(), b.clone())),
                Type::fun(b.clone(), Type::fun(array_a.clone(), b.clone())),
            ),
        );
        engine.inject_native_scheme_typed("prim_foldl", scheme, 3, |engine, call_type, args| {
            let (arg_tys, res_ty) = split_fun_chain("prim_foldl", call_type, 3)?;
            let func_ty = arg_tys[0].clone();
            let acc_ty = arg_tys[1].clone();
            let array_ty = arg_tys[2].clone();
            if acc_ty != res_ty {
                return Err(EngineError::NativeType {
                    name: sym("prim_foldl"),
                    expected: acc_ty.to_string(),
                    got: res_ty.to_string(),
                });
            }
            let elem_ty = array_elem_type(&array_ty, "prim_foldl")?;
            let step_ty = Type::fun(elem_ty.clone(), acc_ty.clone());
            let mut acc = args[1].clone();
            for value in expect_array(&args[2], "prim_foldl")? {
                let step = apply(engine, args[0].clone(), acc, Some(&func_ty), Some(&acc_ty))?;
                acc = apply(engine, step, value, Some(&step_ty), Some(&elem_ty))?;
            }
            Ok(acc)
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let b_tv = engine.types.supply.fresh(Some("b".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let opt_a = option_type(a.clone());
        let scheme = Scheme::new(
            vec![a_tv, b_tv],
            vec![],
            Type::fun(
                Type::fun(b.clone(), Type::fun(a.clone(), b.clone())),
                Type::fun(b.clone(), Type::fun(opt_a.clone(), b.clone())),
            ),
        );
        engine.inject_native_scheme_typed("prim_foldl", scheme, 3, |engine, call_type, args| {
            let (arg_tys, res_ty) = split_fun_chain("prim_foldl", call_type, 3)?;
            let func_ty = arg_tys[0].clone();
            let acc_ty = arg_tys[1].clone();
            let opt_ty = arg_tys[2].clone();
            if acc_ty != res_ty {
                return Err(EngineError::NativeType {
                    name: sym("prim_foldl"),
                    expected: acc_ty.to_string(),
                    got: res_ty.to_string(),
                });
            }
            let elem_ty = option_elem_type(&opt_ty, "prim_foldl")?;
            let step_ty = Type::fun(elem_ty.clone(), acc_ty.clone());
            let acc = args[1].clone();
            match option_value(&args[2])? {
                Some(value) => {
                    let step = apply(engine, args[0].clone(), acc, Some(&func_ty), Some(&acc_ty))?;
                    apply(engine, step, value, Some(&step_ty), Some(&elem_ty))
                }
                None => Ok(acc),
            }
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let b_tv = engine.types.supply.fresh(Some("b".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let list_a = list_type(a.clone());
        let scheme = Scheme::new(
            vec![a_tv, b_tv],
            vec![],
            Type::fun(
                Type::fun(a.clone(), Type::fun(b.clone(), b.clone())),
                Type::fun(b.clone(), Type::fun(list_a.clone(), b.clone())),
            ),
        );
        engine.inject_native_scheme_typed("prim_foldr", scheme, 3, |engine, call_type, args| {
            let (arg_tys, res_ty) = split_fun_chain("prim_foldr", call_type, 3)?;
            let func_ty = arg_tys[0].clone();
            let acc_ty = arg_tys[1].clone();
            let list_ty = arg_tys[2].clone();
            if acc_ty != res_ty {
                return Err(EngineError::NativeType {
                    name: sym("prim_foldr"),
                    expected: acc_ty.to_string(),
                    got: res_ty.to_string(),
                });
            }
            let elem_ty = list_elem_type(&list_ty, "prim_foldr")?;
            let step_ty = Type::fun(acc_ty.clone(), acc_ty.clone());
            let mut acc = args[1].clone();
            let values = list_to_vec(&args[2], "prim_foldr")?;
            for value in values.into_iter().rev() {
                let step = apply(
                    engine,
                    args[0].clone(),
                    value,
                    Some(&func_ty),
                    Some(&elem_ty),
                )?;
                acc = apply(engine, step, acc, Some(&step_ty), Some(&acc_ty))?;
            }
            Ok(acc)
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let b_tv = engine.types.supply.fresh(Some("b".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let array_a = array_type(a.clone());
        let scheme = Scheme::new(
            vec![a_tv, b_tv],
            vec![],
            Type::fun(
                Type::fun(a.clone(), Type::fun(b.clone(), b.clone())),
                Type::fun(b.clone(), Type::fun(array_a.clone(), b.clone())),
            ),
        );
        engine.inject_native_scheme_typed("prim_foldr", scheme, 3, |engine, call_type, args| {
            let (arg_tys, res_ty) = split_fun_chain("prim_foldr", call_type, 3)?;
            let func_ty = arg_tys[0].clone();
            let acc_ty = arg_tys[1].clone();
            let array_ty = arg_tys[2].clone();
            if acc_ty != res_ty {
                return Err(EngineError::NativeType {
                    name: sym("prim_foldr"),
                    expected: acc_ty.to_string(),
                    got: res_ty.to_string(),
                });
            }
            let elem_ty = array_elem_type(&array_ty, "prim_foldr")?;
            let step_ty = Type::fun(acc_ty.clone(), acc_ty.clone());
            let mut acc = args[1].clone();
            let values = expect_array(&args[2], "prim_foldr")?;
            for value in values.into_iter().rev() {
                let step = apply(
                    engine,
                    args[0].clone(),
                    value,
                    Some(&func_ty),
                    Some(&elem_ty),
                )?;
                acc = apply(engine, step, acc, Some(&step_ty), Some(&acc_ty))?;
            }
            Ok(acc)
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let b_tv = engine.types.supply.fresh(Some("b".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let opt_a = option_type(a.clone());
        let scheme = Scheme::new(
            vec![a_tv, b_tv],
            vec![],
            Type::fun(
                Type::fun(a.clone(), Type::fun(b.clone(), b.clone())),
                Type::fun(b.clone(), Type::fun(opt_a.clone(), b.clone())),
            ),
        );
        engine.inject_native_scheme_typed("prim_foldr", scheme, 3, |engine, call_type, args| {
            let (arg_tys, res_ty) = split_fun_chain("prim_foldr", call_type, 3)?;
            let func_ty = arg_tys[0].clone();
            let acc_ty = arg_tys[1].clone();
            let opt_ty = arg_tys[2].clone();
            if acc_ty != res_ty {
                return Err(EngineError::NativeType {
                    name: sym("prim_foldr"),
                    expected: acc_ty.to_string(),
                    got: res_ty.to_string(),
                });
            }
            let elem_ty = option_elem_type(&opt_ty, "prim_foldr")?;
            let step_ty = Type::fun(acc_ty.clone(), acc_ty.clone());
            let acc = args[1].clone();
            match option_value(&args[2])? {
                Some(value) => {
                    let step = apply(
                        engine,
                        args[0].clone(),
                        value,
                        Some(&func_ty),
                        Some(&elem_ty),
                    )?;
                    apply(engine, step, acc, Some(&step_ty), Some(&acc_ty))
                }
                None => Ok(acc),
            }
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let b_tv = engine.types.supply.fresh(Some("b".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let list_a = list_type(a.clone());
        let scheme = Scheme::new(
            vec![a_tv, b_tv],
            vec![],
            Type::fun(
                Type::fun(b.clone(), Type::fun(a.clone(), b.clone())),
                Type::fun(b.clone(), Type::fun(list_a.clone(), b.clone())),
            ),
        );
        engine.inject_native_scheme_typed("prim_fold", scheme, 3, |engine, call_type, args| {
            let (arg_tys, res_ty) = split_fun_chain("prim_fold", call_type, 3)?;
            let func_ty = arg_tys[0].clone();
            let acc_ty = arg_tys[1].clone();
            let list_ty = arg_tys[2].clone();
            if acc_ty != res_ty {
                return Err(EngineError::NativeType {
                    name: sym("prim_fold"),
                    expected: acc_ty.to_string(),
                    got: res_ty.to_string(),
                });
            }
            let elem_ty = list_elem_type(&list_ty, "prim_fold")?;
            let step_ty = Type::fun(elem_ty.clone(), acc_ty.clone());
            let mut acc = args[1].clone();
            for value in list_to_vec(&args[2], "prim_fold")? {
                let step = apply(engine, args[0].clone(), acc, Some(&func_ty), Some(&acc_ty))?;
                acc = apply(engine, step, value, Some(&step_ty), Some(&elem_ty))?;
            }
            Ok(acc)
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let b_tv = engine.types.supply.fresh(Some("b".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let array_a = array_type(a.clone());
        let scheme = Scheme::new(
            vec![a_tv, b_tv],
            vec![],
            Type::fun(
                Type::fun(b.clone(), Type::fun(a.clone(), b.clone())),
                Type::fun(b.clone(), Type::fun(array_a.clone(), b.clone())),
            ),
        );
        engine.inject_native_scheme_typed("prim_fold", scheme, 3, |engine, call_type, args| {
            let (arg_tys, res_ty) = split_fun_chain("prim_fold", call_type, 3)?;
            let func_ty = arg_tys[0].clone();
            let acc_ty = arg_tys[1].clone();
            let array_ty = arg_tys[2].clone();
            if acc_ty != res_ty {
                return Err(EngineError::NativeType {
                    name: sym("prim_fold"),
                    expected: acc_ty.to_string(),
                    got: res_ty.to_string(),
                });
            }
            let elem_ty = array_elem_type(&array_ty, "prim_fold")?;
            let step_ty = Type::fun(elem_ty.clone(), acc_ty.clone());
            let mut acc = args[1].clone();
            for value in expect_array(&args[2], "prim_fold")? {
                let step = apply(engine, args[0].clone(), acc, Some(&func_ty), Some(&acc_ty))?;
                acc = apply(engine, step, value, Some(&step_ty), Some(&elem_ty))?;
            }
            Ok(acc)
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let b_tv = engine.types.supply.fresh(Some("b".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let opt_a = option_type(a.clone());
        let scheme = Scheme::new(
            vec![a_tv, b_tv],
            vec![],
            Type::fun(
                Type::fun(b.clone(), Type::fun(a.clone(), b.clone())),
                Type::fun(b.clone(), Type::fun(opt_a.clone(), b.clone())),
            ),
        );
        engine.inject_native_scheme_typed("prim_fold", scheme, 3, |engine, call_type, args| {
            let (arg_tys, res_ty) = split_fun_chain("prim_fold", call_type, 3)?;
            let func_ty = arg_tys[0].clone();
            let acc_ty = arg_tys[1].clone();
            let opt_ty = arg_tys[2].clone();
            if acc_ty != res_ty {
                return Err(EngineError::NativeType {
                    name: sym("prim_fold"),
                    expected: acc_ty.to_string(),
                    got: res_ty.to_string(),
                });
            }
            let elem_ty = option_elem_type(&opt_ty, "prim_fold")?;
            let step_ty = Type::fun(elem_ty.clone(), acc_ty.clone());
            let acc = args[1].clone();
            match option_value(&args[2])? {
                Some(value) => {
                    let step = apply(engine, args[0].clone(), acc, Some(&func_ty), Some(&acc_ty))?;
                    apply(engine, step, value, Some(&step_ty), Some(&elem_ty))
                }
                None => Ok(acc),
            }
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let list_a = list_type(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(
                Type::fun(a.clone(), Type::con("bool", 0)),
                Type::fun(list_a.clone(), list_a),
            ),
        );
        engine.inject_native_scheme_typed("prim_filter", scheme, 2, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain("prim_filter", call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let list_ty = arg_tys[1].clone();
            let elem_ty = list_elem_type(&list_ty, "prim_filter")?;
            let mut out = Vec::new();
            for value in list_to_vec(&args[1], "prim_filter")? {
                let keep = apply(
                    engine,
                    args[0].clone(),
                    value.clone(),
                    Some(&func_ty),
                    Some(&elem_ty),
                )?;
                if expect_bool(&keep, "prim_filter")? {
                    out.push(value);
                }
            }
            Ok(list_from_vec(out))
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let array_a = array_type(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(
                Type::fun(a.clone(), Type::con("bool", 0)),
                Type::fun(array_a.clone(), array_a),
            ),
        );
        engine.inject_native_scheme_typed("prim_filter", scheme, 2, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain("prim_filter", call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let array_ty = arg_tys[1].clone();
            let elem_ty = array_elem_type(&array_ty, "prim_filter")?;
            let mut out = Vec::new();
            for value in expect_array(&args[1], "prim_filter")? {
                let keep = apply(
                    engine,
                    args[0].clone(),
                    value.clone(),
                    Some(&func_ty),
                    Some(&elem_ty),
                )?;
                if expect_bool(&keep, "prim_filter")? {
                    out.push(value);
                }
            }
            Ok(Value::Array(out))
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let opt_a = option_type(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(
                Type::fun(a.clone(), Type::con("bool", 0)),
                Type::fun(opt_a.clone(), opt_a),
            ),
        );
        engine.inject_native_scheme_typed("prim_filter", scheme, 2, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain("prim_filter", call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let opt_ty = arg_tys[1].clone();
            let elem_ty = option_elem_type(&opt_ty, "prim_filter")?;
            match option_value(&args[1])? {
                Some(v) => {
                    let keep = apply(engine, args[0].clone(), v, Some(&func_ty), Some(&elem_ty))?;
                    if expect_bool(&keep, "prim_filter")? {
                        Ok(args[1].clone())
                    } else {
                        Ok(option_from_value(None))
                    }
                }
                None => Ok(option_from_value(None)),
            }
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let b_tv = engine.types.supply.fresh(Some("b".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let list_a = list_type(a.clone());
        let list_b = list_type(b.clone());
        let scheme = Scheme::new(
            vec![a_tv, b_tv],
            vec![],
            Type::fun(
                Type::fun(a.clone(), option_type(b.clone())),
                Type::fun(list_a.clone(), list_b),
            ),
        );
        engine.inject_native_scheme_typed("prim_filter_map", scheme, 2, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain("prim_filter_map", call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let list_ty = arg_tys[1].clone();
            let elem_ty = list_elem_type(&list_ty, "prim_filter_map")?;
            let mut out = Vec::new();
            for value in list_to_vec(&args[1], "prim_filter_map")? {
                let mapped = apply(
                    engine,
                    args[0].clone(),
                    value,
                    Some(&func_ty),
                    Some(&elem_ty),
                )?;
                if let Some(v) = option_value(&mapped)? {
                    out.push(v);
                }
            }
            Ok(list_from_vec(out))
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let b_tv = engine.types.supply.fresh(Some("b".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let array_a = array_type(a.clone());
        let array_b = array_type(b.clone());
        let scheme = Scheme::new(
            vec![a_tv, b_tv],
            vec![],
            Type::fun(
                Type::fun(a.clone(), option_type(b.clone())),
                Type::fun(array_a.clone(), array_b),
            ),
        );
        engine.inject_native_scheme_typed("prim_filter_map", scheme, 2, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain("prim_filter_map", call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let array_ty = arg_tys[1].clone();
            let elem_ty = array_elem_type(&array_ty, "prim_filter_map")?;
            let mut out = Vec::new();
            for value in expect_array(&args[1], "prim_filter_map")? {
                let mapped = apply(
                    engine,
                    args[0].clone(),
                    value,
                    Some(&func_ty),
                    Some(&elem_ty),
                )?;
                if let Some(v) = option_value(&mapped)? {
                    out.push(v);
                }
            }
            Ok(Value::Array(out))
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let b_tv = engine.types.supply.fresh(Some("b".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let opt_a = option_type(a.clone());
        let opt_b = option_type(b.clone());
        let scheme = Scheme::new(
            vec![a_tv, b_tv],
            vec![],
            Type::fun(
                Type::fun(a.clone(), option_type(b.clone())),
                Type::fun(opt_a.clone(), opt_b),
            ),
        );
        engine.inject_native_scheme_typed("prim_filter_map", scheme, 2, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain("prim_filter_map", call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let opt_ty = arg_tys[1].clone();
            let elem_ty = option_elem_type(&opt_ty, "prim_filter_map")?;
            match option_value(&args[1])? {
                Some(v) => {
                    let mapped = apply(engine, args[0].clone(), v, Some(&func_ty), Some(&elem_ty))?;
                    Ok(mapped)
                }
                None => Ok(option_from_value(None)),
            }
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let b_tv = engine.types.supply.fresh(Some("b".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let list_a = list_type(a.clone());
        let list_b = list_type(b.clone());
        let scheme = Scheme::new(
            vec![a_tv, b_tv],
            vec![],
            Type::fun(
                Type::fun(a.clone(), list_b.clone()),
                Type::fun(list_a.clone(), list_b),
            ),
        );
        engine.inject_native_scheme_typed("prim_flat_map", scheme, 2, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain("prim_flat_map", call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let list_ty = arg_tys[1].clone();
            let elem_ty = list_elem_type(&list_ty, "prim_flat_map")?;
            let mut out = Vec::new();
            for value in list_to_vec(&args[1], "prim_flat_map")? {
                let mapped = apply(
                    engine,
                    args[0].clone(),
                    value,
                    Some(&func_ty),
                    Some(&elem_ty),
                )?;
                let mut inner = list_to_vec(&mapped, "prim_flat_map")?;
                out.append(&mut inner);
            }
            Ok(list_from_vec(out))
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let b_tv = engine.types.supply.fresh(Some("b".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let array_a = array_type(a.clone());
        let array_b = array_type(b.clone());
        let scheme = Scheme::new(
            vec![a_tv, b_tv],
            vec![],
            Type::fun(
                Type::fun(a.clone(), array_b.clone()),
                Type::fun(array_a.clone(), array_b),
            ),
        );
        engine.inject_native_scheme_typed("prim_flat_map", scheme, 2, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain("prim_flat_map", call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let array_ty = arg_tys[1].clone();
            let elem_ty = array_elem_type(&array_ty, "prim_flat_map")?;
            let mut out = Vec::new();
            for value in expect_array(&args[1], "prim_flat_map")? {
                let mapped = apply(
                    engine,
                    args[0].clone(),
                    value,
                    Some(&func_ty),
                    Some(&elem_ty),
                )?;
                let mut inner = expect_array(&mapped, "prim_flat_map")?;
                out.append(&mut inner);
            }
            Ok(Value::Array(out))
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let b_tv = engine.types.supply.fresh(Some("b".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let opt_a = option_type(a.clone());
        let opt_b = option_type(b.clone());
        let scheme = Scheme::new(
            vec![a_tv, b_tv],
            vec![],
            Type::fun(
                Type::fun(a.clone(), opt_b.clone()),
                Type::fun(opt_a.clone(), opt_b),
            ),
        );
        engine.inject_native_scheme_typed("prim_flat_map", scheme, 2, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain("prim_flat_map", call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let opt_ty = arg_tys[1].clone();
            let elem_ty = option_elem_type(&opt_ty, "prim_flat_map")?;
            match option_value(&args[1])? {
                Some(v) => apply(engine, args[0].clone(), v, Some(&func_ty), Some(&elem_ty)),
                None => Ok(option_from_value(None)),
            }
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let b_tv = engine.types.supply.fresh(Some("b".into()));
        let e_tv = engine.types.supply.fresh(Some("e".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let e = Type::var(e_tv.clone());
        let result_a = result_type(a.clone(), e.clone());
        let result_b = result_type(b.clone(), e.clone());
        let scheme = Scheme::new(
            vec![a_tv, b_tv, e_tv],
            vec![],
            Type::fun(
                Type::fun(a.clone(), result_b.clone()),
                Type::fun(result_a.clone(), result_b),
            ),
        );
        engine.inject_native_scheme_typed("prim_flat_map", scheme, 2, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain("prim_flat_map", call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let result_ty = arg_tys[1].clone();
            let (ok_ty, _err_ty) = result_types(&result_ty, "prim_flat_map")?;
            match result_value(&args[1])? {
                Ok(v) => {
                    let mapped = apply(engine, args[0].clone(), v, Some(&func_ty), Some(&ok_ty))?;
                    let _ = result_value(&mapped)?;
                    Ok(mapped)
                }
                Err(e) => Ok(result_from_value(Err(e))),
            }
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let list_a = list_type(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(
                Type::fun(list_a.clone(), list_a.clone()),
                Type::fun(list_a.clone(), list_a),
            ),
        );
        engine.inject_native_scheme_typed("prim_or_else", scheme, 2, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain("prim_or_else", call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let list_ty = arg_tys[1].clone();
            if !list_to_vec(&args[1], "prim_or_else")?.is_empty() {
                return Ok(args[1].clone());
            }
            apply(
                engine,
                args[0].clone(),
                args[1].clone(),
                Some(&func_ty),
                Some(&list_ty),
            )
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let array_a = array_type(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(
                Type::fun(array_a.clone(), array_a.clone()),
                Type::fun(array_a.clone(), array_a),
            ),
        );
        engine.inject_native_scheme_typed("prim_or_else", scheme, 2, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain("prim_or_else", call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let array_ty = arg_tys[1].clone();
            if !expect_array(&args[1], "prim_or_else")?.is_empty() {
                return Ok(args[1].clone());
            }
            apply(
                engine,
                args[0].clone(),
                args[1].clone(),
                Some(&func_ty),
                Some(&array_ty),
            )
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let opt_a = option_type(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(
                Type::fun(opt_a.clone(), opt_a.clone()),
                Type::fun(opt_a.clone(), opt_a),
            ),
        );
        engine.inject_native_scheme_typed("prim_or_else", scheme, 2, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain("prim_or_else", call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let opt_ty = arg_tys[1].clone();
            if option_value(&args[1])?.is_some() {
                return Ok(args[1].clone());
            }
            apply(
                engine,
                args[0].clone(),
                args[1].clone(),
                Some(&func_ty),
                Some(&opt_ty),
            )
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let e_tv = engine.types.supply.fresh(Some("e".into()));
        let a = Type::var(a_tv.clone());
        let e = Type::var(e_tv.clone());
        let result_a = result_type(a.clone(), e.clone());
        let scheme = Scheme::new(
            vec![a_tv, e_tv],
            vec![],
            Type::fun(
                Type::fun(result_a.clone(), result_a.clone()),
                Type::fun(result_a.clone(), result_a),
            ),
        );
        engine.inject_native_scheme_typed("prim_or_else", scheme, 2, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain("prim_or_else", call_type, 2)?;
            let func_ty = arg_tys[0].clone();
            let result_ty = arg_tys[1].clone();
            if matches!(result_value(&args[1])?, Err(_)) {
                apply(
                    engine,
                    args[0].clone(),
                    args[1].clone(),
                    Some(&func_ty),
                    Some(&result_ty),
                )
            } else {
                Ok(args[1].clone())
            }
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let list_a = list_type(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(list_a.clone(), a.clone()),
        );
        engine.inject_native_scheme_typed("sum", scheme, 1, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain("sum", call_type, 1)?;
            let list_ty = arg_tys[0].clone();
            let elem_ty = list_elem_type(&list_ty, "sum")?;
            let mut values = list_to_vec(&args[0], "sum")?;
            if values.is_empty() {
                return engine.resolve_global_value(&sym("zero"), &elem_ty);
            }
            let plus = resolve_binary_op(engine, "+", &elem_ty)?;
            let plus_ty = Type::fun(elem_ty.clone(), Type::fun(elem_ty.clone(), elem_ty.clone()));
            let step_ty = Type::fun(elem_ty.clone(), elem_ty.clone());
            let mut acc = values.remove(0);
            for value in values {
                let step = apply(engine, plus.clone(), acc, Some(&plus_ty), Some(&elem_ty))?;
                acc = apply(engine, step, value, Some(&step_ty), Some(&elem_ty))?;
            }
            Ok(acc)
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let array_a = array_type(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(array_a.clone(), a.clone()),
        );
        engine.inject_native_scheme_typed("sum", scheme, 1, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain("sum", call_type, 1)?;
            let array_ty = arg_tys[0].clone();
            let elem_ty = array_elem_type(&array_ty, "sum")?;
            let mut values = expect_array(&args[0], "sum")?;
            if values.is_empty() {
                return engine.resolve_global_value(&sym("zero"), &elem_ty);
            }
            let plus = resolve_binary_op(engine, "+", &elem_ty)?;
            let plus_ty = Type::fun(elem_ty.clone(), Type::fun(elem_ty.clone(), elem_ty.clone()));
            let step_ty = Type::fun(elem_ty.clone(), elem_ty.clone());
            let mut acc = values.remove(0);
            for value in values {
                let step = apply(engine, plus.clone(), acc, Some(&plus_ty), Some(&elem_ty))?;
                acc = apply(engine, step, value, Some(&step_ty), Some(&elem_ty))?;
            }
            Ok(acc)
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let opt_a = option_type(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(opt_a.clone(), a.clone()),
        );
        engine.inject_native_scheme_typed("sum", scheme, 1, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain("sum", call_type, 1)?;
            let opt_ty = arg_tys[0].clone();
            let elem_ty = option_elem_type(&opt_ty, "sum")?;
            match option_value(&args[0])? {
                Some(v) => Ok(v),
                None => engine.resolve_global_value(&sym("zero"), &elem_ty),
            }
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let list_a = list_type(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(list_a.clone(), a.clone()),
        );
        engine.inject_native_scheme_typed("mean", scheme, 1, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain("mean", call_type, 1)?;
            let list_ty = arg_tys[0].clone();
            let elem_ty = list_elem_type(&list_ty, "mean")?;
            let mut values = list_to_vec(&args[0], "mean")?;
            let len = values.len();
            if len == 0 {
                return Err(EngineError::EmptySequence);
            }
            let plus = resolve_binary_op(engine, "+", &elem_ty)?;
            let div = resolve_binary_op(engine, "/", &elem_ty)?;
            let plus_ty = Type::fun(elem_ty.clone(), Type::fun(elem_ty.clone(), elem_ty.clone()));
            let step_ty = Type::fun(elem_ty.clone(), elem_ty.clone());
            let mut acc = values.remove(0);
            for value in values {
                let step = apply(engine, plus.clone(), acc, Some(&plus_ty), Some(&elem_ty))?;
                acc = apply(engine, step, value, Some(&step_ty), Some(&elem_ty))?;
            }
            let len_val = len_value_for_type(&elem_ty, len, "mean")?;
            let div_step = apply(engine, div.clone(), acc, Some(&plus_ty), Some(&elem_ty))?;
            apply(engine, div_step, len_val, Some(&step_ty), Some(&elem_ty))
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let array_a = array_type(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(array_a.clone(), a.clone()),
        );
        engine.inject_native_scheme_typed("mean", scheme, 1, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain("mean", call_type, 1)?;
            let array_ty = arg_tys[0].clone();
            let elem_ty = array_elem_type(&array_ty, "mean")?;
            let mut values = expect_array(&args[0], "mean")?;
            let len = values.len();
            if len == 0 {
                return Err(EngineError::EmptySequence);
            }
            let plus = resolve_binary_op(engine, "+", &elem_ty)?;
            let div = resolve_binary_op(engine, "/", &elem_ty)?;
            let plus_ty = Type::fun(elem_ty.clone(), Type::fun(elem_ty.clone(), elem_ty.clone()));
            let step_ty = Type::fun(elem_ty.clone(), elem_ty.clone());
            let mut acc = values.remove(0);
            for value in values {
                let step = apply(engine, plus.clone(), acc, Some(&plus_ty), Some(&elem_ty))?;
                acc = apply(engine, step, value, Some(&step_ty), Some(&elem_ty))?;
            }
            let len_val = len_value_for_type(&elem_ty, len, "mean")?;
            let div_step = apply(engine, div.clone(), acc, Some(&plus_ty), Some(&elem_ty))?;
            apply(engine, div_step, len_val, Some(&step_ty), Some(&elem_ty))
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let opt_a = option_type(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(opt_a.clone(), a.clone()),
        );
        engine.inject_native_scheme_typed("mean", scheme, 1, |engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain("mean", call_type, 1)?;
            let opt_ty = arg_tys[0].clone();
            let elem_ty = option_elem_type(&opt_ty, "mean")?;
            match option_value(&args[0])? {
                Some(v) => {
                    let len_val = len_value_for_type(&elem_ty, 1, "mean")?;
                    let div = resolve_binary_op(engine, "/", &elem_ty)?;
                    let div_ty =
                        Type::fun(elem_ty.clone(), Type::fun(elem_ty.clone(), elem_ty.clone()));
                    let step_ty = Type::fun(elem_ty.clone(), elem_ty.clone());
                    let div_step = apply(engine, div.clone(), v, Some(&div_ty), Some(&elem_ty))?;
                    apply(engine, div_step, len_val, Some(&step_ty), Some(&elem_ty))
                }
                None => Err(EngineError::EmptySequence),
            }
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let list_a = list_type(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(list_a.clone(), Type::con("i32", 0)),
        );
        engine.inject_native_scheme_typed("count", scheme, 1, |_engine, _call_type, args| {
            Ok(Value::I32(list_to_vec(&args[0], "count")?.len() as i32))
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let array_a = array_type(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(array_a.clone(), Type::con("i32", 0)),
        );
        engine.inject_native_scheme_typed("count", scheme, 1, |_engine, _call_type, args| {
            Ok(Value::I32(expect_array(&args[0], "count")?.len() as i32))
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let opt_a = option_type(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(opt_a.clone(), Type::con("i32", 0)),
        );
        engine.inject_native_scheme_typed("count", scheme, 1, |_engine, _call_type, args| {
            Ok(Value::I32(option_value(&args[0])?.is_some() as i32))
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let list_a = list_type(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(Type::con("i32", 0), Type::fun(list_a.clone(), list_a)),
        );
        engine.inject_native_scheme_typed("prim_take", scheme, 2, |_engine, _call_type, args| {
            let n = i32::from_value(&args[0], "prim_take")?;
            let n = if n < 0 { 0 } else { n as usize };
            let xs = list_to_vec(&args[1], "prim_take")?;
            Ok(list_from_vec(xs.into_iter().take(n).collect()))
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let array_a = array_type(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(Type::con("i32", 0), Type::fun(array_a.clone(), array_a)),
        );
        engine.inject_native_scheme_typed("prim_take", scheme, 2, |_engine, _call_type, args| {
            let n = i32::from_value(&args[0], "prim_take")?;
            let n = if n < 0 { 0 } else { n as usize };
            let xs = expect_array(&args[1], "prim_take")?;
            Ok(Value::Array(xs.into_iter().take(n).collect()))
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let list_a = list_type(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(Type::con("i32", 0), Type::fun(list_a.clone(), list_a)),
        );
        engine.inject_native_scheme_typed("prim_skip", scheme, 2, |_engine, _call_type, args| {
            let n = i32::from_value(&args[0], "prim_skip")?;
            let n = if n < 0 { 0 } else { n as usize };
            let xs = list_to_vec(&args[1], "prim_skip")?;
            Ok(list_from_vec(xs.into_iter().skip(n).collect()))
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let array_a = array_type(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(Type::con("i32", 0), Type::fun(array_a.clone(), array_a)),
        );
        engine.inject_native_scheme_typed("prim_skip", scheme, 2, |_engine, _call_type, args| {
            let n = i32::from_value(&args[0], "prim_skip")?;
            let n = if n < 0 { 0 } else { n as usize };
            let xs = expect_array(&args[1], "prim_skip")?;
            Ok(Value::Array(xs.into_iter().skip(n).collect()))
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let list_a = list_type(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(Type::con("i32", 0), Type::fun(list_a.clone(), a.clone())),
        );
        engine.inject_native_scheme_typed("prim_get", scheme, 2, |_engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain("prim_get", call_type, 2)?;
            let list_ty = arg_tys[1].clone();
            let _elem_ty = list_elem_type(&list_ty, "prim_get")?;
            let idx = i32::from_value(&args[0], "prim_get")?;
            let idx_usize = if idx < 0 {
                return Err(EngineError::IndexOutOfBounds {
                    name: sym("prim_get"),
                    index: idx,
                    len: 0,
                });
            } else {
                idx as usize
            };
            let xs = list_to_vec(&args[1], "prim_get")?;
            if idx_usize >= xs.len() {
                return Err(EngineError::IndexOutOfBounds {
                    name: sym("prim_get"),
                    index: idx,
                    len: xs.len(),
                });
            }
            Ok(xs[idx_usize].clone())
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let array_a = array_type(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(Type::con("i32", 0), Type::fun(array_a.clone(), a.clone())),
        );
        engine.inject_native_scheme_typed("prim_get", scheme, 2, |_engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain("prim_get", call_type, 2)?;
            let array_ty = arg_tys[1].clone();
            let _elem_ty = array_elem_type(&array_ty, "prim_get")?;
            let idx = i32::from_value(&args[0], "prim_get")?;
            let idx_usize = if idx < 0 {
                return Err(EngineError::IndexOutOfBounds {
                    name: sym("prim_get"),
                    index: idx,
                    len: 0,
                });
            } else {
                idx as usize
            };
            let xs = expect_array(&args[1], "prim_get")?;
            if idx_usize >= xs.len() {
                return Err(EngineError::IndexOutOfBounds {
                    name: sym("prim_get"),
                    index: idx,
                    len: xs.len(),
                });
            }
            Ok(xs[idx_usize].clone())
        })?;
    }

    for size in 2..=32 {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let tuple = Type::tuple(vec![a.clone(); size]);
        let scheme = Scheme::new(
            vec![a_tv],
            vec![],
            Type::fun(Type::con("i32", 0), Type::fun(tuple.clone(), a.clone())),
        );
        engine.inject_native_scheme_typed("prim_get", scheme, 2, move |_engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain("prim_get", call_type, 2)?;
            let tuple_ty = arg_tys[1].clone();
            let _elem_ty = tuple_elem_type(&tuple_ty, "prim_get")?;
            let idx = i32::from_value(&args[0], "prim_get")?;
            let idx_usize = if idx < 0 {
                return Err(EngineError::IndexOutOfBounds {
                    name: sym("prim_get"),
                    index: idx,
                    len: 0,
                });
            } else {
                idx as usize
            };
            match &args[1] {
                Value::Tuple(xs) => {
                    if xs.len() != size {
                        return Err(EngineError::NativeType {
                            name: sym("prim_get"),
                            expected: format!("tuple{}", size),
                            got: format!("tuple{}", xs.len()),
                        });
                    }
                    if idx_usize >= xs.len() {
                        return Err(EngineError::IndexOutOfBounds {
                            name: sym("prim_get"),
                            index: idx,
                            len: xs.len(),
                        });
                    }
                    Ok(xs[idx_usize].clone())
                }
                other => Err(EngineError::NativeType {
                    name: sym("prim_get"),
                    expected: format!("tuple{}", size),
                    got: other.type_name().into(),
                }),
            }
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let b_tv = engine.types.supply.fresh(Some("b".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let list_a = list_type(a.clone());
        let list_b = list_type(b.clone());
        let list_pair = list_type(Type::tuple(vec![a.clone(), b.clone()]));
        let scheme = Scheme::new(
            vec![a_tv, b_tv],
            vec![],
            Type::fun(list_a.clone(), Type::fun(list_b.clone(), list_pair)),
        );
        engine.inject_native_scheme_typed("prim_zip", scheme, 2, |_engine, _call_type, args| {
            let xs = list_to_vec(&args[0], "prim_zip")?;
            let ys = list_to_vec(&args[1], "prim_zip")?;
            let mut out = Vec::new();
            for (x, y) in xs.into_iter().zip(ys.into_iter()) {
                out.push(Value::Tuple(vec![x, y]));
            }
            Ok(list_from_vec(out))
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let b_tv = engine.types.supply.fresh(Some("b".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let array_a = array_type(a.clone());
        let array_b = array_type(b.clone());
        let array_pair = array_type(Type::tuple(vec![a.clone(), b.clone()]));
        let scheme = Scheme::new(
            vec![a_tv, b_tv],
            vec![],
            Type::fun(array_a.clone(), Type::fun(array_b.clone(), array_pair)),
        );
        engine.inject_native_scheme_typed("prim_zip", scheme, 2, |_engine, _call_type, args| {
            let xs = expect_array(&args[0], "prim_zip")?;
            let ys = expect_array(&args[1], "prim_zip")?;
            let mut out = Vec::new();
            for (x, y) in xs.into_iter().zip(ys.into_iter()) {
                out.push(Value::Tuple(vec![x, y]));
            }
            Ok(Value::Array(out))
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let b_tv = engine.types.supply.fresh(Some("b".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let list_pair = list_type(Type::tuple(vec![a.clone(), b.clone()]));
        let list_a = list_type(a.clone());
        let list_b = list_type(b.clone());
        let scheme = Scheme::new(
            vec![a_tv, b_tv],
            vec![],
            Type::fun(list_pair.clone(), Type::tuple(vec![list_a, list_b])),
        );
        engine.inject_native_scheme_typed("prim_unzip", scheme, 1, |_engine, _call_type, args| {
            let mut left = Vec::new();
            let mut right = Vec::new();
            for value in list_to_vec(&args[0], "prim_unzip")? {
                match value {
                    Value::Tuple(mut elems) if elems.len() == 2 => {
                        right.push(elems.pop().unwrap());
                        left.push(elems.pop().unwrap());
                    }
                    other => {
                        return Err(EngineError::NativeType {
                            name: sym("prim_unzip"),
                            expected: "tuple2".into(),
                            got: other.type_name().into(),
                        });
                    }
                }
            }
            Ok(Value::Tuple(vec![
                list_from_vec(left),
                list_from_vec(right),
            ]))
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let b_tv = engine.types.supply.fresh(Some("b".into()));
        let a = Type::var(a_tv.clone());
        let b = Type::var(b_tv.clone());
        let array_pair = array_type(Type::tuple(vec![a.clone(), b.clone()]));
        let array_a = array_type(a.clone());
        let array_b = array_type(b.clone());
        let scheme = Scheme::new(
            vec![a_tv, b_tv],
            vec![],
            Type::fun(array_pair.clone(), Type::tuple(vec![array_a, array_b])),
        );
        engine.inject_native_scheme_typed("prim_unzip", scheme, 1, |_engine, _call_type, args| {
            let mut left = Vec::new();
            let mut right = Vec::new();
            for value in expect_array(&args[0], "prim_unzip")? {
                match value {
                    Value::Tuple(mut elems) if elems.len() == 2 => {
                        right.push(elems.pop().unwrap());
                        left.push(elems.pop().unwrap());
                    }
                    other => {
                        return Err(EngineError::NativeType {
                            name: sym("prim_unzip"),
                            expected: "tuple2".into(),
                            got: other.type_name().into(),
                        });
                    }
                }
            }
            Ok(Value::Tuple(vec![Value::Array(left), Value::Array(right)]))
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let list_a = list_type(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(list_a.clone(), a.clone()),
        );
        engine.inject_native_scheme_typed("min", scheme, 1, |_engine, call_type, args| {
            let min_name = sym("min");
            let (arg_tys, _res_ty) = split_fun_chain("min", call_type, 1)?;
            let list_ty = arg_tys[0].clone();
            let elem_ty = list_elem_type(&list_ty, "min")?;
            let mut values = list_to_vec(&args[0], "min")?.into_iter();
            let mut best = values.next().ok_or(EngineError::EmptySequence)?;
            for value in values {
                let ord = cmp_value_by_type(&min_name, &elem_ty, &value, &best)?;
                if ord == std::cmp::Ordering::Less {
                    best = value;
                }
            }
            Ok(best)
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let array_a = array_type(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(array_a.clone(), a.clone()),
        );
        engine.inject_native_scheme_typed("min", scheme, 1, |_engine, call_type, args| {
            let min_name = sym("min");
            let (arg_tys, _res_ty) = split_fun_chain("min", call_type, 1)?;
            let array_ty = arg_tys[0].clone();
            let elem_ty = array_elem_type(&array_ty, "min")?;
            let mut values = expect_array(&args[0], "min")?.into_iter();
            let mut best = values.next().ok_or(EngineError::EmptySequence)?;
            for value in values {
                let ord = cmp_value_by_type(&min_name, &elem_ty, &value, &best)?;
                if ord == std::cmp::Ordering::Less {
                    best = value;
                }
            }
            Ok(best)
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let opt_a = option_type(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(opt_a.clone(), a.clone()),
        );
        engine.inject_native_scheme_typed("min", scheme, 1, |_engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain("min", call_type, 1)?;
            let opt_ty = arg_tys[0].clone();
            let _elem_ty = option_elem_type(&opt_ty, "min")?;
            match option_value(&args[0])? {
                Some(v) => Ok(v),
                None => Err(EngineError::EmptySequence),
            }
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let list_a = list_type(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(list_a.clone(), a.clone()),
        );
        engine.inject_native_scheme_typed("max", scheme, 1, |_engine, call_type, args| {
            let max_name = sym("max");
            let (arg_tys, _res_ty) = split_fun_chain("max", call_type, 1)?;
            let list_ty = arg_tys[0].clone();
            let elem_ty = list_elem_type(&list_ty, "max")?;
            let mut values = list_to_vec(&args[0], "max")?.into_iter();
            let mut best = values.next().ok_or(EngineError::EmptySequence)?;
            for value in values {
                let ord = cmp_value_by_type(&max_name, &elem_ty, &value, &best)?;
                if ord == std::cmp::Ordering::Greater {
                    best = value;
                }
            }
            Ok(best)
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let array_a = array_type(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(array_a.clone(), a.clone()),
        );
        engine.inject_native_scheme_typed("max", scheme, 1, |_engine, call_type, args| {
            let max_name = sym("max");
            let (arg_tys, _res_ty) = split_fun_chain("max", call_type, 1)?;
            let array_ty = arg_tys[0].clone();
            let elem_ty = array_elem_type(&array_ty, "max")?;
            let mut values = expect_array(&args[0], "max")?.into_iter();
            let mut best = values.next().ok_or(EngineError::EmptySequence)?;
            for value in values {
                let ord = cmp_value_by_type(&max_name, &elem_ty, &value, &best)?;
                if ord == std::cmp::Ordering::Greater {
                    best = value;
                }
            }
            Ok(best)
        })?;
    }

    {
        let a_tv = engine.types.supply.fresh(Some("a".into()));
        let a = Type::var(a_tv.clone());
        let opt_a = option_type(a.clone());
        let scheme = Scheme::new(
            vec![a_tv.clone()],
            vec![],
            Type::fun(opt_a.clone(), a.clone()),
        );
        engine.inject_native_scheme_typed("max", scheme, 1, |_engine, call_type, args| {
            let (arg_tys, _res_ty) = split_fun_chain("max", call_type, 1)?;
            let opt_ty = arg_tys[0].clone();
            let _elem_ty = option_elem_type(&opt_ty, "max")?;
            match option_value(&args[0])? {
                Some(v) => Ok(v),
                None => Err(EngineError::EmptySequence),
            }
        })?;
    }

    Ok(())
}

fn inject_option_result_builtins(engine: &mut Engine) -> Result<(), EngineError> {
    let is_some = sym("is_some");
    let is_some_scheme = lookup_scheme(engine, &is_some)?;
    engine.inject_native_scheme_typed(
        "is_some",
        is_some_scheme,
        1,
        |_engine, _call_type, args| Ok(Value::Bool(option_value(&args[0])?.is_some())),
    )?;
    let is_none = sym("is_none");
    let is_none_scheme = lookup_scheme(engine, &is_none)?;
    engine.inject_native_scheme_typed(
        "is_none",
        is_none_scheme,
        1,
        |_engine, _call_type, args| Ok(Value::Bool(option_value(&args[0])?.is_none())),
    )?;

    let is_ok = sym("is_ok");
    let is_ok_scheme = lookup_scheme(engine, &is_ok)?;
    engine.inject_native_scheme_typed("is_ok", is_ok_scheme, 1, |_engine, _call_type, args| {
        Ok(Value::Bool(result_value(&args[0])?.is_ok()))
    })?;
    let is_err = sym("is_err");
    let is_err_scheme = lookup_scheme(engine, &is_err)?;
    engine.inject_native_scheme_typed(
        "is_err",
        is_err_scheme,
        1,
        |_engine, _call_type, args| Ok(Value::Bool(result_value(&args[0])?.is_err())),
    )?;
    Ok(())
}

fn eval_typed_expr(engine: &Engine, env: &Env, expr: &TypedExpr) -> Result<Value, EngineError> {
    check_cancelled(engine)?;
    let mut env = env.clone();
    let mut cur = expr;
    loop {
        check_cancelled(engine)?;
        match &cur.kind {
            TypedExprKind::Let { name, def, body } => {
                let value = eval_typed_expr(engine, &env, def)?;
                env = env.extend(name.clone(), value);
                cur = body;
            }
            _ => break,
        }
    }
    match &cur.kind {
        TypedExprKind::Bool(v) => Ok(Value::Bool(*v)),
        TypedExprKind::Uint(v) => Ok(Value::I32(*v as i32)),
        TypedExprKind::Int(v) => Ok(Value::I32(*v as i32)),
        TypedExprKind::Float(v) => Ok(Value::F32(*v as f32)),
        TypedExprKind::String(v) => Ok(Value::String(v.clone())),
        TypedExprKind::Uuid(v) => Ok(Value::Uuid(*v)),
        TypedExprKind::DateTime(v) => Ok(Value::DateTime(*v)),
        TypedExprKind::Tuple(elems) => {
            let mut values = Vec::with_capacity(elems.len());
            for elem in elems {
                check_cancelled(engine)?;
                values.push(eval_typed_expr(engine, &env, elem)?);
            }
            Ok(Value::Tuple(values))
        }
        TypedExprKind::List(elems) => {
            let mut values = Vec::with_capacity(elems.len());
            for elem in elems {
                check_cancelled(engine)?;
                values.push(eval_typed_expr(engine, &env, elem)?);
            }
            Ok(list_from_vec(values))
        }
        TypedExprKind::Dict(kvs) => {
            let mut out = BTreeMap::new();
            for (k, v) in kvs {
                check_cancelled(engine)?;
                out.insert(k.clone(), eval_typed_expr(engine, &env, v)?);
            }
            Ok(Value::Dict(out))
        }
        TypedExprKind::RecordUpdate { base, updates } => {
            let base_val = eval_typed_expr(engine, &env, base)?;
            let mut update_vals = BTreeMap::new();
            for (k, v) in updates {
                check_cancelled(engine)?;
                update_vals.insert(k.clone(), eval_typed_expr(engine, &env, v)?);
            }

            match base_val {
                Value::Dict(mut map) => {
                    for (k, v) in update_vals {
                        map.insert(k, v);
                    }
                    Ok(Value::Dict(map))
                }
                Value::Adt(tag, args) if args.len() == 1 => match &args[0] {
                    Value::Dict(map) => {
                        let mut out = map.clone();
                        for (k, v) in update_vals {
                            out.insert(k, v);
                        }
                        Ok(Value::Adt(tag, vec![Value::Dict(out)]))
                    }
                    _ => Err(EngineError::UnsupportedExpr),
                },
                _ => Err(EngineError::UnsupportedExpr),
            }
        }
        TypedExprKind::Var { name, .. } => {
            if let Some(value) = env.get(name) {
                match value {
                    Value::Native(native) if native.arity == 0 && native.applied.is_empty() => {
                        native.call_zero(engine)
                    }
                    _ => Ok(value),
                }
            } else if engine.types.class_methods.contains_key(name) {
                engine.resolve_class_method_value(name, &cur.typ)
            } else {
                let value = engine.resolve_native_value(name.as_ref(), &cur.typ)?;
                match &value {
                    Value::Native(native) if native.arity == 0 && native.applied.is_empty() => {
                        native.call_zero(engine)
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

            let mut func = eval_typed_expr(engine, &env, head)?;
            for (func_type, arg_expr) in spine {
                check_cancelled(engine)?;
                let arg = eval_typed_expr(engine, &env, arg_expr)?;
                func = apply(engine, func, arg, Some(&func_type), Some(&arg_expr.typ))?;
            }
            Ok(func)
        }
        TypedExprKind::Project { expr, field } => {
            let value = eval_typed_expr(engine, &env, expr)?;
            match value {
                Value::Adt(_, args) if args.len() == 1 => match &args[0] {
                    Value::Dict(map) => {
                        map.get(field)
                            .cloned()
                            .ok_or_else(|| EngineError::UnknownField {
                                field: field.clone(),
                                value: "record".into(),
                            })
                    }
                    other => Err(EngineError::UnknownField {
                        field: field.clone(),
                        value: other.type_name().into(),
                    }),
                },
                other => Err(EngineError::UnknownField {
                    field: field.clone(),
                    value: other.type_name().into(),
                }),
            }
        }
        TypedExprKind::Lam { param, body } => Ok(Value::Closure {
            env: env.clone(),
            param: param.clone(),
            param_ty: split_fun(&expr.typ)
                .map(|(arg, _)| arg)
                .ok_or_else(|| EngineError::NotCallable(expr.typ.to_string()))?,
            typ: expr.typ.clone(),
            body: Arc::new(body.as_ref().clone()),
        }),
        TypedExprKind::Ite {
            cond,
            then_expr,
            else_expr,
        } => {
            let value = eval_typed_expr(engine, &env, cond)?;
            match value {
                Value::Bool(true) => eval_typed_expr(engine, &env, then_expr),
                Value::Bool(false) => eval_typed_expr(engine, &env, else_expr),
                other => Err(EngineError::ExpectedBool(other.type_name().into())),
            }
        }
        TypedExprKind::Match { scrutinee, arms } => {
            let value = eval_typed_expr(engine, &env, scrutinee)?;
            for (pat, expr) in arms {
                check_cancelled(engine)?;
                if let Some(bindings) = match_pattern(pat, &value) {
                    let env = env.extend_many(bindings);
                    return eval_typed_expr(engine, &env, expr);
                }
            }
            Err(EngineError::MatchFailure)
        }
        TypedExprKind::Let { .. } => unreachable!("let chain handled in eval_typed_expr loop"),
    }
}

fn apply(
    engine: &Engine,
    func: Value,
    arg: Value,
    func_type: Option<&Type>,
    arg_type: Option<&Type>,
) -> Result<Value, EngineError> {
    match func {
        Value::Closure {
            env,
            param,
            param_ty,
            typ,
            body,
        } => {
            let mut subst = Subst::new_sync();
            if let Some(expected) = func_type {
                let s_fun = unify(&typ, expected).map_err(|_| EngineError::NativeType {
                    name: param.clone(),
                    expected: typ.to_string(),
                    got: expected.to_string(),
                })?;
                subst = compose_subst(s_fun, subst);
            }
            let actual_ty = resolve_arg_type(arg_type, &arg)?;
            let param_ty = param_ty.apply(&subst);
            let s_arg = unify(&param_ty, &actual_ty).map_err(|_| EngineError::NativeType {
                name: param.clone(),
                expected: param_ty.to_string(),
                got: actual_ty.to_string(),
            })?;
            subst = compose_subst(s_arg, subst);
            let env = env.extend(param, arg);
            let body = body.apply(&subst);
            eval_typed_expr(engine, &env, &body)
        }
        Value::Native(native) => native.apply(engine, arg, arg_type),
        Value::Overloaded(over) => over.apply(engine, arg, arg_type),
        other => Err(EngineError::NotCallable(other.type_name().into())),
    }
}

impl NativeFn {
    fn call_zero_with_gas(&self, engine: &Engine, gas: &mut GasMeter) -> Result<Value, EngineError> {
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
        arg: Value,
        arg_type: Option<&Type>,
        gas: &mut GasMeter,
    ) -> Result<Value, EngineError> {
        if self.arity == 0 {
            return Err(EngineError::NativeArity {
                name: self.name,
                expected: 0,
                got: 1,
            });
        }
        let (arg_ty, rest_ty) =
            split_fun(&self.typ).ok_or_else(|| EngineError::NotCallable(self.typ.to_string()))?;
        let actual_ty = resolve_arg_type(arg_type, &arg)?;
        let subst = unify(&arg_ty, &actual_ty).map_err(|_| EngineError::NativeType {
            name: self.name.clone(),
            expected: arg_ty.to_string(),
            got: actual_ty.to_string(),
        })?;
        self.typ = rest_ty.apply(&subst);
        self.applied.push(arg);
        self.applied_types.push(actual_ty);
        if is_function_type(&self.typ) {
            return Ok(Value::Native(self));
        }

        let mut full_ty = self.typ.clone();
        for arg_ty in self.applied_types.iter().rev() {
            full_ty = Type::fun(arg_ty.clone(), full_ty);
        }

        let amount = gas
            .costs
            .native_call_base
            .saturating_add(self.gas_cost)
            .saturating_add(gas.costs.native_call_per_arg.saturating_mul(self.applied.len() as u64));
        gas.charge(amount)?;
        self.func.call_sync(engine, &full_ty, &self.applied)
    }

    async fn call_zero_async_with_gas(
        &self,
        engine: &Engine,
        gas: &mut GasMeter,
    ) -> Result<Value, EngineError> {
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
        arg: Value,
        arg_type: Option<&Type>,
        gas: &mut GasMeter,
    ) -> Result<Value, EngineError> {
        if self.arity == 0 {
            return Err(EngineError::NativeArity {
                name: self.name,
                expected: 0,
                got: 1,
            });
        }
        let (arg_ty, rest_ty) =
            split_fun(&self.typ).ok_or_else(|| EngineError::NotCallable(self.typ.to_string()))?;
        let actual_ty = resolve_arg_type(arg_type, &arg)?;
        let subst = unify(&arg_ty, &actual_ty).map_err(|_| EngineError::NativeType {
            name: self.name.clone(),
            expected: arg_ty.to_string(),
            got: actual_ty.to_string(),
        })?;
        self.typ = rest_ty.apply(&subst);
        self.applied.push(arg);
        self.applied_types.push(actual_ty);
        if is_function_type(&self.typ) {
            return Ok(Value::Native(self));
        }

        let mut full_ty = self.typ.clone();
        for arg_ty in self.applied_types.iter().rev() {
            full_ty = Type::fun(arg_ty.clone(), full_ty);
        }

        let amount = gas
            .costs
            .native_call_base
            .saturating_add(self.gas_cost)
            .saturating_add(gas.costs.native_call_per_arg.saturating_mul(self.applied.len() as u64));
        gas.charge(amount)?;
        self.func
            .call_async(engine.clone(), full_ty, self.applied)
            .await
    }
}

fn eval_typed_expr_with_gas(
    engine: &Engine,
    env: &Env,
    expr: &TypedExpr,
    gas: &mut GasMeter,
) -> Result<Value, EngineError> {
    check_cancelled(engine)?;
    let mut env = env.clone();
    let mut cur = expr;
    loop {
        check_cancelled(engine)?;
        match &cur.kind {
            TypedExprKind::Let { name, def, body } => {
                gas.charge(gas.costs.eval_node)?;
                let value = eval_typed_expr_with_gas(engine, &env, def, gas)?;
                env = env.extend(name.clone(), value);
                cur = body;
            }
            _ => break,
        }
    }

    gas.charge(gas.costs.eval_node)?;
    match &cur.kind {
        TypedExprKind::Bool(v) => Ok(Value::Bool(*v)),
        TypedExprKind::Uint(v) => Ok(Value::I32(*v as i32)),
        TypedExprKind::Int(v) => Ok(Value::I32(*v as i32)),
        TypedExprKind::Float(v) => Ok(Value::F32(*v as f32)),
        TypedExprKind::String(v) => Ok(Value::String(v.clone())),
        TypedExprKind::Uuid(v) => Ok(Value::Uuid(*v)),
        TypedExprKind::DateTime(v) => Ok(Value::DateTime(*v)),
        TypedExprKind::Tuple(elems) => {
            let mut values = Vec::with_capacity(elems.len());
            for elem in elems {
                check_cancelled(engine)?;
                values.push(eval_typed_expr_with_gas(engine, &env, elem, gas)?);
            }
            Ok(Value::Tuple(values))
        }
        TypedExprKind::List(elems) => {
            let mut values = Vec::with_capacity(elems.len());
            for elem in elems {
                check_cancelled(engine)?;
                values.push(eval_typed_expr_with_gas(engine, &env, elem, gas)?);
            }
            Ok(list_from_vec(values))
        }
        TypedExprKind::Dict(kvs) => {
            let mut out = BTreeMap::new();
            for (k, v) in kvs {
                check_cancelled(engine)?;
                out.insert(k.clone(), eval_typed_expr_with_gas(engine, &env, v, gas)?);
            }
            Ok(Value::Dict(out))
        }
        TypedExprKind::RecordUpdate { base, updates } => {
            let base_val = eval_typed_expr_with_gas(engine, &env, base, gas)?;
            let mut update_vals = BTreeMap::new();
            for (k, v) in updates {
                check_cancelled(engine)?;
                update_vals.insert(k.clone(), eval_typed_expr_with_gas(engine, &env, v, gas)?);
            }

            match base_val {
                Value::Dict(mut map) => {
                    for (k, v) in update_vals {
                        gas.charge(gas.costs.eval_record_update_field)?;
                        map.insert(k, v);
                    }
                    Ok(Value::Dict(map))
                }
                Value::Adt(tag, args) if args.len() == 1 => match &args[0] {
                    Value::Dict(map) => {
                        let mut out = map.clone();
                        for (k, v) in update_vals {
                            gas.charge(gas.costs.eval_record_update_field)?;
                            out.insert(k, v);
                        }
                        Ok(Value::Adt(tag, vec![Value::Dict(out)]))
                    }
                    _ => Err(EngineError::UnsupportedExpr),
                },
                _ => Err(EngineError::UnsupportedExpr),
            }
        }
        TypedExprKind::Var { name, .. } => {
            if let Some(value) = env.get(name) {
                match value {
                    Value::Native(native) if native.arity == 0 && native.applied.is_empty() => {
                        native.call_zero_with_gas(engine, gas)
                    }
                    _ => Ok(value),
                }
            } else if engine.types.class_methods.contains_key(name) {
                engine.resolve_class_method_value_with_gas(name, &cur.typ, gas)
            } else {
                let value = engine.resolve_native_value_with_gas(name.as_ref(), &cur.typ, gas)?;
                match &value {
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
                func = apply_with_gas(engine, func, arg, Some(&func_type), Some(&arg_expr.typ), gas)?;
            }
            Ok(func)
        }
        TypedExprKind::Project { expr, field } => {
            let value = eval_typed_expr_with_gas(engine, &env, expr, gas)?;
            match value {
                Value::Adt(_, args) if args.len() == 1 => match &args[0] {
                    Value::Dict(map) => map.get(field).cloned().ok_or_else(|| EngineError::UnknownField {
                        field: field.clone(),
                        value: "record".into(),
                    }),
                    other => Err(EngineError::UnknownField {
                        field: field.clone(),
                        value: other.type_name().into(),
                    }),
                },
                other => Err(EngineError::UnknownField {
                    field: field.clone(),
                    value: other.type_name().into(),
                }),
            }
        }
        TypedExprKind::Lam { param, body } => Ok(Value::Closure {
            env: env.clone(),
            param: param.clone(),
            param_ty: split_fun(&expr.typ)
                .map(|(arg, _)| arg)
                .ok_or_else(|| EngineError::NotCallable(expr.typ.to_string()))?,
            typ: expr.typ.clone(),
            body: Arc::new(body.as_ref().clone()),
        }),
        TypedExprKind::Ite {
            cond,
            then_expr,
            else_expr,
        } => {
            let value = eval_typed_expr_with_gas(engine, &env, cond, gas)?;
            match value {
                Value::Bool(true) => eval_typed_expr_with_gas(engine, &env, then_expr, gas),
                Value::Bool(false) => eval_typed_expr_with_gas(engine, &env, else_expr, gas),
                other => Err(EngineError::ExpectedBool(other.type_name().into())),
            }
        }
        TypedExprKind::Match { scrutinee, arms } => {
            let value = eval_typed_expr_with_gas(engine, &env, scrutinee, gas)?;
            for (pat, expr) in arms {
                check_cancelled(engine)?;
                gas.charge(gas.costs.eval_match_arm)?;
                if let Some(bindings) = match_pattern(pat, &value) {
                    let env = env.extend_many(bindings);
                    return eval_typed_expr_with_gas(engine, &env, expr, gas);
                }
            }
            Err(EngineError::MatchFailure)
        }
        TypedExprKind::Let { .. } => unreachable!("let chain handled in eval_typed_expr_with_gas loop"),
    }
}

fn apply_with_gas(
    engine: &Engine,
    func: Value,
    arg: Value,
    func_type: Option<&Type>,
    arg_type: Option<&Type>,
    gas: &mut GasMeter,
) -> Result<Value, EngineError> {
    match func {
        Value::Closure {
            env,
            param,
            param_ty,
            typ,
            body,
        } => {
            let mut subst = Subst::new_sync();
            if let Some(expected) = func_type {
                let s_fun = unify(&typ, expected).map_err(|_| EngineError::NativeType {
                    name: param.clone(),
                    expected: typ.to_string(),
                    got: expected.to_string(),
                })?;
                subst = compose_subst(s_fun, subst);
            }
            let actual_ty = resolve_arg_type(arg_type, &arg)?;
            let param_ty = param_ty.apply(&subst);
            let s_arg = unify(&param_ty, &actual_ty).map_err(|_| EngineError::NativeType {
                name: param.clone(),
                expected: param_ty.to_string(),
                got: actual_ty.to_string(),
            })?;
            subst = compose_subst(s_arg, subst);
            let env = env.extend(param, arg);
            let body = body.apply(&subst);
            eval_typed_expr_with_gas(engine, &env, &body, gas)
        }
        Value::Native(native) => native.apply_with_gas(engine, arg, arg_type, gas),
        Value::Overloaded(over) => over.apply_with_gas(engine, arg, arg_type, gas),
        other => Err(EngineError::NotCallable(other.type_name().into())),
    }
}

#[async_recursion]
async fn eval_typed_expr_async(engine: &Engine, env: &Env, expr: &TypedExpr) -> Result<Value, EngineError> {
    check_cancelled(engine)?;
    let mut env = env.clone();
    let mut cur = expr;
    loop {
        check_cancelled(engine)?;
        match &cur.kind {
            TypedExprKind::Let { name, def, body } => {
                let value = eval_typed_expr_async(engine, &env, def).await?;
                env = env.extend(name.clone(), value);
                cur = body;
            }
            _ => break,
        }
    }

    match &cur.kind {
        TypedExprKind::Bool(v) => Ok(Value::Bool(*v)),
        TypedExprKind::Uint(v) => Ok(Value::I32(*v as i32)),
        TypedExprKind::Int(v) => Ok(Value::I32(*v as i32)),
        TypedExprKind::Float(v) => Ok(Value::F32(*v as f32)),
        TypedExprKind::String(v) => Ok(Value::String(v.clone())),
        TypedExprKind::Uuid(v) => Ok(Value::Uuid(*v)),
        TypedExprKind::DateTime(v) => Ok(Value::DateTime(*v)),
        TypedExprKind::Tuple(elems) => {
            let mut values = Vec::with_capacity(elems.len());
            for elem in elems {
                check_cancelled(engine)?;
                values.push(eval_typed_expr_async(engine, &env, elem).await?);
            }
            Ok(Value::Tuple(values))
        }
        TypedExprKind::List(elems) => {
            let mut values = Vec::with_capacity(elems.len());
            for elem in elems {
                check_cancelled(engine)?;
                values.push(eval_typed_expr_async(engine, &env, elem).await?);
            }
            Ok(list_from_vec(values))
        }
        TypedExprKind::Dict(kvs) => {
            let mut out = BTreeMap::new();
            for (k, v) in kvs {
                check_cancelled(engine)?;
                out.insert(k.clone(), eval_typed_expr_async(engine, &env, v).await?);
            }
            Ok(Value::Dict(out))
        }
        TypedExprKind::RecordUpdate { base, updates } => {
            let base_val = eval_typed_expr_async(engine, &env, base).await?;
            let mut update_vals = BTreeMap::new();
            for (k, v) in updates {
                check_cancelled(engine)?;
                update_vals.insert(k.clone(), eval_typed_expr_async(engine, &env, v).await?);
            }

            match base_val {
                Value::Dict(mut map) => {
                    for (k, v) in update_vals {
                        map.insert(k, v);
                    }
                    Ok(Value::Dict(map))
                }
                Value::Adt(tag, args) if args.len() == 1 => match &args[0] {
                    Value::Dict(map) => {
                        let mut out = map.clone();
                        for (k, v) in update_vals {
                            out.insert(k, v);
                        }
                        Ok(Value::Adt(tag, vec![Value::Dict(out)]))
                    }
                    _ => Err(EngineError::UnsupportedExpr),
                },
                _ => Err(EngineError::UnsupportedExpr),
            }
        }
        TypedExprKind::Var { name, .. } => {
            if let Some(value) = env.get(name) {
                match value {
                    Value::Native(native) if native.arity == 0 && native.applied.is_empty() => {
                        native.call_zero_async(engine).await
                    }
                    _ => Ok(value),
                }
            } else if engine.types.class_methods.contains_key(name) {
                engine.resolve_class_method_value(name, &cur.typ)
            } else {
                let value = engine.resolve_native_value(name.as_ref(), &cur.typ)?;
                match &value {
                    Value::Native(native) if native.arity == 0 && native.applied.is_empty() => {
                        native.call_zero_async(engine).await
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

            let mut func = eval_typed_expr_async(engine, &env, head).await?;
            for (func_type, arg_expr) in spine {
                check_cancelled(engine)?;
                let arg = eval_typed_expr_async(engine, &env, arg_expr).await?;
                func = apply_async(engine, func, arg, Some(&func_type), Some(&arg_expr.typ)).await?;
            }
            Ok(func)
        }
        TypedExprKind::Project { expr, field } => {
            let value = eval_typed_expr_async(engine, &env, expr).await?;
            match value {
                Value::Adt(_, args) if args.len() == 1 => match &args[0] {
                    Value::Dict(map) => map.get(field).cloned().ok_or_else(|| EngineError::UnknownField {
                        field: field.clone(),
                        value: "record".into(),
                    }),
                    other => Err(EngineError::UnknownField {
                        field: field.clone(),
                        value: other.type_name().into(),
                    }),
                },
                other => Err(EngineError::UnknownField {
                    field: field.clone(),
                    value: other.type_name().into(),
                }),
            }
        }
        TypedExprKind::Lam { param, body } => Ok(Value::Closure {
            env: env.clone(),
            param: param.clone(),
            param_ty: split_fun(&expr.typ)
                .map(|(arg, _)| arg)
                .ok_or_else(|| EngineError::NotCallable(expr.typ.to_string()))?,
            typ: expr.typ.clone(),
            body: Arc::new(body.as_ref().clone()),
        }),
        TypedExprKind::Ite {
            cond,
            then_expr,
            else_expr,
        } => {
            let value = eval_typed_expr_async(engine, &env, cond).await?;
            match value {
                Value::Bool(true) => eval_typed_expr_async(engine, &env, then_expr).await,
                Value::Bool(false) => eval_typed_expr_async(engine, &env, else_expr).await,
                other => Err(EngineError::ExpectedBool(other.type_name().into())),
            }
        }
        TypedExprKind::Match { scrutinee, arms } => {
            let value = eval_typed_expr_async(engine, &env, scrutinee).await?;
            for (pat, expr) in arms {
                check_cancelled(engine)?;
                if let Some(bindings) = match_pattern(pat, &value) {
                    let env = env.extend_many(bindings);
                    return eval_typed_expr_async(engine, &env, expr).await;
                }
            }
            Err(EngineError::MatchFailure)
        }
        TypedExprKind::Let { .. } => unreachable!("let chain handled in eval_typed_expr_async loop"),
    }
}

#[async_recursion]
async fn eval_typed_expr_async_with_gas(
    engine: &Engine,
    env: &Env,
    expr: &TypedExpr,
    gas: &mut GasMeter,
) -> Result<Value, EngineError> {
    check_cancelled(engine)?;
    let mut env = env.clone();
    let mut cur = expr;
    loop {
        check_cancelled(engine)?;
        match &cur.kind {
            TypedExprKind::Let { name, def, body } => {
                gas.charge(gas.costs.eval_node)?;
                let value = eval_typed_expr_async_with_gas(engine, &env, def, gas).await?;
                env = env.extend(name.clone(), value);
                cur = body;
            }
            _ => break,
        }
    }

    gas.charge(gas.costs.eval_node)?;
    match &cur.kind {
        TypedExprKind::Bool(v) => Ok(Value::Bool(*v)),
        TypedExprKind::Uint(v) => Ok(Value::I32(*v as i32)),
        TypedExprKind::Int(v) => Ok(Value::I32(*v as i32)),
        TypedExprKind::Float(v) => Ok(Value::F32(*v as f32)),
        TypedExprKind::String(v) => Ok(Value::String(v.clone())),
        TypedExprKind::Uuid(v) => Ok(Value::Uuid(*v)),
        TypedExprKind::DateTime(v) => Ok(Value::DateTime(*v)),
        TypedExprKind::Tuple(elems) => {
            let mut values = Vec::with_capacity(elems.len());
            for elem in elems {
                check_cancelled(engine)?;
                values.push(eval_typed_expr_async_with_gas(engine, &env, elem, gas).await?);
            }
            Ok(Value::Tuple(values))
        }
        TypedExprKind::List(elems) => {
            let mut values = Vec::with_capacity(elems.len());
            for elem in elems {
                check_cancelled(engine)?;
                values.push(eval_typed_expr_async_with_gas(engine, &env, elem, gas).await?);
            }
            Ok(list_from_vec(values))
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
            Ok(Value::Dict(out))
        }
        TypedExprKind::RecordUpdate { base, updates } => {
            let base_val = eval_typed_expr_async_with_gas(engine, &env, base, gas).await?;
            let mut update_vals = BTreeMap::new();
            for (k, v) in updates {
                check_cancelled(engine)?;
                update_vals.insert(
                    k.clone(),
                    eval_typed_expr_async_with_gas(engine, &env, v, gas).await?,
                );
            }

            match base_val {
                Value::Dict(mut map) => {
                    for (k, v) in update_vals {
                        gas.charge(gas.costs.eval_record_update_field)?;
                        map.insert(k, v);
                    }
                    Ok(Value::Dict(map))
                }
                Value::Adt(tag, args) if args.len() == 1 => match &args[0] {
                    Value::Dict(map) => {
                        let mut out = map.clone();
                        for (k, v) in update_vals {
                            gas.charge(gas.costs.eval_record_update_field)?;
                            out.insert(k, v);
                        }
                        Ok(Value::Adt(tag, vec![Value::Dict(out)]))
                    }
                    _ => Err(EngineError::UnsupportedExpr),
                },
                _ => Err(EngineError::UnsupportedExpr),
            }
        }
        TypedExprKind::Var { name, .. } => {
            if let Some(value) = env.get(name) {
                match value {
                    Value::Native(native) if native.arity == 0 && native.applied.is_empty() => {
                        native.call_zero_async_with_gas(engine, gas).await
                    }
                    _ => Ok(value),
                }
            } else if engine.types.class_methods.contains_key(name) {
                engine.resolve_class_method_value_with_gas(name, &cur.typ, gas)
            } else {
                let value = engine.resolve_native_value_with_gas(name.as_ref(), &cur.typ, gas)?;
                match &value {
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
                func = apply_async_with_gas(engine, func, arg, Some(&func_type), Some(&arg_expr.typ), gas).await?;
            }
            Ok(func)
        }
        TypedExprKind::Project { expr, field } => {
            let value = eval_typed_expr_async_with_gas(engine, &env, expr, gas).await?;
            match value {
                Value::Adt(_, args) if args.len() == 1 => match &args[0] {
                    Value::Dict(map) => map.get(field).cloned().ok_or_else(|| EngineError::UnknownField {
                        field: field.clone(),
                        value: "record".into(),
                    }),
                    other => Err(EngineError::UnknownField {
                        field: field.clone(),
                        value: other.type_name().into(),
                    }),
                },
                other => Err(EngineError::UnknownField {
                    field: field.clone(),
                    value: other.type_name().into(),
                }),
            }
        }
        TypedExprKind::Lam { param, body } => Ok(Value::Closure {
            env: env.clone(),
            param: param.clone(),
            param_ty: split_fun(&expr.typ)
                .map(|(arg, _)| arg)
                .ok_or_else(|| EngineError::NotCallable(expr.typ.to_string()))?,
            typ: expr.typ.clone(),
            body: Arc::new(body.as_ref().clone()),
        }),
        TypedExprKind::Ite {
            cond,
            then_expr,
            else_expr,
        } => {
            let value = eval_typed_expr_async_with_gas(engine, &env, cond, gas).await?;
            match value {
                Value::Bool(true) => eval_typed_expr_async_with_gas(engine, &env, then_expr, gas).await,
                Value::Bool(false) => eval_typed_expr_async_with_gas(engine, &env, else_expr, gas).await,
                other => Err(EngineError::ExpectedBool(other.type_name().into())),
            }
        }
        TypedExprKind::Match { scrutinee, arms } => {
            let value = eval_typed_expr_async_with_gas(engine, &env, scrutinee, gas).await?;
            for (pat, expr) in arms {
                check_cancelled(engine)?;
                gas.charge(gas.costs.eval_match_arm)?;
                if let Some(bindings) = match_pattern(pat, &value) {
                    let env = env.extend_many(bindings);
                    return eval_typed_expr_async_with_gas(engine, &env, expr, gas).await;
                }
            }
            Err(EngineError::MatchFailure)
        }
        TypedExprKind::Let { .. } => unreachable!("let chain handled in eval_typed_expr_async_with_gas loop"),
    }
}

#[async_recursion]
async fn apply_async(
    engine: &Engine,
    func: Value,
    arg: Value,
    func_type: Option<&Type>,
    arg_type: Option<&Type>,
) -> Result<Value, EngineError> {
    match func {
        Value::Closure {
            env,
            param,
            param_ty,
            typ,
            body,
        } => {
            let mut subst = Subst::new_sync();
            if let Some(expected) = func_type {
                let s_fun = unify(&typ, expected).map_err(|_| EngineError::NativeType {
                    name: param.clone(),
                    expected: typ.to_string(),
                    got: expected.to_string(),
                })?;
                subst = compose_subst(s_fun, subst);
            }
            let actual_ty = resolve_arg_type(arg_type, &arg)?;
            let param_ty = param_ty.apply(&subst);
            let s_arg = unify(&param_ty, &actual_ty).map_err(|_| EngineError::NativeType {
                name: param.clone(),
                expected: param_ty.to_string(),
                got: actual_ty.to_string(),
            })?;
            subst = compose_subst(s_arg, subst);
            let env = env.extend(param, arg);
            let body = body.apply(&subst);
            eval_typed_expr_async(engine, &env, &body).await
        }
        Value::Native(native) => native.apply_async(engine, arg, arg_type).await,
        Value::Overloaded(over) => over.apply_async(engine, arg, arg_type).await,
        other => Err(EngineError::NotCallable(other.type_name().into())),
    }
}

#[async_recursion]
async fn apply_async_with_gas(
    engine: &Engine,
    func: Value,
    arg: Value,
    func_type: Option<&Type>,
    arg_type: Option<&Type>,
    gas: &mut GasMeter,
) -> Result<Value, EngineError> {
    match func {
        Value::Closure {
            env,
            param,
            param_ty,
            typ,
            body,
        } => {
            let mut subst = Subst::new_sync();
            if let Some(expected) = func_type {
                let s_fun = unify(&typ, expected).map_err(|_| EngineError::NativeType {
                    name: param.clone(),
                    expected: typ.to_string(),
                    got: expected.to_string(),
                })?;
                subst = compose_subst(s_fun, subst);
            }
            let actual_ty = resolve_arg_type(arg_type, &arg)?;
            let param_ty = param_ty.apply(&subst);
            let s_arg = unify(&param_ty, &actual_ty).map_err(|_| EngineError::NativeType {
                name: param.clone(),
                expected: param_ty.to_string(),
                got: actual_ty.to_string(),
            })?;
            subst = compose_subst(s_arg, subst);
            let env = env.extend(param, arg);
            let body = body.apply(&subst);
            eval_typed_expr_async_with_gas(engine, &env, &body, gas).await
        }
        Value::Native(native) => native.apply_async_with_gas(engine, arg, arg_type, gas).await,
        Value::Overloaded(over) => over.apply_async_with_gas(engine, arg, arg_type, gas).await,
        other => Err(EngineError::NotCallable(other.type_name().into())),
    }
}

fn match_pattern(pat: &Pattern, value: &Value) -> Option<HashMap<Symbol, Value>> {
    match pat {
        Pattern::Wildcard(..) => Some(HashMap::new()),
        Pattern::Var(var) => {
            let mut bindings = HashMap::new();
            bindings.insert(var.name.clone(), value.clone());
            Some(bindings)
        }
        Pattern::Named(_, name, ps) => match value {
            Value::Adt(vname, args) if vname == name && args.len() == ps.len() => {
                match_patterns(ps, args)
            }
            _ => None,
        },
        Pattern::Tuple(_, ps) => match value {
            Value::Tuple(xs) if xs.len() == ps.len() => match_patterns(ps, xs),
            _ => None,
        },
        Pattern::List(_, ps) => {
            let values = list_to_vec(value, "pattern").ok()?;
            if values.len() == ps.len() {
                match_patterns(ps, &values)
            } else {
                None
            }
        }
        Pattern::Cons(_, head, tail) => match value {
            Value::Adt(tag, args) if sym_eq(tag, "Cons") && args.len() == 2 => {
                let mut left = match_pattern(head, &args[0])?;
                let right = match_pattern(tail, &args[1])?;
                left.extend(right);
                Some(left)
            }
            _ => None,
        },
        Pattern::Dict(_, fields) => match value {
            Value::Dict(map) => {
                let mut bindings = HashMap::new();
                for (key, pat) in fields {
                    let v = map.get(key)?;
                    let sub = match_pattern(pat, v)?;
                    bindings.extend(sub);
                }
                Some(bindings)
            }
            _ => None,
        },
    }
}

fn match_patterns(patterns: &[Pattern], values: &[Value]) -> Option<HashMap<Symbol, Value>> {
    let mut bindings = HashMap::new();
    for (p, v) in patterns.iter().zip(values.iter()) {
        let sub = match_pattern(p, v)?;
        bindings.extend(sub);
    }
    Some(bindings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;
    use futures::future;
    use rex_gas::{GasCosts, GasMeter};

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
        Engine::with_prelude()
    }

    fn list_values(value: &Value) -> Vec<Value> {
        list_to_vec(value, "test").unwrap()
    }

    #[test]
    fn eval_let_lambda() {
        let expr = parse(
            r#"
            let
                id = \x -> x
            in
                id (id 1, id 2)
            "#,
        );
        let mut engine = Engine::new();
        let value = engine.eval(expr.as_ref()).unwrap();
        match value {
            Value::Tuple(xs) => {
                assert_eq!(xs.len(), 2);
                assert!(matches!(xs[0], Value::I32(1)));
                assert!(matches!(xs[1], Value::I32(2)));
            }
            _ => panic!("expected tuple"),
        }
    }

    #[test]
    fn eval_async_native_injection() {
        let expr = parse("inc 1");
        let mut engine = Engine::with_prelude();
        engine
            .inject_async_fn1("inc", |x: i32| async move { x + 1 })
            .unwrap();

        let v_async = block_on(engine.eval_async(expr.as_ref())).unwrap();
        assert!(matches!(v_async, Value::I32(2)));

        let v_sync = engine.eval(expr.as_ref()).unwrap();
        assert!(matches!(v_sync, Value::I32(2)));
    }

    #[test]
    fn eval_async_can_be_cancelled() {
        let expr = parse("stall");
        let mut engine = Engine::with_prelude();

        let (started_tx, started_rx) = std::sync::mpsc::channel::<()>();
        engine
            .inject_async_fn0_cancellable("stall", move |token: crate::CancellationToken| {
                let started_tx = started_tx.clone();
                async move {
                    let _ = started_tx.send(());
                    future::pending::<()>().await;
                    let _ = token;
                    0i32
                }
            })
            .unwrap();

        let token = engine.cancellation_token();
        let handle = std::thread::spawn(move || block_on(engine.eval_async(expr.as_ref())));

        started_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("stall native never started");
        token.cancel();

        let res = handle.join().unwrap();
        assert!(matches!(res, Err(EngineError::Cancelled)));
    }

    #[test]
    fn sync_eval_can_be_cancelled_while_blocking_on_async_native() {
        let expr = parse("stall");
        let mut engine = Engine::with_prelude();

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
    fn eval_with_gas_rejects_out_of_budget() {
        let expr = parse("1");
        let mut engine = Engine::with_prelude();
        let mut gas = GasMeter::new(Some(0), GasCosts { eval_node: 1, ..GasCosts::sensible_defaults() });
        let err = match engine.eval_with_gas(expr.as_ref(), &mut gas) {
            Ok(_) => panic!("expected out of gas"),
            Err(e) => e,
        };
        assert!(matches!(err, EngineError::OutOfGas(..)));
    }

    #[test]
    fn native_per_impl_gas_cost_is_charged() {
        let expr = parse("foo");
        let mut engine = Engine::with_prelude();
        let scheme = Scheme::new(vec![], vec![], Type::con("i32", 0));
        engine
            .inject_native_scheme_typed_with_gas_cost("foo", scheme, 0, 50, |_e, _t, _args| {
                Ok(Value::I32(1))
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
    fn eval_deep_list_does_not_overflow() {
        // Regression test: deeply nested terms (right-nested arguments) can overflow the default
        // Rust stack during typechecking/evaluation unless callers opt into a larger stack.
        const N: usize = 2_000;
        let mut code = String::new();
        code.push_str("let xs = ");
        for _ in 0..N {
            code.push_str("Cons 0 (");
        }
        code.push_str("Empty");
        for _ in 0..N {
            code.push(')');
        }
        code.push_str(" in xs");

        let tokens = rex_lexer::Token::tokenize(&code).unwrap();
        let program = rex_parser::Parser::new(tokens)
            .parse_program_with_stack_size(128 * 1024 * 1024)
            .unwrap();
        let expr = program.expr;
        let mut engine = Engine::with_prelude();
        let value = engine
            .eval_with_stack_size(expr.as_ref(), 64 * 1024 * 1024)
            .unwrap();
        let xs = list_values(&value);
        assert_eq!(xs.len(), N);
        assert!(matches!(xs.first(), Some(Value::I32(0))));
        assert!(matches!(xs.last(), Some(Value::I32(0))));
    }

    #[test]
    fn eval_type_annotation_let() {
        let expr = parse("let x: i32 = 42 in x");
        let mut engine = engine_with_arith();
        let value = engine.eval(expr.as_ref()).unwrap();
        assert!(matches!(value, Value::I32(42)));
    }

    #[test]
    fn eval_type_annotation_is() {
        let expr = parse("\"hi\" is str");
        let mut engine = engine_with_arith();
        let value = engine.eval(expr.as_ref()).unwrap();
        assert!(matches!(value, Value::String(ref s) if s == "hi"));
    }

    #[test]
    fn eval_type_annotation_lambda_param() {
        let expr = parse("let f = \\ (a : f32) -> a in f 1.5");
        let mut engine = engine_with_arith();
        let value = engine.eval(expr.as_ref()).unwrap();
        assert!(matches!(value, Value::F32(v) if (v - 1.5).abs() < f32::EPSILON));
    }

    #[test]
    fn eval_record_update_single_variant_adt() {
        let program = parse_program(
            r#"
            type Foo = Bar { x: i32, y: i32, z: i32 }
            let
              foo: Foo = Bar { x = 1, y = 2, z = 3 },
              bar: Foo = { foo with { x = 6 } }
            in
              bar.x
            "#,
        );
        let mut engine = engine_with_arith();
        engine.inject_decls(&program.decls).unwrap();
        let value = engine.eval(program.expr.as_ref()).unwrap();
        assert!(matches!(value, Value::I32(6)));
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

    #[test]
    fn eval_record_update_refined_by_match() {
        let program = parse_program(
            r#"
            type Foo = Bar { x: i32 } | Baz { x: i32 }
            let
              foo: Foo = Bar { x = 1 }
            in
              match foo
                when Bar {x} -> (match { foo with { x = x + 1 } } when Bar {x} -> x when Baz {x} -> x)
                when Baz {x} -> (match { foo with { x = x + 2 } } when Bar {x} -> x when Baz {x} -> x)
            "#,
        );
        let mut engine = engine_with_arith();
        engine.inject_decls(&program.decls).unwrap();
        let value = engine.eval(program.expr.as_ref()).unwrap();
        assert!(matches!(value, Value::I32(2)));
    }

    #[test]
    fn eval_record_update_plain_record_type() {
        let program = parse_program(
            r#"
            let
              f = \ (r : { x: i32, y: i32 }) -> { r with { y = 9 } }
            in
              match (f { x = 1, y = 2 }) when {y} -> y
            "#,
        );
        let mut engine = engine_with_arith();
        engine.inject_decls(&program.decls).unwrap();
        let value = engine.eval(program.expr.as_ref()).unwrap();
        assert!(matches!(value, Value::I32(9)));
    }

    #[test]
    fn eval_type_annotation_mismatch() {
        let expr = parse("let x: i32 = 3.14 in x");
        let mut engine = engine_with_arith();
        match engine.eval(expr.as_ref()) {
            Err(EngineError::Type(err)) => {
                let err = strip_span(err);
                assert!(matches!(err, TypeError::Unification(_, _)));
            }
            Err(other) => panic!("expected type error, got {other:?}"),
            Ok(_) => panic!("expected type error, got Ok"),
        }
    }

    #[test]
    fn eval_native_injection() {
        let mut engine = Engine::new();
        engine.inject_fn0("zero", || -> u32 { 0u32 }).unwrap();
        engine
            .inject_fn2("(+)", |x: u32, y: u32| -> u32 { x + y })
            .unwrap();
        engine.inject_value("one", 1u32).unwrap();

        let expr = parse("one + one");
        let value = engine.eval(expr.as_ref()).unwrap();
        assert!(matches!(value, Value::U32(2)));

        let expr = parse("zero");
        let value = engine.eval(expr.as_ref()).unwrap();
        assert!(matches!(value, Value::U32(0)));
    }

    #[test]
    fn eval_match_list() {
        let mut engine = engine_with_arith();

        let expr = parse(
            r#"
            match [1, 2, 3]
                when [] -> 0
                when x:xs -> x
            "#,
        );
        let value = engine.eval(expr.as_ref()).unwrap();
        assert!(matches!(value, Value::I32(1)));
    }

    #[test]
    fn eval_simple_addition() {
        let expr = parse("420 + 69");
        let mut engine = engine_with_arith();
        let value = engine.eval(expr.as_ref()).unwrap();
        assert!(matches!(value, Value::I32(489)));
    }

    #[test]
    fn eval_simple_mod() {
        let expr = parse("10 % 3");
        let mut engine = engine_with_arith();
        let value = engine.eval(expr.as_ref()).unwrap();
        assert!(matches!(value, Value::I32(1)));
    }

    #[test]
    fn eval_get_list_and_tuple() {
        let mut engine = engine_with_arith();

        let expr = parse("get 1 [1, 2, 3]");
        let value = engine.eval(expr.as_ref()).unwrap();
        assert!(matches!(value, Value::I32(2)));

        let expr = parse("get 2 (1, 2, 3)");
        let value = engine.eval(expr.as_ref()).unwrap();
        assert!(matches!(value, Value::I32(3)));
    }

    #[test]
    fn eval_simple_multiplication_float() {
        let expr = parse("420.0 * 6.9");
        let mut engine = engine_with_arith();
        let value = engine.eval(expr.as_ref()).unwrap();
        match value {
            Value::F32(v) => assert!((v - 2898.0).abs() < 1e-3),
            _ => panic!("expected f32 result"),
        }
    }

    #[test]
    fn eval_let_id_nested() {
        let expr = parse(
            r#"
            let
                id = \x -> x
            in
                id (id 420 + id 69)
            "#,
        );
        let mut engine = engine_with_arith();
        let value = engine.eval(expr.as_ref()).unwrap();
        assert!(matches!(value, Value::I32(489)));
    }

    #[test]
    fn eval_higher_order_add() {
        let expr = parse(
            r#"
            let
                add = \x -> \y -> x + y
            in
                add 40 2
            "#,
        );
        let mut engine = engine_with_arith();
        let value = engine.eval(expr.as_ref()).unwrap();
        assert!(matches!(value, Value::I32(42)));
    }

    #[test]
    fn eval_match_dict_and_tuple() {
        let expr = parse(
            r#"
            let
                inc = \x -> x + 1
            in
                match { foo = 1, bar = 2 }
                    when {foo, bar} -> (inc foo, inc bar)
            "#,
        );
        let mut engine = engine_with_arith();
        let value = engine.eval(expr.as_ref()).unwrap();
        match value {
            Value::Tuple(xs) => {
                assert_eq!(xs.len(), 2);
                assert!(matches!(xs[0], Value::I32(2)));
                assert!(matches!(xs[1], Value::I32(3)));
            }
            _ => panic!("expected tuple result"),
        }
    }

    #[test]
    fn eval_match_missing_arm_errors() {
        let expr = parse("match (Err 1) when Ok x -> x");
        let mut engine = Engine::with_prelude();
        let result = engine.eval(expr.as_ref());
        match result {
            Err(EngineError::Type(err)) => {
                let err = strip_span(err);
                assert!(matches!(err, TypeError::NonExhaustiveMatch { .. }));
            }
            _ => panic!("expected non-exhaustive match type error"),
        }
    }

    #[test]
    fn eval_match_invalid_pattern_type_error() {
        let expr = parse("match (Ok 1) when [] -> 0 when x:xs -> 1");
        let mut engine = Engine::with_prelude();
        let result = engine.eval(expr.as_ref());
        match result {
            Err(EngineError::Type(err)) => {
                let err = strip_span(err);
                assert!(matches!(err, TypeError::Unification(_, _)));
            }
            _ => panic!("expected unification type error"),
        }
    }

    #[test]
    fn eval_nested_match_list_sum() {
        let expr = parse(
            r#"
            match [1, 2, 3]
                when x:xs ->
                    (match xs
                        when [] -> x
                        when y:ys -> x + y)
                when [] -> 0
            "#,
        );
        let mut engine = engine_with_arith();
        let value = engine.eval(expr.as_ref()).unwrap();
        assert!(matches!(value, Value::I32(3)));
    }

    #[test]
    fn eval_safe_div_pipeline() {
        let expr = parse(
            r#"
            let
                id = \x -> x,
                safeDiv = \a b -> if b == 0.0 then None else Some (a / b),
                noneToZero = \x -> match x when None -> zero when Some y -> y,
                someToOne = \x -> match x when Some _ -> one when None -> zero
            in
                (
                    someToOne ((id safeDiv) (id 420.0) (id 6.9)),
                    someToOne (safeDiv 420.0 6.9),
                    noneToZero (safeDiv 420.0 0.0)
                )
            "#,
        );
        let mut engine = Engine::with_prelude();
        let value = engine.eval(expr.as_ref()).unwrap();
        match value {
            Value::Tuple(xs) => {
                assert_eq!(xs.len(), 3);
                match xs[0] {
                    Value::F32(v) => assert!((v - 1.0).abs() < 1e-3),
                    _ => panic!("expected f32 one"),
                }
                match xs[1] {
                    Value::F32(v) => assert!((v - 1.0).abs() < 1e-3),
                    _ => panic!("expected f32 one"),
                }
                match xs[2] {
                    Value::F32(v) => assert!((v - 0.0).abs() < 1e-3),
                    _ => panic!("expected f32 zero"),
                }
            }
            _ => panic!("expected tuple result"),
        }
    }

    #[test]
    fn eval_user_adt_declaration() {
        let program = parse_program(
            r#"
            type Boxed a = Box a
            let
                value = Box 42
            in
                match value
                    when Box x -> x
            "#,
        );
        let mut engine = Engine::with_prelude();
        for decl in &program.decls {
            if let rex_ast::expr::Decl::Type(ty) = decl {
                engine.inject_type_decl(ty).unwrap();
            }
        }
        let value = engine.eval(program.expr.as_ref()).unwrap();
        assert!(matches!(value, Value::I32(42)));
    }

    #[test]
    fn eval_fn_decl_simple() {
        let program = parse_program(
            r#"
            fn add (x: i32, y: i32) -> i32 = x + y
            add 1 2
            "#,
        );
        let mut engine = Engine::with_prelude();
        for decl in &program.decls {
            if let rex_ast::expr::Decl::Type(ty) = decl {
                engine.inject_type_decl(ty).unwrap();
            }
        }
        let expr = program.expr_with_fns();
        let value = engine.eval(expr.as_ref()).unwrap();
        assert!(matches!(value, Value::I32(3)));
    }

    #[test]
    fn eval_fn_decl_with_where_constraints() {
        let program = parse_program(
            r#"
            fn my_add (x: a, y: a) -> a where AdditiveMonoid a = x + y
            my_add 1 2
            "#,
        );
        let mut engine = Engine::with_prelude();
        for decl in &program.decls {
            if let rex_ast::expr::Decl::Type(ty) = decl {
                engine.inject_type_decl(ty).unwrap();
            }
        }
        let expr = program.expr_with_fns();
        let value = engine.eval(expr.as_ref()).unwrap();
        assert!(matches!(value, Value::I32(3)));
    }

    #[test]
    fn eval_adt_record_projection_single_variant() {
        let program = parse_program(
            r#"
            type MyADT = MyVariant1 { field1: i32, field2: f32 }
            let
                x = MyVariant1 { field1 = 1, field2 = 2.0 }
            in
                (x.field1, x.field2)
            "#,
        );
        let mut engine = Engine::with_prelude();
        for decl in &program.decls {
            if let rex_ast::expr::Decl::Type(ty) = decl {
                engine.inject_type_decl(ty).unwrap();
            }
        }
        let value = engine.eval(program.expr.as_ref()).unwrap();
        match value {
            Value::Tuple(xs) => {
                assert!(matches!(xs[0], Value::I32(1)));
                match xs[1] {
                    Value::F32(v) => assert!((v - 2.0).abs() < 1e-3),
                    _ => panic!("expected f32 field"),
                }
            }
            _ => panic!("expected tuple result"),
        }
    }

    #[test]
    fn eval_adt_record_projection_match_arm() {
        let program = parse_program(
            r#"
            type MyADT = MyVariant1 { field1: i32 } | MyVariant2 i32
            let
                x = MyVariant1 { field1 = 1 }
            in
                match x
                    when MyVariant1 { field1 } -> x.field1
                    when MyVariant2 _ -> 0
            "#,
        );
        let mut engine = Engine::with_prelude();
        for decl in &program.decls {
            if let rex_ast::expr::Decl::Type(ty) = decl {
                engine.inject_type_decl(ty).unwrap();
            }
        }
        let value = engine.eval(program.expr.as_ref()).unwrap();
        assert!(matches!(value, Value::I32(1)));
    }

    #[test]
    fn eval_list_map_fold_filter() {
        let expr = parse(
            r#"
            let
                xs = [1, 2, 3],
                ys = map (\x -> x + 1) xs,
                zs = filter (\x -> x == 2) xs,
                total = foldl (\acc x -> acc + x) 0 xs
            in
                (ys, zs, total)
            "#,
        );
        let mut engine = Engine::with_prelude();
        let value = engine.eval(expr.as_ref()).unwrap();
        match value {
            Value::Tuple(xs) => {
                assert_eq!(xs.len(), 3);
                let vals = list_values(&xs[0]);
                assert_eq!(vals.len(), 3);
                assert!(matches!(vals[0], Value::I32(2)));
                assert!(matches!(vals[1], Value::I32(3)));
                assert!(matches!(vals[2], Value::I32(4)));
                let vals = list_values(&xs[1]);
                assert_eq!(vals.len(), 1);
                assert!(matches!(vals[0], Value::I32(2)));
                assert!(matches!(xs[2], Value::I32(6)));
            }
            _ => panic!("expected tuple result"),
        }
    }

    #[test]
    fn eval_list_flat_map_zip_unzip() {
        let expr = parse(
            r#"
            let
                xs = bind (\x -> [x, x]) [1, 2],
                pairs = zip [1, 2] [3, 4],
                unzipped = unzip pairs
            in
                (xs, unzipped)
            "#,
        );
        let mut engine = Engine::with_prelude();
        let value = engine.eval(expr.as_ref()).unwrap();
        match value {
            Value::Tuple(xs) => {
                assert_eq!(xs.len(), 2);
                let vals = list_values(&xs[0]);
                assert_eq!(vals.len(), 4);
                assert!(matches!(vals[0], Value::I32(1)));
                assert!(matches!(vals[1], Value::I32(1)));
                assert!(matches!(vals[2], Value::I32(2)));
                assert!(matches!(vals[3], Value::I32(2)));
                match &xs[1] {
                    Value::Tuple(parts) => {
                        assert_eq!(parts.len(), 2);
                        list_values(&parts[0]);
                        list_values(&parts[1]);
                    }
                    _ => panic!("expected unzip tuple"),
                }
            }
            _ => panic!("expected tuple result"),
        }
    }

    #[test]
    fn eval_list_sum_mean_min_max() {
        let expr = parse(
            r#"
            let
                s = sum [1, 2, 3],
                m = mean [1.0, 2.0, 3.0],
                lo = min [3, 1, 2],
                hi = max [3, 1, 2]
            in
                (s, m, lo, hi)
            "#,
        );
        let mut engine = Engine::with_prelude();
        let value = engine.eval(expr.as_ref()).unwrap();
        match value {
            Value::Tuple(xs) => {
                assert_eq!(xs.len(), 4);
                assert!(matches!(xs[0], Value::I32(6)));
                match xs[1] {
                    Value::F32(v) => assert!((v - 2.0).abs() < 1e-3),
                    _ => panic!("expected mean f32"),
                }
                assert!(matches!(xs[2], Value::I32(1)));
                assert!(matches!(xs[3], Value::I32(3)));
            }
            _ => panic!("expected tuple result"),
        }
    }

    #[test]
    fn eval_option_result_helpers() {
        let expr = parse(
            r#"
            let
                opt = map (\x -> x + 1) (Some 1),
                opt2 = bind (\x -> Some (x + 1)) opt,
                res = map (\x -> x + 1) (Ok 1),
                ok = is_ok res,
                err = is_err (Err "nope")
            in
                (opt2, res, ok, err)
            "#,
        );
        let mut engine = Engine::with_prelude();
        let value = engine.eval(expr.as_ref()).unwrap();
        match value {
            Value::Tuple(xs) => {
                assert_eq!(xs.len(), 4);
                assert!(matches!(xs[0], Value::Adt(ref n, _) if sym_eq(n, "Some")));
                assert!(matches!(xs[1], Value::Adt(ref n, _) if sym_eq(n, "Ok")));
                assert!(matches!(xs[2], Value::Bool(true)));
                assert!(matches!(xs[3], Value::Bool(true)));
            }
            _ => panic!("expected tuple result"),
        }
    }

    #[test]
    fn eval_order_ops() {
        let expr = parse(
            r#"
            let
                a = 1 < 2,
                b = 2 <= 2,
                c = 3 > 2,
                d = 2 >= 3,
                e = "a" < "b"
            in
                (a, b, c, d, e)
            "#,
        );
        let mut engine = Engine::with_prelude();
        let value = engine.eval(expr.as_ref()).unwrap();
        match value {
            Value::Tuple(xs) => {
                assert_eq!(xs.len(), 5);
                assert!(matches!(xs[0], Value::Bool(true)));
                assert!(matches!(xs[1], Value::Bool(true)));
                assert!(matches!(xs[2], Value::Bool(true)));
                assert!(matches!(xs[3], Value::Bool(false)));
                assert!(matches!(xs[4], Value::Bool(true)));
            }
            _ => panic!("expected tuple result"),
        }
    }

    #[test]
    fn eval_option_and_then_or_else() {
        let expr = parse(
            r#"
            let
                inc_if_pos = \x -> if x > 0 then Some (x + 1) else None,
                a = bind inc_if_pos (Some 1),
                b = bind inc_if_pos (Some 0),
                c = or_else (\x -> Some 42) b
            in
                (a, b, c)
            "#,
        );
        let mut engine = Engine::with_prelude();
        let value = engine.eval(expr.as_ref()).unwrap();
        match value {
            Value::Tuple(xs) => {
                assert_eq!(xs.len(), 3);
                assert!(matches!(xs[0], Value::Adt(ref n, _) if sym_eq(n, "Some")));
                assert!(matches!(xs[1], Value::Adt(ref n, _) if sym_eq(n, "None")));
                assert!(matches!(xs[2], Value::Adt(ref n, _) if sym_eq(n, "Some")));
            }
            _ => panic!("expected tuple result"),
        }
    }

    #[test]
    fn eval_result_filter_pipeline() {
        let expr = parse(
            r#"
            let
                classify = \x -> if x < 2 then Err x else Ok x,
                xs = [0, 2, 3],
                ys = map classify xs,
                zs = filter_map (\x -> match x when Ok v -> Some v when Err _ -> None) ys,
                total = sum zs
            in
                (count ys, total)
            "#,
        );
        let mut engine = Engine::with_prelude();
        let value = engine.eval(expr.as_ref()).unwrap();
        match value {
            Value::Tuple(xs) => {
                assert_eq!(xs.len(), 2);
                assert!(matches!(xs[0], Value::I32(3)));
                assert!(matches!(xs[1], Value::I32(5)));
            }
            _ => panic!("expected tuple result"),
        }
    }

    #[test]
    fn eval_array_combinators() {
        let mut engine = Engine::with_prelude();
        engine.inject_value("arr", vec![1i32, 2i32, 3i32]).unwrap();
        let expr = parse(
            r#"
            let
                mapped = map (\x -> x + 1) arr,
                total = sum arr,
                taken = take 2 arr,
                skipped = skip 1 arr,
                pairs = zip arr mapped,
                unzipped = unzip pairs
            in
                (mapped, total, taken, skipped, unzipped)
            "#,
        );
        let value = engine.eval(expr.as_ref()).unwrap();
        match value {
            Value::Tuple(xs) => {
                assert_eq!(xs.len(), 5);
                match &xs[0] {
                    Value::Array(vals) => {
                        assert_eq!(vals.len(), 3);
                        assert!(matches!(vals[0], Value::I32(2)));
                        assert!(matches!(vals[1], Value::I32(3)));
                        assert!(matches!(vals[2], Value::I32(4)));
                    }
                    _ => panic!("expected mapped array"),
                }
                assert!(matches!(xs[1], Value::I32(6)));
                match &xs[2] {
                    Value::Array(vals) => {
                        assert_eq!(vals.len(), 2);
                        assert!(matches!(vals[0], Value::I32(1)));
                        assert!(matches!(vals[1], Value::I32(2)));
                    }
                    _ => panic!("expected taken array"),
                }
                match &xs[3] {
                    Value::Array(vals) => {
                        assert_eq!(vals.len(), 2);
                        assert!(matches!(vals[0], Value::I32(2)));
                        assert!(matches!(vals[1], Value::I32(3)));
                    }
                    _ => panic!("expected skipped array"),
                }
                match &xs[4] {
                    Value::Tuple(parts) => {
                        assert_eq!(parts.len(), 2);
                        match &parts[0] {
                            Value::Array(vals) => assert_eq!(vals.len(), 3),
                            _ => panic!("expected unzipped left array"),
                        }
                        match &parts[1] {
                            Value::Array(vals) => assert_eq!(vals.len(), 3),
                            _ => panic!("expected unzipped right array"),
                        }
                    }
                    _ => panic!("expected unzipped tuple"),
                }
            }
            _ => panic!("expected tuple result"),
        }
    }
}

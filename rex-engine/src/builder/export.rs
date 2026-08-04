use crate::{
    builder::core::NativeRegistration,
    error::EngineError,
    evaluator::{context::Context, native_callable::HostValueCallable},
    handlers::declare_fn_decl_from_scheme,
    memory::traits::IntoRex,
    modules::ROOT_MODULE_NAME,
    util::{normalize_name, validate_host_value_export_scheme},
};
use futures::{FutureExt, future::BoxFuture};
use rex_ast::DeclareFnDecl;
use rex_typesystem::types::{RexType, Scheme, Type};
use std::sync::Arc;

pub trait ExportTarget<State: Clone + Send + Sync + 'static> {
    fn register_native_registration(
        &mut self,
        module_name: &str,
        export_name: &str,
        registration: NativeRegistration<State>,
    ) -> Result<(), EngineError>;
}

type ExportInjector<State> =
    Box<dyn FnOnce(&mut dyn ExportTarget<State>, &str) -> Result<(), EngineError> + Send + 'static>;

pub(crate) enum ExportPayload<State: Clone + Send + Sync + 'static> {
    Injector(ExportInjector<State>),
    Value { value: crate::Value, typ: Type },
}

pub struct Export<State: Clone + Send + Sync + 'static> {
    pub name: String,
    pub(crate) interface: DeclareFnDecl,
    pub(crate) payload: ExportPayload<State>,
}

impl<State> Export<State>
where
    State: Clone + Send + Sync + 'static,
{
    pub(crate) fn is_value(&self) -> bool {
        matches!(&self.payload, ExportPayload::Value { .. })
    }

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
            payload: ExportPayload::Injector(injector),
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
        F: for<'a> Fn(
                Context<State>,
                &'a Type,
                Vec<crate::Value>,
            ) -> Result<crate::Value, EngineError>
            + Send
            + Sync
            + 'static,
    {
        validate_host_value_export_scheme(&scheme, arity)?;
        let name = name.into();
        let normalized = normalize_name(&name).to_string();
        let interface = declare_fn_decl_from_scheme(&normalized, &scheme);
        let handler = Arc::new(handler);
        let injector: ExportInjector<State> = Box::new(move |engine, qualified_name| {
            let handler = Arc::clone(&handler);
            let func: HostValueCallable<State> = Arc::new(move |engine, typ, args| {
                let result = handler(engine, &typ, args);
                async move { result }.boxed()
            });
            let registration = NativeRegistration::sync(scheme.clone(), arity, func);
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
        F: Fn(Context<State>, Type, Vec<crate::Value>) -> NativeFuture + Send + Sync + 'static,
    {
        validate_host_value_export_scheme(&scheme, arity)?;
        let name = name.into();
        let normalized = normalize_name(&name).to_string();
        let interface = declare_fn_decl_from_scheme(&normalized, &scheme);
        let handler = Arc::new(handler);
        let injector: ExportInjector<State> = Box::new(move |engine, qualified_name| {
            let handler = Arc::clone(&handler);
            let func: HostValueCallable<State> =
                Arc::new(move |engine, typ, args| handler(engine, typ, args));
            let registration = NativeRegistration::r#async(scheme.clone(), arity, func);
            engine.register_native_registration(ROOT_MODULE_NAME, qualified_name, registration)
        });
        Self::from_injector(name, interface, injector)
    }

    pub fn from_value<V>(name: impl Into<String>, value: V) -> Result<Self, EngineError>
    where
        V: IntoRex + RexType + Send + Sync + 'static,
    {
        let name = name.into();
        let typ = V::rex_type();
        let interface = declare_fn_decl_from_scheme(
            normalize_name(&name).as_ref(),
            &Scheme::new(vec![], vec![], typ.clone()),
        );
        let name = normalize_name(&name).to_string();
        Ok(Self {
            name,
            interface,
            payload: ExportPayload::Value {
                value: value.into_rex()?,
                typ,
            },
        })
    }
}

pub trait HostFnSync<State: Clone + Send + Sync + 'static, Sig>: Send + Sync + 'static {
    fn interface_decl(export_name: &str) -> DeclareFnDecl;
    fn interface_decl_for(&self, export_name: &str) -> DeclareFnDecl {
        Self::interface_decl(export_name)
    }
    fn inject(
        self,
        engine: &mut dyn ExportTarget<State>,
        export_name: &str,
    ) -> Result<(), EngineError>;
}

pub trait HostFnAsync<State: Clone + Send + Sync + 'static, Sig>: Send + Sync + 'static {
    fn interface_decl(export_name: &str) -> DeclareFnDecl;
    fn interface_decl_for(&self, export_name: &str) -> DeclareFnDecl {
        Self::interface_decl(export_name)
    }
    fn inject_async(
        self,
        engine: &mut dyn ExportTarget<State>,
        export_name: &str,
    ) -> Result<(), EngineError>;
}

pub type NativeFuture = BoxFuture<'static, Result<crate::Value, EngineError>>;

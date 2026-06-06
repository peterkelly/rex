use crate::{
    builder::core::NativeRegistration,
    error::EngineError,
    evaluator::{
        context::Context,
        native_callable::{AsyncNativePointerCallable, SyncNativePointerCallable},
    },
    handlers::declare_fn_decl_from_scheme,
    modules::ROOT_MODULE_NAME,
    util::{normalize_name, validate_native_export_scheme},
    value::{Handle, IntoRex, Pointer},
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

pub struct Export<State: Clone + Send + Sync + 'static> {
    pub name: String,
    pub(crate) interface: DeclareFnDecl,
    pub(crate) injector: ExportInjector<State>,
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
        F: for<'a> Fn(Context<State>, &'a Type, &'a [Handle]) -> Result<Handle, EngineError>
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

    pub fn from_native_async<F>(
        name: impl Into<String>,
        scheme: Scheme,
        arity: usize,
        handler: F,
    ) -> Result<Self, EngineError>
    where
        F: Fn(Context<State>, Type, Vec<Handle>) -> NativeFuture + Send + Sync + 'static,
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

pub type NativeFuture = BoxFuture<'static, Result<Handle, EngineError>>;

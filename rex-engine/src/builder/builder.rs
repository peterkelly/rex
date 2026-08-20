use crate::{
    Value,
    builder::{
        export::{Export, ExportPayload, ExportTarget},
        qualify::qualify_package,
        registry::{NativeRegistry, TypeclassRegistry},
    },
    compiler::{
        Compiler,
        type_check::{check_natives, type_check_engine},
    },
    config::{
        AsyncCallPolicy, EngineOptions, ExecutionBounds, FixedParallelismController,
        ParallelismController, PreludeMode,
    },
    env::RootedEnvironment,
    error::EngineError,
    evaluator::{
        intrinsic_handler::InternalFnSync,
        native_callable::{
            HostValueCallable, NativeCallScheduling, NativeCallable, SchedulerNativeCallable,
            SchedulerNativeResult,
        },
    },
    handlers::RexDefault,
    memory::{
        heap::{Heap, RootScope, RootedPtr},
        traits::IntoRex,
    },
    modules::{
        CompilationPackage, Declarations, ImportRequest, Importer, Module, ModuleExports, ModuleId,
        ModuleSystem, ROOT_MODULE_NAME, ResolvedModule, ResolvedModuleContent, VirtualModule,
        exports_from_package, interface_decls_from_package, prefix_for_module, virtual_export_name,
    },
    prelude::{inject_prelude, inject_prelude_virtual_module, standard_type_system},
    util::{
        adt_family_error_to_engine, normalize_name, split_fun, type_arity,
        validate_native_export_scheme,
    },
};
use futures::future::BoxFuture;
use rex_ast::{DeclareFnDecl, Expr, FnDecl, InstanceDecl, Scope, Symbol};
use rex_typesystem::{
    inference::infer,
    types::{
        AdtDecl, Instance, Predicate, RexAdt, RexType, Scheme, Type, TypeKind, TypeVar, TypedExpr,
        TypedExprKind, Types, adt_shape, adt_shape_eq, compatibility_constructor_name,
        order_adt_family,
    },
    typesystem::{PreparedInstanceDecl, TypeSystem, TypeVarSupply, entails, instantiate},
    unification::unify,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

pub struct Builder<State = ()>
where
    State: Clone + Send + Sync + 'static,
{
    pub(crate) state: Arc<State>,
    pub(crate) env: RootedEnvironment,
    pub(crate) type_system: TypeSystem,
    pub(crate) runtime: RuntimeRegistry<State>,
    pub(crate) module_loader: ModuleLoaderState<State>,
    pub(crate) policy: RuntimePolicy,
    pub(crate) heap: Heap,
}

impl<State> Default for Builder<State>
where
    State: Clone + Send + Sync + 'static + Default,
{
    fn default() -> Self {
        Self::new(State::default())
    }
}

impl<State> Builder<State>
where
    State: Clone + Send + Sync + 'static,
{
    pub fn new(state: State) -> Self {
        Self {
            state: Arc::new(state),
            env: RootedEnvironment::new(),
            type_system: TypeSystem::new(),
            runtime: RuntimeRegistry {
                natives: NativeRegistry::<State>::default(),
                typeclasses: TypeclassRegistry::default(),
            },
            module_loader: ModuleLoaderState::new(Vec::new()),
            policy: RuntimePolicy::default(),
            heap: Heap::new(),
        }
    }

    pub fn with_prelude(state: State) -> Result<Self, EngineError> {
        Self::with_options(state, EngineOptions::default())
    }

    pub fn with_options(state: State, options: EngineOptions) -> Result<Self, EngineError> {
        let type_system = match options.prelude {
            PreludeMode::Enabled => standard_type_system()?,
            PreludeMode::Disabled => TypeSystem::new(),
        };
        let mut builder = Self {
            state: Arc::new(state),
            env: RootedEnvironment::new(),
            type_system,
            runtime: RuntimeRegistry {
                natives: NativeRegistry::<State>::default(),
                typeclasses: TypeclassRegistry::default(),
            },
            module_loader: ModuleLoaderState::new(options.default_imports),
            policy: RuntimePolicy::default(),
            heap: Heap::new(),
        };
        if matches!(options.prelude, PreludeMode::Enabled) {
            inject_prelude(&mut builder)?;
            inject_prelude_virtual_module(&mut builder)?;
        }
        Ok(builder)
    }

    pub fn build_compiler(self) -> Compiler<State> {
        Compiler::from_builder(self)
    }

    pub fn async_call_policy(&self) -> &AsyncCallPolicy {
        &self.policy.async_call_policy
    }

    pub fn set_async_call_policy(&mut self, policy: AsyncCallPolicy) {
        self.policy.async_call_policy = policy;
    }

    pub fn with_async_call_policy(mut self, policy: AsyncCallPolicy) -> Self {
        self.set_async_call_policy(policy);
        self
    }

    pub fn execution_bounds(&self) -> ExecutionBounds {
        self.policy.execution_bounds
    }

    pub fn set_execution_bounds(&mut self, bounds: ExecutionBounds) {
        self.policy.execution_bounds = bounds;
        self.policy.parallelism_controller = Arc::new(FixedParallelismController::new(bounds));
    }

    pub fn with_execution_bounds(mut self, bounds: ExecutionBounds) -> Self {
        self.set_execution_bounds(bounds);
        self
    }

    pub fn set_parallelism_controller(&mut self, controller: Arc<dyn ParallelismController>) {
        self.policy.parallelism_controller = controller;
    }

    pub fn with_parallelism_controller(
        mut self,
        controller: Arc<dyn ParallelismController>,
    ) -> Self {
        self.set_parallelism_controller(controller);
        self
    }

    pub fn set_default_imports(&mut self, imports: Vec<String>) {
        self.module_loader.default_imports = imports;
    }

    pub fn default_imports(&self) -> &[String] {
        &self.module_loader.default_imports
    }

    pub fn inject_module(&mut self, module: Module<State>) -> Result<(), EngineError> {
        let module_name = module.name.trim().to_string();
        if module_name.is_empty() {
            return Err(EngineError::Internal("module name cannot be empty".into()));
        }
        let is_global = module_name == ROOT_MODULE_NAME;
        if !is_global && self.module_loader.injected_modules.contains(&module_name) {
            return Err(EngineError::Internal(format!(
                "module `{module_name}` already injected"
            )));
        }

        if is_global {
            for staged in &module.adts {
                self.inject_adt(staged.adt.clone())?;
            }

            let mut decls = module.declarations();
            decls.types.clear();

            for internal in module.internals {
                self.inject_module_export(ROOT_MODULE_NAME, internal)?;
            }

            let (value_exports, callable_exports): (Vec<_>, Vec<_>) =
                module.exports.into_iter().partition(Export::is_value);
            for export in callable_exports {
                self.inject_module_export(ROOT_MODULE_NAME, export)?;
            }
            self.inject_decls(&decls)?;
            for export in value_exports {
                self.inject_module_export(ROOT_MODULE_NAME, export)?;
            }
            return Ok(());
        }

        let module_id = ModuleId::parse(&module_name)?;
        let installed = install_named_rust_module(self, &module_id, module)?;
        self.module_loader
            .system
            .prepend_importer(Arc::new(StaticModuleImporter {
                module_id: installed.id.clone(),
                resolved: ResolvedModule {
                    id: installed.id,
                    content: ResolvedModuleContent::CompilationPackage(installed.package.clone()),
                },
            }));
        Ok(())
    }

    pub fn inject_rex_default_instance<T>(&mut self) -> Result<(), EngineError>
    where
        T: RexType + RexDefault<State> + IntoRex,
    {
        let class = Symbol::intern("Default");
        let method = Symbol::intern("default");
        let head_ty = T::rex_type();
        if !self.type_system.class_methods.contains_key(&method) {
            return Err(EngineError::UnknownVar(method));
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

        let mut module = Module::global();
        module.add_rex_default_instance::<T>()?;
        self.inject_module(module)
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

    pub fn inject_instance(&mut self, class: &str, inst: Instance) {
        self.type_system.register_instance(class, inst);
    }

    pub fn add_importer(&mut self, importer: Arc<dyn Importer<State>>) {
        self.module_loader.system.append_importer(importer);
    }

    pub fn type_system(&self) -> &TypeSystem {
        &self.type_system
    }

    pub fn type_system_mut(&mut self) -> &mut TypeSystem {
        &mut self.type_system
    }

    #[doc(hidden)]
    pub fn set_extreme_gc_stress(&mut self, enabled: bool) {
        self.heap.set_extreme_stress(enabled);
    }
}

pub(crate) struct RuntimeRegistry<State>
where
    State: Clone + Send + Sync + 'static,
{
    pub(crate) natives: NativeRegistry<State>,
    pub(crate) typeclasses: TypeclassRegistry,
}

pub(crate) struct ModuleLoaderState<State: Clone + Send + Sync + 'static> {
    pub(crate) system: ModuleSystem<State>,
    pub(crate) injected_modules: BTreeSet<String>,
    pub(crate) module_exports_cache: BTreeMap<ModuleId, ModuleExports>,
    pub(crate) module_interface_cache: BTreeMap<ModuleId, Declarations>,
    pub(crate) module_sources: BTreeMap<ModuleId, String>,
    pub(crate) published_cycle_interfaces: BTreeSet<ModuleId>,
    pub(crate) default_imports: Vec<String>,
    pub(crate) virtual_modules: BTreeMap<String, VirtualModule>,
    pub(crate) module_local_type_names: BTreeMap<String, BTreeSet<Symbol>>,
    pub(crate) registration_module_context: Option<String>,
}

pub(crate) struct RuntimePolicy {
    pub(crate) async_call_policy: AsyncCallPolicy,
    pub(crate) execution_bounds: ExecutionBounds,
    pub(crate) parallelism_controller: Arc<dyn ParallelismController>,
}

pub(crate) struct InstalledRustModule {
    pub(crate) id: ModuleId,
    pub(crate) exports: ModuleExports,
    pub(crate) package: CompilationPackage,
}

pub(crate) trait RustModuleInstallContext<State>: ExportTarget<State>
where
    State: Clone + Send + Sync + 'static,
{
    fn module_loader(&self) -> &ModuleLoaderState<State>;
    fn module_loader_mut(&mut self) -> &mut ModuleLoaderState<State>;
    fn inject_decls(&mut self, decls: &Declarations) -> Result<(), EngineError>;
    fn inject_module_export(
        &mut self,
        module_name: &str,
        export: Export<State>,
    ) -> Result<(), EngineError>;
}

impl<State> ModuleLoaderState<State>
where
    State: Clone + Send + Sync + 'static,
{
    fn new(default_imports: Vec<String>) -> Self {
        Self {
            system: ModuleSystem::default(),
            injected_modules: BTreeSet::new(),
            module_exports_cache: BTreeMap::new(),
            module_interface_cache: BTreeMap::new(),
            module_sources: BTreeMap::new(),
            published_cycle_interfaces: BTreeSet::new(),
            default_imports,
            virtual_modules: BTreeMap::new(),
            module_local_type_names: BTreeMap::new(),
            registration_module_context: None,
        }
    }
}

impl Default for RuntimePolicy {
    fn default() -> Self {
        let execution_bounds = ExecutionBounds::default();
        Self {
            async_call_policy: AsyncCallPolicy::default(),
            execution_bounds,
            parallelism_controller: Arc::new(FixedParallelismController::new(execution_bounds)),
        }
    }
}

fn module_export_symbol(module_name: &str, export_name: &str) -> String {
    if module_name == ROOT_MODULE_NAME {
        normalize_name(export_name).to_string()
    } else {
        virtual_export_name(module_name, export_name)
    }
}

fn register_native_registration_parts<State>(
    module_loader: &ModuleLoaderState<State>,
    type_system: &mut TypeSystem,
    runtime: &mut RuntimeRegistry<State>,
    module_name: &str,
    export_name: &str,
    registration: NativeRegistration<State>,
) -> Result<(), EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let NativeRegistration {
        mut scheme,
        arity,
        callable,
    } = registration;
    let scheme_module = if module_name == ROOT_MODULE_NAME {
        module_loader
            .registration_module_context
            .as_deref()
            .unwrap_or(ROOT_MODULE_NAME)
    } else {
        module_name
    };
    if scheme_module != ROOT_MODULE_NAME
        && let Some(local_type_names) = module_loader.module_local_type_names.get(scheme_module)
    {
        scheme = qualify_module_scheme_refs(&scheme, scheme_module, local_type_names);
    }
    let name = normalize_name(&module_export_symbol(module_name, export_name));
    register_native_parts(type_system, runtime, name, scheme, arity, callable)
}

#[allow(clippy::too_many_arguments)]
fn register_owned_value_parts<State>(
    module_loader: &ModuleLoaderState<State>,
    type_system: &mut TypeSystem,
    runtime: &mut RuntimeRegistry<State>,
    heap: &mut Heap,
    module_name: &str,
    export_name: &str,
    mut typ: Type,
    value: Value,
) -> Result<(), EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let scheme_module = if module_name == ROOT_MODULE_NAME {
        module_loader
            .registration_module_context
            .as_deref()
            .unwrap_or(ROOT_MODULE_NAME)
    } else {
        module_name
    };
    if scheme_module != ROOT_MODULE_NAME
        && let Some(local_type_names) = module_loader.module_local_type_names.get(scheme_module)
    {
        typ = qualify_module_scheme_refs(
            &Scheme::new(vec![], vec![], typ),
            scheme_module,
            local_type_names,
        )
        .typ;
    }

    let name = normalize_name(&module_export_symbol(module_name, export_name));
    let root = heap.machine_root_scope(|scope| scope.alloc_value(value, &typ, type_system))?;
    let registration = NativeRegistration::constant(Scheme::new(vec![], vec![], typ), root);
    let NativeRegistration {
        scheme,
        arity,
        callable,
    } = registration;
    // Constants are resolved through the native registry so overloaded
    // constants such as the numeric prelude identities remain type-directed.
    register_native_parts(type_system, runtime, name, scheme, arity, callable)
}

pub(crate) fn install_named_rust_module<State, C>(
    engine: &mut C,
    expected_id: &ModuleId,
    module: Module<State>,
) -> Result<InstalledRustModule, EngineError>
where
    State: Clone + Send + Sync + 'static,
    C: RustModuleInstallContext<State>,
{
    let module_name = module.name.trim().to_string();
    if module_name.is_empty() {
        return Err(EngineError::Internal("module name cannot be empty".into()));
    }
    if module_name == ROOT_MODULE_NAME {
        return Err(EngineError::Internal(
            "importers cannot return the root module".into(),
        ));
    }

    let module_id = ModuleId::parse(&module_name)?;
    if &module_id != expected_id {
        return Err(EngineError::Internal(format!(
            "importer returned Rust module `{module_id}` for requested module `{expected_id}`"
        )));
    }

    let mut package = CompilationPackage {
        decls: module.declarations(),
        body: None,
        docs: module.docs.clone(),
    };
    package.decls.declare_fns.extend(
        module
            .internals
            .iter()
            .chain(&module.exports)
            .map(|export| export.interface.clone()),
    );
    let local_type_names = module_local_type_names_from_declarations(&package.decls);
    engine
        .module_loader_mut()
        .module_local_type_names
        .insert(module_name.clone(), local_type_names);

    let prefix = prefix_for_module(&module_id);
    let exports = exports_from_package(&package, &prefix, &module_id);
    let qualified = qualify_package(&package, &prefix);
    let interfaces = interface_decls_from_package(&qualified);
    engine
        .module_loader_mut()
        .module_exports_cache
        .insert(module_id.clone(), exports.clone());
    engine
        .module_loader_mut()
        .module_interface_cache
        .insert(module_id.clone(), interfaces);
    engine.module_loader_mut().virtual_modules.insert(
        module_name.clone(),
        VirtualModule {
            package: package.clone(),
        },
    );

    for internal in module.internals {
        engine.inject_module_export(&module_name, internal)?;
    }
    let (value_exports, callable_exports): (Vec<_>, Vec<_>) =
        module.exports.into_iter().partition(Export::is_value);
    for export in callable_exports {
        engine.inject_module_export(&module_name, export)?;
    }

    engine.inject_decls(&qualified.decls)?;
    for export in value_exports {
        engine.inject_module_export(&module_name, export)?;
    }
    engine
        .module_loader_mut()
        .injected_modules
        .insert(module_name);
    Ok(InstalledRustModule {
        id: module_id,
        exports,
        package,
    })
}

impl<State> Builder<State>
where
    State: Clone + Send + Sync + 'static,
{
    fn inject_module_export(
        &mut self,
        module_name: &str,
        export: Export<State>,
    ) -> Result<(), EngineError> {
        let Export {
            name,
            interface: _,
            payload,
            required_adts: _,
        } = export;
        let qualified_name = module_export_symbol(module_name, &name);
        let previous_context = self.module_loader.registration_module_context.clone();
        self.module_loader.registration_module_context = if module_name == ROOT_MODULE_NAME {
            None
        } else {
            Some(module_name.to_string())
        };
        let result = match payload {
            ExportPayload::Injector(injector) => injector(self, &qualified_name),
            ExportPayload::Value { value, typ } => register_owned_value_parts(
                &self.module_loader,
                &mut self.type_system,
                &mut self.runtime,
                &mut self.heap,
                ROOT_MODULE_NAME,
                &qualified_name,
                typ,
                value,
            ),
        };
        self.module_loader.registration_module_context = previous_context;
        result
    }

    pub(crate) fn register_native_registration(
        &mut self,
        module_name: &str,
        export_name: &str,
        registration: NativeRegistration<State>,
    ) -> Result<(), EngineError> {
        register_native_registration_parts(
            &self.module_loader,
            &mut self.type_system,
            &mut self.runtime,
            module_name,
            export_name,
            registration,
        )
    }

    pub(crate) fn export<Sig, H>(
        &mut self,
        name: impl Into<String>,
        handler: H,
    ) -> Result<(), EngineError>
    where
        H: InternalFnSync<State, Sig>,
    {
        let name = name.into();
        let symbol = normalize_name(&name);
        let (scheme, arity, callable) =
            handler.into_registration(Arc::clone(&self.state), symbol.clone());
        self.register_native_registration(
            ROOT_MODULE_NAME,
            &name,
            NativeRegistration::scheduler(scheme, arity, callable),
        )
    }

    pub(crate) fn export_native<F>(
        &mut self,
        name: impl Into<String>,
        scheme: Scheme,
        arity: usize,
        handler: F,
    ) -> Result<(), EngineError>
    where
        F: for<'a> Fn(
                &'a mut RootScope<'_>,
                &'a Type,
                &'a [RootedPtr],
            ) -> Result<RootedPtr, EngineError>
            + Send
            + Sync
            + 'static,
    {
        validate_native_export_scheme(&scheme, arity)?;
        let name = name.into();
        let handler = Arc::new(handler);
        let func: SchedulerNativeCallable = Arc::new(move |scope, typ, args| {
            handler(scope, &typ, args).map(SchedulerNativeResult::Ready)
        });
        let registration = NativeRegistration::scheduler(scheme, arity, func);
        self.register_native_registration(ROOT_MODULE_NAME, &name, registration)
    }

    pub(crate) fn export_native_scheduler<F>(
        &mut self,
        name: impl Into<String>,
        scheme: Scheme,
        arity: usize,
        handler: F,
    ) -> Result<(), EngineError>
    where
        F: for<'a, 'heap> Fn(
                &'a mut RootScope<'heap>,
                Type,
                Vec<RootedPtr>,
            ) -> Result<SchedulerNativeResult, EngineError>
            + Send
            + Sync
            + 'static,
    {
        validate_native_export_scheme(&scheme, arity)?;
        let name = name.into();
        let handler = Arc::new(handler);
        let func: SchedulerNativeCallable =
            Arc::new(move |scope, typ, args| handler(scope, typ, args.to_vec()));
        let registration = NativeRegistration::scheduler(scheme, arity, func);
        self.register_native_registration(ROOT_MODULE_NAME, &name, registration)
    }

    pub(crate) fn export_value<V: IntoRex + RexType>(
        &mut self,
        name: &str,
        value: V,
    ) -> Result<(), EngineError> {
        let typ = V::rex_type();
        let value = value.into_rex()?;
        register_owned_value_parts(
            &self.module_loader,
            &mut self.type_system,
            &mut self.runtime,
            &mut self.heap,
            ROOT_MODULE_NAME,
            name,
            typ,
            value,
        )
    }

    pub(crate) fn inject_adt(&mut self, adt: AdtDecl) -> Result<(), EngineError> {
        inject_adt_parts(&mut self.type_system, &mut self.runtime, adt)
    }

    pub(crate) fn inject_decls(&mut self, decls: &Declarations) -> Result<(), EngineError> {
        inject_decls_parts(
            &mut self.env,
            &mut self.type_system,
            &mut self.runtime,
            &mut self.heap,
            decls,
        )
    }

    pub(crate) fn register_typeclass_instance(
        &mut self,
        decl: &InstanceDecl,
        prepared: &PreparedInstanceDecl,
    ) -> Result<(), EngineError> {
        register_typeclass_instance_parts(
            &mut self.type_system,
            &self.env,
            &mut self.runtime,
            decl,
            prepared,
        )
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
        Ok(schemes[0].scheme.clone())
    }

    pub(crate) fn ensure_cycle_interfaces_published(
        &mut self,
        module_id: &ModuleId,
    ) -> Result<(), EngineError> {
        ensure_cycle_interfaces_published_parts(
            &mut self.module_loader,
            &mut self.env,
            &mut self.type_system,
            &mut self.runtime,
            &mut self.heap,
            module_id,
        )
    }
}

impl<State> ExportTarget<State> for Builder<State>
where
    State: Clone + Send + Sync + 'static,
{
    fn register_native_registration(
        &mut self,
        module_name: &str,
        export_name: &str,
        registration: NativeRegistration<State>,
    ) -> Result<(), EngineError> {
        Builder::register_native_registration(self, module_name, export_name, registration)
    }
}

impl<State> RustModuleInstallContext<State> for Builder<State>
where
    State: Clone + Send + Sync + 'static,
{
    fn module_loader(&self) -> &ModuleLoaderState<State> {
        &self.module_loader
    }

    fn module_loader_mut(&mut self) -> &mut ModuleLoaderState<State> {
        &mut self.module_loader
    }

    fn inject_decls(&mut self, decls: &Declarations) -> Result<(), EngineError> {
        Builder::inject_decls(self, decls)
    }

    fn inject_module_export(
        &mut self,
        module_name: &str,
        export: Export<State>,
    ) -> Result<(), EngineError> {
        Builder::inject_module_export(self, module_name, export)
    }
}

impl<State> Compiler<State>
where
    State: Clone + Send + Sync + 'static,
{
    fn inject_module_export(
        &mut self,
        module_name: &str,
        export: Export<State>,
    ) -> Result<(), EngineError> {
        let Export {
            name,
            interface: _,
            payload,
            required_adts: _,
        } = export;
        let qualified_name = module_export_symbol(module_name, &name);
        let previous_context = self.module_loader.registration_module_context.clone();
        self.module_loader.registration_module_context = if module_name == ROOT_MODULE_NAME {
            None
        } else {
            Some(module_name.to_string())
        };
        let result = match payload {
            ExportPayload::Injector(injector) => injector(self, &qualified_name),
            ExportPayload::Value { value, typ } => register_owned_value_parts(
                &self.module_loader,
                &mut self.type_system,
                &mut self.runtime,
                &mut self.heap,
                ROOT_MODULE_NAME,
                &qualified_name,
                typ,
                value,
            ),
        };
        self.module_loader.registration_module_context = previous_context;
        result
    }

    pub(crate) fn inject_decls(&mut self, decls: &Declarations) -> Result<(), EngineError> {
        inject_decls_parts(
            &mut self.env,
            &mut self.type_system,
            &mut self.runtime,
            &mut self.heap,
            decls,
        )
    }

    pub(crate) fn infer_type(
        &mut self,
        expr: &Expr,
    ) -> Result<(Vec<Predicate>, Type), EngineError> {
        infer(&mut self.type_system, expr).map_err(EngineError::Type)
    }

    pub(crate) fn ensure_cycle_interfaces_published(
        &mut self,
        module_id: &ModuleId,
    ) -> Result<(), EngineError> {
        ensure_cycle_interfaces_published_parts(
            &mut self.module_loader,
            &mut self.env,
            &mut self.type_system,
            &mut self.runtime,
            &mut self.heap,
            module_id,
        )
    }
}

impl<State> ExportTarget<State> for Compiler<State>
where
    State: Clone + Send + Sync + 'static,
{
    fn register_native_registration(
        &mut self,
        module_name: &str,
        export_name: &str,
        registration: NativeRegistration<State>,
    ) -> Result<(), EngineError> {
        register_native_registration_parts(
            &self.module_loader,
            &mut self.type_system,
            &mut self.runtime,
            module_name,
            export_name,
            registration,
        )
    }
}

impl<State> RustModuleInstallContext<State> for Compiler<State>
where
    State: Clone + Send + Sync + 'static,
{
    fn module_loader(&self) -> &ModuleLoaderState<State> {
        &self.module_loader
    }

    fn module_loader_mut(&mut self) -> &mut ModuleLoaderState<State> {
        &mut self.module_loader
    }

    fn inject_decls(&mut self, decls: &Declarations) -> Result<(), EngineError> {
        Compiler::inject_decls(self, decls)
    }

    fn inject_module_export(
        &mut self,
        module_name: &str,
        export: Export<State>,
    ) -> Result<(), EngineError> {
        Compiler::inject_module_export(self, module_name, export)
    }
}

#[derive(Clone)]
pub(crate) struct StaticModuleImporter<State: Clone + Send + Sync + 'static> {
    pub(crate) module_id: ModuleId,
    pub(crate) resolved: ResolvedModule<State>,
}

impl<State> Importer<State> for StaticModuleImporter<State>
where
    State: Clone + Send + Sync + 'static,
{
    fn import<'a>(
        &'a self,
        req: ImportRequest,
    ) -> BoxFuture<'a, Result<Option<ResolvedModule<State>>, EngineError>> {
        Box::pin(async move {
            if req.module_id != self.module_id {
                return Ok(None);
            }
            Ok(Some(self.resolved.clone()))
        })
    }
}

pub struct NativeRegistration<State: Clone + Send + Sync + 'static> {
    scheme: Scheme,
    arity: usize,
    callable: NativeCallable<State>,
}

impl<State: Clone + Send + Sync + 'static> NativeRegistration<State> {
    pub(crate) fn sync(scheme: Scheme, arity: usize, func: HostValueCallable<State>) -> Self {
        Self {
            scheme,
            arity,
            callable: NativeCallable::Host {
                callable: func,
                scheduling: NativeCallScheduling::Immediate,
            },
        }
    }

    pub(crate) fn scheduler(scheme: Scheme, arity: usize, func: SchedulerNativeCallable) -> Self {
        Self {
            scheme,
            arity,
            callable: NativeCallable::Scheduler(func),
        }
    }

    pub(crate) fn constant(scheme: Scheme, value: RootedPtr) -> Self {
        Self {
            scheme,
            arity: 0,
            callable: NativeCallable::Constant(value),
        }
    }

    pub(crate) fn r#async(scheme: Scheme, arity: usize, func: HostValueCallable<State>) -> Self {
        Self {
            scheme,
            arity,
            callable: NativeCallable::Host {
                callable: func,
                scheduling: NativeCallScheduling::Deferred,
            },
        }
    }
}

fn inject_adt_parts<State>(
    type_system: &mut TypeSystem,
    runtime: &mut RuntimeRegistry<State>,
    adt: AdtDecl,
) -> Result<(), EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    match type_system.adts.get(&adt.name) {
        Some(existing) if adt_shape_eq(existing, &adt) => {}
        Some(existing) => {
            return Err(EngineError::Custom(format!(
                "conflicting ADT registration for `{}`: existing={} new={}",
                adt.name,
                adt_shape(existing),
                adt_shape(&adt)
            )));
        }
        None => {}
    }
    type_system.register_adt(&adt)?;
    for ((ctor, scheme), variant) in adt.constructor_schemes().into_iter().zip(&adt.variants) {
        let aliases = [
            ctor,
            compatibility_constructor_name(&adt.name, &variant.name),
        ];
        for name in aliases {
            if runtime.natives.contains_scheme(&name, &scheme) {
                continue;
            }
            let ctor_name = name.clone();
            let func: SchedulerNativeCallable = Arc::new(move |scope, _typ, args| {
                let value = scope.alloc_root_adt(runtime_ctor_symbol(&ctor_name), args.to_vec())?;
                Ok(SchedulerNativeResult::Ready(value))
            });
            let arity = type_arity(&scheme.typ);
            register_native_parts(
                type_system,
                runtime,
                name,
                scheme.clone(),
                arity,
                NativeCallable::Scheduler(func),
            )?;
        }
    }
    Ok(())
}

fn inject_fn_runtime_parts<State>(
    env: &mut RootedEnvironment,
    type_system: &mut TypeSystem,
    runtime: &RuntimeRegistry<State>,
    heap: &mut Heap,
    decls: &[FnDecl],
) -> Result<(), EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    if decls.is_empty() {
        return Ok(());
    }

    let mut env_rec = env.clone();
    let mut slots = Vec::with_capacity(decls.len());
    for decl in decls {
        if let Some(existing) = env_rec.get(&decl.name.name) {
            slots.push(existing);
        } else {
            let placeholder = heap.machine_root_scope(|scope| {
                scope.alloc_root_uninitialized(decl.name.name.clone())
            })?;
            env_rec = env_rec.extend(decl.name.name.clone(), placeholder);
            slots.push(placeholder);
        }
    }

    let saved_env = env.clone();
    *env = env_rec.clone();

    let result: Result<(), EngineError> = (|| {
        for (decl, slot) in decls.iter().zip(slots.iter()) {
            let mut lam_body = match decl.body.as_ref() {
                Expr::Dict(..) | Expr::RecordUpdate(..) => Arc::new(Expr::Ann(
                    *decl.body.span(),
                    decl.body.clone(),
                    decl.ret.clone(),
                )),
                _ => decl.body.clone(),
            };
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

            let saved_type_vars = type_system.env.type_vars.clone();
            type_system.env.type_vars = decl
                .type_params
                .iter()
                .map(|param| {
                    let tv: TypeVar = type_system.supply.fresh(Some(param.clone()));
                    (param.clone(), tv)
                })
                .collect();
            let typed_result = type_check_expr_parts(env, type_system, runtime, lam_body.as_ref());
            type_system.env.type_vars = saved_type_vars;
            let typed = typed_result?;
            let (param_ty, _ret_ty) = split_fun(&typed.typ)
                .ok_or_else(|| EngineError::NotCallable(typed.typ.to_string()))?;
            let TypedExprKind::Lam { param, body } = typed.kind.as_ref() else {
                return Err(EngineError::Internal(
                    "fn declaration did not lower to lambda".into(),
                ));
            };
            heap.machine_root_scope(|scope| {
                let closure_env = env.to_scoped_environment();
                let value = scope.alloc_root_closure(
                    closure_env,
                    param.clone(),
                    param_ty,
                    typed.typ.clone(),
                    Arc::new(body.as_ref().clone()),
                )?;
                scope.overwrite_root(*slot, value)
            })?;
        }
        Ok(())
    })();

    if result.is_err() {
        *env = saved_env;
        return result;
    }

    *env = env_rec;
    Ok(())
}

fn inject_decls_parts<State>(
    env: &mut RootedEnvironment,
    type_system: &mut TypeSystem,
    runtime: &mut RuntimeRegistry<State>,
    heap: &mut Heap,
    decls: &Declarations,
) -> Result<(), EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let adts = type_system
        .register_type_decls(&decls.types)
        .map_err(EngineError::Type)?;
    for adt in adts {
        inject_adt_parts(type_system, runtime, adt)?;
    }

    type_system
        .register_class_decls(&decls.classes)
        .map_err(EngineError::Type)?;

    for decl in &decls.declare_fns {
        type_system
            .inject_declare_fn_decl(decl)
            .map_err(EngineError::Type)?;
    }

    let prepared_fns = type_system
        .register_fn_decl_signatures(&decls.fns)
        .map_err(EngineError::Type)?;

    let mut prepared_instances = Vec::with_capacity(decls.instances.len());
    for decl in &decls.instances {
        let prepared = type_system
            .register_instance_decl(decl)
            .map_err(EngineError::Type)?;
        prepared_instances.push((decl.clone(), prepared));
    }

    type_system
        .check_fn_decl_bodies(&prepared_fns)
        .map_err(EngineError::Type)?;
    inject_fn_runtime_parts(env, type_system, runtime, heap, &decls.fns)?;

    for (decl, prepared) in &prepared_instances {
        register_typeclass_instance_parts(type_system, env, runtime, decl, prepared)?;
    }

    Ok(())
}

fn publish_runtime_decl_interfaces_parts(
    env: &mut RootedEnvironment,
    heap: &mut Heap,
    decls: &[DeclareFnDecl],
) -> Result<(), EngineError> {
    for df in decls {
        if env.get(&df.name.name).is_some() {
            continue;
        }
        let placeholder =
            heap.machine_root_scope(|scope| scope.alloc_root_uninitialized(df.name.name.clone()))?;
        *env = env.extend(df.name.name.clone(), placeholder);
    }
    Ok(())
}

fn publish_runtime_interfaces_parts(
    env: &mut RootedEnvironment,
    heap: &mut Heap,
    decls: &Declarations,
) -> Result<(), EngineError> {
    publish_runtime_decl_interfaces_parts(env, heap, &decls.declare_fns)
}

fn register_native_parts<State>(
    type_system: &mut TypeSystem,
    runtime: &mut RuntimeRegistry<State>,
    name: Symbol,
    scheme: Scheme,
    arity: usize,
    func: NativeCallable<State>,
) -> Result<(), EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let expected = type_arity(&scheme.typ);
    if expected != arity {
        return Err(EngineError::NativeArity {
            name: name.clone(),
            expected,
            got: arity,
        });
    }
    register_type_scheme_parts(type_system, &name, &scheme)?;
    runtime.natives.insert(name, arity, scheme, func)
}

fn register_type_scheme_parts(
    type_system: &mut TypeSystem,
    name: &Symbol,
    injected: &Scheme,
) -> Result<(), EngineError> {
    let schemes = type_system.env.lookup(name);
    match schemes {
        None => {
            type_system.add_value(name.as_ref(), injected.clone());
            Ok(())
        }
        Some(schemes) => {
            let has_poly = schemes
                .iter()
                .any(|value| !value.scheme.vars.is_empty() || !value.scheme.preds.is_empty());
            if has_poly {
                for existing in schemes {
                    if scheme_accepts(type_system, &existing.scheme, &injected.typ)? {
                        return Ok(());
                    }
                }
                Err(EngineError::InvalidInjection {
                    name: name.clone(),
                    typ: injected.typ.to_string(),
                })
            } else {
                if schemes.iter().any(|s| &s.scheme == injected) {
                    return Ok(());
                }
                type_system.add_overload(name.as_ref(), injected.clone());
                Ok(())
            }
        }
    }
}

fn type_check_expr_parts<State>(
    env: &RootedEnvironment,
    type_system: &mut TypeSystem,
    runtime: &RuntimeRegistry<State>,
    expr: &Expr,
) -> Result<TypedExpr, EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    type_check_engine(type_system, env, &runtime.natives, expr)
}

fn register_typeclass_instance_parts<State>(
    type_system: &mut TypeSystem,
    env: &RootedEnvironment,
    runtime: &mut RuntimeRegistry<State>,
    decl: &InstanceDecl,
    prepared: &PreparedInstanceDecl,
) -> Result<(), EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let mut methods: BTreeMap<Symbol, Arc<TypedExpr>> = BTreeMap::new();
    for method in &decl.methods {
        let typed = type_system
            .typecheck_instance_method(prepared, method)
            .map_err(EngineError::Type)?;
        check_natives(type_system, env, &runtime.natives, &typed)?;
        methods.insert(method.name.clone(), Arc::new(typed));
    }

    let def_env = env.clone();
    runtime.typeclasses.insert(
        prepared.class.clone(),
        prepared.head.clone(),
        def_env,
        methods,
    )?;
    Ok(())
}

fn ensure_cycle_interfaces_published_parts<State>(
    module_loader: &mut ModuleLoaderState<State>,
    env: &mut RootedEnvironment,
    type_system: &mut TypeSystem,
    runtime: &mut RuntimeRegistry<State>,
    heap: &mut Heap,
    module_id: &ModuleId,
) -> Result<(), EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    if module_loader.published_cycle_interfaces.contains(module_id) {
        return Ok(());
    }
    let Some(decls) = module_loader.module_interface_cache.get(module_id).cloned() else {
        return Ok(());
    };
    inject_decls_parts(env, type_system, runtime, heap, &decls)?;
    publish_runtime_interfaces_parts(env, heap, &decls)?;
    module_loader
        .published_cycle_interfaces
        .insert(module_id.clone());
    Ok(())
}

fn module_local_type_names_from_declarations(decls: &Declarations) -> BTreeSet<Symbol> {
    decls.types.iter().map(|td| td.name.clone()).collect()
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

// TODO: Merge with type_head_and_args_for_adt_family in rex-typesystem
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

fn runtime_ctor_symbol(name: &Symbol) -> Symbol {
    Symbol::intern(name.as_ref().rsplit('.').next().unwrap_or(name.as_ref()))
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
                .map(|t| qualify_module_type_refs(t, module_name, local_type_names)),
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

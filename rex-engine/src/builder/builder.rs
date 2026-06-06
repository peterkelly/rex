use crate::{
    builder::{
        export::{Export, HostFnSync},
        qualify::qualify_program,
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
        context::Context,
        native_callable::{
            AsyncNativePointerCallable, NativeCallable, SchedulerNativeCallable,
            SchedulerNativeResult, SyncNativePointerCallable,
        },
    },
    handlers::RexDefault,
    modules::{
        ImportRequest, Importer, Module, ModuleExports, ModuleId, ModuleSystem, ROOT_MODULE_NAME,
        ResolvedModule, ResolvedModuleContent, StdlibImporter, VirtualModule,
        interface_decls_from_program, prefix_for_module, virtual_export_name,
    },
    prelude::{inject_prelude, inject_prelude_virtual_module, standard_type_system},
    util::{
        adt_family_error_to_engine, normalize_name, split_fun, type_arity,
        validate_native_export_scheme,
    },
    value::{Handle, Heap, IntoRex, Pointer},
};
use futures::future::BoxFuture;
use rex_ast::{CompilationUnit, Decl, DeclareFnDecl, Expr, FnDecl, InstanceDecl, Scope, Symbol};
use rex_typesystem::{
    inference::infer,
    types::{
        AdtDecl, Instance, Predicate, RexAdt, RexType, Scheme, Type, TypeKind, TypeVar, TypedExpr,
        TypedExprKind, Types, adt_shape, adt_shape_eq, order_adt_family,
    },
    typesystem::{PreparedInstanceDecl, TypeSystem, TypeVarSupply, entails, instantiate},
    unification::unify,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

pub struct Builder<State = ()>
where
    State: Clone + Send + Sync + 'static,
{
    pub(crate) state: Arc<State>,
    pub(crate) env: RootedEnvironment,
    pub(crate) type_system: TypeSystem,
    pub(crate) runtime: RuntimeRegistry<State>,
    pub(crate) module_loader: ModuleLoaderState,
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
                typeclass_cache: Arc::new(Mutex::new(BTreeMap::new())),
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
                typeclass_cache: Arc::new(Mutex::new(BTreeMap::new())),
            },
            module_loader: ModuleLoaderState::new(options.default_imports),
            policy: RuntimePolicy::default(),
            heap: Heap::new(),
        };
        if matches!(options.prelude, PreludeMode::Enabled) {
            builder
                .module_loader
                .system
                .append_importer("stdlib", Arc::new(StdlibImporter));
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
            for adt in &module.adts {
                self.inject_adt(adt.clone())?;
            }

            let staged_adt_names: BTreeSet<Symbol> =
                module.adts.iter().map(|adt| adt.name.clone()).collect();
            let decls = module
                .decls
                .iter()
                .filter(|decl| match decl {
                    Decl::Type(ty) => !staged_adt_names.contains(&ty.name),
                    _ => true,
                })
                .cloned()
                .collect::<Vec<_>>();

            for export in module.exports {
                self.inject_module_export(ROOT_MODULE_NAME, export)?;
            }
            self.inject_decls(&decls)?;
            return Ok(());
        }

        let module_id = ModuleId::parse(&module_name)?;

        let mut decls = module.decls.clone();
        decls.extend(
            module
                .exports
                .iter()
                .map(|export| Decl::DeclareFn(export.interface.clone())),
        );
        let local_type_names = module_local_type_names_from_decls(&decls);
        self.module_loader
            .module_local_type_names
            .insert(module_name.clone(), local_type_names);

        let compilation_unit = CompilationUnit { decls, body: None };
        let prefix = prefix_for_module(&module_id);
        let exports = crate::modules::exports_from_program(&compilation_unit, &prefix, &module_id);
        let qualified = qualify_program(&compilation_unit, &prefix);
        let interfaces = interface_decls_from_program(&qualified);
        self.module_loader
            .module_exports_cache
            .insert(module_id.clone(), exports.clone());
        self.module_loader
            .module_interface_cache
            .insert(module_id.clone(), interfaces.clone());
        self.module_loader.virtual_modules.insert(
            module_name.clone(),
            VirtualModule {
                exports,
                decls: compilation_unit.decls.clone(),
                source: None,
            },
        );

        for export in module.exports {
            self.inject_module_export(&module_name, export)?;
        }

        self.inject_decls(&qualified.decls)?;
        self.module_loader
            .system
            .prepend_importer(Arc::new(StaticModuleImporter {
                module_id: module_id.clone(),
                resolved: ResolvedModule {
                    id: module_id,
                    content: ResolvedModuleContent::CompilationUnit(compilation_unit.clone()),
                },
            }));

        self.module_loader.injected_modules.insert(module_name);
        Ok(())
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
        self.export_native(
            native_name.clone(),
            native_scheme,
            0,
            move |engine, _, _| T::rex_default(engine),
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

        let def_env = self.env.clone();
        self.runtime
            .typeclasses
            .insert(class, head_ty, def_env, methods)?;

        Ok(())
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

    pub fn add_importer(&mut self, name: impl Into<String>, importer: Arc<dyn Importer>) {
        self.module_loader.system.append_importer(name, importer);
    }

    pub fn type_system(&self) -> &TypeSystem {
        &self.type_system
    }

    pub fn type_system_mut(&mut self) -> &mut TypeSystem {
        &mut self.type_system
    }

    pub fn heap(&self) -> &Heap {
        &self.heap
    }
}

pub(crate) struct RuntimeRegistry<State>
where
    State: Clone + Send + Sync + 'static,
{
    pub(crate) natives: NativeRegistry<State>,
    pub(crate) typeclasses: TypeclassRegistry,
    pub(crate) typeclass_cache: Arc<Mutex<BTreeMap<(Symbol, Type), Pointer>>>,
}

pub(crate) struct ModuleLoaderState {
    pub(crate) system: ModuleSystem,
    pub(crate) injected_modules: BTreeSet<String>,
    pub(crate) module_exports_cache: BTreeMap<ModuleId, ModuleExports>,
    pub(crate) module_interface_cache: BTreeMap<ModuleId, Vec<Decl>>,
    pub(crate) module_sources: BTreeMap<ModuleId, String>,
    pub(crate) module_source_fingerprints: BTreeMap<ModuleId, String>,
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

impl ModuleLoaderState {
    fn new(default_imports: Vec<String>) -> Self {
        Self {
            system: ModuleSystem::default(),
            injected_modules: BTreeSet::new(),
            module_exports_cache: BTreeMap::new(),
            module_interface_cache: BTreeMap::new(),
            module_sources: BTreeMap::new(),
            module_source_fingerprints: BTreeMap::new(),
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

impl<State> Builder<State>
where
    State: Clone + Send + Sync + 'static,
{
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
        let previous_context = self.module_loader.registration_module_context.clone();
        self.module_loader.registration_module_context = if module_name == ROOT_MODULE_NAME {
            None
        } else {
            Some(module_name.to_string())
        };
        let result = injector(self, &qualified_name);
        self.module_loader.registration_module_context = previous_context;
        result
    }

    pub(crate) fn inject_root_export(&mut self, export: Export<State>) -> Result<(), EngineError> {
        self.inject_module_export(ROOT_MODULE_NAME, export)
    }

    pub(crate) fn register_native_registration(
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
            self.module_loader
                .registration_module_context
                .as_deref()
                .unwrap_or(ROOT_MODULE_NAME)
        } else {
            module_name
        };
        if scheme_module != ROOT_MODULE_NAME
            && let Some(local_type_names) = self
                .module_loader
                .module_local_type_names
                .get(scheme_module)
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
        F: for<'a> Fn(Context<State>, &'a Type, &'a [Handle]) -> Result<Handle, EngineError>
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

    pub(crate) fn export_native_scheduler<F>(
        &mut self,
        name: impl Into<String>,
        scheme: Scheme,
        arity: usize,
        handler: F,
    ) -> Result<(), EngineError>
    where
        F: for<'a> Fn(
                Context<State>,
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

    pub(crate) fn inject_adt(&mut self, adt: AdtDecl) -> Result<(), EngineError> {
        inject_adt_parts(&mut self.type_system, &mut self.runtime, adt)
    }

    pub(crate) fn inject_decls(&mut self, decls: &[Decl]) -> Result<(), EngineError> {
        inject_decls_parts(
            &mut self.env,
            &mut self.type_system,
            &mut self.runtime,
            &self.heap,
            decls,
        )
    }

    fn register_native(
        &mut self,
        name: Symbol,
        scheme: Scheme,
        arity: usize,
        func: NativeCallable<State>,
    ) -> Result<(), EngineError> {
        register_native_parts(
            &mut self.type_system,
            &mut self.runtime,
            name,
            scheme,
            arity,
            func,
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
        Ok(schemes[0].clone())
    }

    pub(crate) fn refresh_if_stale(
        &mut self,
        resolved: &ResolvedModule,
    ) -> Result<Option<String>, EngineError> {
        let Some(next) = resolved.content_fingerprint() else {
            return Ok(None);
        };
        if let Some(prev) = self
            .module_loader
            .module_source_fingerprints
            .get(&resolved.id)
            && prev != &next
        {
            invalidate_module_caches(&mut self.module_loader, &mut self.type_system, &resolved.id);
        }
        Ok(Some(next))
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
            &self.heap,
            module_id,
        )
    }
}

impl<State> Compiler<State>
where
    State: Clone + Send + Sync + 'static,
{
    pub(crate) fn inject_decls(&mut self, decls: &[Decl]) -> Result<(), EngineError> {
        inject_decls_parts(
            &mut self.env,
            &mut self.type_system,
            &mut self.runtime,
            &self.heap,
            decls,
        )
    }

    pub(crate) fn infer_type(
        &mut self,
        expr: &Expr,
    ) -> Result<(Vec<Predicate>, Type), EngineError> {
        infer(&mut self.type_system, expr).map_err(EngineError::Type)
    }

    pub(crate) fn refresh_if_stale(
        &mut self,
        resolved: &ResolvedModule,
    ) -> Result<Option<String>, EngineError> {
        let Some(next) = resolved.content_fingerprint() else {
            return Ok(None);
        };
        if let Some(prev) = self
            .module_loader
            .module_source_fingerprints
            .get(&resolved.id)
            && prev != &next
        {
            invalidate_module_caches(&mut self.module_loader, &mut self.type_system, &resolved.id);
        }
        Ok(Some(next))
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
            &self.heap,
            module_id,
        )
    }
}

#[derive(Clone)]
pub(crate) struct StaticModuleImporter {
    pub(crate) module_id: ModuleId,
    pub(crate) resolved: ResolvedModule,
}

impl Importer for StaticModuleImporter {
    fn import<'a>(
        &'a self,
        req: ImportRequest,
    ) -> BoxFuture<'a, Result<Option<ResolvedModule>, EngineError>> {
        Box::pin(async move {
            if req.module_id != self.module_id {
                return Ok(None);
            }
            Ok(Some(self.resolved.clone()))
        })
    }
}

pub(crate) struct NativeRegistration<State: Clone + Send + Sync + 'static> {
    scheme: Scheme,
    arity: usize,
    callable: NativeCallable<State>,
}

impl<State: Clone + Send + Sync + 'static> NativeRegistration<State> {
    pub(crate) fn sync(
        scheme: Scheme,
        arity: usize,
        func: SyncNativePointerCallable<State>,
    ) -> Self {
        Self {
            scheme,
            arity,
            callable: NativeCallable::Sync(func),
        }
    }

    pub(crate) fn scheduler(
        scheme: Scheme,
        arity: usize,
        func: SchedulerNativeCallable<State>,
    ) -> Self {
        Self {
            scheme,
            arity,
            callable: NativeCallable::Scheduler(func),
        }
    }

    pub(crate) fn r#async(
        scheme: Scheme,
        arity: usize,
        func: AsyncNativePointerCallable<State>,
    ) -> Self {
        Self {
            scheme,
            arity,
            callable: NativeCallable::Async(func),
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
    let register_type = match type_system.adts.get(&adt.name) {
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

    if register_type {
        type_system.register_adt(&adt);
    }
    for (ctor, scheme) in adt.constructor_schemes() {
        if runtime
            .natives
            .get(&ctor)
            .is_some_and(|existing| existing.iter().any(|imp| imp.scheme == scheme))
        {
            continue;
        }
        let ctor_name = ctor.clone();
        let func: SyncNativePointerCallable<State> =
            Arc::new(move |ctx: Context<State>, _: &Type, args: &[Pointer]| {
                ctx.heap()
                    .alloc_ptr_adt(runtime_ctor_symbol(&ctor_name), args.to_vec())
            });
        let arity = type_arity(&scheme.typ);
        register_native_parts(
            type_system,
            runtime,
            ctor,
            scheme,
            arity,
            NativeCallable::Sync(func),
        )?;
    }
    Ok(())
}

fn inject_fn_decls_parts<State>(
    env: &mut RootedEnvironment,
    type_system: &mut TypeSystem,
    runtime: &RuntimeRegistry<State>,
    heap: &Heap,
    decls: &[FnDecl],
) -> Result<(), EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    if decls.is_empty() {
        return Ok(());
    }

    type_system
        .register_fn_decls(decls)
        .map_err(EngineError::Type)?;

    let mut env_rec = env.clone();
    let mut slots = Vec::with_capacity(decls.len());
    for decl in decls {
        if let Some(existing) = env_rec.get(&decl.name.name) {
            slots.push(existing);
        } else {
            let placeholder = heap.alloc_ptr_uninitialized(decl.name.name.clone())?;
            let placeholder = heap.handle(placeholder)?;
            env_rec = env_rec.extend(decl.name.name.clone(), placeholder.clone());
            slots.push(placeholder);
        }
    }

    let saved_env = env.clone();
    *env = env_rec.clone();

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
            let closure_env = env.to_environment()?;
            let ptr = heap.alloc_ptr_closure(
                closure_env,
                param.clone(),
                param_ty,
                typed.typ.clone(),
                Arc::new(body.as_ref().clone()),
            )?;
            let value = heap.clone_cell(&ptr)?;
            let slot = slot.pointer()?;
            heap.overwrite(&slot, value)?;
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
    heap: &Heap,
    decls: &[Decl],
) -> Result<(), EngineError>
where
    State: Clone + Send + Sync + 'static,
{
    let mut pending_fns: Vec<FnDecl> = Vec::new();
    for decl in decls {
        if let Decl::Fn(fd) = decl {
            pending_fns.push(fd.clone());
            continue;
        }
        if !pending_fns.is_empty() {
            inject_fn_decls_parts(env, type_system, runtime, heap, &pending_fns)?;
            pending_fns.clear();
        }

        match decl {
            Decl::Type(ty) => {
                let adt = type_system.adt_from_decl(ty).map_err(EngineError::Type)?;
                inject_adt_parts(type_system, runtime, adt)?;
            }
            Decl::Class(class_decl) => type_system
                .register_class_decl(class_decl)
                .map_err(EngineError::Type)?,
            Decl::Instance(inst_decl) => {
                let prepared = type_system
                    .register_instance_decl(inst_decl)
                    .map_err(EngineError::Type)?;
                register_typeclass_instance_parts(type_system, env, runtime, inst_decl, &prepared)?;
            }
            Decl::Fn(..) => {}
            Decl::DeclareFn(df) => {
                type_system
                    .inject_declare_fn_decl(df)
                    .map_err(EngineError::Type)?;
            }
            Decl::Import(..) => {}
        }
    }
    if !pending_fns.is_empty() {
        inject_fn_decls_parts(env, type_system, runtime, heap, &pending_fns)?;
    }
    Ok(())
}

fn publish_runtime_decl_interfaces_parts(
    env: &mut RootedEnvironment,
    heap: &Heap,
    decls: &[DeclareFnDecl],
) -> Result<(), EngineError> {
    for df in decls {
        if env.get(&df.name.name).is_some() {
            continue;
        }
        let placeholder = heap.alloc_ptr_uninitialized(df.name.name.clone())?;
        let placeholder = heap.handle(placeholder)?;
        *env = env.extend(df.name.name.clone(), placeholder);
    }
    Ok(())
}

fn publish_runtime_interfaces_parts(
    env: &mut RootedEnvironment,
    heap: &Heap,
    decls: &[Decl],
) -> Result<(), EngineError> {
    let mut signatures = Vec::new();
    for decl in decls {
        let Decl::DeclareFn(df) = decl else {
            continue;
        };
        signatures.push(df.clone());
    }
    publish_runtime_decl_interfaces_parts(env, heap, &signatures)
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
                .any(|s| !s.vars.is_empty() || !s.preds.is_empty());
            if has_poly {
                for existing in schemes {
                    if scheme_accepts(type_system, existing, &injected.typ)? {
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
    module_loader: &mut ModuleLoaderState,
    env: &mut RootedEnvironment,
    type_system: &mut TypeSystem,
    runtime: &mut RuntimeRegistry<State>,
    heap: &Heap,
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

fn module_local_type_names_from_decls(decls: &[Decl]) -> BTreeSet<Symbol> {
    let mut out = BTreeSet::new();
    for decl in decls {
        if let Decl::Type(td) = decl {
            out.insert(td.name.clone());
        }
    }
    out
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

fn sanitize_type_name_for_symbol(typ: &Type) -> String {
    typ.to_string()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect()
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

fn invalidate_module_caches(
    module_loader: &mut ModuleLoaderState,
    type_system: &mut TypeSystem,
    id: &ModuleId,
) {
    if let Some(prev_interface) = module_loader.module_interface_cache.get(id).cloned() {
        remove_type_level_symbols_for_module_interface(type_system, &prev_interface);
    }
    module_loader.module_exports_cache.remove(id);
    module_loader.module_interface_cache.remove(id);
    module_loader.module_sources.remove(id);
    module_loader.module_source_fingerprints.remove(id);
    module_loader.published_cycle_interfaces.remove(id);
}

fn remove_type_level_symbols_for_module_interface(type_system: &mut TypeSystem, decls: &[Decl]) {
    for decl in decls {
        match decl {
            Decl::Fn(fd) => {
                type_system.env.remove(&fd.name.name);
                type_system.declared_values.remove(&fd.name.name);
            }
            Decl::DeclareFn(df) => {
                type_system.env.remove(&df.name.name);
                type_system.declared_values.remove(&df.name.name);
            }
            Decl::Type(td) => {
                type_system.adts.remove(&td.name);
                for variant in &td.variants {
                    type_system.env.remove(&variant.name);
                    type_system.declared_values.remove(&variant.name);
                }
            }
            Decl::Class(cd) => {
                type_system.classes.classes.remove(&cd.name);
                type_system.classes.instances.remove(&cd.name);
                type_system.class_info.remove(&cd.name);
                for method in &cd.methods {
                    type_system.env.remove(&method.name);
                    type_system.class_methods.remove(&method.name);
                }
            }
            Decl::Import(..) | Decl::Instance(..) => {}
        }
    }
}

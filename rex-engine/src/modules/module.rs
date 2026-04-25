use rex_ast::expr::{Decl, NameRef, TypeDecl, TypeExpr, TypeVariant, intern, sym};
use rex_lexer::span::Span;
use rex_typesystem::{
    types::collect_adts_in_types,
    types::{AdtDecl, BuiltinTypeId, Predicate, Scheme, Type, TypeKind, TypeVar},
};

use crate::EvaluatorRef;
use crate::engine::{
    AsyncHandler, Export, Handler, NativeFuture, SchedulerNativeResult, order_adt_family,
};
use crate::stack::{NativeLogShow, NativeTask};
use crate::{
    CancellationToken, Engine, EngineError, IntoPointer, Pointer, ROOT_MODULE_NAME, RexType, Value,
};

/// A staged host module that you build up in Rust and later inject into an [`Engine`].
///
/// `Module` is the host-side representation of a Rex module. It lets embedders collect:
///
/// - Rex declarations such as `pub type ...`
/// - typed Rust handlers via [`Module::export`] / [`Module::export_async`]
/// - pointer-level native handlers via [`Module::export_native`] /
///   [`Module::export_native_async`]
///
/// Once the module is assembled, pass it to [`Engine::inject_module`] to make it importable
/// from Rex code.
///
/// This type is intentionally mutable and staged: you can build it incrementally, inspect its
/// raw and structured declarations plus [`Module::exports`], transform them, and only inject it
/// once you are satisfied with the final module shape.
///
/// # Examples
///
/// ```rust,ignore
/// use rex_engine::{Engine, Module};
///
/// let mut engine = Engine::with_prelude(()).unwrap();
/// engine.add_default_resolvers();
///
/// let mut math = Module::new("acme.math");
/// math.export("inc", |_state: &(), x: i32| Ok(x + 1)).unwrap();
/// math.add_raw_declaration("pub type Sign = Positive | Negative").unwrap();
///
/// engine.inject_module(math).unwrap();
/// ```
pub struct Module<State: Clone + Send + Sync + 'static> {
    /// The module name Rex code will import.
    ///
    /// This should be the fully-qualified module path you want users to write in `import`
    /// declarations, such as `acme.math` or `sample`.
    ///
    /// [`Engine::inject_module`] validates and reserves this name when the module is injected.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use rex_engine::Module;
    ///
    /// let module = Module::<()>::new("acme.math");
    /// assert_eq!(module.name, "acme.math");
    /// ```
    pub name: String,

    /// Raw Rex declarations supplied directly by the embedder.
    ///
    /// This is most commonly used for `pub type ...` declarations, but it can hold any raw Rex
    /// declaration text you want included in the virtual module source.
    ///
    /// The usual way to append to this field is [`Module::add_raw_declaration`], which validates that
    /// the added text is non-empty. The field itself is public so callers can inspect or construct
    /// a module in multiple passes.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use rex_engine::Module;
    ///
    /// let mut module = Module::<()>::new("acme.status");
    /// module
    ///     .add_raw_declaration("pub type Status = Ready | Failed string")
    ///     .unwrap();
    ///
    /// assert_eq!(module.raw_declarations.len(), 1);
    /// ```
    pub raw_declarations: Vec<String>,

    /// Structured declarations registered from Rust metadata rather than Rex source.
    ///
    /// APIs such as [`Module::add_adt_decl`] and [`Module::add_rex_adt`] append here instead
    /// of synthesizing Rex source text.
    pub structured_decls: Vec<Decl>,

    pub(crate) staged_adts: Vec<AdtDecl>,

    /// Staged host exports that will become callable Rex values when the module is injected.
    ///
    /// Each [`Export`] bundles a public Rex name, a declaration that is inserted into the virtual
    /// module source, and the runtime injector that registers the implementation with the engine.
    ///
    /// Most callers populate this with [`Module::export`], [`Module::export_async`],
    /// [`Module::export_native`], [`Module::export_native_async`], or [`Module::add_export`].
    /// The field is public so advanced embedders can construct exports separately and assemble the
    /// final module programmatically.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use rex_engine::{Export, Module};
    ///
    /// let mut module = Module::<()>::new("acme.math");
    /// let export = Export::from_handler("inc", |_state: &(), x: i32| Ok(x + 1)).unwrap();
    /// module.exports.push(export);
    ///
    /// assert_eq!(module.exports.len(), 1);
    /// ```
    pub exports: Vec<Export<State>>,
}

impl<State> Module<State>
where
    State: Clone + Send + Sync + 'static,
{
    fn tracing_log_scheme() -> Scheme {
        let a_tv = TypeVar::new(0, Some(sym("a")));
        let a = Type::var(a_tv.clone());
        Scheme::new(
            vec![a_tv],
            vec![Predicate::new("Show", a.clone())],
            Type::fun(a, Type::builtin(BuiltinTypeId::String)),
        )
    }

    /// Create an empty staged module that targets the engine root namespace.
    ///
    /// Injecting a global module installs its declarations and exports directly
    /// into the engine rather than making them importable as a named module.
    pub fn global() -> Self {
        Self::new(ROOT_MODULE_NAME)
    }

    /// Create an empty staged module with the given import name.
    ///
    /// The returned module contains no declarations and no exports yet. Add those with the
    /// helper methods on `Module`, then pass it to [`Engine::inject_module`].
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use rex_engine::Module;
    ///
    /// let module = Module::<()>::new("acme.math");
    /// assert_eq!(module.name, "acme.math");
    /// assert!(module.raw_declarations.is_empty());
    /// assert!(module.exports.is_empty());
    /// ```
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            raw_declarations: Vec::new(),
            structured_decls: Vec::new(),
            staged_adts: Vec::new(),
            exports: Vec::new(),
        }
    }

    /// Append a raw Rex declaration to this staged module.
    ///
    /// Use this when you already have declaration text in Rex syntax, for example `pub type ...`.
    /// The text is stored exactly as provided and later concatenated into the virtual module source
    /// that [`Engine::inject_module`] exposes to Rex imports.
    ///
    /// This rejects empty or whitespace-only strings.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use rex_engine::Module;
    ///
    /// let mut module = Module::<()>::new("acme.status");
    /// module
    ///     .add_raw_declaration("pub type Status = Ready | Failed string")
    ///     .unwrap();
    /// ```
    pub fn add_raw_declaration(
        &mut self,
        declaration: impl Into<String>,
    ) -> Result<(), EngineError> {
        let declaration = declaration.into();
        if declaration.trim().is_empty() {
            return Err(EngineError::Internal(
                "module declaration cannot be empty".into(),
            ));
        }
        self.raw_declarations.push(declaration);
        Ok(())
    }

    /// Append a structured Rex declaration to this staged module.
    pub fn add_decl(&mut self, decl: Decl) {
        self.structured_decls.push(decl);
    }

    /// Append multiple structured Rex declarations to this staged module.
    pub fn add_decls(&mut self, decls: impl IntoIterator<Item = Decl>) {
        self.structured_decls.extend(decls);
    }

    /// Convert an [`AdtDecl`] into a structured type declaration and append it to this module.
    ///
    /// This is a structured alternative to [`Module::add_raw_declaration`] when you already have
    /// an ADT declaration in typed form.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use rex_engine::{Engine, Module};
    ///
    /// let mut engine = Engine::with_prelude(()).unwrap();
    /// let mut module = Module::new("acme.types");
    /// let adt = engine.adt_decl_from_type(&rex_typesystem::Type::user_con("Thing", 0)).unwrap();
    ///
    /// module.add_adt_decl(adt).unwrap();
    /// ```
    pub fn add_adt_decl(&mut self, adt: AdtDecl) -> Result<(), EngineError> {
        self.add_adt_family(vec![adt])
    }

    /// Append an acyclic family of ADT declarations to this staged module.
    ///
    /// Families are ordered before insertion so declarations are staged in
    /// dependency order, and cycles are rejected.
    pub fn add_adt_family(&mut self, adts: Vec<AdtDecl>) -> Result<(), EngineError> {
        for adt in order_adt_family(adts)? {
            let candidate = type_decl_from_adt(&adt);
            let already_staged = self.structured_decls.iter().find_map(|decl| match decl {
                Decl::Type(type_decl) if type_decl.name == adt.name => Some(type_decl),
                _ => None,
            });
            if let Some(existing_decl) = already_staged {
                if existing_decl != &candidate {
                    return Err(EngineError::Custom(format!(
                        "conflicting staged ADT registration for `{}`: existing declaration differs from new ADT declaration",
                        adt.name,
                    )));
                }
                continue;
            }
            self.staged_adts.push(adt);
            self.structured_decls.push(Decl::Type(candidate));
        }
        Ok(())
    }

    /// Discover user ADTs referenced by the supplied types and append their declarations.
    ///
    /// This is useful when you have Rust-side type information and want to register the
    /// corresponding user-defined ADTs for every type it mentions.
    ///
    /// The discovery process:
    ///
    /// - walks the provided types recursively
    /// - deduplicates repeated ADTs
    /// - asks the engine to materialize each discovered ADT declaration
    /// - appends the resulting structured declarations to this module
    ///
    /// If conflicting ADT definitions are found for the same type constructor name, this returns
    /// an [`EngineError`] that describes the conflict instead of silently picking one.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use rex_engine::{Engine, Module};
    /// use rex_typesystem::{BuiltinTypeId, Type};
    ///
    /// let mut engine = Engine::with_prelude(()).unwrap();
    /// let mut module = Module::new("acme.types");
    /// let types = vec![
    ///     Type::app(Type::user_con("Foo", 1), Type::builtin(BuiltinTypeId::I32)),
    ///     Type::user_con("Bar", 0),
    /// ];
    ///
    /// module.add_adt_decls_from_types(&mut engine, types).unwrap();
    /// ```
    pub fn add_adt_decls_from_types(
        &mut self,
        engine: &mut Engine<State>,
        types: Vec<Type>,
    ) -> Result<(), EngineError> {
        let adts = collect_adts_in_types(types).map_err(crate::collect_adts_error_to_engine)?;
        for typ in adts {
            let adt = engine.adt_decl_from_type(&typ)?;
            self.add_adt_decl(adt)?;
        }
        Ok(())
    }

    /// Derive a Rex ADT declaration from a Rust type and append it to this module.
    ///
    /// This is the most ergonomic way to expose a Rust enum or struct that implements [`RexType`]
    /// as a module-local structured Rex type declaration.
    ///
    /// Unlike older engine-level registration helpers, this stages the declaration
    /// inside the module so the caller can choose whether to inject it globally or
    /// as a named module.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use rex_engine::{Engine, Module};
    ///
    /// #[derive(rex::Rex)]
    /// struct Label {
    ///     text: String,
    /// }
    ///
    /// let mut engine = Engine::with_prelude(()).unwrap();
    /// let mut module = Module::new("sample");
    /// module.add_rex_adt::<Label>().unwrap();
    /// ```
    pub fn add_rex_adt<T>(&mut self) -> Result<(), EngineError>
    where
        T: RexType,
    {
        let mut family = Vec::new();
        T::collect_rex_family(&mut family)?;
        self.add_adt_family(family)
    }

    /// Append a preconstructed [`Export`] to this module.
    ///
    /// This is useful when exports are assembled elsewhere, such as from plugin metadata or a
    /// higher-level registration layer.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use rex_engine::{Export, Module};
    ///
    /// let mut module = Module::<()>::new("acme.math");
    /// let export = Export::from_handler("inc", |_state: &(), x: i32| Ok(x + 1)).unwrap();
    /// module.add_export(export);
    /// ```
    pub fn add_export(&mut self, export: Export<State>) {
        self.exports.push(export);
    }

    /// Stage a typed synchronous Rust handler as a module export.
    ///
    /// This is the most convenient API for exporting ordinary Rust functions or closures into a
    /// module. The handler's argument and return types drive the Rex signature automatically.
    ///
    /// The staged export becomes available to Rex code after [`Engine::inject_module`] is called.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use rex_engine::Module;
    ///
    /// let mut module = Module::<()>::new("acme.math");
    /// module.export("inc", |_state: &(), x: i32| Ok(x + 1)).unwrap();
    /// ```
    pub fn export<Sig, H>(&mut self, name: impl Into<String>, handler: H) -> Result<(), EngineError>
    where
        H: Handler<State, Sig>,
    {
        self.exports.push(Export::from_handler(name, handler)?);
        Ok(())
    }

    /// Stage a typed asynchronous Rust handler as a module export.
    ///
    /// Use this when the host implementation is naturally async, for example when it awaits I/O or
    /// other long-running work.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use rex_engine::Module;
    ///
    /// let mut module = Module::<()>::new("acme.math");
    /// module
    ///     .export_async("double_async", |_state: &(), x: i32| async move { Ok(x * 2) })
    ///     .unwrap();
    /// ```
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

    /// Stage a tracing-backed log export with type `a -> str where Show a`.
    pub fn export_tracing_log_function(
        &mut self,
        name: impl Into<String>,
        log: fn(&str),
    ) -> Result<(), EngineError> {
        let name = name.into();
        let name_sym = sym(&name);
        let scheme = Self::tracing_log_scheme();
        self.exports.push(Export::from_native_scheduler(
            name,
            scheme,
            1,
            move |engine, call_type, args| {
                let name_sym = name_sym.clone();
                if args.len() != 1 {
                    return Err(EngineError::NativeArity {
                        name: name_sym.clone(),
                        expected: 1,
                        got: args.len(),
                    });
                }

                let (arg_ty, _ret_ty) = match call_type.as_ref() {
                    TypeKind::Fun(arg, ret) => (arg.clone(), ret.clone()),
                    _ => return Err(EngineError::NotCallable(call_type.to_string())),
                };
                let show_ty = Type::fun(arg_ty.clone(), Type::builtin(BuiltinTypeId::String));
                let _ = engine;
                Ok(SchedulerNativeResult::Task(NativeTask::LogShow(
                    NativeLogShow {
                        show_type: show_ty,
                        arg_type: arg_ty,
                        arg: args[0],
                        log,
                    },
                )))
            },
        )?);
        Ok(())
    }

    /// Stage the standard `debug`/`info`/`warn`/`error` tracing exports.
    pub fn export_tracing_log_functions(&mut self) -> Result<(), EngineError> {
        self.export_tracing_log_function("debug", |s| tracing::debug!("{s}"))?;
        self.export_tracing_log_function("info", |s| tracing::info!("{s}"))?;
        self.export_tracing_log_function("warn", |s| tracing::warn!("{s}"))?;
        self.export_tracing_log_function("error", |s| tracing::error!("{s}"))?;
        Ok(())
    }

    /// Stage a pointer-level synchronous native export with an explicit Rex type scheme.
    ///
    /// This lower-level API is intended for dynamic or runtime-defined integrations where the
    /// handler needs access to the engine heap or where the Rex type cannot be inferred from an
    /// ordinary Rust function signature alone.
    ///
    /// `scheme` describes the Rex-visible type, and `arity` must match the number of arguments the
    /// handler expects.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use rex_engine::{EvaluatorRef, Module, Pointer};
    /// use rex_typesystem::{BuiltinTypeId, Scheme, Type};
    ///
    /// let mut module = Module::<()>::new("acme.dynamic");
    /// let scheme = Scheme::new(
    ///     vec![],
    ///     vec![],
    ///     Type::fun(Type::builtin(BuiltinTypeId::I32), Type::builtin(BuiltinTypeId::I32)),
    /// );
    ///
    /// module
    ///     .export_native("id_ptr", scheme, 1, |_engine: EvaluatorRef<'_, ()>, _typ: &Type, args: &[Pointer]| {
    ///         Ok(args[0].clone())
    ///     })
    ///     .unwrap();
    /// ```
    pub fn export_native<F>(
        &mut self,
        name: impl Into<String>,
        scheme: Scheme,
        arity: usize,
        handler: F,
    ) -> Result<(), EngineError>
    where
        F: for<'a> Fn(
                EvaluatorRef<'a, State>,
                &'a Type,
                &'a [Pointer],
            ) -> Result<Pointer, EngineError>
            + Send
            + Sync
            + 'static,
    {
        self.exports
            .push(Export::from_native(name, scheme, arity, handler)?);
        Ok(())
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
        F: for<'a> Fn(
                EvaluatorRef<'a, State>,
                &'a Type,
                &'a [Pointer],
            ) -> Result<Pointer, EngineError>
            + Send
            + Sync
            + 'static,
    {
        self.exports.push(Export::from_native_with_gas_cost(
            name, scheme, arity, gas_cost, handler,
        )?);
        Ok(())
    }

    /// Stage a pointer-level asynchronous native export with an explicit Rex type scheme.
    ///
    /// This is the async counterpart to [`Module::export_native`]. Use it when the export needs
    /// both direct engine access and asynchronous execution.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use futures::FutureExt;
    /// use rex_engine::{EvaluatorRef, Module, Pointer};
    /// use rex_typesystem::{BuiltinTypeId, Scheme, Type};
    ///
    /// let mut module = Module::<()>::new("acme.dynamic");
    /// let scheme = Scheme::new(vec![], vec![], Type::builtin(BuiltinTypeId::I32));
    ///
    /// module
    ///     .export_native_async(
    ///         "answer_async",
    ///         scheme,
    ///         0,
    ///         |engine: EvaluatorRef<'_, ()>, _typ: Type, _args: Vec<Pointer>| {
    ///             async move { engine.heap.alloc_i32(42) }.boxed()
    ///         },
    ///     )
    ///     .unwrap();
    /// ```
    pub fn export_native_async<F>(
        &mut self,
        name: impl Into<String>,
        scheme: Scheme,
        arity: usize,
        handler: F,
    ) -> Result<(), EngineError>
    where
        F: for<'a> Fn(EvaluatorRef<'a, State>, Type, Vec<Pointer>) -> NativeFuture<'a>
            + Send
            + Sync
            + 'static,
    {
        self.exports
            .push(Export::from_native_async(name, scheme, arity, handler)?);
        Ok(())
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
        F: for<'a> Fn(EvaluatorRef<'a, State>, Type, Vec<Pointer>) -> NativeFuture<'a>
            + Send
            + Sync
            + 'static,
    {
        self.exports.push(Export::from_native_async_with_gas_cost(
            name, scheme, arity, gas_cost, handler,
        )?);
        Ok(())
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
                EvaluatorRef<'a, State>,
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
                EvaluatorRef<'a, State>,
                CancellationToken,
                Type,
                &'a [Pointer],
            ) -> NativeFuture<'a>
            + Send
            + Sync
            + 'static,
    {
        self.exports
            .push(Export::from_native_async_cancellable_with_gas_cost(
                name, scheme, arity, gas_cost, handler,
            )?);
        Ok(())
    }

    pub fn export_value<V>(&mut self, name: impl Into<String>, value: V) -> Result<(), EngineError>
    where
        V: IntoPointer + RexType + Clone + Send + Sync + 'static,
    {
        self.exports.push(Export::from_value(name, value)?);
        Ok(())
    }

    pub fn export_value_typed(
        &mut self,
        name: impl Into<String>,
        typ: Type,
        value: Value,
    ) -> Result<(), EngineError> {
        self.exports
            .push(Export::from_value_typed(name, typ, value)?);
        Ok(())
    }
}

fn type_decl_from_adt(adt: &AdtDecl) -> TypeDecl {
    TypeDecl {
        span: Span::default(),
        is_pub: true,
        name: adt.name.clone(),
        params: adt.params.iter().map(|p| p.name.clone()).collect(),
        variants: adt
            .variants
            .iter()
            .map(|variant| TypeVariant {
                name: variant.name.clone(),
                args: variant.args.iter().map(type_expr_from_type).collect(),
            })
            .collect(),
    }
}

fn type_expr_from_type(typ: &Type) -> TypeExpr {
    match typ.as_ref() {
        TypeKind::Var(tv) => {
            let name = tv
                .name
                .clone()
                .unwrap_or_else(|| intern(&format!("t{}", tv.id)));
            TypeExpr::Name(Span::default(), NameRef::Unqualified(name))
        }
        TypeKind::Con(con) => {
            TypeExpr::Name(Span::default(), NameRef::Unqualified(con.name.clone()))
        }
        TypeKind::App(fun, arg) => {
            if let TypeKind::App(head, err) = fun.as_ref()
                && let TypeKind::Con(con) = head.as_ref()
                && con.builtin_id == Some(BuiltinTypeId::Result)
                && con.arity == 2
            {
                let result =
                    TypeExpr::Name(Span::default(), NameRef::Unqualified(con.name.clone()));
                let ok_expr = type_expr_from_type(arg);
                let err_expr = type_expr_from_type(err);
                let app1 = TypeExpr::App(Span::default(), Box::new(result), Box::new(ok_expr));
                return TypeExpr::App(Span::default(), Box::new(app1), Box::new(err_expr));
            }
            TypeExpr::App(
                Span::default(),
                Box::new(type_expr_from_type(fun)),
                Box::new(type_expr_from_type(arg)),
            )
        }
        TypeKind::Fun(arg, ret) => TypeExpr::Fun(
            Span::default(),
            Box::new(type_expr_from_type(arg)),
            Box::new(type_expr_from_type(ret)),
        ),
        TypeKind::Tuple(elems) => TypeExpr::Tuple(
            Span::default(),
            elems.iter().map(type_expr_from_type).collect(),
        ),
        TypeKind::Record(fields) => TypeExpr::Record(
            Span::default(),
            fields
                .iter()
                .map(|(name, ty)| (name.clone(), type_expr_from_type(ty)))
                .collect(),
        ),
    }
}

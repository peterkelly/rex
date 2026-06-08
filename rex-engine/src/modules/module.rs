use std::sync::Arc;

use rex_ast::{
    ClassDecl, CompilationUnit, Decl, DeclareFnDecl, Expr, FnDecl, ImportDecl, InstanceDecl, Span,
    TypeDecl, TypeVariant,
};
use rex_typesystem::{
    types::{AdtDecl, RexType, Scheme, Type},
    types::{collect_adts_in_types, order_adt_family},
};

use crate::{
    Context, EngineError, Handle, IntoRex,
    builder::{
        core::Builder,
        export::{Export, HostFnAsync, HostFnSync, NativeFuture},
    },
    modules::ROOT_MODULE_NAME,
    util::{adt_family_error_to_engine, type_expr_from_type},
};

#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Declarations {
    pub types: Vec<TypeDecl>,
    pub fns: Vec<FnDecl>,
    pub declare_fns: Vec<DeclareFnDecl>,
    pub imports: Vec<ImportDecl>,
    pub classes: Vec<ClassDecl>,
    pub instances: Vec<InstanceDecl>,
}

impl Declarations {
    pub fn is_empty(&self) -> bool {
        self.types.is_empty()
            && self.fns.is_empty()
            && self.declare_fns.is_empty()
            && self.imports.is_empty()
            && self.classes.is_empty()
            && self.instances.is_empty()
    }

    pub fn push_decl(&mut self, decl: Decl) {
        match decl {
            Decl::Type(decl) => self.types.push(decl),
            Decl::Fn(decl) => self.fns.push(decl),
            Decl::DeclareFn(decl) => self.declare_fns.push(decl),
            Decl::Import(decl) => self.imports.push(decl),
            Decl::Class(decl) => self.classes.push(decl),
            Decl::Instance(decl) => self.instances.push(decl),
        }
    }

    pub fn extend_decls(&mut self, decls: impl IntoIterator<Item = Decl>) {
        for decl in decls {
            self.push_decl(decl);
        }
    }
}

impl From<Vec<Decl>> for Declarations {
    fn from(decls: Vec<Decl>) -> Self {
        let mut out = Declarations::default();
        out.extend_decls(decls);
        out
    }
}

impl From<&[Decl]> for Declarations {
    fn from(decls: &[Decl]) -> Self {
        Declarations::from(decls.to_vec())
    }
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct CompilationPackage {
    pub decls: Declarations,
    pub body: Option<Arc<Expr>>,
}

impl CompilationPackage {
    pub fn from_compilation_unit(unit: CompilationUnit) -> Self {
        Self {
            decls: Declarations::from(unit.decls),
            body: unit.body,
        }
    }
}

impl From<CompilationUnit> for CompilationPackage {
    fn from(unit: CompilationUnit) -> Self {
        Self::from_compilation_unit(unit)
    }
}

impl From<&CompilationUnit> for CompilationPackage {
    fn from(unit: &CompilationUnit) -> Self {
        Self {
            decls: Declarations::from(unit.decls.as_slice()),
            body: unit.body.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct StagedAdtDecl {
    pub adt: AdtDecl,
    pub type_decl: TypeDecl,
}

/// A staged host module that you build up in Rust and later inject into a [`Builder`].
///
/// `Module` is the host-side representation of a Rex module. It lets embedders collect:
///
/// - host-provided ADTs
/// - typeclass instances for existing classes
/// - typed Rust handlers via [`Module::export`] / [`Module::export_async`]
/// - handle-based dynamic native handlers via [`Module::export_native`] /
///   [`Module::export_native_async`]
///
/// Once the module is assembled, pass it to [`Builder::inject_module`] to make it importable
/// from Rex code.
///
/// This type is intentionally mutable and staged: you can build it incrementally, inspect its
/// staged declarations plus [`Module::exports`], transform them, and only inject it once you
/// are satisfied with the final module shape.
///
/// # Examples
///
/// ```rust,ignore
/// use rex_engine::{Builder, Module};
///
/// let mut builder = Builder::with_prelude(()).unwrap();
///
/// let mut math = Module::new("acme.math");
/// math.export("inc", |_state: &(), x: i32| Ok(x + 1)).unwrap();
///
/// builder.inject_module(math).unwrap();
/// ```
pub struct Module<State: Clone + Send + Sync + 'static> {
    /// The module name Rex code will import.
    ///
    /// This should be the fully-qualified module path you want users to write in `import`
    /// declarations, such as `acme.math` or `sample`.
    ///
    /// [`Builder::inject_module`] validates and reserves this name when the module is injected.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use rex_engine::Module;
    ///
    /// let module = Module::<()>::new("acme.math");
    /// assert_eq!(module.name(), "acme.math");
    /// ```
    pub(crate) name: String,

    /// ADT declarations staged for runtime constructor injection.
    ///
    /// APIs such as [`Module::add_adt_decl`], [`Module::add_adt_family`], and
    /// [`Module::add_rex_adt`] append here. The engine uses this list to register
    /// constructor schemes and to derive the module's source-level type declarations.
    pub(crate) adts: Vec<StagedAdtDecl>,

    /// Typeclass instance declarations staged for this module.
    pub(crate) instances: Vec<InstanceDecl>,

    /// Staged host exports that will become callable Rex values when the module is injected.
    ///
    /// Each [`Export`] bundles a public Rex name, a declaration that is inserted into the virtual
    /// module source, and the runtime injector that registers the implementation with the engine.
    ///
    /// Most callers populate this with [`Module::export`], [`Module::export_async`],
    /// [`Module::export_native`], [`Module::export_native_async`], or [`Module::add_export`].
    /// Use [`Module::add_export`] when exports are assembled separately.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use rex_engine::{Export, Module};
    ///
    /// let mut module = Module::<()>::new("acme.math");
    /// let export = Export::from_handler("inc", |_state: &(), x: i32| Ok(x + 1)).unwrap();
    /// module.add_export(export);
    ///
    /// assert_eq!(module.exports().len(), 1);
    /// ```
    pub(crate) exports: Vec<Export<State>>,
}

impl<State> Module<State>
where
    State: Clone + Send + Sync + 'static,
{
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
    /// helper methods on `Module`, then pass it to [`Builder::inject_module`].
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use rex_engine::Module;
    ///
    /// let module = Module::<()>::new("acme.math");
    /// assert_eq!(module.name(), "acme.math");
    /// assert!(module.declarations().is_empty());
    /// assert!(module.exports().is_empty());
    /// ```
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            adts: Vec::new(),
            instances: Vec::new(),
            exports: Vec::new(),
        }
    }

    /// Return the module name Rex code will import.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Return the staged ADTs for this module.
    pub fn adts(&self) -> &[StagedAdtDecl] {
        &self.adts
    }

    /// Return the staged typeclass instances for this module.
    pub fn instances(&self) -> &[InstanceDecl] {
        &self.instances
    }

    /// Return the staged host exports for this module.
    pub fn exports(&self) -> &[Export<State>] {
        &self.exports
    }

    /// Return this module's staged declarations in the compiler package representation.
    ///
    /// Type declarations are derived from [`Module::adts`] so staged ADTs remain the single source
    /// of truth for host-provided module-local types. Host exports are intentionally not included;
    /// named module installation adds their generated interface declarations separately.
    pub fn declarations(&self) -> Declarations {
        Declarations {
            types: self
                .adts
                .iter()
                .map(|staged| staged.type_decl.clone())
                .collect(),
            instances: self.instances.clone(),
            ..Declarations::default()
        }
    }

    /// Append a typeclass instance for a class that already exists.
    ///
    /// This lets embedders connect host-provided types and functions to existing Rex typeclasses
    /// without defining new typeclasses or injecting arbitrary Rex declarations through `Module`.
    pub fn add_instance(&mut self, instance: InstanceDecl) {
        self.instances.push(instance);
    }

    /// Convert an [`AdtDecl`] into a structured type declaration and append it to this module.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use rex_engine::{Builder, Module};
    ///
    /// let mut builder = Builder::with_prelude(()).unwrap();
    /// let mut module = Module::new("acme.types");
    /// let adt = builder.adt_decl_from_type(&rex_typesystem::Type::user_con("Thing", 0)).unwrap();
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
        for adt in order_adt_family(adts).map_err(adt_family_error_to_engine)? {
            let candidate = type_decl_from_adt(&adt);
            let already_staged = self
                .adts
                .iter()
                .find(|staged| staged.type_decl.name == adt.name);
            if let Some(existing) = already_staged {
                if existing.type_decl != candidate {
                    return Err(EngineError::Custom(format!(
                        "conflicting staged ADT registration for `{}`: existing declaration differs from new ADT declaration",
                        adt.name,
                    )));
                }
                continue;
            }
            self.adts.push(StagedAdtDecl {
                adt,
                type_decl: candidate,
            });
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
    /// - asks the builder to materialize each discovered ADT declaration
    /// - appends the resulting structured declarations to this module
    ///
    /// If conflicting ADT definitions are found for the same type constructor name, this returns
    /// an [`EngineError`] that describes the conflict instead of silently picking one.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use rex_engine::{Builder, Module};
    /// use rex_typesystem::{BuiltinTypeId, Type};
    ///
    /// let mut builder = Builder::with_prelude(()).unwrap();
    /// let mut module = Module::new("acme.types");
    /// let types = vec![
    ///     Type::app(Type::user_con("Foo", 1), Type::builtin(BuiltinTypeId::I32)),
    ///     Type::user_con("Bar", 0),
    /// ];
    ///
    /// module.add_adt_decls_from_types(&mut builder, types).unwrap();
    /// ```
    pub fn add_adt_decls_from_types(
        &mut self,
        builder: &mut Builder<State>,
        types: Vec<Type>,
    ) -> Result<(), EngineError> {
        let adts = collect_adts_in_types(types).map_err(crate::collect_adts_error_to_engine)?;
        for typ in adts {
            let adt = builder.adt_decl_from_type(&typ)?;
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
    /// use rex_engine::{Builder, Module};
    ///
    /// #[derive(rex::Rex)]
    /// struct Label {
    ///     text: String,
    /// }
    ///
    /// let mut builder = Builder::with_prelude(()).unwrap();
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
    /// The staged export becomes available to Rex code after [`Builder::inject_module`] is called.
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
        H: HostFnSync<State, Sig>,
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
        H: HostFnAsync<State, Sig>,
    {
        self.exports
            .push(Export::from_async_handler(name, handler)?);
        Ok(())
    }

    /// Stage a handle-based synchronous native export with an explicit Rex type scheme.
    ///
    /// This lower-level API is intended for dynamic or runtime-defined integrations where the
    /// handler needs dynamic Rex values or where the Rex type cannot be inferred from an
    /// ordinary Rust function signature alone.
    ///
    /// `scheme` describes the Rex-visible type, and `arity` must match the number of arguments the
    /// handler expects.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use rex_engine::{Context, Handle, Module};
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
    ///     .export_native("id_ptr", scheme, 1, |_ctx: Context<()>, _typ: &Type, args: &[Handle]| {
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
        F: for<'a> Fn(Context<State>, &'a Type, &'a [Handle]) -> Result<Handle, EngineError>
            + Send
            + Sync
            + 'static,
    {
        self.exports
            .push(Export::from_native(name, scheme, arity, handler)?);
        Ok(())
    }

    /// Stage a handle-based asynchronous native export with an explicit Rex type scheme.
    ///
    /// This is the async counterpart to [`Module::export_native`]. Use it when the export needs
    /// both direct engine access and asynchronous execution.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use futures::FutureExt;
    /// use rex_engine::{Context, Module};
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
    ///         |ctx: Context<()>, _typ: Type, _args| {
    ///             async move { ctx.heap().alloc_i32(42) }.boxed()
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
        F: Fn(Context<State>, Type, Vec<Handle>) -> NativeFuture + Send + Sync + 'static,
    {
        self.exports
            .push(Export::from_native_async(name, scheme, arity, handler)?);
        Ok(())
    }

    pub fn export_value<V>(&mut self, name: impl Into<String>, value: V) -> Result<(), EngineError>
    where
        V: IntoRex + RexType + Clone + Send + Sync + 'static,
    {
        self.exports.push(Export::<State>::from_value(name, value)?);
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

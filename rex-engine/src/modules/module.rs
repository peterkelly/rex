use std::sync::Arc;

use rex_ast::{
    ClassDecl, CompilationUnit, Decl, DeclareFnDecl, Expr, FnDecl, ImportDecl, InstanceDecl, Span,
    TypeDecl, TypeDeclKind, TypeExpr, TypeField, TypeParam, TypeVariant as AstTypeVariant,
    TypeVariantArg,
};
use rex_typesystem::{
    types::{AdtArgument, AdtDecl, AdtField, AdtVariant, RexType, Scheme, Type},
    types::{collect_adts_in_types, merge_adt_docs, order_adt_family},
};

use crate::{
    Context, EngineError, IntoRex, Value,
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

/// Parsed or synthesized module contents carried through compilation.
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct CompilationPackage {
    /// The module's structured declarations.
    pub decls: Declarations,
    /// Optional module body expression.
    pub body: Option<Arc<Expr>>,
    /// Optional Markdown API documentation for the module.
    pub docs: Option<String>,
}

impl CompilationPackage {
    pub fn from_compilation_unit(unit: CompilationUnit) -> Self {
        Self {
            decls: Declarations::from(unit.decls),
            body: unit.body,
            docs: None,
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
            docs: None,
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
/// - value-based dynamic native handlers via [`Module::export_native`] /
///   [`Module::export_native_async`]
///
/// Once the module is assembled, pass it to [`Builder::inject_module`] to make it importable
/// from Rex code.
///
/// This type is intentionally mutable and staged: you can build it incrementally, inspect its
/// staged declarations and exports, continue adding to it, and only inject it once you are
/// satisfied with the final module shape.
///
/// # Examples
///
/// ```rust,ignore
/// use rex_engine::{Builder, Module};
///
/// let mut builder = Builder::with_prelude(()).unwrap();
///
/// let mut math = Module::new("acme.math", None);
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
    /// let module = Module::<()>::new("acme.math", None);
    /// assert_eq!(module.name(), "acme.math");
    /// ```
    pub(crate) name: String,

    /// Markdown API documentation for this module.
    pub(crate) docs: Option<String>,

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
    /// module source, and either a callable registration or an owned value imported once during
    /// module installation.
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
    /// let mut module = Module::<()>::new("acme.math", None);
    /// let export = Export::from_handler("inc", |_state: &(), x: i32| Ok(x + 1)).unwrap();
    /// module.add_export(export)?;
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
        Self::new(ROOT_MODULE_NAME, None)
    }

    /// Create an empty staged module with the given import name and optional Markdown API
    /// documentation.
    ///
    /// The returned module contains no declarations and no exports yet. Add those with the
    /// helper methods on `Module`, then pass it to [`Builder::inject_module`].
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use rex_engine::Module;
    ///
    /// let module = Module::<()>::new(
    ///     "acme.math",
    ///     Some("Arithmetic APIs.".to_owned()),
    /// );
    /// assert_eq!(module.name(), "acme.math");
    /// assert_eq!(module.docs(), Some("Arithmetic APIs."));
    /// assert!(module.declarations().is_empty());
    /// assert!(module.exports().is_empty());
    /// ```
    pub fn new(name: impl Into<String>, docs: Option<String>) -> Self {
        Self {
            name: name.into(),
            docs,
            adts: Vec::new(),
            instances: Vec::new(),
            exports: Vec::new(),
        }
    }

    /// Return the module name Rex code will import.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Return this module's Markdown API documentation.
    pub fn docs(&self) -> Option<&str> {
        self.docs.as_deref()
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
    /// let mut module = Module::new("acme.types", None);
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
            if let Some(existing) = self
                .adts
                .iter_mut()
                .find(|staged| staged.type_decl.name == adt.name)
            {
                merge_adt_docs(&mut existing.adt, &adt)?;
                existing.type_decl = type_decl_from_adt(&existing.adt);
                continue;
            }
            let candidate = type_decl_from_adt(&adt);
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
    /// let mut module = Module::new("acme.types", None);
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
    /// let mut module = Module::new("sample", None);
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
    /// let mut module = Module::<()>::new("acme.math", None);
    /// let export = Export::from_handler("inc", |_state: &(), x: i32| Ok(x + 1)).unwrap();
    /// module.add_export(export)?;
    /// ```
    pub fn add_export(&mut self, mut export: Export<State>) -> Result<(), EngineError> {
        self.add_adt_family(std::mem::take(&mut export.required_adts))?;
        self.exports.push(export);
        Ok(())
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
    /// let mut module = Module::<()>::new("acme.math", None);
    /// module.export("inc", |_state: &(), x: i32| Ok(x + 1)).unwrap();
    /// ```
    pub fn export<Sig, H>(&mut self, name: impl Into<String>, handler: H) -> Result<(), EngineError>
    where
        H: HostFnSync<State, Sig>,
    {
        self.add_export(Export::from_handler(name, handler)?)
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
    /// let mut module = Module::<()>::new("acme.math", None);
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
        self.add_export(Export::from_async_handler(name, handler)?)
    }

    /// Stage a value-based synchronous native export with an explicit Rex type scheme.
    ///
    /// This lower-level API is intended for dynamic or runtime-defined integrations where the
    /// handler needs dynamic Rex values or where the Rex type cannot be inferred from an
    /// ordinary Rust function signature alone.
    ///
    /// `scheme` describes the Rex-visible type, and `arity` must match the number of arguments the
    /// handler expects.
    ///
    /// The evaluator copies arguments into owned [`Value`] trees before invoking the handler and
    /// imports the returned value afterward. Synchronous handlers resume immediately through the
    /// native completion path and do not consume asynchronous admission permits.
    /// They run on the evaluator task, so blocking or long-running work should use
    /// [`Module::export_native_async`] instead.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use rex_engine::{Context, Module, Value};
    /// use rex_typesystem::{BuiltinTypeId, Scheme, Type};
    ///
    /// let mut module = Module::<()>::new("acme.dynamic", None);
    /// let scheme = Scheme::new(
    ///     vec![],
    ///     vec![],
    ///     Type::fun(Type::builtin(BuiltinTypeId::I32), Type::builtin(BuiltinTypeId::I32)),
    /// );
    ///
    /// module
    ///     .export_native("id_value", scheme, 1, |_ctx: Context<()>, _typ: &Type, mut args: Vec<Value>| {
    ///         Ok(args.remove(0))
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
        F: for<'a> Fn(Context<State>, &'a Type, Vec<Value>) -> Result<Value, EngineError>
            + Send
            + Sync
            + 'static,
    {
        self.add_export(Export::from_native(name, scheme, arity, handler)?)
    }

    /// Stage a value-based asynchronous native export with an explicit Rex type scheme.
    ///
    /// This is the deferred counterpart to [`Module::export_native`]. Both APIs use owned
    /// [`Value`] trees; this variant additionally participates in asynchronous admission control
    /// and may remain suspended without retaining access to the evaluator heap.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use futures::FutureExt;
    /// use rex_engine::{Context, Module, Value};
    /// use rex_typesystem::{BuiltinTypeId, Scheme, Type};
    ///
    /// let mut module = Module::<()>::new("acme.dynamic", None);
    /// let scheme = Scheme::new(vec![], vec![], Type::builtin(BuiltinTypeId::I32));
    ///
    /// module
    ///     .export_native_async(
    ///         "answer_async",
    ///         scheme,
    ///         0,
    ///         |_ctx: Context<()>, _typ: Type, _args| {
    ///             async move { Ok(Value::I32(42)) }.boxed()
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
        F: Fn(Context<State>, Type, Vec<Value>) -> NativeFuture + Send + Sync + 'static,
    {
        self.add_export(Export::from_native_async(name, scheme, arity, handler)?)
    }

    pub fn export_value<V>(&mut self, name: impl Into<String>, value: V) -> Result<(), EngineError>
    where
        V: IntoRex + RexType + Send + Sync + 'static,
    {
        self.add_export(Export::<State>::from_value(name, value)?)
    }
}

fn type_decl_from_adt(adt: &AdtDecl) -> TypeDecl {
    TypeDecl {
        span: Span::default(),
        is_pub: true,
        name: adt.name.clone(),
        params: adt
            .params
            .iter()
            .map(|param| TypeParam {
                name: param.name.clone(),
                docs: param.docs.clone(),
            })
            .collect(),
        kind: TypeDeclKind::Adt(adt.variants.iter().map(type_variant_from_adt).collect()),
        docs: adt.docs.clone(),
    }
}

fn type_variant_from_adt(variant: &AdtVariant) -> AstTypeVariant {
    AstTypeVariant {
        name: variant.name.clone(),
        args: variant.args.iter().map(type_variant_arg_from_adt).collect(),
        docs: variant.docs.clone(),
    }
}

fn type_variant_arg_from_adt(arg: &AdtArgument) -> TypeVariantArg {
    match arg {
        AdtArgument::Positional { typ, docs } => TypeVariantArg {
            typ: type_expr_from_type(typ),
            docs: docs.clone(),
        },
        AdtArgument::Record { fields, docs } => TypeVariantArg {
            typ: TypeExpr::Record(
                Span::default(),
                fields.iter().map(type_field_from_adt).collect(),
            ),
            docs: docs.clone(),
        },
    }
}

fn type_field_from_adt(field: &AdtField) -> TypeField {
    TypeField {
        name: field.name.clone(),
        typ: type_expr_from_type(&field.typ),
        docs: field.docs.clone(),
    }
}

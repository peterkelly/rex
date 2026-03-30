use rexlang_typesystem::{AdtDecl, Type, collect_adts_in_types};

use crate::engine::{AsyncHandler, Export, Handler, NativeFuture};
use crate::{Engine, EngineError, Pointer, RexAdt};

/// A staged host library that you build up in Rust and later inject into an [`Engine`].
///
/// `Library` is the host-side representation of a Rex module. It lets embedders collect:
///
/// - Rex declarations such as `pub type ...`
/// - typed Rust handlers via [`Library::export`] / [`Library::export_async`]
/// - pointer-level native handlers via [`Library::export_native`] /
///   [`Library::export_native_async`]
///
/// Once the library is assembled, pass it to [`Engine::inject_library`] to make it importable
/// from Rex code.
///
/// This type is intentionally mutable and staged: you can build it incrementally, inspect its
/// [`Library::declarations`] and [`Library::exports`], transform them, and only inject it once you
/// are satisfied with the final module shape.
///
/// # Examples
///
/// ```rust,ignore
/// use rexlang_engine::{Engine, Library};
///
/// let mut engine = Engine::with_prelude(()).unwrap();
/// engine.add_default_resolvers();
///
/// let mut math = Library::new("acme.math");
/// math.export("inc", |_state: &(), x: i32| Ok(x + 1)).unwrap();
/// math.add_declaration("pub type Sign = Positive | Negative").unwrap();
///
/// engine.inject_library(math).unwrap();
/// ```
pub struct Library<State: Clone + Send + Sync + 'static> {
    /// The module name Rex code will import.
    ///
    /// This should be the fully-qualified library path you want users to write in `import`
    /// declarations, such as `acme.math` or `sample`.
    ///
    /// [`Engine::inject_library`] validates and reserves this name when the library is injected.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use rexlang_engine::Library;
    ///
    /// let library = Library::<()>::new("acme.math");
    /// assert_eq!(library.name, "acme.math");
    /// ```
    pub name: String,

    /// Raw Rex declarations that will be concatenated into the injected module source.
    ///
    /// This is most commonly used for `pub type ...` declarations, but it can hold any raw Rex
    /// declaration text you want included in the virtual module source.
    ///
    /// The usual way to append to this field is [`Library::add_declaration`], which validates that
    /// the added text is non-empty. The field itself is public so callers can inspect or construct
    /// a library in multiple passes.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use rexlang_engine::Library;
    ///
    /// let mut library = Library::<()>::new("acme.status");
    /// library
    ///     .add_declaration("pub type Status = Ready | Failed string")
    ///     .unwrap();
    ///
    /// assert_eq!(library.declarations.len(), 1);
    /// ```
    pub declarations: Vec<String>,

    /// Staged host exports that will become callable Rex values when the library is injected.
    ///
    /// Each [`Export`] bundles a public Rex name, a declaration that is inserted into the virtual
    /// module source, and the runtime injector that registers the implementation with the engine.
    ///
    /// Most callers populate this with [`Library::export`], [`Library::export_async`],
    /// [`Library::export_native`], [`Library::export_native_async`], or [`Library::add_export`].
    /// The field is public so advanced embedders can construct exports separately and assemble the
    /// final library programmatically.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use rexlang_engine::{Export, Library};
    ///
    /// let mut library = Library::<()>::new("acme.math");
    /// let export = Export::from_handler("inc", |_state: &(), x: i32| Ok(x + 1)).unwrap();
    /// library.exports.push(export);
    ///
    /// assert_eq!(library.exports.len(), 1);
    /// ```
    pub exports: Vec<Export<State>>,
}

impl<State> Library<State>
where
    State: Clone + Send + Sync + 'static,
{
    /// Create an empty staged library with the given import name.
    ///
    /// The returned library contains no declarations and no exports yet. Add those with the
    /// helper methods on `Library`, then pass it to [`Engine::inject_library`].
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use rexlang_engine::Library;
    ///
    /// let library = Library::<()>::new("acme.math");
    /// assert_eq!(library.name, "acme.math");
    /// assert!(library.declarations.is_empty());
    /// assert!(library.exports.is_empty());
    /// ```
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            declarations: Vec::new(),
            exports: Vec::new(),
        }
    }

    /// Append a raw Rex declaration to this staged library.
    ///
    /// Use this when you already have declaration text in Rex syntax, for example `pub type ...`.
    /// The text is stored exactly as provided and later concatenated into the virtual module source
    /// that [`Engine::inject_library`] exposes to Rex imports.
    ///
    /// This rejects empty or whitespace-only strings.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use rexlang_engine::Library;
    ///
    /// let mut library = Library::<()>::new("acme.status");
    /// library
    ///     .add_declaration("pub type Status = Ready | Failed string")
    ///     .unwrap();
    /// ```
    pub fn add_declaration(&mut self, declaration: impl Into<String>) -> Result<(), EngineError> {
        let declaration = declaration.into();
        if declaration.trim().is_empty() {
            return Err(EngineError::Internal(
                "library declaration cannot be empty".into(),
            ));
        }
        self.declarations.push(declaration);
        Ok(())
    }

    /// Convert an [`AdtDecl`] into Rex source and append it to [`Library::declarations`].
    ///
    /// This is a structured alternative to [`Library::add_declaration`] when you already have an
    /// ADT declaration in typed form.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use rexlang_engine::{Engine, Library};
    ///
    /// let mut engine = Engine::with_prelude(()).unwrap();
    /// let mut library = Library::new("acme.types");
    /// let adt = engine.adt_decl_from_type(&rexlang_typesystem::Type::user_con("Thing", 0)).unwrap();
    ///
    /// library.add_adt_decl(adt).unwrap();
    /// ```
    pub fn add_adt_decl(&mut self, adt: AdtDecl) -> Result<(), EngineError> {
        self.add_declaration(adt_declaration_line(&adt))
    }

    /// Discover user ADTs referenced by the supplied types and append their declarations.
    ///
    /// This is useful when you have Rust-side type information and want to emit the corresponding
    /// Rex `pub type ...` declarations for every user-defined ADT it mentions.
    ///
    /// The discovery process:
    ///
    /// - walks the provided types recursively
    /// - deduplicates repeated ADTs
    /// - asks the engine to materialize each discovered ADT declaration
    /// - appends the resulting declarations to this library
    ///
    /// If conflicting ADT definitions are found for the same type constructor name, this returns
    /// an [`EngineError`] that describes the conflict instead of silently picking one.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use rex_engine::{Engine, Library};
    /// use rexlang_typesystem::{BuiltinTypeId, Type};
    ///
    /// let mut engine = Engine::with_prelude(()).unwrap();
    /// let mut library = Library::new("acme.types");
    /// let types = vec![
    ///     Type::app(Type::user_con("Foo", 1), Type::builtin(BuiltinTypeId::I32)),
    ///     Type::user_con("Bar", 0),
    /// ];
    ///
    /// library.add_adt_decls_from_types(&mut engine, types).unwrap();
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

    /// Derive a Rex ADT declaration from a Rust type and append it to this library.
    ///
    /// This is the most ergonomic way to expose a Rust enum or struct that implements [`RexAdt`]
    /// as a library-local Rex type declaration.
    ///
    /// Unlike [`RexAdt::inject_rex`], this stages the declaration inside the library instead of
    /// injecting it straight into the engine root environment.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use rexlang_engine::{Engine, Library, RexAdt};
    ///
    /// #[derive(rexlang::Rex)]
    /// struct Label {
    ///     text: String,
    /// }
    ///
    /// let mut engine = Engine::with_prelude(()).unwrap();
    /// let mut library = Library::new("sample");
    /// library.inject_rex_adt::<Label>(&mut engine).unwrap();
    /// ```
    pub fn inject_rex_adt<T>(&mut self, engine: &mut Engine<State>) -> Result<(), EngineError>
    where
        T: RexAdt,
    {
        let adt = T::rex_adt_decl(engine)?;
        self.add_adt_decl(adt)
    }

    /// Append a preconstructed [`Export`] to this library.
    ///
    /// This is useful when exports are assembled elsewhere, such as from plugin metadata or a
    /// higher-level registration layer.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use rexlang_engine::{Export, Library};
    ///
    /// let mut library = Library::<()>::new("acme.math");
    /// let export = Export::from_handler("inc", |_state: &(), x: i32| Ok(x + 1)).unwrap();
    /// library.add_export(export);
    /// ```
    pub fn add_export(&mut self, export: Export<State>) {
        self.exports.push(export);
    }

    /// Stage a typed synchronous Rust handler as a library export.
    ///
    /// This is the most convenient API for exporting ordinary Rust functions or closures into a
    /// library. The handler's argument and return types drive the Rex signature automatically.
    ///
    /// The staged export becomes available to Rex code after [`Engine::inject_library`] is called.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use rexlang_engine::Library;
    ///
    /// let mut library = Library::<()>::new("acme.math");
    /// library.export("inc", |_state: &(), x: i32| Ok(x + 1)).unwrap();
    /// ```
    pub fn export<Sig, H>(&mut self, name: impl Into<String>, handler: H) -> Result<(), EngineError>
    where
        H: Handler<State, Sig>,
    {
        self.exports.push(Export::from_handler(name, handler)?);
        Ok(())
    }

    /// Stage a typed asynchronous Rust handler as a library export.
    ///
    /// Use this when the host implementation is naturally async, for example when it awaits I/O or
    /// other long-running work.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use rexlang_engine::Library;
    ///
    /// let mut library = Library::<()>::new("acme.math");
    /// library
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
    /// use rexlang_engine::{Engine, Library, Pointer};
    /// use rexlang_typesystem::{BuiltinTypeId, Scheme, Type};
    ///
    /// let mut library = Library::<()>::new("acme.dynamic");
    /// let scheme = Scheme::new(
    ///     vec![],
    ///     vec![],
    ///     Type::fun(Type::builtin(BuiltinTypeId::I32), Type::builtin(BuiltinTypeId::I32)),
    /// );
    ///
    /// library
    ///     .export_native("id_ptr", scheme, 1, |_engine: &Engine<()>, _typ: &Type, args: &[Pointer]| {
    ///         Ok(args[0].clone())
    ///     })
    ///     .unwrap();
    /// ```
    pub fn export_native<F>(
        &mut self,
        name: impl Into<String>,
        scheme: rexlang_typesystem::Scheme,
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

    /// Stage a pointer-level asynchronous native export with an explicit Rex type scheme.
    ///
    /// This is the async counterpart to [`Library::export_native`]. Use it when the export needs
    /// both direct engine access and asynchronous execution.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use futures::FutureExt;
    /// use rexlang_engine::{Engine, Library, Pointer};
    /// use rexlang_typesystem::{BuiltinTypeId, Scheme, Type};
    ///
    /// let mut library = Library::<()>::new("acme.dynamic");
    /// let scheme = Scheme::new(vec![], vec![], Type::builtin(BuiltinTypeId::I32));
    ///
    /// library
    ///     .export_native_async(
    ///         "answer_async",
    ///         scheme,
    ///         0,
    ///         |engine: &Engine<()>, _typ: Type, _args: Vec<Pointer>| {
    ///             async move { engine.heap.alloc_i32(42) }.boxed()
    ///         },
    ///     )
    ///     .unwrap();
    /// ```
    pub fn export_native_async<F>(
        &mut self,
        name: impl Into<String>,
        scheme: rexlang_typesystem::Scheme,
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

fn adt_declaration_line(adt: &AdtDecl) -> String {
    let head = if adt.params.is_empty() {
        adt.name.to_string()
    } else {
        let params = adt
            .params
            .iter()
            .map(|p| p.name.to_string())
            .collect::<Vec<_>>()
            .join(" ");
        format!("{} {}", adt.name, params)
    };
    let variants = adt
        .variants
        .iter()
        .map(|variant| {
            if variant.args.is_empty() {
                variant.name.to_string()
            } else {
                let args = variant
                    .args
                    .iter()
                    .map(adt_variant_arg_string)
                    .collect::<Vec<_>>()
                    .join(" ");
                format!("{} {}", variant.name, args)
            }
        })
        .collect::<Vec<_>>()
        .join(" | ");
    format!("pub type {head} = {variants}")
}

fn adt_variant_arg_string(typ: &Type) -> String {
    let s = typ.to_string();
    if s.contains(" -> ") {
        format!("({s})")
    } else {
        s
    }
}

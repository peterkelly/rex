use rexlang_typesystem::{AdtDecl, Type, collect_adts_in_types};

use crate::engine::{AsyncHandler, Export, Handler, NativeFuture};
use crate::{Engine, EngineError, Pointer, RexAdt};

/// A staged host library that can be injected into an [`Engine`].
pub struct Library<State: Clone + Send + Sync + 'static> {
    pub name: String,
    pub(crate) declarations: Vec<String>,
    pub(crate) exports: Vec<Export<State>>,
}

impl<State> Library<State>
where
    State: Clone + Send + Sync + 'static,
{
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            declarations: Vec::new(),
            exports: Vec::new(),
        }
    }

    /// Add raw Rex declarations to this library (for example `pub type ...`).
    ///
    /// Declarations are concatenated into the library source exactly as provided.
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

    /// Add an ADT declaration to this library.
    pub fn add_adt_decl(&mut self, adt: AdtDecl) -> Result<(), EngineError> {
        self.add_declaration(adt_declaration_line(&adt))
    }

    /// Collect user ADTs referenced by `types`, convert each to an `AdtDecl`,
    /// and add the declarations to this library.
    ///
    /// This deduplicates discovered ADTs across all inputs. If conflicting ADT
    /// definitions are found (same name, different constructor definition), this
    /// returns an `EngineError` describing every conflict.
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

    /// Build and add a Rust-backed ADT declaration into this library.
    pub fn inject_rex_adt<T>(&mut self, engine: &mut Engine<State>) -> Result<(), EngineError>
    where
        T: RexAdt,
    {
        let adt = T::rex_adt_decl(engine)?;
        self.add_adt_decl(adt)
    }

    pub fn add_export(&mut self, export: Export<State>) {
        self.exports.push(export);
    }

    pub fn export<Sig, H>(&mut self, name: impl Into<String>, handler: H) -> Result<(), EngineError>
    where
        H: Handler<State, Sig>,
    {
        self.exports.push(Export::from_handler(name, handler)?);
        Ok(())
    }

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

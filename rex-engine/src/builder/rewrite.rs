use crate::{
    builder::{
        core::{Builder, RustModuleInstallContext, install_named_rust_module},
        qualify::qualify_package,
    },
    compiler::Compiler,
    error::EngineError,
    modules::{
        CompilationPackage, Declarations, ImportBindingPolicy, ImportBindings, ModuleId,
        ResolvedModuleCache, add_import_bindings, alias_is_visible, collect_pattern_bindings,
        contains_import_alias_in_declarations, decl_type_names_from_declarations,
        decl_value_names_from_declarations, default_import_decl, exports_from_package,
        import_specifier, interface_decls_from_package, package_from_resolved,
        qualified_alias_member,
        system::ImportChain,
        types::{
            CanonicalSymbol, ImportRequest, ModuleExports, ResolvedModule, ResolvedModuleContent,
            prefix_for_module,
        },
    },
};
use futures::future::BoxFuture;
use rex_ast::{
    ClassDecl, ClassMethodSig, CompilationUnit, Decl, DeclareFnDecl, Expr, FnDecl, ImportDecl,
    InstanceDecl, InstanceMethodImpl, NameRef, Pattern, Symbol, TypeConstraint, TypeDecl, TypeExpr,
    TypeVariant, Var,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::Arc,
};

#[derive(Debug, Eq, PartialEq)]
pub struct ImportUseError {
    pub span: rex_ast::Span,
    pub error: EngineError,
}

impl ImportUseError {
    fn missing_export(span: rex_ast::Span, module: Symbol, export: Symbol) -> Self {
        Self {
            span,
            error: crate::ModuleError::MissingExport { module, export }.into(),
        }
    }

    pub fn into_error(self) -> EngineError {
        self.error
    }
}

impl fmt::Display for ImportUseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.error.fmt(f)
    }
}

pub(crate) trait ModuleRewriteContext<State>:
    RustModuleInstallContext<State> + Send
where
    State: Clone + Send + Sync + 'static,
{
    fn ensure_cycle_interfaces_published(
        &mut self,
        module_id: &ModuleId,
    ) -> Result<(), EngineError>;

    fn default_imports(&self) -> &[String] {
        &self.module_loader().default_imports
    }
}

impl<State> ModuleRewriteContext<State> for Builder<State>
where
    State: Clone + Send + Sync + 'static,
{
    fn ensure_cycle_interfaces_published(
        &mut self,
        module_id: &ModuleId,
    ) -> Result<(), EngineError> {
        Builder::ensure_cycle_interfaces_published(self, module_id)
    }
}

impl<State> ModuleRewriteContext<State> for Compiler<State>
where
    State: Clone + Send + Sync + 'static,
{
    fn ensure_cycle_interfaces_published(
        &mut self,
        module_id: &ModuleId,
    ) -> Result<(), EngineError> {
        Compiler::ensure_cycle_interfaces_published(self, module_id)
    }
}

#[derive(Debug)]
pub struct ModuleLoadState<State: Clone + Send + Sync + 'static = ()> {
    resolved_modules: ResolvedModuleCache<State>,
    loaded: BTreeMap<ModuleId, ModuleExports>,
    loading: BTreeSet<ModuleId>,
}

impl<State> Default for ModuleLoadState<State>
where
    State: Clone + Send + Sync + 'static,
{
    fn default() -> Self {
        Self {
            resolved_modules: ResolvedModuleCache::default(),
            loaded: BTreeMap::new(),
            loading: BTreeSet::new(),
        }
    }
}

impl<State> ModuleLoadState<State>
where
    State: Clone + Send + Sync + 'static,
{
    pub(crate) async fn import(
        &mut self,
        chain: &ImportChain<State>,
        request: ImportRequest,
    ) -> Result<ResolvedModule<State>, EngineError> {
        self.resolved_modules.import(chain, request).await
    }

    pub(crate) fn loaded_mut(&mut self) -> &mut BTreeMap<ModuleId, ModuleExports> {
        &mut self.loaded
    }

    pub(crate) fn loading_mut(&mut self) -> &mut BTreeSet<ModuleId> {
        &mut self.loading
    }
}

pub(crate) fn rewrite_package_with_imports<'a, State, C>(
    engine: &'a mut C,
    package: &'a CompilationPackage,
    importer: Option<ModuleId>,
    prefix: &'a str,
    chain: &'a ImportChain<State>,
    load_state: &'a mut ModuleLoadState<State>,
) -> BoxFuture<'a, Result<CompilationPackage, EngineError>>
where
    State: Clone + Send + Sync + 'static,
    C: ModuleRewriteContext<State> + 'a,
{
    Box::pin(async move {
        let mut bindings = ImportBindings::default();
        let local_values = decl_value_names_from_declarations(&package.decls);
        let local_types = decl_type_names_from_declarations(&package.decls);
        let import_policy = ImportBindingPolicy {
            forbidden_values: &local_values,
            forbidden_types: &local_types,
        };
        for import_decl in &package.decls.imports {
            let exports = resolve_module_exports_for_rewrite(
                engine,
                import_decl,
                importer.clone(),
                chain,
                load_state,
            )
            .await?;
            add_import_bindings(&mut bindings, import_decl, &exports, &import_policy)?;
        }

        let default_imports = engine.default_imports().to_vec();
        for module_name in default_imports {
            let alias = Symbol::intern(default_import_alias(&module_name));
            if contains_import_alias_in_declarations(&package.decls, &alias) {
                continue;
            }
            let import_decl = default_import_decl(&module_name);
            let exports = resolve_module_exports_for_rewrite(
                engine,
                &import_decl,
                importer.clone(),
                chain,
                load_state,
            )
            .await?;
            for (local, target) in exports.values() {
                if !local_values.contains(local) && !bindings.imported_values.contains_key(local) {
                    bindings
                        .imported_values
                        .insert(local.clone(), target.clone());
                }
            }
            for (local, target) in exports.types() {
                if !local_types.contains(local) && !bindings.imported_types.contains_key(local) {
                    bindings
                        .imported_types
                        .insert(local.clone(), target.clone());
                }
            }
            for (local, target) in exports.classes() {
                if !local_types.contains(local) && !bindings.imported_classes.contains_key(local) {
                    bindings
                        .imported_classes
                        .insert(local.clone(), target.clone());
                }
            }
        }

        let qualified = qualify_package(package, prefix);
        validate_import_uses_package(&qualified, &bindings.alias_exports, None)?;
        Ok(rewrite_import_uses_package(
            &qualified,
            &bindings.alias_exports,
            &bindings.imported_values,
            &bindings.imported_types,
            &bindings.imported_classes,
            Some(&local_types),
            None,
        ))
    })
}

fn validate_import_uses_expr(
    expr: &Expr,
    bound: &mut BTreeSet<Symbol>,
    aliases: &BTreeMap<Symbol, ModuleExports>,
    shadowed_values: Option<&BTreeSet<Symbol>>,
) -> Result<(), ImportUseError> {
    match expr {
        Expr::Project(span, base, field) => {
            if let Expr::Var(v) = base.as_ref()
                && alias_is_visible(&v.name, bound, shadowed_values)
                && let Some(exports) = aliases.get(&v.name)
                && exports.value(field).is_none()
            {
                return Err(ImportUseError::missing_export(
                    *span,
                    v.name.clone(),
                    field.clone(),
                ));
            }
            validate_import_uses_expr(base, bound, aliases, shadowed_values)
        }
        Expr::Lam(_, _, param, ann, constraints, body) => {
            if let Some(ann) = ann {
                validate_import_uses_type_expr(ann, bound, aliases, shadowed_values)?;
            }
            for c in constraints {
                validate_import_uses_class_name(
                    &c.class,
                    *c.typ.span(),
                    bound,
                    aliases,
                    shadowed_values,
                )?;
                validate_import_uses_type_expr(&c.typ, bound, aliases, shadowed_values)?;
            }
            bound.insert(param.name.clone());
            let res = validate_import_uses_expr(body, bound, aliases, shadowed_values);
            bound.remove(&param.name);
            res
        }
        Expr::Let(_, var, _type_params, ann, val, body) => {
            if let Some(ann) = ann {
                validate_import_uses_type_expr(ann, bound, aliases, shadowed_values)?;
            }
            validate_import_uses_expr(val, bound, aliases, shadowed_values)?;
            bound.insert(var.name.clone());
            let res = validate_import_uses_expr(body, bound, aliases, shadowed_values);
            bound.remove(&var.name);
            res
        }
        Expr::LetRec(_, bindings, body) => {
            for (_, _, ann, _) in bindings {
                if let Some(ann) = ann {
                    validate_import_uses_type_expr(ann, bound, aliases, shadowed_values)?;
                }
            }
            let names: Vec<Symbol> = bindings
                .iter()
                .map(|(var, _, _, _)| var.name.clone())
                .collect();
            for name in &names {
                bound.insert(name.clone());
            }
            for (_, _, _ann, def) in bindings {
                validate_import_uses_expr(def, bound, aliases, shadowed_values)?;
            }
            let res = validate_import_uses_expr(body, bound, aliases, shadowed_values);
            for name in &names {
                bound.remove(name);
            }
            res
        }
        Expr::Match(_, scrutinee, arms) => {
            validate_import_uses_expr(scrutinee, bound, aliases, shadowed_values)?;
            for (pat, arm_expr) in arms {
                let mut binds = Vec::new();
                collect_pattern_bindings(pat, &mut binds);
                for b in &binds {
                    bound.insert(b.clone());
                }
                let res = validate_import_uses_expr(arm_expr, bound, aliases, shadowed_values);
                for b in &binds {
                    bound.remove(b);
                }
                res?;
            }
            Ok(())
        }
        Expr::Tuple(_, elems) | Expr::List(_, elems) => {
            for e in elems {
                validate_import_uses_expr(e, bound, aliases, shadowed_values)?;
            }
            Ok(())
        }
        Expr::Dict(_, kvs) => {
            for v in kvs.values() {
                validate_import_uses_expr(v, bound, aliases, shadowed_values)?;
            }
            Ok(())
        }
        Expr::RecordUpdate(_, base, updates) => {
            validate_import_uses_expr(base, bound, aliases, shadowed_values)?;
            for v in updates.values() {
                validate_import_uses_expr(v, bound, aliases, shadowed_values)?;
            }
            Ok(())
        }
        Expr::App(_, f, x) => {
            validate_import_uses_expr(f, bound, aliases, shadowed_values)?;
            validate_import_uses_expr(x, bound, aliases, shadowed_values)
        }
        Expr::Ite(_, c, t, e) => {
            validate_import_uses_expr(c, bound, aliases, shadowed_values)?;
            validate_import_uses_expr(t, bound, aliases, shadowed_values)?;
            validate_import_uses_expr(e, bound, aliases, shadowed_values)
        }
        Expr::Ann(_, e, t) => {
            validate_import_uses_expr(e, bound, aliases, shadowed_values)?;
            validate_import_uses_type_expr(t, bound, aliases, shadowed_values)
        }
        Expr::Var(..)
        | Expr::Bool(..)
        | Expr::Uint(..)
        | Expr::Int(..)
        | Expr::Float(..)
        | Expr::String(..)
        | Expr::Uuid(..)
        | Expr::DateTime(..)
        | Expr::Hole(..) => Ok(()),
    }
}

fn validate_import_uses_class_name(
    class: &NameRef,
    span: rex_ast::Span,
    bound: &BTreeSet<Symbol>,
    aliases: &BTreeMap<Symbol, ModuleExports>,
    shadowed_values: Option<&BTreeSet<Symbol>>,
) -> Result<(), ImportUseError> {
    let Some((alias_sym, member_sym)) = qualified_alias_member(class) else {
        return Ok(());
    };
    if !alias_is_visible(alias_sym, bound, shadowed_values) {
        return Ok(());
    }
    let Some(exports) = aliases.get(alias_sym) else {
        return Ok(());
    };
    if exports.class(member_sym).is_some() {
        return Ok(());
    }
    Err(ImportUseError::missing_export(
        span,
        alias_sym.clone(),
        member_sym.clone(),
    ))
}

fn validate_import_uses_type_expr(
    ty: &TypeExpr,
    bound: &BTreeSet<Symbol>,
    aliases: &BTreeMap<Symbol, ModuleExports>,
    shadowed_values: Option<&BTreeSet<Symbol>>,
) -> Result<(), ImportUseError> {
    match ty {
        TypeExpr::Name(span, name) => {
            let Some((alias_sym, member_sym)) = qualified_alias_member(name) else {
                return Ok(());
            };
            if !alias_is_visible(alias_sym, bound, shadowed_values) {
                return Ok(());
            }
            let Some(exports) = aliases.get(alias_sym) else {
                return Ok(());
            };
            if exports.typ(member_sym).is_some() || exports.class(member_sym).is_some() {
                Ok(())
            } else {
                Err(ImportUseError::missing_export(
                    *span,
                    alias_sym.clone(),
                    member_sym.clone(),
                ))
            }
        }
        TypeExpr::App(_, f, x) => {
            validate_import_uses_type_expr(f, bound, aliases, shadowed_values)?;
            validate_import_uses_type_expr(x, bound, aliases, shadowed_values)
        }
        TypeExpr::Fun(_, a, b) => {
            validate_import_uses_type_expr(a, bound, aliases, shadowed_values)?;
            validate_import_uses_type_expr(b, bound, aliases, shadowed_values)
        }
        TypeExpr::Tuple(_, elems) => {
            for e in elems {
                validate_import_uses_type_expr(e, bound, aliases, shadowed_values)?;
            }
            Ok(())
        }
        TypeExpr::Record(_, fields) => {
            for (_, t) in fields {
                validate_import_uses_type_expr(t, bound, aliases, shadowed_values)?;
            }
            Ok(())
        }
    }
}

pub fn validate_import_uses_with_spans(
    compilation_unit: &CompilationUnit,
    aliases: &BTreeMap<Symbol, ModuleExports>,
    shadowed_values: Option<&BTreeSet<Symbol>>,
) -> Result<(), ImportUseError> {
    validate_import_uses_decls_and_body(
        &compilation_unit.decls,
        compilation_unit.body.as_deref(),
        aliases,
        shadowed_values,
    )
}

pub fn validate_import_uses_package_with_spans(
    package: &CompilationPackage,
    aliases: &BTreeMap<Symbol, ModuleExports>,
    shadowed_values: Option<&BTreeSet<Symbol>>,
) -> Result<(), ImportUseError> {
    validate_import_uses_declarations_and_body(
        &package.decls,
        package.body.as_deref(),
        aliases,
        shadowed_values,
    )
}

fn validate_import_uses_decls_and_body(
    decls: &[Decl],
    body: Option<&Expr>,
    aliases: &BTreeMap<Symbol, ModuleExports>,
    shadowed_values: Option<&BTreeSet<Symbol>>,
) -> Result<(), ImportUseError> {
    for decl in decls {
        match decl {
            Decl::Fn(fd) => {
                for (_, t) in &fd.params {
                    validate_import_uses_type_expr(t, &BTreeSet::new(), aliases, shadowed_values)?;
                }
                validate_import_uses_type_expr(
                    &fd.ret,
                    &BTreeSet::new(),
                    aliases,
                    shadowed_values,
                )?;
                for c in &fd.constraints {
                    validate_import_uses_class_name(
                        &c.class,
                        *c.typ.span(),
                        &BTreeSet::new(),
                        aliases,
                        shadowed_values,
                    )?;
                    validate_import_uses_type_expr(
                        &c.typ,
                        &BTreeSet::new(),
                        aliases,
                        shadowed_values,
                    )?;
                }
                let mut bound: BTreeSet<Symbol> =
                    fd.params.iter().map(|(v, _)| v.name.clone()).collect();
                validate_import_uses_expr(fd.body.as_ref(), &mut bound, aliases, shadowed_values)?;
            }
            Decl::DeclareFn(df) => {
                for (_, t) in &df.params {
                    validate_import_uses_type_expr(t, &BTreeSet::new(), aliases, shadowed_values)?;
                }
                validate_import_uses_type_expr(
                    &df.ret,
                    &BTreeSet::new(),
                    aliases,
                    shadowed_values,
                )?;
                for c in &df.constraints {
                    validate_import_uses_class_name(
                        &c.class,
                        *c.typ.span(),
                        &BTreeSet::new(),
                        aliases,
                        shadowed_values,
                    )?;
                    validate_import_uses_type_expr(
                        &c.typ,
                        &BTreeSet::new(),
                        aliases,
                        shadowed_values,
                    )?;
                }
            }
            Decl::Type(td) => {
                for v in &td.variants {
                    for t in &v.args {
                        validate_import_uses_type_expr(
                            t,
                            &BTreeSet::new(),
                            aliases,
                            shadowed_values,
                        )?;
                    }
                }
            }
            Decl::Class(cd) => {
                for c in &cd.supers {
                    validate_import_uses_class_name(
                        &c.class,
                        *c.typ.span(),
                        &BTreeSet::new(),
                        aliases,
                        shadowed_values,
                    )?;
                    validate_import_uses_type_expr(
                        &c.typ,
                        &BTreeSet::new(),
                        aliases,
                        shadowed_values,
                    )?;
                }
                for m in &cd.methods {
                    validate_import_uses_type_expr(
                        &m.typ,
                        &BTreeSet::new(),
                        aliases,
                        shadowed_values,
                    )?;
                }
            }
            Decl::Instance(inst) => {
                validate_import_uses_class_name(
                    &NameRef::from_dotted(inst.class.as_ref()),
                    inst.span,
                    &BTreeSet::new(),
                    aliases,
                    shadowed_values,
                )?;
                validate_import_uses_type_expr(
                    &inst.head,
                    &BTreeSet::new(),
                    aliases,
                    shadowed_values,
                )?;
                for c in &inst.context {
                    validate_import_uses_class_name(
                        &c.class,
                        *c.typ.span(),
                        &BTreeSet::new(),
                        aliases,
                        shadowed_values,
                    )?;
                    validate_import_uses_type_expr(
                        &c.typ,
                        &BTreeSet::new(),
                        aliases,
                        shadowed_values,
                    )?;
                }
                for m in &inst.methods {
                    let mut bound = BTreeSet::new();
                    validate_import_uses_expr(
                        m.body.as_ref(),
                        &mut bound,
                        aliases,
                        shadowed_values,
                    )?;
                }
            }
            Decl::Import(..) => {}
        }
    }
    let mut bound = BTreeSet::new();
    if let Some(body) = body {
        validate_import_uses_expr(body, &mut bound, aliases, shadowed_values)?;
    }
    Ok(())
}

fn validate_import_uses_declarations_and_body(
    decls: &Declarations,
    body: Option<&Expr>,
    aliases: &BTreeMap<Symbol, ModuleExports>,
    shadowed_values: Option<&BTreeSet<Symbol>>,
) -> Result<(), ImportUseError> {
    for fd in &decls.fns {
        for (_, t) in &fd.params {
            validate_import_uses_type_expr(t, &BTreeSet::new(), aliases, shadowed_values)?;
        }
        validate_import_uses_type_expr(&fd.ret, &BTreeSet::new(), aliases, shadowed_values)?;
        for c in &fd.constraints {
            validate_import_uses_class_name(
                &c.class,
                *c.typ.span(),
                &BTreeSet::new(),
                aliases,
                shadowed_values,
            )?;
            validate_import_uses_type_expr(&c.typ, &BTreeSet::new(), aliases, shadowed_values)?;
        }
        let mut bound: BTreeSet<Symbol> = fd.params.iter().map(|(v, _)| v.name.clone()).collect();
        validate_import_uses_expr(fd.body.as_ref(), &mut bound, aliases, shadowed_values)?;
    }
    for df in &decls.declare_fns {
        for (_, t) in &df.params {
            validate_import_uses_type_expr(t, &BTreeSet::new(), aliases, shadowed_values)?;
        }
        validate_import_uses_type_expr(&df.ret, &BTreeSet::new(), aliases, shadowed_values)?;
        for c in &df.constraints {
            validate_import_uses_class_name(
                &c.class,
                *c.typ.span(),
                &BTreeSet::new(),
                aliases,
                shadowed_values,
            )?;
            validate_import_uses_type_expr(&c.typ, &BTreeSet::new(), aliases, shadowed_values)?;
        }
    }
    for td in &decls.types {
        for v in &td.variants {
            for t in &v.args {
                validate_import_uses_type_expr(t, &BTreeSet::new(), aliases, shadowed_values)?;
            }
        }
    }
    for cd in &decls.classes {
        for c in &cd.supers {
            validate_import_uses_class_name(
                &c.class,
                *c.typ.span(),
                &BTreeSet::new(),
                aliases,
                shadowed_values,
            )?;
            validate_import_uses_type_expr(&c.typ, &BTreeSet::new(), aliases, shadowed_values)?;
        }
        for m in &cd.methods {
            validate_import_uses_type_expr(&m.typ, &BTreeSet::new(), aliases, shadowed_values)?;
        }
    }
    for inst in &decls.instances {
        validate_import_uses_class_name(
            &NameRef::from_dotted(inst.class.as_ref()),
            inst.span,
            &BTreeSet::new(),
            aliases,
            shadowed_values,
        )?;
        validate_import_uses_type_expr(&inst.head, &BTreeSet::new(), aliases, shadowed_values)?;
        for c in &inst.context {
            validate_import_uses_class_name(
                &c.class,
                *c.typ.span(),
                &BTreeSet::new(),
                aliases,
                shadowed_values,
            )?;
            validate_import_uses_type_expr(&c.typ, &BTreeSet::new(), aliases, shadowed_values)?;
        }
        for m in &inst.methods {
            let mut bound = BTreeSet::new();
            validate_import_uses_expr(m.body.as_ref(), &mut bound, aliases, shadowed_values)?;
        }
    }
    let mut bound = BTreeSet::new();
    if let Some(body) = body {
        validate_import_uses_expr(body, &mut bound, aliases, shadowed_values)?;
    }
    Ok(())
}

pub fn validate_import_uses(
    compilation_unit: &CompilationUnit,
    aliases: &BTreeMap<Symbol, ModuleExports>,
    shadowed_values: Option<&BTreeSet<Symbol>>,
) -> Result<(), EngineError> {
    validate_import_uses_with_spans(compilation_unit, aliases, shadowed_values)
        .map_err(ImportUseError::into_error)
}

pub fn validate_import_uses_package(
    package: &CompilationPackage,
    aliases: &BTreeMap<Symbol, ModuleExports>,
    shadowed_values: Option<&BTreeSet<Symbol>>,
) -> Result<(), EngineError> {
    validate_import_uses_package_with_spans(package, aliases, shadowed_values)
        .map_err(ImportUseError::into_error)
}

pub fn rewrite_import_uses(
    compilation_unit: &CompilationUnit,
    aliases: &BTreeMap<Symbol, ModuleExports>,
    imported_values: &BTreeMap<Symbol, CanonicalSymbol>,
    imported_types: &BTreeMap<Symbol, CanonicalSymbol>,
    imported_classes: &BTreeMap<Symbol, CanonicalSymbol>,
    shadowed_types: Option<&BTreeSet<Symbol>>,
    shadowed_values: Option<&BTreeSet<Symbol>>,
) -> CompilationUnit {
    let scope = RewriteScope {
        aliases,
        imported_values,
        imported_types,
        imported_classes,
        shadowed_types,
        shadowed_values,
    };
    let (decls, body) = rewrite_import_uses_decls_and_body(
        &compilation_unit.decls,
        compilation_unit.body.as_deref(),
        &scope,
    );
    CompilationUnit { decls, body }
}

pub fn rewrite_import_uses_package(
    package: &CompilationPackage,
    aliases: &BTreeMap<Symbol, ModuleExports>,
    imported_values: &BTreeMap<Symbol, CanonicalSymbol>,
    imported_types: &BTreeMap<Symbol, CanonicalSymbol>,
    imported_classes: &BTreeMap<Symbol, CanonicalSymbol>,
    shadowed_types: Option<&BTreeSet<Symbol>>,
    shadowed_values: Option<&BTreeSet<Symbol>>,
) -> CompilationPackage {
    let scope = RewriteScope {
        aliases,
        imported_values,
        imported_types,
        imported_classes,
        shadowed_types,
        shadowed_values,
    };
    let (decls, body) =
        rewrite_import_uses_declarations_and_body(&package.decls, package.body.as_deref(), &scope);
    CompilationPackage { decls, body }
}

fn rewrite_import_uses_declarations_and_body(
    decls: &Declarations,
    body: Option<&Expr>,
    scope: &RewriteScope<'_>,
) -> (Declarations, Option<Arc<Expr>>) {
    let mut out = Declarations {
        imports: decls.imports.clone(),
        ..Declarations::default()
    };
    for td in &decls.types {
        let rewritten =
            rewrite_import_uses_decls_and_body(&[Decl::Type(td.clone())], None, scope).0;
        if let Some(Decl::Type(td)) = rewritten.into_iter().next() {
            out.types.push(td);
        }
    }
    for fd in &decls.fns {
        let rewritten = rewrite_import_uses_decls_and_body(&[Decl::Fn(fd.clone())], None, scope).0;
        if let Some(Decl::Fn(fd)) = rewritten.into_iter().next() {
            out.fns.push(fd);
        }
    }
    for df in &decls.declare_fns {
        let rewritten =
            rewrite_import_uses_decls_and_body(&[Decl::DeclareFn(df.clone())], None, scope).0;
        if let Some(Decl::DeclareFn(df)) = rewritten.into_iter().next() {
            out.declare_fns.push(df);
        }
    }
    for cd in &decls.classes {
        let rewritten =
            rewrite_import_uses_decls_and_body(&[Decl::Class(cd.clone())], None, scope).0;
        if let Some(Decl::Class(cd)) = rewritten.into_iter().next() {
            out.classes.push(cd);
        }
    }
    for instance in &decls.instances {
        let rewritten =
            rewrite_import_uses_decls_and_body(&[Decl::Instance(instance.clone())], None, scope).0;
        if let Some(Decl::Instance(instance)) = rewritten.into_iter().next() {
            out.instances.push(instance);
        }
    }
    let body = rewrite_import_uses_decls_and_body(&[], body, scope).1;
    (out, body)
}

fn rewrite_import_uses_decls_and_body(
    decls: &[Decl],
    body: Option<&Expr>,
    scope: &RewriteScope<'_>,
) -> (Vec<Decl>, Option<Arc<Expr>>) {
    let aliases = scope.aliases;
    let imported_types = scope.imported_types;
    let imported_classes = scope.imported_classes;
    let shadowed_types = scope.shadowed_types;
    let shadowed_values = scope.shadowed_values;
    let decl_bound = BTreeSet::new();
    let decls = decls
        .iter()
        .map(|decl| match decl {
            Decl::Fn(fd) => {
                let mut bound: BTreeSet<Symbol> =
                    fd.params.iter().map(|(v, _)| v.name.clone()).collect();
                let body = Arc::new(rewrite_import_uses_expr(
                    fd.body.as_ref(),
                    &mut bound,
                    scope,
                ));
                Decl::Fn(FnDecl {
                    span: fd.span,
                    is_pub: fd.is_pub,
                    name: fd.name.clone(),
                    type_params: fd.type_params.clone(),
                    params: fd
                        .params
                        .iter()
                        .map(|(v, t)| {
                            (
                                v.clone(),
                                rewrite_import_uses_type_expr(
                                    t,
                                    &decl_bound,
                                    aliases,
                                    imported_types,
                                    shadowed_types,
                                    shadowed_values,
                                ),
                            )
                        })
                        .collect(),
                    ret: rewrite_import_uses_type_expr(
                        &fd.ret,
                        &decl_bound,
                        aliases,
                        imported_types,
                        shadowed_types,
                        shadowed_values,
                    ),
                    constraints: fd
                        .constraints
                        .iter()
                        .map(|c| TypeConstraint {
                            class: rewrite_import_uses_class_name(
                                &c.class,
                                &decl_bound,
                                aliases,
                                imported_classes,
                                shadowed_types,
                                shadowed_values,
                            ),
                            typ: rewrite_import_uses_type_expr(
                                &c.typ,
                                &decl_bound,
                                aliases,
                                imported_types,
                                shadowed_types,
                                shadowed_values,
                            ),
                        })
                        .collect(),
                    body,
                })
            }
            Decl::DeclareFn(df) => Decl::DeclareFn(DeclareFnDecl {
                span: df.span,
                is_pub: df.is_pub,
                name: df.name.clone(),
                type_params: df.type_params.clone(),
                params: df
                    .params
                    .iter()
                    .map(|(v, t)| {
                        (
                            v.clone(),
                            rewrite_import_uses_type_expr(
                                t,
                                &decl_bound,
                                aliases,
                                imported_types,
                                shadowed_types,
                                shadowed_values,
                            ),
                        )
                    })
                    .collect(),
                ret: rewrite_import_uses_type_expr(
                    &df.ret,
                    &decl_bound,
                    aliases,
                    imported_types,
                    shadowed_types,
                    shadowed_values,
                ),
                constraints: df
                    .constraints
                    .iter()
                    .map(|c| TypeConstraint {
                        class: rewrite_import_uses_class_name(
                            &c.class,
                            &decl_bound,
                            aliases,
                            imported_classes,
                            shadowed_types,
                            shadowed_values,
                        ),
                        typ: rewrite_import_uses_type_expr(
                            &c.typ,
                            &decl_bound,
                            aliases,
                            imported_types,
                            shadowed_types,
                            shadowed_values,
                        ),
                    })
                    .collect(),
            }),
            Decl::Type(td) => Decl::Type(TypeDecl {
                span: td.span,
                is_pub: td.is_pub,
                name: td.name.clone(),
                params: td.params.clone(),
                variants: td
                    .variants
                    .iter()
                    .map(|v| TypeVariant {
                        name: v.name.clone(),
                        args: v
                            .args
                            .iter()
                            .map(|t| {
                                rewrite_import_uses_type_expr(
                                    t,
                                    &decl_bound,
                                    aliases,
                                    imported_types,
                                    shadowed_types,
                                    shadowed_values,
                                )
                            })
                            .collect(),
                    })
                    .collect(),
            }),
            Decl::Class(cd) => Decl::Class(ClassDecl {
                span: cd.span,
                is_pub: cd.is_pub,
                name: cd.name.clone(),
                params: cd.params.clone(),
                supers: cd
                    .supers
                    .iter()
                    .map(|c| TypeConstraint {
                        class: rewrite_import_uses_class_name(
                            &c.class,
                            &decl_bound,
                            aliases,
                            imported_classes,
                            shadowed_types,
                            shadowed_values,
                        ),
                        typ: rewrite_import_uses_type_expr(
                            &c.typ,
                            &decl_bound,
                            aliases,
                            imported_types,
                            shadowed_types,
                            shadowed_values,
                        ),
                    })
                    .collect(),
                methods: cd
                    .methods
                    .iter()
                    .map(|m| ClassMethodSig {
                        name: m.name.clone(),
                        type_params: m.type_params.clone(),
                        typ: rewrite_import_uses_type_expr(
                            &m.typ,
                            &decl_bound,
                            aliases,
                            imported_types,
                            shadowed_types,
                            shadowed_values,
                        ),
                    })
                    .collect(),
            }),
            Decl::Instance(inst) => {
                let methods = inst
                    .methods
                    .iter()
                    .map(|m| {
                        let mut bound = BTreeSet::new();
                        let body =
                            Arc::new(rewrite_import_uses_expr(m.body.as_ref(), &mut bound, scope));
                        InstanceMethodImpl {
                            name: m.name.clone(),
                            type_params: m.type_params.clone(),
                            ann: m.ann.as_ref().map(|t| {
                                rewrite_import_uses_type_expr(
                                    t,
                                    &decl_bound,
                                    aliases,
                                    imported_types,
                                    shadowed_types,
                                    shadowed_values,
                                )
                            }),
                            body,
                        }
                    })
                    .collect();
                Decl::Instance(InstanceDecl {
                    span: inst.span,
                    is_pub: inst.is_pub,
                    type_params: inst.type_params.clone(),
                    class: rewrite_import_uses_class_name(
                        &NameRef::from_dotted(inst.class.as_ref()),
                        &decl_bound,
                        aliases,
                        imported_classes,
                        shadowed_types,
                        shadowed_values,
                    )
                    .to_dotted_symbol(),
                    head: rewrite_import_uses_type_expr(
                        &inst.head,
                        &decl_bound,
                        aliases,
                        imported_types,
                        shadowed_types,
                        shadowed_values,
                    ),
                    context: inst
                        .context
                        .iter()
                        .map(|c| TypeConstraint {
                            class: rewrite_import_uses_class_name(
                                &c.class,
                                &decl_bound,
                                aliases,
                                imported_classes,
                                shadowed_types,
                                shadowed_values,
                            ),
                            typ: rewrite_import_uses_type_expr(
                                &c.typ,
                                &decl_bound,
                                aliases,
                                imported_types,
                                shadowed_types,
                                shadowed_values,
                            ),
                        })
                        .collect(),
                    methods,
                })
            }
            other => other.clone(),
        })
        .collect();

    let body = body.map(|body| {
        let mut bound = BTreeSet::new();
        Arc::new(rewrite_import_uses_expr(body, &mut bound, scope))
    });
    (decls, body)
}

struct RewriteScope<'a> {
    aliases: &'a BTreeMap<Symbol, ModuleExports>,
    imported_values: &'a BTreeMap<Symbol, CanonicalSymbol>,
    imported_types: &'a BTreeMap<Symbol, CanonicalSymbol>,
    imported_classes: &'a BTreeMap<Symbol, CanonicalSymbol>,
    shadowed_types: Option<&'a BTreeSet<Symbol>>,
    shadowed_values: Option<&'a BTreeSet<Symbol>>,
}

fn rewrite_import_uses_expr(
    expr: &Expr,
    bound: &mut BTreeSet<Symbol>,
    scope: &RewriteScope<'_>,
) -> Expr {
    let rewrite_type = |ty: &TypeExpr, bound: &BTreeSet<Symbol>| {
        rewrite_import_uses_type_expr(
            ty,
            bound,
            scope.aliases,
            scope.imported_types,
            scope.shadowed_types,
            scope.shadowed_values,
        )
    };

    match expr {
        Expr::Bool(span, v) => Expr::Bool(*span, *v),
        Expr::Uint(span, v) => Expr::Uint(*span, *v),
        Expr::Int(span, v) => Expr::Int(*span, *v),
        Expr::Float(span, v) => Expr::Float(*span, *v),
        Expr::String(span, v) => Expr::String(*span, v.clone()),
        Expr::Uuid(span, v) => Expr::Uuid(*span, *v),
        Expr::DateTime(span, v) => Expr::DateTime(*span, *v),
        Expr::Hole(span) => Expr::Hole(*span),
        Expr::Project(span, base, field) => {
            if let Expr::Var(v) = base.as_ref()
                && alias_is_visible(&v.name, bound, scope.shadowed_values)
                && let Some(exports) = scope.aliases.get(&v.name)
                && let Some(internal) = exports.value(field)
            {
                return Expr::Var(Var {
                    span: *span,
                    name: internal.symbol().clone(),
                });
            }
            Expr::Project(
                *span,
                Arc::new(rewrite_import_uses_expr(base, bound, scope)),
                field.clone(),
            )
        }
        Expr::Var(v) => {
            if alias_is_visible(&v.name, bound, scope.shadowed_values)
                && let Some(internal) = scope.imported_values.get(&v.name)
            {
                Expr::Var(Var {
                    span: v.span,
                    name: internal.symbol().clone(),
                })
            } else {
                Expr::Var(v.clone())
            }
        }
        Expr::Lam(span, lam_scope, param, ann, constraints, body) => {
            let ann = ann.as_ref().map(|t| rewrite_type(t, bound));
            let constraints = constraints
                .iter()
                .map(|c| TypeConstraint {
                    class: rewrite_import_uses_class_name(
                        &c.class,
                        bound,
                        scope.aliases,
                        scope.imported_classes,
                        scope.shadowed_types,
                        scope.shadowed_values,
                    ),
                    typ: rewrite_type(&c.typ, bound),
                })
                .collect();
            bound.insert(param.name.clone());
            let out = Expr::Lam(
                *span,
                lam_scope.clone(),
                param.clone(),
                ann,
                constraints,
                Arc::new(rewrite_import_uses_expr(body, bound, scope)),
            );
            bound.remove(&param.name);
            out
        }
        Expr::Let(span, var, type_params, ann, val, body) => {
            let val = Arc::new(rewrite_import_uses_expr(val, bound, scope));
            bound.insert(var.name.clone());
            let body = Arc::new(rewrite_import_uses_expr(body, bound, scope));
            bound.remove(&var.name);
            Expr::Let(
                *span,
                var.clone(),
                type_params.clone(),
                ann.as_ref().map(|t| rewrite_type(t, bound)),
                val,
                body,
            )
        }
        Expr::LetRec(span, bindings, body) => {
            let anns: Vec<Option<TypeExpr>> = bindings
                .iter()
                .map(|(_, _, ann, _)| ann.as_ref().map(|t| rewrite_type(t, bound)))
                .collect();
            let names: Vec<Symbol> = bindings
                .iter()
                .map(|(var, _, _, _)| var.name.clone())
                .collect();
            for name in &names {
                bound.insert(name.clone());
            }
            let bindings = bindings
                .iter()
                .zip(anns)
                .map(|((var, type_params, _ann, def), ann)| {
                    (
                        var.clone(),
                        type_params.clone(),
                        ann,
                        Arc::new(rewrite_import_uses_expr(def, bound, scope)),
                    )
                })
                .collect();
            let body = Arc::new(rewrite_import_uses_expr(body, bound, scope));
            for name in &names {
                bound.remove(name);
            }
            Expr::LetRec(*span, bindings, body)
        }
        Expr::Match(span, scrutinee, arms) => {
            let scrutinee = Arc::new(rewrite_import_uses_expr(scrutinee, bound, scope));
            let mut renamed_arms = Vec::new();
            for (pat, arm_expr) in arms {
                let pat = rewrite_import_uses_pattern(pat, scope.imported_values);
                let mut binds = Vec::new();
                collect_pattern_bindings(&pat, &mut binds);
                for b in &binds {
                    bound.insert(b.clone());
                }
                let arm_expr = Arc::new(rewrite_import_uses_expr(arm_expr, bound, scope));
                for b in &binds {
                    bound.remove(b);
                }
                renamed_arms.push((pat, arm_expr));
            }
            Expr::Match(*span, scrutinee, renamed_arms)
        }
        Expr::Tuple(span, elems) => Expr::Tuple(
            *span,
            elems
                .iter()
                .map(|e| Arc::new(rewrite_import_uses_expr(e, bound, scope)))
                .collect(),
        ),
        Expr::List(span, elems) => Expr::List(
            *span,
            elems
                .iter()
                .map(|e| Arc::new(rewrite_import_uses_expr(e, bound, scope)))
                .collect(),
        ),
        Expr::Dict(span, kvs) => Expr::Dict(
            *span,
            kvs.iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        Arc::new(rewrite_import_uses_expr(v, bound, scope)),
                    )
                })
                .collect(),
        ),
        Expr::RecordUpdate(span, base, updates) => Expr::RecordUpdate(
            *span,
            Arc::new(rewrite_import_uses_expr(base, bound, scope)),
            updates
                .iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        Arc::new(rewrite_import_uses_expr(v, bound, scope)),
                    )
                })
                .collect(),
        ),
        Expr::App(span, f, x) => Expr::App(
            *span,
            Arc::new(rewrite_import_uses_expr(f, bound, scope)),
            Arc::new(rewrite_import_uses_expr(x, bound, scope)),
        ),
        Expr::Ite(span, c, t, e) => Expr::Ite(
            *span,
            Arc::new(rewrite_import_uses_expr(c, bound, scope)),
            Arc::new(rewrite_import_uses_expr(t, bound, scope)),
            Arc::new(rewrite_import_uses_expr(e, bound, scope)),
        ),
        Expr::Ann(span, e, t) => Expr::Ann(
            *span,
            Arc::new(rewrite_import_uses_expr(e, bound, scope)),
            rewrite_type(t, bound),
        ),
    }
}

fn rewrite_import_uses_pattern(
    pat: &Pattern,
    imported_values: &BTreeMap<Symbol, CanonicalSymbol>,
) -> Pattern {
    match pat {
        Pattern::Wildcard(span) => Pattern::Wildcard(*span),
        Pattern::Var(v) => Pattern::Var(v.clone()),
        Pattern::Named(span, name, args) => {
            let name = imported_values
                .get(&name.to_dotted_symbol())
                .map(|c| NameRef::Unqualified(c.symbol().clone()))
                .unwrap_or_else(|| name.clone());
            let args = args
                .iter()
                .map(|p| rewrite_import_uses_pattern(p, imported_values))
                .collect();
            Pattern::Named(*span, name, args)
        }
        Pattern::Tuple(span, elems) => Pattern::Tuple(
            *span,
            elems
                .iter()
                .map(|p| rewrite_import_uses_pattern(p, imported_values))
                .collect(),
        ),
        Pattern::List(span, elems) => Pattern::List(
            *span,
            elems
                .iter()
                .map(|p| rewrite_import_uses_pattern(p, imported_values))
                .collect(),
        ),
        Pattern::Cons(span, head, tail) => Pattern::Cons(
            *span,
            Box::new(rewrite_import_uses_pattern(head, imported_values)),
            Box::new(rewrite_import_uses_pattern(tail, imported_values)),
        ),
        Pattern::Dict(span, fields) => Pattern::Dict(
            *span,
            fields
                .iter()
                .map(|(name, p)| {
                    (
                        name.clone(),
                        rewrite_import_uses_pattern(p, imported_values),
                    )
                })
                .collect(),
        ),
    }
}

fn rewrite_import_uses_class_name(
    class: &NameRef,
    bound: &BTreeSet<Symbol>,
    aliases: &BTreeMap<Symbol, ModuleExports>,
    imported_classes: &BTreeMap<Symbol, CanonicalSymbol>,
    shadowed_types: Option<&BTreeSet<Symbol>>,
    shadowed_values: Option<&BTreeSet<Symbol>>,
) -> NameRef {
    if let NameRef::Unqualified(name) = class {
        if shadowed_types.is_some_and(|shadowed| shadowed.contains(name)) {
            return class.clone();
        }
        if let Some(new) = imported_classes.get(name) {
            return NameRef::Unqualified(new.symbol().clone());
        }
        return class.clone();
    }
    let Some((alias_sym, member_sym)) = qualified_alias_member(class) else {
        return class.clone();
    };
    if !alias_is_visible(alias_sym, bound, shadowed_values) {
        return class.clone();
    }
    let Some(exports) = aliases.get(alias_sym) else {
        return class.clone();
    };
    exports
        .class(member_sym)
        .map(|s| s.symbol().clone())
        .map(NameRef::Unqualified)
        .unwrap_or_else(|| class.clone())
}

fn rewrite_import_uses_type_expr(
    ty: &TypeExpr,
    bound: &BTreeSet<Symbol>,
    aliases: &BTreeMap<Symbol, ModuleExports>,
    imported_types: &BTreeMap<Symbol, CanonicalSymbol>,
    shadowed_types: Option<&BTreeSet<Symbol>>,
    shadowed_values: Option<&BTreeSet<Symbol>>,
) -> TypeExpr {
    match ty {
        TypeExpr::Name(span, name) => match name {
            NameRef::Unqualified(name) => {
                if shadowed_types.is_some_and(|shadowed| shadowed.contains(name)) {
                    return TypeExpr::Name(*span, NameRef::Unqualified(name.clone()));
                }
                if let Some(new) = imported_types.get(name) {
                    TypeExpr::Name(*span, NameRef::Unqualified(new.symbol().clone()))
                } else {
                    TypeExpr::Name(*span, NameRef::Unqualified(name.clone()))
                }
            }
            _ => {
                let Some((alias_sym, member_sym)) = qualified_alias_member(name) else {
                    return TypeExpr::Name(*span, name.clone());
                };
                if !alias_is_visible(alias_sym, bound, shadowed_values) {
                    return TypeExpr::Name(*span, name.clone());
                }
                let Some(exports) = aliases.get(alias_sym) else {
                    return TypeExpr::Name(*span, name.clone());
                };
                if let Some(new) = exports.typ(member_sym) {
                    TypeExpr::Name(*span, NameRef::Unqualified(new.symbol().clone()))
                } else if let Some(new) = exports.class(member_sym) {
                    TypeExpr::Name(*span, NameRef::Unqualified(new.symbol().clone()))
                } else {
                    TypeExpr::Name(*span, name.clone())
                }
            }
        },
        TypeExpr::App(span, f, x) => TypeExpr::App(
            *span,
            Box::new(rewrite_import_uses_type_expr(
                f,
                bound,
                aliases,
                imported_types,
                shadowed_types,
                shadowed_values,
            )),
            Box::new(rewrite_import_uses_type_expr(
                x,
                bound,
                aliases,
                imported_types,
                shadowed_types,
                shadowed_values,
            )),
        ),
        TypeExpr::Fun(span, a, b) => TypeExpr::Fun(
            *span,
            Box::new(rewrite_import_uses_type_expr(
                a,
                bound,
                aliases,
                imported_types,
                shadowed_types,
                shadowed_values,
            )),
            Box::new(rewrite_import_uses_type_expr(
                b,
                bound,
                aliases,
                imported_types,
                shadowed_types,
                shadowed_values,
            )),
        ),
        TypeExpr::Tuple(span, elems) => TypeExpr::Tuple(
            *span,
            elems
                .iter()
                .map(|e| {
                    rewrite_import_uses_type_expr(
                        e,
                        bound,
                        aliases,
                        imported_types,
                        shadowed_types,
                        shadowed_values,
                    )
                })
                .collect(),
        ),
        TypeExpr::Record(span, fields) => TypeExpr::Record(
            *span,
            fields
                .iter()
                .map(|(name, ty)| {
                    (
                        name.clone(),
                        rewrite_import_uses_type_expr(
                            ty,
                            bound,
                            aliases,
                            imported_types,
                            shadowed_types,
                            shadowed_values,
                        ),
                    )
                })
                .collect(),
        ),
    }
}

fn resolve_module_exports_for_rewrite<'a, State, C>(
    engine: &'a mut C,
    import_decl: &'a ImportDecl,
    importer: Option<ModuleId>,
    chain: &'a ImportChain<State>,
    load_state: &'a mut ModuleLoadState<State>,
) -> BoxFuture<'a, Result<ModuleExports, EngineError>>
where
    State: Clone + Send + Sync + 'static,
    C: ModuleRewriteContext<State> + 'a,
{
    Box::pin(async move {
        let module_id = import_specifier(&import_decl.path)?;
        let imported = load_state
            .resolved_modules
            .import(
                chain,
                ImportRequest {
                    module_id,
                    importer,
                },
            )
            .await?;
        if let Some(exports) = engine
            .module_loader()
            .module_exports_cache
            .get(&imported.id)
            .cloned()
        {
            engine.ensure_cycle_interfaces_published(&imported.id)?;
            return Ok(exports);
        }
        load_module_types_from_resolved(engine, imported, chain, load_state).await
    })
}

pub(crate) fn load_module_types_from_resolved<'a, State, C>(
    engine: &'a mut C,
    resolved: ResolvedModule<State>,
    chain: &'a ImportChain<State>,
    load_state: &'a mut ModuleLoadState<State>,
) -> BoxFuture<'a, Result<ModuleExports, EngineError>>
where
    State: Clone + Send + Sync + 'static,
    C: ModuleRewriteContext<State> + 'a,
{
    Box::pin(async move {
        if let Some(exports) = load_state.loaded.get(&resolved.id) {
            return Ok(exports.clone());
        }

        if load_state.loading.contains(&resolved.id)
            && let Some(exports) = load_state.loaded.get(&resolved.id)
        {
            return Ok(exports.clone());
        }

        if let ResolvedModuleContent::Module(module) = &resolved.content {
            let installed = install_named_rust_module(engine, &resolved.id, module.take()?)?;
            load_state
                .loaded
                .insert(installed.id.clone(), installed.exports.clone());
            return Ok(installed.exports);
        }

        load_module_types_via_scc(engine, resolved, chain, load_state).await
    })
}

fn load_module_types_via_scc<'a, State, C>(
    engine: &'a mut C,
    root: ResolvedModule<State>,
    chain: &'a ImportChain<State>,
    load_state: &'a mut ModuleLoadState<State>,
) -> BoxFuture<'a, Result<ModuleExports, EngineError>>
where
    State: Clone + Send + Sync + 'static,
    C: ModuleRewriteContext<State> + 'a,
{
    Box::pin(async move {
        #[derive(Clone)]
        struct PendingModule<State: Clone + Send + Sync + 'static> {
            resolved: ResolvedModule<State>,
            package: CompilationPackage,
            prefix: String,
        }

        if let Some(exports) = load_state.loaded.get(&root.id)
            && !load_state.loading.contains(&root.id)
        {
            return Ok(exports.clone());
        }

        let mut pending: BTreeMap<ModuleId, PendingModule<State>> = BTreeMap::new();
        let mut edges: BTreeMap<ModuleId, Vec<ModuleId>> = BTreeMap::new();
        let mut stack = vec![root.clone()];

        while let Some(resolved) = stack.pop() {
            if pending.contains_key(&resolved.id) {
                continue;
            }
            if load_state.loaded.contains_key(&resolved.id)
                && !load_state.loading.contains(&resolved.id)
            {
                continue;
            }

            let prefix = prefix_for_module(&resolved.id);
            let package = package_from_resolved(&resolved)?;
            let exports = exports_from_package(&package, &prefix, &resolved.id);
            load_state.loaded.insert(resolved.id.clone(), exports);
            load_state.loading.insert(resolved.id.clone());
            if let ResolvedModuleContent::Source(source) = &resolved.content {
                engine
                    .module_loader_mut()
                    .module_sources
                    .insert(resolved.id.clone(), source.clone());
            }
            let qualified = qualify_package(&package, &prefix);
            let interfaces = interface_decls_from_package(&qualified);
            engine
                .module_loader_mut()
                .module_interface_cache
                .insert(resolved.id.clone(), interfaces);

            let imports = graph_imports_for_package(&package, engine.default_imports());
            for import_decl in imports {
                let module_id = import_specifier(&import_decl.path)?;
                let imported = load_state
                    .resolved_modules
                    .import(
                        chain,
                        ImportRequest {
                            module_id,
                            importer: Some(resolved.id.clone()),
                        },
                    )
                    .await?;
                if matches!(&imported.content, ResolvedModuleContent::Module(_)) {
                    load_module_types_from_resolved(engine, imported, chain, load_state).await?;
                    continue;
                }
                edges
                    .entry(resolved.id.clone())
                    .or_default()
                    .push(imported.id.clone());
                if (load_state.loading.contains(&imported.id)
                    || !load_state.loaded.contains_key(&imported.id))
                    && !pending.contains_key(&imported.id)
                {
                    stack.push(imported);
                }
            }

            let module_id = resolved.id.clone();
            pending.insert(
                module_id.clone(),
                PendingModule {
                    resolved,
                    package,
                    prefix,
                },
            );
        }

        if pending.is_empty() {
            return load_state.loaded.get(&root.id).cloned().ok_or_else(|| {
                EngineError::Internal("missing module exports after SCC load".into())
            });
        }

        let pending_ids: Vec<ModuleId> = pending.keys().cloned().collect();
        let sccs = tarjan_scc_module_ids(&pending_ids, &edges);

        // Tarjan yields SCCs in reverse topological order of the SCC DAG, so
        // dependencies are processed before dependents.
        for component in sccs {
            let has_cycle = component.len() > 1;
            if has_cycle {
                for module_id in &component {
                    engine.ensure_cycle_interfaces_published(module_id)?;
                }
            }
            for module_id in &component {
                let node = pending
                    .get(module_id)
                    .ok_or_else(|| EngineError::Internal("missing pending module node".into()))?;
                let rewritten = rewrite_package_with_imports(
                    engine,
                    &node.package,
                    Some(node.resolved.id.clone()),
                    &node.prefix,
                    chain,
                    load_state,
                )
                .await?;
                engine.inject_decls(&rewritten.decls)?;
            }
            for module_id in component {
                load_state.loading.remove(&module_id);
            }
        }

        load_state
            .loaded
            .get(&root.id)
            .cloned()
            .ok_or_else(|| EngineError::Internal("missing root exports after SCC load".into()))
    })
}

fn graph_imports_for_package(
    package: &CompilationPackage,
    default_imports: &[String],
) -> Vec<ImportDecl> {
    let mut out = package.decls.imports.clone();
    for module_name in default_imports {
        let alias = Symbol::intern(default_import_alias(module_name));
        if contains_import_alias_in_declarations(&package.decls, &alias) {
            continue;
        }
        out.push(default_import_decl(module_name));
    }
    out
}

fn default_import_alias(module_name: &str) -> &str {
    module_name.rsplit('.').next().unwrap_or(module_name)
}

fn tarjan_scc_module_ids(
    nodes: &[ModuleId],
    edges: &BTreeMap<ModuleId, Vec<ModuleId>>,
) -> Vec<Vec<ModuleId>> {
    // Tarjan's SCC algorithm (linear in |V| + |E|).
    //
    // References:
    // - Tarjan, R. E. (1972). "Depth-first search and linear graph algorithms."
    //   SIAM Journal on Computing, 1(2), 146-160.
    // - Cormen et al. (CLRS), 3rd ed., §22.5 "Strongly connected components".
    //
    // Why Tarjan here:
    // - We need explicit SCC groups to process module cycles as units.
    // - We want one DFS pass with low overhead because this runs in module loading paths.
    #[derive(Default)]
    struct TarjanState {
        index: usize,
        index_of: BTreeMap<ModuleId, usize>,
        lowlink: BTreeMap<ModuleId, usize>,
        stack: Vec<ModuleId>,
        on_stack: BTreeSet<ModuleId>,
        components: Vec<Vec<ModuleId>>,
    }

    fn strong_connect(
        v: &ModuleId,
        node_set: &BTreeSet<ModuleId>,
        edges: &BTreeMap<ModuleId, Vec<ModuleId>>,
        st: &mut TarjanState,
    ) {
        st.index_of.insert(v.clone(), st.index);
        st.lowlink.insert(v.clone(), st.index);
        st.index += 1;

        st.stack.push(v.clone());
        st.on_stack.insert(v.clone());

        if let Some(neighbors) = edges.get(v) {
            for w in neighbors {
                if !node_set.contains(w) {
                    continue;
                }
                if !st.index_of.contains_key(w) {
                    strong_connect(w, node_set, edges, st);
                    let lw = st.lowlink.get(w).copied();
                    if let (Some(lw), Some(lv)) = (lw, st.lowlink.get_mut(v)) {
                        *lv = (*lv).min(lw);
                    }
                } else if st.on_stack.contains(w) {
                    let iw = st.index_of.get(w).copied();
                    if let (Some(iw), Some(lv)) = (iw, st.lowlink.get_mut(v)) {
                        *lv = (*lv).min(iw);
                    }
                }
            }
        }

        // Root of an SCC when lowlink(v) == index(v): pop until we get v.
        let is_root = st.lowlink.get(v) == st.index_of.get(v);
        if is_root {
            let mut component = Vec::new();
            while let Some(w) = st.stack.pop() {
                st.on_stack.remove(&w);
                component.push(w.clone());
                if &w == v {
                    break;
                }
            }
            st.components.push(component);
        }
    }

    let mut st = TarjanState::default();
    let node_set = nodes.iter().cloned().collect::<BTreeSet<_>>();
    for node in nodes {
        if !st.index_of.contains_key(node) {
            strong_connect(node, &node_set, edges, &mut st);
        }
    }
    st.components
}

use crate::prelude::*;
use crate::{completion::*, diagnostics::*, shared::*};

pub(crate) fn inject_program_decls(
    ts: &mut TypeSystem,
    compilation_unit: &CompilationUnit,
    want_prepared_instance: Option<usize>,
) -> std::result::Result<InjectedDecls, TsTypeError> {
    let mut instances = Vec::new();
    let mut prepared_target = None;
    let mut pending_non_instances: Vec<Decl> = Vec::new();

    let flush_non_instances =
        |ts: &mut TypeSystem, pending: &mut Vec<Decl>| -> std::result::Result<(), TsTypeError> {
            if pending.is_empty() {
                return Ok(());
            }
            ts.register_decls(pending)?;
            pending.clear();
            Ok(())
        };

    for (idx, decl) in compilation_unit.decls.iter().enumerate() {
        match decl {
            Decl::Instance(inst_decl) => {
                flush_non_instances(ts, &mut pending_non_instances)?;
                let prepared = ts.register_instance_decl(inst_decl)?;
                if want_prepared_instance.is_some_and(|want| want == idx) {
                    prepared_target = Some(prepared.clone());
                }
                instances.push((idx, prepared));
            }
            _ => pending_non_instances.push(decl.clone()),
        }
    }
    flush_non_instances(ts, &mut pending_non_instances)?;

    Ok((instances, prepared_target))
}

pub(crate) type PreparedInstance = (usize, PreparedInstanceDecl);
pub(crate) type InjectedDecls = (Vec<PreparedInstance>, Option<PreparedInstanceDecl>);

pub type PreparedProgram = (
    CompilationUnit,
    TypeSystem,
    HashMap<Symbol, ImportModuleInfo>,
    Vec<Diagnostic>,
);

pub fn prepare_program_with_imports(
    session: &AnalysisSession,
    uri: &Url,
    compilation_unit: &CompilationUnit,
) -> std::result::Result<PreparedProgram, String> {
    let mut builder =
        Builder::with_prelude(()).map_err(|e| format!("failed to build prelude: {e}"))?;
    let mut diagnostics = Vec::new();

    let module_service = session.module_service_for_uri(uri);

    let mut imports: HashMap<Symbol, ImportModuleInfo> = HashMap::new();
    let mut load_state = ModuleLoadState::default();
    let importer = uri_to_file_path(uri).and_then(|path| module_id_from_path(&path));
    let lsp_importer: Arc<dyn Importer> = Arc::new(module_service.clone());

    for decl in &compilation_unit.decls {
        let Decl::Import(
            import_decl @ ImportDecl {
                span, path, alias, ..
            },
        ) = decl
        else {
            continue;
        };
        let import_span = *span;

        let ImportPath::Local { segments, .. } = path else {
            // LSP does not attempt network fetches; leave remote imports unresolved.
            continue;
        };

        let module_name = segments
            .iter()
            .map(|s| s.as_ref())
            .collect::<Vec<_>>()
            .join(".");

        let loaded_import = match futures::executor::block_on(load_import_for_tooling(
            &mut builder,
            import_decl,
            importer.clone(),
            Some(Arc::clone(&lsp_importer)),
            &mut load_state,
        )) {
            Ok(loaded) => loaded,
            Err(err) => {
                diagnostics.push(diagnostic_for_span(
                    import_span,
                    if module_name.is_empty() {
                        err.to_string()
                    } else {
                        format!("module `{module_name}`: {err}")
                    },
                ));
                continue;
            }
        };

        let module_path = module_service
            .path_for_module(&loaded_import.module_id)
            .or_else(|| {
                module_service
                    .load_import_path(uri, path)
                    .ok()
                    .flatten()
                    .and_then(|module| module.path)
            });

        let mut export_defs = HashMap::new();
        if let Some(source) = &loaded_import.source {
            match tokenize_and_parse(source) {
                Ok((tokens, module_program)) => {
                    let index = index_decl_spans(&module_program, &tokens);
                    let export_names = loaded_import
                        .exports
                        .values()
                        .map(|(name, _)| name.as_ref().to_string())
                        .collect::<BTreeSet<_>>();

                    for name in &export_names {
                        if let Some(span) = index
                            .fn_defs
                            .get(name)
                            .copied()
                            .or_else(|| index.ctor_defs.get(name).copied())
                        {
                            export_defs.insert(name.clone(), span);
                        }
                    }
                }
                Err(err) => {
                    diagnostics.push(diagnostic_for_span(
                        import_span,
                        format!("module `{module_name}` could not be indexed: {err:?}"),
                    ));
                }
            }
        }

        imports.insert(
            alias.clone(),
            ImportModuleInfo {
                path: module_path,
                exports: loaded_import.exports,
                export_defs,
            },
        );
    }

    let ts = builder.type_system().clone();

    let mut bindings = ImportBindings::default();
    let local_values = decl_value_names(&compilation_unit.decls);
    let local_types = decl_type_names(&compilation_unit.decls);
    let import_policy = ImportBindingPolicy {
        forbidden_values: &local_values,
        forbidden_types: &local_types,
    };
    for decl in &compilation_unit.decls {
        let Decl::Import(import_decl) = decl else {
            continue;
        };
        let Some(info) = imports.get(&import_decl.alias) else {
            continue;
        };
        if let Err(err) =
            add_import_bindings(&mut bindings, import_decl, &info.exports, &import_policy)
        {
            diagnostics.push(diagnostic_for_span(import_decl.span, err.to_string()));
        }
    }
    if let Err(err) =
        validate_import_uses_with_spans(compilation_unit, &bindings.alias_exports, None)
    {
        diagnostics.push(diagnostic_for_span(err.span, err.to_string()));
    }
    let rewritten = rewrite_import_uses(
        compilation_unit,
        &bindings.alias_exports,
        &bindings.imported_values,
        &bindings.imported_types,
        &bindings.imported_classes,
        Some(&local_types),
        None,
    );
    Ok((rewritten, ts, imports, diagnostics))
}

pub(crate) fn completion_exports_for_module_alias(
    session: &AnalysisSession,
    uri: &Url,
    compilation_unit: &CompilationUnit,
    alias: &str,
) -> std::result::Result<Vec<String>, String> {
    let alias_sym = Symbol::intern(alias);
    let Some(import_decl) = compilation_unit.decls.iter().find_map(|d| {
        let Decl::Import(id) = d else { return None };
        if id.alias == alias_sym {
            Some(id)
        } else {
            None
        }
    }) else {
        return Ok(Vec::new());
    };

    let Some(module) = session
        .module_service_for_uri(uri)
        .load_import_path(uri, &import_decl.path)
        .map_err(|err| err.to_string())?
    else {
        return Ok(Vec::new());
    };
    let source = module.source;
    let module_id = module.id;
    let (_tokens, module_program) =
        tokenize_and_parse(&source).map_err(|_| "parse error".to_string())?;
    let prefix = prefix_for_module(&module_id);
    let module_exports = exports_from_program(&module_program, &prefix, &module_id);

    let mut exports = BTreeSet::new();
    for (name, _) in module_exports.values() {
        exports.insert(name.as_ref().to_string());
    }
    Ok(exports.into_iter().collect())
}

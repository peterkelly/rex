use crate::{builder::core::Builder, modules::ModuleId};
use rex_ast::{Decl, DeclareFnDecl, Span, Symbol, TypeDecl, TypeExpr};
use rex_typesystem::types::AdtDecl;
use std::{collections::BTreeMap, fmt::Write as _};

/// Return a markdown document that inventories the currently-registered
/// builder state.
///
/// The report includes:
/// - summary counts
/// - modules and exports
/// - ADTs
/// - functions/values in the type environment
/// - type classes, methods, and instances
/// - native implementations
///
/// # Examples
///
/// ```rust,ignore
/// use rex_engine::{Builder, registry_markdown};
///
/// let builder = Builder::with_prelude(()).unwrap();
/// let md = registry_markdown(&builder);
///
/// assert!(md.contains("# Builder Registry"));
/// assert!(md.contains("## ADTs"));
/// ```
pub fn registry_markdown<State>(builder: &Builder<State>) -> String
where
    State: Clone + Send + Sync + 'static,
{
    fn module_anchor(id: &ModuleId) -> String {
        let raw = format!("module-{id}").to_ascii_lowercase();
        let mut out = String::with_capacity(raw.len());
        let mut prev_dash = false;
        for ch in raw.chars() {
            let keep = ch.is_ascii_alphanumeric();
            let mapped = if keep { ch } else { '-' };
            if mapped == '-' {
                if prev_dash {
                    continue;
                }
                prev_dash = true;
            } else {
                prev_dash = false;
            }
            out.push(mapped);
        }
        out.trim_matches('-').to_string()
    }

    fn symbol_list(symbols: &[Symbol]) -> String {
        if symbols.is_empty() {
            "(none)".to_string()
        } else {
            symbols
                .iter()
                .map(|s| format!("`{s}`"))
                .collect::<Vec<_>>()
                .join(", ")
        }
    }

    let mut out = String::new();
    let _ = writeln!(&mut out, "# Builder Registry");
    let _ = writeln!(&mut out);
    let mut module_ids: BTreeMap<String, ModuleId> = BTreeMap::new();
    for id in builder.module_loader.module_exports_cache.keys() {
        module_ids.insert(id.to_string(), id.clone());
    }
    for id in builder.module_loader.module_sources.keys() {
        module_ids.insert(id.to_string(), id.clone());
    }
    for module_name in builder.module_loader.virtual_modules.keys() {
        if let Ok(id) = ModuleId::parse(module_name) {
            module_ids.insert(id.to_string(), id);
        }
    }
    for module_name in &builder.module_loader.injected_modules {
        if let Ok(id) = ModuleId::parse(module_name) {
            module_ids.insert(id.to_string(), id);
        }
    }

    let _ = writeln!(&mut out, "## Summary");
    let env_value_count = builder.type_system.env.values.size();
    let native_impl_count: usize = builder.runtime.natives.entries.values().map(Vec::len).sum();
    let class_count = builder.type_system.classes.classes.len();
    let class_instance_count: usize = builder
        .type_system
        .classes
        .instances
        .values()
        .map(Vec::len)
        .sum();
    let _ = writeln!(&mut out, "- Modules (all kinds): {}", module_ids.len());
    let _ = writeln!(
        &mut out,
        "- Injected modules: {}",
        builder.module_loader.injected_modules.len()
    );
    let _ = writeln!(
        &mut out,
        "- Virtual modules: {}",
        builder.module_loader.virtual_modules.len()
    );
    let _ = writeln!(&mut out, "- ADTs: {}", builder.type_system.adts.len());
    let _ = writeln!(
        &mut out,
        "- Values/functions in type env: {env_value_count}"
    );
    let _ = writeln!(&mut out, "- Type classes: {class_count}");
    let _ = writeln!(&mut out, "- Type class instances: {class_instance_count}");
    let _ = writeln!(&mut out, "- Native implementations: {native_impl_count}");
    let _ = writeln!(&mut out);

    let _ = writeln!(&mut out, "## Module Index");
    if module_ids.is_empty() {
        let _ = writeln!(&mut out, "_No modules registered._");
    } else {
        for (display, id) in &module_ids {
            let anchor = module_anchor(id);
            let _ = writeln!(&mut out, "- [`{display}`](#{anchor})");
        }
    }
    let _ = writeln!(&mut out);

    let _ = writeln!(&mut out, "## Modules");
    if module_ids.is_empty() {
        let _ = writeln!(&mut out, "_No modules registered._");
        let _ = writeln!(&mut out);
    } else {
        for (display, id) in module_ids {
            let anchor = module_anchor(&id);
            let _ = writeln!(&mut out, "<a id=\"{anchor}\"></a>");
            let _ = writeln!(&mut out, "### `{display}`");
            let virtual_source = builder
                .module_loader
                .virtual_modules
                .get(&id.to_string())
                .and_then(|module| {
                    module
                        .source
                        .clone()
                        .or_else(|| render_virtual_module_source(&module.decls))
                });
            if let Some(source) = builder
                .module_loader
                .module_sources
                .get(&id)
                .cloned()
                .or(virtual_source)
            {
                if source.trim().is_empty() {
                    let _ = writeln!(&mut out, "_Module source is empty._");
                } else {
                    let _ = writeln!(&mut out, "```rex");
                    let _ = writeln!(&mut out, "{}", source.trim_end());
                    let _ = writeln!(&mut out, "```");
                }
            } else {
                let _ = writeln!(&mut out, "_No captured source for this module._");
            }

            let exports = builder
                .module_loader
                .module_exports_cache
                .get(&id)
                .or_else(|| {
                    builder
                        .module_loader
                        .virtual_modules
                        .get(&id.to_string())
                        .map(|m| &m.exports)
                });
            if let Some(exports) = exports {
                let mut values: Vec<Symbol> = exports.value_names();
                let mut types: Vec<Symbol> = exports.type_names();
                let mut classes: Vec<Symbol> = exports.class_names();
                values.sort();
                types.sort();
                classes.sort();
                let _ = writeln!(&mut out, "- Values: {}", symbol_list(&values));
                let _ = writeln!(&mut out, "- Types: {}", symbol_list(&types));
                let _ = writeln!(&mut out, "- Classes: {}", symbol_list(&classes));
            } else {
                let _ = writeln!(&mut out, "- Exports: (none cached)");
            }
            let _ = writeln!(&mut out);
        }
    }

    let _ = writeln!(&mut out, "## ADTs");
    if builder.type_system.adts.is_empty() {
        let _ = writeln!(&mut out, "_No ADTs registered._");
        let _ = writeln!(&mut out);
    } else {
        let mut adts: Vec<&AdtDecl> = builder.type_system.adts.values().collect();
        adts.sort_by(|a, b| a.name.cmp(&b.name));
        for adt in adts {
            let params = if adt.params.is_empty() {
                "(none)".to_string()
            } else {
                adt.params
                    .iter()
                    .map(|p| format!("`{}`", p.name))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let _ = writeln!(&mut out, "### `{}`", adt.name);
            let _ = writeln!(&mut out, "- Parameters: {params}");
            if adt.variants.is_empty() {
                let _ = writeln!(&mut out, "- Variants: (none)");
            } else {
                let mut variants = adt.variants.clone();
                variants.sort_by(|a, b| a.name.cmp(&b.name));
                let _ = writeln!(&mut out, "- Variants:");
                for variant in variants {
                    if variant.args.is_empty() {
                        let _ = writeln!(&mut out, "  - `{}`", variant.name);
                    } else {
                        let args = variant
                            .args
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(", ");
                        let _ = writeln!(&mut out, "  - `{}`({args})", variant.name);
                    }
                }
            }
            let _ = writeln!(&mut out);
        }
    }

    let _ = writeln!(&mut out, "## Functions and Values");
    if builder.type_system.env.values.is_empty() {
        let _ = writeln!(&mut out, "_No values registered._");
        let _ = writeln!(&mut out);
    } else {
        let mut names: Vec<Symbol> = builder
            .type_system
            .env
            .values
            .iter()
            .map(|(name, _)| name.clone())
            .collect();
        names.sort();
        for name in names {
            if let Some(schemes) = builder.type_system.env.lookup(&name) {
                let mut scheme_strs: Vec<String> =
                    schemes.iter().map(|s| s.typ.to_string()).collect();
                scheme_strs.sort();
                scheme_strs.dedup();
                let joined = scheme_strs
                    .into_iter()
                    .map(|s| format!("`{s}`"))
                    .collect::<Vec<_>>()
                    .join(", ");
                let _ = writeln!(&mut out, "- `{name}`: {joined}");
            }
        }
        let _ = writeln!(&mut out);
    }

    let _ = writeln!(&mut out, "## Type Classes");
    if builder.type_system.classes.classes.is_empty() {
        let _ = writeln!(&mut out, "_No type classes registered._");
        let _ = writeln!(&mut out);
    } else {
        let mut class_names: Vec<Symbol> = builder
            .type_system
            .classes
            .classes
            .keys()
            .cloned()
            .collect();
        class_names.sort();
        for class_name in class_names {
            let supers = builder.type_system.classes.supers_of(&class_name);
            let mut supers_sorted = supers;
            supers_sorted.sort();
            let _ = writeln!(&mut out, "### `{class_name}`");
            let _ = writeln!(&mut out, "- Superclasses: {}", symbol_list(&supers_sorted));

            let mut methods: Vec<(Symbol, String)> = builder
                .type_system
                .class_methods
                .iter()
                .filter(|(_, info)| info.class == class_name)
                .map(|(name, info)| (name.clone(), info.scheme.typ.to_string()))
                .collect();
            methods.sort_by(|a, b| a.0.cmp(&b.0));
            if methods.is_empty() {
                let _ = writeln!(&mut out, "- Methods: (none)");
            } else {
                let _ = writeln!(&mut out, "- Methods:");
                for (method, scheme) in methods {
                    let _ = writeln!(&mut out, "  - `{method}`: `{scheme}`");
                }
            }

            let mut instances = builder
                .type_system
                .classes
                .instances
                .get(&class_name)
                .cloned()
                .unwrap_or_default();
            instances.sort_by_key(|a| a.head.typ.to_string());
            if instances.is_empty() {
                let _ = writeln!(&mut out, "- Instances: (none)");
            } else {
                let _ = writeln!(&mut out, "- Instances:");
                for instance in instances {
                    let ctx = if instance.context.is_empty() {
                        String::new()
                    } else {
                        let mut parts: Vec<String> = instance
                            .context
                            .iter()
                            .map(|pred| format!("{} {}", pred.class, pred.typ))
                            .collect();
                        parts.sort();
                        format!("({}) => ", parts.join(", "))
                    };
                    let _ = writeln!(
                        &mut out,
                        "  - `{}{} {}`",
                        ctx, instance.head.class, instance.head.typ
                    );
                }
            }
            let _ = writeln!(&mut out);
        }
    }

    let _ = writeln!(&mut out, "## Native Implementations");
    if builder.runtime.natives.entries.is_empty() {
        let _ = writeln!(&mut out, "_No native implementations registered._");
    } else {
        let mut native_names: Vec<Symbol> =
            builder.runtime.natives.entries.keys().cloned().collect();
        native_names.sort();
        for name in native_names {
            if let Some(impls) = builder.runtime.natives.get(&name) {
                let mut rows: Vec<(usize, String)> = impls
                    .iter()
                    .map(|imp| (imp.arity, imp.scheme.typ.to_string()))
                    .collect();
                rows.sort_by(|a, b| a.1.cmp(&b.1));
                let _ = writeln!(&mut out, "### `{name}`");
                for (arity, typ) in rows {
                    let _ = writeln!(&mut out, "- arity `{arity}`, type `{typ}`");
                }
                let _ = writeln!(&mut out);
            }
        }
    }

    out
}

fn render_virtual_module_source(decls: &[Decl]) -> Option<String> {
    let rendered = decls
        .iter()
        .filter_map(render_virtual_decl)
        .collect::<Vec<_>>()
        .join("\n");
    (!rendered.is_empty()).then_some(rendered)
}

fn render_virtual_decl(decl: &Decl) -> Option<String> {
    match decl {
        Decl::Type(td) => Some(render_type_decl(td)),
        Decl::DeclareFn(df) => Some(render_declare_fn_decl(df)),
        _ => None,
    }
}

fn render_type_decl(decl: &TypeDecl) -> String {
    let head = if decl.params.is_empty() {
        decl.name.to_string()
    } else {
        format!(
            "{} {}",
            decl.name,
            decl.params
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(" ")
        )
    };
    let variants = decl
        .variants
        .iter()
        .map(|variant| {
            if variant.args.is_empty() {
                variant.name.to_string()
            } else {
                format!(
                    "{} {}",
                    variant.name,
                    variant
                        .args
                        .iter()
                        .map(|arg| if matches!(arg, TypeExpr::Fun(..)) {
                            format!("({arg})")
                        } else {
                            arg.to_string()
                        })
                        .collect::<Vec<_>>()
                        .join(" ")
                )
            }
        })
        .collect::<Vec<_>>()
        .join(" | ");
    format!("pub type {head} = {variants}")
}

fn render_declare_fn_decl(decl: &DeclareFnDecl) -> String {
    let mut sig = decl.ret.clone();
    for (_, ann) in decl.params.iter().rev() {
        sig = TypeExpr::Fun(Span::default(), Box::new(ann.clone()), Box::new(sig));
    }
    let mut out = format!("pub declare fn {} : {}", decl.name.name, sig);
    if !decl.constraints.is_empty() {
        let preds = decl
            .constraints
            .iter()
            .map(|pred| format!("{} {}", pred.class, pred.typ))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(" where ");
        out.push_str(&preds);
    }
    out
}

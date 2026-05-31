use crate::prelude::*;
use crate::{completion::*, imports::*, shared::*};

pub(crate) fn goto_definition_response(
    session: &AnalysisSession,
    uri: &Url,
    text: &str,
    position: Position,
) -> Option<GotoDefinitionResponse> {
    // Parse on-demand. This keeps steady-state typing latency low; “go to
    // definition” is an explicit user action where a little work is fine.
    let Ok((tokens, program)) = session.tokenize_and_parse_cached(uri, text) else {
        return None;
    };

    let imported_projection = imported_projection_at_position(&tokens, position);

    let (ident, _token_span) = ident_token_at_position(&tokens, position)?;

    // If the cursor is on `alias.field` and `alias` is a local import, jump
    // to the exported declaration in the imported module.
    if let Some((alias, field)) = imported_projection
        && let Ok((_rewritten, _ts, imports, _diags)) =
            prepare_program_with_imports(session, uri, &program)
    {
        let alias_sym = Symbol::intern(&alias);
        if let Some(info) = imports.get(&alias_sym)
            && let Some(span) = info.export_defs.get(&field)
            && let Some(path) = info.path.as_ref()
            && let Some(module_uri) = url_from_file_path(path)
        {
            return Some(GotoDefinitionResponse::Scalar(Location {
                uri: module_uri,
                range: span_to_range(*span),
            }));
        }
    }

    let index = index_decl_spans(&program, &tokens);
    let pos = lsp_to_rex_position(position);

    // Pick the expression tree that actually contains the cursor. Top-level
    // instance method bodies are not part of `body_with_fns()`, so we have
    // to handle them explicitly.
    let body_with_fns = program.body_with_fns();
    let mut root_expr = body_with_fns.as_deref();
    for decl in &program.decls {
        let Decl::Instance(inst) = decl else {
            continue;
        };
        for method in &inst.methods {
            if position_in_span(pos, *method.body.span()) {
                root_expr = Some(method.body.as_ref());
                break;
            }
        }
    }

    let value_def = root_expr.and_then(|expr| {
        definition_span_for_value_ident(expr, pos, &ident, &mut Vec::new(), &tokens)
    });

    let instance_method_def = index
        .instance_method_defs
        .iter()
        .find_map(|(span, methods)| {
            if position_in_span(pos, *span) {
                methods.get(&ident).copied()
            } else {
                None
            }
        });

    let target_span = value_def
        .or(instance_method_def)
        .or(index.class_method_defs.get(&ident).copied())
        .or(index.fn_defs.get(&ident).copied())
        .or(index.ctor_defs.get(&ident).copied())
        .or(index.type_defs.get(&ident).copied())
        .or(index.class_defs.get(&ident).copied())?;

    Some(GotoDefinitionResponse::Scalar(Location {
        uri: uri.clone(),
        range: span_to_range(target_span),
    }))
}

pub(crate) fn range_to_span(range: Range) -> Span {
    Span::new(
        (range.start.line + 1) as usize,
        (range.start.character + 1) as usize,
        (range.end.line + 1) as usize,
        (range.end.character + 1) as usize,
    )
}

pub(crate) fn pattern_bindings_with_spans(pat: &Pattern, out: &mut Vec<(String, Span)>) {
    match pat {
        Pattern::Var(var) => out.push((var.name.to_string(), var.span)),
        Pattern::Named(_, _, args) => {
            for arg in args {
                pattern_bindings_with_spans(arg, out);
            }
        }
        Pattern::Tuple(_, elems) | Pattern::List(_, elems) => {
            for elem in elems {
                pattern_bindings_with_spans(elem, out);
            }
        }
        Pattern::Cons(_, head, tail) => {
            pattern_bindings_with_spans(head, out);
            pattern_bindings_with_spans(tail, out);
        }
        Pattern::Dict(_, fields) => {
            for (_, pat) in fields {
                pattern_bindings_with_spans(pat, out);
            }
        }
        Pattern::Wildcard(..) => {}
    }
}

pub(crate) fn collect_references_in_expr(
    expr: &Expr,
    ident: &str,
    target_span: Span,
    uri: &Url,
    top_level_defs: &HashMap<String, Span>,
    scope: &mut Vec<(String, Span)>,
    out: &mut Vec<Location>,
) {
    match expr {
        Expr::Var(var) => {
            if var.name.as_ref() != ident {
                return;
            }
            let resolved = scope
                .iter()
                .rev()
                .find_map(|(name, span)| (name == ident).then_some(*span))
                .or_else(|| top_level_defs.get(ident).copied());
            if resolved.is_some_and(|span| span == target_span) {
                out.push(Location {
                    uri: uri.clone(),
                    range: span_to_range(var.span),
                });
            }
        }
        Expr::Let(_, var, _, _ann, def, body) => {
            collect_references_in_expr(def, ident, target_span, uri, top_level_defs, scope, out);
            scope.push((var.name.to_string(), var.span));
            collect_references_in_expr(body, ident, target_span, uri, top_level_defs, scope, out);
            scope.pop();
        }
        Expr::LetRec(_, bindings, body) => {
            let base_len = scope.len();
            for (var, _, _ann, _def) in bindings {
                scope.push((var.name.to_string(), var.span));
            }
            for (_var, _, _ann, def) in bindings {
                collect_references_in_expr(
                    def,
                    ident,
                    target_span,
                    uri,
                    top_level_defs,
                    scope,
                    out,
                );
            }
            collect_references_in_expr(body, ident, target_span, uri, top_level_defs, scope, out);
            scope.truncate(base_len);
        }
        Expr::Lam(_, _scope, param, _ann, _constraints, body) => {
            scope.push((param.name.to_string(), param.span));
            collect_references_in_expr(body, ident, target_span, uri, top_level_defs, scope, out);
            scope.pop();
        }
        Expr::Match(_, scrutinee, arms) => {
            collect_references_in_expr(
                scrutinee,
                ident,
                target_span,
                uri,
                top_level_defs,
                scope,
                out,
            );
            for (pat, arm) in arms {
                let base_len = scope.len();
                let mut binds = Vec::new();
                pattern_bindings_with_spans(pat, &mut binds);
                scope.extend(binds);
                collect_references_in_expr(
                    arm,
                    ident,
                    target_span,
                    uri,
                    top_level_defs,
                    scope,
                    out,
                );
                scope.truncate(base_len);
            }
        }
        Expr::App(_, fun, arg) => {
            collect_references_in_expr(fun, ident, target_span, uri, top_level_defs, scope, out);
            collect_references_in_expr(arg, ident, target_span, uri, top_level_defs, scope, out);
        }
        Expr::Project(_, base, _) => {
            collect_references_in_expr(base, ident, target_span, uri, top_level_defs, scope, out);
        }
        Expr::Tuple(_, elems) | Expr::List(_, elems) => {
            for elem in elems {
                collect_references_in_expr(
                    elem,
                    ident,
                    target_span,
                    uri,
                    top_level_defs,
                    scope,
                    out,
                );
            }
        }
        Expr::Dict(_, entries) => {
            for value in entries.values() {
                collect_references_in_expr(
                    value,
                    ident,
                    target_span,
                    uri,
                    top_level_defs,
                    scope,
                    out,
                );
            }
        }
        Expr::RecordUpdate(_, base, updates) => {
            collect_references_in_expr(base, ident, target_span, uri, top_level_defs, scope, out);
            for value in updates.values() {
                collect_references_in_expr(
                    value,
                    ident,
                    target_span,
                    uri,
                    top_level_defs,
                    scope,
                    out,
                );
            }
        }
        Expr::Ite(_, cond, then_expr, else_expr) => {
            collect_references_in_expr(cond, ident, target_span, uri, top_level_defs, scope, out);
            collect_references_in_expr(
                then_expr,
                ident,
                target_span,
                uri,
                top_level_defs,
                scope,
                out,
            );
            collect_references_in_expr(
                else_expr,
                ident,
                target_span,
                uri,
                top_level_defs,
                scope,
                out,
            );
        }
        Expr::Ann(_, inner, _) => {
            collect_references_in_expr(inner, ident, target_span, uri, top_level_defs, scope, out);
        }
        Expr::Bool(..)
        | Expr::Uint(..)
        | Expr::Int(..)
        | Expr::Float(..)
        | Expr::String(..)
        | Expr::Uuid(..)
        | Expr::DateTime(..)
        | Expr::Hole(..) => {}
    }
}

pub(crate) fn references_for_source(
    session: &AnalysisSession,
    uri: &Url,
    text: &str,
    position: Position,
    include_declaration: bool,
) -> Vec<Location> {
    let Ok((tokens, program)) = session.tokenize_and_parse_cached(uri, text) else {
        return Vec::new();
    };
    let Some((ident, _token_span)) = ident_token_at_position(&tokens, position) else {
        return Vec::new();
    };

    let Some(def_response) = goto_definition_response(session, uri, text, position) else {
        return Vec::new();
    };
    let GotoDefinitionResponse::Scalar(def_location) = def_response else {
        return Vec::new();
    };
    if def_location.uri != *uri {
        return Vec::new();
    }
    let target_span = range_to_span(def_location.range);

    let index = index_decl_spans(&program, &tokens);
    let mut top_level_defs = index.fn_defs;
    top_level_defs.extend(index.ctor_defs);

    let mut refs = Vec::new();
    if include_declaration {
        refs.push(def_location);
    }
    if let Some(expr) = program.body_with_fns() {
        collect_references_in_expr(
            expr.as_ref(),
            &ident,
            target_span,
            uri,
            &top_level_defs,
            &mut Vec::new(),
            &mut refs,
        );
    }
    refs.sort_by_key(|location| {
        (
            location.range.start.line,
            location.range.start.character,
            location.range.end.line,
            location.range.end.character,
        )
    });
    refs.dedup_by(|a, b| a.range == b.range && a.uri == b.uri);
    refs
}

pub(crate) fn rename_for_source(
    session: &AnalysisSession,
    uri: &Url,
    text: &str,
    position: Position,
    new_name: &str,
) -> Option<WorkspaceEdit> {
    if !is_ident_like(new_name) {
        return None;
    }
    let refs = references_for_source(session, uri, text, position, true);
    if refs.is_empty() {
        return None;
    }
    let edits: Vec<TextEdit> = refs
        .into_iter()
        .map(|location| TextEdit {
            range: location.range,
            new_text: new_name.to_string(),
        })
        .collect();
    let mut changes = HashMap::new();
    changes.insert(uri.clone(), edits);
    Some(WorkspaceEdit {
        changes: Some(changes),
        document_changes: None,
        change_annotations: None,
    })
}

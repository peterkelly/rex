use crate::prelude::*;
use crate::{code_actions::*, completion::*, imports::*, queries::*, shared::*};

pub fn diagnostics_from_text(uri: &Url, text: &str) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    match tokenize_and_parse_cached(uri, text) {
        Ok((tokens, program)) => {
            push_comment_diagnostics(&tokens, &mut diagnostics);
            if diagnostics.len() < MAX_DIAGNOSTICS {
                push_type_diagnostics(uri, text, &program, &mut diagnostics);
            }
        }
        Err(TokenizeOrParseError::Lex(err)) => {
            diagnostics.push(diagnostic_for_lexical_error(&err));
        }
        Err(TokenizeOrParseError::Parse(errors)) => {
            for err in errors {
                diagnostics.push(diagnostic_for_span(err.span, err.message));
                if diagnostics.len() >= MAX_DIAGNOSTICS {
                    break;
                }
            }
        }
    }

    diagnostics
}

pub(crate) fn diagnostic_for_lexical_error(err: &LexicalError) -> Diagnostic {
    let (span, message) = match err {
        LexicalError::UnexpectedToken(span) => (*span, "Unexpected token".to_string()),
        LexicalError::UnclosedBlockComment(span) => {
            (*span, "Unclosed block comment opener (/*).".to_string())
        }
        LexicalError::UnmatchedBlockCommentClose(span) => {
            (*span, "Unmatched block comment closer (*/).".to_string())
        }
        LexicalError::InvalidLiteral {
            kind,
            text,
            error,
            span,
        } => (*span, format!("invalid {kind} literal `{text}`: {error}")),
        LexicalError::Internal(msg) => (
            Span::new(1, 1, 1, 1),
            format!("internal lexer error: {msg}"),
        ),
    };
    diagnostic_for_span(span, message)
}

pub(crate) fn name_token_at_position(
    tokens: &Tokens,
    position: Position,
) -> Option<(String, Span, bool)> {
    for token in &tokens.items {
        let (name, span, is_ident) = match token {
            Token::Ident(name, span, ..) => (name.clone(), *span, true),
            Token::Add(span) => ("+".to_string(), *span, false),
            Token::Sub(span) => ("-".to_string(), *span, false),
            Token::Mul(span) => ("*".to_string(), *span, false),
            Token::Div(span) => ("/".to_string(), *span, false),
            Token::Mod(span) => ("%".to_string(), *span, false),
            Token::Concat(span) => ("++".to_string(), *span, false),
            Token::Eq(span) => ("==".to_string(), *span, false),
            Token::Ne(span) => ("!=".to_string(), *span, false),
            Token::Lt(span) => ("<".to_string(), *span, false),
            Token::Le(span) => ("<=".to_string(), *span, false),
            Token::Gt(span) => (">".to_string(), *span, false),
            Token::Ge(span) => (">=".to_string(), *span, false),
            Token::And(span) => ("&&".to_string(), *span, false),
            Token::Or(span) => ("||".to_string(), *span, false),
            _ => continue,
        };
        if range_touches_position(span_to_range(span), position) {
            return Some((name, span, is_ident));
        }
    }
    None
}

pub(crate) fn push_comment_diagnostics(tokens: &Tokens, diagnostics: &mut Vec<Diagnostic>) {
    let mut index = 0;

    while index < tokens.items.len() && diagnostics.len() < MAX_DIAGNOSTICS {
        match tokens.items[index] {
            Token::CommentL(span) => {
                let mut cursor = index + 1;
                while cursor < tokens.items.len() {
                    if matches!(tokens.items[cursor], Token::CommentR(_)) {
                        break;
                    }
                    cursor += 1;
                }

                if cursor >= tokens.items.len() {
                    diagnostics.push(diagnostic_for_span(
                        span,
                        "Unclosed block comment opener (/*).",
                    ));
                    break;
                }

                index = cursor + 1;
            }
            Token::CommentR(span) => {
                diagnostics.push(diagnostic_for_span(
                    span,
                    "Unmatched block comment closer (*/).",
                ));
                index += 1;
            }
            _ => index += 1,
        }
    }
}

pub(crate) fn diagnostic_for_span(span: Span, message: impl Into<String>) -> Diagnostic {
    Diagnostic {
        range: span_to_range(span),
        severity: Some(DiagnosticSeverity::ERROR),
        message: message.into(),
        source: Some("rex-lsp".to_string()),
        ..Diagnostic::default()
    }
}

pub(crate) fn push_type_diagnostics(
    uri: &Url,
    text: &str,
    compilation_unit: &CompilationUnit,
    diagnostics: &mut Vec<Diagnostic>,
) {
    // Type inference is meaningfully more expensive than lex/parse, and we run
    // diagnostics on every full-text change. Keep the cost model explicit.
    const MAX_TYPECHECK_BYTES: usize = 256 * 1024;
    if text.len() > MAX_TYPECHECK_BYTES {
        return;
    }

    let mut engine = match Engine::with_prelude(()) {
        Ok(engine) => engine,
        Err(err) => {
            push_engine_error(err, diagnostics, compilation_unit);
            return;
        }
    };

    let result = if let Some(path) = uri_to_file_path(uri) {
        engine.add_importer("lsp-modules", Arc::new(LspModuleService::current()));
        futures::executor::block_on(engine.infer_snippet_at(text, path))
    } else {
        futures::executor::block_on(engine.infer_snippet(text))
    };

    if let Err(err) = result {
        push_engine_error(err.into_engine_error(), diagnostics, compilation_unit);
        return;
    }

    push_hole_diagnostics(compilation_unit, diagnostics);
}

pub(crate) fn push_engine_error(
    err: EngineError,
    diagnostics: &mut Vec<Diagnostic>,
    compilation_unit: &CompilationUnit,
) {
    match err {
        EngineError::Type(err) => {
            let expr = compilation_unit.body_with_fns();
            let before = diagnostics.len();
            push_ts_error(err, diagnostics, expr.as_deref(), None, None);
            if let Some(primary) = diagnostics.get(before).cloned()
                && let Some(expr) = expr.as_deref()
            {
                push_additional_default_record_update_ambiguity_diagnostics(
                    expr,
                    &primary.message,
                    diagnostics,
                );
            }
        }
        EngineError::Module(module_err) => {
            push_module_error(&module_err, diagnostics, compilation_unit);
        }
        other => {
            diagnostics.push(diagnostic_for_span(
                primary_program_span(compilation_unit),
                other.to_string(),
            ));
        }
    }
}

pub(crate) fn push_module_error(
    err: &ModuleError,
    diagnostics: &mut Vec<Diagnostic>,
    compilation_unit: &CompilationUnit,
) {
    match err {
        ModuleError::Lex { source } => {
            diagnostics.push(diagnostic_for_lexical_error(source));
        }
        ModuleError::Parse { errors } => {
            for err in errors {
                diagnostics.push(diagnostic_for_span(err.span, err.message.clone()));
                if diagnostics.len() >= MAX_DIAGNOSTICS {
                    break;
                }
            }
        }
        _ => {
            diagnostics.push(diagnostic_for_span(
                primary_program_span(compilation_unit),
                err.to_string(),
            ));
        }
    }
}

pub(crate) fn primary_program_span(compilation_unit: &CompilationUnit) -> Span {
    match compilation_unit.decls.first() {
        Some(Decl::Type(d)) => d.span,
        Some(Decl::Fn(d)) => d.span,
        Some(Decl::DeclareFn(d)) => d.span,
        Some(Decl::Import(d)) => d.span,
        Some(Decl::Class(d)) => d.span,
        Some(Decl::Instance(d)) => d.span,
        None => compilation_unit
            .body
            .as_deref()
            .map(|expr| *expr.span())
            .unwrap_or_default(),
    }
}

pub(crate) fn push_hole_diagnostics(
    compilation_unit: &CompilationUnit,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(body) = compilation_unit.body_with_fns() else {
        return;
    };
    let mut spans = Vec::new();
    collect_hole_spans(body.as_ref(), &mut spans);
    spans.sort_unstable_by_key(|s| (s.begin.line, s.begin.column, s.end.line, s.end.column));
    spans.dedup();

    for span in spans {
        if diagnostics.len() >= MAX_DIAGNOSTICS {
            break;
        }
        diagnostics.push(Diagnostic {
            range: span_to_range(span),
            severity: Some(DiagnosticSeverity::ERROR),
            message: "typed hole `?` must be filled before evaluation".to_string(),
            source: Some("rex-typesystem".to_string()),
            ..Diagnostic::default()
        });
    }
}

pub(crate) fn unknown_var_name(err: &TsTypeError) -> Option<Symbol> {
    match err {
        TsTypeError::UnknownVar(name) => Some(name.clone()),
        TsTypeError::Spanned { error, .. } => unknown_var_name(error),
        _ => None,
    }
}

pub(crate) fn field_not_definitely_available_tail(message: &str) -> Option<(&str, &str)> {
    let rest = message.strip_prefix("field `")?;
    let (field, tail) = rest.split_once('`')?;
    tail.contains("is not definitely available on")
        .then_some((field, tail))
}

pub(crate) fn push_additional_default_record_update_ambiguity_diagnostics(
    expr: &Expr,
    primary_message: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some((_field, tail)) = field_not_definitely_available_tail(primary_message) else {
        return;
    };
    let mut updates = Vec::new();
    collect_default_record_updates(expr, &mut updates);
    for (span, fields) in updates {
        if diagnostics.len() >= MAX_DIAGNOSTICS {
            break;
        }
        let Some(field) = fields.first() else {
            continue;
        };
        let message = format!("field `{field}`{tail}");
        let range = span_to_range(span);
        if diagnostics
            .iter()
            .any(|d| d.range == range && d.message == message)
        {
            continue;
        }
        diagnostics.push(Diagnostic {
            range,
            severity: Some(DiagnosticSeverity::ERROR),
            message,
            source: Some("rex-typesystem".to_string()),
            ..Diagnostic::default()
        });
    }
}

pub(crate) fn collect_default_record_updates(expr: &Expr, out: &mut Vec<(Span, Vec<String>)>) {
    match expr {
        Expr::RecordUpdate(span, base, updates) => {
            if matches!(base.as_ref(), Expr::Var(v) if v.name.as_ref() == "default") {
                let fields = updates
                    .keys()
                    .map(|name| name.as_ref().to_string())
                    .collect::<Vec<_>>();
                if !fields.is_empty() {
                    out.push((*span, fields));
                }
            }
            collect_default_record_updates(base, out);
            for value in updates.values() {
                collect_default_record_updates(value, out);
            }
        }
        Expr::App(_, fun, arg) => {
            collect_default_record_updates(fun, out);
            collect_default_record_updates(arg, out);
        }
        Expr::Project(_, base, _) => collect_default_record_updates(base, out),
        Expr::Lam(_, _, _, _, _, body) => collect_default_record_updates(body, out),
        Expr::Let(_, _, _, def, body) => {
            collect_default_record_updates(def, out);
            collect_default_record_updates(body, out);
        }
        Expr::LetRec(_, bindings, body) => {
            for (_var, _ann, def) in bindings {
                collect_default_record_updates(def, out);
            }
            collect_default_record_updates(body, out);
        }
        Expr::Ite(_, cond, then_expr, else_expr) => {
            collect_default_record_updates(cond, out);
            collect_default_record_updates(then_expr, out);
            collect_default_record_updates(else_expr, out);
        }
        Expr::Match(_, scrutinee, arms) => {
            collect_default_record_updates(scrutinee, out);
            for (_pat, arm) in arms {
                collect_default_record_updates(arm, out);
            }
        }
        Expr::Ann(_, inner, _) => collect_default_record_updates(inner, out),
        Expr::Tuple(_, items) | Expr::List(_, items) => {
            for item in items {
                collect_default_record_updates(item, out);
            }
        }
        Expr::Dict(_, entries) => {
            for value in entries.values() {
                collect_default_record_updates(value, out);
            }
        }
        Expr::Var(..)
        | Expr::Bool(..)
        | Expr::Uint(..)
        | Expr::Int(..)
        | Expr::Float(..)
        | Expr::String(..)
        | Expr::Uuid(..)
        | Expr::DateTime(..)
        | Expr::Hole(..) => {}
    }
}

pub(crate) fn find_let_binding_for_def_range(
    compilation_unit: &CompilationUnit,
    target: Range,
) -> Option<(String, Position)> {
    let body = compilation_unit.body_with_fns()?;
    find_let_binding_for_def_range_in_expr(body.as_ref(), target)
}

pub(crate) fn find_let_binding_for_def_range_in_expr(
    expr: &Expr,
    target: Range,
) -> Option<(String, Position)> {
    match expr {
        Expr::Let(_, var, ann, def, body) => {
            let def_range = span_to_range(*def.span());
            if ranges_overlap(def_range, target) && ann.is_none() {
                return Some((var.name.as_ref().to_string(), span_to_range(var.span).end));
            }
            find_let_binding_for_def_range_in_expr(def.as_ref(), target)
                .or_else(|| find_let_binding_for_def_range_in_expr(body.as_ref(), target))
        }
        Expr::LetRec(_, bindings, body) => {
            for (var, ann, def) in bindings {
                let def_range = span_to_range(*def.span());
                if ranges_overlap(def_range, target) && ann.is_none() {
                    return Some((var.name.as_ref().to_string(), span_to_range(var.span).end));
                }
                if let Some(found) = find_let_binding_for_def_range_in_expr(def.as_ref(), target) {
                    return Some(found);
                }
            }
            find_let_binding_for_def_range_in_expr(body.as_ref(), target)
        }
        Expr::App(_, fun, arg) => find_let_binding_for_def_range_in_expr(fun.as_ref(), target)
            .or_else(|| find_let_binding_for_def_range_in_expr(arg.as_ref(), target)),
        Expr::Project(_, base, _) => find_let_binding_for_def_range_in_expr(base.as_ref(), target),
        Expr::RecordUpdate(_, base, updates) => {
            find_let_binding_for_def_range_in_expr(base.as_ref(), target).or_else(|| {
                updates
                    .values()
                    .find_map(|expr| find_let_binding_for_def_range_in_expr(expr.as_ref(), target))
            })
        }
        Expr::Lam(_, _, _, _, _, body) => {
            find_let_binding_for_def_range_in_expr(body.as_ref(), target)
        }
        Expr::Ite(_, cond, then_expr, else_expr) => {
            find_let_binding_for_def_range_in_expr(cond.as_ref(), target)
                .or_else(|| find_let_binding_for_def_range_in_expr(then_expr.as_ref(), target))
                .or_else(|| find_let_binding_for_def_range_in_expr(else_expr.as_ref(), target))
        }
        Expr::Match(_, scrutinee, arms) => {
            find_let_binding_for_def_range_in_expr(scrutinee.as_ref(), target).or_else(|| {
                arms.iter().find_map(|(_, arm)| {
                    find_let_binding_for_def_range_in_expr(arm.as_ref(), target)
                })
            })
        }
        Expr::Ann(_, inner, _) => find_let_binding_for_def_range_in_expr(inner.as_ref(), target),
        Expr::Tuple(_, items) | Expr::List(_, items) => items
            .iter()
            .find_map(|item| find_let_binding_for_def_range_in_expr(item.as_ref(), target)),
        Expr::Dict(_, entries) => entries
            .values()
            .find_map(|value| find_let_binding_for_def_range_in_expr(value.as_ref(), target)),
        Expr::Var(..)
        | Expr::Bool(..)
        | Expr::Uint(..)
        | Expr::Int(..)
        | Expr::Float(..)
        | Expr::String(..)
        | Expr::Uuid(..)
        | Expr::DateTime(..)
        | Expr::Hole(..) => None,
    }
}

pub(crate) fn collect_unbound_var_spans(
    expr: &Expr,
    target: &Symbol,
    bound: &mut Vec<Symbol>,
    out: &mut Vec<Span>,
) {
    match expr {
        Expr::Var(var) => {
            if var.name == *target && !bound.iter().any(|name| name == &var.name) {
                out.push(var.span);
            }
        }
        Expr::App(_, fun, arg) => {
            collect_unbound_var_spans(fun, target, bound, out);
            collect_unbound_var_spans(arg, target, bound, out);
        }
        Expr::Project(_, base, _) => {
            collect_unbound_var_spans(base, target, bound, out);
        }
        Expr::Lam(_, _scope, param, _ann, _constraints, body) => {
            bound.push(param.name.clone());
            collect_unbound_var_spans(body, target, bound, out);
            bound.pop();
        }
        Expr::Let(_, var, _ann, def, body) => {
            collect_unbound_var_spans(def, target, bound, out);
            bound.push(var.name.clone());
            collect_unbound_var_spans(body, target, bound, out);
            bound.pop();
        }
        Expr::LetRec(_, bindings, body) => {
            let base_len = bound.len();
            for (var, _ann, _def) in bindings {
                bound.push(var.name.clone());
            }
            for (_var, _ann, def) in bindings {
                collect_unbound_var_spans(def, target, bound, out);
            }
            collect_unbound_var_spans(body, target, bound, out);
            bound.truncate(base_len);
        }
        Expr::Ite(_, cond, then_expr, else_expr) => {
            collect_unbound_var_spans(cond, target, bound, out);
            collect_unbound_var_spans(then_expr, target, bound, out);
            collect_unbound_var_spans(else_expr, target, bound, out);
        }
        Expr::Match(_, scrutinee, arms) => {
            collect_unbound_var_spans(scrutinee, target, bound, out);
            for (pat, arm) in arms {
                let base_len = bound.len();
                let mut pat_bindings = Vec::new();
                collect_pattern_bindings(pat, &mut pat_bindings);
                bound.extend(pat_bindings);
                collect_unbound_var_spans(arm, target, bound, out);
                bound.truncate(base_len);
            }
        }
        Expr::Ann(_, inner, _) => {
            collect_unbound_var_spans(inner, target, bound, out);
        }
        Expr::Tuple(_, items) | Expr::List(_, items) => {
            for item in items {
                collect_unbound_var_spans(item, target, bound, out);
            }
        }
        Expr::Dict(_, kvs) | Expr::RecordUpdate(_, _, kvs) => {
            for expr in kvs.values() {
                collect_unbound_var_spans(expr, target, bound, out);
            }
            if let Expr::RecordUpdate(_, base, _) = expr {
                collect_unbound_var_spans(base, target, bound, out);
            }
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

pub(crate) fn push_ts_error(
    err: TsTypeError,
    diagnostics: &mut Vec<Diagnostic>,
    expr: Option<&Expr>,
    ts: Option<&TypeSystem>,
    fallback_span: Option<Span>,
) {
    let unknown_target = unknown_var_name(&err);
    let (span, message) = match &err {
        TsTypeError::Spanned { span, error } => (*span, error.to_string()),
        other => (
            fallback_span
                .or_else(|| expr.map(|e| *e.span()))
                .unwrap_or_default(),
            other.to_string(),
        ),
    };

    if let (Some(target), Some(expr)) = (unknown_target, expr)
        && ts.is_none_or(|ts| ts.env.lookup(&target).is_none())
    {
        let mut spans = Vec::new();
        collect_unbound_var_spans(expr, &target, &mut Vec::new(), &mut spans);
        spans.sort_unstable_by_key(|s| (s.begin.line, s.begin.column, s.end.line, s.end.column));
        spans.dedup();
        if !spans.is_empty() {
            for unbound_span in spans {
                if diagnostics.len() >= MAX_DIAGNOSTICS {
                    break;
                }
                diagnostics.push(Diagnostic {
                    range: span_to_range(unbound_span),
                    severity: Some(DiagnosticSeverity::ERROR),
                    message: message.clone(),
                    source: Some("rex-typesystem".to_string()),
                    ..Diagnostic::default()
                });
            }
            return;
        }
    }

    diagnostics.push(Diagnostic {
        range: span_to_range(span),
        severity: Some(DiagnosticSeverity::ERROR),
        message,
        source: Some("rex-typesystem".to_string()),
        ..Diagnostic::default()
    });
}

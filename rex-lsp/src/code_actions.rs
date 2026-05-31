use crate::prelude::*;
use crate::{completion::*, diagnostics::*, imports::*, queries::*, shared::*};

pub fn code_actions_for_source(
    session: &AnalysisSession,
    uri: &Url,
    text: &str,
    request_range: Range,
    diagnostics: &[Diagnostic],
) -> Vec<CodeActionOrCommand> {
    let parsed = session
        .tokenize_and_parse_cached(uri, text)
        .ok()
        .map(|(_tokens, program)| program);
    let mut actions = Vec::new();

    // Hole fill is position-driven and should be available even when other diagnostics exist.
    actions.extend(code_actions_for_hole_fill(
        session,
        uri,
        text,
        parsed.as_ref(),
        request_range,
    ));

    for diag in diagnostics {
        let usable_diag_range = range_is_usable_for_text(text, diag.range);
        if usable_diag_range
            && !range_is_empty(diag.range)
            && !ranges_overlap(diag.range, request_range)
            && !range_contains_position(diag.range, request_range.start)
            && !range_contains_position(diag.range, request_range.end)
        {
            continue;
        }
        actions.extend(code_actions_for_diagnostic(
            session,
            uri,
            text,
            parsed.as_ref(),
            request_range,
            diag,
        ));
    }

    actions
}

pub(crate) fn code_actions_for_hole_fill(
    session: &AnalysisSession,
    uri: &Url,
    text: &str,
    compilation_unit: Option<&CompilationUnit>,
    request_range: Range,
) -> Vec<CodeActionOrCommand> {
    let Some(compilation_unit) = compilation_unit else {
        return Vec::new();
    };
    let Some(body) = compilation_unit.body_with_fns() else {
        return Vec::new();
    };
    let mut hole_spans = Vec::new();
    collect_hole_spans(body.as_ref(), &mut hole_spans);
    let Some(hole_span) = hole_spans
        .into_iter()
        .find(|span| ranges_overlap(span_to_range(*span), request_range))
    else {
        return Vec::new();
    };
    let hole_range = span_to_range(hole_span);
    let pos = hole_range.start;
    let candidates = hole_fill_candidates_at_position(session, uri, text, pos);
    let mut actions = Vec::new();
    for (name, replacement) in candidates.into_iter().take(8) {
        let diagnostic = Diagnostic {
            range: hole_range,
            severity: Some(DiagnosticSeverity::HINT),
            message: "hole".to_string(),
            source: Some("rex-lsp".to_string()),
            ..Diagnostic::default()
        };
        actions.push(code_action_replace(
            format!("Fill hole with `{name}`"),
            uri,
            hole_range,
            replacement,
            diagnostic,
        ));
    }
    actions
}

pub(crate) fn code_actions_for_diagnostic(
    session: &AnalysisSession,
    uri: &Url,
    text: &str,
    compilation_unit: Option<&CompilationUnit>,
    request_range: Range,
    diagnostic: &Diagnostic,
) -> Vec<CodeActionOrCommand> {
    let mut actions = Vec::new();
    let target_range = if range_is_usable_for_text(text, diagnostic.range) {
        diagnostic.range
    } else {
        request_range
    };

    if diagnostic
        .message
        .contains("typed hole `?` must be filled before evaluation")
    {
        actions.extend(code_actions_for_hole_fill(
            session,
            uri,
            text,
            compilation_unit,
            target_range,
        ));
    }

    if let Some(name) = unknown_var_name_from_message(&diagnostic.message) {
        if let Some(compilation_unit) = compilation_unit {
            let mut candidates: Vec<String> =
                values_in_scope_at_position(compilation_unit, target_range.start)
                    .into_keys()
                    .filter(|candidate| candidate != name)
                    .collect();
            candidates.sort_by_key(|candidate| levenshtein_distance(candidate, name));
            for candidate in candidates.into_iter().take(3) {
                actions.push(code_action_replace(
                    format!("Replace `{name}` with `{candidate}`"),
                    uri,
                    target_range,
                    candidate,
                    diagnostic.clone(),
                ));
            }
        }

        actions.push(code_action_insert(
            format!("Introduce `let {name} = null`"),
            uri,
            Position {
                line: 0,
                character: 0,
            },
            format!("let {name} = null in\n"),
            diagnostic.clone(),
        ));
    }

    if is_list_scalar_unification_error(&diagnostic.message)
        && let Some(selected) = text_for_range(text, target_range)
    {
        let trimmed = selected.trim();
        if !trimmed.is_empty() {
            actions.push(code_action_replace(
                "Wrap expression in list literal".to_string(),
                uri,
                target_range,
                format!("[{selected}]"),
                diagnostic.clone(),
            ));
            if trimmed.starts_with('[') && trimmed.ends_with(']') && trimmed.len() >= 2 {
                let unwrapped = trimmed[1..trimmed.len() - 1].to_string();
                actions.push(code_action_replace(
                    "Unwrap list literal".to_string(),
                    uri,
                    target_range,
                    unwrapped,
                    diagnostic.clone(),
                ));
            }
        }
    }

    if is_array_list_unification_error(&diagnostic.message) {
        let selected_range =
            if !range_is_empty(request_range) && range_is_usable_for_text(text, request_range) {
                request_range
            } else {
                target_range
            };
        if let Some(selected) = text_for_range(text, selected_range) {
            let trimmed = selected.trim();
            if !trimmed.is_empty() && !trimmed.starts_with("to_list") {
                actions.push(code_action_replace(
                    "Convert expression to list with `to_list`".to_string(),
                    uri,
                    selected_range,
                    format!("to_list ({selected})"),
                    diagnostic.clone(),
                ));
            }
        }
    }

    if is_function_value_unification_error(&diagnostic.message)
        && let Some(selected) = text_for_range(text, target_range)
    {
        let trimmed = selected.trim();
        if !trimmed.is_empty() {
            actions.push(code_action_replace(
                "Apply expression to missing argument".to_string(),
                uri,
                target_range,
                format!("({selected} null)"),
                diagnostic.clone(),
            ));
            actions.push(code_action_replace(
                "Wrap expression in lambda".to_string(),
                uri,
                target_range,
                format!("(\\_ -> {selected})"),
                diagnostic.clone(),
            ));
        }
    }

    if diagnostic.message.starts_with("non-exhaustive match for ") {
        let (insert_pos, new_text) = wildcard_match_arm_insert(text, diagnostic.range)
            .unwrap_or_else(|| {
                let newline = if diagnostic.range.start.line == diagnostic.range.end.line {
                    " "
                } else {
                    "\n"
                };
                (diagnostic.range.end, format!("{newline}case _ -> null;"))
            });
        actions.push(code_action_insert(
            "Add wildcard arm to match".to_string(),
            uri,
            insert_pos,
            new_text,
            diagnostic.clone(),
        ));
    }

    if let Some(field) = field_not_definitely_available_from_message(&diagnostic.message)
        && let Some(compilation_unit) = compilation_unit
        && let Some(selected) = text_for_range(text, target_range)
    {
        let candidates = default_record_candidates_for_field(compilation_unit, field);
        for ty_name in &candidates {
            if let Some(new_text) = replace_first_default_with_is(&selected, ty_name) {
                actions.push(code_action_replace(
                    format!("Disambiguate `default` as `{ty_name}`"),
                    uri,
                    target_range,
                    new_text,
                    diagnostic.clone(),
                ));
            }
        }

        if let Some((binding_name, insert_pos)) =
            find_let_binding_for_def_range(compilation_unit, target_range)
        {
            for ty_name in &candidates {
                actions.push(code_action_insert(
                    format!("Annotate `{binding_name}` as `{ty_name}`"),
                    uri,
                    insert_pos,
                    format!(": {ty_name}"),
                    diagnostic.clone(),
                ));
            }
        }
    }

    actions
}

pub(crate) fn code_action_replace(
    title: String,
    uri: &Url,
    range: Range,
    new_text: String,
    diagnostic: Diagnostic,
) -> CodeActionOrCommand {
    code_action_with_edit(title, uri, TextEdit { range, new_text }, diagnostic)
}

pub(crate) fn code_action_insert(
    title: String,
    uri: &Url,
    position: Position,
    new_text: String,
    diagnostic: Diagnostic,
) -> CodeActionOrCommand {
    code_action_with_edit(
        title,
        uri,
        TextEdit {
            range: Range {
                start: position,
                end: position,
            },
            new_text,
        },
        diagnostic,
    )
}

pub(crate) fn code_action_with_edit(
    title: String,
    uri: &Url,
    edit: TextEdit,
    diagnostic: Diagnostic,
) -> CodeActionOrCommand {
    let mut changes = HashMap::new();
    changes.insert(uri.clone(), vec![edit]);
    CodeActionOrCommand::CodeAction(CodeAction {
        title,
        kind: Some(CodeActionKind::QUICKFIX),
        diagnostics: Some(vec![diagnostic]),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            document_changes: None,
            change_annotations: None,
        }),
        command: None,
        is_preferred: Some(true),
        disabled: None,
        data: None,
    })
}

pub(crate) fn text_for_range(text: &str, range: Range) -> Option<String> {
    let start = offset_at(text, range.start)?;
    let end = offset_at(text, range.end)?;
    (start <= end && end <= text.len()).then(|| text[start..end].to_string())
}

pub(crate) fn range_is_usable_for_text(text: &str, range: Range) -> bool {
    let Some(start) = offset_at(text, range.start) else {
        return false;
    };
    let Some(end) = offset_at(text, range.end) else {
        return false;
    };
    start <= end && end <= text.len()
}

pub(crate) fn ranges_overlap(a: Range, b: Range) -> bool {
    position_leq_lsp(a.start, b.end) && position_leq_lsp(b.start, a.end)
}

pub(crate) fn position_leq_lsp(left: Position, right: Position) -> bool {
    left.line < right.line || (left.line == right.line && left.character <= right.character)
}

pub(crate) fn range_is_empty(range: Range) -> bool {
    range.start.line == range.end.line && range.start.character == range.end.character
}

pub(crate) fn unknown_var_name_from_message(message: &str) -> Option<&str> {
    message.strip_prefix("unbound variable ").map(str::trim)
}

pub(crate) fn field_not_definitely_available_from_message(message: &str) -> Option<&str> {
    let rest = message.strip_prefix("field `")?;
    let (field, tail) = rest.split_once('`')?;
    tail.contains("is not definitely available on")
        .then_some(field)
}

pub(crate) fn default_record_candidates_for_field(
    compilation_unit: &CompilationUnit,
    field: &str,
) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for decl in &compilation_unit.decls {
        let Decl::Instance(inst) = decl else {
            continue;
        };
        if inst.class.as_ref() != "Default" {
            continue;
        }
        let TypeExpr::Name(_, ty_name) = &inst.head else {
            continue;
        };
        if !type_decl_has_record_field(compilation_unit, ty_name.as_ref(), field) {
            continue;
        }
        let ty_name = ty_name.as_ref().to_string();
        if seen.insert(ty_name.clone()) {
            out.push(ty_name);
        }
    }
    out
}

pub(crate) fn type_decl_has_record_field(
    compilation_unit: &CompilationUnit,
    type_name: &str,
    field: &str,
) -> bool {
    compilation_unit.decls.iter().any(|decl| {
        let Decl::Type(td) = decl else {
            return false;
        };
        if td.name.as_ref() != type_name {
            return false;
        }
        td.variants.iter().any(|variant| {
            variant.args.iter().any(|arg| {
                let TypeExpr::Record(_, fields) = arg else {
                    return false;
                };
                fields.iter().any(|(name, _)| name.as_ref() == field)
            })
        })
    })
}

pub(crate) fn replace_first_default_with_is(source: &str, ty_name: &str) -> Option<String> {
    for (idx, _) in source.match_indices("default") {
        let left_ok = if idx == 0 {
            true
        } else {
            !is_ident_char(source[..idx].chars().next_back().unwrap_or('_'))
        };
        let right_idx = idx + "default".len();
        let right_ok = if right_idx >= source.len() {
            true
        } else {
            !is_ident_char(source[right_idx..].chars().next().unwrap_or('_'))
        };
        if !(left_ok && right_ok) {
            continue;
        }

        let mut replaced = String::with_capacity(source.len() + ty_name.len() + 8);
        replaced.push_str(&source[..idx]);
        replaced.push_str("(default is ");
        replaced.push_str(ty_name);
        replaced.push(')');
        replaced.push_str(&source[right_idx..]);
        return Some(replaced);
    }
    None
}

pub(crate) fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

pub(crate) fn is_hole_name(name: &str) -> bool {
    name == "_" || name.starts_with('_')
}

pub fn is_list_scalar_unification_error(message: &str) -> bool {
    let Some(rest) = message.strip_prefix("types do not unify: ") else {
        return false;
    };
    let Some((left, right)) = rest.split_once(" vs ") else {
        return false;
    };
    list_inner_type(left.trim()).is_some_and(|inner| inner == right.trim())
        || list_inner_type(right.trim()).is_some_and(|inner| inner == left.trim())
}

pub(crate) fn list_inner_type(typ: &str) -> Option<&str> {
    if let Some(inner) = typ
        .strip_prefix("List<")
        .and_then(|rest| rest.strip_suffix('>'))
    {
        return Some(inner);
    }
    typ.strip_prefix("(List ")
        .and_then(|rest| rest.strip_suffix(')'))
}

pub fn is_array_list_unification_error(message: &str) -> bool {
    let Some(rest) = message.strip_prefix("types do not unify: ") else {
        return false;
    };
    let Some((left, right)) = rest.split_once(" vs ") else {
        return false;
    };
    let left = left.trim();
    let right = right.trim();
    let left_has_array = left.contains("Array");
    let left_has_list = left.contains("List");
    let right_has_array = right.contains("Array");
    let right_has_list = right.contains("List");
    (left_has_array && right_has_list) || (left_has_list && right_has_array)
}

pub fn is_function_value_unification_error(message: &str) -> bool {
    let Some(rest) = message.strip_prefix("types do not unify: ") else {
        return false;
    };
    let Some((left, right)) = rest.split_once(" vs ") else {
        return false;
    };
    let left_is_fun = looks_like_fun_type(left.trim());
    let right_is_fun = looks_like_fun_type(right.trim());
    left_is_fun ^ right_is_fun
}

pub(crate) fn looks_like_fun_type(typ: &str) -> bool {
    let mut depth = 0usize;
    let bytes = typ.as_bytes();
    let mut i = 0usize;
    while i + 1 < bytes.len() {
        match bytes[i] as char {
            '(' | '{' | '[' => depth += 1,
            ')' | '}' | ']' => depth = depth.saturating_sub(1),
            '-' if bytes[i + 1] as char == '>' && depth == 0 => return true,
            _ => {}
        }
        i += 1;
    }

    if typ.starts_with('(') && typ.ends_with(')') {
        return looks_like_fun_type(&typ[1..typ.len() - 1]);
    }
    false
}

pub(crate) fn split_fun_type(typ: &Type) -> (Vec<Type>, Type) {
    let mut args = Vec::new();
    let mut cur = typ.clone();
    while let TypeKind::Fun(arg, ret) = cur.as_ref() {
        args.push(arg.clone());
        cur = ret.clone();
    }
    (args, cur)
}

pub(crate) fn in_scope_value_types_at_position(
    session: &AnalysisSession,
    uri: &Url,
    text: &str,
    position: Position,
) -> Vec<(String, Type)> {
    let Ok((_tokens, program)) = session.tokenize_and_parse_cached(uri, text) else {
        return Vec::new();
    };
    let Ok((program, mut ts, _imports, _import_diags)) =
        prepare_program_with_imports(session, uri, &program)
    else {
        return Vec::new();
    };
    if inject_program_decls(&mut ts, &program, None).is_err() {
        return Vec::new();
    }

    let Some(expr) = program.body_with_fns() else {
        return Vec::new();
    };
    let Ok((typed, _preds, _ty)) = infer_typed(&mut ts, expr.as_ref()) else {
        return Vec::new();
    };
    let pos = lsp_to_rex_position(position);

    fn visit(
        expr: &Expr,
        typed: &TypedExpr,
        pos: RexPosition,
        scope: &mut Vec<(String, Type)>,
        best: &mut Option<Vec<(String, Type)>>,
    ) {
        if !position_in_span(pos, *expr.span()) {
            return;
        }
        *best = Some(scope.clone());

        match (expr, typed.kind.as_ref()) {
            (
                Expr::Let(_span, var, _, _ann, def, body),
                TypedExprKind::Let {
                    def: tdef,
                    body: tbody,
                    ..
                },
            ) => {
                if position_in_span(pos, *def.span()) {
                    visit(def.as_ref(), tdef.as_ref(), pos, scope, best);
                    return;
                }
                if position_in_span(pos, *body.span()) {
                    scope.push((var.name.to_string(), tdef.typ.clone()));
                    visit(body.as_ref(), tbody.as_ref(), pos, scope, best);
                    scope.pop();
                }
            }
            (
                Expr::LetRec(_span, bindings, body),
                TypedExprKind::LetRec {
                    bindings: typed_bindings,
                    body: typed_body,
                },
            ) => {
                let base = scope.len();
                for ((name, _, _ann, _def), (_typed_name, typed_def)) in
                    bindings.iter().zip(typed_bindings.iter())
                {
                    scope.push((name.name.to_string(), typed_def.typ.clone()));
                }
                for ((_, _, _, def), (_, typed_def)) in bindings.iter().zip(typed_bindings.iter()) {
                    if position_in_span(pos, *def.span()) {
                        visit(def.as_ref(), typed_def, pos, scope, best);
                        scope.truncate(base);
                        return;
                    }
                }
                if position_in_span(pos, *body.span()) {
                    visit(body.as_ref(), typed_body.as_ref(), pos, scope, best);
                }
                scope.truncate(base);
            }
            (
                Expr::Lam(_span, _scope, param, _ann, _constraints, body),
                TypedExprKind::Lam {
                    body: typed_body, ..
                },
            ) => {
                if let TypeKind::Fun(arg, _ret) = typed.typ.as_ref() {
                    scope.push((param.name.to_string(), arg.clone()));
                    visit(body.as_ref(), typed_body.as_ref(), pos, scope, best);
                    scope.pop();
                }
            }
            (Expr::App(_span, fun, arg), TypedExprKind::App(tfun, targ)) => {
                if position_in_span(pos, *fun.span()) {
                    visit(fun.as_ref(), tfun.as_ref(), pos, scope, best);
                } else if position_in_span(pos, *arg.span()) {
                    visit(arg.as_ref(), targ.as_ref(), pos, scope, best);
                }
            }
            (Expr::Project(_span, base, _field), TypedExprKind::Project { expr: tbase, .. }) => {
                visit(base.as_ref(), tbase.as_ref(), pos, scope, best);
            }
            (
                Expr::Ite(_span, cond, then_expr, else_expr),
                TypedExprKind::Ite {
                    cond: tcond,
                    then_expr: tthen,
                    else_expr: telse,
                },
            ) => {
                if position_in_span(pos, *cond.span()) {
                    visit(cond.as_ref(), tcond.as_ref(), pos, scope, best);
                } else if position_in_span(pos, *then_expr.span()) {
                    visit(then_expr.as_ref(), tthen.as_ref(), pos, scope, best);
                } else if position_in_span(pos, *else_expr.span()) {
                    visit(else_expr.as_ref(), telse.as_ref(), pos, scope, best);
                }
            }
            (Expr::Tuple(_span, elems), TypedExprKind::Tuple(typed_elems))
            | (Expr::List(_span, elems), TypedExprKind::List(typed_elems)) => {
                for (elem, typed_elem) in elems.iter().zip(typed_elems.iter()) {
                    if position_in_span(pos, *elem.span()) {
                        visit(elem.as_ref(), typed_elem, pos, scope, best);
                        break;
                    }
                }
            }
            (Expr::Dict(_span, kvs), TypedExprKind::Dict(typed_kvs)) => {
                for (key, value) in kvs {
                    if position_in_span(pos, *value.span())
                        && let Some(typed_v) = typed_kvs.get(key)
                    {
                        visit(value.as_ref(), typed_v, pos, scope, best);
                        break;
                    }
                }
            }
            (
                Expr::RecordUpdate(_span, base, updates),
                TypedExprKind::RecordUpdate {
                    base: tbase,
                    updates: typed_updates,
                },
            ) => {
                if position_in_span(pos, *base.span()) {
                    visit(base.as_ref(), tbase.as_ref(), pos, scope, best);
                } else {
                    for (key, value) in updates {
                        if position_in_span(pos, *value.span())
                            && let Some(typed_v) = typed_updates.get(key)
                        {
                            visit(value.as_ref(), typed_v, pos, scope, best);
                            break;
                        }
                    }
                }
            }
            (
                Expr::Match(_span, scrutinee, arms),
                TypedExprKind::Match {
                    scrutinee: tscrutinee,
                    arms: typed_arms,
                },
            ) => {
                if position_in_span(pos, *scrutinee.span()) {
                    visit(scrutinee.as_ref(), tscrutinee.as_ref(), pos, scope, best);
                } else {
                    for ((_pat, arm), (_typed_pat, typed_arm)) in arms.iter().zip(typed_arms.iter())
                    {
                        if position_in_span(pos, *arm.span()) {
                            visit(arm.as_ref(), typed_arm, pos, scope, best);
                            break;
                        }
                    }
                }
            }
            (Expr::Ann(_span, inner, _), _) => visit(inner.as_ref(), typed, pos, scope, best),
            _ => {}
        }
    }

    let mut best = None;
    visit(expr.as_ref(), &typed, pos, &mut Vec::new(), &mut best);
    best.unwrap_or_default()
}

pub(crate) fn hole_fill_candidates_at_position(
    session: &AnalysisSession,
    uri: &Url,
    text: &str,
    position: Position,
) -> Vec<(String, String)> {
    let Some(target_type) = expected_type_at_position_type(session, uri, text, position) else {
        return Vec::new();
    };
    let Ok((_tokens, program)) = session.tokenize_and_parse_cached(uri, text) else {
        return Vec::new();
    };
    let Ok((program, mut ts, _imports, _import_diags)) =
        prepare_program_with_imports(session, uri, &program)
    else {
        return Vec::new();
    };
    if inject_program_decls(&mut ts, &program, None).is_err() {
        return Vec::new();
    }
    let mut in_scope = in_scope_value_types_at_position(session, uri, text, position)
        .into_iter()
        .filter(|(name, _)| is_ident_like(name))
        .collect::<Vec<_>>();
    if in_scope.len() > MAX_SEMANTIC_IN_SCOPE_VALUES {
        in_scope = in_scope.split_off(in_scope.len().saturating_sub(MAX_SEMANTIC_IN_SCOPE_VALUES));
    }

    let values = semantic_candidate_values(&ts);

    let mut adapters: Vec<(String, Type, Type)> = Vec::new();
    for (name, schemes) in &values {
        let name = name.to_string();
        if !is_ident_like(&name) {
            continue;
        }
        for scheme in schemes {
            let (_preds, inst_ty) = instantiate(scheme, &mut ts.supply);
            let (args, ret) = split_fun_type(&inst_ty);
            if args.len() == 1 {
                adapters.push((name.clone(), args[0].clone(), ret));
            }
        }
    }

    let mut out: Vec<(usize, usize, String, String)> = Vec::new();
    for (name, schemes) in values {
        let name = name.to_string();
        if !is_ident_like(&name) {
            continue;
        }
        for scheme in schemes {
            let (_preds, inst_ty) = instantiate(&scheme, &mut ts.supply);
            let (args, ret) = split_fun_type(&inst_ty);
            if args.is_empty()
                || args.len() > MAX_SEMANTIC_HOLE_FILL_ARITY
                || unify(&ret, &target_type).is_err()
            {
                continue;
            }

            let mut unresolved = 0usize;
            let mut adapter_uses = 0usize;
            let mut rendered_args = Vec::new();
            for arg_ty in args {
                if let Some((value_name, _value_ty)) = in_scope
                    .iter()
                    .rev()
                    .find(|(_, value_ty)| unify(value_ty, &arg_ty).is_ok())
                {
                    rendered_args.push(value_name.clone());
                    continue;
                }

                let mut adapted = None;
                for (adapter_name, adapter_arg, adapter_ret) in &adapters {
                    if unify(adapter_ret, &arg_ty).is_err() {
                        continue;
                    }
                    if let Some((value_name, _value_ty)) = in_scope
                        .iter()
                        .rev()
                        .find(|(_, value_ty)| unify(value_ty, adapter_arg).is_ok())
                    {
                        adapted = Some(format!("({adapter_name} {value_name})"));
                        break;
                    }
                }
                if let Some(expr) = adapted {
                    adapter_uses += 1;
                    rendered_args.push(expr);
                } else {
                    unresolved += 1;
                    rendered_args.push("?".to_string());
                }
            }

            let replacement = format!("{name} {}", rendered_args.join(" "));
            out.push((unresolved, adapter_uses, name.clone(), replacement));
        }
    }

    out.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    out.dedup_by(|a, b| a.2 == b.2 && a.3 == b.3);
    if out.len() > MAX_SEMANTIC_CANDIDATES {
        out.truncate(MAX_SEMANTIC_CANDIDATES);
    }
    out.into_iter()
        .map(|(_u, _a, name, replacement)| (name, replacement))
        .collect()
}

pub(crate) fn levenshtein_distance(left: &str, right: &str) -> usize {
    if left == right {
        return 0;
    }
    if left.is_empty() {
        return right.chars().count();
    }
    if right.is_empty() {
        return left.chars().count();
    }

    let right_len = right.chars().count();
    let mut prev: Vec<usize> = (0..=right_len).collect();
    let mut cur = vec![0usize; right_len + 1];

    for (i, lc) in left.chars().enumerate() {
        cur[0] = i + 1;
        for (j, rc) in right.chars().enumerate() {
            let insert_cost = cur[j] + 1;
            let delete_cost = prev[j + 1] + 1;
            let replace_cost = prev[j] + usize::from(lc != rc);
            cur[j + 1] = insert_cost.min(delete_cost).min(replace_cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }

    prev[right_len]
}

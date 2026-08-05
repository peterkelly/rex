use crate::prelude::*;
use crate::{code_actions::*, completion::*, diagnostics::*, imports::*, shared::*};

pub(crate) struct HoverType {
    span: Span,
    label: String,
    typ: String,
    overloads: Vec<String>,
}

pub(crate) fn hover_type_contents(
    session: &AnalysisSession,
    uri: &Url,
    text: &str,
    position: Position,
) -> Option<HoverContents> {
    let (tokens, program) = session.tokenize_and_parse_cached(uri, text).ok()?;
    let (name, name_span, name_is_ident) = name_token_at_position(&tokens, position)?;
    let (program, mut ts, _imports, _import_diags) =
        prepare_program_with_imports(session, uri, &program).ok()?;

    let pos = lsp_to_rex_position(position);

    // If the cursor is inside an instance method body, typecheck that method
    // body using the instance context rules (so hover works inside methods).
    let mut target_instance: Option<(usize, usize)> = None;
    for (decl_idx, decl) in program.decls.iter().enumerate() {
        let Decl::Instance(inst) = decl else {
            continue;
        };
        for (method_idx, method) in inst.methods.iter().enumerate() {
            if position_in_span(pos, *method.body.span()) {
                target_instance = Some((decl_idx, method_idx));
                break;
            }
        }
        if target_instance.is_some() {
            break;
        }
    }

    let (_instances, prepared_target_instance) = inject_program_decls(
        &mut ts,
        &program,
        target_instance.map(|(decl_idx, _)| decl_idx),
    )
    .ok()?;

    let body_with_fns = program.body_with_fns();

    let root_expr: &Expr;
    let typed_root: TypedExpr;

    if let Some((decl_idx, method_idx)) = target_instance {
        let Decl::Instance(inst) = &program.decls[decl_idx] else {
            return None;
        };
        let prepared = prepared_target_instance?;
        let method = inst.methods.get(method_idx)?;
        typed_root = ts.typecheck_instance_method(&prepared, method).ok()?;
        root_expr = method.body.as_ref();
    } else {
        let body_with_fns = body_with_fns.as_ref()?;
        let (typed, _preds, _) = infer_typed(&mut ts, body_with_fns.as_ref()).ok()?;
        typed_root = typed;
        root_expr = body_with_fns.as_ref();
    }

    let hover = hover_type_in_expr(
        &mut ts,
        root_expr,
        &typed_root,
        pos,
        &name,
        name_span,
        name_is_ident,
    )?;

    let mut md = String::new();
    md.push_str("```rex\n");
    md.push_str(&hover.label);
    md.push_str(" : ");
    md.push_str(&hover.typ);
    md.push_str("\n```");

    if !hover.overloads.is_empty() {
        md.push_str("\n\nOverloads:\n");
        for ov in &hover.overloads {
            md.push_str("- `");
            md.push_str(ov);
            md.push_str("`\n");
        }
    }

    Some(HoverContents::Markup(MarkupContent {
        kind: MarkupKind::Markdown,
        value: md,
    }))
}

pub(crate) fn expected_type_at_position(
    session: &AnalysisSession,
    uri: &Url,
    text: &str,
    position: Position,
) -> Option<String> {
    expected_type_at_position_type(session, uri, text, position).map(|ty| ty.to_string())
}

pub(crate) fn inferred_type_at_position(
    session: &AnalysisSession,
    uri: &Url,
    text: &str,
    position: Position,
) -> Option<String> {
    inferred_type_at_position_type(session, uri, text, position).map(|ty| ty.to_string())
}

pub(crate) fn expected_type_at_position_type(
    session: &AnalysisSession,
    uri: &Url,
    text: &str,
    position: Position,
) -> Option<Type> {
    let (_tokens, program) = session.tokenize_and_parse_cached(uri, text).ok()?;
    let (program, mut ts, _imports, _import_diags) =
        prepare_program_with_imports(session, uri, &program).ok()?;

    let pos = lsp_to_rex_position(position);

    // Mirror hover behavior inside instance methods.
    let mut target_instance: Option<(usize, usize)> = None;
    for (decl_idx, decl) in program.decls.iter().enumerate() {
        let Decl::Instance(inst) = decl else {
            continue;
        };
        for (method_idx, method) in inst.methods.iter().enumerate() {
            if position_in_span(pos, *method.body.span()) {
                target_instance = Some((decl_idx, method_idx));
                break;
            }
        }
        if target_instance.is_some() {
            break;
        }
    }

    let (_instances, prepared_target_instance) = inject_program_decls(
        &mut ts,
        &program,
        target_instance.map(|(decl_idx, _)| decl_idx),
    )
    .ok()?;

    let body_with_fns = program.body_with_fns();
    let root_expr: &Expr;
    let typed_root: TypedExpr;

    if let Some((decl_idx, method_idx)) = target_instance {
        let Decl::Instance(inst) = &program.decls[decl_idx] else {
            return None;
        };
        let prepared = prepared_target_instance?;
        let method = inst.methods.get(method_idx)?;
        typed_root = ts.typecheck_instance_method(&prepared, method).ok()?;
        root_expr = method.body.as_ref();
    } else {
        let body_with_fns = body_with_fns.as_ref()?;
        let (typed, _preds, _) = infer_typed(&mut ts, body_with_fns.as_ref()).ok()?;
        typed_root = typed;
        root_expr = body_with_fns.as_ref();
    }

    expected_type_in_expr(root_expr, &typed_root, pos)
}

pub(crate) fn inferred_type_at_position_type(
    session: &AnalysisSession,
    uri: &Url,
    text: &str,
    position: Position,
) -> Option<Type> {
    let (_tokens, program) = session.tokenize_and_parse_cached(uri, text).ok()?;
    let (program, mut ts, _imports, _import_diags) =
        prepare_program_with_imports(session, uri, &program).ok()?;

    let pos = lsp_to_rex_position(position);

    let mut target_instance: Option<(usize, usize)> = None;
    for (decl_idx, decl) in program.decls.iter().enumerate() {
        let Decl::Instance(inst) = decl else {
            continue;
        };
        for (method_idx, method) in inst.methods.iter().enumerate() {
            if position_in_span(pos, *method.body.span()) {
                target_instance = Some((decl_idx, method_idx));
                break;
            }
        }
        if target_instance.is_some() {
            break;
        }
    }

    let (_instances, prepared_target_instance) = inject_program_decls(
        &mut ts,
        &program,
        target_instance.map(|(decl_idx, _)| decl_idx),
    )
    .ok()?;

    let body_with_fns = program.body_with_fns();
    let root_expr: &Expr;
    let typed_root: TypedExpr;

    if let Some((decl_idx, method_idx)) = target_instance {
        let Decl::Instance(inst) = &program.decls[decl_idx] else {
            return None;
        };
        let prepared = prepared_target_instance?;
        let method = inst.methods.get(method_idx)?;
        typed_root = ts.typecheck_instance_method(&prepared, method).ok()?;
        root_expr = method.body.as_ref();
    } else {
        let body_with_fns = body_with_fns.as_ref()?;
        let (typed, _preds, _) = infer_typed(&mut ts, body_with_fns.as_ref()).ok()?;
        typed_root = typed;
        root_expr = body_with_fns.as_ref();
    }

    inferred_type_in_expr(root_expr, &typed_root, pos)
}

pub(crate) fn expected_type_in_expr(
    expr: &Expr,
    typed: &TypedExpr,
    pos: RexPosition,
) -> Option<Type> {
    #[derive(Clone)]
    struct Candidate {
        span: Span,
        typ: Type,
    }

    fn span_size(span: Span) -> (usize, usize) {
        (
            span.end.line.saturating_sub(span.begin.line),
            span.end.column.saturating_sub(span.begin.column),
        )
    }

    fn consider(best: &mut Option<Candidate>, span: Span, typ: &Type) {
        let replace = best
            .as_ref()
            .is_none_or(|cur| span_size(span) < span_size(cur.span));
        if replace {
            *best = Some(Candidate {
                span,
                typ: typ.clone(),
            });
        }
    }

    fn visit(
        expr: &Expr,
        typed: &TypedExpr,
        pos: RexPosition,
        expected: Option<&Type>,
        best: &mut Option<Candidate>,
    ) {
        if !position_in_span(pos, *expr.span()) {
            return;
        }

        if let Some(expected) = expected {
            consider(best, *expr.span(), expected);
        }

        match (expr, typed.kind.as_ref()) {
            (
                Expr::Let(_span, _name, _, _ann, def, body),
                TypedExprKind::Let {
                    def: tdef,
                    body: tbody,
                    ..
                },
            ) => {
                visit(def.as_ref(), tdef.as_ref(), pos, Some(&tdef.typ), best);
                visit(body.as_ref(), tbody.as_ref(), pos, Some(&typed.typ), best);
            }
            (
                Expr::LetRec(_span, bindings, body),
                TypedExprKind::LetRec {
                    bindings: typed_bindings,
                    body: typed_body,
                },
            ) => {
                for ((_name, _, _ann, def), (_typed_name, typed_def)) in
                    bindings.iter().zip(typed_bindings.iter())
                {
                    visit(def.as_ref(), typed_def, pos, Some(&typed_def.typ), best);
                }
                visit(
                    body.as_ref(),
                    typed_body.as_ref(),
                    pos,
                    Some(&typed.typ),
                    best,
                );
            }
            (
                Expr::Lam(_span, _scope, _param, _ann, _constraints, body),
                TypedExprKind::Lam {
                    body: typed_body, ..
                },
            ) => {
                let body_expected = match typed.typ.as_ref() {
                    TypeKind::Fun(_arg, ret) => Some(ret),
                    _ => None,
                };
                visit(body.as_ref(), typed_body.as_ref(), pos, body_expected, best);
            }
            (Expr::App(_span, f, x), TypedExprKind::App(tf, tx)) => {
                let expected_arg = match tf.typ.as_ref() {
                    TypeKind::Fun(arg, _ret) => Some(arg),
                    _ => None,
                };
                visit(x.as_ref(), tx.as_ref(), pos, expected_arg, best);

                let expected_fun = Type::fun(tx.typ.clone(), typed.typ.clone());
                visit(f.as_ref(), tf.as_ref(), pos, Some(&expected_fun), best);
            }
            (Expr::Project(_span, base, _field), TypedExprKind::Project { expr: tbase, .. }) => {
                visit(base.as_ref(), tbase.as_ref(), pos, None, best);
            }
            (
                Expr::Ite(_span, cond, then_expr, else_expr),
                TypedExprKind::Ite {
                    cond: tcond,
                    then_expr: tthen,
                    else_expr: telse,
                },
            ) => {
                let bool_ty = Type::builtin(BuiltinTypeId::Bool);
                visit(cond.as_ref(), tcond.as_ref(), pos, Some(&bool_ty), best);
                visit(
                    then_expr.as_ref(),
                    tthen.as_ref(),
                    pos,
                    Some(&typed.typ),
                    best,
                );
                visit(
                    else_expr.as_ref(),
                    telse.as_ref(),
                    pos,
                    Some(&typed.typ),
                    best,
                );
            }
            (Expr::Tuple(_span, elems), TypedExprKind::Tuple(typed_elems)) => {
                for (elem, typed_elem) in elems.iter().zip(typed_elems.iter()) {
                    visit(elem.as_ref(), typed_elem, pos, Some(&typed_elem.typ), best);
                }
            }
            (Expr::List(_span, elems), TypedExprKind::List(typed_elems)) => {
                let list_elem_expected = match typed.typ.as_ref() {
                    TypeKind::App(head, elem) => match head.as_ref() {
                        TypeKind::Con(tc)
                            if tc.is_builtin(BuiltinTypeId::List) && tc.arity() == 1 =>
                        {
                            Some(elem)
                        }
                        _ => None,
                    },
                    _ => None,
                };
                for (elem, typed_elem) in elems.iter().zip(typed_elems.iter()) {
                    let expected = list_elem_expected.unwrap_or(&typed_elem.typ);
                    visit(elem.as_ref(), typed_elem, pos, Some(expected), best);
                }
            }
            (Expr::Dict(_span, kvs), TypedExprKind::Dict(typed_kvs)) => {
                for (key, value) in kvs {
                    if let Some(typed_value) = typed_kvs.get(key) {
                        visit(
                            value.as_ref(),
                            typed_value,
                            pos,
                            Some(&typed_value.typ),
                            best,
                        );
                    }
                }
            }
            (
                Expr::RecordUpdate(_span, base, updates),
                TypedExprKind::RecordUpdate {
                    base: typed_base,
                    updates: typed_updates,
                },
            ) => {
                visit(base.as_ref(), typed_base.as_ref(), pos, None, best);
                for (key, value) in updates {
                    if let Some(typed_value) = typed_updates.get(key) {
                        visit(
                            value.as_ref(),
                            typed_value,
                            pos,
                            Some(&typed_value.typ),
                            best,
                        );
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
                visit(
                    scrutinee.as_ref(),
                    tscrutinee.as_ref(),
                    pos,
                    Some(&tscrutinee.typ),
                    best,
                );
                for ((_pat, arm), (_typed_pat, typed_arm)) in arms.iter().zip(typed_arms.iter()) {
                    visit(arm.as_ref(), typed_arm, pos, Some(&typed.typ), best);
                }
            }
            (Expr::Ann(_span, inner, _ann), _) => {
                visit(inner.as_ref(), typed, pos, Some(&typed.typ), best);
            }
            _ => {}
        }
    }

    let mut best: Option<Candidate> = None;
    visit(expr, typed, pos, None, &mut best);
    best.map(|candidate| candidate.typ)
}

pub(crate) fn inferred_type_in_expr(
    expr: &Expr,
    typed: &TypedExpr,
    pos: RexPosition,
) -> Option<Type> {
    fn span_size(span: Span) -> (usize, usize) {
        (
            span.end.line.saturating_sub(span.begin.line),
            span.end.column.saturating_sub(span.begin.column),
        )
    }

    fn visit(expr: &Expr, typed: &TypedExpr, pos: RexPosition, best: &mut Option<(Span, Type)>) {
        let span = *expr.span();
        if !position_in_span(pos, span) {
            return;
        }
        if best
            .as_ref()
            .is_none_or(|(best_span, _)| span_size(span) < span_size(*best_span))
        {
            *best = Some((span, typed.typ.clone()));
        }

        match (expr, typed.kind.as_ref()) {
            (
                Expr::Let(_, _, _, _, def, body),
                TypedExprKind::Let {
                    def: tdef,
                    body: tbody,
                    ..
                },
            ) => {
                visit(def.as_ref(), tdef.as_ref(), pos, best);
                visit(body.as_ref(), tbody.as_ref(), pos, best);
            }
            (
                Expr::LetRec(_, bindings, body),
                TypedExprKind::LetRec {
                    bindings: typed_bindings,
                    body: typed_body,
                },
            ) => {
                for ((_, _, _, def), (_, typed_def)) in bindings.iter().zip(typed_bindings.iter()) {
                    visit(def.as_ref(), typed_def, pos, best);
                }
                visit(body.as_ref(), typed_body.as_ref(), pos, best);
            }
            (
                Expr::Lam(_, _, _, _, _, body),
                TypedExprKind::Lam {
                    body: typed_body, ..
                },
            ) => {
                visit(body.as_ref(), typed_body.as_ref(), pos, best);
            }
            (Expr::App(_, f, x), TypedExprKind::App(tf, tx)) => {
                visit(f.as_ref(), tf.as_ref(), pos, best);
                visit(x.as_ref(), tx.as_ref(), pos, best);
            }
            (Expr::Project(_, base, _), TypedExprKind::Project { expr: tbase, .. }) => {
                visit(base.as_ref(), tbase.as_ref(), pos, best);
            }
            (
                Expr::Ite(_, cond, then_expr, else_expr),
                TypedExprKind::Ite {
                    cond: tcond,
                    then_expr: tthen,
                    else_expr: telse,
                },
            ) => {
                visit(cond.as_ref(), tcond.as_ref(), pos, best);
                visit(then_expr.as_ref(), tthen.as_ref(), pos, best);
                visit(else_expr.as_ref(), telse.as_ref(), pos, best);
            }
            (Expr::Tuple(_, elems), TypedExprKind::Tuple(typed_elems))
            | (Expr::List(_, elems), TypedExprKind::List(typed_elems)) => {
                for (elem, typed_elem) in elems.iter().zip(typed_elems.iter()) {
                    visit(elem.as_ref(), typed_elem, pos, best);
                }
            }
            (Expr::Dict(_, kvs), TypedExprKind::Dict(typed_kvs)) => {
                for (key, value) in kvs {
                    if let Some(typed_value) = typed_kvs.get(key) {
                        visit(value.as_ref(), typed_value, pos, best);
                    }
                }
            }
            (
                Expr::RecordUpdate(_, base, updates),
                TypedExprKind::RecordUpdate {
                    base: typed_base,
                    updates: typed_updates,
                },
            ) => {
                visit(base.as_ref(), typed_base.as_ref(), pos, best);
                for (key, value) in updates {
                    if let Some(typed_value) = typed_updates.get(key) {
                        visit(value.as_ref(), typed_value, pos, best);
                    }
                }
            }
            (
                Expr::Match(_, scrutinee, arms),
                TypedExprKind::Match {
                    scrutinee: tscrutinee,
                    arms: typed_arms,
                },
            ) => {
                visit(scrutinee.as_ref(), tscrutinee.as_ref(), pos, best);
                for ((_pat, arm), (_typed_pat, typed_arm)) in arms.iter().zip(typed_arms.iter()) {
                    visit(arm.as_ref(), typed_arm, pos, best);
                }
            }
            (Expr::Ann(_, inner, _), _) => visit(inner.as_ref(), typed, pos, best),
            _ => {}
        }
    }

    let mut best: Option<(Span, Type)> = None;
    visit(expr, typed, pos, &mut best);
    best.map(|(_, ty)| ty)
}

pub(crate) fn functions_producing_expected_type_at_position(
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

    let preferred_names = decl_value_names(&program.decls);
    let values = semantic_candidate_values(&ts, &preferred_names);

    let mut out = Vec::new();
    for (name, schemes) in values {
        for scheme in schemes {
            let (_preds, inst_ty) = instantiate(&scheme, &mut ts.supply);
            let mut cur = &inst_ty;
            let mut is_function = false;
            while let TypeKind::Fun(_, ret) = cur.as_ref() {
                is_function = true;
                cur = ret;
            }
            if !is_function {
                continue;
            }
            if unify(cur, &target_type).is_ok() {
                out.push((name.to_string(), scheme.typ.to_string()));
            }
        }
    }

    sort_semantic_type_candidates(&mut out, &preferred_names);
    out.dedup();
    if out.len() > MAX_SEMANTIC_CANDIDATES {
        out.truncate(MAX_SEMANTIC_CANDIDATES);
    }
    out
}

pub(crate) fn functions_accepting_inferred_type_at_position(
    session: &AnalysisSession,
    uri: &Url,
    text: &str,
    position: Position,
) -> Vec<(String, String)> {
    let Some(source_type) = inferred_type_at_position_type(session, uri, text, position) else {
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

    let preferred_names = decl_value_names(&program.decls);
    let values = semantic_candidate_values(&ts, &preferred_names);

    let mut out = Vec::new();
    for (name, schemes) in values {
        let name = name.to_string();
        if !is_ident_like(&name) {
            continue;
        }
        for scheme in schemes {
            let (_preds, inst_ty) = instantiate(&scheme, &mut ts.supply);
            let (args, _ret) = split_fun_type(&inst_ty);
            if let Some(first_arg) = args.first()
                && unify(first_arg, &source_type).is_ok()
            {
                out.push((name.clone(), scheme.typ.to_string()));
            }
        }
    }

    sort_semantic_type_candidates(&mut out, &preferred_names);
    out.dedup();
    if out.len() > MAX_SEMANTIC_CANDIDATES {
        out.truncate(MAX_SEMANTIC_CANDIDATES);
    }
    out
}

pub(crate) fn adapters_from_inferred_to_expected_at_position(
    session: &AnalysisSession,
    uri: &Url,
    text: &str,
    position: Position,
) -> Vec<(String, String)> {
    let Some(source_type) = inferred_type_at_position_type(session, uri, text, position) else {
        return Vec::new();
    };
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

    let preferred_names = decl_value_names(&program.decls);
    let values = semantic_candidate_values(&ts, &preferred_names);

    let mut out = Vec::new();
    for (name, schemes) in values {
        let name = name.to_string();
        if !is_ident_like(&name) {
            continue;
        }
        for scheme in schemes {
            let (_preds, inst_ty) = instantiate(&scheme, &mut ts.supply);
            let (args, ret) = split_fun_type(&inst_ty);
            if args.len() == 1
                && unify(&args[0], &source_type).is_ok()
                && unify(&ret, &target_type).is_ok()
            {
                out.push((name.clone(), scheme.typ.to_string()));
            }
        }
    }

    sort_semantic_type_candidates(&mut out, &preferred_names);
    out.dedup();
    if out.len() > MAX_SEMANTIC_CANDIDATES {
        out.truncate(MAX_SEMANTIC_CANDIDATES);
    }
    out
}

pub(crate) fn functions_compatible_with_in_scope_values_at_position(
    session: &AnalysisSession,
    uri: &Url,
    text: &str,
    position: Position,
) -> Vec<String> {
    let produced = functions_producing_expected_type_at_position(session, uri, text, position);
    let mut produced_by_name: HashMap<String, Vec<String>> = HashMap::new();
    for (name, typ) in produced {
        produced_by_name.entry(name).or_default().push(typ);
    }

    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for (name, replacement) in hole_fill_candidates_at_position(session, uri, text, position) {
        if replacement.contains('?') {
            continue;
        }
        if let Some(types) = produced_by_name.get(&name) {
            for typ in types {
                let candidate = format!("{name} : {typ} => {replacement}");
                if seen.insert(candidate.clone()) {
                    out.push(candidate);
                }
            }
        } else {
            let candidate = format!("{name} => {replacement}");
            if seen.insert(candidate.clone()) {
                out.push(candidate);
            }
        }
    }
    // Preserve hole-fill ranking so user declarations are not displaced by a
    // growing prelude before the result limit is applied.
    if out.len() > MAX_SEMANTIC_CANDIDATES {
        out.truncate(MAX_SEMANTIC_CANDIDATES);
    }
    out
}

fn sort_semantic_type_candidates(
    candidates: &mut [(String, String)],
    preferred_names: &BTreeSet<Symbol>,
) {
    candidates.sort_by(|left, right| {
        let left_is_fallback = !preferred_names.contains(left.0.as_str());
        let right_is_fallback = !preferred_names.contains(right.0.as_str());
        left_is_fallback
            .cmp(&right_is_fallback)
            .then(left.0.cmp(&right.0))
            .then(left.1.cmp(&right.1))
    });
}

pub fn execute_query_command_for_document(
    session: &AnalysisSession,
    command: &str,
    uri: &Url,
    text: &str,
    position: Position,
) -> Option<Value> {
    match command {
        CMD_EXPECTED_TYPE_AT => Some(
            match expected_type_at_position(session, uri, text, position) {
                Some(typ) => json!({ "expectedType": typ }),
                None => Value::Null,
            },
        ),
        CMD_FUNCTIONS_ACCEPTING_INFERRED_TYPE_AT => Some(json!({
            "inferredType": inferred_type_at_position(session, uri, text, position),
            "items": functions_accepting_inferred_type_at_position(session, uri, text, position)
                .into_iter()
                .map(|(name, typ)| format!("{name} : {typ}"))
                .collect::<Vec<_>>()
        })),
        CMD_ADAPTERS_FROM_INFERRED_TO_EXPECTED_AT => Some(json!({
            "inferredType": inferred_type_at_position(session, uri, text, position),
            "expectedType": expected_type_at_position(session, uri, text, position),
            "items": adapters_from_inferred_to_expected_at_position(session, uri, text, position)
                .into_iter()
                .map(|(name, typ)| format!("{name} : {typ}"))
                .collect::<Vec<_>>()
        })),
        CMD_FUNCTIONS_COMPATIBLE_WITH_IN_SCOPE_VALUES_AT => Some(json!({
            "items": functions_compatible_with_in_scope_values_at_position(session, uri, text, position)
        })),
        CMD_FUNCTIONS_PRODUCING_EXPECTED_TYPE_AT => {
            let items = functions_producing_expected_type_at_position(session, uri, text, position)
                .into_iter()
                .map(|(name, typ)| format!("{name} : {typ}"))
                .collect::<Vec<_>>();
            Some(json!({ "items": items }))
        }
        _ => None,
    }
}

pub fn execute_query_command_for_document_without_position(
    session: &AnalysisSession,
    command: &str,
    uri: &Url,
    text: &str,
) -> Option<Value> {
    match command {
        CMD_HOLES_EXPECTED_TYPES => Some(json!({
            "holes": hole_expected_types_for_document(session, uri, text)
        })),
        _ => None,
    }
}

pub(crate) fn workspace_edit_fingerprint(edit: &WorkspaceEdit) -> String {
    let mut payload = String::new();
    if let Some(changes) = &edit.changes {
        let mut uris = changes.keys().cloned().collect::<Vec<_>>();
        uris.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        for uri in uris {
            payload.push_str(uri.as_str());
            payload.push('\n');
            if let Some(edits) = changes.get(&uri) {
                for edit in edits {
                    payload.push_str(&format!(
                        "{}:{}-{}:{}\n",
                        edit.range.start.line,
                        edit.range.start.character,
                        edit.range.end.line,
                        edit.range.end.character
                    ));
                    payload.push_str(&edit.new_text);
                    payload.push('\n');
                }
            }
        }
    }
    if let Some(document_changes) = &edit.document_changes
        && let Ok(encoded) = serde_json::to_string(document_changes)
    {
        payload.push_str(&encoded);
    }
    if let Some(change_annotations) = &edit.change_annotations
        && let Ok(encoded) = serde_json::to_string(change_annotations)
    {
        payload.push_str(&encoded);
    }
    blake3::hash(payload.as_bytes()).to_hex().to_string()
}

fn version_workspace_edit(
    mut edit: WorkspaceEdit,
    current_uri: &Url,
    document_version: Option<i32>,
) -> WorkspaceEdit {
    let Some(document_version) = document_version else {
        return edit;
    };
    if edit.document_changes.is_some() {
        return edit;
    }
    let Some(changes) = edit.changes.take() else {
        return edit;
    };

    let mut changes = changes.into_iter().collect::<Vec<_>>();
    changes.sort_by(|(left, _), (right, _)| left.as_str().cmp(right.as_str()));
    let edits = changes
        .into_iter()
        .map(|(uri, edits)| TextDocumentEdit {
            text_document: OptionalVersionedTextDocumentIdentifier {
                version: (&uri == current_uri).then_some(document_version),
                uri,
            },
            edits: edits.into_iter().map(OneOf::Left).collect(),
        })
        .collect();
    edit.document_changes = Some(DocumentChanges::Edits(edits));
    edit
}

fn quick_fix_proposal_id(
    uri: &Url,
    content_hash: &str,
    document_version: Option<i32>,
    title: &str,
    kind: Option<&str>,
    edit: &WorkspaceEdit,
) -> String {
    let payload = serde_json::to_vec(&(
        crate::SEMANTIC_QUICK_FIX_PROTOCOL_VERSION,
        uri.as_str(),
        content_hash,
        document_version,
        title,
        kind.unwrap_or(""),
        workspace_edit_fingerprint(edit),
    ))
    .unwrap_or_default();
    format!("qf2-{}", blake3::hash(&payload).to_hex())
}

fn quick_fix_rejected(reason: &str, detail: &str) -> Value {
    json!({
        "status": "rejected",
        "reason": reason,
        "detail": detail,
    })
}

fn quick_fix_stale(
    reason: &str,
    expected_content_hash: &str,
    actual_content_hash: &str,
    expected_document_version: Option<i32>,
    actual_document_version: Option<i32>,
) -> Value {
    json!({
        "status": "stale",
        "reason": reason,
        "expectedContentHash": expected_content_hash,
        "actualContentHash": actual_content_hash,
        "expectedDocumentVersion": expected_document_version,
        "actualDocumentVersion": actual_document_version,
    })
}

fn workspace_edit_has_document_version(
    edit: &WorkspaceEdit,
    uri: &Url,
    expected_version: i32,
) -> bool {
    let Some(DocumentChanges::Edits(document_edits)) = &edit.document_changes else {
        return false;
    };
    document_edits.iter().any(|document_edit| {
        document_edit.text_document.uri == *uri
            && document_edit.text_document.version == Some(expected_version)
    })
}

fn workspace_edit_only_targets_uri(edit: &WorkspaceEdit, uri: &Url) -> bool {
    if let Some(changes) = &edit.changes {
        return !changes.is_empty() && changes.keys().all(|target| target == uri);
    }
    let Some(DocumentChanges::Edits(document_edits)) = &edit.document_changes else {
        return false;
    };
    !document_edits.is_empty()
        && document_edits
            .iter()
            .all(|document_edit| document_edit.text_document.uri == *uri)
}

pub(crate) fn semantic_quick_fixes_for_range(
    session: &AnalysisSession,
    uri: &Url,
    text: &str,
    cursor_range: Range,
    diagnostics: &[Diagnostic],
    document_version: Option<i32>,
) -> Vec<Value> {
    let content_hash = text_state_hash(text);
    let mut out = code_actions_for_source(session, uri, text, cursor_range, diagnostics)
        .into_iter()
        .filter_map(|action| match action {
            CodeActionOrCommand::CodeAction(action) => Some(action),
            CodeActionOrCommand::Command(_) => None,
        })
        .map(|action| {
            let title = action.title;
            let kind = action
                .kind
                .and_then(|k| to_value(k).ok())
                .and_then(|v| v.as_str().map(str::to_string));
            let edit = version_workspace_edit(
                action.edit.unwrap_or(WorkspaceEdit {
                    changes: None,
                    document_changes: None,
                    change_annotations: None,
                }),
                uri,
                document_version,
            );
            let id = quick_fix_proposal_id(
                uri,
                &content_hash,
                document_version,
                &title,
                kind.as_deref(),
                &edit,
            );
            json!({
                "protocolVersion": crate::SEMANTIC_QUICK_FIX_PROTOCOL_VERSION,
                "id": id,
                "title": title,
                "kind": kind,
                "edit": to_value(edit).unwrap_or(Value::Null),
                "precondition": {
                    "uri": uri.as_str(),
                    "contentHash": content_hash,
                    "documentVersion": document_version,
                },
            })
        })
        .collect::<Vec<_>>();

    out.sort_by_key(|item| {
        (
            item.get("title")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            item.get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        )
    });
    out.dedup_by(|a, b| a.get("id") == b.get("id"));
    out
}

pub fn execute_semantic_loop_step(
    session: &AnalysisSession,
    uri: &Url,
    text: &str,
    position: Position,
) -> Option<Value> {
    execute_semantic_loop_step_with_version(session, uri, text, position, None)
}

pub fn execute_semantic_loop_step_with_version(
    session: &AnalysisSession,
    uri: &Url,
    text: &str,
    position: Position,
    document_version: Option<i32>,
) -> Option<Value> {
    let expected_type = expected_type_at_position(session, uri, text, position)
        .or_else(|| expected_type_from_syntax_context(session, uri, text, position));
    let inferred_type = inferred_type_at_position(session, uri, text, position);

    let mut in_scope_values = in_scope_value_types_at_position(session, uri, text, position)
        .into_iter()
        .filter(|(name, _)| is_ident_like(name))
        .map(|(name, typ)| format!("{name} : {typ}"))
        .collect::<Vec<_>>();
    in_scope_values.sort();
    in_scope_values.dedup();
    if in_scope_values.len() > MAX_SEMANTIC_IN_SCOPE_VALUES {
        in_scope_values.truncate(MAX_SEMANTIC_IN_SCOPE_VALUES);
    }

    let function_candidates =
        functions_producing_expected_type_at_position(session, uri, text, position)
            .into_iter()
            .map(|(name, typ)| format!("{name} : {typ}"))
            .collect::<Vec<_>>();

    let hole_fill_candidates = hole_fill_candidates_at_position(session, uri, text, position)
        .into_iter()
        .map(|(name, replacement)| json!({ "name": name, "replacement": replacement }))
        .collect::<Vec<_>>();
    let functions_accepting_inferred_type =
        functions_accepting_inferred_type_at_position(session, uri, text, position)
            .into_iter()
            .map(|(name, typ)| format!("{name} : {typ}"))
            .collect::<Vec<_>>();
    let adapters_from_inferred_to_expected =
        adapters_from_inferred_to_expected_at_position(session, uri, text, position)
            .into_iter()
            .map(|(name, typ)| format!("{name} : {typ}"))
            .collect::<Vec<_>>();
    let compatible_with_in_scope_values =
        functions_compatible_with_in_scope_values_at_position(session, uri, text, position);

    let cursor_range = Range {
        start: position,
        end: position,
    };
    let mut local_diagnostics: Vec<Diagnostic> = diagnostics_from_text(session, uri, text)
        .into_iter()
        .filter(|diag| ranges_overlap(diag.range, cursor_range))
        .collect();
    local_diagnostics.sort_by_key(|diag| {
        (
            diag.range.start.line,
            diag.range.start.character,
            diag.range.end.line,
            diag.range.end.character,
            diag.message.clone(),
        )
    });

    let quick_fixes = semantic_quick_fixes_for_range(
        session,
        uri,
        text,
        cursor_range,
        &local_diagnostics,
        document_version,
    );
    let mut quick_fix_titles = quick_fixes
        .iter()
        .filter_map(|item| item.get("title").and_then(Value::as_str))
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    quick_fix_titles.sort();
    quick_fix_titles.dedup();

    Some(json!({
        "expectedType": expected_type,
        "inferredType": inferred_type,
        "inScopeValues": in_scope_values,
        "functionCandidates": function_candidates,
        "holeFillCandidates": hole_fill_candidates,
        "functionsAcceptingInferredType": functions_accepting_inferred_type,
        "adaptersFromInferredToExpectedType": adapters_from_inferred_to_expected,
        "functionsCompatibleWithInScopeValues": compatible_with_in_scope_values,
        "localDiagnostics": local_diagnostics.into_iter().map(|diag| {
            json!({
                "message": diag.message,
                "line": diag.range.start.line,
                "character": diag.range.start.character,
            })
        }).collect::<Vec<_>>(),
        "quickFixes": quick_fixes,
        "quickFixTitles": quick_fix_titles,
        "holes": hole_expected_types_for_document(session, uri, text),
    }))
}

pub fn execute_semantic_loop_apply_quick_fix(
    uri: &Url,
    text: &str,
    current_document_version: Option<i32>,
    quick_fix: &Value,
) -> Option<Value> {
    let Some(obj) = quick_fix.as_object() else {
        return Some(quick_fix_rejected(
            "invalidProposal",
            "quick-fix proposal must be an object",
        ));
    };
    if obj.get("protocolVersion").and_then(Value::as_u64)
        != Some(crate::SEMANTIC_QUICK_FIX_PROTOCOL_VERSION)
    {
        return Some(quick_fix_rejected(
            "unsupportedProtocolVersion",
            "quick-fix proposal does not use the current protocol version",
        ));
    }
    let Some(id) = obj.get("id").and_then(Value::as_str) else {
        return Some(quick_fix_rejected(
            "invalidProposal",
            "quick-fix proposal is missing `id`",
        ));
    };
    let Some(title) = obj.get("title").and_then(Value::as_str) else {
        return Some(quick_fix_rejected(
            "invalidProposal",
            "quick-fix proposal is missing `title`",
        ));
    };
    let kind = obj.get("kind").and_then(Value::as_str);
    let Some(edit_value) = obj.get("edit").cloned() else {
        return Some(quick_fix_rejected(
            "invalidProposal",
            "quick-fix proposal is missing `edit`",
        ));
    };
    let Ok(edit) = serde_json::from_value::<WorkspaceEdit>(edit_value) else {
        return Some(quick_fix_rejected(
            "invalidEdit",
            "quick-fix proposal contains an invalid workspace edit",
        ));
    };
    let Some(precondition) = obj.get("precondition").and_then(Value::as_object) else {
        return Some(quick_fix_rejected(
            "invalidProposal",
            "quick-fix proposal is missing `precondition`",
        ));
    };
    let Some(expected_uri) = precondition.get("uri").and_then(Value::as_str) else {
        return Some(quick_fix_rejected(
            "invalidProposal",
            "quick-fix precondition is missing `uri`",
        ));
    };
    if expected_uri != uri.as_str() {
        return Some(quick_fix_rejected(
            "uriMismatch",
            "quick-fix proposal targets a different document",
        ));
    }
    let Some(expected_content_hash) = precondition.get("contentHash").and_then(Value::as_str)
    else {
        return Some(quick_fix_rejected(
            "invalidProposal",
            "quick-fix precondition is missing `contentHash`",
        ));
    };
    let expected_document_version = match precondition.get("documentVersion") {
        Some(Value::Null) => None,
        Some(value) => {
            let Some(version) = value
                .as_i64()
                .and_then(|version| i32::try_from(version).ok())
            else {
                return Some(quick_fix_rejected(
                    "invalidProposal",
                    "quick-fix precondition has an invalid `documentVersion`",
                ));
            };
            Some(version)
        }
        None => {
            return Some(quick_fix_rejected(
                "invalidProposal",
                "quick-fix precondition is missing `documentVersion`",
            ));
        }
    };

    let expected_id = quick_fix_proposal_id(
        uri,
        expected_content_hash,
        expected_document_version,
        title,
        kind,
        &edit,
    );
    if id != expected_id {
        return Some(quick_fix_rejected(
            "proposalIdMismatch",
            "quick-fix proposal contents do not match its id",
        ));
    }

    let actual_content_hash = text_state_hash(text);
    if expected_content_hash != actual_content_hash {
        return Some(quick_fix_stale(
            "documentContentChanged",
            expected_content_hash,
            &actual_content_hash,
            expected_document_version,
            current_document_version,
        ));
    }
    if expected_document_version != current_document_version {
        return Some(quick_fix_stale(
            "documentVersionChanged",
            expected_content_hash,
            &actual_content_hash,
            expected_document_version,
            current_document_version,
        ));
    }
    if !workspace_edit_only_targets_uri(&edit, uri) {
        return Some(quick_fix_rejected(
            "unsupportedEditScope",
            "quick-fix proposals must target exactly one document",
        ));
    }
    if let Some(expected_version) = expected_document_version
        && !workspace_edit_has_document_version(&edit, uri, expected_version)
    {
        return Some(quick_fix_rejected(
            "unversionedEdit",
            "quick-fix edit is not bound to the expected document version",
        ));
    }
    if apply_workspace_edit_to_text(uri, text, &edit).is_none() {
        return Some(quick_fix_rejected(
            "invalidEdit",
            "quick-fix edit cannot be applied to the proposed document snapshot",
        ));
    }

    Some(json!({
        "status": "ready",
        "quickFix": quick_fix,
    }))
}

pub(crate) fn quick_fix_priority(strategy: BulkQuickFixStrategy, title: &str) -> usize {
    let aggressive_introduce =
        strategy == BulkQuickFixStrategy::Aggressive && title.starts_with("Introduce `let ");
    if title.starts_with("Fill hole with `") {
        0
    } else if title.starts_with("Replace `") || aggressive_introduce {
        1
    } else if title.starts_with("Add wildcard arm") {
        2
    } else if title.starts_with("Wrap expression in list literal") {
        3
    } else if title.starts_with("Unwrap single-item list literal") {
        4
    } else if title.starts_with("Apply expression to missing argument") {
        5
    } else if title.starts_with("Wrap expression in lambda") {
        6
    } else if title.starts_with("Introduce `let ") {
        7
    } else {
        10
    }
}

pub fn best_quick_fix_from_candidates(
    candidates: &[Value],
    strategy: BulkQuickFixStrategy,
) -> Option<Value> {
    candidates
        .iter()
        .min_by_key(|item| {
            let title = item.get("title").and_then(Value::as_str).unwrap_or("");
            let id = item.get("id").and_then(Value::as_str).unwrap_or("");
            (
                quick_fix_priority(strategy, title),
                title.to_string(),
                id.to_string(),
            )
        })
        .cloned()
}

pub fn apply_workspace_edit_to_text(uri: &Url, text: &str, edit: &WorkspaceEdit) -> Option<String> {
    let edits = if let Some(changes) = edit.changes.as_ref() {
        changes.get(uri)?.clone()
    } else if let Some(DocumentChanges::Edits(document_edits)) = &edit.document_changes {
        document_edits
            .iter()
            .find(|document_edit| document_edit.text_document.uri == *uri)?
            .edits
            .iter()
            .map(|edit| match edit {
                OneOf::Left(edit) => edit.clone(),
                OneOf::Right(edit) => edit.text_edit.clone(),
            })
            .collect()
    } else {
        return None;
    };
    if edits.is_empty() {
        return Some(text.to_string());
    }
    let mut with_offsets = Vec::new();
    for edit in edits {
        let start = offset_at(text, edit.range.start)?;
        let end = offset_at(text, edit.range.end)?;
        if start > end || end > text.len() {
            return None;
        }
        with_offsets.push((start, end, edit.new_text));
    }
    with_offsets.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));

    let mut out = text.to_string();
    for (start, end, replacement) in with_offsets {
        out.replace_range(start..end, &replacement);
    }
    Some(out)
}

pub fn text_state_hash(text: &str) -> String {
    blake3::hash(text.as_bytes()).to_hex().to_string()
}

pub fn next_no_improvement_streak(streak: usize, diagnostics_delta: i64) -> usize {
    if diagnostics_delta > 0 { 0 } else { streak + 1 }
}

pub fn execute_semantic_loop_apply_best_quick_fixes(
    session: &AnalysisSession,
    uri: &Url,
    text: &str,
    position: Position,
    max_steps: usize,
    strategy: BulkQuickFixStrategy,
    dry_run: bool,
) -> Option<Value> {
    let cursor_range = Range {
        start: position,
        end: position,
    };
    let mut current_text = text.to_string();
    let mut applied = Vec::new();
    let mut steps = Vec::new();
    let mut stopped_reason = "noQuickFix".to_string();
    let mut stopped_reason_detail = "no quick-fixes available at cursor".to_string();
    let mut no_improvement_streak = 0usize;
    let mut last_diagnostics_delta = 0i64;
    let mut seen_states: HashSet<String> = HashSet::new();
    seen_states.insert(text_state_hash(&current_text));

    for step_index in 0..max_steps {
        let local_diagnostics: Vec<Diagnostic> = diagnostics_from_text(session, uri, &current_text)
            .into_iter()
            .filter(|diag| ranges_overlap(diag.range, cursor_range))
            .collect();
        let diagnostics_before = local_diagnostics
            .iter()
            .map(|diag| {
                json!({
                    "message": diag.message,
                    "line": diag.range.start.line,
                    "character": diag.range.start.character,
                })
            })
            .collect::<Vec<_>>();
        let quick_fixes = semantic_quick_fixes_for_range(
            session,
            uri,
            &current_text,
            cursor_range,
            &local_diagnostics,
            None,
        );
        let Some(best) = best_quick_fix_from_candidates(&quick_fixes, strategy) else {
            stopped_reason = "noQuickFix".to_string();
            stopped_reason_detail = "no candidate quick-fix was available".to_string();
            break;
        };
        let edit_value = best.get("edit").cloned().unwrap_or(Value::Null);
        let Ok(edit) = serde_json::from_value::<WorkspaceEdit>(edit_value) else {
            stopped_reason = "invalidEdit".to_string();
            stopped_reason_detail = "selected quick-fix edit was invalid".to_string();
            break;
        };
        let Some(next_text) = apply_workspace_edit_to_text(uri, &current_text, &edit) else {
            stopped_reason = "applyFailed".to_string();
            stopped_reason_detail = "failed to apply selected workspace edit".to_string();
            break;
        };
        if next_text == current_text {
            stopped_reason = "noTextChange".to_string();
            stopped_reason_detail = "selected quick-fix did not change text".to_string();
            break;
        }
        let next_hash = text_state_hash(&next_text);
        if seen_states.contains(&next_hash) {
            stopped_reason = "cycleDetected".to_string();
            stopped_reason_detail = "next text state already seen in this run".to_string();
            break;
        }
        let diagnostics_after_step: Vec<Value> = diagnostics_from_text(session, uri, &next_text)
            .into_iter()
            .filter(|diag| ranges_overlap(diag.range, cursor_range))
            .map(|diag| {
                json!({
                    "message": diag.message,
                    "line": diag.range.start.line,
                    "character": diag.range.start.character,
                })
            })
            .collect();
        let before_count = diagnostics_before.len();
        let after_count = diagnostics_after_step.len();
        let diagnostics_delta = (before_count as i64) - (after_count as i64);
        last_diagnostics_delta = diagnostics_delta;
        no_improvement_streak =
            next_no_improvement_streak(no_improvement_streak, diagnostics_delta);
        steps.push(json!({
            "index": step_index,
            "quickFix": best.clone(),
            "diagnosticsBefore": diagnostics_before,
            "diagnosticsAfter": diagnostics_after_step,
            "diagnosticsBeforeCount": before_count,
            "diagnosticsAfterCount": after_count,
            "diagnosticsDelta": diagnostics_delta,
            "noImprovementStreak": no_improvement_streak,
        }));
        applied.push(best);
        current_text = next_text;
        seen_states.insert(next_hash);
        if no_improvement_streak >= NO_IMPROVEMENT_STREAK_LIMIT {
            stopped_reason = "noImprovementStreak".to_string();
            stopped_reason_detail =
                format!("diagnostics did not improve for {NO_IMPROVEMENT_STREAK_LIMIT} step(s)");
            break;
        }
        stopped_reason = "maxStepsReached".to_string();
        stopped_reason_detail = format!("reached maxSteps={max_steps}");
    }

    let diagnostics_after: Vec<Value> = diagnostics_from_text(session, uri, &current_text)
        .into_iter()
        .filter(|diag| ranges_overlap(diag.range, cursor_range))
        .map(|diag| {
            json!({
                "message": diag.message,
                "line": diag.range.start.line,
                "character": diag.range.start.character,
            })
        })
        .collect();

    Some(json!({
        "strategy": strategy.as_str(),
        "dryRun": dry_run,
        "appliedQuickFixes": applied,
        "appliedCount": applied.len(),
        "steps": steps,
        "updatedText": current_text,
        "localDiagnosticsAfter": diagnostics_after,
        "stoppedReason": stopped_reason,
        "stoppedReasonDetail": stopped_reason_detail,
        "lastDiagnosticsDelta": last_diagnostics_delta,
        "noImprovementStreak": no_improvement_streak,
        "seenStatesCount": seen_states.len(),
    }))
}

pub fn hole_expected_types_for_document(
    session: &AnalysisSession,
    uri: &Url,
    text: &str,
) -> Vec<Value> {
    let mut holes = Vec::new();

    // First-class holes: parse `?` nodes directly.
    if let Ok((_tokens, program)) = session.tokenize_and_parse_cached(uri, text)
        && let Some(body) = program.body_with_fns()
    {
        let mut spans = Vec::new();
        collect_hole_spans(body.as_ref(), &mut spans);
        for span in spans {
            let pos = span_to_range(span).start;
            if let Some(expected_type) = expected_type_at_position(session, uri, text, pos)
                .or_else(|| expected_type_from_syntax_context(session, uri, text, pos))
            {
                holes.push(json!({
                    "name": "?",
                    "line": pos.line,
                    "character": pos.character,
                    "expectedType": expected_type
                }));
            }
        }
    }

    // Backward-compat fallback: `_foo` placeholder variables still treated as holes.
    let diagnostics = diagnostics_from_text(session, uri, text);
    for diag in diagnostics {
        let Some(name) = unknown_var_name_from_message(&diag.message) else {
            continue;
        };
        if !is_hole_name(name) {
            continue;
        }
        if !range_is_usable_for_text(text, diag.range) {
            continue;
        }
        let pos = diag.range.start;
        if let Some(expected_type) = expected_type_at_position(session, uri, text, pos)
            .or_else(|| expected_type_from_syntax_context(session, uri, text, pos))
        {
            holes.push(json!({
                "name": name,
                "line": pos.line,
                "character": pos.character,
                "expectedType": expected_type
            }));
        }
    }
    holes.sort_by_key(|item| {
        let line = item.get("line").and_then(Value::as_u64).unwrap_or(0);
        let ch = item.get("character").and_then(Value::as_u64).unwrap_or(0);
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        (line, ch, name)
    });
    holes.dedup_by(|a, b| {
        a.get("name") == b.get("name")
            && a.get("line") == b.get("line")
            && a.get("character") == b.get("character")
    });
    if holes.len() > MAX_SEMANTIC_HOLES {
        holes.truncate(MAX_SEMANTIC_HOLES);
    }
    holes
}

pub(crate) fn collect_hole_spans(expr: &Expr, out: &mut Vec<Span>) {
    match expr {
        Expr::Hole(span) => out.push(*span),
        Expr::App(_, f, x) => {
            collect_hole_spans(f, out);
            collect_hole_spans(x, out);
        }
        Expr::Project(_, base, _) => collect_hole_spans(base, out),
        Expr::Lam(_, _scope, _param, _ann, _constraints, body) => collect_hole_spans(body, out),
        Expr::Let(_, _var, _, _ann, def, body) => {
            collect_hole_spans(def, out);
            collect_hole_spans(body, out);
        }
        Expr::LetRec(_, bindings, body) => {
            for (_var, _, _ann, def) in bindings {
                collect_hole_spans(def, out);
            }
            collect_hole_spans(body, out);
        }
        Expr::Ite(_, cond, then_expr, else_expr) => {
            collect_hole_spans(cond, out);
            collect_hole_spans(then_expr, out);
            collect_hole_spans(else_expr, out);
        }
        Expr::Match(_, scrutinee, arms) => {
            collect_hole_spans(scrutinee, out);
            for (_pat, arm) in arms {
                collect_hole_spans(arm, out);
            }
        }
        Expr::Ann(_, inner, _) => collect_hole_spans(inner, out),
        Expr::Tuple(_, elems) | Expr::List(_, elems) => {
            for elem in elems {
                collect_hole_spans(elem, out);
            }
        }
        Expr::Dict(_, kvs) => {
            for value in kvs.values() {
                collect_hole_spans(value, out);
            }
        }
        Expr::RecordUpdate(_, base, updates) => {
            collect_hole_spans(base, out);
            for value in updates.values() {
                collect_hole_spans(value, out);
            }
        }
        Expr::Var(_)
        | Expr::Bool(..)
        | Expr::Uint(..)
        | Expr::Int(..)
        | Expr::Float(..)
        | Expr::Char(..)
        | Expr::String(..)
        | Expr::Uuid(..)
        | Expr::DateTime(..) => {}
    }
}

pub(crate) fn expected_type_from_syntax_context(
    session: &AnalysisSession,
    uri: &Url,
    text: &str,
    position: Position,
) -> Option<String> {
    let (_tokens, program) = session.tokenize_and_parse_cached(uri, text).ok()?;
    let pos = lsp_to_rex_position(position);

    fn visit(expr: &Expr, pos: RexPosition) -> Option<String> {
        if !position_in_span(pos, *expr.span()) {
            return None;
        }
        match expr {
            Expr::Let(_span, _name, _, ann, def, body) => {
                if position_in_span(pos, *def.span())
                    && let Some(ann) = ann
                {
                    return Some(ann.to_string());
                }
                visit(def.as_ref(), pos).or_else(|| visit(body.as_ref(), pos))
            }
            Expr::Ann(_span, inner, ann) => {
                if position_in_span(pos, *inner.span()) {
                    return Some(ann.to_string());
                }
                visit(inner.as_ref(), pos)
            }
            Expr::Ite(_span, cond, then_expr, else_expr) => {
                if position_in_span(pos, *cond.span()) {
                    return Some("Bool".to_string());
                }
                visit(cond.as_ref(), pos)
                    .or_else(|| visit(then_expr.as_ref(), pos))
                    .or_else(|| visit(else_expr.as_ref(), pos))
            }
            Expr::App(_span, f, x) => visit(f.as_ref(), pos).or_else(|| visit(x.as_ref(), pos)),
            Expr::Project(_span, base, _field) => visit(base.as_ref(), pos),
            Expr::Lam(_span, _scope, _param, _ann, _constraints, body) => visit(body.as_ref(), pos),
            Expr::LetRec(_span, bindings, body) => {
                for (_name, _, _ann, def) in bindings {
                    if let Some(found) = visit(def.as_ref(), pos) {
                        return Some(found);
                    }
                }
                visit(body.as_ref(), pos)
            }
            Expr::Match(_span, scrutinee, arms) => {
                if let Some(found) = visit(scrutinee.as_ref(), pos) {
                    return Some(found);
                }
                for (_pat, arm) in arms {
                    if let Some(found) = visit(arm.as_ref(), pos) {
                        return Some(found);
                    }
                }
                None
            }
            Expr::Tuple(_span, elems) | Expr::List(_span, elems) => {
                for elem in elems {
                    if let Some(found) = visit(elem.as_ref(), pos) {
                        return Some(found);
                    }
                }
                None
            }
            Expr::Dict(_span, kvs) => {
                for value in kvs.values() {
                    if let Some(found) = visit(value.as_ref(), pos) {
                        return Some(found);
                    }
                }
                None
            }
            Expr::RecordUpdate(_span, base, updates) => {
                if let Some(found) = visit(base.as_ref(), pos) {
                    return Some(found);
                }
                for value in updates.values() {
                    if let Some(found) = visit(value.as_ref(), pos) {
                        return Some(found);
                    }
                }
                None
            }
            Expr::Var(_)
            | Expr::Bool(..)
            | Expr::Uint(..)
            | Expr::Int(..)
            | Expr::Float(..)
            | Expr::Char(..)
            | Expr::String(..)
            | Expr::Uuid(..)
            | Expr::DateTime(..)
            | Expr::Hole(..) => None,
        }
    }

    let body = program.body_with_fns()?;
    visit(body.as_ref(), pos)
}

pub fn command_uri_and_position(arguments: &[Value]) -> Option<(Url, Position)> {
    if arguments.len() >= 3 {
        let uri = arguments.first()?.as_str()?;
        let line = arguments.get(1)?.as_u64()? as u32;
        let character = arguments.get(2)?.as_u64()? as u32;
        let uri = Url::parse(uri).ok()?;
        return Some((uri, Position { line, character }));
    }

    let obj = arguments.first()?.as_object()?;
    let uri = obj.get("uri")?.as_str()?;
    let line = obj.get("line")?.as_u64()? as u32;
    let character = obj.get("character")?.as_u64()? as u32;
    let uri = Url::parse(uri).ok()?;
    Some((uri, Position { line, character }))
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn command_uri(arguments: &[Value]) -> Option<Url> {
    if arguments.is_empty() {
        return None;
    }
    if let Some(uri) = arguments.first().and_then(Value::as_str) {
        return Url::parse(uri).ok();
    }
    let obj = arguments.first()?.as_object()?;
    let uri = obj.get("uri")?.as_str()?;
    Url::parse(uri).ok()
}

pub fn command_uri_and_quick_fix(arguments: &[Value]) -> Option<(Url, Value)> {
    if arguments.len() >= 2 {
        let uri = arguments.first()?.as_str()?;
        let quick_fix = arguments.get(1)?.clone();
        quick_fix.as_object()?;
        let uri = Url::parse(uri).ok()?;
        return Some((uri, quick_fix));
    }

    let obj = arguments.first()?.as_object()?;
    let uri = obj.get("uri")?.as_str()?;
    let quick_fix = obj.get("quickFix")?.clone();
    quick_fix.as_object()?;
    let uri = Url::parse(uri).ok()?;
    Some((uri, quick_fix))
}

pub fn command_uri_position_max_steps_strategy_and_dry_run(
    arguments: &[Value],
) -> Option<(Url, Position, usize, BulkQuickFixStrategy, bool)> {
    if arguments.len() >= 3 {
        let uri = arguments.first()?.as_str()?;
        let line = arguments.get(1)?.as_u64()? as u32;
        let character = arguments.get(2)?.as_u64()? as u32;
        let max_steps = arguments
            .get(3)
            .and_then(Value::as_u64)
            .map(|n| n as usize)
            .unwrap_or(3);
        let strategy = arguments
            .get(4)
            .and_then(Value::as_str)
            .map(BulkQuickFixStrategy::parse)
            .unwrap_or(BulkQuickFixStrategy::Conservative);
        let dry_run = arguments.get(5).and_then(Value::as_bool).unwrap_or(false);
        let uri = Url::parse(uri).ok()?;
        return Some((
            uri,
            Position { line, character },
            max_steps.clamp(1, 20),
            strategy,
            dry_run,
        ));
    }

    let obj = arguments.first()?.as_object()?;
    let uri = obj.get("uri")?.as_str()?;
    let line = obj.get("line")?.as_u64()? as u32;
    let character = obj.get("character")?.as_u64()? as u32;
    let max_steps = obj
        .get("maxSteps")
        .and_then(Value::as_u64)
        .map(|n| n as usize)
        .unwrap_or(3)
        .clamp(1, 20);
    let strategy = obj
        .get("strategy")
        .and_then(Value::as_str)
        .map(BulkQuickFixStrategy::parse)
        .unwrap_or(BulkQuickFixStrategy::Conservative);
    let dry_run = obj.get("dryRun").and_then(Value::as_bool).unwrap_or(false);
    let uri = Url::parse(uri).ok()?;
    Some((
        uri,
        Position { line, character },
        max_steps,
        strategy,
        dry_run,
    ))
}

pub(crate) fn hover_type_in_expr(
    ts: &mut TypeSystem,
    expr: &Expr,
    typed: &TypedExpr,
    pos: RexPosition,
    name: &str,
    name_span: Span,
    name_is_ident: bool,
) -> Option<HoverType> {
    fn span_contains_pos(span: Span, pos: RexPosition) -> bool {
        position_in_span(pos, span)
    }

    fn span_contains_span(outer: Span, inner: Span) -> bool {
        position_leq(outer.begin, inner.begin) && position_leq(inner.end, outer.end)
    }

    fn span_size(span: Span) -> (usize, usize) {
        (
            span.end.line.saturating_sub(span.begin.line),
            span.end.column.saturating_sub(span.begin.column),
        )
    }

    fn peel_fun(ty: &Type) -> (Vec<Type>, Type) {
        let mut args = Vec::new();
        let mut cur = ty.clone();
        while let TypeKind::Fun(a, b) = cur.as_ref() {
            args.push(a.clone());
            cur = b.clone();
        }
        (args, cur)
    }

    fn add_bindings_from_pattern(
        ts: &mut TypeSystem,
        scrutinee_ty: &Type,
        pat: &Pattern,
        out: &mut HashMap<String, Type>,
    ) {
        match pat {
            Pattern::Wildcard(..) => {}
            Pattern::Var(v) => {
                out.insert(v.name.as_ref().to_string(), scrutinee_ty.clone());
            }
            Pattern::Named(_span, ctor, args) => {
                let ctor_name = ctor.to_dotted_symbol();
                let Some(schemes) = ts.env.lookup(&ctor_name) else {
                    return;
                };
                let Some(scheme) = schemes.first() else {
                    return;
                };

                let (_preds, ctor_ty) = instantiate(scheme, &mut ts.supply);
                let (arg_tys, result_ty) = peel_fun(&ctor_ty);
                let Ok(s) = unify(&result_ty, scrutinee_ty) else {
                    return;
                };

                for (subpat, arg_ty) in args.iter().zip(arg_tys.iter()) {
                    add_bindings_from_pattern(ts, &arg_ty.apply(&s), subpat, out);
                }
            }
            Pattern::Tuple(_span, elems) => {
                let elem_tys: Vec<Type> = (0..elems.len())
                    .map(|_| Type::var(ts.supply.fresh(None)))
                    .collect();
                let expected = Type::tuple(elem_tys.clone());
                let Ok(s) = unify(scrutinee_ty, &expected) else {
                    return;
                };
                for (p, ty) in elems.iter().zip(elem_tys.iter()) {
                    add_bindings_from_pattern(ts, &ty.apply(&s), p, out);
                }
            }
            Pattern::List(_span, elems) => {
                let tv = ts.supply.fresh(None);
                let elem = Type::var(tv.clone());
                let list_ty = Type::app(Type::builtin(BuiltinTypeId::List), elem.clone());
                let Ok(s) = unify(scrutinee_ty, &list_ty) else {
                    return;
                };
                let elem_ty = elem.apply(&s);
                for p in elems {
                    add_bindings_from_pattern(ts, &elem_ty, p, out);
                }
            }
            Pattern::Cons(_span, head, tail) => {
                let tv = ts.supply.fresh(None);
                let elem = Type::var(tv.clone());
                let list_ty = Type::app(Type::builtin(BuiltinTypeId::List), elem.clone());
                let Ok(s) = unify(scrutinee_ty, &list_ty) else {
                    return;
                };
                let elem_ty = elem.apply(&s);
                let list_ty = list_ty.apply(&s);
                add_bindings_from_pattern(ts, &elem_ty, head.as_ref(), out);
                add_bindings_from_pattern(ts, &list_ty, tail.as_ref(), out);
            }
            Pattern::Dict(_span, keys) => match scrutinee_ty.as_ref() {
                TypeKind::Record(fields) => {
                    for (key, pat) in keys {
                        if let Some((_, ty)) = fields.iter().find(|(n, _)| n == key) {
                            add_bindings_from_pattern(ts, ty, pat, out);
                        }
                    }
                }
                _ => {
                    let tv = ts.supply.fresh(None);
                    let elem = Type::var(tv.clone());
                    let dict_ty = Type::app(Type::builtin(BuiltinTypeId::Dict), elem.clone());
                    let Ok(s) = unify(scrutinee_ty, &dict_ty) else {
                        return;
                    };
                    let elem_ty = elem.apply(&s);
                    for (_key, pat) in keys {
                        add_bindings_from_pattern(ts, &elem_ty, pat, out);
                    }
                }
            },
        }
    }

    struct VisitCtx<'a> {
        pos: RexPosition,
        name: &'a str,
        name_span: Span,
        name_is_ident: bool,
        best: &'a mut Option<HoverType>,
    }

    fn visit(ts: &mut TypeSystem, expr: &Expr, typed: &TypedExpr, ctx: &mut VisitCtx<'_>) {
        if !span_contains_pos(*expr.span(), ctx.pos) {
            return;
        }

        let consider = |best: &mut Option<HoverType>, candidate: HoverType| {
            let take = best
                .as_ref()
                .is_none_or(|b| span_size(candidate.span) < span_size(b.span));
            if take {
                *best = Some(candidate);
            }
        };

        // 1) Pattern-bound variables (match arms).
        if ctx.name_is_ident
            && let (
                Expr::Match(_span, _scrutinee, arms),
                TypedExprKind::Match {
                    scrutinee,
                    arms: typed_arms,
                },
            ) = (&expr, typed.kind.as_ref())
            && span_contains_span(*expr.span(), ctx.name_span)
        {
            for ((_pat, _arm_body), (typed_pat, _typed_arm_body)) in
                arms.iter().zip(typed_arms.iter())
            {
                // The `Pattern` is cloned into the typed tree; use either.
                let pat_span = *typed_pat.span();
                if span_contains_span(pat_span, ctx.name_span) {
                    let mut bindings: HashMap<String, Type> = HashMap::new();
                    add_bindings_from_pattern(ts, &scrutinee.typ, typed_pat, &mut bindings);
                    if let Some(ty) = bindings.get(ctx.name) {
                        consider(
                            ctx.best,
                            HoverType {
                                span: ctx.name_span,
                                label: ctx.name.to_string(),
                                typ: ty.to_string(),
                                overloads: Vec::new(),
                            },
                        );
                    }
                    break;
                }
            }
        }

        // 2) Binding sites: `let x = ...` and lambda params.
        match (expr, typed.kind.as_ref()) {
            (
                Expr::Let(_span, binding, _, _ann, def, body),
                TypedExprKind::Let {
                    def: tdef,
                    body: tbody,
                    ..
                },
            ) => {
                if span_contains_pos(binding.span, ctx.pos) {
                    consider(
                        ctx.best,
                        HoverType {
                            span: binding.span,
                            label: binding.name.as_ref().to_string(),
                            typ: tdef.typ.to_string(),
                            overloads: Vec::new(),
                        },
                    );
                }
                visit(ts, def.as_ref(), tdef.as_ref(), ctx);
                visit(ts, body.as_ref(), tbody.as_ref(), ctx);
            }
            (
                Expr::LetRec(_span, bindings, body),
                TypedExprKind::LetRec {
                    bindings: typed_bindings,
                    body: typed_body,
                },
            ) => {
                for ((binding, _, _ann, def), (_name, typed_def)) in
                    bindings.iter().zip(typed_bindings.iter())
                {
                    if span_contains_pos(binding.span, ctx.pos) {
                        consider(
                            ctx.best,
                            HoverType {
                                span: binding.span,
                                label: binding.name.as_ref().to_string(),
                                typ: typed_def.typ.to_string(),
                                overloads: Vec::new(),
                            },
                        );
                    }
                    visit(ts, def.as_ref(), typed_def, ctx);
                }
                visit(ts, body.as_ref(), typed_body.as_ref(), ctx);
            }
            (
                Expr::Lam(_span, _scope, param, _ann, _constraints, body),
                TypedExprKind::Lam { body: tbody, .. },
            ) => {
                if span_contains_pos(param.span, ctx.pos) {
                    let param_ty = match typed.typ.as_ref() {
                        TypeKind::Fun(a, _b) => a.to_string(),
                        _ => "<unknown>".to_string(),
                    };
                    consider(
                        ctx.best,
                        HoverType {
                            span: param.span,
                            label: param.name.as_ref().to_string(),
                            typ: param_ty,
                            overloads: Vec::new(),
                        },
                    );
                }
                visit(ts, body.as_ref(), tbody.as_ref(), ctx);
            }
            (Expr::Var(v), TypedExprKind::Var { overloads, .. })
                if span_contains_pos(v.span, ctx.pos) =>
            {
                consider(
                    ctx.best,
                    HoverType {
                        span: v.span,
                        label: v.name.as_ref().to_string(),
                        typ: typed.typ.to_string(),
                        overloads: overloads.iter().map(|t| t.to_string()).collect(),
                    },
                );
            }
            (Expr::Ann(_span, inner, _ann), _) => {
                visit(ts, inner.as_ref(), typed, ctx);
            }
            (Expr::Tuple(_span, elems), TypedExprKind::Tuple(telems)) => {
                for (e, t) in elems.iter().zip(telems.iter()) {
                    visit(ts, e.as_ref(), t, ctx);
                }
            }
            (Expr::List(_span, elems), TypedExprKind::List(telems)) => {
                for (e, t) in elems.iter().zip(telems.iter()) {
                    visit(ts, e.as_ref(), t, ctx);
                }
            }
            (Expr::Dict(_span, kvs), TypedExprKind::Dict(tkvs)) => {
                for (k, v) in kvs {
                    if let Some(tv) = tkvs.get(k) {
                        visit(ts, v.as_ref(), tv, ctx);
                    }
                }
            }
            (
                Expr::RecordUpdate(_span, base, updates),
                TypedExprKind::RecordUpdate {
                    base: tbase,
                    updates: tupdates,
                },
            ) => {
                visit(ts, base.as_ref(), tbase.as_ref(), ctx);
                for (k, v) in updates {
                    if let Some(tv) = tupdates.get(k) {
                        visit(ts, v.as_ref(), tv, ctx);
                    }
                }
            }
            (Expr::App(_span, f, x), TypedExprKind::App(tf, tx)) => {
                visit(ts, f.as_ref(), tf.as_ref(), ctx);
                visit(ts, x.as_ref(), tx.as_ref(), ctx);
            }
            (Expr::Project(_span, e, _field), TypedExprKind::Project { expr: te, .. }) => {
                visit(ts, e.as_ref(), te.as_ref(), ctx);
            }
            (
                Expr::Ite(_span, c, t, e),
                TypedExprKind::Ite {
                    cond,
                    then_expr,
                    else_expr,
                },
            ) => {
                visit(ts, c.as_ref(), cond.as_ref(), ctx);
                visit(ts, t.as_ref(), then_expr.as_ref(), ctx);
                visit(ts, e.as_ref(), else_expr.as_ref(), ctx);
            }
            (
                Expr::Match(_span, scrutinee, arms),
                TypedExprKind::Match {
                    scrutinee: tscrut,
                    arms: tarms,
                },
            ) => {
                visit(ts, scrutinee.as_ref(), tscrut.as_ref(), ctx);
                for ((_pat, arm_body), (_tpat, tarm_body)) in arms.iter().zip(tarms.iter()) {
                    visit(ts, arm_body.as_ref(), tarm_body, ctx);
                }
            }
            _ => {}
        }
    }

    let mut best = None;
    let mut ctx = VisitCtx {
        pos,
        name,
        name_span,
        name_is_ident,
        best: &mut best,
    };
    visit(ts, expr, typed, &mut ctx);
    best
}

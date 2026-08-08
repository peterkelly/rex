use crate::prelude::*;
use crate::{imports::*, shared::*};

pub(crate) fn range_contains_position(range: Range, position: Position) -> bool {
    let after_start = position.line > range.start.line
        || (position.line == range.start.line && position.character >= range.start.character);
    let before_end = position.line < range.end.line
        || (position.line == range.end.line && position.character < range.end.character);
    after_start && before_end
}

pub(crate) fn range_touches_position(range: Range, position: Position) -> bool {
    // LSP ranges are end-exclusive, but VS Code often sends positions at the
    // *end* of a word (especially for single-character identifiers). For user
    // interactions like go-to-definition, treating `position == end` as “still
    // on that token” is a better UX trade.
    if range_contains_position(range, position) {
        return true;
    }
    if position.line != range.end.line || position.character != range.end.character {
        return false;
    }
    // Exclude the degenerate case (empty range), just in case.
    position.line != range.start.line || position.character != range.start.character
}

pub(crate) fn span_to_range(span: Span) -> Range {
    Range {
        start: position_from_span(span.begin.line, span.begin.column),
        end: position_from_span(span.end.line, span.end.column),
    }
}

pub(crate) fn position_from_span(line: usize, column: usize) -> Position {
    Position {
        line: line.saturating_sub(1) as u32,
        character: column.saturating_sub(1) as u32,
    }
}

pub(crate) fn hover_contents(word: &str) -> Option<HoverContents> {
    if let Some(doc) = keyword_doc(word) {
        return Some(markdown_hover(word, "keyword", doc));
    }

    if let Some(doc) = type_doc(word) {
        return Some(markdown_hover(word, "type", doc));
    }

    if let Some(doc) = value_doc(word) {
        return Some(markdown_hover(word, "value", doc));
    }

    None
}

pub(crate) fn markdown_hover(word: &str, kind: &str, doc: &str) -> HoverContents {
    HoverContents::Markup(MarkupContent {
        kind: MarkupKind::Markdown,
        value: format!("**{}** {}\n\n{}", word, kind, doc),
    })
}

pub(crate) fn keyword_doc(word: &str) -> Option<&'static str> {
    match word {
        "let" => Some("Introduces local bindings."),
        "in" => Some("Begins the expression body for a let binding."),
        "type" => Some("Declares a type or ADT."),
        "match" => Some("Starts a pattern match expression."),
        "with" => {
            Some("Separates a match scrutinee from its arm block, or introduces a record update.")
        }
        "case" => Some("Introduces a match arm."),
        "if" => Some("Conditional expression keyword."),
        "then" => Some("Conditional expression branch."),
        "else" => Some("Fallback branch of a conditional expression."),
        "as" => Some("Type ascription or aliasing keyword."),
        "for" => Some("List/dict comprehension keyword (when supported)."),
        "is" => Some("Type assertion keyword."),
        _ => None,
    }
}

pub(crate) fn type_doc(word: &str) -> Option<&'static str> {
    match word {
        "Bool" => Some("Boolean type."),
        "String" => Some("UTF-8 string type."),
        "UUID" => Some("UUID type."),
        "Hash" => Some("BLAKE3 hash type."),
        "DateTime" => Some("Datetime type."),
        "u8" => Some("Unsigned 8-bit integer."),
        "u16" => Some("Unsigned 16-bit integer."),
        "u32" => Some("Unsigned 32-bit integer."),
        "u64" => Some("Unsigned 64-bit integer."),
        "i8" => Some("Signed 8-bit integer."),
        "i16" => Some("Signed 16-bit integer."),
        "i32" => Some("Signed 32-bit integer."),
        "i64" => Some("Signed 64-bit integer."),
        "f32" => Some("32-bit float."),
        "f64" => Some("64-bit float."),
        "List" => Some("List type constructor."),
        "Dict" => Some("Dictionary type constructor."),
        "Option" => Some("Optional type constructor."),
        "Result" => Some("Result type constructor."),
        _ => None,
    }
}

pub(crate) fn value_doc(word: &str) -> Option<&'static str> {
    match word {
        "true" => Some("Boolean literal."),
        "false" => Some("Boolean literal."),
        "null" => Some("Null literal."),
        "Some" => Some("Option constructor."),
        "None" => Some("Option empty constructor."),
        "Ok" => Some("Result success constructor."),
        "Err" => Some("Result error constructor."),
        _ => None,
    }
}

pub(crate) fn completion_items(
    session: &AnalysisSession,
    uri: &Url,
    text: &str,
    position: Position,
) -> Vec<CompletionItem> {
    let field_mode = is_field_completion(text, position);
    let base_ident = if field_mode {
        field_base_ident(text, position)
    } else {
        None
    };
    if let Ok((_tokens, program)) = session.tokenize_and_parse_cached(uri, text) {
        return completion_items_from_program(
            session,
            &program,
            position,
            field_mode,
            base_ident.as_deref(),
            uri,
        );
    }

    completion_items_fallback(text, base_ident.as_deref(), field_mode)
}

pub(crate) fn completion_items_from_program(
    session: &AnalysisSession,
    compilation_unit: &CompilationUnit,
    position: Position,
    field_mode: bool,
    base_ident: Option<&str>,
    uri: &Url,
) -> Vec<CompletionItem> {
    if field_mode {
        if let Some(base_ident) = base_ident
            && let Ok(exports) =
                completion_exports_for_module_alias(session, uri, compilation_unit, base_ident)
            && !exports.is_empty()
        {
            return exports
                .into_iter()
                .map(|label| completion_item(label, CompletionItemKind::FIELD))
                .collect();
        }
        if let Some(fields) = field_completion_for_position(compilation_unit, position, base_ident)
        {
            return fields
                .into_iter()
                .map(|label| completion_item(label, CompletionItemKind::FIELD))
                .collect();
        }
        return Vec::new();
    }

    let mut value_kinds = values_in_scope_at_position(compilation_unit, position);
    let pos = lsp_to_rex_position(position);
    for decl in &compilation_unit.decls {
        let Decl::Import(id) = decl else { continue };
        if position_in_span(pos, id.span) || position_leq(id.span.end, pos) {
            value_kinds
                .entry(id.alias.as_ref().to_string())
                .or_insert(CompletionItemKind::MODULE);
        }
    }
    for value in BUILTIN_VALUES {
        value_kinds
            .entry((*value).to_string())
            .or_insert(CompletionItemKind::VARIABLE);
    }
    for (value, kind) in prelude_completion_values() {
        value_kinds.entry(value.clone()).or_insert(*kind);
    }
    for ctor in collect_constructors(compilation_unit) {
        value_kinds
            .entry(ctor)
            .or_insert(CompletionItemKind::CONSTRUCTOR);
    }

    let mut type_names = collect_type_names(compilation_unit);
    type_names.extend(BUILTIN_TYPES.iter().map(|value| value.to_string()));

    let mut items = Vec::new();
    items.extend(
        value_kinds
            .into_iter()
            .map(|(label, kind)| completion_item(label, kind)),
    );
    items.extend(
        type_names
            .into_iter()
            .map(|label| completion_item(label, CompletionItemKind::CLASS)),
    );

    items
}

pub(crate) fn completion_items_fallback(
    text: &str,
    base_ident: Option<&str>,
    field_mode: bool,
) -> Vec<CompletionItem> {
    let mut identifiers: HashMap<String, CompletionItemKind> = HashMap::new();

    if let Ok(tokens) = Token::tokenize(text) {
        identifiers.extend(function_defs_from_tokens(&tokens));

        let mut index = 0usize;
        while index < tokens.items.len() {
            if let Token::Ident(name, ..) = &tokens.items[index] {
                identifiers
                    .entry(name.clone())
                    .or_insert(CompletionItemKind::VARIABLE);
            }
            index += 1;
        }

        if field_mode {
            if let Some(base_ident) = base_ident
                && let Some(fields) = fallback_field_map(&tokens).get(base_ident)
            {
                return fields
                    .iter()
                    .cloned()
                    .map(|label| completion_item(label, CompletionItemKind::FIELD))
                    .collect();
            }
            return Vec::new();
        }
    }

    let mut items: Vec<CompletionItem> = identifiers
        .into_iter()
        .map(|(label, kind)| completion_item(label, kind))
        .collect();
    items.extend(
        BUILTIN_TYPES
            .iter()
            .map(|label| completion_item((*label).to_string(), CompletionItemKind::CLASS)),
    );
    items
}

pub(crate) fn completion_item(label: String, kind: CompletionItemKind) -> CompletionItem {
    CompletionItem {
        label,
        kind: Some(kind),
        ..CompletionItem::default()
    }
}

pub(crate) fn values_in_scope_at_position(
    compilation_unit: &CompilationUnit,
    position: Position,
) -> HashMap<String, CompletionItemKind> {
    let pos = lsp_to_rex_position(position);
    let Some(expr) = compilation_unit.body_with_fns() else {
        return HashMap::new();
    };
    values_in_scope_at_expr(expr.as_ref(), pos, &mut Vec::new()).unwrap_or_default()
}

pub(crate) fn values_in_scope_at_expr(
    expr: &Expr,
    position: RexPosition,
    scope: &mut Vec<(String, CompletionItemKind)>,
) -> Option<HashMap<String, CompletionItemKind>> {
    if !position_in_span(position, *expr.span()) {
        return None;
    }

    fn scope_to_map(scope: &[(String, CompletionItemKind)]) -> HashMap<String, CompletionItemKind> {
        // If a name appears multiple times, prefer the most “specific” kind.
        // (A function is still a value, but it’s nicer to present it as a function.)
        let mut map = HashMap::new();
        for (name, kind) in scope {
            let slot = map.entry(name.clone()).or_insert(*kind);
            if *slot != CompletionItemKind::FUNCTION && *kind == CompletionItemKind::FUNCTION {
                *slot = *kind;
            }
        }
        map
    }

    match expr {
        Expr::Let(_span, var, _, _ann, def, body) => {
            if position_in_span(position, *def.span()) {
                return values_in_scope_at_expr(def, position, scope)
                    .or_else(|| Some(scope_to_map(scope)));
            }

            if position_in_span(position, *body.span()) {
                let kind = matches!(def.as_ref(), Expr::Lam(..))
                    .then_some(CompletionItemKind::FUNCTION)
                    .unwrap_or(CompletionItemKind::VARIABLE);
                scope.push((var.name.to_string(), kind));
                let out = values_in_scope_at_expr(body, position, scope)
                    .or_else(|| Some(scope_to_map(scope)));
                scope.pop();
                return out;
            }

            Some(scope_to_map(scope))
        }
        Expr::LetRec(_span, bindings, body) => {
            let base_len = scope.len();
            scope.extend(bindings.iter().map(|(var, _, _ann, def)| {
                let kind = matches!(def.as_ref(), Expr::Lam(..))
                    .then_some(CompletionItemKind::FUNCTION)
                    .unwrap_or(CompletionItemKind::VARIABLE);
                (var.name.to_string(), kind)
            }));

            for (_, _, _, def) in bindings {
                if position_in_span(position, *def.span()) {
                    let out = values_in_scope_at_expr(def, position, scope)
                        .or_else(|| Some(scope_to_map(scope)));
                    scope.truncate(base_len);
                    return out;
                }
            }

            if position_in_span(position, *body.span()) {
                let out = values_in_scope_at_expr(body, position, scope)
                    .or_else(|| Some(scope_to_map(scope)));
                scope.truncate(base_len);
                return out;
            }

            scope.truncate(base_len);
            Some(scope_to_map(scope))
        }
        Expr::Lam(_span, _scope, param, _ann, _constraints, body) => {
            if position_in_span(position, *body.span()) {
                scope.push((param.name.to_string(), CompletionItemKind::VARIABLE));
                let out = values_in_scope_at_expr(body, position, scope)
                    .or_else(|| Some(scope_to_map(scope)));
                scope.pop();
                return out;
            }
            Some(scope_to_map(scope))
        }
        Expr::Match(_span, scrutinee, arms) => {
            if position_in_span(position, *scrutinee.span()) {
                return values_in_scope_at_expr(scrutinee, position, scope)
                    .or_else(|| Some(scope_to_map(scope)));
            }
            for (pattern, arm) in arms {
                if position_in_span(position, *pattern.span()) {
                    return Some(scope_to_map(scope));
                }
                if position_in_span(position, *arm.span()) {
                    let base_len = scope.len();
                    scope.extend(
                        pattern_vars(pattern)
                            .into_iter()
                            .map(|name| (name, CompletionItemKind::VARIABLE)),
                    );
                    let out = values_in_scope_at_expr(arm, position, scope)
                        .or_else(|| Some(scope_to_map(scope)));
                    scope.truncate(base_len);
                    return out;
                }
            }
            Some(scope_to_map(scope))
        }
        Expr::App(_span, fun, arg) => {
            if position_in_span(position, *fun.span()) {
                return values_in_scope_at_expr(fun, position, scope)
                    .or_else(|| Some(scope_to_map(scope)));
            }
            if position_in_span(position, *arg.span()) {
                return values_in_scope_at_expr(arg, position, scope)
                    .or_else(|| Some(scope_to_map(scope)));
            }
            Some(scope_to_map(scope))
        }
        Expr::Project(_span, base, _field) => {
            if position_in_span(position, *base.span()) {
                return values_in_scope_at_expr(base, position, scope)
                    .or_else(|| Some(scope_to_map(scope)));
            }
            Some(scope_to_map(scope))
        }
        Expr::Tuple(_span, elems) | Expr::List(_span, elems) => {
            for elem in elems {
                if position_in_span(position, *elem.span()) {
                    return values_in_scope_at_expr(elem, position, scope)
                        .or_else(|| Some(scope_to_map(scope)));
                }
            }
            Some(scope_to_map(scope))
        }
        Expr::Dict(_span, entries) => {
            for value in entries.values() {
                if position_in_span(position, *value.span()) {
                    return values_in_scope_at_expr(value, position, scope)
                        .or_else(|| Some(scope_to_map(scope)));
                }
            }
            Some(scope_to_map(scope))
        }
        Expr::Ite(_span, cond, then_expr, else_expr) => {
            if position_in_span(position, *cond.span()) {
                return values_in_scope_at_expr(cond, position, scope)
                    .or_else(|| Some(scope_to_map(scope)));
            }
            if position_in_span(position, *then_expr.span()) {
                return values_in_scope_at_expr(then_expr, position, scope)
                    .or_else(|| Some(scope_to_map(scope)));
            }
            if position_in_span(position, *else_expr.span()) {
                return values_in_scope_at_expr(else_expr, position, scope)
                    .or_else(|| Some(scope_to_map(scope)));
            }
            Some(scope_to_map(scope))
        }
        Expr::Ann(_span, inner, _ann) => {
            if position_in_span(position, *inner.span()) {
                return values_in_scope_at_expr(inner, position, scope)
                    .or_else(|| Some(scope_to_map(scope)));
            }
            Some(scope_to_map(scope))
        }
        _ => Some(scope_to_map(scope)),
    }
}

pub(crate) fn function_defs_from_tokens(tokens: &Tokens) -> HashMap<String, CompletionItemKind> {
    // Heuristic fallback when parsing fails: detect `let name = \...` and mark
    // `name` as a function completion.
    //
    // Also detects `fn name ...` declarations.
    //
    // This keeps completion useful while the user is mid-edit and the AST is
    // temporarily invalid.
    let mut out = HashMap::new();
    let items = &tokens.items;
    let mut index = 0usize;

    let next_non_ws = |i: usize| -> Option<usize> { (i < items.len()).then_some(i) };

    while index < items.len() {
        if matches!(items[index], Token::Fn(..)) {
            let Some(i) = next_non_ws(index + 1) else {
                break;
            };
            if let Token::Ident(name, ..) = &items[i] {
                out.insert(name.clone(), CompletionItemKind::FUNCTION);
            }
            index += 1;
            continue;
        }

        if !matches!(items[index], Token::Let(..)) {
            index += 1;
            continue;
        }

        let Some(mut i) = next_non_ws(index + 1) else {
            break;
        };

        let name = match &items[i] {
            Token::Ident(name, ..) => name.clone(),
            _ => {
                index += 1;
                continue;
            }
        };

        // Walk to `=` (skipping whitespace) and then check if the next token is `\`.
        i += 1;
        while let Some(j) = next_non_ws(i) {
            match &items[j] {
                Token::Assign(..) => {
                    let Some(k) = next_non_ws(j + 1) else {
                        break;
                    };
                    if matches!(items[k], Token::BackSlash(..)) {
                        out.insert(name, CompletionItemKind::FUNCTION);
                    }
                    break;
                }
                Token::SemiColon(..) => break,
                _ => i = j + 1,
            }
        }

        index += 1;
    }

    out
}

pub(crate) fn collect_type_names(compilation_unit: &CompilationUnit) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for decl in &compilation_unit.decls {
        if let Decl::Type(TypeDecl { name, .. }) = decl {
            names.insert(name.to_string());
        }
    }
    names
}

pub(crate) fn collect_constructors(compilation_unit: &CompilationUnit) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for decl in &compilation_unit.decls {
        if let Decl::Type(TypeDecl {
            kind: TypeDeclKind::Adt(variants),
            ..
        }) = decl
        {
            for variant in variants {
                names.insert(variant.name.to_string());
            }
        }
    }
    names
}

pub(crate) fn collect_fields_type_expr(typ: &TypeExpr, fields: &mut BTreeSet<String>) {
    match typ {
        TypeExpr::Record(_, entries) => {
            for entry in entries {
                fields.insert(entry.name.to_string());
            }
        }
        TypeExpr::App(_, fun, arg) => {
            collect_fields_type_expr(fun, fields);
            collect_fields_type_expr(arg, fields);
        }
        TypeExpr::Fun(_, arg, ret) => {
            collect_fields_type_expr(arg, fields);
            collect_fields_type_expr(ret, fields);
        }
        TypeExpr::Tuple(_, elems) => {
            for elem in elems {
                collect_fields_type_expr(elem, fields);
            }
        }
        TypeExpr::Name(..) => {}
    }
}

pub(crate) fn field_completion_for_position(
    compilation_unit: &CompilationUnit,
    position: Position,
    base_ident: Option<&str>,
) -> Option<BTreeSet<String>> {
    let type_fields = type_fields_map(compilation_unit);
    let env = field_env_at_position(compilation_unit, position, &type_fields);
    let pos = lsp_to_rex_position(position);

    if let Some(expr) = compilation_unit.body_with_fns()
        && let Some(base) = project_base_at_position(expr.as_ref(), pos)
        && let Some(fields) = fields_for_expr(base, &env, &type_fields)
    {
        return Some(fields);
    }

    if let Some(base_ident) = base_ident {
        if let Some(fields) = env.get(base_ident) {
            return Some(fields.clone());
        }
        if let Some(fields) = type_fields.get(base_ident) {
            return Some(fields.clone());
        }
    }

    None
}

pub(crate) fn type_fields_map(
    compilation_unit: &CompilationUnit,
) -> HashMap<String, BTreeSet<String>> {
    let mut map = HashMap::new();
    for decl in &compilation_unit.decls {
        if let Decl::Type(TypeDecl {
            name,
            kind: TypeDeclKind::Adt(variants),
            ..
        }) = decl
        {
            let mut fields = BTreeSet::new();
            for variant in variants {
                for arg in &variant.args {
                    collect_fields_type_expr(&arg.typ, &mut fields);
                }
            }
            if !fields.is_empty() {
                map.insert(name.to_string(), fields);
            }
        }
    }
    map
}

pub(crate) fn field_env_at_position(
    compilation_unit: &CompilationUnit,
    position: Position,
    type_fields: &HashMap<String, BTreeSet<String>>,
) -> HashMap<String, BTreeSet<String>> {
    let pos = lsp_to_rex_position(position);
    let Some(expr) = compilation_unit.body_with_fns() else {
        return HashMap::new();
    };
    field_env_at_expr(expr.as_ref(), pos, &HashMap::new(), type_fields).unwrap_or_default()
}

pub(crate) fn field_env_at_expr(
    expr: &Expr,
    position: RexPosition,
    env: &HashMap<String, BTreeSet<String>>,
    type_fields: &HashMap<String, BTreeSet<String>>,
) -> Option<HashMap<String, BTreeSet<String>>> {
    if !position_in_span(position, *expr.span()) {
        return None;
    }

    match expr {
        Expr::Let(_, var, _, ann, def, body) => {
            if position_in_span(position, *def.span()) {
                return field_env_at_expr(def, position, env, type_fields)
                    .or_else(|| Some(env.clone()));
            }
            if position_in_span(position, *body.span()) {
                let mut env_with = env.clone();
                let fields = binding_fields(ann.as_ref(), def, type_fields).unwrap_or_default();
                env_with.insert(var.name.to_string(), fields);
                if let Some(inner) = field_env_at_expr(body, position, &env_with, type_fields) {
                    return Some(inner);
                }
                return Some(env_with);
            }
            Some(env.clone())
        }
        Expr::LetRec(_, bindings, body) => {
            let mut env_with = env.clone();
            for (var, _, ann, def) in bindings {
                let fields = binding_fields(ann.as_ref(), def, type_fields).unwrap_or_default();
                env_with.insert(var.name.to_string(), fields);
            }
            for (_, _, _, def) in bindings {
                if position_in_span(position, *def.span()) {
                    return field_env_at_expr(def, position, &env_with, type_fields)
                        .or_else(|| Some(env_with.clone()));
                }
            }
            if position_in_span(position, *body.span()) {
                if let Some(inner) = field_env_at_expr(body, position, &env_with, type_fields) {
                    return Some(inner);
                }
                return Some(env_with);
            }
            Some(env.clone())
        }
        Expr::Lam(_, _scope, param, ann, _constraints, body) => {
            if position_in_span(position, *body.span()) {
                let mut env_with = env.clone();
                let fields = ann
                    .as_ref()
                    .and_then(|ann| fields_from_type_expr(ann, type_fields))
                    .unwrap_or_default();
                env_with.insert(param.name.to_string(), fields);
                if let Some(inner) = field_env_at_expr(body, position, &env_with, type_fields) {
                    return Some(inner);
                }
                return Some(env_with);
            }
            Some(env.clone())
        }
        Expr::Match(_, scrutinee, arms) => {
            if position_in_span(position, *scrutinee.span()) {
                return field_env_at_expr(scrutinee, position, env, type_fields)
                    .or_else(|| Some(env.clone()));
            }
            for (pattern, arm) in arms {
                if position_in_span(position, *pattern.span()) {
                    return Some(env.clone());
                }
                if position_in_span(position, *arm.span()) {
                    let mut env_with = env.clone();
                    env_with.extend(
                        pattern_vars(pattern)
                            .into_iter()
                            .map(|name| (name, BTreeSet::new())),
                    );
                    if let Some(inner) = field_env_at_expr(arm, position, &env_with, type_fields) {
                        return Some(inner);
                    }
                    return Some(env_with);
                }
            }
            Some(env.clone())
        }
        Expr::App(_, fun, arg) => {
            if position_in_span(position, *fun.span()) {
                return field_env_at_expr(fun, position, env, type_fields)
                    .or_else(|| Some(env.clone()));
            }
            if position_in_span(position, *arg.span()) {
                return field_env_at_expr(arg, position, env, type_fields)
                    .or_else(|| Some(env.clone()));
            }
            Some(env.clone())
        }
        Expr::Project(_, base, _field) => {
            if position_in_span(position, *base.span()) {
                return field_env_at_expr(base, position, env, type_fields)
                    .or_else(|| Some(env.clone()));
            }
            Some(env.clone())
        }
        Expr::Tuple(_, elems) | Expr::List(_, elems) => {
            for elem in elems {
                if position_in_span(position, *elem.span()) {
                    return field_env_at_expr(elem, position, env, type_fields)
                        .or_else(|| Some(env.clone()));
                }
            }
            Some(env.clone())
        }
        Expr::Dict(_, entries) => {
            for value in entries.values() {
                if position_in_span(position, *value.span()) {
                    return field_env_at_expr(value, position, env, type_fields)
                        .or_else(|| Some(env.clone()));
                }
            }
            Some(env.clone())
        }
        Expr::Ite(_, cond, then_expr, else_expr) => {
            if position_in_span(position, *cond.span()) {
                return field_env_at_expr(cond, position, env, type_fields)
                    .or_else(|| Some(env.clone()));
            }
            if position_in_span(position, *then_expr.span()) {
                return field_env_at_expr(then_expr, position, env, type_fields)
                    .or_else(|| Some(env.clone()));
            }
            if position_in_span(position, *else_expr.span()) {
                return field_env_at_expr(else_expr, position, env, type_fields)
                    .or_else(|| Some(env.clone()));
            }
            Some(env.clone())
        }
        Expr::Ann(_, inner, _ann) => {
            if position_in_span(position, *inner.span()) {
                return field_env_at_expr(inner, position, env, type_fields)
                    .or_else(|| Some(env.clone()));
            }
            Some(env.clone())
        }
        _ => Some(env.clone()),
    }
}

pub(crate) fn binding_fields(
    ann: Option<&TypeExpr>,
    def: &Expr,
    type_fields: &HashMap<String, BTreeSet<String>>,
) -> Option<BTreeSet<String>> {
    if let Some(ann) = ann
        && let Some(fields) = fields_from_type_expr(ann, type_fields)
    {
        return Some(fields);
    }

    if let Expr::Ann(_, _inner, ann) = def
        && let Some(fields) = fields_from_type_expr(ann, type_fields)
    {
        return Some(fields);
    }

    if let Expr::Dict(_, entries) = def {
        let fields: BTreeSet<String> = entries.keys().map(|name| name.to_string()).collect();
        if !fields.is_empty() {
            return Some(fields);
        }
    }

    None
}

pub(crate) fn fields_from_type_expr(
    typ: &TypeExpr,
    type_fields: &HashMap<String, BTreeSet<String>>,
) -> Option<BTreeSet<String>> {
    match typ {
        TypeExpr::Record(_, entries) => {
            let fields: BTreeSet<String> =
                entries.iter().map(|entry| entry.name.to_string()).collect();
            if fields.is_empty() {
                None
            } else {
                Some(fields)
            }
        }
        _ => {
            if let Some(type_name) = type_name_from_type_expr(typ) {
                return type_fields.get(&type_name).cloned();
            }
            None
        }
    }
}

pub(crate) fn type_name_from_type_expr(typ: &TypeExpr) -> Option<String> {
    match typ {
        TypeExpr::Name(_, name) => Some(name.to_string()),
        TypeExpr::App(_, fun, _) => type_name_from_type_expr(fun),
        _ => None,
    }
}

pub(crate) fn fields_for_expr(
    expr: &Expr,
    env: &HashMap<String, BTreeSet<String>>,
    type_fields: &HashMap<String, BTreeSet<String>>,
) -> Option<BTreeSet<String>> {
    match expr {
        Expr::Dict(_, entries) => {
            let fields: BTreeSet<String> = entries.keys().map(|name| name.to_string()).collect();
            if fields.is_empty() {
                None
            } else {
                Some(fields)
            }
        }
        Expr::Var(var) => {
            if let Some(fields) = env.get(var.name.as_ref()) {
                return Some(fields.clone());
            }
            if let Some(fields) = type_fields.get(var.name.as_ref()) {
                return Some(fields.clone());
            }
            None
        }
        Expr::Ann(_, inner, ann) => fields_from_type_expr(ann, type_fields)
            .or_else(|| fields_for_expr(inner, env, type_fields)),
        Expr::Project(_, base, _) => fields_for_expr(base, env, type_fields),
        _ => None,
    }
}

pub(crate) fn project_base_at_position(expr: &Expr, position: RexPosition) -> Option<&Expr> {
    if !position_in_span(position, *expr.span()) {
        return None;
    }

    match expr {
        Expr::Project(_, base, _) => {
            if position_in_span(position, *base.span()) {
                return project_base_at_position(base, position);
            }
            Some(base.as_ref())
        }
        Expr::Let(_, _var, _, _ann, def, body) => {
            if let Some(found) = project_base_at_position(def, position) {
                return Some(found);
            }
            project_base_at_position(body, position)
        }
        Expr::LetRec(_, bindings, body) => {
            for (_, _, _, def) in bindings {
                if let Some(found) = project_base_at_position(def, position) {
                    return Some(found);
                }
            }
            project_base_at_position(body, position)
        }
        Expr::Lam(_, _scope, _param, _ann, _constraints, body) => {
            project_base_at_position(body, position)
        }
        Expr::Match(_, scrutinee, arms) => {
            if let Some(found) = project_base_at_position(scrutinee, position) {
                return Some(found);
            }
            for (_pattern, arm) in arms {
                if let Some(found) = project_base_at_position(arm, position) {
                    return Some(found);
                }
            }
            None
        }
        Expr::App(_, fun, arg) => {
            if let Some(found) = project_base_at_position(fun, position) {
                return Some(found);
            }
            project_base_at_position(arg, position)
        }
        Expr::Tuple(_, elems) | Expr::List(_, elems) => {
            for elem in elems {
                if let Some(found) = project_base_at_position(elem, position) {
                    return Some(found);
                }
            }
            None
        }
        Expr::Dict(_, entries) => {
            for value in entries.values() {
                if let Some(found) = project_base_at_position(value, position) {
                    return Some(found);
                }
            }
            None
        }
        Expr::Ite(_, cond, then_expr, else_expr) => {
            if let Some(found) = project_base_at_position(cond, position) {
                return Some(found);
            }
            if let Some(found) = project_base_at_position(then_expr, position) {
                return Some(found);
            }
            project_base_at_position(else_expr, position)
        }
        Expr::Ann(_, inner, _ann) => project_base_at_position(inner, position),
        _ => None,
    }
}

pub(crate) fn fallback_field_map(tokens: &Tokens) -> HashMap<String, BTreeSet<String>> {
    let mut map = HashMap::new();
    let items = &tokens.items;
    let mut index = 0usize;
    while index + 2 < items.len() {
        if let Token::Ident(name, ..) = &items[index]
            && matches!(items[index + 1], Token::Assign(..) | Token::Colon(..))
            && matches!(items[index + 2], Token::BraceL(..))
            && let Some((fields, end_index)) = parse_record_fields(items, index + 2)
        {
            if !fields.is_empty() {
                map.insert(name.clone(), fields);
            }
            index = end_index + 1;
            continue;
        }
        index += 1;
    }
    map
}

pub(crate) fn parse_record_fields(
    tokens: &[Token],
    start_index: usize,
) -> Option<(BTreeSet<String>, usize)> {
    if !matches!(tokens.get(start_index), Some(Token::BraceL(..))) {
        return None;
    }

    let mut depth = 0usize;
    let mut fields = BTreeSet::new();
    let mut index = start_index;
    while index < tokens.len() {
        match &tokens[index] {
            Token::BraceL(..) => depth += 1,
            Token::BraceR(..) => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some((fields, index));
                }
            }
            Token::Ident(name, ..) if depth == 1 => {
                if let Some(next) = tokens.get(index + 1)
                    && matches!(next, Token::Assign(..) | Token::Colon(..))
                {
                    fields.insert(name.clone());
                }
            }
            _ => {}
        }
        index += 1;
    }

    None
}

pub(crate) fn field_base_ident(text: &str, position: Position) -> Option<String> {
    let offset = offset_at(text, position)?;
    if offset == 0 {
        return None;
    }

    let bytes = text.as_bytes();
    let mut index = offset.min(bytes.len());

    while index > 0 && bytes[index - 1].is_ascii_whitespace() {
        index -= 1;
    }
    while index > 0 && is_word_byte(bytes[index - 1]) {
        index -= 1;
    }
    while index > 0 && bytes[index - 1].is_ascii_whitespace() {
        index -= 1;
    }

    if index == 0 || bytes[index - 1] != b'.' {
        return None;
    }

    index -= 1;
    while index > 0 && bytes[index - 1].is_ascii_whitespace() {
        index -= 1;
    }

    let end = index;
    while index > 0 && is_word_byte(bytes[index - 1]) {
        index -= 1;
    }

    if index == end {
        return None;
    }

    Some(text[index..end].to_string())
}

pub(crate) fn is_word_byte(byte: u8) -> bool {
    let ch = byte as char;
    ch.is_ascii_alphanumeric() || ch == '_'
}

pub(crate) fn ident_token_at_position(
    tokens: &Tokens,
    position: Position,
) -> Option<(String, Span)> {
    for token in &tokens.items {
        let Token::Ident(name, span, ..) = token else {
            continue;
        };
        if range_touches_position(span_to_range(*span), position) {
            return Some((name.clone(), *span));
        }
    }
    None
}

pub(crate) fn imported_projection_at_position(
    tokens: &Tokens,
    position: Position,
) -> Option<(String, String)> {
    fn is_trivia(token: &Token) -> bool {
        matches!(token, Token::CommentL(..) | Token::CommentR(..))
    }

    fn prev_non_trivia(tokens: &Tokens, start: usize) -> Option<usize> {
        let mut idx = start;
        while idx > 0 {
            idx -= 1;
            if !is_trivia(&tokens.items[idx]) {
                return Some(idx);
            }
        }
        None
    }

    let mut ident_index = None;
    let mut field = None;
    for (idx, token) in tokens.items.iter().enumerate() {
        let Token::Ident(name, span, ..) = token else {
            continue;
        };
        if range_touches_position(span_to_range(*span), position) {
            ident_index = Some(idx);
            field = Some(name.clone());
            break;
        }
    }
    let ident_index = ident_index?;
    let field = field?;

    let dot_idx = prev_non_trivia(tokens, ident_index)?;
    if !matches!(tokens.items[dot_idx], Token::Dot(..)) {
        return None;
    }
    let base_idx = prev_non_trivia(tokens, dot_idx)?;
    let Token::Ident(base, ..) = &tokens.items[base_idx] else {
        return None;
    };

    Some((base.clone(), field))
}

pub(crate) struct DeclSpanIndex {
    pub(crate) type_defs: HashMap<String, Span>,
    pub(crate) ctor_defs: HashMap<String, Span>,
    pub(crate) class_defs: HashMap<String, Span>,
    pub(crate) fn_defs: HashMap<String, Span>,
    pub(crate) class_method_defs: HashMap<String, Span>,
    pub(crate) instance_method_defs: Vec<(Span, HashMap<String, Span>)>,
}

pub(crate) fn index_decl_spans(
    compilation_unit: &CompilationUnit,
    tokens: &Tokens,
) -> DeclSpanIndex {
    fn span_contains_span(outer: Span, inner: Span) -> bool {
        position_leq(outer.begin, inner.begin) && position_leq(inner.end, outer.end)
    }

    let mut type_defs = HashMap::new();
    let mut ctor_defs = HashMap::new();
    let mut class_defs = HashMap::new();
    let mut fn_defs = HashMap::new();
    let mut class_method_defs = HashMap::new();
    let mut instance_method_defs = Vec::new();

    for decl in &compilation_unit.decls {
        match decl {
            Decl::Type(td) => {
                let decl_span = td.span;
                let mut expect_type_name = false;
                let mut expect_ctor_name = false;

                for token in &tokens.items {
                    let token_span = token.span();
                    if !span_contains_span(decl_span, token_span) {
                        continue;
                    }

                    match token {
                        Token::Type(..) => {
                            expect_type_name = true;
                            expect_ctor_name = false;
                        }
                        Token::Ident(name, span, ..) if expect_type_name => {
                            type_defs.insert(name.clone(), *span);
                            expect_type_name = false;
                        }
                        Token::Assign(..) | Token::Pipe(..) => {
                            expect_ctor_name = true;
                        }
                        Token::Ident(name, span, ..) if expect_ctor_name => {
                            ctor_defs.insert(name.clone(), *span);
                            expect_ctor_name = false;
                        }
                        _ => {}
                    }
                }
            }
            Decl::Class(cd) => {
                let decl_span = cd.span;
                let mut expect_class_name = false;
                for i in 0..tokens.items.len() {
                    let token = &tokens.items[i];
                    let token_span = token.span();
                    if !span_contains_span(decl_span, token_span) {
                        continue;
                    }
                    match token {
                        Token::Class(..) => expect_class_name = true,
                        Token::Ident(name, span, ..) if expect_class_name => {
                            class_defs.insert(name.clone(), *span);
                            expect_class_name = false;
                        }
                        Token::Ident(name, span, ..) => {
                            if let Some(next) = tokens.items.get(i + 1)
                                && matches!(next, Token::Colon(..))
                            {
                                class_method_defs.insert(name.clone(), *span);
                            }
                        }
                        _ => {}
                    }
                }
            }
            Decl::Instance(id) => {
                let decl_span = id.span;
                let mut methods = HashMap::new();
                for i in 0..tokens.items.len() {
                    let token = &tokens.items[i];
                    let token_span = token.span();
                    if !span_contains_span(decl_span, token_span) {
                        continue;
                    }
                    if let Token::Ident(name, span, ..) = token
                        && let Some(next) = tokens.items.get(i + 1)
                        && matches!(next, Token::Assign(..))
                    {
                        methods.insert(name.clone(), *span);
                    }
                }
                instance_method_defs.push((decl_span, methods));
            }
            Decl::Fn(fd) => {
                fn_defs.insert(fd.name.name.as_ref().to_string(), fd.name.span);
            }
            Decl::DeclareFn(fd) => {
                fn_defs.insert(fd.name.name.as_ref().to_string(), fd.name.span);
            }
            Decl::Import(..) => {}
        }
    }

    DeclSpanIndex {
        type_defs,
        ctor_defs,
        class_defs,
        fn_defs,
        class_method_defs,
        instance_method_defs,
    }
}

pub(crate) fn definition_span_for_value_ident(
    expr: &Expr,
    position: RexPosition,
    ident: &str,
    bindings: &mut Vec<(String, Span)>,
    tokens: &Tokens,
) -> Option<Span> {
    if !position_in_span(position, *expr.span()) {
        return None;
    }

    fn lookup_binding(bindings: &[(String, Span)], ident: &str) -> Option<Span> {
        bindings
            .iter()
            .rev()
            .find_map(|(name, span)| (name == ident).then_some(*span))
    }

    fn definition_in_pattern(
        pat: &Pattern,
        position: RexPosition,
        ident: &str,
        _tokens: &Tokens,
    ) -> Option<Span> {
        if !position_in_span(position, *pat.span()) {
            return None;
        }

        match pat {
            Pattern::Var(var) => (var.name.as_ref() == ident).then_some(var.span),
            Pattern::Named(_span, _name, args) => args
                .iter()
                .find_map(|arg| definition_in_pattern(arg, position, ident, _tokens)),
            Pattern::Tuple(_span, elems) => elems
                .iter()
                .find_map(|elem| definition_in_pattern(elem, position, ident, _tokens)),
            Pattern::List(_span, elems) => elems
                .iter()
                .find_map(|elem| definition_in_pattern(elem, position, ident, _tokens)),
            Pattern::Cons(_span, head, tail) => {
                definition_in_pattern(head, position, ident, _tokens)
                    .or_else(|| definition_in_pattern(tail, position, ident, _tokens))
            }
            Pattern::Dict(_span, fields) => fields
                .iter()
                .find_map(|(_, p)| definition_in_pattern(p, position, ident, _tokens)),
            Pattern::Wildcard(..) => None,
        }
    }

    fn push_pattern_bindings(pat: &Pattern, bindings: &mut Vec<(String, Span)>, _tokens: &Tokens) {
        match pat {
            Pattern::Var(var) => bindings.push((var.name.to_string(), var.span)),
            Pattern::Named(_span, _name, args) => {
                for arg in args {
                    push_pattern_bindings(arg, bindings, _tokens);
                }
            }
            Pattern::Tuple(_span, elems) => {
                for elem in elems {
                    push_pattern_bindings(elem, bindings, _tokens);
                }
            }
            Pattern::List(_span, elems) => {
                for elem in elems {
                    push_pattern_bindings(elem, bindings, _tokens);
                }
            }
            Pattern::Cons(_span, head, tail) => {
                push_pattern_bindings(head, bindings, _tokens);
                push_pattern_bindings(tail, bindings, _tokens);
            }
            Pattern::Dict(_span, fields) => {
                for (_key, pat) in fields {
                    push_pattern_bindings(pat, bindings, _tokens);
                }
            }
            Pattern::Wildcard(..) => {}
        }
    }

    match expr {
        Expr::Var(var) => {
            if position_in_span(position, var.span) && var.name.as_ref() == ident {
                return lookup_binding(bindings, ident);
            }
            None
        }
        Expr::Let(_span, var, _, _ann, def, body) => {
            if position_in_span(position, var.span) && var.name.as_ref() == ident {
                return Some(var.span);
            }

            if position_in_span(position, *def.span()) {
                return definition_span_for_value_ident(def, position, ident, bindings, tokens);
            }
            if position_in_span(position, *body.span()) {
                bindings.push((var.name.to_string(), var.span));
                let out = definition_span_for_value_ident(body, position, ident, bindings, tokens);
                bindings.pop();
                return out;
            }
            None
        }
        Expr::LetRec(_span, rec_bindings, body) => {
            for (var, _, _ann, _def) in rec_bindings {
                if position_in_span(position, var.span) && var.name.as_ref() == ident {
                    return Some(var.span);
                }
            }

            let base_len = bindings.len();
            for (var, _, _ann, _def) in rec_bindings {
                bindings.push((var.name.to_string(), var.span));
            }

            for (_var, _, _ann, def) in rec_bindings {
                if position_in_span(position, *def.span()) {
                    let out =
                        definition_span_for_value_ident(def, position, ident, bindings, tokens);
                    bindings.truncate(base_len);
                    return out;
                }
            }
            if position_in_span(position, *body.span()) {
                let out = definition_span_for_value_ident(body, position, ident, bindings, tokens);
                bindings.truncate(base_len);
                return out;
            }

            bindings.truncate(base_len);
            None
        }
        Expr::Lam(_span, _scope, param, _ann, _constraints, body) => {
            if position_in_span(position, param.span) && param.name.as_ref() == ident {
                return Some(param.span);
            }

            if position_in_span(position, *body.span()) {
                bindings.push((param.name.to_string(), param.span));
                let out = definition_span_for_value_ident(body, position, ident, bindings, tokens);
                bindings.pop();
                return out;
            }
            None
        }
        Expr::Match(_span, scrutinee, arms) => {
            if position_in_span(position, *scrutinee.span()) {
                return definition_span_for_value_ident(
                    scrutinee, position, ident, bindings, tokens,
                );
            }

            for (pat, arm) in arms {
                if position_in_span(position, *pat.span()) {
                    return definition_in_pattern(pat, position, ident, tokens);
                }

                if position_in_span(position, *arm.span()) {
                    let base_len = bindings.len();
                    push_pattern_bindings(pat, bindings, tokens);
                    let out =
                        definition_span_for_value_ident(arm, position, ident, bindings, tokens);
                    bindings.truncate(base_len);
                    return out;
                }
            }
            None
        }
        Expr::App(_span, fun, arg) => {
            if position_in_span(position, *fun.span()) {
                return definition_span_for_value_ident(fun, position, ident, bindings, tokens);
            }
            if position_in_span(position, *arg.span()) {
                return definition_span_for_value_ident(arg, position, ident, bindings, tokens);
            }
            None
        }
        Expr::Project(_span, base, _field) => {
            if position_in_span(position, *base.span()) {
                return definition_span_for_value_ident(base, position, ident, bindings, tokens);
            }
            None
        }
        Expr::Tuple(_span, elems) | Expr::List(_span, elems) => elems.iter().find_map(|elem| {
            position_in_span(position, *elem.span())
                .then(|| definition_span_for_value_ident(elem, position, ident, bindings, tokens))
                .flatten()
        }),
        Expr::Dict(_span, entries) => entries.values().find_map(|value| {
            position_in_span(position, *value.span())
                .then(|| definition_span_for_value_ident(value, position, ident, bindings, tokens))
                .flatten()
        }),
        Expr::Ite(_span, cond, then_expr, else_expr) => {
            if position_in_span(position, *cond.span()) {
                return definition_span_for_value_ident(cond, position, ident, bindings, tokens);
            }
            if position_in_span(position, *then_expr.span()) {
                return definition_span_for_value_ident(
                    then_expr, position, ident, bindings, tokens,
                );
            }
            if position_in_span(position, *else_expr.span()) {
                return definition_span_for_value_ident(
                    else_expr, position, ident, bindings, tokens,
                );
            }
            None
        }
        Expr::Ann(_span, inner, _ann) => {
            if position_in_span(position, *inner.span()) {
                return definition_span_for_value_ident(inner, position, ident, bindings, tokens);
            }
            None
        }
        _ => None,
    }
}

// Note: completion uses `values_in_scope_at_position` instead of a plain list of
// names so it can classify `fn`/`let name = \...` as `CompletionItemKind::FUNCTION`.

pub(crate) fn pattern_vars(pattern: &Pattern) -> Vec<String> {
    let mut vars = Vec::new();
    collect_pattern_vars(pattern, &mut vars);
    vars
}

pub(crate) fn collect_pattern_vars(pattern: &Pattern, vars: &mut Vec<String>) {
    match pattern {
        Pattern::Var(var) => vars.push(var.name.to_string()),
        Pattern::Named(_, _name, args) => {
            for arg in args {
                collect_pattern_vars(arg, vars);
            }
        }
        Pattern::Tuple(_, elems) => {
            for elem in elems {
                collect_pattern_vars(elem, vars);
            }
        }
        Pattern::List(_, elems) => {
            for elem in elems {
                collect_pattern_vars(elem, vars);
            }
        }
        Pattern::Cons(_, head, tail) => {
            collect_pattern_vars(head, vars);
            collect_pattern_vars(tail, vars);
        }
        Pattern::Dict(_, fields) => {
            for (_key, pat) in fields {
                collect_pattern_vars(pat, vars);
            }
        }
        Pattern::Wildcard(_) => {}
    }
}

pub(crate) fn is_field_completion(text: &str, position: Position) -> bool {
    let offset = match offset_at(text, position) {
        Some(offset) => offset,
        None => return false,
    };

    if offset == 0 {
        return false;
    }

    let mut start = offset;
    while start > 0 {
        let prev = text.as_bytes()[start - 1] as char;
        if is_word_char(prev) {
            start -= 1;
            continue;
        }
        break;
    }

    if start > 0 && text.as_bytes()[start - 1] as char == '.' {
        return true;
    }

    text.as_bytes()[offset.saturating_sub(1)] as char == '.'
}

pub(crate) fn lsp_to_rex_position(position: Position) -> RexPosition {
    RexPosition::new(position.line as usize + 1, position.character as usize + 1)
}

pub(crate) fn position_in_span(position: RexPosition, span: Span) -> bool {
    position_leq(span.begin, position) && position_leq(position, span.end)
}

pub(crate) fn position_leq(left: RexPosition, right: RexPosition) -> bool {
    left.line < right.line || (left.line == right.line && left.column <= right.column)
}

pub(crate) fn word_at_position(text: &str, position: Position) -> Option<String> {
    let offset = offset_at(text, position)?;
    if offset >= text.len() {
        return None;
    }

    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut idx = None;
    for (i, (byte_index, _)) in chars.iter().enumerate() {
        if *byte_index == offset {
            idx = Some(i);
            break;
        }
    }

    let idx = idx?;
    if !is_word_char(chars[idx].1) {
        return None;
    }

    let mut start = idx;
    while start > 0 && is_word_char(chars[start - 1].1) {
        start -= 1;
    }

    let mut end = idx + 1;
    while end < chars.len() && is_word_char(chars[end].1) {
        end += 1;
    }

    let start_byte = chars[start].0;
    let end_byte = if end < chars.len() {
        chars[end].0
    } else {
        text.len()
    };

    Some(text[start_byte..end_byte].to_string())
}

pub(crate) fn offset_at(text: &str, position: Position) -> Option<usize> {
    let mut offset = 0usize;
    let mut current_line = 0u32;

    for mut line in text.split('\n') {
        if line.ends_with('\r') {
            line = &line[..line.len().saturating_sub(1)];
        }

        if current_line == position.line {
            let mut remaining = position.character as usize;
            for (byte_index, _) in line.char_indices() {
                if remaining == 0 {
                    return Some(offset + byte_index);
                }
                remaining -= 1;
            }
            return Some(offset + line.len());
        }

        offset += line.len() + 1;
        current_line += 1;
    }

    if current_line == position.line {
        Some(offset)
    } else {
        None
    }
}

pub(crate) fn position_at_offset(text: &str, target: usize) -> Position {
    let mut line = 0u32;
    let mut character = 0u32;

    for (offset, ch) in text.char_indices() {
        if offset >= target {
            break;
        }
        if ch == '\n' {
            line += 1;
            character = 0;
        } else {
            character += 1;
        }
    }

    Position { line, character }
}

pub(crate) fn wildcard_match_arm_insert(text: &str, range: Range) -> Option<(Position, String)> {
    let end = offset_at(text, range.end)?;
    let search = &text[..end.min(text.len())];
    let close_offset = search.rfind('}')?;
    let line_start = text[..close_offset]
        .rfind('\n')
        .map(|idx| idx + 1)
        .unwrap_or(0);
    let closing_prefix = &text[line_start..close_offset];

    if closing_prefix.chars().all(char::is_whitespace) {
        let arm_indent = format!("{closing_prefix}  ");
        Some((
            position_at_offset(text, line_start),
            format!("{arm_indent}case _ -> null;\n"),
        ))
    } else {
        Some((
            position_at_offset(text, close_offset),
            " case _ -> null;".to_string(),
        ))
    }
}

pub(crate) fn is_word_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

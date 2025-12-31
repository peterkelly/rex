#![forbid(unsafe_code)]

use std::collections::{BTreeSet, HashMap};

use rex_ast::expr::{Decl, Expr, Pattern, Program, TypeDecl, TypeExpr};
use rex_lexer::{
    span::{Position as RexPosition, Span, Spanned},
    LexicalError, Token, Tokens,
};
use rex_parser::Parser;
use tokio::sync::RwLock;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, CompletionOptions, CompletionParams, CompletionResponse,
    Diagnostic, DiagnosticSeverity, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, GotoDefinitionParams, GotoDefinitionResponse, Hover, HoverContents,
    HoverParams, HoverProviderCapability, InitializeParams, InitializeResult, InitializedParams,
    Location, MarkupContent, MarkupKind, MessageType, OneOf, Position, Range, ServerCapabilities,
    ServerInfo, TextDocumentSyncCapability, TextDocumentSyncKind, Url,
};
use tower_lsp::{Client, LanguageServer, LspService, Server};

const MAX_DIAGNOSTICS: usize = 50;
const BUILTIN_TYPES: &[&str] = &[
    "u8", "u16", "u32", "u64", "i8", "i16", "i32", "i64", "f32", "f64", "bool", "string", "uuid",
    "datetime", "Dict", "List", "Array", "Option", "Result",
];
const BUILTIN_VALUES: &[&str] = &["true", "false", "null", "Some", "None", "Ok", "Err"];

struct RexServer {
    client: Client,
    documents: RwLock<HashMap<Url, String>>,
}

impl RexServer {
    fn new(client: Client) -> Self {
        Self {
            client,
            documents: RwLock::new(HashMap::new()),
        }
    }

    async fn publish_diagnostics(&self, uri: Url, text: &str) {
        let diagnostics = diagnostics_from_text(text);
        self.client
            .publish_diagnostics(uri, diagnostics, None)
            .await;
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for RexServer {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec![".".to_string()]),
                    ..CompletionOptions::default()
                }),
                definition_provider: Some(OneOf::Left(true)),
                ..ServerCapabilities::default()
            },
            server_info: Some(ServerInfo {
                name: "rex-lsp".to_string(),
                version: Some("0.1.0".to_string()),
            }),
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "Rex LSP initialized")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let text = params.text_document.text;

        self.documents
            .write()
            .await
            .insert(uri.clone(), text.clone());
        self.publish_diagnostics(uri, &text).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        let text = params
            .content_changes
            .into_iter()
            .last()
            .map(|change| change.text);

        if let Some(text) = text {
            self.documents
                .write()
                .await
                .insert(uri.clone(), text.clone());
            self.publish_diagnostics(uri, &text).await;
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.documents.write().await.remove(&uri);
        self.client.publish_diagnostics(uri, Vec::new(), None).await;
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let text = { self.documents.read().await.get(&uri).cloned() };

        let Some(text) = text else {
            return Ok(None);
        };

        let Some(word) = word_at_position(&text, position) else {
            return Ok(None);
        };

        if let Some(contents) = hover_contents(&word) {
            return Ok(Some(Hover {
                contents,
                range: None,
            }));
        }

        Ok(None)
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let text = { self.documents.read().await.get(&uri).cloned() };

        let Some(text) = text else {
            return Ok(None);
        };

        let items = completion_items(&text, position);
        Ok(Some(CompletionResponse::Array(items)))
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let text = { self.documents.read().await.get(&uri).cloned() };

        let Some(text) = text else {
            return Ok(None);
        };

        let Ok(tokens) = Token::tokenize(&text) else {
            return Ok(None);
        };

        let Some((ident, _token_span)) = ident_token_at_position(&tokens, position) else {
            return Ok(None);
        };

        // Parse on-demand. This keeps steady-state typing latency low; “go to
        // definition” is an explicit user action where a little work is fine.
        let Ok(tokens_for_parse) = Token::tokenize(&text) else {
            return Ok(None);
        };
        let mut parser = Parser::new(tokens_for_parse);
        let Ok(program) = parser.parse_program() else {
            return Ok(None);
        };

        let (type_defs, ctor_defs) = index_type_and_constructor_spans(&program, &tokens);
        let pos = lsp_to_rex_position(position);

        let expr = program.expr_with_fns();
        let value_def =
            definition_span_for_value_ident(&expr, pos, &ident, &mut Vec::new(), &tokens);
        let target_span = value_def
            .or_else(|| ctor_defs.get(&ident).copied())
            .or_else(|| type_defs.get(&ident).copied());

        let Some(target_span) = target_span else {
            return Ok(None);
        };

        let location = Location {
            uri,
            range: span_to_range(target_span),
        };
        Ok(Some(GotoDefinitionResponse::Scalar(location)))
    }
}

fn diagnostics_from_text(text: &str) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    match Token::tokenize(text) {
        Ok(tokens) => {
            push_comment_diagnostics(&tokens, &mut diagnostics);

            if diagnostics.len() < MAX_DIAGNOSTICS {
                let mut parser = Parser::new(tokens);
                if let Err(errors) = parser.parse_program() {
                    for err in errors {
                        diagnostics.push(diagnostic_for_span(err.span, err.message));
                        if diagnostics.len() >= MAX_DIAGNOSTICS {
                            break;
                        }
                    }
                }
            }
        }
        Err(err) => {
            let LexicalError::UnexpectedToken(span) = err;
            diagnostics.push(diagnostic_for_span(span, "Unexpected token"));
        }
    }

    diagnostics
}

fn push_comment_diagnostics(tokens: &Tokens, diagnostics: &mut Vec<Diagnostic>) {
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
                        "Unclosed block comment opener ({-).",
                    ));
                    break;
                }

                index = cursor + 1;
            }
            Token::CommentR(span) => {
                diagnostics.push(diagnostic_for_span(
                    span,
                    "Unmatched block comment closer (-}).",
                ));
                index += 1;
            }
            _ => index += 1,
        }
    }
}

fn diagnostic_for_span(span: Span, message: impl Into<String>) -> Diagnostic {
    Diagnostic {
        range: span_to_range(span),
        severity: Some(DiagnosticSeverity::ERROR),
        message: message.into(),
        source: Some("rex-lsp".to_string()),
        ..Diagnostic::default()
    }
}

fn range_contains_position(range: Range, position: Position) -> bool {
    let after_start = position.line > range.start.line
        || (position.line == range.start.line && position.character >= range.start.character);
    let before_end = position.line < range.end.line
        || (position.line == range.end.line && position.character < range.end.character);
    after_start && before_end
}

fn range_touches_position(range: Range, position: Position) -> bool {
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

fn span_to_range(span: Span) -> Range {
    Range {
        start: position_from_span(span.begin.line, span.begin.column),
        end: position_from_span(span.end.line, span.end.column),
    }
}

fn position_from_span(line: usize, column: usize) -> Position {
    Position {
        line: line.saturating_sub(1) as u32,
        character: column.saturating_sub(1) as u32,
    }
}

fn hover_contents(word: &str) -> Option<HoverContents> {
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

fn markdown_hover(word: &str, kind: &str, doc: &str) -> HoverContents {
    HoverContents::Markup(MarkupContent {
        kind: MarkupKind::Markdown,
        value: format!("**{}** {}\n\n{}", word, kind, doc),
    })
}

fn keyword_doc(word: &str) -> Option<&'static str> {
    match word {
        "let" => Some("Introduces local bindings."),
        "in" => Some("Begins the expression body for a let binding."),
        "type" => Some("Declares a type or ADT."),
        "match" => Some("Starts a pattern match expression."),
        "when" => Some("Introduces a match arm."),
        "if" => Some("Conditional expression keyword."),
        "then" => Some("Conditional expression branch."),
        "else" => Some("Fallback branch of a conditional expression."),
        "as" => Some("Type ascription or aliasing keyword."),
        "for" => Some("List/dict comprehension keyword (when supported)."),
        "is" => Some("Type assertion keyword."),
        _ => None,
    }
}

fn type_doc(word: &str) -> Option<&'static str> {
    match word {
        "bool" => Some("Boolean type."),
        "string" => Some("UTF-8 string type."),
        "uuid" => Some("UUID type."),
        "datetime" => Some("Datetime type."),
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
        "Array" => Some("Array type constructor."),
        "Option" => Some("Optional type constructor."),
        "Result" => Some("Result type constructor."),
        _ => None,
    }
}

fn value_doc(word: &str) -> Option<&'static str> {
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

fn completion_items(text: &str, position: Position) -> Vec<CompletionItem> {
    let field_mode = is_field_completion(text, position);
    let base_ident = if field_mode {
        field_base_ident(text, position)
    } else {
        None
    };
    if let Ok(tokens) = Token::tokenize(text) {
        let mut parser = Parser::new(tokens);
        if let Ok(program) = parser.parse_program() {
            return completion_items_from_program(
                &program,
                position,
                field_mode,
                base_ident.as_deref(),
            );
        }
    }

    completion_items_fallback(text, base_ident.as_deref(), field_mode)
}

fn completion_items_from_program(
    program: &Program,
    position: Position,
    field_mode: bool,
    base_ident: Option<&str>,
) -> Vec<CompletionItem> {
    if field_mode {
        if let Some(fields) = field_completion_for_position(program, position, base_ident) {
            return fields
                .into_iter()
                .map(|label| completion_item(label, CompletionItemKind::FIELD))
                .collect();
        }
        return Vec::new();
    }

    let mut value_kinds = values_in_scope_at_position(program, position);
    for value in BUILTIN_VALUES {
        value_kinds
            .entry((*value).to_string())
            .or_insert(CompletionItemKind::VARIABLE);
    }
    for ctor in collect_constructors(program) {
        value_kinds
            .entry(ctor)
            .or_insert(CompletionItemKind::CONSTRUCTOR);
    }

    let mut type_names = collect_type_names(program);
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

fn completion_items_fallback(
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
            if let Some(base_ident) = base_ident {
                if let Some(fields) = fallback_field_map(&tokens).get(base_ident) {
                    return fields
                        .iter()
                        .cloned()
                        .map(|label| completion_item(label, CompletionItemKind::FIELD))
                        .collect();
                }
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

fn completion_item(label: String, kind: CompletionItemKind) -> CompletionItem {
    CompletionItem {
        label,
        kind: Some(kind),
        ..CompletionItem::default()
    }
}

fn values_in_scope_at_position(
    program: &Program,
    position: Position,
) -> HashMap<String, CompletionItemKind> {
    let pos = lsp_to_rex_position(position);
    let expr = program.expr_with_fns();
    values_in_scope_at_expr(&expr, pos, &mut Vec::new()).unwrap_or_default()
}

fn values_in_scope_at_expr(
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
        Expr::Let(_span, var, _ann, def, body) => {
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

fn function_defs_from_tokens(tokens: &Tokens) -> HashMap<String, CompletionItemKind> {
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

    let next_non_ws = |mut i: usize| -> Option<usize> {
        while i < items.len() && items[i].is_whitespace() {
            i += 1;
        }
        (i < items.len()).then_some(i)
    };

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

        // Walk to `=` (skipping whitespace) and then check if the next token is `\` / `λ`.
        i += 1;
        loop {
            let Some(j) = next_non_ws(i) else {
                break;
            };
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

fn collect_type_names(program: &Program) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for decl in &program.decls {
        if let Decl::Type(TypeDecl { name, .. }) = decl {
            names.insert(name.to_string());
        }
    }
    names
}

fn collect_constructors(program: &Program) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for decl in &program.decls {
        if let Decl::Type(TypeDecl { variants, .. }) = decl {
            for variant in variants {
                names.insert(variant.name.to_string());
            }
        }
    }
    names
}

fn collect_fields_type_expr(typ: &TypeExpr, fields: &mut BTreeSet<String>) {
    match typ {
        TypeExpr::Record(_, entries) => {
            for (name, _ty) in entries {
                fields.insert(name.to_string());
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

fn field_completion_for_position(
    program: &Program,
    position: Position,
    base_ident: Option<&str>,
) -> Option<BTreeSet<String>> {
    let type_fields = type_fields_map(program);
    let env = field_env_at_position(program, position, &type_fields);
    let pos = lsp_to_rex_position(position);

    let expr = program.expr_with_fns();
    if let Some(base) = project_base_at_position(&expr, pos) {
        if let Some(fields) = fields_for_expr(base, &env, &type_fields) {
            return Some(fields);
        }
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

fn type_fields_map(program: &Program) -> HashMap<String, BTreeSet<String>> {
    let mut map = HashMap::new();
    for decl in &program.decls {
        if let Decl::Type(TypeDecl { name, variants, .. }) = decl {
            let mut fields = BTreeSet::new();
            for variant in variants {
                for arg in &variant.args {
                    collect_fields_type_expr(arg, &mut fields);
                }
            }
            if !fields.is_empty() {
                map.insert(name.to_string(), fields);
            }
        }
    }
    map
}

fn field_env_at_position(
    program: &Program,
    position: Position,
    type_fields: &HashMap<String, BTreeSet<String>>,
) -> HashMap<String, BTreeSet<String>> {
    let pos = lsp_to_rex_position(position);
    let expr = program.expr_with_fns();
    field_env_at_expr(&expr, pos, &HashMap::new(), type_fields).unwrap_or_default()
}

fn field_env_at_expr(
    expr: &Expr,
    position: RexPosition,
    env: &HashMap<String, BTreeSet<String>>,
    type_fields: &HashMap<String, BTreeSet<String>>,
) -> Option<HashMap<String, BTreeSet<String>>> {
    if !position_in_span(position, *expr.span()) {
        return None;
    }

    match expr {
        Expr::Let(_, var, ann, def, body) => {
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

fn binding_fields(
    ann: Option<&TypeExpr>,
    def: &Expr,
    type_fields: &HashMap<String, BTreeSet<String>>,
) -> Option<BTreeSet<String>> {
    if let Some(ann) = ann {
        if let Some(fields) = fields_from_type_expr(ann, type_fields) {
            return Some(fields);
        }
    }

    if let Expr::Ann(_, _inner, ann) = def {
        if let Some(fields) = fields_from_type_expr(ann, type_fields) {
            return Some(fields);
        }
    }

    if let Expr::Dict(_, entries) = def {
        let fields: BTreeSet<String> = entries.keys().map(|name| name.to_string()).collect();
        if !fields.is_empty() {
            return Some(fields);
        }
    }

    None
}

fn fields_from_type_expr(
    typ: &TypeExpr,
    type_fields: &HashMap<String, BTreeSet<String>>,
) -> Option<BTreeSet<String>> {
    match typ {
        TypeExpr::Record(_, entries) => {
            let fields: BTreeSet<String> =
                entries.iter().map(|(name, _)| name.to_string()).collect();
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

fn type_name_from_type_expr(typ: &TypeExpr) -> Option<String> {
    match typ {
        TypeExpr::Name(_, name) => Some(name.to_string()),
        TypeExpr::App(_, fun, _) => type_name_from_type_expr(fun),
        _ => None,
    }
}

fn fields_for_expr(
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

fn project_base_at_position<'a>(expr: &'a Expr, position: RexPosition) -> Option<&'a Expr> {
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
        Expr::Let(_, _var, _ann, def, body) => {
            if let Some(found) = project_base_at_position(def, position) {
                return Some(found);
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

fn fallback_field_map(tokens: &Tokens) -> HashMap<String, BTreeSet<String>> {
    let mut map = HashMap::new();
    let items = &tokens.items;
    let mut index = 0usize;
    while index + 2 < items.len() {
        if let Token::Ident(name, ..) = &items[index] {
            let mut handled = false;
            if matches!(items[index + 1], Token::Assign(..))
                && matches!(items[index + 2], Token::BraceL(..))
            {
                if let Some((fields, end_index)) = parse_record_fields(items, index + 2) {
                    if !fields.is_empty() {
                        map.insert(name.clone(), fields);
                    }
                    index = end_index + 1;
                    handled = true;
                }
            }
            if !handled
                && matches!(items[index + 1], Token::Colon(..))
                && matches!(items[index + 2], Token::BraceL(..))
            {
                if let Some((fields, end_index)) = parse_record_fields(items, index + 2) {
                    if !fields.is_empty() {
                        map.insert(name.clone(), fields);
                    }
                    index = end_index + 1;
                    continue;
                }
            }
        }
        index += 1;
    }
    map
}

fn parse_record_fields(tokens: &[Token], start_index: usize) -> Option<(BTreeSet<String>, usize)> {
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
                if let Some(next) = tokens.get(index + 1) {
                    if matches!(next, Token::Assign(..) | Token::Colon(..)) {
                        fields.insert(name.clone());
                    }
                }
            }
            _ => {}
        }
        index += 1;
    }

    None
}

fn field_base_ident(text: &str, position: Position) -> Option<String> {
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

fn is_word_byte(byte: u8) -> bool {
    let ch = byte as char;
    ch.is_ascii_alphanumeric() || ch == '_'
}

fn ident_token_at_position(tokens: &Tokens, position: Position) -> Option<(String, Span)> {
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

fn index_type_and_constructor_spans(
    program: &Program,
    tokens: &Tokens,
) -> (HashMap<String, Span>, HashMap<String, Span>) {
    fn span_contains_span(outer: Span, inner: Span) -> bool {
        position_leq(outer.begin, inner.begin) && position_leq(inner.end, outer.end)
    }

    let mut type_defs = HashMap::new();
    let mut ctor_defs = HashMap::new();

    for decl in &program.decls {
        let Decl::Type(td) = decl else {
            continue;
        };
        let decl_span = td.span;

        let mut expect_type_name = false;
        let mut expect_ctor_name = false;

        for token in &tokens.items {
            let token_span = *token.span();
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

    (type_defs, ctor_defs)
}

fn definition_span_for_value_ident(
    expr: &Expr,
    position: RexPosition,
    ident: &str,
    bindings: &mut Vec<(String, Span)>,
    tokens: &Tokens,
) -> Option<Span> {
    if !position_in_span(position, *expr.span()) {
        return None;
    }

    fn span_contains_span(outer: Span, inner: Span) -> bool {
        position_leq(outer.begin, inner.begin) && position_leq(inner.end, outer.end)
    }

    fn lookup_binding(bindings: &[(String, Span)], ident: &str) -> Option<Span> {
        bindings
            .iter()
            .rev()
            .find_map(|(name, span)| (name == ident).then_some(*span))
    }

    fn token_span_for_ident_in_span(tokens: &Tokens, within: Span, ident: &str) -> Option<Span> {
        tokens.items.iter().find_map(|t| match t {
            Token::Ident(name, span, ..) if name == ident && span_contains_span(within, *span) => {
                Some(*span)
            }
            _ => None,
        })
    }

    fn definition_in_pattern(
        pat: &Pattern,
        position: RexPosition,
        ident: &str,
        tokens: &Tokens,
    ) -> Option<Span> {
        if !position_in_span(position, *pat.span()) {
            return None;
        }

        match pat {
            Pattern::Var(var) => (var.name.as_ref() == ident).then_some(var.span),
            Pattern::Named(_span, _name, args) => args
                .iter()
                .find_map(|arg| definition_in_pattern(arg, position, ident, tokens)),
            Pattern::List(_span, elems) => elems
                .iter()
                .find_map(|elem| definition_in_pattern(elem, position, ident, tokens)),
            Pattern::Cons(_span, head, tail) => {
                definition_in_pattern(head, position, ident, tokens)
                    .or_else(|| definition_in_pattern(tail, position, ident, tokens))
            }
            Pattern::Dict(span, keys) => {
                if keys.iter().any(|k| k.as_ref() == ident) {
                    token_span_for_ident_in_span(tokens, *span, ident).or(Some(*span))
                } else {
                    None
                }
            }
            Pattern::Wildcard(..) => None,
        }
    }

    fn push_pattern_bindings(pat: &Pattern, bindings: &mut Vec<(String, Span)>, tokens: &Tokens) {
        match pat {
            Pattern::Var(var) => bindings.push((var.name.to_string(), var.span)),
            Pattern::Named(_span, _name, args) => {
                for arg in args {
                    push_pattern_bindings(arg, bindings, tokens);
                }
            }
            Pattern::List(_span, elems) => {
                for elem in elems {
                    push_pattern_bindings(elem, bindings, tokens);
                }
            }
            Pattern::Cons(_span, head, tail) => {
                push_pattern_bindings(head, bindings, tokens);
                push_pattern_bindings(tail, bindings, tokens);
            }
            Pattern::Dict(span, keys) => {
                for key in keys {
                    let key_span =
                        token_span_for_ident_in_span(tokens, *span, key.as_ref()).unwrap_or(*span);
                    bindings.push((key.to_string(), key_span));
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
        Expr::Let(_span, var, _ann, def, body) => {
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

fn pattern_vars(pattern: &Pattern) -> Vec<String> {
    let mut vars = Vec::new();
    collect_pattern_vars(pattern, &mut vars);
    vars
}

fn collect_pattern_vars(pattern: &Pattern, vars: &mut Vec<String>) {
    match pattern {
        Pattern::Var(var) => vars.push(var.name.to_string()),
        Pattern::Named(_, _name, args) => {
            for arg in args {
                collect_pattern_vars(arg, vars);
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
        Pattern::Dict(_, keys) => {
            for key in keys {
                vars.push(key.to_string());
            }
        }
        Pattern::Wildcard(_) => {}
    }
}

fn is_field_completion(text: &str, position: Position) -> bool {
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

fn lsp_to_rex_position(position: Position) -> RexPosition {
    RexPosition::new(position.line as usize + 1, position.character as usize + 1)
}

fn position_in_span(position: RexPosition, span: Span) -> bool {
    position_leq(span.begin, position) && position_leq(position, span.end)
}

fn position_leq(left: RexPosition, right: RexPosition) -> bool {
    left.line < right.line || (left.line == right.line && left.column <= right.column)
}

fn word_at_position(text: &str, position: Position) -> Option<String> {
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

fn offset_at(text: &str, position: Position) -> Option<usize> {
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

fn is_word_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(RexServer::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}

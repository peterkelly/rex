#![forbid(unsafe_code)]

use std::collections::{BTreeSet, HashMap};

use rex_ast::expr::{Decl, Expr, Pattern, Program, TypeDecl, TypeExpr};
use rex_lexer::{
    span::{Position as RexPosition, Span, Spanned},
    LexicalError, Token, Tokens,
};
use rex_parser::Parser;
use rex_ts::{
    instantiate, unify, Type, TypeError as TsTypeError, TypeKind, TypeSystem, TypedExpr,
    TypedExprKind,
};
use rex_ts::Types;
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

        let contents = hover_type_contents(&text, position).or_else(|| {
            let word = word_at_position(&text, position)?;
            hover_contents(&word)
        });

        Ok(contents.map(|contents| Hover {
            contents,
            range: None,
        }))
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

        let index = index_decl_spans(&program, &tokens);
        let pos = lsp_to_rex_position(position);

        // Pick the expression tree that actually contains the cursor. Top-level
        // instance method bodies are not part of `expr_with_fns()`, so we have
        // to handle them explicitly.
        let expr_with_fns = program.expr_with_fns();
        let mut root_expr: &Expr = expr_with_fns.as_ref();
        for decl in &program.decls {
            let Decl::Instance(inst) = decl else {
                continue;
            };
            for method in &inst.methods {
                if position_in_span(pos, *method.body.span()) {
                    root_expr = method.body.as_ref();
                    break;
                }
            }
        }

        let value_def =
            definition_span_for_value_ident(root_expr, pos, &ident, &mut Vec::new(), &tokens);

        let instance_method_def = index.instance_method_defs.iter().find_map(|(span, methods)| {
            if position_in_span(pos, *span) {
                methods.get(&ident).copied()
            } else {
                None
            }
        });

        let target_span = value_def
            .or_else(|| instance_method_def)
            .or_else(|| index.class_method_defs.get(&ident).copied())
            .or_else(|| index.fn_defs.get(&ident).copied())
            .or_else(|| index.ctor_defs.get(&ident).copied())
            .or_else(|| index.type_defs.get(&ident).copied())
            .or_else(|| index.class_defs.get(&ident).copied());

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
                match parser.parse_program() {
                    Err(errors) => {
                        for err in errors {
                            diagnostics.push(diagnostic_for_span(err.span, err.message));
                            if diagnostics.len() >= MAX_DIAGNOSTICS {
                                break;
                            }
                        }
                    }
                    Ok(program) => {
                        if diagnostics.len() < MAX_DIAGNOSTICS {
                            push_type_diagnostics(text, &program, &mut diagnostics);
                        }
                    }
                };
            }
        }
        Err(err) => {
            let LexicalError::UnexpectedToken(span) = err;
            diagnostics.push(diagnostic_for_span(span, "Unexpected token"));
        }
    }

    diagnostics
}

struct HoverType {
    span: Span,
    label: String,
    typ: String,
    overloads: Vec<String>,
}

fn hover_type_contents(text: &str, position: Position) -> Option<HoverContents> {
    let tokens = Token::tokenize(text).ok()?;
    let (name, name_span, name_is_ident) = name_token_at_position(&tokens, position)?;

    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().ok()?;

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

    let mut ts = TypeSystem::with_prelude();
    let mut prepared_target_instance = None;

    for (decl_idx, decl) in program.decls.iter().enumerate() {
        match decl {
            Decl::Type(ty) => ts.inject_type_decl(ty).ok()?,
            Decl::Class(class_decl) => ts.inject_class_decl(class_decl).ok()?,
            Decl::Instance(inst_decl) => {
                let prepared = ts.inject_instance_decl(inst_decl).ok()?;
                if target_instance.is_some_and(|(i, _)| i == decl_idx) {
                    prepared_target_instance = Some(prepared);
                }
            }
            Decl::Fn(_) => {
                // `fn` bodies are represented in `program.expr_with_fns()`.
            }
            Decl::DeclareFn(fd) => {
                ts.inject_declare_fn_decl(fd).ok()?;
            }
        }
    }

    let expr_with_fns = program.expr_with_fns();

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
        let (typed, _preds, _typ) = ts.infer_typed(expr_with_fns.as_ref()).ok()?;
        typed_root = typed;
        root_expr = expr_with_fns.as_ref();
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

fn hover_type_in_expr(
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
                let Some(schemes) = ts.env.lookup(ctor) else {
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
	                let list_ty = Type::app(Type::con("List", 1), elem.clone());
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
                let list_ty = Type::app(Type::con("List", 1), elem.clone());
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
                    let dict_ty = Type::app(Type::con("Dict", 1), elem.clone());
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

    fn visit(
        ts: &mut TypeSystem,
        expr: &Expr,
        typed: &TypedExpr,
        pos: RexPosition,
        name: &str,
        name_span: Span,
        name_is_ident: bool,
        best: &mut Option<HoverType>,
    ) {
        if !span_contains_pos(*expr.span(), pos) {
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
        if name_is_ident {
            if let (
                Expr::Match(_span, _scrutinee, arms),
                TypedExprKind::Match {
                    scrutinee,
                    arms: typed_arms,
                },
            ) = (&expr, &typed.kind)
            {
                if span_contains_span(*expr.span(), name_span) {
                for ((_pat, _arm_body), (typed_pat, _typed_arm_body)) in
                    arms.iter().zip(typed_arms.iter())
                {
                    // The `Pattern` is cloned into the typed tree; use either.
                    let pat_span = *typed_pat.span();
                    if span_contains_span(pat_span, name_span) {
                        let mut bindings: HashMap<String, Type> = HashMap::new();
                        add_bindings_from_pattern(ts, &scrutinee.typ, typed_pat, &mut bindings);
                        if let Some(ty) = bindings.get(name) {
                            consider(
                                best,
                                HoverType {
                                    span: name_span,
                                    label: name.to_string(),
                                    typ: ty.to_string(),
                                    overloads: Vec::new(),
                                },
                            );
                        }
                        break;
                    }
                }
            }
        }
        }

        // 2) Binding sites: `let x = ...` and lambda params.
        match (expr, &typed.kind) {
            (
                Expr::Let(_span, binding, _ann, def, body),
                TypedExprKind::Let {
                    def: tdef,
                    body: tbody,
                    ..
                },
            ) => {
                if span_contains_pos(binding.span, pos) {
                    consider(
                        best,
                        HoverType {
                            span: binding.span,
                            label: binding.name.as_ref().to_string(),
                            typ: tdef.typ.to_string(),
                            overloads: Vec::new(),
                        },
                    );
                }
                visit(
                    ts,
                    def.as_ref(),
                    tdef.as_ref(),
                    pos,
                    name,
                    name_span,
                    name_is_ident,
                    best,
                );
                visit(
                    ts,
                    body.as_ref(),
                    tbody.as_ref(),
                    pos,
                    name,
                    name_span,
                    name_is_ident,
                    best,
                );
            }
            (Expr::Lam(_span, _scope, param, _ann, _constraints, body), TypedExprKind::Lam { body: tbody, .. }) => {
                if span_contains_pos(param.span, pos) {
                    let param_ty = match typed.typ.as_ref() {
                        TypeKind::Fun(a, _b) => a.to_string(),
                        _ => "<unknown>".to_string(),
                    };
                    consider(
                        best,
                        HoverType {
                            span: param.span,
                            label: param.name.as_ref().to_string(),
                            typ: param_ty,
                            overloads: Vec::new(),
                        },
                    );
                }
                visit(ts, body.as_ref(), tbody.as_ref(), pos, name, name_span, name_is_ident, best);
            }
            (Expr::Var(v), TypedExprKind::Var { overloads, .. }) => {
                if span_contains_pos(v.span, pos) {
                    consider(
                        best,
                        HoverType {
                            span: v.span,
                            label: v.name.as_ref().to_string(),
                            typ: typed.typ.to_string(),
                            overloads: overloads.iter().map(|t| t.to_string()).collect(),
                        },
                    );
                }
            }
            (Expr::Ann(_span, inner, _ann), _) => {
                visit(ts, inner.as_ref(), typed, pos, name, name_span, name_is_ident, best);
            }
            (Expr::Tuple(_span, elems), TypedExprKind::Tuple(telems)) => {
                for (e, t) in elems.iter().zip(telems.iter()) {
                    visit(ts, e.as_ref(), t, pos, name, name_span, name_is_ident, best);
                }
            }
            (Expr::List(_span, elems), TypedExprKind::List(telems)) => {
                for (e, t) in elems.iter().zip(telems.iter()) {
                    visit(ts, e.as_ref(), t, pos, name, name_span, name_is_ident, best);
                }
            }
            (Expr::Dict(_span, kvs), TypedExprKind::Dict(tkvs)) => {
                for (k, v) in kvs {
                    if let Some(tv) = tkvs.get(k) {
                        visit(ts, v.as_ref(), tv, pos, name, name_span, name_is_ident, best);
                    }
                }
            }
            (Expr::RecordUpdate(_span, base, updates), TypedExprKind::RecordUpdate { base: tbase, updates: tupdates }) => {
                visit(ts, base.as_ref(), tbase.as_ref(), pos, name, name_span, name_is_ident, best);
                for (k, v) in updates {
                    if let Some(tv) = tupdates.get(k) {
                        visit(ts, v.as_ref(), tv, pos, name, name_span, name_is_ident, best);
                    }
                }
            }
            (Expr::App(_span, f, x), TypedExprKind::App(tf, tx)) => {
                visit(ts, f.as_ref(), tf.as_ref(), pos, name, name_span, name_is_ident, best);
                visit(ts, x.as_ref(), tx.as_ref(), pos, name, name_span, name_is_ident, best);
            }
            (Expr::Project(_span, e, _field), TypedExprKind::Project { expr: te, .. }) => {
                visit(ts, e.as_ref(), te.as_ref(), pos, name, name_span, name_is_ident, best);
            }
            (Expr::Ite(_span, c, t, e), TypedExprKind::Ite { cond, then_expr, else_expr }) => {
                visit(ts, c.as_ref(), cond.as_ref(), pos, name, name_span, name_is_ident, best);
                visit(ts, t.as_ref(), then_expr.as_ref(), pos, name, name_span, name_is_ident, best);
                visit(ts, e.as_ref(), else_expr.as_ref(), pos, name, name_span, name_is_ident, best);
            }
            (Expr::Match(_span, scrutinee, arms), TypedExprKind::Match { scrutinee: tscrut, arms: tarms }) => {
                visit(ts, scrutinee.as_ref(), tscrut.as_ref(), pos, name, name_span, name_is_ident, best);
                for ((_pat, arm_body), (_tpat, tarm_body)) in arms.iter().zip(tarms.iter()) {
                    visit(ts, arm_body.as_ref(), tarm_body, pos, name, name_span, name_is_ident, best);
                }
            }
            _ => {}
        }
    }

    let mut best = None;
    visit(ts, expr, typed, pos, name, name_span, name_is_ident, &mut best);
    best
}

fn name_token_at_position(tokens: &Tokens, position: Position) -> Option<(String, Span, bool)> {
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

fn push_type_diagnostics(text: &str, program: &Program, diagnostics: &mut Vec<Diagnostic>) {
    // Type inference is meaningfully more expensive than lex/parse, and we run
    // diagnostics on every full-text change. Keep the cost model explicit.
    const MAX_TYPECHECK_BYTES: usize = 256 * 1024;
    if text.len() > MAX_TYPECHECK_BYTES {
        return;
    }

    let mut ts = TypeSystem::with_prelude();
    for decl in &program.decls {
        match decl {
            Decl::Type(ty) => {
                if let Err(err) = ts.inject_type_decl(ty) {
                    push_ts_error(err, diagnostics);
                    return;
                }
            }
            Decl::Class(class_decl) => {
                if let Err(err) = ts.inject_class_decl(class_decl) {
                    push_ts_error(err, diagnostics);
                    return;
                }
            }
            Decl::Instance(inst_decl) => {
                let prepared = match ts.inject_instance_decl(inst_decl) {
                    Ok(prepared) => prepared,
                    Err(err) => {
                        push_ts_error(err, diagnostics);
                        return;
                    }
                };

                // Typecheck instance method bodies too, so errors inside the
                // instance show up as diagnostics.
                for method in &inst_decl.methods {
                    if let Err(err) = ts.typecheck_instance_method(&prepared, method) {
                        push_ts_error(err, diagnostics);
                        return;
                    }
                }
            }
            Decl::Fn(fd) => {
                if let Err(err) = ts.inject_fn_decl(fd) {
                    push_ts_error(err, diagnostics);
                    return;
                }
            }
            Decl::DeclareFn(fd) => {
                if let Err(err) = ts.inject_declare_fn_decl(fd) {
                    push_ts_error(err, diagnostics);
                    return;
                }
            }
        }
    }

    if let Err(err) = ts.infer(program.expr.as_ref()) {
        push_ts_error(err, diagnostics);
    }
}

fn push_ts_error(err: TsTypeError, diagnostics: &mut Vec<Diagnostic>) {
    let (span, message) = match err {
        TsTypeError::Spanned { span, error } => (span, error.to_string()),
        other => (Span::default(), other.to_string()),
    };
    diagnostics.push(Diagnostic {
        range: span_to_range(span),
        severity: Some(DiagnosticSeverity::ERROR),
        message,
        source: Some("rex-ts".to_string()),
        ..Diagnostic::default()
    });
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

struct DeclSpanIndex {
    type_defs: HashMap<String, Span>,
    ctor_defs: HashMap<String, Span>,
    class_defs: HashMap<String, Span>,
    fn_defs: HashMap<String, Span>,
    class_method_defs: HashMap<String, Span>,
    instance_method_defs: Vec<(Span, HashMap<String, Span>)>,
}

fn index_decl_spans(program: &Program, tokens: &Tokens) -> DeclSpanIndex {
    fn span_contains_span(outer: Span, inner: Span) -> bool {
        position_leq(outer.begin, inner.begin) && position_leq(inner.end, outer.end)
    }

    let mut type_defs = HashMap::new();
    let mut ctor_defs = HashMap::new();
    let mut class_defs = HashMap::new();
    let mut fn_defs = HashMap::new();
    let mut class_method_defs = HashMap::new();
    let mut instance_method_defs = Vec::new();

    for decl in &program.decls {
        match decl {
            Decl::Type(td) => {
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
            Decl::Class(cd) => {
                let decl_span = cd.span;
                let mut expect_class_name = false;
                for i in 0..tokens.items.len() {
                    let token = &tokens.items[i];
                    let token_span = *token.span();
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
                            if let Some(next) = tokens.items.get(i + 1) {
                                if matches!(next, Token::Colon(..)) {
                                    class_method_defs.insert(name.clone(), *span);
                                }
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
                    let token_span = *token.span();
                    if !span_contains_span(decl_span, token_span) {
                        continue;
                    }
                    if let Token::Ident(name, span, ..) = token {
                        if let Some(next) = tokens.items.get(i + 1) {
                            if matches!(next, Token::Assign(..)) {
                                methods.insert(name.clone(), *span);
                            }
                        }
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

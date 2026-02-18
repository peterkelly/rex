#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, CompletionItem, CompletionItemKind,
    Diagnostic, DiagnosticSeverity, DocumentSymbol, GotoDefinitionResponse, Hover, HoverContents,
    Location, MarkupContent, MarkupKind, Position, Range, SymbolKind, TextEdit, Url, WorkspaceEdit,
};
use rex_ast::expr::{
    Decl, DeclareFnDecl, Expr, FnDecl, ImportDecl, ImportPath, InstanceDecl, Pattern, Program,
    Symbol, TypeDecl, TypeExpr, Var, intern,
};
use rex_lexer::{
    LexicalError, Token, Tokens,
    span::{Position as RexPosition, Span, Spanned},
};
use rex_parser::Parser;
use rex_parser::error::ParserErr;
use rex_ts::Types;
use rex_ts::{
    Scheme, Type, TypeError as TsTypeError, TypeKind, TypeSystem, TypedExpr, TypedExprKind,
    instantiate, unify,
};
use rex_util::{GasMeter, sha256_hex};
use serde_json::{Value, json, to_value};
#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::RwLock;
#[cfg(not(target_arch = "wasm32"))]
use tower_lsp::jsonrpc::Result;
#[cfg(not(target_arch = "wasm32"))]
use tower_lsp::lsp_types::{
    CodeActionOptions, CodeActionParams, CodeActionResponse, CompletionOptions, CompletionParams,
    CompletionResponse, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DocumentFormattingParams, DocumentSymbolParams,
    DocumentSymbolResponse, ExecuteCommandOptions, ExecuteCommandParams, GotoDefinitionParams,
    HoverParams, HoverProviderCapability, InitializeParams, InitializeResult, InitializedParams,
    MessageType, OneOf, ReferenceParams, RenameParams, ServerCapabilities, ServerInfo,
    TextDocumentSyncCapability, TextDocumentSyncKind,
};
#[cfg(not(target_arch = "wasm32"))]
use tower_lsp::{Client, LanguageServer, LspService, Server};

const MAX_DIAGNOSTICS: usize = 50;
const CMD_EXPECTED_TYPE_AT: &str = "rex.expectedTypeAt";
const CMD_FUNCTIONS_PRODUCING_EXPECTED_TYPE_AT: &str = "rex.functionsProducingExpectedTypeAt";
const CMD_FUNCTIONS_ACCEPTING_INFERRED_TYPE_AT: &str = "rex.functionsAcceptingInferredTypeAt";
const CMD_ADAPTERS_FROM_INFERRED_TO_EXPECTED_AT: &str = "rex.adaptersFromInferredToExpectedAt";
const CMD_FUNCTIONS_COMPATIBLE_WITH_IN_SCOPE_VALUES_AT: &str =
    "rex.functionsCompatibleWithInScopeValuesAt";
const CMD_HOLES_EXPECTED_TYPES: &str = "rex.holesExpectedTypes";
const CMD_SEMANTIC_LOOP_STEP: &str = "rex.semanticLoopStep";
const CMD_SEMANTIC_LOOP_APPLY_QUICK_FIX_AT: &str = "rex.semanticLoopApplyQuickFixAt";
const CMD_SEMANTIC_LOOP_APPLY_BEST_QUICK_FIXES_AT: &str = "rex.semanticLoopApplyBestQuickFixesAt";
const NO_IMPROVEMENT_STREAK_LIMIT: usize = 2;
const MAX_SEMANTIC_ENV_SCHEMES_SCAN: usize = 1024;
const MAX_SEMANTIC_IN_SCOPE_VALUES: usize = 128;
const MAX_SEMANTIC_CANDIDATES: usize = 64;
const MAX_SEMANTIC_HOLE_FILL_ARITY: usize = 8;
const MAX_SEMANTIC_HOLES: usize = 128;
const BUILTIN_TYPES: &[&str] = &[
    "u8", "u16", "u32", "u64", "i8", "i16", "i32", "i64", "f32", "f64", "bool", "string", "uuid",
    "datetime", "Dict", "List", "Array", "Option", "Result",
];
const BUILTIN_VALUES: &[&str] = &["true", "false", "null", "Some", "None", "Ok", "Err"];

#[derive(Debug)]
enum TokenizeOrParseError {
    Lex(LexicalError),
    Parse(Vec<ParserErr>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BulkQuickFixStrategy {
    Conservative,
    Aggressive,
}

impl BulkQuickFixStrategy {
    fn parse(s: &str) -> Self {
        if s.eq_ignore_ascii_case("aggressive") {
            Self::Aggressive
        } else {
            Self::Conservative
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Conservative => "conservative",
            Self::Aggressive => "aggressive",
        }
    }
}

#[derive(Clone)]
struct CachedParse {
    hash: u64,
    tokens: Tokens,
    program: Program,
}

fn text_hash(text: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

fn parse_cache() -> &'static Mutex<HashMap<Url, CachedParse>> {
    static CACHE: OnceLock<Mutex<HashMap<Url, CachedParse>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn semantic_candidate_values(ts: &TypeSystem) -> Vec<(Symbol, Vec<Scheme>)> {
    let mut out = Vec::new();
    let mut scanned = 0usize;
    for (name, schemes) in &ts.env.values {
        if scanned >= MAX_SEMANTIC_ENV_SCHEMES_SCAN {
            break;
        }
        let remaining = MAX_SEMANTIC_ENV_SCHEMES_SCAN - scanned;
        let kept = schemes.iter().take(remaining).cloned().collect::<Vec<_>>();
        if kept.is_empty() {
            continue;
        }
        scanned += kept.len();
        out.push((name.clone(), kept));
    }
    out
}

fn clear_parse_cache(uri: &Url) {
    let Ok(mut cache) = parse_cache().lock() else {
        return;
    };
    cache.remove(uri);
}

#[cfg(not(target_arch = "wasm32"))]
fn uri_to_file_path(uri: &Url) -> Option<PathBuf> {
    uri.to_file_path().ok()
}

#[cfg(target_arch = "wasm32")]
fn uri_to_file_path(_uri: &Url) -> Option<PathBuf> {
    None
}

#[cfg(not(target_arch = "wasm32"))]
fn url_from_file_path(path: &std::path::Path) -> Option<Url> {
    Url::from_file_path(path).ok()
}

#[cfg(target_arch = "wasm32")]
fn url_from_file_path(_path: &std::path::Path) -> Option<Url> {
    None
}

fn tokenize_and_parse(text: &str) -> std::result::Result<(Tokens, Program), TokenizeOrParseError> {
    let tokens = Token::tokenize(text).map_err(TokenizeOrParseError::Lex)?;
    let mut parser = Parser::new(tokens.clone());
    let program = parser
        .parse_program(&mut GasMeter::default())
        .map_err(TokenizeOrParseError::Parse)?;
    Ok((tokens, program))
}

fn tokenize_and_parse_cached(
    uri: &Url,
    text: &str,
) -> std::result::Result<(Tokens, Program), TokenizeOrParseError> {
    let hash = text_hash(text);
    if let Ok(cache) = parse_cache().lock()
        && let Some(cached) = cache.get(uri)
        && cached.hash == hash
    {
        return Ok((cached.tokens.clone(), cached.program.clone()));
    }

    let (tokens, program) = tokenize_and_parse(text)?;
    if let Ok(mut cache) = parse_cache().lock() {
        cache.insert(
            uri.clone(),
            CachedParse {
                hash,
                tokens: tokens.clone(),
                program: program.clone(),
            },
        );
    }
    Ok((tokens, program))
}

#[derive(Clone)]
struct ImportModuleInfo {
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    path: Option<PathBuf>,
    value_map: HashMap<rex_ast::expr::Symbol, rex_ast::expr::Symbol>, // field -> internal name
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    export_defs: HashMap<String, Span>,
}

fn is_ident_like(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn prelude_completion_values() -> &'static Vec<(String, CompletionItemKind)> {
    static PRELUDE_VALUES: OnceLock<Vec<(String, CompletionItemKind)>> = OnceLock::new();
    PRELUDE_VALUES.get_or_init(|| {
        let ts = match TypeSystem::with_prelude() {
            Ok(ts) => ts,
            Err(e) => {
                eprintln!("rex-lsp: failed to build prelude for completions: {e}");
                return Vec::new();
            }
        };
        let mut out = Vec::new();
        for (name, schemes) in ts.env.values.iter() {
            let name = name.as_ref().to_string();
            if !is_ident_like(&name) {
                continue;
            }
            let is_fun = schemes
                .iter()
                .any(|scheme| matches!(scheme.typ.as_ref(), TypeKind::Fun(..)));
            let kind = if is_fun {
                CompletionItemKind::FUNCTION
            } else {
                CompletionItemKind::VARIABLE
            };
            out.push((name, kind));
        }
        out.sort_by(|(a, _), (b, _)| a.cmp(b));
        out
    })
}

fn module_prefix(hash: &str) -> String {
    let short = if hash.len() >= 16 { &hash[..16] } else { hash };
    format!("@m{short}")
}

fn inject_program_decls(
    ts: &mut TypeSystem,
    program: &Program,
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
            ts.inject_decls(pending)?;
            pending.clear();
            Ok(())
        };

    for (idx, decl) in program.decls.iter().enumerate() {
        match decl {
            Decl::Instance(inst_decl) => {
                flush_non_instances(ts, &mut pending_non_instances)?;
                let prepared = ts.inject_instance_decl(inst_decl)?;
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

type PreparedInstanceDecl = rex_ts::PreparedInstanceDecl;
type PreparedInstance = (usize, PreparedInstanceDecl);
type InjectedDecls = (Vec<PreparedInstance>, Option<PreparedInstanceDecl>);

fn rewrite_type_expr(
    ty: &TypeExpr,
    type_map: &HashMap<rex_ast::expr::Symbol, rex_ast::expr::Symbol>,
) -> TypeExpr {
    match ty {
        TypeExpr::Name(span, name) => {
            if let Some(new) = type_map.get(name) {
                TypeExpr::Name(*span, new.clone())
            } else {
                TypeExpr::Name(*span, name.clone())
            }
        }
        TypeExpr::App(span, f, x) => TypeExpr::App(
            *span,
            Box::new(rewrite_type_expr(f, type_map)),
            Box::new(rewrite_type_expr(x, type_map)),
        ),
        TypeExpr::Fun(span, a, b) => TypeExpr::Fun(
            *span,
            Box::new(rewrite_type_expr(a, type_map)),
            Box::new(rewrite_type_expr(b, type_map)),
        ),
        TypeExpr::Tuple(span, elems) => TypeExpr::Tuple(
            *span,
            elems
                .iter()
                .map(|e| rewrite_type_expr(e, type_map))
                .collect(),
        ),
        TypeExpr::Record(span, fields) => TypeExpr::Record(
            *span,
            fields
                .iter()
                .map(|(name, ty)| (name.clone(), rewrite_type_expr(ty, type_map)))
                .collect(),
        ),
    }
}

fn collect_pattern_bindings(pat: &Pattern, out: &mut Vec<rex_ast::expr::Symbol>) {
    match pat {
        Pattern::Wildcard(..) => {}
        Pattern::Var(v) => out.push(v.name.clone()),
        Pattern::Named(_, _, args) => {
            for arg in args {
                collect_pattern_bindings(arg, out);
            }
        }
        Pattern::Tuple(_, elems) | Pattern::List(_, elems) => {
            for elem in elems {
                collect_pattern_bindings(elem, out);
            }
        }
        Pattern::Cons(_, head, tail) => {
            collect_pattern_bindings(head, out);
            collect_pattern_bindings(tail, out);
        }
        Pattern::Dict(_, fields) => {
            for (_, pat) in fields {
                collect_pattern_bindings(pat, out);
            }
        }
    }
}

fn rewrite_import_projections_expr(
    expr: &Expr,
    bound: &mut BTreeSet<rex_ast::expr::Symbol>,
    imports: &HashMap<rex_ast::expr::Symbol, ImportModuleInfo>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Expr {
    match expr {
        Expr::Project(span, base, field) => {
            if let Expr::Var(v) = base.as_ref()
                && !bound.contains(&v.name)
                && let Some(info) = imports.get(&v.name)
            {
                if let Some(internal) = info.value_map.get(field) {
                    return Expr::Var(Var {
                        span: *span,
                        name: internal.clone(),
                    });
                }
                diagnostics.push(diagnostic_for_span(
                    *span,
                    format!("module `{}` does not export `{}`", v.name, field),
                ));
            }
            Expr::Project(
                *span,
                std::sync::Arc::new(rewrite_import_projections_expr(
                    base,
                    bound,
                    imports,
                    diagnostics,
                )),
                field.clone(),
            )
        }
        Expr::Var(v) => Expr::Var(v.clone()),
        Expr::Bool(span, v) => Expr::Bool(*span, *v),
        Expr::Uint(span, v) => Expr::Uint(*span, *v),
        Expr::Int(span, v) => Expr::Int(*span, *v),
        Expr::Float(span, v) => Expr::Float(*span, *v),
        Expr::String(span, v) => Expr::String(*span, v.clone()),
        Expr::Uuid(span, v) => Expr::Uuid(*span, *v),
        Expr::DateTime(span, v) => Expr::DateTime(*span, *v),
        Expr::Hole(span) => Expr::Hole(*span),
        Expr::Tuple(span, elems) => Expr::Tuple(
            *span,
            elems
                .iter()
                .map(|e| {
                    std::sync::Arc::new(rewrite_import_projections_expr(
                        e,
                        bound,
                        imports,
                        diagnostics,
                    ))
                })
                .collect(),
        ),
        Expr::List(span, elems) => Expr::List(
            *span,
            elems
                .iter()
                .map(|e| {
                    std::sync::Arc::new(rewrite_import_projections_expr(
                        e,
                        bound,
                        imports,
                        diagnostics,
                    ))
                })
                .collect(),
        ),
        Expr::Dict(span, kvs) => Expr::Dict(
            *span,
            kvs.iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        std::sync::Arc::new(rewrite_import_projections_expr(
                            v,
                            bound,
                            imports,
                            diagnostics,
                        )),
                    )
                })
                .collect(),
        ),
        Expr::RecordUpdate(span, base, updates) => Expr::RecordUpdate(
            *span,
            std::sync::Arc::new(rewrite_import_projections_expr(
                base,
                bound,
                imports,
                diagnostics,
            )),
            updates
                .iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        std::sync::Arc::new(rewrite_import_projections_expr(
                            v,
                            bound,
                            imports,
                            diagnostics,
                        )),
                    )
                })
                .collect(),
        ),
        Expr::App(span, f, x) => Expr::App(
            *span,
            std::sync::Arc::new(rewrite_import_projections_expr(
                f,
                bound,
                imports,
                diagnostics,
            )),
            std::sync::Arc::new(rewrite_import_projections_expr(
                x,
                bound,
                imports,
                diagnostics,
            )),
        ),
        Expr::Lam(span, scope, param, ann, constraints, body) => {
            bound.insert(param.name.clone());
            let out = Expr::Lam(
                *span,
                scope.clone(),
                param.clone(),
                ann.clone(),
                constraints.clone(),
                std::sync::Arc::new(rewrite_import_projections_expr(
                    body,
                    bound,
                    imports,
                    diagnostics,
                )),
            );
            bound.remove(&param.name);
            out
        }
        Expr::Let(span, var, ann, val, body) => {
            let val = std::sync::Arc::new(rewrite_import_projections_expr(
                val,
                bound,
                imports,
                diagnostics,
            ));
            bound.insert(var.name.clone());
            let body = std::sync::Arc::new(rewrite_import_projections_expr(
                body,
                bound,
                imports,
                diagnostics,
            ));
            bound.remove(&var.name);
            Expr::Let(*span, var.clone(), ann.clone(), val, body)
        }
        Expr::LetRec(span, bindings, body) => {
            let names: Vec<Symbol> = bindings
                .iter()
                .map(|(var, _, _)| var.name.clone())
                .collect();
            for name in &names {
                bound.insert(name.clone());
            }
            let bindings = bindings
                .iter()
                .map(|(var, ann, def)| {
                    (
                        var.clone(),
                        ann.clone(),
                        std::sync::Arc::new(rewrite_import_projections_expr(
                            def,
                            bound,
                            imports,
                            diagnostics,
                        )),
                    )
                })
                .collect();
            let body = std::sync::Arc::new(rewrite_import_projections_expr(
                body,
                bound,
                imports,
                diagnostics,
            ));
            for name in &names {
                bound.remove(name);
            }
            Expr::LetRec(*span, bindings, body)
        }
        Expr::Ite(span, c, t, e) => Expr::Ite(
            *span,
            std::sync::Arc::new(rewrite_import_projections_expr(
                c,
                bound,
                imports,
                diagnostics,
            )),
            std::sync::Arc::new(rewrite_import_projections_expr(
                t,
                bound,
                imports,
                diagnostics,
            )),
            std::sync::Arc::new(rewrite_import_projections_expr(
                e,
                bound,
                imports,
                diagnostics,
            )),
        ),
        Expr::Match(span, scrutinee, arms) => {
            let scrutinee = std::sync::Arc::new(rewrite_import_projections_expr(
                scrutinee,
                bound,
                imports,
                diagnostics,
            ));
            let mut out_arms = Vec::new();
            for (pat, arm_expr) in arms {
                let mut binds = Vec::new();
                collect_pattern_bindings(pat, &mut binds);
                for b in &binds {
                    bound.insert(b.clone());
                }
                let arm_expr = std::sync::Arc::new(rewrite_import_projections_expr(
                    arm_expr,
                    bound,
                    imports,
                    diagnostics,
                ));
                for b in &binds {
                    bound.remove(b);
                }
                out_arms.push((pat.clone(), arm_expr));
            }
            Expr::Match(*span, scrutinee, out_arms)
        }
        Expr::Ann(span, e, t) => Expr::Ann(
            *span,
            std::sync::Arc::new(rewrite_import_projections_expr(
                e,
                bound,
                imports,
                diagnostics,
            )),
            t.clone(),
        ),
    }
}

fn rewrite_program_import_projections(
    program: &Program,
    imports: &HashMap<rex_ast::expr::Symbol, ImportModuleInfo>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Program {
    let decls = program
        .decls
        .iter()
        .map(|decl| match decl {
            Decl::Fn(fd) => {
                let mut bound: BTreeSet<rex_ast::expr::Symbol> =
                    fd.params.iter().map(|(v, _)| v.name.clone()).collect();
                let body = std::sync::Arc::new(rewrite_import_projections_expr(
                    fd.body.as_ref(),
                    &mut bound,
                    imports,
                    diagnostics,
                ));
                Decl::Fn(FnDecl {
                    span: fd.span,
                    is_pub: fd.is_pub,
                    name: fd.name.clone(),
                    params: fd.params.clone(),
                    ret: fd.ret.clone(),
                    constraints: fd.constraints.clone(),
                    body,
                })
            }
            Decl::Instance(inst) => {
                let methods = inst
                    .methods
                    .iter()
                    .map(|m| {
                        let mut bound = BTreeSet::new();
                        let body = std::sync::Arc::new(rewrite_import_projections_expr(
                            m.body.as_ref(),
                            &mut bound,
                            imports,
                            diagnostics,
                        ));
                        rex_ast::expr::InstanceMethodImpl {
                            name: m.name.clone(),
                            body,
                        }
                    })
                    .collect();
                Decl::Instance(InstanceDecl {
                    span: inst.span,
                    is_pub: inst.is_pub,
                    class: inst.class.clone(),
                    head: inst.head.clone(),
                    context: inst.context.clone(),
                    methods,
                })
            }
            other => other.clone(),
        })
        .collect();

    let mut bound = BTreeSet::new();
    let expr = std::sync::Arc::new(rewrite_import_projections_expr(
        program.expr.as_ref(),
        &mut bound,
        imports,
        diagnostics,
    ));

    Program { decls, expr }
}

type PreparedProgram = (
    Program,
    TypeSystem,
    HashMap<rex_ast::expr::Symbol, ImportModuleInfo>,
    Vec<Diagnostic>,
);

fn prepare_program_with_imports(
    uri: &Url,
    program: &Program,
) -> std::result::Result<PreparedProgram, String> {
    let mut ts = TypeSystem::with_prelude().map_err(|e| format!("failed to build prelude: {e}"))?;
    let mut diagnostics = Vec::new();

    let importer = uri_to_file_path(uri);

    let mut imports: HashMap<rex_ast::expr::Symbol, ImportModuleInfo> = HashMap::new();

    for decl in &program.decls {
        let Decl::Import(ImportDecl {
            span, path, alias, ..
        }) = decl
        else {
            continue;
        };
        let import_span = *span;

        let (segments, expected_sha) = match path {
            ImportPath::Local { segments, sha } => (segments.as_slice(), sha.as_deref()),
            ImportPath::Remote { .. } => {
                // LSP does not attempt network fetches; leave it unresolved.
                continue;
            }
        };

        let module_name = segments
            .iter()
            .map(|s| s.as_ref())
            .collect::<Vec<_>>()
            .join(".");

        let (module_path, hash, source, module_label, keep_constraints) =
            if let Some(source) = rex_util::stdlib_source(&module_name) {
                let hash = sha256_hex(source.as_bytes());
                if let Some(expected) = expected_sha {
                    let expected = expected.to_ascii_lowercase();
                    if !hash.starts_with(&expected) {
                        diagnostics.push(diagnostic_for_span(
                        import_span,
                        format!(
                            "sha mismatch for `{module_name}`: expected #{expected}, got #{hash}",
                        ),
                    ));
                    }
                }
                (None, hash, source.to_string(), module_name, true)
            } else {
                let Some(importer) = importer.as_ref() else {
                    // Without a stable file location we cannot resolve local imports.
                    // (Stdlib imports are handled above.)
                    continue;
                };
                let Some(base_dir) = importer.parent() else {
                    diagnostics.push(diagnostic_for_span(
                        import_span,
                        "cannot resolve local import without a base directory".to_string(),
                    ));
                    continue;
                };
                let module_path = match rex_util::resolve_local_import_path(base_dir, segments) {
                    Ok(Some(p)) => p,
                    Ok(None) => {
                        diagnostics.push(diagnostic_for_span(
                            import_span,
                            format!("module not found for import `{module_name}`"),
                        ));
                        continue;
                    }
                    Err(err) => {
                        diagnostics.push(diagnostic_for_span(import_span, err.to_string()));
                        continue;
                    }
                };
                let Ok(module_path) = module_path.canonicalize() else {
                    diagnostics.push(diagnostic_for_span(
                        import_span,
                        format!("module not found for import `{module_name}`"),
                    ));
                    continue;
                };

                let bytes = match fs::read(&module_path) {
                    Ok(b) => b,
                    Err(e) => {
                        diagnostics.push(diagnostic_for_span(
                            import_span,
                            format!("failed to read module `{}`: {e}", module_path.display()),
                        ));
                        continue;
                    }
                };
                let hash = sha256_hex(&bytes);
                if let Some(expected) = expected_sha {
                    let expected = expected.to_ascii_lowercase();
                    if !hash.starts_with(&expected) {
                        diagnostics.push(diagnostic_for_span(
                            import_span,
                            format!(
                                "sha mismatch for `{}`: expected #{expected}, got #{hash}",
                                module_path.display()
                            ),
                        ));
                    }
                }

                let source = match String::from_utf8(bytes) {
                    Ok(s) => s,
                    Err(e) => {
                        diagnostics.push(diagnostic_for_span(
                            import_span,
                            format!("module `{}` is not utf-8: {e}", module_path.display()),
                        ));
                        continue;
                    }
                };
                (
                    Some(module_path.clone()),
                    hash,
                    source,
                    module_path.display().to_string(),
                    false,
                )
            };

        let (tokens, module_program) = match tokenize_and_parse(&source) {
            Ok(v) => v,
            Err(TokenizeOrParseError::Lex(err)) => {
                let msg = match err {
                    LexicalError::UnexpectedToken(span) => format!(
                        "lex error in module `{}` at {}:{}",
                        module_label, span.begin.line, span.begin.column
                    ),
                    LexicalError::InvalidLiteral {
                        kind,
                        text,
                        error,
                        span,
                    } => format!(
                        "lex error in module `{}` at {}:{}: invalid {kind} literal `{text}`: {error}",
                        module_label, span.begin.line, span.begin.column
                    ),
                    LexicalError::Internal(msg) => {
                        format!("internal lexer error in module `{module_label}`: {msg}")
                    }
                };
                diagnostics.push(diagnostic_for_span(import_span, msg));
                continue;
            }
            Err(TokenizeOrParseError::Parse(errs)) => {
                for err in errs {
                    diagnostics.push(diagnostic_for_span(
                        import_span,
                        format!(
                            "parse error in module `{}` at {}:{}: {}",
                            module_label, err.span.begin.line, err.span.begin.column, err.message
                        ),
                    ));
                    if diagnostics.len() >= MAX_DIAGNOSTICS {
                        break;
                    }
                }
                continue;
            }
        };

        let index = index_decl_spans(&module_program, &tokens);
        let prefix = module_prefix(&hash);

        let mut type_map: HashMap<rex_ast::expr::Symbol, rex_ast::expr::Symbol> = HashMap::new();
        for decl in &module_program.decls {
            if let Decl::Type(td) = decl {
                type_map.insert(
                    td.name.clone(),
                    intern(&format!("{prefix}.{}", td.name.as_ref())),
                );
            }
        }

        // Inject module type decls (renamed) so exported signatures can refer to them.
        for decl in &module_program.decls {
            let Decl::Type(td) = decl else { continue };
            let name = type_map
                .get(&td.name)
                .cloned()
                .unwrap_or_else(|| td.name.clone());
            let variants = td
                .variants
                .iter()
                .map(|v| rex_ast::expr::TypeVariant {
                    name: intern(&format!("{prefix}.{}", v.name.as_ref())),
                    args: v
                        .args
                        .iter()
                        .map(|t| rewrite_type_expr(t, &type_map))
                        .collect(),
                })
                .collect();
            let td2 = TypeDecl {
                span: td.span,
                is_pub: td.is_pub,
                name,
                params: td.params.clone(),
                variants,
            };
            let _ = ts.inject_type_decl(&td2);
        }

        let mut value_map: HashMap<rex_ast::expr::Symbol, rex_ast::expr::Symbol> = HashMap::new();
        let mut export_names: BTreeSet<String> = BTreeSet::new();

        // Exported functions (pub only)
        for decl in &module_program.decls {
            match decl {
                Decl::Fn(fd) if fd.is_pub => {
                    let internal = intern(&format!("{prefix}.{}", fd.name.name.as_ref()));
                    value_map.insert(intern(fd.name.name.as_ref()), internal.clone());
                    export_names.insert(fd.name.name.as_ref().to_string());

                    let params = fd
                        .params
                        .iter()
                        .map(|(v, ty)| (v.clone(), rewrite_type_expr(ty, &type_map)))
                        .collect();
                    let ret = rewrite_type_expr(&fd.ret, &type_map);
                    let decl = DeclareFnDecl {
                        span: fd.span,
                        is_pub: true,
                        name: Var {
                            span: fd.name.span,
                            name: internal,
                        },
                        params,
                        ret,
                        constraints: if keep_constraints {
                            fd.constraints.clone()
                        } else {
                            Default::default()
                        },
                    };
                    let _ = ts.inject_declare_fn_decl(&decl);
                }
                Decl::DeclareFn(df) if df.is_pub => {
                    let internal = intern(&format!("{prefix}.{}", df.name.name.as_ref()));
                    value_map.insert(intern(df.name.name.as_ref()), internal.clone());
                    export_names.insert(df.name.name.as_ref().to_string());

                    let params = df
                        .params
                        .iter()
                        .map(|(v, ty)| (v.clone(), rewrite_type_expr(ty, &type_map)))
                        .collect();
                    let ret = rewrite_type_expr(&df.ret, &type_map);
                    let decl = DeclareFnDecl {
                        span: df.span,
                        is_pub: true,
                        name: Var {
                            span: df.name.span,
                            name: internal,
                        },
                        params,
                        ret,
                        constraints: if keep_constraints {
                            df.constraints.clone()
                        } else {
                            Default::default()
                        },
                    };
                    let _ = ts.inject_declare_fn_decl(&decl);
                }
                Decl::Type(td) if td.is_pub => {
                    // Public constructors are accessible as values.
                    for variant in &td.variants {
                        let internal = intern(&format!("{prefix}.{}", variant.name.as_ref()));
                        value_map.insert(variant.name.clone(), internal);
                        export_names.insert(variant.name.as_ref().to_string());
                    }
                }
                _ => {}
            }
        }

        let mut export_defs = HashMap::new();
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

        imports.insert(
            alias.clone(),
            ImportModuleInfo {
                path: module_path,
                value_map,
                export_defs,
            },
        );
    }

    let rewritten = rewrite_program_import_projections(program, &imports, &mut diagnostics);
    Ok((rewritten, ts, imports, diagnostics))
}

fn completion_exports_for_module_alias(
    uri: &Url,
    program: &Program,
    alias: &str,
) -> std::result::Result<Vec<String>, String> {
    let alias_sym = intern(alias);
    let Some(import_decl) = program.decls.iter().find_map(|d| {
        let Decl::Import(id) = d else { return None };
        if id.alias == alias_sym {
            Some(id)
        } else {
            None
        }
    }) else {
        return Ok(Vec::new());
    };

    let ImportPath::Local { segments, sha: _ } = &import_decl.path else {
        return Ok(Vec::new());
    };

    let module_name = segments
        .iter()
        .map(|s| s.as_ref())
        .collect::<Vec<_>>()
        .join(".");

    let source = if let Some(source) = rex_util::stdlib_source(&module_name) {
        source.to_string()
    } else {
        let importer = uri_to_file_path(uri).ok_or_else(|| "not a file uri".to_string())?;
        let Some(base_dir) = importer.parent() else {
            return Ok(Vec::new());
        };
        let Some(module_path) = rex_util::resolve_local_import_path(base_dir, segments)
            .ok()
            .flatten()
            .and_then(|p| p.canonicalize().ok())
        else {
            return Ok(Vec::new());
        };
        fs::read_to_string(&module_path).map_err(|e| e.to_string())?
    };
    let (_tokens, module_program) =
        tokenize_and_parse(&source).map_err(|_| "parse error".to_string())?;

    let mut exports = BTreeSet::new();
    for decl in &module_program.decls {
        match decl {
            Decl::Fn(fd) if fd.is_pub => {
                exports.insert(fd.name.name.as_ref().to_string());
            }
            Decl::DeclareFn(df) if df.is_pub => {
                exports.insert(df.name.name.as_ref().to_string());
            }
            Decl::Type(td) if td.is_pub => {
                for variant in &td.variants {
                    exports.insert(variant.name.as_ref().to_string());
                }
            }
            _ => {}
        }
    }
    Ok(exports.into_iter().collect())
}

#[cfg(not(target_arch = "wasm32"))]
struct RexServer {
    client: Client,
    documents: RwLock<HashMap<Url, String>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl RexServer {
    fn new(client: Client) -> Self {
        Self {
            client,
            documents: RwLock::new(HashMap::new()),
        }
    }

    async fn publish_diagnostics(&self, uri: Url, text: &str) {
        let uri_for_job = uri.clone();
        let text_for_job = text.to_string();
        let diagnostics = match tokio::task::spawn_blocking(move || {
            diagnostics_from_text(&uri_for_job, &text_for_job)
        })
        .await
        {
            Ok(diags) => diags,
            Err(err) => {
                self.client
                    .log_message(
                        MessageType::ERROR,
                        format!("failed to compute diagnostics: {err}"),
                    )
                    .await;
                Vec::new()
            }
        };
        self.client
            .publish_diagnostics(uri, diagnostics, None)
            .await;
    }
}

#[cfg(not(target_arch = "wasm32"))]
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
                code_action_provider: Some(
                    tower_lsp::lsp_types::CodeActionProviderCapability::Options(
                        CodeActionOptions {
                            code_action_kinds: Some(vec![CodeActionKind::QUICKFIX]),
                            ..CodeActionOptions::default()
                        },
                    ),
                ),
                execute_command_provider: Some(ExecuteCommandOptions {
                    commands: vec![
                        CMD_EXPECTED_TYPE_AT.to_string(),
                        CMD_FUNCTIONS_PRODUCING_EXPECTED_TYPE_AT.to_string(),
                        CMD_FUNCTIONS_ACCEPTING_INFERRED_TYPE_AT.to_string(),
                        CMD_ADAPTERS_FROM_INFERRED_TO_EXPECTED_AT.to_string(),
                        CMD_FUNCTIONS_COMPATIBLE_WITH_IN_SCOPE_VALUES_AT.to_string(),
                        CMD_HOLES_EXPECTED_TYPES.to_string(),
                        CMD_SEMANTIC_LOOP_STEP.to_string(),
                        CMD_SEMANTIC_LOOP_APPLY_QUICK_FIX_AT.to_string(),
                        CMD_SEMANTIC_LOOP_APPLY_BEST_QUICK_FIXES_AT.to_string(),
                    ],
                    ..ExecuteCommandOptions::default()
                }),
                definition_provider: Some(OneOf::Left(true)),
                references_provider: Some(OneOf::Left(true)),
                rename_provider: Some(OneOf::Left(true)),
                document_symbol_provider: Some(OneOf::Left(true)),
                document_formatting_provider: Some(OneOf::Left(true)),
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
        clear_parse_cache(&uri);
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
            clear_parse_cache(&uri);
            self.publish_diagnostics(uri, &text).await;
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.documents.write().await.remove(&uri);
        clear_parse_cache(&uri);
        self.client.publish_diagnostics(uri, Vec::new(), None).await;
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let text = { self.documents.read().await.get(&uri).cloned() };

        let Some(text) = text else {
            return Ok(None);
        };

        let uri_for_job = uri.clone();
        let text_for_job = text.clone();
        let type_contents = match tokio::task::spawn_blocking(move || {
            hover_type_contents(&uri_for_job, &text_for_job, position)
        })
        .await
        {
            Ok(contents) => contents,
            Err(err) => {
                self.client
                    .log_message(MessageType::ERROR, format!("hover failed: {err}"))
                    .await;
                None
            }
        };

        let contents = type_contents.or_else(|| {
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

        let uri_for_job = uri.clone();
        let text_for_job = text;
        let items = match tokio::task::spawn_blocking(move || {
            completion_items(&uri_for_job, &text_for_job, position)
        })
        .await
        {
            Ok(items) => items,
            Err(err) => {
                self.client
                    .log_message(MessageType::ERROR, format!("completion failed: {err}"))
                    .await;
                Vec::new()
            }
        };
        Ok(Some(CompletionResponse::Array(items)))
    }

    async fn code_action(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        let uri = params.text_document.uri;
        let text = { self.documents.read().await.get(&uri).cloned() };
        let Some(text) = text else {
            return Ok(None);
        };

        let range = params.range;
        let diagnostics = params.context.diagnostics;
        let uri_for_job = uri.clone();
        let text_for_job = text;
        let actions = match tokio::task::spawn_blocking(move || {
            code_actions_for_source(&uri_for_job, &text_for_job, range, &diagnostics)
        })
        .await
        {
            Ok(actions) => actions,
            Err(err) => {
                self.client
                    .log_message(MessageType::ERROR, format!("code action failed: {err}"))
                    .await;
                Vec::new()
            }
        };

        Ok(Some(actions))
    }

    async fn execute_command(&self, params: ExecuteCommandParams) -> Result<Option<Value>> {
        let arguments = params.arguments;
        let command = params.command;
        if command == CMD_HOLES_EXPECTED_TYPES {
            let Some(uri) = command_uri(&arguments) else {
                return Ok(None);
            };
            let text = { self.documents.read().await.get(&uri).cloned() };
            let Some(text) = text else {
                return Ok(None);
            };
            return Ok(execute_query_command_for_document_without_position(
                &command, &uri, &text,
            ));
        }
        if command == CMD_SEMANTIC_LOOP_STEP {
            let Some((uri, position)) = command_uri_and_position(&arguments) else {
                return Ok(None);
            };
            let text = { self.documents.read().await.get(&uri).cloned() };
            let Some(text) = text else {
                return Ok(None);
            };
            return Ok(execute_semantic_loop_step(&uri, &text, position));
        }
        if command == CMD_SEMANTIC_LOOP_APPLY_QUICK_FIX_AT {
            let Some((uri, position, quick_fix_id)) = command_uri_position_and_id(&arguments)
            else {
                return Ok(None);
            };
            let text = { self.documents.read().await.get(&uri).cloned() };
            let Some(text) = text else {
                return Ok(None);
            };
            return Ok(execute_semantic_loop_apply_quick_fix(
                &uri,
                &text,
                position,
                &quick_fix_id,
            ));
        }
        if command == CMD_SEMANTIC_LOOP_APPLY_BEST_QUICK_FIXES_AT {
            let Some((uri, position, max_steps, strategy, dry_run)) =
                command_uri_position_max_steps_strategy_and_dry_run(&arguments)
            else {
                return Ok(None);
            };
            let text = { self.documents.read().await.get(&uri).cloned() };
            let Some(text) = text else {
                return Ok(None);
            };
            return Ok(execute_semantic_loop_apply_best_quick_fixes(
                &uri, &text, position, max_steps, strategy, dry_run,
            ));
        }

        let Some((uri, position)) = command_uri_and_position(&arguments) else {
            return Ok(None);
        };
        let text = { self.documents.read().await.get(&uri).cloned() };
        let Some(text) = text else {
            return Ok(None);
        };
        Ok(execute_query_command_for_document(
            &command, &uri, &text, position,
        ))
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

        let uri_for_job = uri.clone();
        let text_for_job = text;
        let response = match tokio::task::spawn_blocking(move || {
            goto_definition_response(&uri_for_job, &text_for_job, position)
        })
        .await
        {
            Ok(resp) => resp,
            Err(err) => {
                self.client
                    .log_message(MessageType::ERROR, format!("goto definition failed: {err}"))
                    .await;
                None
            }
        };
        Ok(response)
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let include_declaration = params.context.include_declaration;
        let text = { self.documents.read().await.get(&uri).cloned() };

        let Some(text) = text else {
            return Ok(None);
        };

        let uri_for_job = uri.clone();
        let text_for_job = text;
        let refs = match tokio::task::spawn_blocking(move || {
            references_for_source(&uri_for_job, &text_for_job, position, include_declaration)
        })
        .await
        {
            Ok(items) => items,
            Err(err) => {
                self.client
                    .log_message(MessageType::ERROR, format!("references failed: {err}"))
                    .await;
                Vec::new()
            }
        };
        Ok(Some(refs))
    }

    async fn rename(&self, params: RenameParams) -> Result<Option<WorkspaceEdit>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let new_name = params.new_name;
        let text = { self.documents.read().await.get(&uri).cloned() };

        let Some(text) = text else {
            return Ok(None);
        };

        let uri_for_job = uri.clone();
        let text_for_job = text;
        let edit = match tokio::task::spawn_blocking(move || {
            rename_for_source(&uri_for_job, &text_for_job, position, &new_name)
        })
        .await
        {
            Ok(edit) => edit,
            Err(err) => {
                self.client
                    .log_message(MessageType::ERROR, format!("rename failed: {err}"))
                    .await;
                None
            }
        };
        Ok(edit)
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        let uri = params.text_document.uri;
        let text = { self.documents.read().await.get(&uri).cloned() };
        let Some(text) = text else {
            return Ok(None);
        };
        let symbols = document_symbols_for_source(&uri, &text);
        Ok(Some(DocumentSymbolResponse::Nested(symbols)))
    }

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        let uri = params.text_document.uri;
        let text = { self.documents.read().await.get(&uri).cloned() };
        let Some(text) = text else {
            return Ok(None);
        };
        Ok(format_edits_for_source(&text))
    }
}

fn goto_definition_response(
    uri: &Url,
    text: &str,
    position: Position,
) -> Option<GotoDefinitionResponse> {
    // Parse on-demand. This keeps steady-state typing latency low; “go to
    // definition” is an explicit user action where a little work is fine.
    let Ok((tokens, program)) = tokenize_and_parse_cached(uri, text) else {
        return None;
    };

    let imported_projection = imported_projection_at_position(&tokens, position);

    let (ident, _token_span) = ident_token_at_position(&tokens, position)?;

    // If the cursor is on `alias.field` and `alias` is a local import, jump
    // to the exported declaration in the imported module.
    if let Some((alias, field)) = imported_projection
        && let Ok((_rewritten, _ts, imports, _diags)) = prepare_program_with_imports(uri, &program)
    {
        let alias_sym = intern(&alias);
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

fn range_to_span(range: Range) -> Span {
    Span::new(
        (range.start.line + 1) as usize,
        (range.start.character + 1) as usize,
        (range.end.line + 1) as usize,
        (range.end.character + 1) as usize,
    )
}

fn pattern_bindings_with_spans(pat: &Pattern, out: &mut Vec<(String, Span)>) {
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

fn collect_references_in_expr(
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
        Expr::Let(_, var, _ann, def, body) => {
            collect_references_in_expr(def, ident, target_span, uri, top_level_defs, scope, out);
            scope.push((var.name.to_string(), var.span));
            collect_references_in_expr(body, ident, target_span, uri, top_level_defs, scope, out);
            scope.pop();
        }
        Expr::LetRec(_, bindings, body) => {
            let base_len = scope.len();
            for (var, _ann, _def) in bindings {
                scope.push((var.name.to_string(), var.span));
            }
            for (_var, _ann, def) in bindings {
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

fn references_for_source(
    uri: &Url,
    text: &str,
    position: Position,
    include_declaration: bool,
) -> Vec<Location> {
    let Ok((tokens, program)) = tokenize_and_parse_cached(uri, text) else {
        return Vec::new();
    };
    let Some((ident, _token_span)) = ident_token_at_position(&tokens, position) else {
        return Vec::new();
    };

    let Some(def_response) = goto_definition_response(uri, text, position) else {
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
    let expr = program.expr_with_fns();
    collect_references_in_expr(
        expr.as_ref(),
        &ident,
        target_span,
        uri,
        &top_level_defs,
        &mut Vec::new(),
        &mut refs,
    );
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

fn rename_for_source(
    uri: &Url,
    text: &str,
    position: Position,
    new_name: &str,
) -> Option<WorkspaceEdit> {
    if !is_ident_like(new_name) {
        return None;
    }
    let refs = references_for_source(uri, text, position, true);
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

fn code_actions_for_source(
    uri: &Url,
    text: &str,
    request_range: Range,
    diagnostics: &[Diagnostic],
) -> Vec<CodeActionOrCommand> {
    let parsed = tokenize_and_parse_cached(uri, text)
        .ok()
        .map(|(_tokens, program)| program);
    let mut actions = Vec::new();

    // Hole fill is position-driven and should be available even when other diagnostics exist.
    actions.extend(code_actions_for_hole_fill(
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
            uri,
            text,
            parsed.as_ref(),
            request_range,
            diag,
        ));
    }

    actions
}

fn code_actions_for_hole_fill(
    uri: &Url,
    text: &str,
    program: Option<&Program>,
    request_range: Range,
) -> Vec<CodeActionOrCommand> {
    let Some(program) = program else {
        return Vec::new();
    };
    let mut hole_spans = Vec::new();
    collect_hole_spans(program.expr_with_fns().as_ref(), &mut hole_spans);
    let Some(hole_span) = hole_spans
        .into_iter()
        .find(|span| ranges_overlap(span_to_range(*span), request_range))
    else {
        return Vec::new();
    };
    let hole_range = span_to_range(hole_span);
    let pos = hole_range.start;
    let candidates = hole_fill_candidates_at_position(uri, text, pos);
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

fn code_actions_for_diagnostic(
    uri: &Url,
    text: &str,
    program: Option<&Program>,
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
        actions.extend(code_actions_for_hole_fill(uri, text, program, target_range));
    }

    if let Some(name) = unknown_var_name_from_message(&diagnostic.message) {
        if let Some(program) = program {
            let mut candidates: Vec<String> =
                values_in_scope_at_position(program, target_range.start)
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
        let newline = if diagnostic.range.start.line == diagnostic.range.end.line {
            " "
        } else {
            "\n"
        };
        actions.push(code_action_insert(
            "Add wildcard arm to match".to_string(),
            uri,
            diagnostic.range.end,
            format!("{newline}when _ -> null"),
            diagnostic.clone(),
        ));
    }

    if let Some(field) = field_not_definitely_available_from_message(&diagnostic.message)
        && let Some(program) = program
        && let Some(selected) = text_for_range(text, target_range)
    {
        let candidates = default_record_candidates_for_field(program, field);
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
            find_let_binding_for_def_range(program, target_range)
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

fn code_action_replace(
    title: String,
    uri: &Url,
    range: Range,
    new_text: String,
    diagnostic: Diagnostic,
) -> CodeActionOrCommand {
    code_action_with_edit(title, uri, TextEdit { range, new_text }, diagnostic)
}

fn code_action_insert(
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

fn code_action_with_edit(
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

fn text_for_range(text: &str, range: Range) -> Option<String> {
    let start = offset_at(text, range.start)?;
    let end = offset_at(text, range.end)?;
    (start <= end && end <= text.len()).then(|| text[start..end].to_string())
}

fn range_is_usable_for_text(text: &str, range: Range) -> bool {
    let Some(start) = offset_at(text, range.start) else {
        return false;
    };
    let Some(end) = offset_at(text, range.end) else {
        return false;
    };
    start <= end && end <= text.len()
}

fn ranges_overlap(a: Range, b: Range) -> bool {
    position_leq_lsp(a.start, b.end) && position_leq_lsp(b.start, a.end)
}

fn position_leq_lsp(left: Position, right: Position) -> bool {
    left.line < right.line || (left.line == right.line && left.character <= right.character)
}

fn range_is_empty(range: Range) -> bool {
    range.start.line == range.end.line && range.start.character == range.end.character
}

fn unknown_var_name_from_message(message: &str) -> Option<&str> {
    message.strip_prefix("unbound variable ").map(str::trim)
}

fn field_not_definitely_available_from_message(message: &str) -> Option<&str> {
    let rest = message.strip_prefix("field `")?;
    let (field, tail) = rest.split_once('`')?;
    tail.contains("is not definitely available on")
        .then_some(field)
}

fn default_record_candidates_for_field(program: &Program, field: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for decl in &program.decls {
        let Decl::Instance(inst) = decl else {
            continue;
        };
        if inst.class.as_ref() != "Default" {
            continue;
        }
        let TypeExpr::Name(_, ty_name) = &inst.head else {
            continue;
        };
        if !type_decl_has_record_field(program, ty_name.as_ref(), field) {
            continue;
        }
        let ty_name = ty_name.as_ref().to_string();
        if seen.insert(ty_name.clone()) {
            out.push(ty_name);
        }
    }
    out
}

fn type_decl_has_record_field(program: &Program, type_name: &str, field: &str) -> bool {
    program.decls.iter().any(|decl| {
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

fn replace_first_default_with_is(source: &str, ty_name: &str) -> Option<String> {
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

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn is_hole_name(name: &str) -> bool {
    name == "_" || name.starts_with('_')
}

fn is_list_scalar_unification_error(message: &str) -> bool {
    let Some(rest) = message.strip_prefix("types do not unify: ") else {
        return false;
    };
    let Some((left, right)) = rest.split_once(" vs ") else {
        return false;
    };
    list_inner_type(left.trim()).is_some_and(|inner| inner == right.trim())
        || list_inner_type(right.trim()).is_some_and(|inner| inner == left.trim())
}

fn list_inner_type(typ: &str) -> Option<&str> {
    if let Some(inner) = typ
        .strip_prefix("List<")
        .and_then(|rest| rest.strip_suffix('>'))
    {
        return Some(inner);
    }
    typ.strip_prefix("(List ")
        .and_then(|rest| rest.strip_suffix(')'))
}

fn is_function_value_unification_error(message: &str) -> bool {
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

fn looks_like_fun_type(typ: &str) -> bool {
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

fn split_fun_type(typ: &Type) -> (Vec<Type>, Type) {
    let mut args = Vec::new();
    let mut cur = typ.clone();
    while let TypeKind::Fun(arg, ret) = cur.as_ref() {
        args.push(arg.clone());
        cur = ret.clone();
    }
    (args, cur)
}

fn in_scope_value_types_at_position(
    uri: &Url,
    text: &str,
    position: Position,
) -> Vec<(String, Type)> {
    let Ok((_tokens, program)) = tokenize_and_parse_cached(uri, text) else {
        return Vec::new();
    };
    let Ok((program, mut ts, _imports, _import_diags)) =
        prepare_program_with_imports(uri, &program)
    else {
        return Vec::new();
    };
    if inject_program_decls(&mut ts, &program, None).is_err() {
        return Vec::new();
    }

    let expr = program.expr_with_fns();
    let Ok((typed, _preds, _ty)) = ts.infer_typed(expr.as_ref()) else {
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

        match (expr, &typed.kind) {
            (
                Expr::Let(_span, var, _ann, def, body),
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
                for ((name, _ann, _def), (_typed_name, typed_def)) in
                    bindings.iter().zip(typed_bindings.iter())
                {
                    scope.push((name.name.to_string(), typed_def.typ.clone()));
                }
                for ((_, _, def), (_, typed_def)) in bindings.iter().zip(typed_bindings.iter()) {
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

fn hole_fill_candidates_at_position(
    uri: &Url,
    text: &str,
    position: Position,
) -> Vec<(String, String)> {
    let Some(target_type) = expected_type_at_position_type(uri, text, position) else {
        return Vec::new();
    };
    let Ok((_tokens, program)) = tokenize_and_parse_cached(uri, text) else {
        return Vec::new();
    };
    let Ok((program, mut ts, _imports, _import_diags)) =
        prepare_program_with_imports(uri, &program)
    else {
        return Vec::new();
    };
    if inject_program_decls(&mut ts, &program, None).is_err() {
        return Vec::new();
    }
    let mut in_scope = in_scope_value_types_at_position(uri, text, position)
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

fn levenshtein_distance(left: &str, right: &str) -> usize {
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

#[allow(deprecated)]
fn symbol_for_decl(decl: &Decl) -> Option<DocumentSymbol> {
    match decl {
        Decl::Type(td) => Some(DocumentSymbol {
            name: td.name.to_string(),
            detail: Some("type".to_string()),
            kind: SymbolKind::ENUM,
            tags: None,
            deprecated: None,
            range: span_to_range(td.span),
            selection_range: span_to_range(td.span),
            children: Some(
                td.variants
                    .iter()
                    .map(|variant| DocumentSymbol {
                        name: variant.name.to_string(),
                        detail: Some("variant".to_string()),
                        kind: SymbolKind::ENUM_MEMBER,
                        tags: None,
                        deprecated: None,
                        range: span_to_range(td.span),
                        selection_range: span_to_range(td.span),
                        children: None,
                    })
                    .collect(),
            ),
        }),
        Decl::Fn(fd) => Some(DocumentSymbol {
            name: fd.name.name.to_string(),
            detail: Some("fn".to_string()),
            kind: SymbolKind::FUNCTION,
            tags: None,
            deprecated: None,
            range: span_to_range(fd.span),
            selection_range: span_to_range(fd.name.span),
            children: None,
        }),
        Decl::DeclareFn(df) => Some(DocumentSymbol {
            name: df.name.name.to_string(),
            detail: Some("declare fn".to_string()),
            kind: SymbolKind::FUNCTION,
            tags: None,
            deprecated: None,
            range: span_to_range(df.span),
            selection_range: span_to_range(df.name.span),
            children: None,
        }),
        Decl::Import(id) => Some(DocumentSymbol {
            name: id.alias.to_string(),
            detail: Some("import".to_string()),
            kind: SymbolKind::MODULE,
            tags: None,
            deprecated: None,
            range: span_to_range(id.span),
            selection_range: span_to_range(id.span),
            children: None,
        }),
        Decl::Class(cd) => Some(DocumentSymbol {
            name: cd.name.to_string(),
            detail: Some("class".to_string()),
            kind: SymbolKind::INTERFACE,
            tags: None,
            deprecated: None,
            range: span_to_range(cd.span),
            selection_range: span_to_range(cd.span),
            children: Some(
                cd.methods
                    .iter()
                    .map(|method| DocumentSymbol {
                        name: method.name.to_string(),
                        detail: Some("method".to_string()),
                        kind: SymbolKind::METHOD,
                        tags: None,
                        deprecated: None,
                        range: span_to_range(cd.span),
                        selection_range: span_to_range(cd.span),
                        children: None,
                    })
                    .collect(),
            ),
        }),
        Decl::Instance(id) => Some(DocumentSymbol {
            name: format!("instance {}", id.class),
            detail: Some("instance".to_string()),
            kind: SymbolKind::OBJECT,
            tags: None,
            deprecated: None,
            range: span_to_range(id.span),
            selection_range: span_to_range(id.span),
            children: Some(
                id.methods
                    .iter()
                    .map(|method| DocumentSymbol {
                        name: method.name.to_string(),
                        detail: Some("method".to_string()),
                        kind: SymbolKind::METHOD,
                        tags: None,
                        deprecated: None,
                        range: span_to_range(*method.body.span()),
                        selection_range: span_to_range(*method.body.span()),
                        children: None,
                    })
                    .collect(),
            ),
        }),
    }
}

fn document_symbols_for_source(uri: &Url, text: &str) -> Vec<DocumentSymbol> {
    let Ok((_tokens, program)) = tokenize_and_parse_cached(uri, text) else {
        return Vec::new();
    };
    program.decls.iter().filter_map(symbol_for_decl).collect()
}

fn full_document_range(text: &str) -> Range {
    let mut line = 0u32;
    let mut col = 0u32;
    for ch in text.chars() {
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    Range {
        start: Position {
            line: 0,
            character: 0,
        },
        end: Position {
            line,
            character: col,
        },
    }
}

fn format_source(text: &str) -> String {
    let mut out = String::new();
    let mut first = true;
    for line in text.lines() {
        if !first {
            out.push('\n');
        }
        first = false;
        out.push_str(line.trim_end());
    }
    if text.ends_with('\n') || !out.is_empty() {
        out.push('\n');
    }
    out
}

fn format_edits_for_source(text: &str) -> Option<Vec<TextEdit>> {
    let formatted = format_source(text);
    if formatted == text {
        return None;
    }
    Some(vec![TextEdit {
        range: full_document_range(text),
        new_text: formatted,
    }])
}

fn diagnostics_from_text(uri: &Url, text: &str) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    match tokenize_and_parse_cached(uri, text) {
        Ok((tokens, program)) => {
            push_comment_diagnostics(&tokens, &mut diagnostics);
            if diagnostics.len() < MAX_DIAGNOSTICS {
                push_type_diagnostics(uri, text, &program, &mut diagnostics);
            }
        }
        Err(TokenizeOrParseError::Lex(err)) => {
            let (span, message) = match err {
                LexicalError::UnexpectedToken(span) => (span, "Unexpected token".to_string()),
                LexicalError::InvalidLiteral {
                    kind,
                    text,
                    error,
                    span,
                } => (span, format!("invalid {kind} literal `{text}`: {error}")),
                LexicalError::Internal(msg) => (
                    Span::new(1, 1, 1, 1),
                    format!("internal lexer error: {msg}"),
                ),
            };
            diagnostics.push(diagnostic_for_span(span, message));
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

struct HoverType {
    span: Span,
    label: String,
    typ: String,
    overloads: Vec<String>,
}

fn hover_type_contents(uri: &Url, text: &str, position: Position) -> Option<HoverContents> {
    let (tokens, program) = tokenize_and_parse_cached(uri, text).ok()?;
    let (name, name_span, name_is_ident) = name_token_at_position(&tokens, position)?;
    let (program, mut ts, _imports, _import_diags) =
        prepare_program_with_imports(uri, &program).ok()?;

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
        let (typed, _preds, _) = ts.infer_typed(expr_with_fns.as_ref()).ok()?;
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

fn expected_type_at_position(uri: &Url, text: &str, position: Position) -> Option<String> {
    expected_type_at_position_type(uri, text, position).map(|ty| ty.to_string())
}

fn inferred_type_at_position(uri: &Url, text: &str, position: Position) -> Option<String> {
    inferred_type_at_position_type(uri, text, position).map(|ty| ty.to_string())
}

fn expected_type_at_position_type(uri: &Url, text: &str, position: Position) -> Option<Type> {
    let (_tokens, program) = tokenize_and_parse_cached(uri, text).ok()?;
    let (program, mut ts, _imports, _import_diags) =
        prepare_program_with_imports(uri, &program).ok()?;

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
        let (typed, _preds, _) = ts.infer_typed(expr_with_fns.as_ref()).ok()?;
        typed_root = typed;
        root_expr = expr_with_fns.as_ref();
    }

    expected_type_in_expr(root_expr, &typed_root, pos)
}

fn inferred_type_at_position_type(uri: &Url, text: &str, position: Position) -> Option<Type> {
    let (_tokens, program) = tokenize_and_parse_cached(uri, text).ok()?;
    let (program, mut ts, _imports, _import_diags) =
        prepare_program_with_imports(uri, &program).ok()?;

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
        let (typed, _preds, _) = ts.infer_typed(expr_with_fns.as_ref()).ok()?;
        typed_root = typed;
        root_expr = expr_with_fns.as_ref();
    }

    inferred_type_in_expr(root_expr, &typed_root, pos)
}

fn expected_type_in_expr(expr: &Expr, typed: &TypedExpr, pos: RexPosition) -> Option<Type> {
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

        match (expr, &typed.kind) {
            (
                Expr::Let(_span, _name, _ann, def, body),
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
                for ((_name, _ann, def), (_typed_name, typed_def)) in
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
                let bool_ty = Type::con("bool", 0);
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
                        TypeKind::Con(tc) if tc.name.as_ref() == "List" && tc.arity == 1 => {
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

fn inferred_type_in_expr(expr: &Expr, typed: &TypedExpr, pos: RexPosition) -> Option<Type> {
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

        match (expr, &typed.kind) {
            (
                Expr::Let(_, _, _, def, body),
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
                for ((_, _, def), (_, typed_def)) in bindings.iter().zip(typed_bindings.iter()) {
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

fn functions_producing_expected_type_at_position(
    uri: &Url,
    text: &str,
    position: Position,
) -> Vec<(String, String)> {
    let Some(target_type) = expected_type_at_position_type(uri, text, position) else {
        return Vec::new();
    };

    let Ok((_tokens, program)) = tokenize_and_parse_cached(uri, text) else {
        return Vec::new();
    };
    let Ok((program, mut ts, _imports, _import_diags)) =
        prepare_program_with_imports(uri, &program)
    else {
        return Vec::new();
    };
    if inject_program_decls(&mut ts, &program, None).is_err() {
        return Vec::new();
    }

    let values = semantic_candidate_values(&ts);

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

    out.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    out.dedup();
    if out.len() > MAX_SEMANTIC_CANDIDATES {
        out.truncate(MAX_SEMANTIC_CANDIDATES);
    }
    out
}

fn functions_accepting_inferred_type_at_position(
    uri: &Url,
    text: &str,
    position: Position,
) -> Vec<(String, String)> {
    let Some(source_type) = inferred_type_at_position_type(uri, text, position) else {
        return Vec::new();
    };

    let Ok((_tokens, program)) = tokenize_and_parse_cached(uri, text) else {
        return Vec::new();
    };
    let Ok((program, mut ts, _imports, _import_diags)) =
        prepare_program_with_imports(uri, &program)
    else {
        return Vec::new();
    };
    if inject_program_decls(&mut ts, &program, None).is_err() {
        return Vec::new();
    }

    let values = semantic_candidate_values(&ts);

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

    out.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    out.dedup();
    if out.len() > MAX_SEMANTIC_CANDIDATES {
        out.truncate(MAX_SEMANTIC_CANDIDATES);
    }
    out
}

fn adapters_from_inferred_to_expected_at_position(
    uri: &Url,
    text: &str,
    position: Position,
) -> Vec<(String, String)> {
    let Some(source_type) = inferred_type_at_position_type(uri, text, position) else {
        return Vec::new();
    };
    let Some(target_type) = expected_type_at_position_type(uri, text, position) else {
        return Vec::new();
    };

    let Ok((_tokens, program)) = tokenize_and_parse_cached(uri, text) else {
        return Vec::new();
    };
    let Ok((program, mut ts, _imports, _import_diags)) =
        prepare_program_with_imports(uri, &program)
    else {
        return Vec::new();
    };
    if inject_program_decls(&mut ts, &program, None).is_err() {
        return Vec::new();
    }

    let values = semantic_candidate_values(&ts);

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

    out.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    out.dedup();
    if out.len() > MAX_SEMANTIC_CANDIDATES {
        out.truncate(MAX_SEMANTIC_CANDIDATES);
    }
    out
}

fn functions_compatible_with_in_scope_values_at_position(
    uri: &Url,
    text: &str,
    position: Position,
) -> Vec<String> {
    let produced = functions_producing_expected_type_at_position(uri, text, position);
    let mut produced_by_name: HashMap<String, Vec<String>> = HashMap::new();
    for (name, typ) in produced {
        produced_by_name.entry(name).or_default().push(typ);
    }

    let mut out = Vec::new();
    for (name, replacement) in hole_fill_candidates_at_position(uri, text, position) {
        if replacement.contains('?') {
            continue;
        }
        if let Some(types) = produced_by_name.get(&name) {
            for typ in types {
                out.push(format!("{name} : {typ} => {replacement}"));
            }
        } else {
            out.push(format!("{name} => {replacement}"));
        }
    }
    out.sort();
    out.dedup();
    if out.len() > MAX_SEMANTIC_CANDIDATES {
        out.truncate(MAX_SEMANTIC_CANDIDATES);
    }
    out
}

fn execute_query_command_for_document(
    command: &str,
    uri: &Url,
    text: &str,
    position: Position,
) -> Option<Value> {
    match command {
        CMD_EXPECTED_TYPE_AT => Some(match expected_type_at_position(uri, text, position) {
            Some(typ) => json!({ "expectedType": typ }),
            None => Value::Null,
        }),
        CMD_FUNCTIONS_ACCEPTING_INFERRED_TYPE_AT => Some(json!({
            "inferredType": inferred_type_at_position(uri, text, position),
            "items": functions_accepting_inferred_type_at_position(uri, text, position)
                .into_iter()
                .map(|(name, typ)| format!("{name} : {typ}"))
                .collect::<Vec<_>>()
        })),
        CMD_ADAPTERS_FROM_INFERRED_TO_EXPECTED_AT => Some(json!({
            "inferredType": inferred_type_at_position(uri, text, position),
            "expectedType": expected_type_at_position(uri, text, position),
            "items": adapters_from_inferred_to_expected_at_position(uri, text, position)
                .into_iter()
                .map(|(name, typ)| format!("{name} : {typ}"))
                .collect::<Vec<_>>()
        })),
        CMD_FUNCTIONS_COMPATIBLE_WITH_IN_SCOPE_VALUES_AT => Some(json!({
            "items": functions_compatible_with_in_scope_values_at_position(uri, text, position)
        })),
        CMD_FUNCTIONS_PRODUCING_EXPECTED_TYPE_AT => {
            let items = functions_producing_expected_type_at_position(uri, text, position)
                .into_iter()
                .map(|(name, typ)| format!("{name} : {typ}"))
                .collect::<Vec<_>>();
            Some(json!({ "items": items }))
        }
        _ => None,
    }
}

fn execute_query_command_for_document_without_position(
    command: &str,
    uri: &Url,
    text: &str,
) -> Option<Value> {
    match command {
        CMD_HOLES_EXPECTED_TYPES => Some(json!({
            "holes": hole_expected_types_for_document(uri, text)
        })),
        _ => None,
    }
}

fn workspace_edit_fingerprint(edit: &WorkspaceEdit) -> String {
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
    sha256_hex(payload.as_bytes())
}

fn semantic_quick_fixes_for_range(
    uri: &Url,
    text: &str,
    cursor_range: Range,
    diagnostics: &[Diagnostic],
) -> Vec<Value> {
    let mut out = code_actions_for_source(uri, text, cursor_range, diagnostics)
        .into_iter()
        .filter_map(|action| match action {
            CodeActionOrCommand::CodeAction(action) => Some(action),
            CodeActionOrCommand::Command(_) => None,
        })
        .map(|action| {
            let kind = action
                .kind
                .and_then(|k| to_value(k).ok())
                .and_then(|v| v.as_str().map(str::to_string));
            let edit = action.edit.unwrap_or(WorkspaceEdit {
                changes: None,
                document_changes: None,
                change_annotations: None,
            });
            let fingerprint = workspace_edit_fingerprint(&edit);
            json!({
                "id": format!("qf-{}", &fingerprint[..16]),
                "title": action.title,
                "kind": kind,
                "edit": to_value(edit).unwrap_or(Value::Null),
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

fn execute_semantic_loop_step(uri: &Url, text: &str, position: Position) -> Option<Value> {
    let expected_type = expected_type_at_position(uri, text, position)
        .or_else(|| expected_type_from_syntax_context(uri, text, position));
    let inferred_type = inferred_type_at_position(uri, text, position);

    let mut in_scope_values = in_scope_value_types_at_position(uri, text, position)
        .into_iter()
        .filter(|(name, _)| is_ident_like(name))
        .map(|(name, typ)| format!("{name} : {typ}"))
        .collect::<Vec<_>>();
    in_scope_values.sort();
    in_scope_values.dedup();
    if in_scope_values.len() > MAX_SEMANTIC_IN_SCOPE_VALUES {
        in_scope_values.truncate(MAX_SEMANTIC_IN_SCOPE_VALUES);
    }

    let function_candidates = functions_producing_expected_type_at_position(uri, text, position)
        .into_iter()
        .map(|(name, typ)| format!("{name} : {typ}"))
        .collect::<Vec<_>>();

    let hole_fill_candidates = hole_fill_candidates_at_position(uri, text, position)
        .into_iter()
        .map(|(name, replacement)| json!({ "name": name, "replacement": replacement }))
        .collect::<Vec<_>>();
    let functions_accepting_inferred_type =
        functions_accepting_inferred_type_at_position(uri, text, position)
            .into_iter()
            .map(|(name, typ)| format!("{name} : {typ}"))
            .collect::<Vec<_>>();
    let adapters_from_inferred_to_expected =
        adapters_from_inferred_to_expected_at_position(uri, text, position)
            .into_iter()
            .map(|(name, typ)| format!("{name} : {typ}"))
            .collect::<Vec<_>>();
    let compatible_with_in_scope_values =
        functions_compatible_with_in_scope_values_at_position(uri, text, position);

    let cursor_range = Range {
        start: position,
        end: position,
    };
    let mut local_diagnostics: Vec<Diagnostic> = diagnostics_from_text(uri, text)
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

    let quick_fixes = semantic_quick_fixes_for_range(uri, text, cursor_range, &local_diagnostics);
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
        "holes": hole_expected_types_for_document(uri, text),
    }))
}

fn execute_semantic_loop_apply_quick_fix(
    uri: &Url,
    text: &str,
    position: Position,
    quick_fix_id: &str,
) -> Option<Value> {
    let cursor_range = Range {
        start: position,
        end: position,
    };
    let local_diagnostics: Vec<Diagnostic> = diagnostics_from_text(uri, text)
        .into_iter()
        .filter(|diag| ranges_overlap(diag.range, cursor_range))
        .collect();
    let quick_fixes = semantic_quick_fixes_for_range(uri, text, cursor_range, &local_diagnostics);
    let quick_fix = quick_fixes.into_iter().find(|item| {
        item.get("id")
            .and_then(Value::as_str)
            .is_some_and(|id| id == quick_fix_id)
    });

    Some(match quick_fix {
        Some(quick_fix) => json!({ "quickFix": quick_fix }),
        None => Value::Null,
    })
}

fn quick_fix_priority(strategy: BulkQuickFixStrategy, title: &str) -> usize {
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

fn best_quick_fix_from_candidates(
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

fn apply_workspace_edit_to_text(uri: &Url, text: &str, edit: &WorkspaceEdit) -> Option<String> {
    let changes = edit.changes.as_ref()?;
    let edits = changes.get(uri)?.clone();
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

fn text_state_hash(text: &str) -> String {
    sha256_hex(text.as_bytes())
}

fn next_no_improvement_streak(streak: usize, diagnostics_delta: i64) -> usize {
    if diagnostics_delta > 0 { 0 } else { streak + 1 }
}

fn execute_semantic_loop_apply_best_quick_fixes(
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
        let local_diagnostics: Vec<Diagnostic> = diagnostics_from_text(uri, &current_text)
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
        let quick_fixes =
            semantic_quick_fixes_for_range(uri, &current_text, cursor_range, &local_diagnostics);
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
        let diagnostics_after_step: Vec<Value> = diagnostics_from_text(uri, &next_text)
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

    let diagnostics_after: Vec<Value> = diagnostics_from_text(uri, &current_text)
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

fn hole_expected_types_for_document(uri: &Url, text: &str) -> Vec<Value> {
    let mut holes = Vec::new();

    // First-class holes: parse `?` nodes directly.
    if let Ok((_tokens, program)) = tokenize_and_parse_cached(uri, text) {
        let mut spans = Vec::new();
        collect_hole_spans(program.expr_with_fns().as_ref(), &mut spans);
        for span in spans {
            let pos = span_to_range(span).start;
            if let Some(expected_type) = expected_type_at_position(uri, text, pos)
                .or_else(|| expected_type_from_syntax_context(uri, text, pos))
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
    let diagnostics = diagnostics_from_text(uri, text);
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
        if let Some(expected_type) = expected_type_at_position(uri, text, pos)
            .or_else(|| expected_type_from_syntax_context(uri, text, pos))
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

fn collect_hole_spans(expr: &Expr, out: &mut Vec<Span>) {
    match expr {
        Expr::Hole(span) => out.push(*span),
        Expr::App(_, f, x) => {
            collect_hole_spans(f, out);
            collect_hole_spans(x, out);
        }
        Expr::Project(_, base, _) => collect_hole_spans(base, out),
        Expr::Lam(_, _scope, _param, _ann, _constraints, body) => collect_hole_spans(body, out),
        Expr::Let(_, _var, _ann, def, body) => {
            collect_hole_spans(def, out);
            collect_hole_spans(body, out);
        }
        Expr::LetRec(_, bindings, body) => {
            for (_var, _ann, def) in bindings {
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
        | Expr::String(..)
        | Expr::Uuid(..)
        | Expr::DateTime(..) => {}
    }
}

fn expected_type_from_syntax_context(uri: &Url, text: &str, position: Position) -> Option<String> {
    let (_tokens, program) = tokenize_and_parse_cached(uri, text).ok()?;
    let pos = lsp_to_rex_position(position);

    fn visit(expr: &Expr, pos: RexPosition) -> Option<String> {
        if !position_in_span(pos, *expr.span()) {
            return None;
        }
        match expr {
            Expr::Let(_span, _name, ann, def, body) => {
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
                    return Some("bool".to_string());
                }
                visit(cond.as_ref(), pos)
                    .or_else(|| visit(then_expr.as_ref(), pos))
                    .or_else(|| visit(else_expr.as_ref(), pos))
            }
            Expr::App(_span, f, x) => visit(f.as_ref(), pos).or_else(|| visit(x.as_ref(), pos)),
            Expr::Project(_span, base, _field) => visit(base.as_ref(), pos),
            Expr::Lam(_span, _scope, _param, _ann, _constraints, body) => visit(body.as_ref(), pos),
            Expr::LetRec(_span, bindings, body) => {
                for (_name, _ann, def) in bindings {
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
            | Expr::String(..)
            | Expr::Uuid(..)
            | Expr::DateTime(..)
            | Expr::Hole(..) => None,
        }
    }

    visit(program.expr_with_fns().as_ref(), pos)
}

fn command_uri_and_position(arguments: &[Value]) -> Option<(Url, Position)> {
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

fn command_uri(arguments: &[Value]) -> Option<Url> {
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

fn command_uri_position_and_id(arguments: &[Value]) -> Option<(Url, Position, String)> {
    if arguments.len() >= 4 {
        let uri = arguments.first()?.as_str()?;
        let line = arguments.get(1)?.as_u64()? as u32;
        let character = arguments.get(2)?.as_u64()? as u32;
        let id = arguments.get(3)?.as_str()?.to_string();
        let uri = Url::parse(uri).ok()?;
        return Some((uri, Position { line, character }, id));
    }

    let obj = arguments.first()?.as_object()?;
    let uri = obj.get("uri")?.as_str()?;
    let line = obj.get("line")?.as_u64()? as u32;
    let character = obj.get("character")?.as_u64()? as u32;
    let id = obj.get("id")?.as_str()?.to_string();
    let uri = Url::parse(uri).ok()?;
    Some((uri, Position { line, character }, id))
}

fn command_uri_position_max_steps_strategy_and_dry_run(
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
            ) = (&expr, &typed.kind)
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
        match (expr, &typed.kind) {
            (
                Expr::Let(_span, binding, _ann, def, body),
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
                for ((binding, _ann, def), (_name, typed_def)) in
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
            (Expr::Var(v), TypedExprKind::Var { overloads, .. }) => {
                if span_contains_pos(v.span, ctx.pos) {
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

fn push_type_diagnostics(
    uri: &Url,
    text: &str,
    program: &Program,
    diagnostics: &mut Vec<Diagnostic>,
) {
    // Type inference is meaningfully more expensive than lex/parse, and we run
    // diagnostics on every full-text change. Keep the cost model explicit.
    const MAX_TYPECHECK_BYTES: usize = 256 * 1024;
    if text.len() > MAX_TYPECHECK_BYTES {
        return;
    }

    let (program, mut ts, _imports, import_diags) = match prepare_program_with_imports(uri, program)
    {
        Ok(v) => v,
        Err(err) => {
            diagnostics.push(diagnostic_for_span(primary_program_span(program), err));
            return;
        }
    };
    diagnostics.extend(import_diags);
    if diagnostics.len() >= MAX_DIAGNOSTICS {
        diagnostics.truncate(MAX_DIAGNOSTICS);
        return;
    }

    let (instances, _prepared_target) = match inject_program_decls(&mut ts, &program, None) {
        Ok(v) => v,
        Err(err) => {
            push_ts_error(
                err,
                diagnostics,
                None,
                Some(&ts),
                Some(primary_program_span(&program)),
            );
            return;
        }
    };

    // Typecheck instance method bodies too, so errors inside the instance show
    // up as diagnostics.
    for (decl_idx, prepared) in instances {
        if diagnostics.len() >= MAX_DIAGNOSTICS {
            break;
        }
        let Decl::Instance(inst_decl) = &program.decls[decl_idx] else {
            continue;
        };
        for method in &inst_decl.methods {
            if let Err(err) = ts.typecheck_instance_method(&prepared, method) {
                push_ts_error(
                    err,
                    diagnostics,
                    Some(method.body.as_ref()),
                    Some(&ts),
                    None,
                );
                if diagnostics.len() >= MAX_DIAGNOSTICS {
                    break;
                }
            }
        }
    }

    if let Err(err) = ts.infer(program.expr.as_ref()) {
        let before = diagnostics.len();
        push_ts_error(
            err,
            diagnostics,
            Some(program.expr.as_ref()),
            Some(&ts),
            None,
        );
        if let Some(primary) = diagnostics.get(before).cloned() {
            push_additional_default_record_update_ambiguity_diagnostics(
                program.expr.as_ref(),
                &primary.message,
                diagnostics,
            );
        }
        return;
    }

    push_hole_diagnostics(&program, diagnostics);
}

fn primary_program_span(program: &Program) -> Span {
    match program.decls.first() {
        Some(Decl::Type(d)) => d.span,
        Some(Decl::Fn(d)) => d.span,
        Some(Decl::DeclareFn(d)) => d.span,
        Some(Decl::Import(d)) => d.span,
        Some(Decl::Class(d)) => d.span,
        Some(Decl::Instance(d)) => d.span,
        None => *program.expr.span(),
    }
}

fn push_hole_diagnostics(program: &Program, diagnostics: &mut Vec<Diagnostic>) {
    let mut spans = Vec::new();
    collect_hole_spans(program.expr_with_fns().as_ref(), &mut spans);
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
            source: Some("rex-ts".to_string()),
            ..Diagnostic::default()
        });
    }
}

fn unknown_var_name(err: &TsTypeError) -> Option<Symbol> {
    match err {
        TsTypeError::UnknownVar(name) => Some(name.clone()),
        TsTypeError::Spanned { error, .. } => unknown_var_name(error),
        _ => None,
    }
}

fn field_not_definitely_available_tail(message: &str) -> Option<(&str, &str)> {
    let rest = message.strip_prefix("field `")?;
    let (field, tail) = rest.split_once('`')?;
    tail.contains("is not definitely available on")
        .then_some((field, tail))
}

fn push_additional_default_record_update_ambiguity_diagnostics(
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
            source: Some("rex-ts".to_string()),
            ..Diagnostic::default()
        });
    }
}

fn collect_default_record_updates(expr: &Expr, out: &mut Vec<(Span, Vec<String>)>) {
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

fn find_let_binding_for_def_range(program: &Program, target: Range) -> Option<(String, Position)> {
    find_let_binding_for_def_range_in_expr(program.expr_with_fns().as_ref(), target)
}

fn find_let_binding_for_def_range_in_expr(
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

fn collect_unbound_var_spans(
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

fn push_ts_error(
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

    if let (Some(target), Some(expr), Some(ts)) = (unknown_target, expr, ts)
        && ts.env.lookup(&target).is_none()
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
                    source: Some("rex-ts".to_string()),
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

fn completion_items(uri: &Url, text: &str, position: Position) -> Vec<CompletionItem> {
    let field_mode = is_field_completion(text, position);
    let base_ident = if field_mode {
        field_base_ident(text, position)
    } else {
        None
    };
    if let Ok((_tokens, program)) = tokenize_and_parse_cached(uri, text) {
        return completion_items_from_program(
            &program,
            position,
            field_mode,
            base_ident.as_deref(),
            uri,
        );
    }

    completion_items_fallback(text, base_ident.as_deref(), field_mode)
}

fn completion_items_from_program(
    program: &Program,
    position: Position,
    field_mode: bool,
    base_ident: Option<&str>,
    uri: &Url,
) -> Vec<CompletionItem> {
    if field_mode {
        if let Some(base_ident) = base_ident
            && let Ok(exports) = completion_exports_for_module_alias(uri, program, base_ident)
            && !exports.is_empty()
        {
            return exports
                .into_iter()
                .map(|label| completion_item(label, CompletionItemKind::FIELD))
                .collect();
        }
        if let Some(fields) = field_completion_for_position(program, position, base_ident) {
            return fields
                .into_iter()
                .map(|label| completion_item(label, CompletionItemKind::FIELD))
                .collect();
        }
        return Vec::new();
    }

    let mut value_kinds = values_in_scope_at_position(program, position);
    let pos = lsp_to_rex_position(position);
    for decl in &program.decls {
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
        Expr::LetRec(_span, bindings, body) => {
            let base_len = scope.len();
            scope.extend(bindings.iter().map(|(var, _ann, def)| {
                let kind = matches!(def.as_ref(), Expr::Lam(..))
                    .then_some(CompletionItemKind::FUNCTION)
                    .unwrap_or(CompletionItemKind::VARIABLE);
                (var.name.to_string(), kind)
            }));

            for (_, _, def) in bindings {
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
    if let Some(base) = project_base_at_position(&expr, pos)
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
        Expr::LetRec(_, bindings, body) => {
            let mut env_with = env.clone();
            for (var, ann, def) in bindings {
                let fields = binding_fields(ann.as_ref(), def, type_fields).unwrap_or_default();
                env_with.insert(var.name.to_string(), fields);
            }
            for (_, _, def) in bindings {
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

fn binding_fields(
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

fn project_base_at_position(expr: &Expr, position: RexPosition) -> Option<&Expr> {
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
        Expr::LetRec(_, bindings, body) => {
            for (_, _, def) in bindings {
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

fn fallback_field_map(tokens: &Tokens) -> HashMap<String, BTreeSet<String>> {
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

fn imported_projection_at_position(
    tokens: &Tokens,
    position: Position,
) -> Option<(String, String)> {
    fn is_trivia(token: &Token) -> bool {
        matches!(
            token,
            Token::Whitespace(..) | Token::CommentL(..) | Token::CommentR(..)
        )
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
                    let token_span = *token.span();
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
        Expr::LetRec(_span, rec_bindings, body) => {
            for (var, _ann, _def) in rec_bindings {
                if position_in_span(position, var.span) && var.name.as_ref() == ident {
                    return Some(var.span);
                }
            }

            let base_len = bindings.len();
            for (var, _ann, _def) in rec_bindings {
                bindings.push((var.name.to_string(), var.span));
            }

            for (_var, _ann, def) in rec_bindings {
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

fn in_memory_doc_uri() -> Url {
    match Url::parse("inmemory:///docs.rex") {
        Ok(url) => url,
        Err(_) => panic!("static in-memory URI must parse"),
    }
}

pub fn diagnostics_for_source(source: &str) -> Vec<Diagnostic> {
    let uri = in_memory_doc_uri();
    clear_parse_cache(&uri);
    diagnostics_from_text(&uri, source)
}

pub fn completion_for_source(source: &str, line: u32, character: u32) -> Vec<CompletionItem> {
    let uri = in_memory_doc_uri();
    clear_parse_cache(&uri);
    completion_items(&uri, source, Position { line, character })
}

pub fn hover_for_source(source: &str, line: u32, character: u32) -> Option<Hover> {
    let uri = in_memory_doc_uri();
    clear_parse_cache(&uri);
    let position = Position { line, character };
    let contents = hover_type_contents(&uri, source, position).or_else(|| {
        let word = word_at_position(source, position)?;
        hover_contents(&word)
    })?;
    Some(Hover {
        contents,
        range: None,
    })
}

pub fn expected_type_for_source_public(source: &str, line: u32, character: u32) -> Option<String> {
    let uri = in_memory_doc_uri();
    clear_parse_cache(&uri);
    expected_type_at_position(&uri, source, Position { line, character })
}

pub fn functions_producing_expected_type_for_source_public(
    source: &str,
    line: u32,
    character: u32,
) -> Vec<String> {
    let uri = in_memory_doc_uri();
    clear_parse_cache(&uri);
    functions_producing_expected_type_at_position(&uri, source, Position { line, character })
        .into_iter()
        .map(|(name, typ)| format!("{name} : {typ}"))
        .collect()
}

pub fn references_for_source_public(
    source: &str,
    line: u32,
    character: u32,
    include_declaration: bool,
) -> Vec<Location> {
    let uri = in_memory_doc_uri();
    clear_parse_cache(&uri);
    references_for_source(
        &uri,
        source,
        Position { line, character },
        include_declaration,
    )
}

pub fn rename_for_source_public(
    source: &str,
    line: u32,
    character: u32,
    new_name: &str,
) -> Option<WorkspaceEdit> {
    let uri = in_memory_doc_uri();
    clear_parse_cache(&uri);
    rename_for_source(&uri, source, Position { line, character }, new_name)
}

pub fn document_symbols_for_source_public(source: &str) -> Vec<DocumentSymbol> {
    let uri = in_memory_doc_uri();
    clear_parse_cache(&uri);
    document_symbols_for_source(&uri, source)
}

pub fn format_for_source_public(source: &str) -> Option<Vec<TextEdit>> {
    format_edits_for_source(source)
}

pub fn code_actions_for_source_public(
    source: &str,
    line: u32,
    character: u32,
) -> Vec<CodeActionOrCommand> {
    let uri = in_memory_doc_uri();
    clear_parse_cache(&uri);
    let position = Position { line, character };
    let range = Range {
        start: position,
        end: position,
    };
    let diagnostics: Vec<Diagnostic> = diagnostics_from_text(&uri, source)
        .into_iter()
        .filter(|diag| {
            range_contains_position(diag.range, position)
                || range_touches_position(diag.range, position)
        })
        .collect();
    code_actions_for_source(&uri, source, range, &diagnostics)
}

pub fn goto_definition_for_source(source: &str, line: u32, character: u32) -> Option<Location> {
    let uri = in_memory_doc_uri();
    clear_parse_cache(&uri);
    let pos = Position { line, character };
    let response = goto_definition_response(&uri, source, pos)?;
    match response {
        GotoDefinitionResponse::Scalar(location) => Some(location),
        GotoDefinitionResponse::Array(locations) => locations.into_iter().next(),
        GotoDefinitionResponse::Link(links) => links.into_iter().next().map(|link| Location {
            uri: link.target_uri,
            range: link.target_range,
        }),
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn run_stdio() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(RexServer::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use rex::{Engine, GasMeter, Parser, Token};
    use rex_engine::{ValueDisplayOptions, pointer_display_with};
    use serde_json::Map;

    fn expect_object(value: &Value) -> &Map<String, Value> {
        value.as_object().expect("object")
    }

    fn expect_array_field<'a>(obj: &'a Map<String, Value>, key: &str) -> &'a Vec<Value> {
        obj.get(key)
            .unwrap_or_else(|| panic!("missing `{key}`"))
            .as_array()
            .unwrap_or_else(|| panic!("`{key}` should be array"))
    }

    fn expect_string_field<'a>(obj: &'a Map<String, Value>, key: &str) -> &'a str {
        obj.get(key)
            .unwrap_or_else(|| panic!("missing `{key}`"))
            .as_str()
            .unwrap_or_else(|| panic!("`{key}` should be string"))
    }

    async fn eval_source_to_display(code: &str) -> (String, String) {
        let tokens = Token::tokenize(code).expect("tokenize source");
        let mut parser = Parser::new(tokens);
        let program = parser
            .parse_program(&mut GasMeter::default())
            .expect("parse source");
        let mut engine = Engine::with_prelude(()).expect("build engine");
        engine.inject_decls(&program.decls).expect("inject decls");
        let (ptr, ty) = engine
            .eval(program.expr.as_ref(), &mut GasMeter::default())
            .await
            .expect("evaluate source");
        let display = pointer_display_with(
            &engine.heap,
            &ptr,
            ValueDisplayOptions {
                include_numeric_suffixes: true,
                ..ValueDisplayOptions::default()
            },
        )
        .expect("display value");
        (display, ty.to_string())
    }

    #[test]
    fn stdlib_imports_typecheck_for_non_file_uri() {
        let uri = Url::parse("untitled:Test.rex").expect("uri");
        let text = r#"
import std.io
import std.process

let _ = io.debug "hi" in
let p = process.spawn { cmd = "sh", args = ["-c"] } in
process.wait p
"#;

        let diags = diagnostics_from_text(&uri, text);
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
    }

    #[test]
    fn reports_all_unknown_var_usages() {
        let text = r#"
let
  f = \x -> missing + x
in
  missing + (f missing)
"#;

        let diags = diagnostics_for_source(text);
        let missing_diags = diags
            .iter()
            .filter(|d| d.message.contains("unbound variable") && d.message.contains("missing"))
            .count();
        assert_eq!(missing_diags, 3, "diagnostics: {diags:#?}");
    }

    #[test]
    fn diagnostics_report_typed_hole_error() {
        let text = "let y : i32 = ? in y";
        let diags = diagnostics_for_source(text);
        assert!(
            diags.iter().any(|d| d
                .message
                .contains("typed hole `?` must be filled before evaluation")),
            "diagnostics: {diags:#?}"
        );
    }

    #[test]
    fn diagnostics_report_both_default_record_update_ambiguities() {
        let text = r#"
type A = A { x: i32, y: i32 }
type B = B { x: i32, y: i32 }

instance Default A
    default = A { x = 1, y = 2 }

instance Default B
    default = B { x = 10, y = 20 }

let
    a = { default with { x = 9 } },
    b = { default with { y = 8 } }
in
    (a, b)
"#;
        let diags = diagnostics_for_source(text);
        let field_diags: Vec<&Diagnostic> = diags
            .iter()
            .filter(|d| d.message.contains("is not definitely available on"))
            .collect();
        assert_eq!(field_diags.len(), 2, "diagnostics: {diags:#?}");
        assert!(
            field_diags.iter().any(|d| d.message.contains("field `x`")),
            "diagnostics: {diags:#?}"
        );
        assert!(
            field_diags.iter().any(|d| d.message.contains("field `y`")),
            "diagnostics: {diags:#?}"
        );
    }

    #[tokio::test]
    async fn e2e_ambiguous_default_record_updates_two_quick_fix_styles_then_eval() {
        let text = r#"type A = A { x: i32, y: i32 }
type B = B { x: i32, y: i32 }

instance Default A
    default = A { x = 1, y = 2 }

instance Default B
    default = B { x = 10, y = 20 }

let
    a = { default with { x = 9 } },
    b = { default with { y = 8 } }
in
    (a, b)
"#;
        let uri = in_memory_doc_uri();
        clear_parse_cache(&uri);

        let mut field_diags: Vec<Diagnostic> = diagnostics_from_text(&uri, text)
            .into_iter()
            .filter(|diag| diag.message.contains("is not definitely available on"))
            .collect();
        field_diags.sort_by_key(|diag| {
            (
                diag.range.start.line,
                diag.range.start.character,
                diag.range.end.line,
                diag.range.end.character,
                diag.message.clone(),
            )
        });
        assert_eq!(field_diags.len(), 2, "diagnostics: {field_diags:#?}");
        assert_eq!(
            field_diags[0].message,
            "field `x` is not definitely available on 'a"
        );
        assert_eq!(
            field_diags[0].range,
            Range {
                start: Position {
                    line: 10,
                    character: 8,
                },
                end: Position {
                    line: 10,
                    character: 34,
                },
            }
        );
        assert_eq!(
            field_diags[1].message,
            "field `y` is not definitely available on 'a"
        );
        assert_eq!(
            field_diags[1].range,
            Range {
                start: Position {
                    line: 11,
                    character: 8,
                },
                end: Position {
                    line: 11,
                    character: 34,
                },
            }
        );

        let step_a = execute_semantic_loop_step(
            &uri,
            text,
            Position {
                line: 10,
                character: 25,
            },
        )
        .expect("step for a");
        let quick_fix_a = step_a
            .get("quickFixes")
            .and_then(Value::as_array)
            .and_then(|items| {
                items.iter().find(|item| {
                    item.get("title").and_then(Value::as_str)
                        == Some("Disambiguate `default` as `A`")
                })
            })
            .cloned()
            .expect("quick fix for a using `is`");
        let edit_a: WorkspaceEdit =
            serde_json::from_value(quick_fix_a.get("edit").cloned().expect("edit for a"))
                .expect("workspace edit for a");
        let after_a = apply_workspace_edit_to_text(&uri, text, &edit_a).expect("apply edit for a");

        let step_b = execute_semantic_loop_step(
            &uri,
            &after_a,
            Position {
                line: 11,
                character: 25,
            },
        )
        .expect("step for b");
        let quick_fix_b = step_b
            .get("quickFixes")
            .and_then(Value::as_array)
            .and_then(|items| {
                items.iter().find(|item| {
                    item.get("title").and_then(Value::as_str) == Some("Annotate `b` as `B`")
                })
            })
            .cloned()
            .expect("quick fix for b using let annotation");
        let edit_b: WorkspaceEdit =
            serde_json::from_value(quick_fix_b.get("edit").cloned().expect("edit for b"))
                .expect("workspace edit for b");
        let after_b =
            apply_workspace_edit_to_text(&uri, &after_a, &edit_b).expect("apply edit for b");

        let diagnostics_after = diagnostics_from_text(&uri, &after_b);
        assert!(
            diagnostics_after.is_empty(),
            "unexpected diagnostics after fixes: {diagnostics_after:#?}\nupdated=\n{after_b}"
        );

        let (value, ty) = eval_source_to_display(&after_b).await;
        assert_eq!(value, "(A {x = 9i32, y = 2i32}, B {x = 10i32, y = 8i32})");
        assert_eq!(ty, "(A, B)");
    }

    #[test]
    fn diagnostics_for_decl_type_errors_are_not_whole_document() {
        let text = r#"
fn parse_ph : string -> Result string f64 = \raw ->
  if raw == "7.3" then Ok 7.3 else Err "bad reading"
"#;
        let uri = in_memory_doc_uri();
        clear_parse_cache(&uri);
        let diags = diagnostics_from_text(&uri, text);
        let full = full_document_range(text);
        let unification = diags
            .iter()
            .find(|d| d.message.contains("types do not unify"))
            .expect("unification diagnostic");
        assert_ne!(
            unification.range, full,
            "diagnostic unexpectedly spans whole document: {unification:#?}"
        );
    }

    #[test]
    fn references_find_all_usages() {
        let text = r#"
let
  x = 1
in
  x + x
"#;
        let refs = references_for_source_public(text, 4, 2, true);
        assert_eq!(refs.len(), 3, "refs: {refs:#?}");
    }

    #[test]
    fn rename_returns_workspace_edit() {
        let text = r#"
let
  x = 1
in
  x + x
"#;
        let edit = rename_for_source_public(text, 4, 2, "value").expect("rename edit");
        let changes = edit.changes.expect("changes map");
        let uri = Url::parse("inmemory:///docs.rex").expect("uri");
        let edits = changes.get(&uri).expect("uri edits");
        assert_eq!(edits.len(), 3, "edits: {edits:#?}");
    }

    #[test]
    fn document_symbols_returns_top_level_items() {
        let text = r#"
type T = A | B
fn f : i32 -> i32 = \x -> x + 1
let x = 0 in f x
"#;
        let symbols = document_symbols_for_source_public(text);
        assert!(
            symbols.iter().any(|s| s.name == "T"),
            "symbols: {symbols:#?}"
        );
        assert!(
            symbols.iter().any(|s| s.name == "f"),
            "symbols: {symbols:#?}"
        );
    }

    #[test]
    fn formatter_returns_text_edit() {
        let text = "let x = 1   \n";
        let edits = format_for_source_public(text).expect("format edits");
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_text, "let x = 1\n");
    }

    #[test]
    fn code_actions_offer_unknown_var_replacement() {
        let text = r#"
let
  x = 1
in
  y + x
"#;
        let actions = code_actions_for_source_public(text, 4, 2);
        let titles: Vec<String> = actions
            .into_iter()
            .filter_map(|action| match action {
                CodeActionOrCommand::CodeAction(action) => Some(action.title),
                CodeActionOrCommand::Command(_) => None,
            })
            .collect();
        assert!(
            titles
                .iter()
                .any(|title| title.contains("Replace `y` with `x`")),
            "titles: {titles:#?}"
        );
    }

    #[test]
    fn code_actions_offer_list_wrap_fix() {
        let text = r#"
let
  xs : List i32 = 1
in
  xs
"#;
        let uri = in_memory_doc_uri();
        clear_parse_cache(&uri);
        let diagnostics = diagnostics_from_text(&uri, text);
        let list_diag = diagnostics
            .into_iter()
            .find(|diag| diag.message.contains("types do not unify"))
            .expect("expected unification diagnostic");
        let list_diag_message = list_diag.message.clone();
        assert!(
            is_list_scalar_unification_error(&list_diag_message),
            "expected list/scalar mismatch, got: {list_diag_message}"
        );
        let request_range = full_document_range(text);
        let actions = code_actions_for_source(&uri, text, request_range, &[list_diag]);
        let titles: Vec<String> = actions
            .into_iter()
            .filter_map(|action| match action {
                CodeActionOrCommand::CodeAction(action) => Some(action.title),
                CodeActionOrCommand::Command(_) => None,
            })
            .collect();
        assert!(
            titles
                .iter()
                .any(|title| title == "Wrap expression in list literal"),
            "diag: {:?}; titles: {titles:#?}",
            list_diag_message
        );
    }

    #[test]
    fn code_actions_offer_non_exhaustive_match_fix() {
        let text = "match (Some 1) when Some x -> x";
        let actions = code_actions_for_source_public(text, 0, 2);
        let titles: Vec<String> = actions
            .into_iter()
            .filter_map(|action| match action {
                CodeActionOrCommand::CodeAction(action) => Some(action.title),
                CodeActionOrCommand::Command(_) => None,
            })
            .collect();
        assert!(
            titles
                .iter()
                .any(|title| title == "Add wildcard arm to match"),
            "titles: {titles:#?}"
        );
    }

    #[test]
    fn code_actions_offer_function_value_mismatch_fixes() {
        let text = "let f = \\x -> x in f + 1";
        let uri = in_memory_doc_uri();
        clear_parse_cache(&uri);
        let diagnostics = diagnostics_from_text(&uri, text);
        let fun_mismatch = diagnostics
            .into_iter()
            .find(|diag| is_function_value_unification_error(&diag.message))
            .expect("expected function/value mismatch diagnostic");
        let actions =
            code_actions_for_source(&uri, text, full_document_range(text), &[fun_mismatch]);
        let titles: Vec<String> = actions
            .into_iter()
            .filter_map(|action| match action {
                CodeActionOrCommand::CodeAction(action) => Some(action.title),
                CodeActionOrCommand::Command(_) => None,
            })
            .collect();
        assert!(
            titles
                .iter()
                .any(|title| title == "Apply expression to missing argument"),
            "titles: {titles:#?}"
        );
        assert!(
            titles
                .iter()
                .any(|title| title == "Wrap expression in lambda"),
            "titles: {titles:#?}"
        );
    }

    #[test]
    fn code_actions_offer_default_disambiguation_with_is_for_record_update() {
        let text = r#"
type A = A { x: i32, y: i32 }
type B = B { x: i32, y: i32 }

instance Default A
    default = A { x = 1, y = 2 }

instance Default B
    default = B { x = 10, y = 20 }

let
    a = { default with { x = 9 } },
    b = { default with { y = 8 } }
in
    (a, b)
"#;
        let uri = in_memory_doc_uri();
        clear_parse_cache(&uri);
        let diagnostics = diagnostics_from_text(&uri, text);
        let diag = diagnostics
            .into_iter()
            .find(|d| d.message.contains("field `x` is not definitely available"))
            .expect("expected field availability diagnostic");
        let actions = code_actions_for_source(&uri, text, diag.range, std::slice::from_ref(&diag));
        let code_actions: Vec<CodeAction> = actions
            .into_iter()
            .filter_map(|action| match action {
                CodeActionOrCommand::CodeAction(action) => Some(action),
                CodeActionOrCommand::Command(_) => None,
            })
            .collect();

        let titles: Vec<String> = code_actions.iter().map(|a| a.title.clone()).collect();
        assert!(
            titles
                .iter()
                .any(|title| title == "Disambiguate `default` as `A`"),
            "titles: {titles:#?}"
        );
        assert!(
            titles
                .iter()
                .any(|title| title == "Disambiguate `default` as `B`"),
            "titles: {titles:#?}"
        );
        assert!(
            titles.iter().any(|title| title == "Annotate `a` as `A`"),
            "titles: {titles:#?}"
        );
        assert!(
            titles.iter().any(|title| title == "Annotate `a` as `B`"),
            "titles: {titles:#?}"
        );

        let contains_fix_for = |needle: &str| {
            code_actions.iter().any(|action| {
                action
                    .edit
                    .as_ref()
                    .and_then(|edit| edit.changes.as_ref())
                    .and_then(|changes| changes.get(&uri))
                    .is_some_and(|edits| edits.iter().any(|e| e.new_text.contains(needle)))
            })
        };
        assert!(
            contains_fix_for("(default is A)"),
            "expected `(default is A)` edit"
        );
        assert!(
            contains_fix_for("(default is B)"),
            "expected `(default is B)` edit"
        );
        assert!(contains_fix_for(": A"), "expected `: A` annotation edit");
        assert!(contains_fix_for(": B"), "expected `: B` annotation edit");
    }

    #[test]
    fn code_actions_offer_hole_fill_candidates() {
        let text = r#"
fn aa_mk : i32 -> i32 = \x -> x
let y : i32 = ? in y
"#;
        let actions = code_actions_for_source_public(text, 2, 14);
        let titles: Vec<String> = actions
            .into_iter()
            .filter_map(|action| match action {
                CodeActionOrCommand::CodeAction(action) => Some(action.title),
                CodeActionOrCommand::Command(_) => None,
            })
            .collect();
        assert!(
            titles
                .iter()
                .any(|title| title.contains("Fill hole with `aa_mk`")),
            "titles: {titles:#?}"
        );
    }

    #[test]
    fn code_actions_offer_hole_fill_even_with_diagnostics_present() {
        let text = r#"
fn aa_mk : i32 -> i32 = \x -> x
let y : i32 = ? in y
"#;
        let uri = in_memory_doc_uri();
        clear_parse_cache(&uri);
        let request_range = Range {
            start: Position {
                line: 2,
                character: 14,
            },
            end: Position {
                line: 2,
                character: 14,
            },
        };
        let diagnostics = vec![Diagnostic {
            range: request_range,
            severity: Some(DiagnosticSeverity::ERROR),
            message: "synthetic diagnostic".to_string(),
            source: Some("test".to_string()),
            ..Diagnostic::default()
        }];
        let actions = code_actions_for_source(&uri, text, request_range, &diagnostics);
        let titles: Vec<String> = actions
            .into_iter()
            .filter_map(|action| match action {
                CodeActionOrCommand::CodeAction(action) => Some(action.title),
                CodeActionOrCommand::Command(_) => None,
            })
            .collect();
        assert!(
            titles
                .iter()
                .any(|title| title.contains("Fill hole with `aa_mk`")),
            "titles: {titles:#?}"
        );
    }

    #[test]
    fn code_actions_offer_hole_fill_for_real_typed_hole_diagnostic() {
        let text = r#"
fn aa_mk : i32 -> i32 = \x -> x
let y : i32 = ? in y
"#;
        let uri = in_memory_doc_uri();
        clear_parse_cache(&uri);
        let diagnostics = diagnostics_from_text(&uri, text);
        let hole_diag = diagnostics
            .iter()
            .find(|d| {
                d.message
                    .contains("typed hole `?` must be filled before evaluation")
            })
            .expect("typed hole diagnostic")
            .clone();
        let actions = code_actions_for_source(&uri, text, hole_diag.range, &[hole_diag]);
        let titles: Vec<String> = actions
            .into_iter()
            .filter_map(|action| match action {
                CodeActionOrCommand::CodeAction(action) => Some(action.title),
                CodeActionOrCommand::Command(_) => None,
            })
            .collect();
        assert!(
            titles
                .iter()
                .any(|title| title.contains("Fill hole with `aa_mk`")),
            "titles: {titles:#?}"
        );
    }

    #[test]
    fn expected_type_reports_if_condition_bool() {
        let text = "if true then 1 else 2";
        let ty = expected_type_for_source_public(text, 0, 3).expect("expected type at condition");
        assert_eq!(ty, "bool");
    }

    #[test]
    fn expected_type_reports_if_branch_type() {
        let text = "let x : i32 = if true then 1 else 2 in x";
        let ty = expected_type_for_source_public(text, 0, 27).expect("expected type at branch");
        assert_eq!(ty, "i32");
    }

    #[test]
    fn expected_type_reports_function_argument_type() {
        let text = "let f = \\x -> x + 1 in f 2";
        let ty = expected_type_for_source_public(text, 0, 26).expect("expected type at argument");
        assert_eq!(ty, "'r");
    }

    #[test]
    fn functions_producing_expected_type_include_user_fn() {
        let text = r#"
fn mk : i32 -> i32 = \x -> x
if true then 0 else 1
"#;
        let items = functions_producing_expected_type_for_source_public(text, 2, 13);
        assert!(
            items.iter().any(|item| item.starts_with("mk : ")),
            "items: {items:#?}"
        );
    }

    #[test]
    fn command_parses_uri_position_tuple_args() {
        let args = vec![
            Value::String("inmemory:///docs.rex".to_string()),
            Value::from(2u64),
            Value::from(3u64),
        ];
        let (uri, pos) = command_uri_and_position(&args).expect("parsed command args");
        assert_eq!(uri.as_str(), "inmemory:///docs.rex");
        assert_eq!(pos.line, 2);
        assert_eq!(pos.character, 3);
    }

    #[test]
    fn command_parses_uri_position_and_id_tuple_args() {
        let args = vec![
            Value::String("inmemory:///docs.rex".to_string()),
            Value::from(2u64),
            Value::from(3u64),
            Value::String("qf-abc".to_string()),
        ];
        let (uri, pos, id) = command_uri_position_and_id(&args).expect("parsed command args");
        assert_eq!(uri.as_str(), "inmemory:///docs.rex");
        assert_eq!(pos.line, 2);
        assert_eq!(pos.character, 3);
        assert_eq!(id, "qf-abc");
    }

    #[test]
    fn command_parses_uri_position_max_steps_strategy_and_dry_run_tuple_args() {
        let args = vec![
            Value::String("inmemory:///docs.rex".to_string()),
            Value::from(2u64),
            Value::from(3u64),
            Value::from(5u64),
            Value::String("aggressive".to_string()),
            Value::Bool(true),
        ];
        let (uri, pos, max_steps, strategy, dry_run) =
            command_uri_position_max_steps_strategy_and_dry_run(&args)
                .expect("parsed command args");
        assert_eq!(uri.as_str(), "inmemory:///docs.rex");
        assert_eq!(pos.line, 2);
        assert_eq!(pos.character, 3);
        assert_eq!(max_steps, 5);
        assert_eq!(strategy, BulkQuickFixStrategy::Aggressive);
        assert!(dry_run);
    }

    #[test]
    fn command_parses_uri_position_max_steps_strategy_and_dry_run_object_args() {
        let args = vec![json!({
            "uri": "inmemory:///docs.rex",
            "line": 4,
            "character": 9,
            "maxSteps": 99,
            "strategy": "conservative",
            "dryRun": false
        })];
        let (uri, pos, max_steps, strategy, dry_run) =
            command_uri_position_max_steps_strategy_and_dry_run(&args)
                .expect("parsed command args");
        assert_eq!(uri.as_str(), "inmemory:///docs.rex");
        assert_eq!(pos.line, 4);
        assert_eq!(pos.character, 9);
        assert_eq!(max_steps, 20, "maxSteps should clamp to upper bound");
        assert_eq!(strategy, BulkQuickFixStrategy::Conservative);
        assert!(!dry_run);
    }

    #[test]
    fn execute_expected_type_command_returns_object() {
        let uri = in_memory_doc_uri();
        let text = "if true then 1 else 2";
        let out = execute_query_command_for_document(
            CMD_EXPECTED_TYPE_AT,
            &uri,
            text,
            Position {
                line: 0,
                character: 3,
            },
        )
        .expect("command output");
        let expected = out
            .as_object()
            .and_then(|o| o.get("expectedType"))
            .and_then(Value::as_str)
            .expect("expectedType");
        assert_eq!(expected, "bool");
    }

    #[test]
    fn execute_functions_command_returns_items() {
        let uri = in_memory_doc_uri();
        let text = r#"
fn mk : i32 -> i32 = \x -> x
if true then 0 else 1
"#;
        let out = execute_query_command_for_document(
            CMD_FUNCTIONS_PRODUCING_EXPECTED_TYPE_AT,
            &uri,
            text,
            Position {
                line: 2,
                character: 13,
            },
        )
        .expect("command output");
        let items = out
            .as_object()
            .and_then(|o| o.get("items"))
            .and_then(Value::as_array)
            .expect("items array");
        assert!(
            items
                .iter()
                .filter_map(Value::as_str)
                .any(|item| item.starts_with("mk : ")),
            "items: {items:#?}"
        );
    }

    #[test]
    fn semantic_functions_command_caps_items_count() {
        let uri = in_memory_doc_uri();
        let mut lines = Vec::new();
        for i in 0..200usize {
            lines.push(format!("fn mk_{i} : i32 -> i32 = \\x -> x"));
        }
        lines.push("let y : i32 = ? in y".to_string());
        let line = (lines.len() - 1) as u32;
        let text = lines.join("\n");
        let out = execute_query_command_for_document(
            CMD_FUNCTIONS_PRODUCING_EXPECTED_TYPE_AT,
            &uri,
            &text,
            Position {
                line,
                character: 14,
            },
        )
        .expect("command output");
        let items = out
            .as_object()
            .and_then(|o| o.get("items"))
            .and_then(Value::as_array)
            .expect("items array");
        assert!(
            items.len() <= MAX_SEMANTIC_CANDIDATES,
            "items_len={}; max={MAX_SEMANTIC_CANDIDATES}",
            items.len()
        );
    }

    #[test]
    fn execute_functions_accepting_inferred_type_command_returns_items() {
        let uri = in_memory_doc_uri();
        let text = r#"
fn use_bool : bool -> i32 = \x -> 0
let x = true in x
"#;
        let out = execute_query_command_for_document(
            CMD_FUNCTIONS_ACCEPTING_INFERRED_TYPE_AT,
            &uri,
            text,
            Position {
                line: 2,
                character: 8,
            },
        )
        .expect("command output");
        let obj = out.as_object().expect("object");
        let inferred = obj
            .get("inferredType")
            .and_then(Value::as_str)
            .expect("inferredType");
        assert_eq!(inferred, "bool");
        let items = obj
            .get("items")
            .and_then(Value::as_array)
            .expect("items array");
        assert!(
            items
                .iter()
                .filter_map(Value::as_str)
                .any(|item| item.starts_with("use_bool : ")),
            "items: {items:#?}"
        );
    }

    #[test]
    fn execute_adapters_from_inferred_to_expected_command_returns_items() {
        let uri = in_memory_doc_uri();
        let text = r#"
fn id_i32 : i32 -> i32 = \x -> x
let x = 1 in let y : i32 = x in y
"#;
        let out = execute_query_command_for_document(
            CMD_ADAPTERS_FROM_INFERRED_TO_EXPECTED_AT,
            &uri,
            text,
            Position {
                line: 2,
                character: 30,
            },
        )
        .expect("command output");
        let obj = out.as_object().expect("object");
        let inferred = obj
            .get("inferredType")
            .and_then(Value::as_str)
            .expect("inferredType");
        assert_eq!(inferred, "i32");
        let expected = obj
            .get("expectedType")
            .and_then(Value::as_str)
            .expect("expectedType");
        assert_eq!(expected, "i32");
        let items = obj
            .get("items")
            .and_then(Value::as_array)
            .expect("items array");
        assert!(
            items
                .iter()
                .filter_map(Value::as_str)
                .any(|item| item.starts_with("id_i32 : ")),
            "items: {items:#?}"
        );
    }

    #[test]
    fn semantic_holes_command_caps_hole_count() {
        let uri = in_memory_doc_uri();
        let mut text = String::from("let ys : List i32 = [");
        for i in 0..160usize {
            if i > 0 {
                text.push_str(", ");
            }
            text.push('?');
        }
        text.push_str("] in ys");
        let out = execute_query_command_for_document_without_position(
            CMD_HOLES_EXPECTED_TYPES,
            &uri,
            &text,
        )
        .expect("command output");
        let holes = out
            .as_object()
            .and_then(|o| o.get("holes"))
            .and_then(Value::as_array)
            .expect("holes array");
        assert!(
            holes.len() <= MAX_SEMANTIC_HOLES,
            "holes_len={}; max={MAX_SEMANTIC_HOLES}",
            holes.len()
        );
    }

    #[test]
    fn execute_functions_compatible_with_in_scope_values_command_returns_items() {
        let uri = in_memory_doc_uri();
        let text = r#"
fn to_string_i32 : i32 -> string = \x -> "ok"
let x = 1 in let y : string = ? in y
"#;
        let out = execute_query_command_for_document(
            CMD_FUNCTIONS_COMPATIBLE_WITH_IN_SCOPE_VALUES_AT,
            &uri,
            text,
            Position {
                line: 2,
                character: 30,
            },
        )
        .expect("command output");
        let items = out
            .as_object()
            .and_then(|o| o.get("items"))
            .and_then(Value::as_array)
            .expect("items array");
        assert!(
            items
                .iter()
                .filter_map(Value::as_str)
                .any(|item| item.contains("to_string_i32") && item.contains("to_string_i32 x")),
            "items: {items:#?}"
        );
    }

    #[test]
    fn semantic_loop_step_caps_in_scope_values() {
        let uri = in_memory_doc_uri();
        let mut lines = Vec::new();
        for i in 0..160usize {
            lines.push(format!("fn x{i} : i32 -> i32 = \\v -> v"));
        }
        lines.push("let y : i32 = ? in y".to_string());
        let line = (lines.len() - 1) as u32;
        let text = lines.join("\n");
        let out = execute_semantic_loop_step(
            &uri,
            &text,
            Position {
                line,
                character: 14,
            },
        )
        .expect("step output");
        let in_scope_values = out
            .as_object()
            .and_then(|o| o.get("inScopeValues"))
            .and_then(Value::as_array)
            .expect("inScopeValues");
        assert!(
            in_scope_values.len() <= MAX_SEMANTIC_IN_SCOPE_VALUES,
            "in_scope_values_len={}; max={MAX_SEMANTIC_IN_SCOPE_VALUES}",
            in_scope_values.len()
        );
    }

    #[test]
    fn hole_expected_types_detects_placeholder_vars() {
        let uri = in_memory_doc_uri();
        let text = "let y : i32 = _ in y";
        let holes = hole_expected_types_for_document(&uri, text);
        assert!(!holes.is_empty(), "holes: {holes:#?}");
        let expected = holes
            .iter()
            .find_map(|hole| hole.get("expectedType").and_then(Value::as_str));
        assert_eq!(expected, Some("i32"));
    }

    #[test]
    fn hole_expected_types_detects_question_holes() {
        let uri = in_memory_doc_uri();
        let text = "let y : i32 = ? in y";
        let holes = hole_expected_types_for_document(&uri, text);
        assert!(!holes.is_empty(), "holes: {holes:#?}");
        let hole_name = holes
            .iter()
            .find_map(|hole| hole.get("name").and_then(Value::as_str));
        assert_eq!(hole_name, Some("?"));
        let expected = holes
            .iter()
            .find_map(|hole| hole.get("expectedType").and_then(Value::as_str));
        assert_eq!(expected, Some("i32"));
    }

    #[test]
    fn execute_holes_command_returns_holes_array() {
        let uri = in_memory_doc_uri();
        let text = "let y : i32 = _ in y";
        let out = execute_query_command_for_document_without_position(
            CMD_HOLES_EXPECTED_TYPES,
            &uri,
            text,
        )
        .expect("command output");
        let holes = out
            .as_object()
            .and_then(|o| o.get("holes"))
            .and_then(Value::as_array)
            .expect("holes array");
        assert!(!holes.is_empty(), "holes: {holes:#?}");
    }

    #[test]
    fn semantic_loop_step_reports_expected_type_and_candidates() {
        let uri = in_memory_doc_uri();
        let text = r#"
fn mk : i32 -> i32 = \x -> x
let y : i32 = ? in y
"#;
        let out = execute_semantic_loop_step(
            &uri,
            text,
            Position {
                line: 2,
                character: 14,
            },
        )
        .expect("command output");
        let obj = out.as_object().expect("object");

        let expected = obj
            .get("expectedType")
            .and_then(Value::as_str)
            .expect("expectedType");
        assert_eq!(expected, "i32");

        let function_candidates = obj
            .get("functionCandidates")
            .and_then(Value::as_array)
            .expect("functionCandidates");
        assert!(
            function_candidates
                .iter()
                .filter_map(Value::as_str)
                .any(|item| item.starts_with("mk : ")),
            "function candidates: {function_candidates:#?}"
        );

        let quick_fix_titles = obj
            .get("quickFixTitles")
            .and_then(Value::as_array)
            .expect("quickFixTitles");
        assert!(
            quick_fix_titles
                .iter()
                .filter_map(Value::as_str)
                .any(|title| title == "Fill hole with `mk`"),
            "quick fixes: {quick_fix_titles:#?}"
        );
        let quick_fixes = obj
            .get("quickFixes")
            .and_then(Value::as_array)
            .expect("quickFixes");
        assert!(
            quick_fixes.iter().any(|qf| {
                qf.get("id").and_then(Value::as_str).is_some()
                    && qf.get("title").and_then(Value::as_str) == Some("Fill hole with `mk`")
                    && qf.get("kind").and_then(Value::as_str) == Some("quickfix")
                    && qf.get("edit").is_some()
            }),
            "quickFixes: {quick_fixes:#?}"
        );

        let accepting = obj
            .get("functionsAcceptingInferredType")
            .and_then(Value::as_array)
            .expect("functionsAcceptingInferredType");
        assert!(
            accepting
                .iter()
                .filter_map(Value::as_str)
                .any(|item| item.starts_with("mk : ")),
            "accepting: {accepting:#?}"
        );
    }

    #[test]
    fn semantic_loop_step_reports_local_diagnostics_and_fixes() {
        let uri = in_memory_doc_uri();
        let text = "let y = z in y";
        let out = execute_semantic_loop_step(
            &uri,
            text,
            Position {
                line: 0,
                character: 8,
            },
        )
        .expect("command output");
        let obj = out.as_object().expect("object");

        let local_diagnostics = obj
            .get("localDiagnostics")
            .and_then(Value::as_array)
            .expect("localDiagnostics");
        assert!(
            local_diagnostics.iter().any(|diag| {
                diag.get("message")
                    .and_then(Value::as_str)
                    .is_some_and(|message| message.contains("unbound variable z"))
            }),
            "diagnostics: {local_diagnostics:#?}"
        );

        let quick_fix_titles = obj
            .get("quickFixTitles")
            .and_then(Value::as_array)
            .expect("quickFixTitles");
        assert!(
            quick_fix_titles
                .iter()
                .filter_map(Value::as_str)
                .any(|title| title.contains("Introduce `let z = null`")),
            "quick fixes: {quick_fix_titles:#?}"
        );
        let quick_fixes = obj
            .get("quickFixes")
            .and_then(Value::as_array)
            .expect("quickFixes");
        assert!(
            quick_fixes.iter().any(|qf| {
                qf.get("id").and_then(Value::as_str).is_some()
                    && qf
                        .get("title")
                        .and_then(Value::as_str)
                        .is_some_and(|title| title.contains("Introduce `let z = null`"))
                    && qf.get("kind").and_then(Value::as_str) == Some("quickfix")
            }),
            "quickFixes: {quick_fixes:#?}"
        );
    }

    #[test]
    fn semantic_loop_step_json_contract_is_stable() {
        let uri = in_memory_doc_uri();
        let text = r#"
fn mk : i32 -> i32 = \x -> x
let y : i32 = ? in y
"#;
        let out = execute_semantic_loop_step(
            &uri,
            text,
            Position {
                line: 2,
                character: 14,
            },
        )
        .expect("step output");
        let obj = expect_object(&out);

        // Required top-level fields and types.
        assert!(obj.contains_key("expectedType"));
        assert!(obj.contains_key("inferredType"));
        expect_array_field(obj, "inScopeValues");
        expect_array_field(obj, "functionCandidates");
        expect_array_field(obj, "holeFillCandidates");
        expect_array_field(obj, "functionsAcceptingInferredType");
        expect_array_field(obj, "adaptersFromInferredToExpectedType");
        expect_array_field(obj, "functionsCompatibleWithInScopeValues");
        expect_array_field(obj, "localDiagnostics");
        expect_array_field(obj, "quickFixes");
        expect_array_field(obj, "quickFixTitles");
        expect_array_field(obj, "holes");

        let quick_fixes = expect_array_field(obj, "quickFixes");
        assert!(
            !quick_fixes.is_empty(),
            "quickFixes should not be empty: {obj:#?}"
        );
        let first_quick_fix = expect_object(quick_fixes.first().expect("first quick fix"));
        expect_string_field(first_quick_fix, "id");
        expect_string_field(first_quick_fix, "title");
        // `kind` may be null for some future quick-fix types, but key should exist.
        assert!(
            first_quick_fix.contains_key("kind"),
            "quickFix should include `kind`: {first_quick_fix:#?}"
        );
        assert!(
            first_quick_fix.contains_key("edit"),
            "quickFix should include `edit`: {first_quick_fix:#?}"
        );
    }

    #[test]
    fn semantic_loop_apply_quick_fix_resolves_by_id() {
        let uri = in_memory_doc_uri();
        let text = "let y = z in y";
        let step = execute_semantic_loop_step(
            &uri,
            text,
            Position {
                line: 0,
                character: 8,
            },
        )
        .expect("step output");
        let quick_fix_id = step
            .get("quickFixes")
            .and_then(Value::as_array)
            .and_then(|arr| arr.first())
            .and_then(|item| item.get("id"))
            .and_then(Value::as_str)
            .expect("quick fix id")
            .to_string();

        let out = execute_semantic_loop_apply_quick_fix(
            &uri,
            text,
            Position {
                line: 0,
                character: 8,
            },
            &quick_fix_id,
        )
        .expect("apply output");
        let quick_fix = out
            .get("quickFix")
            .and_then(Value::as_object)
            .expect("quickFix object");
        assert_eq!(
            quick_fix
                .get("id")
                .and_then(Value::as_str)
                .expect("quickFix.id"),
            quick_fix_id
        );
        assert!(quick_fix.get("edit").is_some(), "quickFix: {quick_fix:#?}");
    }

    #[test]
    fn semantic_loop_apply_quick_fix_json_contract_is_stable() {
        let uri = in_memory_doc_uri();
        let text = "let y = z in y";
        let step = execute_semantic_loop_step(
            &uri,
            text,
            Position {
                line: 0,
                character: 8,
            },
        )
        .expect("step output");
        let quick_fix_id = step
            .get("quickFixes")
            .and_then(Value::as_array)
            .and_then(|arr| arr.first())
            .and_then(|item| item.get("id"))
            .and_then(Value::as_str)
            .expect("quick fix id")
            .to_string();

        let out = execute_semantic_loop_apply_quick_fix(
            &uri,
            text,
            Position {
                line: 0,
                character: 8,
            },
            &quick_fix_id,
        )
        .expect("apply output");
        let obj = expect_object(&out);
        let quick_fix = expect_object(obj.get("quickFix").expect("quickFix"));
        expect_string_field(quick_fix, "id");
        expect_string_field(quick_fix, "title");
        assert!(quick_fix.contains_key("kind"));
        assert!(quick_fix.contains_key("edit"));
    }

    #[test]
    fn semantic_loop_apply_quick_fix_unknown_id_returns_null() {
        let uri = in_memory_doc_uri();
        let text = "let y = z in y";
        let out = execute_semantic_loop_apply_quick_fix(
            &uri,
            text,
            Position {
                line: 0,
                character: 8,
            },
            "qf-does-not-exist",
        )
        .expect("apply output");
        assert_eq!(out, Value::Null);
    }

    #[test]
    fn semantic_loop_apply_best_quick_fixes_updates_text_and_reduces_errors() {
        let uri = in_memory_doc_uri();
        let text = "let y = z in y";
        let out = execute_semantic_loop_apply_best_quick_fixes(
            &uri,
            text,
            Position {
                line: 0,
                character: 8,
            },
            3,
            BulkQuickFixStrategy::Conservative,
            false,
        )
        .expect("bulk output");

        let applied_count = out
            .get("appliedCount")
            .and_then(Value::as_u64)
            .expect("appliedCount");
        assert!(applied_count >= 1, "out: {out:#?}");

        let updated_text = out
            .get("updatedText")
            .and_then(Value::as_str)
            .expect("updatedText");
        assert_ne!(updated_text, text, "out: {out:#?}");

        let diagnostics_after = out
            .get("localDiagnosticsAfter")
            .and_then(Value::as_array)
            .expect("localDiagnosticsAfter");
        assert!(
            diagnostics_after.iter().all(|diag| {
                !diag
                    .get("message")
                    .and_then(Value::as_str)
                    .is_some_and(|m| m.contains("unbound variable z"))
            }),
            "diagnostics_after: {diagnostics_after:#?}"
        );
        let steps = out.get("steps").and_then(Value::as_array).expect("steps");
        assert!(!steps.is_empty(), "out: {out:#?}");
        let strategy = out
            .get("strategy")
            .and_then(Value::as_str)
            .expect("strategy");
        assert_eq!(strategy, "conservative");
        let first_step = steps
            .first()
            .and_then(Value::as_object)
            .expect("first step");
        assert!(
            first_step.get("quickFix").is_some(),
            "step: {first_step:#?}"
        );
        let before_count = first_step
            .get("diagnosticsBeforeCount")
            .and_then(Value::as_u64)
            .expect("diagnosticsBeforeCount");
        let after_count = first_step
            .get("diagnosticsAfterCount")
            .and_then(Value::as_u64)
            .expect("diagnosticsAfterCount");
        assert!(after_count <= before_count, "step: {first_step:#?}");
    }

    #[test]
    fn semantic_loop_apply_best_quick_fixes_json_contract_is_stable() {
        let uri = in_memory_doc_uri();
        let text = "let y = z in y";
        let out = execute_semantic_loop_apply_best_quick_fixes(
            &uri,
            text,
            Position {
                line: 0,
                character: 8,
            },
            2,
            BulkQuickFixStrategy::Conservative,
            true,
        )
        .expect("bulk output");
        let obj = expect_object(&out);

        assert_eq!(expect_string_field(obj, "strategy"), "conservative");
        obj.get("dryRun")
            .and_then(Value::as_bool)
            .expect("dryRun bool");
        obj.get("appliedCount")
            .and_then(Value::as_u64)
            .expect("appliedCount u64");
        obj.get("updatedText")
            .and_then(Value::as_str)
            .expect("updatedText string");
        expect_array_field(obj, "appliedQuickFixes");
        expect_array_field(obj, "steps");
        expect_array_field(obj, "localDiagnosticsAfter");
        expect_string_field(obj, "stoppedReason");
        expect_string_field(obj, "stoppedReasonDetail");
        obj.get("lastDiagnosticsDelta")
            .and_then(Value::as_i64)
            .expect("lastDiagnosticsDelta i64");
        obj.get("noImprovementStreak")
            .and_then(Value::as_u64)
            .expect("noImprovementStreak u64");
        obj.get("seenStatesCount")
            .and_then(Value::as_u64)
            .expect("seenStatesCount u64");

        if let Some(first_step) = expect_array_field(obj, "steps").first() {
            let step_obj = expect_object(first_step);
            step_obj
                .get("index")
                .and_then(Value::as_u64)
                .expect("step.index");
            assert!(step_obj.contains_key("quickFix"));
            expect_array_field(step_obj, "diagnosticsBefore");
            expect_array_field(step_obj, "diagnosticsAfter");
            step_obj
                .get("diagnosticsBeforeCount")
                .and_then(Value::as_u64)
                .expect("step diagnosticsBeforeCount");
            step_obj
                .get("diagnosticsAfterCount")
                .and_then(Value::as_u64)
                .expect("step diagnosticsAfterCount");
            step_obj
                .get("diagnosticsDelta")
                .and_then(Value::as_i64)
                .expect("step diagnosticsDelta");
            step_obj
                .get("noImprovementStreak")
                .and_then(Value::as_u64)
                .expect("step noImprovementStreak");
        }
    }

    #[test]
    fn semantic_loop_apply_best_quick_fixes_no_context_returns_no_quickfix() {
        let uri = in_memory_doc_uri();
        let text = "let x = 1 in x";
        let out = execute_semantic_loop_apply_best_quick_fixes(
            &uri,
            text,
            Position {
                line: 0,
                character: 4,
            },
            3,
            BulkQuickFixStrategy::Conservative,
            false,
        )
        .expect("bulk output");
        let obj = expect_object(&out);
        assert_eq!(expect_string_field(obj, "stoppedReason"), "noQuickFix");
        assert_eq!(
            obj.get("appliedCount")
                .and_then(Value::as_u64)
                .expect("appliedCount"),
            0
        );
        assert!(expect_array_field(obj, "steps").is_empty(), "out={out:#?}");
    }

    #[test]
    fn semantic_loop_step_parse_error_still_returns_contract_shape() {
        let uri = in_memory_doc_uri();
        let text = "let x =";
        let out = execute_semantic_loop_step(
            &uri,
            text,
            Position {
                line: 0,
                character: 6,
            },
        )
        .expect("step output");
        let obj = expect_object(&out);
        // On parse failure we still return the same top-level contract shape.
        expect_array_field(obj, "quickFixes");
        expect_array_field(obj, "quickFixTitles");
        expect_array_field(obj, "localDiagnostics");
        expect_array_field(obj, "holes");
    }

    #[test]
    fn golden_flow_hole_to_apply_by_id_reduces_hole_count() {
        let uri = in_memory_doc_uri();
        let text = r#"
fn mk : i32 -> i32 = \x -> x
let y : i32 = ? in y
"#;
        let step = execute_semantic_loop_step(
            &uri,
            text,
            Position {
                line: 2,
                character: 14,
            },
        )
        .expect("step output");
        let quick_fix_id = step
            .get("quickFixes")
            .and_then(Value::as_array)
            .and_then(|arr| {
                arr.iter().find(|item| {
                    item.get("title")
                        .and_then(Value::as_str)
                        .is_some_and(|title| title == "Fill hole with `mk`")
                })
            })
            .and_then(|item| item.get("id"))
            .and_then(Value::as_str)
            .expect("hole fill quick-fix id")
            .to_string();
        let apply = execute_semantic_loop_apply_quick_fix(
            &uri,
            text,
            Position {
                line: 2,
                character: 14,
            },
            &quick_fix_id,
        )
        .expect("apply output");
        let quick_fix = apply.get("quickFix").expect("quickFix returned").clone();
        let edit: WorkspaceEdit =
            serde_json::from_value(quick_fix.get("edit").cloned().expect("quickFix.edit"))
                .expect("workspace edit");
        let updated = apply_workspace_edit_to_text(&uri, text, &edit).expect("apply edit");

        let holes_before = hole_expected_types_for_document(&uri, text);
        let holes_after = hole_expected_types_for_document(&uri, &updated);
        assert!(
            holes_after.len() < holes_before.len(),
            "before={holes_before:#?}; after={holes_after:#?}; updated=\n{updated}"
        );
    }

    #[test]
    fn golden_flow_unknown_var_bulk_repairs_local_error() {
        let uri = in_memory_doc_uri();
        let text = "let y = z in y";
        let out = execute_semantic_loop_apply_best_quick_fixes(
            &uri,
            text,
            Position {
                line: 0,
                character: 8,
            },
            3,
            BulkQuickFixStrategy::Conservative,
            false,
        )
        .expect("bulk output");
        let diagnostics_after = out
            .get("localDiagnosticsAfter")
            .and_then(Value::as_array)
            .expect("localDiagnosticsAfter");
        assert!(
            diagnostics_after.iter().all(|diag| {
                !diag
                    .get("message")
                    .and_then(Value::as_str)
                    .is_some_and(|msg| msg.contains("unbound variable z"))
            }),
            "out={out:#?}"
        );
    }

    #[test]
    fn golden_flow_complex_bulk_preview_then_apply_same_projection() {
        let uri = in_memory_doc_uri();
        let text = r#"
let
  y = z
in
  match y when Some x -> x
"#;
        let preview = execute_semantic_loop_apply_best_quick_fixes(
            &uri,
            text,
            Position {
                line: 2,
                character: 6,
            },
            3,
            BulkQuickFixStrategy::Aggressive,
            true,
        )
        .expect("preview output");
        let apply = execute_semantic_loop_apply_best_quick_fixes(
            &uri,
            text,
            Position {
                line: 2,
                character: 6,
            },
            3,
            BulkQuickFixStrategy::Aggressive,
            false,
        )
        .expect("apply output");

        let preview_text = preview
            .get("updatedText")
            .and_then(Value::as_str)
            .expect("preview updatedText");
        let apply_text = apply
            .get("updatedText")
            .and_then(Value::as_str)
            .expect("apply updatedText");
        assert_eq!(
            preview_text, apply_text,
            "preview={preview:#?}\napply={apply:#?}"
        );

        let preview_steps = preview
            .get("steps")
            .and_then(Value::as_array)
            .expect("preview steps");
        let apply_steps = apply
            .get("steps")
            .and_then(Value::as_array)
            .expect("apply steps");
        assert_eq!(preview_steps.len(), apply_steps.len());
    }

    #[test]
    fn bulk_strategy_simple_unknown_var_applies_introduce_fix() {
        let uri = in_memory_doc_uri();
        let text = "let y = z in y";
        let out = execute_semantic_loop_apply_best_quick_fixes(
            &uri,
            text,
            Position {
                line: 0,
                character: 8,
            },
            1,
            BulkQuickFixStrategy::Conservative,
            false,
        )
        .expect("bulk output");
        let first_title = out
            .get("steps")
            .and_then(Value::as_array)
            .and_then(|steps| steps.first())
            .and_then(|step| step.get("quickFix"))
            .and_then(|qf| qf.get("title"))
            .and_then(Value::as_str)
            .expect("first quick-fix title");
        assert!(
            first_title.contains("Introduce `let z = null`"),
            "first_title={first_title}; out={out:#?}"
        );
    }

    #[test]
    fn bulk_strategy_medium_distinguishes_conservative_vs_aggressive_ranking() {
        let candidates = vec![
            json!({
                "id": "qf-add",
                "title": "Add wildcard arm to match",
                "kind": "quickfix",
                "edit": Value::Null,
            }),
            json!({
                "id": "qf-intro",
                "title": "Introduce `let z = null`",
                "kind": "quickfix",
                "edit": Value::Null,
            }),
        ];
        let conservative =
            best_quick_fix_from_candidates(&candidates, BulkQuickFixStrategy::Conservative)
                .expect("conservative choice");
        let aggressive =
            best_quick_fix_from_candidates(&candidates, BulkQuickFixStrategy::Aggressive)
                .expect("aggressive choice");
        assert_eq!(
            conservative
                .get("title")
                .and_then(Value::as_str)
                .expect("conservative title"),
            "Add wildcard arm to match"
        );
        assert_eq!(
            aggressive
                .get("title")
                .and_then(Value::as_str)
                .expect("aggressive title"),
            "Introduce `let z = null`"
        );
    }

    #[test]
    fn bulk_strategy_complex_returns_step_telemetry_for_each_step() {
        let uri = in_memory_doc_uri();
        let text = r#"
let
  y = z
in
  match y when Some x -> x
"#;
        let out = execute_semantic_loop_apply_best_quick_fixes(
            &uri,
            text,
            Position {
                line: 2,
                character: 6,
            },
            3,
            BulkQuickFixStrategy::Aggressive,
            false,
        )
        .expect("bulk output");

        let steps = out
            .get("steps")
            .and_then(Value::as_array)
            .expect("steps array");
        assert!(!steps.is_empty(), "out: {out:#?}");
        for (i, step) in steps.iter().enumerate() {
            let diagnostics_before = step
                .get("diagnosticsBefore")
                .and_then(Value::as_array)
                .expect("diagnosticsBefore");
            let diagnostics_after = step
                .get("diagnosticsAfter")
                .and_then(Value::as_array)
                .expect("diagnosticsAfter");
            let before_count = step
                .get("diagnosticsBeforeCount")
                .and_then(Value::as_u64)
                .expect("diagnosticsBeforeCount");
            let after_count = step
                .get("diagnosticsAfterCount")
                .and_then(Value::as_u64)
                .expect("diagnosticsAfterCount");
            let delta = step
                .get("diagnosticsDelta")
                .and_then(Value::as_i64)
                .expect("diagnosticsDelta");
            assert_eq!(
                diagnostics_before.len() as u64,
                before_count,
                "step[{i}]={step:#?}"
            );
            assert_eq!(
                diagnostics_after.len() as u64,
                after_count,
                "step[{i}]={step:#?}"
            );
            assert_eq!(
                before_count as i64 - after_count as i64,
                delta,
                "step[{i}]={step:#?}"
            );
        }
    }

    #[test]
    fn bulk_strategy_stops_after_requested_step_limit() {
        let uri = in_memory_doc_uri();
        let text = r#"
let
  y = z
in
  match y when Some x -> x
"#;
        let out = execute_semantic_loop_apply_best_quick_fixes(
            &uri,
            text,
            Position {
                line: 2,
                character: 6,
            },
            1,
            BulkQuickFixStrategy::Aggressive,
            false,
        )
        .expect("bulk output");
        let applied_count = out
            .get("appliedCount")
            .and_then(Value::as_u64)
            .expect("appliedCount");
        let steps = out.get("steps").and_then(Value::as_array).expect("steps");
        let stopped_reason = out
            .get("stoppedReason")
            .and_then(Value::as_str)
            .expect("stoppedReason");
        let stopped_detail = out
            .get("stoppedReasonDetail")
            .and_then(Value::as_str)
            .expect("stoppedReasonDetail");
        let seen_states = out
            .get("seenStatesCount")
            .and_then(Value::as_u64)
            .expect("seenStatesCount");
        assert_eq!(applied_count, 1, "out: {out:#?}");
        assert_eq!(steps.len(), 1, "out: {out:#?}");
        assert_eq!(stopped_reason, "maxStepsReached", "out: {out:#?}");
        assert!(
            stopped_detail.contains("maxSteps"),
            "stopped_detail={stopped_detail}; out={out:#?}"
        );
        assert!(seen_states >= 2, "out: {out:#?}");
    }

    #[test]
    fn bulk_mode_dry_run_reports_flag_and_predicted_text() {
        let uri = in_memory_doc_uri();
        let text = "let y = z in y";
        let out = execute_semantic_loop_apply_best_quick_fixes(
            &uri,
            text,
            Position {
                line: 0,
                character: 8,
            },
            2,
            BulkQuickFixStrategy::Conservative,
            true,
        )
        .expect("bulk output");
        let dry_run = out.get("dryRun").and_then(Value::as_bool).expect("dryRun");
        assert!(dry_run, "out: {out:#?}");
        let updated_text = out
            .get("updatedText")
            .and_then(Value::as_str)
            .expect("updatedText");
        assert_ne!(updated_text, text, "out: {out:#?}");
    }

    #[test]
    fn bulk_progress_guard_no_improvement_streak_logic() {
        assert_eq!(next_no_improvement_streak(0, 1), 0);
        assert_eq!(next_no_improvement_streak(0, 0), 1);
        assert_eq!(next_no_improvement_streak(1, -1), 2);
    }

    #[test]
    fn bulk_progress_guard_cycle_detection_hashes_equal_texts() {
        let a = text_state_hash("let y = z in y");
        let b = text_state_hash("let y = z in y");
        let c = text_state_hash("let y = null in y");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}

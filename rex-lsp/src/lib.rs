#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

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
    Type, TypeError as TsTypeError, TypeKind, TypeSystem, TypedExpr, TypedExprKind, instantiate,
    unify,
};
use rex_util::sha256_hex;
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

#[derive(Debug)]
enum TokenizeOrParseError {
    Lex(LexicalError),
    Parse(Vec<ParserErr>),
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

fn clear_parse_cache(uri: &Url) {
    let Ok(mut cache) = parse_cache().lock() else {
        return;
    };
    cache.remove(uri);
}

fn tokenize_and_parse(text: &str) -> std::result::Result<(Tokens, Program), TokenizeOrParseError> {
    let tokens = Token::tokenize(text).map_err(TokenizeOrParseError::Lex)?;
    let mut parser = Parser::new(tokens.clone());
    let program = parser
        .parse_program()
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
    path: Option<PathBuf>,
    value_map: HashMap<rex_ast::expr::Symbol, rex_ast::expr::Symbol>, // field -> internal name
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

    for (idx, decl) in program.decls.iter().enumerate() {
        match decl {
            Decl::Instance(inst_decl) => {
                let prepared = ts.inject_instance_decl(inst_decl)?;
                if want_prepared_instance.is_some_and(|want| want == idx) {
                    prepared_target = Some(prepared.clone());
                }
                instances.push((idx, prepared));
            }
            _ => ts.inject_decl(decl)?,
        }
    }

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

    let importer = uri.to_file_path().ok();

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
        let importer = uri
            .to_file_path()
            .map_err(|_| "not a file uri".to_string())?;
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
            && let Ok(module_uri) = Url::from_file_path(path)
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
            diagnostics.push(diagnostic_for_span(Span::default(), err));
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
            push_ts_error(err, diagnostics);
            return;
        }
    };

    // Typecheck instance method bodies too, so errors inside the instance show
    // up as diagnostics.
    for (decl_idx, prepared) in instances {
        let Decl::Instance(inst_decl) = &program.decls[decl_idx] else {
            continue;
        };
        for method in &inst_decl.methods {
            if let Err(err) = ts.typecheck_instance_method(&prepared, method) {
                push_ts_error(err, diagnostics);
                return;
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

pub async fn run_stdio() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(RexServer::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}

#[cfg(test)]
mod tests {
    use super::*;

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
}

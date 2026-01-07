#![forbid(unsafe_code)]

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use rex_ast::expr::{
    intern, Decl, DeclareFnDecl, Expr, FnDecl, ImportDecl, ImportPath, InstanceDecl, Pattern,
    Program, TypeDecl, TypeExpr, Var,
};
use rex_lexer::{
    span::{Position as RexPosition, Span, Spanned},
    LexicalError, Token, Tokens,
};
use rex_parser::Parser;
use rex_ts::Types;
use rex_ts::{
    instantiate, unify, Type, TypeError as TsTypeError, TypeKind, TypeSystem, TypedExpr,
    TypedExprKind,
};
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

#[derive(Clone)]
struct ImportModuleInfo {
    path: Option<PathBuf>,
    value_map: HashMap<rex_ast::expr::Symbol, rex_ast::expr::Symbol>, // field -> internal name
    export_defs: HashMap<String, Span>,
}

fn stdlib_source(module: &str) -> Option<&'static str> {
    match module {
        "std.io" => Some(
            r#"
pub declare fn debug (x: a) -> string where Pretty a
pub declare fn info (x: a) -> string where Pretty a
pub declare fn warn (x: a) -> string where Pretty a
pub declare fn error (x: a) -> string where Pretty a

pub declare fn write_all (fd: i32) -> (contents: Array u8) -> ()
pub declare fn read_all (fd: i32) -> Array u8
"#,
        ),
        "std.process" => Some(
            r#"
pub type Subprocess = Subprocess { id: uuid }

pub declare fn spawn (opts: { cmd: string, args: List string }) -> Subprocess
pub declare fn wait (p: Subprocess) -> i32
pub declare fn stdout (p: Subprocess) -> Array u8
pub declare fn stderr (p: Subprocess) -> Array u8
"#,
        ),
        _ => None,
    }
}

fn sha256_hex(input: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    let mut h0: u32 = 0x6a09e667;
    let mut h1: u32 = 0xbb67ae85;
    let mut h2: u32 = 0x3c6ef372;
    let mut h3: u32 = 0xa54ff53a;
    let mut h4: u32 = 0x510e527f;
    let mut h5: u32 = 0x9b05688c;
    let mut h6: u32 = 0x1f83d9ab;
    let mut h7: u32 = 0x5be0cd19;

    let bit_len = (input.len() as u64) * 8;
    let mut msg = Vec::with_capacity(((input.len() + 9 + 63) / 64) * 64);
    msg.extend_from_slice(input);
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, word) in w.iter_mut().take(16).enumerate() {
            let b = &chunk[i * 4..i * 4 + 4];
            *word = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let mut a = h0;
        let mut b = h1;
        let mut c = h2;
        let mut d = h3;
        let mut e = h4;
        let mut f = h5;
        let mut g = h6;
        let mut h = h7;

        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        h0 = h0.wrapping_add(a);
        h1 = h1.wrapping_add(b);
        h2 = h2.wrapping_add(c);
        h3 = h3.wrapping_add(d);
        h4 = h4.wrapping_add(e);
        h5 = h5.wrapping_add(f);
        h6 = h6.wrapping_add(g);
        h7 = h7.wrapping_add(h);
    }

    let out = [
        h0.to_be_bytes(),
        h1.to_be_bytes(),
        h2.to_be_bytes(),
        h3.to_be_bytes(),
        h4.to_be_bytes(),
        h5.to_be_bytes(),
        h6.to_be_bytes(),
        h7.to_be_bytes(),
    ]
    .concat();

    let mut s = String::with_capacity(64);
    for b in out {
        s.push_str(&format!("{:02x}", b));
    }
    s
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
        let ts = TypeSystem::with_prelude();
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

fn resolve_local_import(importer: &Path, segments: &[rex_ast::expr::Symbol]) -> Option<PathBuf> {
    let base_dir = importer.parent()?;
    let mut dir = base_dir.to_path_buf();
    let mut idx = 0usize;
    while idx < segments.len() && segments[idx].as_ref() == "super" {
        dir = dir.parent()?.to_path_buf();
        idx += 1;
    }

    let mut path = dir;
    for seg in &segments[idx..segments.len().saturating_sub(1)] {
        path.push(seg.as_ref());
    }
    let last = segments.last()?;
    path.push(format!("{}.rex", last.as_ref()));
    path.canonicalize().ok()
}

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
            if let Expr::Var(v) = base.as_ref() {
                if !bound.contains(&v.name) {
                    if let Some(info) = imports.get(&v.name) {
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
                }
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

fn prepare_program_with_imports(
    uri: &Url,
    program: &Program,
) -> std::result::Result<
    (
        Program,
        TypeSystem,
        HashMap<rex_ast::expr::Symbol, ImportModuleInfo>,
        Vec<Diagnostic>,
    ),
    String,
> {
    let mut ts = TypeSystem::with_prelude();
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
            if let Some(source) = stdlib_source(&module_name) {
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
                let Some(module_path) = resolve_local_import(importer, segments) else {
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

        let tokens = match Token::tokenize(&source) {
            Ok(t) => t,
            Err(err) => {
                let LexicalError::UnexpectedToken(span) = err;
                diagnostics.push(diagnostic_for_span(
                    import_span,
                    format!(
                        "lex error in module `{}` at {}:{}",
                        module_label,
                        span.begin.line,
                        span.begin.column
                    ),
                ));
                continue;
            }
        };

        let mut parser = Parser::new(tokens.clone());
        let module_program = match parser.parse_program() {
            Ok(p) => p,
            Err(errs) => {
                for err in errs {
                    diagnostics.push(diagnostic_for_span(
                        import_span,
                        format!(
                            "parse error in module `{}` at {}:{}: {}",
                            module_label,
                            err.span.begin.line,
                            err.span.begin.column,
                            err.message
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
                        constraints: keep_constraints
                            .then(|| fd.constraints.clone())
                            .unwrap_or_default(),
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
                        constraints: keep_constraints
                            .then(|| df.constraints.clone())
                            .unwrap_or_default(),
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

    let source = if let Some(source) = stdlib_source(&module_name) {
        source.to_string()
    } else {
        let importer = uri
            .to_file_path()
            .map_err(|_| "not a file uri".to_string())?;
        let Some(module_path) = resolve_local_import(&importer, segments) else {
            return Ok(Vec::new());
        };
        fs::read_to_string(&module_path).map_err(|e| e.to_string())?
    };
    let tokens = Token::tokenize(&source).map_err(|e| e.to_string())?;
    let mut parser = Parser::new(tokens);
    let module_program = parser
        .parse_program()
        .map_err(|_| "parse error".to_string())?;

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
        let diagnostics = diagnostics_from_text(&uri, text);
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

        let contents = hover_type_contents(&uri, &text, position).or_else(|| {
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

        let items = completion_items(&uri, &text, position);
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

        let imported_projection = imported_projection_at_position(&tokens, position);

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

        // If the cursor is on `alias.field` and `alias` is a local import, jump
        // to the exported declaration in the imported module.
        if let Some((alias, field)) = imported_projection {
            if let Ok((_rewritten, _ts, imports, _diags)) =
                prepare_program_with_imports(&uri, &program)
            {
                let alias_sym = intern(&alias);
                if let Some(info) = imports.get(&alias_sym) {
                    if let Some(span) = info.export_defs.get(&field) {
                        if let Some(path) = info.path.as_ref() {
                            if let Ok(module_uri) = Url::from_file_path(path) {
                                return Ok(Some(GotoDefinitionResponse::Scalar(Location {
                                    uri: module_uri,
                                    range: span_to_range(*span),
                                })));
                            }
                        }
                    }
                }
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

fn diagnostics_from_text(uri: &Url, text: &str) -> Vec<Diagnostic> {
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
                            push_type_diagnostics(uri, text, &program, &mut diagnostics);
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

fn hover_type_contents(uri: &Url, text: &str, position: Position) -> Option<HoverContents> {
    let tokens = Token::tokenize(text).ok()?;
    let (name, name_span, name_is_ident) = name_token_at_position(&tokens, position)?;

    let mut parser = Parser::new(tokens);
    let program = parser.parse_program().ok()?;
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
            Decl::Import(..) => {}
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
            (
                Expr::Lam(_span, _scope, param, _ann, _constraints, body),
                TypedExprKind::Lam { body: tbody, .. },
            ) => {
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
                visit(
                    ts,
                    inner.as_ref(),
                    typed,
                    pos,
                    name,
                    name_span,
                    name_is_ident,
                    best,
                );
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
                        visit(
                            ts,
                            v.as_ref(),
                            tv,
                            pos,
                            name,
                            name_span,
                            name_is_ident,
                            best,
                        );
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
                visit(
                    ts,
                    base.as_ref(),
                    tbase.as_ref(),
                    pos,
                    name,
                    name_span,
                    name_is_ident,
                    best,
                );
                for (k, v) in updates {
                    if let Some(tv) = tupdates.get(k) {
                        visit(
                            ts,
                            v.as_ref(),
                            tv,
                            pos,
                            name,
                            name_span,
                            name_is_ident,
                            best,
                        );
                    }
                }
            }
            (Expr::App(_span, f, x), TypedExprKind::App(tf, tx)) => {
                visit(
                    ts,
                    f.as_ref(),
                    tf.as_ref(),
                    pos,
                    name,
                    name_span,
                    name_is_ident,
                    best,
                );
                visit(
                    ts,
                    x.as_ref(),
                    tx.as_ref(),
                    pos,
                    name,
                    name_span,
                    name_is_ident,
                    best,
                );
            }
            (Expr::Project(_span, e, _field), TypedExprKind::Project { expr: te, .. }) => {
                visit(
                    ts,
                    e.as_ref(),
                    te.as_ref(),
                    pos,
                    name,
                    name_span,
                    name_is_ident,
                    best,
                );
            }
            (
                Expr::Ite(_span, c, t, e),
                TypedExprKind::Ite {
                    cond,
                    then_expr,
                    else_expr,
                },
            ) => {
                visit(
                    ts,
                    c.as_ref(),
                    cond.as_ref(),
                    pos,
                    name,
                    name_span,
                    name_is_ident,
                    best,
                );
                visit(
                    ts,
                    t.as_ref(),
                    then_expr.as_ref(),
                    pos,
                    name,
                    name_span,
                    name_is_ident,
                    best,
                );
                visit(
                    ts,
                    e.as_ref(),
                    else_expr.as_ref(),
                    pos,
                    name,
                    name_span,
                    name_is_ident,
                    best,
                );
            }
            (
                Expr::Match(_span, scrutinee, arms),
                TypedExprKind::Match {
                    scrutinee: tscrut,
                    arms: tarms,
                },
            ) => {
                visit(
                    ts,
                    scrutinee.as_ref(),
                    tscrut.as_ref(),
                    pos,
                    name,
                    name_span,
                    name_is_ident,
                    best,
                );
                for ((_pat, arm_body), (_tpat, tarm_body)) in arms.iter().zip(tarms.iter()) {
                    visit(
                        ts,
                        arm_body.as_ref(),
                        tarm_body,
                        pos,
                        name,
                        name_span,
                        name_is_ident,
                        best,
                    );
                }
            }
            _ => {}
        }
    }

    let mut best = None;
    visit(
        ts,
        expr,
        typed,
        pos,
        name,
        name_span,
        name_is_ident,
        &mut best,
    );
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
            Decl::Import(..) => {}
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
    if let Ok(tokens) = Token::tokenize(text) {
        let mut parser = Parser::new(tokens);
        if let Ok(program) = parser.parse_program() {
            return completion_items_from_program(
                &program,
                position,
                field_mode,
                base_ident.as_deref(),
                uri,
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
    uri: &Url,
) -> Vec<CompletionItem> {
    if field_mode {
        if let Some(base_ident) = base_ident {
            if let Ok(exports) = completion_exports_for_module_alias(uri, program, base_ident) {
                if !exports.is_empty() {
                    return exports
                        .into_iter()
                        .map(|label| completion_item(label, CompletionItemKind::FIELD))
                        .collect();
                }
            }
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

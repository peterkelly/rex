use lsp_types::{
    CodeAction, CodeActionOrCommand, Diagnostic, DiagnosticSeverity, Position, Range, Url,
    WorkspaceEdit,
};
use rex::{
    engine::{Builder, CompileOptions},
    parser::parse as parse_rex,
};
use rex_ast::{CompilationUnit, Decl, Expr, NameRef, TypeExpr};
use rex_engine::ValueDisplayOptions;
use rex_lsp::*;
use rex_lsp::{code_actions::*, document::*, imports::*, public::*, queries::*, shared::*};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

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

fn temp_dir(name: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    dir.push(format!("rex-lsp-test-{name}-{nonce}"));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn assert_internal_name_ref(name: &NameRef) {
    match name {
        NameRef::Unqualified(sym) => {
            assert!(
                sym.as_ref().starts_with("@m"),
                "expected internal rewritten symbol, got `{sym}`"
            );
        }
        other => panic!("expected unqualified rewritten name, got {other:?}"),
    }
}

fn position_of(text: &str, needle: &str) -> Position {
    let index = text
        .find(needle)
        .unwrap_or_else(|| panic!("missing `{needle}` in test source"));
    let prefix = &text[..index];
    let line = prefix.bytes().filter(|b| *b == b'\n').count() as u32;
    let character = prefix
        .rsplit('\n')
        .next()
        .expect("split always returns one item")
        .chars()
        .count() as u32;
    Position { line, character }
}

fn engine_module_prefix_for_path(path: &Path) -> String {
    let module_name = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .expect("test module path should have a UTF-8 stem");
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in b"module:" {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
    }
    for segment in module_name.split('.') {
        for b in segment.as_bytes() {
            hash ^= u64::from(*b);
            hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
        }
        hash ^= u64::from(b'.');
        hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
    }
    format!("@m{hash:016x}")
}

async fn eval_source_to_display(code: &str) -> (String, String) {
    let program = parse_rex(code).expect("parse source");
    let builder = Builder::with_prelude(()).expect("build builder");
    let compiler = builder.build_compiler();
    let (compiled, evaluator) = compiler
        .compile_program(&program, CompileOptions::for_module("lsp.test").unwrap())
        .await
        .expect("compile source");
    let ty = compiled.result_type().clone();
    let handle = evaluator
        .run(compiled, Default::default())
        .await
        .expect("evaluate source");
    let display = handle
        .display_with(ValueDisplayOptions {
            include_numeric_suffixes: true,
            ..ValueDisplayOptions::default()
        })
        .expect("display value");
    (display, strip_internal_module_qualifiers(&ty.to_string()))
}

fn strip_internal_module_qualifiers(text: &str) -> String {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '@' || c == '.'))
        .fold(text.to_string(), |out, token| {
            let Some(rest) = token.strip_prefix("@m") else {
                return out;
            };
            let Some((hash, local)) = rest.split_once('.') else {
                return out;
            };
            if hash.len() == 16 && hash.chars().all(|c| c.is_ascii_hexdigit()) {
                out.replace(token, local)
            } else {
                out
            }
        })
}

fn diagnostics_from_text(uri: &Url, text: &str) -> Vec<Diagnostic> {
    let session = AnalysisSession::isolated();
    rex_lsp::diagnostics::diagnostics_from_text(&session, uri, text)
}

fn prepare_program_with_imports(
    uri: &Url,
    compilation_unit: &CompilationUnit,
) -> std::result::Result<PreparedProgram, String> {
    let session = AnalysisSession::isolated();
    rex_lsp::imports::prepare_program_with_imports(&session, uri, compilation_unit)
}

fn code_actions_for_source(
    uri: &Url,
    text: &str,
    request_range: Range,
    diagnostics: &[Diagnostic],
) -> Vec<CodeActionOrCommand> {
    let session = AnalysisSession::isolated();
    rex_lsp::code_actions::code_actions_for_source(&session, uri, text, request_range, diagnostics)
}

fn execute_query_command_for_document(
    command: &str,
    uri: &Url,
    text: &str,
    position: Position,
) -> Option<Value> {
    let session = AnalysisSession::isolated();
    rex_lsp::queries::execute_query_command_for_document(&session, command, uri, text, position)
}

fn execute_query_command_for_document_without_position(
    command: &str,
    uri: &Url,
    text: &str,
) -> Option<Value> {
    let session = AnalysisSession::isolated();
    rex_lsp::queries::execute_query_command_for_document_without_position(
        &session, command, uri, text,
    )
}

fn execute_semantic_loop_step(uri: &Url, text: &str, position: Position) -> Option<Value> {
    let session = AnalysisSession::isolated();
    rex_lsp::queries::execute_semantic_loop_step(&session, uri, text, position)
}

fn execute_semantic_loop_apply_quick_fix(
    uri: &Url,
    text: &str,
    quick_fix: &Value,
) -> Option<Value> {
    rex_lsp::queries::execute_semantic_loop_apply_quick_fix(uri, text, None, quick_fix)
}

fn execute_semantic_loop_apply_best_quick_fixes(
    uri: &Url,
    text: &str,
    position: Position,
    max_steps: usize,
    strategy: BulkQuickFixStrategy,
    dry_run: bool,
) -> Option<Value> {
    let session = AnalysisSession::isolated();
    rex_lsp::queries::execute_semantic_loop_apply_best_quick_fixes(
        &session, uri, text, position, max_steps, strategy, dry_run,
    )
}

fn hole_expected_types_for_document(uri: &Url, text: &str) -> Vec<Value> {
    let session = AnalysisSession::isolated();
    rex_lsp::queries::hole_expected_types_for_document(&session, uri, text)
}

#[test]
fn diagnostics_handle_c_style_comments() {
    let uri = Url::parse("inmemory:///comments.rex").expect("uri");

    let ok = diagnostics_from_text(&uri, "/* arbitrary @#$ text */\n1 // line comment");
    assert!(ok.is_empty(), "unexpected diagnostics: {ok:?}");

    let unclosed = diagnostics_from_text(&uri, "1 /* unfinished");
    assert!(
        unclosed
            .iter()
            .any(|diag| diag.message.contains("Unclosed block comment opener")),
        "expected unclosed block comment diagnostic, got {unclosed:?}"
    );

    let unmatched = diagnostics_from_text(&uri, "1 */");
    assert!(
        unmatched
            .iter()
            .any(|diag| diag.message.contains("Unmatched block comment closer")),
        "expected unmatched block comment diagnostic, got {unmatched:?}"
    );
}

#[test]
fn diagnostics_resolve_imports_from_open_document_snapshot() {
    let dir = temp_dir("diagnostics_resolve_imports_from_open_document_snapshot");
    let main = dir.join("main.rex");
    let dep = dir.join("dep.rex");
    fs::write(&main, "()").expect("write main");
    fs::write(
        &dep,
        r#"
pub fn stale x: i32 -> i32 = x;
"#,
    )
    .expect("write stale dep");

    let uri = Url::from_file_path(&main).expect("main file uri");
    let dep_uri = Url::from_file_path(&dep).expect("dep file uri");
    let source = r#"
import dep as D;

D.fresh 1
"#;

    let disk_diags = diagnostics_from_text(&uri, source);
    assert!(
        disk_diags
            .iter()
            .any(|diag| diag.message.contains("does not export `fresh`")),
        "expected stale disk module to miss fresh export, got {disk_diags:?}"
    );

    let mut first_snapshot = HashMap::new();
    first_snapshot.insert(uri.clone(), source.to_string());
    first_snapshot.insert(
        dep_uri,
        r#"
pub fn fresh x: i32 -> i32 = x + 1;
"#
        .to_string(),
    );
    let state = AnalysisState::new();
    let open_session = state.session(first_snapshot);
    let open_diags = rex_lsp::diagnostics::diagnostics_from_text(&open_session, &uri, source);
    assert!(
        open_diags.is_empty(),
        "expected open dependency buffer to typecheck, got {open_diags:?}"
    );

    let stale_session = state.session(HashMap::from([(uri.clone(), source.to_string())]));
    let stale_diags = rex_lsp::diagnostics::diagnostics_from_text(&stale_session, &uri, source);
    assert!(
        stale_diags
            .iter()
            .any(|diag| diag.message.contains("does not export `fresh`")),
        "expected separate session without open dependency to use stale disk module, got {stale_diags:?}"
    );
}

#[test]
fn prepare_program_rewrites_imported_type_refs_in_annotations() {
    let dir = temp_dir("prepare_program_rewrites_imported_type_refs_in_annotations");
    let main = dir.join("main.rex");
    let dep = dir.join("dep.rex");
    fs::write(
        &dep,
        r#"
pub type Boxed = Boxed i32;
"#,
    )
    .expect("write dep");
    fs::write(&main, "()").expect("write main");

    let uri = Url::from_file_path(&main).expect("main file uri");
    let source = r#"
import dep as D;

let x : D.Boxed = D.Boxed 1 in
x is D.Boxed
"#;
    let program = parse_rex(source).expect("parse");
    let (rewritten, _ts, _imports, diags) =
        prepare_program_with_imports(&uri, &program).expect("prepare");
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");

    let Expr::Let(_, _, _, Some(let_ann), _, body) = rewritten.body.as_ref().unwrap().as_ref()
    else {
        panic!("expected rewritten let expression");
    };
    if let TypeExpr::Name(_, name) = let_ann {
        assert_internal_name_ref(name);
    } else {
        panic!("expected rewritten let annotation");
    }

    let Expr::Ann(_, _, ann_ty) = body.as_ref() else {
        panic!("expected rewritten trailing annotation");
    };
    if let TypeExpr::Name(_, name) = ann_ty {
        assert_internal_name_ref(name);
    } else {
        panic!("expected rewritten annotation type");
    }

    if let Expr::Let(_, _, _, _, def, _) = rewritten.body.as_ref().unwrap().as_ref()
        && let Expr::App(_, ctor, _) = def.as_ref()
        && let Expr::Var(v) = ctor.as_ref()
    {
        assert!(
            v.name.as_ref().starts_with("@m"),
            "expected constructor projection rewrite to internal symbol"
        );
    } else {
        panic!("expected rewritten constructor application");
    }
}

#[test]
fn prepare_program_rewrites_imported_class_refs_in_instance_headers() {
    let dir = temp_dir("prepare_program_rewrites_imported_class_refs_in_instance_headers");
    let main = dir.join("main.rex");
    let dep = dir.join("dep.rex");
    fs::write(
        &dep,
        r#"
pub class Pick a where {
    pick : a;
}
"#,
    )
    .expect("write dep");
    fs::write(&main, "()").expect("write main");

    let uri = Url::from_file_path(&main).expect("main file uri");
    let source = r#"
import dep as D;

instance D.Pick i32 where {
    pick = 7;
}
pick is i32
"#;
    let program = parse_rex(source).expect("parse");
    let (rewritten, _ts, _imports, diags) =
        prepare_program_with_imports(&uri, &program).expect("prepare");
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");

    let Some(inst) = rewritten.decls.iter().find_map(|decl| match decl {
        Decl::Instance(inst) => Some(inst),
        _ => None,
    }) else {
        panic!("expected instance declaration");
    };
    assert!(
        inst.class.as_ref().starts_with("@m"),
        "expected rewritten internal class symbol, got `{}`",
        inst.class
    );
}

#[test]
fn prepare_program_registers_imported_class_decls_for_typechecking() {
    let dir = temp_dir("prepare_program_registers_imported_class_decls_for_typechecking");
    let main = dir.join("main.rex");
    let dep = dir.join("dep.rex");
    fs::write(
        &dep,
        r#"
pub class Pick a where {
    pick : a;
}
"#,
    )
    .expect("write dep");
    fs::write(&main, "()").expect("write main");

    let uri = Url::from_file_path(&main).expect("main file uri");
    let source = r#"
import dep as D;

instance D.Pick i32 where {
    pick = 7;
}
pick is i32
"#;
    let program = parse_rex(source).expect("parse");
    let (rewritten, mut ts, _imports, diags) =
        prepare_program_with_imports(&uri, &program).expect("prepare");
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
    ts.register_decls(&rewritten.decls)
        .expect("prepared declarations should typecheck");
}

#[test]
fn prepare_program_registers_transitive_imported_signature_deps() {
    let dir = temp_dir("prepare_program_registers_transitive_imported_signature_deps");
    let main = dir.join("main.rex");
    let dep = dir.join("dep.rex");
    let base = dir.join("base.rex");
    fs::write(
        &base,
        r#"
pub type Boxed = Boxed i32;
"#,
    )
    .expect("write base");
    fs::write(
        &dep,
        r#"
import base as B;

pub fn make x: i32 -> B.Boxed = B.Boxed x;
"#,
    )
    .expect("write dep");
    fs::write(&main, "()").expect("write main");

    let uri = Url::from_file_path(&main).expect("main file uri");
    let source = r#"
import dep as D;

D.make 1
"#;
    let program = parse_rex(source).expect("parse");
    let (rewritten, ts, _imports, diags) =
        prepare_program_with_imports(&uri, &program).expect("prepare");
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");

    let canonical_dep = dep.canonicalize().expect("canonical dep path");
    let expected_name = format!("{}.make", engine_module_prefix_for_path(&canonical_dep));
    assert!(
        ts.env
            .lookup(&rex_ast::Symbol::intern(&expected_name))
            .is_some(),
        "expected transitive-dependent signature `{expected_name}` to be registered"
    );
    let Expr::App(_, f, _) = rewritten.body.as_ref().expect("body").as_ref() else {
        panic!("expected application, got {:?}", rewritten.body);
    };
    let Expr::Var(var) = f.as_ref() else {
        panic!("expected rewritten imported function var, got {f:?}");
    };
    assert_eq!(var.name.as_ref(), expected_name);
}

#[test]
fn diagnostics_report_missing_class_export_in_instance_header() {
    let dir = temp_dir("diagnostics_report_missing_class_export_in_instance_header");
    let main = dir.join("main.rex");
    let dep = dir.join("dep.rex");
    fs::write(
        &dep,
        r#"
pub class Present a where {
    present : a;
}
"#,
    )
    .expect("write dep");
    fs::write(&main, "()").expect("write main");

    let uri = Url::from_file_path(&main).expect("main file uri");
    let source = r#"
import dep as D;

instance D.Missing i32 where {
    missing = 1;
}
0
"#;
    let diags = diagnostics_from_text(&uri, source);
    assert!(
        diags
            .iter()
            .any(|d| d.message.contains("does not export") && d.message.contains("Missing")),
        "diagnostics: {diags:#?}"
    );
}

#[test]
fn diagnostics_report_missing_type_export_in_annotation() {
    let dir = temp_dir("diagnostics_report_missing_type_export_in_annotation");
    let main = dir.join("main.rex");
    let dep = dir.join("dep.rex");
    fs::write(
        &dep,
        r#"
pub type Present = Present i32;
"#,
    )
    .expect("write dep");
    fs::write(&main, "()").expect("write main");

    let uri = Url::from_file_path(&main).expect("main file uri");
    let source = r#"
import dep as D;

fn id x: D.Missing -> D.Missing = x;

0
"#;
    let diags = diagnostics_from_text(&uri, source);
    assert!(
        diags
            .iter()
            .any(|d| d.message.contains("does not export") && d.message.contains("Missing")),
        "diagnostics: {diags:#?}"
    );
}

#[test]
fn diagnostics_report_missing_value_export_at_projection_span() {
    let dir = temp_dir("diagnostics_report_missing_value_export_at_projection_span");
    let main = dir.join("main.rex");
    let dep = dir.join("dep.rex");
    fs::write(
        &dep,
        r#"
pub fn present x: i32 -> i32 = x;
"#,
    )
    .expect("write dep");
    fs::write(&main, "()").expect("write main");

    let uri = Url::from_file_path(&main).expect("main file uri");
    let source = r#"
import dep as D;

D.missing 1
"#;
    let diags = diagnostics_from_text(&uri, source);
    let diag = diags
        .iter()
        .find(|d| d.message.contains("does not export") && d.message.contains("missing"))
        .unwrap_or_else(|| panic!("diagnostics: {diags:#?}"));
    assert_eq!(diag.range.start, position_of(source, "D.missing"));
}

#[test]
fn diagnostics_report_missing_type_export_at_type_name_span() {
    let dir = temp_dir("diagnostics_report_missing_type_export_at_type_name_span");
    let main = dir.join("main.rex");
    let dep = dir.join("dep.rex");
    fs::write(
        &dep,
        r#"
pub type Present = Present i32;
"#,
    )
    .expect("write dep");
    fs::write(&main, "()").expect("write main");

    let uri = Url::from_file_path(&main).expect("main file uri");
    let source = r#"
import dep as D;

fn id x: D.Missing -> i32 = 0;

0
"#;
    let diags = diagnostics_from_text(&uri, source);
    let diag = diags
        .iter()
        .find(|d| d.message.contains("does not export") && d.message.contains("Missing"))
        .unwrap_or_else(|| panic!("diagnostics: {diags:#?}"));
    assert_eq!(diag.range.start, position_of(source, "D.Missing"));
}

#[test]
fn diagnostics_report_missing_type_export_in_instance_head() {
    let dir = temp_dir("diagnostics_report_missing_type_export_in_instance_head");
    let main = dir.join("main.rex");
    let dep = dir.join("dep.rex");
    fs::write(
        &dep,
        r#"
pub class Marker a where {
    marker : i32;
}
"#,
    )
    .expect("write dep");
    fs::write(&main, "()").expect("write main");

    let uri = Url::from_file_path(&main).expect("main file uri");
    let source = r#"
import dep as D;

instance D.Marker D.Missing where {
    marker = 1;
}
0
"#;
    let diags = diagnostics_from_text(&uri, source);
    assert!(
        diags
            .iter()
            .any(|d| d.message.contains("does not export") && d.message.contains("Missing")),
        "diagnostics: {diags:#?}"
    );
}

#[test]
fn diagnostics_report_missing_class_export_in_fn_where_constraint() {
    let dir = temp_dir("diagnostics_report_missing_class_export_in_fn_where_constraint");
    let main = dir.join("main.rex");
    let dep = dir.join("dep.rex");
    fs::write(
        &dep,
        r#"
pub class Present a where {
    present : a;
}
"#,
    )
    .expect("write dep");
    fs::write(&main, "()").expect("write main");

    let uri = Url::from_file_path(&main).expect("main file uri");
    let source = r#"
import dep as D;

fn id x: i32 -> i32 where D.Missing i32 = x;

0
"#;
    let diags = diagnostics_from_text(&uri, source);
    assert!(
        diags
            .iter()
            .any(|d| d.message.contains("does not export") && d.message.contains("Missing")),
        "diagnostics: {diags:#?}"
    );
}

#[test]
fn diagnostics_report_missing_class_export_in_declare_fn_where_constraint() {
    let dir = temp_dir("diagnostics_report_missing_class_export_in_declare_fn_where_constraint");
    let main = dir.join("main.rex");
    let dep = dir.join("dep.rex");
    fs::write(
        &dep,
        r#"
pub class Present a where {
    present : a;
}
"#,
    )
    .expect("write dep");
    fs::write(&main, "()").expect("write main");

    let uri = Url::from_file_path(&main).expect("main file uri");
    let source = r#"
import dep as D;

declare fn id x: i32 -> i32 where D.Missing i32;

0
"#;
    let diags = diagnostics_from_text(&uri, source);
    assert!(
        diags
            .iter()
            .any(|d| d.message.contains("does not export") && d.message.contains("Missing")),
        "diagnostics: {diags:#?}"
    );
}

#[test]
fn diagnostics_report_missing_class_export_in_class_super_constraint() {
    let dir = temp_dir("diagnostics_report_missing_class_export_in_class_super_constraint");
    let main = dir.join("main.rex");
    let dep = dir.join("dep.rex");
    fs::write(
        &dep,
        r#"
pub class Present a where {
    present : a;
}
"#,
    )
    .expect("write dep");
    fs::write(&main, "()").expect("write main");

    let uri = Url::from_file_path(&main).expect("main file uri");
    let source = r#"
import dep as D;

class Local a <= D.Missing a where {
    local : a;
}
0
"#;
    let diags = diagnostics_from_text(&uri, source);
    assert!(
        diags
            .iter()
            .any(|d| d.message.contains("does not export") && d.message.contains("Missing")),
        "diagnostics: {diags:#?}"
    );
}

#[test]
fn diagnostics_allow_lambda_param_named_like_import_alias_in_annotation() {
    let dir = temp_dir("diagnostics_allow_lambda_param_named_like_import_alias_in_annotation");
    let main = dir.join("main.rex");
    let dep = dir.join("dep.rex");
    fs::write(
        &dep,
        r#"
pub type Boxed = Boxed i32;
"#,
    )
    .expect("write dep");
    fs::write(&main, "()").expect("write main");

    let uri = Url::from_file_path(&main).expect("main file uri");
    let source = r#"
import dep as D;

let f = \ (D : D.Boxed) -> 0 in
0
"#;
    let diags = diagnostics_from_text(&uri, source);
    assert!(diags.is_empty(), "diagnostics: {diags:#?}");
}

#[test]
fn diagnostics_report_missing_type_export_in_letrec_annotation_with_alias_named_binding() {
    let dir = temp_dir(
        "diagnostics_report_missing_type_export_in_letrec_annotation_with_alias_named_binding",
    );
    let main = dir.join("main.rex");
    let dep = dir.join("dep.rex");
    fs::write(
        &dep,
        r#"
pub type Present = Present i32;
"#,
    )
    .expect("write dep");
    fs::write(&main, "()").expect("write main");

    let uri = Url::from_file_path(&main).expect("main file uri");
    let source = r#"
import dep as D;

let rec D: D.Missing = 1 in
0
"#;
    let diags = diagnostics_from_text(&uri, source);
    assert!(
        diags
            .iter()
            .any(|d| d.message.contains("does not export") && d.message.contains("Missing")),
        "diagnostics: {diags:#?}"
    );
}

#[test]
fn diagnostics_allow_letrec_annotation_with_alias_named_binding_for_valid_type() {
    let dir =
        temp_dir("diagnostics_allow_letrec_annotation_with_alias_named_binding_for_valid_type");
    let main = dir.join("main.rex");
    let dep = dir.join("dep.rex");
    fs::write(
        &dep,
        r#"
pub type Num = Num i32;
"#,
    )
    .expect("write dep");
    fs::write(&main, "()").expect("write main");

    let uri = Url::from_file_path(&main).expect("main file uri");
    let source = r#"
import dep as D;
import dep (Num);

let rec D: D.Num -> i32 = \_ -> 0 in
0
"#;
    let diags = diagnostics_from_text(&uri, source);
    assert!(diags.is_empty(), "diagnostics: {diags:#?}");
}

#[test]
fn diagnostics_allow_let_annotation_with_alias_named_binding_for_valid_type() {
    let dir = temp_dir("diagnostics_allow_let_annotation_with_alias_named_binding_for_valid_type");
    let main = dir.join("main.rex");
    let dep = dir.join("dep.rex");
    fs::write(
        &dep,
        r#"
pub type Num = Num i32;
"#,
    )
    .expect("write dep");
    fs::write(&main, "()").expect("write main");

    let uri = Url::from_file_path(&main).expect("main file uri");
    let source = r#"
import dep as D;

let D: D.Num -> i32 = \_ -> 0 in
0
"#;
    let diags = diagnostics_from_text(&uri, source);
    assert!(diags.is_empty(), "diagnostics: {diags:#?}");
}

#[test]
fn diagnostics_accept_selected_value_import_with_alias() {
    let dir = temp_dir("diagnostics_accept_selected_value_import_with_alias");
    let main = dir.join("main.rex");
    let dep = dir.join("dep.rex");
    fs::write(
        &dep,
        r#"
pub fn add x: i32 -> y: i32 -> i32 = x + y;
pub fn triple x: i32 -> i32 = x * 3;
"#,
    )
    .expect("write dep");
    fs::write(&main, "()").expect("write main");

    let uri = Url::from_file_path(&main).expect("main file uri");
    let source = r#"
import dep (add, triple as t);

add (t 10) 2
"#;
    let diags = diagnostics_from_text(&uri, source);
    assert!(diags.is_empty(), "diagnostics: {diags:#?}");
}

#[test]
fn prepare_program_uses_engine_module_id_prefix_for_imported_values() {
    let dir = temp_dir("prepare_program_uses_engine_module_id_prefix_for_imported_values");
    let main = dir.join("main.rex");
    let dep = dir.join("dep.rex");
    fs::write(
        &dep,
        r#"
pub fn add x: i32 -> y: i32 -> i32 = x + y;
"#,
    )
    .expect("write dep");
    fs::write(&main, "()").expect("write main");

    let uri = Url::from_file_path(&main).expect("main file uri");
    let source = r#"
import dep as D;

D.add 20 22
"#;
    let program = parse_rex(source).expect("parse");
    let (rewritten, _ts, _imports, diags) =
        prepare_program_with_imports(&uri, &program).expect("prepare");
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");

    let canonical_dep = dep.canonicalize().expect("canonical dep path");
    let expected_name = format!("{}.add", engine_module_prefix_for_path(&canonical_dep));
    let Expr::App(_, f, _) = rewritten.body.as_ref().expect("body").as_ref() else {
        panic!("expected outer application, got {:?}", rewritten.body);
    };
    let Expr::App(_, f, _) = f.as_ref() else {
        panic!("expected inner application, got {f:?}");
    };
    let Expr::Var(var) = f.as_ref() else {
        panic!("expected rewritten imported function var, got {f:?}");
    };
    assert_eq!(var.name.as_ref(), expected_name);
}

#[test]
fn diagnostics_accept_selected_type_and_constructor_import() {
    let dir = temp_dir("diagnostics_accept_selected_type_and_constructor_import");
    let main = dir.join("main.rex");
    let dep = dir.join("dep.rex");
    fs::write(
        &dep,
        r#"
pub type Token = Token;
"#,
    )
    .expect("write dep");
    fs::write(&main, "()").expect("write main");

    let uri = Url::from_file_path(&main).expect("main file uri");
    let source = r#"
import dep (Token);

let id: Token -> Token = \x -> x in
id Token
"#;
    let diags = diagnostics_from_text(&uri, source);
    assert!(diags.is_empty(), "diagnostics: {diags:#?}");
}

#[test]
fn diagnostics_accept_wildcard_type_and_class_imports() {
    let dir = temp_dir("diagnostics_accept_wildcard_type_and_class_imports");
    let main = dir.join("main.rex");
    let dep = dir.join("dep.rex");
    fs::write(
        &dep,
        r#"
pub type Status = Ready | Failed;

pub class Pick a where {
    pick : a;
}
"#,
    )
    .expect("write dep");
    fs::write(&main, "()").expect("write main");

    let uri = Url::from_file_path(&main).expect("main file uri");
    let source = r#"
import dep (*);

instance Pick Status where {
    pick = Ready;
}

pick is Status
"#;
    let diags = diagnostics_from_text(&uri, source);
    assert!(diags.is_empty(), "diagnostics: {diags:#?}");
}

#[test]
fn diagnostics_report_duplicate_selected_import_name() {
    let dir = temp_dir("diagnostics_report_duplicate_selected_import_name");
    let main = dir.join("main.rex");
    let left = dir.join("foo").join("left.rex");
    let right = dir.join("foo").join("right.rex");
    fs::create_dir_all(left.parent().expect("left parent")).expect("create module dir");
    fs::write(
        &left,
        r#"
pub fn add x: i32 -> y: i32 -> i32 = x + y;
"#,
    )
    .expect("write left");
    fs::write(
        &right,
        r#"
pub fn mul x: i32 -> y: i32 -> i32 = x * y;
"#,
    )
    .expect("write right");
    fs::write(&main, "()").expect("write main");

    let uri = Url::from_file_path(&main).expect("main file uri");
    let source = r#"
import foo.left (add);
import foo.right (mul as add);

add 1 2
"#;
    let diags = diagnostics_from_text(&uri, source);
    assert!(
        diags
            .iter()
            .any(|d| d.message.contains("duplicate imported name `add`")),
        "diagnostics: {diags:#?}"
    );
}

#[test]
fn diagnostics_report_selected_import_conflict_with_local() {
    let dir = temp_dir("diagnostics_report_selected_import_conflict_with_local");
    let main = dir.join("main.rex");
    let dep = dir.join("dep.rex");
    fs::write(
        &dep,
        r#"
pub fn add x: i32 -> y: i32 -> i32 = x + y;
"#,
    )
    .expect("write dep");
    fs::write(&main, "()").expect("write main");

    let uri = Url::from_file_path(&main).expect("main file uri");
    let source = r#"
import dep (add);

fn add x: i32 -> y: i32 -> i32 = x - y;

add 10 2
"#;
    let diags = diagnostics_from_text(&uri, source);
    assert!(
        diags
            .iter()
            .any(|d| d.message.contains("conflicts with local declaration")),
        "diagnostics: {diags:#?}"
    );
}

#[test]
fn diagnostics_report_missing_type_export_in_let_annotation_with_alias_named_binding() {
    let dir = temp_dir(
        "diagnostics_report_missing_type_export_in_let_annotation_with_alias_named_binding",
    );
    let main = dir.join("main.rex");
    let dep = dir.join("dep.rex");
    fs::write(
        &dep,
        r#"
pub type Present = Present i32;
"#,
    )
    .expect("write dep");
    fs::write(&main, "()").expect("write main");

    let uri = Url::from_file_path(&main).expect("main file uri");
    let source = r#"
import dep as D;

let D: D.Missing = 1 in
0
"#;
    let diags = diagnostics_from_text(&uri, source);
    assert!(
        diags
            .iter()
            .any(|d| d.message.contains("does not export") && d.message.contains("Missing")),
        "diagnostics: {diags:#?}"
    );
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
fn diagnostics_use_compiler_path_for_local_constructor_names() {
    let text = r#"
type Tree = Empty | Node { key: i32, left: Tree, right: Tree };

fn insert : i32 -> Tree -> Tree = \k t ->
  match t with {
    case Empty -> Node { key = k, left = Empty, right = Empty };
    case Node {key, left, right} ->
      if k < key then
        Node { key = key, left = insert k left, right = right }
      else if k > key then
        Node { key = key, left = left, right = insert k right }
      else
        t;
  };

insert 1 Empty
"#;
    let diags = diagnostics_for_source(text);
    let errors: Vec<&Diagnostic> = diags
        .iter()
        .filter(|d| d.severity == Some(DiagnosticSeverity::ERROR))
        .collect();
    assert!(errors.is_empty(), "diagnostics: {diags:#?}");
}

#[test]
fn diagnostics_report_both_default_record_update_ambiguities() {
    let text = r#"
type A = A { x: i32, y: i32 };
type B = B { x: i32, y: i32 };

instance Default A where {
    default = A { x = 1, y = 2 };
}
instance Default B where {
    default = B { x = 10, y = 20 };
}
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
    let text = r#"type A = A { x: i32, y: i32 };
type B = B { x: i32, y: i32 };

instance Default A where {
    default = A { x = 1, y = 2 };
}
instance Default B where {
    default = B { x = 10, y = 20 };
}
let
    a = { default with { x = 9 } },
    b = { default with { y = 8 } }
in
    (a, b)
"#;
    let uri = in_memory_doc_uri();

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
                item.get("title").and_then(Value::as_str) == Some("Disambiguate `default` as `A`")
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
    let after_b = apply_workspace_edit_to_text(&uri, &after_a, &edit_b).expect("apply edit for b");

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
fn parse_ph : String -> Result String f64 = \raw ->
  if raw == "7.3" then Ok 7.3 else Err "bad reading";
"#;
    let uri = in_memory_doc_uri();
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
fn diagnostics_for_llms_playground_decl_error_are_not_whole_document() {
    let text = r#"fn parse_i32 : String -> Result String i32 = \s ->
  if s == "42" then Ok 42 else Err "bad-int";

fn plus1 : i32 -> i32 = \n -> n + 1;

let input = "42" in
let out : Result String i32 = ? in
out"#;
    let uri = in_memory_doc_uri();
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
    assert_eq!(
        unification.range.start.line, 0,
        "diagnostic should start on the failing declaration: {unification:#?}"
    );
    assert!(
        unification.range.end.line < 3,
        "diagnostic should stay on the failing declaration: {unification:#?}"
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
type T = A | B;
fn f : i32 -> i32 = \x -> x + 1;
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
    let text = "match (Some 1) with { case Some x -> x; }";
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
    let diagnostics = diagnostics_from_text(&uri, text);
    let fun_mismatch = diagnostics
        .into_iter()
        .find(|diag| is_function_value_unification_error(&diag.message))
        .expect("expected function/value mismatch diagnostic");
    let actions = code_actions_for_source(&uri, text, full_document_range(text), &[fun_mismatch]);
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
type A = A { x: i32, y: i32 };
type B = B { x: i32, y: i32 };

instance Default A where {
    default = A { x = 1, y = 2 };
}
instance Default B where {
    default = B { x = 10, y = 20 };
}
let
    a = { default with { x = 9 } },
    b = { default with { y = 8 } }
in
    (a, b)
"#;
    let uri = in_memory_doc_uri();
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
fn aa_mk : i32 -> i32 = \x -> x;
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
fn aa_mk : i32 -> i32 = \x -> x;
let y : i32 = ? in y
"#;
    let uri = in_memory_doc_uri();
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
fn aa_mk : i32 -> i32 = \x -> x;
let y : i32 = ? in y
"#;
    let uri = in_memory_doc_uri();
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
    assert_eq!(ty, "Bool");
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
fn mk : i32 -> i32 = \x -> x;
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
fn command_parses_uri_and_quick_fix_tuple_args() {
    let quick_fix = json!({
        "protocolVersion": 2,
        "id": "qf2-abc",
    });
    let args = vec![
        Value::String("inmemory:///docs.rex".to_string()),
        quick_fix.clone(),
    ];
    let (uri, parsed_quick_fix) = command_uri_and_quick_fix(&args).expect("parsed command args");
    assert_eq!(uri.as_str(), "inmemory:///docs.rex");
    assert_eq!(parsed_quick_fix, quick_fix);
}

#[test]
fn command_parses_uri_and_quick_fix_object_args() {
    let quick_fix = json!({
        "protocolVersion": 2,
        "id": "qf2-abc",
    });
    let args = vec![json!({
        "uri": "inmemory:///docs.rex",
        "quickFix": quick_fix,
    })];
    let (uri, parsed_quick_fix) = command_uri_and_quick_fix(&args).expect("parsed command args");
    assert_eq!(uri.as_str(), "inmemory:///docs.rex");
    assert_eq!(parsed_quick_fix, quick_fix);
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
        command_uri_position_max_steps_strategy_and_dry_run(&args).expect("parsed command args");
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
        command_uri_position_max_steps_strategy_and_dry_run(&args).expect("parsed command args");
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
    assert_eq!(expected, "Bool");
}

#[test]
fn execute_functions_command_returns_items() {
    let uri = in_memory_doc_uri();
    let text = r#"
fn mk : i32 -> i32 = \x -> x;
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
        lines.push(format!("fn mk_{i} : i32 -> i32 = \\x -> x;"));
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
fn use_bool : Bool -> i32 = \x -> 0;
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
    assert_eq!(inferred, "Bool");
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
fn id_i32 : i32 -> i32 = \x -> x;
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
    let out =
        execute_query_command_for_document_without_position(CMD_HOLES_EXPECTED_TYPES, &uri, &text)
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
fn to_string_i32 : i32 -> String = \x -> "ok";
let x = 1 in let y : String = ? in y
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
        lines.push(format!("fn x{i} : i32 -> i32 = \\v -> v;"));
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
    let out =
        execute_query_command_for_document_without_position(CMD_HOLES_EXPECTED_TYPES, &uri, text)
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
fn mk : i32 -> i32 = \x -> x;
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
fn mk : i32 -> i32 = \x -> x;
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
    assert_eq!(
        first_quick_fix
            .get("protocolVersion")
            .and_then(Value::as_u64),
        Some(SEMANTIC_QUICK_FIX_PROTOCOL_VERSION)
    );
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
    let precondition = expect_object(
        first_quick_fix
            .get("precondition")
            .expect("quickFix.precondition"),
    );
    assert_eq!(expect_string_field(precondition, "uri"), uri.as_str());
    expect_string_field(precondition, "contentHash");
    assert!(precondition.contains_key("documentVersion"));
}

#[test]
fn semantic_loop_apply_quick_fix_returns_exact_proposal() {
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
    let quick_fix = step
        .get("quickFixes")
        .and_then(Value::as_array)
        .and_then(|arr| arr.first())
        .cloned()
        .expect("quick fix");

    let out = execute_semantic_loop_apply_quick_fix(&uri, text, &quick_fix).expect("apply output");
    assert_eq!(out.get("status").and_then(Value::as_str), Some("ready"));
    let returned_quick_fix = out.get("quickFix").expect("quickFix object");
    assert_eq!(returned_quick_fix, &quick_fix);
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
    let quick_fix = step
        .get("quickFixes")
        .and_then(Value::as_array)
        .and_then(|arr| arr.first())
        .cloned()
        .expect("quick fix");

    let out = execute_semantic_loop_apply_quick_fix(&uri, text, &quick_fix).expect("apply output");
    let obj = expect_object(&out);
    assert_eq!(expect_string_field(obj, "status"), "ready");
    let quick_fix = expect_object(obj.get("quickFix").expect("quickFix"));
    assert_eq!(
        quick_fix.get("protocolVersion").and_then(Value::as_u64),
        Some(SEMANTIC_QUICK_FIX_PROTOCOL_VERSION)
    );
    expect_string_field(quick_fix, "id");
    expect_string_field(quick_fix, "title");
    assert!(quick_fix.contains_key("kind"));
    assert!(quick_fix.contains_key("edit"));
}

#[test]
fn semantic_loop_apply_quick_fix_rejects_tampered_proposal() {
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
    let mut quick_fix = step
        .get("quickFixes")
        .and_then(Value::as_array)
        .and_then(|arr| arr.first())
        .cloned()
        .expect("quick fix");
    quick_fix["title"] = Value::String("tampered title".to_string());

    let out = execute_semantic_loop_apply_quick_fix(&uri, text, &quick_fix).expect("apply output");
    assert_eq!(out.get("status").and_then(Value::as_str), Some("rejected"));
    assert_eq!(
        out.get("reason").and_then(Value::as_str),
        Some("proposalIdMismatch")
    );
}

#[test]
fn semantic_loop_apply_quick_fix_rejects_stale_document() {
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
    let quick_fix = step
        .get("quickFixes")
        .and_then(Value::as_array)
        .and_then(|arr| arr.first())
        .cloned()
        .expect("quick fix");

    let out = execute_semantic_loop_apply_quick_fix(&uri, "let y = z in y ", &quick_fix)
        .expect("apply output");
    assert_eq!(out.get("status").and_then(Value::as_str), Some("stale"));
    assert_eq!(
        out.get("reason").and_then(Value::as_str),
        Some("documentContentChanged")
    );
}

#[test]
fn semantic_loop_apply_quick_fix_rejects_stale_document_version() {
    let uri = in_memory_doc_uri();
    let text = "let y = z in y";
    let session = AnalysisSession::isolated();
    let step = rex_lsp::queries::execute_semantic_loop_step_with_version(
        &session,
        &uri,
        text,
        Position {
            line: 0,
            character: 8,
        },
        Some(7),
    )
    .expect("step output");
    let quick_fix = step
        .get("quickFixes")
        .and_then(Value::as_array)
        .and_then(|arr| arr.first())
        .cloned()
        .expect("quick fix");

    let edit: WorkspaceEdit = serde_json::from_value(
        quick_fix
            .get("edit")
            .cloned()
            .expect("versioned quick-fix edit"),
    )
    .expect("workspace edit");
    let Some(lsp_types::DocumentChanges::Edits(document_edits)) = &edit.document_changes else {
        panic!("expected versioned document edits: {edit:#?}");
    };
    assert!(document_edits.iter().any(|document_edit| {
        document_edit.text_document.uri == uri && document_edit.text_document.version == Some(7)
    }));
    let ready =
        rex_lsp::queries::execute_semantic_loop_apply_quick_fix(&uri, text, Some(7), &quick_fix)
            .expect("ready apply output");
    assert_eq!(ready.get("status").and_then(Value::as_str), Some("ready"));

    let out =
        rex_lsp::queries::execute_semantic_loop_apply_quick_fix(&uri, text, Some(8), &quick_fix)
            .expect("apply output");
    assert_eq!(out.get("status").and_then(Value::as_str), Some("stale"));
    assert_eq!(
        out.get("reason").and_then(Value::as_str),
        Some("documentVersionChanged")
    );
}

#[test]
fn semantic_loop_quick_fix_discovery_is_deterministic_across_sessions() {
    let uri = in_memory_doc_uri();
    let text = r#"
fn mk : i32 -> i32 = \x -> x;
let y : i32 = ? in y
"#;
    let position = Position {
        line: 2,
        character: 14,
    };
    let expected = execute_semantic_loop_step(&uri, text, position)
        .and_then(|step| step.get("quickFixes").cloned())
        .expect("initial quick fixes");

    for _ in 0..8 {
        let actual = execute_semantic_loop_step(&uri, text, position)
            .and_then(|step| step.get("quickFixes").cloned())
            .expect("repeated quick fixes");
        assert_eq!(actual, expected);
    }
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
fn golden_flow_hole_to_apply_proposal_reduces_hole_count() {
    let uri = in_memory_doc_uri();
    let text = r#"
fn mk : i32 -> i32 = \x -> x;
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
    let quick_fix = step
        .get("quickFixes")
        .and_then(Value::as_array)
        .and_then(|arr| {
            arr.iter().find(|item| {
                item.get("title")
                    .and_then(Value::as_str)
                    .is_some_and(|title| title == "Fill hole with `mk`")
            })
        })
        .cloned()
        .expect("hole fill quick-fix");
    let apply =
        execute_semantic_loop_apply_quick_fix(&uri, text, &quick_fix).expect("apply output");
    assert_eq!(apply.get("status").and_then(Value::as_str), Some("ready"));
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
  match y with { case Some x -> x; }
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
    let aggressive = best_quick_fix_from_candidates(&candidates, BulkQuickFixStrategy::Aggressive)
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
  match y with { case Some x -> x; }
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
  match y with { case Some x -> x; }
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

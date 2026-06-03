#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use std::borrow::Cow;

use futures::executor::block_on;
use rex_ast::CompilationUnit;
use rex_engine::{CompileOptions, Engine, ValueDisplayOptions, standard_type_system};
use rex_lsp::public::{
    code_actions_for_source_public, completion_for_source, diagnostics_for_source,
    document_symbols_for_source_public, format_for_source_public, goto_definition_for_source,
    hover_for_source, references_for_source_public, rename_for_source_public,
};
use rex_parser::{error::ParseError, parse};
use rex_typesystem::{inference::infer, typesystem::TypeSystemLimits};
use wasm_bindgen::prelude::*;

fn parse_program(source: &str) -> Result<CompilationUnit, String> {
    parse(source).map_err(|errs| format_parse_errors(&errs))
}

fn source_with_docs_body(source: &str) -> Result<Cow<'_, str>, String> {
    let program = parse_program(source)?;
    if program.body.is_some() {
        return Ok(Cow::Borrowed(source));
    }
    // Interactive docs examples often demonstrate declarations by themselves.
    // Treat those as snippets whose final expression is unit without changing
    // the source text shown in the editor.
    Ok(Cow::Owned(format!("{source}\n()")))
}

fn format_parse_errors(errs: &[ParseError]) -> String {
    let mut out = String::from("parse error:");
    for err in errs {
        out.push('\n');
        out.push_str("  ");
        out.push_str(&err.to_string());
    }
    out
}

pub fn parse_to_json(source: &str) -> Result<String, String> {
    let program = parse_program(source)?;
    serde_json::to_string(&program).map_err(|e| format!("serialization error: {e}"))
}

pub fn infer_to_json(source: &str) -> Result<String, String> {
    let source = source_with_docs_body(source)?;
    let program = parse_program(source.as_ref())?;

    let mut ts = standard_type_system().map_err(|e| format!("type system error: {e}"))?;
    ts.set_limits(TypeSystemLimits::safe_defaults());
    ts.register_decls(&program.decls)
        .map_err(|e| format!("type declaration error: {e}"))?;

    let Some(body) = program.body.as_ref() else {
        return Err("missing final expression".to_string());
    };
    let (preds, typ) = infer(&mut ts, body.as_ref()).map_err(|e| format!("type error: {e}"))?;

    let payload = serde_json::json!({
        "type": typ.to_string(),
        "predicates": preds
            .iter()
            .map(|p| format!("{} {}", p.class, p.typ))
            .collect::<Vec<_>>(),
    });
    serde_json::to_string(&payload).map_err(|e| format!("serialization error: {e}"))
}

pub fn lsp_diagnostics_to_json(source: &str) -> Result<String, String> {
    let diagnostics = match source_with_docs_body(source) {
        Ok(source) => diagnostics_for_source(source.as_ref()),
        Err(_) => diagnostics_for_source(source),
    };
    serde_json::to_string(&diagnostics).map_err(|e| format!("serialization error: {e}"))
}

pub fn lsp_completions_to_json(source: &str, line: u32, character: u32) -> Result<String, String> {
    let completions = completion_for_source(source, line, character);
    serde_json::to_string(&completions).map_err(|e| format!("serialization error: {e}"))
}

pub fn lsp_hover_to_json(source: &str, line: u32, character: u32) -> Result<String, String> {
    let hover = hover_for_source(source, line, character);
    serde_json::to_string(&hover).map_err(|e| format!("serialization error: {e}"))
}

pub fn lsp_goto_definition_to_json(
    source: &str,
    line: u32,
    character: u32,
) -> Result<String, String> {
    let location = goto_definition_for_source(source, line, character);
    serde_json::to_string(&location).map_err(|e| format!("serialization error: {e}"))
}

pub fn lsp_references_to_json(
    source: &str,
    line: u32,
    character: u32,
    include_declaration: bool,
) -> Result<String, String> {
    let refs = references_for_source_public(source, line, character, include_declaration);
    serde_json::to_string(&refs).map_err(|e| format!("serialization error: {e}"))
}

pub fn lsp_rename_to_json(
    source: &str,
    line: u32,
    character: u32,
    new_name: &str,
) -> Result<String, String> {
    let edit = rename_for_source_public(source, line, character, new_name);
    serde_json::to_string(&edit).map_err(|e| format!("serialization error: {e}"))
}

pub fn lsp_document_symbols_to_json(source: &str) -> Result<String, String> {
    let symbols = document_symbols_for_source_public(source);
    serde_json::to_string(&symbols).map_err(|e| format!("serialization error: {e}"))
}

pub fn lsp_format_to_json(source: &str) -> Result<String, String> {
    let edits = format_for_source_public(source);
    serde_json::to_string(&edits).map_err(|e| format!("serialization error: {e}"))
}

pub fn lsp_code_actions_to_json(source: &str, line: u32, character: u32) -> Result<String, String> {
    let actions = code_actions_for_source_public(source, line, character);
    serde_json::to_string(&actions).map_err(|e| format!("serialization error: {e}"))
}

pub async fn eval_to_string(source: &str) -> Result<String, String> {
    let source = source_with_docs_body(source)?;

    let mut engine = Engine::with_prelude(()).map_err(|e| format!("engine init error: {e}"))?;
    engine.type_system.set_limits(TypeSystemLimits::unlimited());
    // Match CLI semantics by evaluating snippets through module/snippet rewriting.
    // This avoids behavior differences between the native CLI and wasm playground.
    let parsed = parse_program(source.as_ref())?;
    let mut compiler = engine.into_compiler();
    let program = compiler
        .compile_program(&parsed, CompileOptions::default())
        .await
        .map_err(|e| format!("runtime error: {e}"))?;
    let value = compiler
        .into_evaluator()
        .run(program, Default::default())
        .await
        .map_err(|e| format!("runtime error: {e}"))?;

    value
        .display_with(ValueDisplayOptions::docs())
        .map_err(|e| format!("display error: {e}"))
}

fn as_js_err(err: String) -> JsValue {
    JsValue::from_str(&err)
}

#[wasm_bindgen(js_name = parseToJson)]
pub fn wasm_parse_to_json(source: &str) -> Result<String, JsValue> {
    parse_to_json(source).map_err(as_js_err)
}

#[wasm_bindgen(js_name = inferToJson)]
pub fn wasm_infer_to_json(source: &str) -> Result<String, JsValue> {
    infer_to_json(source).map_err(as_js_err)
}

#[wasm_bindgen(js_name = lspDiagnosticsToJson)]
pub fn wasm_lsp_diagnostics_to_json(source: &str) -> Result<String, JsValue> {
    lsp_diagnostics_to_json(source).map_err(as_js_err)
}

#[wasm_bindgen(js_name = lspCompletionsToJson)]
pub fn wasm_lsp_completions_to_json(
    source: &str,
    line: u32,
    character: u32,
) -> Result<String, JsValue> {
    lsp_completions_to_json(source, line, character).map_err(as_js_err)
}

#[wasm_bindgen(js_name = lspHoverToJson)]
pub fn wasm_lsp_hover_to_json(source: &str, line: u32, character: u32) -> Result<String, JsValue> {
    lsp_hover_to_json(source, line, character).map_err(as_js_err)
}

#[wasm_bindgen(js_name = lspGotoDefinitionToJson)]
pub fn wasm_lsp_goto_definition_to_json(
    source: &str,
    line: u32,
    character: u32,
) -> Result<String, JsValue> {
    lsp_goto_definition_to_json(source, line, character).map_err(as_js_err)
}

#[wasm_bindgen(js_name = lspReferencesToJson)]
pub fn wasm_lsp_references_to_json(
    source: &str,
    line: u32,
    character: u32,
    include_declaration: bool,
) -> Result<String, JsValue> {
    lsp_references_to_json(source, line, character, include_declaration).map_err(as_js_err)
}

#[wasm_bindgen(js_name = lspRenameToJson)]
pub fn wasm_lsp_rename_to_json(
    source: &str,
    line: u32,
    character: u32,
    new_name: &str,
) -> Result<String, JsValue> {
    lsp_rename_to_json(source, line, character, new_name).map_err(as_js_err)
}

#[wasm_bindgen(js_name = lspDocumentSymbolsToJson)]
pub fn wasm_lsp_document_symbols_to_json(source: &str) -> Result<String, JsValue> {
    lsp_document_symbols_to_json(source).map_err(as_js_err)
}

#[wasm_bindgen(js_name = lspFormatToJson)]
pub fn wasm_lsp_format_to_json(source: &str) -> Result<String, JsValue> {
    lsp_format_to_json(source).map_err(as_js_err)
}

#[wasm_bindgen(js_name = lspCodeActionsToJson)]
pub fn wasm_lsp_code_actions_to_json(
    source: &str,
    line: u32,
    character: u32,
) -> Result<String, JsValue> {
    lsp_code_actions_to_json(source, line, character).map_err(as_js_err)
}

#[wasm_bindgen(js_name = evalToJson)]
pub fn wasm_eval_to_json(source: &str) -> Result<String, JsValue> {
    let source = source_with_docs_body(source).map_err(as_js_err)?;

    let fut = async move {
        let engine = Engine::with_prelude(()).map_err(|e| format!("engine init error: {e}"))?;
        let parsed = parse_program(source.as_ref())?;
        let mut compiler = engine.into_compiler();
        let program = compiler
            .compile_program(&parsed, CompileOptions::default())
            .await
            .map_err(|e| format!("runtime error: {e}"))?;
        let value = compiler
            .into_evaluator()
            .run(program, Default::default())
            .await
            .map_err(|e| format!("runtime error: {e}"))?;
        let rendered = value
            .display_with(ValueDisplayOptions::unsanitized())
            .map_err(|e| format!("display error: {e}"))?;
        let payload = serde_json::json!({ "value": rendered });
        serde_json::to_string(&payload).map_err(|e| format!("serialization error: {e}"))
    };
    block_on(fut).map_err(as_js_err)
}

#[wasm_bindgen(js_name = evalToString)]
pub fn wasm_eval_to_string(source: &str) -> Result<String, JsValue> {
    block_on(eval_to_string(source)).map_err(as_js_err)
}

#[cfg(test)]
mod tests {
    use super::{
        eval_to_string, infer_to_json, lsp_code_actions_to_json, lsp_diagnostics_to_json,
        wasm_eval_to_json,
    };
    use futures::executor::block_on;

    #[test]
    fn eval_to_string_hides_snippet_prefix_and_numeric_suffix() {
        let source = r#"
type T = A | B;
let
  x = A,
  n = 2
in
  (n, [x, B])
"#;
        let full = wasm_eval_to_json(source).expect("wasm eval failed");
        assert!(full.contains("2i32"));
        assert!(full.contains("A"));
        assert!(full.contains("B"));

        let sanitized = block_on(eval_to_string(source)).expect("wasm string eval failed");
        assert_eq!(sanitized, "(2, [A, B])");
    }

    #[test]
    fn declaration_only_docs_source_runs_as_unit() {
        let source = "fn inc x: i32 -> i32 = x + 1;";

        let json = infer_to_json(source).expect("docs source should infer");
        let inferred: serde_json::Value = serde_json::from_str(&json).expect("type json");
        assert_eq!(inferred["type"], "()");

        let diagnostics = lsp_diagnostics_to_json(source).expect("diagnostics json");
        let diagnostics: serde_json::Value =
            serde_json::from_str(&diagnostics).expect("diagnostics parse");
        assert_eq!(
            diagnostics.as_array().expect("diagnostics array").len(),
            0,
            "diagnostics: {diagnostics:#?}"
        );

        let full = wasm_eval_to_json(source).expect("wasm eval failed");
        assert!(full.contains("()"));

        let sanitized = block_on(eval_to_string(source)).expect("wasm string eval failed");
        assert_eq!(sanitized, "()");
    }

    #[test]
    fn lsp_diagnostics_preserve_all_unknown_var_usages() {
        let source = r#"
let
  f = \x -> missing + x
in
  missing + (f missing)
"#;
        let json = lsp_diagnostics_to_json(source).expect("diagnostics json");
        let diagnostics: serde_json::Value =
            serde_json::from_str(&json).expect("diagnostics parse");
        let count = diagnostics
            .as_array()
            .expect("diagnostics array")
            .iter()
            .filter(|diag| {
                diag.get("message")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|m| m.contains("unbound variable missing"))
            })
            .count();
        assert_eq!(count, 3, "diagnostics: {diagnostics:#?}");
    }

    #[test]
    fn lsp_code_actions_include_unknown_var_fixes() {
        let source = r#"
let
  x = 1
in
  y + x
"#;
        let json = lsp_code_actions_to_json(source, 4, 2).expect("code actions json");
        let actions: serde_json::Value = serde_json::from_str(&json).expect("actions parse");
        let has_replace = actions
            .as_array()
            .expect("actions array")
            .iter()
            .any(|item| {
                item.get("title")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|title| title.contains("Replace `y` with `x`"))
            });
        assert!(has_replace, "actions: {actions:#?}");
    }
}

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use futures::executor::block_on;
use rex_engine::{Engine, EngineError, ValueDisplayOptions, pointer_display_with};
use rex_lexer::Token;
use rex_lsp::{
    completion_for_source, diagnostics_for_source, document_symbols_for_source_public,
    format_for_source_public, goto_definition_for_source, hover_for_source,
    references_for_source_public, rename_for_source_public,
};
use rex_parser::{Parser, ParserLimits, error::ParserErr};
use rex_ts::{TypeError as TsTypeError, TypeSystem, TypeSystemLimits};
use rex_util::{GasCosts, GasMeter};
use wasm_bindgen::prelude::*;

const DEFAULT_GAS_LIMIT: u64 = 5_000_000;

fn new_gas(limit: Option<u64>) -> GasMeter {
    GasMeter::new(
        Some(limit.unwrap_or(DEFAULT_GAS_LIMIT)),
        GasCosts::sensible_defaults(),
    )
}

fn new_unlimited_gas() -> GasMeter {
    GasMeter::unlimited(GasCosts::sensible_defaults())
}

fn parse_program_with_limits(
    source: &str,
    gas: &mut GasMeter,
    limits: ParserLimits,
) -> Result<rex_ast::expr::Program, String> {
    let tokens = Token::tokenize(source).map_err(|e| format!("lex error: {e}"))?;
    let mut parser = Parser::new(tokens);
    parser.set_limits(limits);
    parser
        .parse_program(gas)
        .map_err(|errs| format_parse_errors(&errs))
}

fn format_parse_errors(errs: &[ParserErr]) -> String {
    let mut out = String::from("parse error:");
    for err in errs {
        out.push('\n');
        out.push_str("  ");
        out.push_str(&err.to_string());
    }
    out
}

pub fn parse_to_json(source: &str, gas_limit: Option<u64>) -> Result<String, String> {
    let mut gas = new_gas(gas_limit);
    let program = parse_program_with_limits(source, &mut gas, ParserLimits::safe_defaults())?;
    serde_json::to_string(&program).map_err(|e| format!("serialization error: {e}"))
}

pub fn infer_to_json(source: &str, gas_limit: Option<u64>) -> Result<String, String> {
    let mut gas = new_gas(gas_limit);
    let program = parse_program_with_limits(source, &mut gas, ParserLimits::safe_defaults())?;

    let mut ts = TypeSystem::with_prelude().map_err(|e| format!("type system error: {e}"))?;
    ts.set_limits(TypeSystemLimits::safe_defaults());
    ts.inject_decls(&program.decls)
        .map_err(|e| format!("type declaration error: {e}"))?;

    let (preds, typ) = ts
        .infer_with_gas(program.expr.as_ref(), &mut gas)
        .map_err(|e| format!("type error: {e}"))?;

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
    let mut diagnostics = diagnostics_for_source(source);
    // Replace rex-lsp type diagnostics with type diagnostics from the same
    // engine snippet inference path used by evaluation.
    diagnostics.retain(|d| d.source.as_deref() != Some("rex-ts"));

    let mut payload =
        serde_json::to_value(diagnostics).map_err(|e| format!("serialization error: {e}"))?;
    let arr = payload
        .as_array_mut()
        .ok_or_else(|| "serialization error: expected diagnostics array".to_string())?;
    arr.extend(engine_type_diagnostics_json(source));
    serde_json::to_string(&payload).map_err(|e| format!("serialization error: {e}"))
}

fn engine_type_diagnostics_json(source: &str) -> Vec<serde_json::Value> {
    let mut engine = match Engine::with_prelude(()) {
        Ok(engine) => engine,
        Err(err) => return vec![diagnostic_json(None, format!("engine init error: {err}"))],
    };
    engine.type_system.set_limits(TypeSystemLimits::unlimited());
    let mut gas = GasMeter::unlimited(GasCosts::sensible_defaults());

    match engine.infer_snippet(source, &mut gas) {
        Ok(_) => Vec::new(),
        Err(EngineError::Type(err)) => vec![type_error_diagnostic_json(&err)],
        Err(err) => vec![diagnostic_json(None, format!("type error: {err}"))],
    }
}

fn type_error_diagnostic_json(err: &TsTypeError) -> serde_json::Value {
    match err {
        TsTypeError::Spanned { span, error } => diagnostic_json(Some(*span), error.to_string()),
        other => diagnostic_json(None, other.to_string()),
    }
}

fn diagnostic_json(span: Option<rex_lexer::span::Span>, message: String) -> serde_json::Value {
    let (start_line, start_char, end_line, end_char) = if let Some(span) = span {
        (
            span.begin.line.saturating_sub(1) as u32,
            span.begin.column.saturating_sub(1) as u32,
            span.end.line.saturating_sub(1) as u32,
            span.end.column.saturating_sub(1) as u32,
        )
    } else {
        (0, 0, 0, 1)
    };

    serde_json::json!({
        "range": {
            "start": { "line": start_line, "character": start_char },
            "end": { "line": end_line, "character": end_char }
        },
        "severity": 1,
        "message": message,
        "source": "rex-ts"
    })
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

pub async fn eval_to_string(source: &str, gas_limit: Option<u64>) -> Result<String, String> {
    let mut gas = if gas_limit.is_some() {
        new_gas(gas_limit)
    } else {
        new_unlimited_gas()
    };
    let _ = parse_program_with_limits(source, &mut gas, ParserLimits::unlimited())?;

    let mut engine = Engine::with_prelude(()).map_err(|e| format!("engine init error: {e}"))?;
    engine
        .type_system
        .set_limits(rex_ts::TypeSystemLimits::unlimited());
    // Match CLI semantics by evaluating snippets through module/snippet rewriting.
    // This avoids behavior differences between native `rex run` and wasm playground.
    let (value_ptr, _value_ty) = engine
        .eval_snippet(source, &mut gas)
        .await
        .map_err(|e| format!("runtime error: {e}"))?;

    pointer_display_with(&engine.heap, &value_ptr, ValueDisplayOptions::docs())
        .map_err(|e| format!("display error: {e}"))
}

fn as_js_err(err: String) -> JsValue {
    JsValue::from_str(&err)
}

#[wasm_bindgen(js_name = parseToJson)]
pub fn wasm_parse_to_json(source: &str, gas_limit: Option<u64>) -> Result<String, JsValue> {
    parse_to_json(source, gas_limit).map_err(as_js_err)
}

#[wasm_bindgen(js_name = inferToJson)]
pub fn wasm_infer_to_json(source: &str, gas_limit: Option<u64>) -> Result<String, JsValue> {
    infer_to_json(source, gas_limit).map_err(as_js_err)
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

#[wasm_bindgen(js_name = evalToJson)]
pub fn wasm_eval_to_json(source: &str, gas_limit: Option<u64>) -> Result<String, JsValue> {
    let mut gas = if gas_limit.is_some() {
        new_gas(gas_limit)
    } else {
        new_unlimited_gas()
    };
    let _ = parse_program_with_limits(source, &mut gas, ParserLimits::unlimited())
        .map_err(as_js_err)?;

    let fut = async move {
        let mut engine = Engine::with_prelude(()).map_err(|e| format!("engine init error: {e}"))?;
        let (value_ptr, _value_ty) = engine
            .eval_snippet(source, &mut gas)
            .await
            .map_err(|e| format!("runtime error: {e}"))?;
        let rendered =
            pointer_display_with(&engine.heap, &value_ptr, ValueDisplayOptions::unsanitized())
                .map_err(|e| format!("display error: {e}"))?;
        let payload = serde_json::json!({ "value": rendered });
        serde_json::to_string(&payload).map_err(|e| format!("serialization error: {e}"))
    };
    block_on(fut).map_err(as_js_err)
}

#[wasm_bindgen(js_name = evalToString)]
pub fn wasm_eval_to_string(source: &str, gas_limit: Option<u64>) -> Result<String, JsValue> {
    block_on(eval_to_string(source, gas_limit)).map_err(as_js_err)
}

#[cfg(test)]
mod tests {
    use super::{eval_to_string, wasm_eval_to_json};
    use futures::executor::block_on;

    #[test]
    fn eval_to_string_hides_snippet_prefix_and_numeric_suffix() {
        let source = r#"
type T = A | B
let
  x = A,
  n = 2
in
  (n, [x, B])
"#;
        let full = wasm_eval_to_json(source, None).expect("wasm eval failed");
        assert!(full.contains("2i32"));
        assert!(full.contains("@snippet"));

        let sanitized = block_on(eval_to_string(source, None)).expect("wasm string eval failed");
        assert_eq!(sanitized, "(2, [A, B])");
    }
}

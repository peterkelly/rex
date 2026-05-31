use crate::prelude::*;
use crate::{
    code_actions::*, completion::*, diagnostics::*, document::*, navigation::*, queries::*,
    shared::*,
};

pub fn in_memory_doc_uri() -> Url {
    match Url::parse("inmemory:///docs.rex") {
        Ok(url) => url,
        Err(_) => panic!("static in-memory URI must parse"),
    }
}

pub fn diagnostics_for_source(source: &str) -> Vec<Diagnostic> {
    let uri = in_memory_doc_uri();
    let session = AnalysisSession::isolated();
    diagnostics_from_text(&session, &uri, source)
}

pub fn completion_for_source(source: &str, line: u32, character: u32) -> Vec<CompletionItem> {
    let uri = in_memory_doc_uri();
    let session = AnalysisSession::isolated();
    completion_items(&session, &uri, source, Position { line, character })
}

pub fn hover_for_source(source: &str, line: u32, character: u32) -> Option<Hover> {
    let uri = in_memory_doc_uri();
    let session = AnalysisSession::isolated();
    let position = Position { line, character };
    let contents = hover_type_contents(&session, &uri, source, position).or_else(|| {
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
    let session = AnalysisSession::isolated();
    expected_type_at_position(&session, &uri, source, Position { line, character })
}

pub fn functions_producing_expected_type_for_source_public(
    source: &str,
    line: u32,
    character: u32,
) -> Vec<String> {
    let uri = in_memory_doc_uri();
    let session = AnalysisSession::isolated();
    functions_producing_expected_type_at_position(
        &session,
        &uri,
        source,
        Position { line, character },
    )
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
    let session = AnalysisSession::isolated();
    references_for_source(
        &session,
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
    let session = AnalysisSession::isolated();
    rename_for_source(
        &session,
        &uri,
        source,
        Position { line, character },
        new_name,
    )
}

pub fn document_symbols_for_source_public(source: &str) -> Vec<DocumentSymbol> {
    let uri = in_memory_doc_uri();
    let session = AnalysisSession::isolated();
    document_symbols_for_source(&session, &uri, source)
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
    let session = AnalysisSession::isolated();
    let position = Position { line, character };
    let range = Range {
        start: position,
        end: position,
    };
    let diagnostics: Vec<Diagnostic> = diagnostics_from_text(&session, &uri, source)
        .into_iter()
        .filter(|diag| {
            range_contains_position(diag.range, position)
                || range_touches_position(diag.range, position)
        })
        .collect();
    code_actions_for_source(&session, &uri, source, range, &diagnostics)
}

pub fn goto_definition_for_source(source: &str, line: u32, character: u32) -> Option<Location> {
    let uri = in_memory_doc_uri();
    let session = AnalysisSession::isolated();
    let pos = Position { line, character };
    let response = goto_definition_response(&session, &uri, source, pos)?;
    match response {
        GotoDefinitionResponse::Scalar(location) => Some(location),
        GotoDefinitionResponse::Array(locations) => locations.into_iter().next(),
        GotoDefinitionResponse::Link(links) => links.into_iter().next().map(|link| Location {
            uri: link.target_uri,
            range: link.target_range,
        }),
    }
}

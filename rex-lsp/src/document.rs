use crate::prelude::*;
use crate::{completion::*, shared::*};

#[allow(deprecated)]
pub(crate) fn symbol_for_decl(decl: &Decl) -> Option<DocumentSymbol> {
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

pub(crate) fn document_symbols_for_source(
    session: &AnalysisSession,
    uri: &Url,
    text: &str,
) -> Vec<DocumentSymbol> {
    let Ok((_tokens, program)) = session.tokenize_and_parse_cached(uri, text) else {
        return Vec::new();
    };
    program.decls.iter().filter_map(symbol_for_decl).collect()
}

pub fn full_document_range(text: &str) -> Range {
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

pub(crate) fn format_source(text: &str) -> String {
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

pub(crate) fn format_edits_for_source(text: &str) -> Option<Vec<TextEdit>> {
    let formatted = format_source(text);
    if formatted == text {
        return None;
    }
    Some(vec![TextEdit {
        range: full_document_range(text),
        new_text: formatted,
    }])
}

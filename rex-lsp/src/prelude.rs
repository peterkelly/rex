pub(crate) use std::collections::{BTreeSet, HashMap, HashSet};
pub(crate) use std::fs;
pub(crate) use std::hash::{Hash, Hasher};
pub(crate) use std::path::{Path, PathBuf};
pub(crate) use std::sync::{Arc, Mutex, OnceLock};

pub(crate) use futures::future::BoxFuture;
pub(crate) use lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, CompletionItem, CompletionItemKind,
    Diagnostic, DiagnosticSeverity, DocumentSymbol, GotoDefinitionResponse, Hover, HoverContents,
    Location, MarkupContent, MarkupKind, Position, Range, SymbolKind, TextEdit, Url, WorkspaceEdit,
};
pub(crate) use rex_ast::{
    CompilationUnit, Decl, Expr, ImportDecl, ImportPath, Pattern, Symbol, TypeDecl, TypeExpr,
};
pub(crate) use rex_ast::{Position as RexPosition, Span, Spanned};
pub(crate) use rex_engine::{
    Builder, EngineError, ImportRequest, Importer, ModuleError, ModuleExports, ModuleId,
    ResolvedModule, ResolvedModuleContent,
    module_semantics::{
        ImportBindingPolicy, ImportBindings, ModuleLoadState, add_import_bindings,
        collect_pattern_bindings, decl_type_names, decl_value_names, exports_from_program,
        load_import_for_tooling, prefix_for_module, rewrite_import_uses,
        validate_import_uses_with_spans,
    },
    standard_type_system,
};
pub(crate) use rex_parser::{
    error::ParseError,
    lexer::{LexicalError, Token, Tokens},
    parse_with_tokens,
};
pub(crate) use rex_typesystem::{
    error::TypeError as TsTypeError,
    inference::infer_typed,
    types::{BuiltinTypeId, Scheme, Type, TypeKind, TypedExpr, TypedExprKind, Types},
    typesystem::{PreparedInstanceDecl, TypeSystem, instantiate},
    unification::unify,
};
pub(crate) use rex_util::{resolve_local_import_path, sha256_hex};
pub(crate) use serde_json::{Value, json, to_value};

pub(crate) use crate::{
    BUILTIN_TYPES, BUILTIN_VALUES, CMD_ADAPTERS_FROM_INFERRED_TO_EXPECTED_AT, CMD_EXPECTED_TYPE_AT,
    CMD_FUNCTIONS_ACCEPTING_INFERRED_TYPE_AT, CMD_FUNCTIONS_COMPATIBLE_WITH_IN_SCOPE_VALUES_AT,
    CMD_FUNCTIONS_PRODUCING_EXPECTED_TYPE_AT, CMD_HOLES_EXPECTED_TYPES, MAX_DIAGNOSTICS,
    MAX_SEMANTIC_CANDIDATES, MAX_SEMANTIC_ENV_SCHEMES_SCAN, MAX_SEMANTIC_HOLE_FILL_ARITY,
    MAX_SEMANTIC_HOLES, MAX_SEMANTIC_IN_SCOPE_VALUES, NO_IMPROVEMENT_STREAK_LIMIT,
};

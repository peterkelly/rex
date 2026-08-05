use std::collections::HashMap;

use lsp_types::{
    CodeActionKind, CompletionResponse, DocumentSymbolResponse, GotoDefinitionResponse, Hover,
    Location, TextEdit, Url, WorkspaceEdit,
};
use serde_json::{Value, json};
use tokio::sync::RwLock;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::{
    CodeActionOptions, CodeActionParams, CodeActionProviderCapability, CodeActionResponse,
    CompletionOptions, CompletionParams, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DocumentFormattingParams, DocumentSymbolParams,
    ExecuteCommandOptions, ExecuteCommandParams, GotoDefinitionParams, HoverParams,
    HoverProviderCapability, InitializeParams, InitializeResult, InitializedParams, MessageType,
    OneOf, ReferenceParams, RenameParams, ServerCapabilities, ServerInfo,
    TextDocumentSyncCapability, TextDocumentSyncKind,
};
use tower_lsp::{Client, LanguageServer, LspService, Server};

use crate::{
    CMD_ADAPTERS_FROM_INFERRED_TO_EXPECTED_AT, CMD_EXPECTED_TYPE_AT,
    CMD_FUNCTIONS_ACCEPTING_INFERRED_TYPE_AT, CMD_FUNCTIONS_COMPATIBLE_WITH_IN_SCOPE_VALUES_AT,
    CMD_FUNCTIONS_PRODUCING_EXPECTED_TYPE_AT, CMD_HOLES_EXPECTED_TYPES,
    CMD_SEMANTIC_LOOP_APPLY_BEST_QUICK_FIXES_AT, CMD_SEMANTIC_LOOP_APPLY_QUICK_FIX_AT,
    CMD_SEMANTIC_LOOP_STEP,
    code_actions::code_actions_for_source,
    completion::{completion_items, hover_contents, word_at_position},
    diagnostics::diagnostics_from_text,
    document::{document_symbols_for_source, format_edits_for_source},
    navigation::{goto_definition_response, references_for_source, rename_for_source},
    queries::{
        command_uri, command_uri_and_position, command_uri_and_quick_fix,
        command_uri_position_max_steps_strategy_and_dry_run, execute_query_command_for_document,
        execute_query_command_for_document_without_position,
        execute_semantic_loop_apply_best_quick_fixes, execute_semantic_loop_apply_quick_fix,
        execute_semantic_loop_step_with_version, hover_type_contents,
    },
    shared::{AnalysisSession, AnalysisState},
};

#[derive(Clone)]
struct DocumentState {
    text: String,
    version: i32,
}

struct RexServer {
    client: Client,
    documents: RwLock<HashMap<Url, DocumentState>>,
    analysis: AnalysisState,
}

impl RexServer {
    fn new(client: Client) -> Self {
        Self {
            client,
            documents: RwLock::new(HashMap::new()),
            analysis: AnalysisState::new(),
        }
    }

    async fn document_with_session(&self, uri: &Url) -> Option<(String, i32, AnalysisSession)> {
        let documents = self.documents.read().await;
        let document = documents.get(uri)?.clone();
        let open_documents = documents
            .iter()
            .map(|(uri, document)| (uri.clone(), document.text.clone()))
            .collect();
        Some((
            document.text,
            document.version,
            self.analysis.session(open_documents),
        ))
    }

    async fn publish_diagnostics(&self, uri: Url, text: &str) {
        let uri_for_job = uri.clone();
        let text_for_job = text.to_string();
        let session = {
            let documents = self.documents.read().await;
            let open_documents = documents
                .iter()
                .map(|(uri, document)| (uri.clone(), document.text.clone()))
                .collect();
            self.analysis.session(open_documents)
        };
        let diagnostics = match tokio::task::spawn_blocking(move || {
            diagnostics_from_text(&session, &uri_for_job, &text_for_job)
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
                code_action_provider: Some(CodeActionProviderCapability::Options(
                    CodeActionOptions {
                        code_action_kinds: Some(vec![CodeActionKind::QUICKFIX]),
                        ..CodeActionOptions::default()
                    },
                )),
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
        let version = params.text_document.version;

        self.documents.write().await.insert(
            uri.clone(),
            DocumentState {
                text: text.clone(),
                version,
            },
        );
        self.analysis.clear_parse_cache(&uri);
        self.publish_diagnostics(uri, &text).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        let version = params.text_document.version;
        let text = params
            .content_changes
            .into_iter()
            .last()
            .map(|change| change.text);

        if let Some(text) = text {
            self.documents.write().await.insert(
                uri.clone(),
                DocumentState {
                    text: text.clone(),
                    version,
                },
            );
            self.analysis.clear_parse_cache(&uri);
            self.publish_diagnostics(uri, &text).await;
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.documents.write().await.remove(&uri);
        self.analysis.clear_parse_cache(&uri);
        self.client.publish_diagnostics(uri, Vec::new(), None).await;
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let Some((text, _version, session)) = self.document_with_session(&uri).await else {
            return Ok(None);
        };

        let uri_for_job = uri.clone();
        let text_for_job = text.clone();
        let type_contents = match tokio::task::spawn_blocking(move || {
            hover_type_contents(&session, &uri_for_job, &text_for_job, position)
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
        let Some((text, _version, session)) = self.document_with_session(&uri).await else {
            return Ok(None);
        };

        let uri_for_job = uri.clone();
        let text_for_job = text;
        let items = match tokio::task::spawn_blocking(move || {
            completion_items(&session, &uri_for_job, &text_for_job, position)
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
        let Some((text, _version, session)) = self.document_with_session(&uri).await else {
            return Ok(None);
        };

        let range = params.range;
        let diagnostics = params.context.diagnostics;
        let uri_for_job = uri.clone();
        let text_for_job = text;
        let actions = match tokio::task::spawn_blocking(move || {
            code_actions_for_source(&session, &uri_for_job, &text_for_job, range, &diagnostics)
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
            let Some((text, _version, session)) = self.document_with_session(&uri).await else {
                return Ok(None);
            };
            return Ok(execute_query_command_for_document_without_position(
                &session, &command, &uri, &text,
            ));
        }
        if command == CMD_SEMANTIC_LOOP_STEP {
            let Some((uri, position)) = command_uri_and_position(&arguments) else {
                return Ok(None);
            };
            let Some((text, version, session)) = self.document_with_session(&uri).await else {
                return Ok(None);
            };
            return Ok(execute_semantic_loop_step_with_version(
                &session,
                &uri,
                &text,
                position,
                Some(version),
            ));
        }
        if command == CMD_SEMANTIC_LOOP_APPLY_QUICK_FIX_AT {
            let Some((uri, quick_fix)) = command_uri_and_quick_fix(&arguments) else {
                return Ok(Some(json!({
                    "status": "rejected",
                    "reason": "invalidArguments",
                    "detail": "expected a document URI and complete quick-fix proposal",
                })));
            };
            let Some((text, version, _session)) = self.document_with_session(&uri).await else {
                return Ok(Some(json!({
                    "status": "rejected",
                    "reason": "documentUnavailable",
                    "detail": "quick-fix target is not an open document",
                })));
            };
            let Some(validation) =
                execute_semantic_loop_apply_quick_fix(&uri, &text, Some(version), &quick_fix)
            else {
                return Ok(Some(json!({
                    "status": "rejected",
                    "reason": "invalidProposal",
                    "detail": "quick-fix proposal validation produced no result",
                })));
            };
            if validation.get("status").and_then(Value::as_str) != Some("ready") {
                return Ok(Some(validation));
            }
            let Some(edit_value) = quick_fix.get("edit").cloned() else {
                return Ok(Some(json!({
                    "status": "rejected",
                    "reason": "invalidEdit",
                    "detail": "validated quick-fix proposal is missing its edit",
                })));
            };
            let Ok(edit) = serde_json::from_value::<WorkspaceEdit>(edit_value) else {
                return Ok(Some(json!({
                    "status": "rejected",
                    "reason": "invalidEdit",
                    "detail": "validated quick-fix proposal contains an invalid edit",
                })));
            };
            return match self.client.apply_edit(edit).await {
                Ok(response) if response.applied => Ok(Some(json!({
                    "status": "applied",
                    "quickFix": quick_fix,
                }))),
                Ok(response) => Ok(Some(json!({
                    "status": "rejected",
                    "reason": "clientRejectedEdit",
                    "detail": response.failure_reason,
                    "failedChange": response.failed_change,
                }))),
                Err(err) => Ok(Some(json!({
                    "status": "rejected",
                    "reason": "clientApplyEditFailed",
                    "detail": err.to_string(),
                }))),
            };
        }
        if command == CMD_SEMANTIC_LOOP_APPLY_BEST_QUICK_FIXES_AT {
            let Some((uri, position, max_steps, strategy, dry_run)) =
                command_uri_position_max_steps_strategy_and_dry_run(&arguments)
            else {
                return Ok(None);
            };
            let Some((text, _version, session)) = self.document_with_session(&uri).await else {
                return Ok(None);
            };
            return Ok(execute_semantic_loop_apply_best_quick_fixes(
                &session, &uri, &text, position, max_steps, strategy, dry_run,
            ));
        }

        let Some((uri, position)) = command_uri_and_position(&arguments) else {
            return Ok(None);
        };
        let Some((text, _version, session)) = self.document_with_session(&uri).await else {
            return Ok(None);
        };
        Ok(execute_query_command_for_document(
            &session, &command, &uri, &text, position,
        ))
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let Some((text, _version, session)) = self.document_with_session(&uri).await else {
            return Ok(None);
        };

        let uri_for_job = uri.clone();
        let text_for_job = text;
        let response = match tokio::task::spawn_blocking(move || {
            goto_definition_response(&session, &uri_for_job, &text_for_job, position)
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
        let Some((text, _version, session)) = self.document_with_session(&uri).await else {
            return Ok(None);
        };

        let uri_for_job = uri.clone();
        let text_for_job = text;
        let refs = match tokio::task::spawn_blocking(move || {
            references_for_source(
                &session,
                &uri_for_job,
                &text_for_job,
                position,
                include_declaration,
            )
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
        let Some((text, _version, session)) = self.document_with_session(&uri).await else {
            return Ok(None);
        };

        let uri_for_job = uri.clone();
        let text_for_job = text;
        let edit = match tokio::task::spawn_blocking(move || {
            rename_for_source(&session, &uri_for_job, &text_for_job, position, &new_name)
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
        let session = self.analysis.empty_session();
        let symbols = document_symbols_for_source(&session, &uri, &text.text);
        Ok(Some(DocumentSymbolResponse::Nested(symbols)))
    }

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        let uri = params.text_document.uri;
        let text = { self.documents.read().await.get(&uri).cloned() };
        let Some(text) = text else {
            return Ok(None);
        };
        Ok(format_edits_for_source(&text.text))
    }
}

pub async fn run_stdio() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(RexServer::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}

use std::collections::HashMap;

use lsp_types::{
    CodeActionKind, CompletionResponse, DocumentSymbolResponse, GotoDefinitionResponse, Hover,
    Location, TextEdit, Url, WorkspaceEdit,
};
use serde_json::Value;
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

use crate::server::{
    CMD_ADAPTERS_FROM_INFERRED_TO_EXPECTED_AT, CMD_EXPECTED_TYPE_AT,
    CMD_FUNCTIONS_ACCEPTING_INFERRED_TYPE_AT, CMD_FUNCTIONS_COMPATIBLE_WITH_IN_SCOPE_VALUES_AT,
    CMD_FUNCTIONS_PRODUCING_EXPECTED_TYPE_AT, CMD_HOLES_EXPECTED_TYPES,
    CMD_SEMANTIC_LOOP_APPLY_BEST_QUICK_FIXES_AT, CMD_SEMANTIC_LOOP_APPLY_QUICK_FIX_AT,
    CMD_SEMANTIC_LOOP_STEP, clear_parse_cache, code_actions_for_source, command_uri,
    command_uri_and_position, command_uri_position_and_id,
    command_uri_position_max_steps_strategy_and_dry_run, completion_items, diagnostics_from_text,
    document_symbols_for_source, execute_query_command_for_document,
    execute_query_command_for_document_without_position,
    execute_semantic_loop_apply_best_quick_fixes, execute_semantic_loop_apply_quick_fix,
    execute_semantic_loop_step, format_edits_for_source, goto_definition_response, hover_contents,
    hover_type_contents, references_for_source, rename_for_source, word_at_position,
};

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

pub async fn run_stdio() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(RexServer::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}

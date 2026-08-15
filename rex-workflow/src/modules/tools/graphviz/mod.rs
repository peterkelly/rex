mod compile;
pub mod types;

use crate::{modules::tools::executor::ToolExecution, state::State};
use compile::*;
use rex::engine::{EngineError, Module};
use types::*;

type GraphvizResult<T> = Result<T, GraphvizError>;

pub fn module() -> Result<Module<State>, EngineError> {
    api::rex_module()
}

/// Semantic Graphviz rendering for completed graphs assembled as Rex values.
///
/// Nodes have unique string identifiers and final attributes; binary edges connect declared node
/// endpoints; subgraphs group their own nodes and edges. Graph-level node and edge defaults remain
/// available, but order-dependent DOT statements are not exposed. The host validates this model,
/// compiles it to private DOT source, invokes Graphviz, and returns a content-addressed artifact.
#[rex::module(
    name = "tools.graphviz",
    defaults(
        Graph,
        GraphAttributes,
        NodeAttributes,
        EdgeAttributes,
        Subgraph,
        SubgraphAttributes,
        Font,
        NodeSize,
        PolygonOptions,
        EdgeEnd,
        EdgeEndpointLabels,
    )
)]
mod api {
    use super::*;

    /// Render a semantic graph with the selected Graphviz layout engine and output format.
    ///
    /// Undefined nodes, invalid ports, duplicate definitions, invalid subgraph arenas, and values
    /// that cannot be represented safely return `Err GraphvizError` without starting Graphviz. A
    /// nonzero Graphviz exit, including an unavailable output plugin or invalid HTML-like label,
    /// is returned as `ProcessFailed`.
    #[rex::export]
    pub(super) async fn render(
        state: State,
        graph: Graph,
        layout: LayoutEngine,
        format: RenderFormat,
    ) -> Result<GraphvizResult<RenderedGraph>, EngineError> {
        let source = match serialize_graph(&graph) {
            Ok(source) => source,
            Err(error) => return Ok(Err(error)),
        };
        let source =
            state.store.put(source.as_bytes()).await.map_err(|error| {
                EngineError::Custom(format!("store generated DOT source: {error}"))
            })?;
        let execution = execute(&state, render_plan(source, layout, format)).await?;
        if execution.exit_code != Some(0) {
            return Ok(Err(process_error(&execution)));
        }
        match execution.outputs.get(&0).map(Vec::as_slice) {
            Some([content]) => Ok(Ok(RenderedGraph {
                content: *content,
                format,
            })),
            Some(values) => Ok(Err(unexpected(format!(
                "Graphviz produced {} files instead of one",
                values.len()
            )))),
            None => Ok(Err(unexpected("Graphviz did not declare its output"))),
        }
    }

    /// Return the installed Graphviz version reported by `dot -V`.
    #[rex::export]
    pub(super) async fn version(state: State) -> Result<GraphvizResult<VersionInfo>, EngineError> {
        let execution = execute(&state, version_plan()).await?;
        if execution.exit_code != Some(0) {
            return Ok(Err(process_error(&execution)));
        }
        let diagnostics = diagnostics(&execution);
        let first = diagnostics
            .lines()
            .next()
            .ok_or_else(|| EngineError::Custom("dot -V returned no output".into()))?;
        let version = first
            .strip_prefix("dot - graphviz version ")
            .unwrap_or(first)
            .trim()
            .to_owned();
        Ok(Ok(VersionInfo { version }))
    }
}

async fn execute(
    state: &State,
    plan: crate::modules::tools::executor::ToolExecutionPlan,
) -> Result<ToolExecution, EngineError> {
    state
        .tools
        .execute(&state.store, plan)
        .await
        .map_err(|error| EngineError::Custom(error.to_string()))
}

fn diagnostics(execution: &ToolExecution) -> String {
    let mut output = String::from_utf8_lossy(&execution.stderr).into_owned();
    let stdout = String::from_utf8_lossy(&execution.stdout);
    if !stdout.trim().is_empty() {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str(&stdout);
    }
    output.trim().to_owned()
}

fn process_error(execution: &ToolExecution) -> GraphvizError {
    GraphvizError {
        kind: GraphvizErrorKind::ProcessFailed,
        exit_code: execution.exit_code.map(i64::from),
        message: diagnostics(execution),
    }
}

fn unexpected(message: impl Into<String>) -> GraphvizError {
    GraphvizError {
        kind: GraphvizErrorKind::UnexpectedOutput,
        exit_code: None,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::api::*;
    use super::*;
    use crate::storage::store::Store;
    use std::collections::BTreeMap;

    fn simple_graph() -> Graph {
        let endpoint = |value: &str| Endpoint {
            node: value.to_owned(),
            port: None,
        };
        Graph {
            id: Some("workflow".to_owned()),
            nodes: BTreeMap::from([
                ("prepare".to_owned(), NodeAttributes::default()),
                ("render".to_owned(), NodeAttributes::default()),
            ]),
            edges: vec![Edge {
                from: endpoint("prepare"),
                to: endpoint("render"),
                attributes: EdgeAttributes::default(),
            }],
            ..Graph::default()
        }
    }

    #[tokio::test]
    async fn real_graphviz_renders_svg_when_available() {
        if std::process::Command::new("dot")
            .arg("-V")
            .output()
            .is_err()
        {
            return;
        }
        let store = Store::new_in_memory();
        let state = State::local(store.clone());
        let output = render(
            state,
            simple_graph(),
            LayoutEngine::LayoutDot,
            RenderFormat::FormatSvg,
        )
        .await
        .unwrap()
        .unwrap();
        let bytes = store.get(output.content).await.unwrap();
        let svg = String::from_utf8(bytes).unwrap();
        assert!(svg.contains("<svg"));
        assert!(svg.contains("prepare"));
        assert!(svg.contains("render"));
    }
}

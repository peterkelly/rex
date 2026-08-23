#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    rex::workflow::tool_protocol::run_tool_cli(
        rex_tool_graphviz::tool::module,
        rex_tool_graphviz::default_state,
    )
    .await
}

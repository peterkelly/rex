#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    rex_workflow::tool_protocol::run_tool_cli(
        rex_tool_gnuplot::tool::module,
        rex_workflow::tool_protocol::default_tool_state,
    )
    .await
}

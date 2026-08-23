#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    rex::workflow::tool_protocol::run_tool_cli(
        rex_tool_poppler::tool::module,
        rex_tool_poppler::default_state,
    )
    .await
}

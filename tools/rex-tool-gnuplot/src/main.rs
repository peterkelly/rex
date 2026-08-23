#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    rex::workflow::tool_protocol::run_tool_cli(
        rex_tool_gnuplot::tool::module,
        rex_tool_gnuplot::default_state,
    )
    .await
}

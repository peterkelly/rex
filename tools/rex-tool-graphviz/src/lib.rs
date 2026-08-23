pub mod modules {
    pub mod tools {
        pub use rex_workflow::modules::tools::executor;
    }
}
pub mod state {
    pub use rex_workflow::state::*;
}
pub mod tool;

#[cfg(test)]
mod examples {
    #[tokio::test]
    async fn typecheck() {
        rex_workflow::tool_protocol::typecheck_tool_examples(
            crate::tool::module,
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../rex-workflow/examples/graphviz"),
        )
        .await
        .unwrap();
    }
}

pub mod modules {
    pub mod tools {
        pub use rex::workflow::executor;
    }
}
pub mod state {
    pub use rex::workflow::state::*;
}
pub mod tool;

pub const IMAGE_ENVIRONMENT_VARIABLE: &str = "REX_WORKFLOW_POPPLER_IMAGE";
pub const DEFAULT_IMAGE: &str = "rex-tool-poppler:local";

pub fn default_state() -> Result<state::State, Box<dyn std::error::Error>> {
    rex::workflow::tool_protocol::default_tool_state(
        "poppler",
        IMAGE_ENVIRONMENT_VARIABLE,
        DEFAULT_IMAGE,
    )
}

#[cfg(test)]
pub(crate) fn development_state(store: rex::storage::Store) -> state::State {
    state::State::docker(
        store,
        modules::tools::executor::OciImage::new(
            "poppler",
            DEFAULT_IMAGE,
            modules::tools::executor::OciPlatform::native_linux(),
        ),
        true,
    )
}

#[cfg(test)]
mod examples {
    #[tokio::test]
    async fn typecheck() {
        rex::workflow::tool_protocol::typecheck_tool_examples(
            crate::tool::module,
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/poppler"),
        )
        .await
        .unwrap();
    }
}

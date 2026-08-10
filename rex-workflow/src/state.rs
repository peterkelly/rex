use crate::{
    modules::tools::executor::{DockerToolImages, ToolExecutor, docker_executor, local_executor},
    storage::store::Store,
};
use std::sync::Arc;

#[derive(Clone)]
pub struct State {
    pub store: Store,
    pub tools: Arc<dyn ToolExecutor>,
}

impl State {
    pub fn local(store: Store) -> Self {
        Self {
            store,
            tools: local_executor(),
        }
    }

    pub fn docker(store: Store, images: DockerToolImages) -> Self {
        Self {
            store,
            tools: docker_executor(images),
        }
    }

    pub fn with_executor(store: Store, tools: Arc<dyn ToolExecutor>) -> Self {
        Self { store, tools }
    }
}

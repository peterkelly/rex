use crate::{
    modules::tools::executor::{ToolExecutor, local_executor},
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

    pub fn with_executor(store: Store, tools: Arc<dyn ToolExecutor>) -> Self {
        Self { store, tools }
    }
}

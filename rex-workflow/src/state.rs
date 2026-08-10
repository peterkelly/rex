use crate::storage::store::Store;

#[derive(Clone)]
pub struct State {
    pub store: Store,
}

impl State {
    pub fn local(store: Store) -> Self {
        Self { store }
    }
}

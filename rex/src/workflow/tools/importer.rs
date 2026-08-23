use crate::engine::{DenyImporter, Importer};
use crate::workflow::{
    state::{ExternalTools, State},
    tools::external::ExternalToolImporter,
};
use std::sync::Arc;

pub(crate) fn importer(external: Option<ExternalTools>) -> Arc<dyn Importer<State>> {
    match external {
        Some(config) => Arc::new(ExternalToolImporter::new(config)),
        None => Arc::new(DenyImporter),
    }
}

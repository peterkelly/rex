use crate::{
    modules::tools::external::ExternalToolImporter,
    state::{ExternalTools, State},
};
use rex::engine::Importer;
use std::sync::Arc;

pub(crate) fn importer(external: Option<ExternalTools>) -> Arc<dyn Importer<State>> {
    match external {
        Some(config) => Arc::new(ExternalToolImporter::new(config)),
        None => Arc::new(rex::engine::DenyImporter),
    }
}

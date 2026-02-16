use crate::Type;

#[cfg(feature = "graphql")]
async_graphql::scalar!(Type);

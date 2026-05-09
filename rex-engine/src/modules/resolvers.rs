use std::sync::Arc;

use rex_util::{sha256_hex, stdlib_source};

use crate::ModuleError;

use super::{ModuleId, ResolveRequest, ResolvedModule, ResolvedModuleContent, ResolverFn};

pub fn default_stdlib_resolver() -> ResolverFn {
    Arc::new(|req: ResolveRequest| {
        let (base, expected_sha) = if let Some((a, b)) = req.module_name.split_once('#') {
            (a, Some(b))
        } else {
            (req.module_name.as_str(), None)
        };

        let Some(source) = stdlib_source(base) else {
            return Ok(None);
        };

        if let Some(expected) = expected_sha {
            let hash = sha256_hex(source.as_bytes());
            let expected = expected.to_ascii_lowercase();
            if !hash.starts_with(&expected) {
                return Err(ModuleError::ShaMismatchStdlib {
                    module: base.to_string(),
                    expected,
                    actual: hash,
                }
                .into());
            }
        }

        Ok(Some(ResolvedModule {
            id: ModuleId::Virtual(base.to_string()),
            content: ResolvedModuleContent::Source(source.to_string()),
        }))
    })
}

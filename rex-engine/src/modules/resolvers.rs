use std::sync::Arc;

#[cfg(feature = "github-imports")]
use std::process::Command;

use rex_util::sha256_hex;

use crate::ModuleError;

use super::{ModuleId, ResolveRequest, ResolvedModule, ResolverFn};

#[cfg(feature = "github-imports")]
pub fn default_github_resolver() -> ResolverFn {
    Arc::new(|req: ResolveRequest| {
        let url = req.module_name;
        let Some(rest) = url.strip_prefix("https://github.com/") else {
            return Ok(None);
        };

        let (path_part, sha_opt) = match rest.split_once('#') {
            Some((a, b)) if !b.is_empty() => (a, Some(b.to_string())),
            _ => (rest, None),
        };

        let mut parts = path_part.splitn(3, '/');
        let owner = parts.next().unwrap_or("");
        let repo = parts.next().unwrap_or("");
        let file_path = parts.next().unwrap_or("");
        if owner.is_empty() || repo.is_empty() || file_path.is_empty() {
            return Err(ModuleError::InvalidGithubImport { url }.into());
        }

        let sha = sha_opt.ok_or_else(|| ModuleError::UnpinnedGithubImport { url: url.clone() })?;
        let raw_url = format!("https://raw.githubusercontent.com/{owner}/{repo}/{sha}/{file_path}");

        let output = Command::new("curl")
            .arg("-fsSL")
            .arg(&raw_url)
            .output()
            .map_err(|e| ModuleError::CurlFailed { source: e })?;
        if !output.status.success() {
            return Err(ModuleError::CurlNonZeroExit {
                url: raw_url,
                status: output.status,
            }
            .into());
        }
        let source = String::from_utf8(output.stdout).map_err(|e| ModuleError::NotUtf8Remote {
            url: raw_url.clone(),
            source: e,
        })?;

        Ok(Some(ResolvedModule {
            id: ModuleId::Remote(url),
            source,
        }))
    })
}

pub fn default_stdlib_resolver() -> ResolverFn {
    Arc::new(|req: ResolveRequest| {
        let (base, expected_sha) = if let Some((a, b)) = req.module_name.split_once('#') {
            (a, Some(b))
        } else {
            (req.module_name.as_str(), None)
        };

        let Some(source) = rex_util::stdlib_source(base) else {
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
            source: source.to_string(),
        }))
    })
}

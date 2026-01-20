use std::path::PathBuf;
use std::sync::Arc;

#[cfg(feature = "github-imports")]
use std::process::Command;

use rex_util::{ImportPathError, resolve_local_import_path, sha256_hex};

use crate::{EngineError, ModuleError};

use super::{ModuleId, ResolveRequest, ResolvedModule, ResolverFn};

fn split_module_name_and_sha(module_name: String) -> (String, Option<String>) {
    match module_name.split_once('#') {
        Some((a, b)) if !b.is_empty() => (a.to_string(), Some(b.to_string())),
        _ => (module_name, None),
    }
}

fn resolve_rex_file(
    path: PathBuf,
    expected_sha: Option<String>,
    kind: &'static str,
) -> Result<Option<ResolvedModule>, EngineError> {
    let Ok(canon) = path.canonicalize() else {
        return Ok(None);
    };
    let bytes = match std::fs::read(&canon) {
        Ok(b) => b,
        Err(_) => return Ok(None),
    };
    let hash = sha256_hex(&bytes);
    if let Some(expected) = expected_sha {
        let expected = expected.to_ascii_lowercase();
        if !hash.starts_with(&expected) {
            return Err(ModuleError::ShaMismatchPath {
                kind,
                path: canon,
                expected,
                actual: hash,
            }
            .into());
        }
    }
    let source = String::from_utf8(bytes).map_err(|e| ModuleError::NotUtf8 {
        kind,
        path: canon.clone(),
        source: e,
    })?;
    Ok(Some(ResolvedModule {
        id: ModuleId::Local { path: canon, hash },
        source,
    }))
}

pub fn default_local_resolver() -> ResolverFn {
    Arc::new(|req: ResolveRequest| {
        if req.module_name.starts_with("https://") {
            return Ok(None);
        }

        let (module_name, expected_sha) = split_module_name_and_sha(req.module_name);

        let base_dir = match req.importer {
            Some(ModuleId::Local { path, .. }) => path.parent().map(|p| p.to_path_buf()),
            _ => std::env::current_dir().ok(),
        }
        .ok_or(ModuleError::NoBaseDirectory)?;

        let segs: Vec<&str> = module_name.split('.').collect();
        let path = match resolve_local_import_path(base_dir.as_path(), &segs) {
            Ok(Some(path)) => path,
            Ok(None) => return Ok(None),
            Err(ImportPathError::EscapesRoot) => return Err(ModuleError::ImportEscapesRoot.into()),
        };

        resolve_rex_file(path, expected_sha, "local")
    })
}

pub fn include_resolver(root: PathBuf) -> ResolverFn {
    Arc::new(move |req: ResolveRequest| {
        if req.module_name.starts_with("https://") {
            return Ok(None);
        }

        let (module_name, expected_sha) = split_module_name_and_sha(req.module_name);

        let segs: Vec<&str> = module_name.split('.').collect();
        if segs.is_empty() {
            return Ok(None);
        }
        let mut path = root.clone();
        for seg in &segs[..segs.len().saturating_sub(1)] {
            path.push(seg);
        }
        let last = segs.last().ok_or(ModuleError::EmptyModulePath)?;
        path.push(format!("{last}.rex"));

        resolve_rex_file(path, expected_sha, "include")
    })
}

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

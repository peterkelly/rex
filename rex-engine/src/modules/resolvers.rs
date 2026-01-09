use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

use futures::FutureExt;
use rex_util::{ImportPathError, resolve_local_import_path, sha256_hex};

use crate::EngineError;

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
            return Err(EngineError::Module(format!(
                "{kind} import sha mismatch for {}: expected #{}, got #{hash}",
                canon.display(),
                expected
            )));
        }
    }
    let source = String::from_utf8(bytes)
        .map_err(|e| EngineError::Module(format!("{kind} module was not utf-8: {e}")))?;
    Ok(Some(ResolvedModule {
        id: ModuleId::Local { path: canon, hash },
        source,
    }))
}

pub fn default_local_resolver() -> ResolverFn {
    Arc::new(|req: ResolveRequest| {
        async move {
            if req.module_name.starts_with("https://") {
                return Ok(None);
            }

            let (module_name, expected_sha) = split_module_name_and_sha(req.module_name);

            let base_dir = match req.importer {
                Some(ModuleId::Local { path, .. }) => path.parent().map(|p| p.to_path_buf()),
                _ => std::env::current_dir().ok(),
            }
            .ok_or_else(|| {
                EngineError::Module("cannot resolve local import without a base directory".into())
            })?;

            let segs: Vec<&str> = module_name.split('.').collect();
            let path = match resolve_local_import_path(base_dir.as_path(), &segs) {
                Ok(Some(path)) => path,
                Ok(None) => return Ok(None),
                Err(ImportPathError::EscapesRoot) => {
                    return Err(EngineError::Module(
                        "import path escapes filesystem root".into(),
                    ));
                }
            };

            resolve_rex_file(path, expected_sha, "local")
        }
        .boxed()
    })
}

pub fn include_resolver(root: PathBuf) -> ResolverFn {
    Arc::new(move |req: ResolveRequest| {
        let root = root.clone();
        async move {
            if req.module_name.starts_with("https://") {
                return Ok(None);
            }

            let (module_name, expected_sha) = split_module_name_and_sha(req.module_name);

            let segs: Vec<&str> = module_name.split('.').collect();
            if segs.is_empty() {
                return Ok(None);
            }
            let mut path = root;
            for seg in &segs[..segs.len().saturating_sub(1)] {
                path.push(seg);
            }
            let last = segs
                .last()
                .ok_or_else(|| EngineError::Module("empty module path".into()))?;
            path.push(format!("{last}.rex"));

            resolve_rex_file(path, expected_sha, "include")
        }
        .boxed()
    })
}

pub fn default_github_resolver() -> ResolverFn {
    Arc::new(|req: ResolveRequest| {
        async move {
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
                return Err(EngineError::Module(format!(
                    "github import must be `https://github.com/<owner>/<repo>/<path>.rex[#sha]` (got {url})"
                )));
            }

            let sha = match sha_opt {
                Some(sha) => sha,
                None => {
                    tracing::warn!(
                        "github import `{}` has no #sha; using latest commit on master",
                        url
                    );
                    let api_url =
                        format!("https://api.github.com/repos/{owner}/{repo}/commits/master");
                    let output = Command::new("curl")
                        .arg("-fsSL")
                        .arg("-H")
                        .arg("User-Agent: rex")
                        .arg(&api_url)
                        .output()
                        .map_err(|e| EngineError::Module(format!("failed to run curl: {e}")))?;
                    if !output.status.success() {
                        return Err(EngineError::Module(format!(
                            "failed to fetch {api_url} (curl exit {})",
                            output.status
                        )));
                    }
                    let body = String::from_utf8(output.stdout).map_err(|e| {
                        EngineError::Module(format!("github api response was not utf-8: {e}"))
                    })?;
                    let needle = "\"sha\":\"";
                    let start = body
                        .find(needle)
                        .ok_or_else(|| EngineError::Module("github api response missing sha".into()))?
                        + needle.len();
                    let end = body[start..]
                        .find('\"')
                        .ok_or_else(|| {
                            EngineError::Module("github api response missing sha terminator".into())
                        })?
                        + start;
                    body[start..end].to_string()
                }
            };

            let raw_url = format!(
                "https://raw.githubusercontent.com/{owner}/{repo}/{sha}/{file_path}"
            );

            let output = Command::new("curl")
                .arg("-fsSL")
                .arg(&raw_url)
                .output()
                .map_err(|e| EngineError::Module(format!("failed to run curl: {e}")))?;
            if !output.status.success() {
                return Err(EngineError::Module(format!(
                    "failed to fetch {raw_url} (curl exit {})",
                    output.status
                )));
            }
            let source = String::from_utf8(output.stdout)
                .map_err(|e| EngineError::Module(format!("remote module was not utf-8: {e}")))?;

            let canonical = if url.contains('#') {
                url
            } else {
                format!("{url}#{sha}")
            };

            Ok(Some(ResolvedModule {
                id: ModuleId::Remote(canonical),
                source,
            }))
        }
        .boxed()
    })
}

pub fn default_stdlib_resolver() -> ResolverFn {
    Arc::new(|req: ResolveRequest| {
        async move {
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
                    return Err(EngineError::Module(format!(
                        "sha mismatch for `{base}`: expected #{expected}, got #{hash}"
                    )));
                }
            }

            Ok(Some(ResolvedModule {
                id: ModuleId::Virtual(base.to_string()),
                source: source.to_string(),
            }))
        }
        .boxed()
    })
}

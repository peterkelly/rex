use crate::prelude::*;

#[derive(Debug)]
pub(crate) enum TokenizeOrParseError {
    Lex(LexicalError),
    Parse(Vec<ParseError>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BulkQuickFixStrategy {
    Conservative,
    Aggressive,
}

impl BulkQuickFixStrategy {
    pub fn parse(s: &str) -> Self {
        if s.eq_ignore_ascii_case("aggressive") {
            Self::Aggressive
        } else {
            Self::Conservative
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Conservative => "conservative",
            Self::Aggressive => "aggressive",
        }
    }
}

#[derive(Clone)]
pub(crate) struct CachedParse {
    hash: u64,
    tokens: Tokens,
    program: CompilationUnit,
}

#[derive(Clone, Default)]
pub struct AnalysisState {
    parse_cache: Arc<Mutex<HashMap<Url, CachedParse>>>,
}

#[derive(Clone)]
pub struct AnalysisSession {
    parse_cache: Arc<Mutex<HashMap<Url, CachedParse>>>,
    open_documents: Arc<HashMap<Url, String>>,
}

impl AnalysisState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn session(&self, open_documents: HashMap<Url, String>) -> AnalysisSession {
        AnalysisSession {
            parse_cache: self.parse_cache.clone(),
            open_documents: Arc::new(open_documents),
        }
    }

    pub fn empty_session(&self) -> AnalysisSession {
        AnalysisSession {
            parse_cache: self.parse_cache.clone(),
            open_documents: Arc::new(HashMap::new()),
        }
    }

    pub fn clear_parse_cache(&self, uri: &Url) {
        let Ok(mut cache) = self.parse_cache.lock() else {
            return;
        };
        cache.remove(uri);
    }
}

impl AnalysisSession {
    pub fn isolated() -> Self {
        AnalysisState::new().empty_session()
    }

    pub fn from_open_documents(open_documents: HashMap<Url, String>) -> Self {
        AnalysisState::new().session(open_documents)
    }

    pub(crate) fn module_service(&self) -> LspModuleService {
        let module_paths = self
            .open_documents
            .keys()
            .filter_map(|uri| {
                let path = uri_to_file_path(uri)?;
                let id = module_id_from_path(&path)?;
                Some((id, path))
            })
            .collect();
        LspModuleService {
            open_documents: self.open_documents.clone(),
            module_paths: Arc::new(module_paths),
            root: None,
        }
    }

    pub(crate) fn module_service_for_uri(&self, uri: &Url) -> LspModuleService {
        let mut service = self.module_service();
        if let Some(path) = uri_to_file_path(uri)
            && let Some(id) = module_id_from_path(&path)
        {
            service.root = path.parent().map(|parent| parent.to_path_buf());
            Arc::make_mut(&mut service.module_paths).insert(id, path);
        }
        service
    }

    pub(crate) fn tokenize_and_parse_cached(
        &self,
        uri: &Url,
        text: &str,
    ) -> std::result::Result<(Tokens, CompilationUnit), TokenizeOrParseError> {
        let hash = text_hash(text);
        if let Ok(cache) = self.parse_cache.lock()
            && let Some(cached) = cache.get(uri)
            && cached.hash == hash
        {
            return Ok((cached.tokens.clone(), cached.program.clone()));
        }

        let (tokens, program) = tokenize_and_parse(text)?;
        if let Ok(mut cache) = self.parse_cache.lock() {
            cache.insert(
                uri.clone(),
                CachedParse {
                    hash,
                    tokens: tokens.clone(),
                    program: program.clone(),
                },
            );
        }
        Ok((tokens, program))
    }
}

pub(crate) fn text_hash(text: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

pub(crate) fn semantic_candidate_values(
    ts: &TypeSystem,
    preferred_names: &BTreeSet<Symbol>,
) -> Vec<(Symbol, Vec<Scheme>)> {
    let mut entries = ts
        .env
        .values
        .iter()
        .map(|(name, schemes)| (name.clone(), schemes.clone()))
        .collect::<Vec<_>>();
    entries.sort_by(|(left, _), (right, _)| {
        let left_priority = !preferred_names.contains(left);
        let right_priority = !preferred_names.contains(right);
        left_priority.cmp(&right_priority).then(left.cmp(right))
    });

    let mut out = Vec::new();
    let mut scanned = 0usize;
    for (name, schemes) in entries {
        if scanned >= MAX_SEMANTIC_ENV_SCHEMES_SCAN {
            break;
        }
        let remaining = MAX_SEMANTIC_ENV_SCHEMES_SCAN - scanned;
        let kept = schemes
            .into_iter()
            .take(remaining)
            .map(|value| value.scheme)
            .collect::<Vec<_>>();
        if kept.is_empty() {
            continue;
        }
        scanned += kept.len();
        out.push((name, kept));
    }
    out
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn uri_to_file_path(uri: &Url) -> Option<PathBuf> {
    uri.to_file_path().ok()
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn uri_to_file_path(_uri: &Url) -> Option<PathBuf> {
    None
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn url_from_file_path(path: &std::path::Path) -> Option<Url> {
    Url::from_file_path(path).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_candidates_prioritize_user_names_before_enforcing_limit() {
        let mut ts = TypeSystem::new();
        let scheme = Scheme::new(Vec::new(), Vec::new(), Type::builtin(BuiltinTypeId::I32));
        for index in 0..(MAX_SEMANTIC_ENV_SCHEMES_SCAN + 32) {
            ts.add_value(format!("prelude_{index:04}"), scheme.clone());
        }
        let local_name = Symbol::intern("zz_local");
        ts.add_value(local_name.as_ref(), scheme);
        let preferred_names = BTreeSet::from([local_name.clone()]);

        let candidates = semantic_candidate_values(&ts, &preferred_names);

        assert_eq!(candidates.first().map(|(name, _)| name), Some(&local_name));
        assert_eq!(
            candidates
                .iter()
                .map(|(_, schemes)| schemes.len())
                .sum::<usize>(),
            MAX_SEMANTIC_ENV_SCHEMES_SCAN
        );
        let non_preferred_names = candidates
            .iter()
            .skip(1)
            .map(|(name, _)| name)
            .collect::<Vec<_>>();
        assert!(
            non_preferred_names
                .windows(2)
                .all(|names| names[0] <= names[1])
        );
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn url_from_file_path(_path: &std::path::Path) -> Option<Url> {
    None
}

pub(crate) fn module_id_from_path(path: &Path) -> Option<ModuleId> {
    let stem = path.file_stem()?.to_str()?;
    ModuleId::parse(stem).ok()
}

#[derive(Clone, Default)]
pub(crate) struct LspModuleService {
    pub(crate) open_documents: Arc<HashMap<Url, String>>,
    module_paths: Arc<HashMap<ModuleId, PathBuf>>,
    root: Option<PathBuf>,
}

#[derive(Clone)]
pub(crate) struct LspLoadedModule {
    pub(crate) id: ModuleId,
    pub(crate) path: Option<PathBuf>,
    pub(crate) source: String,
}

impl LspModuleService {
    pub(crate) fn path_for_module(&self, id: &ModuleId) -> Option<PathBuf> {
        self.module_paths.get(id).cloned()
    }

    fn import_specifier(path: &ImportPath) -> Result<ModuleId, EngineError> {
        let id = ModuleId::from_segments(
            path.segments
                .iter()
                .map(|segment| segment.as_ref().to_string())
                .collect::<Vec<_>>(),
        )?;
        Ok(id)
    }

    pub(crate) fn load_import_path(
        &self,
        uri: &Url,
        path: &ImportPath,
    ) -> Result<Option<LspLoadedModule>, EngineError> {
        let module_id = Self::import_specifier(path)?;
        self.load_request(ImportRequest {
            module_id,
            importer: uri_to_file_path(uri).and_then(|path| module_id_from_path(&path)),
        })
    }

    fn load_request(&self, req: ImportRequest) -> Result<Option<LspLoadedModule>, EngineError> {
        let importer_path = req
            .importer
            .as_ref()
            .and_then(|id| self.module_paths.get(id));
        let base_dir = importer_path
            .and_then(|path| path.parent().map(|p| p.to_path_buf()))
            .or_else(|| self.root.clone());
        let Some(base_dir) = base_dir else {
            return Ok(None);
        };

        let resolved_id = match req.importer.as_ref().and_then(ModuleId::parent) {
            Some(parent) => parent.join(&req.module_id),
            None => req.module_id,
        };
        let segments = resolved_id
            .segments()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        let path = match resolve_local_import_path(base_dir.as_path(), &segments) {
            Ok(Some(path)) => path,
            Ok(None) => return Ok(None),
            Err(err) => {
                return Err(match err {
                    rex_util::ImportPathError::EscapesRoot => ModuleError::ImportEscapesRoot,
                }
                .into());
            }
        };
        self.load_path(resolved_id, path, "local")
    }

    fn load_path(
        &self,
        id: ModuleId,
        path: PathBuf,
        kind: &'static str,
    ) -> Result<Option<LspLoadedModule>, EngineError> {
        if let Some(module) = self.load_open_document(id.clone(), &path, kind)? {
            return Ok(Some(module));
        }

        let Ok(canon) = path.canonicalize() else {
            return Ok(None);
        };

        if let Some(module) = self.load_open_document(id.clone(), &canon, kind)? {
            return Ok(Some(module));
        }

        let bytes = match fs::read(&canon) {
            Ok(bytes) => bytes,
            Err(_) => return Ok(None),
        };
        self.loaded_from_bytes(id, canon, bytes, kind).map(Some)
    }

    fn load_open_document(
        &self,
        id: ModuleId,
        path: &Path,
        kind: &'static str,
    ) -> Result<Option<LspLoadedModule>, EngineError> {
        let Some(url) = url_from_file_path(path) else {
            return Ok(None);
        };
        let Some(source) = self.open_documents.get(&url).cloned() else {
            return Ok(None);
        };
        self.loaded_from_source(id, path.to_path_buf(), source, kind)
            .map(Some)
    }

    fn loaded_from_bytes(
        &self,
        id: ModuleId,
        path: PathBuf,
        bytes: Vec<u8>,
        kind: &'static str,
    ) -> Result<LspLoadedModule, EngineError> {
        let source = String::from_utf8(bytes).map_err(|source| ModuleError::NotUtf8 {
            kind,
            path: path.clone(),
            source,
        })?;
        Ok(LspLoadedModule {
            id,
            path: Some(path),
            source,
        })
    }

    fn loaded_from_source(
        &self,
        id: ModuleId,
        path: PathBuf,
        source: String,
        _kind: &'static str,
    ) -> Result<LspLoadedModule, EngineError> {
        Ok(LspLoadedModule {
            id,
            path: Some(path),
            source,
        })
    }
}

impl Importer for LspModuleService {
    fn import<'a>(
        &'a self,
        req: ImportRequest,
    ) -> BoxFuture<'a, Result<Option<ResolvedModule>, EngineError>> {
        Box::pin(async move {
            Ok(self.load_request(req)?.map(|module| ResolvedModule {
                id: module.id,
                content: ResolvedModuleContent::Source(module.source),
            }))
        })
    }
}

pub(crate) fn tokenize_and_parse(
    text: &str,
) -> std::result::Result<(Tokens, CompilationUnit), TokenizeOrParseError> {
    let tokens = Token::tokenize(text).map_err(TokenizeOrParseError::Lex)?;
    let program = parse_with_tokens(tokens.clone()).map_err(TokenizeOrParseError::Parse)?;
    Ok((tokens, program))
}

#[derive(Clone)]
pub struct ImportModuleInfo {
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) path: Option<PathBuf>,
    pub(crate) exports: ModuleExports,
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) export_defs: HashMap<String, Span>,
}

pub(crate) fn is_ident_like(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub(crate) fn prelude_completion_values() -> &'static Vec<(String, CompletionItemKind)> {
    static PRELUDE_VALUES: OnceLock<Vec<(String, CompletionItemKind)>> = OnceLock::new();
    PRELUDE_VALUES.get_or_init(|| {
        let ts = match standard_type_system() {
            Ok(ts) => ts,
            Err(e) => {
                eprintln!("rex-lsp: failed to build prelude for completions: {e}");
                return Vec::new();
            }
        };
        let mut out = Vec::new();
        for (name, schemes) in ts.env.values.iter() {
            let name = name.as_ref().to_string();
            if !is_ident_like(&name) {
                continue;
            }
            let is_fun = schemes
                .iter()
                .any(|value| matches!(value.scheme.typ.as_ref(), TypeKind::Fun(..)));
            let kind = if is_fun {
                CompletionItemKind::FUNCTION
            } else {
                CompletionItemKind::VARIABLE
            };
            out.push((name, kind));
        }
        out.sort_by(|(a, _), (b, _)| a.cmp(b));
        out
    })
}

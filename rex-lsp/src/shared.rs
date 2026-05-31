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
        LspModuleService {
            open_documents: self.open_documents.clone(),
        }
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

pub(crate) fn semantic_candidate_values(ts: &TypeSystem) -> Vec<(Symbol, Vec<Scheme>)> {
    let mut out = Vec::new();
    let mut scanned = 0usize;
    for (name, schemes) in &ts.env.values {
        if scanned >= MAX_SEMANTIC_ENV_SCHEMES_SCAN {
            break;
        }
        let remaining = MAX_SEMANTIC_ENV_SCHEMES_SCAN - scanned;
        let kept = schemes.iter().take(remaining).cloned().collect::<Vec<_>>();
        if kept.is_empty() {
            continue;
        }
        scanned += kept.len();
        out.push((name.clone(), kept));
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

#[cfg(target_arch = "wasm32")]
pub(crate) fn url_from_file_path(_path: &std::path::Path) -> Option<Url> {
    None
}

#[derive(Clone, Default)]
pub(crate) struct LspModuleService {
    pub(crate) open_documents: Arc<HashMap<Url, String>>,
}

#[derive(Clone)]
pub(crate) struct LspLoadedModule {
    pub(crate) id: ModuleId,
    pub(crate) source: String,
}

impl LspModuleService {
    fn import_specifier(path: &ImportPath) -> Option<String> {
        match path {
            ImportPath::Local { segments, sha } => {
                let base = segments
                    .iter()
                    .map(|s| s.as_ref())
                    .collect::<Vec<_>>()
                    .join(".");
                Some(if let Some(sha) = sha {
                    format!("{base}#{sha}")
                } else {
                    base
                })
            }
            ImportPath::Remote { .. } => None,
        }
    }

    fn split_name_and_sha(module_name: String) -> (String, Option<String>) {
        match module_name.split_once('#') {
            Some((base, sha)) if !sha.is_empty() => (base.to_string(), Some(sha.to_string())),
            _ => (module_name, None),
        }
    }

    pub(crate) fn load_import_path(
        &self,
        uri: &Url,
        path: &ImportPath,
    ) -> Result<Option<LspLoadedModule>, EngineError> {
        let Some(module_name) = Self::import_specifier(path) else {
            return Ok(None);
        };
        self.load_request(ImportRequest {
            module_name,
            importer: uri_to_file_path(uri).map(|path| ModuleId::Local { path }),
        })
    }

    fn load_request(&self, req: ImportRequest) -> Result<Option<LspLoadedModule>, EngineError> {
        if req.module_name.starts_with("https://") {
            return Ok(None);
        }

        let (module_name, expected_sha) = Self::split_name_and_sha(req.module_name);

        if let Some(module) = self.load_stdlib(&module_name, expected_sha.clone())? {
            return Ok(Some(module));
        }

        if module_name.ends_with(".rex") || Path::new(&module_name).components().count() > 1 {
            return self.load_path(PathBuf::from(module_name), expected_sha, "local");
        }

        let base_dir = match req.importer {
            Some(ModuleId::Local { path }) => path.parent().map(|p| p.to_path_buf()),
            _ => None,
        };
        let Some(base_dir) = base_dir else {
            return Ok(None);
        };

        let segments = module_name.split('.').collect::<Vec<_>>();
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
        self.load_path(path, expected_sha, "local")
    }

    fn load_stdlib(
        &self,
        module_name: &str,
        expected_sha: Option<String>,
    ) -> Result<Option<LspLoadedModule>, EngineError> {
        let Some(source) = stdlib_source(module_name) else {
            return Ok(None);
        };
        let hash = sha256_hex(source.as_bytes());
        if let Some(expected) = expected_sha {
            let expected = expected.to_ascii_lowercase();
            if !hash.starts_with(&expected) {
                return Err(ModuleError::ShaMismatchStdlib {
                    module: module_name.to_string(),
                    expected,
                    actual: hash,
                }
                .into());
            }
        }
        Ok(Some(LspLoadedModule {
            id: ModuleId::Virtual(module_name.to_string()),
            source: source.to_string(),
        }))
    }

    fn load_path(
        &self,
        path: PathBuf,
        expected_sha: Option<String>,
        kind: &'static str,
    ) -> Result<Option<LspLoadedModule>, EngineError> {
        if let Some(module) = self.load_open_document(&path, expected_sha.as_deref(), kind)? {
            return Ok(Some(module));
        }

        let Ok(canon) = path.canonicalize() else {
            return Ok(None);
        };

        if let Some(module) = self.load_open_document(&canon, expected_sha.as_deref(), kind)? {
            return Ok(Some(module));
        }

        let bytes = match fs::read(&canon) {
            Ok(bytes) => bytes,
            Err(_) => return Ok(None),
        };
        self.loaded_from_bytes(canon, bytes, expected_sha.as_deref(), kind)
            .map(Some)
    }

    fn load_open_document(
        &self,
        path: &Path,
        expected_sha: Option<&str>,
        kind: &'static str,
    ) -> Result<Option<LspLoadedModule>, EngineError> {
        let Some(url) = url_from_file_path(path) else {
            return Ok(None);
        };
        let Some(source) = self.open_documents.get(&url).cloned() else {
            return Ok(None);
        };
        self.loaded_from_source(path.to_path_buf(), source, expected_sha, kind)
            .map(Some)
    }

    fn loaded_from_bytes(
        &self,
        path: PathBuf,
        bytes: Vec<u8>,
        expected_sha: Option<&str>,
        kind: &'static str,
    ) -> Result<LspLoadedModule, EngineError> {
        let hash = sha256_hex(&bytes);
        self.check_path_hash(&path, &hash, expected_sha, kind)?;
        let source = String::from_utf8(bytes).map_err(|source| ModuleError::NotUtf8 {
            kind,
            path: path.clone(),
            source,
        })?;
        Ok(LspLoadedModule {
            id: ModuleId::Local { path: path.clone() },
            source,
        })
    }

    fn loaded_from_source(
        &self,
        path: PathBuf,
        source: String,
        expected_sha: Option<&str>,
        kind: &'static str,
    ) -> Result<LspLoadedModule, EngineError> {
        let hash = sha256_hex(source.as_bytes());
        self.check_path_hash(&path, &hash, expected_sha, kind)?;
        Ok(LspLoadedModule {
            id: ModuleId::Local { path: path.clone() },
            source,
        })
    }

    fn check_path_hash(
        &self,
        path: &Path,
        hash: &str,
        expected_sha: Option<&str>,
        kind: &'static str,
    ) -> Result<(), EngineError> {
        let Some(expected) = expected_sha else {
            return Ok(());
        };
        let expected = expected.to_ascii_lowercase();
        if hash.starts_with(&expected) {
            return Ok(());
        }
        Err(ModuleError::ShaMismatchPath {
            kind,
            path: path.to_path_buf(),
            expected,
            actual: hash.to_string(),
        }
        .into())
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
        let ts = match TypeSystem::new_with_prelude() {
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
                .any(|scheme| matches!(scheme.typ.as_ref(), TypeKind::Fun(..)));
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

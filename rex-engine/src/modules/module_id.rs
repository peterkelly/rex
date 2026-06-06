use std::fmt;

/// Stable identity for a Rex module in the abstract module namespace.
///
/// A module ID is intentionally only a qualified Rex name, such as
/// `std.prelude` or `ffmpeg.formats.av1`. It does not record where the module
/// source came from. Filesystems, databases, hard-coded strings, open LSP
/// buffers, and network fetches are all importer concerns; the engine caches,
/// detects cycles, and qualifies symbols by this abstract identity alone.
#[derive(Clone, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub struct ModuleId {
    segments: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModuleIdError {
    input: String,
    reason: &'static str,
}

impl ModuleIdError {
    fn new(input: impl Into<String>, reason: &'static str) -> Self {
        Self {
            input: input.into(),
            reason,
        }
    }

    pub fn input(&self) -> &str {
        &self.input
    }

    pub fn reason(&self) -> &'static str {
        self.reason
    }
}

impl fmt::Display for ModuleIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid module id `{}`: {}", self.input, self.reason)
    }
}

impl std::error::Error for ModuleIdError {}

impl ModuleId {
    pub fn parse(input: impl AsRef<str>) -> Result<Self, ModuleIdError> {
        let input = input.as_ref();
        if input.is_empty() {
            return Err(ModuleIdError::new(input, "module id cannot be empty"));
        }
        if input.trim() != input {
            return Err(ModuleIdError::new(
                input,
                "module id cannot contain leading or trailing whitespace",
            ));
        }
        let segments = input
            .split('.')
            .map(|segment| segment.to_string())
            .collect::<Vec<_>>();
        Self::from_segments(segments).map_err(|err| ModuleIdError::new(input, err.reason))
    }

    pub fn from_segments<I, S>(segments: I) -> Result<Self, ModuleIdError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let segments = segments.into_iter().map(Into::into).collect::<Vec<_>>();
        if segments.is_empty() {
            return Err(ModuleIdError::new("", "module id cannot be empty"));
        }
        for segment in &segments {
            validate_segment(segment)?;
        }
        Ok(Self { segments })
    }

    pub fn segments(&self) -> &[String] {
        &self.segments
    }

    pub fn parent(&self) -> Option<Self> {
        if self.segments.len() <= 1 {
            return None;
        }
        Some(Self {
            segments: self.segments[..self.segments.len() - 1].to_vec(),
        })
    }

    pub fn join(&self, child: &ModuleId) -> Self {
        let mut segments = self.segments.clone();
        segments.extend(child.segments.iter().cloned());
        Self { segments }
    }
}

impl fmt::Display for ModuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (idx, segment) in self.segments.iter().enumerate() {
            if idx > 0 {
                write!(f, ".")?;
            }
            write!(f, "{segment}")?;
        }
        Ok(())
    }
}

fn validate_segment(segment: &str) -> Result<(), ModuleIdError> {
    if segment.is_empty() {
        return Err(ModuleIdError::new("", "module id segment cannot be empty"));
    }
    let mut chars = segment.chars();
    let Some(first) = chars.next() else {
        return Err(ModuleIdError::new("", "module id segment cannot be empty"));
    };
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return Err(ModuleIdError::new(
            segment,
            "module id segment must start with a letter or underscore",
        ));
    }
    if chars.any(|ch| !(ch == '_' || ch.is_ascii_alphanumeric())) {
        return Err(ModuleIdError::new(
            segment,
            "module id segment must contain only letters, digits, or underscores",
        ));
    }
    Ok(())
}

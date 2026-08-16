use super::types::*;
use crate::modules::tools::executor::{
    CasInput, ExpectedOutput, InputKind, OutputId, OutputKind, ToolArgument, ToolExecutionPlan,
    ToolProgram,
};

struct PlanBuilder {
    arguments: Vec<ToolArgument>,
    inputs: Vec<CasInput>,
    outputs: Vec<ExpectedOutput>,
}

impl PlanBuilder {
    fn new() -> Self {
        Self {
            arguments: Vec::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
        }
    }

    fn literal(&mut self, value: impl Into<String>) {
        self.arguments.push(ToolArgument::literal(value));
    }

    fn argument(&mut self, value: ToolArgument) {
        self.arguments.push(value);
    }

    fn input_pdf(&mut self, pdf: Pdf) -> usize {
        let id = self.inputs.len();
        self.inputs.push(CasInput {
            hash: pdf.content,
            extension: "pdf".to_string(),
            kind: InputKind::Blob,
        });
        id
    }

    fn output(&mut self, kind: OutputKind, extension: &str) -> OutputId {
        let id = self.outputs.len();
        self.outputs.push(ExpectedOutput {
            kind,
            extension: extension.to_string(),
        });
        id
    }

    fn password(&mut self, password: Option<String>) {
        if let Some(password) = password {
            self.literal(format!("--password={password}"));
        }
    }

    fn finish(self) -> ToolExecutionPlan {
        ToolExecutionPlan {
            program: ToolProgram::Qpdf,
            arguments: self.arguments,
            inputs: self.inputs,
            outputs: self.outputs,
        }
    }
}

pub(crate) fn check_plan(pdf: Pdf, password: Option<String>) -> ToolExecutionPlan {
    let mut builder = PlanBuilder::new();
    let input = builder.input_pdf(pdf);
    builder.literal("--check");
    builder.password(password);
    builder.argument(ToolArgument::input(input));
    builder.finish()
}

pub(crate) fn show_npages_plan(pdf: Pdf, password: Option<String>) -> ToolExecutionPlan {
    let mut builder = PlanBuilder::new();
    let input = builder.input_pdf(pdf);
    builder.literal("--show-npages");
    builder.password(password);
    builder.argument(ToolArgument::input(input));
    builder.finish()
}

pub(crate) fn version_plan() -> ToolExecutionPlan {
    let mut builder = PlanBuilder::new();
    builder.literal("--version");
    builder.finish()
}

pub(crate) fn json_plan(
    pdf: Pdf,
    password: Option<String>,
    options: JsonOptions,
) -> Result<ToolExecutionPlan, QpdfError> {
    let mut builder = PlanBuilder::new();
    let input = builder.input_pdf(pdf);
    let output = builder.output(OutputKind::Single, "json");
    builder.literal("--json=2");
    for key in options.keys {
        builder.literal(format!("--json-key={}", json_key(key)));
    }
    for object in options.objects {
        require_json_object(&object)?;
        builder.literal(format!("--json-object={object}"));
    }
    builder.literal(format!(
        "--json-stream-data={}",
        match options.stream_data {
            JsonStreamData::JsonStreamDataNone => "none",
            JsonStreamData::JsonStreamDataInline => "inline",
        }
    ));
    builder.literal(format!(
        "--decode-level={}",
        decode_level(options.decode_level)
    ));
    builder.password(password);
    builder.argument(ToolArgument::input(input));
    builder.argument(ToolArgument::output(output));
    Ok(builder.finish())
}

pub(crate) fn transform_plan(
    pdf: Pdf,
    password: Option<String>,
    options: Vec<WriteOption>,
) -> Result<ToolExecutionPlan, QpdfError> {
    let mut builder = PlanBuilder::new();
    let input = builder.input_pdf(pdf);
    let output = builder.output(OutputKind::Single, "pdf");
    compile_write_options(&mut builder, options)?;
    builder.password(password);
    builder.argument(ToolArgument::input(input));
    builder.argument(ToolArgument::output(output));
    Ok(builder.finish())
}

pub(crate) fn pages_plan(
    primary: Option<Pdf>,
    password: Option<String>,
    sources: Vec<PageSource>,
    collate: Option<Vec<u64>>,
    options: Vec<WriteOption>,
) -> Result<ToolExecutionPlan, QpdfError> {
    if sources.is_empty() {
        return Err(invalid("pages requires at least one page source"));
    }
    let mut builder = PlanBuilder::new();
    compile_write_options(&mut builder, options)?;
    match primary {
        Some(pdf) => {
            let input = builder.input_pdf(pdf);
            builder.password(password);
            builder.argument(ToolArgument::input(input));
        }
        None => builder.literal("--empty"),
    }
    if let Some(groups) = collate {
        if groups.is_empty() || groups.contains(&0) {
            return Err(invalid(
                "collate groups must contain one or more positive page counts",
            ));
        }
        builder.literal(format!(
            "--collate={}",
            groups
                .into_iter()
                .map(|group| group.to_string())
                .collect::<Vec<_>>()
                .join(",")
        ));
    }
    builder.literal("--pages");
    for source in sources {
        require_page_range(&source.range, "page source range")?;
        let input = builder.input_pdf(source.pdf);
        builder.argument(ToolArgument::input_decorated(input, "--file=", ""));
        builder.literal(format!("--range={}", source.range));
        if let Some(password) = source.password {
            builder.literal(format!("--password={password}"));
        }
    }
    builder.literal("--");
    let output = builder.output(OutputKind::Single, "pdf");
    builder.argument(ToolArgument::output(output));
    Ok(builder.finish())
}

pub(crate) fn split_pages_plan(
    pdf: Pdf,
    password: Option<String>,
    pages_per_file: u64,
    options: Vec<WriteOption>,
) -> Result<ToolExecutionPlan, QpdfError> {
    if pages_per_file == 0 {
        return Err(invalid("pages_per_file must be greater than zero"));
    }
    let mut builder = PlanBuilder::new();
    let input = builder.input_pdf(pdf);
    let output = builder.output(OutputKind::Directory, "pdf");
    compile_write_options(&mut builder, options)?;
    builder.literal(format!("--split-pages={pages_per_file}"));
    builder.password(password);
    builder.argument(ToolArgument::input(input));
    builder.argument(ToolArgument::output_with_suffix(output, "/page.pdf"));
    Ok(builder.finish())
}

pub(crate) fn overlay_plan(
    underlay: bool,
    pdf: Pdf,
    password: Option<String>,
    other: Pdf,
    spec: OverlaySpec,
    options: Vec<WriteOption>,
) -> Result<ToolExecutionPlan, QpdfError> {
    for (range, context) in [
        (spec.to.as_deref(), "overlay to range"),
        (spec.from.as_deref(), "overlay from range"),
        (spec.repeat.as_deref(), "overlay repeat range"),
    ] {
        if let Some(range) = range {
            require_page_range(range, context)?;
        }
    }
    let mut builder = PlanBuilder::new();
    let input = builder.input_pdf(pdf);
    let other = builder.input_pdf(other);
    let output = builder.output(OutputKind::Single, "pdf");
    compile_write_options(&mut builder, options)?;
    builder.password(password);
    builder.argument(ToolArgument::input(input));
    builder.literal(if underlay { "--underlay" } else { "--overlay" });
    builder.argument(ToolArgument::input(other));
    if let Some(password) = spec.password {
        builder.literal(format!("--password={password}"));
    }
    if let Some(range) = spec.to {
        builder.literal(format!("--to={range}"));
    }
    if let Some(range) = spec.from {
        builder.literal(format!("--from={range}"));
    }
    if let Some(range) = spec.repeat {
        builder.literal(format!("--repeat={range}"));
    }
    builder.literal("--");
    builder.argument(ToolArgument::output(output));
    Ok(builder.finish())
}

fn compile_write_options(
    builder: &mut PlanBuilder,
    options: Vec<WriteOption>,
) -> Result<(), QpdfError> {
    let mut encryption_seen = false;
    let mut decrypt_seen = false;
    for option in options {
        match option {
            WriteOption::Linearize => builder.literal("--linearize"),
            WriteOption::CompressStreams(enabled) => {
                builder.literal(format!("--compress-streams={}", yes_no(enabled)))
            }
            WriteOption::RecompressFlate => builder.literal("--recompress-flate"),
            WriteOption::CompressionLevel(level) => {
                if level > 9 {
                    return Err(invalid("compression level must be between 0 and 9"));
                }
                builder.literal(format!("--compression-level={level}"));
            }
            WriteOption::ObjectStreams(mode) => builder.literal(format!(
                "--object-streams={}",
                match mode {
                    ObjectStreamMode::ObjectStreamsPreserve => "preserve",
                    ObjectStreamMode::ObjectStreamsDisable => "disable",
                    ObjectStreamMode::ObjectStreamsGenerate => "generate",
                }
            )),
            WriteOption::StreamData(mode) => builder.literal(format!(
                "--stream-data={}",
                match mode {
                    StreamDataMode::StreamDataCompress => "compress",
                    StreamDataMode::StreamDataPreserve => "preserve",
                    StreamDataMode::StreamDataUncompress => "uncompress",
                }
            )),
            WriteOption::DecodeStreams(level) => {
                builder.literal(format!("--decode-level={}", decode_level(level)))
            }
            WriteOption::NormalizeContent(enabled) => {
                builder.literal(format!("--normalize-content={}", yes_no(enabled)))
            }
            WriteOption::PreserveUnreferenced => builder.literal("--preserve-unreferenced"),
            WriteOption::RemoveUnreferencedResources(mode) => builder.literal(format!(
                "--remove-unreferenced-resources={}",
                match mode {
                    RemoveUnreferencedResourcesMode::RemoveResourcesAuto => "auto",
                    RemoveUnreferencedResourcesMode::RemoveResourcesYes => "yes",
                    RemoveUnreferencedResourcesMode::RemoveResourcesNo => "no",
                }
            )),
            WriteOption::CoalesceContents => builder.literal("--coalesce-contents"),
            WriteOption::ExternalizeInlineImages => builder.literal("--externalize-inline-images"),
            WriteOption::FlattenRotation => builder.literal("--flatten-rotation"),
            WriteOption::Rotate(spec) => builder.literal(compile_rotation(spec)?),
            WriteOption::GenerateAppearances => builder.literal("--generate-appearances"),
            WriteOption::FlattenAnnotations(mode) => builder.literal(format!(
                "--flatten-annotations={}",
                match mode {
                    FlattenAnnotationsMode::FlattenAllAnnotations => "all",
                    FlattenAnnotationsMode::FlattenPrintAnnotations => "print",
                    FlattenAnnotationsMode::FlattenScreenAnnotations => "screen",
                }
            )),
            WriteOption::RemovePageLabels => builder.literal("--remove-page-labels"),
            WriteOption::DeterministicId => builder.literal("--deterministic-id"),
            WriteOption::StaticId => builder.literal("--static-id"),
            WriteOption::NoOriginalObjectIds => builder.literal("--no-original-object-ids"),
            WriteOption::MinimumVersion(version) => {
                require_pdf_version(&version)?;
                builder.literal(format!("--min-version={version}"));
            }
            WriteOption::ForceVersion(version) => {
                require_pdf_version(&version)?;
                builder.literal(format!("--force-version={version}"));
            }
            WriteOption::Decrypt => {
                if encryption_seen {
                    return Err(invalid("Decrypt and Encrypt cannot be combined"));
                }
                decrypt_seen = true;
                builder.literal("--decrypt");
            }
            WriteOption::RemoveRestrictions => builder.literal("--remove-restrictions"),
            WriteOption::Encrypt(spec) => {
                if encryption_seen {
                    return Err(invalid("only one Encrypt option may be supplied"));
                }
                if decrypt_seen {
                    return Err(invalid("Decrypt and Encrypt cannot be combined"));
                }
                encryption_seen = true;
                compile_encryption(builder, spec)?;
            }
        }
    }
    Ok(())
}

fn compile_encryption(builder: &mut PlanBuilder, spec: EncryptionSpec) -> Result<(), QpdfError> {
    if spec.owner_password.as_deref().is_none_or(str::is_empty) {
        return Err(invalid(
            "modern encryption requires a non-empty owner password",
        ));
    }
    builder.literal("--encrypt");
    if let Some(password) = spec.user_password {
        builder.literal(format!("--user-password={password}"));
    }
    if let Some(password) = spec.owner_password {
        builder.literal(format!("--owner-password={password}"));
    }
    match spec.method {
        EncryptionMethod::EncryptionAes128 => {
            builder.literal("--bits=128");
            builder.literal("--use-aes=y");
        }
        EncryptionMethod::EncryptionAes256 => builder.literal("--bits=256"),
    }
    builder.literal(format!(
        "--print={}",
        match spec.print {
            PrintPermission::PrintNone => "none",
            PrintPermission::PrintLowResolution => "low",
            PrintPermission::PrintFullResolution => "full",
        }
    ));
    builder.literal(format!(
        "--modify={}",
        match spec.modify {
            ModifyPermission::ModifyNone => "none",
            ModifyPermission::ModifyAssembly => "assembly",
            ModifyPermission::ModifyForms => "form",
            ModifyPermission::ModifyAnnotations => "annotate",
            ModifyPermission::ModifyAll => "all",
        }
    ));
    builder.literal(format!("--extract={}", yes_no(spec.extract)));
    builder.literal(format!("--accessibility={}", yes_no(spec.accessibility)));
    builder.literal(format!("--annotate={}", yes_no(spec.annotate)));
    builder.literal(format!("--assemble={}", yes_no(spec.assemble)));
    builder.literal(format!("--form={}", yes_no(spec.form)));
    if spec.cleartext_metadata {
        builder.literal("--cleartext-metadata");
    }
    builder.literal("--");
    Ok(())
}

fn compile_rotation(spec: RotationSpec) -> Result<String, QpdfError> {
    let (prefix, angle) = match spec.rotation {
        Rotation::AbsoluteRotation(angle) if matches!(angle, 0 | 90 | 180 | 270) => ("", angle),
        Rotation::AbsoluteRotation(_) => {
            return Err(invalid("absolute rotation must be 0, 90, 180, or 270"));
        }
        Rotation::RelativeRotation(angle)
            if matches!(angle, -270 | -180 | -90 | 0 | 90 | 180 | 270) =>
        {
            (if angle >= 0 { "+" } else { "" }, angle)
        }
        Rotation::RelativeRotation(_) => {
            return Err(invalid(
                "relative rotation must be a multiple of 90 from -270 to 270",
            ));
        }
    };
    let mut value = format!("--rotate={prefix}{angle}");
    if let Some(pages) = spec.pages {
        require_page_range(&pages, "rotation page range")?;
        value.push(':');
        value.push_str(&pages);
    }
    Ok(value)
}

fn json_key(key: JsonKey) -> &'static str {
    match key {
        JsonKey::JsonAcroform => "acroform",
        JsonKey::JsonAttachments => "attachments",
        JsonKey::JsonEncrypt => "encrypt",
        JsonKey::JsonObjectInfo => "objectinfo",
        JsonKey::JsonObjects => "objects",
        JsonKey::JsonOutlines => "outlines",
        JsonKey::JsonPageLabels => "pagelabels",
        JsonKey::JsonPages => "pages",
        JsonKey::JsonQpdf => "qpdf",
    }
}

fn decode_level(level: DecodeLevel) -> &'static str {
    match level {
        DecodeLevel::DecodeNone => "none",
        DecodeLevel::DecodeGeneralized => "generalized",
        DecodeLevel::DecodeSpecialized => "specialized",
        DecodeLevel::DecodeAll => "all",
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "y" } else { "n" }
}

fn require_page_range(value: &str, context: &str) -> Result<(), QpdfError> {
    if value.is_empty()
        || value.starts_with("--")
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, ',' | '-' | ':')
        })
    {
        Err(invalid(format!("invalid {context} `{value}`")))
    } else {
        Ok(())
    }
}

fn require_pdf_version(value: &str) -> Result<(), QpdfError> {
    if value.is_empty()
        || !value
            .chars()
            .all(|character| character.is_ascii_digit() || matches!(character, '.' | ':'))
    {
        Err(invalid(format!("invalid PDF version `{value}`")))
    } else {
        Ok(())
    }
}

fn require_json_object(value: &str) -> Result<(), QpdfError> {
    if value == "trailer"
        || (!value.is_empty()
            && value
                .chars()
                .all(|character| character.is_ascii_digit() || character == ','))
    {
        Ok(())
    } else {
        Err(invalid(format!("invalid JSON object selector `{value}`")))
    }
}

pub(crate) fn invalid(message: impl Into<String>) -> QpdfError {
    QpdfError {
        kind: QpdfErrorKind::InvalidRequest,
        exit_code: None,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use blake3::hash;

    fn pdf() -> Pdf {
        Pdf {
            content: hash(b"pdf"),
        }
    }

    #[test]
    fn transform_paths_remain_symbolic() {
        let plan = transform_plan(
            pdf(),
            None,
            vec![
                WriteOption::Linearize,
                WriteOption::ObjectStreams(ObjectStreamMode::ObjectStreamsGenerate),
            ],
        )
        .unwrap();
        assert_eq!(plan.program, ToolProgram::Qpdf);
        assert_eq!(plan.inputs.len(), 1);
        assert_eq!(plan.outputs.len(), 1);
        assert!(
            plan.arguments
                .iter()
                .filter(|argument| matches!(argument, ToolArgument::Path { .. }))
                .count()
                >= 2
        );
    }

    #[test]
    fn pages_uses_qpdf_file_and_range_grouping() {
        let plan = pages_plan(
            None,
            None,
            vec![PageSource {
                pdf: pdf(),
                range: "1-z:odd".to_string(),
                password: None,
            }],
            Some(vec![1]),
            vec![],
        )
        .unwrap();
        assert!(plan.arguments.iter().any(
            |argument| matches!(argument, ToolArgument::Literal(value) if value == "--pages")
        ));
        assert!(plan.arguments.iter().any(
            |argument| matches!(argument, ToolArgument::Literal(value) if value == "--range=1-z:odd")
        ));
        assert!(plan.arguments.iter().any(
            |argument| matches!(argument, ToolArgument::Literal(value) if value == "--collate=1")
        ));
    }

    #[test]
    fn insecure_and_conflicting_options_are_rejected() {
        assert!(transform_plan(pdf(), None, vec![WriteOption::CompressionLevel(10)]).is_err());
        assert!(
            pages_plan(
                None,
                None,
                vec![PageSource {
                    pdf: pdf(),
                    range: "1".to_string(),
                    password: None,
                }],
                Some(vec![0]),
                vec![],
            )
            .is_err()
        );
        assert!(
            transform_plan(
                pdf(),
                None,
                vec![WriteOption::Encrypt(EncryptionSpec {
                    user_password: Some("reader".to_string()),
                    owner_password: None,
                    method: EncryptionMethod::EncryptionAes256,
                    print: PrintPermission::PrintNone,
                    modify: ModifyPermission::ModifyNone,
                    extract: false,
                    accessibility: true,
                    annotate: false,
                    assemble: false,
                    form: false,
                    cleartext_metadata: false,
                })],
            )
            .is_err()
        );
        assert!(
            transform_plan(
                pdf(),
                None,
                vec![
                    WriteOption::Decrypt,
                    WriteOption::Encrypt(EncryptionSpec {
                        user_password: None,
                        owner_password: Some("owner".to_string()),
                        method: EncryptionMethod::EncryptionAes256,
                        print: PrintPermission::PrintFullResolution,
                        modify: ModifyPermission::ModifyAll,
                        extract: true,
                        accessibility: true,
                        annotate: true,
                        assemble: true,
                        form: true,
                        cleartext_metadata: false,
                    }),
                ],
            )
            .is_err()
        );
    }
}

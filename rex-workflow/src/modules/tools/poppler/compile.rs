use super::types::*;
use crate::modules::tools::executor::{
    CasInput, ExpectedOutput, InputKind, OutputId, OutputKind, ToolArgument, ToolExecutionPlan,
    ToolProgram,
};

pub(crate) struct PlannedCairoOutput {
    pub raster: bool,
    pub multiple: bool,
}

struct PlanBuilder {
    program: ToolProgram,
    arguments: Vec<ToolArgument>,
    inputs: Vec<CasInput>,
    outputs: Vec<ExpectedOutput>,
}

impl PlanBuilder {
    fn new(program: ToolProgram) -> Self {
        Self {
            program,
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

    fn option(&mut self, name: &str, value: impl Into<String>) {
        self.literal(name);
        self.literal(value);
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

    fn finish(self) -> ToolExecutionPlan {
        ToolExecutionPlan {
            program: self.program,
            arguments: self.arguments,
            inputs: self.inputs,
            outputs: self.outputs,
            stdin: None,
        }
    }
}

pub(crate) fn pdfinfo_plan(
    pdf: Pdf,
    options: PdfInfoOptions,
) -> Result<ToolExecutionPlan, PopplerError> {
    validate_page_range(options.first_page, options.last_page)?;
    let mut builder = PlanBuilder::new(ToolProgram::PdfInfo);
    let input = builder.input_pdf(pdf);
    builder.literal("-box");
    builder.literal("-isodates");
    compile_pages(&mut builder, options.first_page, options.last_page);
    compile_passwords(&mut builder, options.owner_password, options.user_password);
    builder.argument(ToolArgument::input(input));
    Ok(builder.finish())
}

pub(crate) fn pdftotext_plan(
    pdf: Pdf,
    options: PdfToTextOptions,
) -> Result<ToolExecutionPlan, PopplerError> {
    validate_page_range(options.first_page, options.last_page)?;
    validate_positive_f64(options.resolution, "resolution")?;
    validate_positive_f64(options.column_spacing, "column spacing")?;
    let mut builder = PlanBuilder::new(ToolProgram::PdfToText);
    let input = builder.input_pdf(pdf);
    let extension = match options.format {
        TextFormat::PlainText | TextFormat::PhysicalLayout | TextFormat::ContentStreamOrder => {
            "txt"
        }
        TextFormat::HtmlMetadata | TextFormat::BoundingBox | TextFormat::BoundingBoxLayout => {
            "html"
        }
        TextFormat::TabSeparated => "tsv",
    };
    let output = builder.output(OutputKind::Single, extension);
    compile_pages(&mut builder, options.first_page, options.last_page);
    match options.format {
        TextFormat::PlainText => {}
        TextFormat::PhysicalLayout => builder.literal("-layout"),
        TextFormat::ContentStreamOrder => builder.literal("-raw"),
        TextFormat::HtmlMetadata => builder.literal("-htmlmeta"),
        TextFormat::BoundingBox => builder.literal("-bbox"),
        TextFormat::BoundingBoxLayout => builder.literal("-bbox-layout"),
        TextFormat::TabSeparated => builder.literal("-tsv"),
    }
    if let Some(resolution) = options.resolution {
        builder.option("-r", number(resolution));
    }
    if let Some(crop) = options.crop {
        builder.option("-x", crop.x.to_string());
        builder.option("-y", crop.y.to_string());
        builder.option("-W", crop.width.to_string());
        builder.option("-H", crop.height.to_string());
    }
    if options.crop_box {
        builder.literal("-cropbox");
    }
    if options.discard_diagonal_text {
        builder.literal("-nodiag");
    }
    if let Some(spacing) = options.column_spacing {
        builder.option("-colspacing", number(spacing));
    }
    builder.option("-enc", "UTF-8");
    builder.option(
        "-eol",
        match options.end_of_line {
            EndOfLine::EolUnix => "unix",
            EndOfLine::EolDos => "dos",
            EndOfLine::EolMac => "mac",
        },
    );
    if options.no_page_breaks {
        builder.literal("-nopgbrk");
    }
    compile_passwords(&mut builder, options.owner_password, options.user_password);
    builder.argument(ToolArgument::input(input));
    builder.argument(ToolArgument::output(output));
    Ok(builder.finish())
}

pub(crate) fn pdftocairo_plan(
    pdf: Pdf,
    format: CairoFormat,
    options: PdfToCairoOptions,
) -> Result<(ToolExecutionPlan, PlannedCairoOutput), PopplerError> {
    validate_page_range(options.first_page, options.last_page)?;
    for (value, context) in [
        (options.resolution, "resolution"),
        (options.resolution_x, "X resolution"),
        (options.resolution_y, "Y resolution"),
    ] {
        validate_positive_f64(value, context)?;
    }
    if options.resolution.is_some()
        && (options.resolution_x.is_some() || options.resolution_y.is_some())
    {
        return Err(invalid(
            "resolution cannot be combined with resolution_x or resolution_y",
        ));
    }
    if options.scale_to_x.is_some_and(|value| value < -1)
        || options.scale_to_y.is_some_and(|value| value < -1)
    {
        return Err(invalid("scale_to_x and scale_to_y must be -1 or greater"));
    }
    if options.scale_to == Some(0) || options.scale_to_x == Some(0) || options.scale_to_y == Some(0)
    {
        return Err(invalid("scale dimensions must be positive or -1"));
    }
    if options.scale_to.is_some() && (options.scale_to_x.is_some() || options.scale_to_y.is_some())
    {
        return Err(invalid(
            "scale_to cannot be combined with scale_to_x or scale_to_y",
        ));
    }
    let raster = matches!(
        format,
        CairoFormat::CairoPng | CairoFormat::CairoJpeg | CairoFormat::CairoTiff
    );
    if options.single_file && !raster {
        return Err(invalid(
            "single_file is available only for PNG, JPEG, and TIFF",
        ));
    }
    if matches!(
        format,
        CairoFormat::CairoSvg | CairoFormat::CairoEncapsulatedPostScript
    ) && !(options.first_page.is_some() && options.first_page == options.last_page)
    {
        return Err(invalid(
            "SVG and EPS output require the same explicit first_page and last_page",
        ));
    }
    if options.transparent && format != CairoFormat::CairoPng {
        return Err(invalid("transparent output is available only for PNG"));
    }
    if options.color != CairoColorMode::CairoColor
        && !matches!(format, CairoFormat::CairoPng | CairoFormat::CairoJpeg)
    {
        return Err(invalid(
            "grayscale and monochrome modes are available only for PNG and JPEG",
        ));
    }
    if !options.jpeg_options.is_empty() && format != CairoFormat::CairoJpeg {
        return Err(invalid("jpeg_options require CairoJpeg output"));
    }
    if options.tiff_compression.is_some() && format != CairoFormat::CairoTiff {
        return Err(invalid("tiff_compression requires CairoTiff output"));
    }
    let multiple = raster && !options.single_file;

    let mut builder = PlanBuilder::new(ToolProgram::PdfToCairo);
    let input = builder.input_pdf(pdf);
    let extension = cairo_extension(&format);
    let output = builder.output(
        if raster {
            OutputKind::Tree
        } else {
            OutputKind::Single
        },
        extension,
    );
    builder.literal(cairo_flag(&format));
    compile_pages(&mut builder, options.first_page, options.last_page);
    match options.page_selection {
        PageSelection::AllPages => {}
        PageSelection::OddPages => builder.literal("-o"),
        PageSelection::EvenPages => builder.literal("-e"),
    }
    if options.single_file {
        builder.literal("-singlefile");
    }
    if let Some(value) = options.resolution {
        builder.option("-r", number(value));
    }
    if let Some(value) = options.resolution_x {
        builder.option("-rx", number(value));
    }
    if let Some(value) = options.resolution_y {
        builder.option("-ry", number(value));
    }
    if let Some(value) = options.scale_to {
        builder.option("-scale-to", value.to_string());
    }
    if let Some(value) = options.scale_to_x {
        builder.option("-scale-to-x", value.to_string());
    }
    if let Some(value) = options.scale_to_y {
        builder.option("-scale-to-y", value.to_string());
    }
    if let Some(crop) = options.crop {
        builder.option("-x", crop.x.to_string());
        builder.option("-y", crop.y.to_string());
        builder.option("-W", crop.width.to_string());
        builder.option("-H", crop.height.to_string());
    }
    if options.crop_box {
        builder.literal("-cropbox");
    }
    match options.color {
        CairoColorMode::CairoColor => {}
        CairoColorMode::CairoGrayscale => builder.literal("-gray"),
        CairoColorMode::CairoMonochrome => builder.literal("-mono"),
    }
    if options.transparent {
        builder.literal("-transp");
    }
    builder.option("-antialias", antialias(options.antialias));
    if !options.jpeg_options.is_empty() {
        for option in &options.jpeg_options {
            require_name_value(option, "JPEG option")?;
        }
        builder.option("-jpegopt", options.jpeg_options.join(","));
    }
    if let Some(compression) = options.tiff_compression {
        require_name(&compression, "TIFF compression")?;
        builder.option("-tiffcompression", compression);
    }
    compile_passwords(&mut builder, options.owner_password, options.user_password);
    builder.argument(ToolArgument::input(input));
    if raster {
        builder.argument(ToolArgument::output_with_suffix(output, "/page"));
    } else {
        builder.argument(ToolArgument::output(output));
    }
    Ok((builder.finish(), PlannedCairoOutput { raster, multiple }))
}

pub(crate) fn pdfimages_plan(
    pdf: Pdf,
    options: PdfImagesOptions,
) -> Result<ToolExecutionPlan, PopplerError> {
    validate_page_range(options.first_page, options.last_page)?;
    let mut builder = PlanBuilder::new(ToolProgram::PdfImages);
    let input = builder.input_pdf(pdf);
    let output = builder.output(OutputKind::Tree, "images");
    compile_pdfimages_options(&mut builder, &options);
    builder.argument(ToolArgument::input(input));
    builder.argument(ToolArgument::output_with_suffix(output, "/image"));
    Ok(builder.finish())
}

pub(crate) fn pdfimages_list_plan(
    pdf: Pdf,
    options: PdfImagesOptions,
) -> Result<ToolExecutionPlan, PopplerError> {
    validate_page_range(options.first_page, options.last_page)?;
    let mut builder = PlanBuilder::new(ToolProgram::PdfImages);
    let input = builder.input_pdf(pdf);
    builder.literal("-list");
    compile_pdfimages_options(&mut builder, &options);
    builder.argument(ToolArgument::input(input));
    Ok(builder.finish())
}

pub(crate) fn version_plan() -> ToolExecutionPlan {
    let mut builder = PlanBuilder::new(ToolProgram::PdfInfo);
    builder.literal("-v");
    builder.finish()
}

fn compile_pdfimages_options(builder: &mut PlanBuilder, options: &PdfImagesOptions) {
    compile_pages(builder, options.first_page, options.last_page);
    match options.format {
        PdfImagesFormat::ImagesDefault => {}
        PdfImagesFormat::ImagesPng => builder.literal("-png"),
        PdfImagesFormat::ImagesTiff => builder.literal("-tiff"),
        PdfImagesFormat::ImagesJpegNative => builder.literal("-j"),
        PdfImagesFormat::ImagesJpeg2000Native => builder.literal("-jp2"),
        PdfImagesFormat::ImagesJbig2Native => builder.literal("-jbig2"),
        PdfImagesFormat::ImagesCcittNative => builder.literal("-ccitt"),
        PdfImagesFormat::ImagesAll => builder.literal("-all"),
    }
    if options.include_page_numbers {
        builder.literal("-p");
    }
    compile_passwords(
        builder,
        options.owner_password.clone(),
        options.user_password.clone(),
    );
}

fn compile_pages(builder: &mut PlanBuilder, first: Option<u64>, last: Option<u64>) {
    if let Some(page) = first {
        builder.option("-f", page.to_string());
    }
    if let Some(page) = last {
        builder.option("-l", page.to_string());
    }
}

fn compile_passwords(
    builder: &mut PlanBuilder,
    owner_password: Option<String>,
    user_password: Option<String>,
) {
    if let Some(password) = owner_password {
        builder.option("-opw", password);
    }
    if let Some(password) = user_password {
        builder.option("-upw", password);
    }
}

fn validate_page_range(first: Option<u64>, last: Option<u64>) -> Result<(), PopplerError> {
    if first == Some(0) || last == Some(0) {
        return Err(invalid("Poppler page numbers are one-based"));
    }
    if let (Some(first), Some(last)) = (first, last)
        && last < first
    {
        return Err(invalid("last_page must not precede first_page"));
    }
    Ok(())
}

fn validate_positive_f64(value: Option<f64>, context: &str) -> Result<(), PopplerError> {
    if value.is_some_and(|value| !value.is_finite() || value <= 0.0) {
        Err(invalid(format!(
            "{context} must be finite and greater than zero"
        )))
    } else {
        Ok(())
    }
}

fn cairo_flag(format: &CairoFormat) -> &'static str {
    match format {
        CairoFormat::CairoPng => "-png",
        CairoFormat::CairoJpeg => "-jpeg",
        CairoFormat::CairoTiff => "-tiff",
        CairoFormat::CairoPdf => "-pdf",
        CairoFormat::CairoPostScript => "-ps",
        CairoFormat::CairoEncapsulatedPostScript => "-eps",
        CairoFormat::CairoSvg => "-svg",
    }
}

fn cairo_extension(format: &CairoFormat) -> &'static str {
    match format {
        CairoFormat::CairoPng => "png",
        CairoFormat::CairoJpeg => "jpg",
        CairoFormat::CairoTiff => "tiff",
        CairoFormat::CairoPdf => "pdf",
        CairoFormat::CairoPostScript => "ps",
        CairoFormat::CairoEncapsulatedPostScript => "eps",
        CairoFormat::CairoSvg => "svg",
    }
}

fn antialias(value: CairoAntialias) -> &'static str {
    match value {
        CairoAntialias::AntialiasDefault => "default",
        CairoAntialias::AntialiasNone => "none",
        CairoAntialias::AntialiasGray => "gray",
        CairoAntialias::AntialiasSubpixel => "subpixel",
        CairoAntialias::AntialiasFast => "fast",
        CairoAntialias::AntialiasGood => "good",
        CairoAntialias::AntialiasBest => "best",
    }
}

fn require_name(value: &str, context: &str) -> Result<(), PopplerError> {
    if value.is_empty()
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        Err(invalid(format!("invalid {context} `{value}`")))
    } else {
        Ok(())
    }
}

fn require_name_value(value: &str, context: &str) -> Result<(), PopplerError> {
    if value.is_empty()
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | '=')
        })
    {
        Err(invalid(format!("invalid {context} `{value}`")))
    } else {
        Ok(())
    }
}

fn number(value: f64) -> String {
    value.to_string()
}

pub(crate) fn invalid(message: impl Into<String>) -> PopplerError {
    PopplerError {
        kind: PopplerErrorKind::InvalidRequest,
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
    fn pdftotext_uses_symbolic_paths_and_documented_flags() {
        let plan = pdftotext_plan(
            pdf(),
            PdfToTextOptions {
                first_page: Some(2),
                last_page: Some(4),
                format: TextFormat::TabSeparated,
                resolution: None,
                crop: None,
                crop_box: true,
                discard_diagonal_text: false,
                column_spacing: Some(0.7),
                end_of_line: EndOfLine::EolUnix,
                no_page_breaks: true,
                owner_password: None,
                user_password: None,
            },
        )
        .unwrap();
        assert_eq!(plan.program, ToolProgram::PdfToText);
        assert!(
            plan.arguments.iter().any(
                |argument| matches!(argument, ToolArgument::Literal(value) if value == "-tsv")
            )
        );
        assert_eq!(
            plan.arguments
                .iter()
                .filter(|argument| matches!(argument, ToolArgument::Path { .. }))
                .count(),
            2
        );
    }

    #[test]
    fn raster_cairo_output_uses_a_private_output_directory() {
        let (plan, output) = pdftocairo_plan(
            pdf(),
            CairoFormat::CairoPng,
            PdfToCairoOptions {
                first_page: None,
                last_page: None,
                page_selection: PageSelection::AllPages,
                single_file: false,
                resolution: Some(144.0),
                resolution_x: None,
                resolution_y: None,
                scale_to: None,
                scale_to_x: None,
                scale_to_y: None,
                crop: None,
                crop_box: false,
                color: CairoColorMode::CairoColor,
                transparent: true,
                antialias: CairoAntialias::AntialiasDefault,
                jpeg_options: vec![],
                tiff_compression: None,
                owner_password: None,
                user_password: None,
            },
        )
        .unwrap();
        assert!(output.multiple);
        assert!(output.raster);
        assert_eq!(plan.outputs[0].kind, OutputKind::Tree);
    }

    #[test]
    fn invalid_page_and_format_combinations_are_rejected() {
        let options = PdfToCairoOptions {
            first_page: None,
            last_page: None,
            page_selection: PageSelection::AllPages,
            single_file: true,
            resolution: None,
            resolution_x: None,
            resolution_y: None,
            scale_to: None,
            scale_to_x: None,
            scale_to_y: None,
            crop: None,
            crop_box: false,
            color: CairoColorMode::CairoColor,
            transparent: false,
            antialias: CairoAntialias::AntialiasDefault,
            jpeg_options: vec![],
            tiff_compression: None,
            owner_password: None,
            user_password: None,
        };
        assert!(pdftocairo_plan(pdf(), CairoFormat::CairoPdf, options).is_err());
    }
}

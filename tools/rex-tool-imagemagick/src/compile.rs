use super::types::*;
use crate::modules::tools::executor::{
    CasInput, ExpectedOutput, InputKind, OutputId, OutputKind, ToolArgument, ToolExecutionPlan,
    ToolProgram,
};

pub(crate) struct PlanBuilder {
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

    fn option(&mut self, name: &str, value: impl Into<String>) {
        self.literal(name);
        self.literal(value);
    }

    fn input(&mut self, hash: blake3::Hash, extension: impl Into<String>) -> usize {
        let id = self.inputs.len();
        self.inputs.push(CasInput {
            hash,
            extension: extension.into(),
            kind: InputKind::Blob,
        });
        id
    }

    fn input_argument(&mut self, id: usize, prefix: &str, suffix: &str) {
        self.arguments
            .push(ToolArgument::input_decorated(id, prefix, suffix));
    }

    fn input_path(&mut self, id: usize) {
        self.arguments.push(ToolArgument::input(id));
    }

    fn output(&mut self, kind: OutputKind, extension: impl Into<String>) -> OutputId {
        let id = self.outputs.len();
        self.outputs.push(ExpectedOutput {
            kind,
            extension: extension.into(),
        });
        id
    }

    fn output_argument(&mut self, id: OutputId, coder: &str) {
        self.arguments
            .push(ToolArgument::output_decorated(id, format!("{coder}:")));
    }

    fn output_path(&mut self, id: OutputId) {
        self.arguments.push(ToolArgument::output(id));
    }

    fn finish(self) -> ToolExecutionPlan {
        ToolExecutionPlan {
            program: self.program,
            arguments: self.arguments,
            inputs: self.inputs,
            outputs: self.outputs,
        }
    }
}

pub(crate) fn render_plan(
    instructions: Vec<ImageInstruction>,
    encoding: Encoding,
) -> Result<ToolExecutionPlan, ImageMagickError> {
    let mut builder = PlanBuilder::new(ToolProgram::new("magick"));
    for instruction in instructions {
        compile_instruction(&mut builder, instruction)?;
    }
    compile_encoding(&mut builder, encoding)?;
    Ok(builder.finish())
}

pub(crate) fn transform_plan(
    source: ImageSource,
    operations: Vec<ImageOperation>,
    encoding: Encoding,
) -> Result<ToolExecutionPlan, ImageMagickError> {
    let mut builder = PlanBuilder::new(ToolProgram::new("magick"));
    compile_source(&mut builder, source)?;
    for operation in operations {
        compile_operation(&mut builder, operation)?;
    }
    compile_encoding(&mut builder, encoding)?;
    Ok(builder.finish())
}

pub(crate) fn transform_many_plan(
    images: Vec<Image>,
    operations: Vec<ImageOperation>,
    encoding: Encoding,
) -> Result<ToolExecutionPlan, ImageMagickError> {
    if images.is_empty() {
        return Err(invalid("transform_many requires at least one image"));
    }
    if encoding.mode != OutputMode::Adjoin {
        return Err(invalid(
            "transform_many requires AdjoinFrames; each input already produces a separate output",
        ));
    }
    let format = format_name(&encoding.format)?;
    let mut builder = PlanBuilder::new(ToolProgram::with_prefix_arguments("magick", ["mogrify"]));
    for operation in operations {
        compile_operation(&mut builder, operation)?;
    }
    compile_write_options(&mut builder, encoding.options)?;
    builder.option("-format", &format);
    let output = builder.output(OutputKind::Directory, &format);
    builder.literal("-path");
    builder.output_path(output);
    for image in images {
        let input = builder.input(image.content, "bin");
        builder.input_path(input);
    }
    Ok(builder.finish())
}

pub(crate) fn identify_plan(
    image: Image,
    options: Vec<IdentifyOption>,
) -> Result<ToolExecutionPlan, ImageMagickError> {
    let mut builder = PlanBuilder::new(ToolProgram::with_prefix_arguments("magick", ["identify"]));
    for option in options {
        match option {
            IdentifyOption::Ping => builder.literal("-ping"),
            IdentifyOption::Verbose => builder.literal("-verbose"),
            IdentifyOption::Features(distance) => builder.option("-features", distance.to_string()),
            IdentifyOption::Moments => builder.literal("-moments"),
        }
    }
    builder.option(
        "-format",
        "%m\t%[mime:type]\t%w\t%h\t%p\t%z\t%[colorspace]\t%[channels]\t%[orientation]\\n",
    );
    let input = builder.input(image.content, "bin");
    builder.input_path(input);
    Ok(builder.finish())
}

pub(crate) fn compare_plan(
    first: Image,
    second: Image,
    metric: &ComparisonMetric,
    options: Vec<CompareOption>,
) -> Result<ToolExecutionPlan, ImageMagickError> {
    let mut builder = PlanBuilder::new(ToolProgram::with_prefix_arguments("magick", ["compare"]));
    for option in options {
        match option {
            CompareOption::Fuzz(value) => builder.option("-fuzz", value),
            CompareOption::HighlightColor(color) => builder.option("-highlight-color", color.value),
            CompareOption::LowlightColor(color) => builder.option("-lowlight-color", color.value),
            CompareOption::Compose(operator) => builder.option("-compose", compose_name(&operator)),
            CompareOption::Channels(channels) => {
                builder.option("-channel", channel_list(&channels))
            }
        }
    }
    builder.option("-metric", metric_name(metric));
    let first = builder.input(first.content, "bin");
    let second = builder.input(second.content, "bin");
    builder.input_path(first);
    builder.input_path(second);
    let output = builder.output(OutputKind::Single, "png");
    builder.output_argument(output, "png");
    Ok(builder.finish())
}

pub(crate) fn composite_plan(
    background: Image,
    overlay: Image,
    mask: Option<Image>,
    operator: ComposeOperator,
    options: Vec<CompositeOption>,
    encoding: Encoding,
) -> Result<ToolExecutionPlan, ImageMagickError> {
    let mut builder = PlanBuilder::new(ToolProgram::with_prefix_arguments("magick", ["composite"]));
    builder.option("-compose", compose_name(&operator));
    for option in options {
        match option {
            CompositeOption::Gravity(gravity) => builder.option("-gravity", gravity_name(&gravity)),
            CompositeOption::Geometry(rectangle) => {
                builder.option("-geometry", rectangle_geometry(rectangle))
            }
            CompositeOption::Blend(value) => builder.option("-blend", value),
            CompositeOption::Dissolve(value) => builder.option("-dissolve", value),
            CompositeOption::Tile => builder.literal("-tile"),
            CompositeOption::Clamp => builder.literal("-clamp"),
            CompositeOption::Define(define) => compile_define(&mut builder, define),
        }
    }
    let overlay = builder.input(overlay.content, "bin");
    let background = builder.input(background.content, "bin");
    builder.input_path(overlay);
    builder.input_path(background);
    if let Some(mask) = mask {
        let mask = builder.input(mask.content, "bin");
        builder.input_path(mask);
    }
    compile_encoding(&mut builder, encoding)?;
    Ok(builder.finish())
}

pub(crate) fn montage_plan(
    images: Vec<Image>,
    layout: MontageLayout,
    options: Vec<MontageOption>,
    encoding: Encoding,
) -> Result<ToolExecutionPlan, ImageMagickError> {
    if images.is_empty() {
        return Err(invalid("montage requires at least one image"));
    }
    let mut builder = PlanBuilder::new(ToolProgram::with_prefix_arguments("magick", ["montage"]));
    builder.literal("+label");
    match layout {
        MontageLayout::Automatic => {}
        MontageLayout::Columns(columns) => builder.option("-tile", format!("{columns}x")),
        MontageLayout::Rows(rows) => builder.option("-tile", format!("x{rows}")),
        MontageLayout::Grid(columns, rows) => builder.option("-tile", format!("{columns}x{rows}")),
    }
    for option in options {
        match option {
            MontageOption::Geometry(value) => builder.option("-geometry", value),
            MontageOption::Gravity(gravity) => builder.option("-gravity", gravity_name(&gravity)),
            MontageOption::Background(color) => builder.option("-background", color.value),
            MontageOption::Border(width, color) => {
                builder.option("-bordercolor", color.value);
                builder.option("-border", width.to_string());
            }
            MontageOption::Frame(value) => builder.option("-frame", value),
            MontageOption::Shadow => builder.literal("-shadow"),
            MontageOption::Label(value) => builder.option("-label", value),
            MontageOption::Font(hash) => {
                let font = builder.input(hash, "font");
                builder.literal("-font");
                builder.input_path(font);
            }
            MontageOption::PointSize(value) => builder.option("-pointsize", number(value)),
            MontageOption::Spacing(size) => {
                builder.option("-geometry", format!("+{}+{}", size.width, size.height))
            }
        }
    }
    for image in images {
        let input = builder.input(image.content, "bin");
        builder.input_path(input);
    }
    compile_encoding(&mut builder, encoding)?;
    Ok(builder.finish())
}

pub(crate) fn stream_plan(
    image: Image,
    spec: &PixelSpec,
) -> Result<ToolExecutionPlan, ImageMagickError> {
    if spec.channels.is_empty() {
        return Err(invalid("extract_pixels requires at least one channel"));
    }
    let mut builder = PlanBuilder::new(ToolProgram::with_prefix_arguments("magick", ["stream"]));
    if let PixelRegion::Rectangle(rectangle) = spec.region {
        builder.option("-extract", rectangle_geometry(rectangle));
    }
    builder.option("-map", stream_channel_map(&spec.channels));
    builder.option("-storage-type", pixel_storage_name(&spec.storage_type));
    let input = builder.input(image.content, "bin");
    builder.input_path(input);
    let output = builder.output(OutputKind::Single, "bin");
    builder.output_path(output);
    Ok(builder.finish())
}

pub(crate) fn version_plan() -> ToolExecutionPlan {
    let mut builder = PlanBuilder::new(ToolProgram::new("magick"));
    builder.literal("-version");
    builder.finish()
}

pub(crate) fn formats_plan() -> ToolExecutionPlan {
    let mut builder = PlanBuilder::new(ToolProgram::new("magick"));
    builder.literal("-list");
    builder.literal("format");
    builder.finish()
}

pub(crate) fn capabilities_plan(domain: CapabilityDomain) -> ToolExecutionPlan {
    let mut builder = PlanBuilder::new(ToolProgram::new("magick"));
    builder.literal("-list");
    builder.literal(capability_name(&domain));
    builder.finish()
}

fn compile_instruction(
    builder: &mut PlanBuilder,
    instruction: ImageInstruction,
) -> Result<(), ImageMagickError> {
    match instruction {
        ImageInstruction::Read(source) => compile_source(builder, source),
        ImageInstruction::Setting(setting) => compile_setting(builder, setting),
        ImageInstruction::Operation(operation) => compile_operation(builder, operation),
        ImageInstruction::Group(operations) => {
            builder.literal("(");
            for operation in operations {
                compile_operation(builder, operation)?;
            }
            builder.literal(")");
            Ok(())
        }
        ImageInstruction::Sequence(operation) => {
            compile_sequence(builder, operation);
            Ok(())
        }
    }
}

fn compile_source(builder: &mut PlanBuilder, source: ImageSource) -> Result<(), ImageMagickError> {
    match source {
        ImageSource::Stored(image, frames, options) => {
            let mut format_hint = None;
            for option in options {
                match option {
                    ReadOption::Density(resolution) => {
                        builder.option("-density", resolution_geometry(resolution))
                    }
                    ReadOption::Colorspace(colorspace) => {
                        builder.option("-colorspace", colorspace_name(&colorspace))
                    }
                    ReadOption::Depth(depth) => builder.option("-depth", depth.to_string()),
                    ReadOption::Page(page) => builder.option("-page", rectangle_geometry(page)),
                    ReadOption::Size(size) => builder.option("-size", size_geometry(size)),
                    ReadOption::Alpha(mode) => builder.option("-alpha", alpha_name(&mode)),
                    ReadOption::Background(color) => builder.option("-background", color.value),
                    ReadOption::Profile(hash) => {
                        let profile = builder.input(hash, "profile");
                        builder.literal("-profile");
                        builder.input_path(profile);
                    }
                    ReadOption::Define(define) => compile_define(builder, define),
                    ReadOption::FormatHint(format) => format_hint = Some(format_name(&format)?),
                }
            }
            let extension = format_hint.clone().unwrap_or_else(|| "bin".to_string());
            let input = builder.input(image.content, extension);
            let prefix = format_hint
                .map(|format| format!("{format}:"))
                .unwrap_or_default();
            builder.input_argument(input, &prefix, &frame_suffix(&frames));
        }
        ImageSource::Canvas(size, color) => {
            builder.option("-size", size_geometry(size));
            builder.literal(format!("xc:{}", color.value));
        }
        ImageSource::LinearGradient(size, first, second) => {
            builder.option("-size", size_geometry(size));
            builder.literal(format!("gradient:{}-{}", first.value, second.value));
        }
        ImageSource::RadialGradient(size, first, second) => {
            builder.option("-size", size_geometry(size));
            builder.literal(format!("radial-gradient:{}-{}", first.value, second.value));
        }
        ImageSource::Checkerboard(size) => {
            builder.option("-size", size_geometry(size));
            builder.literal("pattern:checkerboard");
        }
        ImageSource::Noise(size, noise) => {
            builder.option("-size", size_geometry(size));
            builder.literal("xc:gray");
            builder.option("+noise", noise_name(&noise));
        }
        ImageSource::Builtin(kind) => builder.literal(builtin_name(&kind)),
    }
    Ok(())
}

fn compile_setting(
    builder: &mut PlanBuilder,
    setting: ImageSetting,
) -> Result<(), ImageMagickError> {
    match setting {
        ImageSetting::Antialias(enabled) => {
            builder.literal(if enabled { "-antialias" } else { "+antialias" })
        }
        ImageSetting::Authenticate(password) => builder.option("-authenticate", password),
        ImageSetting::Bias(value) => builder.option("-bias", value),
        ImageSetting::Background(color) => builder.option("-background", color.value),
        ImageSetting::BorderColor(color) => builder.option("-bordercolor", color.value),
        ImageSetting::Fill(color) => builder.option("-fill", color.value),
        ImageSetting::Stroke(color) => builder.option("-stroke", color.value),
        ImageSetting::StrokeWidth(width) => builder.option("-strokewidth", number(width)),
        ImageSetting::Font(hash) => {
            let font = builder.input(hash, "font");
            builder.literal("-font");
            builder.input_path(font);
        }
        ImageSetting::PointSize(size) => builder.option("-pointsize", number(size)),
        ImageSetting::Gravity(gravity) => builder.option("-gravity", gravity_name(&gravity)),
        ImageSetting::Filter(filter) => builder.option("-filter", filter_name(&filter)),
        ImageSetting::Density(value) => builder.option("-density", resolution_geometry(value)),
        ImageSetting::Depth(value) => builder.option("-depth", value.to_string()),
        ImageSetting::Direction(value) => builder.option("-direction", value),
        ImageSetting::Dispose(value) => builder.option("-dispose", value),
        ImageSetting::Dither(value) => builder.option("-dither", value),
        ImageSetting::Endian(value) => builder.option("-endian", value),
        ImageSetting::Intent(value) => builder.option("-intent", value),
        ImageSetting::Interpolate(value) => builder.option("-interpolate", value),
        ImageSetting::Label(value) => builder.option("-label", value),
        ImageSetting::Page(value) => builder.option("-page", rectangle_geometry(value)),
        ImageSetting::Precision(value) => builder.option("-precision", value.to_string()),
        ImageSetting::Quality(value) => builder.option("-quality", value.to_string()),
        ImageSetting::SamplingFactor(value) => builder.option("-sampling-factor", value),
        ImageSetting::Scene(value) => builder.option("-scene", value.to_string()),
        ImageSetting::Support(value) => builder.option("-support", number(value)),
        ImageSetting::Units(value) => builder.option("-units", value),
        ImageSetting::VirtualPixel(method) => builder.option("-virtual-pixel", method),
        ImageSetting::Seed(seed) => builder.option("-seed", seed.to_string()),
        ImageSetting::Fuzz(value) => builder.option("-fuzz", value),
        ImageSetting::Define(define) => compile_define(builder, define),
    }
    Ok(())
}

fn compile_operation(
    builder: &mut PlanBuilder,
    operation: ImageOperation,
) -> Result<(), ImageMagickError> {
    match operation {
        ImageOperation::AutoGamma => builder.literal("-auto-gamma"),
        ImageOperation::AutoLevel => builder.literal("-auto-level"),
        ImageOperation::AutoOrient => builder.literal("-auto-orient"),
        ImageOperation::AutoThreshold(method) => builder.option("-auto-threshold", method),
        ImageOperation::Resize(geometry) => builder.option("-resize", resize_geometry(&geometry)),
        ImageOperation::AdaptiveResize(size) => {
            builder.option("-adaptive-resize", size_geometry(size))
        }
        ImageOperation::InterpolativeResize(size) => {
            builder.option("-interpolative-resize", size_geometry(size))
        }
        ImageOperation::Crop(rectangle) => builder.option("-crop", rectangle_geometry(rectangle)),
        ImageOperation::Extent(rectangle, gravity, color) => {
            builder.option("-gravity", gravity_name(&gravity));
            builder.option("-background", color.value);
            builder.option("-extent", rectangle_geometry(rectangle));
        }
        ImageOperation::Extract(rectangle) => {
            builder.option("-extract", rectangle_geometry(rectangle))
        }
        ImageOperation::Chop(rectangle) => builder.option("-chop", rectangle_geometry(rectangle)),
        ImageOperation::Shave(size) => builder.option("-shave", size_geometry(size)),
        ImageOperation::Trim => builder.literal("-trim"),
        ImageOperation::Rotate(degrees) => builder.option("-rotate", number(degrees)),
        ImageOperation::Shear(x, y) => {
            builder.option("-shear", format!("{}x{}", number(x), number(y)))
        }
        ImageOperation::Roll(x, y) => builder.option("-roll", signed_offset(x, y)),
        ImageOperation::Flip => builder.literal("-flip"),
        ImageOperation::Flop => builder.literal("-flop"),
        ImageOperation::Transpose => builder.literal("-transpose"),
        ImageOperation::Transverse => builder.literal("-transverse"),
        ImageOperation::Scale(size) => builder.option("-scale", size_geometry(size)),
        ImageOperation::Sample(size) => builder.option("-sample", size_geometry(size)),
        ImageOperation::LiquidRescale(size) => {
            builder.option("-liquid-rescale", size_geometry(size))
        }
        ImageOperation::Blur(value) => builder.option("-blur", blur_geometry(value)),
        ImageOperation::BilateralBlur(value) => builder.option("-bilateral-blur", value),
        ImageOperation::AdaptiveBlur(value) => {
            builder.option("-adaptive-blur", blur_geometry(value))
        }
        ImageOperation::GaussianBlur(value) => {
            builder.option("-gaussian-blur", blur_geometry(value))
        }
        ImageOperation::MotionBlur(radius, sigma, angle) => builder.option(
            "-motion-blur",
            format!("{}x{}+{}", number(radius), number(sigma), number(angle)),
        ),
        ImageOperation::RotationalBlur(angle) => builder.option("-rotational-blur", number(angle)),
        ImageOperation::SelectiveBlur(radius, sigma, threshold) => builder.option(
            "-selective-blur",
            format!("{}x{}+{threshold}", number(radius), number(sigma)),
        ),
        ImageOperation::Sharpen(value) => builder.option("-sharpen", blur_geometry(value)),
        ImageOperation::AdaptiveSharpen(value) => {
            builder.option("-adaptive-sharpen", blur_geometry(value))
        }
        ImageOperation::Median(radius) => builder.option("-median", number(radius)),
        ImageOperation::Despeckle => builder.literal("-despeckle"),
        ImageOperation::Enhance => builder.literal("-enhance"),
        ImageOperation::Edge(radius) => builder.option("-edge", number(radius)),
        ImageOperation::BlueShift(factor) => builder.option("-blue-shift", number(factor)),
        ImageOperation::Clahe(value) => builder.option("-clahe", value),
        ImageOperation::Clamp => builder.literal("-clamp"),
        ImageOperation::Canny(radius, sigma, lower, upper) => builder.option(
            "-canny",
            format!("{}x{}+{lower}+{upper}", number(radius), number(sigma)),
        ),
        ImageOperation::Emboss(value) => builder.option("-emboss", blur_geometry(value)),
        ImageOperation::Charcoal(radius) => builder.option("-charcoal", number(radius)),
        ImageOperation::Sketch(radius, sigma, angle) => builder.option(
            "-sketch",
            format!("{}x{}+{}", number(radius), number(sigma), number(angle)),
        ),
        ImageOperation::AddNoise(kind, attenuation) => {
            builder.option("-attenuate", number(attenuation));
            builder.option("+noise", noise_name(&kind));
        }
        ImageOperation::ReduceNoise(radius) => builder.option("-noise", number(radius)),
        ImageOperation::Gamma(value) => builder.option("-gamma", number(value)),
        ImageOperation::Level(value) => builder.option("-level", value),
        ImageOperation::BrightnessContrast(brightness, contrast) => builder.option(
            "-brightness-contrast",
            format!("{}x{}", number(brightness), number(contrast)),
        ),
        ImageOperation::SigmoidalContrast(direction, contrast, midpoint) => builder.option(
            match direction {
                BoolValue::Enabled => "-sigmoidal-contrast",
                BoolValue::Disabled => "+sigmoidal-contrast",
            },
            format!("{}x{}", number(contrast), number(midpoint)),
        ),
        ImageOperation::ContrastStretch(value) => builder.option("-contrast-stretch", value),
        ImageOperation::LinearStretch(value) => builder.option("-linear-stretch", value),
        ImageOperation::Normalize => builder.literal("-normalize"),
        ImageOperation::Equalize => builder.literal("-equalize"),
        ImageOperation::Contrast(increase) => {
            builder.literal(if increase { "-contrast" } else { "+contrast" })
        }
        ImageOperation::Threshold(value) => builder.option("-threshold", value),
        ImageOperation::AdaptiveThreshold(size, offset) => builder.option(
            "-adaptive-threshold",
            format!("{}+{offset}", size_geometry(size)),
        ),
        ImageOperation::BlackThreshold(value) => builder.option("-black-threshold", value),
        ImageOperation::WhiteThreshold(value) => builder.option("-white-threshold", value),
        ImageOperation::Colorize(amount, color) => {
            builder.option("-fill", color.value);
            builder.option("-colorize", amount);
        }
        ImageOperation::Modulate(brightness, saturation, hue) => builder.option(
            "-modulate",
            format!(
                "{},{},{}",
                number(brightness),
                number(saturation),
                number(hue)
            ),
        ),
        ImageOperation::SepiaTone(value) => builder.option("-sepia-tone", value),
        ImageOperation::Solarize(value) => builder.option("-solarize", value),
        ImageOperation::Negate => builder.literal("-negate"),
        ImageOperation::Grayscale(method) => {
            builder.option("-grayscale", intensity_method_name(&method))
        }
        ImageOperation::ConvertColorspace(colorspace) => {
            builder.option("-colorspace", colorspace_name(&colorspace))
        }
        ImageOperation::ColorMatrix(values) => {
            builder.option("-color-matrix", number_list(&values))
        }
        ImageOperation::ColorThreshold(value) => builder.option("-color-threshold", value),
        ImageOperation::Alpha(mode) => builder.option("-alpha", alpha_name(&mode)),
        ImageOperation::SelectChannels(channels) => {
            builder.option("-channel", channel_list(&channels))
        }
        ImageOperation::SeparateChannels => builder.literal("-separate"),
        ImageOperation::CombineChannels(colorspace) => {
            builder.option("-colorspace", colorspace_name(&colorspace));
            builder.literal("-combine");
        }
        ImageOperation::Transparent(color, fuzz) => {
            builder.option("-fuzz", fuzz);
            builder.option("-transparent", color.value);
        }
        ImageOperation::Opaque(from, to) => {
            builder.option("-fill", to.value);
            builder.option("-opaque", from.value);
        }
        ImageOperation::FloodFill(point, color) => {
            builder.option("-fill", color.value);
            builder.option(
                "-floodfill",
                format!("{},{}", number(point.x), number(point.y)),
            );
        }
        ImageOperation::Clut(image) => {
            let input = builder.input(image.content, "bin");
            builder.literal("-clut");
            builder.input_path(input);
        }
        ImageOperation::HaldClut(image) => {
            let input = builder.input(image.content, "bin");
            builder.literal("-hald-clut");
            builder.input_path(input);
        }
        ImageOperation::ReadMask(image) => {
            let input = builder.input(image.content, "bin");
            builder.literal("-read-mask");
            builder.input_path(input);
        }
        ImageOperation::WriteMask(image) => {
            let input = builder.input(image.content, "bin");
            builder.literal("-write-mask");
            builder.input_path(input);
        }
        ImageOperation::Convolve(values) => builder.option("-convolve", number_list(&values)),
        ImageOperation::Morphology(method, kernel) => {
            builder.literal("-morphology");
            builder.literal(morphology_name(&method));
            builder.literal(kernel);
        }
        ImageOperation::ForwardFft => builder.literal("-fft"),
        ImageOperation::InverseFft => builder.literal("-ift"),
        ImageOperation::ConnectedComponents(connectivity) => {
            builder.option("-connected-components", connectivity.to_string())
        }
        ImageOperation::HoughLines(value) => builder.option("-hough-lines", value),
        ImageOperation::Integral => builder.literal("-integral"),
        ImageOperation::Kmeans(value) => builder.option("-kmeans", value),
        ImageOperation::Kuwahara(radius) => builder.option("-kuwahara", number(radius)),
        ImageOperation::LocalAdaptiveThreshold(value) => builder.option("-lat", value),
        ImageOperation::LocalContrast(value) => builder.option("-local-contrast", value),
        ImageOperation::MeanShift(value) => builder.option("-mean-shift", value),
        ImageOperation::Distort(method, arguments, best_fit) => {
            if best_fit {
                builder.option("-define", "distort:viewport=bestfit");
            }
            builder.literal("-distort");
            builder.literal(distort_name(&method));
            builder.literal(number_list(&arguments));
        }
        ImageOperation::Implode(amount) => builder.option("-implode", number(amount)),
        ImageOperation::Swirl(degrees) => builder.option("-swirl", number(degrees)),
        ImageOperation::Wave(amplitude, wavelength) => builder.option(
            "-wave",
            format!("{}x{}", number(amplitude), number(wavelength)),
        ),
        ImageOperation::Deskew(value) => builder.option("-deskew", value),
        ImageOperation::CycleColors(amount) => builder.option("-cycle", amount.to_string()),
        ImageOperation::Polaroid(angle) => builder.option("-polaroid", number(angle)),
        ImageOperation::Posterize(levels, dither) => {
            if !dither {
                builder.literal("+dither");
            }
            builder.option("-posterize", levels.to_string());
        }
        ImageOperation::Quantize(colors, colorspace) => {
            builder.option("-colorspace", colorspace_name(&colorspace));
            builder.option("-colors", colors.to_string());
        }
        ImageOperation::Monochrome => builder.literal("-monochrome"),
        ImageOperation::OrderedDither(value) => builder.option("-ordered-dither", value),
        ImageOperation::OilPaint(radius) => builder.option("-paint", number(radius)),
        ImageOperation::Perceptible(epsilon) => builder.option("-perceptible", number(epsilon)),
        ImageOperation::RandomThreshold(value) => builder.option("-random-threshold", value),
        ImageOperation::RangeThreshold(value) => builder.option("-range-threshold", value),
        ImageOperation::Raise(value, raised) => {
            builder.option(if raised { "-raise" } else { "+raise" }, value)
        }
        ImageOperation::Reshape(value) => builder.option("-reshape", value),
        ImageOperation::Segment(value) => builder.option("-segment", value),
        ImageOperation::Shade(azimuth, elevation) => builder.option(
            "-shade",
            format!("{}x{}", number(azimuth), number(elevation)),
        ),
        ImageOperation::Spread(radius) => builder.option("-spread", number(radius)),
        ImageOperation::Statistic(kind, geometry) => {
            builder.literal("-statistic");
            builder.literal(kind);
            builder.literal(geometry);
        }
        ImageOperation::Unsharp(value) => builder.option("-unsharp", value),
        ImageOperation::Vignette(value) => builder.option("-vignette", value),
        ImageOperation::WaveletDenoise(threshold, softness) => builder.option(
            "-wavelet-denoise",
            format!("{}x{}", number(threshold), number(softness)),
        ),
        ImageOperation::WhiteBalance => builder.literal("-white-balance"),
        ImageOperation::UniqueColors => builder.literal("-unique-colors"),
        ImageOperation::Draw(styles, primitives) => {
            for style in &styles {
                compile_draw_style(builder, style)?;
            }
            builder.option("-draw", draw_program(&primitives)?);
        }
        ImageOperation::Annotate(point, text) => {
            builder.option(
                "-annotate",
                format!("+{}+{}", number(point.x), number(point.y)),
            );
            builder.literal(text);
        }
        ImageOperation::StripMetadata => builder.literal("-strip"),
        ImageOperation::SetProperty(name, value) => {
            builder.literal("-set");
            builder.literal(name);
            builder.literal(value);
        }
        ImageOperation::DeleteProperty(name) => {
            builder.literal("+set");
            builder.literal(name);
        }
        ImageOperation::ApplyProfile(hash) => {
            let profile = builder.input(hash, "profile");
            builder.literal("-profile");
            builder.input_path(profile);
        }
        ImageOperation::ColorDecisionList(hash) => {
            let cdl = builder.input(hash, "cdl");
            builder.literal("-cdl");
            builder.input_path(cdl);
        }
        ImageOperation::RemoveProfile(name) => builder.option("+profile", name),
        ImageOperation::Evaluate(operator, value) => {
            builder.literal("-evaluate");
            builder.literal(evaluate_name(&operator));
            builder.literal(number(value));
        }
        ImageOperation::FxExpression(expression) => builder.option("-fx", expression),
        ImageOperation::Shadow(value) => builder.option("-shadow", value),
        ImageOperation::Border(size, color) => {
            builder.option("-bordercolor", color.value);
            builder.option("-border", size_geometry(size));
        }
        ImageOperation::Frame(value, color) => {
            builder.option("-mattecolor", color.value);
            builder.option("-frame", value);
        }
        ImageOperation::Define(define) => compile_define(builder, define),
    }
    Ok(())
}

fn compile_draw_style(
    builder: &mut PlanBuilder,
    style: &DrawStyle,
) -> Result<(), ImageMagickError> {
    match style {
        DrawStyle::Fill(color) => builder.option("-fill", color.value.clone()),
        DrawStyle::NoFill => builder.option("-fill", "none"),
        DrawStyle::Stroke(color) => builder.option("-stroke", color.value.clone()),
        DrawStyle::NoStroke => builder.option("-stroke", "none"),
        DrawStyle::StrokeWidth(width) => builder.option("-strokewidth", number(*width)),
        DrawStyle::Font(hash) => {
            let font = builder.input(*hash, "font");
            builder.literal("-font");
            builder.input_path(font);
        }
        DrawStyle::PointSize(size) => builder.option("-pointsize", number(*size)),
        DrawStyle::Gravity(gravity) => builder.option("-gravity", gravity_name(gravity)),
    }
    Ok(())
}

fn compile_sequence(builder: &mut PlanBuilder, operation: SequenceOperation) {
    match operation {
        SequenceOperation::AppendHorizontal => builder.literal("+append"),
        SequenceOperation::AppendVertical => builder.literal("-append"),
        SequenceOperation::CoalesceFrames => builder.literal("-coalesce"),
        SequenceOperation::DeconstructFrames => builder.literal("-deconstruct"),
        SequenceOperation::FlattenLayers => builder.literal("-flatten"),
        SequenceOperation::MergeLayers => builder.option("-layers", "merge"),
        SequenceOperation::MosaicLayers => builder.literal("-mosaic"),
        SequenceOperation::OptimizeFrames => builder.option("-layers", "optimize"),
        SequenceOperation::OptimizeTransparency => {
            builder.option("-layers", "optimize-transparency")
        }
        SequenceOperation::Reverse => builder.literal("-reverse"),
        SequenceOperation::DeleteFrames(selection) => {
            builder.option("-delete", frame_expression(&selection))
        }
        SequenceOperation::DuplicateFrames(selection, count) => builder.option(
            "-duplicate",
            format!("{},{}", count, frame_expression(&selection)),
        ),
        SequenceOperation::SwapFrames(first, second) => {
            builder.option("-swap", format!("{first},{second}"))
        }
    }
}

fn compile_encoding(builder: &mut PlanBuilder, encoding: Encoding) -> Result<(), ImageMagickError> {
    let format = format_name(&encoding.format)?;
    compile_write_options(builder, encoding.options)?;
    let kind = match encoding.mode {
        OutputMode::Adjoin => OutputKind::Single,
        OutputMode::Separate => OutputKind::Numbered,
    };
    let output = builder.output(kind, &format);
    builder.output_argument(output, &format);
    Ok(())
}

fn compile_write_options(
    builder: &mut PlanBuilder,
    options: Vec<WriteOption>,
) -> Result<(), ImageMagickError> {
    for option in options {
        match option {
            WriteOption::Quality(value) => builder.option("-quality", value.to_string()),
            WriteOption::Depth(value) => builder.option("-depth", value.to_string()),
            WriteOption::Compression(value) => {
                builder.option("-compress", compression_name(&value))
            }
            WriteOption::Colorspace(value) => {
                builder.option("-colorspace", colorspace_name(&value))
            }
            WriteOption::Interlace(value) => builder.option("-interlace", interlace_name(&value)),
            WriteOption::Density(value) => builder.option("-density", resolution_geometry(value)),
            WriteOption::SamplingFactor(value) => builder.option("-sampling-factor", value),
            WriteOption::StripMetadata => builder.literal("-strip"),
            WriteOption::Define(define) => compile_define(builder, define),
        }
    }
    Ok(())
}

fn compile_define(builder: &mut PlanBuilder, define: Define) {
    builder.option(
        "-define",
        format!("{}:{}={}", define.namespace, define.name, define.value),
    );
}

fn draw_program(primitives: &[DrawingPrimitive]) -> Result<String, ImageMagickError> {
    if primitives.is_empty() {
        return Err(invalid("Draw requires at least one primitive"));
    }
    primitives
        .iter()
        .map(|primitive| match primitive {
            DrawingPrimitive::Line(first, second) => Ok(format!(
                "line {},{} {},{}",
                number(first.x),
                number(first.y),
                number(second.x),
                number(second.y)
            )),
            DrawingPrimitive::Rectangle(rectangle) => Ok(format!(
                "rectangle {},{} {},{}",
                rectangle.x,
                rectangle.y,
                rectangle.x + rectangle.width as i64,
                rectangle.y + rectangle.height as i64
            )),
            DrawingPrimitive::RoundedRectangle(rectangle, rx, ry) => Ok(format!(
                "roundrectangle {},{} {},{} {},{}",
                rectangle.x,
                rectangle.y,
                rectangle.x + rectangle.width as i64,
                rectangle.y + rectangle.height as i64,
                number(*rx),
                number(*ry)
            )),
            DrawingPrimitive::Circle(center, edge) => Ok(format!(
                "circle {},{} {},{}",
                number(center.x),
                number(center.y),
                number(edge.x),
                number(edge.y)
            )),
            DrawingPrimitive::Ellipse(center, rx, ry, start, end) => Ok(format!(
                "ellipse {},{} {},{} {},{}",
                number(center.x),
                number(center.y),
                number(*rx),
                number(*ry),
                number(*start),
                number(*end)
            )),
            DrawingPrimitive::Polygon(points) => Ok(format!("polygon {}", points_string(points)?)),
            DrawingPrimitive::Polyline(points) => {
                Ok(format!("polyline {}", points_string(points)?))
            }
            DrawingPrimitive::Bezier(points) => Ok(format!("bezier {}", points_string(points)?)),
            DrawingPrimitive::Text(point, text) => Ok(format!(
                "text {},{} '{}'",
                number(point.x),
                number(point.y),
                text.replace('\\', "\\\\").replace('\'', "\\'")
            )),
            DrawingPrimitive::Path(path) => Ok(format!("path '{path}'")),
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|parts| parts.join(" "))
}

fn points_string(points: &[Point]) -> Result<String, ImageMagickError> {
    if points.is_empty() {
        return Err(invalid("drawing point list cannot be empty"));
    }
    Ok(points
        .iter()
        .map(|point| format!("{},{}", number(point.x), number(point.y)))
        .collect::<Vec<_>>()
        .join(" "))
}

fn format_name(format: &Format) -> Result<String, ImageMagickError> {
    let value = format.name.trim();
    if value.is_empty()
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '+'))
    {
        return Err(invalid(format!("invalid image format `{}`", format.name)));
    }
    let value = value.to_ascii_lowercase();
    if matches!(
        value.as_str(),
        "clipboard" | "print" | "scan" | "scanx" | "screenshot" | "show" | "win" | "x"
    ) {
        return Err(invalid(format!(
            "image format `{}` is not available in headless workflows",
            format.name
        )));
    }
    Ok(value)
}

fn frame_suffix(selection: &FrameSelection) -> String {
    match selection {
        FrameSelection::All => String::new(),
        FrameSelection::Frame(frame) => format!("[{frame}]"),
        FrameSelection::Range(first, last) => format!("[{first}-{last}]"),
        FrameSelection::Selected(frames) => format!(
            "[{}]",
            frames
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

fn frame_expression(selection: &FrameSelection) -> String {
    match selection {
        FrameSelection::All => "0--1".to_string(),
        FrameSelection::Frame(frame) => frame.to_string(),
        FrameSelection::Range(first, last) => format!("{first}-{last}"),
        FrameSelection::Selected(frames) => frames
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(","),
    }
}

fn size_geometry(size: Size) -> String {
    format!("{}x{}", size.width, size.height)
}

fn rectangle_geometry(rectangle: Rectangle) -> String {
    format!(
        "{}x{}{:+}{:+}",
        rectangle.width, rectangle.height, rectangle.x, rectangle.y
    )
}

fn resize_geometry(geometry: &ResizeGeometry) -> String {
    match geometry {
        ResizeGeometry::FitWithin(size) => size_geometry(*size),
        ResizeGeometry::FillArea(size) => format!("{}^", size_geometry(*size)),
        ResizeGeometry::ExactSize(size) => format!("{}!", size_geometry(*size)),
        ResizeGeometry::ShrinkWithin(size) => format!("{}>", size_geometry(*size)),
        ResizeGeometry::EnlargeWithin(size) => format!("{}<", size_geometry(*size)),
        ResizeGeometry::PixelArea(area) => format!("{area}@"),
        ResizeGeometry::ResizePercentage(x, y) => format!("{}x{}%", number(*x), number(*y)),
    }
}

fn blur_geometry(value: BlurGeometry) -> String {
    format!("{}x{}", number(value.radius), number(value.sigma))
}

fn resolution_geometry(value: Resolution) -> String {
    format!("{}x{}", number(value.x), number(value.y))
}

fn signed_offset(x: i64, y: i64) -> String {
    format!("{x:+}{y:+}")
}

fn number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.1}")
    } else {
        value.to_string()
    }
}

fn number_list(values: &[f64]) -> String {
    values
        .iter()
        .map(|value| number(*value))
        .collect::<Vec<_>>()
        .join(",")
}

fn gravity_name(value: &Gravity) -> String {
    match value {
        Gravity::NorthWest => "NorthWest",
        Gravity::North => "North",
        Gravity::NorthEast => "NorthEast",
        Gravity::West => "West",
        Gravity::Center => "Center",
        Gravity::East => "East",
        Gravity::SouthWest => "SouthWest",
        Gravity::South => "South",
        Gravity::SouthEast => "SouthEast",
    }
    .to_string()
}

fn filter_name(value: &Filter) -> String {
    match value {
        Filter::Point => "Point",
        Filter::Box => "Box",
        Filter::Triangle => "Triangle",
        Filter::Hermite => "Hermite",
        Filter::Hann => "Hann",
        Filter::Hamming => "Hamming",
        Filter::Blackman => "Blackman",
        Filter::Gaussian => "Gaussian",
        Filter::Quadratic => "Quadratic",
        Filter::Cubic => "Cubic",
        Filter::Catrom => "Catrom",
        Filter::Mitchell => "Mitchell",
        Filter::Lanczos => "Lanczos",
        Filter::Robidoux => "Robidoux",
        Filter::Other(value) => return value.clone(),
    }
    .to_string()
}

fn colorspace_name(value: &Colorspace) -> String {
    match value {
        Colorspace::Srgb => "sRGB",
        Colorspace::Rgb => "RGB",
        Colorspace::Gray => "Gray",
        Colorspace::Cmyk => "CMYK",
        Colorspace::Lab => "Lab",
        Colorspace::Lch => "LCH",
        Colorspace::Hsl => "HSL",
        Colorspace::Hsv => "HSV",
        Colorspace::Xyz => "XYZ",
        Colorspace::Yuv => "YUV",
        Colorspace::Other(value) => return value.clone(),
    }
    .to_string()
}

fn intensity_method_name(value: &IntensityMethod) -> String {
    match value {
        IntensityMethod::Average => "Average",
        IntensityMethod::Brightness => "Brightness",
        IntensityMethod::Lightness => "Lightness",
        IntensityMethod::Mean => "Mean",
        IntensityMethod::MeanSquare => "MS",
        IntensityMethod::Rec601Luma => "Rec601Luma",
        IntensityMethod::Rec601Luminance => "Rec601Luminance",
        IntensityMethod::Rec709Luma => "Rec709Luma",
        IntensityMethod::Rec709Luminance => "Rec709Luminance",
        IntensityMethod::RootMeanSquare => "RMS",
        IntensityMethod::Other(value) => return value.clone(),
    }
    .to_string()
}

fn channel_name(value: &Channel) -> String {
    match value {
        Channel::Red => "R",
        Channel::Green => "G",
        Channel::Blue => "B",
        Channel::Alpha => "A",
        Channel::Black => "K",
        Channel::Cyan => "C",
        Channel::Magenta => "M",
        Channel::Yellow => "Y",
        Channel::Gray => "Gray",
        Channel::Rgb => "RGB",
        Channel::Rgba => "RGBA",
        Channel::Cmyk => "CMYK",
        Channel::Cmyka => "CMYKA",
        Channel::All => "All",
        Channel::Other(value) => return value.clone(),
    }
    .to_string()
}

fn channel_list(values: &[Channel]) -> String {
    values
        .iter()
        .map(channel_name)
        .collect::<Vec<_>>()
        .join(",")
}

fn stream_channel_map(values: &[Channel]) -> String {
    values.iter().map(channel_name).collect::<Vec<_>>().join("")
}

fn alpha_name(value: &AlphaMode) -> String {
    match value {
        AlphaMode::Activate => "on",
        AlphaMode::Deactivate => "off",
        AlphaMode::Set => "set",
        AlphaMode::Opaque => "opaque",
        AlphaMode::Transparent => "transparent",
        AlphaMode::Extract => "extract",
        AlphaMode::Copy => "copy",
        AlphaMode::Shape => "shape",
        AlphaMode::Background => "background",
        AlphaMode::Other(value) => return value.clone(),
    }
    .to_string()
}

fn noise_name(value: &NoiseKind) -> String {
    match value {
        NoiseKind::Gaussian => "Gaussian",
        NoiseKind::Impulse => "Impulse",
        NoiseKind::Laplacian => "Laplacian",
        NoiseKind::MultiplicativeGaussian => "Multiplicative",
        NoiseKind::Poisson => "Poisson",
        NoiseKind::Random => "Random",
        NoiseKind::Uniform => "Uniform",
        NoiseKind::Other(value) => return value.clone(),
    }
    .to_string()
}

fn compose_name(value: &ComposeOperator) -> String {
    match value {
        ComposeOperator::Over => "Over",
        ComposeOperator::Atop => "Atop",
        ComposeOperator::In => "In",
        ComposeOperator::Out => "Out",
        ComposeOperator::Xor => "Xor",
        ComposeOperator::Multiply => "Multiply",
        ComposeOperator::Screen => "Screen",
        ComposeOperator::Overlay => "Overlay",
        ComposeOperator::Darken => "Darken",
        ComposeOperator::Lighten => "Lighten",
        ComposeOperator::Difference => "Difference",
        ComposeOperator::Exclusion => "Exclusion",
        ComposeOperator::Plus => "Plus",
        ComposeOperator::Minus => "Minus",
        ComposeOperator::Copy => "Copy",
        ComposeOperator::CopyAlpha => "CopyAlpha",
        ComposeOperator::DstOver => "DstOver",
        ComposeOperator::Src => "Src",
        ComposeOperator::Dst => "Dst",
        ComposeOperator::Other(value) => return value.clone(),
    }
    .to_string()
}

fn compression_name(value: &Compression) -> String {
    match value {
        Compression::None => "None",
        Compression::BZip => "BZip",
        Compression::Fax => "Fax",
        Compression::Group4 => "Group4",
        Compression::Jpeg => "JPEG",
        Compression::LosslessJpeg => "LosslessJPEG",
        Compression::Lzw => "LZW",
        Compression::Rle => "RLE",
        Compression::Zip => "Zip",
        Compression::Zstd => "Zstd",
        Compression::Other(value) => return value.clone(),
    }
    .to_string()
}

fn interlace_name(value: &Interlace) -> String {
    match value {
        Interlace::None => "None",
        Interlace::Line => "Line",
        Interlace::Plane => "Plane",
        Interlace::Partition => "Partition",
        Interlace::Gif => "GIF",
        Interlace::Jpeg => "JPEG",
        Interlace::Png => "PNG",
        Interlace::Other(value) => return value.clone(),
    }
    .to_string()
}

fn morphology_name(value: &MorphologyMethod) -> String {
    match value {
        MorphologyMethod::Convolve => "Convolve",
        MorphologyMethod::Correlate => "Correlate",
        MorphologyMethod::Erode => "Erode",
        MorphologyMethod::Dilate => "Dilate",
        MorphologyMethod::Open => "Open",
        MorphologyMethod::Close => "Close",
        MorphologyMethod::EdgeIn => "EdgeIn",
        MorphologyMethod::EdgeOut => "EdgeOut",
        MorphologyMethod::Edge => "Edge",
        MorphologyMethod::TopHat => "TopHat",
        MorphologyMethod::BottomHat => "BottomHat",
        MorphologyMethod::HitAndMiss => "HitAndMiss",
        MorphologyMethod::Thinning => "Thinning",
        MorphologyMethod::Thicken => "Thicken",
        MorphologyMethod::Other(value) => return value.clone(),
    }
    .to_string()
}

fn distort_name(value: &DistortMethod) -> String {
    match value {
        DistortMethod::Affine => "Affine",
        DistortMethod::AffineProjection => "AffineProjection",
        DistortMethod::ScaleRotateTranslate => "ScaleRotateTranslate",
        DistortMethod::Perspective => "Perspective",
        DistortMethod::PerspectiveProjection => "PerspectiveProjection",
        DistortMethod::BilinearForward => "BilinearForward",
        DistortMethod::BilinearReverse => "BilinearReverse",
        DistortMethod::Polynomial => "Polynomial",
        DistortMethod::Arc => "Arc",
        DistortMethod::Polar => "Polar",
        DistortMethod::DePolar => "DePolar",
        DistortMethod::Barrel => "Barrel",
        DistortMethod::BarrelInverse => "BarrelInverse",
        DistortMethod::Other(value) => return value.clone(),
    }
    .to_string()
}

fn evaluate_name(value: &EvaluateOperator) -> String {
    match value {
        EvaluateOperator::Add => "Add",
        EvaluateOperator::Subtract => "Subtract",
        EvaluateOperator::Multiply => "Multiply",
        EvaluateOperator::Divide => "Divide",
        EvaluateOperator::Pow => "Pow",
        EvaluateOperator::Log => "Log",
        EvaluateOperator::Min => "Min",
        EvaluateOperator::Max => "Max",
        EvaluateOperator::Set => "Set",
        EvaluateOperator::Threshold => "Threshold",
        EvaluateOperator::And => "And",
        EvaluateOperator::Or => "Or",
        EvaluateOperator::Xor => "Xor",
        EvaluateOperator::Other(value) => return value.clone(),
    }
    .to_string()
}

fn metric_name(value: &ComparisonMetric) -> String {
    match value {
        ComparisonMetric::AbsoluteError => "AE",
        ComparisonMetric::Fuzz => "FUZZ",
        ComparisonMetric::MeanAbsoluteError => "MAE",
        ComparisonMetric::MeanErrorPerPixel => "MEPP",
        ComparisonMetric::MeanSquaredError => "MSE",
        ComparisonMetric::NormalizedCrossCorrelation => "NCC",
        ComparisonMetric::PeakAbsoluteError => "PAE",
        ComparisonMetric::PeakSignalToNoiseRatio => "PSNR",
        ComparisonMetric::RootMeanSquaredError => "RMSE",
        ComparisonMetric::StructuralSimilarity => "SSIM",
        ComparisonMetric::StructuralDissimilarity => "DSSIM",
        ComparisonMetric::Other(value) => return value.clone(),
    }
    .to_string()
}

fn pixel_storage_name(value: &PixelStorageType) -> String {
    match value {
        PixelStorageType::Char => "char",
        PixelStorageType::Short => "short",
        PixelStorageType::Integer => "integer",
        PixelStorageType::Long => "long",
        PixelStorageType::Float => "float",
        PixelStorageType::Double => "double",
        PixelStorageType::Quantum => "quantum",
    }
    .to_string()
}

fn builtin_name(value: &BuiltinImageKind) -> String {
    match value {
        BuiltinImageKind::Logo => "logo:",
        BuiltinImageKind::Rose => "rose:",
        BuiltinImageKind::Wizard => "wizard:",
        BuiltinImageKind::Granite => "granite:",
        BuiltinImageKind::Netscape => "netscape:",
    }
    .to_string()
}

fn capability_name(value: &CapabilityDomain) -> String {
    match value {
        CapabilityDomain::Align => "Align",
        CapabilityDomain::Alpha => "Alpha",
        CapabilityDomain::Channel => "Channel",
        CapabilityDomain::Colorspace => "Colorspace",
        CapabilityDomain::Command => "Command",
        CapabilityDomain::Compose => "Compose",
        CapabilityDomain::Compress => "Compress",
        CapabilityDomain::Distort => "Distort",
        CapabilityDomain::Dither => "Dither",
        CapabilityDomain::Evaluate => "Evaluate",
        CapabilityDomain::Filter => "Filter",
        CapabilityDomain::Font => "Font",
        CapabilityDomain::Format => "Format",
        CapabilityDomain::Gravity => "Gravity",
        CapabilityDomain::Interlace => "Interlace",
        CapabilityDomain::Interpolate => "Interpolate",
        CapabilityDomain::Kernel => "Kernel",
        CapabilityDomain::Metric => "Metric",
        CapabilityDomain::Morphology => "Morphology",
        CapabilityDomain::Noise => "Noise",
        CapabilityDomain::Orientation => "Orientation",
        CapabilityDomain::Policy => "Policy",
        CapabilityDomain::Storage => "Storage",
        CapabilityDomain::Tool => "Tool",
        CapabilityDomain::Type => "Type",
        CapabilityDomain::Units => "Units",
        CapabilityDomain::Other(value) => return value.clone(),
    }
    .to_string()
}

fn invalid(message: impl Into<String>) -> ImageMagickError {
    ImageMagickError {
        kind: ImageMagickErrorKind::InvalidRequest,
        exit_code: None,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png() -> Encoding {
        Encoding {
            format: Format {
                name: "png".to_string(),
            },
            mode: OutputMode::Adjoin,
            options: vec![],
        }
    }

    #[test]
    fn transform_compiles_semantics_without_real_paths() {
        let image = Image {
            content: blake3::hash(b"input"),
        };
        let plan = transform_plan(
            ImageSource::Stored(image, FrameSelection::Frame(2), vec![]),
            vec![
                ImageOperation::AutoOrient,
                ImageOperation::Resize(ResizeGeometry::FitWithin(Size {
                    width: 640,
                    height: 480,
                })),
            ],
            png(),
        )
        .unwrap();
        assert_eq!(plan.inputs.len(), 1);
        assert_eq!(plan.outputs.len(), 1);
        assert_eq!(
            plan.arguments,
            vec![
                ToolArgument::input_decorated(0, "", "[2]"),
                ToolArgument::literal("-auto-orient"),
                ToolArgument::literal("-resize"),
                ToolArgument::literal("640x480"),
                ToolArgument::output_decorated(0, "png:"),
            ]
        );
    }

    #[test]
    fn resize_modes_compile_modifiers() {
        let size = Size {
            width: 100,
            height: 80,
        };
        assert_eq!(resize_geometry(&ResizeGeometry::FitWithin(size)), "100x80");
        assert_eq!(resize_geometry(&ResizeGeometry::FillArea(size)), "100x80^");
        assert_eq!(resize_geometry(&ResizeGeometry::ExactSize(size)), "100x80!");
        assert_eq!(
            resize_geometry(&ResizeGeometry::ShrinkWithin(size)),
            "100x80>"
        );
        assert_eq!(
            resize_geometry(&ResizeGeometry::EnlargeWithin(size)),
            "100x80<"
        );
    }

    #[test]
    fn grayscale_compiles_an_intensity_method() {
        let plan = transform_plan(
            ImageSource::Stored(
                Image {
                    content: blake3::hash(b"input"),
                },
                FrameSelection::All,
                vec![],
            ),
            vec![ImageOperation::Grayscale(IntensityMethod::Rec709Luminance)],
            png(),
        )
        .unwrap();
        assert_eq!(
            plan.arguments,
            vec![
                ToolArgument::input(0),
                ToolArgument::literal("-grayscale"),
                ToolArgument::literal("Rec709Luminance"),
                ToolArgument::output_decorated(0, "png:"),
            ]
        );
    }

    #[test]
    fn montage_materializes_an_explicit_font() {
        let plan = montage_plan(
            vec![Image {
                content: blake3::hash(b"image"),
            }],
            MontageLayout::Columns(1),
            vec![MontageOption::Font(blake3::hash(b"font"))],
            png(),
        )
        .unwrap();
        assert_eq!(plan.inputs.len(), 2);
        assert_eq!(plan.inputs[0].hash, blake3::hash(b"font"));
        assert!(plan.arguments.windows(2).any(|arguments| {
            arguments == [ToolArgument::literal("-font"), ToolArgument::input(0)]
        }));
    }

    #[test]
    fn interactive_formats_are_rejected() {
        for name in [
            "CLIPBOARD",
            "PRINT",
            "SCAN",
            "SCANX",
            "SCREENSHOT",
            "SHOW",
            "WIN",
            "X",
        ] {
            let error = format_name(&Format {
                name: name.to_string(),
            })
            .unwrap_err();
            assert_eq!(error.kind, ImageMagickErrorKind::InvalidRequest);
            assert!(error.message.contains("headless workflows"));
        }
    }
}

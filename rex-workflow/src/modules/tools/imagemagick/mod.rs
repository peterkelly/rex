mod compile;
pub mod types;

use crate::{modules::tools::executor::ToolExecution, state::State};
use compile::*;
use rex::engine::{EngineError, Module};
use std::collections::BTreeMap;
use types::*;

type ImResult<T> = Result<T, ImageMagickError>;

pub fn module() -> Result<Module<State>, EngineError> {
    api::rex_module()
}

/// Semantic ImageMagick tools for content-addressed image workflows.
///
/// Stored images and generated results use the shared `std.artifacts.Image` type, which carries a
/// content hash rather than a host path. Prefer `transform`, `generate`, `compare`, `composite`, or
/// `montage` for their focused tasks;
/// use `render` when image reads, persistent settings, immediate image operators, and sequence
/// operators must be interleaved in exact ImageMagick command-line order. Expected invalid requests
/// and tool-process failures are returned as `Err ImageMagickError`, while storage and executor
/// failures remain Rex evaluation errors. Supported formats and operations depend on the installed
/// ImageMagick build and its delegates.
#[rex::module(name = "tools.imagemagick")]
mod api {
    use super::*;

    /// Execute an ordered ImageMagick program and encode its final image sequence.
    ///
    /// `instructions` may interleave image reads, persistent settings, immediate image operations, and
    /// sequence operations. Their order is significant. Use this general API when multiple sources or
    /// sequence-wide operations are needed; use `transform` for one source and a simple operation list.
    #[rex::export]
    pub(super) async fn render(
        state: State,
        instructions: Vec<ImageInstruction>,
        encoding: Encoding,
    ) -> Result<ImResult<ImageOutput>, EngineError> {
        let plan = match render_plan(instructions, encoding) {
            Ok(plan) => plan,
            Err(error) => return Ok(Err(error)),
        };
        image_output(&state, plan).await
    }

    /// Read one image source, apply image operations in order, and encode the result.
    ///
    /// `source` may reference stored content or generate an image. `encoding.mode` determines whether
    /// a multi-frame result becomes one adjoined file or a list of separately encoded images.
    #[rex::export]
    pub(super) async fn transform(
        state: State,
        source: ImageSource,
        operations: Vec<ImageOperation>,
        encoding: Encoding,
    ) -> Result<ImResult<ImageOutput>, EngineError> {
        let plan = match transform_plan(source, operations, encoding) {
            Ok(plan) => plan,
            Err(error) => return Ok(Err(error)),
        };
        image_output(&state, plan).await
    }

    /// Generate an image from a canvas, gradient, pattern, noise source, or built-in image.
    ///
    /// `source` should be a synthetic `ImageSource`; for stored content prefer `transform`. Operations
    /// are applied in order before the result is encoded.
    #[rex::export]
    pub(super) async fn generate(
        state: State,
        source: ImageSource,
        operations: Vec<ImageOperation>,
        encoding: Encoding,
    ) -> Result<ImResult<ImageOutput>, EngineError> {
        transform(state, source, operations, encoding).await
    }

    /// Apply the same operations independently to several stored images with ImageMagick `mogrify`.
    ///
    /// `images` must be non-empty. `encoding.mode` must be `AdjoinFrames` because each input already
    /// produces a separate output file; the returned list follows ImageMagick's output ordering.
    #[rex::export]
    pub(super) async fn transform_many(
        state: State,
        images: Vec<Image>,
        operations: Vec<ImageOperation>,
        encoding: Encoding,
    ) -> Result<ImResult<Vec<Image>>, EngineError> {
        let plan = match transform_many_plan(images, operations, encoding) {
            Ok(plan) => plan,
            Err(error) => return Ok(Err(error)),
        };
        let execution = execute(&state, plan).await?;
        if !is_success(&execution) {
            return Ok(Err(process_error(&execution)));
        }
        let images = execution
            .outputs
            .get(&0)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|content| Image { content })
            .collect::<Vec<_>>();
        if images.is_empty() {
            return Ok(Err(unexpected("mogrify produced no output images")));
        }
        Ok(Ok(images))
    }

    /// Return typed metadata for every frame in a stored image.
    ///
    /// `options` can request fast header-only inspection, verbose properties, image features, or
    /// moments. Expensive options may decode pixels. The result contains one `ImageInfo` per frame.
    #[rex::export]
    pub(super) async fn identify(
        state: State,
        image: Image,
        options: Vec<IdentifyOption>,
    ) -> Result<ImResult<Vec<ImageInfo>>, EngineError> {
        let plan = match identify_plan(image, options) {
            Ok(plan) => plan,
            Err(error) => return Ok(Err(error)),
        };
        let execution = execute(&state, plan).await?;
        if !is_success(&execution) {
            return Ok(Err(process_error(&execution)));
        }
        match parse_identify(&execution.stdout) {
            Ok(infos) => Ok(Ok(infos)),
            Err(error) => Ok(Err(error)),
        }
    }

    /// Measure the difference between two stored images and optionally return a difference image.
    ///
    /// `metric` selects the reported distortion and `options` configure fuzz, channels, highlighting,
    /// and composition. ImageMagick exit status 1 means the images differ and is returned as a
    /// successful `Comparison` with `equal = false`, not as `ImageMagickError`.
    #[rex::export]
    pub(super) async fn compare(
        state: State,
        first: Image,
        second: Image,
        metric: ComparisonMetric,
        options: Vec<CompareOption>,
    ) -> Result<ImResult<Comparison>, EngineError> {
        let plan = match compare_plan(first, second, &metric, options) {
            Ok(plan) => plan,
            Err(error) => return Ok(Err(error)),
        };
        let execution = execute(&state, plan).await?;
        if !matches!(execution.exit_code, Some(0 | 1)) {
            return Ok(Err(process_error(&execution)));
        }
        let diagnostic = String::from_utf8_lossy(&execution.stderr);
        let distortion = diagnostic
            .split_whitespace()
            .next()
            .and_then(|value| value.parse::<f64>().ok())
            .ok_or_else(|| {
                EngineError::Custom(format!(
                    "ImageMagick compare returned an unrecognized metric: {diagnostic}"
                ))
            })?;
        let difference = execution
            .outputs
            .get(&0)
            .and_then(|outputs| outputs.first())
            .copied()
            .map(|content| Image { content });
        Ok(Ok(Comparison {
            equal: execution.exit_code == Some(0),
            metric,
            distortion,
            difference,
        }))
    }

    /// Composite `overlay` onto `background`, optionally using `mask`, and encode the result.
    ///
    /// `operator` selects ImageMagick's alpha composition rule. `options` control geometry, gravity,
    /// blending, dissolve, tiling, clamping, and compose-specific defines; their order is preserved.
    #[rex::export]
    pub(super) async fn composite(
        state: State,
        background: Image,
        overlay: Image,
        mask: Option<Image>,
        operator: ComposeOperator,
        options: Vec<CompositeOption>,
        encoding: Encoding,
    ) -> Result<ImResult<ImageOutput>, EngineError> {
        let plan = match composite_plan(background, overlay, mask, operator, options, encoding) {
            Ok(plan) => plan,
            Err(error) => return Ok(Err(error)),
        };
        image_output(&state, plan).await
    }

    /// Build a contact sheet from stored images.
    ///
    /// `layout` constrains columns and/or rows. `options` control tile geometry, gravity, background,
    /// borders, frames, shadows, labels, fonts, point size, and spacing. Use a CAS-backed font for
    /// portable labeled montages because hosts need not provide a default font.
    #[rex::export]
    pub(super) async fn montage(
        state: State,
        images: Vec<Image>,
        layout: MontageLayout,
        options: Vec<MontageOption>,
        encoding: Encoding,
    ) -> Result<ImResult<ImageOutput>, EngineError> {
        let plan = match montage_plan(images, layout, options, encoding) {
            Ok(plan) => plan,
            Err(error) => return Ok(Err(error)),
        };
        image_output(&state, plan).await
    }

    /// Export raw pixels from a stored image into a content-addressed byte buffer.
    ///
    /// `spec.region` chooses the whole image or a rectangle, `channels` controls channel order, and
    /// `storage_type` selects ImageMagick's sample representation. Interpret the returned bytes using
    /// those same channels and storage type; no image header is included.
    #[rex::export]
    pub(super) async fn extract_pixels(
        state: State,
        image: Image,
        spec: PixelSpec,
    ) -> Result<ImResult<PixelBuffer>, EngineError> {
        let plan = match stream_plan(image, &spec) {
            Ok(plan) => plan,
            Err(error) => return Ok(Err(error)),
        };
        let execution = execute(&state, plan).await?;
        if !is_success(&execution) {
            return Ok(Err(process_error(&execution)));
        }
        let Some(content) = execution
            .outputs
            .get(&0)
            .and_then(|outputs| outputs.first())
            .copied()
        else {
            return Ok(Err(unexpected("stream produced no pixel buffer")));
        };
        Ok(Ok(PixelBuffer {
            content,
            channels: spec.channels,
            storage_type: spec.storage_type,
        }))
    }

    /// Return the installed ImageMagick version, enabled features, and built-in delegates.
    #[rex::export]
    pub(super) async fn version(state: State) -> Result<ImResult<VersionInfo>, EngineError> {
        let execution = execute(&state, version_plan()).await?;
        if !is_success(&execution) {
            return Ok(Err(process_error(&execution)));
        }
        let output = String::from_utf8_lossy(&execution.stdout);
        let mut version = String::new();
        let mut features = String::new();
        let mut delegates = String::new();
        for line in output.lines() {
            if let Some(value) = line.strip_prefix("Version: ") {
                version = value.to_string();
            } else if let Some(value) = line.strip_prefix("Features: ") {
                features = value.to_string();
            } else if let Some(value) = line.strip_prefix("Delegates (built-in): ") {
                delegates = value.to_string();
            }
        }
        if version.is_empty() {
            return Ok(Err(unexpected(
                "could not parse ImageMagick version output",
            )));
        }
        Ok(Ok(VersionInfo {
            version,
            features,
            delegates,
        }))
    }

    /// List image format names supported by the installed ImageMagick build.
    ///
    /// The returned names come from `magick -list format`; read/write support still depends on each
    /// format's mode and delegates, so use `capabilities CapabilityFormat` when those details matter.
    #[rex::export]
    pub(super) async fn formats(state: State) -> Result<ImResult<Vec<String>>, EngineError> {
        let execution = execute(&state, formats_plan()).await?;
        if !is_success(&execution) {
            return Ok(Err(process_error(&execution)));
        }
        let mut formats = Vec::new();
        for line in String::from_utf8_lossy(&execution.stdout).lines() {
            let mut columns = line.split_whitespace();
            let Some(name) = columns.next() else {
                continue;
            };
            let Some(mode) = columns.next() else {
                continue;
            };
            if mode.len() == 3 && mode.chars().all(|c| matches!(c, 'r' | 'w' | '+' | '-')) {
                formats.push(name.trim_end_matches('*').to_string());
            }
        }
        formats.sort();
        formats.dedup();
        Ok(Ok(formats))
    }

    /// List values in one capability domain supported by the installed ImageMagick build.
    ///
    /// `domain` maps to ImageMagick's `-list` categories such as formats, filters, compose operators,
    /// fonts, policies, storage types, and tools. Results are host-specific strings.
    #[rex::export]
    pub(super) async fn capabilities(
        state: State,
        domain: CapabilityDomain,
    ) -> Result<ImResult<Vec<String>>, EngineError> {
        let execution = execute(&state, capabilities_plan(domain)).await?;
        if !is_success(&execution) {
            return Ok(Err(process_error(&execution)));
        }
        let values = String::from_utf8_lossy(&execution.stdout)
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(ToOwned::to_owned)
            .collect();
        Ok(Ok(values))
    }
}

async fn image_output(
    state: &State,
    plan: crate::modules::tools::executor::ToolExecutionPlan,
) -> Result<ImResult<ImageOutput>, EngineError> {
    let execution = execute(state, plan).await?;
    if !is_success(&execution) {
        return Ok(Err(process_error(&execution)));
    }
    let outputs = execution.outputs.get(&0).cloned().unwrap_or_default();
    match outputs.as_slice() {
        [] => Ok(Err(unexpected("ImageMagick produced no output image"))),
        [content] => Ok(Ok(ImageOutput::SingleImage(Image { content: *content }))),
        _ => Ok(Ok(ImageOutput::MultipleImages(
            outputs
                .into_iter()
                .map(|content| Image { content })
                .collect(),
        ))),
    }
}

async fn execute(
    state: &State,
    plan: crate::modules::tools::executor::ToolExecutionPlan,
) -> Result<ToolExecution, EngineError> {
    state
        .tools
        .execute(&state.store, plan)
        .await
        .map_err(|error| EngineError::Custom(error.to_string()))
}

fn parse_identify(bytes: &[u8]) -> Result<Vec<ImageInfo>, ImageMagickError> {
    let text = String::from_utf8(bytes.to_vec())
        .map_err(|_| unexpected("identify returned non-UTF-8 output"))?;
    let mut infos = Vec::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let fields = line.split('\t').collect::<Vec<_>>();
        if fields.len() != 9 {
            return Err(unexpected(format!(
                "identify returned {} fields instead of 9: `{line}`",
                fields.len()
            )));
        }
        let parse_u64 = |index: usize, name: &str| {
            fields[index]
                .parse::<u64>()
                .map_err(|_| unexpected(format!("invalid identify {name}: `{}`", fields[index])))
        };
        let channels = fields[7].to_ascii_lowercase();
        infos.push(ImageInfo {
            format: fields[0].to_string(),
            mime_type: fields[1].to_string(),
            width: parse_u64(2, "width")?,
            height: parse_u64(3, "height")?,
            frame_index: parse_u64(4, "frame index")?,
            depth: parse_u64(5, "depth")?,
            colorspace: fields[6].to_string(),
            has_alpha: channels.contains('a') || channels.contains("alpha"),
            orientation: fields[8].to_string(),
            properties: BTreeMap::new(),
        });
    }
    if infos.is_empty() {
        return Err(unexpected("identify returned no image records"));
    }
    Ok(infos)
}

fn is_success(execution: &ToolExecution) -> bool {
    execution.exit_code == Some(0)
}

fn process_error(execution: &ToolExecution) -> ImageMagickError {
    let stderr = String::from_utf8_lossy(&execution.stderr)
        .trim()
        .to_string();
    let stdout = String::from_utf8_lossy(&execution.stdout)
        .trim()
        .to_string();
    ImageMagickError {
        kind: ImageMagickErrorKind::ProcessFailed,
        exit_code: execution.exit_code,
        message: if stderr.is_empty() { stdout } else { stderr },
    }
}

fn unexpected(message: impl Into<String>) -> ImageMagickError {
    ImageMagickError {
        kind: ImageMagickErrorKind::UnexpectedOutput,
        exit_code: None,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::api::*;
    use super::*;
    use rex::storage::Store;

    #[test]
    fn identify_protocol_is_parsed_semantically() {
        let records =
            parse_identify(b"PNG\timage/png\t320\t200\t0\t8\tsRGB\tsrgba\tTopLeft\n").unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].width, 320);
        assert_eq!(records[0].height, 200);
        assert!(records[0].has_alpha);
    }

    #[tokio::test]
    async fn real_magick_generates_and_identifies_an_image_when_available() {
        if std::process::Command::new("magick")
            .arg("-version")
            .output()
            .is_err()
        {
            return;
        }

        let state = State::local(Store::new_in_memory());
        let generated = transform(
            state.clone(),
            ImageSource::Canvas(
                Size {
                    width: 32,
                    height: 24,
                },
                Color {
                    value: "#336699".to_string(),
                },
            ),
            vec![],
            Encoding {
                format: Format {
                    name: "png".to_string(),
                },
                mode: OutputMode::AdjoinFrames,
                options: vec![],
            },
        )
        .await
        .unwrap()
        .unwrap();
        let ImageOutput::SingleImage(image) = generated else {
            panic!("expected one generated image")
        };
        let info = identify(state, image, vec![IdentifyOption::IdentifyPing])
            .await
            .unwrap()
            .unwrap();
        assert_eq!(info[0].width, 32);
        assert_eq!(info[0].height, 24);
        assert_eq!(info[0].format, "PNG");
    }

    #[tokio::test]
    async fn real_magick_grayscale_accepts_the_compiled_intensity_method_when_available() {
        if std::process::Command::new("magick")
            .arg("-version")
            .output()
            .is_err()
        {
            return;
        }

        let state = State::local(Store::new_in_memory());
        let source = generated_image(&state, 32, 24, "#336699").await;
        let output = transform(
            state.clone(),
            ImageSource::StoredImage(source, FrameSelection::AllFrames, vec![]),
            vec![ImageOperation::Grayscale(
                IntensityMethod::IntensityRec709Luminance,
            )],
            png_encoding(),
        )
        .await
        .unwrap()
        .unwrap();
        let ImageOutput::SingleImage(image) = output else {
            panic!("expected one grayscale image")
        };
        let info = identify(state, image, vec![IdentifyOption::IdentifyPing])
            .await
            .unwrap()
            .unwrap();
        assert_eq!(info[0].colorspace, "Gray");
    }

    #[tokio::test]
    async fn real_magick_specialized_tools_exchange_cas_images_when_available() {
        if std::process::Command::new("magick")
            .arg("-version")
            .output()
            .is_err()
        {
            return;
        }

        let state = State::local(Store::new_in_memory());
        let first = generated_image(&state, 40, 30, "red").await;
        let second = generated_image(&state, 20, 10, "blue").await;

        let comparison = compare(
            state.clone(),
            first.clone(),
            first.clone(),
            ComparisonMetric::MetricRootMeanSquaredError,
            vec![],
        )
        .await
        .unwrap()
        .unwrap();
        assert!(comparison.equal);
        assert_eq!(comparison.distortion, 0.0);

        let composited = composite(
            state.clone(),
            first.clone(),
            second.clone(),
            None,
            ComposeOperator::ComposeOver,
            vec![CompositeOption::CompositeGravity(Gravity::GravityCenter)],
            png_encoding(),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(matches!(composited, ImageOutput::SingleImage(_)));

        let sheet = montage(
            state.clone(),
            vec![first.clone(), second.clone()],
            MontageLayout::MontageColumns(2),
            vec![],
            png_encoding(),
        )
        .await
        .unwrap();
        match sheet {
            Ok(output) => assert!(matches!(output, ImageOutput::SingleImage(_))),
            Err(error) => assert!(
                error.message.contains("unable to read font"),
                "unexpected montage error: {error:?}"
            ),
        }

        let pixels = extract_pixels(
            state.clone(),
            first.clone(),
            PixelSpec {
                region: PixelRegion::PixelRectangle(Rectangle {
                    width: 2,
                    height: 2,
                    x: 0,
                    y: 0,
                }),
                channels: vec![
                    Channel::ChannelRed,
                    Channel::ChannelGreen,
                    Channel::ChannelBlue,
                ],
                storage_type: PixelStorageType::PixelsChar,
            },
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(state.store.size(pixels.content).await.unwrap(), 12);

        let batch = transform_many(
            state,
            vec![first, second],
            vec![ImageOperation::Resize(ResizeGeometry::FitWithin(Size {
                width: 10,
                height: 10,
            }))],
            png_encoding(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(batch.len(), 2);
    }

    fn png_encoding() -> Encoding {
        Encoding {
            format: Format {
                name: "png".to_string(),
            },
            mode: OutputMode::AdjoinFrames,
            options: vec![],
        }
    }

    async fn generated_image(state: &State, width: u64, height: u64, color: &str) -> Image {
        let output = transform(
            state.clone(),
            ImageSource::Canvas(
                Size { width, height },
                Color {
                    value: color.to_string(),
                },
            ),
            vec![],
            png_encoding(),
        )
        .await
        .unwrap()
        .unwrap();
        let ImageOutput::SingleImage(image) = output else {
            panic!("expected one generated image")
        };
        image
    }
}

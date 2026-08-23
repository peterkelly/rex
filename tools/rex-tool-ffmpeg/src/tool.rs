#[path = "compile.rs"]
mod compile;
#[path = "types.rs"]
pub mod types;

use crate::{modules::tools::executor::ToolExecution, state::State};
use compile::*;
use rex::engine::{EngineError, Module};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use types::*;

type FfResult<T> = Result<T, FfmpegError>;

pub fn module() -> Result<Module<State>, EngineError> {
    api::rex_module()
}

/// Headless FFmpeg and FFprobe tools for content-addressed media workflows.
///
/// Media inputs and outputs use the shared `std.artifacts.Media` type, which carries a content hash
/// rather than a host path. Prefer the narrow functions such as `transcode`, `thumbnail`, `mux`, or
/// `probe` when they express the task;
/// use `render` for typed multi-input filter graphs or multiple outputs. Expected invalid requests
/// and tool-process failures are returned as `Err FfmpegError`, while storage and executor failures
/// remain Rex evaluation errors. Available codecs, formats, devices, filters, and hardware features
/// depend on the FFmpeg installation used by the workflow host.
#[rex::module(
    name = "tools.ffmpeg",
    defaults(VideoEqualizer, FilterGraph, ProbeOptions)
)]
mod api {
    use super::*;

    /// Execute a typed FFmpeg program with one or more inputs and outputs.
    ///
    /// `program` describes input options, a complex filter graph, stream mappings, encoders, and
    /// output modes. Use this general API for named filter links, multiple inputs, or multiple outputs;
    /// prefer a narrower function for ordinary single-output tasks. Artifacts are returned in output
    /// order as single media files, numbered sequences, or tree-backed HLS/DASH packages.
    #[rex::export]
    pub(super) async fn render(
        state: State,
        program: MediaProgram,
    ) -> Result<FfResult<Vec<MediaArtifact>>, EngineError> {
        let (plan, artifacts) = match render_plan(program) {
            Ok(value) => value,
            Err(error) => return Ok(Err(error)),
        };
        let execution = execute(&state, plan).await?;
        if !is_success(&execution) {
            return Ok(Err(process_error(&execution)));
        }
        artifacts_from_execution(&execution, &artifacts)
            .map_or_else(|error| Ok(Err(error)), |artifacts| Ok(Ok(artifacts)))
    }

    /// Transform and encode one media source into a single content-addressed media file.
    ///
    /// `operations` are applied in order. `encoding` selects the container and independently includes
    /// or omits video, audio, and subtitle streams. Probe unfamiliar input before selecting a stream
    /// index or choosing parameters that depend on its dimensions, rates, or channel layout.
    #[rex::export]
    pub(super) async fn transcode(
        state: State,
        source: MediaSource,
        operations: Vec<MediaOperation>,
        encoding: Encoding,
    ) -> Result<FfResult<Media>, EngineError> {
        let plan = match transcode_plan(source, operations, encoding) {
            Ok(plan) => plan,
            Err(error) => return Ok(Err(error)),
        };
        single_media(&state, plan).await
    }

    /// Copy every stream from `media` into a different container without re-encoding.
    ///
    /// `format` names the output muxer/container and `options` configure that muxer. Remuxing is fast
    /// and lossless, but fails when the chosen container cannot carry an input codec.
    #[rex::export]
    pub(super) async fn remux(
        state: State,
        media: Media,
        format: ContainerFormat,
        options: Vec<MuxOption>,
    ) -> Result<FfResult<Media>, EngineError> {
        let plan = match remux_plan(media, format, options) {
            Ok(plan) => plan,
            Err(error) => return Ok(Err(error)),
        };
        single_media(&state, plan).await
    }

    /// Encode the first audio stream from `media` and omit all other streams.
    ///
    /// `encoding` selects the audio codec and its options; `format` selects the output container. Use
    /// the general `render` or `mux` API when a different audio stream must be selected explicitly.
    #[rex::export]
    pub(super) async fn extract_audio(
        state: State,
        media: Media,
        encoding: AudioEncoding,
        format: ContainerFormat,
    ) -> Result<FfResult<Media>, EngineError> {
        let plan = match extract_audio_plan(media, encoding, format) {
            Ok(plan) => plan,
            Err(error) => return Ok(Err(error)),
        };
        single_media(&state, plan).await
    }

    /// Decode selected video frames as an ordered list of independently stored image files.
    ///
    /// `selection` controls the sampling strategy and `encoding` selects the image codec. `AtTimes`
    /// performs one seek per timestamp and preserves the requested order; its list must not be empty.
    #[rex::export]
    pub(super) async fn extract_frames(
        state: State,
        media: Media,
        selection: FrameSelection,
        encoding: ImageEncoding,
    ) -> Result<FfResult<Vec<Media>>, EngineError> {
        let selection = match selection {
            FrameSelection::AtTimes(times) => {
                if times.is_empty() {
                    return Ok(Err(compile::invalid(
                        "AtTimes requires at least one timestamp",
                    )));
                }
                let mut frames = Vec::with_capacity(times.len());
                for time in times {
                    let plan = match thumbnail_plan(
                        media.clone(),
                        ThumbnailSpec {
                            at: Some(time),
                            size: None,
                            preserve_aspect_ratio: true,
                        },
                        encoding.clone(),
                    ) {
                        Ok(plan) => plan,
                        Err(error) => return Ok(Err(error)),
                    };
                    match single_media(&state, plan).await? {
                        Ok(frame) => frames.push(frame),
                        Err(error) => return Ok(Err(error)),
                    }
                }
                return Ok(Ok(frames));
            }
            selection => selection,
        };
        let plan = match extract_frames_plan(media, selection, encoding) {
            Ok(plan) => plan,
            Err(error) => return Ok(Err(error)),
        };
        media_sequence(&state, plan).await
    }

    /// Extract one representative frame, optionally at a requested time and resized to a bounding box.
    ///
    /// When `spec.at` is `None`, FFmpeg's thumbnail filter chooses a representative frame. When
    /// `preserve_aspect_ratio` is true, `spec.size` is treated as a maximum box rather than an exact
    /// output size.
    #[rex::export]
    pub(super) async fn thumbnail(
        state: State,
        media: Media,
        spec: ThumbnailSpec,
        encoding: ImageEncoding,
    ) -> Result<FfResult<Media>, EngineError> {
        let plan = match thumbnail_plan(media, spec, encoding) {
            Ok(plan) => plan,
            Err(error) => return Ok(Err(error)),
        };
        single_media(&state, plan).await
    }

    /// Concatenate stored media files into one encoded output.
    ///
    /// `spec` chooses video and audio participation and optional normalization needed by FFmpeg's
    /// concat filter. All inputs must resolve to compatible stream layouts after normalization; use
    /// `encoding` to choose the final codecs and container.
    #[rex::export]
    pub(super) async fn concatenate(
        state: State,
        media: Vec<Media>,
        spec: ConcatSpec,
        encoding: Encoding,
    ) -> Result<FfResult<Media>, EngineError> {
        let plan = match concat_plan(media, spec, encoding) {
            Ok(plan) => plan,
            Err(error) => return Ok(Err(error)),
        };
        single_media(&state, plan).await
    }

    /// Select streams from several stored media files and combine them into one output.
    ///
    /// Each `mapping` identifies an input and stream kind/index and chooses copying or encoding.
    /// Input indices are zero-based positions in `media`; invalid indices or incompatible copy/container
    /// combinations return `FfmpegError`.
    #[rex::export]
    pub(super) async fn mux(
        state: State,
        media: Vec<Media>,
        mappings: Vec<MuxMapping>,
        encoding: Encoding,
    ) -> Result<FfResult<Media>, EngineError> {
        let plan = match mux_plan(media, mappings, encoding) {
            Ok(plan) => plan,
            Err(error) => return Ok(Err(error)),
        };
        single_media(&state, plan).await
    }

    /// Encode a source as an ordered list of independent, numbered media segments.
    ///
    /// `segment` controls target duration, timestamp reset behavior, and starting number. Segment
    /// boundaries depend on keyframes and therefore need not occur at exact multiples of the requested
    /// duration.
    #[rex::export]
    pub(super) async fn segment(
        state: State,
        source: MediaSource,
        operations: Vec<MediaOperation>,
        encoding: Encoding,
        segment: SegmentOutput,
    ) -> Result<FfResult<Vec<Media>>, EngineError> {
        let plan = match segment_plan(source, operations, encoding, segment) {
            Ok(plan) => plan,
            Err(error) => return Ok(Err(error)),
        };
        media_sequence(&state, plan).await
    }

    /// Encode a source as an HLS package stored as one content-addressed directory tree.
    ///
    /// The returned package preserves the playlist and segment filenames referenced by that playlist.
    /// `hls` controls segment duration, playlist size, segment format, flags, and master-playlist output.
    #[rex::export]
    pub(super) async fn package_hls(
        state: State,
        source: MediaSource,
        operations: Vec<MediaOperation>,
        encoding: Encoding,
        hls: HlsOutput,
    ) -> Result<FfResult<MediaPackage>, EngineError> {
        let plan = match hls_plan(source, operations, encoding, hls) {
            Ok(plan) => plan,
            Err(error) => return Ok(Err(error)),
        };
        media_package(&state, plan, PackageKind::HlsPackage).await
    }

    /// Encode a source as a DASH package stored as one content-addressed directory tree.
    ///
    /// The returned package preserves the MPD manifest and all referenced segments. `dash` controls
    /// segment duration, live window sizes, and template/timeline generation.
    #[rex::export]
    pub(super) async fn package_dash(
        state: State,
        source: MediaSource,
        operations: Vec<MediaOperation>,
        encoding: Encoding,
        dash: DashOutput,
    ) -> Result<FfResult<MediaPackage>, EngineError> {
        let plan = match dash_plan(source, operations, encoding, dash) {
            Ok(plan) => plan,
            Err(error) => return Ok(Err(error)),
        };
        media_package(&state, plan, PackageKind::DashPackage).await
    }

    /// Read stable container, stream, chapter, and program metadata with FFprobe.
    ///
    /// `options.detail` controls which sections are requested. Frame and packet counting can be
    /// expensive because FFprobe must read the media; `read_intervals` uses FFprobe's interval syntax
    /// and seeking may begin near, rather than exactly at, the requested timestamp.
    #[rex::export]
    pub(super) async fn probe(
        state: State,
        media: Media,
        options: ProbeOptions,
    ) -> Result<FfResult<MediaInfo>, EngineError> {
        let plan = match probe_plan(media, options) {
            Ok(plan) => plan,
            Err(error) => return Ok(Err(error)),
        };
        let execution = execute(&state, plan).await?;
        if !is_success(&execution) {
            return Ok(Err(process_error(&execution)));
        }
        Ok(parse_media_info(&execution.stdout))
    }

    /// Inspect selected FFprobe frame or packet fields.
    ///
    /// `query.kind` selects frames or packets, `stream` optionally narrows the stream, `read_intervals`
    /// uses FFprobe interval syntax, and `entries` contains exact FFprobe field names. Results retain
    /// flexible string dictionaries so codec-specific fields do not change the Rex type surface.
    #[rex::export]
    pub(super) async fn inspect(
        state: State,
        media: Media,
        query: InspectionQuery,
    ) -> Result<FfResult<Vec<InspectionRecord>>, EngineError> {
        let kind = query.kind.clone();
        let plan = match inspect_plan(media, query) {
            Ok(plan) => plan,
            Err(error) => return Ok(Err(error)),
        };
        let execution = execute(&state, plan).await?;
        if !is_success(&execution) {
            return Ok(Err(process_error(&execution)));
        }
        Ok(parse_inspection(&execution.stdout, kind))
    }

    /// Return the installed FFmpeg version, build configuration, and linked library versions.
    #[rex::export]
    pub(super) async fn version(state: State) -> Result<FfResult<VersionInfo>, EngineError> {
        let execution = execute(&state, version_plan()).await?;
        if !is_success(&execution) {
            return Ok(Err(process_error(&execution)));
        }
        Ok(parse_version(&execution.stdout))
    }

    /// List capabilities reported by the installed FFmpeg build for one domain.
    ///
    /// Availability is host-specific: encoders, decoders, filters, formats, devices, and hardware
    /// accelerators vary with the installed build. Each returned item preserves FFmpeg's flags, name,
    /// and description where that domain reports them.
    #[rex::export]
    pub(super) async fn capabilities(
        state: State,
        domain: CapabilityDomain,
    ) -> Result<FfResult<Vec<Capability>>, EngineError> {
        let execution = execute(&state, capabilities_plan(domain.clone())).await?;
        if !is_success(&execution) {
            return Ok(Err(process_error(&execution)));
        }
        let mut bytes = execution.stdout;
        bytes.extend_from_slice(&execution.stderr);
        Ok(parse_capabilities(&bytes, domain))
    }
}

async fn single_media(
    state: &State,
    plan: crate::modules::tools::executor::ToolExecutionPlan,
) -> Result<FfResult<Media>, EngineError> {
    let execution = execute(state, plan).await?;
    if !is_success(&execution) {
        return Ok(Err(process_error(&execution)));
    }
    match execution.outputs.get(&0).map(Vec::as_slice) {
        Some([content]) => Ok(Ok(Media { content: *content })),
        Some(values) => Ok(Err(unexpected(format!(
            "FFmpeg produced {} files where one was expected",
            values.len()
        )))),
        None => Ok(Err(unexpected("FFmpeg did not declare its output"))),
    }
}

async fn media_sequence(
    state: &State,
    plan: crate::modules::tools::executor::ToolExecutionPlan,
) -> Result<FfResult<Vec<Media>>, EngineError> {
    let execution = execute(state, plan).await?;
    if !is_success(&execution) {
        return Ok(Err(process_error(&execution)));
    }
    let media = execution
        .outputs
        .get(&0)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|content| Media { content })
        .collect::<Vec<_>>();
    if media.is_empty() {
        Ok(Err(unexpected("FFmpeg produced no output files")))
    } else {
        Ok(Ok(media))
    }
}

async fn media_package(
    state: &State,
    plan: crate::modules::tools::executor::ToolExecutionPlan,
    kind: PackageKind,
) -> Result<FfResult<MediaPackage>, EngineError> {
    let execution = execute(state, plan).await?;
    if !is_success(&execution) {
        return Ok(Err(process_error(&execution)));
    }
    match execution.outputs.get(&0).map(Vec::as_slice) {
        Some([content]) => Ok(Ok(MediaPackage {
            content: *content,
            kind,
        })),
        Some(values) => Ok(Err(unexpected(format!(
            "FFmpeg produced {} package roots where one was expected",
            values.len()
        )))),
        None => Ok(Err(unexpected("FFmpeg did not produce a package"))),
    }
}

fn artifacts_from_execution(
    execution: &ToolExecution,
    plans: &[PlannedArtifact],
) -> Result<Vec<MediaArtifact>, FfmpegError> {
    let mut artifacts = Vec::with_capacity(plans.len());
    for plan in plans {
        let values = execution
            .outputs
            .get(&plan.output)
            .cloned()
            .unwrap_or_default();
        let artifact = match &plan.kind {
            ArtifactKind::Single => match values.as_slice() {
                [content] => MediaArtifact::EncodedMedia(Media { content: *content }),
                _ => {
                    return Err(unexpected(format!(
                        "FFmpeg output {} contained {} files instead of one",
                        plan.output,
                        values.len()
                    )));
                }
            },
            ArtifactKind::Sequence => {
                if values.is_empty() {
                    return Err(unexpected(format!(
                        "FFmpeg output {} contained no files",
                        plan.output
                    )));
                }
                MediaArtifact::MediaSequence(
                    values
                        .into_iter()
                        .map(|content| Media { content })
                        .collect(),
                )
            }
            ArtifactKind::Package(kind) => match values.as_slice() {
                [content] => MediaArtifact::PackagedMedia(MediaPackage {
                    content: *content,
                    kind: kind.clone(),
                }),
                _ => {
                    return Err(unexpected(format!(
                        "FFmpeg package {} contained {} roots instead of one",
                        plan.output,
                        values.len()
                    )));
                }
            },
        };
        artifacts.push(artifact);
    }
    Ok(artifacts)
}

async fn execute(
    state: &State,
    plan: crate::modules::tools::executor::ToolExecutionPlan,
) -> Result<ToolExecution, EngineError> {
    state
        .execute_tool(plan)
        .await
        .map_err(|error| EngineError::Custom(error.to_string()))
}

fn parse_media_info(bytes: &[u8]) -> FfResult<MediaInfo> {
    let root = parse_json(bytes, "ffprobe media information")?;
    let object = root
        .as_object()
        .ok_or_else(|| unexpected("ffprobe returned a non-object JSON document"))?;
    let format = object
        .get("format")
        .and_then(Value::as_object)
        .map(parse_format);
    let streams = object
        .get("streams")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .map(parse_stream)
        .collect::<Vec<_>>();
    let chapters = object
        .get("chapters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .map(parse_chapter)
        .collect::<Vec<_>>();
    let programs = object
        .get("programs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .map(parse_program)
        .collect::<Vec<_>>();
    Ok(MediaInfo {
        format,
        streams,
        chapters,
        programs,
    })
}

fn parse_format(object: &Map<String, Value>) -> FormatInfo {
    FormatInfo {
        format_name: string_field(object, "format_name"),
        format_long_name: string_field(object, "format_long_name"),
        start_time: f64_field(object, "start_time"),
        duration: f64_field(object, "duration"),
        size: u64_field(object, "size"),
        bit_rate: u64_field(object, "bit_rate"),
        probe_score: u64_field(object, "probe_score"),
        tags: string_map(object.get("tags")),
    }
}

fn parse_stream(object: &Map<String, Value>) -> StreamInfo {
    let tags = string_map(object.get("tags"));
    let language = tags.get("language").cloned();
    StreamInfo {
        index: u64_field(object, "index").unwrap_or(0),
        kind: media_kind(&string_field(object, "codec_type")),
        codec_name: string_field(object, "codec_name"),
        codec_long_name: string_field(object, "codec_long_name"),
        profile: optional_string(object, "profile"),
        codec_tag: optional_string(object, "codec_tag_string"),
        width: u64_field(object, "width"),
        height: u64_field(object, "height"),
        pixel_format: optional_string(object, "pix_fmt"),
        sample_aspect_ratio: optional_string(object, "sample_aspect_ratio"),
        display_aspect_ratio: optional_string(object, "display_aspect_ratio"),
        frame_rate: optional_string(object, "avg_frame_rate"),
        sample_rate: u64_field(object, "sample_rate"),
        channels: u64_field(object, "channels"),
        channel_layout: optional_string(object, "channel_layout"),
        bit_rate: u64_field(object, "bit_rate"),
        start_time: f64_field(object, "start_time"),
        duration: f64_field(object, "duration"),
        frame_count: u64_field(object, "nb_frames").or_else(|| u64_field(object, "nb_read_frames")),
        packet_count: u64_field(object, "nb_read_packets"),
        language,
        disposition: bool_map(object.get("disposition")),
        tags,
    }
}

fn parse_chapter(object: &Map<String, Value>) -> ChapterInfo {
    ChapterInfo {
        id: u64_field(object, "id").unwrap_or(0),
        start: f64_field(object, "start_time").unwrap_or(0.0),
        end: f64_field(object, "end_time").unwrap_or(0.0),
        tags: string_map(object.get("tags")),
    }
}

fn parse_program(object: &Map<String, Value>) -> ProgramInfo {
    let stream_indices = object
        .get("streams")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .filter_map(|stream| u64_field(stream, "index"))
        .collect();
    ProgramInfo {
        id: u64_field(object, "program_id")
            .or_else(|| u64_field(object, "id"))
            .unwrap_or(0),
        stream_indices,
        tags: string_map(object.get("tags")),
    }
}

fn parse_inspection(bytes: &[u8], kind: InspectionKind) -> FfResult<Vec<InspectionRecord>> {
    let root = parse_json(bytes, "ffprobe inspection")?;
    let key = match kind {
        InspectionKind::InspectPackets => "packets",
        InspectionKind::InspectFrames => "frames",
    };
    let records = root
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .map(|object| InspectionRecord {
            fields: object
                .iter()
                .map(|(name, value)| (name.clone(), json_field(value)))
                .collect(),
        })
        .collect::<Vec<_>>();
    Ok(records)
}

fn parse_version(bytes: &[u8]) -> FfResult<VersionInfo> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| unexpected("ffmpeg -version returned non-UTF-8 output"))?;
    let mut lines = text.lines();
    let first = lines
        .next()
        .ok_or_else(|| unexpected("ffmpeg -version returned no output"))?;
    let version = first
        .strip_prefix("ffmpeg version ")
        .and_then(|value| value.split_whitespace().next())
        .unwrap_or(first)
        .to_string();
    let mut configuration = String::new();
    let mut libraries = BTreeMap::new();
    for line in lines {
        if let Some(value) = line.strip_prefix("configuration: ") {
            configuration = value.to_string();
            continue;
        }
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() >= 3 && fields[0].starts_with("lib") {
            let separator = fields
                .iter()
                .position(|field| *field == "/")
                .unwrap_or(fields.len());
            libraries.insert(fields[0].to_string(), fields[1..separator].join(""));
        }
    }
    Ok(VersionInfo {
        version,
        configuration,
        libraries,
    })
}

fn parse_capabilities(bytes: &[u8], domain: CapabilityDomain) -> FfResult<Vec<Capability>> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| unexpected("ffmpeg capability listing returned non-UTF-8 output"))?;
    let mut capabilities = Vec::new();
    let mut protocol_direction = String::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("ffmpeg version") {
            continue;
        }
        if domain == CapabilityDomain::Protocols {
            if trimmed == "Input:" || trimmed == "Output:" {
                protocol_direction = trimmed.trim_end_matches(':').to_ascii_lowercase();
                continue;
            }
            if !protocol_direction.is_empty()
                && trimmed.chars().all(|character| !character.is_whitespace())
            {
                capabilities.push(Capability {
                    flags: protocol_direction.clone(),
                    name: trimmed.to_string(),
                    description: String::new(),
                });
            }
            continue;
        }
        if trimmed.starts_with('-')
            || trimmed.starts_with("Encoders:")
            || trimmed.starts_with("Decoders:")
            || trimmed.starts_with("Codecs:")
            || trimmed.starts_with("Filters:")
            || trimmed.starts_with("File formats:")
            || trimmed.starts_with("Devices:")
            || trimmed.starts_with("Pixel formats:")
            || trimmed.starts_with("Hardware acceleration methods:")
            || trimmed.contains(" = ")
        {
            continue;
        }
        let fields = trimmed.split_whitespace().collect::<Vec<_>>();
        if fields.is_empty() {
            continue;
        }
        let (flags, name, description_start) = if fields.len() >= 2
            && fields[0]
                .chars()
                .all(|character| character.is_ascii_alphabetic() || character == '.')
        {
            (fields[0], fields[1], 2)
        } else {
            ("", fields[0], 1)
        };
        capabilities.push(Capability {
            flags: flags.to_string(),
            name: name.trim_end_matches(',').to_string(),
            description: fields[description_start..].join(" "),
        });
    }
    if capabilities.is_empty() {
        Err(unexpected("ffmpeg capability listing contained no entries"))
    } else {
        Ok(capabilities)
    }
}

fn parse_json(bytes: &[u8], context: &str) -> Result<Value, FfmpegError> {
    serde_json::from_slice(bytes).map_err(|error| unexpected(format!("invalid {context}: {error}")))
}

fn string_field(object: &Map<String, Value>, name: &str) -> String {
    optional_string(object, name).unwrap_or_default()
}

fn optional_string(object: &Map<String, Value>, name: &str) -> Option<String> {
    object.get(name).and_then(|value| match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    })
}

fn u64_field(object: &Map<String, Value>, name: &str) -> Option<u64> {
    object.get(name).and_then(|value| match value {
        Value::Number(value) => value.as_u64(),
        Value::String(value) => value.parse().ok(),
        _ => None,
    })
}

fn f64_field(object: &Map<String, Value>, name: &str) -> Option<f64> {
    object.get(name).and_then(|value| match value {
        Value::Number(value) => value.as_f64(),
        Value::String(value) => value.parse().ok(),
        _ => None,
    })
}

fn string_map(value: Option<&Value>) -> BTreeMap<String, String> {
    value
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .map(|(name, value)| (name.clone(), json_field(value)))
        .collect()
}

fn bool_map(value: Option<&Value>) -> BTreeMap<String, bool> {
    value
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(name, value)| {
            value
                .as_bool()
                .or_else(|| value.as_i64().map(|number| number != 0))
                .map(|value| (name.clone(), value))
        })
        .collect()
}

fn json_field(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => value.clone(),
        Value::Array(_) | Value::Object(_) => value.to_string(),
    }
}

fn media_kind(value: &str) -> MediaKind {
    match value {
        "video" => MediaKind::VideoStream,
        "audio" => MediaKind::AudioStream,
        "subtitle" => MediaKind::SubtitleStream,
        "data" => MediaKind::DataStream,
        "attachment" => MediaKind::AttachmentStream,
        other => MediaKind::UnknownStream(other.to_string()),
    }
}

fn is_success(execution: &ToolExecution) -> bool {
    execution.exit_code == Some(0)
}

fn process_error(execution: &ToolExecution) -> FfmpegError {
    let stderr = String::from_utf8_lossy(&execution.stderr)
        .trim()
        .to_string();
    let stdout = String::from_utf8_lossy(&execution.stdout)
        .trim()
        .to_string();
    FfmpegError {
        kind: FfmpegErrorKind::ProcessFailed,
        exit_code: execution.exit_code.map(i64::from),
        message: if stderr.is_empty() { stdout } else { stderr },
    }
}

fn unexpected(message: impl Into<String>) -> FfmpegError {
    FfmpegError {
        kind: FfmpegErrorKind::UnexpectedOutput,
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
    fn ffprobe_json_is_decoded_semantically() {
        let info = parse_media_info(
            br#"{
                "streams": [{
                    "index": 0,
                    "codec_name": "h264",
                    "codec_long_name": "H.264",
                    "codec_type": "video",
                    "width": 1920,
                    "height": 1080,
                    "avg_frame_rate": "30000/1001",
                    "disposition": {"default": 1},
                    "tags": {"language": "eng"}
                }],
                "format": {
                    "format_name": "mov,mp4",
                    "format_long_name": "QuickTime / MOV",
                    "duration": "12.5",
                    "size": "1000"
                }
            }"#,
        )
        .unwrap();
        assert_eq!(info.streams[0].kind, MediaKind::VideoStream);
        assert_eq!(info.streams[0].width, Some(1920));
        assert_eq!(info.streams[0].language.as_deref(), Some("eng"));
        assert_eq!(info.format.unwrap().duration, Some(12.5));
    }

    #[tokio::test]
    async fn docker_ffmpeg_generates_transcodes_and_probes_media_when_enabled() {
        if std::env::var("REX_WORKFLOW_DOCKER_TESTS").as_deref() != Ok("1") {
            return;
        }
        let state = crate::development_state(Store::new_in_memory());
        let media = transcode(
            state.clone(),
            MediaSource::TestVideo(TestVideoSource {
                pattern: TestPattern::TestSource,
                size: VideoSize {
                    width: 96,
                    height: 64,
                },
                frame_rate: Rational {
                    numerator: 10,
                    denominator: 1,
                },
                duration: Some(Time { seconds: 0.5 }),
            }),
            vec![],
            test_encoding(),
        )
        .await
        .unwrap()
        .unwrap();
        let info = probe(
            state.clone(),
            media.clone(),
            ProbeOptions {
                detail: ProbeDetail::ProbeAll,
                count_frames: false,
                count_packets: false,
                read_intervals: None,
            },
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(info.streams[0].width, Some(96));
        assert_eq!(info.streams[0].height, Some(64));

        let thumbnail = thumbnail(
            state.clone(),
            media.clone(),
            ThumbnailSpec {
                at: Some(Time { seconds: 0.1 }),
                size: Some(VideoSize {
                    width: 48,
                    height: 32,
                }),
                preserve_aspect_ratio: true,
            },
            ImageEncoding {
                format: ContainerFormat {
                    name: "png".to_string(),
                },
                video: VideoEncoding {
                    codec: VideoCodec::PngVideo,
                    options: vec![],
                },
            },
        )
        .await
        .unwrap()
        .unwrap();
        assert!(state.store.size(thumbnail.content).await.unwrap() > 0);

        let package = package_hls(
            state.clone(),
            MediaSource::StoredMedia(media),
            vec![],
            test_encoding(),
            HlsOutput {
                segment_duration: Time { seconds: 0.25 },
                playlist_size: 0,
                segment_format: "mpegts".to_string(),
                flags: vec!["independent_segments".to_string()],
                master_playlist: false,
            },
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(package.kind, PackageKind::HlsPackage);
        assert!(
            !state
                .store
                .get_tree(package.content)
                .await
                .unwrap()
                .is_empty()
        );
        let unpacked = transcode(
            state.clone(),
            MediaSource::StoredPackage(package),
            vec![],
            test_encoding(),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(state.store.size(unpacked.content).await.unwrap() > 0);

        let version = version(state.clone()).await.unwrap().unwrap();
        assert!(!version.version.is_empty());
        let encoders = capabilities(state, CapabilityDomain::Encoders)
            .await
            .unwrap()
            .unwrap();
        assert!(encoders.iter().any(|encoder| encoder.name == "libx264"));
    }

    #[tokio::test]
    async fn docker_ffmpeg_specialized_functions_exchange_cas_media_when_enabled() {
        if std::env::var("REX_WORKFLOW_DOCKER_TESTS").as_deref() != Ok("1") {
            return;
        }
        let state = crate::development_state(Store::new_in_memory());
        let video = transcode(
            state.clone(),
            MediaSource::TestVideo(TestVideoSource {
                pattern: TestPattern::TestSource2,
                size: VideoSize {
                    width: 80,
                    height: 48,
                },
                frame_rate: Rational {
                    numerator: 8,
                    denominator: 1,
                },
                duration: Some(Time { seconds: 0.75 }),
            }),
            vec![],
            test_encoding(),
        )
        .await
        .unwrap()
        .unwrap();
        let audio = transcode(
            state.clone(),
            MediaSource::SineAudio(SineAudioSource {
                frequency: 440.0,
                sample_rate: 48_000,
                duration: Some(Time { seconds: 0.75 }),
            }),
            vec![],
            Encoding {
                format: ContainerFormat {
                    name: "wav".to_string(),
                },
                video: None,
                audio: Some(AudioEncoding {
                    codec: AudioCodec::PcmS16Le,
                    options: vec![],
                }),
                subtitle: None,
                options: vec![],
                metadata: BTreeMap::new(),
            },
        )
        .await
        .unwrap()
        .unwrap();

        let muxed = mux(
            state.clone(),
            vec![video.clone(), audio],
            vec![
                MuxMapping {
                    input: 0,
                    kind: MediaKind::VideoStream,
                    stream_index: Some(0),
                    copy: true,
                },
                MuxMapping {
                    input: 1,
                    kind: MediaKind::AudioStream,
                    stream_index: Some(0),
                    copy: false,
                },
            ],
            Encoding {
                format: ContainerFormat {
                    name: "mp4".to_string(),
                },
                video: None,
                audio: Some(AudioEncoding {
                    codec: AudioCodec::Aac,
                    options: vec![AudioEncodeOption::AudioBitRate(96_000)],
                }),
                subtitle: None,
                options: vec![MuxOption::ShortestOutput],
                metadata: BTreeMap::new(),
            },
        )
        .await
        .unwrap()
        .unwrap();
        let info = probe(
            state.clone(),
            muxed.clone(),
            ProbeOptions {
                detail: ProbeDetail::ProbeStreams,
                count_frames: false,
                count_packets: false,
                read_intervals: None,
            },
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            info.streams
                .iter()
                .any(|stream| stream.kind == MediaKind::VideoStream)
        );
        assert!(
            info.streams
                .iter()
                .any(|stream| stream.kind == MediaKind::AudioStream)
        );

        let extracted = extract_audio(
            state.clone(),
            muxed.clone(),
            AudioEncoding {
                codec: AudioCodec::Flac,
                options: vec![],
            },
            ContainerFormat {
                name: "flac".to_string(),
            },
        )
        .await
        .unwrap()
        .unwrap();
        assert!(state.store.size(extracted.content).await.unwrap() > 0);

        let frames = extract_frames(
            state.clone(),
            muxed.clone(),
            FrameSelection::EveryNthFrame(2),
            ImageEncoding {
                format: ContainerFormat {
                    name: "png".to_string(),
                },
                video: VideoEncoding {
                    codec: VideoCodec::PngVideo,
                    options: vec![],
                },
            },
        )
        .await
        .unwrap()
        .unwrap();
        assert!(!frames.is_empty());
        let selected_frames = extract_frames(
            state.clone(),
            muxed.clone(),
            FrameSelection::AtTimes(vec![Time { seconds: 0.1 }, Time { seconds: 0.4 }]),
            ImageEncoding {
                format: ContainerFormat {
                    name: "jpg".to_string(),
                },
                video: VideoEncoding {
                    codec: VideoCodec::MjpegVideo,
                    options: vec![VideoEncodeOption::VideoQuality(3.0)],
                },
            },
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(selected_frames.len(), 2);

        let concatenated = concatenate(
            state.clone(),
            vec![video.clone(), video],
            ConcatSpec {
                video: true,
                audio: false,
                normalize_video: None,
                normalize_video_frame_rate: None,
                normalize_video_pixel_format: None,
                normalize_audio_rate: None,
                normalize_audio_channel_layout: None,
            },
            test_encoding(),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(state.store.size(concatenated.content).await.unwrap() > 0);

        let segments = segment(
            state.clone(),
            MediaSource::StoredMedia(muxed.clone()),
            vec![],
            test_encoding(),
            SegmentOutput {
                segment_duration: Time { seconds: 0.25 },
                reset_timestamps: true,
                start_number: 0,
            },
        )
        .await
        .unwrap()
        .unwrap();
        assert!(!segments.is_empty());

        let artifacts = render(
            state.clone(),
            MediaProgram {
                inputs: vec![MediaInput {
                    source: MediaSource::StoredMedia(muxed.clone()),
                    options: vec![],
                }],
                filters: FilterGraph { chains: vec![] },
                outputs: vec![copied_output("mp4"), copied_output("matroska")],
            },
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(artifacts.len(), 2);
        assert!(
            artifacts
                .iter()
                .all(|artifact| matches!(artifact, MediaArtifact::EncodedMedia(_)))
        );

        let records = inspect(
            state,
            muxed,
            InspectionQuery {
                kind: InspectionKind::InspectPackets,
                stream: Some(StreamRef {
                    input: 0,
                    kind: MediaKind::VideoStream,
                    index: Some(0),
                }),
                read_intervals: Some("%+0.2".to_string()),
                entries: vec!["pts_time".to_string(), "size".to_string()],
            },
        )
        .await
        .unwrap()
        .unwrap();
        assert!(!records.is_empty());
    }

    fn copied_output(format: &str) -> MediaOutput {
        let stream = |kind: MediaKind| OutputStream {
            source: StreamSource::InputStream(StreamRef {
                input: 0,
                kind: kind.clone(),
                index: Some(0),
            }),
            encoding: StreamEncoding::CopyStream(kind),
            metadata: BTreeMap::new(),
            dispositions: vec![],
        };
        MediaOutput {
            format: ContainerFormat {
                name: format.to_string(),
            },
            mode: OutputMode::SingleFile,
            streams: vec![
                stream(MediaKind::VideoStream),
                stream(MediaKind::AudioStream),
            ],
            options: vec![],
            metadata: BTreeMap::new(),
        }
    }

    fn test_encoding() -> Encoding {
        Encoding {
            format: ContainerFormat {
                name: "mp4".to_string(),
            },
            video: Some(VideoEncoding {
                codec: VideoCodec::H264,
                options: vec![
                    VideoEncodeOption::Preset("ultrafast".to_string()),
                    VideoEncodeOption::PixelFormat("yuv420p".to_string()),
                ],
            }),
            audio: None,
            subtitle: None,
            options: vec![MuxOption::MovFlags(vec!["faststart".to_string()])],
            metadata: BTreeMap::new(),
        }
    }
}

use super::types::*;
use crate::modules::tools::executor::{
    CasInput, ExpectedOutput, InputKind, OutputId, OutputKind, ToolArgument, ToolExecutionPlan,
    ToolProgram,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ArtifactKind {
    Single,
    Sequence,
    Package(PackageKind),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PlannedArtifact {
    pub output: OutputId,
    pub kind: ArtifactKind,
}

struct PlanBuilder {
    program: ToolProgram,
    arguments: Vec<ToolArgument>,
    inputs: Vec<CasInput>,
    outputs: Vec<ExpectedOutput>,
}

impl PlanBuilder {
    fn ffmpeg() -> Self {
        Self {
            program: ToolProgram::new("ffmpeg"),
            arguments: vec![
                ToolArgument::literal("-hide_banner"),
                ToolArgument::literal("-nostdin"),
                ToolArgument::literal("-y"),
                ToolArgument::literal("-loglevel"),
                ToolArgument::literal("error"),
                ToolArgument::literal("-nostats"),
            ],
            inputs: Vec::new(),
            outputs: Vec::new(),
        }
    }

    fn command(program: ToolProgram) -> Self {
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

    fn argument(&mut self, argument: ToolArgument) {
        self.arguments.push(argument);
    }

    fn option(&mut self, name: &str, value: impl Into<String>) {
        self.literal(name);
        self.literal(value);
    }

    fn input_blob(&mut self, media: Media, extension: &str) -> usize {
        let id = self.inputs.len();
        self.inputs.push(CasInput {
            hash: media.content,
            extension: extension.to_string(),
            kind: InputKind::Blob,
        });
        id
    }

    fn input_tree(&mut self, package: MediaPackage) -> usize {
        let id = self.inputs.len();
        self.inputs.push(CasInput {
            hash: package.content,
            extension: "bundle".to_string(),
            kind: InputKind::Tree,
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
        }
    }
}

pub(crate) fn render_plan(
    program: MediaProgram,
) -> Result<(ToolExecutionPlan, Vec<PlannedArtifact>), FfmpegError> {
    if program.inputs.is_empty() {
        return Err(invalid("render requires at least one input"));
    }
    if program.outputs.is_empty() {
        return Err(invalid("render requires at least one output"));
    }
    let mut builder = PlanBuilder::ffmpeg();
    for input in program.inputs {
        compile_input(&mut builder, input)?;
    }
    compile_filter_graph(&mut builder, program.filters)?;
    let mut artifacts = Vec::new();
    for output in program.outputs {
        artifacts.push(compile_output(&mut builder, output)?);
    }
    Ok((builder.finish(), artifacts))
}

pub(crate) fn transcode_plan(
    source: MediaSource,
    operations: Vec<MediaOperation>,
    encoding: Encoding,
) -> Result<ToolExecutionPlan, FfmpegError> {
    simple_plan(source, operations, encoding, OutputMode::SingleFile).map(|(plan, _)| plan)
}

pub(crate) fn segment_plan(
    source: MediaSource,
    operations: Vec<MediaOperation>,
    encoding: Encoding,
    segment: SegmentOutput,
) -> Result<ToolExecutionPlan, FfmpegError> {
    simple_plan(
        source,
        operations,
        encoding,
        OutputMode::SegmentedFiles(segment),
    )
    .map(|(plan, _)| plan)
}

pub(crate) fn hls_plan(
    source: MediaSource,
    operations: Vec<MediaOperation>,
    encoding: Encoding,
    hls: HlsOutput,
) -> Result<ToolExecutionPlan, FfmpegError> {
    simple_plan(source, operations, encoding, OutputMode::HlsStreaming(hls)).map(|(plan, _)| plan)
}

pub(crate) fn dash_plan(
    source: MediaSource,
    operations: Vec<MediaOperation>,
    encoding: Encoding,
    dash: DashOutput,
) -> Result<ToolExecutionPlan, FfmpegError> {
    simple_plan(
        source,
        operations,
        encoding,
        OutputMode::DashStreaming(dash),
    )
    .map(|(plan, _)| plan)
}

fn simple_plan(
    source: MediaSource,
    operations: Vec<MediaOperation>,
    encoding: Encoding,
    mode: OutputMode,
) -> Result<(ToolExecutionPlan, PlannedArtifact), FfmpegError> {
    let mut builder = PlanBuilder::ffmpeg();
    compile_input(
        &mut builder,
        MediaInput {
            source,
            options: Vec::new(),
        },
    )?;

    let mut video_filters = Vec::new();
    let mut audio_filters = Vec::new();
    let mut drop_video = encoding.video.is_none();
    let mut drop_audio = encoding.audio.is_none();
    let mut drop_subtitles = encoding.subtitle.is_none();
    let mut selected_video = None;
    let mut selected_audio = None;
    let mut selected_subtitle = None;
    let mut operation_metadata = Vec::new();
    for operation in operations {
        match operation {
            MediaOperation::Trim(range) => compile_time_range(&mut builder, &range),
            MediaOperation::VideoOperation(filter) => video_filters.push(filter),
            MediaOperation::AudioOperation(filter) => audio_filters.push(filter),
            MediaOperation::DropVideo => drop_video = true,
            MediaOperation::DropAudio => drop_audio = true,
            MediaOperation::DropSubtitles => drop_subtitles = true,
            MediaOperation::SelectVideoStream(index) => selected_video = Some(index),
            MediaOperation::SelectAudioStream(index) => selected_audio = Some(index),
            MediaOperation::SelectSubtitleStream(index) => selected_subtitle = Some(index),
            MediaOperation::SetOutputMetadata(name, value) => {
                operation_metadata.push((name, value))
            }
        }
    }
    if drop_video && drop_audio && drop_subtitles {
        return Err(invalid("transcode would omit every media stream"));
    }
    if !video_filters.is_empty() {
        builder.literal("-vf");
        let argument = compile_video_filter_chain(&mut builder, video_filters)?;
        builder.argument(argument);
    }
    if !audio_filters.is_empty() {
        builder.literal("-af");
        builder.argument(compile_audio_filter_chain(audio_filters)?);
    }

    let explicit_selection = selected_video.is_some()
        || selected_audio.is_some()
        || selected_subtitle.is_some()
        || matches!(&mode, OutputMode::SegmentedFiles(_));
    if explicit_selection {
        if !drop_video {
            builder.option(
                "-map",
                selected_video.map_or_else(|| "0:v:0?".to_string(), |index| format!("0:v:{index}")),
            );
        }
        if !drop_audio {
            builder.option(
                "-map",
                selected_audio.map_or_else(|| "0:a:0?".to_string(), |index| format!("0:a:{index}")),
            );
        }
        if !drop_subtitles {
            builder.option(
                "-map",
                selected_subtitle
                    .map_or_else(|| "0:s:0?".to_string(), |index| format!("0:s:{index}")),
            );
        }
    }

    if drop_video {
        builder.literal("-vn");
    } else if let Some(video) = encoding.video {
        compile_video_encoding(&mut builder, &video, "v:0")?;
    }
    if drop_audio {
        builder.literal("-an");
    } else if let Some(audio) = encoding.audio {
        compile_audio_encoding(&mut builder, &audio, "a:0")?;
    }
    if drop_subtitles {
        builder.literal("-sn");
    } else if let Some(subtitle) = encoding.subtitle {
        compile_subtitle_encoding(&mut builder, &subtitle, "s:0")?;
    }
    for option in encoding.options {
        compile_mux_option(&mut builder, option)?;
    }
    for (name, value) in operation_metadata {
        builder.option("-metadata", format!("{name}={value}"));
    }
    for (name, value) in encoding.metadata {
        builder.option("-metadata", format!("{name}={value}"));
    }
    let artifact = compile_output_target(&mut builder, encoding.format, mode)?;
    Ok((builder.finish(), artifact))
}

pub(crate) fn remux_plan(
    media: Media,
    format: ContainerFormat,
    options: Vec<MuxOption>,
) -> Result<ToolExecutionPlan, FfmpegError> {
    let mut builder = PlanBuilder::ffmpeg();
    compile_input(
        &mut builder,
        MediaInput {
            source: MediaSource::StoredMedia(media),
            options: Vec::new(),
        },
    )?;
    builder.option("-map", "0");
    builder.option("-c", "copy");
    for option in options {
        compile_mux_option(&mut builder, option)?;
    }
    compile_output_target(&mut builder, format, OutputMode::SingleFile)?;
    Ok(builder.finish())
}

pub(crate) fn extract_audio_plan(
    media: Media,
    encoding: AudioEncoding,
    format: ContainerFormat,
) -> Result<ToolExecutionPlan, FfmpegError> {
    let mut builder = PlanBuilder::ffmpeg();
    compile_input(
        &mut builder,
        MediaInput {
            source: MediaSource::StoredMedia(media),
            options: Vec::new(),
        },
    )?;
    builder.option("-map", "0:a:0");
    builder.literal("-vn");
    builder.literal("-sn");
    compile_audio_encoding(&mut builder, &encoding, "a:0")?;
    compile_output_target(&mut builder, format, OutputMode::SingleFile)?;
    Ok(builder.finish())
}

pub(crate) fn extract_frames_plan(
    media: Media,
    selection: FrameSelection,
    encoding: ImageEncoding,
) -> Result<ToolExecutionPlan, FfmpegError> {
    let mut builder = PlanBuilder::ffmpeg();
    compile_input(
        &mut builder,
        MediaInput {
            source: MediaSource::StoredMedia(media),
            options: Vec::new(),
        },
    )?;
    builder.option("-map", "0:v:0");
    builder.literal("-an");
    builder.literal("-sn");
    match selection {
        FrameSelection::EveryFrame => {}
        FrameSelection::FramesPerSecond(rate) => builder.option("-vf", format!("fps={rate}")),
        FrameSelection::EveryNthFrame(n) => {
            if n == 0 {
                return Err(invalid("EveryNthFrame requires a positive interval"));
            }
            builder.option("-vf", format!("select=not(mod(n\\,{n}))"));
            builder.option("-fps_mode", "vfr");
        }
        FrameSelection::AtTimes(times) => {
            let _ = times;
            return Err(invalid(
                "AtTimes is executed as one precise extraction per timestamp",
            ));
        }
        FrameSelection::SceneChanges(threshold) => {
            builder.option("-vf", format!("select=gt(scene\\,{threshold})"));
            builder.option("-fps_mode", "vfr");
        }
        FrameSelection::BestThumbnail(frames) => {
            if frames == 0 {
                return Err(invalid("BestThumbnail requires a positive frame count"));
            }
            builder.option("-vf", format!("thumbnail={frames}"));
            builder.option("-fps_mode", "vfr");
        }
    }
    compile_video_encoding(&mut builder, &encoding.video, "v:0")?;
    compile_image_output_target(&mut builder, encoding.format, true)?;
    Ok(builder.finish())
}

pub(crate) fn thumbnail_plan(
    media: Media,
    spec: ThumbnailSpec,
    encoding: ImageEncoding,
) -> Result<ToolExecutionPlan, FfmpegError> {
    let mut builder = PlanBuilder::ffmpeg();
    compile_input(
        &mut builder,
        MediaInput {
            source: MediaSource::StoredMedia(media),
            options: Vec::new(),
        },
    )?;
    if let Some(time) = spec.at {
        builder.option("-ss", seconds(time));
    }
    builder.option("-map", "0:v:0");
    builder.literal("-an");
    builder.literal("-sn");
    if let Some(size) = spec.size {
        let force = if spec.preserve_aspect_ratio {
            ":force_original_aspect_ratio=decrease"
        } else {
            ""
        };
        builder.option(
            "-vf",
            format!("scale={}:{}{force}", size.width, size.height),
        );
    }
    builder.option("-frames:v", "1");
    compile_video_encoding(&mut builder, &encoding.video, "v:0")?;
    compile_image_output_target(&mut builder, encoding.format, false)?;
    Ok(builder.finish())
}

pub(crate) fn concat_plan(
    media: Vec<Media>,
    spec: ConcatSpec,
    encoding: Encoding,
) -> Result<ToolExecutionPlan, FfmpegError> {
    if media.len() < 2 {
        return Err(invalid("concatenate requires at least two media inputs"));
    }
    if !spec.video && !spec.audio {
        return Err(invalid("concatenate must include video, audio, or both"));
    }
    if spec.video != encoding.video.is_some() || spec.audio != encoding.audio.is_some() {
        return Err(invalid(
            "concatenate encoding must define exactly the enabled video and audio outputs",
        ));
    }
    if encoding.subtitle.is_some() {
        return Err(invalid("concatenate does not produce subtitle streams"));
    }
    let mut builder = PlanBuilder::ffmpeg();
    for item in media {
        compile_input(
            &mut builder,
            MediaInput {
                source: MediaSource::StoredMedia(item),
                options: Vec::new(),
            },
        )?;
    }
    let input_count = builder.inputs.len();
    let mut graph = String::new();
    for index in 0..input_count {
        if spec.video {
            let mut filters = Vec::new();
            if let Some(scale) = &spec.normalize_video {
                filters.push(video_filter_text(&VideoFilter::Scale(scale.clone()))?);
            }
            if let Some(rate) = &spec.normalize_video_frame_rate {
                filters.push(format!("fps={}", rational(rate)?));
            }
            if let Some(format) = &spec.normalize_video_pixel_format {
                filters.push(format!("format={}", quote_filter_value(format)));
            }
            filters.push("setpts=PTS-STARTPTS".to_string());
            graph.push_str(&format!("[{index}:v:0]{}[v{index}];", filters.join(",")));
        }
        if spec.audio {
            let mut filters = Vec::new();
            if let Some(rate) = spec.normalize_audio_rate {
                filters.push(format!("aresample={rate}"));
            }
            if let Some(layout) = &spec.normalize_audio_channel_layout {
                filters.push(format!(
                    "aformat=channel_layouts={}",
                    quote_filter_value(layout)
                ));
            }
            filters.push("asetpts=PTS-STARTPTS".to_string());
            graph.push_str(&format!("[{index}:a:0]{}[a{index}];", filters.join(",")));
        }
    }
    for index in 0..input_count {
        if spec.video {
            graph.push_str(&format!("[v{index}]"));
        }
        if spec.audio {
            graph.push_str(&format!("[a{index}]"));
        }
    }
    graph.push_str(&format!(
        "concat=n={input_count}:v={}:a={}",
        u8::from(spec.video),
        u8::from(spec.audio)
    ));
    if spec.video {
        graph.push_str("[video]");
    }
    if spec.audio {
        graph.push_str("[audio]");
    }
    builder.option("-filter_complex", graph);
    if spec.video {
        builder.option("-map", "[video]");
    }
    if spec.audio {
        builder.option("-map", "[audio]");
    }
    compile_simple_encoding(&mut builder, encoding)?;
    Ok(builder.finish())
}

pub(crate) fn mux_plan(
    media: Vec<Media>,
    mappings: Vec<MuxMapping>,
    encoding: Encoding,
) -> Result<ToolExecutionPlan, FfmpegError> {
    if media.is_empty() || mappings.is_empty() {
        return Err(invalid("mux requires inputs and stream mappings"));
    }
    let mut builder = PlanBuilder::ffmpeg();
    for item in media {
        compile_input(
            &mut builder,
            MediaInput {
                source: MediaSource::StoredMedia(item),
                options: Vec::new(),
            },
        )?;
    }
    let mut video = 0usize;
    let mut audio = 0usize;
    let mut subtitle = 0usize;
    let mut data = 0usize;
    let mut attachment = 0usize;
    for mapping in mappings {
        if mapping.input as usize >= builder.inputs.len() {
            return Err(invalid(format!(
                "mux input {} does not exist",
                mapping.input
            )));
        }
        let selector = stream_selector(&StreamRef {
            input: mapping.input,
            kind: mapping.kind.clone(),
            index: mapping.stream_index,
        })?;
        builder.option("-map", selector);
        let (specifier, fallback) = match mapping.kind {
            MediaKind::VideoStream => {
                let specifier = format!("v:{video}");
                video += 1;
                (
                    specifier,
                    encoding.video.as_ref().map(StreamEncodingRef::Video),
                )
            }
            MediaKind::AudioStream => {
                let specifier = format!("a:{audio}");
                audio += 1;
                (
                    specifier,
                    encoding.audio.as_ref().map(StreamEncodingRef::Audio),
                )
            }
            MediaKind::SubtitleStream => {
                let specifier = format!("s:{subtitle}");
                subtitle += 1;
                (
                    specifier,
                    encoding.subtitle.as_ref().map(StreamEncodingRef::Subtitle),
                )
            }
            MediaKind::DataStream => {
                let specifier = format!("d:{data}");
                data += 1;
                (specifier, None)
            }
            MediaKind::AttachmentStream => {
                let specifier = format!("t:{attachment}");
                attachment += 1;
                (specifier, None)
            }
            MediaKind::UnknownStream(name) => {
                return Err(invalid(format!("cannot mux unknown stream kind `{name}`")));
            }
        };
        if mapping.copy {
            builder.option(&format!("-c:{specifier}"), "copy");
        } else if let Some(fallback) = fallback {
            compile_encoding_ref(&mut builder, fallback, &specifier)?;
        } else {
            return Err(invalid(format!(
                "mux mapping for {specifier} must use stream copy"
            )));
        }
    }
    for option in encoding.options {
        compile_mux_option(&mut builder, option)?;
    }
    for (name, value) in encoding.metadata {
        builder.option("-metadata", format!("{name}={value}"));
    }
    compile_output_target(&mut builder, encoding.format, OutputMode::SingleFile)?;
    Ok(builder.finish())
}

pub(crate) fn probe_plan(
    media: Media,
    options: ProbeOptions,
) -> Result<ToolExecutionPlan, FfmpegError> {
    let mut builder = PlanBuilder::command(ToolProgram::new("ffprobe"));
    builder.option("-v", "error");
    builder.option("-print_format", "json");
    match options.detail {
        ProbeDetail::ProbeContainer => builder.literal("-show_format"),
        ProbeDetail::ProbeStreams => builder.literal("-show_streams"),
        ProbeDetail::ProbeChapters => builder.literal("-show_chapters"),
        ProbeDetail::ProbePrograms => builder.literal("-show_programs"),
        ProbeDetail::ProbeAll => {
            builder.literal("-show_format");
            builder.literal("-show_streams");
            builder.literal("-show_chapters");
            builder.literal("-show_programs");
        }
    }
    if options.count_frames {
        builder.literal("-count_frames");
    }
    if options.count_packets {
        builder.literal("-count_packets");
    }
    if let Some(intervals) = options.read_intervals {
        builder.option("-read_intervals", intervals);
    }
    let input = builder.input_blob(media, "bin");
    builder.argument(ToolArgument::input(input));
    Ok(builder.finish())
}

pub(crate) fn inspect_plan(
    media: Media,
    query: InspectionQuery,
) -> Result<ToolExecutionPlan, FfmpegError> {
    let mut builder = PlanBuilder::command(ToolProgram::new("ffprobe"));
    builder.option("-v", "error");
    builder.option("-print_format", "json");
    builder.literal(match query.kind {
        InspectionKind::InspectPackets => "-show_packets",
        InspectionKind::InspectFrames => "-show_frames",
    });
    if let Some(stream) = query.stream {
        builder.option("-select_streams", probe_stream_selector(&stream)?);
    }
    if let Some(intervals) = query.read_intervals {
        builder.option("-read_intervals", intervals);
    }
    if !query.entries.is_empty() {
        let section = match query.kind {
            InspectionKind::InspectPackets => "packet",
            InspectionKind::InspectFrames => "frame",
        };
        builder.option(
            "-show_entries",
            format!("{section}={}", query.entries.join(",")),
        );
    }
    let input = builder.input_blob(media, "bin");
    builder.argument(ToolArgument::input(input));
    Ok(builder.finish())
}

pub(crate) fn version_plan() -> ToolExecutionPlan {
    let mut builder = PlanBuilder::command(ToolProgram::new("ffmpeg"));
    builder.literal("-version");
    builder.finish()
}

pub(crate) fn capabilities_plan(domain: CapabilityDomain) -> ToolExecutionPlan {
    let mut builder = PlanBuilder::command(ToolProgram::new("ffmpeg"));
    builder.literal("-hide_banner");
    builder.literal(match domain {
        CapabilityDomain::Encoders => "-encoders",
        CapabilityDomain::Decoders => "-decoders",
        CapabilityDomain::Codecs => "-codecs",
        CapabilityDomain::Demuxers => "-demuxers",
        CapabilityDomain::Muxers => "-muxers",
        CapabilityDomain::Formats => "-formats",
        CapabilityDomain::Filters => "-filters",
        CapabilityDomain::Protocols => "-protocols",
        CapabilityDomain::Devices => "-devices",
        CapabilityDomain::PixelFormats => "-pix_fmts",
        CapabilityDomain::SampleFormats => "-sample_fmts",
        CapabilityDomain::ChannelLayouts => "-layouts",
        CapabilityDomain::HardwareAccelerators => "-hwaccels",
    });
    builder.finish()
}

fn compile_input(builder: &mut PlanBuilder, input: MediaInput) -> Result<(), FfmpegError> {
    for option in input.options {
        compile_input_option(builder, option)?;
    }
    match input.source {
        MediaSource::StoredMedia(media) => {
            let id = builder.input_blob(media, "bin");
            builder.literal("-i");
            builder.argument(ToolArgument::input(id));
        }
        MediaSource::StoredPackage(package) => {
            let manifest = match package.kind {
                PackageKind::HlsPackage => "/index.m3u8",
                PackageKind::DashPackage => "/manifest.mpd",
            };
            let id = builder.input_tree(package);
            builder.literal("-i");
            builder.argument(ToolArgument::input_decorated(id, "", manifest));
        }
        MediaSource::TestVideo(source) => {
            validate_rational(&source.frame_rate, "test video frame rate")?;
            let pattern = test_pattern_name(&source.pattern)?;
            let mut filter = format!(
                "{pattern}=size={}x{}:rate={}",
                source.size.width,
                source.size.height,
                rational(&source.frame_rate)?
            );
            if let Some(duration) = source.duration {
                filter.push_str(&format!(":duration={}", seconds(duration)));
            }
            builder.option("-f", "lavfi");
            builder.option("-i", filter);
        }
        MediaSource::SolidVideo(source) => {
            validate_rational(&source.frame_rate, "solid video frame rate")?;
            let mut filter = format!(
                "color=c={}:s={}x{}:r={}",
                quote_filter_value(&source.color.value),
                source.size.width,
                source.size.height,
                rational(&source.frame_rate)?
            );
            if let Some(duration) = source.duration {
                filter.push_str(&format!(":d={}", seconds(duration)));
            }
            builder.option("-f", "lavfi");
            builder.option("-i", filter);
        }
        MediaSource::SineAudio(source) => {
            let mut filter = format!(
                "sine=frequency={}:sample_rate={}",
                source.frequency, source.sample_rate
            );
            if let Some(duration) = source.duration {
                filter.push_str(&format!(":duration={}", seconds(duration)));
            }
            builder.option("-f", "lavfi");
            builder.option("-i", filter);
        }
        MediaSource::SilenceAudio(source) => {
            let mut filter = format!(
                "anullsrc=sample_rate={}:channel_layout={}",
                source.sample_rate,
                quote_filter_value(&source.channel_layout)
            );
            if let Some(duration) = source.duration {
                filter.push_str(&format!(":duration={}", seconds(duration)));
            }
            builder.option("-f", "lavfi");
            builder.option("-i", filter);
        }
    }
    Ok(())
}

fn compile_input_option(builder: &mut PlanBuilder, option: InputOption) -> Result<(), FfmpegError> {
    match option {
        InputOption::InputFormat(format) => {
            builder.option("-f", checked_headless_format(&format.name, "input format")?)
        }
        InputOption::InputSeek(time) => builder.option("-ss", seconds(time)),
        InputOption::InputDuration(time) => builder.option("-t", seconds(time)),
        InputOption::InputEndAt(time) => builder.option("-to", seconds(time)),
        InputOption::InputFrameRate(rate) => builder.option("-framerate", rational(&rate)?),
        InputOption::InputVideoSize(size) => {
            builder.option("-video_size", format!("{}x{}", size.width, size.height))
        }
        InputOption::InputPixelFormat(format) => builder.option("-pixel_format", format),
        InputOption::InputSampleRate(rate) => builder.option("-sample_rate", rate.to_string()),
        InputOption::InputChannels(channels) => builder.option("-channels", channels.to_string()),
        InputOption::InputStreamLoop(count) => builder.option("-stream_loop", count.to_string()),
        InputOption::InputReadAtNativeRate => builder.literal("-re"),
        InputOption::InputThreadQueueSize(size) => {
            builder.option("-thread_queue_size", size.to_string())
        }
        InputOption::InputDecoder(codec) => builder.option("-c", codec),
        InputOption::InputProtocolOption(option) | InputOption::InputDemuxerOption(option) => {
            builder.option(
                &format!("-{}", checked_name(&option.name, "input option")?),
                option.value,
            )
        }
    }
    Ok(())
}

fn compile_filter_graph(builder: &mut PlanBuilder, graph: FilterGraph) -> Result<(), FfmpegError> {
    if graph.chains.is_empty() {
        return Ok(());
    }
    let mut parts = Vec::new();
    for (chain_index, chain) in graph.chains.into_iter().enumerate() {
        if chain.filters.is_empty() {
            return Err(invalid(format!(
                "filter chain {chain_index} has no filters"
            )));
        }
        if chain_index != 0 {
            parts.push(ToolArgument::literal(";"));
        }
        for input in chain.inputs {
            parts.push(ToolArgument::literal(format!(
                "[{}]",
                filter_pad_name(&input)?
            )));
        }
        for (filter_index, filter) in chain.filters.into_iter().enumerate() {
            if filter_index != 0 {
                parts.push(ToolArgument::literal(","));
            }
            parts.extend(compile_media_filter(builder, filter)?);
        }
        for output in chain.outputs {
            parts.push(ToolArgument::literal(format!(
                "[{}]",
                checked_label(&output)?
            )));
        }
    }
    builder.literal("-filter_complex");
    builder.argument(ToolArgument::joined(parts));
    Ok(())
}

fn compile_media_filter(
    builder: &mut PlanBuilder,
    filter: MediaFilter,
) -> Result<Vec<ToolArgument>, FfmpegError> {
    match filter {
        MediaFilter::Video(filter) => compile_video_filter(builder, filter),
        MediaFilter::Audio(filter) => Ok(vec![ToolArgument::literal(audio_filter_text(&filter)?)]),
        MediaFilter::TrimVideo(range) => {
            Ok(vec![ToolArgument::literal(trim_filter("trim", range))])
        }
        MediaFilter::TrimAudio(range) => {
            Ok(vec![ToolArgument::literal(trim_filter("atrim", range))])
        }
        MediaFilter::ConcatFilter(inputs, video, audio) => Ok(vec![ToolArgument::literal(
            format!("concat=n={inputs}:v={video}:a={audio}"),
        )]),
        MediaFilter::SplitVideo(outputs) => {
            Ok(vec![ToolArgument::literal(format!("split={outputs}"))])
        }
        MediaFilter::SplitAudio(outputs) => {
            Ok(vec![ToolArgument::literal(format!("asplit={outputs}"))])
        }
        MediaFilter::MixAudio(inputs, normalize) => Ok(vec![ToolArgument::literal(format!(
            "amix=inputs={inputs}:normalize={}",
            u8::from(normalize)
        ))]),
        MediaFilter::MergeAudio(inputs) => Ok(vec![ToolArgument::literal(format!(
            "amerge=inputs={inputs}"
        ))]),
        MediaFilter::CrossFadeVideo(duration, offset) => Ok(vec![ToolArgument::literal(format!(
            "xfade=duration={}:offset={}",
            seconds(duration),
            seconds(offset)
        ))]),
        MediaFilter::CrossFadeAudio(duration) => Ok(vec![ToolArgument::literal(format!(
            "acrossfade=d={}",
            seconds(duration)
        ))]),
        MediaFilter::CustomFilter(name, options) => Ok(vec![ToolArgument::literal(
            custom_filter_text(&name, &options)?,
        )]),
    }
}

fn compile_video_filter(
    builder: &mut PlanBuilder,
    filter: VideoFilter,
) -> Result<Vec<ToolArgument>, FfmpegError> {
    match filter {
        VideoFilter::DrawText(draw) => {
            let mut parts = vec![ToolArgument::literal(format!(
                "drawtext=text={}:x={}:y={}:fontsize={}:fontcolor={}",
                quote_filter_value(&draw.text),
                quote_filter_value(&draw.x),
                quote_filter_value(&draw.y),
                draw.font_size,
                quote_filter_value(&draw.font_color.value)
            ))];
            if let Some(font) = draw.font_file {
                let input = builder.input_blob(font, "font");
                parts.push(ToolArgument::literal(":fontfile='"));
                parts.push(ToolArgument::input(input));
                parts.push(ToolArgument::literal("'"));
            }
            if let Some(family) = draw.font_family {
                parts.push(ToolArgument::literal(format!(
                    ":font={}",
                    quote_filter_value(&family)
                )));
            }
            if let Some(color) = draw.box_color {
                parts.push(ToolArgument::literal(format!(
                    ":box=1:boxcolor={}:boxborderw={}",
                    quote_filter_value(&color.value),
                    draw.box_border
                )));
            }
            Ok(parts)
        }
        VideoFilter::BurnSubtitles(subtitles) => {
            let subtitle = builder.input_blob(subtitles.subtitles, "sub");
            let mut parts = vec![
                ToolArgument::literal("subtitles=filename='"),
                ToolArgument::input(subtitle),
                ToolArgument::literal("'"),
            ];
            if let Some(index) = subtitles.stream_index {
                parts.push(ToolArgument::literal(format!(":si={index}")));
            }
            if !subtitles.fonts.is_empty() {
                let mut first = None;
                for font in subtitles.fonts {
                    let input = builder.input_blob(font, "font");
                    first.get_or_insert(input);
                }
                if let Some(first) = first {
                    parts.push(ToolArgument::literal(":fontsdir='"));
                    parts.push(ToolArgument::input_parent_decorated(first, "", ""));
                    parts.push(ToolArgument::literal("'"));
                }
            }
            if let Some(style) = subtitles.force_style {
                parts.push(ToolArgument::literal(format!(
                    ":force_style={}",
                    quote_filter_value(&style)
                )));
            }
            Ok(parts)
        }
        other => Ok(vec![ToolArgument::literal(video_filter_text(&other)?)]),
    }
}

fn compile_video_filter_chain(
    builder: &mut PlanBuilder,
    filters: Vec<VideoFilter>,
) -> Result<ToolArgument, FfmpegError> {
    let mut parts = Vec::new();
    for (index, filter) in filters.into_iter().enumerate() {
        if index != 0 {
            parts.push(ToolArgument::literal(","));
        }
        parts.extend(compile_video_filter(builder, filter)?);
    }
    Ok(ToolArgument::joined(parts))
}

fn compile_audio_filter_chain(filters: Vec<AudioFilter>) -> Result<ToolArgument, FfmpegError> {
    let text = filters
        .iter()
        .map(audio_filter_text)
        .collect::<Result<Vec<_>, _>>()?
        .join(",");
    Ok(ToolArgument::literal(text))
}

fn video_filter_text(filter: &VideoFilter) -> Result<String, FfmpegError> {
    let text = match filter {
        VideoFilter::Scale(scale) => {
            let width = if scale.prevent_upscale && scale.width > 0 {
                format!("min(iw\\,{})", scale.width)
            } else {
                scale.width.to_string()
            };
            let height = if scale.prevent_upscale && scale.height > 0 {
                format!("min(ih\\,{})", scale.height)
            } else {
                scale.height.to_string()
            };
            let mut value = format!("scale={width}:{height}");
            if let Some(algorithm) = &scale.algorithm {
                value.push_str(&format!(":flags={}", quote_filter_value(algorithm)));
            }
            if scale.preserve_aspect_ratio {
                value.push_str(":force_original_aspect_ratio=decrease");
            }
            value
        }
        VideoFilter::Crop(crop) => {
            format!("crop={}:{}:{}:{}", crop.width, crop.height, crop.x, crop.y)
        }
        VideoFilter::Pad(pad) => format!(
            "pad={}:{}:{}:{}:{}",
            pad.width,
            pad.height,
            pad.x,
            pad.y,
            quote_filter_value(&pad.color.value)
        ),
        VideoFilter::FrameRate(rate) => format!("fps={}", rational(rate)?),
        VideoFilter::PixelFormat(format) => format!("format={}", quote_filter_value(format)),
        VideoFilter::SetSampleAspectRatio(ratio) => format!("setsar={}", rational(ratio)?),
        VideoFilter::SetDisplayAspectRatio(ratio) => format!("setdar={}", rational(ratio)?),
        VideoFilter::Rotate(rotate) => {
            let mut value = format!("rotate={}", rotate.radians);
            if let Some(width) = rotate.output_width {
                value.push_str(&format!(":ow={width}"));
            }
            if let Some(height) = rotate.output_height {
                value.push_str(&format!(":oh={height}"));
            }
            value
        }
        VideoFilter::Transpose(direction) => format!(
            "transpose={}",
            match direction {
                TransposeDirection::Clockwise => "clock",
                TransposeDirection::CounterClockwise => "cclock",
                TransposeDirection::ClockwiseFlip => "clock_flip",
                TransposeDirection::CounterClockwiseFlip => "cclock_flip",
            }
        ),
        VideoFilter::HorizontalFlip => "hflip".to_string(),
        VideoFilter::VerticalFlip => "vflip".to_string(),
        VideoFilter::Deinterlace => "yadif".to_string(),
        VideoFilter::Denoise(strength) => format!("hqdn3d={strength}"),
        VideoFilter::BoxBlur(radius) => format!("boxblur={radius}"),
        VideoFilter::GaussianBlur(sigma) => format!("gblur=sigma={sigma}"),
        VideoFilter::Sharpen(amount) => format!("unsharp=5:5:{amount}"),
        VideoFilter::Equalizer(equalizer) => {
            let mut options = Vec::new();
            if let Some(value) = equalizer.brightness {
                options.push(format!("brightness={value}"));
            }
            if let Some(value) = equalizer.contrast {
                options.push(format!("contrast={value}"));
            }
            if let Some(value) = equalizer.saturation {
                options.push(format!("saturation={value}"));
            }
            if let Some(value) = equalizer.gamma {
                options.push(format!("gamma={value}"));
            }
            format!("eq={}", options.join(":"))
        }
        VideoFilter::Hue(hue, saturation) => format!("hue=h={hue}:s={saturation}"),
        VideoFilter::Gamma(gamma) => format!("eq=gamma={gamma}"),
        VideoFilter::ChromaKey(color, similarity, blend) => format!(
            "chromakey={}:{}:{}",
            quote_filter_value(&color.value),
            similarity,
            blend
        ),
        VideoFilter::VideoFade(fade) => format!(
            "fade=t={}:st={}:d={}:color={}",
            fade_direction(&fade.direction),
            seconds(fade.start),
            seconds(fade.duration),
            quote_filter_value(&fade.color.value)
        ),
        VideoFilter::Overlay(overlay) => format!(
            "overlay=x={}:y={}:shortest={}:repeatlast={}",
            quote_filter_value(&overlay.x),
            quote_filter_value(&overlay.y),
            u8::from(overlay.shortest),
            u8::from(overlay.repeat_last)
        ),
        VideoFilter::ThumbnailFrames(frames) => format!("thumbnail={frames}"),
        VideoFilter::SelectFrames(expression) => {
            format!("select={}", quote_filter_value(expression))
        }
        VideoFilter::SetPresentationTimestamps(expression) => {
            format!("setpts={}", quote_filter_value(expression))
        }
        VideoFilter::ReverseVideo => "reverse".to_string(),
        VideoFilter::LoopVideo(loop_count, size) => {
            format!("loop=loop={loop_count}:size={size}")
        }
        VideoFilter::CustomVideoFilter(name, options) => custom_filter_text(name, options)?,
        VideoFilter::DrawText(_) | VideoFilter::BurnSubtitles(_) => {
            return Err(invalid("path-based video filter requires plan compilation"));
        }
    };
    Ok(text)
}

fn audio_filter_text(filter: &AudioFilter) -> Result<String, FfmpegError> {
    Ok(match filter {
        AudioFilter::Volume(volume) => format!("volume={volume}"),
        AudioFilter::NormalizeLoudness(loudness) => format!(
            "loudnorm=I={}:LRA={}:TP={}",
            loudness.integrated_loudness, loudness.loudness_range, loudness.true_peak
        ),
        AudioFilter::Resample(rate) => format!("aresample={rate}"),
        AudioFilter::ChannelLayout(layout) => {
            format!("aformat=channel_layouts={}", quote_filter_value(layout))
        }
        AudioFilter::AudioFade(fade) => format!(
            "afade=t={}:st={}:d={}",
            fade_direction(&fade.direction),
            seconds(fade.start),
            seconds(fade.duration)
        ),
        AudioFilter::Delay(delays) => format!(
            "adelay={}",
            delays
                .iter()
                .map(|time| ((time.seconds * 1000.0).round() as i64).to_string())
                .collect::<Vec<_>>()
                .join("|")
        ),
        AudioFilter::Tempo(tempo) => format!("atempo={tempo}"),
        AudioFilter::Equalizer(frequency, width, gain) => {
            format!("equalizer=f={frequency}:width_type=h:width={width}:g={gain}")
        }
        AudioFilter::HighPass(frequency) => format!("highpass=f={frequency}"),
        AudioFilter::LowPass(frequency) => format!("lowpass=f={frequency}"),
        AudioFilter::Compressor(threshold, ratio, attack, release) => format!(
            "acompressor=threshold={threshold}:ratio={ratio}:attack={attack}:release={release}"
        ),
        AudioFilter::Limiter(limit) => format!("alimiter=limit={limit}"),
        AudioFilter::Gate(threshold, ratio) => {
            format!("agate=threshold={threshold}:ratio={ratio}")
        }
        AudioFilter::Echo(input_gain, output_gain, delay, decay) => format!(
            "aecho={input_gain}:{output_gain}:{}:{decay}",
            (delay.seconds * 1000.0).round()
        ),
        AudioFilter::RemoveSilence(threshold, duration) => format!(
            "silenceremove=start_periods=1:start_duration={}:start_threshold={}dB",
            seconds(*duration),
            threshold
        ),
        AudioFilter::ReverseAudio => "areverse".to_string(),
        AudioFilter::CustomAudioFilter(name, options) => custom_filter_text(name, options)?,
    })
}

fn compile_output(
    builder: &mut PlanBuilder,
    output: MediaOutput,
) -> Result<PlannedArtifact, FfmpegError> {
    if output.streams.is_empty() {
        return Err(invalid(
            "general media outputs require at least one explicit stream",
        ));
    }
    let mut counters = StreamCounters::default();
    for stream in output.streams {
        builder.option("-map", stream_source_name(&stream.source)?);
        let specifier = counters.next(&stream.encoding);
        compile_stream_encoding(builder, &stream.encoding, &specifier)?;
        for (name, value) in stream.metadata {
            builder.option(
                &format!("-metadata:s:{specifier}"),
                format!("{name}={value}"),
            );
        }
        if !stream.dispositions.is_empty() {
            let value = stream
                .dispositions
                .iter()
                .map(disposition_name)
                .collect::<Vec<_>>()
                .join("+");
            builder.option(&format!("-disposition:{specifier}"), value);
        }
    }
    for option in output.options {
        compile_mux_option(builder, option)?;
    }
    for (name, value) in output.metadata {
        builder.option("-metadata", format!("{name}={value}"));
    }
    compile_output_target(builder, output.format, output.mode)
}

fn compile_output_target(
    builder: &mut PlanBuilder,
    format: ContainerFormat,
    mode: OutputMode,
) -> Result<PlannedArtifact, FfmpegError> {
    let format_name = checked_headless_format(&format.name, "output format")?;
    let muxer_name = ffmpeg_muxer_name(&format_name);
    match mode {
        OutputMode::SingleFile => {
            builder.option("-f", muxer_name);
            let output = builder.output(OutputKind::Single, &format_name);
            builder.argument(ToolArgument::output(output));
            Ok(PlannedArtifact {
                output,
                kind: ArtifactKind::Single,
            })
        }
        OutputMode::NumberedFiles(start) => {
            builder.option("-f", muxer_name);
            builder.option("-start_number", start.to_string());
            let output = builder.output(OutputKind::Numbered, &format_name);
            builder.argument(ToolArgument::output(output));
            Ok(PlannedArtifact {
                output,
                kind: ArtifactKind::Sequence,
            })
        }
        OutputMode::SegmentedFiles(segment) => {
            builder.option("-f", "segment");
            builder.option("-segment_format", muxer_name);
            builder.option("-segment_time", seconds(segment.segment_duration));
            builder.option(
                "-reset_timestamps",
                u8::from(segment.reset_timestamps).to_string(),
            );
            builder.option("-segment_start_number", segment.start_number.to_string());
            let output = builder.output(OutputKind::Numbered, &format_name);
            builder.argument(ToolArgument::output(output));
            Ok(PlannedArtifact {
                output,
                kind: ArtifactKind::Sequence,
            })
        }
        OutputMode::HlsStreaming(hls) => {
            let segment_format = checked_name(&hls.segment_format, "HLS segment format")?;
            builder.option("-f", "hls");
            builder.option("-hls_time", seconds(hls.segment_duration));
            builder.option("-hls_list_size", hls.playlist_size.to_string());
            builder.option("-hls_segment_type", &segment_format);
            if !hls.flags.is_empty() {
                builder.option("-hls_flags", hls.flags.join("+"));
            }
            if hls.master_playlist {
                builder.option("-master_pl_name", "master.m3u8");
            }
            let output = builder.output(OutputKind::Tree, "hls");
            builder.literal("-hls_segment_filename");
            builder.argument(ToolArgument::output_with_suffix(
                output,
                format!("/segment-%06d.{segment_format}"),
            ));
            builder.argument(ToolArgument::output_with_suffix(output, "/index.m3u8"));
            Ok(PlannedArtifact {
                output,
                kind: ArtifactKind::Package(PackageKind::HlsPackage),
            })
        }
        OutputMode::DashStreaming(dash) => {
            builder.option("-f", "dash");
            builder.option("-seg_duration", seconds(dash.segment_duration));
            builder.option("-window_size", dash.window_size.to_string());
            builder.option("-extra_window_size", dash.extra_window_size.to_string());
            builder.option("-use_template", u8::from(dash.use_template).to_string());
            builder.option("-use_timeline", u8::from(dash.use_timeline).to_string());
            let output = builder.output(OutputKind::Tree, "dash");
            builder.argument(ToolArgument::output_with_suffix(output, "/manifest.mpd"));
            Ok(PlannedArtifact {
                output,
                kind: ArtifactKind::Package(PackageKind::DashPackage),
            })
        }
    }
}

fn ffmpeg_muxer_name(format_name: &str) -> &str {
    match format_name {
        "m4a" => "ipod",
        other => other,
    }
}

fn compile_image_output_target(
    builder: &mut PlanBuilder,
    format: ContainerFormat,
    numbered: bool,
) -> Result<PlannedArtifact, FfmpegError> {
    let extension = checked_headless_format(&format.name, "image format")?;
    builder.option("-f", "image2");
    if numbered {
        builder.option("-start_number", "1");
    }
    let output = builder.output(
        if numbered {
            OutputKind::Numbered
        } else {
            OutputKind::Single
        },
        &extension,
    );
    builder.argument(ToolArgument::output(output));
    Ok(PlannedArtifact {
        output,
        kind: if numbered {
            ArtifactKind::Sequence
        } else {
            ArtifactKind::Single
        },
    })
}

fn compile_simple_encoding(
    builder: &mut PlanBuilder,
    encoding: Encoding,
) -> Result<(), FfmpegError> {
    if let Some(video) = &encoding.video {
        compile_video_encoding(builder, video, "v:0")?;
    }
    if let Some(audio) = &encoding.audio {
        compile_audio_encoding(builder, audio, "a:0")?;
    }
    if let Some(subtitle) = &encoding.subtitle {
        compile_subtitle_encoding(builder, subtitle, "s:0")?;
    }
    for option in encoding.options {
        compile_mux_option(builder, option)?;
    }
    for (name, value) in encoding.metadata {
        builder.option("-metadata", format!("{name}={value}"));
    }
    compile_output_target(builder, encoding.format, OutputMode::SingleFile)?;
    Ok(())
}

enum StreamEncodingRef<'a> {
    Video(&'a VideoEncoding),
    Audio(&'a AudioEncoding),
    Subtitle(&'a SubtitleEncoding),
}

fn compile_encoding_ref(
    builder: &mut PlanBuilder,
    encoding: StreamEncodingRef<'_>,
    specifier: &str,
) -> Result<(), FfmpegError> {
    match encoding {
        StreamEncodingRef::Video(value) => compile_video_encoding(builder, value, specifier),
        StreamEncodingRef::Audio(value) => compile_audio_encoding(builder, value, specifier),
        StreamEncodingRef::Subtitle(value) => compile_subtitle_encoding(builder, value, specifier),
    }
}

fn compile_stream_encoding(
    builder: &mut PlanBuilder,
    encoding: &StreamEncoding,
    specifier: &str,
) -> Result<(), FfmpegError> {
    match encoding {
        StreamEncoding::CopyStream(_) => {
            builder.option(&format!("-c:{specifier}"), "copy");
            Ok(())
        }
        StreamEncoding::EncodeVideo(value) => compile_video_encoding(builder, value, specifier),
        StreamEncoding::EncodeAudio(value) => compile_audio_encoding(builder, value, specifier),
        StreamEncoding::EncodeSubtitle(value) => {
            compile_subtitle_encoding(builder, value, specifier)
        }
        StreamEncoding::EncodeData(codec, options) => {
            builder.option(&format!("-c:{specifier}"), codec);
            for option in options {
                builder.option(
                    &format!(
                        "-{}:{specifier}",
                        checked_name(&option.name, "data encoder option")?
                    ),
                    &option.value,
                );
            }
            Ok(())
        }
    }
}

fn compile_video_encoding(
    builder: &mut PlanBuilder,
    encoding: &VideoEncoding,
    specifier: &str,
) -> Result<(), FfmpegError> {
    builder.option(
        &format!("-c:{specifier}"),
        video_codec_name(&encoding.codec)?,
    );
    for option in &encoding.options {
        match option {
            VideoEncodeOption::VideoBitRate(value) => {
                builder.option(&format!("-b:{specifier}"), value.to_string())
            }
            VideoEncodeOption::ConstantRateFactor(value) => {
                builder.option(&format!("-crf:{specifier}"), value.to_string())
            }
            VideoEncodeOption::VideoQuality(value) => {
                builder.option(&format!("-q:{specifier}"), value.to_string())
            }
            VideoEncodeOption::Preset(value) => {
                builder.option(&format!("-preset:{specifier}"), value)
            }
            VideoEncodeOption::Tune(value) => builder.option(&format!("-tune:{specifier}"), value),
            VideoEncodeOption::Profile(value) => {
                builder.option(&format!("-profile:{specifier}"), value)
            }
            VideoEncodeOption::Level(value) => {
                builder.option(&format!("-level:{specifier}"), value)
            }
            VideoEncodeOption::PixelFormat(value) => {
                builder.option(&format!("-pix_fmt:{specifier}"), value)
            }
            VideoEncodeOption::VideoFrameRate(value) => {
                builder.option(&format!("-r:{specifier}"), rational(value)?)
            }
            VideoEncodeOption::GroupOfPictures(value) => {
                builder.option(&format!("-g:{specifier}"), value.to_string())
            }
            VideoEncodeOption::BFrames(value) => {
                builder.option(&format!("-bf:{specifier}"), value.to_string())
            }
            VideoEncodeOption::MaximumBitRate(value) => {
                builder.option(&format!("-maxrate:{specifier}"), value.to_string())
            }
            VideoEncodeOption::BufferSize(value) => {
                builder.option(&format!("-bufsize:{specifier}"), value.to_string())
            }
            VideoEncodeOption::EncoderThreads(value) => {
                builder.option(&format!("-threads:{specifier}"), value.to_string())
            }
            VideoEncodeOption::VideoBitstreamFilter(name, options) => builder.option(
                &format!("-bsf:{specifier}"),
                custom_filter_text(name, options)?,
            ),
            VideoEncodeOption::VideoEncoderOption(option) => builder.option(
                &format!(
                    "-{}:{specifier}",
                    checked_name(&option.name, "video encoder option")?
                ),
                &option.value,
            ),
        }
    }
    Ok(())
}

fn compile_audio_encoding(
    builder: &mut PlanBuilder,
    encoding: &AudioEncoding,
    specifier: &str,
) -> Result<(), FfmpegError> {
    builder.option(
        &format!("-c:{specifier}"),
        audio_codec_name(&encoding.codec)?,
    );
    for option in &encoding.options {
        match option {
            AudioEncodeOption::AudioBitRate(value) => {
                builder.option(&format!("-b:{specifier}"), value.to_string())
            }
            AudioEncodeOption::AudioQuality(value) => {
                builder.option(&format!("-q:{specifier}"), value.to_string())
            }
            AudioEncodeOption::AudioSampleRate(value) => {
                builder.option(&format!("-ar:{specifier}"), value.to_string())
            }
            AudioEncodeOption::AudioChannels(value) => {
                builder.option(&format!("-ac:{specifier}"), value.to_string())
            }
            AudioEncodeOption::AudioChannelLayout(value) => {
                builder.option(&format!("-channel_layout:{specifier}"), value)
            }
            AudioEncodeOption::AudioCompressionLevel(value) => builder.option(
                &format!("-compression_level:{specifier}"),
                value.to_string(),
            ),
            AudioEncodeOption::AudioCutoff(value) => {
                builder.option(&format!("-cutoff:{specifier}"), value.to_string())
            }
            AudioEncodeOption::AudioBitstreamFilter(name, options) => builder.option(
                &format!("-bsf:{specifier}"),
                custom_filter_text(name, options)?,
            ),
            AudioEncodeOption::AudioEncoderOption(option) => builder.option(
                &format!(
                    "-{}:{specifier}",
                    checked_name(&option.name, "audio encoder option")?
                ),
                &option.value,
            ),
        }
    }
    Ok(())
}

fn compile_subtitle_encoding(
    builder: &mut PlanBuilder,
    encoding: &SubtitleEncoding,
    specifier: &str,
) -> Result<(), FfmpegError> {
    builder.option(
        &format!("-c:{specifier}"),
        subtitle_codec_name(&encoding.codec)?,
    );
    for option in &encoding.options {
        builder.option(
            &format!(
                "-{}:{specifier}",
                checked_name(&option.name, "subtitle encoder option")?
            ),
            &option.value,
        );
    }
    Ok(())
}

fn compile_mux_option(builder: &mut PlanBuilder, option: MuxOption) -> Result<(), FfmpegError> {
    match option {
        MuxOption::ShortestOutput => builder.literal("-shortest"),
        MuxOption::OutputDuration(time) => builder.option("-t", seconds(time)),
        MuxOption::OutputStartAt(time) => builder.option("-ss", seconds(time)),
        MuxOption::CopyInputTimestamps => builder.literal("-copyts"),
        MuxOption::AvoidNegativeTimestamps(mode) => builder.option("-avoid_negative_ts", mode),
        MuxOption::MaximumMuxingQueueSize(size) => {
            builder.option("-max_muxing_queue_size", size.to_string())
        }
        MuxOption::MovFlags(flags) => builder.option("-movflags", flags.join("+")),
        MuxOption::MapMetadataFrom(input) => builder.option(
            "-map_metadata",
            input.map_or_else(|| "-1".to_string(), |value| value.to_string()),
        ),
        MuxOption::MapChaptersFrom(input) => builder.option(
            "-map_chapters",
            input.map_or_else(|| "-1".to_string(), |value| value.to_string()),
        ),
        MuxOption::MuxerOption(option) => builder.option(
            &format!("-{}", checked_name(&option.name, "muxer option")?),
            option.value,
        ),
    }
    Ok(())
}

fn compile_time_range(builder: &mut PlanBuilder, range: &TimeRange) {
    if let Some(start) = range.start {
        builder.option("-ss", seconds(start));
    }
    if let Some(duration) = range.duration {
        builder.option("-t", seconds(duration));
    }
}

fn trim_filter(name: &str, range: TimeRange) -> String {
    let mut options = Vec::new();
    if let Some(start) = range.start {
        options.push(format!("start={}", seconds(start)));
    }
    if let Some(duration) = range.duration {
        options.push(format!("duration={}", seconds(duration)));
    }
    format!("{name}={}", options.join(":"))
}

#[derive(Default)]
struct StreamCounters {
    video: usize,
    audio: usize,
    subtitle: usize,
    data: usize,
}

impl StreamCounters {
    fn next(&mut self, encoding: &StreamEncoding) -> String {
        match encoding {
            StreamEncoding::CopyStream(kind) => match kind {
                MediaKind::VideoStream => next_specifier("v", &mut self.video),
                MediaKind::AudioStream => next_specifier("a", &mut self.audio),
                MediaKind::SubtitleStream => next_specifier("s", &mut self.subtitle),
                _ => next_specifier("d", &mut self.data),
            },
            StreamEncoding::EncodeVideo(_) => next_specifier("v", &mut self.video),
            StreamEncoding::EncodeAudio(_) => next_specifier("a", &mut self.audio),
            StreamEncoding::EncodeSubtitle(_) => next_specifier("s", &mut self.subtitle),
            StreamEncoding::EncodeData(_, _) => next_specifier("d", &mut self.data),
        }
    }
}

fn next_specifier(prefix: &str, counter: &mut usize) -> String {
    let value = format!("{prefix}:{}", *counter);
    *counter += 1;
    value
}

fn stream_source_name(source: &StreamSource) -> Result<String, FfmpegError> {
    match source {
        StreamSource::InputStream(stream) => stream_selector(stream),
        StreamSource::FilterOutput(label) => Ok(format!("[{}]", checked_label(label)?)),
    }
}

fn filter_pad_name(pad: &FilterPad) -> Result<String, FfmpegError> {
    match pad {
        FilterPad::InputPad(stream) => stream_selector(stream),
        FilterPad::LinkPad(label) => checked_label(label),
    }
}

fn stream_selector(stream: &StreamRef) -> Result<String, FfmpegError> {
    let mut value = format!("{}:{}", stream.input, stream_kind_letter(&stream.kind)?);
    if let Some(index) = stream.index {
        value.push_str(&format!(":{index}"));
    }
    Ok(value)
}

fn probe_stream_selector(stream: &StreamRef) -> Result<String, FfmpegError> {
    let mut value = stream_kind_letter(&stream.kind)?.to_string();
    if let Some(index) = stream.index {
        value.push_str(&format!(":{index}"));
    }
    Ok(value)
}

fn stream_kind_letter(kind: &MediaKind) -> Result<&'static str, FfmpegError> {
    match kind {
        MediaKind::VideoStream => Ok("v"),
        MediaKind::AudioStream => Ok("a"),
        MediaKind::SubtitleStream => Ok("s"),
        MediaKind::DataStream => Ok("d"),
        MediaKind::AttachmentStream => Ok("t"),
        MediaKind::UnknownStream(_) => Err(invalid(
            "unknown stream kinds cannot be used as stream selectors",
        )),
    }
}

fn video_codec_name(codec: &VideoCodec) -> Result<&str, FfmpegError> {
    match codec {
        VideoCodec::H264 => Ok("libx264"),
        VideoCodec::H265 => Ok("libx265"),
        VideoCodec::Av1 => Ok("libsvtav1"),
        VideoCodec::Vp8 => Ok("libvpx"),
        VideoCodec::Vp9 => Ok("libvpx-vp9"),
        VideoCodec::ProRes => Ok("prores_ks"),
        VideoCodec::DnxHd => Ok("dnxhd"),
        VideoCodec::Mpeg2Video => Ok("mpeg2video"),
        VideoCodec::Mpeg4Video => Ok("mpeg4"),
        VideoCodec::Theora => Ok("libtheora"),
        VideoCodec::GifVideo => Ok("gif"),
        VideoCodec::PngVideo => Ok("png"),
        VideoCodec::MjpegVideo => Ok("mjpeg"),
        VideoCodec::RawVideo => Ok("rawvideo"),
        VideoCodec::OtherVideoCodec(name) => checked_name_ref(name, "video codec"),
    }
}

fn audio_codec_name(codec: &AudioCodec) -> Result<&str, FfmpegError> {
    match codec {
        AudioCodec::Aac => Ok("aac"),
        AudioCodec::Opus => Ok("libopus"),
        AudioCodec::Vorbis => Ok("libvorbis"),
        AudioCodec::Mp3 => Ok("libmp3lame"),
        AudioCodec::Flac => Ok("flac"),
        AudioCodec::Alac => Ok("alac"),
        AudioCodec::Ac3 => Ok("ac3"),
        AudioCodec::Eac3 => Ok("eac3"),
        AudioCodec::PcmS16Le => Ok("pcm_s16le"),
        AudioCodec::PcmS24Le => Ok("pcm_s24le"),
        AudioCodec::PcmF32Le => Ok("pcm_f32le"),
        AudioCodec::OtherAudioCodec(name) => checked_name_ref(name, "audio codec"),
    }
}

fn subtitle_codec_name(codec: &SubtitleCodec) -> Result<&str, FfmpegError> {
    match codec {
        SubtitleCodec::WebVtt => Ok("webvtt"),
        SubtitleCodec::SubRip => Ok("subrip"),
        SubtitleCodec::Ass => Ok("ass"),
        SubtitleCodec::MovText => Ok("mov_text"),
        SubtitleCodec::DvdSubtitle => Ok("dvdsub"),
        SubtitleCodec::CopySubtitle => Ok("copy"),
        SubtitleCodec::OtherSubtitleCodec(name) => checked_name_ref(name, "subtitle codec"),
    }
}

fn test_pattern_name(pattern: &TestPattern) -> Result<&str, FfmpegError> {
    match pattern {
        TestPattern::TestSource => Ok("testsrc"),
        TestPattern::TestSource2 => Ok("testsrc2"),
        TestPattern::RgbTestSource => Ok("rgbtestsrc"),
        TestPattern::ColorBars => Ok("smptebars"),
        TestPattern::ColorBarsHd => Ok("smptehdbars"),
        TestPattern::ZonePlate => Ok("zoneplate"),
        TestPattern::OtherTestPattern(name) => checked_name_ref(name, "test pattern"),
    }
}

fn custom_filter_text(name: &str, options: &[FilterOption]) -> Result<String, FfmpegError> {
    let name = checked_name(name, "filter")?;
    if options.is_empty() {
        return Ok(name);
    }
    let options = options
        .iter()
        .map(|option| match &option.name {
            Some(option_name) => Ok(format!(
                "{}={}",
                checked_name(option_name, "filter option")?,
                quote_filter_value(&option.value)
            )),
            None => Ok(quote_filter_value(&option.value)),
        })
        .collect::<Result<Vec<_>, FfmpegError>>()?;
    Ok(format!("{name}={}", options.join(":")))
}

fn disposition_name(disposition: &StreamDisposition) -> &str {
    match disposition {
        StreamDisposition::DefaultDisposition => "default",
        StreamDisposition::DubDisposition => "dub",
        StreamDisposition::OriginalDisposition => "original",
        StreamDisposition::CommentaryDisposition => "comment",
        StreamDisposition::LyricsDisposition => "lyrics",
        StreamDisposition::KaraokeDisposition => "karaoke",
        StreamDisposition::ForcedDisposition => "forced",
        StreamDisposition::HearingImpairedDisposition => "hearing_impaired",
        StreamDisposition::VisualImpairedDisposition => "visual_impaired",
        StreamDisposition::CleanEffectsDisposition => "clean_effects",
        StreamDisposition::AttachedPictureDisposition => "attached_pic",
        StreamDisposition::TimedThumbnailsDisposition => "timed_thumbnails",
        StreamDisposition::OtherDisposition(value) => value,
    }
}

fn fade_direction(direction: &FadeDirection) -> &str {
    match direction {
        FadeDirection::FadeIn => "in",
        FadeDirection::FadeOut => "out",
    }
}

fn rational(value: &Rational) -> Result<String, FfmpegError> {
    validate_rational(value, "rational")?;
    Ok(format!("{}/{}", value.numerator, value.denominator))
}

fn validate_rational(value: &Rational, context: &str) -> Result<(), FfmpegError> {
    if value.denominator == 0 {
        Err(invalid(format!("{context} denominator cannot be zero")))
    } else {
        Ok(())
    }
}

fn seconds(time: Time) -> String {
    time.seconds.to_string()
}

fn quote_filter_value(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
}

fn checked_label(value: &str) -> Result<String, FfmpegError> {
    if value.is_empty()
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
    {
        Err(invalid(format!("invalid filter label `{value}`")))
    } else {
        Ok(value.to_string())
    }
}

fn checked_name(value: &str, context: &str) -> Result<String, FfmpegError> {
    checked_name_ref(value, context).map(str::to_string)
}

const NON_HEADLESS_FORMATS: &[&str] = &[
    "alsa",
    "android_camera",
    "audiotoolbox",
    "avfoundation",
    "bktr",
    "caca",
    "decklink",
    "dshow",
    "fbdev",
    "gdigrab",
    "iec61883",
    "jack",
    "kmsgrab",
    "libcdio",
    "libdc1394",
    "openal",
    "opengl",
    "oss",
    "pulse",
    "sdl",
    "sdl2",
    "sndio",
    "v4l2",
    "video4linux2",
    "vfwcap",
    "x11grab",
    "xcbgrab",
    "xv",
];

fn checked_headless_format(value: &str, context: &str) -> Result<String, FfmpegError> {
    let value = checked_name(value, context)?;
    if NON_HEADLESS_FORMATS.contains(&value.to_ascii_lowercase().as_str()) {
        return Err(invalid(format!(
            "{context} `{value}` is not available in headless workflows"
        )));
    }
    Ok(value)
}

fn checked_name_ref<'a>(value: &'a str, context: &str) -> Result<&'a str, FfmpegError> {
    require_name(value, context)?;
    Ok(value)
}

fn require_name(value: &str, context: &str) -> Result<(), FfmpegError> {
    if value.is_empty()
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.' | ',')
        })
    {
        Err(invalid(format!("invalid {context} `{value}`")))
    } else {
        Ok(())
    }
}

pub(crate) fn invalid(message: impl Into<String>) -> FfmpegError {
    FfmpegError {
        kind: FfmpegErrorKind::InvalidRequest,
        exit_code: None,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use blake3::hash;

    #[test]
    fn transcode_plan_keeps_media_paths_symbolic() {
        let plan = transcode_plan(
            MediaSource::StoredMedia(Media {
                content: hash(b"video"),
            }),
            vec![MediaOperation::VideoOperation(VideoFilter::Scale(
                ScaleFilter {
                    width: 1280,
                    height: 720,
                    algorithm: Some("lanczos".to_string()),
                    preserve_aspect_ratio: true,
                    prevent_upscale: true,
                },
            ))],
            Encoding {
                format: ContainerFormat {
                    name: "mp4".to_string(),
                },
                video: Some(VideoEncoding {
                    codec: VideoCodec::H264,
                    options: vec![VideoEncodeOption::ConstantRateFactor(23.0)],
                }),
                audio: None,
                subtitle: None,
                options: Vec::new(),
                metadata: Default::default(),
            },
        )
        .unwrap();
        assert_eq!(plan.program, ToolProgram::new("ffmpeg"));
        assert_eq!(plan.inputs.len(), 1);
        assert_eq!(plan.outputs.len(), 1);
        assert!(
            plan.arguments
                .iter()
                .any(|argument| { matches!(argument, ToolArgument::Path { .. }) })
        );
        assert!(!plan.arguments.iter().any(|argument| {
            matches!(argument, ToolArgument::Literal(value) if value.contains("/tmp"))
        }));
    }

    #[test]
    fn hls_is_declared_as_a_tree_output() {
        let (_, artifact) = simple_plan(
            MediaSource::TestVideo(TestVideoSource {
                pattern: TestPattern::TestSource,
                size: VideoSize {
                    width: 320,
                    height: 180,
                },
                frame_rate: Rational {
                    numerator: 30,
                    denominator: 1,
                },
                duration: Some(Time { seconds: 1.0 }),
            }),
            Vec::new(),
            Encoding {
                format: ContainerFormat {
                    name: "mp4".to_string(),
                },
                video: Some(VideoEncoding {
                    codec: VideoCodec::H264,
                    options: Vec::new(),
                }),
                audio: None,
                subtitle: None,
                options: Vec::new(),
                metadata: Default::default(),
            },
            OutputMode::HlsStreaming(HlsOutput {
                segment_duration: Time { seconds: 2.0 },
                playlist_size: 0,
                segment_format: "mpegts".to_string(),
                flags: Vec::new(),
                master_playlist: false,
            }),
        )
        .unwrap();
        assert_eq!(
            artifact.kind,
            ArtifactKind::Package(PackageKind::HlsPackage)
        );
    }

    #[test]
    fn m4a_uses_the_ipod_muxer_and_keeps_its_extension() {
        let plan = transcode_plan(
            MediaSource::SineAudio(SineAudioSource {
                frequency: 440.0,
                sample_rate: 48_000,
                duration: Some(Time { seconds: 1.0 }),
            }),
            Vec::new(),
            Encoding {
                format: ContainerFormat {
                    name: "m4a".to_string(),
                },
                video: None,
                audio: Some(AudioEncoding {
                    codec: AudioCodec::Aac,
                    options: Vec::new(),
                }),
                subtitle: None,
                options: Vec::new(),
                metadata: Default::default(),
            },
        )
        .unwrap();
        assert!(plan.arguments.windows(2).any(|arguments| {
            arguments == [ToolArgument::literal("-f"), ToolArgument::literal("ipod")]
        }));
        assert_eq!(plan.outputs[0].extension, "m4a");
    }

    #[test]
    fn segmented_output_maps_encoded_streams_explicitly() {
        let plan = segment_plan(
            MediaSource::StoredMedia(Media {
                content: hash(b"media"),
            }),
            Vec::new(),
            Encoding {
                format: ContainerFormat {
                    name: "matroska".to_string(),
                },
                video: Some(VideoEncoding {
                    codec: VideoCodec::H264,
                    options: Vec::new(),
                }),
                audio: Some(AudioEncoding {
                    codec: AudioCodec::Aac,
                    options: Vec::new(),
                }),
                subtitle: None,
                options: Vec::new(),
                metadata: Default::default(),
            },
            SegmentOutput {
                segment_duration: Time { seconds: 10.0 },
                reset_timestamps: true,
                start_number: 0,
            },
        )
        .unwrap();
        assert!(plan.arguments.windows(2).any(|arguments| {
            arguments
                == [
                    ToolArgument::literal("-map"),
                    ToolArgument::literal("0:v:0?"),
                ]
        }));
        assert!(plan.arguments.windows(2).any(|arguments| {
            arguments
                == [
                    ToolArgument::literal("-map"),
                    ToolArgument::literal("0:a:0?"),
                ]
        }));
    }

    #[test]
    fn device_formats_are_rejected() {
        for name in NON_HEADLESS_FORMATS {
            let error =
                checked_headless_format(&name.to_ascii_uppercase(), "input format").unwrap_err();
            assert_eq!(error.kind, FfmpegErrorKind::InvalidRequest);
            assert!(error.message.contains("headless workflows"));
        }
    }
}

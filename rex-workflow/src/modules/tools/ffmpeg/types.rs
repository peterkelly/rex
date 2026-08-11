use blake3::Hash;
use rex::Rex;
use std::collections::BTreeMap;

/// One encoded media file stored as a content-addressed blob.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct Media {
    pub content: Hash,
}

/// A manifest and its referenced media segments stored together as a content-addressed tree.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct MediaPackage {
    pub content: Hash,
    pub kind: PackageKind,
}

/// One output produced by a general `MediaProgram`.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum MediaArtifact {
    EncodedMedia(Media),
    MediaSequence(Vec<Media>),
    PackagedMedia(MediaPackage),
}

/// The streaming-package layout stored by a `MediaPackage`.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum PackageKind {
    HlsPackage,
    DashPackage,
}

/// Video dimensions in pixels.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Rex)]
pub struct VideoSize {
    pub width: u64,
    pub height: u64,
}

/// An exact rational number, commonly used for frame rates and aspect ratios.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Rex)]
pub struct Rational {
    pub numerator: i64,
    pub denominator: i64,
}

/// A media timestamp or duration measured in seconds.
#[derive(Clone, Copy, Debug, PartialEq, Rex)]
pub struct Time {
    pub seconds: f64,
}

/// A trim interval with optional start time and duration, both measured in seconds.
#[derive(Clone, Copy, Debug, PartialEq, Rex)]
pub struct TimeRange {
    pub start: Option<Time>,
    pub duration: Option<Time>,
}

/// An FFmpeg color expression such as a name, hex value, or supported color syntax.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct Color {
    pub value: String,
}

/// An FFmpeg container, muxer, demuxer, or image format name without a leading dot.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct ContainerFormat {
    pub name: String,
}

/// One named FFmpeg option and its textual value.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct NameValue {
    pub name: String,
    pub value: String,
}

/// The media type of a stream.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum MediaKind {
    VideoStream,
    AudioStream,
    SubtitleStream,
    DataStream,
    AttachmentStream,
    UnknownStream(String),
}

/// A zero-based stream reference within one zero-based program input.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct StreamRef {
    pub input: u64,
    pub kind: MediaKind,
    pub index: Option<u64>,
}

/// A stream read directly from an input or from a named filter-graph output pad.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum StreamSource {
    InputStream(StreamRef),
    FilterOutput(String),
}

/// A generated video test pattern; custom names are accepted through `OtherTestPattern`.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum TestPattern {
    TestSource,
    TestSource2,
    RgbTestSource,
    ColorBars,
    ColorBarsHd,
    ZonePlate,
    OtherTestPattern(String),
}

/// Parameters for a generated FFmpeg test-video source.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct TestVideoSource {
    pub pattern: TestPattern,
    pub size: VideoSize,
    pub frame_rate: Rational,
    pub duration: Option<Time>,
}

/// Parameters for a generated constant-color video source.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct SolidVideoSource {
    pub color: Color,
    pub size: VideoSize,
    pub frame_rate: Rational,
    pub duration: Option<Time>,
}

/// Parameters for a generated sine-wave audio source.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct SineAudioSource {
    pub frequency: f64,
    pub sample_rate: u64,
    pub duration: Option<Time>,
}

/// Parameters for a generated silent audio source.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct SilenceAudioSource {
    pub sample_rate: u64,
    pub channel_layout: String,
    pub duration: Option<Time>,
}

/// A stored or generated media source.
#[derive(Clone, Debug, PartialEq, Rex)]
pub enum MediaSource {
    StoredMedia(Media),
    StoredPackage(MediaPackage),
    TestVideo(TestVideoSource),
    SolidVideo(SolidVideoSource),
    SineAudio(SineAudioSource),
    SilenceAudio(SilenceAudioSource),
}

/// An input-scoped FFmpeg option applied before its corresponding media source.
#[derive(Clone, Debug, PartialEq, Rex)]
pub enum InputOption {
    InputFormat(ContainerFormat),
    InputSeek(Time),
    InputDuration(Time),
    InputEndAt(Time),
    InputFrameRate(Rational),
    InputVideoSize(VideoSize),
    InputPixelFormat(String),
    InputSampleRate(u64),
    InputChannels(u64),
    InputStreamLoop(i64),
    InputReadAtNativeRate,
    InputThreadQueueSize(u64),
    InputDecoder(String),
    InputProtocolOption(NameValue),
    InputDemuxerOption(NameValue),
}

/// One input to a general `MediaProgram`, including options that must precede it.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct MediaInput {
    pub source: MediaSource,
    pub options: Vec<InputOption>,
}

/// An input pad for a filter chain, sourced from an input stream or a prior named link.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum FilterPad {
    InputPad(StreamRef),
    LinkPad(String),
}

/// One filter option; `None` names a positional value and `Some` names a `key=value` option.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct FilterOption {
    pub name: Option<String>,
    pub value: String,
}

/// Video scaling parameters; negative dimensions retain FFmpeg's calculated-dimension semantics.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct ScaleFilter {
    pub width: i64,
    pub height: i64,
    pub algorithm: Option<String>,
    pub preserve_aspect_ratio: bool,
    pub prevent_upscale: bool,
}

/// A video crop rectangle in pixels with a signed top-left offset.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Rex)]
pub struct CropFilter {
    pub width: u64,
    pub height: u64,
    pub x: i64,
    pub y: i64,
}

/// A padded output canvas in pixels, including placement offsets and fill color.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct PadFilter {
    pub width: u64,
    pub height: u64,
    pub x: i64,
    pub y: i64,
    pub color: Color,
}

/// A clockwise rotation in radians with optional explicit output dimensions.
#[derive(Clone, Copy, Debug, PartialEq, Rex)]
pub struct RotateFilter {
    pub radians: f64,
    pub output_width: Option<u64>,
    pub output_height: Option<u64>,
}

/// A 90-degree transpose direction, optionally combined with a flip.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum TransposeDirection {
    Clockwise,
    CounterClockwise,
    ClockwiseFlip,
    CounterClockwiseFlip,
}

/// A fade beginning at `start` and lasting `duration`, both measured in seconds.
#[derive(Clone, Copy, Debug, PartialEq, Rex)]
pub struct FadeFilter {
    pub start: Time,
    pub duration: Time,
}

/// Placement and end-of-stream behavior for a video overlay filter.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct OverlayFilter {
    pub x: String,
    pub y: String,
    pub shortest: bool,
    pub repeat_last: bool,
}

/// Text-overlay settings using FFmpeg expressions for coordinates and CAS-backed font files.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct DrawTextFilter {
    pub text: String,
    pub x: String,
    pub y: String,
    pub font_file: Option<Media>,
    pub font_family: Option<String>,
    pub font_size: f64,
    pub font_color: Color,
    pub box_color: Option<Color>,
    pub box_border: u64,
}

/// Subtitle-rendering settings, including optional attachment fonts stored in the CAS.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct SubtitleFilter {
    pub subtitles: Media,
    pub stream_index: Option<u64>,
    pub fonts: Vec<Media>,
    pub force_style: Option<String>,
}

/// Optional video equalizer adjustments; omitted fields retain FFmpeg defaults.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct VideoEqualizer {
    pub brightness: Option<f64>,
    pub contrast: Option<f64>,
    pub saturation: Option<f64>,
    pub gamma: Option<f64>,
}

/// A colored video fade with direction, start time, and duration.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct VideoFade {
    pub direction: FadeDirection,
    pub start: Time,
    pub duration: Time,
    pub color: Color,
}

/// Whether a fade transitions into or out of the signal.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum FadeDirection {
    FadeIn,
    FadeOut,
}

/// A semantic video-filter operation; list order is the FFmpeg filter order.
#[derive(Clone, Debug, PartialEq, Rex)]
pub enum VideoFilter {
    Scale(ScaleFilter),
    Crop(CropFilter),
    Pad(PadFilter),
    FrameRate(Rational),
    PixelFormat(String),
    SetSampleAspectRatio(Rational),
    SetDisplayAspectRatio(Rational),
    Rotate(RotateFilter),
    Transpose(TransposeDirection),
    HorizontalFlip,
    VerticalFlip,
    Deinterlace,
    Denoise(f64),
    BoxBlur(f64),
    GaussianBlur(f64),
    Sharpen(f64),
    Equalizer(VideoEqualizer),
    Hue(f64, f64),
    Gamma(f64),
    ChromaKey(Color, f64, f64),
    VideoFade(VideoFade),
    Overlay(OverlayFilter),
    DrawText(DrawTextFilter),
    BurnSubtitles(SubtitleFilter),
    ThumbnailFrames(u64),
    SelectFrames(String),
    SetPresentationTimestamps(String),
    ReverseVideo,
    LoopVideo(u64, u64),
    CustomVideoFilter(String, Vec<FilterOption>),
}

/// EBU R128 loudness-normalization targets in LUFS/LU and dBTP.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct LoudnessNormalization {
    pub integrated_loudness: f64,
    pub loudness_range: f64,
    pub true_peak: f64,
}

/// An audio fade with direction, start time, and duration.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct AudioFade {
    pub direction: FadeDirection,
    pub start: Time,
    pub duration: Time,
}

/// A semantic audio-filter operation; list order is the FFmpeg filter order.
#[derive(Clone, Debug, PartialEq, Rex)]
pub enum AudioFilter {
    Volume(f64),
    NormalizeLoudness(LoudnessNormalization),
    Resample(u64),
    ChannelLayout(String),
    AudioFade(AudioFade),
    Delay(Vec<Time>),
    Tempo(f64),
    Equalizer(f64, f64, f64),
    HighPass(f64),
    LowPass(f64),
    Compressor(f64, f64, f64, f64),
    Limiter(f64),
    Gate(f64, f64),
    Echo(f64, f64, Time, f64),
    RemoveSilence(f64, Time),
    ReverseAudio,
    CustomAudioFilter(String, Vec<FilterOption>),
}

/// A filter usable in a general graph, including stream-specific and multi-stream operations.
#[derive(Clone, Debug, PartialEq, Rex)]
pub enum MediaFilter {
    Video(VideoFilter),
    Audio(AudioFilter),
    TrimVideo(TimeRange),
    TrimAudio(TimeRange),
    ConcatFilter(u64, u64, u64),
    SplitVideo(u64),
    SplitAudio(u64),
    MixAudio(u64, bool),
    MergeAudio(u64),
    CrossFadeVideo(Time, Time),
    CrossFadeAudio(Time),
    CustomFilter(String, Vec<FilterOption>),
}

/// One ordered filter chain with input pads and named output pads.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct FilterChain {
    pub inputs: Vec<FilterPad>,
    pub filters: Vec<MediaFilter>,
    pub outputs: Vec<String>,
}

/// A complex FFmpeg filter graph made of ordered chains.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct FilterGraph {
    pub chains: Vec<FilterChain>,
}

/// A video encoder family; `OtherVideoCodec` accepts an installed FFmpeg encoder name.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum VideoCodec {
    H264,
    H265,
    Av1,
    Vp8,
    Vp9,
    ProRes,
    DnxHd,
    Mpeg2Video,
    Mpeg4Video,
    Theora,
    GifVideo,
    PngVideo,
    MjpegVideo,
    RawVideo,
    OtherVideoCodec(String),
}

/// An audio encoder family; `OtherAudioCodec` accepts an installed FFmpeg encoder name.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum AudioCodec {
    Aac,
    Opus,
    Vorbis,
    Mp3,
    Flac,
    Alac,
    Ac3,
    Eac3,
    PcmS16Le,
    PcmS24Le,
    PcmF32Le,
    OtherAudioCodec(String),
}

/// A subtitle encoder or stream-copy choice.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum SubtitleCodec {
    WebVtt,
    SubRip,
    Ass,
    MovText,
    DvdSubtitle,
    CopySubtitle,
    OtherSubtitleCodec(String),
}

/// A video-encoder option scoped to its output video stream.
#[derive(Clone, Debug, PartialEq, Rex)]
pub enum VideoEncodeOption {
    VideoBitRate(u64),
    ConstantRateFactor(f64),
    VideoQuality(f64),
    Preset(String),
    Tune(String),
    Profile(String),
    Level(String),
    PixelFormat(String),
    VideoFrameRate(Rational),
    GroupOfPictures(u64),
    BFrames(u64),
    MaximumBitRate(u64),
    BufferSize(u64),
    EncoderThreads(u64),
    VideoBitstreamFilter(String, Vec<FilterOption>),
    VideoEncoderOption(NameValue),
}

/// A video codec and the ordered options used to configure its encoder.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct VideoEncoding {
    pub codec: VideoCodec,
    pub options: Vec<VideoEncodeOption>,
}

/// An audio-encoder option scoped to its output audio stream.
#[derive(Clone, Debug, PartialEq, Rex)]
pub enum AudioEncodeOption {
    AudioBitRate(u64),
    AudioQuality(f64),
    AudioSampleRate(u64),
    AudioChannels(u64),
    AudioChannelLayout(String),
    AudioCompressionLevel(f64),
    AudioCutoff(f64),
    AudioBitstreamFilter(String, Vec<FilterOption>),
    AudioEncoderOption(NameValue),
}

/// An audio codec and the ordered options used to configure its encoder.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct AudioEncoding {
    pub codec: AudioCodec,
    pub options: Vec<AudioEncodeOption>,
}

/// A subtitle codec and codec-specific named options.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct SubtitleEncoding {
    pub codec: SubtitleCodec,
    pub options: Vec<NameValue>,
}

/// Copy or encode one output stream in a general `MediaProgram`.
#[derive(Clone, Debug, PartialEq, Rex)]
pub enum StreamEncoding {
    CopyStream(MediaKind),
    EncodeVideo(VideoEncoding),
    EncodeAudio(AudioEncoding),
    EncodeSubtitle(SubtitleEncoding),
    EncodeData(String, Vec<NameValue>),
}

/// An FFmpeg stream disposition flag applied to an output stream.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum StreamDisposition {
    DefaultDisposition,
    DubDisposition,
    OriginalDisposition,
    CommentaryDisposition,
    LyricsDisposition,
    KaraokeDisposition,
    ForcedDisposition,
    HearingImpairedDisposition,
    VisualImpairedDisposition,
    CleanEffectsDisposition,
    AttachedPictureDisposition,
    TimedThumbnailsDisposition,
    OtherDisposition(String),
}

/// One mapped output stream with encoding, metadata, and disposition choices.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct OutputStream {
    pub source: StreamSource,
    pub encoding: StreamEncoding,
    pub metadata: BTreeMap<String, String>,
    pub dispositions: Vec<StreamDisposition>,
}

/// An output-scoped muxer or timing option.
#[derive(Clone, Debug, PartialEq, Rex)]
pub enum MuxOption {
    ShortestOutput,
    OutputDuration(Time),
    OutputStartAt(Time),
    CopyInputTimestamps,
    AvoidNegativeTimestamps(String),
    MaximumMuxingQueueSize(u64),
    MovFlags(Vec<String>),
    MapMetadataFrom(Option<u64>),
    MapChaptersFrom(Option<u64>),
    MuxerOption(NameValue),
}

/// HLS muxer settings; durations are targets and actual cuts normally follow keyframes.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct HlsOutput {
    pub segment_duration: Time,
    pub playlist_size: u64,
    pub segment_format: String,
    pub flags: Vec<String>,
    pub master_playlist: bool,
}

/// DASH muxer settings for segment duration, live windows, templates, and timelines.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct DashOutput {
    pub segment_duration: Time,
    pub window_size: u64,
    pub extra_window_size: u64,
    pub use_template: bool,
    pub use_timeline: bool,
}

/// Segment-muxer settings for target duration, timestamp reset, and first sequence number.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct SegmentOutput {
    pub segment_duration: Time,
    pub reset_timestamps: bool,
    pub start_number: u64,
}

/// The physical shape of an output: one file, a sequence, or a streaming package.
#[derive(Clone, Debug, PartialEq, Rex)]
pub enum OutputMode {
    SingleFile,
    NumberedFiles(u64),
    SegmentedFiles(SegmentOutput),
    HlsStreaming(HlsOutput),
    DashStreaming(DashOutput),
}

/// One output of a general media program, including mappings, encoding, and metadata.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct MediaOutput {
    pub format: ContainerFormat,
    pub mode: OutputMode,
    pub streams: Vec<OutputStream>,
    pub options: Vec<MuxOption>,
    pub metadata: BTreeMap<String, String>,
}

/// A complete typed FFmpeg invocation with inputs, a filter graph, and outputs.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct MediaProgram {
    pub inputs: Vec<MediaInput>,
    pub filters: FilterGraph,
    pub outputs: Vec<MediaOutput>,
}

/// A high-level operation for the single-source transcoding APIs.
#[derive(Clone, Debug, PartialEq, Rex)]
pub enum MediaOperation {
    Trim(TimeRange),
    VideoOperation(VideoFilter),
    AudioOperation(AudioFilter),
    DropVideo,
    DropAudio,
    DropSubtitles,
    SelectVideoStream(u64),
    SelectAudioStream(u64),
    SelectSubtitleStream(u64),
    SetOutputMetadata(String, String),
}

/// Container and optional video, audio, and subtitle encodings for a simple output.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct Encoding {
    pub format: ContainerFormat,
    pub video: Option<VideoEncoding>,
    pub audio: Option<AudioEncoding>,
    pub subtitle: Option<SubtitleEncoding>,
    pub options: Vec<MuxOption>,
    pub metadata: BTreeMap<String, String>,
}

/// Container and video encoder used when writing extracted still images.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct ImageEncoding {
    pub format: ContainerFormat,
    pub video: VideoEncoding,
}

/// A strategy for selecting decoded video frames.
#[derive(Clone, Debug, PartialEq, Rex)]
pub enum FrameSelection {
    EveryFrame,
    FramesPerSecond(f64),
    EveryNthFrame(u64),
    AtTimes(Vec<Time>),
    SceneChanges(f64),
    BestThumbnail(u64),
}

/// Selection and optional bounding-box resize for a single thumbnail.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct ThumbnailSpec {
    pub at: Option<Time>,
    pub size: Option<VideoSize>,
    pub preserve_aspect_ratio: bool,
}

/// Stream participation and optional normalization for concatenating media files.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct ConcatSpec {
    pub video: bool,
    pub audio: bool,
    pub normalize_video: Option<ScaleFilter>,
    pub normalize_video_frame_rate: Option<Rational>,
    pub normalize_video_pixel_format: Option<String>,
    pub normalize_audio_rate: Option<u64>,
    pub normalize_audio_channel_layout: Option<String>,
}

/// A zero-based input stream mapping for the `mux` convenience function.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct MuxMapping {
    pub input: u64,
    pub kind: MediaKind,
    pub stream_index: Option<u64>,
    pub copy: bool,
}

/// The FFprobe metadata sections included by `probe`.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum ProbeDetail {
    ProbeContainer,
    ProbeStreams,
    ProbeChapters,
    ProbePrograms,
    ProbeAll,
}

/// FFprobe metadata controls, including optional counting and interval restriction.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct ProbeOptions {
    pub detail: ProbeDetail,
    pub count_frames: bool,
    pub count_packets: bool,
    pub read_intervals: Option<String>,
}

/// Stable metadata returned by FFprobe for a media file.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct MediaInfo {
    pub format: Option<FormatInfo>,
    pub streams: Vec<StreamInfo>,
    pub chapters: Vec<ChapterInfo>,
    pub programs: Vec<ProgramInfo>,
}

/// FFprobe container metadata and tags.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct FormatInfo {
    pub format_name: String,
    pub format_long_name: String,
    pub start_time: Option<f64>,
    pub duration: Option<f64>,
    pub size: Option<u64>,
    pub bit_rate: Option<u64>,
    pub probe_score: Option<u64>,
    pub tags: BTreeMap<String, String>,
}

/// FFprobe metadata for one stream; absent fields do not apply or were not reported.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct StreamInfo {
    pub index: u64,
    pub kind: MediaKind,
    pub codec_name: String,
    pub codec_long_name: String,
    pub profile: Option<String>,
    pub codec_tag: Option<String>,
    pub width: Option<u64>,
    pub height: Option<u64>,
    pub pixel_format: Option<String>,
    pub sample_aspect_ratio: Option<String>,
    pub display_aspect_ratio: Option<String>,
    pub frame_rate: Option<String>,
    pub sample_rate: Option<u64>,
    pub channels: Option<u64>,
    pub channel_layout: Option<String>,
    pub bit_rate: Option<u64>,
    pub start_time: Option<f64>,
    pub duration: Option<f64>,
    pub frame_count: Option<u64>,
    pub packet_count: Option<u64>,
    pub language: Option<String>,
    pub disposition: BTreeMap<String, bool>,
    pub tags: BTreeMap<String, String>,
}

/// One FFprobe chapter with second-based boundaries and tags.
#[derive(Clone, Debug, PartialEq, Rex)]
pub struct ChapterInfo {
    pub id: u64,
    pub start: f64,
    pub end: f64,
    pub tags: BTreeMap<String, String>,
}

/// One FFprobe program and the indices of its member streams.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct ProgramInfo {
    pub id: u64,
    pub stream_indices: Vec<u64>,
    pub tags: BTreeMap<String, String>,
}

/// Whether `inspect` returns decoded frames/subtitles or demuxed packets.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum InspectionKind {
    InspectPackets,
    InspectFrames,
}

/// A selective FFprobe frame/packet query.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct InspectionQuery {
    pub kind: InspectionKind,
    pub stream: Option<StreamRef>,
    pub read_intervals: Option<String>,
    pub entries: Vec<String>,
}

/// One inspected frame or packet represented by requested FFprobe fields.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct InspectionRecord {
    pub fields: BTreeMap<String, String>,
}

/// Installed FFmpeg version, build configuration, and linked library versions.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct VersionInfo {
    pub version: String,
    pub configuration: String,
    pub libraries: BTreeMap<String, String>,
}

/// A category accepted by the FFmpeg capability-listing function.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum CapabilityDomain {
    Encoders,
    Decoders,
    Codecs,
    Demuxers,
    Muxers,
    Formats,
    Filters,
    Protocols,
    Devices,
    PixelFormats,
    SampleFormats,
    ChannelLayouts,
    HardwareAccelerators,
}

/// One capability row as reported by the installed FFmpeg build.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct Capability {
    pub flags: String,
    pub name: String,
    pub description: String,
}

/// The class of an expected semantic FFmpeg failure.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub enum FfmpegErrorKind {
    InvalidRequest,
    ProcessFailed,
    UnexpectedOutput,
}

/// An expected invalid request, process failure, or unrecognized FFmpeg result.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct FfmpegError {
    pub kind: FfmpegErrorKind,
    pub exit_code: Option<i64>,
    pub message: String,
}
